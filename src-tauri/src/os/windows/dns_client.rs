//! 系统 DNS：网卡枚举 + 分族设/还原 + 缓存清理（设计方案 §4）
//!
//! 枚举走 netioapi FFI（`GetAdaptersAddresses`）：PowerShell 方案 B 每次要
//! spawn powershell.exe 并加载 NetAdapter 模块，实测约 3 秒/次，而 DNS 状态
//! 读取遍布全应用（DNS 页、状态聚合、写入前白名单校验），是界面"点了没反应"
//! 的最大单一来源。
//!
//! 写入/还原走 **netsh 分族命令**（ipv4/ipv6 各自独立）：
//!   - `Set-DnsClientServerAddress` 没有 `-AddressFamily` 参数（2026-10-07 实测
//!     才发现，`-ResetServerAddresses` 也只能两族一起还原），分族语义无法用它表达；
//!   - netsh 写的就是回读校验所读的同一对注册表值
//!     （Tcpip{,6}\Parameters\Interfaces\{GUID}\NameServer），verify 逻辑不变。
//!
//! 「是否 DHCP」的判定不看任何文案（会被本地化），而是读注册表
//! `Tcpip\Parameters\Interfaces\{GUID}\NameServer`（v4）与 `Tcpip6\...`（v6）：
//! 值存在且非空 = 手动配置，否则 = 自动获取。这与 Windows 自身的语义一致；
//! 此时的 servers 取自 GetAdaptersAddresses 的 DNS 服务器列表（即 DHCP 实际下发值）。

use crate::error::{AppError, Result, E4001, E4002, E4003};
use crate::os::winapi::{
    self, DnsFlushResolverCache, IpAdapterAddresses, IpAdapterDnsServerAddress,
    IpAdapterUnicastAddress, SocketAddress,
};
use serde::{Deserialize, Serialize};
use std::os::windows::process::CommandExt;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
use winreg::RegKey;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AddressFamily {
    V4,
    V6,
}

impl AddressFamily {
    pub fn label(&self) -> &'static str {
        match self {
            AddressFamily::V4 => "IPv4",
            AddressFamily::V6 => "IPv6",
        }
    }

    fn reg_service(&self) -> &'static str {
        match self {
            AddressFamily::V4 => "Tcpip",
            AddressFamily::V6 => "Tcpip6",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct DnsFamilyState {
    /// true = 自动获取（DHCP），此时 servers 为 DHCP 下发的地址（仅供展示）
    pub is_dhcp: bool,
    pub servers: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NetInterface {
    pub alias: String,
    pub description: String,
    pub guid: String,
    pub status: String,
    pub is_physical: bool,
    /// GAA 的 IPv4 接口索引：与 GetBestInterface 的返回值对齐
    pub if_index: u32,
    /// 是否有默认网关 —— "当前正在使用"的判定依据（有网关 = 流量真的走它）
    pub has_default_gateway: bool,
    /// 是否有可用的全局 IPv6 地址（§12 风险 9a：无 IPv6 通路时写 v6 DNS 只会变慢）
    pub has_v6_global: bool,
    pub v4: DnsFamilyState,
    pub v6: DnsFamilyState,
}

/// 路由表探测目标：223.5.5.5（公网地址即可，GetBestInterface 只查本地路由表，不发包）
const ROUTE_PROBE_TARGET: u32 = u32::from_be_bytes([223, 5, 5, 5]);

/// 承载默认流量的网卡索引（查不到路由 = 离线/无默认路由 → None）
fn best_interface_index() -> Option<u32> {
    let mut idx: u32 = 0;
    let r = unsafe { winapi::GetBestInterface(ROUTE_PROBE_TARGET, &mut idx) };
    if r == 0 {
        Some(idx)
    } else {
        None
    }
}

/// 从枚举结果里挑"当前正在使用"的网卡：
/// ① 路由表权威判定（默认流量走哪块就是哪块，天然排除虚拟网卡的干扰）；
/// ② 路由查不到（离线）时退回启发式：物理+已连接+有网关 > 物理+已连接 > 已连接 > 第一个。
pub fn pick_in_use(list: &[NetInterface]) -> Option<NetInterface> {
    let up = |i: &NetInterface| i.status.eq_ignore_ascii_case("Up");
    if let Some(idx) = best_interface_index() {
        if let Some(i) = list.iter().find(|i| i.if_index == idx) {
            return Some(i.clone());
        }
    }
    list.iter()
        .find(|i| i.is_physical && up(i) && i.has_default_gateway)
        .or_else(|| list.iter().find(|i| i.is_physical && up(i)))
        .or_else(|| list.iter().find(|i| up(i)))
        .or_else(|| list.first())
        .cloned()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DnsState {
    pub interface_alias: String,
    pub interface_guid: String,
    pub v4: DnsFamilyState,
    pub v6: DnsFamilyState,
}

const GAA_FLAGS: u32 =
    winapi::GAA_FLAG_SKIP_ANYCAST | winapi::GAA_FLAG_SKIP_MULTICAST | winapi::GAA_FLAG_INCLUDE_GATEWAYS;
/// 官方建议的初始工作缓冲（不足时按返回值扩一倍重试）
const GAA_BUF_INIT: usize = 15 * 1024;

fn read_manual_dns(guid: &str, family: AddressFamily) -> Option<Vec<String>> {
    let svc = family.reg_service();
    let path = format!(r"SYSTEM\CurrentControlSet\Services\{svc}\Parameters\Interfaces\{guid}");
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = hklm.open_subkey_with_flags(path, KEY_READ).ok()?;
    let ns: String = key.get_value("NameServer").ok()?;
    let list: Vec<String> = ns
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if list.is_empty() {
        None
    } else {
        Some(list)
    }
}

/// 从 sockaddr 原始字节解析 IP（AF_INET：+4 起 4 字节；AF_INET6：+8 起 16 字节，
/// 均为网络字节序）。其余地址族返回 None。
unsafe fn sockaddr_to_ip(sa: *const u8) -> Option<std::net::IpAddr> {
    if sa.is_null() {
        return None;
    }
    let family = (sa as *const u16).read_unaligned();
    match family {
        winapi::AF_INET => {
            let octets = std::slice::from_raw_parts(sa.add(4), 4);
            Some(std::net::IpAddr::V4(std::net::Ipv4Addr::new(
                octets[0], octets[1], octets[2], octets[3],
            )))
        }
        winapi::AF_INET6 => {
            let octets = std::slice::from_raw_parts(sa.add(8), 16);
            Some(std::net::IpAddr::V6(std::net::Ipv6Addr::from(
                <[u8; 16]>::try_from(octets).ok()?,
            )))
        }
        _ => None,
    }
}

/// 与原 PowerShell 判定等价的全局 IPv6 条件：
/// DadState=Preferred 且 PrefixOrigin≠WellKnown，排除 fe80::/10 链路本地与 fc00::/7 ULA
fn is_v6_global(prefix_origin: u32, dad_state: u32, addr: &std::net::Ipv6Addr) -> bool {
    dad_state == winapi::IP_DAD_STATE_PREFERRED
        && prefix_origin != winapi::IP_PREFIX_ORIGIN_WELLKNOWN
        && !addr.is_unicast_link_local()
        && !addr.is_unique_local()
}

/// ANSI（CP_ACP）→ String：GAA 的 Description 只有窄字符版本
unsafe fn ansi_to_string(p: *const u8) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0isize;
    while *p.offset(len) != 0 {
        len += 1;
    }
    if len == 0 {
        return String::new();
    }
    // 先算需要的宽字符数，再转换（两段式调用，避免固定缓冲截断）
    let n = winapi::MultiByteToWideChar(winapi::CP_ACP, 0, p, len as i32, std::ptr::null_mut(), 0);
    if n <= 0 {
        return String::new();
    }
    let mut wide = vec![0u16; n as usize];
    let written = winapi::MultiByteToWideChar(winapi::CP_ACP, 0, p, len as i32, wide.as_mut_ptr(), n);
    if written <= 0 {
        return String::new();
    }
    wide.truncate(written as usize);
    String::from_utf16_lossy(&wide)
}

/// 遍历 GAA 返回的链表（Next 单链结构）
unsafe trait Linked {
    fn next_ptr(&mut self) -> *mut Self;
}

unsafe fn chain<T: Linked>(mut p: *mut T) -> impl Iterator<Item = *mut T> {
    std::iter::from_fn(move || {
        if p.is_null() {
            None
        } else {
            let cur = p;
            p = (*p).next_ptr();
            Some(cur)
        }
    })
}

unsafe impl Linked for IpAdapterUnicastAddress {
    fn next_ptr(&mut self) -> *mut Self {
        self.next
    }
}

unsafe impl Linked for IpAdapterDnsServerAddress {
    fn next_ptr(&mut self) -> *mut Self {
        self.next
    }
}

impl SocketAddress {
    unsafe fn ip(&self) -> Option<std::net::IpAddr> {
        sockaddr_to_ip(self.sockaddr)
    }
}

/// 网卡枚举缓存（与 macOS 侧对齐）：UI 会在极短时间内连续调用
/// `dns_interfaces` / `dns_get` / `dns_v6_ready`，每次都要重走 GAA 并逐个网卡读注册表。
/// 写入（set / reset）后立即失效，保证写后回读拿到新值。
const CACHE_TTL: Duration = Duration::from_secs(2);

type CacheCell = Mutex<Option<(Instant, Vec<NetInterface>)>>;

fn cache() -> &'static CacheCell {
    static CACHE: OnceLock<CacheCell> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// 使网卡枚举缓存失效（DNS 写入后必须调用）
pub fn invalidate_cache() {
    if let Ok(mut c) = cache().lock() {
        *c = None;
    }
}

fn cache_get() -> Option<Vec<NetInterface>> {
    let c = cache().lock().ok()?;
    let (at, list) = c.as_ref()?;
    if at.elapsed() <= CACHE_TTL {
        Some(list.clone())
    } else {
        None
    }
}

fn cache_put(list: &[NetInterface]) {
    if let Ok(mut c) = cache().lock() {
        *c = Some((Instant::now(), list.to_vec()));
    }
}

/// 枚举网卡（含 DNS 状态），带短 TTL 缓存。纯内存读取，毫秒级；不受 PowerShell 冷启动影响。
pub fn list_interfaces() -> Result<Vec<NetInterface>> {
    if let Some(list) = cache_get() {
        return Ok(list);
    }
    let list = enumerate_interfaces()?;
    cache_put(&list);
    Ok(list)
}

fn enumerate_interfaces() -> Result<Vec<NetInterface>> {
    let adapters = unsafe { gaa_adapters() }?;

    let mut out = Vec::new();
    unsafe {
        let mut a: *mut IpAdapterAddresses = adapters.head;
        while !a.is_null() {
            let ad = &*a;
            let alias = winapi::from_wide_ptr(ad.friendly_name);
            if alias.is_empty() {
                a = ad.next;
                continue;
            }
            let guid = ansi_to_string(ad.adapter_name);
            let description = ansi_to_string(ad.description);

            // DNS 服务器列表：AF_UNSPEC 下 v4 / v6 混在一条链表里，按 sockaddr 族分桶
            let mut d4: Vec<String> = Vec::new();
            let mut d6: Vec<String> = Vec::new();
            for dns in chain(ad.first_dns_server) {
                match (*dns).address.ip() {
                    Some(ip) if ip.is_ipv4() => d4.push(ip.to_string()),
                    Some(ip) => d6.push(ip.to_string()),
                    None => {}
                }
            }

            // 全局 IPv6 通路判定（§12 风险 9a）
            let has_v6_global = chain(ad.first_unicast).any(|u| {
                let ip = (*u).address.ip();
                match ip {
                    Some(std::net::IpAddr::V6(v6)) => {
                        is_v6_global((*u).prefix_origin, (*u).dad_state, &v6)
                    }
                    _ => false,
                }
            });

            let manual4 = read_manual_dns(&guid, AddressFamily::V4);
            let manual6 = read_manual_dns(&guid, AddressFamily::V6);
            let v4 = match manual4 {
                Some(s) => DnsFamilyState {
                    is_dhcp: false,
                    servers: s,
                },
                None => DnsFamilyState {
                    is_dhcp: true,
                    servers: d4,
                },
            };
            let v6 = match manual6 {
                Some(s) => DnsFamilyState {
                    is_dhcp: false,
                    servers: s,
                },
                None => DnsFamilyState {
                    is_dhcp: true,
                    servers: d6,
                },
            };

            out.push(NetInterface {
                status: oper_status_text(ad.oper_status),
                if_index: ad.if_index,
                // 近似 Get-NetAdapter -Physical：真实判定依赖 NDIS 物理介质信息，
                // GAA 拿不到；以太网 / Wi-Fi 两类足够覆盖 UI 的"虚拟"标注与偏好排序
                is_physical: matches!(
                    ad.if_type,
                    winapi::IF_TYPE_ETHERNET_CSMACD | winapi::IF_TYPE_IEEE80211
                ),
                has_default_gateway: !ad.first_gateway_address.is_null(),
                has_v6_global,
                alias,
                description,
                guid,
                v4,
                v6,
            });

            a = ad.next;
        }
    }

    if out.is_empty() {
        return Err(AppError::internal(
            "GetAdaptersAddresses 未返回任何网卡",
        ));
    }
    // 物理且已连接的排前面
    out.sort_by_key(|i| {
        let rank = if i.is_physical && i.status.eq_ignore_ascii_case("Up") {
            0
        } else if i.status.eq_ignore_ascii_case("Up") {
            1
        } else {
            2
        };
        (rank, i.alias.clone())
    });
    Ok(out)
}

/// 调用 GetAdaptersAddresses，返回首节点指针与工作缓冲（缓冲必须活得和指针一样久）
struct GaaBuffer {
    _buf: Vec<u64>,
    head: *mut IpAdapterAddresses,
}

unsafe fn gaa_adapters() -> Result<GaaBuffer> {
    let mut size = GAA_BUF_INIT as u32;
    for _ in 0..3 {
        let mut buf: Vec<u64> = vec![0; (size as usize + 7) / 8];
        let head = buf.as_mut_ptr() as *mut IpAdapterAddresses;
        let ret = winapi::GetAdaptersAddresses(
            winapi::AF_UNSPEC,
            GAA_FLAGS,
            std::ptr::null_mut(),
            head,
            &mut size,
        );
        match ret {
            0 => return Ok(GaaBuffer { _buf: buf, head }),
            winapi::ERROR_BUFFER_OVERFLOW => continue, // size 已被更新为所需值
            winapi::ERROR_NO_DATA => {
                return Err(AppError::internal("GetAdaptersAddresses 未返回任何网卡"));
            }
            other => {
                return Err(AppError::internal(format!(
                    "GetAdaptersAddresses 失败：Win32 错误码 {other}"
                )));
            }
        }
    }
    Err(AppError::internal("GetAdaptersAddresses 缓冲区重试超限"))
}

fn oper_status_text(v: u32) -> String {
    match v {
        winapi::IF_OPER_STATUS_UP => "Up".into(),
        2 => "Down".into(),
        3 => "Testing".into(),
        5 => "Dormant".into(),
        6 => "Not Present".into(),
        7 => "Lower Layer Down".into(),
        _ => "Unknown".into(),
    }
}

/// 校验别名：必须存在于系统网卡列表中（白名单，杜绝把任意字符串送进脚本）
pub fn resolve_alias(alias: &str) -> Result<NetInterface> {
    let list = list_interfaces()?;
    list.into_iter()
        .find(|i| i.alias.eq_ignore_ascii_case(alias))
        .ok_or_else(|| AppError::coded(E4001).with_detail(format!("未找到网卡「{alias}」")))
}

pub fn get(alias: &str) -> Result<DnsState> {
    let iface = resolve_alias(alias)?;
    Ok(DnsState {
        interface_alias: iface.alias,
        interface_guid: iface.guid,
        v4: iface.v4,
        v6: iface.v6,
    })
}

fn validate_servers(family: AddressFamily, servers: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for s in servers {
        let cleaned = s.trim();
        if cleaned.is_empty() {
            continue;
        }
        match cleaned.parse::<std::net::IpAddr>() {
            Ok(ip) => {
                let ok = matches!(family, AddressFamily::V4) == ip.is_ipv4();
                if !ok {
                    return Err(AppError::coded(E4002).with_detail(format!(
                        "{} 地址族不匹配：{cleaned}",
                        family.label()
                    )));
                }
                out.push(ip.to_string());
            }
            Err(_) => {
                return Err(AppError::coded(E4002).with_detail(format!("非法地址：{cleaned}")));
            }
        }
    }
    if out.len() > 4 {
        return Err(AppError::coded(E4002).with_detail("单个地址族最多 4 个 DNS"));
    }
    Ok(out)
}

/// 执行 netsh（argv 直传，不经 shell；别名来自枚举白名单、地址经 IpAddr 解析）
fn run_netsh(args: &[String]) -> Result<()> {
    let out = std::process::Command::new("netsh.exe")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| AppError::internal(format!("无法启动 netsh：{e}")))?;
    if out.status.code() != Some(0) {
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(AppError::internal(format!(
            "netsh {} 失败：{}",
            args.iter().take(4).cloned().collect::<Vec<_>>().join(" "),
            if stdout.is_empty() { stderr } else { stdout }
        )));
    }
    Ok(())
}

fn netsh_stack(family: AddressFamily) -> &'static str {
    match family {
        AddressFamily::V4 => "ipv4",
        AddressFamily::V6 => "ipv6",
    }
}

/// 构造分族 netsh 参数。单独拆出来是为了能单测命令形状（不含 netsh 语义）。
fn netsh_set_args(stack: &str, alias: &str, list: &[String]) -> Vec<Vec<String>> {
    let mut cmds = Vec::new();
    // 第一条 set：整表替换为静态 + 首地址
    cmds.push(vec![
        "interface".into(),
        stack.into(),
        "set".into(),
        "dnsservers".into(),
        format!("name={alias}"),
        "source=static".into(),
        format!("address={}", list[0]),
        "validate=no".into(),
    ]);
    // 其余地址按优先级追加
    for (i, addr) in list.iter().enumerate().skip(1) {
        cmds.push(vec![
            "interface".into(),
            stack.into(),
            "add".into(),
            "dnsservers".into(),
            format!("name={alias}"),
            format!("address={addr}"),
            format!("index={}", i + 1),
            "validate=no".into(),
        ]);
    }
    cmds
}

fn netsh_reset_args(stack: &str, alias: &str) -> Vec<String> {
    vec![
        "interface".into(),
        stack.into(),
        "set".into(),
        "dnsservers".into(),
        format!("name={alias}"),
        "source=dhcp".into(),
    ]
}

/// 设置某一族 DNS（servers 为空 → 该族还原为自动获取，§4.3 交互规则 2）
pub fn set(alias: &str, family: AddressFamily, servers: &[String]) -> Result<()> {
    let iface = resolve_alias(alias)?;
    let list = validate_servers(family, servers)?;
    if list.is_empty() {
        return reset(&iface.alias, Some(family));
    }
    if matches!(family, AddressFamily::V6) {
        if !iface.has_v6_global {
            log::warn!(
                target: "dns",
                "[E4004] 网卡 {} 未检测到可用 IPv6 全局地址，写入 IPv6 DNS 可能导致解析变慢",
                iface.alias
            );
        }
    }
    let stack = netsh_stack(family);
    for cmd in netsh_set_args(stack, &iface.alias, &list) {
        run_netsh(&cmd)?;
    }

    // 写后回读：不一致必须报失败（组策略刷回场景）
    let manual = read_manual_dns(&iface.guid, family);
    let ok = match manual {
        Some(actual) => {
            let a: Vec<String> = actual.iter().map(|s| s.to_lowercase()).collect();
            let b: Vec<String> = list.iter().map(|s| s.to_lowercase()).collect();
            a == b
        }
        None => false,
    };
    if !ok {
        return Err(AppError::coded(E4003).with_detail(format!(
            "{} 写入后回读不一致，可能被组策略限制",
            family.label()
        )));
    }
    log::info!(
        target: "dns",
        "DNS 变更：网卡={} 地址族={} 地址={} 回读=一致",
        iface.alias,
        family.label(),
        list.join(",")
    );
    invalidate_cache();
    Ok(())
}

/// 还原为自动获取（family = None → 两族全部还原）
pub fn reset(alias: &str, family: Option<AddressFamily>) -> Result<()> {
    let iface = resolve_alias(alias)?;
    let families: Vec<AddressFamily> = match family {
        Some(f) => vec![f],
        None => vec![AddressFamily::V4, AddressFamily::V6],
    };
    for f in &families {
        run_netsh(&netsh_reset_args(netsh_stack(*f), &iface.alias))?;
    }

    for f in &families {
        if read_manual_dns(&iface.guid, *f).is_some() {
            return Err(AppError::coded(E4003).with_detail(format!(
                "{} 还原后仍存在手动 DNS，可能被组策略限制",
                f.label()
            )));
        }
    }
    log::info!(
        target: "dns",
        "DNS 还原：网卡={} 地址族={}",
        iface.alias,
        family.map(|f| f.label().to_string()).unwrap_or_else(|| "全部".into())
    );
    invalidate_cache();
    Ok(())
}

/// 清空 DNS 客户端缓存（等价 ipconfig /flushdns，无需提权）
/// 刷新 DNS 缓存是否需要管理员权限。
///
/// Windows 走 `DnsFlushResolverCache`（Dnscache 服务 IPC），普通用户即可 → false。
/// （macOS 对应实现见 `os/macos/dns_client.rs`，那边需要 root，恒为 true。）
pub fn flush_needs_root() -> bool {
    false
}

pub fn flush() -> Result<()> {
    let ok = unsafe { DnsFlushResolverCache() };
    if ok == 0 {
        log::warn!(target: "dns", "DnsFlushResolverCache 返回失败（不中断流程）");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv6Addr;

    #[test]
    fn pick_in_use_prefers_gateway() {
        let mk = |alias: &str, physical: bool, status: &str, gw: bool| NetInterface {
            alias: alias.into(),
            description: String::new(),
            guid: String::new(),
            status: status.into(),
            is_physical: physical,
            if_index: 0,
            has_default_gateway: gw,
            has_v6_global: false,
            v4: DnsFamilyState::default(),
            v6: DnsFamilyState::default(),
        };
        let list = vec![
            mk("vEthernet", false, "Up", true),
            mk("蓝牙网络连接", true, "Down", false),
            mk("以太网", true, "Up", true),
            mk("Wi-Fi", true, "Up", false),
        ];
        // 物理 + Up + 网关优先
        assert_eq!(pick_in_use(&list).unwrap().alias, "以太网");
        // 没有带网关的 → 物理 + Up
        let no_gw: Vec<NetInterface> = list
            .iter()
            .filter(|i| i.alias != "以太网")
            .cloned()
            .collect();
        assert_eq!(pick_in_use(&no_gw).unwrap().alias, "Wi-Fi");
        // 空列表 → None
        assert!(pick_in_use(&[]).is_none());
    }

    #[test]
    fn netsh_command_shape() {
        // 首条 set + 后续 add index，别名含空格也必须是单个 name= 令牌
        let cmds = netsh_set_args("ipv4", "以太网 2", &["119.29.29.29".to_string(), "223.5.5.5".to_string()]);
        assert_eq!(cmds.len(), 2);
        assert!(cmds[0].contains(&"set".to_string()));
        assert!(cmds[0].contains(&"dnsservers".to_string()));
        assert!(cmds[0].contains(&"name=以太网 2".to_string()), "别名含空格必须是完整令牌：{:?}", cmds[0]);
        assert!(cmds[0].contains(&"address=119.29.29.29".to_string()));
        assert!(cmds[0].contains(&"validate=no".to_string()));
        assert!(cmds[1].contains(&"add".to_string()));
        assert!(cmds[1].contains(&"index=2".to_string()));

        // 单地址只产生一条 set
        assert_eq!(netsh_set_args("ipv6", "X", &["2402:4e00::".to_string()]).len(), 1);

        // 还原 = source=dhcp
        let reset = netsh_reset_args("ipv6", "X");
        assert!(reset.contains(&"source=dhcp".to_string()));
        assert!(reset.contains(&"name=X".to_string()));
    }

    #[test]
    fn v6_global_predicate_matches_ps_rule() {
        let parse = |s: &str| s.parse::<Ipv6Addr>().unwrap();
        let preferred = winapi::IP_DAD_STATE_PREFERRED;
        let tentative = 1;
        let manual = 1;
        let wellknown = winapi::IP_PREFIX_ORIGIN_WELLKNOWN;

        // 全局单播（2000::/3）+ Preferred + 非 WellKnown → 通过
        assert!(is_v6_global(manual, preferred, &parse("2408:8756::1234")));
        assert!(is_v6_global(3, preferred, &parse("2001:db8::1"))); // DHCP 下发也算
        // 链路本地 fe80::/10 排除
        assert!(!is_v6_global(manual, preferred, &parse("fe80::1")));
        // ULA fc00::/7（fc/fd 开头）排除
        assert!(!is_v6_global(manual, preferred, &parse("fd12:3456::1")));
        assert!(!is_v6_global(manual, preferred, &parse("fc00::1")));
        // 未达 Preferred（DAD 进行中 / 重复检测）排除
        assert!(!is_v6_global(manual, tentative, &parse("2408:8756::1234")));
        // WellKnown 前缀排除（与原 PowerShell 规则一致）
        assert!(!is_v6_global(wellknown, preferred, &parse("2408:8756::1234")));
    }
}

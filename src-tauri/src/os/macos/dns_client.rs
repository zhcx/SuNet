//! DNS 与网络服务读写（macOS）
//!
//! Windows 的 DNS 是「每张网卡 + 每协议族两份」，macOS 是「每个网络服务 + 一份列表」：
//!   - 网卡概念 → **网络服务**（Wi-Fi / Built-in Ethernet / Thunderbolt Bridge …）
//!   - `networksetup -getdnsservers <服务>` 返回该服务的手动 DNS 列表；
//!     输出 "There aren't any DNS Servers set on …" 表示走 DHCP/自动获取
//!   - **没有按协议族分开的 API**：`-setdnsservers` 一次写入整份列表
//!     ⇒ `set(alias, V4, list)` 会把「另一族当前的值」合并保留，
//!        先 V4 后 V6 两次调用组合出的结果与 Windows 的逐族写入等价
//!   - 服务名可能含空格（"Built-in Ethernet"）→ 必须整段作为单个 argv（`super::net` 已保证不过 shell）
//!   - 改配置需要管理员权限；读配置不需要

use crate::error::{AppError, Result, E4001};
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::net;

/// 网卡枚举的缓存时长：UI 轮询会反复调用，networksetup 每次要起若干进程
const CACHE_TTL: Duration = Duration::from_secs(2);

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

    /// 地址属于本族？（macOS 只有一份列表，需要按形状分流）
    fn matches(&self, addr: &str) -> bool {
        match self {
            AddressFamily::V4 => !addr.contains(':'),
            AddressFamily::V6 => addr.contains(':'),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct DnsFamilyState {
    /// true = 自动获取（DHCP）；macOS 无法按族区分，两族同时为 true
    pub is_dhcp: bool,
    pub servers: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct NetInterface {
    /// 网络服务名（`networksetup` 的 `<service>`，写入时用它）
    pub alias: String,
    /// 硬件端口名（"Wi-Fi" / "Thunderbolt Ethernet"）
    pub description: String,
    /// BSD 设备名（en0）——稳定标识，快照与日志用它
    pub guid: String,
    /// "Up" / "Down" / "Disabled"
    pub status: String,
    pub is_physical: bool,
    pub if_index: u32,
    /// 服务顺序最前且未停用的服务 = macOS 的「主服务」（决定默认路由）
    pub has_default_gateway: bool,
    pub has_v6_global: bool,
    pub v4: DnsFamilyState,
    pub v6: DnsFamilyState,
}

impl NetInterface {
    pub fn is_up(&self) -> bool {
        self.status == "Up"
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct DnsState {
    pub interface_alias: String,
    pub interface_guid: String,
    pub v4: DnsFamilyState,
    pub v6: DnsFamilyState,
}

// ---------------------------------------------------------------------------
// 缓存
// ---------------------------------------------------------------------------

type CacheCell = Mutex<Option<(Instant, Vec<NetInterface>)>>;

fn cache() -> &'static CacheCell {
    static CACHE: OnceLock<CacheCell> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

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

// ---------------------------------------------------------------------------
// 枚举
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct Service {
    name: String,
    disabled: bool,
    hardware_port: String,
    device: String,
}

#[derive(Debug, Clone, Copy, Default)]
struct IfInfo {
    up: bool,
    v6_global: bool,
}

/// 可枚举的「在用网卡」：物理 + 已连接 + 有默认网关优先（与 Windows 侧的取舍一致）
pub fn pick_in_use(list: &[NetInterface]) -> Option<NetInterface> {
    fn score(i: &NetInterface) -> u8 {
        (i.is_physical as u8) * 4 + (i.is_up() as u8) * 2 + (i.has_default_gateway as u8)
    }
    let mut viable: Vec<&NetInterface> = list.iter().filter(|i| i.status != "Disabled").collect();
    if viable.is_empty() {
        viable = list.iter().collect();
    }
    // rev + max_by_key：并列时取「服务顺序更靠前」的那个（服务顺序即优先级）
    viable
        .iter()
        .rev()
        .max_by_key(|i| score(i))
        .map(|i| (*i).clone())
}

pub fn list_interfaces() -> Result<Vec<NetInterface>> {
    if let Some(list) = cache_get() {
        return Ok(list);
    }
    let list = enumerate()?;
    cache_put(&list);
    Ok(list)
}

fn enumerate() -> Result<Vec<NetInterface>> {
    if !net::available(net::NETWORKSETUP) {
        return Err(AppError::coded(E4001).with_detail("找不到 /usr/sbin/networksetup"));
    }
    let order = net::networksetup(&["-listnetworkserviceorder".to_string()])?;
    let services = parse_service_order(&order);
    let physical_devices = net::networksetup(&["-listallhardwareports".to_string()])
        .map(|t| parse_hardware_ports(&t))
        .unwrap_or_default();

    let mut out = Vec::with_capacity(services.len());
    let mut primary_taken = false;
    for s in &services {
        if s.name.is_empty() {
            continue;
        }
        let info = ifconfig_info(&s.device).unwrap_or_default();
        let physical = !s.device.is_empty()
            && physical_devices.iter().any(|d| d == &s.device)
            && !is_virtual_device(&s.device);
        let status = if s.disabled {
            "Disabled"
        } else if info.up {
            "Up"
        } else {
            "Down"
        };
        // 服务顺序里的第一个未停用服务 = 主服务
        let has_default_gateway = if !s.disabled && !primary_taken {
            primary_taken = true;
            true
        } else {
            false
        };
        let (is_dhcp, servers) = read_dns(&s.name);
        let (v4, v6) = split_by_family(&servers, is_dhcp);
        out.push(NetInterface {
            alias: s.name.clone(),
            description: if s.hardware_port.is_empty() {
                s.device.clone()
            } else {
                s.hardware_port.clone()
            },
            guid: s.device.clone(),
            status: status.to_string(),
            is_physical: physical,
            if_index: if_index_of(&s.device),
            has_default_gateway,
            has_v6_global: info.v6_global,
            v4,
            v6,
        });
    }
    Ok(out)
}

/// `networksetup -listnetworkserviceorder` 解析
///
/// ```text
/// An asterisk (*) denotes that a network service is disabled.
/// (1) Wi-Fi
/// (Hardware Port: Wi-Fi, Device: en0)
///
/// (2) *iPhone USB
/// (Hardware Port: iPhone USB, Device: en6)
/// ```
fn parse_service_order(text: &str) -> Vec<Service> {
    let mut out: Vec<Service> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        let Some(rest) = l.strip_prefix('(') else {
            continue;
        };
        // 括号里的内容是「行首的一段」，不是整行：服务行 `(1) Wi-Fi` 后面还有名字，
        // 硬件端口行 `(Hardware Port: Wi-Fi, Device: en0)` 才是以 `)` 收尾。
        let Some(close) = rest.find(')') else {
            continue;
        };
        let inner = &rest[..close];
        let name_part = rest[close + 1..].trim();
        if let Some(hp) = inner.strip_prefix("Hardware Port:") {
            if let Some((port, dev)) = hp.split_once(", Device:") {
                if let Some(last) = out.last_mut() {
                    last.hardware_port = port.trim().to_string();
                    last.device = dev.trim().to_string();
                }
            }
            continue;
        }
        let num = inner.trim();
        if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) || name_part.is_empty() {
            continue;
        }
        let disabled = name_part.starts_with('*');
        out.push(Service {
            name: name_part.trim_start_matches('*').trim().to_string(),
            disabled,
            hardware_port: String::new(),
            device: String::new(),
        });
    }
    out
}

/// `networksetup -listallhardwareports` 里的 Device 行
fn parse_hardware_ports(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("Device:")
                .map(|d| d.trim().to_string())
        })
        .filter(|d| !d.is_empty())
        .collect()
}

/// 虚拟接口前缀（不是"真实网卡"，不该被选为落点）
fn is_virtual_device(dev: &str) -> bool {
    const VIRTUAL: &[&str] = &[
        "utun", "ipsec", "ppp", "awdl", "llw", "gif", "stf", "anpi", "ap", "tap", "tun", "lo",
    ];
    VIRTUAL.iter().any(|p| dev.starts_with(p))
}

fn if_index_of(device: &str) -> u32 {
    if device.is_empty() {
        return 0;
    }
    let c = match std::ffi::CString::new(device) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    unsafe { libc::if_nametoindex(c.as_ptr()) }
}

/// `ifconfig <dev>`：链路状态与是否有全局 IPv6 地址
fn ifconfig_info(device: &str) -> Option<IfInfo> {
    if device.is_empty() {
        return None;
    }
    let (ok, stdout, _) = net::try_run(net::IFCONFIG, &[device.to_string()]).ok()?;
    if !ok {
        return None;
    }
    let mut info = IfInfo::default();
    let prefix = format!("{device}:");
    for line in stdout.lines() {
        let l = line.trim();
        if l.starts_with(&prefix) {
            // flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST>
            info.up = l.contains("<UP") || l.contains(",UP,") || l.contains(",RUNNING");
        } else if let Some(rest) = l.strip_prefix("status:") {
            // status 比 flags 更准确地反映"能不能用"
            info.up = rest.trim().starts_with("active");
        } else if let Some(rest) = l.strip_prefix("inet6 ") {
            let raw = rest.split_whitespace().next().unwrap_or("");
            let addr = raw.split('%').next().unwrap_or(raw);
            if !addr.starts_with("fe80") && addr != "::1" && !addr.is_empty() {
                info.v6_global = true;
            }
        }
    }
    Some(info)
}

// ---------------------------------------------------------------------------
// 读写 DNS
// ---------------------------------------------------------------------------

/// 读该服务的 DNS 列表；返回 (是否自动获取, 服务器列表)
fn read_dns(service: &str) -> (bool, Vec<String>) {
    let args = vec!["-getdnsservers".to_string(), service.to_string()];
    match net::networksetup_soft(&args) {
        Ok((_, stdout)) => {
            if stdout.contains("aren't any DNS Servers") {
                return (true, Vec::new());
            }
            let servers: Vec<String> = stdout
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && !l.starts_with("There aren"))
                .collect();
            (servers.is_empty(), servers)
        }
        Err(e) => {
            log::debug!(target: "dns", "读取 {service} 的 DNS 失败：{e}");
            (true, Vec::new())
        }
    }
}

fn split_by_family(servers: &[String], is_dhcp: bool) -> (DnsFamilyState, DnsFamilyState) {
    let v4 = servers
        .iter()
        .filter(|s| !s.contains(':'))
        .cloned()
        .collect::<Vec<_>>();
    let v6 = servers
        .iter()
        .filter(|s| s.contains(':'))
        .cloned()
        .collect::<Vec<_>>();
    (
        DnsFamilyState {
            is_dhcp,
            servers: v4,
        },
        DnsFamilyState {
            is_dhcp,
            servers: v6,
        },
    )
}

fn write_dns(service: &str, list: &[String]) -> Result<()> {
    let mut args = vec!["-setdnsservers".to_string(), service.to_string()];
    if list.is_empty() {
        // "empty" 是 networksetup 的专用词：清空手动 DNS，回到 DHCP 下发
        args.push("empty".to_string());
    } else {
        args.extend(list.iter().cloned());
    }
    net::networksetup(&args)
        .map_err(|e| AppError::coded(E4001).with_detail(format!("设置 DNS 失败（{service}）：{e}")))?;
    Ok(())
}

pub fn resolve_alias(alias: &str) -> Result<NetInterface> {
    let want = alias.trim();
    let list = list_interfaces()?;
    let found = list
        .iter()
        .find(|i| i.alias == want)
        .or_else(|| list.iter().find(|i| i.alias.eq_ignore_ascii_case(want)))
        .or_else(|| {
            list.iter()
                .find(|i| i.guid.eq_ignore_ascii_case(want) && !want.is_empty())
        })
        .or_else(|| {
            list.iter()
                .find(|i| i.description.eq_ignore_ascii_case(want) && !want.is_empty())
        })
        .cloned();
    found.ok_or_else(|| {
        AppError::coded(E4001).with_detail(format!(
            "找不到网络服务「{alias}」（可用：{}）",
            list.iter()
                .map(|i| i.alias.as_str())
                .collect::<Vec<_>>()
                .join(" / ")
        ))
    })
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

/// 写入某一协议族的 DNS（另一族保持原样；空列表 = 该族回到自动获取）
pub fn set(alias: &str, family: AddressFamily, servers: &[String]) -> Result<()> {
    let iface = resolve_alias(alias)?;
    let (_, current) = read_dns(&iface.alias);
    let mut next: Vec<String> = servers
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && family.matches(s))
        .collect();
    let keep: Vec<String> = current
        .iter()
        .filter(|s| !family.matches(s))
        .cloned()
        .collect();
    next.extend(keep);
    dedup_keep_order(&mut next);

    write_dns(&iface.alias, &next)?;
    invalidate_cache();

    // 回读校验：与 §3.3 的「写完必回读」一致，不一致宁可报错也不假装成功
    let (_, after) = read_dns(&iface.alias);
    let expect: Vec<String> = next.iter().filter(|s| family.matches(s)).cloned().collect();
    let actual: Vec<String> = after
        .iter()
        .filter(|s| family.matches(s))
        .cloned()
        .collect();
    if expect != actual {
        return Err(AppError::coded(E4001).with_detail(format!(
            "{} DNS 回读不一致（服务 {}）：期望 {} / 实际 {}",
            family.label(),
            iface.alias,
            expect.join(", "),
            actual.join(", ")
        )));
    }
    Ok(())
}

pub fn reset(alias: &str, family: Option<AddressFamily>) -> Result<()> {
    let iface = resolve_alias(alias)?;
    match family {
        Some(f) => set(&iface.alias, f, &[]),
        None => {
            write_dns(&iface.alias, &[])?;
            invalidate_cache();
            Ok(())
        }
    }
}

/// 刷新 DNS 缓存。macOS 需要 root（由提权任务调用），见 `flush_needs_root()`
pub fn flush() -> Result<()> {
    let mut ok = true;
    let (a, _, e1) = net::try_run(net::DSCACHEUTIL, &["-flushcache".to_string()])?;
    if !a {
        ok = false;
        log::warn!(target: "dns", "dscacheutil -flushcache 失败：{}", net::first_line(&e1).unwrap_or_default());
    }
    let (b, _, e2) = net::try_run(
        net::KILLALL,
        &["-HUP".to_string(), "mDNSResponder".to_string()],
    )?;
    if !b {
        ok = false;
        log::warn!(target: "dns", "killall -HUP mDNSResponder 失败：{}", net::first_line(&e2).unwrap_or_default());
    }
    if ok {
        Ok(())
    } else {
        Err(AppError::coded(crate::error::E4003)
            .with_detail("DNS 缓存刷新失败（macOS 需要管理员权限）"))
    }
}

/// macOS 刷 DNS 缓存要 root；Windows 的 dnsapi 刷新不需要
pub fn flush_needs_root() -> bool {
    true
}

fn dedup_keep_order(list: &mut Vec<String>) {
    let mut seen = Vec::new();
    list.retain(|s| {
        let key = s.to_lowercase();
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_order_extracts_service_and_device() {
        let text = "\
An asterisk (*) denotes that a network service is disabled.
(1) Wi-Fi
(Hardware Port: Wi-Fi, Device: en0)

(2) Thunderbolt Bridge
(Hardware Port: Thunderbolt Bridge, Device: bridge0)

(3) *iPhone USB
(Hardware Port: iPhone USB, Device: en6)
";
        let s = parse_service_order(text);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].name, "Wi-Fi");
        assert_eq!(s[0].device, "en0");
        assert_eq!(s[0].hardware_port, "Wi-Fi");
        assert!(!s[0].disabled);
        assert_eq!(s[2].name, "iPhone USB");
        assert!(s[2].disabled);
    }

    #[test]
    fn parse_hardware_ports_devices() {
        let text = "\
Hardware Port: Wi-Fi
Device: en0
Ethernet Address: aa:bb:cc

Hardware Port: Thunderbolt 1
Device: en1
";
        let d = parse_hardware_ports(text);
        assert_eq!(d, vec!["en0".to_string(), "en1".to_string()]);
    }

    #[test]
    fn family_split_and_match() {
        let servers = vec!["8.8.8.8".to_string(), "2001:4860:4860::8888".to_string()];
        let (v4, v6) = split_by_family(&servers, false);
        assert_eq!(v4.servers, vec!["8.8.8.8".to_string()]);
        assert_eq!(v6.servers, vec!["2001:4860:4860::8888".to_string()]);
        assert!(AddressFamily::V4.matches("1.1.1.1"));
        assert!(!AddressFamily::V4.matches("::1"));
        assert!(AddressFamily::V6.matches("::1"));
    }

    #[test]
    fn pick_prefers_physical_up_primary() {
        let mk =
            |alias: &str, physical: bool, _up: bool, primary: bool, status: &str| NetInterface {
                alias: alias.to_string(),
                status: status.to_string(),
                is_physical: physical,
                has_default_gateway: primary,
                ..Default::default()
            };
        let list = vec![
            mk("Thunderbolt Bridge", true, false, false, "Down"),
            mk("Wi-Fi", true, true, true, "Up"),
            mk("iPhone USB", false, true, false, "Up"),
        ];
        let picked = pick_in_use(&list).unwrap();
        assert_eq!(picked.alias, "Wi-Fi");
    }

    #[test]
    fn virtual_device_detection() {
        assert!(is_virtual_device("utun3"));
        assert!(is_virtual_device("awdl0"));
        assert!(!is_virtual_device("en0"));
        assert!(!is_virtual_device("bridge0"));
    }
}

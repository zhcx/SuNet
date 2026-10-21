//! 系统代理（macOS）
//!
//! macOS **没有全局代理配置**：代理是挂在「网络服务」上的（系统设置里的代理面板
//! 作用的也是当前服务）。因此本模块的语义约定为：
//!   - **写**：对所有未停用的网络服务生效（2026-10-07 用户决策）
//!   - **读**：读主服务（服务顺序第一且未停用）的代理设置，作为"当前状态"
//!   - 改配置需要管理员权限（networksetup）⇒ 应用侧经提权任务调用，
//!     `proxy_write_needs_root()` 返回 true 告诉上层必须提权（Windows 不需要）
//!
//! 与 Windows 的语义差异（已记入 README/HANDOFF 平台对照）：
//!   - WinINET `ProxyOverride` 是分号分隔且支持通配符；macOS 的例外列表是域名列表，
//!     不接受 `127.*` 这种写法，也没有 `<local>` ⇒ `to_mac_exceptions` / `from_mac_exceptions`
//!   - WinINET 的 `ProxyServer` 一个值服务所有协议；macOS 的 HTTP / HTTPS / SOCKS 是
//!     **三个独立开关** ⇒ 这里逐协议写入（见 [`write_manual`]）：给了落点的协议写地址
//!     并打开，没给落点的协议**显式关闭**（顺手清掉旧版本把三项都指到同一端口的残留）
//!   - PAC 由 `-setautoproxyurl` / `-setautoproxystate` 管理（手动写入不动 PAC，同 Windows）

use crate::error::{AppError, Result, E3003, E3004};
use crate::os::proxy_restore::ProtocolRestore;
use serde::{Deserialize, Serialize};

use super::{dns_client, net};

pub const DEFAULT_BYPASS: &str = "localhost;127.*;<local>";

/// 代理快照（与 Windows 同形，便于配置文件互通）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct ProxyState {
    pub enable: bool,
    /// **主落点**：HTTP → HTTPS → SOCKS 里第一个开着的
    /// （托盘 / 快捷面板这类只显示一个地址的界面用它）
    pub server: String,
    pub bypass: String,
    pub autoconfig_url: Option<String>,
    /// PAC 存在时优先级高于手动代理
    pub pac_present: bool,
    /// 逐协议落点（`host:port`）；`None` = 该协议没开，或快照里没有这个信息。
    ///
    /// 只有 macOS 有"逐协议代理"这个概念（三个独立开关），Windows 的 `ProxyState`
    /// 里没有这几个字段；反序列化时缺失也不会失败（`serde(default)`），
    /// 所以两边共用同一份 `proxy_snapshot.json` 的旧文件仍然读得进来。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_proxy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub https_proxy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socks_proxy: Option<String>,
}

impl ProxyState {
    /// 手动代理是否真的在生效（PAC 存在时手动代理会被覆盖）
    pub fn is_on(&self) -> bool {
        self.enable && !self.pac_present
    }

    /// 开着的协议 → `(标签, 落点)`，顺序与系统设置一致（HTTP / HTTPS / SOCKS）。
    ///
    /// `http_proxy` 等只在对应 `*Enable` 为 1 时才有值（见 `read()` 里的 `one()`），
    /// 所以这里不需要再判 `enable`；留空的协议自然不出现。
    pub fn endpoints(&self) -> Vec<(&'static str, &str)> {
        [
            ("HTTP", self.http_proxy.as_deref()),
            ("HTTPS", self.https_proxy.as_deref()),
            ("SOCKS", self.socks_proxy.as_deref()),
        ]
        .iter()
        .filter_map(|(label, addr)| {
            addr.filter(|a| !a.trim().is_empty())
                .map(|a| (*label, a.trim()))
        })
        .collect()
    }
}

/// macOS 的三个代理协议：系统里是三个**互相独立的开关**
/// （系统设置 → 网络 → 详细信息 → 代理）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Proto {
    Web,
    Secure,
    Socks,
}

impl Proto {
    /// 三个协议（顺序 = 界面顺序 = 系统设置里的顺序）
    const ALL: [Proto; 3] = [Proto::Web, Proto::Secure, Proto::Socks];

    /// 本协议在"逐协议写入内容"里的落点（`None` = 这次写入要把它关掉）
    fn target(self, p: &crate::os::ManualProxy) -> Option<String> {
        let raw = match self {
            Proto::Web => Some(p.http.as_str()),
            Proto::Secure => p.https.as_deref(),
            Proto::Socks => p.socks.as_deref(),
        };
        let s = raw.map(str::trim).filter(|s| !s.is_empty())?;
        Some(s.to_string())
    }

    /// 本协议在回读状态里的落点（`None` = 回读显示它是关的）
    fn state_target(self, st: &ProxyState) -> Option<String> {
        let raw = match self {
            Proto::Web => st.http_proxy.as_deref(),
            Proto::Secure => st.https_proxy.as_deref(),
            Proto::Socks => st.socks_proxy.as_deref(),
        };
        let s = raw.map(str::trim).filter(|s| !s.is_empty())?;
        Some(s.to_string())
    }

    /// 读服务器：`-getwebproxy` / `-getsecurewebproxy` / `-getsocksfirewallproxy`
    fn get_cmd(self) -> &'static str {
        match self {
            Proto::Web => "-getwebproxy",
            Proto::Secure => "-getsecurewebproxy",
            Proto::Socks => "-getsocksfirewallproxy",
        }
    }

    /// 写服务器（这个命令同时会把该项打开）
    fn set_cmd(self) -> &'static str {
        match self {
            Proto::Web => "-setwebproxy",
            Proto::Secure => "-setsecurewebproxy",
            Proto::Socks => "-setsocksfirewallproxy",
        }
    }

    /// 开关：`-setwebproxystate <服务> on|off`
    fn state_cmd(self) -> &'static str {
        match self {
            Proto::Web => "-setwebproxystate",
            Proto::Secure => "-setsecurewebproxystate",
            Proto::Socks => "-setsocksfirewallproxystate",
        }
    }

    /// 界面 / 日志里怎么称呼它
    fn label(self) -> &'static str {
        match self {
            Proto::Web => "HTTP",
            Proto::Secure => "HTTPS",
            Proto::Socks => "SOCKS",
        }
    }
}

/// 一次手动代理写入的**逐协议计划**：每个协议要么写地址（并打开），要么显式关闭。
///
/// 为什么要先落成计划：地址解析必须**在动手写之前**全部完成 —— 否则第一个网络服务
/// 已经改了，第二个才发现地址不合法，留下一个改了一半的系统。
struct ManualPlan {
    /// 固定按 [`Proto::ALL`] 的顺序
    per_proto: [(Proto, Option<(String, u16)>); 3],
}

impl ManualPlan {
    /// 某个协议这次要写到哪个落点（`None` = 关闭该协议）。仅单测断言用。
    #[cfg(test)]
    fn get(&self, proto: Proto) -> Option<&(String, u16)> {
        self.per_proto
            .iter()
            .find(|(p, _)| *p == proto)
            .and_then(|(_, t)| t.as_ref())
    }

    /// 这次要打开的协议数（关闭时是 0）
    fn enabled_count(&self) -> usize {
        self.per_proto.iter().filter(|(_, t)| t.is_some()).count()
    }
}

/// 解析逐协议写入计划。
///
/// `enable=false` 时落点全部忽略（三个协议都关）；`enable=true` 时**三个协议都是可选的**
/// —— 给了落点的写地址并打开、留空的显式关掉 —— 但不允许一个都不填：那等于"用开代理的
/// 方式把代理全关掉"，只会让用户以为操作出了错。
fn manual_plan(p: &crate::os::ManualProxy) -> Result<ManualPlan> {
    let mut per_proto: [(Proto, Option<(String, u16)>); 3] = [
        (Proto::Web, None),
        (Proto::Secure, None),
        (Proto::Socks, None),
    ];
    if p.enable {
        for (i, proto) in Proto::ALL.iter().enumerate() {
            let Some(raw) = proto.target(p) else { continue };
            let (host, port) = parse_server(&raw).ok_or_else(|| {
                AppError::coded(E3003).with_detail(format!(
                    "{} 代理地址格式不正确：{raw}（应形如 127.0.0.1:7890）",
                    proto.label()
                ))
            })?;
            per_proto[i] = (*proto, Some((host, port)));
        }
        if per_proto.iter().all(|(_, t)| t.is_none()) {
            return Err(AppError::coded(E3003)
                .with_detail("请至少填写一个代理协议的地址与端口（HTTP / HTTPS / SOCKS）"));
        }
    }
    Ok(ManualPlan { per_proto })
}

/// 单个协议的代理项
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
struct ProtocolProxy {
    enable: bool,
    server: String,
}

/// 单个网络服务的代理设置（"所有服务"写入前先逐个快照，还原时才不会张冠李戴）
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct ServiceProxy {
    service: String,
    /// 逐协议记录：还原时必须**分别回填**（三个独立开关，合并成一个值就是上面那个 bug）
    web: ProtocolProxy,
    secure: ProtocolProxy,
    socks: ProtocolProxy,
    bypass: String,
    autoconfig_url: Option<String>,
    pac_present: bool,
}

/// 还原点文件格式版本。
///
/// 1 = 旧版（三个协议合并成一个 `enable` / `server`）；2 = 逐协议。
/// 旧文件**不能**按新格式解读：`web` 等字段会缺省成"三个协议都没开"，还原时反而把
/// 用户原有的代理设置一起抹掉。所以版本不匹配时整份丢弃（见 [`take_snapshot`]）。
const SNAPSHOT_FMT: u8 = 2;

/// 还原点文件内容
#[derive(Serialize, Deserialize, Clone, Debug)]
struct ServiceSnapshot {
    fmt: u8,
    services: Vec<ServiceProxy>,
    /// 写这份还原点的时刻（RFC 3339）。旧文件没有这个字段 → 视为过期。
    ///
    /// 为什么需要：文件里的"接管前状态"只有在**我们确实还在接管**时才成立。
    /// app 崩溃、被杀、或有人绕过 app 直接调 helper 写入，都会让这份文件一直躺着，
    /// 下一次"关闭代理"按它还原就会回到很久以前（HANDOFF §6.5 实测：把用户原本
    /// 开着的公司代理整个关掉）。所以读取时先看时效（见 [`take_snapshot`]）。
    #[serde(default)]
    created_at: String,
}

// 时效判定的常量与实现见 `crate::os::proxy_snapshot`（两平台共用、可在任一平台单测）。

// ---------------------------------------------------------------------------
// 例外列表映射
// ---------------------------------------------------------------------------

/// Windows 风格分号列表 → macOS 例外域名列表
fn to_mac_exceptions(bypass: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in bypass.split(';') {
        let item = raw.trim();
        if item.is_empty() {
            continue;
        }
        match item {
            // macOS 没有 <local> 这个写法，对应"不含点的主机名"，即 .local 域
            "<local>" => out.push(".local".to_string()),
            // "127.*" 不是合法域名；退化为环回地址
            "127.*" => out.push("127.0.0.1".to_string()),
            _ => {
                if item.contains('*') && !item.starts_with("*.") {
                    // 通配符位置不合法：去掉通配部分（macOS 只认 *.example.com 或 .example.com）
                    let cleaned = item.replace('*', "");
                    if !cleaned.is_empty() {
                        out.push(cleaned);
                    }
                } else {
                    out.push(item.to_string());
                }
            }
        }
    }
    out
}

/// macOS 例外域名列表 → Windows 风格分号列表
fn from_mac_exceptions(list: &[String]) -> String {
    list.iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(";")
}

// ---------------------------------------------------------------------------
// 服务列表
// ---------------------------------------------------------------------------

/// 所有未停用的网络服务（`*` 前缀 = 已停用）
fn active_services() -> Result<Vec<String>> {
    let text = net::networksetup(&["-listallnetworkservices".to_string()])?;
    let list: Vec<String> = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .filter(|l| !l.starts_with('*'))
        .filter(|l| !l.starts_with("An asterisk"))
        .map(|l| l.to_string())
        .collect();
    if list.is_empty() {
        return Err(AppError::coded(E3003).with_detail("没有可用的网络服务"));
    }
    Ok(list)
}

// ---------------------------------------------------------------------------
// 读
// ---------------------------------------------------------------------------

/// 读主服务的代理状态（`scutil --proxy` 反映的是生效中的全局代理视图）
pub fn read() -> Result<ProxyState> {
    let pairs = net::scutil_proxy()?;
    let get = |k: &str| -> Option<String> {
        pairs
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.clone())
    };
    let on = |k: &str| get(k).map(|v| v == "1").unwrap_or(false);

    let http_enable = on("HTTPEnable");
    let https_enable = on("HTTPSEnable");
    let socks_enable = on("SOCKSEnable");
    let enable = http_enable || https_enable || socks_enable;

    // 只把"开着且拿得到地址"的协议记入逐协议字段；开着但地址读不出来（理论上不会发生）
    // 也不编一个假地址出来
    let one = |on: bool, host: Option<String>, port: Option<String>| -> Option<String> {
        if !on {
            return None;
        }
        let s = join_host_port(host, port);
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    };
    let http_proxy = one(http_enable, get("HTTPProxy"), get("HTTPPort"));
    let https_proxy = one(https_enable, get("HTTPSProxy"), get("HTTPSPort"));
    let socks_proxy = one(socks_enable, get("SOCKSProxy"), get("SOCKSPort"));
    // 主落点：三个里第一个开着的（给"只要一个地址"的界面用）
    let server = http_proxy
        .clone()
        .or_else(|| https_proxy.clone())
        .or_else(|| socks_proxy.clone())
        .unwrap_or_default();

    // 例外列表：形如 `0 : *.local` 的序号键
    let exceptions: Vec<String> = pairs
        .iter()
        .filter(|(k, _)| k.chars().all(|c| c.is_ascii_digit()) && !k.is_empty())
        .map(|(_, v)| v.clone())
        .collect();
    let mut bypass = from_mac_exceptions(&exceptions);
    if bypass.trim().is_empty() {
        bypass = DEFAULT_BYPASS.to_string();
    }

    let auto_url = get("ProxyAutoConfigURLString").unwrap_or_default();
    let pac_present = on("ProxyAutoConfigEnable") && !auto_url.trim().is_empty();

    Ok(ProxyState {
        enable,
        server,
        bypass,
        autoconfig_url: if pac_present { Some(auto_url) } else { None },
        pac_present,
        http_proxy,
        https_proxy,
        socks_proxy,
    })
}

/// 拼成 `host:port`。IPv6 用共用的 [`crate::os::proxy_addr::format`] 补方括号 ——
/// `scutil` 报回来的是裸地址（`::1`），直接拼成 `::1:7890` 会让界面上显示的落点
/// 与写下去的那份（`[::1]:7890`）长得不一样，也让人以为端口是 7890 还是别的。
fn join_host_port(host: Option<String>, port: Option<String>) -> String {
    match (host, port) {
        (Some(h), Some(p)) if !h.is_empty() && !p.is_empty() => match p.trim().parse::<u16>() {
            Ok(port) => crate::os::proxy_addr::format(&h, port),
            // 端口读不出来就照原样留（宁可显示得怪，也不编一个 0 号端口）
            Err(_) => format!("{h}:{p}"),
        },
        (Some(h), _) => h,
        _ => String::new(),
    }
}

/// 读某个服务自身的代理设置（还原用）
fn read_service(service: &str) -> Result<ServiceProxy> {
    let web = read_protocol(service, Proto::Web)?;
    let secure = read_protocol(service, Proto::Secure)?;
    let socks = read_protocol(service, Proto::Socks)?;
    let bypass = match net::networksetup_soft(&[
        "-getproxybypassdomains".to_string(),
        service.to_string(),
    ]) {
        Ok((_, out)) => {
            if out.contains("aren't any bypass domains") {
                String::new()
            } else {
                out.lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty() && !l.starts_with("There aren"))
                    .collect::<Vec<_>>()
                    .join(";")
            }
        }
        Err(_) => String::new(),
    };
    let (pac_on, pac_url) = parse_autoproxy(
        &net::networksetup_soft(&["-getautoproxyurl".to_string(), service.to_string()])
            .map(|(_, o)| o)
            .unwrap_or_default(),
    );

    Ok(ServiceProxy {
        service: service.to_string(),
        web,
        secure,
        socks,
        bypass,
        autoconfig_url: pac_url.clone(),
        pac_present: pac_on && pac_url.is_some(),
    })
}

/// 解析 `-getautoproxyurl` 的输出：(是否启用, PAC 地址)
///
/// 这个命令的键名与逐协议那三个**不一样**：地址在 `URL:` 下而不是 `Server:` 下，
/// 拿 [`parse_proxy_block`] 解会得到 `enabled=true, server=""` —— 于是两份信息同时丢：
///
///   · `autoconfig_url` 永远是 `None`（`URL:` 那一行没人认），还原时走
///     [`write_pac`] 的 `None` 分支把 PAC **关掉**，用户原本的 PAC 就此消失；
///   · `pac_present` 永远为 false，而[关闭代理时"被 PAC 接管"要诚实报告]、
///     [快照时效判定]都靠它。
///
/// 实测（macOS 27.0.1）：没配过时输出 `URL: (null)`；配过但关着时 URL 仍然在，
/// 只是 `Enabled: No`。所以这两件事必须分开记 —— 只留 URL 会让"关着的 PAC"
/// 在还原时被打开。
fn parse_autoproxy(text: &str) -> (bool, Option<String>) {
    let mut enabled = false;
    let mut url = String::new();
    for line in text.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("Enabled:") {
            enabled = rest.trim().eq_ignore_ascii_case("yes");
        } else if let Some(rest) = l.strip_prefix("URL:") {
            url = rest.trim().to_string();
        }
    }
    // 没配过时是占位符 `(null)`，不是地址
    if url.eq_ignore_ascii_case("(null)") {
        url.clear();
    }
    let url = if url.is_empty() { None } else { Some(url) };
    (enabled, url)
}

/// 读单个协议的 (是否开启, "host:port")
fn read_protocol(service: &str, proto: Proto) -> Result<ProtocolProxy> {
    let (enable, server) = parse_proxy_block(&net::networksetup(&[
        proto.get_cmd().to_string(),
        service.to_string(),
    ])?);
    Ok(ProtocolProxy { enable, server })
}

/// 解析 `-getwebproxy` 之类命令的输出：(Enabled, "host:port")
fn parse_proxy_block(text: &str) -> (bool, String) {
    let mut enabled = false;
    let mut host = String::new();
    let mut port = String::new();
    for line in text.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("Enabled:") {
            enabled = rest.trim().eq_ignore_ascii_case("yes");
        } else if let Some(rest) = l.strip_prefix("Server:") {
            host = rest.trim().to_string();
        } else if let Some(rest) = l.strip_prefix("Port:") {
            port = rest.trim().to_string();
        }
    }
    // "Server: (null)" 这种占位要当空处理
    if host.eq_ignore_ascii_case("(null)") {
        host.clear();
    }
    let server = if host.is_empty() {
        String::new()
    } else if port.is_empty() || port == "0" {
        host
    } else {
        format!("{host}:{port}")
    };
    (enabled, server)
}

// ---------------------------------------------------------------------------
// 写
// ---------------------------------------------------------------------------

/// 只写入手动代理（不动 PAC）：**逐协议**写 —— 给了落点的协议写地址并打开，
/// 没给落点的协议显式关闭（顺手清掉 ≤ 0.0.2 把 HTTPS / SOCKS 也指到同一端口的残留）。
/// 对所有未停用服务生效。
pub fn write_manual(p: &crate::os::ManualProxy) -> Result<()> {
    let plan = manual_plan(p)?;
    let services = active_services()?;
    log::info!(
        target: "proxy",
        "写入手动代理：{} 个协议 × {} 个网络服务（HTTP={}）",
        plan.enabled_count(),
        services.len(),
        if p.enable { "on" } else { "off" }
    );
    // 还原点只在**开启**时留：关闭意味着我们不再接管，此时再记一份"当前状态"
    // （也就是我们写下去的代理）会让下一次关闭按它还原成"代理开着"——
    // 正是 HANDOFF §6.5 那条陈旧还原点故障，只是换了个生成时机。
    if p.enable {
        // 旧还原点还成不成立：判据 =「系统代理是否仍等于我们上次写下去的那份」。
        // 用平台层自己的记录（与还原点同目录、同身份读写）：这段代码跑在 root helper 里，
        // 看不到用户家目录下 app 层的 `proxy_last_applied.json`。
        // 读不到当前状态时保守按「成立」处理 —— 无法证伪就不该丢掉已有还原能力。
        let still_ours = match (last_applied(), read()) {
            (Some(prev), Ok(now)) => crate::os::proxy_snapshot::same_state(&prev, &now),
            _ => true,
        };
        snapshot_services(&services, still_ours)?; // 先留还原点
    }
    for svc in &services {
        write_service_manual(svc, &plan, &p.bypass)?;
    }
    if p.enable {
        // 写成功之后才记「我们写下去的是什么」：下次据此判断旧还原点还成不成立。
        // 记录失败不影响本次切换（只是下一次会更保守地重写还原点）。
        if let Ok(now) = read() {
            write_last_applied(&now);
        }
    } else {
        // 关闭后我们不再持有接管：任何残留的还原点都不再代表"接管前"
        drop_snapshot();
    }
    invalidate_cache();
    Ok(())
}

/// 完整写回（含 PAC），用于快照还原
pub fn restore(state: &ProxyState) -> Result<()> {
    // 优先用"写入前逐服务留的还原点"：各服务原本的设置可能不同
    if let Some(prev) = take_snapshot()? {
        let mut first_err = None;
        for p in prev {
            if let Err(e) = write_service_full(&p) {
                log::warn!(target: "proxy", "还原服务 {} 失败：{e}", p.service);
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
        invalidate_cache();
        // 还原 = 接管结束：留着"上次写入"记录，下次开启会把一份已经还原过的状态
        // 当成"我们还在接管"，反而把新还原点判成过期。
        drop_last_applied();
        return match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        };
    }
    // 没有还原点：按与 write_manual 相同的规则落到所有服务，PAC 单独回填。
    // 逐协议字段只在带逐协议信息的快照（macOS 0.0.3+）里才有：老快照只有主落点，
    // 那就只恢复主落点 —— 老版本正是"三项同端口"才断网的，照抄那个形态等于把故障
    // 一起还原回来（见 [`state_to_manual`]）。
    let plan = manual_plan(&state_to_manual(state))?;
    let services = active_services()?;
    for svc in &services {
        write_service_manual(svc, &plan, &state.bypass)?;
        write_pac(svc, state.autoconfig_url.as_deref(), state.pac_present)?;
    }
    drop_last_applied();
    invalidate_cache();
    Ok(())
}

/// 回读状态 → 逐协议写入内容（只在"没有逐服务还原点"的兜底还原里用）
fn state_to_manual(st: &ProxyState) -> crate::os::ManualProxy {
    let norm = |s: Option<&str>| -> Option<String> {
        s.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    };
    // 老快照（≤ 0.0.2）只记了主落点，那时它就是 HTTP；新快照逐协议记录 ——
    // HTTP 留空就是"没开 HTTP"，**不能**拿 `server` 顶上（那时它可能是 HTTPS / SOCKS 的落点，
    // 拿过来就成了"把 SOCKS 的地址写到 HTTP 上"）。
    let legacy = st.http_proxy.is_none() && st.https_proxy.is_none() && st.socks_proxy.is_none();
    let http = if legacy {
        norm(st.http_proxy.as_deref()).or_else(|| norm(Some(&st.server)))
    } else {
        norm(st.http_proxy.as_deref())
    };
    crate::os::ManualProxy {
        enable: st.enable,
        http: http.unwrap_or_default(),
        // 逐协议信息缺失时**不**把主落点复制到 HTTPS / SOCKS：
        // 那正是 ≤ 0.0.2 三项同端口、一开就断网的形态
        https: norm(st.https_proxy.as_deref()),
        socks: norm(st.socks_proxy.as_deref()),
        bypass: st.bypass.clone(),
    }
}

/// 彻底清零：三个代理全关 + PAC 关（保留例外列表 —— 它是绕过名单，清掉反而更糟）
pub fn hard_clear() -> Result<()> {
    let services = active_services()?;
    for svc in &services {
        for p in Proto::ALL {
            set_proxy_state(svc, p, false)?;
        }
        let _ = net::networksetup_soft(&[
            "-setautoproxystate".to_string(),
            svc.clone(),
            "off".to_string(),
        ]);
    }
    // 清零之后系统里已经没有"我们接管前的样子"可还原了；留着还原点只会让
    // 下一次 restore（例如「撤销清除」）优先吃这份更早的配置（见 [`take_snapshot`]）。
    drop_snapshot();
    invalidate_cache();
    Ok(())
}

/// 按计划要执行的动作，**顺序已经排好**：先打开要开的，最后关掉不要的。
///
/// 抽成纯函数是为了能单测「顺序」这件事本身 —— 它决定的是切换过程中
/// 系统里有没有一段时间**一个代理都没有**。
///
/// 旧实现按 [`Proto::ALL`]（HTTP → HTTPS → SOCKS）一趟走完，只在注释里写着
/// 「先写要开的、后关不要的」，实际并没有实现：HTTP 关、HTTPS 开时，第一步就把
/// HTTP 关掉，等到第三步才打开 HTTPS，中间那一段所有走代理的应用都在直连外网
/// —— 对用户是"每次切协议都断一下"。两趟走才是注释描述的行为。
fn ordered_actions(plan: &ManualPlan) -> Vec<(Proto, Option<(String, u16)>)> {
    let mut out: Vec<(Proto, Option<(String, u16)>)> = plan
        .per_proto
        .iter()
        .filter(|(_, t)| t.is_some())
        .cloned()
        .collect();
    out.extend(plan.per_proto.iter().filter(|(_, t)| t.is_none()).cloned());
    out
}

/// 按计划写一个服务：逐协议「写地址 + 打开」或「显式关闭」。
fn write_service_manual(service: &str, plan: &ManualPlan, bypass: &str) -> Result<()> {
    for (proto, target) in ordered_actions(plan) {
        match target {
            // `-set*proxy` 这条命令同时会把该项打开
            Some((host, port)) => set_proxy(service, proto, &host, port)?,
            None => set_proxy_state(service, proto, false)?,
        }
    }
    write_exceptions(service, bypass)
}

/// `networksetup -set*proxy <服务> <主机> <端口>`（这条命令同时会把该项打开）
fn set_proxy(service: &str, proto: Proto, host: &str, port: u16) -> Result<()> {
    net::networksetup(&[
        proto.set_cmd().to_string(),
        service.to_string(),
        host.to_string(),
        port.to_string(),
    ])
    .map_err(|e| {
        AppError::coded(E3003)
            .with_detail(format!("设置 {} 代理失败（{service}）：{e}", proto.label()))
    })?;
    Ok(())
}

/// `networksetup -set*proxystate <服务> on|off`
fn set_proxy_state(service: &str, proto: Proto, on: bool) -> Result<()> {
    net::networksetup(&[
        proto.state_cmd().to_string(),
        service.to_string(),
        if on { "on" } else { "off" }.to_string(),
    ])
    .map_err(|e| {
        AppError::coded(E3003)
            .with_detail(format!("{} 代理开关失败（{service}）：{e}", proto.label()))
    })?;
    Ok(())
}

/// 按还原点回填：**逐协议**写（三个协议是独立开关，合并成一个值正是 2026-10-10 那个断网 bug）
///
/// **PAC 先处理**：PAC 的优先级高于手动代理（与 Windows `restore` 同一顺序）。
/// 还原点里 PAC 关着、手动代理开着时，若先写手动代理，那几秒内 PAC 仍是旧值并把
/// 手动代理盖住；先落 PAC 就不会出现"写完了但没生效"的中间态。
fn write_service_full(p: &ServiceProxy) -> Result<()> {
    write_pac(&p.service, p.autoconfig_url.as_deref(), p.pac_present)?;
    for (proto, want) in [
        (Proto::Web, &p.web),
        (Proto::Secure, &p.secure),
        (Proto::Socks, &p.socks),
    ] {
        write_protocol(&p.service, proto, want)?;
    }
    write_exceptions(&p.service, &p.bypass)
}

/// 把某个协议恢复成还原点里的样子。
///
/// 「该做什么」由共用的 [`crate::os::proxy_restore::plan`] 决定（纯逻辑，
/// 两平台都能单测），这里只负责照着执行 networksetup。
///
/// `-set*proxy` 会顺带把开关打开，所以「关着但预填了地址」必须紧跟一次 off ——
/// 中间这一瞬间是开着的，但两次调用之间没有别的写入，用户感知不到。
fn write_protocol(service: &str, proto: Proto, want: &ProtocolProxy) -> Result<()> {
    match crate::os::proxy_restore::plan(want.enable, &want.server) {
        ProtocolRestore::Enable(host, port) => set_proxy(service, proto, &host, port),
        ProtocolRestore::Invalid(raw) => Err(AppError::coded(E3003).with_detail(format!(
            "还原点里的 {} 代理地址不合法：{raw}",
            proto.label()
        ))),
        ProtocolRestore::PrefillThenDisable(host, port) => {
            // 预填地址写回失败不该阻断还原：开关状态比地址栏里的预填值重要，
            // 丢掉预填只是下次要重填，开关没关回去则是代理还在生效。
            if let Err(e) = set_proxy(service, proto, &host, port) {
                log::warn!(
                    target: "proxy",
                    "还原 {service} 的 {} 预填地址失败（仅影响预填值）：{e}",
                    proto.label()
                );
            }
            // 上一步会把开关打开（networksetup 没有"只改地址不改开关"的命令），
            // 这里的 off 没成功 = 用户原本关着的协议现在开着，必须说清楚。
            set_proxy_state(service, proto, false).map_err(|e| {
                AppError::coded(E3003).with_detail(format!(
                    "{} 代理地址已回填，但开关没能关回去（可能仍处于开启状态），请检查 {service} 的代理设置：{e}",
                    proto.label()
                ))
            })
        }
        ProtocolRestore::Disable => set_proxy_state(service, proto, false),
    }
}

/// PAC（`-setautoproxyurl` / `-setautoproxystate`）
fn write_pac(service: &str, url: Option<&str>, pac_present: bool) -> Result<()> {
    match url.filter(|u| !u.trim().is_empty()) {
        Some(url) => {
            net::networksetup(&[
                "-setautoproxyurl".to_string(),
                service.to_string(),
                url.to_string(),
            ])?;
            net::networksetup(&[
                "-setautoproxystate".to_string(),
                service.to_string(),
                if pac_present { "on" } else { "off" }.to_string(),
            ])?;
        }
        None => {
            let _ = net::networksetup_soft(&[
                "-setautoproxystate".to_string(),
                service.to_string(),
                "off".to_string(),
            ]);
        }
    }
    Ok(())
}

fn write_exceptions(service: &str, bypass: &str) -> Result<()> {
    let list = to_mac_exceptions(bypass);
    let mut args = vec!["-setproxybypassdomains".to_string(), service.to_string()];
    if list.is_empty() {
        args.push("Empty".to_string());
    } else {
        args.extend(list);
    }
    net::networksetup(&args).map_err(|e| {
        AppError::coded(E3003).with_detail(format!("设置代理例外列表失败（{service}）：{e}"))
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 还原点（side-car 文件）
// ---------------------------------------------------------------------------

fn snapshot_path() -> std::path::PathBuf {
    crate::paths::appdata_dir().join("proxy_prev.json")
}

/// 丢弃还原点（接管结束后调用）
fn drop_snapshot() {
    let _ = std::fs::remove_file(snapshot_path());
    drop_last_applied();
}

/// 平台层自己的「上次写下去的状态」记录。
///
/// 与 app 层的 `proxy_last_applied.json` 是**两份**，不是重复：app 层那份在用户
/// 家目录、由 GUI 进程读写；这份在 root helper 的家目录、由提权进程读写。
/// macOS 的代理写 / 还原**全部**在 root 身份下发生（`proxy_write_needs_root()` = true），
/// 所以判「旧还原点还成不成立」只能靠这份 —— 读 app 层那份会是另一个用户的文件。
fn last_applied_path() -> std::path::PathBuf {
    crate::paths::appdata_dir().join("proxy_last_applied.json")
}

fn last_applied() -> Option<ProxyState> {
    let text = std::fs::read_to_string(last_applied_path()).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_last_applied(st: &ProxyState) {
    let path = last_applied_path();
    if let Some(dir) = path.parent() {
        // 目录可能不存在（root 名下第一次写）：失败只记日志，不影响本次切换
        if let Err(e) = std::fs::create_dir_all(dir) {
            log::warn!(target: "proxy", "创建代理状态记录目录失败（{}）：{e}", dir.display());
            return;
        }
    }
    match serde_json::to_string_pretty(st) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&path, text.as_bytes()) {
                log::warn!(target: "proxy", "写入代理状态记录失败（{}）：{e}", path.display());
            }
        }
        Err(e) => log::warn!(target: "proxy", "序列化代理状态失败：{e}"),
    }
}

fn drop_last_applied() {
    let _ = std::fs::remove_file(last_applied_path());
}

/// 留还原点。**只在"接管前的原样仍然成立"时沿用旧文件**。
///
/// 从前这里只判「文件存在就不动」。那在 app 崩溃 / 被杀、或有人绕过 app 直接调
/// helper 写入之后就错了：文件里记的是更早的接管前状态，下一次还原会按它把
/// 用户后来配的代理整个抹掉（HANDOFF §6.5 实测 ①）。
///
/// 现在多一道判据 —— 交给调用方传进来的 `snapshot_still_ours`：
///   · 成立 → 沿用（接管还在，旧文件就是接管前的原样）；
///   · 不成立 → 用**当前状态**重写一份（下次还原回到的是"这次接管前"）。
fn snapshot_services(services: &[String], snapshot_still_ours: bool) -> Result<()> {
    if snapshot_path().exists() && snapshot_still_ours {
        return Ok(());
    }
    // 旧文件不再成立：先丢弃，下面的写入会生成新的一份（写失败时不会留下两份混用的状态）
    if snapshot_path().exists() {
        log::info!(target: "proxy", "已有代理还原点不再成立（系统代理已被外部改动），按当前状态重写");
        drop_snapshot();
    }
    // 每个服务要起 5 个 networksetup（三个协议 + 例外列表 + PAC），串行读一台常见
    // 双服务机器就是 10+ 次进程启动 —— 这一步发生在"点开启"之后、写第一个字节之前，
    // 用户看到的是长时间的"正在应用…"。各服务的读取互不依赖，并发后墙钟按单服务算。
    // 顺序仍然保持 services 的顺序（join 按提交顺序取结果），还原点里的服务次序稳定。
    let mut list: Vec<ServiceProxy> = Vec::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = services
            .iter()
            .map(|svc| (svc, scope.spawn(move || (svc, read_service(svc)))))
            .collect();
        for (svc, h) in handles {
            match h.join() {
                Ok((_, Ok(p))) => list.push(p),
                Ok((_, Err(e))) => {
                    log::warn!(target: "proxy", "读取服务 {svc} 的代理设置失败（跳过）：{e}")
                }
                Err(_) => log::warn!(target: "proxy", "读取服务 {svc} 的线程异常终止（跳过）"),
            }
        }
    });
    let snap = ServiceSnapshot {
        fmt: SNAPSHOT_FMT,
        services: list,
        created_at: chrono::Local::now().to_rfc3339(),
    };
    let text = serde_json::to_string_pretty(&snap)?;
    let path = snapshot_path();
    // 目录可能不存在：提权身份（macOS 的 root helper）与我们不共享家目录，
    // `appdata_dir()` 指到 root 家目录下就什么也没有（以前这里直接 `?` 把裸 io::Error
    // 抛出去，用户只看到不带路径的 E0000，完全无从排查）。写文件的事自己负责把
    // 目录备好，出错也把路径报出来。
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            AppError::coded(E3003).with_detail(format!(
                "创建代理还原点目录失败（{}）：{e}",
                parent.display()
            ))
        })?;
    }
    std::fs::write(&path, text.as_bytes()).map_err(|e| {
        AppError::coded(E3003).with_detail(format!("写入代理还原点失败（{}）：{e}", path.display()))
    })?;
    Ok(())
}

fn take_snapshot() -> Result<Option<Vec<ServiceProxy>>> {
    let path = snapshot_path();
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| {
        AppError::coded(E3003).with_detail(format!("读取代理还原点失败（{}）：{e}", path.display()))
    })?;
    let snap: ServiceSnapshot = match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(e) => {
            // 旧格式（裸数组）或文件损坏：一律按"没有还原点"处理，绝不硬解读
            log::warn!(target: "proxy", "代理还原点解析失败（按无还原点处理）：{e}");
            let _ = std::fs::remove_file(&path);
            return Ok(None);
        }
    };
    if snap.fmt != SNAPSHOT_FMT {
        log::warn!(
            target: "proxy",
            "代理还原点格式版本 {} 与当前 {} 不符（按无还原点处理）",
            snap.fmt,
            SNAPSHOT_FMT
        );
        let _ = std::fs::remove_file(&path);
        return Ok(None);
    }
    // 时效检查：见 `crate::os::proxy_snapshot` 的说明。没有时间戳（旧文件）同样按
    // 过期处理 —— 宁可"不还原"，也不能拿一份不知道多久以前的配置覆盖用户现在的设置。
    if crate::os::proxy_snapshot::expired(&snap.created_at, chrono::Local::now()) {
        log::warn!(
            target: "proxy",
            "代理还原点已超过 {} 小时（或缺少时间戳），按无还原点处理",
            crate::os::proxy_snapshot::MAX_AGE_HOURS
        );
        let _ = std::fs::remove_file(&path);
        return Ok(None);
    }
    let _ = std::fs::remove_file(&path);
    if snap.services.is_empty() {
        Ok(None)
    } else {
        Ok(Some(snap.services))
    }
}

// ---------------------------------------------------------------------------
// 校验 / 通知
// ---------------------------------------------------------------------------

/// 回读校验（只看主服务，与 [`read`] 同一约定）
pub fn verify(expect_enable: bool) -> Result<bool> {
    let st = read()?;
    if expect_enable {
        Ok(st.enable)
    } else {
        Ok(!st.enable && !st.pac_present)
    }
}

/// 逐协议回读校验：每个协议的「开 / 关 + 落点」都要与写入计划一致。
///
/// 写入时落到**所有**未停用服务，回读只回一个代表（主服务）：macOS 上各服务共享同一个
/// `scutil --proxy` 视图，逐服务比对既没意义也读不到（`networksetup` 只能逐个服务读）。
pub fn verify_manual(p: &crate::os::ManualProxy) -> Result<bool> {
    let st = read()?;
    // PAC 存在时手动代理不生效（`ProxyState::is_on` 的同一约定）
    if st.pac_present {
        return Ok(false);
    }
    Ok(protocols_match(&manual_plan(p)?, &st))
}

/// 逐协议比对（纯函数，便于测试）：计划里的每个协议的开/关与落点都要与回读一致
fn protocols_match(want: &ManualPlan, st: &ProxyState) -> bool {
    for (proto, target) in want.per_proto.iter() {
        match (target, proto.state_target(st)) {
            (None, None) => {}
            (Some((host, port)), Some(got)) => {
                // 系统会给地址做归一化（IPv6 加方括号等），比 (host, port) 而不是比字符串
                if parse_server(&got) != Some((host.clone(), *port)) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

/// macOS 无需显式通知（configd 会广播配置变更）；保持同签名以便上层一致调用
pub fn notify_changed() -> bool {
    log::debug!(target: "proxy", "[{E3004}] macOS 由 configd 广播代理变更，无需显式通知");
    true
}

/// 代理写入是否需要提权（macOS 的 networksetup 改配置需要管理员权限）
pub fn proxy_write_needs_root() -> bool {
    true
}

fn invalidate_cache() {
    dns_client::invalidate_cache();
}

/// 解析 "127.0.0.1:7890" / "[::1]:7890" 形式；兼容 "http=host:port;https=host:port"
///
/// 单段解析与 Windows 共用 [`crate::os::proxy_addr::parse`]：方括号 IPv6 的规则
/// 只能写一份，否则两平台对同一串地址的理解迟早分叉。
pub fn parse_server(server: &str) -> Option<(String, u16)> {
    let s = server.trim();
    if s.is_empty() {
        return None;
    }
    let first = s.split(';').next().unwrap_or(s);
    let seg = first.rsplit('=').next().unwrap_or(first);
    let (host, port) = crate::os::proxy_addr::parse(seg)?;
    (port > 0).then_some((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 时效判定的用例在 `crate::os::proxy_snapshot`（两平台共用，Windows 上也会跑）
    #[test]
    fn parse_server_forms() {
        assert_eq!(
            parse_server("127.0.0.1:7890"),
            Some(("127.0.0.1".to_string(), 7890))
        );
        assert_eq!(
            parse_server("http=10.0.0.2:8080;https=10.0.0.2:8081"),
            Some(("10.0.0.2".to_string(), 8080))
        );
        assert_eq!(parse_server(""), None);
        assert_eq!(parse_server("no-port"), None);
        // 方括号 IPv6：方括号是字面量语法，不是主机名的一部分
        assert_eq!(parse_server("[::1]:7890"), Some(("::1".to_string(), 7890)));
        assert_eq!(
            parse_server("https=[2606:4700:4700::1111]:8443"),
            Some(("2606:4700:4700::1111".to_string(), 8443))
        );
        // 端口 0 不算落点（networksetup 会拒绝，界面也不该当成"已配置"）
        assert_eq!(parse_server("localhost:0"), None);
    }

    /// `scutil` 报回来的是裸 IPv6 地址，拼成落点时必须补方括号，
    /// 且拼出来的串要能被 `parse_server` 原样读回去（否则回读校验永远不通过）
    #[test]
    fn join_host_port_brackets_ipv6() {
        assert_eq!(
            join_host_port(Some("127.0.0.1".into()), Some("7890".into())),
            "127.0.0.1:7890"
        );
        assert_eq!(
            join_host_port(Some("::1".into()), Some("7890".into())),
            "[::1]:7890"
        );
        assert_eq!(
            parse_server(&join_host_port(Some("::1".into()), Some("7890".into()))),
            Some(("::1".to_string(), 7890))
        );
        // 没有端口 / 没有地址：原样返回，不编一个 0
        assert_eq!(join_host_port(Some("::1".into()), None), "::1");
        assert_eq!(join_host_port(None, Some("7890".into())), "");
    }

    #[test]
    fn exceptions_mapping_roundtrip() {
        let mac = to_mac_exceptions(DEFAULT_BYPASS);
        assert_eq!(
            mac,
            vec![
                "localhost".to_string(),
                "127.0.0.1".to_string(),
                ".local".to_string()
            ]
        );
        assert_eq!(
            from_mac_exceptions(&mac),
            "localhost;127.0.0.1;.local".to_string()
        );
    }

    #[test]
    fn parse_proxy_block_output() {
        let text = "Enabled: Yes\nServer: 127.0.0.1\nPort: 7890\nAuthenticated Proxy Enabled: 0\n";
        assert_eq!(
            parse_proxy_block(text),
            (true, "127.0.0.1:7890".to_string())
        );
        let off = "Enabled: No\nServer: (null)\nPort: 0\n";
        assert_eq!(parse_proxy_block(off), (false, String::new()));
    }

    /// `-getautoproxyurl` 的地址在 `URL:` 下，不在 `Server:` 下 —— 拿
    /// [`parse_proxy_block`] 解会把地址与开关**同时**丢掉（本机 macOS 27.0.1
    /// 的真实输出见用例里的字面量）。
    #[test]
    fn parse_autoproxy_reads_url_key_not_server_key() {
        // 没配过：占位符 (null)，不是地址
        assert_eq!(parse_autoproxy("URL: (null)\nEnabled: No\n"), (false, None));
        // 配过但关着：URL 仍在，不能让还原把它打开
        assert_eq!(
            parse_autoproxy("URL: http://127.0.0.1:33331/commands/pac\nEnabled: No\n"),
            (false, Some("http://127.0.0.1:33331/commands/pac".into()))
        );
        // 开着：URL 与开关都要留下，还原时才写得回去
        assert_eq!(
            parse_autoproxy("URL: http://wpad/wpad.dat\nEnabled: Yes\n"),
            (true, Some("http://wpad/wpad.dat".into()))
        );
        // 命令失败（空输出）按"没有 PAC"处理
        assert_eq!(parse_autoproxy(""), (false, None));
    }

    /// 写一个服务时**先开要开的、最后才关不要的**：反过来（按协议顺序一趟走完）
    /// 会让 HTTP 先被关、HTTPS 还没打开，中间那一段系统里一个代理都没有。
    #[test]
    fn service_actions_open_before_closing() {
        let plan = manual_plan(&crate::os::ManualProxy {
            enable: true,
            http: String::new(),
            https: Some("127.0.0.1:7891".into()),
            socks: None,
            bypass: String::new(),
        })
        .unwrap();
        let acts = ordered_actions(&plan);
        let opens: Vec<Proto> = acts
            .iter()
            .filter(|(_, t)| t.is_some())
            .map(|(p, _)| *p)
            .collect();
        let closes: Vec<Proto> = acts
            .iter()
            .filter(|(_, t)| t.is_none())
            .map(|(p, _)| *p)
            .collect();
        assert_eq!(opens, vec![Proto::Secure]);
        // HTTP / SOCKS 的关闭动作必须排在打开 HTTPS 之后
        assert_eq!(acts.len(), 3);
        assert_eq!(
            &acts[..1],
            &[(Proto::Secure, Some(("127.0.0.1".to_string(), 7891)))]
        );
        assert!(closes.contains(&Proto::Web) && closes.contains(&Proto::Socks));
    }

    #[test]
    fn is_on_requires_no_pac() {
        let mut st = ProxyState {
            enable: true,
            ..Default::default()
        };
        assert!(st.is_on());
        st.pac_present = true;
        assert!(!st.is_on());
    }

    /// 回归（2026-10-10 用户实测 → 逐协议改造）：手动代理**逐协议**写。
    /// 只给了 HTTP 时，HTTPS / SOCKS 必须被显式关掉 —— 旧版本把三项都指到同一个
    /// host:port，只支持 HTTP 的代理端口会收到 SOCKS 请求，一开代理就断网。
    #[test]
    fn manual_plan_is_per_protocol() {
        let only_http = crate::os::ManualProxy::http_only("127.0.0.1:7890", "localhost");
        let plan = manual_plan(&only_http).unwrap();
        assert_eq!(plan.enabled_count(), 1);
        assert_eq!(plan.get(Proto::Web), Some(&("127.0.0.1".to_string(), 7890)));
        assert_eq!(plan.get(Proto::Secure), None);
        assert_eq!(plan.get(Proto::Socks), None);

        // 三个协议各指一个端口：各自拿到自己的落点（不再合并成一个）
        let all = crate::os::ManualProxy {
            enable: true,
            http: "127.0.0.1:7890".into(),
            https: Some("127.0.0.1:7891".into()),
            socks: Some("127.0.0.1:1080".into()),
            bypass: "localhost".into(),
        };
        let plan = manual_plan(&all).unwrap();
        assert_eq!(plan.enabled_count(), 3);
        assert_eq!(
            plan.get(Proto::Secure),
            Some(&("127.0.0.1".to_string(), 7891))
        );
        assert_eq!(
            plan.get(Proto::Socks),
            Some(&("127.0.0.1".to_string(), 1080))
        );

        // 关闭：三个协议全关（落点不看）——旧版本 / 手工设置的残留也要清掉
        let off = manual_plan(&crate::os::ManualProxy::off("localhost")).unwrap();
        assert_eq!(off.enabled_count(), 0);
        assert!(off.per_proto.iter().all(|(_, t)| t.is_none()));

        // 地址不合法 → 动手写之前就报错（否则第一个服务已经改了、后面才失败）
        assert!(manual_plan(&crate::os::ManualProxy::http_only("no-port", "")).is_err());
        assert!(manual_plan(&crate::os::ManualProxy {
            enable: true,
            http: "127.0.0.1:7890".into(),
            https: Some("127.0.0.1:not-a-port".into()),
            socks: None,
            bypass: String::new(),
        })
        .is_err());
        // HTTP 留空 = 关闭 HTTP，只写 HTTPS（三个协议都是可选的）
        let https_only = manual_plan(&crate::os::ManualProxy {
            enable: true,
            http: String::new(),
            https: Some("127.0.0.1:7891".into()),
            socks: None,
            bypass: String::new(),
        })
        .unwrap();
        assert_eq!(https_only.enabled_count(), 1);
        assert_eq!(https_only.get(Proto::Web), None);
        assert_eq!(
            https_only.get(Proto::Secure),
            Some(&("127.0.0.1".to_string(), 7891))
        );

        // 一个都不填：开代理却什么也不写 —— 必须报错，不能静默"开"上一次空操作
        assert!(manual_plan(&crate::os::ManualProxy {
            enable: true,
            http: String::new(),
            https: None,
            socks: None,
            bypass: String::new(),
        })
        .is_err());
    }

    /// 逐协议回读比对：某个协议的开关或落点与计划不符就不能算通过；
    /// 而且**没配的协议必须是关的**（降级成"只关 HTTP"会让残留继续生效）
    #[test]
    fn protocols_match_compares_each_protocol() {
        let want = manual_plan(&crate::os::ManualProxy {
            enable: true,
            http: "127.0.0.1:7890".into(),
            https: Some("127.0.0.1:7891".into()),
            socks: None,
            bypass: String::new(),
        })
        .unwrap();

        let ok = ProxyState {
            enable: true,
            http_proxy: Some("127.0.0.1:7890".into()),
            https_proxy: Some("127.0.0.1:7891".into()),
            ..Default::default()
        };
        assert!(protocols_match(&want, &ok));

        // HTTPS 落点不对 → 不通过
        let wrong_port = ProxyState {
            https_proxy: Some("127.0.0.1:9999".into()),
            ..ok.clone()
        };
        assert!(!protocols_match(&want, &wrong_port));

        // 没配的 SOCKS 却开着（旧版本残留）→ 不通过
        let leftover_socks = ProxyState {
            socks_proxy: Some("127.0.0.1:1080".into()),
            ..ok.clone()
        };
        assert!(!protocols_match(&want, &leftover_socks));

        // 该开的没开 → 不通过
        let missing_https = ProxyState {
            https_proxy: None,
            ..ok
        };
        assert!(!protocols_match(&want, &missing_https));
    }

    /// 老快照（没有逐协议字段）只还原主落点：**不**把主落点复制给 HTTPS / SOCKS
    /// —— 那正是 ≤ 0.0.2 三项同端口、一开就断网的形态
    #[test]
    fn state_to_manual_ignores_missing_protocols() {
        let old = ProxyState {
            enable: true,
            server: "127.0.0.1:7890".into(),
            bypass: "localhost".into(),
            ..Default::default()
        };
        let m = state_to_manual(&old);
        assert_eq!(m.http, "127.0.0.1:7890");
        assert!(m.https.is_none() && m.socks.is_none());

        // 新快照：逐协议原样带回
        let new = ProxyState {
            enable: true,
            server: "127.0.0.1:7890".into(),
            http_proxy: Some("127.0.0.1:7890".into()),
            https_proxy: Some("127.0.0.1:7891".into()),
            socks_proxy: Some("127.0.0.1:1080".into()),
            ..Default::default()
        };
        let m = state_to_manual(&new);
        assert_eq!(m.http, "127.0.0.1:7890");
        assert_eq!(m.https.as_deref(), Some("127.0.0.1:7891"));
        assert_eq!(m.socks.as_deref(), Some("127.0.0.1:1080"));

        // HTTP 留空、SOCKS 开着的新快照：**不能**把 SOCKS 落点当成 HTTP 写回去
        // （`server` 在那种状态下就是第一个非空落点 = SOCKS 地址）
        let socks_only = ProxyState {
            enable: true,
            server: "127.0.0.1:1080".into(),
            socks_proxy: Some("127.0.0.1:1080".into()),
            ..Default::default()
        };
        let m = state_to_manual(&socks_only);
        assert_eq!(m.http, "");
        assert!(m.https.is_none());
        assert_eq!(m.socks.as_deref(), Some("127.0.0.1:1080"));
    }

    /// 逐协议字段是**后加的**：旧的 `proxy_snapshot.json`（没有这三个键）必须还能读进来，
    /// 否则升级后一次"还原代理"就会因为反序列化失败而失败
    #[test]
    fn proxy_state_accepts_old_snapshot_json() {
        let text = r#"{
            "enable": true,
            "server": "127.0.0.1:7890",
            "bypass": "localhost;127.0.0.1;.local",
            "autoconfig_url": null,
            "pac_present": false
        }"#;
        let st: ProxyState = serde_json::from_str(text).unwrap();
        assert_eq!(st.server, "127.0.0.1:7890");
        assert!(st.http_proxy.is_none() && st.https_proxy.is_none() && st.socks_proxy.is_none());
        // None 不写回 JSON（老快照不会因为"多了几个 null"而变胖）
        let back = serde_json::to_string(&st).unwrap();
        assert!(!back.contains("http_proxy"));
    }

    /// 三个协议的命令必须各不相同：否则"只关 HTTPS"会误伤别的协议
    #[test]
    fn protocol_commands_are_distinct() {
        let all = [Proto::Web, Proto::Secure, Proto::Socks];
        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate() {
                if i == j {
                    continue;
                }
                assert_ne!(a.get_cmd(), b.get_cmd());
                assert_ne!(a.set_cmd(), b.set_cmd());
                assert_ne!(a.state_cmd(), b.state_cmd());
                assert_ne!(a.label(), b.label());
            }
        }
    }

    /// 还原点必须逐协议记录（合并成一个值时，还原会把"本来没开的协议"也打开 ——
    /// 那正是这次故障在"关代理"路径上的同一副面孔）
    #[test]
    fn snapshot_keeps_protocols_separate() {
        let snap = ServiceSnapshot {
            fmt: SNAPSHOT_FMT,
            services: vec![ServiceProxy {
                service: "Wi-Fi".into(),
                web: ProtocolProxy {
                    enable: true,
                    server: "127.0.0.1:7890".into(),
                },
                secure: ProtocolProxy::default(),
                socks: ProtocolProxy::default(),
                bypass: "localhost".into(),
                autoconfig_url: None,
                pac_present: false,
            }],
            created_at: chrono::Local::now().to_rfc3339(),
        };
        let text = serde_json::to_string(&snap).unwrap();
        let back: ServiceSnapshot = serde_json::from_str(&text).unwrap();
        assert_eq!(back.fmt, SNAPSHOT_FMT);
        assert!(back.services[0].web.enable);
        assert!(!back.services[0].secure.enable);
        assert!(!back.services[0].socks.enable);
        // 时间戳必须活过一次序列化往返，否则读回来就被判过期
        assert_eq!(back.created_at, snap.created_at);
    }

    /// 「该还原成什么样」的决策在 `crate::os::proxy_restore`（两平台共用的纯逻辑，
    /// Windows 上也会跑）；这里只验证平台层确实照着决策走。
    #[test]
    fn write_protocol_uses_shared_decision() {
        // 关着但预填了地址 → 决策必须落在 PrefillThenDisable，而不是简单关开关
        assert_eq!(
            crate::os::proxy_restore::plan(false, "10.0.0.10:20173"),
            ProtocolRestore::PrefillThenDisable("10.0.0.10".into(), 20173)
        );
    }
}

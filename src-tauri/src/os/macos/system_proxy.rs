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
//!     **三个独立开关** ⇒ 这里只写 HTTP 一项（见 [`MANUAL_PROTO`] 的说明），
//!     HTTPS / SOCKS 只在"清残留"时被动关掉
//!   - PAC 由 `-setautoproxyurl` / `-setautoproxystate` 管理（手动写入不动 PAC，同 Windows）

use crate::error::{AppError, Result, E3003, E3004};
use serde::{Deserialize, Serialize};

use super::{dns_client, net};

pub const DEFAULT_BYPASS: &str = "localhost;127.*;<local>";

/// 代理快照（与 Windows 同形，便于配置文件互通）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct ProxyState {
    pub enable: bool,
    pub server: String,
    pub bypass: String,
    pub autoconfig_url: Option<String>,
    /// PAC 存在时优先级高于手动代理
    pub pac_present: bool,
}

impl ProxyState {
    /// 手动代理是否真的在生效（PAC 存在时手动代理会被覆盖）
    pub fn is_on(&self) -> bool {
        self.enable && !self.pac_present
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

/// 「手动代理」写入时**只写 HTTP 这一项**（2026-10-10 用户实测修正）。
///
/// 原先三项都指到同一个 `host:port`：只支持 HTTP 的代理端口会收到 SOCKS / HTTPS 请求，
/// 结果是一开代理就断网。HTTP 一项已经覆盖绝大多数场景 —— CFNetwork 在没有
/// Secure Web Proxy 时，HTTPS 请求也会走 HTTP 代理（CONNECT）。
///
/// 想支持"SOCKS 走另一个端口"之类的玩法，就得先把方案里的代理地址拆成逐协议配置；
/// 在那之前，多写协议只会制造上面那个故障。
const MANUAL_PROTO: Proto = Proto::Web;

/// 写入手动代理时**必须顺手关掉**的两项。
///
/// 旧版本（≤ 0.0.2）把 HTTPS / SOCKS 也指到了同一个端口：升级后不清掉的话，残留设置
/// 会继续把 HTTPS / SOCKS 流量送去那个端口（用户看到的现象就是"升级了还是上不了网"）。
const MANUAL_CLEAR: &[Proto] = &[Proto::Secure, Proto::Socks];

/// 关闭手动代理时三项全关 —— 同样是为了清掉旧版本 / 用户手工留下的三项
const MANUAL_OFF: &[Proto] = &[Proto::Web, Proto::Secure, Proto::Socks];

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
}

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

/// 主服务（`-listnetworkserviceorder` 里第一个未停用的服务）
fn primary_service() -> Result<String> {
    let list = dns_client::list_interfaces()?;
    list.iter()
        .find(|i| i.has_default_gateway && i.status != "Disabled")
        .or_else(|| list.iter().find(|i| i.status != "Disabled"))
        .or_else(|| list.first())
        .map(|i| i.alias.clone())
        .ok_or_else(|| AppError::coded(E3003).with_detail("找不到主网络服务"))
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

    let server = if http_enable {
        join_host_port(get("HTTPProxy"), get("HTTPPort"))
    } else if https_enable {
        join_host_port(get("HTTPSProxy"), get("HTTPSPort"))
    } else if socks_enable {
        join_host_port(get("SOCKSProxy"), get("SOCKSPort"))
    } else {
        String::new()
    };

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
    })
}

fn join_host_port(host: Option<String>, port: Option<String>) -> String {
    match (host, port) {
        (Some(h), Some(p)) if !h.is_empty() && !p.is_empty() => format!("{h}:{p}"),
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
    let auto = parse_proxy_block(
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
        autoconfig_url: if auto.1.is_empty() {
            None
        } else {
            Some(auto.1.clone())
        },
        pac_present: auto.0 && !auto.1.is_empty(),
    })
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

/// 只写入手动代理（不动 PAC）：**只写 HTTP 一项**（见 [`MANUAL_PROTO`]），
/// 并顺手清掉旧版本留下的 HTTPS / SOCKS 残留。对所有未停用服务生效。
pub fn write_manual(enable: bool, server: &str, bypass: &str) -> Result<()> {
    let services = active_services()?;
    snapshot_services(&services)?; // 先留还原点
    for svc in &services {
        write_service_manual(svc, enable, server, bypass)?;
    }
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
        return match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        };
    }
    // 没有还原点：按与 write_manual 相同的规则落到所有服务（只写 HTTP），PAC 单独回填
    let services = active_services()?;
    for svc in &services {
        write_service_manual(svc, state.enable, &state.server, &state.bypass)?;
        write_pac(svc, state.autoconfig_url.as_deref(), state.pac_present)?;
    }
    Ok(())
}

/// 彻底清零：三个代理全关 + PAC 关（保留例外列表 —— 它是绕过名单，清掉反而更糟）
pub fn hard_clear() -> Result<()> {
    let services = active_services()?;
    for svc in &services {
        for p in MANUAL_OFF {
            set_proxy_state(svc, *p, false)?;
        }
        let _ = net::networksetup_soft(&[
            "-setautoproxystate".to_string(),
            svc.clone(),
            "off".to_string(),
        ]);
    }
    invalidate_cache();
    Ok(())
}

/// 手动代理：启用时**只写 HTTP**，关闭时三项全关。
///
/// 两种情况下都会顺手关掉"不该开"的项：启用时清掉旧版本留下的 HTTPS / SOCKS 残留
/// （否则它们会把流量继续送去那个端口），关闭时把三项一起清掉（用户手工设过、或用过旧版本）。
fn write_service_manual(service: &str, enable: bool, server: &str, bypass: &str) -> Result<()> {
    if enable {
        let (host, port) = parse_server(server).ok_or_else(|| {
            AppError::coded(E3003).with_detail(format!("代理地址格式不正确：{server}"))
        })?;
        set_proxy(service, MANUAL_PROTO, &host, port)?;
    }
    for p in if enable { MANUAL_CLEAR } else { MANUAL_OFF } {
        set_proxy_state(service, *p, false)?;
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
fn write_service_full(p: &ServiceProxy) -> Result<()> {
    for (proto, want) in [
        (Proto::Web, &p.web),
        (Proto::Secure, &p.secure),
        (Proto::Socks, &p.socks),
    ] {
        write_protocol(&p.service, proto, want)?;
    }
    write_pac(&p.service, p.autoconfig_url.as_deref(), p.pac_present)?;
    write_exceptions(&p.service, &p.bypass)
}

/// 把某个协议恢复成还原点里的样子：开着就写回服务器，关着就显式关掉
fn write_protocol(service: &str, proto: Proto, want: &ProtocolProxy) -> Result<()> {
    if !want.enable {
        return set_proxy_state(service, proto, false);
    }
    let (host, port) = parse_server(&want.server).ok_or_else(|| {
        AppError::coded(E3003).with_detail(format!(
            "还原点里的 {} 代理地址不合法：{}",
            proto.label(),
            want.server
        ))
    })?;
    set_proxy(service, proto, &host, port)
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

fn snapshot_services(services: &[String]) -> Result<()> {
    // 只在还没有还原点时留一份：开着代理再写一次、最后关闭，都不该把"接管前"的那份
    // 覆盖成"接管后"的状态 —— 否则还原出来的是我们自己的代理配置（与 app 层的
    // `ensure_proxy_snapshot` 同一约定）。
    if snapshot_path().exists() {
        return Ok(());
    }
    let mut list = Vec::new();
    for svc in services {
        match read_service(svc) {
            Ok(p) => list.push(p),
            Err(e) => log::warn!(target: "proxy", "读取服务 {svc} 的代理设置失败（跳过）：{e}"),
        }
    }
    let snap = ServiceSnapshot {
        fmt: SNAPSHOT_FMT,
        services: list,
    };
    let text = serde_json::to_string_pretty(&snap)?;
    std::fs::write(snapshot_path(), text.as_bytes())?;
    Ok(())
}

fn take_snapshot() -> Result<Option<Vec<ServiceProxy>>> {
    let path = snapshot_path();
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)?;
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

/// 回读校验
pub fn verify(expect_enable: bool) -> Result<bool> {
    let st = read()?;
    if expect_enable {
        Ok(st.enable)
    } else {
        Ok(!st.enable && !st.pac_present)
    }
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

/// 解析 "127.0.0.1:7890" 形式；兼容 "http=host:port;https=host:port"
pub fn parse_server(server: &str) -> Option<(String, u16)> {
    let s = server.trim();
    if s.is_empty() {
        return None;
    }
    let first = s.split(';').next().unwrap_or(s);
    let seg = first.rsplit('=').next().unwrap_or(first);
    let (host, port) = seg.rsplit_once(':')?;
    let port: u16 = port.trim().parse().ok()?;
    if host.trim().is_empty() {
        return None;
    }
    Some((host.trim().to_string(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// 回归（2026-10-10 用户实测）：手动代理**只许写 HTTP 一项**。
    /// 旧版本把三项都指到同一个 host:port —— 只支持 HTTP 的代理端口会收到 SOCKS 请求，
    /// 一开代理就断网。
    #[test]
    fn manual_proxy_only_writes_http() {
        assert_eq!(MANUAL_PROTO, Proto::Web);
        assert_eq!(MANUAL_PROTO.set_cmd(), "-setwebproxy");
        // 启用时要顺手关掉的，正是旧版本多写的那两项
        assert_eq!(MANUAL_CLEAR, [Proto::Secure, Proto::Socks].as_slice());
        // 关闭时三项全关（旧版本 / 手工设置的残留也要清掉）
        assert_eq!(MANUAL_OFF.len(), 3);
        assert!(MANUAL_OFF.contains(&Proto::Web));
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
        };
        let text = serde_json::to_string(&snap).unwrap();
        let back: ServiceSnapshot = serde_json::from_str(&text).unwrap();
        assert_eq!(back.fmt, SNAPSHOT_FMT);
        assert!(back.services[0].web.enable);
        assert!(!back.services[0].secure.enable);
        assert!(!back.services[0].socks.enable);
    }
}

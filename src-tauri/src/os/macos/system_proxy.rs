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
//!   - WinINET 的 `ProxyServer` 一个值服务所有协议 ⇒ macOS 显式设置 HTTP/HTTPS/SOCKS 三项
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

/// 单个网络服务的代理设置（"所有服务"写入前先逐个快照，还原时才不会张冠李戴）
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct ServiceProxy {
    service: String,
    enable: bool,
    server: String,
    bypass: String,
    autoconfig_url: Option<String>,
    pac_present: bool,
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
    let web = parse_proxy_block(&net::networksetup(&[
        "-getwebproxy".to_string(),
        service.to_string(),
    ])?);
    let secure = parse_proxy_block(&net::networksetup(&[
        "-getsecurewebproxy".to_string(),
        service.to_string(),
    ])?);
    let socks = parse_proxy_block(&net::networksetup(&[
        "-getsocksfirewallproxy".to_string(),
        service.to_string(),
    ])?);
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

    let enable = web.0 || secure.0 || socks.0;
    let server = if !web.1.is_empty() {
        web.1.clone()
    } else if !secure.1.is_empty() {
        secure.1.clone()
    } else {
        socks.1.clone()
    };
    Ok(ServiceProxy {
        service: service.to_string(),
        enable,
        server,
        bypass,
        autoconfig_url: if auto.1.is_empty() {
            None
        } else {
            Some(auto.1.clone())
        },
        pac_present: auto.0 && !auto.1.is_empty(),
    })
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

/// 只写入手动代理三件套（不动 PAC）。对所有未停用服务生效。
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
    // 没有还原点：把给定状态写到所有服务（与 write_manual 对称）
    let services = active_services()?;
    for svc in &services {
        write_service_full(&ServiceProxy {
            service: svc.clone(),
            enable: state.enable,
            server: state.server.clone(),
            bypass: state.bypass.clone(),
            autoconfig_url: state.autoconfig_url.clone(),
            pac_present: state.pac_present,
        })?;
    }
    Ok(())
}

/// 彻底清零：三个代理全关 + PAC 关（保留例外列表 —— 它是绕过名单，清掉反而更糟）
pub fn hard_clear() -> Result<()> {
    let services = active_services()?;
    for svc in &services {
        for set in [
            "-setwebproxystate",
            "-setsecurewebproxystate",
            "-setsocksfirewallproxystate",
        ] {
            net::networksetup(&[set.to_string(), svc.clone(), "off".to_string()])?;
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

fn write_service_manual(service: &str, enable: bool, server: &str, bypass: &str) -> Result<()> {
    if enable {
        let (host, port) = parse_server(server).ok_or_else(|| {
            AppError::coded(E3003).with_detail(format!("代理地址格式不正确：{server}"))
        })?;
        for cmd in [
            "-setwebproxy",
            "-setsecurewebproxy",
            "-setsocksfirewallproxy",
        ] {
            net::networksetup(&[
                cmd.to_string(),
                service.to_string(),
                host.clone(),
                port.to_string(),
            ])
            .map_err(|e| {
                AppError::coded(E3003).with_detail(format!("{cmd} 失败（{service}）：{e}"))
            })?;
        }
    } else {
        for cmd in [
            "-setwebproxystate",
            "-setsecurewebproxystate",
            "-setsocksfirewallproxystate",
        ] {
            net::networksetup(&[cmd.to_string(), service.to_string(), "off".to_string()])?;
        }
    }
    write_exceptions(service, bypass)
}

fn write_service_full(p: &ServiceProxy) -> Result<()> {
    write_service_manual(&p.service, p.enable, &p.server, &p.bypass)?;
    match p.autoconfig_url.as_ref().filter(|u| !u.trim().is_empty()) {
        Some(url) => {
            net::networksetup(&[
                "-setautoproxyurl".to_string(),
                p.service.clone(),
                url.clone(),
            ])?;
            let state = if p.pac_present { "on" } else { "off" };
            net::networksetup(&[
                "-setautoproxystate".to_string(),
                p.service.clone(),
                state.to_string(),
            ])?;
        }
        None => {
            let _ = net::networksetup_soft(&[
                "-setautoproxystate".to_string(),
                p.service.clone(),
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
    let mut list = Vec::new();
    for svc in services {
        match read_service(svc) {
            Ok(p) => list.push(p),
            Err(e) => log::warn!(target: "proxy", "读取服务 {svc} 的代理设置失败（跳过）：{e}"),
        }
    }
    let text = serde_json::to_string_pretty(&list)?;
    std::fs::write(snapshot_path(), text.as_bytes())?;
    Ok(())
}

fn take_snapshot() -> Result<Option<Vec<ServiceProxy>>> {
    let path = snapshot_path();
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)?;
    let list: Vec<ServiceProxy> = match serde_json::from_str(&text) {
        Ok(l) => l,
        Err(e) => {
            log::warn!(target: "proxy", "代理还原点解析失败（按无还原点处理）：{e}");
            let _ = std::fs::remove_file(&path);
            return Ok(None);
        }
    };
    let _ = std::fs::remove_file(&path);
    if list.is_empty() {
        Ok(None)
    } else {
        Ok(Some(list))
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
}

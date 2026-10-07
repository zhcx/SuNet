//! 连通性自检（设计方案 §3.4 —— 硬性需求）
//!
//! 没有这一条，代理开关会造成全网中断：代理开着但代理软件没运行 =
//! 所有遵循系统代理的应用瞬间失联，而用户往往反应不过来是本工具干的。
//!
//! 注意：探测**不能**放进 hosts/DNS 写入的临界区（§7.2 第 3 点），
//! 否则一次 800ms 的探测会把写操作阻塞近一秒，用户侧表现为"点了没反应"。

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// 默认探测超时（§3.4：800ms）
pub const PROXY_PROBE_TIMEOUT: Duration = Duration::from_millis(800);
/// 开启后的外网复检超时（§3.4 第 4 步：5 秒内不通 → 提示）
pub const INTERNET_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// 主机名合法性（本工具面向本机用户，这里只做形状校验，
/// 防止把奇怪的字符串塞进解析器与后续脚本）
fn host_shape_ok(host: &str) -> bool {
    let h = host.trim();
    if h.is_empty() || h.len() > 253 {
        return false;
    }
    if h.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    h.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        && !h.starts_with('-')
}

/// TCP 探测：连得上即视为端口在监听
pub fn tcp_probe(host: &str, port: u16, timeout: Duration) -> bool {
    if !host_shape_ok(host) || port == 0 {
        return false;
    }
    let deadline = Instant::now() + timeout;
    let addrs: Vec<SocketAddr> = match (host, port).to_socket_addrs() {
        Ok(it) => it.collect(),
        Err(_) => return false,
    };
    for addr in addrs {
        let remain = deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            return false;
        }
        if TcpStream::connect_timeout(&addr, remain.min(timeout)).is_ok() {
            return true;
        }
    }
    false
}

/// 代理前置探测（§3.4 第 2 步）
pub fn probe_proxy(host: &str, port: u16) -> bool {
    tcp_probe(host, port, PROXY_PROBE_TIMEOUT)
}

/// 外网可达性复检（§3.4 第 4 步）：任一目标连通即视为通
pub fn probe_internet() -> bool {
    const TARGETS: &[(&str, u16)] = &[("1.1.1.1", 443), ("223.5.5.5", 443)];
    for (h, p) in TARGETS {
        if tcp_probe(h, *p, INTERNET_PROBE_TIMEOUT / 2) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// DNS 测速（§11 M3 验收项 / §12 风险 9c：写入前先探测该地址是否响应）
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, Clone, Debug)]
pub struct DnsTestResult {
    pub server: String,
    pub family: String,
    pub ok: bool,
    pub latency_ms: u128,
    pub message: String,
}

/// 极简 DNS 查询：只构造一条标准查询并等回应，用于测响应与往返时延。
/// 不解析响应内容（本工具不做解析器），只校验事务 ID 与 RCODE。
pub fn dns_latency(server: &str, name: &str, qtype: u16, timeout: Duration) -> std::result::Result<u128, String> {
    let ip: std::net::IpAddr = server
        .trim()
        .parse()
        .map_err(|_| "地址非法".to_string())?;
    let bind = if ip.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
    let sock = std::net::UdpSocket::bind(bind).map_err(|e| format!("无法创建套接字({e})"))?;
    sock.set_read_timeout(Some(timeout))
        .map_err(|e| format!("设置超时失败({e})"))?;

    let id_bytes = uuid::Uuid::new_v4();
    let id = u16::from_be_bytes([id_bytes.as_bytes()[0], id_bytes.as_bytes()[1]]);
    let mut q: Vec<u8> = Vec::with_capacity(64);
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0x01, 0x00]); // RD
    q.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    q.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // AN / NS / AR
    for label in name.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err("查询名非法".into());
        }
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&qtype.to_be_bytes()); // A=1 / AAAA=28
    q.extend_from_slice(&1u16.to_be_bytes()); // IN

    let start = Instant::now();
    sock.send_to(&q, std::net::SocketAddr::new(ip, 53))
        .map_err(|e| format!("发送失败({e})"))?;
    let mut resp = [0u8; 1024];
    let n = sock
        .recv(&mut resp)
        .map_err(|_| "无响应（超时）".to_string())?;
    let elapsed = start.elapsed().as_millis();
    if n < 12 {
        return Err("响应过短".into());
    }
    if resp[0..2] != id.to_be_bytes() {
        return Err("事务 ID 不匹配".into());
    }
    let rcode = resp[3] & 0x0f;
    if rcode != 0 {
        return Err(format!("服务器返回错误码 {rcode}"));
    }
    Ok(elapsed)
}

/// 批量测速：IPv4 用 A 记录、IPv6 用 AAAA 记录
pub fn dns_test(servers: &[String], timeout: Duration) -> Vec<DnsTestResult> {
    const NAME: &str = "example.com";
    let mut out = Vec::new();
    for s in servers {
        let family = match s.trim().parse::<std::net::IpAddr>() {
            Ok(ip) if ip.is_ipv4() => ("v4", 1u16),
            Ok(_) => ("v6", 28u16),
            Err(_) => {
                out.push(DnsTestResult {
                    server: s.clone(),
                    family: "?".into(),
                    ok: false,
                    latency_ms: 0,
                    message: "地址非法".into(),
                });
                continue;
            }
        };
        match dns_latency(s, NAME, family.1, timeout) {
            Ok(ms) => out.push(DnsTestResult {
                server: s.clone(),
                family: family.0.into(),
                ok: true,
                latency_ms: ms,
                message: format!("{ms} ms"),
            }),
            Err(e) => out.push(DnsTestResult {
                server: s.clone(),
                family: family.0.into(),
                ok: false,
                latency_ms: 0,
                message: e,
            }),
        }
    }
    out
}

/// 解析 + 连通诊断（§8 system_resolve）
#[derive(serde::Serialize, Clone, Debug)]
pub struct ResolveResult {
    pub host: String,
    /// hits_hosts | dns
    pub source: String,
    pub addresses: Vec<String>,
    pub hosts_hit: Option<String>,
    pub ping_ok: bool,
    pub message: String,
}

pub fn resolve_host(host: &str) -> ResolveResult {
    let mut addresses: Vec<String> = Vec::new();
    let mut message = String::new();
    let hosts_hit = match crate::os::hosts_file::read_block_entries() {
        Ok(entries) => entries
            .into_iter()
            .filter(|e| e.enabled)
            .find(|e| e.hostnames().iter().any(|h| h == &host.to_ascii_lowercase()))
            .map(|e| e.ip),
        Err(_) => None,
    };

    match (host, 0u16).to_socket_addrs() {
        Ok(it) => {
            for a in it {
                addresses.push(a.ip().to_string());
            }
        }
        Err(e) => message = format!("解析失败：{e}"),
    }
    let source = if hosts_hit.is_some() {
        "hits_hosts".to_string()
    } else {
        "dns".to_string()
    };
    if hosts_hit.is_none() && addresses.is_empty() {
        message = "未命中 hosts，且 DNS 解析失败".to_string();
    }
    let ping_ok = addresses
        .first()
        .map(|ip| {
            ip.parse::<std::net::IpAddr>()
                .map(|a| tcp_probe(&a.to_string(), 443, Duration::from_millis(1200)))
                .unwrap_or(false)
        })
        .unwrap_or(false);

    ResolveResult {
        host: host.to_string(),
        source,
        addresses,
        hosts_hit,
        ping_ok,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_shape() {
        assert!(host_shape_ok("127.0.0.1"));
        assert!(host_shape_ok("example.com"));
        assert!(!host_shape_ok("a b"));
        assert!(!host_shape_ok("a;calc"));
        assert!(!host_shape_ok(""));
    }

    #[test]
    fn probe_invalid_host_is_false() {
        assert!(!tcp_probe("a;b", 80, Duration::from_millis(50)));
    }
}

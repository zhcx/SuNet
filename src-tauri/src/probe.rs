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

/// 主机名合法性（形状校验；与写前校验共用同一份规则，见
/// [`crate::os::proxy_addr::host_ok`]）。
///
/// 从前这里是独立的一份 `chars().all(alnum | . | - | _)` —— 于是 IPv6 字面量
/// 在写前校验通过了、到探测这一步又被判成非法，用户看到的是"地址非法"却查不出哪一步。
fn host_shape_ok(host: &str) -> bool {
    crate::os::proxy_addr::host_ok(host)
}

/// 带超时的地址解析。
///
/// `to_socket_addrs` 是**阻塞**的，且系统解析器（Windows 的 DNS Client、
/// macOS 的 mDNSResponder）没有可设的超时：域名不通、或本机 DNS 正被改成
/// 一个不可达的地址时，这一步能卡住数秒甚至更久，而调用它的位置是
/// "点开启代理 → 前置自检"，用户侧就是按钮点了不动。
///
/// 把解析丢到一个短命线程里等结果：超时就按"探测失败"返回，让闸门在
/// 预定的 800ms 附近结束，而不是被解析器拖走。线程本身无法被强杀，
/// 但它只是阻塞在系统调用上，不持有任何锁，也不会影响后续操作。
fn resolve_within(host: &str, port: u16, timeout: Duration) -> Option<Vec<SocketAddr>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let target = format!("{host}:{port}");
    std::thread::spawn(move || {
        let addrs = target
            .to_socket_addrs()
            .map(|it| it.collect::<Vec<_>>())
            .unwrap_or_default();
        let _ = tx.send(addrs);
    });
    match rx.recv_timeout(timeout) {
        Ok(addrs) => Some(addrs),
        Err(_) => {
            log::warn!(target: "probe", "解析 {host} 超时（>{timeout:?}），按探测失败处理");
            None
        }
    }
}

/// TCP 探测：连得上即视为端口在监听
pub fn tcp_probe(host: &str, port: u16, timeout: Duration) -> bool {
    if !host_shape_ok(host) || port == 0 {
        return false;
    }
    let deadline = Instant::now() + timeout;
    let Some(addrs) = resolve_within(host, port, timeout) else {
        return false;
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
    dns_latency_at(ip, 53, name, qtype, timeout)
}

/// [`dns_latency`] 的实现体，端口可指定 —— 单测起本地假服务器走真实代码路径，
/// 而不是把查询/校验逻辑在测试里再抄一遍（抄一遍只能证明抄得对）。
fn dns_latency_at(
    ip: std::net::IpAddr,
    port: u16,
    name: &str,
    qtype: u16,
    timeout: Duration,
) -> std::result::Result<u128, String> {
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

    let server = std::net::SocketAddr::new(ip, port);
    let start = Instant::now();
    sock.send_to(&q, server)
        .map_err(|e| format!("发送失败({e})"))?;

    // 只认**来自被探测服务器**的响应：`recv` 会收下任意来源的第一个 UDP 包，
    // 本机若还跑着别的解析器（或上一个用例的迟到响应）就会串台。
    // 同时按截止时间循环，而不是"一个包不对就判失败" —— 偶发的无关报文
    // 不该让一个好地址显示成"无响应"。
    let deadline = start + timeout;
    let mut resp = [0u8; 1024];
    loop {
        let remain = deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            return Err("无响应（超时）".to_string());
        }
        sock.set_read_timeout(Some(remain))
            .map_err(|e| format!("设置超时失败({e})"))?;
        let (n, from) = match sock.recv_from(&mut resp) {
            Ok(v) => v,
            Err(_) => return Err("无响应（超时）".to_string()),
        };
        if from.ip() != ip {
            continue; // 不是我们问的那台服务器，忽略
        }
        let elapsed = start.elapsed().as_millis();
        if n < 12 {
            return Err("响应过短".into());
        }
        if resp[0..2] != id.to_be_bytes() {
            continue; // 迟到的旧事务，继续等
        }
        let rcode = resp[3] & 0x0f;
        if rcode != 0 {
            return Err(format!("服务器返回错误码 {rcode}"));
        }
        return Ok(elapsed);
    }
}

/// 批量测速：IPv4 用 A 记录、IPv6 用 AAAA 记录
pub fn dns_test(servers: &[String], timeout: Duration) -> Vec<DnsTestResult> {
    const NAME: &str = "example.com";
    // 并发探测：串行时 8 个地址 × 1.5s 超时 = 最坏 12 秒，界面一直转圈。
    // 并发后墙钟 ≈ 单个超时。结果按输入顺序归还（join 顺序 = 提交顺序）。
    let handles: Vec<_> = servers
        .iter()
        .map(|s| {
            let s = s.clone();
            std::thread::spawn(move || test_one(&s, NAME, timeout))
        })
        .collect();
    handles
        .into_iter()
        .map(|h| {
            h.join().unwrap_or_else(|_| DnsTestResult {
                server: "（线程异常）".into(),
                family: "?".into(),
                ok: false,
                latency_ms: 0,
                message: "探测线程异常终止".into(),
            })
        })
        .collect()
}

fn test_one(s: &str, name: &str, timeout: Duration) -> DnsTestResult {
    let family = match s.trim().parse::<std::net::IpAddr>() {
        Ok(ip) if ip.is_ipv4() => ("v4", 1u16),
        Ok(_) => ("v6", 28u16),
        Err(_) => {
            return DnsTestResult {
                server: s.to_string(),
                family: "?".into(),
                ok: false,
                latency_ms: 0,
                message: "地址非法".into(),
            };
        }
    };
    match dns_latency(s, name, family.1, timeout) {
        Ok(ms) => DnsTestResult {
            server: s.to_string(),
            family: family.0.into(),
            ok: true,
            latency_ms: ms,
            message: format!("{ms} ms"),
        },
        Err(e) => DnsTestResult {
            server: s.to_string(),
            family: family.0.into(),
            ok: false,
            latency_ms: 0,
            message: e,
        },
    }
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
        // IPv6：探测侧必须与写前校验给同一个答案，否则代理页永远开不起来
        assert!(host_shape_ok("::1"));
        assert!(host_shape_ok("2606:4700:4700::1111"));
    }

    #[test]
    fn unresolvable_host_fails_within_timeout() {
        // 不依赖网络：非法形状在解析之前就被挡下，必须是即时的 false
        let started = Instant::now();
        assert!(!tcp_probe("bad host", 80, Duration::from_millis(50)));
        assert!(started.elapsed() < Duration::from_secs(1));

        // 本地回环上一个几乎不可能有人在监听的端口：探测要给出 false，
        // 且不能因为连接被拒之外的原因拖过超时上限太多
        let started = Instant::now();
        let _ = tcp_probe("127.0.0.1", 9, Duration::from_millis(120));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn probe_invalid_host_is_false() {
        assert!(!tcp_probe("a;b", 80, Duration::from_millis(50)));
    }

    /// 本地起一个假 DNS 服务器，验证 `dns_latency` 的完整往返：
    /// 真正走发送 → 收包 → 校验来源 / 事务 ID / RCODE 的代码路径。
    /// 不需要外网、不碰系统配置，只绑 127.0.0.1 的随机端口。
    #[test]
    fn dns_latency_roundtrip_against_local_server() {
        use std::net::UdpSocket;
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let srv = std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            let (n, from) = server.recv_from(&mut buf).unwrap();
            // 原样回一个"成功"响应：复制事务 ID + 置 QR 位，RCODE=0
            let mut resp = buf[..n].to_vec();
            resp[2] |= 0x80; // QR = response
            resp[3] &= 0xf0; // RCODE = 0
            server.send_to(&resp, from).unwrap();
        });

        let ms = dns_latency_at(
            addr.ip(),
            addr.port(),
            "example.com",
            1,
            Duration::from_secs(1),
        )
        .expect("本地假服务器应当回应");
        srv.join().unwrap();
        assert!(ms < 1000, "往返耗时 {ms} ms 不合理");
    }

    /// 迟到的旧事务（ID 不匹配）必须被跳过，而不是当成失败或当成答案。
    #[test]
    fn dns_latency_ignores_mismatched_transaction_id() {
        use std::net::UdpSocket;
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let srv = std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            let (n, from) = server.recv_from(&mut buf).unwrap();
            // 先灌一个 ID 不对的包（模拟上一个用例的迟到响应）
            let mut stale = buf[..n].to_vec();
            stale[0] ^= 0xff;
            stale[1] ^= 0xff;
            stale[2] |= 0x80;
            stale[3] &= 0xf0;
            server.send_to(&stale, from).unwrap();
            // 再发真正的响应
            let mut resp = buf[..n].to_vec();
            resp[2] |= 0x80;
            resp[3] &= 0xf0;
            server.send_to(&resp, from).unwrap();
        });

        let got = dns_latency_at(
            addr.ip(),
            addr.port(),
            "example.com",
            1,
            Duration::from_secs(2),
        );
        srv.join().unwrap();
        assert!(got.is_ok(), "旧事务不该让探测失败：{got:?}");
    }

    /// 服务器返回错误码时，探测要把 RCODE 带出来，而不是报"超时"。
    #[test]
    fn dns_latency_reports_server_rcode() {
        use std::net::UdpSocket;
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let srv = std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            let (n, from) = server.recv_from(&mut buf).unwrap();
            let mut resp = buf[..n].to_vec();
            resp[2] |= 0x80;
            resp[3] = (resp[3] & 0xf0) | 3; // NXDOMAIN
            server.send_to(&resp, from).unwrap();
        });

        let err = dns_latency_at(
            addr.ip(),
            addr.port(),
            "example.com",
            1,
            Duration::from_secs(1),
        )
        .unwrap_err();
        srv.join().unwrap();
        assert!(err.contains('3'), "错误信息应包含 RCODE 3：{err}");
    }
}

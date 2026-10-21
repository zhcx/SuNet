//! 代理落点（`host:port`）的解析与格式化 —— 两平台共用一份规则
//!
//! 为什么单独成一个模块：落点字符串在至少五处被解释（Windows 注册表回读、
//! macOS `networksetup` 回读、界面表单填充、托盘摘要、回写前的校验），
//! 各写一份 `rsplit_once(':')` 就会在 IPv6 上全部跑偏：
//!
//!   `[::1]:7890` 用 `rsplit_once` 拆出来是 `("[::1]", 7890)`，看着没错，
//!   但 `::1:7890`（不加方括号的系统写法）会被拆成 `("::1", 7890)` —— 端口对、
//!   主机也"像"，问题是 IPv6 地址本身可以以冒号结尾的数字段结尾，
//!   两种写法无法靠"最后一个冒号"区分，必须按方括号规则解析。
//!
//! 规则（与 RFC 3986 的 host:port 一致）：
//!   - `[v6]:port` → 主机 `v6`（去掉方括号）、端口 port
//!   - `[v6]`      → 主机 `v6`、端口缺省 0
//!   - `host:port` → 按**最后一个**冒号拆（`host` 不能含冒号）
//!   - `host`      → 主机 host、端口 0
//!   - 空串 / 只含空白 → None
//!
//! 主机里含冒号但没加方括号（例如用户手填 `::1:7890`）时，按最后一个冒号拆是
//! **唯一**可行的解释；解析出来的"主机"仍是合法 IPv6 文本，写进系统后由系统按
//! 自己的规则处理。这里不替用户猜，只保证不把端口和地址搅在一起。

/// 解析 `host:port` / `[v6]:port` / `host`。端口缺失时给 0（调用方自行判定是否可写）。
pub fn parse(addr: &str) -> Option<(String, u16)> {
    let s = addr.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = s.strip_prefix('[') {
        // `[v6]:port` / `[v6]`
        if let Some((host, tail)) = rest.split_once(']') {
            let host = host.trim();
            if host.is_empty() {
                return None;
            }
            let port = tail.trim().strip_prefix(':').unwrap_or("").trim();
            let port = if port.is_empty() {
                0
            } else {
                port.parse::<u16>().ok()?
            };
            return Some((host.to_string(), port));
        }
        // 有左括号没右括号：不是合法形状
        return None;
    }
    match s.rsplit_once(':') {
        Some((host, port)) => {
            let host = host.trim();
            if host.is_empty() {
                return None;
            }
            let port: u16 = port.trim().parse().ok()?;
            Some((host.to_string(), port))
        }
        None => Some((s.to_string(), 0)),
    }
}

/// 主机形状校验：IP 字面量（v4 / v6）直接放行，域名只收常见的 ASCII 字符。
///
/// 两平台共用（写前校验 `apply::valid_proxy_host` 与探测侧 `probe::host_shape_ok`
/// 曾经各写一份，IPv6 在两处都被拒 —— 填 `::1` 时界面报"地址非法"）。
/// 不是 RFC 级的严格校验：本工具面向本机用户，目的是挡住明显不该进注册表 /
/// `networksetup` 参数的字符串，而不是替系统做 DNS 命名规范检查。
pub fn host_ok(host: &str) -> bool {
    let h = host.trim();
    if h.is_empty() || h.len() > 253 {
        return false;
    }
    if h.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    // 带方括号的 IPv6 也算合法输入（用户从浏览器地址栏复制过来常是这个形状）
    if let Some(inner) = h.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        return inner.parse::<std::net::Ipv6Addr>().is_ok();
    }
    h.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        && !h.starts_with('-')
}

/// 统一成 `[v6]:port` / `host:port` 的写法（写回系统与界面回显共用）。
///
/// 主机本身带冒号（IPv6）时补方括号 —— 系统侧（`networksetup`、注册表回读）
/// 的逐协议段都按这个形状拆，不加括号会被当成"地址里又出现冒号"。
pub fn format(host: &str, port: u16) -> String {
    let h = host.trim();
    if h.contains(':') && !h.starts_with('[') {
        format!("[{h}]:{port}")
    } else {
        format!("{h}:{port}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ipv4_and_hostnames() {
        assert_eq!(parse("127.0.0.1:7890"), Some(("127.0.0.1".into(), 7890)));
        assert_eq!(parse("proxy.local:1080"), Some(("proxy.local".into(), 1080)));
        assert_eq!(parse("  proxy.local:1080  "), Some(("proxy.local".into(), 1080)));
        assert_eq!(parse("proxy.local"), Some(("proxy.local".into(), 0)));
        assert_eq!(parse(""), None);
        assert_eq!(parse("   "), None);
        // 端口非法 → None（调用方按"地址非法"报错，不能悄悄当成 0）
        assert_eq!(parse("127.0.0.1:99999"), None);
        assert_eq!(parse("127.0.0.1:abc"), None);
        assert_eq!(parse(":7890"), None);
    }

    #[test]
    fn parses_bracketed_ipv6() {
        assert_eq!(parse("[::1]:7890"), Some(("::1".into(), 7890)));
        assert_eq!(parse("[::1]"), Some(("::1".into(), 0)));
        assert_eq!(
            parse("[2606:4700:4700::1111]:1080"),
            Some(("2606:4700:4700::1111".into(), 1080))
        );
        // 方括号里不是合法地址也照常拆（不在这一层做地址族校验）
        assert_eq!(parse("[::1]:") , Some(("::1".into(), 0)));
        assert_eq!(parse("[::1"), None);
        assert_eq!(parse("[]:80"), None);
    }

    #[test]
    fn unbracketed_ipv6_splits_on_last_colon() {
        // 没加方括号：最后一个冒号右边是端口，左边整段是地址
        assert_eq!(parse("::1:7890"), Some(("::1".into(), 7890)));
        assert_eq!(
            parse("2606:4700:4700::1111:1080"),
            Some(("2606:4700:4700::1111".into(), 1080))
        );
    }

    #[test]
    fn format_brackets_ipv6_and_roundtrips() {
        assert_eq!(format("127.0.0.1", 7890), "127.0.0.1:7890");
        assert_eq!(format("proxy.local", 80), "proxy.local:80");
        assert_eq!(format("::1", 7890), "[::1]:7890");
        assert_eq!(format("[::1]", 7890), "[::1]:7890");
        // 编码 → 解析闭环：IPv6 补上的方括号必须能被自己读回来
        for (host, port) in [
            ("127.0.0.1", 7890u16),
            ("proxy.local", 1080),
            ("::1", 7890),
            ("2606:4700:4700::1111", 1080),
        ] {
            assert_eq!(parse(&format(host, port)), Some((host.to_string(), port)));
        }
    }

    #[test]
    fn host_shape_accepts_ipv6_and_rejects_injection() {
        // 两个平台层原先各自实现的 host_shape_ok 都把 `::1` 判成非法
        assert!(host_ok("127.0.0.1"));
        assert!(host_ok("::1"));
        assert!(host_ok("2606:4700:4700::1111"));
        assert!(host_ok("[::1]"));
        assert!(host_ok("proxy.local"));
        assert!(host_ok("a-b_c.example"));
        // 明显不该进命令行 / 注册表的形状
        assert!(!host_ok(""));
        assert!(!host_ok("a b"));
        assert!(!host_ok("a;calc"));
        assert!(!host_ok("-leading"));
        assert!(!host_ok("中文.example"));
        assert!(!host_ok("[not-v6]"));
        assert!(!host_ok(&"x".repeat(254)));
    }
}

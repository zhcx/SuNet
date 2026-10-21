//! 代理还原点的时效判定（两平台共用）
//!
//! 还原点记的是"我们接管之前"系统代理的样子。它只有在**我们确实还在接管**
//! 的前提下才成立：app 崩溃、被杀、或有人绕过 app 直接写系统代理，文件都会
//! 一直躺在那里；下一次"关闭代理"按它还原，就会回到很久以前的状态
//! （HANDOFF §6.5 的实测故障：把用户原本开着的公司代理整个关掉）。
//!
//! 判定放在这里而不是平台层，是为了让它能在任一平台上被单测覆盖 ——
//! macOS 侧的真机验证成本太高，纯逻辑不该只有"读过一遍代码"这一种验证。

/// 还原点的最长有效期。
///
/// 取 7 天：正常使用里"开着代理跨周不关"是可能的，但那种情况下系统状态基本
/// 已经被用户自己改过；宁可退回"不按旧还原点覆盖"，也不拿一份来历不明的配置
/// 去替用户做决定。
pub const MAX_AGE_HOURS: i64 = 24 * 7;

/// 还原点是否已经过期（或根本没有时间戳）。
///
/// 以下都算过期：
///   · 解析不了的时间戳（旧格式文件、被手改）
///   · 时间戳在未来（时钟回拨）—— 不可信
///   · 超过 [`MAX_AGE_HOURS`]
pub fn expired(created_at: &str, now: chrono::DateTime<chrono::Local>) -> bool {
    let Ok(at) = chrono::DateTime::parse_from_rfc3339(created_at.trim()) else {
        return true;
    };
    let age = now.signed_duration_since(at.with_timezone(&chrono::Local));
    age.num_seconds() < 0 || age > chrono::Duration::hours(MAX_AGE_HOURS)
}

/// 两个代理状态是否"实际等效"：总开关、逐协议落点、PAC 接管状态。
///
/// 不比 `server` 原始串（平台写法不同）、不比 `bypass`（系统会补默认值）。
///
/// **落点按 (host, port) 语义比，不按字符串比**：同一个地址有 `[::1]:7890`
/// 与 `::1:7890` 两种合法写法（我们自己写出去时补方括号，系统读回来可能不补），
/// 大小写与首尾空白也会变。按字符串比会把"还是我们那份"判成"被改过"，
/// 于是每次关闭代理都丢弃还原点、退回只关开关 —— 用户原配置就还原不回来了。
///
/// 两处调用方：
///   · `apply.rs` 判 app 层还原点（`proxy_snapshot.json`）是否过期；
///   · macOS 平台层判**它自己**那份逐服务还原点（`proxy_prev.json`）是否过期 ——
///     那份文件只有平台层读得到（root helper 的 HOME 与用户不同），所以判定必须
///     能在平台层跑。规则只能有一份，否则两边对"算不算被改过"迟早分叉。
pub fn same_state(
    a: &crate::os::system_proxy::ProxyState,
    b: &crate::os::system_proxy::ProxyState,
) -> bool {
    fn norm(addr: &str) -> (String, u16) {
        match crate::os::proxy_addr::parse(addr) {
            Some((host, port)) => (host.to_ascii_lowercase(), port),
            // 解析不出来就原样留作标记：两边同样解析失败且文本一致时才算相同
            None => (addr.trim().to_ascii_lowercase(), 0),
        }
    }
    let eps = |st: &crate::os::system_proxy::ProxyState| -> Vec<(String, (String, u16))> {
        st.endpoints()
            .into_iter()
            .map(|(label, addr)| (label.to_string(), norm(addr)))
            .collect()
    };
    let pac = |u: Option<&str>| {
        u.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    a.enable == b.enable
        && a.pac_present == b.pac_present
        && pac(a.autoconfig_url.as_deref()) == pac(b.autoconfig_url.as_deref())
        && eps(a) == eps(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_snapshot_is_used_stale_is_not() {
        let now = chrono::Local::now();
        let fresh = (now - chrono::Duration::hours(1)).to_rfc3339();
        let stale = (now - chrono::Duration::hours(MAX_AGE_HOURS + 1)).to_rfc3339();
        assert!(!expired(&fresh, now));
        assert!(expired(&stale, now));
        // 边界：刚好到期不算过期（> 才算），差 1 小时就过期
        let edge = (now - chrono::Duration::hours(MAX_AGE_HOURS)).to_rfc3339();
        assert!(!expired(&edge, now));
    }

    #[test]
    fn unusable_timestamps_are_treated_as_stale() {
        let now = chrono::Local::now();
        // 旧格式文件没有 created_at
        assert!(expired("", now));
        assert!(expired("   ", now));
        assert!(expired("not-a-timestamp", now));
        // 时钟回拨 / 手改到未来
        let future = (now + chrono::Duration::hours(2)).to_rfc3339();
        assert!(expired(&future, now));
    }

    /// 语义比较：平台写法差异不算"被改过"，落点 / 开关 / PAC 任一项变化才算。
    #[test]
    fn same_state_compares_semantics_not_raw_strings() {
        use crate::os::system_proxy::ProxyState;
        let base = ProxyState {
            enable: true,
            server: "127.0.0.1:7890".into(),
            bypass: "localhost".into(),
            autoconfig_url: None,
            pac_present: false,
            http_proxy: Some("127.0.0.1:7890".into()),
            https_proxy: None,
            socks_proxy: Some("127.0.0.1:1080".into()),
        };

        // 原始串 / 大小写 / 首尾空白 / bypass 不同，但实际落点一致 → 同一份状态
        let mut same = base.clone();
        same.server = "http=127.0.0.1:7890;socks=127.0.0.1:1080".into();
        same.bypass = "localhost;127.*;<local>".into();
        same.socks_proxy = Some(" 127.0.0.1:1080 ".into());
        assert!(same_state(&base, &same));

        // IPv6 的两种合法写法是同一个落点
        let mut v6 = base.clone();
        v6.http_proxy = Some("[::1]:7890".into());
        let mut v6_back = v6.clone();
        v6_back.http_proxy = Some("::1:7890".into());
        assert!(same_state(&v6, &v6_back));

        // 端口变了 / 关掉了 / 多出 PAC → 都不是同一份
        let mut other_port = base.clone();
        other_port.http_proxy = Some("127.0.0.1:7891".into());
        assert!(!same_state(&base, &other_port));

        let mut off = base.clone();
        off.enable = false;
        assert!(!same_state(&base, &off));

        let mut pac = base.clone();
        pac.pac_present = true;
        pac.autoconfig_url = Some("http://wpad/wpad.dat".into());
        assert!(!same_state(&base, &pac));
    }
}

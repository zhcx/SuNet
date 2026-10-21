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
}

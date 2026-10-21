//! 「把某个协议恢复成还原点里的样子」的决策逻辑（纯逻辑，两平台共用）
//!
//! 放在这里而不是 macOS 平台层，理由与 [`crate::os::proxy_snapshot`] 相同：
//! 这段逻辑决定了**用户原有的代理配置能不能还原回去**，却在 macOS 上才跑得到，
//! 而开发机是 Windows、交叉编译又被 C 编译器卡住。纯决策抽出来之后，
//! macOS 平台层只剩「照着决策执行 networksetup」，决策本身在任一平台都能单测。

use crate::os::proxy_addr;

/// 还原单个协议要执行的动作
#[derive(Debug, PartialEq, Eq)]
pub enum ProtocolRestore {
    /// 写回服务器并保持开启（`networksetup -set*proxy` 本身就会打开）
    Enable(String, u16),
    /// 先写回预填地址、再把开关关掉
    ///
    /// 系统设置面板允许「开关关着、但地址栏里预填着公司代理」——很常见的形态，
    /// 用户只是临时关掉了它。只执行 `-set*proxystate off` 不会动地址栏，于是我们
    /// 写入时把地址改成测试值、还原时又只关开关，用户原有的预填地址就永久丢了
    /// （开关状态看着"对"，下次手动打开时却连一个不存在的端口）。
    /// HANDOFF §6.5 实测问题 ②。
    PrefillThenDisable(String, u16),
    /// 开关关着、也没有可回填的地址：只关开关
    Disable,
    /// 还原点说开着、地址却解析不出来：数据坏了，不能静默跳过
    Invalid(String),
}

/// 由还原点里的一项（开/关 + 落点串）决定要做什么。
///
/// 落点串兼容 `host:port` 与 `http=host:port` 两种形态（与平台层的
/// `parse_server` 同一口径），端口必须是有效值（0 不算落点）。
pub fn plan(enable: bool, server: &str) -> ProtocolRestore {
    match (enable, parse(server)) {
        (true, Some((host, port))) => ProtocolRestore::Enable(host, port),
        (true, None) => ProtocolRestore::Invalid(server.to_string()),
        (false, Some((host, port))) => ProtocolRestore::PrefillThenDisable(host, port),
        (false, None) => ProtocolRestore::Disable,
    }
}

/// 解析落点：取第一个分段、去掉 `scheme=` 前缀，再按共用规则拆 host / port。
///
/// 主机形状也校验：还原点的内容来自磁盘文件（可能被手改，也可能是旧版本写下的），
/// 不能把一个带空格的串原样交给 `networksetup` 当参数。
fn parse(server: &str) -> Option<(String, u16)> {
    let s = server.trim();
    if s.is_empty() {
        return None;
    }
    let first = s.split(';').next().unwrap_or(s);
    let seg = first.rsplit('=').next().unwrap_or(first);
    let (host, port) = proxy_addr::parse(seg)?;
    (port > 0 && proxy_addr::host_ok(&host)).then_some((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HANDOFF §6.5 实测问题 ②：关着但预填了地址的协议，必须先把地址写回去
    #[test]
    fn disabled_protocol_with_prefilled_address_is_written_back_first() {
        assert_eq!(
            plan(false, "10.0.0.10:20173"),
            ProtocolRestore::PrefillThenDisable("10.0.0.10".into(), 20173)
        );
        // 带 scheme 前缀的旧写法同样认得
        assert_eq!(
            plan(false, "http=10.0.0.10:20173"),
            ProtocolRestore::PrefillThenDisable("10.0.0.10".into(), 20173)
        );
    }

    #[test]
    fn enabled_protocol_is_restored_and_kept_on() {
        assert_eq!(
            plan(true, "10.0.0.10:20173"),
            ProtocolRestore::Enable("10.0.0.10".into(), 20173)
        );
        // IPv6：方括号是字面量语法，不能进主机名
        assert_eq!(
            plan(true, "[::1]:7890"),
            ProtocolRestore::Enable("::1".into(), 7890)
        );
    }

    #[test]
    fn empty_address_only_toggles_the_switch() {
        assert_eq!(plan(false, ""), ProtocolRestore::Disable);
        assert_eq!(plan(false, "   "), ProtocolRestore::Disable);
        // 端口 0 不是可用落点
        assert_eq!(plan(false, "localhost:0"), ProtocolRestore::Disable);
    }

    /// 还原点说「开着」但地址坏了：必须报错，不能静默跳过 ——
    /// 否则会留下一个「显示开着、却没有落点」的状态，用户以为在走代理。
    #[test]
    fn broken_address_on_enabled_protocol_is_an_error() {
        assert!(matches!(plan(true, "no-port"), ProtocolRestore::Invalid(_)));
        assert!(matches!(plan(true, ""), ProtocolRestore::Invalid(_)));
        assert_eq!(
            plan(true, "bad host:7890"),
            ProtocolRestore::Invalid("bad host:7890".into())
        );
    }
}

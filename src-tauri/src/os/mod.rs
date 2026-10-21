//! 系统操作层
//!
//! 本层所有函数都不持有 UI 状态、不依赖 WebView2 上下文——
//! 这是设计方案 §1.4.8「服务端升级路径」得以成立的前提：
//! 日后若要把 hosts/DNS 搬进常驻提权服务，只需替换调用方。
//!
//! **平台分叉**：`windows/` 与 `macos/` 各提供一套同名同签名实现，
//! 上层（apply / task_runner / cmd / tray）只依赖 `crate::os::*`；
//! 平台无关的文本处理（`hosts_file`、`dns_presets`）放在本层根部共用。

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "macos")]
pub(crate) mod macos;

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
compile_error!("SuNet 目前只支持 Windows 与 macOS");

#[cfg(target_os = "windows")]
pub use windows::{
    autostart, dns_client, dock, fs_security, hosts_io, privilege, system_proxy, winapi,
};

/// PowerShell 封装：Windows 的 DNS 写入已改走 netsh（见 dns_client.rs 模块注释），
/// 本模块整体保留未用 —— 留作方案 B 的参考实现与回退路径，勿随手删除。
#[cfg(target_os = "windows")]
#[allow(dead_code, unused_imports)]
pub use windows::ps;

#[cfg(target_os = "macos")]
pub use macos::{dns_client, dock, fs_security, hosts_io, privilege, system_proxy};

pub mod dns_presets;
pub mod hosts_file;
/// 代理落点 `host:port` 的唯一解析 / 格式化实现（含 IPv6 方括号规则）
pub mod proxy_addr;
/// 还原单个协议时要执行的动作（纯逻辑，两平台共用；macOS 平台层照着执行）
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod proxy_restore;
/// 代理还原点的时效判定（两平台共用，纯逻辑）
///
/// 生产代码里只有 macOS 的 `take_snapshot` 调用它（Windows 的还原点时效靠
/// `apply.rs` 的 `same_proxy_state` 判定），但单测在 Windows 上也会跑 ——
/// 所以只对非 macOS 的非测试构建放行 dead_code。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod proxy_snapshot;

use serde::{Deserialize, Serialize};

/// 手动代理的**逐协议**写入内容（跨平台同形，具体语义由各平台的 `system_proxy` 解释）。
///
/// macOS 把 HTTP / HTTPS / SOCKS 当作三个**互相独立**的开关，每个协议各有自己的
/// 落点，所以这里逐协议给值：`None`（或空串）表示**显式关闭该协议**——不是"保持原样"，
/// 用户把 HTTPS 那一行的地址清空就是要关掉它。
///
/// Windows 的 WinINET 用一个 `ProxyServer` 值承载全部协议（分段语法
/// `http=…;https=…;socks=…`，即系统代理设置里逐协议填地址的写法；`socks=` 是
/// 系统对 SOCKS 的原生段名），因此 Windows 对 [`ManualProxy::https`] / [`ManualProxy::socks`]
/// 与 macOS 同语义：有值就编进对应段，`None` = 关闭该协议——
/// 「HTTP 代理与 HTTPS / SOCKS 代理分开配」在两平台上都成立。
/// 只有 HTTP 一个落点时 Windows 仍写裸 `host:port`（与 0.0.1 起的单一落点一致，
/// 旧版还原与第三方工具都认这个形态）。
///
/// `enable = false` 时三个落点全部忽略：语义是"关掉所有协议"，
/// **必须把三项残留一起清掉**（0.0.2 及更早的 macOS 版本会把 HTTPS / SOCKS
/// 也指到同一个端口，只关 HTTP 会留下半开的代理）。
///
/// 序列化时字段名与提权载荷一致（`server` 而不是 `http`）：载荷 `{enable, server, https,
/// socks, bypass}` 从 0.0.2 起就是这个形状，`server` 这个键名不能再动——
/// 常驻 helper 是**安装那一刻**的可执行文件拷贝，没有版本握手，
/// 改名会让旧 helper 收到空地址（见 `crate::helper`）。
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ManualProxy {
    /// 总开关；false = 关闭全部协议（三协议的落点都不看）
    pub enable: bool,
    /// HTTP 落点 `host:port`（提权载荷里叫 `server`；留空 = 关闭该协议）
    ///
    /// IPv6 写 `[::1]:7890`：`proxy_addr::format` 负责补方括号，两边平台层
    /// 都按同一份规则解析（见 [`proxy_addr`]）。
    #[serde(rename = "server")]
    pub http: String,
    /// HTTPS 落点 `host:port`；`None` = 关闭该协议
    #[serde(default)]
    pub https: Option<String>,
    /// SOCKS 落点 `host:port`；`None` = 关闭该协议
    #[serde(default)]
    pub socks: Option<String>,
    /// 绕过列表（`;` 分隔，各平台自己翻译成系统格式）
    pub bypass: String,
}

impl ManualProxy {
    /// 关闭全部协议（清残留用）
    pub fn off(bypass: &str) -> Self {
        Self {
            enable: false,
            http: String::new(),
            https: None,
            socks: None,
            bypass: bypass.to_string(),
        }
    }

    /// 只有 HTTP 一个落点（macOS 的「只写 HTTP」与 Windows 的传统单落点形态）
    ///
    /// 仅测试构造用：生产路径由 `apply.rs` 的 `prepare_proxy` 逐项拼装载荷，
    /// 没有"只可能是 HTTP"的调用点，标 `cfg(test)` 免得在 release 里变成死代码。
    #[cfg(test)]
    pub fn http_only(server: &str, bypass: &str) -> Self {
        Self {
            enable: true,
            http: server.to_string(),
            https: None,
            socks: None,
            bypass: bypass.to_string(),
        }
    }

    /// 启用的协议个数（回读/展示用；Windows 恒为 0 或 1）
    ///
    /// 同 [`Self::http_only`]：只在单测里用来断言载荷形状。
    #[cfg(test)]
    pub fn enabled_protocols(&self) -> usize {
        if !self.enable {
            return 0;
        }
        let mut n = usize::from(!self.http.trim().is_empty());
        n += usize::from(self.https.as_deref().is_some_and(|s| !s.trim().is_empty()));
        n += usize::from(self.socks.as_deref().is_some_and(|s| !s.trim().is_empty()));
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_proxy_serde_uses_server_key() {
        // 提权载荷的键名是跨版本契约（常驻 helper 可能是旧版本），改名即破坏兼容
        let v = serde_json::to_value(ManualProxy::http_only("127.0.0.1:7890", "localhost")).unwrap();
        assert_eq!(v["server"], "127.0.0.1:7890");
        assert!(v["https"].is_null());
        assert!(v["socks"].is_null());
        assert_eq!(v["enable"], true);

        // 旧版 GUI 只发 {enable, server, bypass}，也必须能解析
        let old: ManualProxy = serde_json::from_value(serde_json::json!({
            "enable": true,
            "server": "127.0.0.1:7890",
            "bypass": "localhost"
        }))
        .unwrap();
        assert_eq!(old.http, "127.0.0.1:7890");
        assert!(old.https.is_none() && old.socks.is_none());
        assert_eq!(old.enabled_protocols(), 1);
    }

    #[test]
    fn enabled_protocols_counts_all_three() {
        let p = ManualProxy {
            enable: true,
            http: "127.0.0.1:7890".into(),
            https: Some("127.0.0.1:7891".into()),
            socks: Some("127.0.0.1:7892".into()),
            bypass: String::new(),
        };
        assert_eq!(p.enabled_protocols(), 3);
        assert_eq!(ManualProxy::off("x").enabled_protocols(), 0);
    }
}

//! macOS 平台实现（`crate::os` 的平台层）
//!
//! 与 Windows 侧保持**同名同签名**，上层代码不需要任何 `cfg`。
//! 约定：本层不持有 UI 状态、不依赖 WebView2/WKWebView，只做系统调用与数据读写。

pub mod dns_client;
pub mod fs_security;
pub mod hosts_io;
pub mod net;
pub mod privilege;
pub mod system_proxy;

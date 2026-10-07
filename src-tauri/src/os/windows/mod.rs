//! Windows 平台实现（`crate::os` 的平台层）
//!
//! 上层只依赖 `crate::os::*` 导出的同名同签名接口，因此这里可以整目录替换。
//! 约定：本层**不持有 UI 状态、不依赖 WebView2**，只做系统调用与数据读写
//! —— 这是设计方案 §1.4.8「服务端升级路径」的前提。

pub mod dns_client;
pub mod fs_security;
pub mod hosts_io;
pub mod privilege;
pub mod ps;
pub mod system_proxy;
pub mod winapi;

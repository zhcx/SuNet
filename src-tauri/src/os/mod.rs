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
    autostart, dns_client, fs_security, hosts_io, privilege, system_proxy, winapi,
};

/// PowerShell 封装：Windows 的 DNS 写入已改走 netsh（见 dns_client.rs 模块注释），
/// 本模块整体保留未用 —— 留作方案 B 的参考实现与回退路径，勿随手删除。
#[cfg(target_os = "windows")]
#[allow(dead_code, unused_imports)]
pub use windows::ps;

#[cfg(target_os = "macos")]
pub use macos::{dns_client, fs_security, hosts_io, privilege, system_proxy};

pub mod dns_presets;
pub mod hosts_file;

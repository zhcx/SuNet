//! 系统操作层
//!
//! 本层所有函数都不持有 UI 状态、不依赖 WebView2 上下文——
//! 这是设计方案 §1.4.8「服务端升级路径」得以成立的前提：
//! 日后若要把 hosts/DNS 搬进常驻提权服务，只需替换调用方。

pub mod dns_client;
pub mod dns_presets;
pub mod fs_security;
pub mod hosts_file;
pub mod privilege;
/// PowerShell 封装：DNS 写入已改走 netsh（见 dns_client.rs 模块注释），
/// 本模块整体保留未用 —— 留作方案 B 的参考实现与回退路径，勿随手删除。
#[allow(dead_code)]
pub mod ps;
pub mod winapi;
pub mod wininet;

//! 权限探测（macOS）
//!
//! Windows 用 `TOKEN_ELEVATION` 判管理员；macOS 的对应概念是 **euid == 0**
//! （提权就是"以 root 身份运行"）。
//!
//! 与 Windows 的一处语义差异：`networksetup` 改网络配置需要管理员权限，
//! 但应用本身**不需要**以 root 运行 —— 通过常驻 helper（或 osascript 授权框）
//! 即可完成，因此 `can_write_*` 反映的是"提权通道是否可用"。

use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct PrivilegeState {
    pub is_elevated: bool,
    pub can_write_proxy: bool,
    pub can_write_hosts: bool,
    pub can_write_dns: bool,
}

pub fn is_elevated() -> bool {
    unsafe { libc::geteuid() == 0 }
}

pub fn detect() -> PrivilegeState {
    let elevated = is_elevated();
    // 非 root 时看提权通道：常驻 helper 已安装（无需每次输密码）即可写；
    // 即便没装，也能用 osascript 授权框完成，所以通道始终视为可用（代价是要输密码）。
    let channel = elevated || crate::helper::available() || crate::helper::osascript_available();
    PrivilegeState {
        is_elevated: elevated,
        can_write_proxy: channel,
        can_write_hosts: channel,
        can_write_dns: channel,
    }
}

//! 权限探测（设计方案 §1.4.9）
//!
//! 模型 C 下不需要 SID 比对逻辑（§1.4.6 已整段删除）：代理恒为可写，hosts/DNS 取决于是否提权。

use super::winapi::*;
use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct PrivilegeState {
    pub is_elevated: bool,
    /// 恒为 true —— HKCU 用户 hive 默认可写
    pub can_write_proxy: bool,
    pub can_write_hosts: bool,
    pub can_write_dns: bool,
}

pub fn is_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TokenElevation::default();
        let mut ret_len: DWORD = 0;
        let ok = GetTokenInformation(
            token,
            TOKEN_ELEVATION_CLASS,
            &mut elevation as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<TokenElevation>() as DWORD,
            &mut ret_len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

pub fn detect() -> PrivilegeState {
    let elevated = is_elevated();
    PrivilegeState {
        is_elevated: elevated,
        can_write_proxy: true,
        can_write_hosts: elevated,
        can_write_dns: elevated,
    }
}

//! 文件安全校验：提权载荷的属主 / 重解析点检查（设计方案 §1.4.10 安全约束）
//!
//! 威胁模型：提权子进程以管理员身份读取一个"路径由命令行传入"的文件。
//! 若该路径可被低权限主体预置（符号链接、Everyone 可写目录、属主为低权限组），
//! 就构成一次提权链的入口。因此：
//!   1. 拒绝重解析点（符号链接 / 目录联接），阻断路径替换
//!   2. 拒绝属主为众所周知的低权限主体（Everyone / Users / Authenticated Users / Guests / Anonymous）
//!   3. 记录属主 SID 到日志，便于事后审计
//!
//! 说明：本工具**无法**在子进程内验证"发起用户是谁"（标准用户 + 管理员凭据场景下
//! 子进程身份是另一个账户，两边 SID 天然不同），因此采用"排除低权限属主 + 拒绝重解析点
//! + 随机 uuid 文件名 + 限定 LocalAppData 目录"的组合。这条边界记录在 README 的已知偏差表。

use super::winapi::*;
use crate::error::{AppError, Result, E1004};
use std::path::Path;

const GENERIC_READ: DWORD = 0x8000_0000;
const FILE_SHARE_READ_WRITE: DWORD = 0x0000_0001 | 0x0000_0002;
const OPEN_EXISTING: DWORD = 3;
const INVALID_HANDLE_VALUE: HANDLE = usize::MAX as HANDLE;

/// 众所周知的低权限 SID，出现在载荷文件属主上即视为不可信
const LOW_PRIVILEGE_SIDS: &[&str] = &[
    "S-1-1-0",      // Everyone
    "S-1-5-7",      // Anonymous
    "S-1-5-11",     // Authenticated Users
    "S-1-5-32-545", // BUILTIN\Users
    "S-1-5-32-546", // BUILTIN\Guests
];

pub fn is_reparse_point(path: &Path) -> bool {
    let w = wide(&path.to_string_lossy());
    unsafe {
        let attrs = GetFileAttributesW(w.as_ptr());
        attrs != FILE_ATTRIBUTE_INVALID && (attrs & FILE_ATTRIBUTE_REPARSE_POINT) != 0
    }
}

/// 读取文件属主 SID 字符串（失败返回 None）
pub fn file_owner_sid(path: &Path) -> Option<String> {
    unsafe {
        let w = wide(&path.to_string_lossy());
        let h = CreateFileW(
            w.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut owner: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut sd: *mut std::ffi::c_void = std::ptr::null_mut();
        let rc = GetSecurityInfo(
            h,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut sd,
        );
        let mut result = None;
        if rc == ERROR_SUCCESS && !owner.is_null() {
            let mut sid_str: LPWSTR = std::ptr::null_mut();
            if ConvertSidToStringSidW(owner, &mut sid_str) != 0 {
                result = Some(from_wide_ptr(sid_str));
                LocalFree(sid_str as HANDLE);
            }
        }
        if !sd.is_null() {
            LocalFree(sd as HANDLE);
        }
        CloseHandle(h);
        result
    }
}

/// 校验提权载荷文件是否可信（子进程侧调用，校验不通过一律拒绝执行）
pub fn verify_payload_file(path: &Path) -> Result<()> {
    if !path.is_file() {
        return Err(AppError::coded(E1004).with_detail("载荷文件不存在"));
    }
    // 1. 名称形状：task-<uuid>.in.json（随机 uuid 防止可预测路径被抢占）
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let stem = name
        .strip_prefix("task-")
        .and_then(|s| s.strip_suffix(".in.json"))
        .unwrap_or("");
    if uuid::Uuid::parse_str(stem).is_err() {
        return Err(AppError::coded(E1004).with_detail("载荷文件名不符合 task-<uuid>.in.json"));
    }
    // 2. 目录形状：必须位于 ...\SuNet\ipc\
    let dir_ok = path
        .parent()
        .map(|p| {
            p.file_name()
                .map(|n| n.eq_ignore_ascii_case("ipc"))
                .unwrap_or(false)
                && p.parent()
                    .and_then(|q| q.file_name())
                    .map(|n| n.eq_ignore_ascii_case("SuNet"))
                    .unwrap_or(false)
        })
        .unwrap_or(false);
    if !dir_ok {
        return Err(AppError::coded(E1004).with_detail("载荷文件不在 SuNet\\ipc 目录内"));
    }
    // 3. 拒绝重解析点（文件与父目录）
    if is_reparse_point(path) {
        return Err(AppError::coded(E1004).with_detail("载荷文件是重解析点，已拒绝"));
    }
    if let Some(parent) = path.parent() {
        if is_reparse_point(parent) {
            return Err(AppError::coded(E1004).with_detail("载荷目录是重解析点，已拒绝"));
        }
    }
    // 4. 属主不得是低权限主体
    match file_owner_sid(path) {
        Some(sid) => {
            log::debug!(target: "ipc", "载荷文件属主 SID = {sid}");
            if LOW_PRIVILEGE_SIDS.iter().any(|s| sid.eq_ignore_ascii_case(s)) {
                return Err(AppError::coded(E1004)
                    .with_detail(format!("载荷文件属主为低权限主体 {sid}，已拒绝执行")));
            }
        }
        None => {
            return Err(AppError::coded(E1004).with_detail("无法读取载荷文件属主，已拒绝执行"));
        }
    }
    Ok(())
}

/// 当前进程用户 SID 字符串（诊断用）
pub fn current_user_sid() -> Option<String> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut len: DWORD = 0;
        GetTokenInformation(
            token,
            TOKEN_USER_CLASS,
            std::ptr::null_mut(),
            0,
            &mut len,
        );
        let mut buf = vec![0u8; len.max(64) as usize];
        let ok = GetTokenInformation(
            token,
            TOKEN_USER_CLASS,
            buf.as_mut_ptr() as *mut std::ffi::c_void,
            len,
            &mut len,
        );
        let mut result = None;
        if ok != 0 {
            // TOKEN_USER 首字段即 SID_AND_ATTRIBUTES{ PSID Sid; DWORD Attributes; }
            let sid_ptr = *(buf.as_ptr() as *const *mut std::ffi::c_void);
            let mut sid_str: LPWSTR = std::ptr::null_mut();
            if ConvertSidToStringSidW(sid_ptr, &mut sid_str) != 0 {
                result = Some(from_wide_ptr(sid_str));
                LocalFree(sid_str as HANDLE);
            }
        }
        CloseHandle(token);
        result
    }
}

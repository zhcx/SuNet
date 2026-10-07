//! Windows 平台的 hosts 落盘实现（平台层）
//!
//! 只有两件事属于平台相关：路径、原子替换那一步。
//! 文本处理（区块解析/渲染/校验/备份/回读校验）都在平台无关的
//! `crate::os::hosts_file` 里，macOS 复用同一套逻辑。
//!
//! 要点：
//!   1. 临时文件必须与目标**同目录同卷**，否则 ReplaceFileW/rename 不具原子性
//!   2. `ReplaceFileW` 保留目标原有的 ACL 与文件属性（这是选它而不是
//!      直接覆盖的原因：/etc/hosts 在 Windows 上带继承 ACL，重写会破坏）
//!   3. 替换失败时回退为截断写入（会丢 ACL），必须让调用方知道走了回退
//!      —— `used_fallback` 会写进日志与报告

use crate::error::{AppError, Result, E2001};
use crate::os::winapi::{wide, ReplaceFileW, REPLACEFILE_IGNORE_MERGE_ERRORS};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Windows hosts 路径（`%SystemRoot%\System32\drivers\etc\hosts` 的固定约定）
pub fn hosts_path() -> PathBuf {
    PathBuf::from(r"C:\Windows\System32\drivers\etc\hosts")
}

/// 同目录临时文件路径
fn tmp_path_for(path: &Path) -> PathBuf {
    path.parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hosts.tmp")
}

/// 写临时文件 → 原子替换 → 清理。返回**是否走了回退路径**。
pub fn replace_atomic(path: &Path, bytes: &[u8]) -> Result<bool> {
    let tmp_path = tmp_path_for(path);
    {
        let mut f = std::fs::File::create(&tmp_path)
            .map_err(|e| AppError::coded(E2001).with_detail(format!("创建临时文件失败: {e}")))?;
        f.write_all(bytes)
            .map_err(|e| AppError::coded(E2001).with_detail(format!("写入临时文件失败: {e}")))?;
        f.flush()?;
    }

    let used_fallback = if path.exists() {
        let replaced = unsafe {
            let target = wide(&path.to_string_lossy());
            let replacement = wide(&tmp_path.to_string_lossy());
            ReplaceFileW(
                target.as_ptr(),
                replacement.as_ptr(),
                std::ptr::null(),
                REPLACEFILE_IGNORE_MERGE_ERRORS,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if replaced == 0 {
            let err = crate::os::winapi::last_error_text();
            log::warn!(target: "hosts", "ReplaceFileW 失败（{err}），回退为截断写入");
            fallback_write(path, bytes)?;
            true
        } else {
            false
        }
    } else {
        std::fs::rename(&tmp_path, path)
            .map_err(|e| AppError::coded(E2001).with_detail(format!("重命名临时文件失败: {e}")))?;
        false
    };
    let _ = std::fs::remove_file(&tmp_path);
    Ok(used_fallback)
}

/// 回退写入：直接截断目标文件（丢 ACL/属性，仅在 ReplaceFileW 失败时使用）
fn fallback_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .attributes(FILE_ATTRIBUTE_NORMAL)
        .open(path)
        .map_err(|e| AppError::coded(E2001).with_detail(format!("回退写入打开失败: {e}")))?;
    f.write_all(bytes)
        .map_err(|e| AppError::coded(E2001).with_detail(format!("回退写入失败: {e}")))?;
    f.flush()?;
    Ok(())
}

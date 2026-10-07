//! hosts 文件读写的平台细节（macOS）
//!
//! 与 Windows 的差异：
//!   - 路径固定为 `/etc/hosts`（`/etc` 本身是 `/private/etc` 的符号链接，等价）
//!   - 没有 `ReplaceFileW`，用"同目录临时文件 + rename"实现原子替换
//!   - 换行必须是 `\n`（`\r` 会被解析成主机名的一部分，静默失效）
//!   - 写完必须把属主/权限修回 `root:wheel 0644`（否则 umask 077 之类的环境
//!     会生成 0600 的 /etc/hosts，导致非 root 进程读不到）
//!   - 所有这些都需要 root ⇒ 由提权任务（helper/osascript）执行

use crate::error::{AppError, Result, E2001};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const HOSTS_PATH: &str = "/etc/hosts";

pub fn hosts_path() -> PathBuf {
    PathBuf::from(HOSTS_PATH)
}

/// 原子替换：同目录写临时文件 → `rename` → 修正属主与权限
///
/// 返回 `Ok(true)` 表示走了"回退写入"路径；macOS 只有一种写路径，恒为 `false`
/// （保留该返回值是为了与 Windows 版保持同签名）。
pub fn replace_atomic(path: &Path, bytes: &[u8]) -> Result<bool> {
    let dir = path
        .parent()
        .ok_or_else(|| AppError::coded(E2001).with_detail("hosts 路径没有父目录"))?;
    let tmp = dir.join(format!("hosts.tmp.{}", std::process::id()));

    let write_tmp = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    };
    write_tmp().map_err(|e| {
        AppError::coded(E2001).with_detail(format!(
            "写临时文件 {} 失败：{e}（需要管理员权限）",
            tmp.display()
        ))
    })?;

    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(AppError::coded(E2001).with_detail(format!(
            "替换 {} 失败：{e}（需要管理员权限）",
            path.display()
        )));
    }

    fix_ownership(path);
    Ok(false)
}

/// 把 /etc/hosts 的属主与权限修正为 `root:wheel 0644`
fn fix_ownership(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let c = match std::ffi::CString::new(path.as_os_str().as_bytes()) {
        Ok(c) => c,
        Err(_) => return, // 路径含 NUL，不可能出现在真实路径里
    };
    unsafe {
        // wheel 的 gid = 0
        if libc::chown(c.as_ptr(), 0, 0) != 0 {
            log::warn!(target: "hosts", "chown({}) 失败（errno {}）", path.display(), *libc::__error());
        }
        if libc::chmod(c.as_ptr(), 0o644) != 0 {
            log::warn!(target: "hosts", "chmod({}) 失败（errno {}）", path.display(), *libc::__error());
        }
    }
}

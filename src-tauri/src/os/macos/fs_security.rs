//! 文件安全校验（macOS）
//!
//! 威胁模型与 Windows 版一致：提权子进程（root）要读取"路径由命令行传入"的载荷文件，
//! 攻击者若能提前放下一个同名文件（或符号链接指向别处），就能让 root 去读它。
//! 因此校验项的 macOS 对应实现是：
//!   1. 文件名形状 `task-<uuid>.in.json`（uuid 随机，路径不可预测）
//!   2. 所在目录末两段必须是 `SuNet/ipc`（大小写不敏感）
//!   3. 文件与父目录都不能是符号链接（Windows 的"重解析点"）
//!   4. 父目录不能对其他人可写（否则攻击者可以在里面放文件）
//!   5. 文件属主必须与父目录属主一致（= 调用者自己），取不到属主一律拒绝

use crate::error::{AppError, Result, E1004};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// 符号链接判定（对应 Windows 的重解析点检查）
pub fn is_reparse_point(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// 属主标识；Windows 返回 SID 字符串，macOS 用 `uid:<n>`
pub fn file_owner_sid(path: &Path) -> Option<String> {
    std::fs::symlink_metadata(path)
        .ok()
        .map(|m| format!("uid:{}", m.uid()))
}

pub fn current_user_sid() -> Option<String> {
    Some(format!("uid:{}", unsafe { libc::geteuid() }))
}

/// 校验提权载荷文件是否可信；不通过返回 E1004
pub fn verify_payload_file(path: &Path) -> Result<()> {
    let deny = |why: &str| AppError::coded(E1004).with_detail(format!("载荷文件校验失败：{why}"));

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| deny("文件名不是 UTF-8"))?;
    let stem = name
        .strip_prefix("task-")
        .and_then(|s| s.strip_suffix(".in.json"))
        .ok_or_else(|| deny("文件名形状不符"))?;
    if uuid::Uuid::parse_str(stem).is_err() {
        return Err(deny("文件名中的 uuid 非法"));
    }

    let parent = path.parent().ok_or_else(|| deny("没有父目录"))?;
    let dir_name = |p: &Path| {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default()
    };
    let grand = parent.parent().ok_or_else(|| deny("没有上级目录"))?;
    if dir_name(parent) != "ipc" || dir_name(grand) != "sunet" {
        return Err(deny("目录不是 SuNet/ipc"));
    }

    if !path.is_file() {
        return Err(deny("不是普通文件"));
    }
    if is_reparse_point(path) {
        return Err(deny("文件是符号链接"));
    }
    if is_reparse_point(parent) {
        return Err(deny("父目录是符号链接"));
    }

    let pmeta = std::fs::metadata(parent).map_err(|e| deny(&format!("读父目录失败：{e}")))?;
    // 其他人可写的目录里可能被塞进别人的文件
    if pmeta.mode() & 0o002 != 0 {
        return Err(deny("父目录对其他人可写"));
    }
    let fmeta = std::fs::metadata(path).map_err(|e| deny(&format!("读文件失败：{e}")))?;
    if fmeta.uid() != pmeta.uid() {
        return Err(deny("文件属主与父目录属主不一致"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_name() {
        let p = std::path::Path::new("/tmp/hosts");
        assert!(verify_payload_file(p).is_err());
    }

    #[test]
    fn accepts_canonical_shape() {
        let dir = std::env::temp_dir().join("SuNet").join("ipc");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("task-{}.in.json", uuid::Uuid::new_v4()));
        std::fs::write(&file, b"{}").unwrap();
        assert!(verify_payload_file(&file).is_ok());
        let _ = std::fs::remove_file(&file);
    }
}

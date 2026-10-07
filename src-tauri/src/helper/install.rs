//! 安装 / 卸载常驻 helper（macOS）
//!
//! 由应用侧通过 `osascript ... with administrator privileges` 调用
//! `SuNet --helper-install`（一次性输密码），之后切换方案不再需要授权。
//!
//! 安装动作（对齐 Apple 的 PrivilegedHelperTools 惯例）：
//!   1. 把当前可执行文件拷到 `/Library/PrivilegedHelperTools/com.sunet.helper`（root:wheel 0755）
//!   2. 写 `/Library/LaunchDaemons/com.sunet.helper.plist`（root:wheel 0644）
//!   3. `launchctl bootout`（清理旧实例）→ `launchctl bootstrap system <plist>`
//!   4. 等 socket 出现，确认真的起来了

use crate::error::{AppError, Result};
use crate::os::macos::net;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

/// LaunchDaemon plist 内容
pub fn plist_xml() -> String {
    let label = crate::helper::LABEL;
    let bin = crate::helper::HELPER_BIN;
    let log = crate::helper::LOG_PATH;
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{bin}</string>
		<string>--helper-daemon</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>ProcessType</key>
	<string>Background</string>
	<key>StandardOutPath</key>
	<string>{log}</string>
	<key>StandardErrorPath</key>
	<string>{log}</string>
</dict>
</plist>
"#
    )
}

/// 安装（需要 root）
pub fn install() -> Result<String> {
    if !crate::os::privilege::is_elevated() {
        return Err(AppError::internal("安装 helper 需要管理员权限"));
    }
    let src = crate::paths::exe_path()?;
    if !src.is_file() {
        return Err(AppError::internal(format!(
            "找不到可执行文件：{}",
            src.display()
        )));
    }

    let bin = Path::new(crate::helper::HELPER_BIN);
    if let Some(dir) = bin.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| AppError::internal(format!("创建 {} 失败：{e}", dir.display())))?;
        set_owner_mode(dir, 0o755)?;
    }
    std::fs::copy(&src, bin)
        .map_err(|e| AppError::internal(format!("拷贝 helper 到 {} 失败：{e}", bin.display())))?;
    set_owner_mode(bin, 0o755)?;

    let plist = Path::new(crate::helper::PLIST_PATH);
    if let Some(dir) = plist.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::write(plist, plist_xml().as_bytes())
        .map_err(|e| AppError::internal(format!("写 {} 失败：{e}", plist.display())))?;
    set_owner_mode(plist, 0o644)?;

    // 清理旧实例：bootout 目标可能不存在，失败忽略
    let _ = net::try_run(
        net::LAUNCHCTL,
        &[
            "bootout".to_string(),
            format!("system/{}", crate::helper::LABEL),
        ],
    );
    let _ = net::try_run(
        net::LAUNCHCTL,
        &[
            "bootout".to_string(),
            "system".to_string(),
            crate::helper::PLIST_PATH.to_string(),
        ],
    );

    let (ok, out, err) = net::try_run(
        net::LAUNCHCTL,
        &[
            "bootstrap".to_string(),
            "system".to_string(),
            crate::helper::PLIST_PATH.to_string(),
        ],
    )?;
    if !ok {
        let detail = if err.trim().is_empty() {
            out.trim().to_string()
        } else {
            err.trim().to_string()
        };
        return Err(AppError::internal(format!(
            "launchctl bootstrap 失败：{detail}"
        )));
    }

    // 等 socket 出现（最多 5 秒）
    for _ in 0..50 {
        if crate::helper::available() {
            return Ok("常驻 helper 已安装并运行，之后切换方案不再需要管理员授权".to_string());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok("helper 已安装，但 socket 未在 5 秒内出现：可能被 macOS 的\
后台任务管理（BTM）拦截，请在「系统设置 → 通用 → 登录项与扩展」中允许；\
在此期间应用仍可用逐次授权的回退通道"
        .to_string())
}

/// 卸载（需要 root）
pub fn uninstall() -> Result<String> {
    if !crate::os::privilege::is_elevated() {
        return Err(AppError::internal("卸载 helper 需要管理员权限"));
    }
    let mut notes = Vec::new();
    if let Ok((ok, _, err)) = net::try_run(
        net::LAUNCHCTL,
        &[
            "bootout".to_string(),
            format!("system/{}", crate::helper::LABEL),
        ],
    ) {
        if !ok {
            notes.push(format!("bootout 返回非零（可忽略）：{}", err.trim()));
        }
    }
    let _ = net::try_run(
        net::LAUNCHCTL,
        &[
            "bootout".to_string(),
            "system".to_string(),
            crate::helper::PLIST_PATH.to_string(),
        ],
    );
    for p in [
        crate::helper::PLIST_PATH,
        crate::helper::HELPER_BIN,
        crate::helper::SOCKET_PATH,
    ] {
        match std::fs::remove_file(p) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => notes.push(format!("删除 {p} 失败：{e}")),
        }
    }
    Ok(if notes.is_empty() {
        "常驻 helper 已卸载".to_string()
    } else {
        format!("常驻 helper 已卸载；{}", notes.join("；"))
    })
}

/// 状态文本（给设置界面/日志用）
pub fn status() -> String {
    let installed = crate::helper::installed();
    let running = crate::helper::available();
    format!(
        "helper：{}；socket：{}",
        if installed { "已安装" } else { "未安装" },
        if running { "在运行" } else { "未运行" }
    )
}

fn set_owner_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| AppError::internal("路径含非法字符"))?;
    let rc = unsafe { libc::chown(c.as_ptr(), 0, 0) }; // root:wheel（wheel gid = 0）
    if rc != 0 {
        log::warn!(target: "helper", "chown({}) 失败", path.display());
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|e| {
        AppError::internal(format!("chmod({}, {mode:o}) 失败：{e}", path.display()))
    })?;
    Ok(())
}

//! 路径约定（设计方案 §0.0 品牌资产清单）
//!
//! Windows：
//!   配置目录 %APPDATA%\SuNet\        —— 配置 / 快照 / 暂存
//!   本地目录 %LOCALAPPDATA%\SuNet\   —— IPC 载荷（提权子进程需要与主进程互通，
//!                                       必须用绝对路径显式传递，见 §1.4.10）
//!
//! macOS（遵循 Apple 的目录约定，便于备份工具与用户查找）：
//!   配置目录 ~/Library/Application Support/SuNet/
//!   本地目录 ~/Library/Caches/SuNet/  —— IPC 载荷
//!   日志目录 ~/Library/Logs/SuNet/

use std::path::PathBuf;

#[cfg(target_os = "windows")]
pub fn appdata_dir() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| {
        let profile = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into());
        format!("{profile}\\AppData\\Roaming")
    });
    PathBuf::from(base).join("SuNet")
}

#[cfg(target_os = "windows")]
pub fn local_appdata_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| {
        let profile = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into());
        format!("{profile}\\AppData\\Local")
    });
    PathBuf::from(base).join("SuNet")
}

#[cfg(target_os = "macos")]
fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

#[cfg(target_os = "macos")]
pub fn appdata_dir() -> PathBuf {
    home_dir().join("Library/Application Support/SuNet")
}

#[cfg(target_os = "macos")]
pub fn local_appdata_dir() -> PathBuf {
    home_dir().join("Library/Caches/SuNet")
}

pub fn config_path() -> PathBuf {
    appdata_dir().join("config.json")
}

pub fn snapshots_dir() -> PathBuf {
    appdata_dir().join("snapshots")
}

pub fn backup_dir() -> PathBuf {
    appdata_dir().join("backups")
}

#[cfg(target_os = "windows")]
pub fn logs_dir() -> PathBuf {
    appdata_dir().join("logs")
}

/// macOS 约定：日志放 ~/Library/Logs/<App>/（Console.app 能直接看到）
#[cfg(target_os = "macos")]
pub fn logs_dir() -> PathBuf {
    home_dir().join("Library/Logs/SuNet")
}

/// macOS 登录自启动（LaunchAgent）标签
#[cfg(target_os = "macos")]
pub const AUTOSTART_LABEL: &str = "com.sunet.desktop.autostart";

/// macOS 用户级 LaunchAgent 目录（与 helper 的 LaunchDaemon 区分：这个不需要 root）
#[cfg(target_os = "macos")]
pub fn launch_agents_dir() -> PathBuf {
    home_dir().join("Library/LaunchAgents")
}

pub fn staging_dir() -> PathBuf {
    appdata_dir().join("staging")
}

pub fn session_marker_path() -> PathBuf {
    appdata_dir().join("session.marker")
}

pub fn proxy_snapshot_path() -> PathBuf {
    appdata_dir().join("proxy_snapshot.json")
}

pub fn ipc_dir() -> PathBuf {
    local_appdata_dir().join("ipc")
}

pub fn ensure_dirs() -> std::io::Result<()> {
    for d in [
        appdata_dir(),
        snapshots_dir(),
        backup_dir(),
        logs_dir(),
        staging_dir(),
        local_appdata_dir(),
        ipc_dir(),
    ] {
        std::fs::create_dir_all(&d)?;
    }
    Ok(())
}

pub fn exe_path() -> std::io::Result<PathBuf> {
    std::env::current_exe()
}

//! 路径约定（设计方案 §0.0 品牌资产清单）
//!
//! 配置目录 %APPDATA%\SuNet\    —— 配置 / 快照 / 暂存
//! 本地目录 %LOCALAPPDATA%\SuNet\ —— IPC 载荷（提权子进程需要与主进程互通，
//!                                   必须用绝对路径显式传递，见 §1.4.10）

use std::path::PathBuf;

pub fn appdata_dir() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| {
        let profile = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into());
        format!("{profile}\\AppData\\Roaming")
    });
    PathBuf::from(base).join("SuNet")
}

pub fn local_appdata_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| {
        let profile = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into());
        format!("{profile}\\AppData\\Local")
    });
    PathBuf::from(base).join("SuNet")
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

pub fn logs_dir() -> PathBuf {
    appdata_dir().join("logs")
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

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
    // 普通 GUI 进程一定有 HOME
    if let Some(h) = std::env::var_os("HOME") {
        let p = PathBuf::from(h);
        if p.is_absolute() {
            return p;
        }
    }
    // launchd 拉起的 root helper **没有 HOME**。从前这里退回 `/tmp`，于是提权身份的
    // 还原点落到了 `/tmp/Library/Application Support/SuNet/proxy_prev.json`
    // （2026-10-21 在本机实测确认：`/tmp` 下真有这一份，属主 root）。后果有三个：
    //   · `/tmp` 会被系统清理 ⇒ 跨重启的还原点直接丢失；
    //   · 它不是用户目录里那份，排查时按文档去 `~/Library/...` 找永远找不到；
    //   · `/tmp` 是 1777，别的本地用户可以先建同名目录把路径占住。
    // 改成用 `getpwuid` 取真实家目录（root → `/var/root`）。写与读都在同一身份下
    // （见 HANDOFF §4.21：不要把还原点改到用户家目录），所以两边仍然一致。
    if let Some(p) = passwd_home() {
        return p;
    }
    PathBuf::from("/tmp")
}

/// 当前**有效身份**的家目录（`getpwuid_r`，线程安全版本）
#[cfg(target_os = "macos")]
fn passwd_home() -> Option<PathBuf> {
    let uid = unsafe { libc::geteuid() };
    let mut buf = vec![0 as libc::c_char; 1024];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut result) };
    if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
        return None;
    }
    let dir = unsafe { std::ffi::CStr::from_ptr(pwd.pw_dir) };
    let s = dir.to_string_lossy();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s.as_ref()))
    }
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    /// launchd 拉起的 root helper 没有 HOME：家目录必须仍能解析出来
    /// （从前退回 `/tmp`，还原点被写到 `/tmp/Library/...` —— 2026-10-21 实测确认）
    #[test]
    fn home_dir_survives_missing_home_env() {
        let home = super::passwd_home().expect("getpwuid_r 应当能取到本用户家目录");
        assert!(
            home.is_absolute(),
            "家目录必须是绝对路径：{}",
            home.display()
        );
        assert_ne!(home, std::path::PathBuf::from("/tmp"));
        // 与 $HOME 一致（普通 GUI 进程两种取法给出同一个答案）
        if let Some(env_home) = std::env::var_os("HOME") {
            assert_eq!(home, std::path::PathBuf::from(env_home));
        }
    }
}

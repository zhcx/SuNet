//! macOS 命令行封装（平台层内部工具，不对外暴露）
//!
//! macOS 不存在 Win32 那样的网络配置 API（要调 SystemConfiguration.framework 得写一堆 FFI），
//! 系统自带的 `networksetup` / `scutil` / `ifconfig` 才是官方支持的稳定入口：
//!   - `networksetup` 读写 DNS 与代理（**改配置需要管理员权限**）
//!   - `scutil --proxy` 读全局代理状态（免权限）
//!   - `ifconfig` 读接口地址与链路状态（免权限）
//!
//! 安全：所有调用都是 argv 直传（`Command::args`），**不经过 shell** ——
//! 因此含空格的服务名（"Built-in Ethernet"）与 `*` 前缀不会被解释成多个参数或通配。

use crate::error::{AppError, Result};
use std::process::Command;

pub const NETWORKSETUP: &str = "/usr/sbin/networksetup";
pub const SCUTIL: &str = "/usr/sbin/scutil";
pub const IFCONFIG: &str = "/sbin/ifconfig";
pub const LAUNCHCTL: &str = "/bin/launchctl";
pub const OSASCRIPT: &str = "/usr/bin/osascript";
pub const SW_VERS: &str = "/usr/bin/sw_vers";
pub const DEFAULTS: &str = "/usr/bin/defaults";
pub const DSCACHEUTIL: &str = "/usr/bin/dscacheutil";
pub const KILLALL: &str = "/usr/bin/killall";
pub const PLUTIL: &str = "/usr/bin/plutil";

/// 运行命令，返回 `(是否成功, stdout, stderr)`；只有「启动不了」才算 Err
pub fn try_run(program: &str, args: &[String]) -> Result<(bool, String, String)> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| AppError::internal(format!("启动 {program} 失败：{e}")))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    ))
}

/// 运行命令并要求成功
pub fn run(program: &str, args: &[String]) -> Result<String> {
    let (ok, stdout, stderr) = try_run(program, args)?;
    if !ok {
        return Err(AppError::internal(format!(
            "{} {} 执行失败：{}",
            short(program),
            args.join(" "),
            first_line(&stderr)
                .or_else(|| first_line(&stdout))
                .unwrap_or_else(|| "无输出".to_string())
        )));
    }
    Ok(stdout)
}

pub fn networksetup(args: &[String]) -> Result<String> {
    run(NETWORKSETUP, args)
}

/// 允许失败的 networksetup（有些查询在"没配置过"时也返回非 0，属于正常分支）
pub fn networksetup_soft(args: &[String]) -> Result<(bool, String)> {
    let (ok, stdout, stderr) = try_run(NETWORKSETUP, args)?;
    if !ok {
        log::debug!(
            target: "os",
            "networksetup {} 退出码非 0：{}",
            args.join(" "),
            first_line(&stderr).unwrap_or_default()
        );
    }
    Ok((ok, stdout))
}

/// `scutil --proxy` 的顶层键值对（扁平字典）
pub fn scutil_proxy() -> Result<Vec<(String, String)>> {
    let text = run(SCUTIL, &["--proxy".to_string()])?;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some((k, v)) = line.split_once(" : ") {
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    Ok(out)
}

pub fn first_line(s: &str) -> Option<String> {
    s.lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .map(|l| l.to_string())
}

/// 路径末段（日志里不必打全路径）
pub fn short(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// 命令是否可用（路径存在）
pub fn available(program: &str) -> bool {
    std::path::Path::new(program).exists()
}

/// 取当前控制台登录用户的 uid（无人登录时返回 None）
///
/// `stat -f %u /dev/console` 是 macOS 上判断"谁坐在机器前"的常用手法，
/// `scutil` 的 `State:/Users/ConsoleUser` 亦可，这里用前者（更少依赖）。
pub fn console_uid() -> Option<u32> {
    let out = std::fs::metadata("/dev/console").ok()?;
    use std::os::unix::fs::MetadataExt;
    Some(out.uid())
}

/// 取用户名（诊断用）
pub fn user_name_of(uid: u32) -> Option<String> {
    let (ok, stdout, _) = try_run("/usr/bin/id", &["-nu".to_string(), uid.to_string()]).ok()?;
    if ok {
        Some(stdout.trim().to_string())
    } else {
        None
    }
}

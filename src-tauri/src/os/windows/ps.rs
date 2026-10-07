//! PowerShell 调用封装（设计方案 §4.1 方案 B）
//!
//! 安全约束（全局安全规则 RULE 2 命令注入）：
//!   - 一律 argv 形式传给 CreateProcessW，不经过 cmd.exe，无 shell 元字符解析
//!   - 所有进入脚本的值都已在上层完成校验：网卡别名白名单（来自枚举结果）、
//!     IP 地址经 `IpAddr::from_str` 解析，序列化回文本必然只含 [0-9a-f:.%]
//!   - 附加防线：`quote_ps` 对单引号加倍，`assert_script_safe` 对最终脚本做一次拒绝式校验
//!   - CREATE_NO_WINDOW：不弹黑窗
//!
//! 注意：DNS 写入已改走 netsh（见 `dns_client.rs` 模块注释），本模块整体保留未用，
//! 留作方案 B 的参考实现与回退路径 —— 所以这里显式允许死代码。

#![allow(dead_code)]

use crate::error::{AppError, Result};
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const PREAMBLE: &str = "$ErrorActionPreference='Stop';\
$ProgressPreference='SilentlyContinue';\
try{[Console]::OutputEncoding=New-Object System.Text.UTF8Encoding $false}catch{};\
$OutputEncoding=[System.Text.Encoding]::UTF8;";

/// PowerShell 单引号字符串字面量转义
pub fn quote_ps(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// 拒绝式校验：脚本里不允许出现 NUL（argv 以 NUL 结束，混入会截断脚本）
///
/// 注入防线不在这一层，而是：
///   1. 脚本作为单个 argv 元素经 CreateProcessW 传递 —— 全程不经过 cmd.exe，无 shell 元字符解析
///   2. 所有插值都已完成校验：网卡别名来自枚举结果的白名单，地址必须能被 `IpAddr::from_str` 解析
///   3. 插值一律经 `quote_ps` 单引号转义
fn assert_script_safe(script: &str) -> Result<()> {
    if script.contains('\0') {
        return Err(AppError::internal("脚本包含 NUL 字符，已拒绝执行"));
    }
    Ok(())
}

pub struct PsResult {
    pub stdout: String,
    pub exit_code: i32,
    pub timed_out: bool,
}

/// 执行 PowerShell 脚本片段，返回 stdout（UTF-8，失败时按 GBK 兜底）
pub fn run_raw(script: &str, timeout: Duration) -> Result<PsResult> {
    assert_script_safe(script)?;
    let full = format!("{PREAMBLE}\n{script}");

    let mut child = Command::new("powershell.exe")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-Command")
        .arg(&full)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| AppError::internal(format!("无法启动 PowerShell: {e}")))?;

    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let exit_code: i32;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_code = status.code().unwrap_or(-1);
                break;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    exit_code = -2;
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(AppError::internal(format!("等待 PowerShell 失败: {e}"))),
        }
    }

    let out_bytes = out_thread.join().unwrap_or_default();
    let err_bytes = err_thread.join().unwrap_or_default();
    let stderr = decode_console(&err_bytes);
    if !stderr.trim().is_empty() {
        log::debug!(target: "ps", "PowerShell stderr: {}", stderr.trim());
    }

    Ok(PsResult {
        stdout: decode_console(&out_bytes),
        exit_code,
        timed_out,
    })
}

/// 控制台输出解码：优先 UTF-8，失败退 GBK（中文 Windows 常见）
fn decode_console(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let (cow, _, _) = encoding_rs::GBK.decode(bytes);
            cow.to_string()
        }
    }
}

/// 在脚本内包一层 try/catch，把异常转成 `__error` 字段，保证 stdout 永远是合法 JSON
fn wrap(script: &str) -> String {
    format!(
        "$__r = try {{ {script} }} catch {{ @{{ __error = ($_.Exception.Message) }} }}; \
$__r | ConvertTo-Json -Depth 8 -Compress"
    )
}

/// 执行返回 JSON 的脚本，并解析为指定类型
pub fn run_json<T: serde::de::DeserializeOwned>(script: &str, timeout: Duration) -> Result<T> {
    let res = run_raw(&wrap(script), timeout)?;
    if res.timed_out {
        return Err(AppError::internal("PowerShell 执行超时"));
    }
    let text = res.stdout.trim().to_string();
    if text.is_empty() {
        return Err(AppError::internal(format!(
            "PowerShell 无输出（退出码 {}）",
            res.exit_code
        )));
    }
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| AppError::internal(format!("解析 PowerShell 输出失败: {e}")))?;
    if let Some(err) = value.get("__error").and_then(|v| v.as_str()) {
        return Err(AppError::internal(err));
    }
    serde_json::from_value(value).map_err(|e| AppError::internal(format!("结果字段不匹配: {e}")))
}

/// PowerShell 单元素数组会被 ConvertTo-Json 展平，这里统一还原为数组语义。
/// 网卡枚举改走 FFI 后仅剩测试在用；保留给后续需要解析数组输出的脚本。
#[cfg_attr(not(test), allow(dead_code))]
pub fn as_array(v: Option<&serde_json::Value>) -> Vec<serde_json::Value> {
    match v {
        Some(serde_json::Value::Array(a)) => a.clone(),
        Some(serde_json::Value::Null) | None => Vec::new(),
        Some(other) => vec![other.clone()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiline_script_is_allowed() {
        // 多行脚本是本项目的正常形态（网卡枚举脚本就是多行的），不能被拒
        assert!(assert_script_safe("$a = 1\n$b = 2\n@{ ok = $true }").is_ok());
    }

    #[test]
    fn nul_is_rejected() {
        assert!(assert_script_safe("$a = \"\0\"").is_err());
    }

    #[test]
    fn quote_ps_escapes_single_quote() {
        assert_eq!(quote_ps("it's"), "'it''s'");
        assert_eq!(quote_ps("以太网"), "'以太网'");
    }

    #[test]
    fn single_element_array_is_normalized() {
        let v: serde_json::Value = serde_json::json!({ "a": { "x": 1 } });
        assert_eq!(as_array(v.get("a")).len(), 1);
        let v2: serde_json::Value = serde_json::json!({ "a": [1, 2] });
        assert_eq!(as_array(v2.get("a")).len(), 2);
        let v3: serde_json::Value = serde_json::json!({ "a": null });
        assert_eq!(as_array(v3.get("a")).len(), 0);
    }
}

//! 提权子进程的任务分派（设计方案 §1.4.4 ①）
//!
//! 硬性约束：子进程**不进 Tauri setup**、不创建窗口、不注册托盘、不加载 WebView2。
//! 分流放在 `main()` 的第一次判断里，这是模型 C 的核心安全边界（§10 提权面）。
//!
//! 载荷是纯数据（JSON），这里只按 `task` 名分派到一张固定函数表，
//! 不做任何动态派发、不执行表达式。

use crate::error::{AppError, Result, E1004};
use crate::ipc::{ApplyStep, Payload, TaskOutput};
use crate::os::hosts_file::{self, HostsEntry};
use crate::os::dns_client::{self, AddressFamily};
use serde_json::Value;

const KNOWN_TASKS: &[&str] = &[
    "hosts_apply",
    "hosts_restore",
    "dns_set",
    "dns_reset",
    "dns_flush",
    "probe",
];

pub struct TaskArgs {
    pub task: String,
    pub in_path: String,
    pub out_path: String,
}

/// main() 第一行调用：解析 --task 参数，命中则说明自己是提权子进程
pub fn parse_task_arg() -> Option<TaskArgs> {
    let args: Vec<String> = std::env::args().collect();
    let mut task = None;
    let mut in_path = None;
    let mut out_path = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--task" if i + 1 < args.len() => {
                task = Some(args[i + 1].clone());
                i += 2;
            }
            "--in" if i + 1 < args.len() => {
                in_path = Some(args[i + 1].clone());
                i += 2;
            }
            "--out" if i + 1 < args.len() => {
                out_path = Some(args[i + 1].clone());
                i += 2;
            }
            _ => i += 1,
        }
    }
    match (task, in_path, out_path) {
        (Some(task), Some(in_path), Some(out_path)) => Some(TaskArgs {
            task,
            in_path,
            out_path,
        }),
        _ => None,
    }
}

/// 提权子进程主流程：执行 → 写结果文件 → 返回退出码
pub fn run_and_exit(args: TaskArgs) -> i32 {
    // 子进程也要记日志（写到提权身份对应的 %APPDATA%，父进程只记载荷/结果路径）
    crate::logging::init(
        &crate::paths::logs_dir(),
        log::LevelFilter::Info,
        true,
        7,
    );
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    log::info!(
        target: "task",
        "提权子进程启动：task={} exe={exe} in={} out={}",
        args.task,
        crate::paths::ipc_dir().display(),
        args.out_path
    );

    let result: Result<TaskOutput> = (|| {
        if !KNOWN_TASKS.contains(&args.task.as_str()) {
            return Err(AppError::coded(E1004).with_detail(format!(
                "未知任务名：{}",
                args.task
            )));
        }
        let payload: Payload = crate::ipc::read_payload(&args.in_path)?;
        if payload.task != args.task {
            return Err(AppError::coded(E1004).with_detail("载荷中的任务名与命令行不一致"));
        }
        Ok(execute(&payload.task, &payload.data))
    })();

    let (output, exit_code) = match result {
        Ok(out) => {
            let code = if out.ok {
                0
            } else {
                out.error.as_ref().map(|e| e.exit_code()).unwrap_or(6)
            };
            (out, code)
        }
        Err(err) => {
            let code = err.exit_code();
            (TaskOutput::fail(err), code)
        }
    };

    if let Err(e) = crate::ipc::write_output(&args.out_path, &output) {
        log::error!(target: "task", "写出结果文件失败：{e}");
    }
    log::info!(target: "task", "提权子进程退出：task={} code={exit_code}", args.task);
    exit_code
}

/// 固定函数表分派（不做动态求值）
pub fn execute(task: &str, data: &Value) -> TaskOutput {
    match task {
        "probe" => TaskOutput::ok().with_step(ApplyStep {
            kind: "probe".into(),
            applied: true,
            verified: true,
            message: format!(
                "提权任务通道正常：已提权={}，用户={}",
                crate::os::privilege::is_elevated(),
                crate::os::fs_security::current_user_sid().unwrap_or_else(|| "未知".into())
            ),
        }),
        "hosts_apply" => hosts_apply(data),
        "hosts_restore" => hosts_restore(data),
        "dns_set" => dns_set(data),
        "dns_reset" => dns_reset(data),
        "dns_flush" => {
            let r = crate::critsec::with_critical_section(|| dns_client::flush());
            match r {
                Ok(_) => TaskOutput::ok().with_step(ok_step("dns", "DNS 缓存已刷新")),
                Err(e) => TaskOutput::fail(e),
            }
        }
        other => TaskOutput::fail(
            AppError::coded(E1004).with_detail(format!("未知任务名：{other}")),
        ),
    }
}

fn ok_step(kind: &str, msg: impl Into<String>) -> ApplyStep {
    ApplyStep {
        kind: kind.to_string(),
        applied: true,
        verified: true,
        message: msg.into(),
    }
}

fn bad_step(kind: &str, msg: impl Into<String>) -> ApplyStep {
    ApplyStep {
        kind: kind.to_string(),
        applied: false,
        verified: false,
        message: msg.into(),
    }
}

fn hosts_apply(data: &Value) -> TaskOutput {
    let entries: Vec<HostsEntry> = match serde_json::from_value(data["entries"].clone()) {
        Ok(v) => v,
        Err(e) => {
            return TaskOutput::fail(
                AppError::coded(E1004).with_detail(format!("entries 字段解析失败：{e}")),
            )
        }
    };
    // 锁包住「读—改—写」全过程
    let r = crate::critsec::with_critical_section(|| hosts_file::apply(&entries));
    match r {
        Ok(rep) => TaskOutput {
            ok: true,
            steps: vec![ok_step(
                "hosts",
                format!(
                    "写入 {} 条（启用 {}），{} 字节，耗时 {}ms{}",
                    rep.total_entries,
                    rep.enabled_entries,
                    rep.bytes,
                    rep.elapsed_ms,
                    if rep.used_fallback {
                        "，走了回退路径"
                    } else {
                        ""
                    }
                ),
            )],
            rolled_back: false,
            recovery_required: false,
            extra: serde_json::to_value(&rep).unwrap_or(Value::Null),
            error: None,
        },
        Err(e) => {
            let mut out = TaskOutput::fail(e.clone());
            out.steps.push(bad_step("hosts", e.to_string()));
            out
        }
    }
}

fn hosts_restore(data: &Value) -> TaskOutput {
    let hex = data["raw_hex"].as_str().unwrap_or("");
    let bytes = match crate::util::decode_hex(hex) {
        Some(b) => b,
        None => {
            return TaskOutput::fail(
                AppError::coded(E1004).with_detail("raw_hex 字段不是合法十六进制"),
            )
        }
    };
    let r = crate::critsec::with_critical_section(|| hosts_file::restore_raw(&bytes));
    match r {
        Ok(_) => TaskOutput::ok().with_step(ok_step("hosts", "hosts 已逐字节还原")),
        Err(e) => {
            let mut out = TaskOutput::fail(e.clone());
            out.steps.push(bad_step("hosts", e.to_string()));
            out
        }
    }
}

fn dns_set(data: &Value) -> TaskOutput {
    let alias = match data["alias"].as_str() {
        Some(a) => a.to_string(),
        None => {
            return TaskOutput::fail(AppError::coded(E1004).with_detail("缺少 alias"))
        }
    };
    let v4: Option<Vec<String>> = data
        .get("v4")
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    let v6: Option<Vec<String>> = data
        .get("v6")
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    let r = crate::critsec::with_critical_section(|| -> Result<Vec<ApplyStep>> {
        let mut steps = Vec::new();
        if let Some(list) = v4.as_ref() {
            match dns_client::set(&alias, AddressFamily::V4, list) {
                Ok(_) => steps.push(ok_step(
                    "dns",
                    if list.is_empty() {
                        "IPv4 已还原为自动获取".to_string()
                    } else {
                        format!("IPv4 → {}", list.join(", "))
                    },
                )),
                Err(e) => return Err(e),
            }
        }
        if let Some(list) = v6.as_ref() {
            match dns_client::set(&alias, AddressFamily::V6, list) {
                Ok(_) => steps.push(ok_step(
                    "dns",
                    if list.is_empty() {
                        "IPv6 已还原为自动获取".to_string()
                    } else {
                        format!("IPv6 → {}", list.join(", "))
                    },
                )),
                Err(e) => return Err(e),
            }
        }
        Ok(steps)
    });

    match r {
        Ok(steps) => TaskOutput {
            ok: true,
            steps,
            rolled_back: false,
            recovery_required: false,
            extra: Value::Null,
            error: None,
        },
        Err(e) => {
            let mut out = TaskOutput::fail(e.clone());
            out.steps.push(bad_step("dns", e.to_string()));
            out
        }
    }
}

fn dns_reset(data: &Value) -> TaskOutput {
    let alias = match data["alias"].as_str() {
        Some(a) => a.to_string(),
        None => return TaskOutput::fail(AppError::coded(E1004).with_detail("缺少 alias")),
    };
    let family = match data.get("family").and_then(|v| v.as_str()) {
        Some("v4") | Some("V4") => Some(AddressFamily::V4),
        Some("v6") | Some("V6") => Some(AddressFamily::V6),
        _ => None,
    };
    let r = crate::critsec::with_critical_section(|| dns_client::reset(&alias, family));
    match r {
        Ok(_) => TaskOutput::ok().with_step(ok_step(
            "dns",
            match family {
                Some(f) => format!("{} 已还原为自动获取", f.label()),
                None => "IPv4 / IPv6 均已还原为自动获取".to_string(),
            },
        )),
        Err(e) => {
            let mut out = TaskOutput::fail(e.clone());
            out.steps.push(bad_step("dns", e.to_string()));
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let data = vec![0u8, 1, 15, 16, 255, 128];
        let hex = crate::util::encode_hex(&data);
        assert_eq!(hex, "00010f10ff80");
        assert_eq!(crate::util::decode_hex(&hex).unwrap(), data);
    }

    #[test]
    fn unknown_task_rejected() {
        let out = execute("rm_rf", &serde_json::json!({}));
        assert!(!out.ok);
        assert_eq!(out.error.unwrap().code, E1004);
    }

    #[test]
    fn dns_set_requires_alias() {
        let out = execute("dns_set", &serde_json::json!({ "v4": [] }));
        assert!(!out.ok);
    }

    #[test]
    fn hosts_apply_requires_entries_field() {
        let out = execute("hosts_apply", &serde_json::json!({ "entries": "nope" }));
        assert!(!out.ok);
        assert_eq!(out.error.unwrap().code, E1004);
    }
}

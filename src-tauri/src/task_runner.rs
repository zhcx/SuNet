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
use crate::os::system_proxy::{self, ProxyState};
use serde_json::Value;

const KNOWN_TASKS: &[&str] = &[
    "hosts_apply",
    "hosts_restore",
    "dns_set",
    "dns_reset",
    "dns_flush",
    "proxy_write",
    "proxy_restore",
    "proxy_clear",
    "probe",
    // 登录自启计划任务（任务库只有管理员可写 → 只能在提权进程里做）
    "autostart_set",
    // 批处理外壳：内部只允许上面这些子任务，且不允许嵌套自己
    "batch",
    // 静默提权通道的安装 / 卸载（仅 Windows 有效；必须在提权进程里执行）
    "silent_install",
    "silent_uninstall",
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
        // 批处理：一次提权跑多个子任务（见 batch 的说明）
        "batch" => batch(data),
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
        // ── 代理写任务（macOS 专用路径：networksetup 需要 root）──
        // Windows 不会走到这里（proxy_write_needs_root() = false，进程内直接写）
        "proxy_write" => proxy_write(data),
        "proxy_restore" => proxy_restore(data),
        "proxy_clear" => proxy_clear(),
        // ── 登录自启的计划任务（任务库只有管理员可写）──
        "autostart_set" => autostart_set(data),
        // ── 静默提权通道（Windows 计划任务）的安装 / 卸载 ──
        // 装/删一个 `RL HIGHEST` 的计划任务本身就需要管理员，所以只能在提权进程里做。
        "silent_install" | "silent_uninstall" => silent_channel(task == "silent_install"),
        other => TaskOutput::fail(
            AppError::coded(E1004).with_detail(format!("未知任务名：{other}")),
        ),
    }
}

/// autostart_set：创建 / 删除「登录时静默启动」的计划任务（载荷 `{ enable: bool }`）。
///
/// 必须在提权进程里执行：任务库 `C:\Windows\System32\Tasks` 只有管理员可写，
/// 主进程是 asInvoker，直接调 `schtasks /Create` 必定被拒（实测退出码 1）。
#[cfg(target_os = "windows")]
fn autostart_set(data: &Value) -> TaskOutput {
    let enable = data["enable"].as_bool().unwrap_or(false);
    match crate::os::autostart::apply(enable) {
        Ok(()) => TaskOutput::ok().with_step(ok_step(
            "autostart",
            if enable {
                "已创建登录自启计划任务（SuNet）"
            } else {
                "已删除登录自启计划任务"
            },
        )),
        Err(e) => {
            let mut out = TaskOutput::fail(e.clone());
            out.steps.push(bad_step("autostart", e.to_string()));
            out
        }
    }
}

/// 非 Windows 不会走到这里（macOS 的自启用 LaunchAgent，在命令层直接处理）
#[cfg(not(target_os = "windows"))]
fn autostart_set(_data: &Value) -> TaskOutput {
    TaskOutput::fail(AppError::coded(E1004).with_detail("该任务仅 Windows 可用"))
}

/// 静默提权通道（Windows 计划任务）的安装 / 卸载。
///
/// 装上之后，hosts / DNS 的写入不再需要逐次 UAC（见 `elevation_task.rs`）。
/// 非 Windows 平台没有这条通道，`elevation_task::install` 会返回「仅 Windows 可用」。
fn silent_channel(install: bool) -> TaskOutput {
    let r = if install {
        crate::elevation_task::install()
    } else {
        crate::elevation_task::uninstall()
    };
    match r {
        Ok(msg) => TaskOutput::ok().with_step(ok_step("elevation", msg)),
        Err(e) => {
            let mut out = TaskOutput::fail(e.clone());
            out.steps.push(bad_step("elevation", e.to_string()));
            out
        }
    }
}

/// batch：把一串子任务按顺序放进**同一个提权进程**里执行。
///
/// 动机：一次方案切换要写 hosts 与 DNS 两层，而两层都必须提权 —— 原先各起一个提权
/// 进程，Windows 上就是连着弹两次 UAC。合并后只起一次；子任务顺序与父进程原先的
/// 逐层调用顺序完全一致，失败即停（与父进程 short-circuit 的语义一致），已完成
/// 的步骤照实上报，父进程据此判断哪些层已落地、要回滚哪些层。
///
/// 安全约束：子任务仍然只能来自 [`KNOWN_TASKS`] 白名单，且**禁止嵌套 batch** ——
/// 否则一个载荷就能把子进程拖进自增递归。
fn batch(data: &Value) -> TaskOutput {
    let Some(ops) = data["ops"].as_array() else {
        return TaskOutput::fail(AppError::coded(E1004).with_detail("缺少 ops 数组"));
    };
    if ops.is_empty() {
        return TaskOutput::fail(AppError::coded(E1004).with_detail("ops 为空"));
    }

    let mut out = TaskOutput::ok();
    for op in ops {
        let name = match op["task"].as_str() {
            Some(n) => n,
            None => {
                return TaskOutput::fail(AppError::coded(E1004).with_detail("op 缺少 task 字段"))
            }
        };
        if name == "batch" || !KNOWN_TASKS.contains(&name) {
            return TaskOutput::fail(
                AppError::coded(E1004).with_detail(format!("batch 不接受子任务：{name}")),
            );
        }
        let sub = op.get("data").cloned().unwrap_or(Value::Null);
        let TaskOutput {
            ok,
            steps: sub_steps,
            extra,
            error,
            ..
        } = execute(name, &sub);
        if !extra.is_null() {
            out.extra = extra;
        }
        let reported = sub_steps.len();
        out.steps.extend(sub_steps);
        if !ok {
            // 失败即停：后面的层一步都不执行，父进程按已上报的步骤决定回滚。
            //
            // 个别子任务在「载荷本身不合法」时会直接失败、一条步骤都不上报
            // （例如 proxy_restore 的 state 解析失败）—— 这里补一条，
            // 保证每次失败都能在报告里看到，也让「已上报步骤数」这个信号可靠：
            // 父进程正是靠它反推哪些层已经落地。
            if reported == 0 {
                let why = error
                    .as_ref()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "未知错误".to_string());
                out.steps
                    .push(bad_step("batch", format!("子任务 {name} 失败：{why}")));
            }
            out.ok = false;
            out.error = error;
            break;
        }
    }
    out
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

/// proxy_write：写入手动代理（`enable=false` 即关闭手动代理）
///
/// 载荷：`{ enable: bool, server: "host:port", bypass: String }`
fn proxy_write(data: &Value) -> TaskOutput {
    let enable = data["enable"].as_bool().unwrap_or(false);
    let server = data["server"].as_str().unwrap_or("").to_string();
    let bypass = data["bypass"].as_str().unwrap_or("").to_string();
    let r = crate::critsec::with_critical_section(|| {
        system_proxy::write_manual(enable, &server, &bypass)
    });
    match r {
        Ok(_) => TaskOutput::ok().with_step(ok_step(
            "proxy",
            if enable {
                "系统代理已启用（回读校验由调用方完成）".to_string()
            } else {
                "手动代理已关闭".to_string()
            },
        )),
        Err(e) => {
            let mut out = TaskOutput::fail(e.clone());
            out.steps.push(bad_step("proxy", e.to_string()));
            out
        }
    }
}

/// proxy_restore：按操作前的快照原样还原（含 PAC）
///
/// 载荷：`{ state: ProxyState }`
fn proxy_restore(data: &Value) -> TaskOutput {
    let state: ProxyState = match serde_json::from_value(data["state"].clone()) {
        Ok(s) => s,
        Err(e) => {
            return TaskOutput::fail(
                AppError::coded(E1004).with_detail(format!("state 字段解析失败：{e}")),
            )
        }
    };
    let r = crate::critsec::with_critical_section(|| system_proxy::restore(&state));
    match r {
        Ok(_) => TaskOutput::ok().with_step(ok_step("proxy", "系统代理已按快照还原")),
        Err(e) => {
            let mut out = TaskOutput::fail(e.clone());
            out.steps.push(bad_step("proxy", e.to_string()));
            out
        }
    }
}

/// proxy_clear：彻底清空代理配置（保留绕过列表）
fn proxy_clear() -> TaskOutput {
    let r = crate::critsec::with_critical_section(system_proxy::hard_clear);
    match r {
        Ok(_) => TaskOutput::ok().with_step(ok_step("proxy", "系统代理配置已清空")),
        Err(e) => {
            let mut out = TaskOutput::fail(e.clone());
            out.steps.push(bad_step("proxy", e.to_string()));
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

    #[test]
    fn proxy_restore_rejects_bad_state() {
        // 载荷不合法时必须在碰系统配置之前就报错
        let out = execute("proxy_restore", &serde_json::json!({ "state": 42 }));
        assert!(!out.ok);
        assert_eq!(out.error.unwrap().code, E1004);
    }

    #[test]
    fn proxy_tasks_are_whitelisted() {
        for t in ["proxy_write", "proxy_restore", "proxy_clear"] {
            assert!(KNOWN_TASKS.contains(&t), "{t} 不在白名单里");
        }
    }

    /// 批处理：失败即停，且已完成的步骤必须上报（父进程靠它决定回滚哪些层）。
    /// 这里刻意用「载荷非法因而在碰系统之前就失败」的子任务，避免测试动真实系统配置。
    #[test]
    fn batch_stops_at_first_failure_and_reports_steps() {
        let out = execute(
            "batch",
            &serde_json::json!({
                "ops": [
                    { "task": "proxy_restore", "data": { "state": 42 } },
                    { "task": "proxy_restore", "data": { "state": 43 } }
                ]
            }),
        );
        assert!(!out.ok);
        assert_eq!(out.steps.len(), 1, "第一个子任务失败后必须停止（第二个不得执行）");
        assert_eq!(out.steps[0].kind, "batch", "子任务未上报步骤时要补一条失败步骤");
        assert_eq!(out.error.unwrap().code, E1004);
    }

    /// 嵌套 batch 与白名单外的子任务都必须被拒（否则载荷能把子进程拖进自增递归）
    #[test]
    fn batch_rejects_nesting_and_unknown_tasks() {
        let nested = execute("batch", &serde_json::json!({ "ops": [ { "task": "batch" } ] }));
        assert!(!nested.ok);
        assert_eq!(nested.error.unwrap().code, E1004);

        let unknown = execute("batch", &serde_json::json!({ "ops": [ { "task": "rm_rf" } ] }));
        assert!(!unknown.ok);
        assert_eq!(unknown.error.unwrap().code, E1004);

        let empty = execute("batch", &serde_json::json!({ "ops": [] }));
        assert!(!empty.ok);
    }
}

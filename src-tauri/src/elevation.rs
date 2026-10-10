//! 提权调用（设计方案 §1.4.4 / §1.4.10）
//!
//! 统一入口是 [`run_elevated`]，平台各自实现：
//!
//! **Windows**
//!   1. 必须 `ShellExecuteExW` + `runas` verb —— `CreateProcess` 不触发 UAC，
//!      直接返回 ERROR_ELEVATION_REQUIRED(740)
//!   2. `SEE_MASK_NOCLOSEPROCESS` 拿进程句柄，才能等待并取退出码
//!   3. 载荷走文件（lpParameters 有长度上限，超长会**静默截断**）
//!   4. 提权子进程按 argv 分派，不加载 WebView2、不建托盘、不常驻
//!
//! **macOS**
//!   1. 优先走常驻 root helper（`/var/run/sunet-helper.sock`，装一次之后不再弹密码）
//!   2. helper 不可用时退回 `osascript -e 'do shell script "..." with administrator privileges'`，
//!      语义等价于 Windows 的 `runas`：用户逐次授权
//!   3. 载荷同样走文件；提权子进程仍是同一个可执行文件（argv 分派，不进 GUI）

use crate::error::{AppError, Result};
use crate::ipc::{self, TaskOutput};
use crate::paths;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[cfg(target_os = "windows")]
use crate::os::winapi::*;

/// 提权任务的整体超时（§1.4.10：30 秒）
pub const TASK_TIMEOUT: Duration = Duration::from_secs(30);

/// 提权是否**走到了要用户交互的那条路**（Windows = `runas` 弹 UAC，
/// macOS = `osascript` 弹系统授权框）。
///
/// 用途：填进 `ApplyReport.prompted` —— 界面据此提示"装一次静默通道 / 免密助手之后，
/// 切换就不用再输密码了"。只记最近一次，由 [`take_prompted`] 取走并清零。
static PROMPTED: AtomicBool = AtomicBool::new(false);

/// 取走并清零「这次提权弹框了吗」
pub fn take_prompted() -> bool {
    PROMPTED.swap(false, Ordering::Relaxed)
}

fn mark_prompted() {
    PROMPTED.store(true, Ordering::Relaxed);
}

/// 任务名白名单形状校验：任务名会被拼进命令行，必须限制字符集
fn task_name_ok(task: &str) -> bool {
    !task.is_empty()
        && task.len() <= 32
        && task
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

pub fn is_elevated() -> bool {
    crate::os::privilege::is_elevated()
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// 需要提权的操作统一入口。
/// 若当前进程已是管理员，则直接在本进程执行，不重复弹 UAC。
#[cfg(target_os = "windows")]
pub fn run_elevated(task: &str, payload: Value, timeout: Duration) -> Result<TaskOutput> {
    let output = run_elevated_raw(task, payload, timeout)?;
    if !output.ok {
        if let Some(err) = output.error.clone() {
            return Err(err);
        }
    }
    Ok(output)
}

/// 与 [`run_elevated`] 相同，但**不把 `!out.ok` 转成 `Err`**。
///
/// 批处理任务（一次提权跑多层）需要拿到子进程上报的**部分步骤**，
/// 才知道哪些层已经落地、需要回滚 —— 那部分信息会被 `run_elevated` 的
/// 错误转换丢掉。`Err` 只保留"基础设施失败"（启动失败 / 超时 / 用户取消）。
#[cfg(target_os = "windows")]
pub fn run_elevated_raw(task: &str, payload: Value, timeout: Duration) -> Result<TaskOutput> {
    if !task_name_ok(task) {
        return Err(AppError::internal(format!("非法任务名：{task}")));
    }
    if is_elevated() {
        log::info!(target: "elevation", "当前已是管理员，任务 {task} 在本进程执行");
        return Ok(crate::task_runner::execute(task, &payload));
    }

    // 静默通道优先：用户装过计划任务（且开关打开）就完全不再弹 UAC；
    // 通道不可用/被删/正在忙时自动回退到下面的 runas 路径，行为与从前一致。
    if crate::elevation_task::usable() {
        match crate::elevation_task::run(task, payload.clone(), timeout) {
            Ok(out) => return Ok(out),
            Err(e) => log::warn!(
                target: "elevation",
                "静默通道执行 {task} 失败，回退 runas：{e}"
            ),
        }
    }

    // 走到这里就是要弹 UAC 了（静默通道不可用 / 没装）：记一笔，供上层提示
    mark_prompted();

    let (in_path, out_path) = ipc::prepare(task, payload)?;
    let exe = paths::exe_path()?;
    let params = format!(
        "--task {task} --in \"{}\" --out \"{}\"",
        in_path.display(),
        out_path.display()
    );

    let verb = wide("runas");
    let file = wide(&exe.to_string_lossy());
    let params_w = wide(&params);

    let mut sei = SHELLEXECUTEINFOW::default();
    sei.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    sei.lpVerb = verb.as_ptr();
    sei.lpFile = file.as_ptr();
    sei.lpParameters = params_w.as_ptr();
    // 提权子进程只干活、不露面：SW_HIDE 避免 UAC 通过后任务栏闪一个窗口
    sei.nShow = SW_HIDE;

    let started = std::time::Instant::now();
    let ok = unsafe { ShellExecuteExW(&mut sei) };
    if ok == 0 {
        let err = unsafe { GetLastError() };
        ipc::cleanup(&in_path, &out_path);
        if err == ERROR_CANCELLED {
            log::info!(target: "elevation", "任务 {task}：用户取消 UAC（E1001）");
            return Err(AppError::coded(crate::error::E1001));
        }
        log::error!(target: "elevation", "任务 {task}：ShellExecuteExW 失败，错误码 {err}");
        return Err(AppError::internal(format!(
            "无法启动提权进程（Win32 错误码 {err}）"
        )));
    }

    let hproc = sei.hProcess;
    if hproc.is_null() {
        ipc::cleanup(&in_path, &out_path);
        return Err(AppError::internal("提权进程句柄为空（fMask 未生效）"));
    }

    let wait_ms = timeout.as_millis().min(u32::MAX as u128) as DWORD;
    let wait = unsafe { WaitForSingleObject(hproc, wait_ms) };
    let mut exit_code: DWORD = 0;
    let mut timed_out = false;
    match wait {
        WAIT_OBJECT_0 => {
            unsafe {
                GetExitCodeProcess(hproc, &mut exit_code);
            }
        }
        WAIT_TIMEOUT => {
            timed_out = true;
            unsafe {
                TerminateProcess(hproc, 1);
            }
            log::warn!(target: "elevation", "任务 {task}：等待超时，已强制结束");
        }
        _ => {
            unsafe {
                GetExitCodeProcess(hproc, &mut exit_code);
            }
        }
    }
    unsafe {
        CloseHandle(hproc);
    }

    if timed_out {
        ipc::cleanup(&in_path, &out_path);
        return Err(AppError::coded(crate::error::E1003).with_detail(format!(
            "任务 {task} 超过 {} 秒未完成",
            timeout.as_secs()
        )));
    }

    let output = ipc::collect_out(&out_path, exit_code as i32);
    ipc::cleanup(&in_path, &out_path);

    log::info!(
        target: "elevation",
        "提权任务 {task}：退出码 {exit_code}，耗时 {}ms，载荷 {} / 结果 {}",
        started.elapsed().as_millis(),
        in_path.display(),
        out_path.display()
    );

    Ok(output)
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

/// 需要提权的操作统一入口（macOS）。
///
/// 与 Windows 的差别：这里优先用常驻 helper（不需要每次输密码），
/// helper 不可用或调用失败时退回 osascript 授权框。
#[cfg(target_os = "macos")]
pub fn run_elevated(task: &str, payload: Value, timeout: Duration) -> Result<TaskOutput> {
    let output = run_elevated_raw(task, payload, timeout)?;
    if !output.ok {
        if let Some(err) = output.error.clone() {
            return Err(err);
        }
    }
    Ok(output)
}

/// 与 [`run_elevated`] 相同，但**不把 `!out.ok` 转成 `Err`**（批处理任务要靠子进程
/// 上报的部分步骤决定回滚哪些层）。`Err` 只保留基础设施失败（启动失败 / 用户取消）。
#[cfg(target_os = "macos")]
pub fn run_elevated_raw(task: &str, payload: Value, timeout: Duration) -> Result<TaskOutput> {
    if !task_name_ok(task) {
        return Err(AppError::internal(format!("非法任务名：{task}")));
    }
    if is_elevated() {
        log::info!(target: "elevation", "当前已是 root，任务 {task} 在本进程执行");
        return Ok(crate::task_runner::execute(task, &payload));
    }

    if crate::helper::available() {
        let started = std::time::Instant::now();
        match crate::helper::run_task(task, payload.clone(), timeout) {
            Ok(out) => {
                log::info!(
                    target: "elevation",
                    "helper 任务 {task}：ok={}，耗时 {}ms",
                    out.ok,
                    started.elapsed().as_millis()
                );
                return Ok(out);
            }
            Err(e) => {
                log::warn!(target: "elevation", "helper 执行 {task} 失败，回退 osascript：{e}");
            }
        }
    }

    run_via_osascript(task, payload)
}

/// 回退通道：`osascript ... with administrator privileges`（用户每次授权）
///
/// 注意：这条路径**不施加 timeout** —— 授权框在等用户输密码，
/// 把等待算作超时会把"用户正在输密码"误判成失败。
#[cfg(target_os = "macos")]
fn run_via_osascript(task: &str, payload: Value) -> Result<TaskOutput> {
    use crate::os::macos::net;

    // 用户马上要输密码了：记一笔，供上层提示"可以装免密助手"
    mark_prompted();

    let (in_path, out_path) = ipc::prepare(task, payload)?;
    let exe = paths::exe_path()?;
    let command = format!(
        "{} --task {} --in {} --out {}",
        sh_quote(&exe.to_string_lossy()),
        sh_quote(task),
        sh_quote(&in_path.to_string_lossy()),
        sh_quote(&out_path.to_string_lossy())
    );
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        as_escape(&command)
    );

    log::info!(target: "elevation", "任务 {task}：走 osascript 授权（需要用户输入密码）");
    let started = std::time::Instant::now();
    let res = net::try_run(net::OSASCRIPT, &["-e".to_string(), script]);
    let had_out = out_path.exists();

    let output = match res {
        Ok((ok, out, err)) => {
            if !ok {
                if is_cancelled(&out, &err) {
                    ipc::cleanup(&in_path, &out_path);
                    log::info!(target: "elevation", "任务 {task}：用户取消授权（E1001）");
                    return Err(AppError::coded(crate::error::E1001));
                }
                if !had_out {
                    ipc::cleanup(&in_path, &out_path);
                    return Err(AppError::internal(format!(
                        "提权执行失败：{}",
                        first_nonempty(&err, &out)
                    )));
                }
            }
            ipc::collect_out(&out_path, if ok { 0 } else { 1 })
        }
        Err(e) => {
            ipc::cleanup(&in_path, &out_path);
            return Err(AppError::internal(format!("无法启动 osascript：{e}")));
        }
    };
    ipc::cleanup(&in_path, &out_path);

    log::info!(
        target: "elevation",
        "提权任务 {task}：ok={}，耗时 {}ms",
        output.ok,
        started.elapsed().as_millis()
    );

    Ok(output)
}

/// 用 `osascript ... with administrator privileges` 以管理员身份跑一条**固定的自身子命令**
/// （例如 `--helper-install` / `--helper-uninstall`）。
///
/// 与 [`run_via_osascript`] 的区别：那条走的是 `--task` 载荷协议，只能跑 task_runner 的
/// 白名单任务；而安装/卸载常驻助手要写 `/Library` 并加载 LaunchDaemon，不是 task，
/// 所以单开一条。**不接受任意命令**，只由本模块内部的固定调用点使用。
///
/// 注意：不施加 timeout —— 授权框在等用户输密码，把等待算作超时是误判。
#[cfg(target_os = "macos")]
pub fn run_self_command_elevated(extra_args: &[&str]) -> Result<String> {
    use crate::os::macos::net;

    let exe = paths::exe_path()?;
    let mut parts = vec![sh_quote(&exe.to_string_lossy())];
    for a in extra_args {
        parts.push(sh_quote(a));
    }
    let command = parts.join(" ");
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        as_escape(&command)
    );

    log::info!(target: "elevation", "以管理员身份执行自身子命令：{extra_args:?}");
    match net::try_run(net::OSASCRIPT, &["-e".to_string(), script]) {
        Ok((true, out, _)) => Ok(out.trim().to_string()),
        Ok((false, out, err)) => {
            if is_cancelled(&out, &err) {
                log::info!(target: "elevation", "用户取消授权（E1001）");
                return Err(AppError::coded(crate::error::E1001));
            }
            Err(AppError::internal(format!(
                "命令执行失败：{}",
                first_nonempty(&err, &out)
            )))
        }
        Err(e) => Err(AppError::internal(format!("无法启动 osascript：{e}"))),
    }
}

/// POSIX 单引号转义
#[cfg(target_os = "macos")]
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// AppleScript 字符串字面量转义（反斜杠与双引号）
#[cfg(target_os = "macos")]
fn as_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// 判断 osascript 的失败是不是"用户点了取消"
#[cfg(target_os = "macos")]
fn is_cancelled(stdout: &str, stderr: &str) -> bool {
    let text = format!("{stdout}\n{stderr}");
    text.contains("-128")
        || text.contains("User canceled")
        || text.contains("User cancelled")
        || text.contains("用户已取消")
        || text.contains("已取消")
}

#[cfg(target_os = "macos")]
fn first_nonempty(a: &str, b: &str) -> String {
    let pick = if a.trim().is_empty() { b } else { a };
    pick.trim().lines().next().unwrap_or("未知错误").to_string()
}

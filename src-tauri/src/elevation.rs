//! 提权调用（设计方案 §1.4.4 / §1.4.10）
//!
//! 要点：
//!   1. 必须 `ShellExecuteExW` + `runas` verb —— `CreateProcess` 不触发 UAC，
//!      直接返回 ERROR_ELEVATION_REQUIRED(740)
//!   2. `SEE_MASK_NOCLOSEPROCESS` 拿进程句柄，才能等待并取退出码
//!   3. 载荷走文件（lpParameters 有长度上限，超长会**静默截断**）
//!   4. 提权子进程按 argv 分派，不加载 WebView2、不建托盘、不常驻

use crate::error::{AppError, Result};
use crate::ipc::{self, TaskOutput};
use crate::os::winapi::*;
use crate::paths;
use serde_json::Value;
use std::time::Duration;

/// 提权任务的整体超时（§1.4.10：30 秒）
pub const TASK_TIMEOUT: Duration = Duration::from_secs(30);

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

/// 需要提权的操作统一入口。
/// 若当前进程已是管理员，则直接在本进程执行，不重复弹 UAC。
pub fn run_elevated(task: &str, payload: Value, timeout: Duration) -> Result<TaskOutput> {
    if !task_name_ok(task) {
        return Err(AppError::internal(format!("非法任务名：{task}")));
    }
    if is_elevated() {
        log::info!(target: "elevation", "当前已是管理员，任务 {task} 在本进程执行");
        return Ok(crate::task_runner::execute(task, &payload));
    }

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
    sei.nShow = SW_SHOWNORMAL;

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

    if !output.ok {
        if let Some(err) = output.error.clone() {
            return Err(err);
        }
    }
    Ok(output)
}

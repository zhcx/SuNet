//! 开机自启动（Windows：登录时计划任务）
//!
//! 为什么用计划任务而不是注册表 Run 键：`--minimized` 让应用登录后静默进托盘，
//! 计划任务在「任务计划程序」里可见、可被组策略统一管理，与 macOS 侧的
//! LaunchAgent（`cmd/system.rs` 的 `platform_autostart`）一一对应。
//!
//! **必须提权**：任务库 `C:\Windows\System32\Tasks` 只有管理员可写，而主进程是
//! asInvoker。之前在主进程直接调 `schtasks /Create` 的做法必定失败
//! （退出码 1 + "拒绝访问"，实测截图里就是这条报错），所以这个函数只在提权
//! 子进程里执行（见 `task_runner` 的 `autostart_set`）。

use crate::error::{AppError, Result};
use crate::os::winapi::decode_console_output;
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// 计划任务名（用户在「任务计划程序」里能直接看到）
const TASK_NAME: &str = "SuNet";

/// 创建（`enable = true`）/ 删除（`enable = false`）登录自启任务。幂等。
pub fn apply(enable: bool) -> Result<()> {
    let mut cmd = std::process::Command::new("schtasks.exe");
    if enable {
        let exe = crate::paths::exe_path()?;
        let account = safe_account_name()?;
        // /RL LIMITED —— 主进程是 asInvoker，用 HIGHEST 反而会让登录时弹 UAC
        // /IT          —— 仅在用户登录时运行（无需存密码）
        cmd.args([
            "/Create",
            "/TN",
            TASK_NAME,
            "/TR",
            &format!("\"{}\" --minimized", exe.display()),
            "/SC",
            "ONLOGON",
            "/RL",
            "LIMITED",
            "/RU",
            &account,
            "/IT",
            "/F",
        ]);
    } else {
        cmd.args(["/Delete", "/TN", TASK_NAME, "/F"]);
    }

    let out = cmd
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| AppError::internal(format!("调用 schtasks 失败：{e}")))?;
    let code = out.status.code().unwrap_or(-1);
    // schtasks 的文本按 OEM 代码页输出，必须按 OEM 解码，否则界面上一片方块
    let err_text = decode_console_output(&out.stderr);
    let out_text = decode_console_output(&out.stdout);
    let detail = if err_text.is_empty() { out_text } else { err_text };
    log::info!(
        target: "autostart",
        "schtasks（enable={enable}）退出码 {code}：{detail}"
    );

    if enable && code != 0 {
        return Err(AppError::coded(crate::error::E1002).with_detail(format!(
            "创建计划任务失败（退出码 {code}）：{detail}"
        )));
    }
    // 关闭时「任务不存在」不算错误（幂等）
    Ok(())
}

/// 当前账户名（`DOMAIN\user`）。只允许常见字符，防止把奇怪内容塞进 schtasks 参数。
fn safe_account_name() -> Result<String> {
    let user = std::env::var("USERNAME").map_err(|_| AppError::internal("无法获取当前用户名"))?;
    let domain = std::env::var("USERDOMAIN").unwrap_or_default();
    let name = if domain.is_empty() {
        user
    } else {
        format!("{domain}\\{user}")
    };
    if !name.chars().all(|c| {
        c.is_ascii_alphanumeric() || c == '\\' || c == '-' || c == '_' || c == '.' || c == '$'
    }) {
        return Err(AppError::internal("账户名包含非预期字符，已拒绝"));
    }
    Ok(name)
}

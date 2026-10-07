//! helper 服务端：常驻 root 守护进程（macOS）
//!
//! 由 LaunchDaemon 拉起（`SuNet --helper-daemon`），只做三件事：
//! 校验连接者身份 → 解析单行 JSON 请求 → 调用 `task_runner::execute` 回写结果。
//! 它不持有任何 UI 状态，也不接受任意命令，只是把既有的任务执行器搬到 root 上下文。

use crate::error::{AppError, E1002, E1004};
use crate::ipc::TaskOutput;
use serde::Deserialize;
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};

#[derive(Deserialize)]
struct Request {
    task: String,
    payload: Value,
}

/// 守护进程主循环；返回值即进程退出码
pub fn daemon_main() -> i32 {
    if !crate::os::privilege::is_elevated() {
        eprintln!("[sunet-helper] 必须以 root 运行（当前 euid != 0），退出");
        return 2;
    }
    // 清掉上次退出留下的 socket
    let _ = std::fs::remove_file(crate::helper::SOCKET_PATH);
    let listener = match UnixListener::bind(crate::helper::SOCKET_PATH) {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "[sunet-helper] bind {} 失败：{e}",
                crate::helper::SOCKET_PATH
            );
            return 1;
        }
    };
    // 0666 只是为了让非 root 的控制台用户能连上；真正的授权在 peer_allowed()
    if let Err(e) = std::fs::set_permissions(
        crate::helper::SOCKET_PATH,
        std::fs::Permissions::from_mode(0o666),
    ) {
        eprintln!("[sunet-helper] chmod socket 失败：{e}");
    }
    eprintln!(
        "[sunet-helper] 已就绪：{}（只接受 root 与控制台用户的连接）",
        crate::helper::SOCKET_PATH
    );

    for conn in listener.incoming() {
        match conn {
            Ok(stream) => handle(stream),
            Err(e) => eprintln!("[sunet-helper] accept 失败：{e}"),
        }
    }
    0
}

fn handle(mut stream: UnixStream) {
    let uid = peer_uid(&stream);
    if !peer_allowed(uid) {
        eprintln!("[sunet-helper] 拒绝来自 uid {uid:?} 的连接");
        let out = TaskOutput::fail(
            AppError::coded(E1002).with_detail("helper 拒绝了该用户的连接（仅限控制台用户）"),
        );
        let _ = write_line(&mut stream, &out);
        return;
    }

    let mut line = String::new();
    let read = {
        let mut reader = BufReader::new(&stream);
        reader.read_line(&mut line)
    };
    match read {
        Ok(0) => return,
        Ok(_) => {}
        Err(e) => {
            eprintln!("[sunet-helper] 读请求失败：{e}");
            return;
        }
    }

    let out = match serde_json::from_str::<Request>(line.trim_end()) {
        Ok(req) => {
            eprintln!("[sunet-helper] 执行任务 {}（uid {uid:?}）", req.task);
            crate::task_runner::execute(&req.task, &req.payload)
        }
        Err(e) => TaskOutput::fail(
            AppError::coded(E1004).with_detail(format!("helper 请求格式非法：{e}")),
        ),
    };
    if let Err(e) = write_line(&mut stream, &out) {
        eprintln!("[sunet-helper] 回写结果失败：{e}");
    }
}

fn write_line(stream: &mut UnixStream, out: &TaskOutput) -> std::io::Result<()> {
    let mut body = serde_json::to_string(out)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    body.push('\n');
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

/// 取对端 uid（macOS: `LOCAL_PEERCRED`）
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    let fd = stream.as_raw_fd();
    let mut cred: libc::xucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::xucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERCRED,
            &mut cred as *mut libc::xucred as *mut libc::c_void,
            &mut len,
        )
    };
    if rc == 0 {
        Some(cred.cr_uid)
    } else {
        None
    }
}

/// 只允许 root 与控制台用户；取不到凭据一律拒绝
fn peer_allowed(uid: Option<u32>) -> bool {
    match uid {
        Some(0) => true,
        Some(u) => crate::os::macos::net::console_uid() == Some(u),
        None => false,
    }
}

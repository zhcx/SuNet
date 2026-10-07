//! helper 客户端：把任务交给常驻 root helper 执行（macOS）

use crate::error::{AppError, Result, E1003};
use crate::ipc::TaskOutput;
use serde::Serialize;
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// 单行 JSON 请求
#[derive(Serialize)]
struct Request<'a> {
    task: &'a str,
    payload: Value,
}

/// 通过 unix socket 请求 helper 执行任务；任何一步失败都返回 Err，
/// 由调用方（`elevation`）决定是否退回 osascript 逐次授权。
pub fn run_task(task: &str, payload: Value, timeout: Duration) -> Result<TaskOutput> {
    if !super::available() {
        return Err(AppError::internal("helper 未运行（socket 不存在）"));
    }
    let mut stream = UnixStream::connect(super::SOCKET_PATH)
        .map_err(|e| AppError::internal(format!("连接 helper 失败：{e}")))?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let mut body = serde_json::to_string(&Request { task, payload })?;
    body.push('\n');
    stream
        .write_all(body.as_bytes())
        .map_err(|e| AppError::internal(format!("向 helper 写请求失败：{e}")))?;
    let _ = stream.flush();

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let n = reader.read_line(&mut line).map_err(|e| {
        if matches!(
            e.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ) {
            AppError::coded(E1003)
                .with_detail(format!("等待 helper 响应超时（{}s）", timeout.as_secs()))
        } else {
            AppError::internal(format!("读 helper 响应失败：{e}"))
        }
    })?;
    if n == 0 {
        return Err(AppError::coded(E1003).with_detail("helper 未返回结果（连接被关闭）"));
    }
    let out: TaskOutput = serde_json::from_str(line.trim_end())
        .map_err(|e| AppError::internal(format!("helper 响应无法解析：{e}")))?;
    Ok(out)
}

//! 提权子进程的通信协议（设计方案 §1.4.10）
//!
//! ShellExecuteW 不返回子进程 stdout、也不管道化输出，因此必须显式定义通道：
//!   - 进程退出码：粗粒度失败分类
//!   - 结果文件（JSON）：完整 TaskOutput，供界面如实展示
//!   - 载荷走文件而非命令行：lpParameters 有长度上限，超长会被**静默截断**
//!
//! 安全约束：随机 uuid 文件名、子进程校验属主与重解析点、用完即删、
//! 只按 task 名分派到固定函数表（不做任何动态派发）。

use crate::error::{AppError, Result, E1004};
use crate::os::fs_security;
use crate::paths;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Payload {
    pub version: u32,
    pub task: String,
    pub created_at: String,
    pub data: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ApplyStep {
    pub kind: String,
    pub applied: bool,
    pub verified: bool,
    pub message: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TaskOutput {
    pub ok: bool,
    #[serde(default)]
    pub steps: Vec<ApplyStep>,
    #[serde(default)]
    pub rolled_back: bool,
    #[serde(default)]
    pub recovery_required: bool,
    #[serde(default)]
    pub extra: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<AppError>,
}

impl TaskOutput {
    pub fn ok() -> Self {
        Self {
            ok: true,
            steps: Vec::new(),
            rolled_back: false,
            recovery_required: false,
            extra: serde_json::Value::Null,
            error: None,
        }
    }

    pub fn fail(err: AppError) -> Self {
        Self {
            ok: false,
            steps: Vec::new(),
            rolled_back: false,
            recovery_required: false,
            extra: serde_json::Value::Null,
            error: Some(err),
        }
    }

    pub fn with_step(mut self, step: ApplyStep) -> Self {
        self.steps.push(step);
        self
    }
}

pub fn prepare(task: &str, data: serde_json::Value) -> Result<(PathBuf, PathBuf)> {
    let dir = paths::ipc_dir();
    std::fs::create_dir_all(&dir)?;
    let id = uuid::Uuid::new_v4();
    let in_path = dir.join(format!("task-{id}.in.json"));
    let out_path = dir.join(format!("task-{id}.out.json"));
    let payload = Payload {
        version: 1,
        task: task.to_string(),
        created_at: chrono::Local::now().to_rfc3339(),
        data,
    };
    let text = serde_json::to_string_pretty(&payload)?;
    std::fs::write(&in_path, text.as_bytes())?;
    Ok((in_path, out_path))
}

/// 子进程侧：读取载荷。校验不通过一律拒绝（E1004）
pub fn read_payload(in_path: &str) -> Result<Payload> {
    let path = PathBuf::from(in_path);
    fs_security::verify_payload_file(&path)?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| AppError::coded(E1004).with_detail(format!("读取载荷失败：{e}")))?;
    // 读完立即删除（先删再解析，避免解析失败时留下残留文件）
    let _ = std::fs::remove_file(&path);
    // 容忍 UTF-8 BOM：外部工具生成的载荷可能带 BOM，不该因此被拒
    let text = text.trim_start_matches('\u{feff}');
    let payload: Payload = serde_json::from_str(text)
        .map_err(|e| AppError::coded(E1004).with_detail(format!("载荷 JSON 解析失败：{e}")))?;
    if payload.version != 1 {
        return Err(AppError::coded(E1004).with_detail(format!(
            "不支持的载荷版本：{}",
            payload.version
        )));
    }
    Ok(payload)
}

/// 父进程侧：读取结果文件；不存在则按退出码生成兜底报告
pub fn collect_out(out_path: &PathBuf, exit_code: i32) -> TaskOutput {
    if let Ok(text) = std::fs::read_to_string(out_path) {
        match serde_json::from_str::<TaskOutput>(&text) {
            Ok(out) => return out,
            Err(e) => log::warn!(target: "ipc", "结果文件解析失败（{e}），按退出码兜底"),
        }
    }
    if exit_code == 0 {
        return TaskOutput::ok().with_step(ApplyStep {
            kind: "ipc".into(),
            applied: true,
            verified: false,
            message: "提权任务未返回结果文件，按退出码 0 判定为成功".into(),
        });
    }
    TaskOutput::fail(
        AppError::coded(AppError::code_for_exit_code(exit_code))
            .with_detail(format!("提权子进程退出码 {exit_code}")),
    )
}

/// 子进程侧：写出结果（原子写，避免父进程读到半截 JSON）
pub fn write_output(out_path: &str, output: &TaskOutput) -> Result<()> {
    let path = PathBuf::from(out_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(output)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text.as_bytes())?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// 清理载荷与结果文件
pub fn cleanup(in_path: &PathBuf, out_path: &PathBuf) {
    let _ = std::fs::remove_file(in_path);
    let _ = std::fs::remove_file(out_path);
    let _ = std::fs::remove_file(out_path.with_extension("tmp"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_mapping() {
        assert_eq!(AppError::coded("E1004").exit_code(), 2);
        assert_eq!(AppError::coded("E1002").exit_code(), 3);
        assert_eq!(AppError::coded("E2001").exit_code(), 4);
        assert_eq!(AppError::coded("E2004").exit_code(), 5);
        assert_eq!(AppError::coded("E4003").exit_code(), 5);
        assert_eq!(AppError::coded("E1003").exit_code(), 7);
        assert_eq!(AppError::coded("E9001").exit_code(), 6);
    }

    #[test]
    fn payload_roundtrip_and_shape() {
        let data = serde_json::json!({ "entries": [1, 2, 3], "big": "x".repeat(5000) });
        let (in_path, out_path) = prepare("hosts_apply", data.clone()).unwrap();
        let name = in_path.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("task-") && name.ends_with(".in.json"));

        // 直接读 JSON（不做属主校验，校验路径在 fs_security 单测覆盖）
        let text = std::fs::read_to_string(&in_path).unwrap();
        let p: Payload = serde_json::from_str(&text).unwrap();
        assert_eq!(p.task, "hosts_apply");
        assert_eq!(p.data["entries"], data["entries"]);

        let out = TaskOutput::ok().with_step(ApplyStep {
            kind: "hosts".into(),
            applied: true,
            verified: true,
            message: "ok".into(),
        });
        write_output(&out_path.to_string_lossy(), &out).unwrap();
        let back = collect_out(&out_path, 0);
        assert!(back.ok && back.steps.len() == 1);

        cleanup(&in_path, &out_path);
        assert!(!in_path.exists() && !out_path.exists());
    }

    #[test]
    fn fallback_report_on_missing_file() {
        let out = collect_out(&PathBuf::from("C:\\definitely\\missing.out.json"), 4);
        assert!(!out.ok);
        assert_eq!(out.error.unwrap().code, "E2001");
    }
}

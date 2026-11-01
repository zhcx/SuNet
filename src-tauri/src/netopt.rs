//! 网络优化（内嵌 netopt）
//!
//! netopt 是一套跨平台的网络配置巡检 / 优化工具（Python 3.8+，零第三方依赖），
//! 从一开始就是按"被软件集成"设计的：
//!   · stdout 只输出 JSON，日志走 stderr；
//!   · 默认只读，变更必须显式 `--commit`；
//!   · 退出码有语义（0 正常 / 10 告警 / 20 故障 / 30 需管理员 / 40 不支持）；
//!   · 后端只产出 argv 计划，执行放在核心层 —— 界面可以先展示计划再决定执行。
//!
//! 集成方式：**整包嵌进可执行文件**（`include_str!`），首次使用时释放到家目录。
//! 不放 app 资源目录，是因为提权子进程（root）要读同一份脚本，而 root 的家目录
//! 不是用户的；绝对路径随载荷一起传过去，两边读到的必然是同一份文件。
//!
//! 安全边界（与 `task_runner` 的既有约定一致）：
//!   · 子进程只按任务名分派到固定函数表；
//!   · 脚本路径必须形如 `<…>/netopt/netopt.py`（`validate_script`），不接受任意脚本；
//!   · 所有外部命令走 argv 数组，不经过 shell。

use crate::error::{AppError, Result};
use crate::paths;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 巡检超时：DNS / IPv6 探测单项就有 1.5 秒级的等待，留足余量
pub const AUDIT_TIMEOUT: Duration = Duration::from_secs(60);
/// 执行超时：一次要改 DNS / 前缀 / 防火墙，比纯巡检慢
pub const APPLY_TIMEOUT: Duration = Duration::from_secs(120);

/// netopt 的子系统顺序（与 `netopt_core.SECTION_ORDER` 一致）
pub const SECTION_IDS: &[&str] = &["dns", "ipv6", "proxy", "firewall", "power"];

/// 内嵌的源码清单：文件名 → 内容
const SCRIPTS: &[(&str, &str)] = &[
    ("netopt.py", include_str!("../resources/netopt/netopt.py")),
    (
        "netopt_core.py",
        include_str!("../resources/netopt/netopt_core.py"),
    ),
    (
        "backend_macos.py",
        include_str!("../resources/netopt/backend_macos.py"),
    ),
    (
        "backend_linux.py",
        include_str!("../resources/netopt/backend_linux.py"),
    ),
    (
        "backend_windows.py",
        include_str!("../resources/netopt/backend_windows.py"),
    ),
];

// ---------------------------------------------------------------------------
// 脚本释放与解释器定位
// ---------------------------------------------------------------------------

/// 释放内嵌源码到 `<配置目录>/netopt/`，返回入口脚本的绝对路径。
///
/// 内容一致就不重写：这个函数在每次巡检前都会调用，没必要每次落盘。
/// 版本升级时内容自然不同 → 自动覆盖。
pub fn ensure_scripts() -> Result<PathBuf> {
    let dir = paths::appdata_dir().join("netopt");
    std::fs::create_dir_all(&dir)
        .map_err(|e| AppError::internal(format!("无法创建目录 {}：{e}", dir.display())))?;
    for (name, body) in SCRIPTS {
        let path = dir.join(name);
        let same = std::fs::read(&path)
            .map(|old| old == body.as_bytes())
            .unwrap_or(false);
        if !same {
            std::fs::write(&path, body.as_bytes()).map_err(|e| {
                AppError::internal(format!("无法写入 {}：{e}", path.display()))
            })?;
        }
    }
    Ok(dir.join("netopt.py"))
}

/// 校验脚本路径形状：必须是绝对路径，且落在名为 `netopt` 的目录下、文件名为 `netopt.py`
///
/// 提权子进程会以 root 身份执行这个路径，所以不能是任意文件。不比对"用户配置目录"
/// 是因为 root 的 `appdata_dir()` 不是用户的那个，比对必然失败。
fn validate_script(script: &str) -> Result<PathBuf> {
    let p = PathBuf::from(script);
    let ok = p.is_absolute()
        && p.file_name().map(|n| n == "netopt.py").unwrap_or(false)
        && p.parent()
            .and_then(|d| d.file_name())
            .map(|n| n == "netopt")
            .unwrap_or(false)
        && p.exists();
    if !ok {
        return Err(AppError::coded(crate::error::E7001)
            .with_detail(format!("拒绝执行非 netopt 脚本：{script}")));
    }
    Ok(p)
}

/// 家目录：netopt 的基线快照放在 `~/Library/Application Support/netopt/`，
/// 提权身份下 `HOME` 可能是 `/var/root`，必须显式传调用者那一份，
/// 否则"预览看到的快照"和"提权写入的快照"会分居两处，回滚找不到。
pub fn user_home() -> Option<String> {
    #[cfg(target_os = "windows")]
    let key = "USERPROFILE";
    #[cfg(not(target_os = "windows"))]
    let key = "HOME";
    std::env::var(key).ok().filter(|s| !s.trim().is_empty())
}

/// 候选解释器（绝对路径优先：GUI 进程与提权子进程的 PATH 都很窄，裸命令常常找不到）
#[cfg(target_os = "macos")]
const PYTHON_CANDIDATES: &[&str] = &[
    "/opt/homebrew/bin/python3",
    "/usr/local/bin/python3",
    // CLT 里的真实解释器。优先于 /usr/bin/python3：后者是 shim，
    // 命令行工具没装时会弹系统安装框，不该在探测阶段弹
    "/Library/Developer/CommandLineTools/usr/bin/python3",
    "/Applications/Xcode.app/Contents/Developer/usr/bin/python3",
    "/usr/bin/python3",
    "python3",
];

// py.exe 在 C:\Windows，提权子进程的 PATH 里也一定找得到；python / python3
// 可能只是 WindowsApps 的应用执行别名（未安装 Python 时会拉起商店），排后面
#[cfg(target_os = "windows")]
const PYTHON_CANDIDATES: &[&str] = &["py", "python", "python3"];

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const PYTHON_CANDIDATES: &[&str] = &["python3", "python"];

/// 找可用的 Python 3 解释器
pub fn find_python() -> Option<String> {
    if let Ok(custom) = std::env::var("SUNET_PYTHON") {
        let custom = custom.trim();
        if !custom.is_empty() && probe_python(custom) {
            return Some(custom.to_string());
        }
    }
    for c in PYTHON_CANDIDATES {
        // 绝对路径先看文件在不在：`-V` 在不存在的路径上只会报错，但在 macOS 的
        // `/usr/bin/python3` shim 上会拉起安装提示，所以能静态排除的先排除
        if c.starts_with('/') && !Path::new(c).exists() {
            continue;
        }
        if probe_python(c) {
            return Some((*c).to_string());
        }
    }
    None
}

fn probe_python(bin: &str) -> bool {
    let mut cmd = std::process::Command::new(bin);
    cmd.arg("-V");
    match run_capture(cmd, Duration::from_secs(5)) {
        Ok((0, out, _)) => out.trim().starts_with("Python 3"),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// 调用
// ---------------------------------------------------------------------------

/// 一次 netopt 调用的完整参数。
///
/// 提权子进程拿到的就是这个结构（走 IPC 载荷），保证父进程与 root 子进程
/// 用的是同一个解释器、同一份脚本、同一份快照目录。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Invocation {
    pub python: String,
    pub script: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
}

impl Invocation {
    /// `only` : `None` = 全部子系统；`Some("dns,ipv6")` = 只处理这些
    pub fn new(only: Option<String>) -> Result<Self> {
        let python = find_python().ok_or_else(|| {
            AppError::coded(crate::error::E7001).with_detail("未找到 Python 3 解释器")
        })?;
        let script = ensure_scripts()?;
        Ok(Self {
            python,
            script: script.to_string_lossy().to_string(),
            only: normalize_only(only)?,
            home: user_home(),
        })
    }

    /// 从载荷还原（提权子进程侧，多一道脚本形状校验）
    pub fn from_payload(data: &Value) -> Result<Self> {
        let python = data["python"]
            .as_str()
            .ok_or_else(|| AppError::coded(crate::error::E1004).with_detail("缺少 python"))?;
        let script = data["script"]
            .as_str()
            .ok_or_else(|| AppError::coded(crate::error::E1004).with_detail("缺少 script"))?;
        validate_script(script)?;
        let only = data["only"].as_str().map(|s| s.to_string());
        Ok(Self {
            python: python.to_string(),
            script: script.to_string(),
            only: normalize_only(only)?,
            home: data["home"].as_str().map(|s| s.to_string()),
        })
    }

    fn command(&self, sub: &str, commit: bool) -> std::process::Command {
        let mut cmd = std::process::Command::new(&self.python);
        cmd.arg(&self.script).arg("--json").arg(sub);
        if let Some(only) = self.only.as_deref() {
            cmd.arg("--only").arg(only);
        }
        if commit {
            cmd.arg("--commit");
        }
        // 统一 UTF-8：Windows 上 Python 默认按控制台代码页写管道，中文会变乱码
        cmd.env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1");
        if let Some(home) = self.home.as_deref() {
            cmd.env("HOME", home).env("USERPROFILE", home);
        }
        cmd
    }

    /// 只读巡检：`audit`
    pub fn audit(&self) -> Result<Value> {
        self.exec("audit", false, AUDIT_TIMEOUT)
    }

    /// 预览修复计划：`apply`（不带 `--commit`，不改动任何配置）
    pub fn preview(&self) -> Result<Value> {
        self.exec("apply", false, AUDIT_TIMEOUT)
    }

    /// 真正执行：`apply --commit`（提权子进程里调用）
    pub fn commit(&self) -> Result<Value> {
        self.exec("apply", true, APPLY_TIMEOUT)
    }

    /// 回滚：`rollback --to <latest|baseline> --commit`
    pub fn rollback(&self, to: &str) -> Result<Value> {
        let to = if to == "baseline" { "baseline" } else { "latest" };
        let mut cmd = self.command("rollback", true);
        cmd.arg("--to").arg(to);
        finish(cmd, APPLY_TIMEOUT)
    }

    fn exec(&self, sub: &str, commit: bool, timeout: Duration) -> Result<Value> {
        finish(self.command(sub, commit), timeout)
    }
}

fn normalize_only(only: Option<String>) -> Result<Option<String>> {
    let Some(raw) = only else { return Ok(None) };
    let mut picked: Vec<&str> = Vec::new();
    for item in raw.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let hit = SECTION_IDS.iter().find(|s| **s == item).ok_or_else(|| {
            AppError::coded(crate::error::E4002).with_detail(format!("未知子系统：{item}"))
        })?;
        if !picked.contains(hit) {
            picked.push(hit);
        }
    }
    Ok(if picked.is_empty() {
        None
    } else {
        Some(picked.join(","))
    })
}

/// 跑一次并解析 stdout 的 JSON。
///
/// netopt 在"有告警 / 有故障"时退出码非 0，但 **stdout 仍是完整 JSON** ——
/// 所以这里不按退出码判失败，只在解析不出来时才报错。
fn finish(cmd: std::process::Command, timeout: Duration) -> Result<Value> {
    let (code, out, err) =
        run_capture(cmd, timeout).map_err(|e| AppError::internal(format!("执行 netopt 失败：{e}")))?;
    let text = out.trim();
    if text.is_empty() {
        return Err(AppError::internal(format!(
            "netopt 没有输出（退出码 {code}）：{}",
            first_line(&err)
        )));
    }
    serde_json::from_str::<Value>(text).map_err(|e| {
        AppError::internal(format!("netopt 输出不是合法 JSON（退出码 {code}）：{e}"))
    })
}

fn first_line(s: &str) -> String {
    s.trim().lines().next().unwrap_or("").to_string()
}

/// 带超时地执行，返回 (退出码, stdout, stderr)。
///
/// 必须先起读者线程再等：输出超过管道缓冲（Windows 约 4KB / POSIX 64KB）时，
/// 父进程还在 `wait` 而子进程阻塞在 `write` 上 —— 直接死锁。
fn run_capture(mut cmd: std::process::Command, timeout: Duration) -> Result<(i32, String, String)> {
    use std::io::Read;

    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // 提权子进程 / 父进程都不该因为拉起 python 闪一个黑框
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().map_err(|e| {
        AppError::internal(format!(
            "无法启动 {}：{e}",
            cmd.get_program().to_string_lossy()
        ))
    })?;
    let mut out_pipe = child
        .stdout
        .take()
        .ok_or_else(|| AppError::internal("无法读取 netopt 的输出管道"))?;
    let mut err_pipe = child
        .stderr
        .take()
        .ok_or_else(|| AppError::internal("无法读取 netopt 的错误管道"))?;

    let (out_buf, err_buf, status, timed_out) = std::thread::scope(|scope| {
        let h_out = scope.spawn(move || {
            let mut buf = Vec::new();
            let _ = out_pipe.read_to_end(&mut buf);
            buf
        });
        let h_err = scope.spawn(move || {
            let mut buf = Vec::new();
            let _ = err_pipe.read_to_end(&mut buf);
            buf
        });
        let deadline = std::time::Instant::now() + timeout;
        let mut timed_out = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        timed_out = true;
                        let _ = child.kill();
                        let _ = child.wait();
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(40));
                }
                Err(e) => {
                    log::warn!(target: "netopt", "等待子进程失败：{e}");
                    let _ = child.kill();
                    break None;
                }
            }
        };
        (
            h_out.join().unwrap_or_default(),
            h_err.join().unwrap_or_default(),
            status,
            timed_out,
        )
    });

    if timed_out {
        return Err(AppError::internal(format!(
            "netopt 超过 {} 秒未完成，已中断",
            timeout.as_secs()
        )));
    }
    let code = status.and_then(|s| s.code()).unwrap_or(-1);
    Ok((
        code,
        String::from_utf8_lossy(&out_buf).to_string(),
        String::from_utf8_lossy(&err_buf).to_string(),
    ))
}

// ---------------------------------------------------------------------------
// 输出模型（给前端）
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Finding {
    pub code: String,
    pub level: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PlanItem {
    pub desc: String,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub needs_admin: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Section {
    pub id: String,
    pub title: String,
    pub supported: bool,
    pub status: String,
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub plan: Vec<PlanItem>,
    #[serde(default)]
    pub unsupported_reason: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct Summary {
    #[serde(default)]
    pub ok: u32,
    #[serde(default)]
    pub warn: u32,
    #[serde(default)]
    pub fail: u32,
    #[serde(default)]
    pub unsupported: u32,
}

/// 界面用的巡检结果。
///
/// `available = false` 时 `error` 一定有值（例如"未找到 Python 3"）—— 界面据此
/// 把入口置灰并给出原因，而不是让用户点了没反应。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct View {
    pub available: bool,
    pub version: String,
    pub generated_at: String,
    pub mode: String,
    pub summary: Summary,
    pub sections: Vec<Section>,
    pub exit_code: i32,
    /// 实际使用的解释器（排查"跑的是哪个 python"用）
    pub python: Option<String>,
    pub error: Option<String>,
}

impl View {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            available: false,
            error: Some(reason.into()),
            ..Default::default()
        }
    }

    /// 从 netopt 的 JSON 文档构造
    pub fn from_doc(doc: &Value, python: Option<String>) -> Self {
        let parsed: Vec<Section> =
            serde_json::from_value(doc["sections"].clone()).unwrap_or_default();
        // 只保留已知子系统，避免上游多出字段时界面出现无法解释的行
        let sections: Vec<Section> = parsed
            .into_iter()
            .filter(|s| SECTION_IDS.contains(&s.id.as_str()))
            .collect();
        let summary = serde_json::from_value::<Summary>(doc["summary"].clone());
        let summary = summary.unwrap_or_else(|_| Summary {
            ok: sections.iter().filter(|s| s.status == "ok").count() as u32,
            warn: sections.iter().filter(|s| s.status == "warn").count() as u32,
            fail: sections.iter().filter(|s| s.status == "fail").count() as u32,
            unsupported: sections
                .iter()
                .filter(|s| s.status == "unsupported")
                .count() as u32,
        });
        Self {
            available: true,
            version: doc["version"].as_str().unwrap_or_default().to_string(),
            generated_at: doc["generated_at"].as_str().unwrap_or_default().to_string(),
            mode: doc["mode"].as_str().unwrap_or_default().to_string(),
            summary,
            sections,
            exit_code: doc["exit_code"].as_i64().unwrap_or(0) as i32,
            python,
            error: doc["error"].as_str().map(|s| s.to_string()),
        }
    }

    /// 待执行的变更条数
    pub fn plan_count(&self) -> usize {
        self.sections.iter().map(|s| s.plan.len()).sum()
    }
}

// ---------------------------------------------------------------------------
// 提权任务侧：把 JSON 文档翻成界面要的步骤
// ---------------------------------------------------------------------------

/// 执行 `apply --commit`（提权子进程内调用）
pub fn apply_elevated(data: &Value) -> crate::ipc::TaskOutput {
    let inv = match Invocation::from_payload(data) {
        Ok(v) => v,
        Err(e) => return crate::ipc::TaskOutput::fail(e),
    };
    let doc = match inv.commit() {
        Ok(v) => v,
        Err(e) => return crate::ipc::TaskOutput::fail(e),
    };
    let actions = doc["actions"].as_array().cloned().unwrap_or_default();
    let mut out = crate::ipc::TaskOutput {
        ok: true,
        ..crate::ipc::TaskOutput::ok()
    };
    let mut failed = 0usize;
    for a in &actions {
        let result = a["result"].as_str().unwrap_or("unknown");
        let desc = a["desc"].as_str().unwrap_or("未命名操作");
        let applied = matches!(result, "applied" | "planned");
        // netopt 执行完会自己回读，`applied` 即已确认
        let verified = result == "applied";
        if !applied {
            failed += 1;
        }
        let note = match result {
            "applied" => "",
            "planned" => "（预览，未执行）",
            "skipped_need_admin" => "（缺少管理员权限，未执行）",
            "error" => "（执行失败）",
            _ => "（结果未知）",
        };
        out.steps.push(crate::ipc::ApplyStep {
            kind: a["section"].as_str().unwrap_or("netopt").to_string(),
            applied,
            verified,
            message: format!("{desc}{note}"),
        });
    }
    if actions.is_empty() {
        out.steps.push(crate::ipc::ApplyStep {
            kind: "netopt".into(),
            applied: true,
            verified: true,
            message: "网络配置已符合建议，没有需要执行的变更".into(),
        });
    }
    // 全量文档留在 extra 里：父进程要把"执行后的真实状态"交给界面复现
    out.extra = doc;
    out.ok = failed == 0;
    if !out.ok {
        out.error = Some(
            AppError::coded(crate::error::E4003)
                .with_detail(format!("{failed} 项操作未能完成，详见步骤")),
        );
    }
    out
}

/// 回滚到最近一次 / 首次执行前的快照（提权子进程内调用）
pub fn rollback_elevated(data: &Value) -> crate::ipc::TaskOutput {
    let to = data["to"].as_str().unwrap_or("latest");
    let inv = match Invocation::from_payload(data) {
        Ok(v) => v,
        Err(e) => return crate::ipc::TaskOutput::fail(e),
    };
    let doc = match inv.rollback(to) {
        Ok(v) => v,
        Err(e) => return crate::ipc::TaskOutput::fail(e),
    };
    let mut out = crate::ipc::TaskOutput::ok();
    let actions = doc["actions"].as_array().cloned().unwrap_or_default();
    let mut failed = 0usize;
    for a in &actions {
        let result = a["result"].as_str().unwrap_or("unknown");
        let applied = matches!(result, "applied" | "planned");
        if !applied {
            failed += 1;
        }
        out.steps.push(crate::ipc::ApplyStep {
            kind: "netopt".into(),
            applied,
            verified: result == "applied",
            message: format!(
                "{}{}",
                a["desc"].as_str().unwrap_or("回滚"),
                if applied { "" } else { "（失败）" }
            ),
        });
    }
    if actions.is_empty() {
        out.steps.push(crate::ipc::ApplyStep {
            kind: "netopt".into(),
            applied: true,
            verified: true,
            message: "快照中没有需要回滚的项".into(),
        });
    }
    if doc["error"].is_string() {
        out.ok = false;
        out.error = Some(
            AppError::coded(crate::error::E4003)
                .with_detail(doc["error"].as_str().unwrap_or("回滚失败")),
        );
    } else if failed > 0 {
        out.ok = false;
        out.error = Some(
            AppError::coded(crate::error::E4003).with_detail(format!("{failed} 项回滚未能完成")),
        );
    }
    out.extra = doc;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_is_whitelisted_and_deduped() {
        assert_eq!(
            normalize_only(Some("dns, dns ,ipv6".into())).unwrap(),
            Some("dns,ipv6".into())
        );
        assert_eq!(normalize_only(Some(" , ".into())).unwrap(), None);
        assert!(normalize_only(Some("nope".into())).is_err());
    }

    #[test]
    fn script_path_shape_is_enforced() {
        // 非 netopt.py / 非 netopt 目录 / 相对路径 一律拒绝
        assert!(validate_script("/tmp/netopt/netopt.py").is_err());
        assert!(validate_script("/tmp/evil/netopt.py").is_err());
        assert!(validate_script("netopt/netopt.py").is_err());
        assert!(validate_script(&format!(
            "{}/netopt/netopt.py",
            std::env::temp_dir().display()
        ))
        .is_err());
    }

    #[test]
    fn view_is_built_from_document() {
        let doc = serde_json::json!({
            "version": "1.0.0",
            "generated_at": "2026-11-01T14:50:12+08:00",
            "mode": "audit",
            "exit_code": 10,
            "summary": { "ok": 3, "warn": 1, "fail": 0, "unsupported": 1 },
            "sections": [
                { "id": "dns", "title": "DNS 解析器", "supported": true, "status": "warn",
                  "findings": [{ "code": "dns.single", "level": "warn", "message": "只有一台解析器" }],
                  "plan": [{ "desc": "设置 DNS", "argv": ["networksetup"], "needs_admin": true }] },
                { "id": "unknown", "title": "未知", "supported": true, "status": "ok" }
            ]
        });
        let v = View::from_doc(&doc, Some("/usr/bin/python3".into()));
        assert!(v.available);
        assert_eq!(v.sections.len(), 1, "未知子系统要被过滤掉");
        assert_eq!(v.plan_count(), 1);
        assert_eq!(v.summary.warn, 1);
        assert_eq!(v.python.as_deref(), Some("/usr/bin/python3"));
    }

    #[test]
    fn unavailable_view_carries_reason() {
        let v = View::unavailable("未找到 Python 3");
        assert!(!v.available);
        assert_eq!(v.error.as_deref(), Some("未找到 Python 3"));
    }
}

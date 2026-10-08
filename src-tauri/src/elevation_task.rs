//! 静默提权通道（Windows）
//!
//! 背景：模型 C 下 hosts / DNS 的写入每次都要走 `ShellExecuteExW runas`，
//! 也就是**每次都得让用户点一次 UAC**。macOS 侧用常驻 root helper 解决了这件事
//! （装一次之后免密码），Windows 侧原本没有等价物。
//!
//! 这里用**计划任务**做等价物：
//!   1. 用户显式点「安装静默提权通道」→ 走**一次**提权（UAC）执行
//!      `schtasks /Create /RL HIGHEST`，把 `SuNet.exe --silent-worker` 注册成最高权限任务；
//!   2. 之后每次写入只要 `schtasks /Run`（**不需要 UAC**）：工作进程读走 `SuNet/ipc`
//!      里刚写下的载荷、执行、写回结果文件，做完即退 —— 不常驻、不留后台进程。
//!
//! ## 安全边界（必须知情）
//!
//! 这条通道等价于「一次授权、长期有效」：任何**以该用户身份**运行的代码都能触发它，
//! 从而在没有 UAC 确认的情况下完成 hosts / DNS / 代理写入。因此：
//!   - **默认关闭**，必须由用户显式安装（设置 → 权限里的开关）；
//!   - 卸载后立刻回退到逐次 UAC 的 `runas` 路径；
//!   - 工作进程仍然只执行 `task_runner::execute` 的白名单任务；载荷还要过
//!     `fs_security::verify_payload_file`（属主 / 目录形状 / 非符号链接）**且必须是
//!     60 秒内刚写下的**，避免把上次崩溃残留的载荷再执行一遍。
//!
//! 非 Windows 平台没有这条通道（macOS 有常驻 helper），所有接口退化为「不支持」。

#[cfg(target_os = "windows")]
mod imp {
    use crate::error::{AppError, Result};
    use crate::ipc::{self, TaskOutput};
    use serde::{Deserialize, Serialize};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    use std::time::{Duration, Instant, SystemTime};

    /// 计划任务名（安装 / 查询 / 运行 / 删除都用它）
    const TASK_NAME: &str = "SuNet-SilentElevation";
    /// 只认「刚写下」的载荷：更早的一律视为崩溃残留，不再执行（并顺手清理）
    const FRESH_WINDOW: Duration = Duration::from_secs(60);
    /// 工作进程等待作业出现的上限（正常情况下父进程已先写好载荷，无需等待）
    const WORKER_WAIT: Duration = Duration::from_secs(3);
    /// 轮询结果文件的间隔
    const POLL_INTERVAL: Duration = Duration::from_millis(50);

    /// 用户设置：是否启用静默通道（启动与保存设置时由 config 同步进来）
    static ENABLED: AtomicBool = AtomicBool::new(false);
    /// 安装状态缓存：0 = 未知，1 = 可用，2 = 不可用
    static STATE: AtomicU8 = AtomicU8::new(0);

    #[derive(Serialize, Deserialize)]
    struct Marker {
        /// 安装时写进计划任务的可执行文件路径（换了路径就必须重装）
        exe: String,
        installed_at: String,
    }

    fn marker_path() -> PathBuf {
        crate::paths::appdata_dir().join("elevation_task.json")
    }

    pub fn supported() -> bool {
        true
    }

    /// 界面上这条通道叫什么
    pub fn noun() -> &'static str {
        "静默提权通道"
    }

    /// 状态说明（直接给界面显示）
    pub fn detail(installed: bool, enabled: bool) -> String {
        if installed {
            "已安装：hosts / DNS 写入不再弹 UAC（一次授权、长期有效；卸载后恢复逐次确认）"
                .to_string()
        } else if enabled {
            "开关已打开但通道未安装，仍会逐次弹 UAC".to_string()
        } else {
            "未安装：每次写 hosts / DNS 都会弹一次 UAC".to_string()
        }
    }

    pub fn enabled() -> bool {
        ENABLED.load(Ordering::Relaxed)
    }

    pub fn set_enabled(on: bool) {
        ENABLED.store(on, Ordering::Relaxed);
    }

    /// 安装状态变了（安装 / 卸载 / 任务被外部删除）后调用
    pub fn invalidate_cache() {
        STATE.store(0, Ordering::Relaxed);
    }

    /// 设置已启用 **且** 通道装好 —— 可以走静默路径
    pub fn usable() -> bool {
        enabled() && is_installed()
    }

    /// 计划任务存在，且指向当前这个 exe
    pub fn is_installed() -> bool {
        match STATE.load(Ordering::Relaxed) {
            1 => return true,
            2 => return false,
            _ => {}
        }
        let ok = probe();
        STATE.store(if ok { 1 } else { 2 }, Ordering::Relaxed);
        ok
    }

    fn probe() -> bool {
        let Some(m) = read_marker() else {
            return false;
        };
        let Ok(exe) = crate::paths::exe_path() else {
            return false;
        };
        // 升级换了安装路径时必须重装：否则任务会去拉旧路径，甚至那个文件已经不存在
        if !m.exe.eq_ignore_ascii_case(&exe.to_string_lossy()) {
            log::info!(target: "elevation", "静默通道记录的可执行文件与当前不一致，需要重装");
            return false;
        }
        schtasks(&["/Query", "/TN", TASK_NAME]).unwrap_or(false)
    }

    fn read_marker() -> Option<Marker> {
        let text = std::fs::read_to_string(marker_path()).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn write_marker(exe: &str) -> Result<()> {
        let m = Marker {
            exe: exe.to_string(),
            installed_at: chrono::Local::now().to_rfc3339(),
        };
        let text = serde_json::to_string_pretty(&m)?;
        std::fs::write(marker_path(), text.as_bytes())?;
        Ok(())
    }

    /// 执行 schtasks；返回是否退出码为 0（失败只记日志，不抛错 —— 调用方按 false 回退）
    fn schtasks(args: &[&str]) -> Result<bool> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = std::process::Command::new("schtasks.exe");
        cmd.args(args).creation_flags(CREATE_NO_WINDOW);
        let out = cmd
            .output()
            .map_err(|e| AppError::internal(format!("无法执行 schtasks：{e}")))?;
        if !out.status.success() {
            // schtasks 的输出按 OEM 代码页编码，必须按 OEM 解码（否则日志里是一片方块）
            let text = crate::os::winapi::decode_console_output(&out.stderr);
            log::warn!(target: "elevation", "schtasks {args:?} 失败：{text}");
            return Ok(false);
        }
        Ok(true)
    }

    // -----------------------------------------------------------------------
    // 安装 / 卸载（**只能在已提权的子进程里调用**，见 task_runner 的 silent_install）
    // -----------------------------------------------------------------------

    pub fn install() -> Result<String> {
        let exe = crate::paths::exe_path()?;
        let tr = format!("\"{}\" --silent-worker", exe.display());
        // /SC ONCE + /ST 00:00 => 触发时间落在「今天 00:00」，永远是过去时刻，
        // 任务不会自己跑起来；我们只用 /Run 主动触发它。
        let ok = schtasks(&[
            "/Create",
            "/TN",
            TASK_NAME,
            "/TR",
            &tr,
            "/SC",
            "ONCE",
            "/ST",
            "00:00",
            "/RL",
            "HIGHEST",
            "/F",
        ])?;
        if !ok {
            return Err(AppError::internal("schtasks /Create 失败（详见日志）"));
        }
        write_marker(&exe.to_string_lossy())?;
        invalidate_cache();
        Ok("已安装：之后的 hosts / DNS 写入不再弹 UAC".into())
    }

    pub fn uninstall() -> Result<String> {
        let existed = schtasks(&["/Delete", "/TN", TASK_NAME, "/F"]).unwrap_or(false);
        let _ = std::fs::remove_file(marker_path());
        invalidate_cache();
        Ok(if existed {
            "已卸载：恢复为每次操作询问 UAC".into()
        } else {
            "通道本来就不存在，已清理记录".into()
        })
    }

    // -----------------------------------------------------------------------
    // 执行任务（父进程侧）
    // -----------------------------------------------------------------------

    /// 走静默通道执行一个任务：写载荷 → 触发计划任务 → 等结果文件。
    ///
    /// `Err` 只表示「通道不可用 / 没拿到结果」，调用方应当回退到 `runas`。
    /// 任务本身的失败通过 `TaskOutput::ok = false` 正常返回。
    pub fn run(task: &str, payload: serde_json::Value, timeout: Duration) -> Result<TaskOutput> {
        let (in_path, out_path) = ipc::prepare(task, payload)?;
        let started = Instant::now();
        if let Err(e) = trigger() {
            ipc::cleanup(&in_path, &out_path);
            return Err(e);
        }
        loop {
            if out_path.exists() {
                let out = ipc::collect_out(&out_path, 0);
                ipc::cleanup(&in_path, &out_path);
                log::info!(
                    target: "elevation",
                    "静默通道任务 {task}：ok={}，耗时 {}ms",
                    out.ok,
                    started.elapsed().as_millis()
                );
                return Ok(out);
            }
            if started.elapsed() > timeout {
                ipc::cleanup(&in_path, &out_path);
                return Err(AppError::coded(crate::error::E1003).with_detail(format!(
                    "静默提权任务 {task} 超过 {} 秒未返回结果",
                    timeout.as_secs()
                )));
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    fn trigger() -> Result<()> {
        if schtasks(&["/Run", "/TN", TASK_NAME])? {
            return Ok(());
        }
        // 任务被删 / 被禁用 / 上一次还没跑完都会到这里 → 立刻让上层回退 runas
        invalidate_cache();
        Err(AppError::internal(
            "无法启动静默提权任务（可能已被删除，或上一次仍在运行）",
        ))
    }

    // -----------------------------------------------------------------------
    // 执行任务（工作进程侧，由计划任务以最高权限拉起）
    // -----------------------------------------------------------------------

    /// `--silent-worker` 入口：做完一件事就退出（不常驻、不建托盘、不进 Tauri setup）
    pub fn worker_main() -> i32 {
        crate::logging::init(
            &crate::paths::logs_dir(),
            log::LevelFilter::Info,
            true,
            7,
        );
        log::info!(
            target: "elevation",
            "静默提权工作进程启动（elevated={}）",
            crate::os::privilege::is_elevated()
        );

        let Some((in_path, out_path)) = wait_for_job() else {
            log::info!(target: "elevation", "静默提权工作进程：没有等待中的作业，退出");
            return 0;
        };

        let result = (|| -> Result<TaskOutput> {
            let payload = ipc::read_payload(&in_path.to_string_lossy())?;
            Ok(crate::task_runner::execute(&payload.task, &payload.data))
        })();

        let (out, code) = match result {
            Ok(o) => {
                let c = if o.ok {
                    0
                } else {
                    o.error.as_ref().map(|e| e.exit_code()).unwrap_or(6)
                };
                (o, c)
            }
            Err(e) => {
                let c = e.exit_code();
                (TaskOutput::fail(e), c)
            }
        };
        if let Err(e) = ipc::write_output(&out_path.to_string_lossy(), &out) {
            log::error!(target: "elevation", "静默提权工作进程写结果失败：{e}");
        }
        log::info!(target: "elevation", "静默提权工作进程退出：code={code}");
        code
    }

    fn wait_for_job() -> Option<(PathBuf, PathBuf)> {
        let dir = crate::paths::ipc_dir();
        let deadline = Instant::now() + WORKER_WAIT;
        loop {
            if let Some(hit) = scan_job(&dir) {
                return Some(hit);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// 取「最新且够新」的载荷；顺带清掉过期的崩溃残留
    fn scan_job(dir: &Path) -> Option<(PathBuf, PathBuf)> {
        let mut best: Option<(SystemTime, PathBuf)> = None;
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("task-") || !name.ends_with(".in.json") {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let Ok(mtime) = meta.modified() else { continue };
            let age = SystemTime::now()
                .duration_since(mtime)
                .unwrap_or_default();
            let path = entry.path();
            if age > FRESH_WINDOW {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            // 与提权子进程同一套校验：属主 / 目录形状 / 非重解析点
            if crate::os::fs_security::verify_payload_file(&path).is_err() {
                continue;
            }
            if best.as_ref().map_or(true, |(t, _)| mtime > *t) {
                best = Some((mtime, path));
            }
        }
        let (_, in_path) = best?;
        let out_path = PathBuf::from(in_path.to_string_lossy().replace(".in.json", ".out.json"));
        Some((in_path, out_path))
    }
}

/// macOS：静默提权由**常驻 root helper** 承担（见 `helper/` 模块）。
///
/// 这里把「静默提权通道」的接口映射到助手的安装状态上，于是「设置 → 提权」
/// 在两个平台是同一套 UI、同一套文案口径 —— 差别只在底层实现：
/// Windows 用计划任务，macOS 用 LaunchDaemon 助手。
#[cfg(target_os = "macos")]
mod macos_impl {
    use crate::error::Result;
    use crate::ipc::TaskOutput;
    use std::time::Duration;

    pub fn supported() -> bool {
        true
    }

    /// macOS 没有单独的开关：装了助手即免密（helper 可用时直接生效）
    pub fn enabled() -> bool {
        is_installed()
    }

    pub fn set_enabled(_on: bool) {}

    pub fn invalidate_cache() {}

    /// 免密执行的前提是守护进程真的在跑（socket 存在）
    pub fn usable() -> bool {
        crate::helper::available()
    }

    pub fn is_installed() -> bool {
        crate::helper::installed()
    }

    pub fn noun() -> &'static str {
        "免密提权助手"
    }

    pub fn detail(installed: bool, _enabled: bool) -> String {
        if !installed {
            return "未安装：每次写 hosts / DNS / 系统代理都会弹一次系统授权框".to_string();
        }
        if crate::helper::available() {
            "已安装：切换方案不再输密码（后台助手常驻；卸载后恢复逐次授权）".to_string()
        } else {
            "已安装但守护进程未运行：可能需要在「系统设置 → 通用 → 登录项与扩展」里放行，或重启后再试"
                .to_string()
        }
    }

    /// 安装 / 卸载自带 osascript 提权（一次性输密码），不走 `run_elevated`
    pub fn install() -> Result<String> {
        crate::elevation::run_self_command_elevated(&["--helper-install"])
    }

    pub fn uninstall() -> Result<String> {
        crate::elevation::run_self_command_elevated(&["--helper-uninstall"])
    }

    /// 计划任务通道只属于 Windows；macOS 的提权路径在 `elevation.rs` 里直接走 helper
    pub fn run(
        _task: &str,
        _payload: serde_json::Value,
        _timeout: Duration,
    ) -> Result<TaskOutput> {
        Err(crate::error::AppError::internal(
            "macOS 不使用计划任务通道（请走 helper）",
        ))
    }

    pub fn worker_main() -> i32 {
        0
    }
}

/// 兜底：理论到不了这里（`os/mod.rs` 对非 win/mac 直接 compile_error）
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod unsupported {
    use crate::error::{AppError, Result};
    use crate::ipc::TaskOutput;
    use std::time::Duration;

    pub fn supported() -> bool {
        false
    }
    pub fn enabled() -> bool {
        false
    }
    pub fn set_enabled(_on: bool) {}
    pub fn invalidate_cache() {}
    pub fn usable() -> bool {
        false
    }
    pub fn is_installed() -> bool {
        false
    }
    pub fn noun() -> &'static str {
        "静默提权"
    }
    pub fn detail(_installed: bool, _enabled: bool) -> String {
        "本平台不支持".to_string()
    }
    pub fn install() -> Result<String> {
        Err(AppError::internal("静默提权通道不受支持"))
    }
    pub fn uninstall() -> Result<String> {
        Err(AppError::internal("静默提权通道不受支持"))
    }
    pub fn run(_task: &str, _payload: serde_json::Value, _timeout: Duration) -> Result<TaskOutput> {
        Err(AppError::internal("静默提权通道不受支持"))
    }
    pub fn worker_main() -> i32 {
        0
    }
}

#[cfg(target_os = "windows")]
pub use imp::*;

#[cfg(target_os = "macos")]
pub use macos_impl::*;

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub use unsupported::*;

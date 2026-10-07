//! 全局状态与并发控制（设计方案 §5.4.1 / §6.3 / §7.2）

use crate::config::Config;
use crate::error::{AppError, Result};
use crate::os::system_proxy::ProxyState;
use crate::paths;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 共享句柄：Tauri 托管 `Arc<AppState>`，命令层 clone 后丢进 blocking 线程池
pub type SharedState = std::sync::Arc<AppState>;

/// 进程内锁：所有 apply_* / rollback 命令先取这把锁（§7.2 第 1 层）
pub struct AppState {
    pub cfg: Mutex<Config>,
    /// Some(err) = 只读模式（配置由更新版本创建 / 损坏），任何写操作被拒
    pub read_only: Mutex<Option<AppError>>,
    pub apply_lock: Mutex<()>,
    pub runtime: Mutex<RuntimeStatus>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ClearState {
    pub before: ProxyState,
    pub cleared_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CrashInfo {
    pub detected_at: String,
    pub marker: String,
    pub snapshot_id: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct RuntimeStatus {
    /// 是否存在未回滚的变更（§6.3）
    pub dirty: bool,
    /// 通知被系统抑制 / 发送失败时的降级标记（§5.4.5）
    pub warn_unread: bool,
    /// 热键真实注册结果 —— 不允许读配置文件就显示"已启用"（§5.5.3）
    pub shortcut_registered: bool,
    pub shortcut_error: Option<AppError>,
    pub shortcut_binding: String,
    /// 彻底清零的撤销暂存（**只在内存**，不落盘，§5.5.7）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_clear: Option<ClearState>,
    #[serde(skip)]
    pub clear_deadline: Option<Instant>,
    pub crash_recovery: Option<CrashInfo>,
    pub lock_contended: bool,
    /// 最近一次已知的真实 DNS 摘要（v4, v6），避免为了 tooltip 反复起 PowerShell
    #[serde(skip)]
    pub dns_summary: Option<(String, String)>,
    /// 快捷面板正在执行操作：此时失焦不收起（否则 UAC 一抢焦点面板就消失，
    /// 用户看不到操作结果）
    #[serde(skip)]
    pub quick_pinned: bool,
    /// 面板自上次显示以来是否真的获得过焦点。
    /// 窗口刚 show 出来时系统可能立即投递一次失焦事件（启动竞态），
    /// 若无条件收起，面板会在弹出的瞬间消失 —— 表现为"点托盘没反应"。
    #[serde(skip)]
    pub quick_focus_seen: bool,
}

impl RuntimeStatus {
    pub fn clear_expired(&self) -> bool {
        match self.clear_deadline {
            Some(t) => Instant::now() > t,
            None => true,
        }
    }
}

impl AppState {
    pub fn new(cfg: Config, read_only: Option<AppError>) -> Self {
        let binding = cfg.settings.shortcuts.clear_proxy.binding.clone();
        Self {
            cfg: Mutex::new(cfg),
            read_only: Mutex::new(read_only),
            apply_lock: Mutex::new(()),
            runtime: Mutex::new(RuntimeStatus {
                shortcut_binding: binding,
                ..Default::default()
            }),
        }
    }

    /// 取进程内申请锁；只读模式下拒绝
    pub fn lock_apply(&self) -> Result<std::sync::MutexGuard<'_, ()>> {
        self.assert_writable()?;
        self.apply_lock
            .lock()
            .map_err(|_| AppError::internal("申请锁已被污染"))
    }

    pub fn assert_writable(&self) -> Result<()> {
        match self.read_only.lock() {
            Ok(guard) => match guard.as_ref() {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            },
            Err(_) => Err(AppError::internal("只读状态锁已被污染")),
        }
    }

    /// 保存配置（只读模式下会被 assert_writable 拦下，调用方应先检查）
    pub fn save_cfg(&self) -> Result<()> {
        let cfg = self
            .cfg
            .lock()
            .map_err(|_| AppError::internal("配置锁已被污染"))?
            .clone();
        crate::config::save(&cfg)
    }

    pub fn cfg_clone(&self) -> Result<Config> {
        self.cfg
            .lock()
            .map(|g| g.clone())
            .map_err(|_| AppError::internal("配置锁已被污染"))
    }

    pub fn with_cfg_mut<T>(&self, f: impl FnOnce(&mut Config) -> Result<T>) -> Result<T> {
        let mut guard = self
            .cfg
            .lock()
            .map_err(|_| AppError::internal("配置锁已被污染"))?;
        f(&mut guard)
    }

    pub fn set_dirty(&self, dirty: bool) {
        if let Ok(mut r) = self.runtime.lock() {
            r.dirty = dirty;
        }
        write_session_marker(dirty);
    }

    pub fn mark_warn_unread(&self) {
        if let Ok(mut r) = self.runtime.lock() {
            r.warn_unread = true;
        }
    }

    pub fn clear_warn_unread(&self) {
        if let Ok(mut r) = self.runtime.lock() {
            r.warn_unread = false;
        }
    }
}

// ---------------------------------------------------------------------------
// 会话标记与崩溃恢复（§6.3）
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SessionMarker {
    pub pid: u32,
    pub started_at: String,
    pub dirty: bool,
}

pub fn write_session_marker(dirty: bool) {
    let marker = SessionMarker {
        pid: std::process::id(),
        started_at: chrono::Local::now().to_rfc3339(),
        dirty,
    };
    if let Ok(text) = serde_json::to_string(&marker) {
        let _ = std::fs::write(paths::session_marker_path(), text.as_bytes());
    }
}

pub fn read_session_marker() -> Option<SessionMarker> {
    let text = std::fs::read_to_string(paths::session_marker_path()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn clear_session_marker() {
    let _ = std::fs::remove_file(paths::session_marker_path());
}

/// 启动时检测上次是否异常退出（§6.3：**不做自动还原**，只给信息 + 给选择）
pub fn detect_crash_recovery() -> Option<CrashInfo> {
    let marker = read_session_marker()?;
    let my_pid = std::process::id();
    if marker.pid == my_pid {
        return None;
    }
    // 同 pid 进程仍在运行 → 不是崩溃（几乎不会发生，pid 复用概率极低）
    if marker.dirty {
        let snapshot_id = crate::backup::latest().ok().flatten().map(|s| s.id);
        log::warn!(
            target: "recovery",
            "检测到上次异常退出：pid={} 于 {}，未回滚变更，对应快照 {}",
            marker.pid,
            marker.started_at,
            snapshot_id.clone().unwrap_or_else(|| "无".into())
        );
        Some(CrashInfo {
            detected_at: chrono::Local::now().to_rfc3339(),
            marker: format!("pid={} 启动于 {}", marker.pid, marker.started_at),
            snapshot_id,
        })
    } else {
        None
    }
}

/// 撤销窗口时长（§5.5.7：5 秒）
pub const CLEAR_UNDO_WINDOW: Duration = Duration::from_secs(5);

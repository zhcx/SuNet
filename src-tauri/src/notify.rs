//! 通知与降级链路（设计方案 §5.4.4 / §5.4.5）
//!
//! 原则：只对"用户可能没预期到的结果"发通知。
//! 用户刚在界面上点的按钮，成功不弹提示（界面内已有反馈）；
//! 只有失败、后台自动发生的事、以及带时效撤销窗口的操作才提示。
//!
//! 降级链路（专注助手 / 勿扰模式 / 用户关闭通知都会导致气泡被静默丢弃）：
//!   气泡不可用 → 主窗口可见时用窗口内顶部横幅
//!              → 主窗口隐藏时托盘 tooltip 前置「⚠ 」
//!              → 无论哪种，都写一条 info 日志

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NotifyAction {
    pub label: String,
    /// 前端点击后要调用的事件名
    pub action: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

#[derive(Serialize, Clone, Debug)]
pub struct NotifyPayload {
    pub level: String,
    pub title: String,
    pub body: String,
    pub sticky: bool,
    pub actions: Vec<NotifyAction>,
    pub ts: String,
}

pub const EVENT: &str = "sunet://notify";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

pub fn send(
    app: &AppHandle,
    level: Level,
    title: &str,
    body: &str,
    actions: Vec<NotifyAction>,
    sticky: bool,
) {
    match level {
        Level::Info => log::info!(target: "notify", "{title}：{body}"),
        Level::Warn => log::warn!(target: "notify", "{title}：{body}"),
        Level::Error => log::error!(target: "notify", "{title}：{body}"),
    }

    let payload = NotifyPayload {
        level: level.as_str().to_string(),
        title: title.to_string(),
        body: body.to_string(),
        sticky,
        actions,
        ts: chrono::Local::now().to_rfc3339(),
    };

    // 发往前端：窗口可见 → 顶部横幅；窗口隐藏 → 事件被丢弃，
    // 由 warn_unread 兜底为托盘 tooltip 前缀
    if app.emit(EVENT, payload).is_err() {
        log::info!(target: "notify", "前端未接收通知事件，降级为托盘 tooltip 前缀");
    }

    // 只有非 Info 才会置「未读警告」标记，进而改变托盘 tooltip 的「⚠ 」前缀；
    // Info 通知（后台完成等）不影响托盘状态，跳过刷新省掉一次整份 hosts 读盘。
    if level != Level::Info {
        if let Some(state) = app.try_state::<crate::state::AppState>() {
            state.mark_warn_unread();
        }
        crate::tray::refresh(app);
    }
}

pub fn action(label: &str, action: &str) -> NotifyAction {
    NotifyAction {
        label: label.to_string(),
        action: action.to_string(),
        args: json!(null),
    }
}

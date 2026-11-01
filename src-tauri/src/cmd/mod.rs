//! Tauri 命令层（设计方案 §8）
//!
//! 权限标注约定：
//!   [无提权] = 主进程可直接执行
//!   [需提权] = 内部自动走 ShellExecuteW + runas 拉起 --task 子进程
//! 前端**不需要感知权限差异**，只在返回的 ApplyReport 里看是否弹过 UAC。

pub mod dns;
pub mod hosts;
pub mod netopt;
pub mod profile;
pub mod proxy;
pub mod system;

use crate::apply::ApplyReport;
use crate::error::{AppError, Result};
use tauri::{AppHandle, Emitter};

/// 阻塞任务丢到 blocking 线程池：提权调用会阻塞到 UAC 结束，不能占住 UI 线程
pub async fn blocking<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::internal(format!("后台任务异常：{e}")))?
}

/// 把事务报告推给前端（每一步的真实结果都可见，§8）
pub fn emit_report(app: &AppHandle, report: &ApplyReport) {
    let _ = app.emit("sunet://report", report);
}

/// 轻量「状态已变」事件：前端据此重新拉一次 get_app_state。
/// 与 emit_report 的区别是它不带报告，用于「改动不是一次事务、但显示必须跟上」的场景
/// （例如快捷面板被唤出：它是常驻窗口，失焦只隐藏不重建）。
pub const EVENT_REFRESH: &str = "sunet://refresh";

pub fn emit_refresh(app: &AppHandle) {
    let _ = app.emit(EVENT_REFRESH, ());
}

/// 状态变化后统一刷新托盘
pub fn after_change(app: &AppHandle) {
    crate::tray::refresh(app);
}

/// DNS 被改写之后的统一收尾：重采真实值 → 刷托盘 → 通知前端重拉状态。
///
/// `alias = Some(_)` 用于「明确知道改的是哪张网卡」的场景（DNS 页应用 / 还原），
/// 采样那张网卡；`None` 用于方案切换 / 回滚 / 全局还原这类一次改多层的场景，
/// 按「当前正在使用的网卡」采样。
///
/// **顺序不能反**：`emit_refresh` 会让快捷面板立刻 `get_app_state`，
/// 缓存还没更新就推事件，面板拿到的仍是旧值 —— 这正是"切换成功但小窗还显示旧
/// DNS"的竞态来源。所有会改 DNS 的路径都必须走这里，不要各自拼一遍。
pub fn after_dns_change(app: &AppHandle, alias: Option<&str>) {
    match alias {
        Some(a) => crate::tray::remember_dns(app, a),
        // 主动改动后不需要比对"是否变化"：事件本来就要推一次
        None => {
            crate::tray::refresh_dns_summary(app);
        }
    }
    crate::tray::refresh(app);
    emit_refresh(app);
}

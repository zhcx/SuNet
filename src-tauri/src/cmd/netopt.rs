//! 网络优化命令（内嵌 netopt 的界面入口）
//!
//! 三层职责划得很清楚，与 netopt 自身的设计约定一致：
//!   · `netopt_audit`    —— 只读巡检，拿到状态与**建议计划**；
//!   · `netopt_optimize` —— 先预览再提权执行，界面能拿到"将要改什么"；
//!   · `netopt_rollback` —— 按 netopt 自己的快照回滚。
//!
//! 所有写入都发生在提权子进程里（`task_runner` 的 `netopt_apply` / `netopt_rollback`），
//! 父进程只读状态、组织计划，和 hosts / DNS 的既有路径保持同一套安全边界。

use crate::apply::ApplyReport;
use crate::error::Result;
use crate::netopt::{self, View};
use crate::state::SharedState;
use serde_json::json;
use std::sync::Arc;
use tauri::{AppHandle, State};

/// 只读巡检（不需要管理员权限）
#[tauri::command]
pub async fn netopt_audit(only: Option<String>) -> Result<View> {
    crate::cmd::blocking(move || {
        let inv = match netopt::Invocation::new(only) {
            Ok(v) => v,
            // 解释器缺失 / 脚本落盘失败都按"不可用"返回：界面据此给出可见原因，
            // 而不是弹一个用户无法处理的错误
            Err(e) => return Ok(View::unavailable(e.to_string())),
        };
        let python = Some(inv.python.clone());
        match inv.audit() {
            Ok(doc) => Ok(View::from_doc(&doc, python)),
            Err(e) => Ok(View {
                python,
                ..View::unavailable(e.to_string())
            }),
        }
    })
    .await
}

/// [需提权] 一键优化：按 netopt 的建议计划执行变更
///
/// 没有可执行的计划时**不申请提权**（不会白弹一次授权框）。
#[tauri::command]
pub async fn netopt_optimize(
    app: AppHandle,
    only: Option<String>,
    state: State<'_, SharedState>,
) -> Result<ApplyReport> {
    let s: Arc<_> = state.inner().clone();
    let report = crate::cmd::blocking(move || {
        // 与方案切换 / hosts 写入共用进程内锁：网络配置不能被两处同时改
        let _guard = s.lock_apply()?;
        let inv = netopt::Invocation::new(only)?;
        let preview = inv.preview()?;
        let view = View::from_doc(&preview, Some(inv.python.clone()));
        if view.plan_count() == 0 {
            return Ok(ApplyReport {
                ok: true,
                steps: Vec::new(),
                rolled_back: false,
                recovery_required: false,
                snapshot_id: None,
                message: "网络配置已符合建议，没有需要执行的变更".into(),
                prompted: false,
            });
        }
        let payload = json!({
            "python": inv.python,
            "script": inv.script,
            "only": inv.only,
            "home": inv.home,
        });
        let out = crate::elevation::run_elevated_raw("netopt_apply", payload, netopt::APPLY_TIMEOUT)?;
        Ok(to_report(out, "网络优化完成"))
    })
    .await?;

    // 优化会动 DNS：缓存与事件都要走统一收尾，否则面板还显示旧值
    crate::cmd::after_dns_change(&app, None);
    crate::cmd::emit_report(&app, &report);
    if !report.ok {
        crate::notify::send(
            &app,
            crate::notify::Level::Warn,
            "网络优化未全部完成",
            &report.message,
            vec![crate::notify::action("查看详情", "open:dns")],
            false,
        );
    }
    Ok(report)
}

/// [需提权] 回滚 netopt 的变更：`to = latest`（上一次）/ `baseline`（首次）
#[tauri::command]
pub async fn netopt_rollback(
    app: AppHandle,
    to: Option<String>,
    state: State<'_, SharedState>,
) -> Result<ApplyReport> {
    let s: Arc<_> = state.inner().clone();
    let target = if to.as_deref() == Some("baseline") {
        "baseline"
    } else {
        "latest"
    };
    let report = crate::cmd::blocking(move || {
        let _guard = s.lock_apply()?;
        let inv = netopt::Invocation::new(None)?;
        let payload = json!({
            "python": inv.python,
            "script": inv.script,
            "home": inv.home,
            "to": target,
        });
        let out =
            crate::elevation::run_elevated_raw("netopt_rollback", payload, netopt::APPLY_TIMEOUT)?;
        Ok(to_report(out, "网络优化已回滚"))
    })
    .await?;

    crate::cmd::after_dns_change(&app, None);
    crate::cmd::emit_report(&app, &report);
    Ok(report)
}

fn to_report(out: crate::ipc::TaskOutput, ok_message: &str) -> ApplyReport {
    ApplyReport {
        ok: out.ok,
        steps: out.steps,
        rolled_back: out.rolled_back,
        recovery_required: out.recovery_required,
        snapshot_id: None,
        message: if out.ok {
            ok_message.to_string()
        } else {
            out.error
                .as_ref()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "网络优化未完成".to_string())
        },
        prompted: crate::elevation::take_prompted(),
    }
}

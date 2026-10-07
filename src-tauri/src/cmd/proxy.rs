//! 代理相关命令（[无提权]：HKCU 用户 hive 可写）

use crate::apply::{self, ApplyTargets, ProxyTarget};
use crate::error::{AppError, Result};
use crate::os::wininet::{self, ProxyState};
use crate::state::{SharedState, CLEAR_UNDO_WINDOW};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

#[derive(Deserialize, Clone, Debug)]
pub struct ProxyConfig {
    pub enable: bool,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub bypass: String,
    /// 前端可覆盖全局设置（§3.4 第 2 步的"仍然开启"出口）
    #[serde(default)]
    pub force: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct ProxyView {
    pub state: ProxyState,
    pub can_undo: bool,
    pub undo_deadline_ms: u64,
    pub probe_before_enable: bool,
    /// 代理生效范围与盲区（§3.5：这张表必须做进界面）
    pub scope: Vec<(String, String)>,
}

#[tauri::command]
pub async fn proxy_get(state: State<'_, SharedState>) -> Result<ProxyView> {
    let s = state.inner().clone();
    crate::cmd::blocking(move || {
        let st = wininet::read()?;
        let cfg = s.cfg_clone().unwrap_or_default();
        let (can_undo, ms) = s
            .runtime
            .lock()
            .map(|rt| {
                let can = rt.last_clear.is_some() && !rt.clear_expired();
                let ms = rt
                    .clear_deadline
                    .map(|d| d.saturating_duration_since(std::time::Instant::now()).as_millis() as u64)
                    .unwrap_or(0);
                (can, ms)
            })
            .unwrap_or((false, 0));
        Ok(ProxyView {
            state: st,
            can_undo,
            undo_deadline_ms: ms,
            probe_before_enable: cfg.settings.proxy_probe_before_enable,
            scope: scope_table(),
        })
    })
    .await
}

/// §3.5 生效范围表（工具只承诺它承诺的）
fn scope_table() -> Vec<(String, String)> {
    [
        ("Chrome / Edge / Firefox（Windows 版）", "跟随"),
        ("IE / Edge 传统模式", "跟随"),
        ("Electron 应用（VS Code / Discord / 新版 QQ）", "跟随"),
        ("大部分国产办公软件、微信 PC 版", "跟随"),
        ("Windows Update / 部分系统服务", "不跟随（走 WinHTTP）"),
        ("UWP / 微软商店应用", "部分跟随"),
        ("部分游戏客户端", "不跟随（自带加速模块）"),
        ("curl / git / npm", "不跟随（需自行配置）"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect()
}

/// [无提权] 设置 / 关闭代理。probe=true 时先做前置连通性自检。
#[tauri::command]
pub async fn proxy_set(
    app: AppHandle,
    cfg: ProxyConfig,
    probe: bool,
    state: State<'_, SharedState>,
) -> Result<apply::ApplyReport> {
    let s = state.inner().clone();
    let report = crate::cmd::blocking(move || {
        let bypass = if cfg.bypass.trim().is_empty() {
            crate::config::DEFAULT_BYPASS.to_string()
        } else {
            cfg.bypass.clone()
        };
        // force = 用户在"仍要开启"确认框里明确选择的出口（高级用户，明确知情）
        let effective_probe = probe && !cfg.force;
        let targets = ApplyTargets {
            proxy: Some(ProxyTarget {
                enable: cfg.enable,
                host: cfg.host.trim().to_string(),
                port: cfg.port,
                bypass,
                probe: effective_probe,
            }),
            strict_verify: false,
            ..Default::default()
        };
        apply::apply(&s, &targets, "手动设置系统代理")
    })
    .await?;

    crate::cmd::emit_report(&app, &report);
    crate::cmd::after_change(&app);
    if !report.ok {
        crate::notify::send(
            &app,
            crate::notify::Level::Error,
            "系统代理设置失败",
            &report.message,
            vec![crate::notify::action("查看详情", "open:proxy")],
            false,
        );
    }
    Ok(report)
}

/// [无提权] 彻底清零 + 可撤销（§5.5.6 / §5.5.7）
#[tauri::command]
pub async fn proxy_clear(app: AppHandle, state: State<'_, SharedState>) -> Result<apply::ClearReport> {
    let s = state.inner().clone();
    let report = crate::cmd::blocking(move || apply::clear_proxy(&s)).await?;
    crate::cmd::after_change(&app);
    crate::notify::send(
        &app,
        crate::notify::Level::Warn,
        "已清除系统代理设置",
        &format!(
            "（含删除 PAC 配置）{} 秒内可撤销",
            CLEAR_UNDO_WINDOW.as_secs()
        ),
        vec![crate::notify::action("撤销", "proxy:undo_clear")],
        false,
    );
    Ok(report)
}

/// 托盘菜单路径：与快捷键调用**同一个函数**（§5.5.8）
pub fn tray_clear_proxy(app: &AppHandle) -> Result<()> {
    let Some(s) = app.try_state::<SharedState>() else {
        return Ok(());
    };
    let report = apply::clear_proxy(&s)?;
    crate::tray::refresh(app);
    crate::notify::send(
        app,
        crate::notify::Level::Warn,
        "已清除系统代理设置",
        &format!(
            "（清除前：{}）{} 秒内可撤销",
            if report.before.enable { "代理开启" } else { "代理关闭" },
            CLEAR_UNDO_WINDOW.as_secs()
        ),
        vec![crate::notify::action("撤销", "proxy:undo_clear")],
        false,
    );
    Ok(())
}

#[tauri::command]
pub async fn proxy_undo_clear(
    app: AppHandle,
    state: State<'_, SharedState>,
) -> Result<bool> {
    let s = state.inner().clone();
    let done = crate::cmd::blocking(move || apply::undo_clear(&s)).await?;
    crate::cmd::after_change(&app);
    if done {
        crate::notify::send(
            &app,
            crate::notify::Level::Info,
            "已撤销上次清除",
            "系统代理配置已原样写回",
            vec![],
            false,
        );
    } else {
        return Err(AppError::internal("没有可撤销的清除操作（暂存只保留最近一次）"));
    }
    Ok(done)
}

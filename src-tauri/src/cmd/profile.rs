//! 方案 / 事务 / 快照 / 订阅相关命令（设计方案 §6 / §8 / §10.1）

use crate::apply::{self, ApplyReport};
use crate::backup::{self, DiffReport, SnapshotMeta};
use crate::config::{Profile, Source, StagedChange};
use crate::error::{AppError, Result};
use crate::state::SharedState;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

// ---------------------------------------------------------------------------
// 方案
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn profile_list(state: State<'_, SharedState>) -> Result<Vec<Profile>> {
    let s: Arc<_> = state.inner().clone();
    crate::cmd::blocking(move || Ok(s.cfg_clone()?.profiles))
        .await
}

#[tauri::command]
pub async fn profile_save(
    app: AppHandle,
    profile: Profile,
    state: State<'_, SharedState>,
) -> Result<Vec<Profile>> {
    let s: Arc<_> = state.inner().clone();
    let list = crate::cmd::blocking(move || {
        s.assert_writable()?;
        let mut p = profile;
        if p.name.trim().is_empty() {
            return Err(AppError::internal("方案名称不能为空"));
        }
        if p.id.trim().is_empty() {
            p.id = uuid::Uuid::new_v4().to_string();
        }
        if p.is_builtin {
            return Err(AppError::internal("内置方案不可修改"));
        }
        s.with_cfg_mut(|c| {
            match c.profiles.iter_mut().find(|x| x.id == p.id) {
                Some(slot) => *slot = p.clone(),
                None => c.profiles.push(p.clone()),
            }
            Ok(())
        })?;
        s.save_cfg()?;
        Ok(s.cfg_clone()?.profiles)
    })
    .await?;
    crate::cmd::after_change(&app);
    Ok(list)
}

#[tauri::command]
pub async fn profile_delete(
    app: AppHandle,
    id: String,
    state: State<'_, SharedState>,
) -> Result<Vec<Profile>> {
    let s: Arc<_> = state.inner().clone();
    let list = crate::cmd::blocking(move || {
        s.assert_writable()?;
        let cfg = s.cfg_clone()?;
        if cfg
            .profile(&id)
            .map(|p| p.is_builtin)
            .unwrap_or(false)
        {
            // 内置方案是任何回滚的终点，删掉它回滚就没有安全落点（§7.1）
            return Err(AppError::internal("内置方案「默认 · 直连」不可删除"));
        }
        s.with_cfg_mut(|c| {
            c.profiles.retain(|p| p.id != id);
            if c.active_profile_id.as_deref() == Some(id.as_str()) {
                c.active_profile_id = None;
            }
            Ok(())
        })?;
        s.save_cfg()?;
        Ok(s.cfg_clone()?.profiles)
    })
    .await?;
    crate::cmd::after_change(&app);
    Ok(list)
}

/// 每个会话只提示一次「装了免密通道就不用再输密码」
static NO_PASSWORD_HINTED: AtomicBool = AtomicBool::new(false);

/// 免密提示：这次操作**确实弹了系统授权框 / UAC**（报告里的 `prompted` 由提权层标记），
/// 而免密通道还没装（或装了没跑起来）→ 提示一次，并给一个「去设置」的入口。
///
/// 为什么不是每次切换都提示：那是骚扰。每个会话只在第一次真的让用户输了密码之后提示。
/// 是否提示、提示什么由后端按平台决定（见 `elevation_task::no_password_hint`），
/// 前端不需要判断平台。
fn hint_no_password(app: &AppHandle, report: &ApplyReport) {
    if !report.ok || !report.prompted {
        return;
    }
    if NO_PASSWORD_HINTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(body) = crate::elevation_task::no_password_hint() else {
        return;
    };
    crate::notify::send(
        app,
        crate::notify::Level::Info, // Info：不置托盘警告，只在窗口里提示
        "以后切换可以不用输密码",
        &body,
        vec![crate::notify::action("去设置", "open:settings")],
        false,
    );
}

#[tauri::command]
pub async fn profile_apply(
    app: AppHandle,
    id: String,
    state: State<'_, SharedState>,
) -> Result<ApplyReport> {
    let s: Arc<_> = state.inner().clone();
    let report = crate::cmd::blocking(move || apply::apply_profile(&s, &id)).await?;
    crate::cmd::emit_report(&app, &report);
    crate::tray::refresh(&app);

    // 通知策略（§5.4.4）：成功只记日志，失败才提示
    if !report.ok {
        crate::notify::send(
            &app,
            if report.recovery_required {
                crate::notify::Level::Error
            } else {
                crate::notify::Level::Warn
            },
            if report.recovery_required {
                "切换失败且需人工介入"
            } else {
                "切换失败，已回滚"
            },
            &report.message,
            vec![
                crate::notify::action("查看差异", "profile:show_diff"),
                crate::notify::action("打开备份目录", "system:open_backup"),
            ],
            report.recovery_required,
        );
    } else {
        log::info!(target: "apply", "方案切换成功：{}", report.message);
        hint_no_password(&app, &report);
    }
    Ok(report)
}

/// 热键连发的闸门。
///
/// 旧行为没有任何闸门：热键回调每次都 spawn 一个新线程去跑切换，而 apply_profile 内部
/// 用阻塞式 Mutex 串行化，于是连按两次不会报错，只会排队 —— 第一次切到直连、第二次又切
/// 回原方案，用户看到的就是「按一下没反应 / 结果乱跳」；如果方案含 hosts 或 DNS，
/// 两次切换会连着弹两次 UAC。
///
/// 这里在**按下热键的主线程**上抢闸门：抢不到说明上一次切换还没结束（正在等 UAC 或
/// 正在写 hosts），直接丢弃这一次按键。切换是幂等语义的“反向操作”，排队毫无意义。
static TOGGLE_BUSY: AtomicBool = AtomicBool::new(false);

struct ToggleGate;

impl ToggleGate {
    fn acquire() -> Option<ToggleGate> {
        TOGGLE_BUSY
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| ToggleGate)
    }
}

impl Drop for ToggleGate {
    fn drop(&mut self) {
        // 工作线程跑完（含出错、含 UAC 被取消）都会走到这里
        TOGGLE_BUSY.store(false, Ordering::SeqCst);
    }
}

/// 全局热键（§5.5.5，2026-10 调整）：「默认直连 ↔ 上一个模式」。
/// 当前不是内置直连 → 切到「默认 · 直连」并记住原方案；
/// 已是直连 → 切回记住的方案。记忆随配置持久化，重启仍有效。
/// 含 hosts / DNS 的切换会弹 UAC；热键路径没有确认对话框，结果用通知反馈。
pub fn hotkey_toggle_direct(app: &AppHandle) {
    let Some(s) = app.try_state::<SharedState>() else {
        return;
    };
    // 闸门要在回调线程上拿：拿不到就是「上一次切换还在跑」，丢弃本次按键
    let Some(gate) = ToggleGate::acquire() else {
        log::debug!(target: "hotkey", "上一次方案切换尚未结束，忽略本次按键");
        return;
    };
    let state = s.inner().clone();
    let a = app.clone();
    // 热键回调在主线程上触发；apply 可能等 UAC / PowerShell，必须丢到工作线程
    std::thread::spawn(move || {
        // 闸门随线程结束自动释放（出错也一样）
        let _gate = gate;
        match toggle_direct(&state) {
            Ok(ToggleDirect::Switched { to_direct, name, message }) => {
                // 热键路径不经过任何前端命令，前端唯一的状态刷新入口就是 sunet://report。
                // 不推报告 → 代理/hosts/DNS 其实已经生效，但主窗口与快捷面板仍显示切换前的方案。
                crate::cmd::emit_report(&a, &message);
                let (title, level) = if !message.ok {
                    ("热键切换未完成".to_string(), crate::notify::Level::Warn)
                } else if to_direct {
                    ("已切换到默认直连".to_string(), crate::notify::Level::Info)
                } else {
                    (format!("已切换回「{name}」"), crate::notify::Level::Info)
                };
                crate::notify::send(&a, level, &title, &message.message, vec![], false);
                hint_no_password(&a, &message);
            }
            Ok(ToggleDirect::NoPrevious) => {
                crate::notify::send(
                    &a,
                    crate::notify::Level::Info,
                    "当前已是直连模式",
                    "再切换过其他方案后，热键才能在两者之间来回切换",
                    vec![],
                    false,
                );
            }
            Err(e) => {
                crate::notify::send(
                    &a,
                    crate::notify::Level::Error,
                    "直连切换失败",
                    &e.to_string(),
                    vec![crate::notify::action("打开主窗口", "open:profiles")],
                    false,
                );
            }
        }
        crate::cmd::after_change(&a);
    });
}

enum ToggleDirect {
    /// to_direct=true 表示切到了直连；name/message 来自事务报告
    Switched {
        to_direct: bool,
        name: String,
        message: crate::apply::ApplyReport,
    },
    NoPrevious,
}

/// 纯函数：给定当前配置，算出热键该切到哪个方案。None = 没有"可切回"的目标。
/// 有活动方案且不是直连 → 切直连；已是直连（或无活动方案）→ 切回记忆的方案。
fn toggle_target(cfg: &crate::config::Config) -> Option<String> {
    match cfg.active_profile_id.as_deref() {
        Some(id) if id != crate::config::BUILTIN_PROFILE_ID => {
            Some(crate::config::BUILTIN_PROFILE_ID.to_string())
        }
        _ => cfg
            .previous_profile_id
            .as_deref()
            .filter(|prev| {
                // 只在方案还存在、且不是直连本身时才有"切回"的意义
                cfg.profile(prev).is_some() && *prev != crate::config::BUILTIN_PROFILE_ID
            })
            .map(|prev| prev.to_string()),
    }
}

fn toggle_direct(state: &crate::state::AppState) -> Result<ToggleDirect> {
    let cfg = state.cfg_clone()?;
    let Some(target) = toggle_target(&cfg) else {
        return Ok(ToggleDirect::NoPrevious);
    };
    let to_direct = target == crate::config::BUILTIN_PROFILE_ID;
    let name = cfg
        .profile(&target)
        .map(|p| p.name.clone())
        .unwrap_or_else(|| target.clone());
    let report = apply::apply_profile(state, &target)?;
    Ok(ToggleDirect::Switched {
        to_direct,
        name,
        message: report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Profile, ProfileDns, ProfileHosts, ProfileProxy};

    fn profile_with(id: &str) -> Profile {
        Profile {
            id: id.into(),
            name: id.into(),
            is_builtin: false,
            note: String::new(),
            hosts: ProfileHosts::default(),
            proxy: ProfileProxy::default(),
            dns: ProfileDns::default(),
        }
    }

    #[test]
    fn hotkey_target_selection() {
        let mut cfg = Config::default();
        cfg.profiles.push(profile_with("p-office"));
        cfg.profiles.push(profile_with("p-accel"));

        // 活动是普通方案 → 切直连
        cfg.active_profile_id = Some("p-office".into());
        cfg.previous_profile_id = None;
        assert_eq!(
            toggle_target(&cfg).as_deref(),
            Some(crate::config::BUILTIN_PROFILE_ID)
        );

        // 已是直连且记着上个方案 → 切回
        cfg.active_profile_id = Some(crate::config::BUILTIN_PROFILE_ID.into());
        cfg.previous_profile_id = Some("p-accel".into());
        assert_eq!(toggle_target(&cfg).as_deref(), Some("p-accel"));

        // 已是直连但没有可切回的（没记住 / 记的是直连自己 / 方案已被删）→ 无目标
        cfg.previous_profile_id = None;
        assert_eq!(toggle_target(&cfg), None);
        cfg.previous_profile_id = Some(crate::config::BUILTIN_PROFILE_ID.into());
        assert_eq!(toggle_target(&cfg), None);
        cfg.previous_profile_id = Some("p-gone".into());
        assert_eq!(toggle_target(&cfg), None);

        // 没有活动方案但记着上个方案 → 切回
        cfg.active_profile_id = None;
        cfg.previous_profile_id = Some("p-office".into());
        assert_eq!(toggle_target(&cfg).as_deref(), Some("p-office"));
    }
}

/// 启动时按上次方案自动恢复（§5.2：开机任务只启动主进程，恢复在这里按需提权）
pub fn do_restore_on_launch(app: &AppHandle) -> Result<()> {
    let Some(s) = app.try_state::<SharedState>() else {
        return Ok(());
    };
    let cfg = s.cfg_clone()?;
    let Some(id) = cfg.active_profile_id.clone() else {
        log::info!(target: "boot", "没有上次生效的方案，跳过自动恢复");
        return Ok(());
    };
    if crate::subscribe::sync_all_if_due(&s) {
        log::info!(target: "boot", "已触发到期的订阅同步");
    }
    let name = cfg.profile(&id).map(|p| p.name.clone()).unwrap_or_default();
    let report = apply::apply_profile(&s, &id)?;
    crate::cmd::emit_report(app, &report);
    crate::tray::refresh(app);
    crate::notify::send(
        app,
        if report.ok {
            crate::notify::Level::Info
        } else {
            crate::notify::Level::Warn
        },
        if report.ok {
            "已自动恢复上次方案"
        } else {
            "自动恢复未成功"
        },
        &format!("{name}：{}", report.message),
        vec![],
        false,
    );
    Ok(())
}

/// 撤销当前方案：回到上一个快照（§6.3）
#[tauri::command]
pub async fn rollback(
    app: AppHandle,
    snapshot_id: Option<String>,
    state: State<'_, SharedState>,
) -> Result<ApplyReport> {
    let s: Arc<_> = state.inner().clone();
    let report =
        crate::cmd::blocking(move || apply::restore_from_snapshot(&s, snapshot_id.as_deref()))
            .await?;
    crate::cmd::emit_report(&app, &report);
    crate::tray::refresh(&app);
    hint_no_password(&app, &report);
    Ok(report)
}

/// 全局还原：清除托管区块 + 关闭代理 + 方案置空
#[tauri::command]
pub async fn global_restore(app: AppHandle, state: State<'_, SharedState>) -> Result<ApplyReport> {
    let s: Arc<_> = state.inner().clone();
    let report = crate::cmd::blocking(move || apply::global_restore(&s)).await?;
    crate::cmd::emit_report(&app, &report);
    crate::tray::refresh(&app);
    hint_no_password(&app, &report);
    if !report.ok {
        crate::notify::send(
            &app,
            crate::notify::Level::Error,
            "全局还原未完全成功",
            &report.message,
            vec![crate::notify::action("打开备份目录", "system:open_backup")],
            true,
        );
    }
    Ok(report)
}

// ---------------------------------------------------------------------------
// 快照
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn snapshots_list() -> Result<Vec<SnapshotMeta>> {
    crate::cmd::blocking(backup::list).await
}

#[tauri::command]
pub async fn snapshots_diff(id: String) -> Result<DiffReport> {
    crate::cmd::blocking(move || backup::diff(&id)).await
}

// ---------------------------------------------------------------------------
// 订阅
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn sources_list(state: State<'_, SharedState>) -> Result<Vec<Source>> {
    let s: Arc<_> = state.inner().clone();
    crate::cmd::blocking(move || Ok(s.cfg_clone()?.sources))
        .await
}

#[tauri::command]
pub async fn sources_save(
    app: AppHandle,
    source: Source,
    state: State<'_, SharedState>,
) -> Result<Vec<Source>> {
    let s: Arc<_> = state.inner().clone();
    let list = crate::cmd::blocking(move || {
        s.assert_writable()?;
        let mut src = source;
        if src.url.trim().is_empty() {
            return Err(AppError::internal("订阅地址不能为空"));
        }
        if !src.url.starts_with("https://")
            || (!src.mirror.trim().is_empty() && !src.mirror.starts_with("https://"))
        {
            return Err(AppError::coded(crate::error::E6002)
                .with_detail("仅接受 https 订阅地址"));
        }
        if src.id.trim().is_empty() {
            src.id = format!("src-{}", uuid::Uuid::new_v4());
        }
        s.with_cfg_mut(|c| {
            match c.sources.iter_mut().find(|x| x.id == src.id) {
                Some(slot) => *slot = src.clone(),
                None => c.sources.push(src.clone()),
            }
            Ok(())
        })?;
        s.save_cfg()?;
        Ok(s.cfg_clone()?.sources)
    })
    .await?;
    crate::cmd::after_change(&app);
    Ok(list)
}

#[tauri::command]
pub async fn sources_delete(
    app: AppHandle,
    id: String,
    state: State<'_, SharedState>,
) -> Result<Vec<Source>> {
    let s: Arc<_> = state.inner().clone();
    let list = crate::cmd::blocking(move || {
        s.assert_writable()?;
        s.with_cfg_mut(|c| {
            c.sources.retain(|x| x.id != id);
            c.source_entries.retain(|x| x.source_id != id);
            Ok(())
        })?;
        s.save_cfg()?;
        Ok(s.cfg_clone()?.sources)
    })
    .await?;
    crate::cmd::after_change(&app);
    Ok(list)
}

#[tauri::command]
pub async fn sources_sync(
    app: AppHandle,
    id: String,
    state: State<'_, SharedState>,
) -> Result<crate::subscribe::SyncResult> {
    let s: Arc<_> = state.inner().clone();
    let result = crate::cmd::blocking(move || crate::subscribe::sync(&s, &id)).await?;
    crate::cmd::after_change(&app);
    if result.needs_confirm {
        crate::notify::send(
            &app,
            crate::notify::Level::Warn,
            "订阅内容有变更，需要你确认",
            &format!(
                "{}：新增 {} / 修改 {} / 删除 {}（危险变更 {} 条）",
                result.name,
                result.pending.as_ref().map(|p| p.added.len()).unwrap_or(0),
                result.pending.as_ref().map(|p| p.changed.len()).unwrap_or(0),
                result.pending.as_ref().map(|p| p.removed.len()).unwrap_or(0),
                result.pending.as_ref().map(|p| p.risky_count).unwrap_or(0)
            ),
            vec![crate::notify::action("查看变更", "sources:show_diff")],
            true,
        );
    } else if !result.ok {
        crate::notify::send(
            &app,
            crate::notify::Level::Warn,
            "订阅同步失败",
            &result.message,
            vec![crate::notify::action("重试", "sources:retry")],
            false,
        );
    }
    Ok(result)
}

#[tauri::command]
pub async fn sources_sync_all(
    app: AppHandle,
    state: State<'_, SharedState>,
) -> Result<Vec<crate::subscribe::SyncResult>> {
    let s: Arc<_> = state.inner().clone();
    let out = crate::cmd::blocking(move || Ok(crate::subscribe::sync_all(&s))).await?;
    crate::cmd::after_change(&app);
    Ok(out)
}

#[tauri::command]
pub async fn sources_commit(
    app: AppHandle,
    id: String,
    include_sensitive: bool,
    state: State<'_, SharedState>,
) -> Result<usize> {
    let s: Arc<_> = state.inner().clone();
    let total =
        crate::cmd::blocking(move || crate::subscribe::commit(&s, &id, include_sensitive)).await?;
    crate::cmd::after_change(&app);
    crate::notify::send(
        &app,
        crate::notify::Level::Info,
        "订阅内容已写入",
        &format!("共 {total} 条（已并入订阅条目库，应用到 hosts 需切方案或手动写入）"),
        vec![],
        false,
    );
    Ok(total)
}

#[tauri::command]
pub async fn sources_discard(
    app: AppHandle,
    id: String,
    state: State<'_, SharedState>,
) -> Result<()> {
    let s: Arc<_> = state.inner().clone();
    crate::cmd::blocking(move || crate::subscribe::discard(&s, &id)).await?;
    crate::cmd::after_change(&app);
    Ok(())
}

#[derive(Serialize, Clone, Debug)]
pub struct StagedView {
    pub source_id: String,
    pub entries_count: usize,
    pub staged: Option<StagedChange>,
    pub last_sync: Option<String>,
}

/// 待确认变更 + 已存条目数
#[tauri::command]
pub async fn sources_staged(state: State<'_, SharedState>) -> Result<Vec<StagedView>> {
    let s: Arc<_> = state.inner().clone();
    crate::cmd::blocking(move || {
        let cfg = s.cfg_clone()?;
        let mut out = Vec::new();
        for src in &cfg.sources {
            let se = cfg
                .source_entries
                .iter()
                .find(|x| x.source_id == src.id)
                .cloned()
                .unwrap_or_default();
            out.push(StagedView {
                source_id: src.id.clone(),
                entries_count: se.entries.len(),
                staged: se.staged.clone(),
                last_sync: se.last_sync.clone(),
            });
        }
        Ok(out)
    })
    .await
}

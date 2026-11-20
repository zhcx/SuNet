//! hosts 相关命令（设计方案 §2 / §8）
//!
//! 编辑模型说明：`hosts_upsert` / `hosts_delete` 只改**配置草稿**（不弹 UAC），
//! 由用户点击「写入系统 hosts」触发一次 `hosts_apply`（提权一次，覆盖所有改动）。
//! 这样既满足"hosts 写入需要提权"，又不至于每改一行都弹一次 UAC。

use crate::error::{AppError, Result};
use crate::os::hosts_file::{self, HostsEntry, HostsStats};
use crate::state::SharedState;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, State};

#[derive(Serialize, Clone, Debug)]
pub struct HostsView {
    pub entries: Vec<HostsEntry>,
    pub stats: HostsStats,
    pub duplicates: Vec<String>,
    /// 配置草稿与系统文件中的托管区块是否不一致
    pub pending: bool,
    pub preview: String,
}

/// 托管区块实际会写入的条目 = 当前方案的展开结果（手工条目 + 订阅条目）。
///
/// 关键点：`/etc/hosts` 的托管区块是全局唯一的一份，而订阅同步出来的条目存在
/// `source_entries`，手工编辑的条目存在 `hosts_entries`。如果这里只取
/// `hosts_entries`，就会出现两个致命现象：
///   1) 订阅同步了 33 条，但 hosts 页「没有数据可写」；
///   2) 系统里已由方案写入 33 条，草稿却是 0 条 → `pending=true` 反复提示待写入，
///      一点「写入系统 hosts」就用空草稿把托管区块整个删掉 → 33 条凭空消失。
/// 因此统一改用方案展开结果，保证「显示 / 预览 / 写入」三者一致。
fn resolved_entries(cfg: &crate::config::Config) -> Vec<HostsEntry> {
    cfg.resolved_hosts_entries_active()
}

/// 把手工条目挂到当前方案（若有），保证「按方案展开」能看到并写入它。
fn attach_to_active_profile(c: &mut crate::config::Config, id: &str) {
    if let Some(pid) = c.active_profile_id.clone() {
        if let Some(p) = c.profiles.iter_mut().find(|p| p.id == pid) {
            if !p.hosts.entry_ids.iter().any(|x| x == id) {
                p.hosts.entry_ids.push(id.to_string());
            }
            p.hosts.enabled = true;
        }
    }
}

fn build_view(state: &crate::state::AppState) -> Result<HostsView> {
    let cfg = state.cfg_clone()?;
    let entries = resolved_entries(&cfg);
    let stats = hosts_file::stats()?;
    let duplicates = hosts_file::find_duplicates(&entries);
    let preview = hosts_file::preview(&entries)?;

    let mut file_lines: Vec<String> = hosts_file::read_block_entries()
        .unwrap_or_default()
        .iter()
        .map(|e| e.to_line())
        .collect();
    let mut cfg_lines: Vec<String> = entries.iter().map(|e| e.to_line()).collect();
    file_lines.sort();
    cfg_lines.sort();
    let pending = file_lines != cfg_lines;

    Ok(HostsView {
        entries,
        stats,
        duplicates,
        pending,
        preview,
    })
}

#[tauri::command]
pub async fn hosts_list(state: State<'_, SharedState>) -> Result<HostsView> {
    let s: Arc<_> = state.inner().clone();
    crate::cmd::blocking(move || build_view(&s)).await
}

/// 新增 / 更新一条手工条目（只写配置草稿）
#[tauri::command]
pub async fn hosts_upsert(
    app: AppHandle,
    entry: HostsEntry,
    state: State<'_, SharedState>,
) -> Result<HostsView> {
    let s = state.inner().clone();
    let view = crate::cmd::blocking(move || {
        s.assert_writable()?;
        let mut entry = entry;
        hosts_file::validate_entry(&entry)?;
        if entry.id.trim().is_empty() {
            entry.id = uuid::Uuid::new_v4().to_string();
        }
        if entry.origin.trim().is_empty() {
            entry.origin = "manual".into();
        }
        s.with_cfg_mut(|c| {
            match c.hosts_entries.iter_mut().find(|e| e.id == entry.id) {
                Some(slot) => *slot = entry.clone(),
                None => c.hosts_entries.push(entry.clone()),
            }
            // 新条目自动挂到当前方案（若有）
            if let Some(pid) = c.active_profile_id.clone() {
                if let Some(p) = c.profiles.iter_mut().find(|p| p.id == pid) {
                    if !p.hosts.entry_ids.contains(&entry.id) {
                        p.hosts.entry_ids.push(entry.id.clone());
                    }
                    p.hosts.enabled = true;
                }
            }
            Ok(())
        })?;
        s.save_cfg()?;
        build_view(&s)
    })
    .await?;
    crate::cmd::after_change(&app);
    Ok(view)
}

#[tauri::command]
pub async fn hosts_delete(
    app: AppHandle,
    id: String,
    state: State<'_, SharedState>,
) -> Result<HostsView> {
    let s = state.inner().clone();
    let view = crate::cmd::blocking(move || {
        s.assert_writable()?;
        s.with_cfg_mut(|c| {
            c.hosts_entries.retain(|e| e.id != id);
            for p in c.profiles.iter_mut() {
                p.hosts.entry_ids.retain(|x| x != &id);
            }
            Ok(())
        })?;
        s.save_cfg()?;
        build_view(&s)
    })
    .await?;
    crate::cmd::after_change(&app);
    Ok(view)
}

/// 清空托管区块（危险操作，前端需二次确认）
#[tauri::command]
pub async fn hosts_clear(app: AppHandle, state: State<'_, SharedState>) -> Result<HostsView> {
    let s = state.inner().clone();
    let view = crate::cmd::blocking(move || {
        s.assert_writable()?;
        s.with_cfg_mut(|c| {
            c.hosts_entries.clear();
            for p in c.profiles.iter_mut() {
                p.hosts.entry_ids.clear();
                // 托管区块现在跟随方案展开（含订阅条目），要真正清空必须同时解绑订阅源，
                // 否则「清空后写入」会把订阅条目又写回来。
                p.hosts.source_ids.clear();
            }
            Ok(())
        })?;
        s.save_cfg()?;
        build_view(&s)
    })
    .await?;
    crate::cmd::after_change(&app);
    Ok(view)
}

/// 批量屏蔽：把域名指向 0.0.0.0（§2.5，前端必须先把清单列给用户看）
#[tauri::command]
pub async fn hosts_batch_block(
    app: AppHandle,
    domains: Vec<String>,
    state: State<'_, SharedState>,
) -> Result<HostsView> {
    let s = state.inner().clone();
    let view = crate::cmd::blocking(move || {
        s.assert_writable()?;
        let mut created = Vec::new();
        for d in domains {
            let mut e = HostsEntry::new("0.0.0.0", d.trim());
            e.comment = "批量屏蔽".into();
            hosts_file::validate_entry(&e)?;
            created.push(e);
        }
        s.with_cfg_mut(|c| {
            for e in created {
                let id = e.id.clone();
                c.hosts_entries.push(e);
                attach_to_active_profile(c, &id);
            }
            Ok(())
        })?;
        s.save_cfg()?;
        build_view(&s)
    })
    .await?;
    crate::cmd::after_change(&app);
    Ok(view)
}

/// 预览将要写入的托管区块（写前预览是必做项）
#[tauri::command]
pub async fn hosts_preview(entries: Vec<HostsEntry>) -> Result<String> {
    crate::cmd::blocking(move || hosts_file::preview(&entries)).await
}

/// 写入系统 hosts 文件（[需提权]，一次覆盖所有草稿改动）
#[tauri::command]
pub async fn hosts_apply(app: AppHandle, state: State<'_, SharedState>) -> Result<crate::apply::ApplyReport> {
    let s = state.inner().clone();
    let report = crate::cmd::blocking(move || {
        let cfg = s.cfg_clone()?;
        let targets = crate::apply::ApplyTargets {
            hosts: Some(resolved_entries(&cfg)),
            strict_verify: false,
            ..Default::default()
        };
        crate::apply::apply(&s, &targets, "手动写入 hosts 托管区块")
    })
    .await?;

    crate::cmd::emit_report(&app, &report);
    crate::cmd::after_change(&app);
    let stats = hosts_file::stats()?;
    crate::notify::send(
        &app,
        if report.ok {
            crate::notify::Level::Info
        } else {
            crate::notify::Level::Error
        },
        "hosts 写入",
        &format!("{}（当前启用 {} 条）", report.message, stats.enabled_entries),
        vec![],
        !report.ok,
    );
    Ok(report)
}

/// 从系统文件导入现有托管区块（首次接管 / 换机恢复）
#[tauri::command]
pub async fn hosts_import(app: AppHandle, state: State<'_, SharedState>) -> Result<HostsView> {
    let s = state.inner().clone();
    let view = crate::cmd::blocking(move || {
        s.assert_writable()?;
        let imported = hosts_file::read_block_entries()?;
        s.with_cfg_mut(|c| {
            for mut e in imported {
                e.origin = "manual".into();
                if !c
                    .hosts_entries
                    .iter()
                    .any(|x| x.hostname == e.hostname && x.ip == e.ip)
                {
                    let id = e.id.clone();
                    c.hosts_entries.push(e);
                    attach_to_active_profile(c, &id);
                }
            }
            Ok(())
        })?;
        s.save_cfg()?;
        build_view(&s)
    })
    .await?;
    crate::cmd::after_change(&app);
    Ok(view)
}

/// hosts 诊断：区分「命中 hosts」与「走 DNS」（§8 system_resolve）
#[tauri::command]
pub async fn hosts_resolve(host: String) -> Result<crate::probe::ResolveResult> {
    crate::cmd::blocking(move || {
        if host.trim().is_empty() {
            return Err(AppError::internal("请输入要诊断的域名"));
        }
        Ok(crate::probe::resolve_host(host.trim()))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ProfileHosts};

    #[test]
    fn resolved_entries_includes_subscription_when_profile_active() {
        let mut cfg = Config::default();
        cfg.active_profile_id = Some(crate::config::BUILTIN_PROFILE_ID.into());
        cfg.profiles[0].hosts = ProfileHosts {
            enabled: true,
            entry_ids: vec![],
            source_ids: vec!["src-github".into()],
        };
        cfg.source_entries.push(crate::config::SourceEntries {
            source_id: "src-github".into(),
            entries: vec![
                HostsEntry::new("1.1.1.1", "a.com"),
                HostsEntry::new("2.2.2.2", "b.com"),
            ],
            ..Default::default()
        });
        // 订阅同步出来的条目必须进入托管区块视图，否则「没有数据可写」
        assert_eq!(resolved_entries(&cfg).len(), 2);
    }

    #[test]
    fn resolved_entries_falls_back_to_manual_without_profile() {
        let mut cfg = Config::default();
        cfg.active_profile_id = None;
        cfg.hosts_entries.push(HostsEntry::new("1.1.1.1", "m.com"));
        assert_eq!(resolved_entries(&cfg).len(), 1);
    }
}

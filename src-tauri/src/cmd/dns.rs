//! DNS 相关命令（设计方案 §4 / §8）

use crate::apply::{self, ApplyTargets, DnsTarget};
use crate::error::{AppError, Result, E4002};
use crate::os::dns_client::{self, AddressFamily, DnsState, NetInterface};
use crate::os::dns_presets::{self, DnsPreset, DohReference};
use crate::state::SharedState;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

#[derive(Deserialize, Clone, Debug)]
pub struct DnsSetRequest {
    pub alias: String,
    /// None = 不改动该族；Some(空) = 该族还原为自动获取（§4.3 交互规则 2）
    #[serde(default)]
    pub v4: Option<Vec<String>>,
    #[serde(default)]
    pub v6: Option<Vec<String>>,
}

#[derive(Serialize, Clone, Debug)]
pub struct DnsInterfaceView {
    pub alias: String,
    pub description: String,
    pub guid: String,
    pub status: String,
    pub is_physical: bool,
    /// 当前正在使用（物理 + 已连接 + 有默认网关）：前端据此做默认选中与标注
    pub is_in_use: bool,
    pub has_v6_global: bool,
    pub v4_summary: String,
    pub v6_summary: String,
}

fn summarize(f: &dns_client::DnsFamilyState) -> String {
    if f.is_dhcp {
        if f.servers.is_empty() {
            "自动获取".to_string()
        } else {
            format!("自动获取（DHCP 下发 {}）", f.servers.join(", "))
        }
    } else {
        f.servers.join(", ")
    }
}

fn to_view(i: &NetInterface, in_use_alias: Option<&str>) -> DnsInterfaceView {
    DnsInterfaceView {
        alias: i.alias.clone(),
        description: i.description.clone(),
        guid: i.guid.clone(),
        status: i.status.clone(),
        is_physical: i.is_physical,
        is_in_use: in_use_alias == Some(i.alias.as_str()),
        has_v6_global: i.has_v6_global,
        v4_summary: summarize(&i.v4),
        v6_summary: summarize(&i.v6),
    }
}

#[tauri::command]
pub async fn dns_interfaces() -> Result<Vec<DnsInterfaceView>> {
    crate::cmd::blocking(move || {
        let list = dns_client::list_interfaces()?;
        let in_use = dns_client::pick_in_use(&list).map(|i| i.alias);
        Ok(list.iter().map(|i| to_view(i, in_use.as_deref())).collect())
    })
    .await
}

#[tauri::command]
pub async fn dns_get(app: AppHandle, alias: String) -> Result<DnsState> {
    let a = app.clone();
    crate::cmd::blocking(move || {
        let st = dns_client::get(&alias)?;
        // 真实状态写入运行时缓存，供托盘 tooltip 使用
        if let Some(state) = a.try_state::<SharedState>() {
            if let Ok(mut rt) = state.runtime.lock() {
                rt.dns_summary = Some(crate::tray::dns_summary_of(&st));
            }
        }
        Ok(st)
    })
    .await
}

fn validate_family(family: AddressFamily, list: &Option<Vec<String>>) -> Result<()> {
    let Some(list) = list else { return Ok(()) };
    for s in list {
        let ip: std::net::IpAddr = s
            .trim()
            .parse()
            .map_err(|_| AppError::coded(E4002).with_detail(format!("非法地址：{s}")))?;
        let ok = matches!(family, AddressFamily::V4) == ip.is_ipv4();
        if !ok {
            return Err(AppError::coded(E4002)
                .with_detail(format!("{} 地址族不匹配：{s}", family.label())));
        }
    }
    Ok(())
}

/// [需提权] 分族设置 DNS
#[tauri::command]
pub async fn dns_set(
    app: AppHandle,
    req: DnsSetRequest,
    state: State<'_, SharedState>,
) -> Result<apply::ApplyReport> {
    let s: Arc<_> = state.inner().clone();
    let alias_check = req.alias.clone();
    let report = crate::cmd::blocking(move || {
        validate_family(AddressFamily::V4, &req.v4)?;
        validate_family(AddressFamily::V6, &req.v6)?;
        let targets = ApplyTargets {
            dns: Some(DnsTarget {
                alias: req.alias.clone(),
                v4: req.v4.clone(),
                v6: req.v6.clone(),
            }),
            strict_verify: false,
            ..Default::default()
        };
        apply::apply(&s, &targets, "设置系统 DNS")
    })
    .await?;

    crate::cmd::emit_report(&app, &report);
    crate::tray::remember_dns(&app, &alias_check);
    crate::cmd::after_change(&app);
    if !report.ok {
        crate::notify::send(
            &app,
            crate::notify::Level::Error,
            "DNS 设置失败",
            &report.message,
            vec![crate::notify::action("查看详情", "open:dns")],
            true,
        );
    }
    Ok(report)
}

/// [需提权] 还原为自动获取（family = None → 两族全部还原）
#[tauri::command]
pub async fn dns_reset(
    app: AppHandle,
    alias: String,
    family: Option<String>,
    state: State<'_, SharedState>,
) -> Result<apply::ApplyReport> {
    let s: Arc<_> = state.inner().clone();
    let alias_check = alias.clone();
    let report = crate::cmd::blocking(move || {
        let (v4, v6) = match family.as_deref() {
            Some("v4") | Some("V4") => (Some(Vec::new()), None),
            Some("v6") | Some("V6") => (None, Some(Vec::new())),
            _ => (Some(Vec::new()), Some(Vec::new())),
        };
        let targets = ApplyTargets {
            dns: Some(DnsTarget { alias, v4, v6 }),
            strict_verify: false,
            ..Default::default()
        };
        apply::apply(&s, &targets, "还原 DNS 为自动获取")
    })
    .await?;

    crate::cmd::emit_report(&app, &report);
    crate::tray::remember_dns(&app, &alias_check);
    crate::cmd::after_change(&app);
    Ok(report)
}

#[tauri::command]
pub async fn dns_flush(app: AppHandle) -> Result<()> {
    crate::cmd::blocking(dns_client::flush).await?;
    crate::cmd::after_change(&app);
    Ok(())
}

#[tauri::command]
pub async fn dns_presets(query: String, state: State<'_, SharedState>) -> Result<Vec<DnsPreset>> {
    let s: Arc<_> = state.inner().clone();
    crate::cmd::blocking(move || {
        let cfg = s.cfg_clone()?;
        Ok(dns_presets::search(&query, &cfg.dns_presets.custom))
    })
    .await
}

/// DoH / DoT 端点只读参考（§4.4：v1 不做 DoH，但必须把地址摆在界面上）
#[tauri::command]
pub async fn dns_doh_reference() -> Result<Vec<DohReference>> {
    crate::cmd::blocking(move || Ok(dns_presets::doh_reference())).await
}

/// 保存自定义 DNS（独立于内置清单，§4.3「我的 DNS」）
#[tauri::command]
pub async fn dns_custom_save(
    app: AppHandle,
    preset: DnsPreset,
    state: State<'_, SharedState>,
) -> Result<Vec<DnsPreset>> {
    let s: Arc<_> = state.inner().clone();
    let list = crate::cmd::blocking(move || {
        s.assert_writable()?;
        validate_family(AddressFamily::V4, &Some(preset.v4.clone()))?;
        validate_family(AddressFamily::V6, &Some(preset.v6.clone()))?;
        if preset.v4.is_empty() && preset.v6.is_empty() {
            return Err(AppError::coded(E4002).with_detail("至少填写一个地址族"));
        }
        let mut preset = preset;
        preset.is_custom = true;
        if preset.id.trim().is_empty() {
            preset.id = format!("u-{}", uuid::Uuid::new_v4());
        }
        s.with_cfg_mut(|c| {
            match c.dns_presets.custom.iter_mut().find(|p| p.id == preset.id) {
                Some(slot) => *slot = preset.clone(),
                None => c.dns_presets.custom.push(preset.clone()),
            }
            Ok(())
        })?;
        s.save_cfg()?;
        let cfg = s.cfg_clone()?;
        Ok(dns_presets::search("", &cfg.dns_presets.custom))
    })
    .await?;
    crate::cmd::after_change(&app);
    Ok(list)
}

#[tauri::command]
pub async fn dns_custom_delete(
    app: AppHandle,
    id: String,
    state: State<'_, SharedState>,
) -> Result<Vec<DnsPreset>> {
    let s: Arc<_> = state.inner().clone();
    let list = crate::cmd::blocking(move || {
        s.assert_writable()?;
        s.with_cfg_mut(|c| {
            c.dns_presets.custom.retain(|p| p.id != id);
            Ok(())
        })?;
        s.save_cfg()?;
        let cfg = s.cfg_clone()?;
        Ok(dns_presets::search("", &cfg.dns_presets.custom))
    })
    .await?;
    crate::cmd::after_change(&app);
    Ok(list)
}

/// 写入前测速：确认该 DNS 地址真的会响应（§12 风险 9c）
#[tauri::command]
pub async fn dns_test(servers: Vec<String>) -> Result<Vec<crate::probe::DnsTestResult>> {
    crate::cmd::blocking(move || {
        if servers.len() > 8 {
            return Err(AppError::coded(E4002).with_detail("一次最多测 8 个地址"));
        }
        Ok(crate::probe::dns_test(
            &servers,
            std::time::Duration::from_millis(1500),
        ))
    })
    .await
}

/// 当前网卡是否有可用 IPv6 通路（§12 风险 9a 的写入前提示）
#[tauri::command]
pub async fn dns_v6_ready(alias: String) -> Result<bool> {
    crate::cmd::blocking(move || {
        let list = dns_client::list_interfaces()?;
        Ok(list
            .into_iter()
            .find(|i| i.alias.eq_ignore_ascii_case(&alias))
            .map(|i| i.has_v6_global)
            .unwrap_or(false))
    })
    .await
}

//! 事务式切换引擎（设计方案 §6）—— 本工具的核心
//!
//! 状态机：Idle → Backup → Apply(hosts → proxy → dns) → Verify → Committed
//!                                    任一步失败 ↓
//!                                 Rollback（逆序：dns → proxy → hosts）
//!                                    回滚也失败 ↓
//!                                 Recovery（人工介入，展示差异清单）
//!
//! **逆序还原是必须的**：先恢复解析层（dns/hosts），再恢复传输层（proxy）。
//! 反过来可能出现"hosts 已还原但代理还开着且代理进程已死"的组合。

use crate::backup::{self, Snapshot};
use crate::config::{Config, Profile};
use crate::elevation;
use crate::error::{AppError, Result, E3001, E3002, E4002};
use crate::ipc::ApplyStep;
use crate::os::hosts_file::HostsEntry;
use crate::os::wininet::{self, ProxyState};
use crate::paths;
use crate::probe;
use crate::state::AppState;
use serde::Serialize;
use serde_json::json;
use std::time::Duration;

const DNS_TASK_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Serialize, Clone, Debug)]
pub struct ApplyReport {
    pub ok: bool,
    pub steps: Vec<ApplyStep>,
    pub rolled_back: bool,
    pub recovery_required: bool,
    pub snapshot_id: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct ProxyTarget {
    pub enable: bool,
    pub host: String,
    pub port: u16,
    pub bypass: String,
    /// 开启前是否做 TCP 连通性自检（§3.4 硬性需求）
    pub probe: bool,
}

/// DNS 目标：None = 该族不改动；Some(空) = 该族还原为自动获取
#[derive(Clone, Debug, Default)]
pub struct DnsTarget {
    pub alias: String,
    pub v4: Option<Vec<String>>,
    pub v6: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default)]
pub struct ApplyTargets {
    pub hosts: Option<Vec<HostsEntry>>,
    pub proxy: Option<ProxyTarget>,
    pub dns: Option<DnsTarget>,
    /// 校验失败（外网不通）时是否自动回滚：方案切换为 true，手动开关为 false
    pub strict_verify: bool,
}

fn step_ok(kind: &str, msg: impl Into<String>) -> ApplyStep {
    ApplyStep {
        kind: kind.into(),
        applied: true,
        verified: true,
        message: msg.into(),
    }
}

fn step_err(kind: &str, err: &AppError) -> ApplyStep {
    ApplyStep {
        kind: kind.into(),
        applied: false,
        verified: false,
        message: err.to_string(),
    }
}

fn step_soft(kind: &str, msg: impl Into<String>) -> ApplyStep {
    ApplyStep {
        kind: kind.into(),
        applied: true,
        verified: false,
        message: msg.into(),
    }
}

/// 由方案展开成写入目标
pub fn build_targets(cfg: &Config, p: &Profile) -> ApplyTargets {
    let hosts = if p.hosts.enabled {
        Some(cfg.resolve_hosts_entries(p))
    } else {
        None
    };
    let proxy = Some(ProxyTarget {
        enable: p.proxy.enabled,
        host: p.proxy.host.clone(),
        port: p.proxy.port,
        bypass: if p.proxy.bypass.trim().is_empty() {
            crate::config::DEFAULT_BYPASS.to_string()
        } else {
            p.proxy.bypass.clone()
        },
        probe: cfg.settings.proxy_probe_before_enable,
    });
    let dns = if p.dns.enabled {
        // 取消勾选 = 该族还原为自动获取（§4.3 规则 2：不留隐性残留）
        let v4 = if p.dns.enable_v4 {
            p.dns.v4.clone()
        } else {
            Vec::new()
        };
        let v6 = if p.dns.enable_v6 {
            p.dns.v6.clone()
        } else {
            Vec::new()
        };
        Some(DnsTarget {
            alias: p.dns.interface_alias.clone(),
            v4: Some(v4),
            v6: Some(v6),
        })
    } else {
        None
    };
    ApplyTargets {
        hosts,
        proxy,
        dns,
        strict_verify: true,
    }
}

// ---------------------------------------------------------------------------
// 主流程
// ---------------------------------------------------------------------------

pub fn apply_profile(state: &AppState, profile_id: &str) -> Result<ApplyReport> {
    let cfg = state.cfg_clone()?;
    let profile = cfg
        .profile(profile_id)
        .ok_or_else(|| AppError::internal(format!("方案不存在：{profile_id}")))?
        .clone();
    let targets = build_targets(&cfg, &profile);
    let reason = format!("切换到方案「{}」", profile.name);
    let report = apply(state, &targets, &reason)?;

    if report.ok && !report.rolled_back && !report.recovery_required {
        state.with_cfg_mut(|c| {
            // 换到不同方案时记下被换掉的活动方案：热键「直连 ↔ 上一模式」靠它切回
            if c.active_profile_id.is_some() && c.active_profile_id != Some(profile.id.clone()) {
                c.previous_profile_id = c.active_profile_id.clone();
            }
            c.active_profile_id = Some(profile.id.clone());
            Ok(())
        })?;
        state.save_cfg()?;
        state.set_dirty(true);
    }
    Ok(report)
}

/// 事务式应用：快照 → 逐层执行并回读校验 → 失败逆序回滚
pub fn apply(state: &AppState, targets: &ApplyTargets, reason: &str) -> Result<ApplyReport> {
    // 进程内锁：锁包住整个事务，避免两个 apply 互相踩踏
    let _guard = state.lock_apply()?;

    let cfg = state.cfg_clone()?;
    let keep = cfg.settings.hosts_backup_keep.max(3) as usize;
    let active_id = cfg.active_profile_id.clone();

    if targets.hosts.is_none() && targets.proxy.is_none() && targets.dns.is_none() {
        return Ok(ApplyReport {
            ok: true,
            steps: vec![],
            rolled_back: false,
            recovery_required: false,
            snapshot_id: None,
            message: "没有需要变更的项目".into(),
        });
    }

    // ── Backup ──
    let aliases: Vec<String> = targets.dns.iter().map(|d| d.alias.clone()).collect();
    let snap = backup::take(
        reason,
        active_id.as_deref(),
        keep,
        targets.hosts.is_some(),
        targets.proxy.is_some(),
        &aliases,
    )?;

    let mut steps: Vec<ApplyStep> = Vec::new();
    let mut applied: Vec<&'static str> = Vec::new();
    let mut failure: Option<AppError> = None;

    // ── Apply ① hosts ──
    if failure.is_none() {
        if let Some(entries) = targets.hosts.as_ref() {
            match step_hosts(entries) {
                Ok(s) => {
                    steps.push(s);
                    applied.push("hosts");
                }
                Err(e) => {
                    steps.push(step_err("hosts", &e));
                    failure = Some(e);
                }
            }
        }
    }

    // ── Apply ② proxy（不弹 UAC 的一层）──
    if failure.is_none() {
        if let Some(t) = targets.proxy.as_ref() {
            match step_proxy(t) {
                Ok(s) => {
                    // 关闭代理时若被 PAC 覆盖，这一步是"诚实失败"而非异常
                    let verified = s.verified;
                    steps.push(s);
                    applied.push("proxy");
                    if !verified && t.enable {
                        log::warn!(target: "apply", "代理步骤未通过校验");
                    }
                }
                Err(e) => {
                    steps.push(step_err("proxy", &e));
                    failure = Some(e);
                }
            }
        }
    }

    // ── Apply ③ dns ──
    if failure.is_none() {
        if let Some(t) = targets.dns.as_ref() {
            match step_dns(t) {
                Ok(s) => {
                    steps.push(s);
                    applied.push("dns");
                }
                Err(e) => {
                    steps.push(step_err("dns", &e));
                    failure = Some(e);
                }
            }
        }
    }

    // ── Verify：代理开启后的外网复检（§3.4 第 4 步）──
    if failure.is_none() {
        if let Some(t) = targets.proxy.as_ref() {
            if t.enable {
                let reachable = probe::probe_internet();
                if reachable {
                    steps.push(step_ok("verify", "外网连通性正常"));
                } else {
                    let host = t.host.clone();
                    let port = t.port;
                    steps.push(step_soft(
                        "verify",
                        format!(
                            "[{E3002}] 代理已开启但外网不通（{host}:{port} 可能未转发流量）"
                        ),
                    ));
                    if targets.strict_verify {
                        failure = Some(AppError::coded(E3002).with_detail(format!(
                            "代理 {host}:{port} 已写入但外网不可达，已自动回滚"
                        )));
                    }
                }
            }
        }
    }

    // ── Rollback（逆序：dns → proxy → hosts）──
    let mut rolled_back = false;
    let mut recovery_required = false;
    if failure.is_some() {
        let (rollback_steps, recovery) = rollback_layers(&snap, &applied);
        steps.extend(rollback_steps);
        rolled_back = true;
        recovery_required = recovery;
        if !recovery {
            let _ = backup::mark_restored(&snap.id);
        }
        log::warn!(
            target: "apply",
            "事务失败并已回滚：原因={}，恢复需人工介入={}",
            failure.as_ref().map(|e| e.to_string()).unwrap_or_default(),
            recovery_required
        );
    }

    // 脏标记：已应用且成功 → dirty；已回滚干净 → 不脏
    if failure.is_none() && !applied.is_empty() {
        state.set_dirty(true);
    } else if rolled_back && !recovery_required {
        state.set_dirty(false);
    }

    let ok = failure.is_none();
    let message = match (&failure, recovery_required) {
        (Some(e), true) => format!("{e}；且回滚未完全成功，需人工介入"),
        (Some(e), false) => format!("{e}；已回滚到操作前状态"),
        (None, _) => "切换完成".to_string(),
    };

    Ok(ApplyReport {
        ok,
        steps,
        rolled_back,
        recovery_required,
        snapshot_id: Some(snap.id),
        message,
    })
}

// ---------------------------------------------------------------------------
// 单层执行
// ---------------------------------------------------------------------------

fn step_hosts(entries: &[HostsEntry]) -> Result<ApplyStep> {
    let out = elevation::run_elevated(
        "hosts_apply",
        json!({ "entries": entries }),
        elevation::TASK_TIMEOUT,
    )?;
    if !out.ok {
        return Err(out
            .error
            .unwrap_or_else(|| AppError::internal("hosts 写入失败")));
    }
    Ok(out.steps.into_iter().next().unwrap_or_else(|| {
        step_ok(
            "hosts",
            format!("写入 {} 条托管条目", entries.len()),
        )
    }))
}

fn proxy_snapshot_file() -> std::path::PathBuf {
    paths::proxy_snapshot_path()
}

/// 首次启用代理前，把"接管前"的完整状态落盘（§3.3：快照文件是崩溃恢复的依据）
fn ensure_proxy_snapshot(before: &ProxyState) -> Result<()> {
    let path = proxy_snapshot_file();
    if path.exists() {
        return Ok(());
    }
    let text = serde_json::to_string_pretty(before)?;
    std::fs::write(&path, text.as_bytes())?;
    Ok(())
}

fn take_proxy_snapshot_file() -> Option<ProxyState> {
    let path = proxy_snapshot_file();
    let text = std::fs::read_to_string(&path).ok()?;
    let state: ProxyState = serde_json::from_str(&text).ok()?;
    let _ = std::fs::remove_file(&path);
    Some(state)
}

fn valid_proxy_host(host: &str) -> bool {
    let h = host.trim();
    if h.is_empty() || h.len() > 253 {
        return false;
    }
    if h.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    h.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        && !h.starts_with('-')
}

fn step_proxy(t: &ProxyTarget) -> Result<ApplyStep> {
    let probe_first = t.probe;
    if t.enable {
        let host = t.host.trim().to_string();
        if host.is_empty() || t.port == 0 {
            return Err(AppError::coded(E4002)
                .with_detail("请填写代理地址与端口（1–65535）"));
        }
        if !valid_proxy_host(&host) {
            return Err(AppError::coded(E4002).with_detail(format!("代理地址非法：{host}")));
        }
        // 前置连通性自检：没有这一条，这个功能会造成全网中断
        if probe_first && !probe::probe_proxy(&host, t.port) {
            return Err(AppError::coded(E3001).with_detail(format!(
                "无法连接到 {host}:{}，开启后所有应用将无法联网。请先启动代理软件，或检查端口。",
                t.port
            )));
        }
        let before = wininet::read()?;
        ensure_proxy_snapshot(&before)?;
        let server = format!("{host}:{}", t.port);
        wininet::write_manual(true, &server, &t.bypass)?;
        let notified = wininet::notify_changed();
        let verified = wininet::verify(true).unwrap_or(false);
        let msg = if verified {
            format!(
                "已启用系统代理 ***:{}（绕过：{}）{}",
                t.port,
                t.bypass,
                if notified { "" } else { "；[E3004] 变更通知失败，部分应用需重启" }
            )
        } else {
            format!("代理已写入但回读校验不通过（***:{}）", t.port)
        };
        log::info!(
            target: "proxy",
            "代理变更：enable=true server={} bypass={} 前置探测={} 通知={} 回读={}",
            crate::logging::redact_host_port(&host, t.port),
            t.bypass,
            probe_first,
            notified,
            verified
        );
        Ok(ApplyStep {
            kind: "proxy".into(),
            applied: true,
            verified,
            message: msg,
        })
    } else {
        // 关闭代理：不用"ProxyEnable=0"简单粗暴，而是快照还原（§3.3）
        match take_proxy_snapshot_file() {
            Some(snap) => {
                wininet::restore(&snap)?;
                wininet::notify_changed();
                let verified = wininet::verify(snap.enable).unwrap_or(false);
                log::info!(
                    target: "proxy",
                    "代理已按接管前快照还原：enable={} pac={}",
                    snap.enable,
                    snap.autoconfig_url.is_some()
                );
                Ok(ApplyStep {
                    kind: "proxy".into(),
                    applied: true,
                    verified,
                    message: if snap.pac_present {
                        "已还原为接管前的代理配置（含 PAC）".into()
                    } else {
                        "已还原为接管前的代理配置".into()
                    },
                })
            }
            None => {
                let now = wininet::read()?;
                wininet::write_manual(false, "", &now.bypass)?;
                wininet::notify_changed();
                if now.pac_present {
                    // 诚实报告：PAC 接管时 ProxyEnable=0 并不生效
                    Ok(step_soft(
                        "proxy",
                        "[E3004] 已关闭手动代理，但系统存在 PAC（AutoConfigURL），代理仍由 PAC 接管；如需彻底清除请使用「清除代理设置」",
                    ))
                } else {
                    Ok(step_ok("proxy", "系统代理已关闭"))
                }
            }
        }
    }
}

fn step_dns(t: &DnsTarget) -> Result<ApplyStep> {
    // 方案未指定网卡（内置直连就是空网卡）时的语义：
    //   - 要写手动地址 → 没有落点，必须报错让用户去选；
    //   - 纯还原为自动（两族列表皆空）→ 自动选择"当前正在使用"的网卡
    //     （物理 + 已连接 + 有默认网关优先，2026-10-07 用户要求）。
    let wants_manual = t.v4.as_ref().map_or(false, |l| !l.is_empty())
        || t.v6.as_ref().map_or(false, |l| !l.is_empty());
    let alias = if t.alias.trim().is_empty() {
        if wants_manual {
            return Err(AppError::coded(crate::error::E4001).with_detail(
                "方案未指定网卡：请编辑方案选择网卡，或将 DNS 模式改为「还原为自动获取」",
            ));
        }
        let list = crate::os::dns_client::list_interfaces()?;
        let picked = crate::os::dns_client::pick_in_use(&list)
            .map(|i| i.alias)
            .ok_or_else(|| {
                AppError::coded(crate::error::E4001).with_detail("没有可用网卡")
            })?;
        log::info!(target: "apply", "方案未指定网卡，DNS 自动落点到当前使用网卡：{picked}");
        picked
    } else {
        t.alias.clone()
    };
    let out = elevation::run_elevated(
        "dns_set",
        json!({ "alias": alias, "v4": t.v4, "v6": t.v6 }),
        DNS_TASK_TIMEOUT,
    )?;
    if !out.ok {
        return Err(out
            .error
            .unwrap_or_else(|| AppError::internal("DNS 设置失败")));
    }
    Ok(out.steps.into_iter().next().unwrap_or_else(|| {
        step_ok(
            "dns",
            format!("DNS 已更新（网卡 {alias}）"),
        )
    }))
}

// ---------------------------------------------------------------------------
// 回滚（逆序）
// ---------------------------------------------------------------------------

fn rollback_layers(snap: &Snapshot, applied: &[&str]) -> (Vec<ApplyStep>, bool) {
    let mut steps = Vec::new();
    let mut recovery = false;

    for layer in applied.iter().rev() {
        match *layer {
            "dns" => {
                let mut all_ok = true;
                for d in &snap.dns {
                    let v4 = Some(if d.v4.is_dhcp {
                        Vec::new()
                    } else {
                        d.v4.servers.clone()
                    });
                    let v6 = Some(if d.v6.is_dhcp {
                        Vec::new()
                    } else {
                        d.v6.servers.clone()
                    });
                    let r = elevation::run_elevated(
                        "dns_set",
                        json!({ "alias": d.interface_alias, "v4": v4, "v6": v6 }),
                        DNS_TASK_TIMEOUT,
                    );
                    if r.is_err() {
                        all_ok = false;
                    }
                }
                if all_ok {
                    steps.push(step_ok("rollback", "DNS 已还原为操作前状态"));
                } else {
                    recovery = true;
                    steps.push(ApplyStep {
                        kind: "rollback".into(),
                        applied: false,
                        verified: false,
                        message: "[E4003] DNS 回滚失败，请手动检查网卡 DNS 设置".into(),
                    });
                }
            }
            "proxy" => {
                let r = match snap.proxy.as_ref() {
                    Some(p) => {
                        let w = wininet::restore(p);
                        if w.is_ok() {
                            wininet::notify_changed();
                        }
                        w
                    }
                    None => wininet::hard_clear(),
                };
                match r {
                    Ok(_) => steps.push(step_ok("rollback", "系统代理已还原为操作前状态")),
                    Err(e) => {
                        recovery = true;
                        steps.push(step_err("rollback", &e));
                    }
                }
            }
            "hosts" => {
                let hex = snap
                    .hosts_raw
                    .clone()
                    .unwrap_or_else(|| crate::util::encode_hex(b""));
                match elevation::run_elevated(
                    "hosts_restore",
                    json!({ "raw_hex": hex }),
                    elevation::TASK_TIMEOUT,
                ) {
                    Ok(out) if out.ok => {
                        steps.push(step_ok("rollback", "hosts 已逐字节还原为操作前内容"))
                    }
                    Ok(out) => {
                        recovery = true;
                        steps.push(step_err(
                            "rollback",
                            &out.error
                                .unwrap_or_else(|| AppError::internal("hosts 还原失败")),
                        ));
                    }
                    Err(e) => {
                        recovery = true;
                        steps.push(step_err("rollback", &e));
                    }
                }
            }
            _ => {}
        }
    }
    (steps, recovery)
}

// ---------------------------------------------------------------------------
// 全局还原 / 崩溃恢复还原
// ---------------------------------------------------------------------------

/// 按快照还原三层（§6.3 的「还原」入口）
pub fn restore_from_snapshot(state: &AppState, snapshot_id: Option<&str>) -> Result<ApplyReport> {
    let _guard = state.lock_apply()?;
    let snap = match snapshot_id {
        Some(id) => backup::load(id)?,
        None => backup::latest()?
            .ok_or_else(|| AppError::internal("没有可用快照"))?,
    };
    let mut layers: Vec<&'static str> = Vec::new();
    if snap.hosts_raw.is_some() {
        layers.push("hosts");
    }
    if snap.proxy.is_some() || snap.proxy_existed {
        layers.push("proxy");
    }
    if !snap.dns.is_empty() {
        layers.push("dns");
    }
    let (steps, recovery) = rollback_layers(&snap, &layers);
    if !recovery {
        let _ = backup::mark_restored(&snap.id);
        state.set_dirty(false);
    }
    Ok(ApplyReport {
        ok: !recovery,
        steps,
        rolled_back: true,
        recovery_required: recovery,
        snapshot_id: Some(snap.id.clone()),
        message: if recovery {
            "还原未完全成功，需人工介入".into()
        } else {
            format!("已还原到快照（{}）", snap.created_at)
        },
    })
}

/// 全局还原：清除托管区块 + 关闭代理 + DNS 还原为自动
pub fn global_restore(state: &AppState) -> Result<ApplyReport> {
    let targets = ApplyTargets {
        hosts: Some(Vec::new()),
        proxy: Some(ProxyTarget {
            enable: false,
            ..Default::default()
        }),
        dns: None,
        strict_verify: false,
    };
    let report = apply(state, &targets, "全局还原")?;
    if report.ok {
        state.with_cfg_mut(|c| {
            c.active_profile_id = None;
            Ok(())
        })?;
        state.save_cfg()?;
    }
    Ok(report)
}

// ---------------------------------------------------------------------------
// 彻底清零 + 撤销（§5.5.6 / §5.5.7）
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug)]
pub struct ClearReport {
    pub before: ProxyState,
    pub ok: bool,
    pub undone: bool,
}

/// 彻底清零：ProxyEnable=0 + 删 AutoConfigURL + ProxyServer 置空 + 保留 ProxyOverride
pub fn clear_proxy(state: &AppState) -> Result<ClearReport> {
    // 与 apply 共享同一把进程内锁 + 同一份快照逻辑（§5.5.8）
    let _guard = state.apply_lock.lock().map_err(|_| AppError::internal("锁被污染"))?;
    let before = wininet::read()?;
    wininet::hard_clear()?;
    wininet::notify_changed();
    let ok = wininet::verify(false).unwrap_or(false);

    if let Ok(mut rt) = state.runtime.lock() {
        rt.last_clear = Some(crate::state::ClearState {
            before: before.clone(),
            cleared_at: chrono::Local::now().to_rfc3339(),
        });
        rt.clear_deadline = Some(std::time::Instant::now() + crate::state::CLEAR_UNDO_WINDOW);
    }

    log::info!(
        target: "proxy",
        "代理彻底清零：清除前 enable={} pac={} server=***:{}；回读校验={}",
        before.enable,
        before.pac_present,
        wininet::parse_server(&before.server).map(|(_, p)| p).unwrap_or(0),
        ok
    );

    Ok(ClearReport {
        before,
        ok,
        undone: false,
    })
}

/// 撤销上次清除（内存暂存，只保留最近一次）
pub fn undo_clear(state: &AppState) -> Result<bool> {
    let staged = {
        let mut rt = state
            .runtime
            .lock()
            .map_err(|_| AppError::internal("运行时锁被污染"))?;
        rt.clear_deadline = None;
        rt.last_clear.take()
    };
    let Some(staged) = staged else {
        return Ok(false);
    };
    wininet::restore(&staged.before)?;
    wininet::notify_changed();
    let verified = wininet::verify(staged.before.enable).unwrap_or(false);
    log::info!(
        target: "proxy",
        "已撤销上次代理清零：还原到 enable={} pac={}，回读校验={}",
        staged.before.enable,
        staged.before.pac_present,
        verified
    );
    Ok(true)
}

/// 丢弃过期的撤销暂存（超时未点 → 丢弃，但已在日志中留有记录）
pub fn expire_clear_undo(state: &AppState) {
    let mut should_drop = false;
    if let Ok(rt) = state.runtime.lock() {
        should_drop = rt.last_clear.is_some() && rt.clear_expired();
    }
    if should_drop {
        if let Ok(mut rt) = state.runtime.lock() {
            if let Some(c) = rt.last_clear.take() {
                log::info!(
                    target: "proxy",
                    "撤销窗口已过期，丢弃暂存（清除于 {}）",
                    c.cleared_at
                );
            }
            rt.clear_deadline = None;
        }
    }
}

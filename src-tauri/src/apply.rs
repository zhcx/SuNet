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
use crate::os::dns_client::DnsFamilyState;
use crate::os::hosts_file::HostsEntry;
use crate::os::system_proxy::{self, ProxyState};
use crate::paths;
use crate::probe;
use crate::state::AppState;
use serde::Serialize;
use serde_json::json;
use std::time::Duration;

const DNS_TASK_TIMEOUT: Duration = Duration::from_secs(60);
/// 批处理（hosts + DNS 合并成一次提权）的超时：两层各自超时之和再留余量
const BATCH_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Serialize, Clone, Debug)]
pub struct ApplyReport {
    pub ok: bool,
    pub steps: Vec<ApplyStep>,
    pub rolled_back: bool,
    pub recovery_required: bool,
    pub snapshot_id: Option<String>,
    pub message: String,
    /// 这次事务里**弹过系统授权框 / UAC**（用户输了密码）。
    /// 界面据此提示"装一次免密通道（macOS 免密助手 / Windows 静默通道）就不用再输密码"。
    pub prompted: bool,
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
// no-op 剪枝
// ---------------------------------------------------------------------------

/// 剪掉「当前已经是目标状态」的层。
///
/// 这些层写下去也只能是 no-op，却要为它提权一次 —— Windows 上就是白弹一个 UAC。
/// 读当前状态不需要权限（FFI 枚举 + 读注册表/配置文件），所以这一步很便宜。
fn prune_satisfied(targets: &ApplyTargets) -> ApplyTargets {
    let mut out = targets.clone();

    // hosts：目标是「清空托管区块」且当前本来就没有区块 → 无事可做
    if out.hosts.as_ref().map_or(false, |e| e.is_empty()) {
        if let Ok(st) = crate::os::hosts_file::stats() {
            if !st.block_present && st.enabled_entries == 0 {
                out.hosts = None;
            }
        }
    }

    // proxy：目标是「关闭」且当前本来就关着、也没有待还原的接管前快照 → 无事可做。
    // 有快照时**不能剪**：关代理的语义是「按接管前的快照还原」，那一步可能反而是打开。
    if let Some(p) = out.proxy.as_ref() {
        if !p.enable && !crate::paths::proxy_snapshot_path().exists() {
            if let Ok(cur) = system_proxy::read() {
                if !cur.enable && !cur.pac_present {
                    out.proxy = None;
                }
            }
        }
    }

    // dns：逐族与当前状态比对，已一致的族不再下发
    if let Some(dns) = out.dns.as_ref() {
        let mut d = dns.clone();
        if let Some((cur4, cur6)) = current_dns_state(&d.alias) {
            d.v4 = prune_family(d.v4, &cur4);
            d.v6 = prune_family(d.v6, &cur6);
        }
        out.dns = if d.v4.is_none() && d.v6.is_none() {
            None
        } else {
            Some(d)
        };
    }
    out
}

/// 目标族与当前族一致 → 不需要写（None）；否则原样保留
fn prune_family(desired: Option<Vec<String>>, cur: &DnsFamilyState) -> Option<Vec<String>> {
    let want = desired?;
    if want.is_empty() {
        // 目标 = 还原为自动获取；当前已经是 DHCP → no-op
        return if cur.is_dhcp { None } else { Some(want) };
    }
    if !cur.is_dhcp && cur.servers == want {
        return None;
    }
    Some(want)
}

/// 当前 DNS 状态（v4, v6）。alias 为空时按「当前在用网卡」取，与 step_dns 的落点保持一致。
fn current_dns_state(alias: &str) -> Option<(DnsFamilyState, DnsFamilyState)> {
    let target = if alias.trim().is_empty() {
        let list = crate::os::dns_client::list_interfaces().ok()?;
        crate::os::dns_client::pick_in_use(&list)?.alias
    } else {
        alias.to_string()
    };
    let st = crate::os::dns_client::get(&target).ok()?;
    Some((st.v4, st.v6))
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

    // 先清掉别处（刷 DNS 缓存、安装免密助手…）留下的提权标记，只统计本次事务自己弹的框
    let _ = elevation::take_prompted();

    let cfg = state.cfg_clone()?;
    let keep = cfg.settings.hosts_backup_keep.max(3) as usize;
    let active_id = cfg.active_profile_id.clone();

    // 先剪掉「已经是目标状态」的层：避免为一次 no-op 白弹提权框、白起一个提权子进程
    let targets = &prune_satisfied(targets);

    if targets.hosts.is_none() && targets.proxy.is_none() && targets.dns.is_none() {
        return Ok(ApplyReport {
            ok: true,
            steps: vec![],
            rolled_back: false,
            recovery_required: false,
            snapshot_id: None,
            message: "没有需要变更的项目".into(),
            prompted: false,
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

    // ── Apply ① proxy：父进程侧准备（校验 / 前置探测 / 快照）──
    //
    // 顺序说明（2026-10-08 调整）：代理改到 hosts / DNS 之前。
    //   · 代理的失败模式最多 —— 前置连通性自检（§3.4）本身就可能直接失败。先做它，
    //     "代理不通"时一个提权层都不用写，也就没有东西需要回滚；
    //   · 回滚是 apply 的逆序，于是变成 hosts → DNS → proxy，正好是设计方案要的
    //     "先恢复解析层（dns/hosts），再恢复传输层（proxy）"；
    //   · 旧顺序（hosts → proxy → dns）的逆序是 dns → proxy → hosts，与文档口径相悖。
    let mut proxy_plan: Option<ProxyPlan> = None;
    if failure.is_none() {
        if let Some(t) = targets.proxy.as_ref() {
            match prepare_proxy(t) {
                Ok(p) => proxy_plan = Some(p),
                Err(e) => {
                    steps.push(step_err("proxy", &e));
                    failure = Some(e);
                }
            }
        }
    }

    // 代理写入落在哪里：macOS 需要 root → 并入下面的提权批处理（一次切换只弹一次授权框）；
    // Windows 不需要 → 本进程直接写，行为与从前完全一致（不弹 UAC）。
    let proxy_needs_root = proxy_plan.as_ref().map_or(false, |p| p.write.needs_root());
    if failure.is_none() && !proxy_needs_root {
        if let Some(p) = proxy_plan.as_ref() {
            match p.write.write_here() {
                Ok(notified) => {
                    let s = proxy_step(p, notified);
                    // 关闭代理时若被 PAC 覆盖，这一步是"诚实失败"而非异常
                    if !s.verified && matches!(p.report, ProxyReport::Enabled { .. }) {
                        log::warn!(target: "apply", "代理步骤未通过校验");
                    }
                    steps.push(s);
                    applied.push("proxy");
                }
                Err(e) => {
                    steps.push(step_err("proxy", &e));
                    failure = Some(e);
                }
            }
        }
    }
    let batch_proxy: Option<&ProxyPlan> = match proxy_plan.as_ref() {
        Some(p) if proxy_needs_root && failure.is_none() => Some(p),
        _ => None,
    };

    // ── Apply ②③ hosts + DNS（macOS 上连 ① 代理一起）：打包成一次提权 ──
    //
    // 这些层都必须提权，原先各起一个提权进程（Windows = 连着两次 UAC，macOS 上含
    // hosts / DNS 的方案 = 连着两个授权框），现在合并成一次。
    // 客户端顺序、失败即停、逐层回滚的语义都不变：子进程把已完成的步骤如实上报。
    if failure.is_none()
        && (batch_proxy.is_some() || targets.hosts.is_some() || targets.dns.is_some())
    {
        match step_privileged_layers(targets, batch_proxy) {
            Ok(mut o) => {
                // 代理那一步的收尾只能由父进程做：子进程只知道"写成功了"，回读要读系统当前状态。
                // 子进程只写、不发变更通知（通知历来是调用方的事），这里补上，
                // 与 proxy_ops::write_manual 的语义对齐（macOS 上本就是个 no-op）。
                if let Some(p) = batch_proxy {
                    if let Some(i) = o.steps.iter().position(|s| s.applied && s.kind == "proxy") {
                        let notified = system_proxy::notify_changed();
                        o.steps[i] = proxy_step(p, notified);
                    }
                }
                steps.extend(o.steps);
                applied.extend(o.applied);
                if o.failure.is_some() {
                    failure = o.failure;
                }
            }
            Err(e) => {
                // 基础设施失败（启动 / 超时 / 取消）：一步都没执行，按第一个目标层报错
                let kind = if batch_proxy.is_some() {
                    "proxy"
                } else if targets.hosts.is_some() {
                    "hosts"
                } else {
                    "dns"
                };
                steps.push(step_err(kind, &e));
                failure = Some(e);
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
        prompted: elevation::take_prompted(),
    })
}

// ---------------------------------------------------------------------------
// 单层执行
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// 代理层：准备 → 写入 → 收尾
// ---------------------------------------------------------------------------
//
// 拆成三段，是为了让「写入落在哪里」由平台决定：
//   · Windows 写 HKCU，普通权限即可 → 本进程直接写（与从前一致，不弹 UAC）；
//   · macOS 的 networksetup 必须 root → 写入并入 hosts / DNS 的**同一个提权批处理**。
// 从前 macOS 上代理独占一次提权，于是含 hosts / DNS 的方案一次切换要弹两次系统授权框
// （hosts + DNS 早已合并成一次，代理这一路当时没跟上）。合并后只弹一次，而且校验 /
// 前置探测 / 快照都留在父进程 —— 失败时连授权框都不用弹。

/// 代理写入内容（父进程侧已完成校验、前置探测与快照落盘）
enum ProxyWrite {
    /// 写入手动代理（`enable=false` 即关闭手动代理）
    Manual {
        enable: bool,
        server: String,
        bypass: String,
    },
    /// 按接管前的快照原样还原（含 PAC）
    Restore(ProxyState),
}

/// 代理层收尾（回读校验 + 报告文案）需要的语义信息
enum ProxyReport {
    Enabled {
        host: String,
        port: u16,
        bypass: String,
        /// 开启前是否做了 TCP 连通性自检（§3.4）
        probe: bool,
    },
    /// 没有快照可还原：直接关掉手动开关
    Disabled { pac_present: bool },
    /// 已按快照还原
    Restored { enable: bool, pac_present: bool },
}

struct ProxyPlan {
    write: ProxyWrite,
    report: ProxyReport,
}

impl ProxyWrite {
    /// macOS 的 networksetup 要 root；Windows 写 HKCU 不需要（见 `os::system_proxy`）
    fn needs_root(&self) -> bool {
        system_proxy::proxy_write_needs_root()
    }

    /// 在本进程直接写（Windows，或进程本身已是管理员 / root）。
    ///
    /// 仍然走 `proxy_ops` 这个「系统代理写操作的唯一出口」，返回的是
    /// 「系统配置变更通知是否成功」（E3004 降级用）。
    fn write_here(&self) -> Result<bool> {
        match self {
            ProxyWrite::Manual {
                enable,
                server,
                bypass,
            } => crate::proxy_ops::write_manual(*enable, server, bypass),
            ProxyWrite::Restore(state) => crate::proxy_ops::restore(state),
        }
    }

    /// 提权批处理里的子任务载荷（任务名必须落在 `task_runner` 的白名单里）
    fn op(&self) -> serde_json::Value {
        match self {
            ProxyWrite::Manual {
                enable,
                server,
                bypass,
            } => json!({
                "task": crate::proxy_ops::TASK_WRITE,
                "data": { "enable": enable, "server": server, "bypass": bypass },
            }),
            ProxyWrite::Restore(state) => json!({
                "task": crate::proxy_ops::TASK_RESTORE,
                "data": { "state": state },
            }),
        }
    }
}

/// 代理层的父进程侧准备：校验 → 前置连通性自检 → 接管前快照。
///
/// 这一步**不碰系统代理配置**（写的部分见 [`ProxyWrite`]），所以授权框要等到
/// 确定真要写的时候才弹。
fn prepare_proxy(t: &ProxyTarget) -> Result<ProxyPlan> {
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
        if t.probe && !probe::probe_proxy(&host, t.port) {
            return Err(AppError::coded(E3001).with_detail(format!(
                "无法连接到 {host}:{}，开启后所有应用将无法联网。请先启动代理软件，或检查端口。",
                t.port
            )));
        }
        let before = system_proxy::read()?;
        ensure_proxy_snapshot(&before)?;
        Ok(ProxyPlan {
            write: ProxyWrite::Manual {
                enable: true,
                server: format!("{host}:{}", t.port),
                bypass: t.bypass.clone(),
            },
            report: ProxyReport::Enabled {
                host,
                port: t.port,
                bypass: t.bypass.clone(),
                probe: t.probe,
            },
        })
    } else {
        // 关闭代理：不用"ProxyEnable=0"简单粗暴，而是快照还原（§3.3）
        match take_proxy_snapshot_file() {
            Some(snap) => {
                let report = ProxyReport::Restored {
                    enable: snap.enable,
                    pac_present: snap.pac_present,
                };
                Ok(ProxyPlan {
                    write: ProxyWrite::Restore(snap),
                    report,
                })
            }
            None => {
                let now = system_proxy::read()?;
                Ok(ProxyPlan {
                    write: ProxyWrite::Manual {
                        enable: false,
                        server: String::new(),
                        bypass: now.bypass.clone(),
                    },
                    report: ProxyReport::Disabled {
                        pac_present: now.pac_present,
                    },
                })
            }
        }
    }
}

/// 代理层的收尾：回读校验 + 报告文案（写入已经完成，无论落在本进程还是提权子进程）
fn proxy_step(plan: &ProxyPlan, notified: bool) -> ApplyStep {
    match &plan.report {
        ProxyReport::Enabled {
            host,
            port,
            bypass,
            probe,
        } => {
            let verified = system_proxy::verify(true).unwrap_or(false);
            let message = if verified {
                format!(
                    "已启用系统代理 ***:{}（绕过：{}）{}",
                    port,
                    bypass,
                    if notified {
                        ""
                    } else {
                        "；[E3004] 变更通知失败，部分应用需重启"
                    }
                )
            } else {
                format!("代理已写入但回读校验不通过（***:{}）", port)
            };
            log::info!(
                target: "proxy",
                "代理变更：enable=true server={} bypass={} 前置探测={} 通知={} 回读={}",
                crate::logging::redact_host_port(host, *port),
                bypass,
                probe,
                notified,
                verified
            );
            ApplyStep {
                kind: "proxy".into(),
                applied: true,
                verified,
                message,
            }
        }
        ProxyReport::Restored {
            enable,
            pac_present,
        } => {
            let verified = system_proxy::verify(*enable).unwrap_or(false);
            log::info!(
                target: "proxy",
                "代理已按接管前快照还原：enable={} pac={} 回读={}",
                enable,
                pac_present,
                verified
            );
            ApplyStep {
                kind: "proxy".into(),
                applied: true,
                verified,
                message: if *pac_present {
                    "已还原为接管前的代理配置（含 PAC）".into()
                } else {
                    "已还原为接管前的代理配置".into()
                },
            }
        }
        ProxyReport::Disabled { pac_present } => {
            if *pac_present {
                // 诚实报告：PAC 接管时 ProxyEnable=0 并不生效
                step_soft(
                    "proxy",
                    "[E3004] 已关闭手动代理，但系统存在 PAC（AutoConfigURL），代理仍由 PAC 接管；如需彻底清除请使用「清除代理设置」",
                )
            } else {
                step_ok("proxy", "系统代理已关闭")
            }
        }
    }
}

#[derive(Default)]
struct LayersOutcome {
    steps: Vec<ApplyStep>,
    /// 已落地的层（顺序即落地顺序，回滚按它的逆序走）
    applied: Vec<&'static str>,
    /// 子进程上报的失败（None = 全部成功）
    failure: Option<AppError>,
}

/// 一次提权要跑的子任务清单，顺序 = 落地顺序（回滚按它的逆序走）：
/// 代理（仅当它需要提权）→ hosts → DNS。
///
/// 代理排在最前，是为了让落地顺序与逐层调用时**完全一致** —— 逆序回滚于是仍是
/// dns → hosts → proxy，即设计方案要的"先恢复解析层，再恢复传输层"。
fn privileged_ops(
    targets: &ApplyTargets,
    proxy: Option<&ProxyPlan>,
) -> Result<Vec<serde_json::Value>> {
    let mut ops: Vec<serde_json::Value> = Vec::new();
    if let Some(p) = proxy {
        ops.push(p.write.op());
    }
    if let Some(entries) = targets.hosts.as_ref() {
        ops.push(json!({ "task": "hosts_apply", "data": { "entries": entries } }));
    }
    if let Some(t) = targets.dns.as_ref() {
        // 网卡在父进程侧先解析：task_runner 的 dns_set 要求 alias 非空
        let alias = resolve_dns_alias(t)?;
        ops.push(json!({
            "task": "dns_set",
            "data": { "alias": alias, "v4": t.v4, "v6": t.v6 }
        }));
    }
    Ok(ops)
}

/// 把需要提权的层打包进**同一个提权子进程**执行。
///
/// 这些层都必须提权（hosts、DNS，以及 macOS 上的代理）：原先各起一个提权进程，
/// 一次方案切换就要连着弹好几次 UAC / 授权框；合并后只起一次。
/// `Err` 只表示"基础设施失败"（启动失败 / 超时 / 用户取消），此时一步都没执行；
/// 子任务本身的失败放在 [`LayersOutcome::failure`] 里返回，并把已完成的步骤一并带回来，
/// 父进程据此决定回滚哪些层。
fn step_privileged_layers(
    targets: &ApplyTargets,
    proxy: Option<&ProxyPlan>,
) -> Result<LayersOutcome> {
    let ops = match privileged_ops(targets, proxy) {
        Ok(ops) => ops,
        // 组装阶段的失败只可能来自 DNS 网卡解析（代理 / hosts 的载荷是纯数据）
        Err(e) => {
            return Ok(LayersOutcome {
                steps: vec![step_err("dns", &e)],
                applied: Vec::new(),
                failure: Some(e),
            })
        }
    };
    if ops.is_empty() {
        return Ok(LayersOutcome::default());
    }

    let out = elevation::run_elevated_raw("batch", json!({ "ops": ops }), BATCH_TIMEOUT)?;

    // 从子进程上报的步骤里反推哪些层已落地（顺序即 apply 的落地顺序）
    let mut applied: Vec<&'static str> = Vec::new();
    for s in &out.steps {
        if !s.applied {
            continue;
        }
        match s.kind.as_str() {
            "proxy" => applied.push("proxy"),
            "hosts" => applied.push("hosts"),
            "dns" => applied.push("dns"),
            _ => {}
        }
    }
    Ok(LayersOutcome {
        steps: out.steps,
        applied,
        failure: if out.ok {
            None
        } else {
            Some(out.error.unwrap_or_else(|| AppError::internal("提权任务失败")))
        },
    })
}

/// 方案未指定网卡（内置直连就是空网卡）时的语义：
///   - 要写手动地址 → 没有落点，必须报错让用户去选；
///   - 纯还原为自动（两族列表皆空）→ 自动选择"当前正在使用"的网卡
///     （物理 + 已连接 + 有默认网关优先，2026-10-07 用户要求）。
fn resolve_dns_alias(t: &DnsTarget) -> Result<String> {
    if !t.alias.trim().is_empty() {
        return Ok(t.alias.clone());
    }
    let wants_manual = t.v4.as_ref().map_or(false, |l| !l.is_empty())
        || t.v6.as_ref().map_or(false, |l| !l.is_empty());
    if wants_manual {
        return Err(AppError::coded(crate::error::E4001).with_detail(
            "方案未指定网卡：请编辑方案选择网卡，或将 DNS 模式改为「还原为自动获取」",
        ));
    }
    let list = crate::os::dns_client::list_interfaces()?;
    let picked = crate::os::dns_client::pick_in_use(&list)
        .map(|i| i.alias)
        .ok_or_else(|| AppError::coded(crate::error::E4001).with_detail("没有可用网卡"))?;
    log::info!(target: "apply", "方案未指定网卡，DNS 自动落点到当前使用网卡：{picked}");
    Ok(picked)
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
                    Some(p) => crate::proxy_ops::restore(p).map(|_| ()),
                    None => crate::proxy_ops::hard_clear().map(|_| ()),
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
    let _ = elevation::take_prompted(); // 只统计本次还原自己弹的框
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
        prompted: elevation::take_prompted(),
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
    let before = system_proxy::read()?;
    crate::proxy_ops::hard_clear()?;
    let ok = system_proxy::verify(false).unwrap_or(false);

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
        system_proxy::parse_server(&before.server).map(|(_, p)| p).unwrap_or(0),
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
    crate::proxy_ops::restore(&staged.before)?;
    let verified = system_proxy::verify(staged.before.enable).unwrap_or(false);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled_proxy_plan() -> ProxyPlan {
        ProxyPlan {
            write: ProxyWrite::Manual {
                enable: true,
                server: "127.0.0.1:7890".into(),
                bypass: "localhost;127.*;<local>".into(),
            },
            report: ProxyReport::Enabled {
                host: "127.0.0.1".into(),
                port: 7890,
                bypass: "localhost;127.*;<local>".into(),
                probe: false,
            },
        }
    }

    fn targets_with_hosts_and_dns() -> ApplyTargets {
        ApplyTargets {
            hosts: Some(Vec::new()),
            proxy: Some(ProxyTarget {
                enable: true,
                ..Default::default()
            }),
            dns: Some(DnsTarget {
                alias: "Wi-Fi".into(),
                v4: Some(vec!["1.1.1.1".into()]),
                v6: None,
            }),
            strict_verify: true,
        }
    }

    /// macOS 上代理必须和 hosts / DNS 落在**同一次**提权里 —— 从前代理独占一次提权，
    /// 于是含 hosts / DNS 的方案一次切换要弹两次系统授权框。
    /// 顺序也必须是 代理 → hosts → DNS：回滚按落地顺序的逆序走，才是 dns → hosts → proxy。
    #[test]
    fn privileged_ops_packs_proxy_with_hosts_and_dns_in_one_batch() {
        let plan = enabled_proxy_plan();
        let ops = privileged_ops(&targets_with_hosts_and_dns(), Some(&plan)).unwrap();

        let tasks: Vec<&str> = ops.iter().map(|o| o["task"].as_str().unwrap()).collect();
        assert_eq!(tasks, ["proxy_write", "hosts_apply", "dns_set"]);
        assert_eq!(ops[0]["data"]["server"], "127.0.0.1:7890");

        // 子任务名必须落在白名单里，否则整个 batch 会被子进程按 E1004 拒掉
        for op in &ops {
            let t = op["task"].as_str().unwrap();
            assert!(
                crate::task_runner::KNOWN_TASKS.contains(&t),
                "{t} 不在任务白名单里"
            );
        }
    }

    /// 方案不含 hosts / DNS 时（只有代理要提权），批处理里就只该有代理一步
    #[test]
    fn privileged_ops_without_hosts_or_dns_has_only_proxy() {
        let plan = enabled_proxy_plan();
        let targets = ApplyTargets {
            hosts: None,
            proxy: Some(ProxyTarget {
                enable: true,
                ..Default::default()
            }),
            dns: None,
            strict_verify: true,
        };
        let ops = privileged_ops(&targets, Some(&plan)).unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0]["task"], "proxy_write");
    }

    /// Windows（代理不需要提权）不会把代理塞进批处理：传 None 时清单里就没有它，
    /// 行为与从前完全一致
    #[test]
    fn privileged_ops_skips_proxy_when_it_needs_no_root() {
        let ops = privileged_ops(&targets_with_hosts_and_dns(), None).unwrap();
        let tasks: Vec<&str> = ops.iter().map(|o| o["task"].as_str().unwrap()).collect();
        assert_eq!(tasks, ["hosts_apply", "dns_set"]);
    }

    /// 还原路径（关闭代理 / 回滚）也可以进批处理，载荷形状必须是 `{ state: ProxyState }`
    #[test]
    fn proxy_restore_op_carries_snapshot_state() {
        let plan = ProxyPlan {
            write: ProxyWrite::Restore(ProxyState {
                enable: true,
                server: "10.0.0.1:1080".into(),
                ..Default::default()
            }),
            report: ProxyReport::Restored {
                enable: true,
                pac_present: false,
            },
        };
        let op = plan.write.op();
        assert_eq!(op["task"], "proxy_restore");
        assert_eq!(op["data"]["state"]["server"], "10.0.0.1:1080");
        assert!(crate::task_runner::KNOWN_TASKS.contains(&op["task"].as_str().unwrap()));
    }
}

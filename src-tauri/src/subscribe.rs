//! 远程订阅：拉取 + 内容校验 + 变更确认（设计方案 §2.6 / §10.1）
//!
//! **订阅源是不可信输入**：写进 hosts 的 IP 直接决定域名解析结果，
//! 一个被劫持的源可以把银行域名指到攻击者 IP。因此校验必须逐条做：
//!   仅 https / 逐行语法 / IP 合法性 / 数量上限 / 变更需确认 / 敏感域名保护 / 失败不清空

use crate::config::{EntryChange, Source, StagedChange};
use crate::error::{AppError, Result, E6001, E6002, E6003};
use crate::os::dns_presets;
use crate::os::hosts_file::{self, HostsEntry};
use crate::state::AppState;
use serde::Serialize;
use std::io::Read;
use std::net::IpAddr;
use std::time::Duration;

const MAX_ENTRIES: usize = 5000;
const MAX_LINE_LEN: usize = 2048;
const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Serialize, Clone, Debug)]
pub struct SyncResult {
    pub source_id: String,
    pub name: String,
    pub ok: bool,
    pub used_url: String,
    pub http_status: u16,
    pub parsed: usize,
    pub rejected_lines: usize,
    pub rejected_sample: Vec<String>,
    pub needs_confirm: bool,
    pub pending: Option<StagedChange>,
    pub message: String,
    pub last_sync: Option<String>,
}

// ---------------------------------------------------------------------------
// 网络
// ---------------------------------------------------------------------------

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent("SuNet/1.0")
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.url().scheme() != "https" {
                // 禁止 302 跳到其他协议
                attempt.error("仅允许 https 重定向")
            } else if attempt.previous().len() > 3 {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|e| AppError::internal(format!("构建 HTTP 客户端失败：{e}")))
}

fn fetch(url: &str) -> Result<(String, u16)> {
    if !url.starts_with("https://") {
        return Err(AppError::coded(E6002).with_detail(format!("仅接受 https 地址：{url}")));
    }
    let c = client()?;
    let resp = c
        .get(url)
        .send()
        .map_err(|e| AppError::coded(E6001).with_detail(format!("请求失败：{e}")))?;
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        return Err(AppError::coded(E6001).with_detail(format!("HTTP {status}")));
    }
    if let Some(len) = resp.content_length() {
        if len > MAX_BODY_BYTES {
            return Err(AppError::coded(E6003)
                .with_detail(format!("响应体过大：{} 字节", len)));
        }
    }
    let mut buf = String::new();
    let mut handle = resp.take(MAX_BODY_BYTES + 1);
    handle
        .read_to_string(&mut buf)
        .map_err(|e| AppError::coded(E6001).with_detail(format!("读取响应失败：{e}")))?;
    if buf.len() as u64 > MAX_BODY_BYTES {
        return Err(AppError::coded(E6003).with_detail("响应体超过 8 MB 上限"));
    }
    Ok((buf, status))
}

// ---------------------------------------------------------------------------
// 内容校验
// ---------------------------------------------------------------------------

/// 特殊地址判定：拒绝组播 / 广播 / 非 127.0.0.1 的回环 / 链路本地 / 未指定 IPv6
fn special_address_reason(ip: &IpAddr) -> Option<String> {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            if v4.is_multicast() {
                return Some("组播地址".into());
            }
            if o == [255, 255, 255, 255] {
                return Some("广播地址".into());
            }
            if v4.is_loopback() && o != [127, 0, 0, 1] {
                return Some("回环地址".into());
            }
            if v4.is_link_local() {
                return Some("链路本地地址".into());
            }
            None
        }
        IpAddr::V6(v6) => {
            if v6.is_multicast() {
                return Some("组播地址".into());
            }
            if v6.is_unspecified() {
                return Some("未指定地址".into());
            }
            if v6.is_unicast_link_local() {
                return Some("链路本地地址".into());
            }
            None
        }
    }
}

/// 逐行解析 + 校验（§10.1）。返回 (条目, 被拒绝行的样例)
pub fn parse_content(text: &str) -> Result<(Vec<HostsEntry>, Vec<String>)> {
    let mut entries: Vec<HostsEntry> = Vec::new();
    let mut rejected: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut total_lines = 0usize;

    for raw_line in text.split('\n') {
        let line = raw_line.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        total_lines += 1;
        if line.len() > MAX_LINE_LEN {
            rejected.push(format!("超长行（>{} 字符）", MAX_LINE_LEN));
            continue;
        }
        if line.chars().any(|c| c.is_control() && c != '\t') {
            // 注意：\t 是 is_control() 的子集，但传统 hosts 就用 TAB 分隔 IP 与域名，
            // 这里把 TAB 排除掉，否则 TAB 分隔的整份订阅会被 100% 拒绝（2026-10-07 实测踩过）
            rejected.push("含控制字符的行".into());
            continue;
        }
        if line.contains('*') {
            rejected.push("含通配符的行（本工具只做精确映射）".into());
            continue;
        }
        if line.contains(|c: char| "[](){}|+?^$\\".contains(c)) {
            rejected.push(format!("含正则元字符的行：{}", truncate(line)));
            continue;
        }
        let (main, comment) = match line.split_once('#') {
            Some((a, b)) => (a.trim(), b.trim().to_string()),
            None => (line, String::new()),
        };
        let mut parts = main.split_whitespace();
        let Some(ip_str) = parts.next() else { continue };
        let hostnames: Vec<String> = parts.map(|s| s.to_string()).collect();
        if hostnames.is_empty() {
            rejected.push(format!("缺少域名：{}", truncate(line)));
            continue;
        }
        let ip: IpAddr = match ip_str.parse() {
            Ok(v) => v,
            Err(_) => {
                rejected.push(format!("IP 非法：{}", truncate(&ip_str)));
                continue;
            }
        };
        if ip_str != "0.0.0.0" {
            if let Some(reason) = special_address_reason(&ip) {
                rejected.push(format!("{ip_str} 是{reason}"));
                continue;
            }
        }
        let hostname = hostnames.join(" ");
        if let Err(e) = hosts_file::validate_hostname(&hostname) {
            rejected.push(format!("域名非法（{e}）"));
            continue;
        }
        // 同一域名只保留首条（hosts 首命中规则）
        let key = hostname.to_ascii_lowercase();
        if !seen.insert(key) {
            continue;
        }
        entries.push(HostsEntry {
            id: uuid::Uuid::new_v4().to_string(),
            ip: ip_str.to_string(),
            hostname,
            enabled: true,
            comment,
            origin: "source".into(),
        });
    }

    if total_lines > MAX_ENTRIES {
        return Err(AppError::coded(E6003).with_detail(format!(
            "单源条目数 {total_lines} 超过上限 {MAX_ENTRIES}，已整源拒绝（不悄悄截断）"
        )));
    }
    Ok((entries, rejected))
}

fn truncate(s: &str) -> String {
    if s.chars().count() <= 60 {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(60).collect::<String>())
    }
}

/// 计算变更（用于 diff 确认）
pub fn compute_change(old: &[HostsEntry], new: &[HostsEntry], first_import: bool) -> StagedChange {
    use std::collections::HashMap;
    let old_map: HashMap<String, &HostsEntry> = old
        .iter()
        .map(|e| (e.hostname.to_ascii_lowercase(), e))
        .collect();
    let new_map: HashMap<String, &HostsEntry> = new
        .iter()
        .map(|e| (e.hostname.to_ascii_lowercase(), e))
        .collect();

    let mut added = Vec::new();
    let mut changed = Vec::new();
    let mut removed = Vec::new();

    for (k, e) in new_map.iter() {
        match old_map.get(k) {
            None => added.push((*e).clone()),
            Some(old_e) => {
                if old_e.ip != e.ip {
                    let sensitive = e
                        .hostnames()
                        .iter()
                        .any(|h| dns_presets::is_sensitive(h));
                    changed.push(EntryChange {
                        before: (*old_e).clone(),
                        after: (*e).clone(),
                        sensitive,
                    });
                }
            }
        }
    }
    for (k, e) in old_map.iter() {
        if !new_map.contains_key(k) {
            removed.push((*e).clone());
        }
    }

    added.sort_by(|a, b| a.hostname.cmp(&b.hostname));
    removed.sort_by(|a, b| a.hostname.cmp(&b.hostname));
    changed.sort_by(|a, b| a.after.hostname.cmp(&b.after.hostname));

    let sensitive_added: Vec<String> = added
        .iter()
        .flat_map(|e| e.hostnames())
        .filter(|h| dns_presets::is_sensitive(h))
        .collect();
    let risky = changed.iter().filter(|c| c.sensitive).count() + sensitive_added.len();

    StagedChange {
        created_at: chrono::Local::now().to_rfc3339(),
        added,
        changed,
        removed,
        total_after: new_map.len(),
        risky_count: risky,
        first_import,
        sensitive_added,
    }
}

/// 变更是否需要用户确认（§10.1：首次导入，或变更 > 总量 20%）
pub fn needs_confirm(change: &StagedChange, old_total: usize) -> bool {
    // 首次导入（本地还没有任何该源条目）一律要确认
    if change.first_import || old_total == 0 {
        return true;
    }
    let touched = change.added.len() + change.changed.len() + change.removed.len();
    let base = change.total_after.max(old_total).max(1);
    touched * 100 / base > 20
}

// ---------------------------------------------------------------------------
// 同步入口
// ---------------------------------------------------------------------------

pub fn sync(state: &AppState, source_id: &str) -> Result<SyncResult> {
    let cfg = state.cfg_clone()?;
    let source: Source = cfg
        .sources
        .iter()
        .find(|s| s.id == source_id)
        .ok_or_else(|| AppError::internal(format!("订阅源不存在：{source_id}")))?
        .clone();

    let old_entries = cfg.source_entries_of(source_id);
    let old_total = old_entries.len();

    // 主源失败 → 自动切镜像；镜像也失败 → **保留上一次内容，绝不清空**（E6001）
    let mut attempt_err = None;
    let mut fetched: Option<(String, u16, String)> = None;
    for url in [source.url.clone(), source.mirror.clone()] {
        if url.trim().is_empty() {
            continue;
        }
        match fetch(&url) {
            Ok((body, status)) => {
                fetched = Some((body, status, url));
                break;
            }
            Err(e) => {
                log::warn!(
                    target: "subscribe",
                    "订阅源 {} 拉取失败（{}）：{e}",
                    source.name,
                    crate::logging::redact_url(&url)
                );
                attempt_err = Some(e);
            }
        }
    }

    let Some((body, status, used_url)) = fetched else {
        let err = attempt_err.unwrap_or_else(|| AppError::coded(E6001));
        log::warn!(target: "subscribe", "订阅源 {} 主源与镜像均不可达，保留本地内容", source.name);
        mark_status(state, source_id, "failed")?;
        return Ok(SyncResult {
            source_id: source_id.to_string(),
            name: source.name.clone(),
            ok: false,
            used_url: crate::logging::redact_url(&source.url),
            http_status: 0,
            parsed: old_total,
            rejected_lines: 0,
            rejected_sample: vec![],
            needs_confirm: false,
            pending: None,
            message: format!("{err}（已保留本地 {old_total} 条，未清空）"),
            last_sync: source.last_sync.clone(),
        });
    };

    let (parsed, rejected) = parse_content(&body)?;
    let first_import = old_total == 0;

    // 首次导入却解析出 0 条：说明响应根本不是 hosts 清单（错误页 / 被劫持页 / 全非法行）。
    // 绝不能把"空变更"暂存后让用户去确认 —— 那个确认界面里什么都没有，只会把人困住。
    if parsed.is_empty() && first_import {
        mark_status(state, source_id, "failed")?;
        let rejected_count = rejected.len();
        log::warn!(
            target: "subscribe",
            "订阅源 {} 响应不含任何有效条目（拒绝 {} 行），按失败处理：{}",
            source.name,
            rejected_count,
            rejected.first().cloned().unwrap_or_else(|| "响应为空".into())
        );
        return Ok(SyncResult {
            source_id: source_id.to_string(),
            name: source.name.clone(),
            ok: false,
            used_url: crate::logging::redact_url(&used_url),
            http_status: status,
            parsed: 0,
            rejected_lines: rejected_count,
            rejected_sample: rejected.into_iter().take(10).collect(),
            needs_confirm: false,
            pending: None,
            message: format!(
                "响应不是有效的 hosts 清单（0 条有效，{rejected_count} 行被拒），未写入任何内容"
            ),
            last_sync: None,
        });
    }

    let change = compute_change(&old_entries, &parsed, first_import);
    let confirm = needs_confirm(&change, old_total);

    log::info!(
        target: "subscribe",
        "订阅同步：源={} HTTP={} 解析 {} 条，拒绝 {} 行，新增 {} / 修改 {} / 删除 {}，需确认={}",
        source.name,
        status,
        parsed.len(),
        rejected.len(),
        change.added.len(),
        change.changed.len(),
        change.removed.len(),
        confirm
    );

    if confirm {
        // 暂存变更，等用户在界面上看一眼 diff（§10.1：唯一能拦住恶意变更的时机）
        state.with_cfg_mut(|c| {
            let se = c
                .source_mut(source_id)
                .ok_or_else(|| AppError::internal("订阅存储初始化失败"))?;
            se.staged = Some(change.clone());
            if let Some(s) = c.sources.iter_mut().find(|s| s.id == source_id) {
                s.last_status = "staged".into();
            }
            Ok(())
        })?;
        state.save_cfg()?;
        return Ok(SyncResult {
            source_id: source_id.to_string(),
            name: source.name.clone(),
            ok: true,
            used_url: crate::logging::redact_url(&used_url),
            http_status: status,
            parsed: parsed.len(),
            rejected_lines: rejected.len(),
            rejected_sample: rejected.into_iter().take(10).collect(),
            needs_confirm: true,
            pending: Some(change),
            message: if first_import {
                "首次导入，需要你确认变更清单后才写入".into()
            } else {
                "本次变更超过总量 20%，需要你确认后才写入".into()
            },
            last_sync: source.last_sync.clone(),
        });
    }

    // 变更很小 → 直接采用
    write_entries(state, source_id, parsed, false)?;
    update_source_meta(state, source_id, "ok")?;
    Ok(SyncResult {
        source_id: source_id.to_string(),
        name: source.name.clone(),
        ok: true,
        used_url: crate::logging::redact_url(&used_url),
        http_status: status,
        parsed: change.total_after,
        rejected_lines: rejected.len(),
        rejected_sample: rejected.into_iter().take(10).collect(),
        needs_confirm: false,
        pending: Some(change),
        message: "同步完成".into(),
        last_sync: Some(chrono::Local::now().to_rfc3339()),
    })
}

/// 用户确认后提交暂存变更（include_sensitive = 是否接受敏感域名变更）
pub fn commit(state: &AppState, source_id: &str, include_sensitive: bool) -> Result<usize> {
    let staged = state.with_cfg_mut(|c| {
        let se = c
            .source_mut(source_id)
            .ok_or_else(|| AppError::internal("订阅存储不存在"))?;
        let staged = se.staged.clone().ok_or_else(|| {
            AppError::internal("没有待确认的变更（可能已提交或已过期）")
        })?;
        Ok(staged)
    })?;

    let cfg = state.cfg_clone()?;
    let mut entries = cfg.source_entries_of(source_id);

    // 删除
    let removed: std::collections::HashSet<String> = staged
        .removed
        .iter()
        .map(|e| e.hostname.to_ascii_lowercase())
        .collect();
    entries.retain(|e| !removed.contains(&e.hostname.to_ascii_lowercase()));

    // 修改：敏感项需显式接受，否则保留旧值
    let mut changed_applied = 0usize;
    for ch in &staged.changed {
        if ch.sensitive && !include_sensitive {
            continue;
        }
        let key = ch.after.hostname.to_ascii_lowercase();
        if let Some(slot) = entries
            .iter_mut()
            .find(|e| e.hostname.to_ascii_lowercase() == key)
        {
            slot.ip = ch.after.ip.clone();
            slot.comment = ch.after.comment.clone();
            changed_applied += 1;
        }
    }

    // 新增
    let existing: std::collections::HashSet<String> = entries
        .iter()
        .map(|e| e.hostname.to_ascii_lowercase())
        .collect();
    for e in &staged.added {
        let sensitive = e.hostnames().iter().any(|h| dns_presets::is_sensitive(h));
        if sensitive && !include_sensitive {
            continue;
        }
        if !existing.contains(&e.hostname.to_ascii_lowercase()) {
            entries.push(e.clone());
        }
    }

    let total = entries.len();
    write_entries(state, source_id, entries, true)?;
    update_source_meta(state, source_id, "ok")?;
    let _ = changed_applied;

    log::info!(
        target: "subscribe",
        "订阅变更已确认写入：源={} 共 {} 条（包含敏感变更={}）",
        source_id,
        total,
        include_sensitive
    );
    Ok(total)
}

/// 放弃暂存变更
pub fn discard(state: &AppState, source_id: &str) -> Result<()> {
    state.with_cfg_mut(|c| {
        let se = c
            .source_mut(source_id)
            .ok_or_else(|| AppError::internal("订阅存储不存在"))?;
        se.staged = None;
        Ok(())
    })?;
    update_source_meta(state, source_id, "failed")?;
    Ok(())
}

fn write_entries(
    state: &AppState,
    source_id: &str,
    entries: Vec<HostsEntry>,
    clear_staged: bool,
) -> Result<()> {
    state.with_cfg_mut(|c| {
        let se = c
            .source_mut(source_id)
            .ok_or_else(|| AppError::internal("订阅存储初始化失败"))?;
        se.entries = entries;
        if clear_staged {
            se.staged = None;
        }
        se.last_sync = Some(chrono::Local::now().to_rfc3339());
        Ok(())
    })?;
    state.save_cfg()
}

fn update_source_meta(state: &AppState, source_id: &str, status: &str) -> Result<()> {
    let now = chrono::Local::now().to_rfc3339();
    state.with_cfg_mut(|c| {
        if let Some(s) = c.sources.iter_mut().find(|s| s.id == source_id) {
            s.last_status = status.to_string();
            if status == "ok" {
                s.last_sync = Some(now.clone());
            }
        }
        if let Some(se) = c.source_entries.iter_mut().find(|s| s.source_id == source_id) {
            if status == "ok" {
                se.last_sync = Some(now.clone());
            }
        }
        Ok(())
    })?;
    state.save_cfg()
}

fn mark_status(state: &AppState, source_id: &str, status: &str) -> Result<()> {
    update_source_meta(state, source_id, status)
}

/// 启动时按 interval_hours 判断哪些源需要同步（到点才拉，避免每次启动都出网）
pub fn sync_all_if_due(state: &AppState) -> bool {
    let Ok(cfg) = state.cfg_clone() else {
        return false;
    };
    let now = chrono::Local::now();
    for s in cfg.sources.iter().filter(|s| s.enabled) {
        let due = match s.last_sync.as_deref().and_then(|t| {
            chrono::DateTime::parse_from_rfc3339(t).ok()
        }) {
            Some(t) => {
                let elapsed = now.signed_duration_since(t.with_timezone(&chrono::Local));
                elapsed.num_hours() >= s.interval_hours.max(1) as i64
            }
            None => true,
        };
        if due {
            if let Err(e) = sync(state, &s.id) {
                log::warn!(target: "subscribe", "启动时同步 {} 失败：{e}", s.name);
            }
            return true;
        }
    }
    false
}

pub fn sync_all(state: &AppState) -> Vec<SyncResult> {
    let cfg = state.cfg_clone().unwrap_or_default();
    let mut out = Vec::new();
    for s in cfg.sources.iter().filter(|s| s.enabled) {
        match sync(state, &s.id) {
            Ok(r) => out.push(r),
            Err(e) => out.push(SyncResult {
                source_id: s.id.clone(),
                name: s.name.clone(),
                ok: false,
                used_url: crate::logging::redact_url(&s.url),
                http_status: 0,
                parsed: 0,
                rejected_lines: 0,
                rejected_sample: vec![],
                needs_confirm: false,
                pending: None,
                message: e.to_string(),
                last_sync: s.last_sync.clone(),
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_lines_only() {
        let text = "# 注释\n\
                    140.82.114.26 alive.github.com\n\
                    1.1.1.1 a.com b.com # 行尾注释\n\
                    *.wild.com bad\n\
                    999.1.1.1 bad-ip.com\n\
                    224.0.0.1 multicast.com\n\
                    255.255.255.255 bcast.com\n\
                    0.0.0.0 blocked.com\n\
                    1.1.1.1 duplicate.com\n\
                    2.2.2.2 duplicate.com\n\
                    12.34.56.78\n";
        let (entries, rejected) = parse_content(text).unwrap();
        let names: Vec<String> = entries.iter().map(|e| e.hostname.clone()).collect();
        assert!(names.contains(&"alive.github.com".to_string()));
        assert!(names.contains(&"a.com b.com".to_string()));
        assert!(names.contains(&"blocked.com".to_string()), "0.0.0.0 允许");
        assert_eq!(
            entries.iter().filter(|e| e.hostname == "duplicate.com").count(),
            1,
            "重复域名只保留首条"
        );
        assert!(!names.iter().any(|n| n.contains("wild.com")));
        assert!(!names.iter().any(|n| n.contains("bad-ip.com")));
        assert!(!names.iter().any(|n| n.contains("multicast.com")));
        assert!(!names.iter().any(|n| n.contains("bcast.com")));
        assert!(rejected.len() >= 4, "非法行必须被记录：{rejected:?}");
    }

    #[test]
    fn tab_separated_lines_parse() {
        // 传统 hosts 用 TAB 分隔；\t 属于 is_control()，曾被整份拒成"含控制字符"
        let text = "# c\r\n140.82.113.26\t\t\talive.github.com\r\n185.199.109.215\tdesktop.githubusercontent.com # t\r\n";
        let (entries, rejected) = parse_content(text).unwrap();
        assert_eq!(entries.len(), 2, "TAB 分隔行必须正常解析：{rejected:?}");
        assert_eq!(entries[0].hostname, "alive.github.com");
        assert_eq!(rejected.len(), 0);
    }

    #[test]
    fn garbage_body_yields_empty_parse() {
        // 首次导入防线的语义前提：HTML 错误页 / 乱码必须解析出 0 条
        let (entries, _) = parse_content("<html><body>503 Service Unavailable</body></html>").unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn oversize_source_is_rejected_entirely() {
        let mut text = String::new();
        for i in 0..(MAX_ENTRIES + 10) {
            text.push_str(&format!("1.1.1.1 h{i}.com\n"));
        }
        let err = parse_content(&text).unwrap_err();
        assert_eq!(err.code, E6003);
    }

    #[test]
    fn control_chars_rejected() {
        let text = "1.1.1.1 good.com\n1.1.1.1 bad\u{7}.com\n";
        let (entries, rejected) = parse_content(text).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(rejected.len(), 1);
    }

    #[test]
    fn change_detection_flags_sensitive() {
        let old = vec![HostsEntry::new("1.2.3.4", "icbc.com.cn")];
        let new = vec![HostsEntry::new("9.9.9.9", "icbc.com.cn")];
        let ch = compute_change(&old, &new, false);
        assert_eq!(ch.changed.len(), 1);
        assert!(ch.changed[0].sensitive, "银行域名必须被标为危险变更");
        assert!(ch.risky_count >= 1);
    }

    #[test]
    fn confirm_threshold() {
        let old: Vec<HostsEntry> = (0..100)
            .map(|i| HostsEntry::new("1.1.1.1", &format!("h{i}.com")))
            .collect();
        let mut new = old.clone();
        new.push(HostsEntry::new("1.1.1.1", "extra.com"));
        let small = compute_change(&old, &new, false);
        assert!(!needs_confirm(&small, old.len()), "1% 变更不需确认");

        let mut big = old.clone();
        for i in 0..50 {
            big.push(HostsEntry::new("1.1.1.1", &format!("n{i}.com")));
        }
        let large = compute_change(&old, &big, false);
        assert!(needs_confirm(&large, old.len()), ">20% 变更必须确认");

        assert!(needs_confirm(&small, 0), "首次导入必须确认");
    }

    #[test]
    fn http_url_rejected() {
        assert!(fetch("http://example.com/hosts").is_err());
    }
}

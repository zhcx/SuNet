//! 快照与回滚（设计方案 §6.2）
//!
//! `hosts_raw` 存**原始字节**而非结构化内容：还原追求的是"逐字节回到原样"，
//! 任何重新序列化都可能引入差异（编码、行尾、空行数量）。
//!
//! 快照落盘 `%APPDATA%\SuNet\snapshots\`，保留最近 20 份，自动清理。

use crate::error::Result;
use crate::os::dns_client::{self, DnsFamilyState};
use crate::os::hosts_file;
use crate::os::system_proxy::{self, ProxyState};
use crate::paths;
use crate::util;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DnsInterfaceSnapshot {
    pub interface_alias: String,
    pub interface_guid: String,
    pub v4: DnsFamilyState,
    pub v6: DnsFamilyState,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Snapshot {
    pub id: String,
    pub created_at: String,
    pub reason: String,
    /// hosts 原文件完整字节（hex）
    pub hosts_raw: Option<String>,
    pub hosts_existed: bool,
    pub proxy: Option<ProxyState>,
    pub proxy_existed: bool,
    pub dns: Vec<DnsInterfaceSnapshot>,
    #[serde(default)]
    pub active_profile_id: Option<String>,
    /// 是否已经被回滚/还原过（rollback 幂等的依据）
    #[serde(default)]
    pub restored: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SnapshotMeta {
    pub id: String,
    pub created_at: String,
    pub reason: String,
    pub hosts_bytes: usize,
    pub hosts_existed: bool,
    pub proxy_state: String,
    pub dns_count: usize,
    pub active_profile_id: Option<String>,
    pub restored: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct DiffItem {
    pub kind: String,
    pub field: String,
    pub before: String,
    pub after: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct DiffReport {
    pub snapshot_id: String,
    pub created_at: String,
    pub reason: String,
    pub items: Vec<DiffItem>,
    pub identical: bool,
}

/// 采集快照（写入磁盘）。只记录**将被修改**的对象，避免快照里塞无关网卡。
pub fn take(
    reason: &str,
    active_profile_id: Option<&str>,
    keep: usize,
    include_hosts: bool,
    include_proxy: bool,
    dns_aliases: &[String],
) -> Result<Snapshot> {
    let (hosts_raw, hosts_existed) = if include_hosts {
        let (bytes, existed) = hosts_file::read_raw()?;
        (Some(util::encode_hex(&bytes)), existed)
    } else {
        (None, true)
    };

    let (proxy, proxy_existed) = if include_proxy {
        match system_proxy::read() {
            Ok(p) => (Some(p), true),
            Err(e) => {
                log::warn!(target: "backup", "读取代理状态失败（视为不存在）：{e}");
                (None, false)
            }
        }
    } else {
        (None, true)
    };

    let mut dns = Vec::new();
    for alias in dns_aliases {
        match dns_client::get(alias) {
            Ok(state) => dns.push(DnsInterfaceSnapshot {
                interface_alias: state.interface_alias,
                interface_guid: state.interface_guid,
                v4: state.v4,
                v6: state.v6,
            }),
            Err(e) => log::warn!(target: "backup", "采集网卡 {alias} 的 DNS 状态失败：{e}"),
        }
    }

    let snap = Snapshot {
        id: uuid::Uuid::new_v4().to_string(),
        created_at: chrono::Local::now().to_rfc3339(),
        reason: reason.to_string(),
        hosts_raw,
        hosts_existed,
        proxy,
        proxy_existed,
        dns,
        active_profile_id: active_profile_id.map(|s| s.to_string()),
        restored: false,
    };
    write(&snap)?;
    prune(keep.max(3));
    log::info!(
        target: "backup",
        "已创建快照 {}（原因：{reason}）",
        snap.id
    );
    Ok(snap)
}

fn dir() -> std::path::PathBuf {
    paths::snapshots_dir()
}

pub fn write(snap: &Snapshot) -> Result<()> {
    let d = dir();
    std::fs::create_dir_all(&d)?;
    let text = serde_json::to_string_pretty(snap)?;
    std::fs::write(d.join(format!("{}.json", snap.id)), text.as_bytes())?;
    Ok(())
}

pub fn list() -> Result<Vec<SnapshotMeta>> {
    let mut out = Vec::new();
    let d = dir();
    if !d.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(&d)?.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e != "json").unwrap_or(true) {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(s) = serde_json::from_str::<Snapshot>(&text) {
                out.push(meta_of(&s));
            }
        }
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(out)
}

fn meta_of(s: &Snapshot) -> SnapshotMeta {
    SnapshotMeta {
        id: s.id.clone(),
        created_at: s.created_at.clone(),
        reason: s.reason.clone(),
        hosts_bytes: s
            .hosts_raw
            .as_ref()
            .map(|h| h.len() / 2)
            .unwrap_or(0),
        hosts_existed: s.hosts_existed,
        proxy_state: match &s.proxy {
            Some(p) if p.enable => match system_proxy::parse_server(&p.server) {
                Some((_, port)) => format!("已开启（***:{port}）"),
                None => "已开启".into(),
            },
            Some(_) => "关闭".into(),
            None => "未记录".into(),
        },
        dns_count: s.dns.len(),
        active_profile_id: s.active_profile_id.clone(),
        restored: s.restored,
    }
}

pub fn load(id: &str) -> Result<Snapshot> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err(crate::error::AppError::internal("快照 id 非法"));
    }
    let path = dir().join(format!("{id}.json"));
    let text = std::fs::read_to_string(&path)
        .map_err(|e| crate::error::AppError::internal(format!("读取快照失败：{e}")))?;
    serde_json::from_str(&text)
        .map_err(|e| crate::error::AppError::internal(format!("快照解析失败：{e}")))
}

pub fn latest() -> Result<Option<Snapshot>> {
    let d = dir();
    if !d.exists() {
        return Ok(None);
    }
    let mut best: Option<Snapshot> = None;
    for entry in std::fs::read_dir(&d)?.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e != "json").unwrap_or(true) {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(s) = serde_json::from_str::<Snapshot>(&text) {
                if best
                    .as_ref()
                    .map(|b| s.created_at > b.created_at)
                    .unwrap_or(true)
                {
                    best = Some(s);
                }
            }
        }
    }
    Ok(best)
}

/// 保留最近 keep 份
pub fn prune(keep: usize) {
    let d = dir();
    let Ok(rd) = std::fs::read_dir(&d) else { return };
    let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e != "json").unwrap_or(true) {
            continue;
        }
        files.push((path.file_name().unwrap().to_string_lossy().to_string(), path));
    }
    if files.len() <= keep {
        return;
    }
    // 以文件修改时间排序（快照名是 uuid，不带时间信息）
    let mut with_time: Vec<(std::time::SystemTime, std::path::PathBuf)> = files
        .into_iter()
        .map(|(_, p)| {
            let t = std::fs::metadata(&p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            (t, p)
        })
        .collect();
    with_time.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p) in with_time.into_iter().skip(keep) {
        let _ = std::fs::remove_file(p);
    }
}

/// 快照 vs 当前状态
pub fn diff(id: &str) -> Result<DiffReport> {
    let snap = load(id)?;
    let mut items = Vec::new();

    if let Some(hex) = snap.hosts_raw.as_ref() {
        let before_bytes = util::decode_hex(hex).unwrap_or_default();
        let before_entries = hosts_file::decode(&before_bytes)
            .map(|t| hosts_file::parse(&t).map(|p| hosts_file::parse_block_lines(&p.block)))
            .unwrap_or(Ok(Vec::new()))?;
        let after_entries = hosts_file::read_block_entries().unwrap_or_default();
        let mut b: Vec<String> = before_entries.iter().map(|e| e.to_line()).collect();
        let mut a: Vec<String> = after_entries.iter().map(|e| e.to_line()).collect();
        b.sort();
        a.sort();
        if b != a {
            items.push(DiffItem {
                kind: "hosts".into(),
                field: "托管区块条目".into(),
                before: format!("{} 条", before_entries.len()),
                after: format!("{} 条", after_entries.len()),
            });
            for line in a.iter().filter(|l| !b.contains(l)).take(50) {
                items.push(DiffItem {
                    kind: "hosts".into(),
                    field: "新增".into(),
                    before: "—".into(),
                    after: line.clone(),
                });
            }
            for line in b.iter().filter(|l| !a.contains(l)).take(50) {
                items.push(DiffItem {
                    kind: "hosts".into(),
                    field: "移除".into(),
                    before: line.clone(),
                    after: "—".into(),
                });
            }
        }
    }

    if let Some(p) = snap.proxy.as_ref() {
        let now = system_proxy::read().unwrap_or_default();
        if now.enable != p.enable {
            items.push(DiffItem {
                kind: "proxy".into(),
                field: "ProxyEnable".into(),
                before: if p.enable { "1".into() } else { "0".into() },
                after: if now.enable { "1".into() } else { "0".into() },
            });
        }
        if now.pac_present != p.pac_present {
            items.push(DiffItem {
                kind: "proxy".into(),
                field: "AutoConfigURL".into(),
                before: if p.pac_present { "存在" } else { "无" }.into(),
                after: if now.pac_present { "存在" } else { "无" }.into(),
            });
        }
        if now.server != p.server && !now.server.is_empty() {
            items.push(DiffItem {
                kind: "proxy".into(),
                field: "ProxyServer".into(),
                before: mask_server(&p.server),
                after: mask_server(&now.server),
            });
        }
    }

    for d in snap.dns.iter() {
        if let Ok(now) = dns_client::get(&d.interface_alias) {
            if now.v4 != d.v4 {
                items.push(DiffItem {
                    kind: "dns".into(),
                    field: format!("{} IPv4", d.interface_alias),
                    before: family_text(&d.v4),
                    after: family_text(&now.v4),
                });
            }
            if now.v6 != d.v6 {
                items.push(DiffItem {
                    kind: "dns".into(),
                    field: format!("{} IPv6", d.interface_alias),
                    before: family_text(&d.v6),
                    after: family_text(&now.v6),
                });
            }
        }
    }

    Ok(DiffReport {
        snapshot_id: snap.id.clone(),
        created_at: snap.created_at.clone(),
        reason: snap.reason.clone(),
        identical: items.is_empty(),
        items,
    })
}

fn family_text(f: &DnsFamilyState) -> String {
    if f.is_dhcp {
        "自动获取".into()
    } else {
        f.servers.join(", ")
    }
}

fn mask_server(s: &str) -> String {
    match system_proxy::parse_server(s) {
        Some((_, port)) => format!("***:{port}"),
        None => "—".into(),
    }
}

pub fn mark_restored(id: &str) -> Result<()> {
    let mut snap = load(id)?;
    snap.restored = true;
    write(&snap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_roundtrip_keeps_raw_bytes() {
        let bytes = b"127.0.0.1 localhost\r\n";
        let hex = util::encode_hex(bytes);
        let snap = Snapshot {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: chrono::Local::now().to_rfc3339(),
            reason: "test".into(),
            hosts_raw: Some(hex.clone()),
            hosts_existed: true,
            proxy: None,
            proxy_existed: false,
            dns: vec![],
            active_profile_id: None,
            restored: false,
        };
        let text = serde_json::to_string(&snap).unwrap();
        let back: Snapshot = serde_json::from_str(&text).unwrap();
        assert_eq!(
            util::decode_hex(back.hosts_raw.as_ref().unwrap()).unwrap(),
            bytes.to_vec()
        );
    }

    #[test]
    fn meta_reports_proxy_without_host() {
        let snap = Snapshot {
            id: "x".into(),
            created_at: "now".into(),
            reason: "r".into(),
            hosts_raw: None,
            hosts_existed: false,
            proxy: Some(ProxyState {
                enable: true,
                server: "127.0.0.1:7890".into(),
                bypass: String::new(),
                autoconfig_url: None,
                pac_present: false,
            }),
            proxy_existed: true,
            dns: vec![],
            active_profile_id: None,
            restored: false,
        };
        let m = meta_of(&snap);
        assert!(m.proxy_state.contains("7890"));
        assert!(!m.proxy_state.contains("127.0.0.1"));
    }
}

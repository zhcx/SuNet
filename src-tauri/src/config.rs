//! 配置 / 方案持久化（设计方案 §7.1）
//!
//! 三点设计说明：
//!   1. 内置 DNS 清单不进配置文件（编译进二进制）——修正地址不需要迁移
//!   2. 方案里同时存 `preset_id` 与展开后的 `v4`/`v6`：显示用前者，写入用后者
//!   3. `v6` 空数组 = 该族保持 DHCP，方案层不允许留下"上次开了 IPv6"的隐性残留

use crate::error::{AppError, Result, E9001, E9002};
use crate::migrate::CURRENT_SCHEMA;
use crate::os::dns_presets::DnsPreset;
use crate::os::hosts_file::HostsEntry;
use crate::paths;
use serde::{Deserialize, Serialize};

pub const DEFAULT_BYPASS: &str = "localhost;127.*;<local>";
pub const BUILTIN_PROFILE_ID: &str = "builtin-direct";

// ---------------------------------------------------------------------------
// 设置
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct ShortcutBinding {
    pub enabled: bool,
    pub binding: String,
}

impl Default for ShortcutBinding {
    fn default() -> Self {
        Self {
            enabled: true,
            binding: "Ctrl+Alt+S".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Shortcuts {
    pub clear_proxy: ShortcutBinding,
}

impl Default for Shortcuts {
    fn default() -> Self {
        Self {
            clear_proxy: ShortcutBinding::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Autostart {
    pub enabled: bool,
    pub method: String,
}

impl Default for Autostart {
    fn default() -> Self {
        Self {
            enabled: false,
            method: "scheduled_task".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub start_minimized: bool,
    pub autostart: Autostart,
    /// keep | restore_if_changed | always_restore（§6.3，默认 keep）
    pub on_exit: String,
    pub proxy_probe_before_enable: bool,
    /// 提权前先自绘说明对话框（§1.4.4 ③）
    pub confirm_elevation_notice: bool,
    /// 启动时自动恢复上次方案（§5.2）
    pub restore_on_launch: bool,
    pub language: String,
    pub theme: String,
    pub hosts_backup_keep: u32,
    pub log_keep_days: u32,
    pub log_level: String,
    pub log_redact: bool,
    pub shortcuts: Shortcuts,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            start_minimized: true,
            autostart: Autostart::default(),
            on_exit: "keep".into(),
            proxy_probe_before_enable: true,
            confirm_elevation_notice: true,
            restore_on_launch: false,
            language: "zh-CN".into(),
            theme: "follow_system".into(),
            hosts_backup_keep: 20,
            log_keep_days: 7,
            log_level: "info".into(),
            log_redact: true,
            shortcuts: Shortcuts::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// 方案
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct ProfileHosts {
    pub enabled: bool,
    pub entry_ids: Vec<String>,
    pub source_ids: Vec<String>,
}

impl Default for ProfileHosts {
    fn default() -> Self {
        Self {
            enabled: true,
            entry_ids: Vec::new(),
            source_ids: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct ProfileProxy {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub bypass: String,
}

impl Default for ProfileProxy {
    fn default() -> Self {
        Self {
            enabled: false,
            host: String::new(),
            port: 0,
            bypass: DEFAULT_BYPASS.into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct ProfileDns {
    /// 该方案是否管理 DNS
    pub enabled: bool,
    /// dhcp | manual
    pub mode: String,
    pub interface_alias: String,
    pub preset_id: Option<String>,
    pub v4: Vec<String>,
    pub v6: Vec<String>,
    /// 地址族独立勾选（§4.3 交互规则 1：默认 IPv4 勾选、IPv6 不勾选）
    pub enable_v4: bool,
    pub enable_v6: bool,
}

impl Default for ProfileDns {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: "dhcp".into(),
            interface_alias: String::new(),
            preset_id: None,
            v4: Vec::new(),
            v6: Vec::new(),
            enable_v4: true,
            enable_v6: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub is_builtin: bool,
    pub note: String,
    pub hosts: ProfileHosts,
    pub proxy: ProfileProxy,
    pub dns: ProfileDns,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            is_builtin: false,
            note: String::new(),
            hosts: ProfileHosts::default(),
            proxy: ProfileProxy::default(),
            dns: ProfileDns::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// 订阅
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub url: String,
    pub mirror: String,
    pub interval_hours: u32,
    pub enabled: bool,
    pub last_sync: Option<String>,
    /// never | ok | failed | staged
    pub last_status: String,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            url: String::new(),
            mirror: String::new(),
            interval_hours: 24,
            enabled: true,
            last_sync: None,
            last_status: "never".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EntryChange {
    pub before: HostsEntry,
    pub after: HostsEntry,
    /// 命中敏感域名清单（§10.1），默认不勾选
    pub sensitive: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct StagedChange {
    pub created_at: String,
    pub added: Vec<HostsEntry>,
    pub changed: Vec<EntryChange>,
    pub removed: Vec<HostsEntry>,
    pub total_after: usize,
    pub risky_count: usize,
    pub first_import: bool,
    /// 新增条目中命中敏感域名清单的 hostname（界面高亮为危险变更）
    #[serde(default)]
    pub sensitive_added: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SourceEntries {
    pub source_id: String,
    pub entries: Vec<HostsEntry>,
    pub last_sync: Option<String>,
    /// 待确认的变更（§10.1：首次导入，或变更 > 总量 20% 时必须过 diff 确认）
    pub staged: Option<StagedChange>,
}

// ---------------------------------------------------------------------------
// 顶层配置
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct DnsPresetStore {
    pub custom: Vec<DnsPreset>,
}

impl Default for DnsPresetStore {
    fn default() -> Self {
        Self { custom: Vec::new() }
    }
}

fn custom_dns_default() -> DnsPreset {
    DnsPreset {
        id: String::new(),
        name: String::new(),
        region: crate::os::dns_presets::DnsRegion::Domestic,
        features: Vec::new(),
        v4: Vec::new(),
        v6: Vec::new(),
        doh: None,
        dot: None,
        note: String::new(),
        is_custom: true,
        is_favorite: false,
    }
}

#[allow(dead_code)]
pub fn blank_custom_preset() -> DnsPreset {
    custom_dns_default()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub schema_version: u32,
    pub settings: Settings,
    pub dns_presets: DnsPresetStore,
    pub sources: Vec<Source>,
    pub profiles: Vec<Profile>,
    pub active_profile_id: Option<String>,
    /// 热键「直连 ↔ 上一模式」的记忆：最近一次被切走的活动方案。
    /// 切到内置直连时记录原方案，再按一次热键切回去（§5.5.5）。
    pub previous_profile_id: Option<String>,
    /// 手工条目（渲染时写在托管区块顶部，永远优先）
    pub hosts_entries: Vec<HostsEntry>,
    /// 订阅条目（与手工条目分离存储，§2.6）
    pub source_entries: Vec<SourceEntries>,
    /// 保留占位：日后加更新通道时不必再做一次迁移（§15.3）
    pub channels: Vec<String>,
    /// 是否已完成首次运行引导（§15.1）
    pub initialized: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA,
            settings: Settings::default(),
            dns_presets: DnsPresetStore::default(),
            sources: vec![Source {
                id: "src-github".into(),
                name: "GitHub 加速".into(),
                url: "https://cdn.jsdelivr.net/gh/Clov614/SteamHostSync@main/Hosts_github".into(),
                mirror: "https://cdn.statically.io/gh/Clov614/SteamHostSync@main/Hosts_github"
                    .into(),
                interval_hours: 24,
                enabled: true,
                last_sync: None,
                last_status: "never".into(),
            }],
            profiles: vec![builtin_profile()],
            active_profile_id: None,
            previous_profile_id: None,
            hosts_entries: Vec::new(),
            source_entries: Vec::new(),
            channels: Vec::new(),
            initialized: false,
        }
    }
}

/// 内置方案「默认 · 直连」不可删除（§7.1）：它是任何回滚的终点
pub fn builtin_profile() -> Profile {
    Profile {
        id: BUILTIN_PROFILE_ID.into(),
        name: "默认 · 直连".into(),
        is_builtin: true,
        note: "清除托管区块、关闭系统代理、DNS 还原为自动获取".into(),
        hosts: ProfileHosts {
            enabled: true,
            entry_ids: Vec::new(),
            source_ids: Vec::new(),
        },
        proxy: ProfileProxy::default(),
        dns: ProfileDns {
            enabled: true,
            mode: "dhcp".into(),
            ..Default::default()
        },
    }
}

impl Config {
    pub fn profile(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn active_profile(&self) -> Option<&Profile> {
        self.active_profile_id
            .as_ref()
            .and_then(|id| self.profile(id))
    }

    /// 展开方案要写入的 hosts 条目：手工条目在前（永远优先），订阅条目在后
    pub fn resolve_hosts_entries(&self, p: &Profile) -> Vec<HostsEntry> {
        let mut out = Vec::new();
        for id in &p.hosts.entry_ids {
            if let Some(e) = self.hosts_entries.iter().find(|e| &e.id == id) {
                out.push(e.clone());
            }
        }
        for sid in &p.hosts.source_ids {
            if let Some(se) = self.source_entries.iter().find(|s| &s.source_id == sid) {
                out.extend(se.entries.iter().cloned());
            }
        }
        out
    }

    pub fn source_entries_of(&self, source_id: &str) -> Vec<HostsEntry> {
        self.source_entries
            .iter()
            .find(|s| s.source_id == source_id)
            .map(|s| s.entries.clone())
            .unwrap_or_default()
    }

    pub fn source_mut(&mut self, source_id: &str) -> Option<&mut SourceEntries> {
        if !self.source_entries.iter().any(|s| s.source_id == source_id) {
            self.source_entries.push(SourceEntries {
                source_id: source_id.to_string(),
                ..Default::default()
            });
        }
        self.source_entries
            .iter_mut()
            .find(|s| s.source_id == source_id)
    }
}

// ---------------------------------------------------------------------------
// 读写
// ---------------------------------------------------------------------------

pub enum LoadOutcome {
    Ready(Config),
    /// 只读模式：配置由更新版本创建 / 损坏或迁移失败 —— 绝不清空用户配置
    ReadOnly(Config, AppError),
}

pub fn load() -> LoadOutcome {
    let path = paths::config_path();
    if !path.exists() {
        let cfg = Config::default();
        if let Err(e) = save(&cfg) {
            return LoadOutcome::ReadOnly(cfg, e);
        }
        return LoadOutcome::Ready(cfg);
    }

    // 按字节读再解码：用户很可能用记事本 / PowerShell 改配置，二者都会写入 UTF-8 BOM。
    // 带 BOM 直接喂给 serde_json 会解析失败，进而把程序推进只读模式 —— 那是不可接受的。
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            return LoadOutcome::ReadOnly(
                Config::default(),
                AppError::coded(E9002).with_detail(format!("读取配置失败：{e}")),
            )
        }
    };
    let text = crate::os::hosts_file::decode(&bytes)
        .unwrap_or_else(|_| String::from_utf8_lossy(&bytes).to_string());
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            // 损坏的配置**不覆盖**，原文件保留，以只读模式启动
            return LoadOutcome::ReadOnly(
                Config::default(),
                AppError::coded(E9002).with_detail(format!("配置 JSON 解析失败：{e}")),
            );
        }
    };

    let from_version = value
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    match crate::migrate::migrate_value(value) {
        Ok((version, migrated)) => match serde_json::from_value::<Config>(migrated) {
            Ok(cfg) => {
                if version != from_version {
                    // 迁移前先备份为 config.v<n>.bak.json（与快照目录分开存放）
                    if let Err(e) = std::fs::write(
                        crate::migrate::backup_path_for(from_version),
                        &text,
                    ) {
                        log::warn!(target: "config", "迁移备份写入失败：{e}");
                    }
                    log::info!(
                        target: "config",
                        "配置已从 v{from_version} 迁移到 v{version}"
                    );
                    if let Err(e) = save(&cfg) {
                        return LoadOutcome::ReadOnly(cfg, e);
                    }
                }
                LoadOutcome::Ready(cfg)
            }
            Err(e) => LoadOutcome::ReadOnly(
                Config::default(),
                AppError::coded(E9002).with_detail(format!("配置字段不匹配：{e}")),
            ),
        },
        Err(e) => {
            let code = e.code.clone();
            if code == E9001 {
                // 用户装过更高版本：拒绝写入，只读启动
                if let Some(cfg) = best_effort_parse(&text) {
                    return LoadOutcome::ReadOnly(cfg, e);
                }
            }
            LoadOutcome::ReadOnly(Config::default(), e)
        }
    }
}

/// 更高版本配置的"尽力解析"：只用于界面展示，任何写操作都会被拒绝
fn best_effort_parse(text: &str) -> Option<Config> {
    serde_json::from_str::<Config>(text).ok()
}

pub fn save(cfg: &Config) -> Result<()> {
    let path = paths::config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(cfg)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text.as_bytes())?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_sane() {
        let cfg = Config::default();
        assert_eq!(cfg.schema_version, CURRENT_SCHEMA);
        assert!(cfg.profile(BUILTIN_PROFILE_ID).is_some());
        assert_eq!(cfg.settings.on_exit, "keep");
        assert_eq!(cfg.settings.shortcuts.clear_proxy.binding, "Ctrl+Alt+S");
        assert!(cfg.sources[0].url.starts_with("https://"));
    }

    #[test]
    fn resolve_orders_manual_before_source() {
        let mut cfg = Config::default();
        cfg.hosts_entries.push(HostsEntry::new("1.1.1.1", "manual.com"));
        cfg.source_entries.push(SourceEntries {
            source_id: "s1".into(),
            entries: vec![HostsEntry::new("2.2.2.2", "source.com")],
            ..Default::default()
        });
        let mut p = Profile::default();
        p.hosts.entry_ids = vec![cfg.hosts_entries[0].id.clone()];
        p.hosts.source_ids = vec!["s1".into()];
        let resolved = cfg.resolve_hosts_entries(&p);
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].hostname, "manual.com");
        assert_eq!(resolved[1].hostname, "source.com");
    }

    #[test]
    fn profile_dns_default_family_flags() {
        let d = ProfileDns::default();
        assert!(d.enable_v4, "IPv4 默认勾选");
        assert!(!d.enable_v6, "IPv6 默认不勾选（§4.3 规则 1）");
    }

    #[test]
    fn serialization_roundtrip() {
        let cfg = Config::default();
        let text = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(back.profiles.len(), cfg.profiles.len());
        assert_eq!(back.settings.log_keep_days, 7);
    }
}

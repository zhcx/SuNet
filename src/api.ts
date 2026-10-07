// 与 Rust 命令层一一对应的类型化封装（设计方案 §8）

import { invoke } from "@tauri-apps/api/core";

export interface CodedError {
  code: string;
  message: string;
  detail?: string;
}

export class ApiError extends Error {
  code: string;
  detail?: string;
  constructor(e: CodedError) {
    super(`[${e.code}] ${e.message}`);
    this.code = e.code;
    this.detail = e.detail;
  }
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    if (e && typeof e === "object" && "code" in e) {
      throw new ApiError(e as CodedError);
    }
    throw new ApiError({ code: "E0000", message: String(e) });
  }
}

// ---------------- 类型 ----------------

export interface HostsEntry {
  id: string;
  ip: string;
  hostname: string;
  enabled: boolean;
  comment: string;
  origin: string;
}

export interface HostsStats {
  path: string;
  exists: boolean;
  block_present: boolean;
  total_entries: number;
  enabled_entries: number;
  file_lines: number;
}

export interface HostsView {
  entries: HostsEntry[];
  stats: HostsStats;
  duplicates: string[];
  pending: boolean;
  preview: string;
}

export interface ProxyState {
  enable: boolean;
  server: string;
  bypass: string;
  autoconfig_url?: string | null;
  pac_present: boolean;
}

export interface ProxyView {
  state: ProxyState;
  can_undo: boolean;
  undo_deadline_ms: number;
  probe_before_enable: boolean;
  scope: [string, string][];
}

export interface ApplyStep {
  kind: string;
  applied: boolean;
  verified: boolean;
  message: string;
}

export interface ApplyReport {
  ok: boolean;
  steps: ApplyStep[];
  rolled_back: boolean;
  recovery_required: boolean;
  snapshot_id?: string | null;
  message: string;
}

export interface ClearReport {
  before: ProxyState;
  ok: boolean;
  undone: boolean;
}

export interface DnsFamilyState {
  is_dhcp: boolean;
  servers: string[];
}

export interface DnsState {
  interface_alias: string;
  interface_guid: string;
  v4: DnsFamilyState;
  v6: DnsFamilyState;
}

export interface DnsInterfaceView {
  alias: string;
  description: string;
  guid: string;
  status: string;
  is_physical: boolean;
  /** 当前正在使用（物理 + 已连接 + 有默认网关） */
  is_in_use: boolean;
  has_v6_global: boolean;
  v4_summary: string;
  v6_summary: string;
}

export interface DnsPreset {
  id: string;
  name: string;
  region: "system" | "domestic" | "global";
  features: string[];
  v4: string[];
  v6: string[];
  doh?: string | null;
  dot?: string | null;
  note: string;
  is_custom: boolean;
  is_favorite: boolean;
}

export interface DohReference {
  name: string;
  doh: string;
  dot: string;
}

export interface DnsTestResult {
  server: string;
  family: string;
  ok: boolean;
  latency_ms: number;
  message: string;
}

export interface ProfileHosts {
  enabled: boolean;
  entry_ids: string[];
  source_ids: string[];
}

export interface ProfileProxy {
  enabled: boolean;
  host: string;
  port: number;
  bypass: string;
}

export interface ProfileDns {
  enabled: boolean;
  mode: string;
  interface_alias: string;
  preset_id?: string | null;
  v4: string[];
  v6: string[];
  enable_v4: boolean;
  enable_v6: boolean;
}

export interface Profile {
  id: string;
  name: string;
  is_builtin: boolean;
  note: string;
  hosts: ProfileHosts;
  proxy: ProfileProxy;
  dns: ProfileDns;
}

export interface SnapshotMeta {
  id: string;
  created_at: string;
  reason: string;
  hosts_bytes: number;
  hosts_existed: boolean;
  proxy_state: string;
  dns_count: number;
  active_profile_id?: string | null;
  restored: boolean;
}

export interface DiffItem {
  kind: string;
  field: string;
  before: string;
  after: string;
}

export interface DiffReport {
  snapshot_id: string;
  created_at: string;
  reason: string;
  items: DiffItem[];
  identical: boolean;
}

export interface ShortcutBinding {
  enabled: boolean;
  binding: string;
}

export interface Settings {
  start_minimized: boolean;
  autostart: { enabled: boolean; method: string };
  on_exit: string;
  proxy_probe_before_enable: boolean;
  confirm_elevation_notice: boolean;
  restore_on_launch: boolean;
  language: string;
  theme: string;
  hosts_backup_keep: number;
  log_keep_days: number;
  log_level: string;
  log_redact: boolean;
  shortcuts: { clear_proxy: ShortcutBinding };
}

export interface PrivilegeState {
  is_elevated: boolean;
  can_write_proxy: boolean;
  can_write_hosts: boolean;
  can_write_dns: boolean;
}

export interface RuntimeStatus {
  dirty: boolean;
  warn_unread: boolean;
  shortcut_registered: boolean;
  shortcut_error?: CodedError | null;
  shortcut_binding: string;
  crash_recovery?: CrashInfo | null;
  lock_contended: boolean;
}

export interface CrashInfo {
  detected_at: string;
  marker: string;
  snapshot_id?: string | null;
}

export interface TrayStatus {
  profile_name?: string | null;
  hosts_count: number;
  proxy: string;
  dns_v4: string;
  dns_v6: string;
  dirty: boolean;
  warn_unread: boolean;
  autostart_enabled: boolean;
  proxy_on: boolean;
}

export interface ProfileBrief {
  id: string;
  name: string;
  is_builtin: boolean;
  proxy_enabled: boolean;
  proxy_host: string;
  proxy_port: number;
  proxy_bypass: string;
  dns_enabled: boolean;
  dns_alias: string;
}

export interface AppStateView {
  version: string;
  initialized: boolean;
  read_only?: CodedError | null;
  prand: PrivilegeState;
  proxy: ProxyState;
  hosts: HostsStats;
  hosts_pending: boolean;
  settings: Settings;
  profiles: ProfileBrief[];
  active_profile_id?: string | null;
  active_profile_name?: string | null;
  autostart_enabled: boolean;
  runtime: RuntimeStatus;
  crash_recovery?: CrashInfo | null;
  tray: TrayStatus;
  can_undo_clear: boolean;
}

export interface Source {
  id: string;
  name: string;
  url: string;
  mirror: string;
  interval_hours: number;
  enabled: boolean;
  last_sync?: string | null;
  last_status: string;
}

export interface EntryChange {
  before: HostsEntry;
  after: HostsEntry;
  sensitive: boolean;
}

export interface StagedChange {
  created_at: string;
  added: HostsEntry[];
  changed: EntryChange[];
  removed: HostsEntry[];
  total_after: number;
  risky_count: number;
  first_import: boolean;
  sensitive_added: string[];
}

export interface SyncResult {
  source_id: string;
  name: string;
  ok: boolean;
  used_url: string;
  http_status: number;
  parsed: number;
  rejected_lines: number;
  rejected_sample: string[];
  needs_confirm: boolean;
  pending?: StagedChange | null;
  message: string;
  last_sync?: string | null;
}

export interface StagedView {
  source_id: string;
  entries_count: number;
  staged?: StagedChange | null;
  last_sync?: string | null;
}

export interface LogRecord {
  ts: string;
  level: string;
  target: string;
  message: string;
}

export interface ShortcutProbe {
  /** 后端收敛后的规范形态，例如 Ctrl+Alt+KeyS → Ctrl+Alt+S */
  binding: string;
  /** 该组合就是当前正在生效的热键 */
  is_current: boolean;
  available: boolean;
}

export interface ShortcutStatus {
  binding: string;
  enabled: boolean;
  registered: boolean;
  error?: CodedError | null;
  reserved_combinations: string[];
}

export interface DirInfo {
  config_dir: string;
  logs_dir: string;
  snapshots_dir: string;
  backup_dir: string;
  hosts_path: string;
}

export interface FirstRunReport {
  initialized: boolean;
  hosts_path: string;
  hosts_custom_lines: number;
  hosts_block_lines: number;
  proxy: ProxyState;
  dns: [string, string][];
  interfaces: number;
}

export interface ResolveResult {
  host: string;
  source: string;
  addresses: string[];
  hosts_hit?: string | null;
  ping_ok: boolean;
  message: string;
}

// ---------------- 命令 ----------------

export const api = {
  getAppState: () => call<AppStateView>("get_app_state"),
  settingsSave: (settings: Settings) => call<Settings>("settings_save", { settings }),

  // hosts
  hostsList: () => call<HostsView>("hosts_list"),
  hostsUpsert: (entry: HostsEntry) => call<HostsView>("hosts_upsert", { entry }),
  hostsDelete: (id: string) => call<HostsView>("hosts_delete", { id }),
  hostsClear: () => call<HostsView>("hosts_clear"),
  hostsBatchBlock: (domains: string[]) => call<HostsView>("hosts_batch_block", { domains }),
  hostsPreview: (entries: HostsEntry[]) => call<string>("hosts_preview", { entries }),
  hostsApply: () => call<ApplyReport>("hosts_apply"),
  hostsImport: () => call<HostsView>("hosts_import"),
  hostsResolve: (host: string) => call<ResolveResult>("hosts_resolve", { host }),

  // proxy
  proxyGet: () => call<ProxyView>("proxy_get"),
  proxySet: (cfg: unknown, probe: boolean) => call<ApplyReport>("proxy_set", { cfg, probe }),
  proxyClear: () => call<ClearReport>("proxy_clear"),
  proxyUndoClear: () => call<boolean>("proxy_undo_clear"),

  // dns
  dnsInterfaces: () => call<DnsInterfaceView[]>("dns_interfaces"),
  dnsGet: (alias: string) => call<DnsState>("dns_get", { alias }),
  dnsSet: (req: { alias: string; v4?: string[] | null; v6?: string[] | null }) =>
    call<ApplyReport>("dns_set", { req }),
  dnsReset: (alias: string, family?: string | null) =>
    call<ApplyReport>("dns_reset", { alias, family: family ?? null }),
  dnsFlush: () => call<void>("dns_flush"),
  dnsPresets: (query = "") => call<DnsPreset[]>("dns_presets", { query }),
  dnsDohReference: () => call<DohReference[]>("dns_doh_reference"),
  dnsCustomSave: (preset: DnsPreset) => call<DnsPreset[]>("dns_custom_save", { preset }),
  dnsCustomDelete: (id: string) => call<DnsPreset[]>("dns_custom_delete", { id }),
  dnsV6Ready: (alias: string) => call<boolean>("dns_v6_ready", { alias }),
  dnsTest: (servers: string[]) => call<DnsTestResult[]>("dns_test", { servers }),

  // profile / 事务
  profileList: () => call<Profile[]>("profile_list"),
  profileSave: (profile: Profile) => call<Profile[]>("profile_save", { profile }),
  profileDelete: (id: string) => call<Profile[]>("profile_delete", { id }),
  profileApply: (id: string) => call<ApplyReport>("profile_apply", { id }),
  rollback: (snapshotId?: string | null) =>
    call<ApplyReport>("rollback", { snapshotId: snapshotId ?? null }),
  globalRestore: () => call<ApplyReport>("global_restore"),
  snapshotsList: () => call<SnapshotMeta[]>("snapshots_list"),
  snapshotsDiff: (id: string) => call<DiffReport>("snapshots_diff", { id }),

  // 订阅
  sourcesList: () => call<Source[]>("sources_list"),
  sourcesSave: (source: Source) => call<Source[]>("sources_save", { source }),
  sourcesDelete: (id: string) => call<Source[]>("sources_delete", { id }),
  sourcesSync: (id: string) => call<SyncResult>("sources_sync", { id }),
  sourcesSyncAll: () => call<SyncResult[]>("sources_sync_all"),
  sourcesCommit: (id: string, includeSensitive: boolean) =>
    call<number>("sources_commit", { id, includeSensitive }),
  sourcesDiscard: (id: string) => call<void>("sources_discard", { id }),
  sourcesStaged: () => call<StagedView[]>("sources_staged"),

  // 系统
  autostartSet: (enable: boolean) => call<void>("autostart_set", { enable }),
  shortcutCheck: (binding: string) => call<ShortcutProbe>("shortcut_check", { binding }),
  shortcutSet: (binding: string) => call<ShortcutStatus>("shortcut_set", { binding }),
  shortcutStatus: () => call<ShortcutStatus>("shortcut_status"),
  /** 录制按键期间临时让出 / 收回全局热键（见 settings.ts 的按键捕获控件） */
  shortcutCapture: (on: boolean) => call<void>("shortcut_capture", { on }),
  logsRecent: (limit = 300) => call<LogRecord[]>("logs_recent", { limit }),
  systemDirs: () => call<DirInfo>("system_dirs"),
  systemOpenDir: (kind: string) => call<void>("system_open_dir", { kind }),
  logsExport: () => call<string>("logs_export"),
  windowHide: () => call<void>("window_hide"),
  windowMinimize: () => call<void>("window_minimize"),
  windowShow: () => call<void>("window_show"),
  quickHide: () => call<void>("quick_hide"),
  quickShow: () => call<void>("quick_show"),
  quickFit: (height: number) => call<void>("quick_fit", { height }),
  quickOpenMain: (tab?: string | null) => call<void>("quick_open_main", { tab: tab ?? null }),
  quickSetPinned: (pinned: boolean) => call<void>("quick_set_pinned", { pinned }),
  markNotificationsRead: () => call<void>("mark_notifications_read"),
  firstRunReport: () => call<FirstRunReport>("first_run_report"),
  firstRunFinish: (name?: string | null) =>
    call<string>("first_run_finish", { name: name ?? null }),
};

export function errText(e: unknown): string {
  if (e instanceof ApiError) {
    return e.detail ? `[${e.code}] ${e.message}（${e.detail}）` : `[${e.code}] ${e.message}`;
  }
  return String(e);
}

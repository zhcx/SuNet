//! 系统命令：状态聚合 / 设置 / 自启 / 全局热键 / 日志 / 诊断包（§5 / §8 / §14）

use crate::config::Settings;
use crate::error::{AppError, Result, E5001, E5002, E5003, E5004};
use crate::os::hosts_file::HostsStats;
use crate::os::privilege::{self, PrivilegeState};
#[cfg(target_os = "windows")]
use crate::os::winapi::{RegisterHotKey, UnregisterHotKey};
use crate::os::system_proxy::ProxyState;
use crate::state::{AppState, CrashInfo, RuntimeStatus, SharedState};
use crate::tray::TrayStatus;
use serde::Serialize;
use std::io::Write;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ---------------------------------------------------------------------------
// 状态聚合
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug)]
pub struct ProfileBrief {
    pub id: String,
    pub name: String,
    pub is_builtin: bool,
    /// 快捷面板要在不开主窗口的前提下判断"能不能一键开代理"，所以摘要里带上代理目标
    pub proxy_enabled: bool,
    pub proxy_host: String,
    pub proxy_port: u16,
    pub proxy_bypass: String,
    /// 方案里记的网卡：快捷面板不摆网卡选择器，「还原自动」直接作用于它
    pub dns_enabled: bool,
    pub dns_alias: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct AppStateView {
    pub version: String,
    pub initialized: bool,
    pub read_only: Option<AppError>,
    pub prand: PrivilegeState,
    pub proxy: ProxyState,
    pub hosts: HostsStats,
    /// 配置草稿与系统文件里的托管区块是否不一致（快捷面板据此决定是否显示「写入」）
    pub hosts_pending: bool,
    pub settings: Settings,
    pub profiles: Vec<ProfileBrief>,
    pub active_profile_id: Option<String>,
    pub active_profile_name: Option<String>,
    pub autostart_enabled: bool,
    pub runtime: RuntimeStatus,
    pub crash_recovery: Option<CrashInfo>,
    pub tray: TrayStatus,
    pub can_undo_clear: bool,
}

fn effective_state(app: &AppHandle) -> Option<Arc<AppState>> {
    app.try_state::<SharedState>().map(|s| s.inner().clone())
}

#[tauri::command]
pub async fn get_app_state(app: AppHandle) -> Result<AppStateView> {
    let a = app.clone();
    crate::cmd::blocking(move || {
        let Some(state) = effective_state(&a) else {
            return Err(AppError::internal("应用状态未初始化"));
        };
        let cfg = state.cfg_clone()?;
        log::debug!(target: "ipc", "get_app_state");
        let mut runtime = state.runtime.lock().map(|r| r.clone()).unwrap_or_default();
        // 另一个 SuNet 进程正在写 hosts/DNS 时如实告知（跨进程互斥体状态）
        runtime.lock_contended = crate::critsec::is_contended();
        let crash = state
            .runtime
            .lock()
            .ok()
            .and_then(|r| r.crash_recovery.clone());
        let can_undo = state
            .runtime
            .lock()
            .map(|r| r.last_clear.is_some() && !r.clear_expired())
            .unwrap_or(false);
        // 统计与托管区块同源一次读出（见 hosts_file::snapshot 的说明）
        let (hosts_stats, block_entries) = crate::os::hosts_file::snapshot()?;
        // 代理状态这里读一次：托盘聚合与下面返回给前端的 proxy 字段共用，少读一次
        let proxy = crate::os::system_proxy::read().ok();
        let tray = crate::tray::status_with(&a, hosts_stats.enabled_entries, proxy.clone());
        Ok(AppStateView {
            version: env!("CARGO_PKG_VERSION").to_string(),
            initialized: cfg.initialized,
            read_only: state.read_only.lock().ok().and_then(|g| g.clone()),
            prand: privilege::detect(),
            proxy: proxy.unwrap_or_default(),
            hosts: hosts_stats,
            hosts_pending: {
                let mut file_lines: Vec<String> =
                    block_entries.iter().map(|e| e.to_line()).collect();
                let mut draft_lines: Vec<String> =
                    cfg.hosts_entries.iter().map(|e| e.to_line()).collect();
                file_lines.sort();
                draft_lines.sort();
                file_lines != draft_lines
            },
            settings: cfg.settings.clone(),
            profiles: cfg
                .profiles
                .iter()
                .map(|p| ProfileBrief {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    is_builtin: p.is_builtin,
                    proxy_enabled: p.proxy.enabled,
                    proxy_host: p.proxy.host.clone(),
                    proxy_port: p.proxy.port,
                    proxy_bypass: p.proxy.bypass.clone(),
                    dns_enabled: p.dns.enabled,
                    dns_alias: p.dns.interface_alias.clone(),
                })
                .collect(),
            active_profile_id: cfg.active_profile_id.clone(),
            active_profile_name: cfg.active_profile().map(|p| p.name.clone()),
            autostart_enabled: cfg.settings.autostart.enabled,
            runtime,
            crash_recovery: crash,
            tray,
            can_undo_clear: can_undo,
        })
    })
    .await
}

// ---------------------------------------------------------------------------
// 设置
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn settings_save(
    app: AppHandle,
    settings: Settings,
    state: State<'_, SharedState>,
) -> Result<Settings> {
    let s: Arc<_> = state.inner().clone();
    let out = crate::cmd::blocking(move || {
        s.assert_writable()?;
        s.with_cfg_mut(|c| {
            c.settings = settings.clone();
            Ok(())
        })?;
        s.save_cfg()?;
        // 日志设置立即生效
        crate::logging::set_level(crate::logging::parse_level(&settings.log_level));
        crate::logging::set_redact(settings.log_redact);
        crate::logging::prune(&crate::paths::logs_dir(), settings.log_keep_days);
        // 静默提权开关也随保存立即生效（用户直接改 config.json 时同样适用）
        crate::elevation_task::set_enabled(settings.silent_elevation);
        Ok(settings)
    })
    .await?;
    crate::cmd::after_change(&app);
    Ok(out)
}

// ---------------------------------------------------------------------------
// 开机自启（计划任务，§5.2）
// ---------------------------------------------------------------------------

pub fn set_autostart(app: &AppHandle, enable: bool) -> Result<()> {
    platform_autostart(enable)?;
    log::info!(target: "autostart", "开机自启动 → {enable}");
    if let Some(s) = effective_state(app) {
        let _ = s.with_cfg_mut(|c| {
            c.settings.autostart.enabled = enable;
            Ok(())
        });
        let _ = s.save_cfg();
    }
    crate::tray::refresh(app);
    Ok(())
}

/// Windows：登录时计划任务（实现见 `os::autostart`，`/RL LIMITED`）。
///
/// **必须提权**：任务库 `C:\Windows\System32\Tasks` 只有管理员可写，
/// 而主进程是 asInvoker —— 之前在主进程里直接调 `schtasks /Create`，
/// 结果是必定被拒（退出码 1 + 拒绝访问）。现在走提权任务通道：
/// 装过静默提权通道时这一步是静默的，否则会弹一次 UAC。
#[cfg(target_os = "windows")]
fn platform_autostart(enable: bool) -> Result<()> {
    crate::elevation::run_elevated(
        "autostart_set",
        serde_json::json!({ "enable": enable }),
        crate::elevation::TASK_TIMEOUT,
    )?;
    Ok(())
}

/// macOS：LaunchAgent（登录时以**当前用户**启动，不弹密码）。
///
/// 不用 LaunchDaemon：那会以 root 常驻并全权限运行，与"主进程普通权限、
/// helper 是唯一常驻 root 进程且只跑白名单任务"的安全边界冲突。
#[cfg(target_os = "macos")]
fn platform_autostart(enable: bool) -> Result<()> {
    use crate::os::macos::net;

    let plist = autostart_plist_path()?;
    let uid = unsafe { libc::getuid() };
    let service = format!("gui/{uid}/{}", crate::paths::AUTOSTART_LABEL);

    if !enable {
        // 本来就没装时 bootout 会失败，不算错误
        if let Ok((ok, _, err)) = net::try_run(net::LAUNCHCTL, &["bootout".into(), service]) {
            if !ok {
                log::debug!(target: "autostart", "bootout 返回非零（可能本就没装）：{err}");
            }
        }
        if plist.exists() {
            std::fs::remove_file(&plist)?;
        }
        return Ok(());
    }

    let exe = crate::paths::exe_path()?;
    if let Some(dir) = plist.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&plist, launch_agent_xml(&exe.to_string_lossy()))?;

    // 先 bootout 旧实例再 bootstrap（重复启用时不会失败）
    let _ = net::try_run(net::LAUNCHCTL, &["bootout".into(), service.clone()]);
    let (ok, out, err) = net::try_run(
        net::LAUNCHCTL,
        &[
            "bootstrap".into(),
            format!("gui/{uid}"),
            plist.to_string_lossy().to_string(),
        ],
    )?;
    if !ok {
        return Err(AppError::coded("E1002").with_detail(format!(
            "注册 LaunchAgent 失败：{}",
            first_line(&err).or_else(|| first_line(&out)).unwrap_or_default()
        )));
    }
    log::info!(target: "autostart", "LaunchAgent 已注册：{}", plist.display());
    Ok(())
}

#[cfg(target_os = "macos")]
fn autostart_plist_path() -> Result<std::path::PathBuf> {
    Ok(crate::paths::launch_agents_dir().join(format!("{}.plist", crate::paths::AUTOSTART_LABEL)))
}

/// LaunchAgent plist（--minimized：登录后静默进托盘，不弹主窗口）
#[cfg(target_os = "macos")]
fn launch_agent_xml(exe: &str) -> String {
    let exe = xml_escape(exe);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>--minimized</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
  <key>ProcessType</key>
  <string>Interactive</string>
</dict>
</plist>
"#,
        label = crate::paths::AUTOSTART_LABEL,
    )
}

#[cfg(target_os = "macos")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(target_os = "macos")]
fn first_line(s: &str) -> Option<String> {
    let l = s.trim().lines().next().unwrap_or("").trim();
    if l.is_empty() {
        None
    } else {
        Some(l.to_string())
    }
}

#[tauri::command]
pub async fn autostart_set(app: AppHandle, enable: bool) -> Result<()> {
    let a = app.clone();
    crate::cmd::blocking(move || set_autostart(&a, enable)).await
}

// ---------------------------------------------------------------------------
// 全局热键（§5.5）
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug)]
pub struct ShortcutStatus {
    pub binding: String,
    pub enabled: bool,
    /// 真实注册结果 —— 不允许"读配置文件就显示已启用"
    pub registered: bool,
    pub error: Option<AppError>,
    pub reserved_combinations: Vec<String>,
}

#[cfg(target_os = "windows")]
const PROBE_ID: i32 = 0x5F27;

// 修饰键位标志。取值与 Win32 `MOD_*` 相同（Windows 的探测要按这个值传）。
// macOS 侧只用于「规范名 ↔ 位标志」的转换与展示，真正的注册交给插件解析绑定串。
const MOD_ALT: u32 = 0x0001;
const MOD_CONTROL: u32 = 0x0002;
const MOD_SHIFT: u32 = 0x0004;
const MOD_WIN: u32 = 0x0008;

/// 按键录制中标志（供 `shortcut_capture` 置位）。
/// 录制期间全局热键是被**故意**从系统里摘掉的，此时 `runtime.shortcut_registered` 为 false。
/// `shortcut_status` 有一个「掉线就自愈重注册」的补救，如果不看这个标志，
/// 它就会在录制中途把热键又注册回去 —— 用户按到当前组合时依然会切一次方案。
static CAPTURING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// ---------------------------------------------------------------------------
// 快捷键键名表（唯一真源）
//
// 这里以前有三套互不相认的键名，是"快捷键只能手打、还老切换失败"的根源之一：
//   1. 界面上只有一个自由文本输入框，用户只能自己敲 Ctrl+Alt+S；
//   2. 本文件的 key_to_vk 只认字母 / 数字 / F1-F24 和少数几个命名键；
//   3. 插件 global-hotkey 另有一套名字（MINUS / NUMPAD5 / BRACKETLEFT / UP …）。
// 于是"检测可用性"和"应用"对同一个组合可能给出相反答案，用户看到的就是不稳定。
// 现在统一到本表：既收界面录制产出的 KeyboardEvent.code（KeyS / Digit1 / Numpad5
// / ArrowUp …），也收插件名与字符写法，输出统一的规范名（见 HotkeySpec::text）。
// ---------------------------------------------------------------------------

/// 探测结果：比一个裸 bool 多两件事 —— 规范化后的组合、以及"这就是当前生效的那个"
#[derive(Serialize, Clone, Debug)]
pub struct ShortcutProbe {
    pub binding: String,
    /// 该组合正是当前生效的热键（此时探测必然"被占用"，那是自己）
    pub is_current: bool,
    pub available: bool,
}

/// 修饰键：兼容插件名、code 名与旧配置里可能出现的写法，统一成 Ctrl / Alt / Shift / Super
fn mods_of(part: &str) -> Option<u32> {
    match part.trim().to_ascii_lowercase().as_str() {
        "ctrl" | "control" | "cmdorctrl" | "commandorcontrol" => Some(MOD_CONTROL),
        "alt" | "option" => Some(MOD_ALT),
        "shift" => Some(MOD_SHIFT),
        // 与插件保持一致：CMD / COMMAND 指 Win 键；旧代码把 command 当 Ctrl，同样属于两套表现不一致
        "super" | "win" | "meta" | "cmd" | "command" | "logo" => Some(MOD_WIN),
        _ => None,
    }
}

/// 主键：返回 (VK, 规范名)。规范名与 KeyboardEvent.code 同名（去掉 Key / Digit 前缀），
/// 同时也是插件 parse_key 认识的写法，可以直接拼进注册串与配置文件。
fn key_of(part: &str) -> Option<(u32, String)> {
    let raw = part.trim();
    if raw.is_empty() {
        return None;
    }
    let k = raw.to_ascii_lowercase();

    // A–Z / KeyA
    let letter = k.strip_prefix("key").unwrap_or(k.as_str());
    if letter.len() == 1 {
        let c = letter.chars().next().unwrap();
        if c.is_ascii_lowercase() {
            let up = c.to_ascii_uppercase();
            return Some((up as u32, up.to_string()));
        }
    }
    // 0–9 / Digit0
    let digit = k.strip_prefix("digit").unwrap_or(k.as_str());
    if digit.len() == 1 {
        let c = digit.chars().next().unwrap();
        if c.is_ascii_digit() {
            return Some((c as u32, c.to_string()));
        }
    }
    // F1–F24
    if let Some(rest) = k.strip_prefix('f') {
        if let Ok(n) = rest.parse::<u32>() {
            if (1..=24).contains(&n) {
                return Some((0x70 + n - 1, format!("F{n}")));
            }
        }
    }
    // 小键盘数字：Numpad5 / Num5
    if let Some(rest) = k.strip_prefix("numpad").or_else(|| k.strip_prefix("num")) {
        if rest.len() == 1 && rest.as_bytes()[0].is_ascii_digit() {
            let n = (rest.as_bytes()[0] - b'0') as u32;
            return Some((0x60 + n, format!("Numpad{n}")));
        }
    }

    let (vk, name): (u32, &str) = match k.as_str() {
        "space" | "spacebar" => (0x20, "Space"),
        "escape" | "esc" => (0x1B, "Esc"),
        "enter" | "return" => (0x0D, "Enter"),
        "tab" => (0x09, "Tab"),
        "backspace" => (0x08, "Backspace"),
        "insert" | "ins" => (0x2D, "Insert"),
        "delete" | "del" => (0x2E, "Delete"),
        "home" => (0x24, "Home"),
        "end" => (0x23, "End"),
        "pageup" | "pgup" => (0x21, "PageUp"),
        "pagedown" | "pgdn" => (0x22, "PageDown"),
        "up" | "arrowup" => (0x26, "Up"),
        "down" | "arrowdown" => (0x28, "Down"),
        "left" | "arrowleft" => (0x25, "Left"),
        "right" | "arrowright" => (0x27, "Right"),
        "capslock" => (0x14, "CapsLock"),
        "numlock" => (0x90, "NumLock"),
        "scrolllock" => (0x91, "ScrollLock"),
        "printscreen" | "snapshot" => (0x2C, "PrintScreen"),
        // 主键盘标点：code 名 / 字符 / 插件名都能收
        "minus" | "-" => (0xBD, "Minus"),
        "equal" | "=" => (0xBB, "Equal"),
        "bracketleft" | "[" => (0xDB, "BracketLeft"),
        "bracketright" | "]" => (0xDD, "BracketRight"),
        "backslash" | "\\" => (0xDC, "Backslash"),
        "semicolon" | ";" => (0xBA, "Semicolon"),
        "quote" | "'" => (0xDE, "Quote"),
        "comma" | "," => (0xBC, "Comma"),
        "period" | "." => (0xBE, "Period"),
        "slash" | "/" => (0xBF, "Slash"),
        "backquote" | "`" => (0xC0, "Backquote"),
        // 小键盘运算符
        "numpadadd" | "numpadplus" | "numadd" | "numplus" => (0x6B, "NumpadAdd"),
        "numpadsubtract" | "numpadminus" | "numsub" => (0x6D, "NumpadSubtract"),
        "numpadmultiply" | "nummul" => (0x6A, "NumpadMultiply"),
        "numpaddivide" | "numdiv" => (0x6F, "NumpadDivide"),
        "numpaddecimal" | "numdec" => (0x6E, "NumpadDecimal"),
        "numpadequal" | "numequal" => (0x92, "NumpadEqual"),
        "numpadenter" | "numenter" => (0x0D, "NumpadEnter"),
        _ => return None,
    };
    Some((vk, name.to_string()))
}

/// 解析结果：修饰键掩码 + VK + 规范键名
struct HotkeySpec {
    mods: u32,
    vk: u32,
    key: String,
}

impl HotkeySpec {
    /// 规范串：修饰键固定按 Ctrl → Alt → Shift → Super 排序（配置里存的就是这个形态）
    fn text(&self) -> String {
        let mut s = String::new();
        for (bit, name) in [
            (MOD_CONTROL, "Ctrl"),
            (MOD_ALT, "Alt"),
            (MOD_SHIFT, "Shift"),
            (MOD_WIN, "Super"),
        ] {
            if self.mods & bit != 0 {
                s.push_str(name);
                s.push('+');
            }
        }
        s.push_str(&self.key);
        s
    }
}

/// 解析快捷键串。要求：至少一个修饰键（没有修饰键的全局热键会把正常打字全抢走），
/// 且修饰键必须写在主键之前（插件按顺序解析，混写会解析失败）。
fn parse_binding(binding: &str) -> Option<HotkeySpec> {
    let mut mods = 0u32;
    let mut key: Option<(u32, String)> = None;
    for part in binding.split('+') {
        let t = part.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(m) = mods_of(t) {
            if key.is_some() {
                return None;
            }
            mods |= m;
            continue;
        }
        if key.is_some() {
            return None;
        }
        key = Some(key_of(t)?);
    }
    let (vk, key) = key?;
    if mods == 0 {
        return None;
    }
    Some(HotkeySpec { mods, vk, key })
}

/// 无法识别的组合一律走 E5004，文案里直接说清缺什么
fn bad_binding(raw: &str) -> AppError {
    AppError::coded(E5004).with_detail(raw.trim().to_string())
}

/// 规范化：把各种合法写法收敛成唯一形态。
/// 改绑判定依赖它 —— 字符串比较 "ctrl+alt+s" 与 "Ctrl+Alt+S" 才是同一个热键。
fn normalize_binding(raw: &str) -> Result<String> {
    parse_binding(raw)
        .map(|s| s.text())
        .ok_or_else(|| bad_binding(raw))
}

/// 试探性注册探测（§5.5.4）：Win32 没有查询接口，借 RegisterHotKey 的独占语义。
/// 注意它分不出"被别的程序占用"和"被本程序自己占用"，所以调用方必须先排除
/// "这个组合就是当前正在生效的那个"，否则同一个组合点两次应用就会莫名报 E5001。
#[cfg(target_os = "windows")]
fn probe_available(mods: u32, vk: u32) -> bool {
    unsafe {
        // 传 hWnd = NULL：不需要窗口，只探测系统热键表
        let ok = RegisterHotKey(std::ptr::null_mut(), PROBE_ID, mods, vk);
        if ok != 0 {
            // 探测成功后必须立即释放，否则自己把这个组合占了
            UnregisterHotKey(std::ptr::null_mut(), PROBE_ID);
        }
        ok != 0
    }
}

/// macOS：系统不提供"查询组合是否被占用"的接口（Carbon 的 RegisterEventHotKey 也只有成败），
/// 所以不做预判 —— 返回 true 交给真实注册判定（注册失败由 `map_plugin_error` 分 E5001/E5003）。
#[cfg(target_os = "macos")]
fn probe_available(_mods: u32, _vk: u32) -> bool {
    true
}

fn map_plugin_error(e: &str) -> AppError {
    let lower = e.to_lowercase();
    if lower.contains("1409") || lower.contains("already") || lower.contains("registered") {
        AppError::coded(E5001).with_detail(e.to_string())
    } else if lower.contains("reserved") {
        AppError::coded(E5002).with_detail(e.to_string())
    } else {
        AppError::coded(E5003).with_detail(e.to_string())
    }
}

/// 释放本程序注册的全部全局热键。
///
/// 为什么不是 `unregister(旧组合)`：插件内部按"组合"存注册表，拿**新**组合去 unregister
/// 释放的是新组合（旧代码就是这么写的），改绑之后旧组合会一直留在系统热键表里，
/// 新旧两个组合同时生效 —— 这是"改绑不稳定"最直接的来源。
/// 本程序只注册这一个全局热键（切换直连 / 上一个方案），整表释放是唯一可靠的清理方式。
fn release_all_shortcuts(app: &AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    if let Err(e) = app.global_shortcut().unregister_all() {
        log::debug!(target: "hotkey", "释放全局热键：{e}");
    }
}

fn write_shortcut_state(app: &AppHandle, binding: &str, registered: bool, error: Option<AppError>) {
    if let Some(s) = effective_state(app) {
        if let Ok(mut rt) = s.runtime.lock() {
            rt.shortcut_registered = registered;
            rt.shortcut_error = error;
            // 释放时保留旧值：shortcut_status 还要靠它回落显示"配置里想注册什么"
            if registered {
                rt.shortcut_binding = binding.to_string();
            }
        }
    }
}

/// 注册全局热键，返回规范化后的组合串。
/// 先整表释放再注册 —— 保证任何一次改绑之后，系统里只剩这一个组合。
pub fn register_shortcut(app: &AppHandle, binding: &str) -> Result<String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    if binding.trim().is_empty() {
        // 没配置热键不是错误：什么都不做，也不动 runtime 状态
        return Ok(String::new());
    }
    let text = normalize_binding(binding)?;
    let shortcut = text
        .parse::<tauri_plugin_global_shortcut::Shortcut>()
        .map_err(|e| AppError::coded(E5003).with_detail(format!("{text}：{e}")))?;
    release_all_shortcuts(app);
    app.global_shortcut()
        .register(shortcut)
        .map_err(|e| map_plugin_error(&e.to_string()))?;

    log::info!(target: "hotkey", "全局热键注册成功：{text}");
    write_shortcut_state(app, &text, true, None);
    Ok(text)
}

fn record_shortcut_failure(app: &AppHandle, err: &AppError) {
    log::warn!(target: "hotkey", "全局热键未生效：{err}");
    write_shortcut_state(app, "", false, Some(err.clone()));
}

/// runtime 里的"实际注册情况"（配置只说明想注册什么，不能当真）
fn runtime_shortcut(s: &AppState) -> RuntimeStatus {
    s.runtime.lock().map(|r| r.clone()).unwrap_or_default()
}

fn status_of(s: &AppState, configured: &crate::config::ShortcutBinding) -> ShortcutStatus {
    let rt = runtime_shortcut(s);
    ShortcutStatus {
        binding: if rt.shortcut_binding.is_empty() {
            configured.binding.clone()
        } else {
            rt.shortcut_binding
        },
        enabled: configured.enabled,
        registered: rt.shortcut_registered,
        error: rt.shortcut_error,
        reserved_combinations: reserved_list(),
    }
}

/// 热键只有一个入口：注册成功后立刻落盘，避免"界面生效了、重启又回去"
fn save_shortcut_cfg(s: &AppState, binding: &str) -> Result<()> {
    s.with_cfg_mut(|c| {
        c.settings.shortcuts.clear_proxy.binding = binding.to_string();
        c.settings.shortcuts.clear_proxy.enabled = true;
        Ok(())
    })?;
    s.save_cfg()
}

#[tauri::command]
pub async fn shortcut_check(
    binding: String,
    state: State<'_, SharedState>,
) -> Result<ShortcutProbe> {
    let s: Arc<_> = state.inner().clone();
    crate::cmd::blocking(move || {
        let spec = parse_binding(&binding).ok_or_else(|| bad_binding(&binding))?;
        let text = spec.text();
        let rt = runtime_shortcut(&s);
        // 自己正在用的组合必然探测出"被占用"，那其实是自己，直接判可用并说明
        if rt.shortcut_registered && rt.shortcut_binding == text {
            return Ok(ShortcutProbe {
                binding: text,
                is_current: true,
                available: true,
            });
        }
        Ok(ShortcutProbe {
            binding: text,
            is_current: false,
            available: probe_available(spec.mods, spec.vk),
        })
    })
    .await
}

#[tauri::command]
pub async fn shortcut_set(
    app: AppHandle,
    binding: String,
    state: State<'_, SharedState>,
) -> Result<ShortcutStatus> {
    let s: Arc<_> = state.inner().clone();
    let a = app.clone();
    let outer = s.clone();
    let out = crate::cmd::blocking(move || {
        s.assert_writable()?;
        let spec = parse_binding(&binding).ok_or_else(|| bad_binding(&binding))?;
        let text = spec.text();
        let rt = runtime_shortcut(&s);

        // ① 目标就是当前正在生效的组合：直接确认即可。
        //    绝不能走"先释放再注册"：RegisterHotKey 探测会被自己占用而失败，
        //    旧代码因此把"同一个组合再应用一次"误报成 E5001 已被其他程序占用。
        if rt.shortcut_registered && rt.shortcut_binding == text {
            save_shortcut_cfg(&s, &text)?;
            let c = s.cfg_clone()?;
            return Ok(status_of(&s, &c.settings.shortcuts.clear_proxy));
        }

        // ② 先让出自己占用的组合，再做占用探测
        let had_own = rt.shortcut_registered && !rt.shortcut_binding.is_empty();
        if had_own {
            release_all_shortcuts(&a);
            write_shortcut_state(&a, "", false, None);
        }
        if !probe_available(spec.mods, spec.vk) {
            if had_own {
                // 只是探测失败，原热键仍然是我们的，要接回来
                let _ = register_shortcut(&a, &rt.shortcut_binding);
            }
            return Err(AppError::coded(E5001).with_detail(format!("{text} 已被其他程序占用")));
        }

        // ③ 注册新组合；失败则回滚到原来那个（§5.5.9）
        if let Err(e) = register_shortcut(&a, &text) {
            if had_own {
                let _ = register_shortcut(&a, &rt.shortcut_binding);
            }
            return Err(e);
        }
        save_shortcut_cfg(&s, &text)?;
        let c = s.cfg_clone()?;
        Ok(status_of(&s, &c.settings.shortcuts.clear_proxy))
    })
    .await;

    match out {
        Ok(v) => Ok(v),
        Err(e) => {
            // 只有"现在确实没有生效的热键"才记为注册失败。
            // 旧代码在这里无条件 record_shortcut_failure，会把刚回滚成功的状态抹成"未生效"，
            // 界面显示未生效、实际却在工作 —— 也是"不稳定"观感的一部分。
            if !runtime_shortcut(&outer).shortcut_registered {
                record_shortcut_failure(&app, &e);
            }
            Err(e)
        }
    }
}

/// 录制按键期间临时让出全局热键。
/// 全局热键是系统级注册：录制时若按到当前正在生效的那个组合，会真的切一次方案
/// （含 hosts/DNS 时还会弹 UAC），所以录制开始先释放，录制结束或取消再按配置恢复。
#[tauri::command]
pub async fn shortcut_capture(
    app: AppHandle,
    on: bool,
    state: State<'_, SharedState>,
) -> Result<()> {
    let s: Arc<_> = state.inner().clone();
    let a = app.clone();
    crate::cmd::blocking(move || {
        if on {
            CAPTURING.store(true, std::sync::atomic::Ordering::SeqCst);
            release_all_shortcuts(&a);
            write_shortcut_state(&a, "", false, None);
            log::debug!(target: "hotkey", "按键录制中：全局热键已临时释放");
            return Ok(());
        }
        CAPTURING.store(false, std::sync::atomic::Ordering::SeqCst);
        let b = s.cfg_clone()?.settings.shortcuts.clear_proxy.clone();
        if !b.enabled || b.binding.trim().is_empty() {
            return Ok(());
        }
        if let Err(e) = register_shortcut(&a, &b.binding) {
            record_shortcut_failure(&a, &e);
        }
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn shortcut_status(
    app: AppHandle,
    state: State<'_, SharedState>,
) -> Result<ShortcutStatus> {
    let s: Arc<_> = state.inner().clone();
    let configured = s.cfg_clone()?.settings.shortcuts.clear_proxy.clone();
    let rt = runtime_shortcut(&s);
    // 以真实注册结果为准，配置文件只提供"想注册什么"。
    // 查状态是唯一的补救点（启动 2 秒后前端会问一次），顺手把掉线的热键补回来。
    if configured.enabled
        && !rt.shortcut_registered
        && !configured.binding.trim().is_empty()
        // 录制中释放是故意的：此时绝不能“自愈”重注册
        && !CAPTURING.load(std::sync::atomic::Ordering::SeqCst)
    {
        let a = app.clone();
        let b = configured.binding.clone();
        crate::cmd::blocking(move || {
            if let Err(e) = register_shortcut(&a, &b) {
                record_shortcut_failure(&a, &e);
            }
            Ok(())
        })
        .await?;
    }
    Ok(status_of(&s, &configured))
}

fn reserved_list() -> Vec<String> {
    [
        "Ctrl+Alt+Del",
        "Win+L",
        "Win+U",
        "Win+E",
        "Win+D",
        "Win+I",
        "Win+S",
        "Win+G",
        "Win+P",
        "Win+Tab",
        "Win+方向键",
        "Alt+Tab",
        "Win+Ctrl+Shift+B",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

// ---------------------------------------------------------------------------
// 日志与诊断
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn logs_recent(limit: usize) -> Result<Vec<crate::logging::LogRecord>> {
    crate::cmd::blocking(move || Ok(crate::logging::recent(limit.clamp(10, 800)))).await
}

#[derive(Serialize, Clone, Debug)]
pub struct DirInfo {
    pub config_dir: String,
    pub logs_dir: String,
    pub snapshots_dir: String,
    pub backup_dir: String,
    pub hosts_path: String,
}

#[tauri::command]
pub async fn system_dirs() -> Result<DirInfo> {
    crate::cmd::blocking(move || {
        Ok(DirInfo {
            config_dir: crate::paths::appdata_dir().to_string_lossy().to_string(),
            logs_dir: crate::paths::logs_dir().to_string_lossy().to_string(),
            snapshots_dir: crate::paths::snapshots_dir().to_string_lossy().to_string(),
            backup_dir: crate::paths::backup_dir().to_string_lossy().to_string(),
            hosts_path: crate::os::hosts_file::hosts_path().to_string_lossy().to_string(),
        })
    })
    .await
}

fn open_folder(path: &std::path::Path) -> Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
    }
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("explorer.exe");
        c.arg(path.as_os_str());
        c.creation_flags(CREATE_NO_WINDOW);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        // Finder 的通用打开入口；打开自己的目录不需要任何权限
        let mut c = std::process::Command::new("/usr/bin/open");
        c.arg(path.as_os_str());
        c
    };
    cmd.spawn()
        .map_err(|e| AppError::internal(format!("打开目录失败：{e}")))?;
    Ok(())
}

/// kind: config | logs | snapshots | backup | hosts
#[tauri::command]
pub async fn system_open_dir(kind: String) -> Result<()> {
    crate::cmd::blocking(move || match kind.as_str() {
        "logs" => open_folder(&crate::paths::logs_dir()),
        "snapshots" => open_folder(&crate::paths::snapshots_dir()),
        "backup" => open_folder(&crate::paths::backup_dir()),
        "hosts" => open_folder(
            crate::os::hosts_file::hosts_path()
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default()
                .as_path(),
        ),
        _ => open_folder(&crate::paths::appdata_dir()),
    })
    .await
}

#[cfg(target_os = "windows")]
fn os_version() -> String {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    let hklm = winreg::RegKey::predef(HKEY_LOCAL_MACHINE);
    let path = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    match hklm.open_subkey_with_flags(path, KEY_READ) {
        Ok(k) => {
            let product: String = k.get_value("ProductName").unwrap_or_default();
            let display: String = k.get_value("DisplayVersion").unwrap_or_default();
            let build: String = k.get_value("CurrentBuild").unwrap_or_default();
            format!("{product} {display} (Build {build})")
        }
        Err(_) => "未知".to_string(),
    }
}

/// macOS：`sw_vers` 三个字段，比解析 `sysctl kern.osrelease` 可靠（诊断包要人看的）
#[cfg(target_os = "macos")]
fn os_version() -> String {
    let field = |flag: &str| -> Option<String> {
        std::process::Command::new("/usr/bin/sw_vers")
            .arg(flag)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let name = field("-productName").unwrap_or_else(|| "macOS".to_string());
    let ver = field("-productVersion").unwrap_or_else(|| "未知".to_string());
    match field("-buildVersion") {
        Some(b) => format!("{name} {ver} (Build {b})"),
        None => format!("{name} {ver}"),
    }
}

/// 导出诊断包（§14.2）：默认脱敏
#[tauri::command]
pub async fn logs_export(_app: AppHandle, state: State<'_, SharedState>) -> Result<String> {
    let s: Arc<_> = state.inner().clone();
    let path = crate::cmd::blocking(move || {
        let cfg = s.cfg_clone()?;
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let out_path = crate::paths::appdata_dir().join(format!("diagnostic-{ts}.zip"));
        let file = std::fs::File::create(&out_path)?;
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        zip.start_file("README.txt", opts)?;
        zip.write_all(
            format!(
                "SuNet 诊断包\n\
                 生成时间：{}\n\
                 程序版本：{}\n\
                 脱敏状态：{}\n\n\
                 包含内容：\n\
                 - logs/            最近 3 天日志\n\
                 - state.json       当前三层状态 + 方案列表\n\
                 - environment.json 系统与运行环境\n\
                 - config.schema.json\n\n\
                 已脱敏：代理主机名（保留端口）、内网地址、用户目录、订阅 URL 的 query 段。\n",
                chrono::Local::now().to_rfc3339(),
                env!("CARGO_PKG_VERSION"),
                if crate::logging::is_redacting() { "已开启" } else { "已关闭（用户设置）" }
            )
            .as_bytes(),
        )?;

        for (name, content) in crate::logging::read_recent_files(&crate::paths::logs_dir(), 3) {
            zip.start_file(format!("logs/{name}"), opts)?;
            zip.write_all(content.as_bytes())?;
        }

        let proxy = crate::os::system_proxy::read().unwrap_or_default();
        let state_json = serde_json::json!({
            "active_profile_id": cfg.active_profile_id,
            "profiles": cfg.profiles.iter().map(|p| &p.name).collect::<Vec<_>>(),
            "proxy": {
                "enable": proxy.enable,
                "server": if proxy.server.is_empty() { String::new() } else { "***".to_string() },
                "pac_present": proxy.pac_present,
            },
            "hosts": crate::os::hosts_file::stats().ok(),
            "sources": cfg.sources.iter().map(|s| serde_json::json!({
                "name": s.name,
                "status": s.last_status,
                "last_sync": s.last_sync,
            })).collect::<Vec<_>>(),
        });
        zip.start_file("state.json", opts)?;
        zip.write_all(serde_json::to_string_pretty(&state_json)?.as_bytes())?;

        let env_json = serde_json::json!({
            "os": os_version(),
            "arch": std::env::consts::ARCH,
            "app_version": env!("CARGO_PKG_VERSION"),
            "elevated": crate::os::privilege::is_elevated(),
            "user_sid": crate::os::fs_security::current_user_sid(),
            "interfaces": crate::os::dns_client::list_interfaces().map(|list| {
                list.iter().map(|i| serde_json::json!({
                    "alias": i.alias,
                    "status": i.status,
                    "physical": i.is_physical,
                    "has_v6": i.has_v6_global,
                })).collect::<Vec<_>>()
            }).unwrap_or_default(),
        });
        zip.start_file("environment.json", opts)?;
        zip.write_all(serde_json::to_string_pretty(&env_json)?.as_bytes())?;

        zip.start_file("config.schema.json", opts)?;
        zip.write_all(
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": cfg.schema_version,
                "current_schema": crate::migrate::CURRENT_SCHEMA,
            }))?
            .as_bytes(),
        )?;

        zip.finish()?;
        log::info!(target: "diagnostic", "已导出诊断包：{}", out_path.display());
        Ok(out_path.to_string_lossy().to_string())
    })
    .await?;
    open_folder(
        std::path::Path::new(&path)
            .parent()
            .unwrap_or(std::path::Path::new(".")),
    )?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// 窗口
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn window_hide(app: AppHandle) -> Result<()> {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
    Ok(())
}

/// 最小化主窗口。
/// 主窗口是 `decorations: false`（自绘顶栏），系统标题栏上的最小化按钮不存在了，
/// 所以由界面上的「−」按钮调过来。窗口操作统一走 Rust，前端不开放 window 权限。
#[tauri::command]
pub async fn window_minimize(app: AppHandle) -> Result<()> {
    let win = app
        .get_webview_window("main")
        .ok_or_else(|| AppError::internal("主窗口不存在"))?;
    win.minimize()
        .map_err(|e| AppError::internal(format!("最小化主窗口失败：{e}")))?;
    Ok(())
}

#[tauri::command]
pub async fn window_show(app: AppHandle) -> Result<()> {
    crate::tray::show_window(&app, None);
    Ok(())
}

/// 收起快捷面板（面板里的 Esc / 关闭按钮 / 打开主窗口都会走到这里）
#[tauri::command]
pub async fn quick_hide(app: AppHandle) -> Result<()> {
    crate::tray::hide_quick(&app);
    Ok(())
}

/// 从快捷面板跳主窗口：先收起面板，再显示主窗口并切到指定标签页
#[tauri::command]
pub async fn quick_open_main(app: AppHandle, tab: Option<String>) -> Result<()> {
    crate::tray::show_window(&app, tab.as_deref());
    Ok(())
}

/// 面板高度贴合内容（飞行面板不该留一大片空白）。
/// 高度由前端量出来传进来，这里只负责：夹住范围、保持**底边不动**地向右上生长。
#[tauri::command]
pub async fn quick_fit(app: AppHandle, height: f64) -> Result<()> {
    let Some(win) = app.get_webview_window("quick") else {
        return Ok(());
    };
    let h = height.clamp(200.0, 720.0);
    let (old_pos, old_inner, scale) = match (
        win.outer_position(),
        win.inner_size(),
        win.scale_factor(),
    ) {
        (Ok(p), Ok(sz), Ok(sc)) => (p, sz, sc),
        _ => return Ok(()),
    };
    // 高度变化很小就跳过，避免每次刷新都抖动
    let new_phys_h = (h * scale).round() as i32;
    let delta = new_phys_h - old_inner.height as i32;
    if delta.abs() <= 2 {
        return Ok(());
    }
    win.set_size(tauri::LogicalSize::new(372.0, h))
        .map_err(|e| AppError::internal(format!("调整面板尺寸失败：{e}")))?;
    // 锚在托盘侧：以**内高**变化量反向平移，保证底边不动（外框厚度是常量）
    win.set_position(tauri::PhysicalPosition::new(old_pos.x, old_pos.y - delta))
        .map_err(|e| AppError::internal(format!("调整面板位置失败：{e}")))?;
    log::debug!(target: "tray", "面板高度自适应 → {:.0}（物理 {new_phys_h}）", h);
    Ok(())
}

/// 从主窗口唤出快捷面板（托盘左键之外的第二条路径）
#[tauri::command]
pub async fn quick_show(app: AppHandle) -> Result<()> {
    crate::tray::show_quick_anchored(&app);
    Ok(())
}

/// 标记面板是否正在执行操作（执行中失焦不收起，否则 UAC 一抢焦点面板就没了）
#[tauri::command]
pub async fn quick_set_pinned(app: AppHandle, pinned: bool) -> Result<()> {
    log::debug!(target: "panel", "快捷面板钉住状态 → {pinned}");
    crate::tray::set_quick_pinned(&app, pinned);
    Ok(())
}

/// 主窗口获得焦点 → 清除"未读警告"标记（降级链路的收尾）
#[tauri::command]
pub async fn mark_notifications_read(app: AppHandle) -> Result<()> {
    if let Some(s) = effective_state(&app) {
        s.clear_warn_unread();
    }
    crate::tray::refresh(&app);
    Ok(())
}

// ---------------------------------------------------------------------------
// 首次运行（§15.1：不弹向导，也不抓「原始设置」）
// ---------------------------------------------------------------------------

/// 首次运行收尾：只把「已完成引导」标记写回配置。
///
/// 不再把当前系统状态抓成一个方案（旧的「保存一份原始设置」引导已移除）：
/// 默认基线就是**不写 hosts、不开系统代理、不改 DNS**，要什么由用户主动开启。
#[tauri::command]
pub async fn first_run_finish(
    app: AppHandle,
    state: State<'_, SharedState>,
) -> Result<()> {
    let s: Arc<_> = state.inner().clone();
    crate::cmd::blocking(move || {
        s.assert_writable()?;
        s.with_cfg_mut(|c| {
            c.initialized = true;
            Ok(())
        })?;
        s.save_cfg()?;
        log::info!(
            target: "firstrun",
            "首次运行已完成（默认基线：无 hosts / 无代理 / 无自定义 DNS）"
        );
        Ok(())
    })
    .await?;
    crate::cmd::after_change(&app);
    Ok(())
}

// ---------------------------------------------------------------------------
// 静默提权通道（Windows：计划任务版；安全边界见 elevation_task.rs 的模块说明）
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug)]
pub struct SilentElevationStatus {
    /// 本平台是否支持静默提权（两个平台都支持，只是实现不同）
    pub supported: bool,
    /// 是否处于启用状态（Windows = 配置开关；macOS = 装好助手即启用）
    pub enabled: bool,
    /// 通道是否已装好（Windows = 计划任务指向当前 exe；macOS = LaunchDaemon plist 存在）
    pub installed: bool,
    /// 界面上这条通道叫什么（「静默提权通道」/「免密提权助手」）
    pub noun: String,
    /// 状态说明（直接给界面显示，平台差异由后端收口，前端不判断平台）
    pub detail: String,
}

fn silent_status() -> SilentElevationStatus {
    let installed = crate::elevation_task::is_installed();
    let enabled = crate::elevation_task::enabled();
    SilentElevationStatus {
        supported: crate::elevation_task::supported(),
        enabled,
        installed,
        noun: crate::elevation_task::noun().to_string(),
        detail: crate::elevation_task::detail(installed, enabled),
    }
}

#[tauri::command]
pub async fn silent_elevation_status() -> Result<SilentElevationStatus> {
    crate::cmd::blocking(|| Ok(silent_status())).await
}

#[tauri::command]
pub async fn silent_elevation_install(
    app: AppHandle,
    state: State<'_, SharedState>,
) -> Result<SilentElevationStatus> {
    let s: Arc<_> = state.inner().clone();
    crate::cmd::blocking(move || {
        s.assert_writable()?;
        // Windows：注册一个 `RL HIGHEST` 的计划任务本身就需要管理员 → 交给提权子进程
        //          （若通道已经装好，这一步会复用静默路径，不会再弹 UAC）
        #[cfg(target_os = "windows")]
        crate::elevation::run_elevated(
            "silent_install",
            serde_json::json!({}),
            crate::elevation::TASK_TIMEOUT,
        )?;
        // macOS：安装常驻助手自带 osascript 提权（一次性输密码），不需要再套一层
        #[cfg(target_os = "macos")]
        crate::elevation_task::install()?;
        s.with_cfg_mut(|c| {
            c.settings.silent_elevation = true;
            Ok(())
        })?;
        s.save_cfg()?;
        Ok(())
    })
    .await?;
    // 安装发生在子进程 / 外部命令里，父进程侧把缓存与开关同步过来
    crate::elevation_task::invalidate_cache();
    crate::elevation_task::set_enabled(true);
    let st = crate::cmd::blocking(|| Ok(silent_status())).await?;
    crate::cmd::after_change(&app);
    Ok(st)
}

#[tauri::command]
pub async fn silent_elevation_uninstall(
    app: AppHandle,
    state: State<'_, SharedState>,
) -> Result<SilentElevationStatus> {
    let s: Arc<_> = state.inner().clone();
    crate::cmd::blocking(move || {
        s.assert_writable()?;
        #[cfg(target_os = "windows")]
        crate::elevation::run_elevated(
            "silent_uninstall",
            serde_json::json!({}),
            crate::elevation::TASK_TIMEOUT,
        )?;
        #[cfg(target_os = "macos")]
        crate::elevation_task::uninstall()?;
        s.with_cfg_mut(|c| {
            c.settings.silent_elevation = false;
            Ok(())
        })?;
        s.save_cfg()?;
        Ok(())
    })
    .await?;
    crate::elevation_task::invalidate_cache();
    crate::elevation_task::set_enabled(false);
    let st = crate::cmd::blocking(|| Ok(silent_status())).await?;
    crate::cmd::after_change(&app);
    Ok(st)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 界面上「按键录制」产出的组合串，全部必须能被后端收敛成规范形态。
    /// 这条测试是前端录制控件与后端注册器之间的契约：录制控件的输出 = `Ctrl+<KeyboardEvent.code>`，
    /// 后端不许出现「界面能录、后端不认」的组合（那是老的「检测可用性说不行、应用却成功」的根源）。
    #[test]
    fn capture_output_round_trips() {
        // 字母 / 数字
        assert_eq!(normalize_binding("Ctrl+KeyA").unwrap(), "Ctrl+A");
        assert_eq!(normalize_binding("Ctrl+Shift+Digit1").unwrap(), "Ctrl+Shift+1");
        // 功能键
        assert_eq!(normalize_binding("Ctrl+F11").unwrap(), "Ctrl+F11");
        assert_eq!(normalize_binding("Alt+F24").unwrap(), "Alt+F24");
        // 方向键 / 命名键
        assert_eq!(normalize_binding("Super+ArrowUp").unwrap(), "Super+Up");
        assert_eq!(normalize_binding("Ctrl+Escape").unwrap(), "Ctrl+Esc");
        assert_eq!(normalize_binding("Ctrl+Space").unwrap(), "Ctrl+Space");
        assert_eq!(normalize_binding("Alt+Backquote").unwrap(), "Alt+Backquote");
        assert_eq!(normalize_binding("Ctrl+PageDown").unwrap(), "Ctrl+PageDown");
        // 主键盘标点（旧代码全部不认 —— 界面能录、后端报“无法解析”）
        for (code, want) in [
            ("Minus", "Ctrl+Minus"),
            ("Equal", "Ctrl+Equal"),
            ("BracketLeft", "Ctrl+BracketLeft"),
            ("BracketRight", "Ctrl+BracketRight"),
            ("Backslash", "Ctrl+Backslash"),
            ("Semicolon", "Ctrl+Semicolon"),
            ("Quote", "Ctrl+Quote"),
            ("Comma", "Ctrl+Comma"),
            ("Period", "Ctrl+Period"),
            ("Slash", "Ctrl+Slash"),
            ("Backquote", "Ctrl+Backquote"),
        ] {
            assert_eq!(normalize_binding(&format!("Ctrl+{code}")).unwrap(), want);
        }
        // 小键盘
        assert_eq!(normalize_binding("Ctrl+Numpad5").unwrap(), "Ctrl+Numpad5");
        assert_eq!(normalize_binding("Ctrl+NumpadAdd").unwrap(), "Ctrl+NumpadAdd");
        assert_eq!(normalize_binding("Ctrl+NumpadEnter").unwrap(), "Ctrl+NumpadEnter");
    }

    /// 同一个热键的各种写法必须收敛到同一个串 —— 改绑判定靠字符串比较，
    /// 收敛不一致就会出现「明明换了一个键，系统里还留着旧的」。
    #[test]
    fn aliases_collapse_to_one_form() {
        for raw in [
            "Ctrl+Alt+S",
            "ctrl+alt+s",
            "Control+Option+KeyS",
            "CMDORCTRL+ALT+S",
        ] {
            assert_eq!(normalize_binding(raw).unwrap(), "Ctrl+Alt+S", "raw={raw}");
        }
        assert_eq!(normalize_binding("win+shift+up").unwrap(), "Shift+Super+Up");
        assert_eq!(normalize_binding("Ctrl+Ctrl+S").unwrap(), "Ctrl+S");
        // 修饰键固定顺序，与用户输入顺序无关
        assert_eq!(normalize_binding("Alt+Shift+Ctrl+D").unwrap(), "Ctrl+Alt+Shift+D");
    }

    /// 非法组合一律 E5004，且不许被当成合法串写进配置。
    #[test]
    fn invalid_bindings_are_rejected_with_e5004() {
        for raw in [
            "",            // 空
            "S",           // 没有修饰键：会把正常打字全抢走
            "F11",         // 同上
            "Ctrl+",       // 只有修饰键
            "Ctrl+Alt",    // 只有修饰键
            "A+B",         // 两个主键
            "Ctrl+S+Alt",  // 修饰键写在主键之后（插件解析会失败）
            "Ctrl+NotAKey",// 不认识的键
            "Ctrl+Alt+",   // 尾部悬空
        ] {
            let err = normalize_binding(raw).unwrap_err();
            assert_eq!(err.code, E5004, "raw={raw:?}");
        }
    }

    /// VK 值必须与 Win32 一致；解析错了会注册不成功或注册成别的键。
    #[test]
    fn virtual_keys_match_win32() {
        assert_eq!(parse_binding("Ctrl+A").unwrap().vk, 0x41);
        assert_eq!(parse_binding("Ctrl+1").unwrap().vk, 0x31);
        assert_eq!(parse_binding("Ctrl+F11").unwrap().vk, 0x7A);
        assert_eq!(parse_binding("Ctrl+Numpad5").unwrap().vk, 0x65);
        assert_eq!(parse_binding("Ctrl+Up").unwrap().vk, 0x26);
        // 字母键与 code 名指向同一个 VK
        assert_eq!(
            parse_binding("Ctrl+KeyS").unwrap().vk,
            parse_binding("ctrl+s").unwrap().vk
        );
        // 修饰键掩码
        let spec = parse_binding("Ctrl+Alt+Shift+Super+S").unwrap();
        assert_eq!(
            spec.mods,
            MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_WIN,
            "四个修饰键都要进掩码"
        );
    }
}

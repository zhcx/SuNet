//! 托盘常驻（设计方案 §5.1 / §4.5.5 / §5.4）
//!
//! 三个要点：
//!   1. 图标必须 `include_bytes!` 内嵌 —— 前端 `TrayIcon.new()` 只接受绝对路径，
//!      打包后路径不存在会导致图标透明
//!   2. 托盘图标必须做深浅两版 —— 单色图形在任务栏上必然有一态是隐形的
//!   3. `CheckMenuItem` 的勾选由代码在每次状态变更后设置，不显示配置文件里的值
//!      菜单弹出前重新读真实状态，保证"菜单说开着、实际已经关了"不可能出现

use crate::error::AppError;
use crate::os::system_proxy::{self, ProxyState};
use crate::state::AppState;
use serde::Serialize;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewWindow, Wry};
#[cfg(target_os = "windows")]
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
#[cfg(target_os = "windows")]
use winreg::RegKey;

/// 供 UI 展示的聚合状态（§5.4.1）
#[derive(Serialize, Clone, Debug, Default)]
pub struct TrayStatus {
    pub profile_name: Option<String>,
    pub hosts_count: usize,
    pub proxy: String,
    pub dns_v4: String,
    pub dns_v6: String,
    pub dirty: bool,
    pub warn_unread: bool,
    pub autostart_enabled: bool,
    /// 手动代理是否真的在生效（PAC 存在时为 false）
    pub proxy_on: bool,
}

pub struct TrayHandles {
    pub status: MenuItem<Wry>,
    pub proxy: CheckMenuItem<Wry>,
    pub autostart: CheckMenuItem<Wry>,
}

/// 托盘/菜单栏背景主题：true = 浅色背景（§4.5.5）
///
/// Windows：`AppsUseLightTheme`（0/不存在 → 深色）
#[cfg(target_os = "windows")]
pub fn taskbar_is_light() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) = hkcu.open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
        KEY_READ,
    ) else {
        return false;
    };
    key.get_value::<u32, _>("AppsUseLightTheme").unwrap_or(0) == 1
}

/// macOS：`defaults read -g AppleInterfaceStyle` 在深色模式下输出 `Dark`，
/// 浅色模式该键不存在（命令报错）→ 读不到就当作浅色。
#[cfg(target_os = "macos")]
pub fn taskbar_is_light() -> bool {
    let out = std::process::Command::new("/usr/bin/defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
            !text.eq_ignore_ascii_case("dark")
        }
        // 键不存在 = 浅色外观
        _ => true,
    }
}

fn tray_image(light_taskbar: bool) -> tauri::Result<Image<'static>> {
    // §4.5.5：两版图标，运行时按主题选
    let bytes: &'static [u8] = if light_taskbar {
        include_bytes!("../icons/tray-light-32.png")
    } else {
        include_bytes!("../icons/tray-32.png")
    };
    Image::from_bytes(bytes)
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "status", "当前：未启用任何方案", false, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let panel = MenuItem::with_id(app, "panel", "快捷面板", true, None::<&str>)?;
    let profile = MenuItem::with_id(app, "profile", "切换方案…", true, None::<&str>)?;
    let proxy = CheckMenuItem::with_id(app, "proxy", "系统代理", true, false, None::<&str>)?;
    let clearp = MenuItem::with_id(app, "clear_proxy", "清除代理设置", true, None::<&str>)?;
    let dns = MenuItem::with_id(app, "dns", "切换 DNS…", true, None::<&str>)?;
    let hosts = MenuItem::with_id(app, "hosts", "编辑 hosts…", true, None::<&str>)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let autostart = CheckMenuItem::with_id(app, "autostart", "开机自启动", true, false, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "打开主窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出 SuNet", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &status, &sep1, &panel, &profile, &proxy, &clearp, &dns, &hosts, &sep2, &autostart,
            &open, &quit,
        ],
    )?;

    app.manage(TrayHandles {
        status,
        proxy,
        autostart,
    });

    let light = taskbar_is_light();
    TrayIconBuilder::with_id("main-tray")
        .icon(tray_image(light)?)
        .tooltip("SuNet — 速网")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            if let Err(e) = on_menu(app, event.id().as_ref()) {
                log::warn!(target: "tray", "托盘菜单处理失败：{e}");
            }
        })
        .on_tray_icon_event(|tray, event| match event {
            // 左键单击 = 快捷面板（贴着托盘图标弹），这是日常 90% 操作的入口
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                position,
                ..
            } => {
                toggle_quick(tray.app_handle(), Some(position));
            }
            // 左键双击 = 直接开主窗口（单击已把面板开/关过一次，这里顺手收起）
            TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => {
                let app = tray.app_handle();
                hide_quick(app);
                show_window(app, None);
            }
            _ => {}
        })
        .build(app)?;

    Ok(())
}

fn on_menu(app: &AppHandle, id: &str) -> Result<(), AppError> {
    match id {
        "status" => {}
        // 从右键菜单打开时没有点击坐标，用当前光标位置定位
        "panel" => toggle_quick(app, None),
        "profile" | "dns" | "hosts" => {
            let tab = match id {
                "profile" => "profiles",
                "dns" => "dns",
                _ => "hosts",
            };
            show_window(app, Some(tab));
        }
        "proxy" => tray_toggle_proxy(app)?,
        "clear_proxy" => crate::cmd::proxy::tray_clear_proxy(app)?,
        "autostart" => {
            let enable = app
                .try_state::<AppState>()
                .and_then(|s| s.cfg_clone().ok())
                .map(|c| !c.settings.autostart.enabled)
                .unwrap_or(true);
            if let Err(e) = crate::cmd::system::set_autostart(app, enable) {
                crate::notify::send(
                    app,
                    crate::notify::Level::Error,
                    "开机自启动设置失败",
                    &e.to_string(),
                    vec![crate::notify::action("去设置", "open:settings")],
                    true,
                );
            }
        }
        "open" => show_window(app, None),
        "quit" => {
            crate::quit_app(app);
        }
        _ => {}
    }
    refresh(app);
    Ok(())
}

/// 托盘里的代理开关：没有可用配置时明确提示，而不是静默失败
fn tray_toggle_proxy(app: &AppHandle) -> Result<(), AppError> {
    let Some(state) = app.try_state::<AppState>() else {
        return Ok(());
    };
    let cfg = state.cfg_clone()?;
    let current = system_proxy::read().unwrap_or_default();
    if current.enable && !current.pac_present {
        let report = crate::apply::apply(
            &state,
            &crate::apply::ApplyTargets {
                proxy: Some(crate::apply::ProxyTarget {
                    enable: false,
                    ..Default::default()
                }),
                strict_verify: false,
                ..Default::default()
            },
            "托盘关闭代理",
        )?;
        crate::notify::send(
            app,
            crate::notify::Level::Info,
            "系统代理已关闭",
            &report.message,
            vec![],
            false,
        );
        return Ok(());
    }
    // 开启：用当前方案里的代理配置
    let proxy = cfg
        .active_profile()
        .map(|p| p.proxy.clone())
        .filter(|p| !p.host.trim().is_empty() && p.port > 0);
    match proxy {
        Some(p) => {
            let report = crate::apply::apply(
                &state,
                &crate::apply::ApplyTargets {
                    proxy: Some(crate::apply::ProxyTarget {
                        enable: true,
                        host: p.host,
                        port: p.port,
                        bypass: p.bypass,
                        probe: cfg.settings.proxy_probe_before_enable,
                    }),
                    strict_verify: false,
                    ..Default::default()
                },
                "托盘开启代理",
            )?;
            crate::notify::send(
                app,
                if report.ok {
                    crate::notify::Level::Info
                } else {
                    crate::notify::Level::Error
                },
                "系统代理已开启",
                &report.message,
                vec![],
                false,
            );
        }
        None => {
            show_window(app, Some("proxy"));
            crate::notify::send(
                app,
                crate::notify::Level::Warn,
                "尚未配置代理地址",
                "请先在「代理」页填写地址与端口，再从这里一键开关",
                vec![crate::notify::action("去代理页", "open:proxy")],
                false,
            );
        }
    }
    Ok(())
}

pub fn show_window(app: &AppHandle, tab: Option<&str>) {
    // 打开主窗口时收起快捷面板，避免两个窗口叠在一起
    hide_quick(app);
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
    if let Some(tab) = tab {
        use tauri::Emitter;
        let _ = app.emit("sunet://navigate", tab);
    }
    if let Some(state) = app.try_state::<AppState>() {
        state.clear_warn_unread();
    }
}

// ---------------------------------------------------------------------------
// 快捷面板（托盘左键弹出的小窗，label = quick）
// ---------------------------------------------------------------------------

/// 面板是否处于「操作进行中」：进行中时失焦不收起
pub fn is_quick_pinned(app: &AppHandle) -> bool {
    app.try_state::<AppState>()
        .and_then(|s| s.runtime.lock().ok().map(|r| r.quick_pinned))
        .unwrap_or(false)
}

pub fn set_quick_pinned(app: &AppHandle, pinned: bool) {
    if let Some(s) = app.try_state::<AppState>() {
        if let Ok(mut rt) = s.runtime.lock() {
            rt.quick_pinned = pinned;
        }
    }
}

pub fn hide_quick(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("quick") {
        let _ = win.hide();
    }
    set_quick_pinned(app, false);
}

/// 面板是否真正获得过焦点（用于过滤"刚弹出就失焦"的竞态事件）
pub fn quick_focus_seen(app: &AppHandle) -> bool {
    app.try_state::<AppState>()
        .and_then(|s| s.runtime.lock().ok().map(|r| r.quick_focus_seen))
        .unwrap_or(false)
}

pub fn set_quick_focus_seen(app: &AppHandle, seen: bool) {
    if let Some(s) = app.try_state::<AppState>() {
        if let Ok(mut rt) = s.runtime.lock() {
            rt.quick_focus_seen = seen;
        }
    }
}

/// 开/关面板。at = 触发点（托盘点击坐标），None 时用当前光标位置
pub fn toggle_quick(app: &AppHandle, at: Option<PhysicalPosition<f64>>) {
    let Some(win) = app.get_webview_window("quick") else {
        log::warn!(target: "tray", "快捷面板窗口不存在（配置里缺少 label=quick 的窗口）");
        return;
    };
    if win.is_visible().unwrap_or(false) {
        hide_quick(app);
        return;
    }
    let point = at.or_else(|| app.cursor_position().ok());
    match point {
        Some(p) => place_quick(app, &win, p),
        None => {
            let _ = win.center();
        }
    }
    // 每次重新显示都重置"曾获得焦点"标记，否则上一轮的状态会让本次弹出被误收起
    set_quick_focus_seen(app, false);
    let _ = win.show();
    let _ = win.set_focus();
    // 面板里的状态也需要是最新的
    if let Some(state) = app.try_state::<AppState>() {
        state.clear_warn_unread();
    }
    refresh(app);
}

/// 从主窗口内唤出面板：没有托盘点击坐标，用主显示器工作区的右下角当锚点
/// （托盘区通常就在那里，观感与从左键点击弹出基本一致）
pub fn show_quick_anchored(app: &AppHandle) {
    let Some(win) = app.get_webview_window("quick") else {
        return;
    };
    let anchor = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| {
            let wa = m.work_area();
            PhysicalPosition::new(
                (wa.position.x + wa.size.width as i32 - 8) as f64,
                (wa.position.y + wa.size.height as i32 - 8) as f64,
            )
        });
    match anchor {
        Some(p) => {
            log::debug!(target: "tray", "快捷面板唤出（锚点 {:.0},{:.0}）", p.x, p.y);
            place_quick(app, &win, p)
        }
        None => {
            let _ = win.center();
        }
    }
    set_quick_focus_seen(app, false);
    let _ = win.show();
    let _ = win.set_focus();
    // 面板是常驻窗口：失焦只隐藏、不重建，WebView 里的状态不会自己变新。
    // 每次唤出都让前端重新拉一次状态，避免面板显示上一个方案 / 旧代理状态。
    crate::cmd::emit_refresh(app);
    refresh(app);
}

/// 贴着托盘图标摆放：按点击点离哪条屏幕边最近，判断任务栏在上/下/左/右，再决定弹出方向
fn place_quick(app: &AppHandle, win: &WebviewWindow, click: PhysicalPosition<f64>) {
    let size = win
        .outer_size()
        .unwrap_or_else(|_| PhysicalSize::new(372, 528));
    let Ok(Some(mon)) = app.monitor_from_point(click.x, click.y) else {
        let _ = win.center();
        return;
    };
    let wa = mon.work_area();
    let margin = (8.0 * mon.scale_factor().max(1.0)).round() as i32;
    let left = wa.position.x;
    let top = wa.position.y;
    let right = left + wa.size.width as i32;
    let bottom = top + wa.size.height as i32;
    let cx = click.x.round() as i32;
    let cy = click.y.round() as i32;
    let w = size.width as i32;
    let h = size.height as i32;

    // 点击点位于任务栏上，比较它到四条边的距离即可判断任务栏位置
    let dists = [bottom - cy, cy - top, cx - left, right - cx];
    let edge = dists
        .iter()
        .enumerate()
        .min_by_key(|(_, v)| **v)
        .map(|(i, _)| i)
        .unwrap_or(0);

    let raw = match edge {
        0 => (cx - w / 2, cy - h - margin), // 任务栏在底部 → 向上弹
        1 => (cx - w / 2, cy + margin),     // 顶部 → 向下弹
        2 => (cx + margin, cy - h / 2),     // 左侧竖排 → 向右弹
        _ => (cx - w - margin, cy - h / 2), // 右侧竖排 → 向左弹
    };
    let x = raw.0.clamp(left + margin, (right - w - margin).max(left + margin));
    let y = raw.1.clamp(top + margin, (bottom - h - margin).max(top + margin));
    let _ = win.set_position(PhysicalPosition::new(x, y));
}

// ---------------------------------------------------------------------------
// 状态聚合与刷新
// ---------------------------------------------------------------------------

pub fn status(app: &AppHandle) -> TrayStatus {
    let hosts_count = crate::os::hosts_file::stats()
        .map(|s| s.enabled_entries)
        .unwrap_or(0);
    status_with_hosts(app, hosts_count)
}

/// 与 status 相同，但 hosts 条目数由调用方提供。
/// get_app_state 已经为 hosts_pending 读过一次文件，避免同样的内容读两遍。
pub fn status_with_hosts(app: &AppHandle, hosts_enabled_entries: usize) -> TrayStatus {
    status_with(app, hosts_enabled_entries, system_proxy::read().ok())
}

/// 与 `status_with_hosts` 相同，但代理状态也由调用方提供：
/// `get_app_state` 本来就要返回 `proxy` 字段，读一次即可，没必要再读第二遍。
pub fn status_with(
    app: &AppHandle,
    hosts_enabled_entries: usize,
    proxy: Option<ProxyState>,
) -> TrayStatus {
    let mut out = TrayStatus::default();
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(cfg) = state.cfg_clone() {
            out.profile_name = cfg.active_profile().map(|p| p.name.clone());
            out.autostart_enabled = cfg.settings.autostart.enabled;
        }
        if let Ok(rt) = state.runtime.lock() {
            out.dirty = rt.dirty;
            out.warn_unread = rt.warn_unread;
            if let Some((v4, v6)) = rt.dns_summary.clone() {
                out.dns_v4 = v4;
                out.dns_v6 = v6;
            }
        }
    }
    if out.dns_v4.is_empty() {
        out.dns_v4 = "DNS 自动(v4)".into();
        out.dns_v6 = "自动(v6)".into();
    }
    out.hosts_count = hosts_enabled_entries;
    match proxy {
        Some(p) => {
            out.proxy_on = p.is_on();
            out.proxy = if p.pac_present {
                "代理 PAC".into()
            } else if p.enable {
                match system_proxy::parse_server(&p.server) {
                    Some((_, port)) => format!("代理 已开启(***:{port})"),
                    None => "代理 已开启".into(),
                }
            } else {
                "代理 关闭".into()
            };
        }
        None => out.proxy = "代理 未知".into(),
    }
    out
}

/// 真实 DNS 状态 → tooltip 摘要（分族显示，§9：状态栏的 DNS 段必须分族）
pub fn dns_summary_of(state: &crate::os::dns_client::DnsState) -> (String, String) {
    let v4 = if state.v4.is_dhcp {
        "DNS 自动(v4)".to_string()
    } else {
        format!("DNS 手动(v4) {}", state.v4.servers.join("/"))
    };
    let v6 = if state.v6.is_dhcp {
        "自动(v6)".to_string()
    } else {
        format!("手动(v6) {}", state.v6.servers.join("/"))
    };
    (v4, v6)
}

/// 写入运行时 DNS 摘要（真实状态缓存，供托盘 tooltip 显示）
pub fn remember_dns(app: &AppHandle, alias: &str) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if let Ok(d) = crate::os::dns_client::get(alias) {
        let summary = dns_summary_of(&d);
        if let Ok(mut rt) = state.runtime.lock() {
            rt.dns_summary = Some(summary);
        }
    }
}

/// 生效状态标题（与主界面「当前生效」同一口径，见 src/tabs/profiles.ts）：
///   1. 有活动方案 → 方案名；
///   2. 没有活动方案，但三层里已有实际生效的（代理开着 / 有托管 hosts 条目 /
///      DNS 手动）→ 「自定义设置」——这些是用户手动改过的，不能再报「未启用方案」；
///   3. 都没有 → 回落内置直连（它本身就是真实生效状态）。
fn effective_headline(s: &TrayStatus) -> String {
    if let Some(name) = s.profile_name.as_ref() {
        return name.clone();
    }
    let dns_manual = !s.dns_v4.contains("自动") || !s.dns_v6.contains("自动");
    if s.proxy_on || s.hosts_count > 0 || dns_manual {
        "自定义设置".to_string()
    } else {
        "默认 · 直连".to_string()
    }
}

/// Tooltip 文案（§5.4.2）：单行、`·` 分隔、固定字段顺序、≤127 字符，
/// 超长时截断 DNS 部分而不截断代理 —— 代理是唯一会"全局断网"的层
pub fn tooltip_text(s: &TrayStatus) -> String {
    let mut parts = Vec::new();
    parts.push(effective_headline(s));
    parts.push(s.proxy.clone());
    parts.push(s.dns_v4.clone());
    parts.push(s.dns_v6.clone());
    if s.hosts_count > 0 {
        parts.push(format!("hosts {} 条", s.hosts_count));
    }
    let mut text = format!("SuNet · {}", parts.join(" · "));
    if text.chars().count() > 120 {
        // 截断 DNS 之后的部分，保留代理状态
        text = format!(
            "SuNet · {} · {}",
            parts[0].clone(),
            s.proxy.clone()
        );
    }
    if s.warn_unread {
        text = format!("⚠ {text}");
    }
    text
}

/// 状态变化后重算 tooltip + 重设菜单勾选（勾选必须反映真实状态）
///
/// 托盘/菜单的写操作统一投递到主线程执行：这些调用可能来自命令的 blocking 线程
/// 或后台维护线程，托盘实现（tray-icon / muda）对线程有要求。
pub fn refresh(app: &AppHandle) {
    let h = app.clone();
    let inner = h.clone();
    // 事件循环已退出时忽略（退出过程中会走到这里）
    let _ = h.run_on_main_thread(move || refresh_now(&inner));
}

fn refresh_now(app: &AppHandle) {
    let s = status(app);
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_tooltip(Some(tooltip_text(&s)));
    }
    if let Some(handles) = app.try_state::<TrayHandles>() {
        let _ = handles.status.set_text(format!("当前：{}", effective_headline(&s)));
        let _ = handles.proxy.set_checked(s.proxy_on);
        let _ = handles.autostart.set_checked(s.autostart_enabled);
    }
}

/// 主题切换后重建图标（§4.5.5：任务栏深浅两态，单色图形必有一态是隐形的）
pub fn refresh_icon_theme(app: &AppHandle) {
    let h = app.clone();
    let inner = h.clone();
    let _ = h.run_on_main_thread(move || {
        if let Some(tray) = inner.tray_by_id("main-tray") {
            if let Ok(img) = tray_image(taskbar_is_light()) {
                let _ = tray.set_icon(Some(img));
            }
        }
    });
}

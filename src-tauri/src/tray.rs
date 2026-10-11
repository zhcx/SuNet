//! 托盘常驻（设计方案 §5.1 / §4.5.5 / §5.4）
//!
//! 三个要点：
//!   1. 图标必须 `include_bytes!` 内嵌 —— 前端 `TrayIcon.new()` 只接受绝对路径，
//!      打包后路径不存在会导致图标透明
//!   2. 图标配色必须跟着背景走 —— Windows 按任务栏明暗在两版图标间切，
//!      macOS 用「模板图」交给系统上色（见 `tray_icon_image`）
//!   3. `CheckMenuItem` 的勾选由代码在每次状态变更后设置，不显示配置文件里的值
//!      菜单弹出前重新读真实状态，保证"菜单说开着、实际已经关了"不可能出现

use crate::error::AppError;
use crate::os::system_proxy::{self, ProxyState};
use crate::state::AppState;
use serde::Serialize;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Monitor, PhysicalPosition, PhysicalSize, Rect, WebviewWindow, Wry};
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

/// 托盘背景主题：true = 浅色背景（§4.5.5）
///
/// 只有 Windows 需要它 —— macOS 的菜单栏图标是模板图，配色由系统负责
/// （见 [`tray_icon_image`]）。Windows：`AppsUseLightTheme`（0/不存在 → 深色）
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

/// 托盘图标。Windows：任务栏明暗无法自动适配，仍按主题在两版图标间选（§4.5.5）。
#[cfg(target_os = "windows")]
fn tray_icon_image() -> tauri::Result<Image<'static>> {
    let bytes: &'static [u8] = if taskbar_is_light() {
        include_bytes!("../icons/tray-light-32.png")
    } else {
        include_bytes!("../icons/tray-32.png")
    };
    Image::from_bytes(bytes)
}

/// macOS 菜单栏图标 = 纯黑 + alpha 的模板图（`icons/tray-template.png`，按 `icons/tray.svg`
/// 的几何生成）。
///
/// 模板图只提供形状，配色由系统负责：浅色菜单栏画成深色、深色菜单栏画成白色 ——
/// 两态都不会隐形，也就不必再去 fork `defaults` 猜当前主题（旧实现猜错就会得到
/// 「浅色菜单栏上挂一个奶油色图标」的结果）。中间那条用 66% alpha，上色后比
/// 上下两条浅一档，保住「三条横杠」的层次。
#[cfg(target_os = "macos")]
fn tray_icon_image() -> tauri::Result<Image<'static>> {
    Image::from_bytes(include_bytes!("../icons/tray-template.png"))
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

    TrayIconBuilder::with_id("main-tray")
        .icon(tray_icon_image()?)
        // macOS 的模板图必须显式声明，否则系统不会接管配色（Windows 上是空操作）
        .icon_as_template(cfg!(target_os = "macos"))
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
                rect,
                position,
                ..
            } => {
                // rect 是图标本身的外框（查不到时是空框，下游会退回光标点）
                toggle_quick(tray.app_handle(), Some((rect, position)));
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
        // 从右键菜单打开时没有点击事件 → 现查托盘图标外框，查不到才退回光标位置
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
    // 三个协议任一配好就能开（macOS 可以只配 HTTPS / SOCKS；Windows 只有 HTTP 这一项）
    let proxy = cfg
        .active_profile()
        .map(|p| p.proxy.clone())
        .filter(|p| p.any_endpoint());
    match proxy {
        Some(p) => {
            // 逐协议落点跟随方案（Windows 上这两个方法永远返回 None）；
            // 先算好再搬走 p 的字段：endpoint() 借的是 &p
            let (https, socks) = (p.https_endpoint(), p.socks_endpoint());
            let report = crate::apply::apply(
                &state,
                &crate::apply::ApplyTargets {
                    proxy: Some(crate::apply::ProxyTarget {
                        enable: true,
                        host: p.host,
                        port: p.port,
                        bypass: p.bypass,
                        probe: cfg.settings.proxy_probe_before_enable,
                        https,
                        socks,
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
        // macOS：先让 Dock 图标回来再显示窗口 —— UIElement → Foreground 是异步的，
        // 顺序反了窗口可能"显示在别的应用后面"、也拿不到焦点（Windows 上是空实现）
        crate::os::dock::show(app);
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

/// 开/关面板。`at` = 左键点击事件里的（托盘图标外框, 光标位置）；右键菜单唤出时是 None。
///
/// 定位优先用**图标外框**：旧实现拿光标点当中心，图标越靠屏幕右缘越明显 ——
/// 面板被 clamp 推到屏幕边上后，看起来就不在图标下方了。
pub fn toggle_quick(app: &AppHandle, at: Option<(Rect, PhysicalPosition<f64>)>) {
    let Some(win) = app.get_webview_window("quick") else {
        log::warn!(target: "tray", "快捷面板窗口不存在（配置里缺少 label=quick 的窗口）");
        return;
    };
    if win.is_visible().unwrap_or(false) {
        hide_quick(app);
        return;
    }
    let scale = win.scale_factor().unwrap_or(1.0);
    let anchor = at
        .as_ref()
        .and_then(|(r, _)| Anchor::from_rect(r, scale))
        .or_else(|| tray_anchor(app, scale))
        .or_else(|| at.map(|(_, p)| Anchor::from_point(p)))
        .or_else(|| app.cursor_position().ok().map(Anchor::from_point));
    match anchor {
        Some(a) => place_quick(app, &win, a),
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

/// 从主窗口内唤出面板（热键 / 主窗口按钮）：用托盘图标的外框当锚点；
/// 查不到才退回「主显示器工作区的一角」。
pub fn show_quick_anchored(app: &AppHandle) {
    let Some(win) = app.get_webview_window("quick") else {
        return;
    };
    let scale = win.scale_factor().unwrap_or(1.0);
    match tray_anchor(app, scale).or_else(|| corner_anchor(app)) {
        Some(a) => place_quick(app, &win, a),
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

/// 面板锚点：托盘图标在屏幕上的外框（物理像素）。
///
/// 拿不到图标外框时退化成"一个点"（光标位置），此时 `left == right`、`top == bottom`。
#[derive(Clone, Copy)]
struct Anchor {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl Anchor {
    /// tauri 的 [`Rect`] → 物理像素外框。空框（宽或高 ≤ 0）返回 None ——
    /// Windows 的 `Shell_NotifyIconGetRect` 查不到图标时会给出一个全 0 的框。
    fn from_rect(r: &Rect, scale: f64) -> Option<Self> {
        let pos = r.position.to_physical::<i32>(scale);
        let size = r.size.to_physical::<u32>(scale);
        let (w, h) = (size.width as i32, size.height as i32);
        if w <= 0 || h <= 0 {
            return None;
        }
        Some(Self {
            left: pos.x,
            top: pos.y,
            right: pos.x + w,
            bottom: pos.y + h,
        })
    }

    fn from_point(p: PhysicalPosition<f64>) -> Self {
        let x = p.x.round() as i32;
        let y = p.y.round() as i32;
        Self {
            left: x,
            top: y,
            right: x,
            bottom: y,
        }
    }

    fn center_x(&self) -> i32 {
        self.left + (self.right - self.left) / 2
    }

    fn center_y(&self) -> i32 {
        self.top + (self.bottom - self.top) / 2
    }
}

/// 现查一次托盘图标外框：右键菜单 / 热键唤出时没有点击事件，只能问托盘要
fn tray_anchor(app: &AppHandle, scale: f64) -> Option<Anchor> {
    let tray = app.tray_by_id("main-tray")?;
    let rect = tray.rect().ok().flatten()?;
    Anchor::from_rect(&rect, scale)
}

/// 兜底锚点：托盘区的一角。macOS 菜单栏在**右上角**、Windows 任务栏默认在**右下角**，
/// 工作区的角落因此按平台取（旧实现一律取右下角，macOS 上热键唤出的面板会跑到屏幕底部）。
fn corner_anchor(app: &AppHandle) -> Option<Anchor> {
    let mon = app.primary_monitor().ok().flatten()?;
    let wa = mon.work_area();
    let margin = (8.0 * mon.scale_factor().max(1.0)).round() as i32;
    let x = wa.position.x + wa.size.width as i32 - margin;
    let y = if cfg!(target_os = "macos") {
        wa.position.y + margin // 菜单栏在顶部 → 面板向下弹
    } else {
        wa.position.y + wa.size.height as i32 - margin
    };
    Some(Anchor::from_point(PhysicalPosition::new(x as f64, y as f64)))
}

/// 锚点落在哪块显示器上（物理像素口径）。
///
/// **不能用 `AppHandle::monitor_from_point`**：macOS 上它比的是「点」、我们给的是「物理像素」——
/// tao 0.37 的 `monitor::from_point` 直接拿入参去撞 `CGDisplayBounds`，而 `CGDisplayBounds`
/// 在 Retina 屏上仍是点坐标（本机实测 1512×982 点 / 3024×1964 像素）。scale = 2 时
/// 凡是落在屏幕右半边的锚点都会被判成「不在任何显示器上」，`place_quick` 于是兜底
/// `win.center()` —— 用户看到的就是「点托盘图标，面板弹在屏幕正中」
/// （2026-10-11 实测：图标在右上角，面板出现在屏幕中间）。
/// 这里自己按 `available_monitors` 的**物理外框**判包含关系，锚点 / 工作区 / 显示器三者单位一致。
///
/// 三档优先级：
///   ① 菜单栏带（显示器顶边 → 工作区顶边）：macOS 的托盘图标必在这一条里，
///      多屏时能唯一定位到「用户点的是哪块屏的菜单栏」；
///   ② 工作区：Windows 任务栏占边时锚点也落在工作区内；
///   ③ 显示器外框：工作区之外的那一圈（Dock / 任务栏覆盖区）。
/// 都不中才退回主显示器 —— 兜底宁可"贴错边"，也不要回到屏幕正中。
fn monitor_at(app: &AppHandle, x: i32, y: i32) -> Option<Monitor> {
    let monitors = app.available_monitors().unwrap_or_default();
    let screens: Vec<ScreenRect> = monitors.iter().map(ScreenRect::of).collect();
    if let Some(i) = pick_screen(&screens, x, y) {
        return monitors.get(i).cloned();
    }
    // 一个都没中：退回主显示器（macOS 上就是带菜单栏那块屏）。
    // 宁可"贴错边"，也不要回到 win.center() —— 面板弹在屏幕正中是明确的破窗体验。
    log::debug!(target: "tray", "快捷面板锚点 [{x},{y}] 不在任何显示器外框内 → 退回主显示器");
    app.primary_monitor().ok().flatten()
}

/// 一块显示器的物理几何（外框 + 工作区）。抽成纯数据是为了给 `pick_screen` 写单测 ——
/// 这里的坐标系错一次，表现就是"面板弹在屏幕正中"。
#[derive(Clone, Copy, Debug)]
struct ScreenRect {
    pos: (i32, i32),
    size: (u32, u32),
    work: (i32, i32, u32, u32),
}

impl ScreenRect {
    fn of(m: &Monitor) -> Self {
        let p = m.position();
        let s = m.size();
        let wa = m.work_area();
        Self {
            pos: (p.x, p.y),
            size: (s.width, s.height),
            work: (wa.position.x, wa.position.y, wa.size.width, wa.size.height),
        }
    }
}

/// 左上原点 + 右下开区间（与 tao 的 `monitor_from_point` 同语义）
fn inside_box(pos: (i32, i32), size: (u32, u32), x: i32, y: i32) -> bool {
    x >= pos.0 && x < pos.0 + size.0 as i32 && y >= pos.1 && y < pos.1 + size.1 as i32
}

/// 锚点归哪块屏（下标）。三档优先级见 [`monitor_at`]。
fn pick_screen(screens: &[ScreenRect], x: i32, y: i32) -> Option<usize> {
    let mut outer = None;
    for (i, s) in screens.iter().enumerate() {
        if !inside_box(s.pos, s.size, x, y) {
            continue;
        }
        if y < s.work.1 {
            return Some(i); // ① 菜单栏带
        }
        if inside_box((s.work.0, s.work.1), (s.work.2, s.work.3), x, y) {
            return Some(i); // ② 工作区内
        }
        outer = outer.or(Some(i)); // ③ 只在外框里（Dock / 任务栏那圈）
    }
    outer
}

/// 贴着托盘图标摆放：先按图标离哪条屏幕边最近，判断托盘在上/下/左/右，
/// 再决定弹出方向；水平方向优先"居中于图标"，被屏幕夹住时改成与图标同侧对齐。
fn place_quick(app: &AppHandle, win: &WebviewWindow, a: Anchor) {
    let size = win
        .outer_size()
        .unwrap_or_else(|_| PhysicalSize::new(372, 528));
    let (cx, cy) = (a.center_x(), a.center_y());
    let Some(mon) = monitor_at(app, cx, cy) else {
        let _ = win.center();
        return;
    };
    let wa = mon.work_area();
    let margin = (8.0 * mon.scale_factor().max(1.0)).round() as i32;
    let left = wa.position.x;
    let top = wa.position.y;
    let right = left + wa.size.width as i32;
    let bottom = top + wa.size.height as i32;
    let w = size.width as i32;
    let h = size.height as i32;

    // 图标就在托盘上，比较它到四条边的距离即可判断托盘位置（0 底 / 1 顶 / 2 左 / 3 右）
    let dists = [
        bottom - a.bottom,
        a.top - top,
        a.left - left,
        right - a.right,
    ];
    let edge = dists
        .iter()
        .enumerate()
        .min_by_key(|(_, v)| **v)
        .map(|(i, _)| i)
        .unwrap_or(0);

    let raw = match edge {
        0 => (a.center_x() - w / 2, a.top - h - margin), // 托盘在底 → 面板底边贴图标上沿
        1 => (a.center_x() - w / 2, a.bottom + margin),  // 顶 → 顶边贴图标下沿
        2 => (a.right + margin, a.center_y() - h / 2),   // 左 → 左边贴图标右侧
        _ => (a.left - w - margin, a.center_y() - h / 2), // 右 → 右边贴图标左侧
    };
    let lo = left + margin;
    let hi = (right - w - margin).max(lo);
    let x = if raw.0 < lo {
        a.left.min(hi).max(lo) // 靠左放不下 → 用图标左缘对齐
    } else if raw.0 > hi {
        (a.right - w).max(lo).min(hi) // 靠右放不下 → 用图标右缘对齐
    } else {
        raw.0
    };
    let y = raw.1.clamp(top + margin, (bottom - h - margin).max(top + margin));
    let _ = win.set_position(PhysicalPosition::new(x, y));
    log::debug!(
        target: "tray",
        "快捷面板定位：图标 [{},{},{},{}] 屏 [{},{} {}x{}] 工作区 [{left},{top},{right},{bottom}] 尺寸 {w}x{h} → ({x},{y})",
        a.left,
        a.top,
        a.right,
        a.bottom,
        mon.position().x,
        mon.position().y,
        mon.size().width,
        mon.size().height
    );
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

/// 主题切换后重建图标（§4.5.5：任务栏深浅两态，单色图形必有一态是隐形的）。
///
/// 只有 Windows 需要：macOS 的菜单栏图标是模板图，系统自己按明暗上色。
#[cfg(target_os = "windows")]
pub fn refresh_icon_theme(app: &AppHandle) {
    let h = app.clone();
    let inner = h.clone();
    let _ = h.run_on_main_thread(move || {
        if let Some(tray) = inner.tray_by_id("main-tray") {
            if let Ok(img) = tray_icon_image() {
                let _ = tray.set_icon(Some(img));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 本机 macOS 实测的屏（Retina，scale = 2，菜单栏 33 点、Dock 在底部 55 点）：
    /// 全部换算成物理像素 —— 外框 3024×1964，工作区 y 从 66 到 1854。
    /// 注意「点」口径下这块屏只有 1512×982。
    fn macos_retina() -> ScreenRect {
        ScreenRect {
            pos: (0, 0),
            size: (3024, 1964),
            work: (0, 66, 3024, 1788),
        }
    }

    /// 主屏左侧接一块 1× 副屏（全局物理坐标为负，不许 max(0, x) 这种自作聪明）
    fn macos_left_secondary() -> ScreenRect {
        ScreenRect {
            pos: (-1920, 0),
            size: (1920, 1080),
            work: (-1920, 25, 1920, 1055),
        }
    }

    /// 回归（2026-10-11 用户实测）：点托盘图标，面板弹在**屏幕正中**。
    /// 根因是 `monitor_from_point` 拿物理像素去比点坐标，Retina 上屏幕右半边的锚点
    /// 一律"不在任何显示器上" → 兜底 `win.center()`。这里把"物理像素必须能中"锁住。
    #[test]
    fn right_half_anchor_still_hits_on_retina() {
        let screen = macos_retina();
        assert_eq!(pick_screen(&[screen], 2900, 24), Some(0)); // 右上角托盘图标
        assert_eq!(pick_screen(&[screen], 2900, 1000), Some(0)); // 屏幕右半边
        assert_eq!(pick_screen(&[screen], 2900, 1900), Some(0)); // 右下角（Dock 覆盖区）
    }

    /// 三档优先级：菜单栏带 / 工作区 / 外框那圈，各自都要能中；屏外则必须落空（交给调用方兜底）
    #[test]
    fn menu_bar_work_area_and_outer_ring() {
        let screen = macos_retina();
        assert_eq!(pick_screen(&[screen], 100, 20), Some(0)); // 菜单栏带（y 66 以上）
        assert_eq!(pick_screen(&[screen], 100, 66), Some(0)); // 工作区首行（闭区间）
        assert_eq!(pick_screen(&[screen], 100, 1853), Some(0)); // 工作区末行
        assert_eq!(pick_screen(&[screen], 100, 1900), Some(0)); // Dock 那圈（外框内、工作区外）
        assert_eq!(pick_screen(&[screen], 100, 1964), None); // 屏幕下边缘之外（右下开区间）
        assert_eq!(pick_screen(&[screen], -1, 100), None); // 屏幕左侧之外
    }

    /// 多屏：托盘图标所在的菜单栏带能唯一定位到"用户点的那块屏"
    #[test]
    fn picks_the_screen_whose_menu_bar_was_clicked() {
        let screens = [macos_retina(), macos_left_secondary()];
        assert_eq!(pick_screen(&screens, 2900, 24), Some(0));
        assert_eq!(pick_screen(&screens, -1000, 12), Some(1));
    }

    /// Windows：任务栏在底部（工作区不含它），托盘图标落在"外框之内、工作区之外"那条里
    #[test]
    fn windows_taskbar_anchor_lands_in_the_outer_ring() {
        let screen = ScreenRect {
            pos: (0, 0),
            size: (2560, 1440),
            work: (0, 0, 2560, 1400),
        };
        assert_eq!(pick_screen(&[screen], 2400, 1420), Some(0)); // 任务栏里
        assert_eq!(pick_screen(&[screen], 2400, 700), Some(0)); // 工作区里
        assert_eq!(pick_screen(&[screen], 2570, 700), None); // 屏外
    }
}

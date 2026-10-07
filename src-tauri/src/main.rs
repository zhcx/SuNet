// 速网 SuNet · 网络配置切换器
//
// 核心安全边界（设计方案 §1.4.9，任何实现改动不得破坏）：
//   未安装为服务、无常驻提权进程、提权子进程不加载 WebView2
// 分流放在 main() 的第一行 —— 提权子进程不进 Tauri setup、不建托盘、干完就退。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod apply;
mod backup;
mod cmd;
mod config;
mod critsec;
mod elevation;
mod error;
mod ipc;
mod logging;
mod migrate;
mod notify;
mod os;
mod paths;
mod probe;
mod state;
mod subscribe;
mod task_runner;
mod tray;
mod util;

use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::ShortcutState;

fn main() {
    // ① 提权子进程分流（必须在最前面）
    if let Some(args) = task_runner::parse_task_arg() {
        std::process::exit(task_runner::run_and_exit(args));
    }
    run_gui();
}

pub fn quit_app(app: &AppHandle) {
    let Some(state) = app.try_state::<state::SharedState>() else {
        app.exit(0);
        return;
    };
    let cfg = state.cfg_clone().unwrap_or_default();
    let dirty = state.runtime.lock().map(|r| r.dirty).unwrap_or(false);
    let need_restore = match cfg.settings.on_exit.as_str() {
        "always_restore" => true,
        "restore_if_changed" => dirty,
        _ => false, // keep（默认）：开了代理就是想一直在用，退出应用不等于关代理
    };
    if need_restore {
        let s = state.inner().clone();
        match apply::restore_from_snapshot(&s, None) {
            Ok(r) => log::info!(target: "exit", "退出时还原：{}", r.message),
            Err(e) => log::warn!(target: "exit", "退出时还原失败：{e}"),
        }
    }
    state::clear_session_marker();
    log::info!(target: "exit", "SuNet 正常退出（on_exit={}）", cfg.settings.on_exit);
    app.exit(0);
}

fn run_gui() {
    let _ = paths::ensure_dirs();
    state::write_session_marker(false);

    let (cfg, read_only) = match config::load() {
        config::LoadOutcome::Ready(c) => (c, None),
        config::LoadOutcome::ReadOnly(c, e) => (c, Some(e)),
    };

    logging::init(
        &paths::logs_dir(),
        logging::parse_level(&cfg.settings.log_level),
        cfg.settings.log_redact,
        cfg.settings.log_keep_days,
    );
    log::info!(
        target: "boot",
        "SuNet {} 启动（提权={}，只读={}）",
        env!("CARGO_PKG_VERSION"),
        os::privilege::is_elevated(),
        read_only.is_some()
    );
    if let Some(err) = read_only.as_ref() {
        log::error!(target: "boot", "配置只读模式：{err}");
    }

    let crash = state::detect_crash_recovery();
    // --minimized（自启路径）优先；用户设置 start_minimized 仅在已完成首次运行后生效，
    // 免得第一次双击图标什么都没有发生
    let minimized =
        std::env::args().any(|a| a == "--minimized")
            || (cfg.settings.start_minimized && cfg.initialized);
    let restore_flag = std::env::args().any(|a| a == "--restore");
    // --panel：直接弹出快捷面板（可做成桌面快捷方式，跳过主窗口）
    let panel_only = std::env::args().any(|a| a == "--panel");

    let app_state = Arc::new(state::AppState::new(cfg, read_only));
    if let Some(c) = crash {
        if let Ok(mut rt) = app_state.runtime.lock() {
            rt.crash_recovery = Some(c);
        }
    }

    tauri::Builder::default()
        // 多实例防护：第二次启动把参数转发给已有实例并唤起窗口（§5.3）
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            log::info!(target: "boot", "检测到第二实例启动，参数：{argv:?}");
            tray::show_window(app, None);
            if argv.iter().any(|a| a == "--restore") {
                let a = app.clone();
                std::thread::spawn(move || {
                    let _ = cmd::profile::do_restore_on_launch(&a);
                });
            }
        }))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    // WM_HOTKEY 在按下与释放时各投递一次，必须过滤，
                    // 否则一次按键会执行两遍（§5.5.5）
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    // 热键语义（2026-10 调整）：不是清代理，而是
                    // 「默认直连 ↔ 上一个方案」来回切；通知反馈结果
                    cmd::profile::hotkey_toggle_direct(app);
                })
                .build(),
        )
        .manage(app_state)
        .setup(move |app| {
            let handle = app.handle().clone();
            tray::build(&handle)?;

            let st = handle.state::<state::SharedState>();
            let cfg = st.cfg_clone().unwrap_or_default();

            // 无边框窗口的第一帧修正（详见 os::winapi::force_frame_recalc）：
            // 系统在窗口刚创建时会按 WS_CAPTION 画一条真标题栏，必须主动触发
            // 一次框架重算，否则主窗口在首次移动/缩放前都会顶着系统标题栏。
            for label in ["main", "quick"] {
                if let Some(w) = handle.get_webview_window(label) {
                    if let Ok(h) = w.hwnd() {
                        os::winapi::force_frame_recalc(h.0);
                    }
                }
            }

            // 主窗口：--minimized（自启路径）时不显示，直接常驻托盘
            if let Some(win) = handle.get_webview_window("main") {
                if minimized || panel_only {
                    let _ = win.hide();
                } else {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }
            // --panel：冷启动直接给快捷面板（贴在屏幕右下角）
            if panel_only {
                tray::show_quick_anchored(&handle);
            }
            for label in ["main", "quick"] {
                if handle.get_webview_window(label).is_none() {
                    log::warn!(target: "boot", "配置里缺少窗口 {label}");
                }
            }
            // 启动后核对两个窗口是否真的落在了自己的页面上。
            // 这条日志很值钱：dev 模式下 webview 连的是外部 dev server，
            // 如果端口被别的项目占用，界面会加载成别人的页面而进程一切正常。
            {
                let h = handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(4));
                    for label in ["main", "quick"] {
                        if let Some(w) = h.get_webview_window(label) {
                            let url = w
                                .url()
                                .map(|u| u.to_string())
                                .unwrap_or_else(|e| format!("<{e}>"));
                            log::info!(
                                target: "boot",
                                "窗口 {label}：url={url} 可见={} 标题={}",
                                w.is_visible().unwrap_or(false),
                                w.title().unwrap_or_default()
                            );
                            // 记下窗口真实几何（物理像素）：贴边定位与截图核对都要用
                            if let (Ok(p), Ok(sz)) = (w.outer_position(), w.outer_size()) {
                                log::debug!(
                                    target: "boot",
                                    "窗口 {label} 几何：x={} y={} w={} h={}",
                                    p.x,
                                    p.y,
                                    sz.width,
                                    sz.height
                                );
                            }
                        }
                    }
                });
            }

            // 全局热键：注册失败必须如实报告，而不是静默失败（§5.5.3）
            let sc = cfg.settings.shortcuts.clear_proxy.clone();
            if sc.enabled {
                if let Err(e) = cmd::system::register_shortcut(&handle, &sc.binding) {
                    log::warn!(target: "hotkey", "启动时注册热键失败：{e}");
                    let h = handle.clone();
                    let binding = sc.binding.clone();
                    std::thread::spawn(move || {
                        // 等窗口就绪后再提示，避免登录瞬间抢焦点
                        std::thread::sleep(std::time::Duration::from_secs(3));
                        notify::send(
                            &h,
                            notify::Level::Warn,
                            "全局热键未能注册",
                            &format!("{binding}：{e}"),
                            vec![notify::action("去设置", "open:settings")],
                            false,
                        );
                    });
                }
            }

            // 后台维护线程：撤销窗口超时 + 托盘主题变化重绘图标
            {
                let h = handle.clone();
                std::thread::spawn(move || {
                    let mut last_light = tray::taskbar_is_light();
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        if let Some(s) = h.try_state::<state::SharedState>() {
                            apply::expire_clear_undo(&s);
                        }
                        let now_light = tray::taskbar_is_light();
                        if now_light != last_light {
                            last_light = now_light;
                            tray::refresh_icon_theme(&h);
                            tray::refresh(&h);
                        }
                        if h.get_webview_window("main").is_none() {
                            break;
                        }
                    }
                });
            }

            // 预热：读取当前网卡 DNS，写入托盘 tooltip 缓存（不阻塞启动）
            {
                let h = handle.clone();
                std::thread::spawn(move || {
                    if let Ok(list) = os::dns_client::list_interfaces() {
                        if let Some(first) = os::dns_client::pick_in_use(&list) {
                            tray::remember_dns(&h, &first.alias);
                        }
                    }
                    tray::refresh(&h);
                });
            }

            // 开机自动恢复上次方案（需要提权，所以放在登录后按需弹一次 UAC，§5.2）
            if restore_flag || cfg.settings.restore_on_launch {
                let h = handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    let _ = cmd::profile::do_restore_on_launch(&h);
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| match event {
            // 点关闭按钮 → 隐藏到托盘（不是退出）
            tauri::WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
                if window.label() == "quick" {
                    tray::set_quick_pinned(window.app_handle(), false);
                    return;
                }
                if let Some(s) = window.app_handle().try_state::<state::SharedState>() {
                    s.clear_warn_unread();
                }
            }
            tauri::WindowEvent::Focused(true) if window.label() == "quick" => {
                tray::set_quick_focus_seen(window.app_handle(), true);
            }
            // 快捷面板：点到别处就收起。两种例外都不收：
            //   1. 正在执行操作（UAC 授权框会抢焦点，收起来用户就看不到结果了）
            //   2. 自上次弹出以来还没真正获得过焦点（启动瞬间的失焦事件会把面板立刻收掉）
            tauri::WindowEvent::Focused(false) if window.label() == "quick" => {
                let app = window.app_handle();
                if !tray::is_quick_pinned(app) && tray::quick_focus_seen(app) {
                    let _ = window.hide();
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            // 状态
            cmd::system::get_app_state,
            cmd::system::settings_save,
            // hosts
            cmd::hosts::hosts_list,
            cmd::hosts::hosts_upsert,
            cmd::hosts::hosts_delete,
            cmd::hosts::hosts_clear,
            cmd::hosts::hosts_batch_block,
            cmd::hosts::hosts_preview,
            cmd::hosts::hosts_apply,
            cmd::hosts::hosts_import,
            cmd::hosts::hosts_resolve,
            // proxy
            cmd::proxy::proxy_get,
            cmd::proxy::proxy_set,
            cmd::proxy::proxy_clear,
            cmd::proxy::proxy_undo_clear,
            // dns
            cmd::dns::dns_interfaces,
            cmd::dns::dns_get,
            cmd::dns::dns_set,
            cmd::dns::dns_reset,
            cmd::dns::dns_flush,
            cmd::dns::dns_presets,
            cmd::dns::dns_doh_reference,
            cmd::dns::dns_custom_save,
            cmd::dns::dns_custom_delete,
            cmd::dns::dns_v6_ready,
            cmd::dns::dns_test,
            // profile / 事务 / 快照
            cmd::profile::profile_list,
            cmd::profile::profile_save,
            cmd::profile::profile_delete,
            cmd::profile::profile_apply,
            cmd::profile::rollback,
            cmd::profile::global_restore,
            cmd::profile::snapshots_list,
            cmd::profile::snapshots_diff,
            // 订阅
            cmd::profile::sources_list,
            cmd::profile::sources_save,
            cmd::profile::sources_delete,
            cmd::profile::sources_sync,
            cmd::profile::sources_sync_all,
            cmd::profile::sources_commit,
            cmd::profile::sources_discard,
            cmd::profile::sources_staged,
            // 系统
            cmd::system::autostart_set,
            cmd::system::shortcut_check,
            cmd::system::shortcut_set,
            cmd::system::shortcut_status,
            cmd::system::shortcut_capture,
            cmd::system::logs_recent,
            cmd::system::system_dirs,
            cmd::system::system_open_dir,
            cmd::system::logs_export,
            cmd::system::window_hide,
            cmd::system::window_minimize,
            cmd::system::window_show,
            cmd::system::quick_hide,
            cmd::system::quick_show,
            cmd::system::quick_fit,
            cmd::system::quick_open_main,
            cmd::system::quick_set_pinned,
            cmd::system::mark_notifications_read,
            cmd::system::first_run_report,
            cmd::system::first_run_finish,
        ])
        .run(tauri::generate_context!())
        .expect("SuNet 启动失败");
}

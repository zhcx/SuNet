//! Dock 图标（仅 macOS）
//!
//! SuNet 是「常驻菜单栏 + 按需弹主窗口」的形态：窗口关掉（= 藏到托盘）之后，
//! Dock 里不该再留一个点不出窗口的图标 —— 那时用户只在菜单栏看得到 SuNet。
//!
//! 做法是 tao 的 `AppHandle::set_dock_visibility`（底层 `TransformProcessType` 的
//! UIElement ↔ Foreground 转换，语义与 `LSUIElement` 一致），**不是** `bundle.macOS.dockIcon = false`：
//! 后者是编译期的 LSUIElement，会让窗口开着时也没有 Dock 图标、Cmd+Tab 里也找不到这个应用。
//! 这里让图标跟着主窗口的可见性走：窗口在 → 有图标（可 Cmd+Tab 回来），窗口藏了 → 没有图标。
//!
//! 两处时序（都是实测过的坑，改调用顺序前先读一眼）：
//!   1. **显示方向要在 `show()` 之前调用**：UIElement → Foreground 是异步的，
//!      转换还没落地就显示窗口，窗口可能"显示在别的应用后面"、拿不到焦点；
//!   2. **隐藏方向要在 `hide()` 之后调用**，而且 tao 会**丢弃**「刚 show 过不到 1 秒就 hide」的那次
//!      （macOS 的转换是异步的，来回切会在 Dock 里留下多个图标）。我们记住自己上次 show 的时间，
//!      撞上这个去抖窗口就 1.2 秒后补一次，并重新核对窗口确实还藏着 —— 期间用户又把窗口打开了，
//!      这次补同步就什么都不做。
//!
//! Windows 侧有同名同签名的空实现（`os/windows/dock.rs`）：任务栏按钮是窗口的属性，
//! `hide()` 之后按钮自然消失，没有"进程级图标"这回事。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

/// tao 的去抖窗口（`DOCK_SHOW_TIMEOUT = 1s`）：这段时间内的 hide 会被它丢掉
const DEBOUNCE: Duration = Duration::from_secs(1);

/// 上次 [`show`] 的时间（只用来判断本次 hide 会不会被 tao 丢掉）
static LAST_SHOW: Mutex<Option<Instant>> = Mutex::new(None);

/// 让 Dock 图标回来。**在显示主窗口之前**调用。
pub fn show(app: &AppHandle) {
    if let Ok(mut at) = LAST_SHOW.lock() {
        *at = Some(Instant::now());
    }
    set(app, true);
}

/// 藏掉 Dock 图标。**在隐藏主窗口之后**调用（含撞上 tao 去抖时的补同步）。
pub fn hide(app: &AppHandle) {
    let debounced = debounced_by_tao(LAST_SHOW.lock().ok().and_then(|at| *at), Instant::now());
    set(app, false);
    if !debounced {
        return;
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(DEBOUNCE + Duration::from_millis(200));
        // 按此刻的真实状态再判一次：这几百毫秒里用户可能又把窗口打开了
        if !main_visible(&handle) {
            set(&handle, false);
        }
    });
}

fn set(app: &AppHandle, visible: bool) {
    if let Err(e) = app.set_dock_visibility(visible) {
        log::warn!(target: "dock", "切换 Dock 图标（visible={visible}）失败：{e}");
    } else {
        log::debug!(target: "dock", "Dock 图标 → {}", if visible { "显示" } else { "隐藏" });
    }
}

/// 主窗口是否还在（**最小化也算在**：Dock 里留着它的缩略图是 macOS 的习惯，
/// 只有 `hide()` 到托盘才算"没有窗口"）
fn main_visible(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

/// 这次 hide 会不会被 tao 丢掉（`last_show` 是上次 [`show`] 的时刻）
fn debounced_by_tao(last_show: Option<Instant>, now: Instant) -> bool {
    last_show.is_some_and(|at| now.duration_since(at) < DEBOUNCE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_window_matches_tao() {
        let now = Instant::now();
        // 刚 show 过（去抖窗口内）：tao 会丢掉这次 hide，我们必须补一次
        assert!(debounced_by_tao(Some(now), now + Duration::from_millis(100)));
        assert!(debounced_by_tao(Some(now), now + DEBOUNCE - Duration::from_millis(1)));
        // 过了窗口（或本会话还没 show 过）：正常 hide，不需要补
        assert!(!debounced_by_tao(Some(now), now + DEBOUNCE));
        assert!(!debounced_by_tao(Some(now), now + Duration::from_secs(30)));
        assert!(!debounced_by_tao(None, now));
    }
}

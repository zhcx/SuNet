//! Dock 图标（Windows 侧：空实现）
//!
//! Windows 没有 Dock —— 任务栏按钮是**窗口**的属性，`Window::hide()` 之后按钮自然消失，
//! 不存在"进程还在跑却留一个点不出窗口的图标"这回事。这里与 macOS 同名同签名、
//! 什么也不做，好让 tray / cmd / main 里的调用点不用写 `cfg`（见 `os/mod.rs` 的模块约定）。

use tauri::AppHandle;

/// 让 Dock 图标回来（Windows：无此概念）
pub fn show(_app: &AppHandle) {}

/// 藏掉 Dock 图标（Windows：无此概念）
pub fn hide(_app: &AppHandle) {}

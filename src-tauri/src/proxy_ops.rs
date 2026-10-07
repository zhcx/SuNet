//! 系统代理「写」操作的唯一出口（跨平台提权封装）
//!
//! 读操作（`read` / `verify` / `parse_server`）各平台都不需要权限，照旧直接调用。
//! 写操作在 Windows 上写 HKCU，普通权限即可；在 macOS 上必须改
//! SystemConfiguration（`networksetup -setwebproxy` 等），**需要 root**。
//!
//! 因此这里把「写」统一收口：
//!   - `os::system_proxy::proxy_write_needs_root()` 为 false（Windows）→ 进程内直接调；
//!   - 为 true（macOS）→ 走 `elevation::run_elevated`（常驻 helper，回退 osascript）。
//!
//! 调用方语义不变：返回「系统配置变更通知是否成功」（E3004 降级用）。

use crate::error::Result;
use crate::os::system_proxy::{self, ProxyState};
use serde_json::json;

/// 任务名（与 `task_runner::execute` 的固定函数表对应）
pub const TASK_WRITE: &str = "proxy_write";
pub const TASK_RESTORE: &str = "proxy_restore";
pub const TASK_CLEAR: &str = "proxy_clear";

fn via_elevation(task: &str, data: serde_json::Value) -> Result<()> {
    // run_elevated 在 !out.ok 时已把 error 转成 Err，这里只做丢弃
    crate::elevation::run_elevated(task, data, crate::elevation::TASK_TIMEOUT)?;
    Ok(())
}

/// 写入手动代理（enable=false 即关闭手动代理）
pub fn write_manual(enable: bool, server: &str, bypass: &str) -> Result<bool> {
    if system_proxy::proxy_write_needs_root() {
        via_elevation(
            TASK_WRITE,
            json!({ "enable": enable, "server": server, "bypass": bypass }),
        )?;
        return Ok(system_proxy::notify_changed());
    }
    system_proxy::write_manual(enable, server, bypass)?;
    Ok(system_proxy::notify_changed())
}

/// 按给定状态原样还原（含 PAC）
pub fn restore(state: &ProxyState) -> Result<bool> {
    if system_proxy::proxy_write_needs_root() {
        via_elevation(TASK_RESTORE, json!({ "state": state }))?;
        return Ok(system_proxy::notify_changed());
    }
    system_proxy::restore(state)?;
    Ok(system_proxy::notify_changed())
}

/// 彻底清空代理配置（保留绕过列表）
pub fn hard_clear() -> Result<bool> {
    if system_proxy::proxy_write_needs_root() {
        via_elevation(TASK_CLEAR, json!({}))?;
        return Ok(system_proxy::notify_changed());
    }
    system_proxy::hard_clear()?;
    Ok(system_proxy::notify_changed())
}

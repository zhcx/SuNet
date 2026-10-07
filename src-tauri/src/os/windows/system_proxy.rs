//! 系统代理（WinINET，HKCU）—— 无需提权（设计方案 §3、§1.4.1）
//!
//! 写入要点：
//!   1. 写注册表后必须 InternetSetOptionW(39) + (37)，否则 Chromium 系应用继续用启动时缓存
//!   2. 通知失败**不中断事务**（E3004 降级为 warning）：注册表已写成功，写回原值反而更糟
//!   3. 清零必须删 AutoConfigURL，否则 PAC 仍接管，"清零"是假的（§5.5.6 第 3 步）

use crate::error::{Result, E3003, E3004};
use crate::os::winapi::{InternetSetOptionW, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED};
use serde::{Deserialize, Serialize};
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
use winreg::RegKey;

const KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
pub const DEFAULT_BYPASS: &str = "localhost;127.*;<local>";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct ProxyState {
    pub enable: bool,
    pub server: String,
    pub bypass: String,
    pub autoconfig_url: Option<String>,
    /// PAC 存在时优先级高于手动代理（§3.1）
    pub pac_present: bool,
}

impl ProxyState {
    /// 手动代理是否真的在生效（PAC 存在时手动代理会被覆盖）
    pub fn is_on(&self) -> bool {
        self.enable && !self.pac_present
    }
}

fn open(read_only: bool) -> Result<RegKey> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let flag = if read_only { KEY_READ } else { KEY_READ | KEY_WRITE };
    hkcu.open_subkey_with_flags(KEY_PATH, flag)
        .map_err(|e| crate::error::AppError::new(E3003, format!("打开代理注册表键失败：{e}")))
}

fn get_string(key: &RegKey, name: &str) -> String {
    key.get_value::<String, _>(name).unwrap_or_default()
}

/// 读取当前代理状态
pub fn read() -> Result<ProxyState> {
    let key = open(true)?;
    let enable = key.get_value::<u32, _>("ProxyEnable").unwrap_or(0) != 0;
    let server = get_string(&key, "ProxyServer");
    let mut bypass = get_string(&key, "ProxyOverride");
    if bypass.trim().is_empty() {
        bypass = DEFAULT_BYPASS.to_string();
    }
    let pac = get_string(&key, "AutoConfigURL");
    let pac_present = !pac.trim().is_empty();
    Ok(ProxyState {
        enable,
        server,
        bypass,
        autoconfig_url: if pac_present { Some(pac) } else { None },
        pac_present,
    })
}

/// 只写入手动代理三件套（不动 AutoConfigURL）
pub fn write_manual(enable: bool, server: &str, bypass: &str) -> Result<()> {
    let key = open(false)?;
    key.set_value("ProxyEnable", &if enable { 1u32 } else { 0u32 })
        .map_err(|e| crate::error::AppError::new(E3003, format!("写入 ProxyEnable 失败：{e}")))?;
    key.set_value("ProxyServer", &server.to_string())
        .map_err(|e| crate::error::AppError::new(E3003, format!("写入 ProxyServer 失败：{e}")))?;
    if !bypass.trim().is_empty() {
        key.set_value("ProxyOverride", &bypass.to_string())
            .map_err(|e| crate::error::AppError::new(E3003, format!("写入 ProxyOverride 失败：{e}")))?;
    }
    Ok(())
}

/// 完整写回（含 PAC），用于快照还原
pub fn restore(state: &ProxyState) -> Result<()> {
    let key = open(false)?;
    key.set_value("ProxyEnable", &if state.enable { 1u32 } else { 0u32 })
        .map_err(|e| crate::error::AppError::new(E3003, format!("写入 ProxyEnable 失败：{e}")))?;
    key.set_value("ProxyServer", &state.server)
        .map_err(|e| crate::error::AppError::new(E3003, format!("写入 ProxyServer 失败：{e}")))?;
    key.set_value("ProxyOverride", &state.bypass)
        .map_err(|e| crate::error::AppError::new(E3003, format!("写入 ProxyOverride 失败：{e}")))?;
    match &state.autoconfig_url {
        Some(url) if !url.trim().is_empty() => {
            key.set_value("AutoConfigURL", url)
                .map_err(|e| crate::error::AppError::new(E3003, format!("写入 AutoConfigURL 失败：{e}")))?;
        }
        _ => {
            // 原状态无 PAC → 确保删除，否则残留 PAC 会覆盖手动代理
            let _ = key.delete_value("AutoConfigURL");
        }
    }
    Ok(())
}

/// 彻底清零（§5.5.6）：ProxyEnable=0 + 删 AutoConfigURL + ProxyServer 置空 + 保留 ProxyOverride
pub fn hard_clear() -> Result<()> {
    let key = open(false)?;
    key.set_value("ProxyEnable", &0u32)
        .map_err(|e| crate::error::AppError::new(E3003, format!("写入 ProxyEnable 失败：{e}")))?;
    // 第 3 步：最容易漏的一步
    match key.delete_value("AutoConfigURL") {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log::warn!(target: "proxy", "删除 AutoConfigURL 失败：{e}"),
    }
    key.set_value("ProxyServer", &String::new())
        .map_err(|e| crate::error::AppError::new(E3003, format!("清空 ProxyServer 失败：{e}")))?;
    // ProxyOverride 保留：它是绕过列表，清空反而让 *.local 与内网地址走代理
    Ok(())
}

/// 回读校验
pub fn verify(expect_enable: bool) -> Result<bool> {
    let st = read()?;
    if expect_enable {
        Ok(st.enable)
    } else {
        Ok(!st.enable && !st.pac_present)
    }
}

/// 通知系统配置已变更（§3.2）；失败只记 warning，不中断事务
pub fn notify_changed() -> bool {
    unsafe {
        let a = InternetSetOptionW(
            std::ptr::null_mut(),
            INTERNET_OPTION_SETTINGS_CHANGED,
            std::ptr::null_mut(),
            0,
        );
        let b = InternetSetOptionW(
            std::ptr::null_mut(),
            INTERNET_OPTION_REFRESH,
            std::ptr::null_mut(),
            0,
        );
        if a == 0 || b == 0 {
            log::warn!(
                target: "proxy",
                "[{E3004}] InternetSetOptionW 通知失败（设置已生效，仅即时性打折）：{}",
                crate::os::winapi::last_error_text()
            );
            false
        } else {
            true
        }
    }
}

/// 写代理是否需要管理员权限。
///
/// Windows 走 HKCU，普通用户即可写 → false。
/// （macOS 对应实现见 `os/macos/system_proxy.rs`，那边恒为 true。）
pub fn proxy_write_needs_root() -> bool {
    false
}

/// 解析 "127.0.0.1:7890" 形式
pub fn parse_server(server: &str) -> Option<(String, u16)> {
    let s = server.trim();
    if s.is_empty() {
        return None;
    }
    // 支持 http=host:port;https=host:port 形式 → 取第一段
    let first = s.split(';').next().unwrap_or(s);
    let seg = first.rsplit('=').next().unwrap_or(first);
    let (host, port) = seg.rsplit_once(':')?;
    let port: u16 = port.trim().parse().ok()?;
    Some((host.trim().to_string(), port))
}

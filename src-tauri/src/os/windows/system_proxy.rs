//! 系统代理（WinINET，HKCU）—— 无需提权（设计方案 §3、§1.4.1）
//!
//! 写入要点：
//!   1. 写注册表后必须 InternetSetOptionW(39) + (37)，否则 Chromium 系应用继续用启动时缓存
//!   2. 通知失败**不中断事务**（E3004 降级为 warning）：注册表已写成功，写回原值反而更糟
//!   3. 清零必须删 AutoConfigURL，否则 PAC 仍接管，"清零"是假的（§5.5.6 第 3 步）

use crate::error::{Result, E3003, E3004};
use crate::os::winapi::{
    InternetSetOptionW, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED,
};
use serde::{Deserialize, Serialize};
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
use winreg::RegKey;

const KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
pub const DEFAULT_BYPASS: &str = "localhost;127.*;<local>";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct ProxyState {
    pub enable: bool,
    /// `ProxyServer` 的**原始值**（可能是裸 `host:port`，也可能是
    /// `http=…;https=…;socks=…` 分段串）：快照还原 [`restore`] 要原样写回，
    /// 所以这里不做归一化。逐协议落点看下面的 `*_proxy` 字段。
    pub server: String,
    pub bypass: String,
    pub autoconfig_url: Option<String>,
    /// PAC 存在时优先级高于手动代理（§3.1）
    pub pac_present: bool,
    /// 逐协议落点（`host:port`），由 [`read`] 从 `ProxyServer` 解析而来；
    /// `None` = 该协议没开，或快照里没有这个信息。
    ///
    /// WinINET 用一个值同时承载三个协议（分段语法 `http=…;https=…;socks=…`），
    /// 与 macOS 的三个独立开关在语义上对齐；`serde(default)` 让旧快照
    /// （以及 macOS 侧同一份 `proxy_snapshot.json`）照常读得进来。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_proxy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub https_proxy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socks_proxy: Option<String>,
}

impl ProxyState {
    /// 手动代理是否真的在生效（PAC 存在时手动代理会被覆盖）
    pub fn is_on(&self) -> bool {
        self.enable && !self.pac_present
    }

    /// 开着的协议 → `(标签, 落点)`，顺序与 Windows 系统代理设置一致（HTTP / HTTPS / SOCKS）。
    ///
    /// `*_proxy` 只在对应协议真的配了地址时才有值（见 [`read`]），这里不需要再判
    /// `enable`；与 macOS 同名同签名，托盘 `proxy_label` 与前端 `proxyEndpoints`
    /// 不用分平台就能逐协议显示。
    pub fn endpoints(&self) -> Vec<(&'static str, &str)> {
        [
            ("HTTP", self.http_proxy.as_deref()),
            ("HTTPS", self.https_proxy.as_deref()),
            ("SOCKS", self.socks_proxy.as_deref()),
        ]
        .iter()
        .filter_map(|(label, addr)| {
            addr.filter(|a| !a.trim().is_empty())
                .map(|a| (*label, a.trim()))
        })
        .collect()
    }
}

fn open(read_only: bool) -> Result<RegKey> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let flag = if read_only {
        KEY_READ
    } else {
        KEY_READ | KEY_WRITE
    };
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
    // server 字段保留原始串（restore 原样写回），逐协议字段才是展示 / 校验用的解析结果
    let (http_proxy, https_proxy, socks_proxy) = split_server(&server);
    Ok(ProxyState {
        enable,
        server,
        bypass,
        autoconfig_url: if pac_present { Some(pac) } else { None },
        pac_present,
        http_proxy,
        https_proxy,
        socks_proxy,
    })
}

/// 解析 `ProxyServer` 的两种写法 → 逐协议落点（Windows 系统代理的两种形态）：
///
/// · 裸 `host:port` —— 所有协议共用这个代理（本工具 0.0.3 前的写法，也是系统设置
///   「对以下条目不使用代理服务器」上方那个单框的写法）→ 记到 HTTP
/// · 分段 `http=host:port;https=host:port;socks=host:port` —— 与系统设置
///   里逐协议填地址等价，`socks=` 就是系统代理对 SOCKS 的写法
///   （`socks5=` 是第三方工具的变体，一并按 SOCKS 收）
fn split_server(server: &str) -> (Option<String>, Option<String>, Option<String>) {
    let s = server.trim();
    if s.is_empty() {
        return (None, None, None);
    }
    if !s.contains('=') {
        return (Some(s.to_string()), None, None);
    }
    let mut http = None;
    let mut https = None;
    let mut socks = None;
    for seg in s.split(';') {
        let seg = seg.trim();
        if seg.is_empty() {
            continue;
        }
        let Some((scheme, value)) = seg.split_once('=') else {
            continue;
        };
        let (scheme, value) = (scheme.trim().to_ascii_lowercase(), value.trim());
        if value.is_empty() {
            continue;
        }
        let slot = match scheme.as_str() {
            "http" => &mut http,
            "https" => &mut https,
            "socks" | "socks5" => &mut socks,
            // ftp= 等其它分段不属于本工具管辖，忽略
            _ => continue,
        };
        *slot = Some(value.to_string());
    }
    (http, https, socks)
}

/// 把逐协议落点编码成 `ProxyServer` 值（[`split_server`] 的逆运算）：
///
/// · 只有 HTTP → 裸 `host:port`（与 0.0.3 前完全一致，旧版还原 / 第三方工具都认）
/// · 出现 HTTPS / SOCKS 任一 → 段语法 `http=…;https=…;socks=…`，只写非空协议。
///   注意分段里**必须给每个非空协议都带上 scheme 前缀**：Chromium 一旦看到
///   `scheme=` 就只按分段解析，裸 `host:port` 段会被整体忽略
fn join_server(p: &crate::os::ManualProxy) -> String {
    let http = p.http.trim();
    let https = p.https.as_deref().map(str::trim).unwrap_or("");
    let socks = p.socks.as_deref().map(str::trim).unwrap_or("");
    if https.is_empty() && socks.is_empty() {
        return http.to_string();
    }
    [
        (!http.is_empty()).then(|| format!("http={http}")),
        (!https.is_empty()).then(|| format!("https={https}")),
        (!socks.is_empty()).then(|| format!("socks={socks}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(";")
}

/// 只写入手动代理三件套（不动 AutoConfigURL）
///
/// WinINET 用一个 `ProxyServer` 值承载全部协议：只有 HTTP 时写裸 `host:port`
/// （与 0.0.3 前一致）；配了 HTTPS / SOCKS 时写系统代理同款分段语法
/// `http=…;https=…;socks=…`（`socks=` 即系统设置里 SOCKS 一项的写法）。
/// 三个协议的语义与 macOS 对齐：给了落点的写进去，`None` / 空串 = 关闭该协议。
pub fn write_manual(p: &crate::os::ManualProxy) -> Result<()> {
    if !p.enable {
        // 关闭时不看落点：清掉已有的 proxy 字符串（与 macOS 的「三项全关」对齐）
        let key = open(false)?;
        key.set_value("ProxyEnable", &0u32).map_err(|e| {
            crate::error::AppError::new(E3003, format!("写入 ProxyEnable 失败：{e}"))
        })?;
        key.set_value("ProxyServer", &String::new()).map_err(|e| {
            crate::error::AppError::new(E3003, format!("写入 ProxyServer 失败：{e}"))
        })?;
        if !p.bypass.trim().is_empty() {
            key.set_value("ProxyOverride", &p.bypass).map_err(|e| {
                crate::error::AppError::new(E3003, format!("写入 ProxyOverride 失败：{e}"))
            })?;
        }
        return Ok(());
    }

    let key = open(false)?;
    key.set_value("ProxyEnable", &1u32)
        .map_err(|e| crate::error::AppError::new(E3003, format!("写入 ProxyEnable 失败：{e}")))?;
    key.set_value("ProxyServer", &join_server(p))
        .map_err(|e| crate::error::AppError::new(E3003, format!("写入 ProxyServer 失败：{e}")))?;
    if !p.bypass.trim().is_empty() {
        key.set_value("ProxyOverride", &p.bypass).map_err(|e| {
            crate::error::AppError::new(E3003, format!("写入 ProxyOverride 失败：{e}"))
        })?;
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
            key.set_value("AutoConfigURL", url).map_err(|e| {
                crate::error::AppError::new(E3003, format!("写入 AutoConfigURL 失败：{e}"))
            })?;
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

/// 逐协议回读校验。
///
/// WinINET 的三个协议共用一个 `ProxyServer` 值，但语义上已是逐协议的
/// （见 [`split_server`] / [`join_server`]）：把"想要的状态"逐协议与回读结果比对，
/// 写一半（比如只落了 `http=` 段）会被发现。
pub fn verify_manual(want: &crate::os::ManualProxy) -> Result<bool> {
    let st = read()?;
    if !want.enable {
        return Ok(!st.enable && !st.pac_present);
    }
    Ok(
        st.enable
            && !st.pac_present
            && norm_opt(st.http_proxy.as_deref()) == norm_opt(Some(want.http.as_str()))
            && norm_opt(st.https_proxy.as_deref()) == norm_opt(want.https.as_deref())
            && norm_opt(st.socks_proxy.as_deref()) == norm_opt(want.socks.as_deref()),
    )
}

/// `None` / 空串统一成 `None`（逐协议比对用）
fn norm_opt(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
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
    // 分段语法 `http=…;https=…;socks=…`：优先 HTTP 段（主落点语义与 macOS 一致），
    // 没有 HTTP 再依次看 HTTPS / SOCKS
    if s.contains('=') {
        let (http, https, socks) = split_server(s);
        for seg in [http, https, socks].into_iter().flatten() {
            if let Some(hit) = parse_single(&seg) {
                return Some(hit);
            }
        }
        return None;
    }
    parse_single(s)
}

/// 解析单个 `host:port` 段
fn parse_single(seg: &str) -> Option<(String, u16)> {
    let (host, port) = seg.trim().rsplit_once(':')?;
    let port: u16 = port.trim().parse().ok()?;
    Some((host.trim().to_string(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_server_bare_value_goes_to_http() {
        // 裸 `host:port` = 所有协议共用（本工具旧写法）→ 记到 HTTP
        assert_eq!(
            split_server("127.0.0.1:7890"),
            (Some("127.0.0.1:7890".into()), None, None)
        );
        assert_eq!(split_server("  "), (None, None, None));
    }

    #[test]
    fn split_server_segments() {
        assert_eq!(
            split_server("http=127.0.0.1:7890;https=127.0.0.1:7891;socks=127.0.0.1:1080"),
            (
                Some("127.0.0.1:7890".into()),
                Some("127.0.0.1:7891".into()),
                Some("127.0.0.1:1080".into())
            )
        );
        // 只有 SOCKS：系统设置里「套接字」单独一项的写法；socks5= 变体一并收
        assert_eq!(
            split_server("socks=127.0.0.1:1080"),
            (None, None, Some("127.0.0.1:1080".into()))
        );
        assert_eq!(
            split_server("socks5=127.0.0.1:1080"),
            (None, None, Some("127.0.0.1:1080".into()))
        );
        // 未知 scheme（ftp=）与空段不进任何协议
        assert_eq!(
            split_server("http=1.2.3.4:80;ftp=1.2.3.4:21;;"),
            (Some("1.2.3.4:80".into()), None, None)
        );
    }

    #[test]
    fn join_server_roundtrip() {
        // 只有 HTTP → 裸串（与 0.0.3 前完全一致）
        assert_eq!(
            join_server(&crate::os::ManualProxy::http_only("127.0.0.1:7890", "localhost")),
            "127.0.0.1:7890"
        );
        let p = crate::os::ManualProxy {
            enable: true,
            http: "127.0.0.1:7890".into(),
            https: Some("127.0.0.1:7891".into()),
            socks: Some("127.0.0.1:1080".into()),
            bypass: String::new(),
        };
        let joined = join_server(&p);
        assert_eq!(joined, "http=127.0.0.1:7890;https=127.0.0.1:7891;socks=127.0.0.1:1080");
        // 编码 → 解析闭环
        assert_eq!(split_server(&joined), (
            Some("127.0.0.1:7890".into()),
            Some("127.0.0.1:7891".into()),
            Some("127.0.0.1:1080".into())
        ));

        // 只开 SOCKS（HTTP / HTTPS 关）→ 段里不出现空落点
        let socks_only = crate::os::ManualProxy {
            enable: true,
            http: String::new(),
            https: None,
            socks: Some("127.0.0.1:1080".into()),
            bypass: String::new(),
        };
        assert_eq!(join_server(&socks_only), "socks=127.0.0.1:1080");
    }

    #[test]
    fn parse_server_prefers_http_segment() {
        // 主落点语义与 macOS 一致：HTTP → HTTPS → SOCKS 里第一个开着的
        assert_eq!(
            parse_server("http=127.0.0.1:7890;socks=127.0.0.1:1080"),
            Some(("127.0.0.1".into(), 7890))
        );
        assert_eq!(
            parse_server("socks=127.0.0.1:1080"),
            Some(("127.0.0.1".into(), 1080))
        );
        assert_eq!(parse_server("127.0.0.1:7890"), Some(("127.0.0.1".into(), 7890)));
        assert_eq!(parse_server(""), None);
        assert_eq!(parse_server("http=broken"), None);
    }

    #[test]
    fn endpoints_reports_each_protocol() {
        let st = ProxyState {
            enable: true,
            server: "http=127.0.0.1:7890;socks=127.0.0.1:1080".into(),
            bypass: String::new(),
            autoconfig_url: None,
            pac_present: false,
            http_proxy: Some("127.0.0.1:7890".into()),
            https_proxy: None,
            socks_proxy: Some("127.0.0.1:1080".into()),
        };
        assert_eq!(
            st.endpoints(),
            vec![("HTTP", "127.0.0.1:7890"), ("SOCKS", "127.0.0.1:1080")]
        );
    }
}

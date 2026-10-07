//! 提权通道：常驻 root helper（macOS）
//!
//! 为什么需要它：Windows 有 UAC，一次授权即可；macOS 的 `osascript ... with
//! administrator privileges` **每次都要输密码**，而 hosts / DNS / 代理的写入
//! 又都在每次切换方案时发生。因此按 2026-10-07 的决策装一个常驻 root helper：
//! 装一次，之后切换方案不再弹密码；`osascript` 只作回退（没装 helper 时）。
//!
//! 安全模型（写在这里，改代码前先读）：
//!   - helper 以 LaunchDaemon 形式常驻，root 身份运行，只做一件事：
//!     接收 `{task, payload}` → 调用 `task_runner::execute`（**不是任意命令执行**）
//!   - 通道是 unix socket `/var/run/sunet-helper.sock`，文件权限 0666 是为了让
//!     非 root 的控制台用户可以连上；**授权靠 peer uid 校验**（见 `daemon.rs`）
//!   - 只接受 root 与控制台用户（`/dev/console` 的属主）的连接
//!   - 任务名与载荷仍走 `task_runner` 的既有校验（白名单任务、IP/域名/服务器地址校验）
//!   - helper 二进制安装到 `/Library/PrivilegedHelperTools/com.sunet.helper`，
//!     不从 .app 内部直接运行（应用被移动/删除后守护进程不会失效）
//!
//! 已知限制：macOS 14.6.1 起，`/Library/LaunchDaemons` 的后台任务还会被 BTM
//! （后台任务管理）拦一道，用户可能要在「系统设置 → 通用 → 登录项与扩展」里
//! 手动放行；失败时就退回 osascript 逐次授权。

pub mod client;
pub mod daemon;
pub mod install;

pub use client::run_task;

/// unix socket 路径
pub const SOCKET_PATH: &str = "/var/run/sunet-helper.sock";
/// LaunchDaemon 标签
pub const LABEL: &str = "com.sunet.helper";
/// LaunchDaemon plist 路径
pub const PLIST_PATH: &str = "/Library/LaunchDaemons/com.sunet.helper.plist";
/// 常驻二进制路径（把 .app 里的可执行文件拷到这里再运行）
pub const HELPER_BIN: &str = "/Library/PrivilegedHelperTools/com.sunet.helper";
/// helper 日志（launchd 的 StandardOutPath/StandardErrorPath）
pub const LOG_PATH: &str = "/var/log/sunet-helper.log";

/// helper 是否在跑（socket 存在即可认为在跑）
pub fn available() -> bool {
    std::path::Path::new(SOCKET_PATH).exists()
}

/// 回退通道是否可用（osascript 一定存在，这里只是给上层统一判断）
pub fn osascript_available() -> bool {
    std::path::Path::new(crate::os::macos::net::OSASCRIPT).exists()
}

/// 是否已安装（plist 存在即视为已安装）
pub fn installed() -> bool {
    std::path::Path::new(PLIST_PATH).exists()
}

//! 跨进程临界区（设计方案 §7.2）
//!
//! 只有一条进程内 Mutex 是不够的：模型 C 下 hosts/DNS 的写入发生在
//! **另一个进程**（提权子进程）里，进程内的锁对它没有约束力。两层锁各管一段：
//!   - 进程内 `APPLY_LOCK`：同一进程内并发的 apply / rollback 命令
//!   - 跨进程锁：主进程 + 提权子进程共享的写入临界区
//!     Windows → 命名互斥体（`WAIT_ABANDONED` 不是错误：上一个持有者崩溃时内核
//!     已自动释放，此时应当**接管并记一条 warning**，而不是一直等下去）
//!     macOS → `/tmp` 下的 `flock`（helper 以 root 运行，`~/Library/...` 会各锁各的）

use crate::error::{AppError, Result};
use std::time::Duration;

#[cfg(target_os = "windows")]
use crate::os::winapi::*;

#[cfg(target_os = "windows")]
const IPC_MUTEX: &str = r"Local\SuNet.HostsDns.CriticalSection";

/// macOS 的跨进程锁文件。放在 `/tmp` 而不是 `~/Library/...`：
/// 提权 helper 的 HOME 是 `/var/root`，两边路径不同就变成"各锁各的"，等于没锁。
#[cfg(target_os = "macos")]
const LOCK_PATH: &std::ffi::CStr = c"/tmp/sunet-critsec.lock";

// ---------------------------------------------------------------------------
// Windows：命名互斥体
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
struct HandleGuard(HANDLE);

#[cfg(target_os = "windows")]
impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}

/// 在跨进程临界区内执行 f（锁包住「读—改—写」全过程，不能只包写入那一步）
#[cfg(target_os = "windows")]
pub fn with_critical_section<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    let name = wide(IPC_MUTEX);
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(AppError::internal(format!(
            "创建命名互斥体失败：{}",
            last_error_text()
        )));
    }
    let _guard = HandleGuard(handle);
    // 从这里到函数结束，本进程对外表现为"正在写入"
    BUSY.store(true, std::sync::atomic::Ordering::Relaxed);
    let _busy = BusyGuard;

    let result = match unsafe { WaitForSingleObject(handle, LOCK_TIMEOUT.as_millis() as DWORD) } {
        WAIT_OBJECT_0 => f(),
        WAIT_ABANDONED => {
            log::warn!(
                target: "critsec",
                "检测到 WAIT_ABANDONED：上一个持有者异常退出，已接管临界区"
            );
            f()
        }
        WAIT_TIMEOUT => Err(AppError::internal(
            "另一个 SuNet 进程正在修改 hosts/DNS，请稍后重试",
        )),
        other => Err(AppError::internal(format!("获取临界区失败：wait 返回 {other}"))),
    };
    result
}

// ---------------------------------------------------------------------------
// macOS：flock
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub fn with_critical_section<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    let fd = unsafe {
        libc::open(
            LOCK_PATH.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC,
            0o666 as libc::c_uint,
        )
    };
    if fd < 0 {
        // 锁文件不可用（极少见）不能把功能挡死：退回进程内串行，并如实记一条日志
        log::warn!(
            target: "critsec",
            "跨进程锁文件不可用（{}），本次仅使用进程内锁",
            std::io::Error::last_os_error()
        );
        BUSY.store(true, std::sync::atomic::Ordering::Relaxed);
        let _busy = BusyGuard;
        return f();
    }
    // 权限修正：helper 以 root 建的文件得让用户进程也能打开（反之亦然）
    unsafe {
        libc::fchmod(fd, 0o666 as libc::mode_t);
    }

    let started = std::time::Instant::now();
    loop {
        if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            break;
        }
        let err = std::io::Error::last_os_error();
        let would_block = matches!(err.raw_os_error(), Some(libc::EWOULDBLOCK));
        if !would_block {
            unsafe {
                libc::close(fd);
            }
            return Err(AppError::internal(format!("获取临界区失败：{err}")));
        }
        if started.elapsed() >= LOCK_TIMEOUT {
            unsafe {
                libc::close(fd);
            }
            return Err(AppError::internal(
                "另一个 SuNet 进程正在修改 hosts/DNS，请稍后重试",
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    BUSY.store(true, std::sync::atomic::Ordering::Relaxed);
    let _busy = BusyGuard;
    let result = f();
    unsafe {
        libc::flock(fd, libc::LOCK_UN);
        libc::close(fd);
    }
    result
}

// ---------------------------------------------------------------------------
// 公共部分
// ---------------------------------------------------------------------------

/// 本进程是否正在写入 hosts/DNS。
///
/// 注意这里**不能**用"试着 Wait(0) 一下命名互斥体"来探测：
/// Wait 本身会获取所有权，两个窗口并发查询状态时会互相把对方看成"正在写入"，
/// 界面就会一直闪一条假的冲突警告（实测踩过）。
/// 跨进程的真实冲突仍然由 `with_critical_section` 的等待与超时来如实报错。
static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn is_contended() -> bool {
    BUSY.load(std::sync::atomic::Ordering::Relaxed)
}

struct BusyGuard;

impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

/// 临界区等待上限（超出即报"另一个 SuNet 进程正在修改 hosts/DNS"）
pub const LOCK_TIMEOUT: Duration = Duration::from_millis(10_000);

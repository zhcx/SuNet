//! 跨进程临界区（设计方案 §7.2）
//!
//! 只有一条进程内 Mutex 是不够的：模型 C 下 hosts/DNS 的写入发生在
//! **另一个进程**（提权子进程）里，进程内的锁对它没有约束力。两层锁各管一段：
//!   - 进程内 `APPLY_LOCK`：同一进程内并发的 apply / rollback 命令
//!   - 命名互斥体：主进程 + 提权子进程共享的写入临界区
//!
//! `WAIT_ABANDONED` 不是错误：表示上一个持有者崩溃/被杀，内核已自动释放，
//! 此时应当**接管并记一条 warning**，而不是一直等下去。

use crate::error::{AppError, Result};
use crate::os::winapi::*;
use std::time::Duration;

const IPC_MUTEX: &str = r"Local\SuNet.HostsDns.CriticalSection";
const LOCK_TIMEOUT_MS: DWORD = 10_000;

struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}

/// 在跨进程临界区内执行 f（锁包住「读—改—写」全过程，不能只包写入那一步）
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

/// 互斥体等待上限（超出即报"另一个 SuNet 进程正在修改 hosts/DNS"）
pub const LOCK_TIMEOUT: Duration = Duration::from_millis(LOCK_TIMEOUT_MS as u64);

//! Win32 FFI 声明
//!
//! 设计文档 §1.2 建议用 `windows` crate，但其 API 面貌随小版本漂移
//! （文档自身在 `0.6` / `0.61` 之间摇摆）。这里改为手写最小 FFI：
//! 只声明本项目真正调用的 10 余个导出函数，签名固定、无版本风险，
//! 同时避免把编译时间花在一个数百 crate 的元包上。

#![allow(non_snake_case, non_camel_case_types, dead_code)]

use std::os::raw::{c_int, c_uint, c_void};

pub type HANDLE = *mut c_void;
pub type BOOL = i32;
pub type DWORD = u32;
pub type LPCWSTR = *const u16;
pub type LPWSTR = *mut u16;

// ---------------------------------------------------------------------------
// kernel32
// ---------------------------------------------------------------------------
#[link(name = "kernel32")]
extern "system" {
    pub fn GetLastError() -> DWORD;
    pub fn CreateMutexW(
        lpMutexAttributes: *const c_void,
        bInitialOwner: BOOL,
        lpName: LPCWSTR,
    ) -> HANDLE;
    pub fn ReleaseMutex(hMutex: HANDLE) -> BOOL;
    pub fn CloseHandle(hObject: HANDLE) -> BOOL;
    pub fn WaitForSingleObject(hHandle: HANDLE, dwMilliseconds: DWORD) -> DWORD;
    pub fn GetExitCodeProcess(hProcess: HANDLE, lpExitCode: *mut DWORD) -> BOOL;
    pub fn TerminateProcess(hProcess: HANDLE, uExitCode: c_uint) -> BOOL;
    pub fn GetCurrentProcess() -> HANDLE;
    pub fn GetFileAttributesW(lpFileName: LPCWSTR) -> DWORD;
    pub fn ReplaceFileW(
        lpReplacedFileName: LPCWSTR,
        lpReplacementFileName: LPCWSTR,
        lpBackupFileName: LPCWSTR,
        dwReplaceFlags: DWORD,
        lpExclude: *mut c_void,
        lpReserved: *mut c_void,
    ) -> BOOL;
    pub fn LocalFree(hMem: HANDLE) -> HANDLE;
    pub fn CreateFileW(
        lpFileName: LPCWSTR,
        dwDesiredAccess: DWORD,
        dwShareMode: DWORD,
        lpSecurityAttributes: *const c_void,
        dwCreationDisposition: DWORD,
        dwFlagsAndAttributes: DWORD,
        hTemplateFile: HANDLE,
    ) -> HANDLE;
    pub fn MultiByteToWideChar(
        CodePage: DWORD,
        dwFlags: DWORD,
        lpMultiByteStr: *const u8,
        cbMultiByte: c_int,
        lpWideCharStr: LPWSTR,
        cchWideChar: c_int,
    ) -> c_int;
}

pub const WAIT_OBJECT_0: DWORD = 0x0000_0000;
pub const WAIT_ABANDONED: DWORD = 0x0000_0080;
pub const WAIT_TIMEOUT: DWORD = 0x0000_0102;
pub const FILE_ATTRIBUTE_REPARSE_POINT: DWORD = 0x0000_0400;
pub const FILE_ATTRIBUTE_INVALID: DWORD = 0xFFFF_FFFF;
pub const REPLACEFILE_IGNORE_MERGE_ERRORS: DWORD = 0x0000_0002;
pub const ERROR_ALREADY_EXISTS: DWORD = 183;
pub const ERROR_SUCCESS: DWORD = 0;

// ---------------------------------------------------------------------------
// advapi32
// ---------------------------------------------------------------------------
#[link(name = "advapi32")]
extern "system" {
    pub fn OpenProcessToken(
        ProcessHandle: HANDLE,
        DesiredAccess: DWORD,
        TokenHandle: *mut HANDLE,
    ) -> BOOL;
    pub fn GetTokenInformation(
        TokenHandle: HANDLE,
        TokenInformationClass: c_int,
        TokenInformation: *mut c_void,
        TokenInformationLength: DWORD,
        ReturnLength: *mut DWORD,
    ) -> BOOL;
    pub fn GetSecurityInfo(
        handle: HANDLE,
        ObjectType: c_int,
        SecurityInfo: DWORD,
        ppsidOwner: *mut *mut c_void,
        ppsidGroup: *mut *mut c_void,
        ppDacl: *mut *mut c_void,
        ppSacl: *mut *mut c_void,
        ppSecurityDescriptor: *mut *mut c_void,
    ) -> DWORD;
    pub fn ConvertSidToStringSidW(Sid: *mut c_void, StringSid: *mut LPWSTR) -> BOOL;
}

pub const TOKEN_QUERY: DWORD = 0x0008;
pub const TOKEN_USER_CLASS: c_int = 1; // TokenUser
pub const TOKEN_ELEVATION_CLASS: c_int = 20; // TokenElevation
pub const SE_FILE_OBJECT: c_int = 1;
pub const OWNER_SECURITY_INFORMATION: DWORD = 0x0000_0001;

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct TokenElevation {
    pub TokenIsElevated: DWORD,
}

// ---------------------------------------------------------------------------
// wininet（系统代理通知，§3.2）
// ---------------------------------------------------------------------------
#[link(name = "wininet")]
extern "system" {
    pub fn InternetSetOptionW(
        hInternet: HANDLE,
        dwOption: DWORD,
        lpBuffer: *mut c_void,
        dwBufferLength: DWORD,
    ) -> BOOL;
}

pub const INTERNET_OPTION_REFRESH: DWORD = 37;
pub const INTERNET_OPTION_SETTINGS_CHANGED: DWORD = 39;

// ---------------------------------------------------------------------------
// dnsapi（hosts 变更后刷新解析缓存，§2.2 步骤 7）
// ---------------------------------------------------------------------------
#[link(name = "dnsapi")]
extern "system" {
    pub fn DnsFlushResolverCache() -> BOOL;
}

// ---------------------------------------------------------------------------
// shell32（提权入口，§1.4.4：必须走 runas verb，CreateProcess 不触发 UAC）
// ---------------------------------------------------------------------------
#[link(name = "shell32")]
extern "system" {
    pub fn ShellExecuteExW(lpExecInfo: *mut SHELLEXECUTEINFOW) -> BOOL;
}

pub const SEE_MASK_NOCLOSEPROCESS: DWORD = 0x0000_0040;
pub const SEE_MASK_NOASYNC: DWORD = 0x0000_0100;
/// 提权子进程是纯后台任务，隐藏其窗口避免任务栏闪一下
pub const SW_HIDE: c_int = 0;
pub const SW_SHOWNORMAL: c_int = 1;
pub const ERROR_CANCELLED: DWORD = 1223;
pub const ERROR_ELEVATION_REQUIRED: DWORD = 740;

#[repr(C)]
pub struct SHELLEXECUTEINFOW {
    pub cbSize: DWORD,
    pub fMask: DWORD,
    pub hwnd: HANDLE,
    pub lpVerb: LPCWSTR,
    pub lpFile: LPCWSTR,
    pub lpParameters: LPCWSTR,
    pub lpDirectory: LPCWSTR,
    pub nShow: c_int,
    pub hInstApp: HANDLE,
    pub lpIDList: *mut c_void,
    pub lpClass: LPCWSTR,
    pub hkeyClass: HANDLE,
    pub dwHotKey: DWORD,
    /// 联合体 hIcon / hMonitor，二者在 ABI 上同为 HANDLE
    pub hIcon: HANDLE,
    pub hProcess: HANDLE,
}

impl Default for SHELLEXECUTEINFOW {
    fn default() -> Self {
        Self {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as DWORD,
            fMask: 0,
            hwnd: std::ptr::null_mut(),
            lpVerb: std::ptr::null(),
            lpFile: std::ptr::null(),
            lpParameters: std::ptr::null(),
            lpDirectory: std::ptr::null(),
            nShow: SW_SHOWNORMAL,
            hInstApp: std::ptr::null_mut(),
            lpIDList: std::ptr::null_mut(),
            lpClass: std::ptr::null(),
            hkeyClass: std::ptr::null_mut(),
            dwHotKey: 0,
            hIcon: std::ptr::null_mut(),
            hProcess: std::ptr::null_mut(),
        }
    }
}

// ---------------------------------------------------------------------------
// user32（全局热键冲突预检，§5.5.4）
// ---------------------------------------------------------------------------
#[link(name = "user32")]
extern "system" {
    pub fn RegisterHotKey(hWnd: HANDLE, id: c_int, fsModifiers: DWORD, vk: DWORD) -> BOOL;
    pub fn UnregisterHotKey(hWnd: HANDLE, id: c_int) -> BOOL;
    /// 无边框窗口的第一帧修正（见 `force_frame_recalc`）
    pub fn SetWindowPos(
        hWnd: HANDLE,
        hWndInsertAfter: HANDLE,
        X: c_int,
        Y: c_int,
        cx: c_int,
        cy: c_int,
        uFlags: c_uint,
    ) -> BOOL;
}

pub const MOD_ALT: DWORD = 0x0001;
pub const MOD_CONTROL: DWORD = 0x0002;
pub const MOD_SHIFT: DWORD = 0x0004;
pub const MOD_WIN: DWORD = 0x0008;

// ---------------------------------------------------------------------------
// iphlpapi（网卡枚举，§4：替代 PowerShell 的 Get-NetAdapter，毫秒级）
// 结构体只声明到本项目实际读取的字段为止，尾部字段不访问；
// 字段偏移必须与 Windows SDK 的 IP_ADAPTER_ADDRESSES_LH 系列保持一致（x64）。
// ---------------------------------------------------------------------------
#[link(name = "iphlpapi")]
extern "system" {
    pub fn GetAdaptersAddresses(
        Family: DWORD,
        Flags: DWORD,
        Reserved: *mut c_void,
        AdapterAddresses: *mut IpAdapterAddresses,
        SizePointer: *mut DWORD,
    ) -> DWORD;
    /// 查路由表：返回发往 dwDestAddr 的流量会走哪块网卡（ifIndex）。
    /// 纯本地路由查询，不发任何网络包 —— "当前正在使用"的权威判定。
    pub fn GetBestInterface(dwDestAddr: DWORD, pdwBestIfIndex: *mut DWORD) -> DWORD;
}

pub const AF_UNSPEC: DWORD = 0;
pub const AF_INET: u16 = 2;
pub const AF_INET6: u16 = 23;
pub const GAA_FLAG_SKIP_ANYCAST: DWORD = 0x0000_0002;
pub const GAA_FLAG_SKIP_MULTICAST: DWORD = 0x0000_0004;
pub const GAA_FLAG_INCLUDE_GATEWAYS: DWORD = 0x0000_0040;
pub const ERROR_BUFFER_OVERFLOW: DWORD = 111;
pub const ERROR_NO_DATA: DWORD = 232;
pub const IF_OPER_STATUS_UP: u32 = 1;
pub const IF_TYPE_ETHERNET_CSMACD: u32 = 6;
pub const IF_TYPE_IEEE80211: u32 = 71;
pub const IP_PREFIX_ORIGIN_WELLKNOWN: u32 = 2;
pub const IP_DAD_STATE_PREFERRED: u32 = 4;
pub const CP_ACP: DWORD = 0;

#[repr(C)]
pub struct IpAdapterAddresses {
    /// 联合体低半：Length；高半：IfIndex
    pub length: DWORD,
    pub if_index: DWORD,
    pub next: *mut IpAdapterAddresses,
    /// ANSI 的 "{GUID}" 字符串，与 Get-NetAdapter 的 InterfaceGuid 一致
    pub adapter_name: *const u8,
    pub first_unicast: *mut IpAdapterUnicastAddress,
    pub first_anycast: *mut c_void,
    pub first_multicast: *mut c_void,
    pub first_dns_server: *mut IpAdapterDnsServerAddress,
    pub dns_suffix: *const u8,
    /// ANSI（CP_ACP）；GAA 不提供宽字符版本
    pub description: *const u8,
    pub friendly_name: *const u16,
    pub physical_address: [u8; 8],
    pub physical_address_length: DWORD,
    pub flags: DWORD,
    pub mtu: DWORD,
    pub if_type: DWORD,
    pub oper_status: DWORD,
    pub ipv6_if_index: DWORD,
    pub zone_indices: [DWORD; 16],
    pub first_prefix: *mut c_void,
    // —— 以下为 LH 扩展字段（x64 偏移由前述字段唯一确定），只读网关判"正在使用" ——
    pub transmit_link_speed: u64,
    pub receive_link_speed: u64,
    pub first_wins_server_address: *mut c_void,
    /// 非空 = 该网卡有默认网关；只判空不解引用，无需完整 GatewayAddress 结构
    pub first_gateway_address: *mut c_void,
}

#[repr(C)]
pub struct IpAdapterUnicastAddress {
    /// 联合体低半：Length；高半：Flags
    pub length: DWORD,
    pub flags: DWORD,
    pub next: *mut IpAdapterUnicastAddress,
    pub address: SocketAddress,
    pub prefix_origin: DWORD,
    pub suffix_origin: DWORD,
    pub dad_state: DWORD,
}

#[repr(C)]
pub struct IpAdapterDnsServerAddress {
    pub length: DWORD,
    pub reserved: DWORD,
    pub next: *mut IpAdapterDnsServerAddress,
    pub address: SocketAddress,
}

#[repr(C)]
pub struct SocketAddress {
    pub sockaddr: *const u8,
    /// 实为 c_int；补 4 字节对齐到 16（x64 上 SOCKET_ADDRESS 总长 16）
    pub i_sockaddr_length: DWORD,
}

// ---------------------------------------------------------------------------
// 辅助
// ---------------------------------------------------------------------------

/// Rust &str → 以 NUL 结尾的 UTF-16 缓冲
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 指针 → String（读到 NUL 为止）
pub unsafe fn from_wide_ptr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while *p.add(len) != 0 {
        len += 1;
    }
    let slice = std::slice::from_raw_parts(p, len);
    String::from_utf16_lossy(slice)
}

/// 格式化 GetLastError() 的可读文本
pub fn last_error_text() -> String {
    let code = unsafe { GetLastError() };
    format!("Win32 错误码 {code}")
}

/// 强制窗口重算框架（SWP_FRAMECHANGED）。
///
/// 无边框窗口（`decorations: false`）在 Windows 上并非真正没有非客户区：
/// tao 只在 `WM_NCCALCSIZE(wParam = TRUE)` 那一次里把标题栏剥掉，
/// 而 `CreateWindowExW` 之后的第一次计算（wParam = FALSE）走的是系统默认分支，
/// 于是窗口样式里的 `WS_CAPTION` 会被画成一条真实的系统标题栏，
/// 直到窗口被移动、缩放或最大化过才消失。
/// 这里发一次 SWP_FRAMECHANGED 主动触发框架重算，让窗口从第一帧起就无边框。
pub fn force_frame_recalc(hwnd: HANDLE) {
    const SWP_NOSIZE: c_uint = 0x0001;
    const SWP_NOMOVE: c_uint = 0x0002;
    const SWP_NOZORDER: c_uint = 0x0004;
    const SWP_NOACTIVATE: c_uint = 0x0010;
    const SWP_FRAMECHANGED: c_uint = 0x0020;
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOSIZE | SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

//! 错误码与用户文案（设计方案 §14.3）
//!
//! 三段原则：
//! 1. 能降级的降级，不要中断事务（典型 E3004）
//! 2. 不能假装的绝不假装（E2004 / E4003 必须报失败）
//! 3. 区分「权限不足」与「功能被占用」（E1xxx vs E5xxx）

use serde::{Deserialize, Serialize};

pub const E1001: &str = "E1001";
pub const E1002: &str = "E1002";
pub const E1003: &str = "E1003";
pub const E1004: &str = "E1004";
pub const E2001: &str = "E2001";
pub const E2002: &str = "E2002";
pub const E2003: &str = "E2003";
pub const E2004: &str = "E2004";
pub const E3001: &str = "E3001";
pub const E3002: &str = "E3002";
pub const E3003: &str = "E3003";
pub const E3004: &str = "E3004";
pub const E4001: &str = "E4001";
pub const E4002: &str = "E4002";
pub const E4003: &str = "E4003";
pub const E4004: &str = "E4004";
pub const E5001: &str = "E5001";
pub const E5002: &str = "E5002";
pub const E5003: &str = "E5003";
pub const E5004: &str = "E5004";
pub const E6001: &str = "E6001";
pub const E6002: &str = "E6002";
pub const E6003: &str = "E6003";
pub const E9001: &str = "E9001";
pub const E9002: &str = "E9002";
pub const E0000: &str = "E0000";

/// 错误码 → 标准中文文案
pub fn text(code: &str) -> &'static str {
    match code {
        E1001 => "你取消了管理员授权，操作未执行",
        E1002 => "已获得管理员权限但仍被拒绝（可能被组策略或文件 ACL 限制）",
        E1003 => "提权任务超时，已中断",
        E1004 => "提权载荷校验失败，已拒绝执行",
        E2001 => "hosts 文件被占用，写入失败（常见于安全软件）",
        E2002 => "无法识别 hosts 文件编码，为避免破坏已拒绝修改",
        E2003 => "hosts 托管标记损坏（有 BEGIN 无 END 或相反），已停止写入",
        E2004 => "hosts 写入后回读校验不一致",
        E3001 => "前置连通性探测失败，已拒绝开启代理",
        E3002 => "代理已开启但外网不通",
        E3003 => "代理设置写入被拒绝（可能需要管理员权限）",
        E3004 => "已通知系统配置变更失败（设置已生效，仅即时性打折）",
        E4001 => "网卡不存在或已断开",
        E4002 => "地址格式非法",
        E4003 => "DNS 写入后回读不一致（可能被组策略限制）",
        E4004 => "当前网卡无 IPv6 通路",
        E5001 => "该组合已被其他程序占用",
        E5002 => "该组合被系统保留",
        E5003 => "全局热键被安全软件拦截",
        E5004 => "快捷键组合无法识别（需要至少一个 Ctrl/Alt/Shift/Win 修饰键，后面跟一个主键）",
        E6001 => "订阅主源与镜像均不可达（已保留本地内容，未清空）",
        E6002 => "订阅内容校验失败",
        E6003 => "订阅条目数超上限",
        E9001 => "配置文件由更新版本创建，已以只读模式启动",
        E9002 => "配置文件损坏或迁移失败，已以只读模式启动",
        E0000 => "内部错误",
        _ => "内部错误",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl AppError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            detail: None,
        }
    }

    /// 用标准文案构造
    pub fn coded(code: &str) -> Self {
        Self {
            code: code.to_string(),
            message: text(code).to_string(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        Self {
            code: E0000.to_string(),
            message: "内部错误".to_string(),
            detail: Some(detail.into()),
        }
    }

    /// 提权子进程退出码（设计方案 §1.4.10）
    pub fn exit_code(&self) -> i32 {
        match self.code.as_str() {
            E1004 => 2,
            E1002 => 3,
            E2001 => 4,
            E2004 | E4003 => 5,
            E1003 => 7,
            _ => 6,
        }
    }

    pub fn code_for_exit_code(c: i32) -> &'static str {
        match c {
            0 => "",
            2 => E1004,
            3 => E1002,
            4 => E2001,
            5 => E2004,
            7 => E1003,
            _ => "E0000",
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.detail {
            Some(d) => write!(f, "[{}] {}（{}）", self.code, self.message, d),
            None => write!(f, "[{}] {}", self.code, self.message),
        }
    }
}

impl std::error::Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::internal(e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::internal(format!("JSON 解析失败: {e}"))
    }
}

impl From<zip::result::ZipError> for AppError {
    fn from(e: zip::result::ZipError) -> Self {
        AppError::internal(format!("压缩包写入失败: {e}"))
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::internal(e.to_string())
    }
}

/// 统一返回类型：前端拿到 { code, message, detail }
pub type Result<T> = std::result::Result<T, AppError>;

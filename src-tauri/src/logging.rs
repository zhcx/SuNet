//! 日志：按天切分 + 脱敏 + 内存环形缓冲（设计方案 §14.1）

use log::{LevelFilter, Log, Metadata, Record};
use serde::Serialize;
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

const RECENT_CAP: usize = 800;

#[derive(Serialize, Clone, Debug)]
pub struct LogRecord {
    pub ts: String,
    pub level: String,
    pub target: String,
    pub message: String,
}

struct Inner {
    file: Option<File>,
    file_date: String,
    recent: VecDeque<LogRecord>,
}

pub struct SuNetLogger {
    dir: PathBuf,
    level: AtomicU8,
    redact: AtomicBool,
    inner: Mutex<Inner>,
}

static LOGGER: OnceLock<SuNetLogger> = OnceLock::new();

fn level_to_u8(l: LevelFilter) -> u8 {
    match l {
        LevelFilter::Off => 0,
        LevelFilter::Error => 1,
        LevelFilter::Warn => 2,
        LevelFilter::Info => 3,
        LevelFilter::Debug => 4,
        LevelFilter::Trace => 5,
    }
}

impl Log for SuNetLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() as u8 <= self.level.load(Ordering::Relaxed)
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let ts = chrono::Local::now()
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string();
        let level = record.level().to_string();
        let target = record.target().to_string();
        let raw = format!("{}", record.args());
        // 脱敏：日志文件与界面读到的是同一份结果，避免"界面干净、文件带内网拓扑"
        let message = if self.redact.load(Ordering::Relaxed) {
            redact_text(&raw)
        } else {
            raw
        };
        let line = format!("{ts} [{level:<5}] {target}: {message}\n");

        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        if inner.file.is_none() || inner.file_date != today {
            let path = self.dir.join(format!("sunet-{today}.log"));
            if let Ok(f) = OpenOptions::new().create(true).append(true).open(&path) {
                inner.file = Some(f);
                inner.file_date = today.clone();
            }
        }
        if let Some(f) = inner.file.as_mut() {
            let _ = f.write_all(line.as_bytes());
            let _ = f.flush();
        }
        if inner.recent.len() >= RECENT_CAP {
            inner.recent.pop_front();
        }
        inner.recent.push_back(LogRecord {
            ts,
            level,
            target,
            message,
        });
    }

    fn flush(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(f) = inner.file.as_mut() {
                let _ = f.flush();
            }
        }
    }
}

pub fn init(dir: &Path, level: LevelFilter, redact: bool, keep_days: u32) {
    let _ = std::fs::create_dir_all(dir);
    if LOGGER.get().is_none() {
        let logger = SuNetLogger {
            dir: dir.to_path_buf(),
            level: AtomicU8::new(level_to_u8(level)),
            redact: AtomicBool::new(redact),
            inner: Mutex::new(Inner {
                file: None,
                file_date: String::new(),
                recent: VecDeque::with_capacity(RECENT_CAP),
            }),
        };
        let _ = LOGGER.set(logger);
        if let Some(l) = LOGGER.get() {
            let _ = log::set_logger(l);
        }
    }
    log::set_max_level(level);
    set_level(level);
    set_redact(redact);
    prune_old(dir, keep_days);
}

pub fn set_level(level: LevelFilter) {
    if let Some(l) = LOGGER.get() {
        l.level.store(level_to_u8(level), Ordering::Relaxed);
    }
    log::set_max_level(level);
}

pub fn set_redact(enabled: bool) {
    if let Some(l) = LOGGER.get() {
        l.redact.store(enabled, Ordering::Relaxed);
    }
}

pub fn is_redacting() -> bool {
    LOGGER
        .get()
        .map(|l| l.redact.load(Ordering::Relaxed))
        .unwrap_or(true)
}

pub fn parse_level(s: &str) -> LevelFilter {
    match s.to_ascii_lowercase().as_str() {
        "error" => LevelFilter::Error,
        "warn" => LevelFilter::Warn,
        "debug" => LevelFilter::Debug,
        "trace" => LevelFilter::Trace,
        "off" => LevelFilter::Off,
        _ => LevelFilter::Info,
    }
}

pub fn recent(limit: usize) -> Vec<LogRecord> {
    match LOGGER.get() {
        Some(l) => match l.inner.lock() {
            Ok(inner) => {
                let n = inner.recent.len();
                let skip = n.saturating_sub(limit);
                inner.recent.iter().skip(skip).cloned().collect()
            }
            Err(_) => Vec::new(),
        },
        None => Vec::new(),
    }
}

/// 导出最近 N 天日志原文（诊断包用，§14.2）
pub fn read_recent_files(dir: &Path, days: u32) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let today = chrono::Local::now().date_naive();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("sunet-") || !name.ends_with(".log") {
                continue;
            }
            let date_part = name.trim_start_matches("sunet-").trim_end_matches(".log");
            let Ok(d) = chrono::NaiveDate::parse_from_str(date_part, "%Y-%m-%d") else {
                continue;
            };
            let age = (today - d).num_days();
            if !(0..days as i64).contains(&age) {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                out.push((name, content));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub fn prune(dir: &Path, keep_days: u32) {
    prune_old(dir, keep_days);
}

fn prune_old(dir: &Path, keep_days: u32) {
    let today = chrono::Local::now().date_naive();
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("sunet-") || !name.ends_with(".log") {
            continue;
        }
        let date_part = name.trim_start_matches("sunet-").trim_end_matches(".log");
        if let Ok(d) = chrono::NaiveDate::parse_from_str(date_part, "%Y-%m-%d") {
            if (today - d).num_days() >= keep_days.max(1) as i64 {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 脱敏规则（§14.1）
// ---------------------------------------------------------------------------

struct Redact {
    private_v4: regex::Regex,
    local_v6: regex::Regex,
    local_name: regex::Regex,
    user_path: regex::Regex,
    url_query: regex::Regex,
}

static REDACT: OnceLock<Redact> = OnceLock::new();

fn rx() -> &'static Redact {
    REDACT.get_or_init(|| Redact {
        private_v4: regex::Regex::new(
            r"\b(?:10\.\d{1,3}|172\.(?:1[6-9]|2\d|3[01])|192\.168|169\.254)\.\d{1,3}\.\d{1,3}\b",
        )
        .unwrap(),
        local_v6: regex::Regex::new(r"(?i)\b(?:fe80|fc|fd)[0-9a-f:]*\b").unwrap(),
        local_name: regex::Regex::new(r"(?i)\b[a-z0-9_-]{1,63}\.(?:local|lan|internal|corp)\b")
            .unwrap(),
        user_path: regex::Regex::new(r"(?i)([a-z]:\\users\\)([^\\]+)").unwrap(),
        url_query: regex::Regex::new(r"\?[^#\s]*").unwrap(),
    })
}

/// 代理地址：保留端口（排查常用），主机部分打码
pub fn redact_host_port(_host: &str, port: u16) -> String {
    format!("***:{port}")
}

/// 订阅 URL：query 段（部分私有源会带凭据）整段打码
pub fn redact_url(u: &str) -> String {
    rx().url_query.replace(u, "?<redacted>").to_string()
}

/// 文本级脱敏：内网地址、主机名后缀、用户目录
pub fn redact_text(s: &str) -> String {
    let r = rx();
    let out = r.private_v4.replace_all(s, "***.***.***.***");
    let out = r.local_v6.replace_all(&out, "***");
    let out = r.local_name.replace_all(&out, "***");
    let out = r.user_path.replace_all(&out, "$1<user>");
    out.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_private_and_path() {
        let t = redact_text("hosts 里的 192.168.1.2 与 10.0.0.5，路径 C:\\Users\\bob\\x");
        assert!(!t.contains("192.168.1.2"));
        assert!(!t.contains("10.0.0.5"));
        assert!(t.contains("C:\\Users\\<user>\\x"));
    }

    #[test]
    fn redact_url_query() {
        assert_eq!(
            redact_url("https://example.com/a?token=abc#frag"),
            "https://example.com/a?<redacted>#frag"
        );
    }

    #[test]
    fn public_ip_untouched() {
        assert!(redact_text("1.1.1.1 与 8.8.8.8").contains("1.1.1.1"));
    }
}

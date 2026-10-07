//! Hosts 文件读写：托管区块 + 编码兜底 + 原子替换 + 回读校验（设计方案 §2）

use crate::error::{AppError, Result, E2001, E2002, E2003, E2004};
use crate::os::winapi::{wide, ReplaceFileW, REPLACEFILE_IGNORE_MERGE_ERRORS};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// 托管标记（§0.0 品牌资产清单：使用 SuNet）
pub const BEGIN: &str = "# >>> SuNet BEGIN (managed by SuNet, do not edit this block)";
pub const END: &str = "# <<< SuNet END";
/// 历史暂定名，升级时需清理（§0.0）
const LEGACY_BEGIN: &str = "# >>> NetBox BEGIN";
const LEGACY_END: &str = "# <<< NetBox END";

pub fn hosts_path() -> PathBuf {
    PathBuf::from(r"C:\Windows\System32\drivers\etc\hosts")
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct HostsEntry {
    #[serde(default)]
    pub id: String,
    pub ip: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub comment: String,
    /// 来源：manual | source:<source_id>
    #[serde(default)]
    pub origin: String,
}

fn default_true() -> bool {
    true
}

impl HostsEntry {
    pub fn new(ip: &str, hostname: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            ip: ip.to_string(),
            hostname: hostname.to_string(),
            enabled: true,
            comment: String::new(),
            origin: "manual".into(),
        }
    }

    pub fn hostnames(&self) -> Vec<String> {
        self.hostname
            .split_whitespace()
            .map(|s| s.to_ascii_lowercase())
            .collect()
    }

    pub fn to_line(&self) -> String {
        let body = if self.comment.trim().is_empty() {
            format!("{}\t{}", self.ip, self.hostname)
        } else {
            format!("{}\t{}\t# {}", self.ip, self.hostname, self.comment.trim())
        };
        if self.enabled {
            body
        } else {
            format!("# {body}")
        }
    }
}

/// 托管区块解析结果（保留 head / tail 原文，实现幂等）
#[derive(Debug, Clone)]
pub struct ParsedHosts {
    pub head: String,
    pub block: Vec<String>,
    pub tail: String,
    pub block_present: bool,
    pub legacy_present: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct HostsStats {
    pub path: String,
    pub exists: bool,
    pub block_present: bool,
    pub total_entries: usize,
    pub enabled_entries: usize,
    pub file_lines: usize,
}

#[derive(Serialize, Clone, Debug)]
pub struct HostsWriteReport {
    pub path: String,
    pub total_entries: usize,
    pub enabled_entries: usize,
    pub bytes: usize,
    pub hash_before: String,
    pub hash_after: String,
    pub used_fallback: bool,
    pub elapsed_ms: u64,
}

// ---------------------------------------------------------------------------
// 编码（§2.3 保守优先）
// ---------------------------------------------------------------------------

/// 字节 → 文本。失败返回 E2002 并拒绝写入。
pub fn decode(bytes: &[u8]) -> Result<String> {
    if bytes.is_empty() {
        return Ok(String::new());
    }
    // UTF-8 BOM
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        let (cow, _, had_err) = encoding_rs::UTF_8.decode(&bytes[3..]);
        if had_err {
            return Err(AppError::coded(E2002).with_detail("UTF-8 BOM 之后的字节不是合法 UTF-8"));
        }
        return Ok(cow.to_string());
    }
    // UTF-16LE BOM
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let (cow, _, had_err) = encoding_rs::UTF_16LE.decode(&bytes[2..]);
        if had_err {
            return Err(AppError::coded(E2002).with_detail("UTF-16LE 解码失败"));
        }
        return Ok(cow.to_string());
    }
    // 纯 UTF-8
    if let Ok(s) = std::str::from_utf8(bytes) {
        return Ok(s.to_string());
    }
    // GBK / ANSI 兜底（中文注释场景）
    let (cow, _, had_err) = encoding_rs::GBK.decode(bytes);
    if !had_err {
        return Ok(cow.to_string());
    }
    Err(AppError::coded(E2002))
}

/// 统一换行为 LF，便于按行处理（写出时统一回 CRLF）
fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

fn to_crlf(lines: &[String]) -> String {
    let mut out = String::new();
    for l in lines {
        out.push_str(l);
        out.push_str("\r\n");
    }
    out
}

// ---------------------------------------------------------------------------
// 解析 / 渲染（§2.4 托管区块）
// ---------------------------------------------------------------------------

pub fn parse(text: &str) -> Result<ParsedHosts> {
    let normalized = normalize_newlines(text);
    let lines: Vec<&str> = normalized.split('\n').collect();

    let begin_idx = lines.iter().position(|l| l.trim() == BEGIN);
    let end_idx = lines.iter().position(|l| l.trim() == END);
    let legacy_begin_idx = lines.iter().position(|l| l.trim() == LEGACY_BEGIN);
    let legacy_end_idx = lines.iter().position(|l| l.trim() == LEGACY_END);

    // 有 BEGIN 无 END（或相反）→ E2003，不猜
    match (begin_idx, end_idx) {
        (Some(_), None) => {
            return Err(AppError::coded(E2003).with_detail("找到 BEGIN 但没有 END"));
        }
        (None, Some(_)) => {
            return Err(AppError::coded(E2003).with_detail("找到 END 但没有 BEGIN"));
        }
        _ => {}
    }
    if legacy_begin_idx.is_some() != legacy_end_idx.is_some() {
        return Err(AppError::coded(E2003).with_detail("旧 NetBox 标记不成对"));
    }
    if let (Some(b), Some(e)) = (begin_idx, end_idx) {
        if b > e {
            return Err(AppError::coded(E2003).with_detail("BEGIN 出现在 END 之后"));
        }
    }
    if let (Some(b), Some(e)) = (legacy_begin_idx, legacy_end_idx) {
        if b > e {
            return Err(AppError::coded(E2003).with_detail("旧 NetBox 标记顺序颠倒"));
        }
    }
    // 新区块与旧区块不得同时存在
    if begin_idx.is_some() && legacy_begin_idx.is_some() {
        return Err(AppError::coded(E2003)
            .with_detail("同时存在 SuNet 与旧 NetBox 区块，请先手动清理一个"));
    }

    let (head, block, tail, present, legacy_present) = if let (Some(b), Some(e)) = (begin_idx, end_idx) {
        let head = lines[..b].join("\n");
        let block: Vec<String> = lines[b + 1..e].iter().map(|s| s.to_string()).collect();
        let tail = lines[e + 1..].join("\n");
        (head, block, tail, true, false)
    } else if let (Some(b), Some(e)) = (legacy_begin_idx, legacy_end_idx) {
        // 旧区块：内容丢弃（升级清理），位置保留为插入点
        let head = lines[..b].join("\n");
        let tail = lines[e + 1..].join("\n");
        (head, Vec::new(), tail, false, true)
    } else {
        (
            normalized.trim_end_matches('\n').to_string(),
            Vec::new(),
            String::new(),
            false,
            false,
        )
    };

    Ok(ParsedHosts {
        head,
        block,
        tail,
        block_present: present,
        legacy_present,
    })
}

/// 渲染：head + 区块 + tail，CRLF 结尾。幂等（parse → render → parse 逐字节相等）。
pub fn render(parsed: &ParsedHosts, lines: &[String]) -> String {
    let mut out_lines: Vec<String> = Vec::new();
    for l in parsed.head.split('\n') {
        out_lines.push(l.to_string());
    }
    // head 末尾若是空串（文件以换行结尾）→ 去掉一个多余空行
    if let Some(last) = out_lines.last() {
        if last.is_empty() {
            out_lines.pop();
        }
    }
    if !out_lines.is_empty() {
        // 与托管区块之间留一个空行
        out_lines.push(String::new());
    }
    out_lines.push(BEGIN.to_string());
    out_lines.extend(lines.iter().cloned());
    out_lines.push(END.to_string());
    for l in parsed.tail.split('\n') {
        out_lines.push(l.to_string());
    }
    while out_lines.last().map(|s| s.is_empty()).unwrap_or(false) {
        out_lines.pop();
    }
    to_crlf(&out_lines)
}

/// 渲染时**删除**整个托管区块（条目为空 = 关闭方案，不是写一个空区块）
///
/// 设计方案 §2.4：关闭方案 = 删除整个区块，而不是注释掉。
pub fn render_without_block(parsed: &ParsedHosts) -> String {
    let mut out_lines: Vec<String> = Vec::new();
    for l in parsed.head.split('\n') {
        out_lines.push(l.to_string());
    }
    if out_lines.last().map(|s| s.is_empty()).unwrap_or(false) {
        out_lines.pop();
    }
    for l in parsed.tail.split('\n') {
        out_lines.push(l.to_string());
    }
    while out_lines.last().map(|s| s.is_empty()).unwrap_or(false) {
        out_lines.pop();
    }
    to_crlf(&out_lines)
}

/// 区块行 → 条目（读取用户/其他工具在区块内改过的内容）
pub fn parse_block_lines(lines: &[String]) -> Vec<HostsEntry> {
    let mut out = Vec::new();
    for raw in lines {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let (enabled, body) = if let Some(rest) = line.strip_prefix('#') {
            (false, rest.trim())
        } else {
            (true, line)
        };
        if body.is_empty() {
            continue;
        }
        // 拆注释
        let (main, comment) = match body.split_once('#') {
            Some((a, b)) => (a.trim(), b.trim().to_string()),
            None => (body, String::new()),
        };
        let mut parts = main.split_whitespace();
        let Some(ip) = parts.next() else { continue };
        let hostname = parts.collect::<Vec<_>>().join(" ");
        if hostname.is_empty() {
            continue;
        }
        if ip.parse::<IpAddr>().is_err() {
            continue;
        }
        out.push(HostsEntry {
            id: uuid::Uuid::new_v4().to_string(),
            ip: ip.to_string(),
            hostname,
            enabled,
            comment,
            origin: "file".into(),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// 读取
// ---------------------------------------------------------------------------

pub fn read_raw() -> Result<(Vec<u8>, bool)> {
    let path = hosts_path();
    match std::fs::read(&path) {
        Ok(b) => Ok((b, true)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Vec::new(), false)),
        Err(e) => Err(AppError::internal(format!("读取 hosts 失败: {e}"))),
    }
}

pub fn read_text() -> Result<String> {
    let (bytes, _) = read_raw()?;
    decode(&bytes)
}

pub fn stats() -> Result<HostsStats> {
    let (bytes, exists) = read_raw()?;
    let text = decode(&bytes)?;
    let parsed = parse(&text)?;
    let entries = parse_block_lines(&parsed.block);
    Ok(HostsStats {
        path: hosts_path().to_string_lossy().to_string(),
        exists,
        block_present: parsed.block_present || parsed.legacy_present,
        total_entries: entries.len(),
        enabled_entries: entries.iter().filter(|e| e.enabled).count(),
        file_lines: normalize_newlines(&text).split('\n').count(),
    })
}

/// 读取当前托管区块内容（用于首次接管导入）
pub fn read_block_entries() -> Result<Vec<HostsEntry>> {
    let text = read_text()?;
    let parsed = parse(&text)?;
    Ok(parse_block_lines(&parsed.block))
}

/// 一次读取同时拿到统计与托管区块条目。
/// get_app_state 每次轮询都要两者；分开调 stats() + read_block_entries()
/// 会把整份 hosts 读三次、解析两次，纯属浪费。
pub fn snapshot() -> Result<(HostsStats, Vec<HostsEntry>)> {
    let (bytes, exists) = read_raw()?;
    let text = decode(&bytes)?;
    let parsed = parse(&text)?;
    let entries = parse_block_lines(&parsed.block);
    let stats = HostsStats {
        path: hosts_path().to_string_lossy().to_string(),
        exists,
        block_present: parsed.block_present || parsed.legacy_present,
        total_entries: entries.len(),
        enabled_entries: entries.iter().filter(|e| e.enabled).count(),
        file_lines: normalize_newlines(&text).split('\n').count(),
    };
    Ok((stats, entries))
}

// ---------------------------------------------------------------------------
// 条目校验（§2.5）
// ---------------------------------------------------------------------------

pub fn validate_ip(ip: &str) -> std::result::Result<IpAddr, String> {
    let trimmed = ip.trim();
    if trimmed.is_empty() {
        return Err("IP 地址不能为空".into());
    }
    trimmed
        .parse::<IpAddr>()
        .map_err(|_| format!("「{trimmed}」不是合法的 IPv4 / IPv6 地址"))
}

/// 域名校验：每段 a-zA-Z0-9-_，不以 - 开头/结尾，总长 ≤ 253
pub fn validate_hostname(hostname: &str) -> std::result::Result<(), String> {
    let names: Vec<&str> = hostname.split_whitespace().collect();
    if names.is_empty() {
        return Err("域名为空".into());
    }
    for name in names {
        if name.contains('*') {
            return Err(format!("不支持通配符：{name}"));
        }
        if name.len() > 253 {
            return Err(format!("域名过长（>253）：{name}"));
        }
        for label in name.split('.') {
            if label.is_empty() {
                return Err(format!("域名存在空段：{name}"));
            }
            if label.len() > 63 {
                return Err(format!("域名段过长（>63）：{name}"));
            }
            if label.starts_with('-') || label.ends_with('-') {
                return Err(format!("域名段不能以连字符开头或结尾：{name}"));
            }
            if !label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(format!("域名含非法字符：{name}"));
            }
        }
        // 纯数字且形如 IP 的，视为误填
        if name.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return Err(format!("{name} 看起来是 IP 地址，不是域名"));
        }
    }
    Ok(())
}

pub fn validate_entry(entry: &HostsEntry) -> Result<()> {
    validate_ip(&entry.ip).map_err(|e| AppError::new("E2005", format!("IP 非法：{e}")))?;
    validate_hostname(&entry.hostname).map_err(|e| AppError::new("E2005", format!("域名非法：{e}")))?;
    if entry.comment.contains(|c| c == '\n' || c == '\r') {
        return Err(AppError::new("E2005", "注释不能包含换行"));
    }
    Ok(())
}

/// 重复 hostname 检测：返回重复的域名列表（同一域名出现多次时首条生效）
pub fn find_duplicates(entries: &[HostsEntry]) -> Vec<String> {
    use std::collections::HashMap;
    let mut seen: HashMap<String, usize> = HashMap::new();
    for e in entries.iter().filter(|e| e.enabled) {
        for h in e.hostnames() {
            *seen.entry(h).or_insert(0) += 1;
        }
    }
    let mut out: Vec<String> = seen
        .into_iter()
        .filter(|(_, c)| *c > 1)
        .map(|(h, _)| h)
        .collect();
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// 写入（§2.2：绝不整文件覆盖）
// ---------------------------------------------------------------------------

fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// 应用托管区块：读 → 改 → 原子替换 → 回读校验 → 刷新缓存
pub fn apply(entries: &[HostsEntry]) -> Result<HostsWriteReport> {
    let started = Instant::now();
    let path = hosts_path();

    for e in entries {
        validate_entry(e)?;
    }

    // 1. 读取 + 解码
    let (raw_before, _existed) = read_raw()?;
    let hash_before = fnv1a_hex(&raw_before);
    let text = decode(&raw_before)?;

    // 2. 结构化编辑：只替换（或整块删除）自己的区块
    let parsed = parse(&text)?;
    let mut lines: Vec<String> = Vec::new();
    for e in entries {
        lines.push(e.to_line());
    }
    let output = if entries.is_empty() {
        // 条目为空 = 关闭方案：整个区块连标记一起删除（§2.4）
        render_without_block(&parsed)
    } else {
        render(&parsed, &lines)
    };

    // 3. 内容无变化时直接返回（幂等：反复 apply 不产生磁盘写入）
    let output_bytes = output.as_bytes();
    if output_bytes == raw_before.as_slice() {
        log::info!(
            target: "hosts",
            "托管区块内容未变化，跳过写入（条目 {} 条）",
            entries.len()
        );
        return Ok(HostsWriteReport {
            path: path.to_string_lossy().to_string(),
            total_entries: entries.len(),
            enabled_entries: entries.iter().filter(|e| e.enabled).count(),
            bytes: output_bytes.len(),
            hash_before: hash_before.clone(),
            hash_after: hash_before,
            used_fallback: false,
            elapsed_ms: started.elapsed().as_millis() as u64,
        });
    }

    // 4. 备份（与 hosts 同目录）
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let backup_path = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join(format!("hosts.bak.{stamp}"));
    let backup_ok = if path.exists() {
        std::fs::copy(&path, &backup_path).is_ok()
    } else {
        true
    };
    if !backup_ok {
        log::warn!(target: "hosts", "备份 hosts 失败，继续写入（备份路径可能无权限）");
    }

    // 5. 写临时文件（必须同卷，保证 ReplaceFileW 原子性）
    let tmp_path = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hosts.tmp");
    {
        let mut f = std::fs::File::create(&tmp_path)
            .map_err(|e| AppError::coded(E2001).with_detail(format!("创建临时文件失败: {e}")))?;
        f.write_all(output_bytes)
            .map_err(|e| AppError::coded(E2001).with_detail(format!("写入临时文件失败: {e}")))?;
        f.flush()?;
    }

    // 6. 原子替换：保留目标原有 ACL 与属性
    let used_fallback = if path.exists() {
        let replaced = unsafe {
            let target = wide(&path.to_string_lossy());
            let replacement = wide(&tmp_path.to_string_lossy());
            ReplaceFileW(
                target.as_ptr(),
                replacement.as_ptr(),
                std::ptr::null(),
                REPLACEFILE_IGNORE_MERGE_ERRORS,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if replaced == 0 {
            let err = crate::os::winapi::last_error_text();
            log::warn!(target: "hosts", "ReplaceFileW 失败（{err}），回退为截断写入");
            // 7. 失败回退
            fallback_write(&path, output_bytes)?;
            true
        } else {
            false
        }
    } else {
        std::fs::rename(&tmp_path, &path).map_err(|e| {
            AppError::coded(E2001).with_detail(format!("重命名临时文件失败: {e}"))
        })?;
        false
    };
    let _ = std::fs::remove_file(&tmp_path);

    // 8. 回读校验：不一致必须报失败，不允许"调用没抛异常就当成成功"
    let after = std::fs::read(&path).map_err(|e| {
        AppError::coded(E2004).with_detail(format!("回读 hosts 失败: {e}"))
    })?;
    let hash_after = fnv1a_hex(&after);
    if after != output_bytes {
        return Err(AppError::coded(E2004).with_detail(format!(
            "写入后回读不一致（期望 {} 字节 / 实际 {} 字节），可能被安全软件或组策略刷回",
            output_bytes.len(),
            after.len()
        )));
    }

    // 9. 刷新解析缓存（dnsapi，不弹黑窗）
    crate::os::dns_client::flush()?;

    let elapsed_ms = started.elapsed().as_millis() as u64;
    log::info!(
        target: "hosts",
        "hosts 写入完成：{} 条（启用 {}），{} 字节，{}-{}，回退路径={}，耗时 {}ms",
        entries.len(),
        entries.iter().filter(|e| e.enabled).count(),
        output_bytes.len(),
        &hash_before[..8],
        &hash_after[..8],
        used_fallback,
        elapsed_ms
    );

    Ok(HostsWriteReport {
        path: path.to_string_lossy().to_string(),
        total_entries: entries.len(),
        enabled_entries: entries.iter().filter(|e| e.enabled).count(),
        bytes: output_bytes.len(),
        hash_before,
        hash_after,
        used_fallback,
        elapsed_ms,
    })
}

fn fallback_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .attributes(FILE_ATTRIBUTE_NORMAL)
        .open(path)
        .map_err(|e| AppError::coded(E2001).with_detail(format!("回退写入打开失败: {e}")))?;
    f.write_all(bytes)
        .map_err(|e| AppError::coded(E2001).with_detail(format!("回退写入失败: {e}")))?;
    f.flush()?;
    Ok(())
}

/// 逐字节还原（回滚用，§6.2：还原追求"逐字节回到原样"）
pub fn restore_raw(bytes: &[u8]) -> Result<()> {
    let path = hosts_path();
    if bytes.is_empty() {
        // 原文件不存在 → 移除托管区块（写出空文件，保留文件本身以避免误删系统文件）
        let empty: &[u8] = b"";
        return write_bytes_exact(&path, empty);
    }
    write_bytes_exact(&path, bytes)
}

fn write_bytes_exact(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hosts.tmp");
    {
        let mut f = std::fs::File::create(&tmp)
            .map_err(|e| AppError::coded(E2001).with_detail(format!("创建临时文件失败: {e}")))?;
        f.write_all(bytes)?;
        f.flush()?;
    }
    let ok = unsafe {
        let target = wide(&path.to_string_lossy());
        let replacement = wide(&tmp.to_string_lossy());
        ReplaceFileW(
            target.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_IGNORE_MERGE_ERRORS,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        fallback_write(path, bytes)?;
    }
    let _ = std::fs::remove_file(&tmp);
    let after = std::fs::read(path)?;
    if after != bytes {
        return Err(AppError::coded(E2004).with_detail("还原后回读不一致"));
    }
    crate::os::dns_client::flush()?;
    Ok(())
}

/// 生成将要写入的托管区块预览文本（§9：写前预览是必做项）
pub fn preview(entries: &[HostsEntry]) -> Result<String> {
    let mut out = String::new();
    out.push_str(BEGIN);
    out.push_str("\r\n");
    for e in entries {
        out.push_str(&e.to_line());
        out.push_str("\r\n");
    }
    out.push_str(END);
    out.push_str("\r\n");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_render_idempotent() {
        let original =
            "# 本地注释\r\n127.0.0.1 localhost\r\n\r\n# >>> SuNet BEGIN (managed by SuNet, do not edit this block)\r\n1.1.1.1 a.com\r\n# <<< SuNet END\r\n# 尾部注释\r\n";
        let parsed = parse(original).unwrap();
        assert!(parsed.block_present);
        assert_eq!(parsed.block.len(), 1);
        let rendered = render(&parsed, &parsed.block.clone());
        let reparsed = parse(&rendered).unwrap();
        let rendered2 = render(&reparsed, &reparsed.block.clone());
        assert_eq!(rendered, rendered2, "parse → render → parse 必须幂等");
        // 区块内原有行原样保留（不重新排版），这是幂等的前提
        assert!(rendered.contains("1.1.1.1 a.com"));
        assert!(rendered.contains("# 尾部注释"));
        assert!(rendered.contains("# 本地注释"));
        assert!(rendered.ends_with("\r\n"));
    }

    #[test]
    fn empty_entries_remove_block_entirely() {
        let original = "# 头部\r\n1.2.3.4 x.com\r\n\r\n# >>> SuNet BEGIN (managed by SuNet, do not edit this block)\r\n1.1.1.1 a.com\r\n# <<< SuNet END\r\n# 尾部\r\n";
        let parsed = parse(original).unwrap();
        let out = render_without_block(&parsed);
        assert!(!out.contains("SuNet BEGIN"), "关闭方案必须整块删除标记");
        assert!(!out.contains("SuNet END"));
        assert!(out.contains("1.2.3.4 x.com"), "区块外内容必须保留");
        assert!(out.contains("# 头部") && out.contains("# 尾部"));
        // 幂等：删除后再次解析，区块不存在
        assert!(!parse(&out).unwrap().block_present);
    }

    #[test]
    fn legacy_netbox_block_is_cleaned() {
        let original = "# >>> NetBox BEGIN\r\n1.1.1.1 old.com\r\n# <<< NetBox END\r\n127.0.0.1 localhost\r\n";
        let parsed = parse(original).unwrap();
        assert!(parsed.legacy_present);
        let out = render(&parsed, &["2.2.2.2 new.com".to_string()]);
        assert!(!out.contains("NetBox"));
        assert!(out.contains("2.2.2.2 new.com"));
        assert!(out.contains("127.0.0.1 localhost"));
    }

    #[test]
    fn begin_without_end_is_error() {
        let text = "# >>> SuNet BEGIN (managed by SuNet, do not edit this block)\r\n1.1.1.1 a.com\r\n";
        let err = parse(text).unwrap_err();
        assert_eq!(err.code, E2003);
    }

    #[test]
    fn decode_variants() {
        assert!(decode(b"127.0.0.1 x\r\n").is_ok());
        let bom = decode(&[0xEF, 0xBB, 0xBF, b'a']).unwrap();
        assert_eq!(bom, "a");
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .iter()
            .copied()
            .chain("中文".encode_utf16().flat_map(|u| u.to_le_bytes()))
            .collect();
        assert_eq!(decode(&utf16).unwrap(), "中文");
        let gbk = encoding_rs::GBK.encode("中文注释").0;
        assert_eq!(decode(&gbk).unwrap(), "中文注释");
    }

    #[test]
    fn validation_rules() {
        assert!(validate_ip("1.1.1.1").is_ok());
        assert!(validate_ip("256.1.1.1").is_err());
        assert!(validate_ip("2606:4700:4700::1111").is_ok());
        assert!(validate_hostname("a.com b.com").is_ok());
        assert!(validate_hostname("*.com").is_err());
        assert!(validate_hostname("-bad.com").is_err());
        assert!(validate_hostname("1.1.1.1").is_err());
        let e = HostsEntry::new("1.1.1.1", "a.com");
        assert!(validate_entry(&e).is_ok());
        let mut bad = e.clone();
        bad.comment = "x\ny".into();
        assert!(validate_entry(&bad).is_err());
    }

    #[test]
    fn duplicates_detected() {
        let a = HostsEntry::new("1.1.1.1", "x.com");
        let mut b = HostsEntry::new("2.2.2.2", "x.com");
        b.enabled = false;
        let dup = find_duplicates(&[a, b]);
        assert!(dup.is_empty(), "禁用条目不参与重复判定");
        let c = HostsEntry::new("2.2.2.2", "x.com");
        assert_eq!(find_duplicates(&[HostsEntry::new("1.1.1.1", "x.com"), c]), vec!["x.com"]);
    }

    #[test]
    fn disabled_entry_written_as_comment() {
        let mut e = HostsEntry::new("1.1.1.1", "a.com");
        e.enabled = false;
        assert!(e.to_line().starts_with("# 1.1.1.1"));
        let parsed = parse_block_lines(&[e.to_line()]);
        assert_eq!(parsed.len(), 1);
        assert!(!parsed[0].enabled);
    }
}

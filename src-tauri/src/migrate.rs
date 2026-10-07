//! 配置 schema 迁移（设计方案 §15.2）
//!
//! 三条原则：
//!   1. 迁移函数必须**纯函数、可单元测试**（输入旧 JSON，输出新 JSON），不依赖系统状态
//!   2. 迁移**只增不减**：宁可留下废弃字段，也不要删用户数据
//!   3. `schema_version > 当前` 时绝不"向下兼容猜测"——猜错的代价是用户丢失全部方案
//!
//! 内置 DNS 清单不进配置文件（§7.1），因此修正地址不需要任何迁移。

use crate::error::{AppError, Result, E9001, E9002};
use serde_json::{json, Value};

pub const CURRENT_SCHEMA: u32 = 1;

type Migration = fn(Value) -> Result<Value>;

/// 逐级迁移表：v(n) → v(n+1)
const MIGRATIONS: &[(u32, Migration)] = &[(0, migrate_v0_to_v1)];

/// v0 → v1：早期版本没有 schema_version，也缺少 settings / hosts_entries 等顶层字段。
/// 这里只补齐结构，不动已有数据。
fn migrate_v0_to_v1(mut v: Value) -> Result<Value> {
    let obj = v
        .as_object_mut()
        .ok_or_else(|| AppError::coded(E9002).with_detail("配置根节点不是 JSON 对象"))?;
    obj.insert("schema_version".into(), json!(1));
    if !obj.contains_key("settings") {
        obj.insert("settings".into(), json!({}));
    }
    if !obj.contains_key("profiles") {
        obj.insert("profiles".into(), json!([]));
    }
    if !obj.contains_key("hosts_entries") {
        obj.insert("hosts_entries".into(), json!([]));
    }
    if !obj.contains_key("source_entries") {
        obj.insert("source_entries".into(), json!([]));
    }
    if !obj.contains_key("sources") {
        obj.insert("sources".into(), json!([]));
    }
    if !obj.contains_key("channels") {
        obj.insert("channels".into(), json!([]));
    }
    Ok(v)
}

/// 对配置 JSON 执行迁移（纯函数，可单测）
pub fn migrate_value(value: Value) -> Result<(u32, Value)> {
    let mut version = value
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    if version > CURRENT_SCHEMA {
        return Err(AppError::coded(E9001).with_detail(format!(
            "配置文件 schema_version = {version}，本程序支持到 {CURRENT_SCHEMA}"
        )));
    }

    let mut current = value;
    while version < CURRENT_SCHEMA {
        let step = MIGRATIONS.iter().find(|(from, _)| *from == version);
        let Some((_, f)) = step else {
            return Err(AppError::coded(E9002)
                .with_detail(format!("缺少 v{version} → v{} 的迁移函数", version + 1)));
        };
        current = f(current)?;
        version += 1;
    }
    Ok((version, current))
}

pub fn backup_path_for(from_version: u32) -> std::path::PathBuf {
    crate::paths::appdata_dir().join(format!("config.v{from_version}.bak.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v0_migrates_to_current() {
        let old = json!({
            "profiles": [{ "id": "x", "name": "旧方案" }],
            "active_profile_id": "x"
        });
        let (v, out) = migrate_value(old).unwrap();
        assert_eq!(v, CURRENT_SCHEMA);
        assert_eq!(out["schema_version"], json!(1));
        assert!(out["settings"].is_object());
        assert!(out["hosts_entries"].is_array());
        // 只增不减：原有数据必须保留
        assert_eq!(out["profiles"][0]["name"], json!("旧方案"));
        assert_eq!(out["active_profile_id"], json!("x"));
    }

    #[test]
    fn newer_schema_is_refused() {
        let future = json!({ "schema_version": CURRENT_SCHEMA + 5 });
        let err = migrate_value(future).unwrap_err();
        assert_eq!(err.code, E9001);
    }

    #[test]
    fn current_schema_is_passthrough() {
        let now = json!({ "schema_version": CURRENT_SCHEMA, "a": 1 });
        let (v, out) = migrate_value(now).unwrap();
        assert_eq!(v, CURRENT_SCHEMA);
        assert_eq!(out["a"], json!(1));
    }
}

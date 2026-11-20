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

pub const CURRENT_SCHEMA: u32 = 2;

type Migration = fn(Value) -> Result<Value>;

/// 逐级迁移表：v(n) → v(n+1)
const MIGRATIONS: &[(u32, Migration)] = &[(0, migrate_v0_to_v1), (1, migrate_v1_to_v2)];

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

/// v1 → v2：内置订阅源扩容（新增 GitHub520、ineo6 hosts）。
/// 只按 id 追加缺失的内置源，不覆盖用户已编辑的同类源、也不复活用户删掉的内置源
/// （删除的源在同一次升到 v2 时若不存在仍会被补一次，属预期：升级即恢复出厂内置项）。
fn migrate_v1_to_v2(mut v: Value) -> Result<Value> {
    let obj = v
        .as_object_mut()
        .ok_or_else(|| AppError::coded(E9002).with_detail("配置根节点不是 JSON 对象"))?;
    let mut sources: Vec<Value> = match obj.remove("sources") {
        Some(Value::Array(a)) => a,
        _ => Vec::new(),
    };
    let existing: std::collections::HashSet<String> = sources
        .iter()
        .filter_map(|s| s.get("id").and_then(|i| i.as_str()).map(|x| x.to_string()))
        .collect();
    for builtin in crate::config::builtin_sources() {
        if !existing.contains(&builtin.id) {
            let val = serde_json::to_value(&builtin)
                .map_err(|e| AppError::coded(E9002).with_detail(format!("内置源序列化失败：{e}")))?;
            sources.push(val);
        }
    }
    obj.insert("sources".into(), Value::Array(sources));
    obj.insert("schema_version".into(), json!(2));
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
        assert_eq!(out["schema_version"], json!(2));
        assert!(out["settings"].is_object());
        assert!(out["hosts_entries"].is_array());
        // 只增不减：原有数据必须保留
        assert_eq!(out["profiles"][0]["name"], json!("旧方案"));
        assert_eq!(out["active_profile_id"], json!("x"));
        // v1→v2 补入内置订阅源
        let ids: Vec<String> = out["sources"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|s| s.get("id").and_then(|i| i.as_str()).map(|x| x.to_string()))
            .collect();
        assert!(ids.contains(&"src-github".to_string()));
        assert!(ids.contains(&"src-github520".to_string()));
        assert!(ids.contains(&"src-ineo6".to_string()));
    }

    #[test]
    fn v1_to_v2_keeps_user_sources_and_appends_builtins() {
        let old = json!({
            "schema_version": 1,
            "sources": [
                { "id": "src-github", "name": "我改过的名字", "url": "https://x/y", "interval_hours": 6, "enabled": false },
                { "id": "user-custom", "name": "自定义", "url": "https://a/b", "interval_hours": 12, "enabled": true }
            ]
        });
        let (v, out) = migrate_value(old).unwrap();
        assert_eq!(v, 2);
        let arr = out["sources"].as_array().unwrap();
        // 用户已有源原样保留（不覆盖名字/间隔/启用状态）
        let gh = arr.iter().find(|s| s["id"] == json!("src-github")).unwrap();
        assert_eq!(gh["name"], json!("我改过的名字"));
        assert_eq!(gh["interval_hours"], json!(6));
        assert_eq!(gh["enabled"], json!(false));
        assert!(arr.iter().any(|s| s["id"] == json!("user-custom")));
        // 补齐缺失的内置源
        assert!(arr.iter().any(|s| s["id"] == json!("src-github520")));
        assert!(arr.iter().any(|s| s["id"] == json!("src-ineo6")));
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

//! 数据库完整性检查与安全修复。
//!
//! 自动修复只处理可以从其他数据重新推导的内容（全文索引、节点路径、排序键、名称索引）
//! 以及父节点丢失的孤儿节点（移到根级）。SQLite 本身损坏时不尝试修复，提示从备份还原。

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::vacuum_into;
use crate::entity::Entity;
use crate::error::Result;
use crate::model::{Author, Node};
use crate::util::{is_valid_sort_key, key_between, now_ms};
use crate::world::World;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct IntegrityIssue {
    pub code: String,
    pub message: String,
    /// 能否用"安全修复"自动修好。
    pub repairable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct IntegrityReport {
    pub ok: bool,
    pub issues: Vec<IntegrityIssue>,
    pub checked_at: i64,
}

fn issue(code: &str, message: String, repairable: bool) -> IntegrityIssue {
    IntegrityIssue { code: code.into(), message, repairable }
}

fn fts_ok(world: &World, table: &str) -> bool {
    world.conn().execute(&format!("INSERT INTO {table}({table}, rank) VALUES('integrity-check', 1)"), []).is_ok()
}

pub fn check(world: &World) -> Result<IntegrityReport> {
    let conn = world.conn();
    let mut issues = vec![];

    let rows: Vec<String> = {
        let mut stmt = conn.prepare("PRAGMA integrity_check(20)")?;
        stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    if rows != ["ok"] {
        issues.push(issue("sqlite_corrupt", format!("数据库文件损坏：{}。请从备份还原", rows.join("；")), false));
        // 底层已损坏时，后续检查的结果不可靠。
        return Ok(IntegrityReport { ok: false, issues, checked_at: now_ms() });
    }
    // 节点父链断裂单独作为"孤儿节点"报告（可修复），这里不重复计入。
    let fk: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_foreign_key_check WHERE NOT (\"table\" = 'nodes' AND parent = 'nodes')",
        [],
        |r| r.get(0),
    )?;
    if fk > 0 {
        issues.push(issue("foreign_key", format!("{fk} 条记录引用了不存在的数据"), false));
    }
    for (table, label) in [("nodes_fts", "世界树"), ("script_items_fts", "剧本条目")] {
        if !fts_ok(world, table) {
            issues.push(issue("fts", format!("{label}的全文索引与数据不一致（搜索结果可能不准）"), true));
        }
    }
    let orphans: i64 = conn.query_row(
        "SELECT COUNT(*) FROM nodes c LEFT JOIN nodes p ON p.id = c.parent_id WHERE c.parent_id IS NOT NULL AND p.id IS NULL",
        [],
        |r| r.get(0),
    )?;
    if orphans > 0 {
        issues.push(issue("orphan_nodes", format!("{orphans} 个节点的父节点不存在"), true));
    }
    let bad_paths: i64 = conn.query_row(
        "SELECT COUNT(*) FROM nodes c LEFT JOIN nodes p ON p.id = c.parent_id
         WHERE c.path <> COALESCE(p.path, '/') || c.id || '/'",
        [],
        |r| r.get(0),
    )?;
    if bad_paths > 0 {
        issues.push(issue("node_paths", format!("{bad_paths} 个节点的层级路径不正确（影响子树查询）"), true));
    }
    let mut bad_keys = 0;
    for table in ["nodes", "script_items"] {
        let mut stmt = conn.prepare(&format!("SELECT sort_key FROM {table}"))?;
        for k in stmt.query_map([], |r| r.get::<_, String>(0))? {
            if !is_valid_sort_key(&k?) {
                bad_keys += 1;
            }
        }
    }
    if bad_keys > 0 {
        issues.push(issue("sort_keys", format!("{bad_keys} 条记录的排序键无效（影响显示顺序）"), true));
    }
    let missing_names: i64 = conn.query_row(
        "SELECT COUNT(*) FROM nodes n WHERE NOT EXISTS (SELECT 1 FROM node_names k WHERE k.node_id = n.id)",
        [],
        |r| r.get(0),
    )?;
    if missing_names > 0 {
        issues.push(issue("name_index", format!("{missing_names} 个节点缺少名称索引（影响查重与按名称查找）"), true));
    }
    let mut bad_json = 0;
    {
        let mut stmt = conn.prepare("SELECT ops FROM changesets")?;
        for ops in stmt.query_map([], |r| r.get::<_, String>(0))? {
            if serde_json::from_str::<Vec<crate::changeset::Op>>(&ops?).is_err() {
                bad_json += 1;
            }
        }
    }
    if bad_json > 0 {
        issues.push(issue("changeset_json", format!("{bad_json} 个变更集的内容无法解析"), false));
    }
    Ok(IntegrityReport { ok: issues.is_empty(), issues, checked_at: now_ms() })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct RepairReport {
    /// 修复前的备份文件名。
    pub backup: String,
    pub actions: Vec<String>,
    pub after: IntegrityReport,
}

/// 安全修复。修复前先备份；孤儿节点移到根级（记录修订；不能撤销回断链状态），其余为派生数据重建。
pub fn repair(world: &mut World) -> Result<RepairReport> {
    let stem = world.path().file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let backup = format!("{stem}-before-repair-{}.cgw", now_ms());
    vacuum_into(world.conn(), &world.backup_dir().join(&backup))?;
    let before = check(world)?;
    let has = |code: &str| before.issues.iter().any(|i| i.code == code && i.repairable);
    let mut actions = vec![];

    // 先重建全文索引：后续修改会触发索引同步，索引坏着时会失败。
    if before.issues.iter().any(|i| i.code == "fts") {
        world.conn().execute_batch(
            "INSERT INTO nodes_fts(nodes_fts) VALUES('rebuild'); INSERT INTO script_items_fts(script_items_fts) VALUES('rebuild');",
        )?;
        actions.push("重建了全文索引".into());
    }

    if has("orphan_nodes") {
        let orphans: Vec<Node> = {
            let mut stmt = world.conn().prepare(
                "SELECT c.* FROM nodes c LEFT JOIN nodes p ON p.id = c.parent_id WHERE c.parent_id IS NOT NULL AND p.id IS NULL",
            )?;
            stmt.query_map([], Node::from_row)?.collect::<rusqlite::Result<_>>()?
        };
        let n = orphans.len();
        world.write(&Author::System, "修复：父节点丢失的节点移到根级", |w| {
            for o in orphans {
                let mut fixed = o.clone();
                fixed.parent_id = None;
                fixed.path = format!("/{}/", o.id);
                w.put_update(&o, fixed)?;
            }
            Ok(())
        })?;
        actions.push(format!("{n} 个孤儿节点移到根级"));
    }
    if has("node_paths") || has("orphan_nodes") {
        // 自上而下重算所有路径（派生数据）。
        let tx = world.conn.transaction()?;
        let n = tx.execute(
            "WITH RECURSIVE t(id, path) AS (
                SELECT id, '/' || id || '/' FROM nodes WHERE parent_id IS NULL
                UNION ALL
                SELECT n.id, t.path || n.id || '/' FROM nodes n JOIN t ON n.parent_id = t.id
             )
             UPDATE nodes SET path = (SELECT path FROM t WHERE t.id = nodes.id)
             WHERE path <> (SELECT path FROM t WHERE t.id = nodes.id)",
            [],
        )?;
        tx.commit()?;
        actions.push(format!("重算了 {n} 个节点的层级路径"));
    }
    if has("sort_keys") {
        let tx = world.conn.transaction()?;
        let mut fixed = 0;
        for table in ["nodes", "script_items"] {
            let parents: Vec<Option<String>> = {
                let mut stmt = tx.prepare(&format!("SELECT DISTINCT parent_id FROM {table}"))?;
                stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
            };
            for p in parents {
                let rows: Vec<(String, String)> = {
                    let mut stmt = tx.prepare(&format!("SELECT id, sort_key FROM {table} WHERE parent_id IS ?1 ORDER BY sort_key, created_at"))?;
                    stmt.query_map([&p], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
                };
                if rows.iter().all(|(_, k)| is_valid_sort_key(k)) {
                    continue;
                }
                // 按原有顺序重新分配有效的排序键。
                let mut prev: Option<String> = None;
                for (id, _) in rows {
                    let k = key_between(prev.as_deref(), None);
                    tx.execute(&format!("UPDATE {table} SET sort_key = ?2 WHERE id = ?1"), params![id, k])?;
                    prev = Some(k);
                    fixed += 1;
                }
            }
        }
        tx.commit()?;
        actions.push(format!("重新分配了 {fixed} 条记录的排序键"));
    }
    if has("name_index") {
        let ids: Vec<String> = {
            let mut stmt = world.conn().prepare("SELECT id FROM nodes n WHERE NOT EXISTS (SELECT 1 FROM node_names k WHERE k.node_id = n.id)")?;
            stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
        };
        for id in &ids {
            if let Some(n) = Node::load(world.conn(), id)? {
                crate::entity::sync_names(world.conn(), &n)?;
            }
        }
        actions.push(format!("重建了 {} 个节点的名称索引", ids.len()));
    }
    let after = check(world)?;
    Ok(RepairReport { backup, actions, after })
}

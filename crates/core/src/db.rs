//! 数据库打开与迁移。
//!
//! 迁移规则：
//! - 版本号存于 `PRAGMA user_version`。
//! - 已有数据的库在升级前先用 `VACUUM INTO` 备份到备份目录。
//! - 每个迁移在独立事务中执行，失败即回滚，库保持在上一个版本，备份保留。
//! - 库版本高于程序支持的版本时拒绝打开，不做任何修改。

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::error::{CoreError, Result};
use crate::util::now_ms;

pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

pub const WORLD_MIGRATIONS: &[Migration] = &[
    Migration { version: 1, name: "init", sql: include_str!("../migrations/world/0001_init.sql") },
    Migration { version: 2, name: "script_search", sql: include_str!("../migrations/world/0002_script_search.sql") },
    Migration { version: 3, name: "relation_indexes", sql: include_str!("../migrations/world/0003_relation_indexes.sql") },
    Migration { version: 4, name: "campaign_memory", sql: include_str!("../migrations/world/0004_campaign_memory.sql") },
    Migration { version: 5, name: "core_canon_index", sql: include_str!("../migrations/world/0005_core_canon_index.sql") },
    Migration { version: 6, name: "timelines", sql: include_str!("../migrations/world/0006_timelines.sql") },
    Migration { version: 7, name: "bonds", sql: include_str!("../migrations/world/0007_bonds.sql") },
    Migration { version: 8, name: "npc_minds_records", sql: include_str!("../migrations/world/0008_npc_minds_records.sql") },
    Migration { version: 9, name: "node_images", sql: include_str!("../migrations/world/0009_node_images.sql") },
    Migration { version: 10, name: "image_thumbs", sql: include_str!("../migrations/world/0010_image_thumbs.sql") },
    Migration { version: 11, name: "game_profile", sql: include_str!("../migrations/world/0011_game_profile.sql") },
    Migration { version: 12, name: "script_versions", sql: include_str!("../migrations/world/0012_script_versions.sql") },
    Migration { version: 13, name: "summary_compression", sql: include_str!("../migrations/world/0013_summary_compression.sql") },
    Migration { version: 14, name: "campaign_trash", sql: include_str!("../migrations/world/0014_campaign_trash.sql") },
];

pub const LIBRARY_MIGRATIONS: &[Migration] = &[
    Migration { version: 1, name: "init", sql: include_str!("../migrations/library/0001_init.sql") },
    Migration { version: 2, name: "providers", sql: include_str!("../migrations/library/0002_providers.sql") },
    Migration { version: 3, name: "managed_models", sql: include_str!("../migrations/library/0003_managed_models.sql") },
    Migration { version: 4, name: "request_timings", sql: include_str!("../migrations/library/0004_request_timings.sql") },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    pub from: i64,
    pub to: i64,
    pub backup: Option<PathBuf>,
}

pub fn open_connection(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    // 写事务一开始就拿写锁（IMMEDIATE）：默认的延迟事务先读后写，若其间另一个连接写过（例如另一个进程打开存档），
    // 升级写锁时 SQLite 会立即返回"database is locked"而不等待。Phase 8 压测实测复现（500 次写入失败 30 次）。
    let mut conn = conn;
    conn.set_transaction_behavior(rusqlite::TransactionBehavior::Immediate);
    // SQLite 推荐的长连接用法：打开时按需更新查询统计信息（只分析需要的表，开销很小）。
    conn.execute_batch("PRAGMA optimize=0x10002")?;
    Ok(conn)
}

pub fn user_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// 把整个数据库一致地复制到 `dest`（目标文件不能已存在）。
pub fn vacuum_into(conn: &Connection, dest: &Path) -> Result<()> {
    if dest.exists() {
        return Err(CoreError::Invalid(format!("目标文件已存在: {}", dest.display())));
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    conn.execute("VACUUM INTO ?1", [dest.to_string_lossy().as_ref()])?;
    Ok(())
}

pub fn migrate(
    conn: &mut Connection,
    migrations: &[Migration],
    backup_dir: &Path,
    label: &str,
) -> Result<MigrationReport> {
    let latest = migrations.iter().map(|m| m.version).max().unwrap_or(0);
    let current = user_version(conn)?;
    if current > latest {
        return Err(CoreError::Migration(format!(
            "数据库版本 {current} 高于程序支持的版本 {latest}，请升级程序后再打开"
        )));
    }
    if current == latest {
        return Ok(MigrationReport { from: current, to: current, backup: None });
    }

    let backup = if current > 0 {
        let name = format!("{label}-v{current}-{}.db", now_ms());
        let path = backup_dir.join(name);
        vacuum_into(conn, &path)?;
        Some(path)
    } else {
        None
    };

    let mut pending: Vec<&Migration> = migrations.iter().filter(|m| m.version > current).collect();
    pending.sort_by_key(|m| m.version);
    for m in pending {
        let tx = conn.transaction()?;
        let res = tx.execute_batch(m.sql).and_then(|_| tx.pragma_update(None, "user_version", m.version));
        match res {
            Ok(()) => tx.commit()?,
            Err(e) => {
                drop(tx); // 回滚
                let hint = backup
                    .as_ref()
                    .map(|p| format!("，迁移前备份：{}", p.display()))
                    .unwrap_or_default();
                return Err(CoreError::Migration(format!(
                    "迁移 v{} ({}) 失败：{e}{hint}",
                    m.version, m.name
                )));
            }
        }
    }
    Ok(MigrationReport { from: current, to: latest, backup })
}

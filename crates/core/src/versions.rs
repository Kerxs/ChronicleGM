//! 世界版本快照：把整个世界文件一致地复制一份，可随时回滚。

use std::path::PathBuf;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::db::vacuum_into;
use crate::error::{CoreError, Result};
use crate::util::{new_id, now_ms};
use crate::world::World;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct WorldVersion {
    pub id: String,
    pub name: String,
    pub note: String,
    pub commit_id: Option<String>,
    pub file_name: String,
    pub created_at: i64,
}

fn versions_dir(world: &World) -> PathBuf {
    let stem = world.path().file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "world".into());
    world.path().parent().unwrap_or(std::path::Path::new(".")).join("versions").join(stem)
}

pub fn list_versions(conn: &Connection) -> Result<Vec<WorldVersion>> {
    let mut stmt = conn.prepare("SELECT * FROM world_versions ORDER BY created_at DESC")?;
    let rows = stmt.query_map([], |r| {
        Ok(WorldVersion {
            id: r.get("id")?,
            name: r.get("name")?,
            note: r.get("note")?,
            commit_id: r.get("commit_id")?,
            file_name: r.get("file_name")?,
            created_at: r.get("created_at")?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn create_version(world: &World, name: &str, note: &str) -> Result<WorldVersion> {
    if name.trim().is_empty() {
        return Err(CoreError::Invalid("版本名称不能为空".into()));
    }
    let id = new_id();
    let file_name = format!("{id}.{}", crate::world::WORLD_FILE_EXT);
    let v = WorldVersion {
        id,
        name: name.trim().into(),
        note: note.into(),
        commit_id: world.head_commit()?,
        file_name,
        created_at: now_ms(),
    };
    // 先登记再复制，使快照文件里也包含这条记录。
    world.conn().execute(
        "INSERT INTO world_versions (id, name, note, commit_id, file_name, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
        params![v.id, v.name, v.note, v.commit_id, v.file_name, v.created_at],
    )?;
    if let Err(e) = vacuum_into(world.conn(), &versions_dir(world).join(&v.file_name)) {
        world.conn().execute("DELETE FROM world_versions WHERE id = ?1", [&v.id])?;
        return Err(e);
    }
    Ok(v)
}

/// 回滚到某个版本。回滚前自动把当前状态备份到备份目录；版本列表会保留。
/// 返回重新打开的世界与回滚前备份文件的路径。
pub fn restore_version(world: World, version_id: &str) -> Result<(World, PathBuf)> {
    let versions = list_versions(world.conn())?;
    let v = versions
        .iter()
        .find(|v| v.id == version_id)
        .cloned()
        .ok_or_else(|| CoreError::NotFound("版本", version_id.into()))?;
    let snapshot = versions_dir(&world).join(&v.file_name);
    if !snapshot.exists() {
        return Err(CoreError::NotFound("版本文件", snapshot.display().to_string()));
    }
    replace_world_file(world, &snapshot)
}

/// 用另一个世界文件（快照或备份）替换当前世界：先把当前状态备份，替换后重新打开，并保留版本列表。
pub(crate) fn replace_world_file(world: World, src: &std::path::Path) -> Result<(World, PathBuf)> {
    let versions = list_versions(world.conn())?;
    let path = world.path().to_path_buf();
    let backup_dir = world.backup_dir().to_path_buf();
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let backup = backup_dir.join(format!("{stem}-before-restore-{}.cgw", now_ms()));
    vacuum_into(world.conn(), &backup)?;

    world.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    drop(world);
    // 连接关闭后 WAL 已合并；若仍残留则移到一旁，而不是删除。
    for ext in ["cgw-wal", "cgw-shm"] {
        let side = path.with_extension(ext);
        if side.exists() {
            std::fs::rename(&side, backup_dir.join(format!("{stem}-{}.{ext}", now_ms())))?;
        }
    }
    std::fs::copy(src, &path)?;
    let (world, _) = World::open(&path, &backup_dir)?;
    // 把被替换文件之后创建的版本记录补回来。
    for old in versions {
        world.conn().execute(
            "INSERT OR IGNORE INTO world_versions (id, name, note, commit_id, file_name, created_at)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![old.id, old.name, old.note, old.commit_id, old.file_name, old.created_at],
        )?;
    }
    Ok((world, backup))
}
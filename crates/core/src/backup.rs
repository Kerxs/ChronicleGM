//! 自动备份与轮换、从备份还原。
//!
//! 只轮换程序自己创建的自动备份（`<世界>-auto-<时间>.cgw`）；迁移前备份、回滚前备份、
//! 修复前备份等永远不会被自动删除。

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::{WORLD_MIGRATIONS, user_version, vacuum_into};
use crate::error::{CoreError, Result};
use crate::util::now_ms;
use crate::world::World;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct BackupInfo {
    pub file_name: String,
    /// auto / migration / before_restore / before_repair / other
    pub kind: String,
    pub size: u64,
    /// 文件名中的时间戳（毫秒）；没有时用文件修改时间。
    pub created_at: i64,
}

fn stem(world: &World) -> String {
    world.path().file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

fn classify(name: &str, stem: &str) -> Option<(String, i64)> {
    let rest = name.strip_prefix(stem)?.strip_prefix('-')?;
    let rest = rest.strip_suffix(".cgw").or_else(|| rest.strip_suffix(".db"))?;
    let (kind, ts) = match rest.rsplit_once('-') {
        Some((k, ts)) => (k, ts.parse::<i64>().ok()),
        None => (rest, None),
    };
    let kind = match kind {
        "auto" => "auto",
        "before-restore" => "before_restore",
        "before-repair" => "before_repair",
        k if k.starts_with('v') && k[1..].chars().all(|c| c.is_ascii_digit()) => "migration",
        _ => "other",
    };
    Some((kind.to_string(), ts.unwrap_or(0)))
}

/// 删除一个备份文件（只能删这个世界自己的备份，文件名不能带路径）。
pub fn delete_backup(world: &World, file_name: &str) -> Result<()> {
    if file_name.contains(['/', '\\']) || file_name.contains("..") {
        return Err(crate::error::CoreError::Invalid("备份文件名不对".into()));
    }
    if classify(file_name, &stem(world)).is_none() {
        return Err(crate::error::CoreError::Invalid("这不是这个世界的备份".into()));
    }
    std::fs::remove_file(world.backup_dir().join(file_name))?;
    Ok(())
}

/// 列出这个世界的全部备份（新的在前）。
pub fn list_backups(world: &World) -> Result<Vec<BackupInfo>> {
    let dir = world.backup_dir();
    let stem = stem(world);
    let mut out = vec![];
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(out) };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some((kind, ts)) = classify(&name, &stem) else { continue };
        let meta = e.metadata()?;
        if !meta.is_file() {
            continue;
        }
        let created_at = if ts > 0 {
            ts
        } else {
            meta.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0)
        };
        out.push(BackupInfo { file_name: name, kind, size: meta.len(), created_at });
    }
    out.sort_by_key(|b| std::cmp::Reverse(b.created_at));
    Ok(out)
}

/// 自动备份：距离上次自动备份超过 `min_interval_ms` 才备份，并只保留最近 `keep` 份自动备份。
/// 返回新建的备份（没有到时间则为 None）。
pub fn auto_backup(world: &World, keep: usize, min_interval_ms: i64) -> Result<Option<BackupInfo>> {
    let all = list_backups(world)?;
    let autos: Vec<&BackupInfo> = all.iter().filter(|b| b.kind == "auto").collect();
    let now = now_ms();
    if let Some(latest) = autos.first()
        && now - latest.created_at < min_interval_ms
    {
        return Ok(None);
    }
    let name = format!("{}-auto-{now}.cgw", stem(world));
    let path = world.backup_dir().join(&name);
    vacuum_into(world.conn(), &path)?;
    // 轮换：只删除超出数量的自动备份。
    let mut autos: Vec<BackupInfo> = list_backups(world)?.into_iter().filter(|b| b.kind == "auto").collect();
    autos.sort_by_key(|b| std::cmp::Reverse(b.created_at));
    for old in autos.iter().skip(keep.max(1)) {
        let p = world.backup_dir().join(&old.file_name);
        if p.starts_with(world.backup_dir()) {
            let _ = std::fs::remove_file(p);
        }
    }
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    Ok(Some(BackupInfo { file_name: name, kind: "auto".into(), size, created_at: now }))
}

/// 检查备份文件是否是可用的世界文件（只读打开）。
pub fn verify_world_file(path: &Path) -> Result<()> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let v = user_version(&conn)?;
    let latest = WORLD_MIGRATIONS.iter().map(|m| m.version).max().unwrap_or(0);
    if v == 0 || v > latest {
        return Err(CoreError::Invalid(format!("备份文件版本 {v} 无法使用（程序支持 1–{latest}）")));
    }
    let ok: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    if ok != "ok" {
        return Err(CoreError::Invalid(format!("备份文件已损坏：{ok}")));
    }
    let has_meta: Option<String> = conn.query_row("SELECT value FROM world_meta WHERE key = 'world_id'", [], |r| r.get(0)).optional()?;
    if has_meta.is_none() {
        return Err(CoreError::Invalid("备份文件不是世界文件".into()));
    }
    Ok(())
}

/// 从备份还原。当前状态会先备份（before-restore），版本列表保留。返回重新打开的世界与该备份路径。
pub fn restore_backup(world: World, file_name: &str) -> Result<(World, PathBuf)> {
    if file_name.contains(['/', '\\']) || file_name.contains("..") {
        return Err(CoreError::Invalid("备份文件名无效".into()));
    }
    let src = world.backup_dir().join(file_name);
    if !src.is_file() {
        return Err(CoreError::NotFound("备份文件", file_name.into()));
    }
    verify_world_file(&src)?;
    // 先复制到临时文件，避免还原过程中轮换删掉源文件。
    let tmp = world.backup_dir().join(format!("restoring-{}.tmp", now_ms()));
    std::fs::copy(&src, &tmp)?;
    let res = crate::versions::replace_world_file(world, &tmp);
    let _ = std::fs::remove_file(&tmp);
    res
}

#[cfg(test)]
mod tests {
    use super::classify;

    #[test]
    fn classifies_backup_names() {
        assert_eq!(classify("W1-auto-1700000000000.cgw", "W1"), Some(("auto".into(), 1700000000000)));
        assert_eq!(classify("W1-v2-1700000000000.db", "W1"), Some(("migration".into(), 1700000000000)));
        assert_eq!(classify("W1-before-restore-5.cgw", "W1"), Some(("before_restore".into(), 5)));
        assert_eq!(classify("W2-auto-5.cgw", "W1"), None);
        assert_eq!(classify("W1-auto-5.cgw-wal", "W1"), None);
    }
}

//! 世界文件：打开/创建、写事务（commit + revisions）、撤销。

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::db::{self, MigrationReport, WORLD_MIGRATIONS};
use crate::entity::{Entity, js};
use crate::error::{CoreError, Result};
use crate::model::*;
use crate::util::{new_id, now_ms};

pub const WORLD_FILE_EXT: &str = "cgw";

pub struct World {
    pub(crate) conn: Connection,
    path: PathBuf,
    backup_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CommitInfo {
    pub id: String,
    pub author: String,
    pub changeset_id: Option<String>,
    pub message: String,
    pub created_at: i64,
    pub undone_by: Option<String>,
    pub undo_of: Option<String>,
    pub revision_count: i64,
}

impl World {
    /// 新建世界文件；文件已存在时报错，绝不覆盖。
    pub fn create(path: &Path, backup_dir: &Path, name: &str) -> Result<World> {
        if path.exists() {
            return Err(CoreError::Invalid(format!("世界文件已存在: {}", path.display())));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let (w, _) = Self::open(path, backup_dir)?;
        let world_id = new_id();
        w.conn.execute_batch("BEGIN")?;
        let res = (|| -> Result<()> {
            for (k, v) in [("world_id", world_id.as_str()), ("name", name), ("created_at", &now_ms().to_string())] {
                w.conn.execute("INSERT INTO world_meta(key, value) VALUES (?1, ?2)", params![k, v])?;
            }
            Ok(())
        })();
        match res {
            Ok(()) => w.conn.execute_batch("COMMIT")?,
            Err(e) => {
                w.conn.execute_batch("ROLLBACK")?;
                return Err(e);
            }
        }
        Ok(w)
    }

    /// 打开（必要时迁移）世界文件。
    pub fn open(path: &Path, backup_dir: &Path) -> Result<(World, MigrationReport)> {
        let mut conn = db::open_connection(path)?;
        let label = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "world".into());
        let report = db::migrate(&mut conn, WORLD_MIGRATIONS, backup_dir, &label)?;
        Ok((World { conn, path: path.to_path_buf(), backup_dir: backup_dir.to_path_buf() }, report))
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn backup_dir(&self) -> &Path {
        &self.backup_dir
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM world_meta WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO world_meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn name(&self) -> Result<String> {
        Ok(self.meta("name")?.unwrap_or_default())
    }

    /// 在一个事务中执行写操作，生成一个 commit。闭包返回错误时整体回滚，不留下任何修改。
    pub fn write<T>(
        &mut self,
        author: &Author,
        message: &str,
        f: impl FnOnce(&mut Writer) -> Result<T>,
    ) -> Result<(T, Option<String>)> {
        self.write_with(author, message, None, None, f)
    }

    pub(crate) fn write_with<T>(
        &mut self,
        author: &Author,
        message: &str,
        changeset_id: Option<&str>,
        undo_of: Option<&str>,
        f: impl FnOnce(&mut Writer) -> Result<T>,
    ) -> Result<(T, Option<String>)> {
        let tx = self.conn.transaction()?;
        let commit_id = new_id();
        tx.execute(
            "INSERT INTO commits (id, author, changeset_id, message, created_at, undo_of) VALUES (?1,?2,?3,?4,?5,?6)",
            params![commit_id, author.tag(), changeset_id, message, now_ms(), undo_of],
        )?;
        let mut w = Writer { tx, commit_id: commit_id.clone(), author: author.clone(), revisions: 0, now: now_ms() };
        let out = f(&mut w)?;
        let Writer { tx, revisions, .. } = w;
        let commit = if revisions == 0 {
            tx.execute("DELETE FROM commits WHERE id = ?1", [&commit_id])?;
            None
        } else {
            if let Some(orig) = undo_of {
                tx.execute("UPDATE commits SET undone_by = ?1 WHERE id = ?2", params![commit_id, orig])?;
            }
            Some(commit_id)
        };
        tx.commit()?;
        Ok((out, commit))
    }

    pub fn commits(&self, limit: i64, offset: i64) -> Result<Vec<CommitInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.*, (SELECT COUNT(*) FROM revisions r WHERE r.commit_id = c.id) AS n
             FROM commits c ORDER BY c.created_at DESC, c.rowid DESC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map([limit, offset], |r| {
            Ok(CommitInfo {
                id: r.get("id")?,
                author: r.get("author")?,
                changeset_id: r.get("changeset_id")?,
                message: r.get("message")?,
                created_at: r.get("created_at")?,
                undone_by: r.get("undone_by")?,
                undo_of: r.get("undo_of")?,
                revision_count: r.get("n")?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn head_commit(&self) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT id FROM commits ORDER BY created_at DESC, rowid DESC LIMIT 1", [], |r| r.get(0))
            .optional()?)
    }

    /// 撤销一个 commit：按相反顺序恢复每个实体的修订前内容。
    /// 若实体在此之后又被修改过（版本号不一致），拒绝撤销并报告冲突，不做任何修改。
    /// 撤销本身也是一个 commit，因此可以再次撤销（重做）。
    pub fn undo(&mut self, commit_id: &str, author: &Author) -> Result<Option<String>> {
        let (undone_by, message): (Option<String>, String) = self
            .conn
            .query_row("SELECT undone_by, message FROM commits WHERE id = ?1", [commit_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?
            .ok_or_else(|| CoreError::NotFound("commit", commit_id.into()))?;
        if undone_by.is_some() {
            return Err(CoreError::Invalid(format!("commit {commit_id} 已被撤销")));
        }
        let revs: Vec<(String, String, Option<String>, Option<String>)> = {
            let mut stmt = self.conn.prepare(
                "SELECT entity, entity_id, before_json, after_json FROM revisions WHERE commit_id = ?1 ORDER BY seq DESC",
            )?;
            stmt.query_map([commit_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        let msg = format!("撤销：{message}");
        let (_, c) = self.write_with(author, &msg, None, Some(commit_id), |w| {
            for (entity, id, before, after) in &revs {
                match entity.as_str() {
                    "nodes" => w.restore::<Node>(id, before.as_deref(), after.as_deref())?,
                    "relations" => w.restore::<Relation>(id, before.as_deref(), after.as_deref())?,
                    "scripts" => w.restore::<Script>(id, before.as_deref(), after.as_deref())?,
                    "script_items" => w.restore::<ScriptItem>(id, before.as_deref(), after.as_deref())?,
                    "script_cast" => w.restore::<CastMember>(id, before.as_deref(), after.as_deref())?,
                    "plot_nodes" => w.restore::<PlotNode>(id, before.as_deref(), after.as_deref())?,
                    "plot_edges" => w.restore::<PlotEdge>(id, before.as_deref(), after.as_deref())?,
                    other => return Err(CoreError::Invalid(format!("未知实体类型 {other}"))),
                }
            }
            Ok(())
        })?;
        Ok(c)
    }
}

/// 写事务句柄。所有修改都通过它进行，并自动记录修订。
pub struct Writer<'a> {
    pub(crate) tx: Transaction<'a>,
    pub(crate) commit_id: String,
    pub(crate) author: Author,
    pub(crate) revisions: usize,
    pub(crate) now: i64,
}

impl Writer<'_> {
    pub fn conn(&self) -> &Connection {
        &self.tx
    }
    pub fn author(&self) -> &Author {
        &self.author
    }
    pub fn commit_id(&self) -> &str {
        &self.commit_id
    }

    pub(crate) fn record<E: Entity>(&mut self, id: &str, before: Option<&E>, after: Option<&E>) -> Result<()> {
        self.tx.execute(
            "INSERT INTO revisions (commit_id, entity, entity_id, before_json, after_json) VALUES (?1,?2,?3,?4,?5)",
            params![self.commit_id, E::NAME, id, before.map(js), after.map(js)],
        )?;
        self.revisions += 1;
        Ok(())
    }

    /// 插入新实体并记录修订。
    pub(crate) fn put_new<E: Entity>(&mut self, e: &E) -> Result<()> {
        e.insert(&self.tx)?;
        self.record::<E>(e.id(), None, Some(e))
    }

    /// 用新内容覆盖实体（自动递增版本号）并记录修订。返回写入后的实体。
    pub(crate) fn put_update<E: Entity>(&mut self, before: &E, mut after: E) -> Result<E> {
        after.set_rev(before.rev() + 1);
        after.set_updated_at(self.now);
        after.update(&self.tx)?;
        self.record::<E>(after.id(), Some(before), Some(&after))?;
        Ok(after)
    }

    fn restore<E: Entity>(&mut self, id: &str, before: Option<&str>, after: Option<&str>) -> Result<()> {
        let current = E::load(&self.tx, id)?;
        let after: Option<E> = after.map(serde_json::from_str).transpose()?;
        let before: Option<E> = before.map(serde_json::from_str).transpose()?;
        match (&current, &after) {
            (Some(cur), Some(aft)) if cur.rev() == aft.rev() => {}
            (None, None) => {}
            (Some(cur), Some(aft)) => {
                return Err(CoreError::RevConflict {
                    entity: E::NAME,
                    id: id.into(),
                    expected: aft.rev(),
                    found: cur.rev(),
                });
            }
            _ => return Err(CoreError::Invalid(format!("{} {id} 的当前状态与修订记录不一致，无法撤销", E::NAME))),
        }
        match (current, before) {
            (Some(cur), None) => {
                E::hard_delete(&self.tx, id)?;
                self.record::<E>(id, Some(&cur), None)?;
            }
            (Some(cur), Some(b)) => {
                self.put_update(&cur, b)?;
            }
            (None, Some(mut b)) => {
                b.set_updated_at(self.now);
                self.put_new(&b)?;
            }
            (None, None) => {}
        }
        Ok(())
    }

    /// AI 写入的通用保护：锁定内容一律不可改；核心正典 AI 不可改。
    pub(crate) fn guard(&self, what: &str, locked: bool, core: bool, unlock_only: bool) -> Result<()> {
        if locked && !(unlock_only && !self.author.is_ai()) {
            return Err(CoreError::Protected(format!("{what} 已锁定，需先手动解锁")));
        }
        if core && self.author.is_ai() {
            return Err(CoreError::Protected(format!("{what} 属于核心设定，AI 不能修改")));
        }
        Ok(())
    }
}

pub(crate) fn check_rev(entity: &'static str, id: &str, expected: Option<i64>, found: i64) -> Result<()> {
    match expected {
        Some(e) if e != found => Err(CoreError::RevConflict { entity, id: id.into(), expected: e, found }),
        _ => Ok(()),
    }
}

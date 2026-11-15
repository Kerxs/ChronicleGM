//! 剧本管理：删除（进回收站，连同条目、角色、剧情图一起，可恢复）、复制、版本快照（对比、恢复）。
//!
//! - 还有未归档的战役在用这个剧本时不能删除（战役需要剧本才能继续），先归档或删除那些战役；
//! - 复制会给所有条目、剧情节点、分支、角色换新编号，并把内部引用（父条目、分支两端、效果和条件里的编号）一起换掉；
//! - 恢复版本前自动保存当前状态为一个版本，恢复本身也是普通的写入提交（可以撤销）。

use std::collections::{BTreeMap, HashMap};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::entity::Entity;
use crate::error::{CoreError, Result};
use crate::model::*;
use crate::util::{new_id, now_ms};
use crate::world::{World, Writer};

/// 一个剧本的全部内容（未删除的）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptBundle {
    pub script: Script,
    pub items: Vec<ScriptItem>,
    pub cast: Vec<CastMember>,
    pub nodes: Vec<PlotNode>,
    pub edges: Vec<PlotEdge>,
}

fn rows<E: Entity>(conn: &Connection, sql: &str, script_id: &str) -> Result<Vec<E>> {
    let mut stmt = conn.prepare(sql)?;
    let r = stmt.query_map([script_id], |r| E::from_row(r))?;
    Ok(r.collect::<rusqlite::Result<_>>()?)
}

pub fn bundle(conn: &Connection, script_id: &str) -> Result<ScriptBundle> {
    let script = crate::scripts::require_script(conn, script_id)?;
    Ok(ScriptBundle {
        script,
        items: rows(conn, "SELECT * FROM script_items WHERE script_id = ?1 AND deleted_at IS NULL ORDER BY sort_key", script_id)?,
        cast: rows(conn, "SELECT * FROM script_cast WHERE script_id = ?1 AND deleted_at IS NULL", script_id)?,
        nodes: rows(conn, "SELECT * FROM plot_nodes WHERE script_id = ?1 AND deleted_at IS NULL", script_id)?,
        edges: rows(conn, "SELECT * FROM plot_edges WHERE script_id = ?1 AND deleted_at IS NULL", script_id)?,
    })
}

/// 还在使用这个剧本、没有归档的战役名称。
pub fn campaigns_using(conn: &Connection, script_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT name FROM campaigns WHERE script_id = ?1 AND archived = 0 ORDER BY created_at")?;
    let r = stmt.query_map([script_id], |r| r.get::<_, String>(0))?;
    Ok(r.collect::<rusqlite::Result<_>>()?)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct DeletedScript {
    pub id: String,
    pub title: String,
    pub deleted_at: i64,
    pub items: i64,
}

/// 回收站里的剧本。
pub fn script_trash(conn: &Connection) -> Result<Vec<DeletedScript>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.title, s.deleted_at, (SELECT COUNT(*) FROM script_items i WHERE i.script_id = s.id AND i.deleted_at = s.deleted_at)
         FROM scripts s WHERE s.deleted_at IS NOT NULL ORDER BY s.deleted_at DESC",
    )?;
    let r = stmt.query_map([], |r| Ok(DeletedScript { id: r.get(0)?, title: r.get(1)?, deleted_at: r.get(2)?, items: r.get(3)? }))?;
    Ok(r.collect::<rusqlite::Result<_>>()?)
}

/// 用 JSON 文本替换编号（编号是 ULID，不会误伤普通文字）。
fn remap<T: Serialize + serde::de::DeserializeOwned>(v: &T, map: &HashMap<String, String>) -> Result<T> {
    let mut s = serde_json::to_string(v)?;
    for (old, new) in map {
        if s.contains(old.as_str()) {
            s = s.replace(old.as_str(), new.as_str());
        }
    }
    Ok(serde_json::from_str(&s)?)
}

impl Writer<'_> {
    /// 删除剧本：剧本、条目、角色、剧情节点和分支一起进回收站（同一个删除时间，恢复时一起恢复）。
    pub fn delete_script_cascade(&mut self, id: &str) -> Result<usize> {
        let b = bundle(&self.tx, id)?;
        if self.author.is_ai() {
            return Err(CoreError::Protected("AI 不能删除整个剧本".into()));
        }
        let using = campaigns_using(&self.tx, id)?;
        if !using.is_empty() {
            return Err(CoreError::Invalid(format!("还有 {} 个战役在用这个剧本：{}。先在游戏页归档或删除这些战役，再删除剧本", using.len(), using.join("、"))));
        }
        let now = self.now;
        let mut n = 0;
        macro_rules! del {
            ($list:expr) => {
                for e in $list {
                    let mut d = e.clone();
                    d.deleted_at = Some(now);
                    self.put_update(e, d)?;
                    n += 1;
                }
            };
        }
        del!(&b.edges);
        del!(&b.nodes);
        del!(&b.cast);
        del!(&b.items);
        let mut s = b.script.clone();
        s.deleted_at = Some(now);
        self.put_update(&b.script, s)?;
        Ok(n + 1)
    }

    /// 从回收站恢复剧本（以及同一次删除的条目、角色、剧情图）。
    pub fn restore_script(&mut self, id: &str) -> Result<usize> {
        let cur = Script::load(&self.tx, id)?.ok_or_else(|| CoreError::NotFound("剧本", id.into()))?;
        let Some(ts) = cur.deleted_at else { return Ok(0) };
        let mut n = 0;
        macro_rules! undel {
            ($ty:ty, $table:literal) => {
                let list: Vec<$ty> = {
                    let mut stmt = self.tx.prepare(concat!("SELECT * FROM ", $table, " WHERE script_id = ?1 AND deleted_at = ?2"))?;
                    let r = stmt.query_map(params![id, ts], |r| <$ty>::from_row(r))?;
                    r.collect::<rusqlite::Result<_>>()?
                };
                for e in &list {
                    let mut d = e.clone();
                    d.deleted_at = None;
                    self.put_update(e, d)?;
                    n += 1;
                }
            };
        }
        let mut s = cur.clone();
        s.deleted_at = None;
        self.put_update(&cur, s)?;
        undel!(ScriptItem, "script_items");
        undel!(CastMember, "script_cast");
        undel!(PlotNode, "plot_nodes");
        undel!(PlotEdge, "plot_edges");
        Ok(n + 1)
    }

    /// 复制剧本（条目、角色、剧情图），返回新剧本。
    pub fn copy_script(&mut self, id: &str, title: &str) -> Result<Script> {
        let b = bundle(&self.tx, id)?;
        let mut map: HashMap<String, String> = HashMap::new();
        map.insert(b.script.id.clone(), new_id());
        for i in &b.items {
            map.insert(i.id.clone(), new_id());
        }
        for c in &b.cast {
            map.insert(c.id.clone(), new_id());
        }
        for x in &b.nodes {
            map.insert(x.id.clone(), new_id());
        }
        for e in &b.edges {
            map.insert(e.id.clone(), new_id());
        }
        let now = self.now;
        let tag = self.author.tag();
        let title = if title.trim().is_empty() { format!("{}（副本）", b.script.title) } else { title.trim().to_string() };
        let mut s = remap(&b.script, &map)?;
        s.title = title;
        s.status = ContentStatus::Draft;
        (s.rev, s.created_at, s.updated_at, s.source) = (1, now, now, tag.clone());
        self.put_new(&s)?;
        macro_rules! copy {
            ($list:expr) => {
                for e in $list {
                    let mut c = remap(e, &map)?;
                    (c.rev, c.created_at, c.updated_at, c.source) = (1, now, now, tag.clone());
                    self.put_new(&c)?;
                }
            };
        }
        copy!(&b.items);
        copy!(&b.cast);
        copy!(&b.nodes);
        copy!(&b.edges);
        Ok(s)
    }

    /// 把剧本内容恢复成某个快照：快照里有的写回（含已删除的恢复），快照里没有的进回收站。
    fn apply_bundle(&mut self, snap: &ScriptBundle) -> Result<usize> {
        let id = snap.script.id.clone();
        let now = self.now;
        let mut n = 0;
        let cur_script = Script::load(&self.tx, &id)?.ok_or_else(|| CoreError::NotFound("剧本", id.clone()))?;
        let mut s = snap.script.clone();
        s.deleted_at = None;
        if s != cur_script {
            self.put_update(&cur_script, s)?;
        }
        macro_rules! sync {
            ($ty:ty, $table:literal, $list:expr) => {
                let wanted: BTreeMap<String, &$ty> = $list.iter().map(|e| (e.id.clone(), e)).collect();
                let current: Vec<$ty> = {
                    let mut stmt = self.tx.prepare(concat!("SELECT * FROM ", $table, " WHERE script_id = ?1"))?;
                    let r = stmt.query_map([&id], |r| <$ty>::from_row(r))?;
                    r.collect::<rusqlite::Result<_>>()?
                };
                for c in &current {
                    match wanted.get(&c.id) {
                        Some(w) => {
                            let mut v = (*w).clone();
                            v.deleted_at = None;
                            v.rev = c.rev;
                            v.updated_at = c.updated_at;
                            if &v != c {
                                self.put_update(c, v)?;
                                n += 1;
                            }
                        }
                        None if c.deleted_at.is_none() => {
                            let mut d = c.clone();
                            d.deleted_at = Some(now);
                            self.put_update(c, d)?;
                            n += 1;
                        }
                        None => {}
                    }
                }
                for (wid, w) in &wanted {
                    if !current.iter().any(|c| &c.id == wid) {
                        let mut v = (*w).clone();
                        v.deleted_at = None;
                        self.put_new(&v)?;
                        n += 1;
                    }
                }
            };
        }
        sync!(ScriptItem, "script_items", snap.items);
        sync!(CastMember, "script_cast", snap.cast);
        sync!(PlotNode, "plot_nodes", snap.nodes);
        sync!(PlotEdge, "plot_edges", snap.edges);
        Ok(n)
    }
}

// ===================== 版本快照 =====================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ScriptVersion {
    pub id: String,
    pub script_id: String,
    pub name: String,
    pub note: String,
    pub created_at: i64,
    /// 快照里的条目数 / 剧情节点数。
    pub items: i64,
    pub nodes: i64,
}

pub fn create_script_version(world: &World, script_id: &str, name: &str, note: &str) -> Result<ScriptVersion> {
    let b = bundle(world.conn(), script_id)?;
    let name = if name.trim().is_empty() { format!("{} 的快照", b.script.title) } else { name.trim().to_string() };
    let v = ScriptVersion { id: new_id(), script_id: script_id.into(), name, note: note.into(), created_at: now_ms(), items: b.items.len() as i64, nodes: b.nodes.len() as i64 };
    world.conn().execute(
        "INSERT INTO script_versions (id, script_id, name, note, data, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
        params![v.id, v.script_id, v.name, v.note, serde_json::to_string(&b)?, v.created_at],
    )?;
    Ok(v)
}

fn load_version(conn: &Connection, version_id: &str) -> Result<(ScriptVersion, ScriptBundle)> {
    let (sid, name, note, data, at): (String, String, String, String, i64) = conn
        .query_row("SELECT script_id, name, note, data, created_at FROM script_versions WHERE id = ?1", [version_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .map_err(|_| CoreError::NotFound("剧本版本", version_id.into()))?;
    let b: ScriptBundle = serde_json::from_str(&data)?;
    Ok((ScriptVersion { id: version_id.into(), script_id: sid, name, note, created_at: at, items: b.items.len() as i64, nodes: b.nodes.len() as i64 }, b))
}

pub fn list_script_versions(conn: &Connection, script_id: &str) -> Result<Vec<ScriptVersion>> {
    let mut stmt = conn.prepare("SELECT id FROM script_versions WHERE script_id = ?1 ORDER BY created_at DESC")?;
    let ids: Vec<String> = stmt.query_map([script_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    ids.iter().map(|i| load_version(conn, i).map(|x| x.0)).collect()
}

pub fn delete_script_version(world: &World, version_id: &str) -> Result<()> {
    world.conn().execute("DELETE FROM script_versions WHERE id = ?1", [version_id])?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct VersionDiff {
    /// 快照之后新增的（恢复时会移到回收站）。
    pub added: Vec<String>,
    /// 快照之后删除的（恢复时会回来）。
    pub removed: Vec<String>,
    /// 内容有变化的。
    pub changed: Vec<String>,
}

/// 当前剧本和快照的差别（按条目 / 剧情节点 / 分支 / 角色）。
pub fn diff_script_version(conn: &Connection, version_id: &str) -> Result<VersionDiff> {
    let (v, snap) = load_version(conn, version_id)?;
    let cur = bundle(conn, &v.script_id)?;
    let mut d = VersionDiff { added: vec![], removed: vec![], changed: vec![] };
    fn cmp<T: Serialize>(d: &mut VersionDiff, label: &str, old: Vec<(String, String, &T)>, new: Vec<(String, String, &T)>) {
        let key = |t: &T| {
            let mut v = serde_json::to_value(t).unwrap_or_default();
            if let Some(o) = v.as_object_mut() {
                for k in ["rev", "updated_at", "created_at", "deleted_at"] {
                    o.remove(k);
                }
            }
            v
        };
        for (id, title, o) in &old {
            match new.iter().find(|(i, ..)| i == id) {
                None => d.removed.push(format!("{label}「{title}」")),
                Some((_, nt, n)) if key(o) != key(n) => d.changed.push(format!("{label}「{nt}」")),
                _ => {}
            }
        }
        for (id, title, _) in &new {
            if !old.iter().any(|(i, ..)| i == id) {
                d.added.push(format!("{label}「{title}」"));
            }
        }
    }
    let script_old = vec![(snap.script.id.clone(), snap.script.title.clone(), &snap.script)];
    let script_new = vec![(cur.script.id.clone(), cur.script.title.clone(), &cur.script)];
    cmp(&mut d, "剧本设定", script_old, script_new);
    cmp(&mut d, "条目", snap.items.iter().map(|i| (i.id.clone(), i.title.clone(), i)).collect(), cur.items.iter().map(|i| (i.id.clone(), i.title.clone(), i)).collect());
    cmp(&mut d, "剧情节点", snap.nodes.iter().map(|i| (i.id.clone(), i.title.clone(), i)).collect(), cur.nodes.iter().map(|i| (i.id.clone(), i.title.clone(), i)).collect());
    let ename = |e: &PlotEdge| if e.label.is_empty() { "分支".to_string() } else { e.label.clone() };
    cmp(&mut d, "分支", snap.edges.iter().map(|i| (i.id.clone(), ename(i), i)).collect(), cur.edges.iter().map(|i| (i.id.clone(), ename(i), i)).collect());
    cmp(&mut d, "角色", snap.cast.iter().map(|i| (i.id.clone(), i.role.clone(), i)).collect(), cur.cast.iter().map(|i| (i.id.clone(), i.role.clone(), i)).collect());
    Ok(d)
}

/// 恢复到某个版本：先把当前状态自动存成一个版本，再写回快照内容（一次提交，可以撤销）。
pub fn restore_script_version(world: &mut World, version_id: &str) -> Result<usize> {
    let (v, snap) = load_version(world.conn(), version_id)?;
    create_script_version(world, &v.script_id, &format!("恢复「{}」之前自动保存", v.name), "自动")?;
    let n = world.write(&Author::User, &format!("恢复剧本版本「{}」", v.name), |x| x.apply_bundle(&snap))?.0;
    Ok(n)
}

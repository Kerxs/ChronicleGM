//! 世界树节点与关联的读写。

use rusqlite::{Connection, OptionalExtension, params};

use crate::entity::Entity;
use crate::error::{CoreError, Result};
use crate::model::*;
use crate::payload::validate_node_payload;
use crate::util::{key_between, new_id};
use crate::world::{Writer, check_rev};

const MAX_TITLE: usize = 200;

fn check_title(t: &str) -> Result<()> {
    let n = t.trim().chars().count();
    if n == 0 {
        return Err(CoreError::Invalid("标题不能为空".into()));
    }
    if n > MAX_TITLE {
        return Err(CoreError::Invalid(format!("标题不能超过 {MAX_TITLE} 字")));
    }
    Ok(())
}

// ===================== 读取 =====================

pub fn get_node(conn: &Connection, id: &str) -> Result<Option<Node>> {
    Node::load_active(conn, id)
}

pub fn require_node(conn: &Connection, id: &str) -> Result<Node> {
    get_node(conn, id)?.ok_or_else(|| CoreError::NotFound("节点", id.into()))
}

/// 按排序键分页列出子节点；`parent = None` 表示根节点。
pub fn children(conn: &Connection, parent: Option<&str>, offset: i64, limit: i64) -> Result<Vec<Node>> {
    // 根节点与非根节点分开写，确保两种查询都能使用 (parent_id, sort_key) 部分索引。
    let rows = match parent {
        Some(p) => {
            let mut stmt = conn.prepare_cached(
                "SELECT * FROM nodes WHERE parent_id = ?1 AND deleted_at IS NULL ORDER BY sort_key LIMIT ?2 OFFSET ?3",
            )?;
            stmt.query_map(params![p, limit, offset], Node::from_row)?.collect::<rusqlite::Result<_>>()?
        }
        None => {
            let mut stmt = conn.prepare_cached(
                "SELECT * FROM nodes WHERE parent_id IS NULL AND deleted_at IS NULL ORDER BY sort_key LIMIT ?1 OFFSET ?2",
            )?;
            stmt.query_map(params![limit, offset], Node::from_row)?.collect::<rusqlite::Result<_>>()?
        }
    };
    Ok(rows)
}

pub fn child_count(conn: &Connection, parent: Option<&str>) -> Result<i64> {
    Ok(match parent {
        Some(p) => conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE parent_id = ?1 AND deleted_at IS NULL",
            [p],
            |r| r.get(0),
        )?,
        None => conn.query_row("SELECT COUNT(*) FROM nodes WHERE parent_id IS NULL AND deleted_at IS NULL", [], |r| {
            r.get(0)
        })?,
    })
}

fn sibling_keys(conn: &Connection, parent: Option<&str>, exclude: Option<&str>) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, sort_key FROM nodes WHERE parent_id IS ?1 AND deleted_at IS NULL ORDER BY sort_key",
    )?;
    let rows = stmt.query_map([parent], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?.into_iter().filter(|(id, _)| Some(id.as_str()) != exclude).collect())
}

/// 根据插入位置计算排序键（兄弟按排序键有序）。
pub(crate) fn key_for_position(siblings: &[(String, String)], pos: &Position) -> Result<String> {
    let first = siblings.first().map(|s| s.1.as_str());
    let last = siblings.last().map(|s| s.1.as_str());
    Ok(match pos {
        Position::Last => key_between(last, None),
        Position::First => key_between(None, first),
        Position::After(id) | Position::Before(id) => {
            let i = siblings
                .iter()
                .position(|s| &s.0 == id)
                .ok_or_else(|| CoreError::Invalid(format!("参照节点 {id} 不是同级节点")))?;
            if matches!(pos, Position::After(_)) {
                key_between(Some(&siblings[i].1), siblings.get(i + 1).map(|s| s.1.as_str()))
            } else {
                key_between(if i == 0 { None } else { Some(siblings[i - 1].1.as_str()) }, Some(&siblings[i].1))
            }
        }
    })
}

/// 从根到父节点的祖先链（不含自身）。
pub fn ancestors(conn: &Connection, id: &str) -> Result<Vec<Node>> {
    let n = Node::load(conn, id)?.ok_or_else(|| CoreError::NotFound("节点", id.into()))?;
    let ids: Vec<&str> = n.path.split('/').filter(|s| !s.is_empty() && *s != id).collect();
    let mut out = Vec::with_capacity(ids.len());
    for a in ids {
        if let Some(node) = Node::load(conn, a)? {
            out.push(node);
        }
    }
    Ok(out)
}

/// 子树（含自身），按路径排序；`max` 限制数量。
pub fn subtree(conn: &Connection, id: &str, max: i64) -> Result<Vec<Node>> {
    let n = require_node(conn, id)?;
    let upper = format!("{}\u{7f}", n.path);
    let mut stmt = conn.prepare_cached(
        "SELECT * FROM nodes WHERE path >= ?1 AND path < ?2 AND deleted_at IS NULL ORDER BY path LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![n.path, upper, max], Node::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn nodes_by_kind(conn: &Connection, kind: NodeKind, offset: i64, limit: i64) -> Result<Vec<Node>> {
    let mut stmt = conn.prepare_cached(
        "SELECT * FROM nodes WHERE kind = ?1 AND deleted_at IS NULL ORDER BY title LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt.query_map(params![kind.as_str(), limit, offset], Node::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 按规范化名称（标题或别名）查找节点。
pub fn find_by_name(conn: &Connection, name: &str, kind: Option<NodeKind>) -> Result<Vec<Node>> {
    let key = crate::util::normalize_name(name);
    let mut stmt = conn.prepare_cached(
        "SELECT n.* FROM node_names k JOIN nodes n ON n.id = k.node_id
         WHERE k.name_key = ?1 AND n.deleted_at IS NULL AND (?2 IS NULL OR n.kind = ?2)",
    )?;
    let rows = stmt.query_map(params![key, kind.map(|k| k.as_str())], Node::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 回收站：已删除的子树根（父节点未删除或为根）。
pub fn trash(conn: &Connection, limit: i64) -> Result<Vec<Node>> {
    let mut stmt = conn.prepare(
        "SELECT n.* FROM nodes n LEFT JOIN nodes p ON p.id = n.parent_id
         WHERE n.deleted_at IS NOT NULL AND (p.id IS NULL OR p.deleted_at IS NULL OR p.deleted_at <> n.deleted_at)
         ORDER BY n.deleted_at DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], Node::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn get_relation(conn: &Connection, id: &str) -> Result<Option<Relation>> {
    Relation::load_active(conn, id)
}

/// 与节点相关的所有关联（出边与入边）。
pub fn relations_of(conn: &Connection, node_id: &str) -> Result<Vec<Relation>> {
    let mut stmt = conn.prepare_cached(
        // 拆成两段 UNION ALL，两端都能走索引（OR 写法会退化为全表扫描）。
        "SELECT * FROM (
            SELECT * FROM relations WHERE src_id = ?1 AND deleted_at IS NULL
            UNION ALL
            SELECT * FROM relations WHERE dst_id = ?1 AND src_id <> ?1 AND deleted_at IS NULL
         ) ORDER BY rel_type, created_at",
    )?;
    let rows = stmt.query_map([node_id], Relation::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn relations_between(conn: &Connection, a: &str, b: &str) -> Result<Vec<Relation>> {
    let mut stmt = conn.prepare_cached(
        "SELECT * FROM relations WHERE src_id = ?1 AND dst_id = ?2 AND deleted_at IS NULL
         UNION ALL
         SELECT * FROM relations WHERE src_id = ?2 AND dst_id = ?1 AND deleted_at IS NULL",
    )?;
    let rows = stmt.query_map([a, b], Relation::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ===================== 写入 =====================

impl Writer<'_> {
    pub fn create_node(&mut self, n: NewNode) -> Result<Node> {
        self.create_node_with_id(new_id(), n)
    }

    pub(crate) fn create_node_with_id(&mut self, id: String, n: NewNode) -> Result<Node> {
        check_title(&n.title)?;
        validate_node_payload(n.kind, &n.payload)?;
        if self.author.is_ai() && (n.canon_level == CanonLevel::Core || n.locked) {
            return Err(CoreError::Protected("AI 不能直接创建核心设定或锁定内容".into()));
        }
        let parent_path = match &n.parent_id {
            Some(p) => require_node(&self.tx, p)?.path,
            None => "/".to_string(),
        };
        let siblings = sibling_keys(&self.tx, n.parent_id.as_deref(), None)?;
        let sort_key = key_for_position(&siblings, &n.position)?;
        let node = Node {
            path: format!("{parent_path}{id}/"),
            id,
            kind: n.kind,
            parent_id: n.parent_id,
            sort_key,
            title: n.title.trim().to_string(),
            aliases: n.aliases,
            summary: n.summary,
            player_text: n.player_text,
            gm_text: n.gm_text,
            payload: n.payload,
            tags: n.tags,
            canon_level: n.canon_level,
            locked: n.locked,
            status: n.status,
            source: self.author.tag(),
            rev: 1,
            created_at: self.now,
            updated_at: self.now,
            deleted_at: None,
        };
        self.put_new(&node)?;
        Ok(node)
    }

    pub fn update_node(&mut self, id: &str, patch: NodePatch, expected_rev: Option<i64>) -> Result<Node> {
        let cur = require_node(&self.tx, id)?;
        check_rev("节点", id, expected_rev, cur.rev)?;
        if patch.is_empty() {
            return Ok(cur);
        }
        self.guard(&format!("节点「{}」", cur.title), cur.locked, cur.canon_level == CanonLevel::Core, patch.is_unlock_only())?;
        if self.author.is_ai() && (patch.locked.is_some() || patch.canon_level == Some(CanonLevel::Core)) {
            return Err(CoreError::Protected("AI 不能修改锁定状态或提升为核心设定".into()));
        }
        let mut n = cur.clone();
        if let Some(v) = patch.title {
            check_title(&v)?;
            n.title = v.trim().to_string();
        }
        if let Some(v) = patch.aliases {
            n.aliases = v;
        }
        if let Some(v) = patch.summary {
            n.summary = v;
        }
        if let Some(v) = patch.player_text {
            n.player_text = v;
        }
        if let Some(v) = patch.gm_text {
            n.gm_text = v;
        }
        if let Some(v) = patch.payload {
            validate_node_payload(n.kind, &v)?;
            n.payload = v;
        }
        if let Some(v) = patch.tags {
            n.tags = v;
        }
        if let Some(v) = patch.canon_level {
            n.canon_level = v;
        }
        if let Some(v) = patch.locked {
            n.locked = v;
        }
        if let Some(v) = patch.status {
            n.status = v;
        }
        self.put_update(&cur, n)
    }

    /// 移动节点到新父节点下的指定位置（同时改写整棵子树的路径）。
    pub fn move_node(
        &mut self,
        id: &str,
        new_parent: Option<&str>,
        position: Position,
        expected_rev: Option<i64>,
    ) -> Result<Node> {
        let cur = require_node(&self.tx, id)?;
        check_rev("节点", id, expected_rev, cur.rev)?;
        if self.author.is_ai() && (cur.locked || cur.canon_level == CanonLevel::Core) {
            return Err(CoreError::Protected(format!("AI 不能移动受保护的节点「{}」", cur.title)));
        }
        let parent_path = match new_parent {
            Some(p) => {
                let pn = require_node(&self.tx, p)?;
                if pn.path.starts_with(&cur.path) {
                    return Err(CoreError::Invalid("不能把节点移动到自己或自己的子孙节点下".into()));
                }
                pn.path
            }
            None => "/".into(),
        };
        let siblings = sibling_keys(&self.tx, new_parent, Some(id))?;
        let mut n = cur.clone();
        n.sort_key = key_for_position(&siblings, &position)?;
        n.parent_id = new_parent.map(str::to_string);
        n.path = format!("{parent_path}{id}/");
        self.put_update(&cur, n)
    }

    /// 软删除节点及其整棵子树，以及与这些节点相关的关联。可从回收站恢复或撤销。
    pub fn delete_node(&mut self, id: &str, expected_rev: Option<i64>) -> Result<usize> {
        let root = require_node(&self.tx, id)?;
        check_rev("节点", id, expected_rev, root.rev)?;
        let nodes = subtree(&self.tx, id, i64::MAX)?;
        for n in &nodes {
            self.guard(&format!("节点「{}」", n.title), n.locked, n.canon_level == CanonLevel::Core, false)?;
        }
        let ts = self.now;
        for n in &nodes {
            for rel in relations_of(&self.tx, &n.id)? {
                let mut r = rel.clone();
                r.deleted_at = Some(ts);
                self.put_update(&rel, r)?;
            }
            let mut d = n.clone();
            d.deleted_at = Some(ts);
            self.put_update(n, d)?;
        }
        Ok(nodes.len())
    }

    /// 从回收站恢复：恢复节点及同一次删除的后代与关联。父节点必须存在。
    pub fn restore_node(&mut self, id: &str) -> Result<usize> {
        let root = Node::load(&self.tx, id)?.ok_or_else(|| CoreError::NotFound("节点", id.into()))?;
        let Some(ts) = root.deleted_at else {
            return Ok(0);
        };
        if let Some(p) = &root.parent_id {
            require_node(&self.tx, p).map_err(|_| CoreError::Invalid("父节点已删除，请先恢复父节点".into()))?;
        }
        let upper = format!("{}\u{7f}", root.path);
        let nodes: Vec<Node> = {
            let mut stmt = self
                .tx
                .prepare("SELECT * FROM nodes WHERE path >= ?1 AND path < ?2 AND deleted_at = ?3 ORDER BY path")?;
            stmt.query_map(params![root.path, upper, ts], Node::from_row)?.collect::<rusqlite::Result<_>>()?
        };
        for n in &nodes {
            let mut r = n.clone();
            r.deleted_at = None;
            self.put_update(n, r)?;
        }
        let rels: Vec<Relation> = {
            let mut stmt = self.tx.prepare("SELECT * FROM relations WHERE deleted_at = ?1")?;
            stmt.query_map([ts], Relation::from_row)?.collect::<rusqlite::Result<_>>()?
        };
        for rel in rels {
            let src_ok = get_node(&self.tx, &rel.src_id)?.is_some();
            let dst_ok = get_node(&self.tx, &rel.dst_id)?.is_some();
            if src_ok && dst_ok {
                let mut r = rel.clone();
                r.deleted_at = None;
                self.put_update(&rel, r)?;
            }
        }
        Ok(nodes.len())
    }

    pub fn create_relation(&mut self, r: NewRelation) -> Result<Relation> {
        self.create_relation_with_id(new_id(), r)
    }

    pub(crate) fn create_relation_with_id(&mut self, id: String, r: NewRelation) -> Result<Relation> {
        if r.rel_type.trim().is_empty() {
            return Err(CoreError::Invalid("关系类型不能为空".into()));
        }
        if r.src_id == r.dst_id {
            return Err(CoreError::Invalid("关系两端不能是同一节点".into()));
        }
        if let (Some(a), Some(b)) = (r.valid_from, r.valid_to)
            && a > b
        {
            return Err(CoreError::Invalid("关系起始时间晚于结束时间".into()));
        }
        require_node(&self.tx, &r.src_id)?;
        require_node(&self.tx, &r.dst_id)?;
        let rel = Relation {
            id,
            src_id: r.src_id,
            dst_id: r.dst_id,
            rel_type: r.rel_type.trim().to_string(),
            directed: r.directed,
            label: r.label,
            attrs: r.attrs,
            visibility: r.visibility,
            valid_from: r.valid_from,
            valid_to: r.valid_to,
            source: self.author.tag(),
            rev: 1,
            created_at: self.now,
            updated_at: self.now,
            deleted_at: None,
        };
        self.put_new(&rel)?;
        Ok(rel)
    }

    pub fn delete_relation(&mut self, id: &str, expected_rev: Option<i64>) -> Result<()> {
        let cur = get_relation(&self.tx, id)?.ok_or_else(|| CoreError::NotFound("关系", id.into()))?;
        check_rev("关系", id, expected_rev, cur.rev)?;
        // 关系依附于两端节点：任一端受保护时 AI 不能删除。
        if self.author.is_ai() {
            for nid in [&cur.src_id, &cur.dst_id] {
                if let Some(n) = get_node(&self.tx, nid)? {
                    self.guard(&format!("节点「{}」的关系", n.title), n.locked, n.canon_level == CanonLevel::Core, false)?;
                }
            }
        }
        let mut d = cur.clone();
        d.deleted_at = Some(self.now);
        self.put_update(&cur, d)?;
        Ok(())
    }
}

/// 节点是否存在（未删除）。
pub fn node_exists(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM nodes WHERE id = ?1 AND deleted_at IS NULL", [id], |_| Ok(()))
        .optional()?
        .is_some())
}

/// 地点类节点（区域、国家、城市、地点），按名称筛选。用于日程、任务地点等选择。
pub fn find_places(conn: &Connection, query: &str, limit: i64) -> Result<Vec<Node>> {
    let mut stmt = conn.prepare(
        "SELECT * FROM nodes WHERE deleted_at IS NULL AND kind IN ('region','nation','city','site')
         AND (?1 = '' OR instr(title, ?1) > 0 OR instr(aliases, ?1) > 0) ORDER BY path LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![query.trim(), limit.clamp(1, 500)], <Node as crate::entity::Entity>::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

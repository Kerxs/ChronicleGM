//! 关系类型目录、关系编辑、邻域图查询。

use std::collections::{HashSet, VecDeque};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::entity::Entity;
use crate::error::{CoreError, Result};
use crate::model::*;
use crate::nodes::{get_relation, require_node};
use crate::world::{Writer, check_rev};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RelationGroup {
    Person,
    Faction,
    Geography,
    Event,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct RelationType {
    pub id: &'static str,
    pub label: &'static str,
    pub group: RelationGroup,
    /// 是否有方向（如"隶属"有方向，"盟友"无方向）。
    pub directed: bool,
}

const fn rt(id: &'static str, label: &'static str, group: RelationGroup, directed: bool) -> RelationType {
    RelationType { id, label, group, directed }
}

/// 内置关系类型。用户也可以使用目录外的自定义类型。
pub const RELATION_TYPES: &[RelationType] = &[
    rt("family", "亲属", RelationGroup::Person, false),
    rt("parent_of", "父母", RelationGroup::Person, true),
    rt("spouse", "配偶", RelationGroup::Person, false),
    rt("lover", "恋人", RelationGroup::Person, false),
    rt("friend", "朋友", RelationGroup::Person, false),
    rt("mentor_of", "师父", RelationGroup::Person, true),
    rt("rival", "竞争对手", RelationGroup::Person, false),
    rt("ally", "盟友", RelationGroup::Person, false),
    rt("enemy", "敌对", RelationGroup::Person, false),
    rt("serves", "效忠", RelationGroup::Person, true),
    rt("member_of", "隶属", RelationGroup::Faction, true),
    rt("leads", "领导", RelationGroup::Faction, true),
    rt("alliance", "同盟", RelationGroup::Faction, false),
    rt("war", "战争", RelationGroup::Faction, false),
    rt("vassal_of", "附庸", RelationGroup::Faction, true),
    rt("trade", "贸易往来", RelationGroup::Faction, false),
    rt("located_in", "位于", RelationGroup::Geography, true),
    rt("borders", "接壤", RelationGroup::Geography, false),
    rt("route_to", "通往", RelationGroup::Geography, true),
    rt("rules", "统治", RelationGroup::Geography, true),
    rt("participated_in", "参与", RelationGroup::Event, true),
    rt("caused", "导致", RelationGroup::Event, true),
    rt("happened_at", "发生于", RelationGroup::Event, true),
    rt("related_to", "相关", RelationGroup::Other, false),
];

pub fn relation_type(id: &str) -> Option<&'static RelationType> {
    RELATION_TYPES.iter().find(|t| t.id == id)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct RelationPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rel_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Visibility>,
    /// `Some(None)` 表示清除。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<Option<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<Option<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directed: Option<bool>,
}

impl Writer<'_> {
    pub fn update_relation(&mut self, id: &str, p: RelationPatch, expected_rev: Option<i64>) -> Result<Relation> {
        let cur = get_relation(&self.tx, id)?.ok_or_else(|| CoreError::NotFound("关系", id.into()))?;
        check_rev("关系", id, expected_rev, cur.rev)?;
        if self.author.is_ai() {
            for nid in [&cur.src_id, &cur.dst_id] {
                let n = require_node(&self.tx, nid)?;
                self.guard(&format!("节点「{}」的关系", n.title), n.locked, n.canon_level == CanonLevel::Core, false)?;
            }
        }
        let mut r = cur.clone();
        if let Some(v) = p.rel_type {
            if v.trim().is_empty() {
                return Err(CoreError::Invalid("关系类型不能为空".into()));
            }
            r.rel_type = v.trim().into();
        }
        if let Some(v) = p.label {
            r.label = v;
        }
        if let Some(v) = p.visibility {
            r.visibility = v;
        }
        if let Some(v) = p.valid_from {
            r.valid_from = v;
        }
        if let Some(v) = p.valid_to {
            r.valid_to = v;
        }
        if let Some(v) = p.directed {
            r.directed = v;
        }
        if let (Some(a), Some(b)) = (r.valid_from, r.valid_to)
            && a > b
        {
            return Err(CoreError::Invalid("关系起始时间晚于结束时间".into()));
        }
        if r == cur {
            return Ok(cur);
        }
        self.put_update(&cur, r)
    }
}

/// 批量查询子节点数量（用于树的展开标记）。
pub fn child_counts(conn: &Connection, ids: &[String]) -> Result<Vec<(String, i64)>> {
    let mut stmt =
        conn.prepare_cached("SELECT COUNT(*) FROM nodes WHERE parent_id = ?1 AND deleted_at IS NULL")?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push((id.clone(), stmt.query_row([id], |r| r.get(0))?));
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub relations: Vec<Relation>,
    /// 达到数量上限而未展开的节点。
    pub truncated: bool,
}

/// 以某节点为中心，按关系向外扩展 `depth` 层，最多 `limit` 个节点。
pub fn neighborhood(conn: &Connection, center: &str, depth: u32, limit: usize) -> Result<Graph> {
    require_node(conn, center)?;
    let mut seen: HashSet<String> = HashSet::from([center.to_string()]);
    let mut order = vec![center.to_string()];
    let mut rel_ids: HashSet<String> = HashSet::new();
    let mut relations = vec![];
    let mut queue = VecDeque::from([(center.to_string(), 0u32)]);
    let mut truncated = false;
    let mut stmt = conn.prepare_cached(
        "SELECT * FROM (
            SELECT * FROM relations WHERE src_id = ?1 AND deleted_at IS NULL
            UNION ALL
            SELECT * FROM relations WHERE dst_id = ?1 AND src_id <> ?1 AND deleted_at IS NULL
         ) ORDER BY created_at LIMIT 500",
    )?;
    while let Some((id, d)) = queue.pop_front() {
        if d >= depth {
            continue;
        }
        let rels: Vec<Relation> = stmt.query_map([&id], Relation::from_row)?.collect::<rusqlite::Result<_>>()?;
        for r in rels {
            let other = if r.src_id == id { r.dst_id.clone() } else { r.src_id.clone() };
            if !seen.contains(&other) {
                if seen.len() >= limit {
                    truncated = true;
                    continue;
                }
                seen.insert(other.clone());
                order.push(other.clone());
                queue.push_back((other, d + 1));
            }
            if rel_ids.insert(r.id.clone()) {
                relations.push(r);
            }
        }
    }
    let mut nodes = Vec::with_capacity(order.len());
    for id in &order {
        if let Some(n) = Node::load_active(conn, id)? {
            nodes.push(n);
        }
    }
    relations.retain(|r| seen.contains(&r.src_id) && seen.contains(&r.dst_id));
    Ok(Graph { nodes, relations, truncated })
}

/// 节点数量统计（按类型）。
pub fn kind_counts(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare("SELECT kind, COUNT(*) FROM nodes WHERE deleted_at IS NULL GROUP BY kind ORDER BY 2 DESC")?;
    let rows = stmt.query_map(params![], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

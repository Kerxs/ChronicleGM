//! 人物关系网络：世界正典里人物 / 势力之间的关系，叠加某个战役中的动态关系
//! （双方各自的看法：强度、信任、好感、敌意）。关系的形成原因与变化历史从事件日志读取，
//! 每一次变化都对应一条事件，随时间线回溯一起恢复。

use std::collections::{BTreeMap, HashMap, HashSet};

use chronicle_rules::effect::Bond;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::model::NodeKind;

/// 网络中的"玩家"节点 ID（与物品持有者 `party` 一致）。
pub const PARTY: &str = "party";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NetNode {
    pub id: String,
    pub title: String,
    /// character / faction / party / 其他节点类型
    pub kind: String,
    pub summary: String,
    /// 战役中的生命状态等覆盖（例如 wounded / dead）。
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NetEdge {
    /// world:<关系ID> 或 bond:<a>|<b>
    pub id: String,
    pub a: String,
    pub b: String,
    /// world（世界设定中的关系）/ campaign（本战役中的动态关系）
    pub source: String,
    pub rel_type: String,
    pub label: String,
    pub directed: bool,
    /// 战役中该关系被设为不成立（set_relation active=false）。
    pub inactive: bool,
    /// a 对 b 的看法、b 对 a 的看法（只有战役关系有）。
    pub a_to_b: Option<Bond>,
    pub b_to_a: Option<Bond>,
    pub kind_a_to_b: String,
    pub kind_b_to_a: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Network {
    pub nodes: Vec<NetNode>,
    pub edges: Vec<NetEdge>,
    /// 节点数超过上限时被截断（用搜索或筛选缩小范围）。
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NetworkQuery {
    pub campaign_id: Option<String>,
    /// 名称包含这些文字的节点及其直接关系对象。
    #[serde(default)]
    pub search: String,
    /// 只显示这些关系类型（空 = 全部）。
    #[serde(default)]
    pub rel_types: Vec<String>,
    /// 包含世界设定中的关系。
    #[serde(default = "yes")]
    pub include_world: bool,
    /// 战役关系强度至少多少才显示。
    #[serde(default)]
    pub min_strength: i64,
    /// 最多节点数（默认 400）。
    #[serde(default)]
    pub limit: usize,
}

fn yes() -> bool {
    true
}

/// 人物关系网络包含的节点类型：人物、势力（家族、组织都用势力表示）、种族。
pub const PEOPLE_KINDS: &[NodeKind] = &[NodeKind::Character, NodeKind::Faction, NodeKind::Race];

fn bonds_of(conn: &Connection, campaign_id: &str) -> Result<Vec<(String, String, String, Bond)>> {
    let mut stmt = conn.prepare("SELECT from_id, to_id, kind, strength, trust, affection, hostility FROM campaign_bonds WHERE campaign_id = ?1")?;
    let rows = stmt.query_map([campaign_id], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, Bond { strength: r.get(3)?, trust: r.get(4)?, affection: r.get(5)?, hostility: r.get(6)? }))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn network(conn: &Connection, q: &NetworkQuery) -> Result<Network> {
    let limit = if q.limit == 0 { 400 } else { q.limit.min(3000) };
    let kinds: Vec<String> = PEOPLE_KINDS.iter().map(|k| format!("'{}'", k.as_str())).collect();
    // 1. 候选节点：人物、势力、种族（搜索时只取名称匹配的）。
    let mut nodes: BTreeMap<String, NetNode> = BTreeMap::new();
    {
        let sql = format!(
            "SELECT id, title, kind, summary FROM nodes WHERE deleted_at IS NULL AND kind IN ({}) AND (?1 = '' OR instr(title, ?1) > 0 OR instr(aliases, ?1) > 0) ORDER BY path LIMIT ?2",
            kinds.join(",")
        );
        let mut stmt = conn.prepare(&sql)?;
        for r in stmt.query_map(params![q.search.trim(), limit as i64 + 1], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))? {
            let (id, title, kind, summary) = r?;
            nodes.insert(id.clone(), NetNode { id, title, kind, summary, status: None });
        }
    }
    let mut truncated = nodes.len() > limit;
    while nodes.len() > limit {
        let k = nodes.keys().next_back().cloned().unwrap();
        nodes.remove(&k);
    }
    let type_ok = |t: &str| q.rel_types.is_empty() || q.rel_types.iter().any(|x| x == t);
    let mut edges = vec![];
    // 2. 世界关系（两端都是人物 / 势力 / 种族；搜索时把直接关系对象也带进来）。
    let overrides: HashMap<(String, String, String), bool> = match &q.campaign_id {
        Some(c) => {
            let mut stmt = conn.prepare("SELECT a, b, rel_type, active FROM relation_overrides WHERE campaign_id = ?1")?;
            stmt.query_map([c], |r| Ok(((r.get(0)?, r.get(1)?, r.get(2)?), r.get::<_, bool>(3)?)))?.collect::<rusqlite::Result<_>>()?
        }
        None => HashMap::new(),
    };
    if q.include_world && !nodes.is_empty() {
        let ids: Vec<String> = nodes.keys().cloned().collect();
        let mut stmt = conn.prepare(&format!(
            "SELECT r.id, r.src_id, r.dst_id, r.rel_type, r.label, r.directed FROM relations r
             JOIN nodes a ON a.id = r.src_id AND a.deleted_at IS NULL AND a.kind IN ({k})
             JOIN nodes b ON b.id = r.dst_id AND b.deleted_at IS NULL AND b.kind IN ({k})
             WHERE r.deleted_at IS NULL AND (r.src_id IN (SELECT value FROM json_each(?1)) OR r.dst_id IN (SELECT value FROM json_each(?1)))",
            k = kinds.join(",")
        ))?;
        let rows: Vec<(String, String, String, String, String, bool)> = stmt
            .query_map([serde_json::to_string(&ids)?], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, a, b, t, label, directed) in rows {
            if !type_ok(&t) {
                continue;
            }
            let inactive = overrides.get(&(a.clone(), b.clone(), t.clone())) == Some(&false);
            edges.push(NetEdge { id: format!("world:{id}"), a, b, source: "world".into(), rel_type: t, label, directed, inactive, a_to_b: None, b_to_a: None, kind_a_to_b: String::new(), kind_b_to_a: String::new() });
        }
    }
    // 3. 战役中的动态关系：把 a→b 与 b→a 合成一条边，保留双方各自的看法。
    if let Some(c) = &q.campaign_id {
        let mut pairs: BTreeMap<(String, String), NetEdge> = BTreeMap::new();
        for (from, to, kind, bond) in bonds_of(conn, c)? {
            let (a, b) = if from <= to { (from.clone(), to.clone()) } else { (to.clone(), from.clone()) };
            let e = pairs.entry((a.clone(), b.clone())).or_insert_with(|| NetEdge {
                id: format!("bond:{a}|{b}"),
                a: a.clone(),
                b: b.clone(),
                source: "campaign".into(),
                rel_type: String::new(),
                label: String::new(),
                directed: false,
                inactive: false,
                a_to_b: None,
                b_to_a: None,
                kind_a_to_b: String::new(),
                kind_b_to_a: String::new(),
            });
            if from == a {
                e.a_to_b = Some(bond);
                e.kind_a_to_b = kind;
            } else {
                e.b_to_a = Some(bond);
                e.kind_b_to_a = kind;
            }
        }
        // 搜索时只保留与搜索结果直接相连的战役关系。
        let matched: HashSet<String> = nodes.keys().cloned().collect();
        let searching = !q.search.trim().is_empty();
        for mut e in pairs.into_values() {
            if searching && !matched.contains(&e.a) && !matched.contains(&e.b) {
                continue;
            }
            e.directed = e.a_to_b.is_none() || e.b_to_a.is_none();
            e.rel_type = if !e.kind_a_to_b.is_empty() { e.kind_a_to_b.clone() } else { e.kind_b_to_a.clone() };
            let strength = e.a_to_b.map(|x| x.strength).max(e.b_to_a.map(|x| x.strength)).unwrap_or(0);
            if strength < q.min_strength || !(type_ok(&e.rel_type) || e.rel_type.is_empty() && q.rel_types.is_empty()) {
                continue;
            }
            edges.push(e);
        }
    }
    // 4. 把边的另一端补进节点（搜索结果的直接关系对象、玩家、临时人物）。
    let present: HashSet<String> = nodes.keys().cloned().collect();
    let mut missing: Vec<String> = edges.iter().flat_map(|e| [e.a.clone(), e.b.clone()]).filter(|x| !present.contains(x)).collect();
    missing.sort();
    missing.dedup();
    for id in missing {
        if id == PARTY {
            nodes.insert(id.clone(), NetNode { id, title: "玩家".into(), kind: "party".into(), summary: "玩家队伍".into(), status: None });
            continue;
        }
        if nodes.len() >= limit + 200 {
            truncated = true;
            break;
        }
        if let Some(n) = crate::nodes::get_node(conn, &id)? {
            nodes.insert(id, NetNode { id: n.id, title: n.title, kind: n.kind.as_str().into(), summary: n.summary, status: None });
        }
    }
    edges.retain(|e| nodes.contains_key(&e.a) && nodes.contains_key(&e.b));
    // 5. 战役中的生命状态等覆盖。
    if let Some(c) = &q.campaign_id {
        let mut stmt = conn.prepare("SELECT entity_id, value FROM entity_overrides WHERE campaign_id = ?1 AND field = 'life_status'")?;
        for r in stmt.query_map([c], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (id, v) = r?;
            if let Some(n) = nodes.get_mut(&id) {
                n.status = serde_json::from_str::<Value>(&v).ok().and_then(|v| v.as_str().map(str::to_string)).or(Some(v));
            }
        }
    }
    Ok(Network { nodes: nodes.into_values().collect(), edges, truncated })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct BondChange {
    /// 来源事件序号（关系变化必须有来源事件）。
    pub seq: i64,
    pub turn: i64,
    pub from: String,
    pub to: String,
    pub old_kind: String,
    pub kind: String,
    pub old: Bond,
    pub new: Bond,
    pub reason: String,
    /// 当回合玩家的行动（帮助理解关系为什么变化）。
    pub player_action: String,
}

/// 两人之间关系的全部变化（双方的看法都包括），按时间顺序。
pub fn bond_history(conn: &Connection, campaign_id: &str, a: &str, b: &str) -> Result<Vec<BondChange>> {
    let mut stmt = conn.prepare(
        "SELECT seq, turn, kind, payload FROM event_log WHERE campaign_id = ?1 AND kind IN ('effects', 'transition') AND instr(payload, 'adjust_bond') > 0 ORDER BY seq",
    )?;
    let rows: Vec<(i64, i64, String, String)> = stmt.query_map([campaign_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut out = vec![];
    for (seq, turn, kind, payload) in rows {
        let v: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
        let changes = if kind == "effects" { &v["changes"] } else { &v["outcome"]["changes"] };
        let Ok(list) = serde_json::from_value::<Vec<chronicle_rules::StateChange>>(changes.clone()) else { continue };
        for ch in list {
            if let chronicle_rules::StateChange::AdjustBond { from, to, old_kind, kind, old, new, reason } = ch
                && ((from == a && to == b) || (from == b && to == a))
            {
                let player_action: String = conn
                    .query_row(
                        "SELECT payload FROM event_log WHERE campaign_id = ?1 AND kind = 'player' AND seq < ?2 ORDER BY seq DESC LIMIT 1",
                        params![campaign_id, seq],
                        |r| r.get::<_, String>(0),
                    )
                    .ok()
                    .and_then(|p| serde_json::from_str::<Value>(&p).ok())
                    .and_then(|p| p["text"].as_str().map(|s| s.chars().take(80).collect()))
                    .unwrap_or_default();
                out.push(BondChange { seq, turn, from, to, old_kind, kind, old, new, reason, player_action });
            }
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct BondSummary {
    pub other: String,
    pub other_title: String,
    pub kind: String,
    /// 这个人对对方的看法。
    pub outgoing: Option<Bond>,
    /// 对方对这个人的看法。
    pub incoming: Option<Bond>,
}

/// 人物档案用：某人在战役中的全部关系。
pub fn bonds_of_person(conn: &Connection, campaign_id: &str, id: &str) -> Result<Vec<BondSummary>> {
    let mut map: BTreeMap<String, BondSummary> = BTreeMap::new();
    for (from, to, kind, bond) in bonds_of(conn, campaign_id)? {
        let (other, out) = if from == id {
            (to, true)
        } else if to == id {
            (from, false)
        } else {
            continue;
        };
        let title = if other == PARTY { "玩家".to_string() } else { crate::nodes::get_node(conn, &other)?.map(|n| n.title).unwrap_or_else(|| other.clone()) };
        let e = map.entry(other.clone()).or_insert(BondSummary { other, other_title: title, kind: String::new(), outgoing: None, incoming: None });
        if out {
            e.outgoing = Some(bond);
            e.kind = kind;
        } else {
            e.incoming = Some(bond);
            if e.kind.is_empty() {
                e.kind = kind;
            }
        }
    }
    Ok(map.into_values().collect())
}

/// 世界图谱用：给定节点之间的世界关系（只取两端都在集合里的）。
pub fn relations_among(conn: &Connection, ids: &[String]) -> Result<Vec<crate::model::Relation>> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let json = serde_json::to_string(ids)?;
    let mut stmt = conn.prepare(
        "SELECT r.* FROM relations r
         WHERE r.deleted_at IS NULL
           AND r.src_id IN (SELECT value FROM json_each(?1))
           AND r.dst_id IN (SELECT value FROM json_each(?1))
         LIMIT 5000",
    )?;
    let rows = stmt.query_map([json], <crate::model::Relation as crate::entity::Entity>::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

//! 全世界一致性检查：找出设定之间的矛盾与重复。只读，不修改任何内容。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::changeset::{OPPOSING_RELATIONS, Severity};
use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Finding {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub node_ids: Vec<String>,
    pub relation_ids: Vec<String>,
}

const LIMIT: i64 = 200;

pub fn check_world(conn: &Connection) -> Result<Vec<Finding>> {
    Ok(check_world_profiled(conn)?.0)
}

/// 每一项检查的名称与耗时（毫秒）。
pub type Timings = Vec<(&'static str, f64)>;

/// 同 [`check_world`]，并返回每一项检查的耗时，用于性能分析。
pub fn check_world_profiled(conn: &Connection) -> Result<(Vec<Finding>, Timings)> {
    let mut out = vec![];
    let mut timings = vec![];
    let mut t = std::time::Instant::now();
    let mut lap = |name: &'static str, timings: &mut Vec<(&'static str, f64)>| {
        timings.push((name, t.elapsed().as_secs_f64() * 1000.0));
        t = std::time::Instant::now();
    };

    // 1. 同一对节点之间、时间重叠的对立关系（如同时是盟友和敌人）。
    for (a, b) in OPPOSING_RELATIONS {
        // 正向与反向两段分别走 (src_id, dst_id, rel_type) 索引；OR 写法会变成平方级扫描。
        let overlap = "AND COALESCE(r1.valid_from, -9223372036854775807) <= COALESCE(r2.valid_to, 9223372036854775807)
                       AND COALESCE(r2.valid_from, -9223372036854775807) <= COALESCE(r1.valid_to, 9223372036854775807)";
        let mut stmt = conn.prepare(&format!(
            "SELECT r1.id, r2.id, n1.id, n1.title, n2.id, n2.title FROM relations r1
             JOIN relations r2 ON r2.src_id = r1.src_id AND r2.dst_id = r1.dst_id AND r2.rel_type = ?2 AND r2.deleted_at IS NULL {overlap}
             JOIN nodes n1 ON n1.id = r1.src_id JOIN nodes n2 ON n2.id = r1.dst_id
             WHERE r1.rel_type = ?1 AND r1.deleted_at IS NULL
             UNION ALL
             SELECT r1.id, r2.id, n1.id, n1.title, n2.id, n2.title FROM relations r1
             JOIN relations r2 ON r2.src_id = r1.dst_id AND r2.dst_id = r1.src_id AND r2.rel_type = ?2 AND r2.deleted_at IS NULL {overlap}
             JOIN nodes n1 ON n1.id = r1.src_id JOIN nodes n2 ON n2.id = r1.dst_id
             WHERE r1.rel_type = ?1 AND r1.deleted_at IS NULL
             LIMIT ?3"
        ))?;
        let rows = stmt.query_map(rusqlite::params![a, b, LIMIT], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?))
        })?;
        for row in rows {
            let (r1, r2, i1, t1, i2, t2) = row?;
            out.push(Finding {
                code: "relation_conflict".into(),
                severity: Severity::Warning,
                message: format!("「{t1}」与「{t2}」在同一时期既是「{a}」又是「{b}」"),
                node_ids: vec![i1, i2],
                relation_ids: vec![r1, r2],
            });
        }
    }

    lap("对立关系", &mut timings);

    // 2. 人物死亡之后仍参与事件；出生之前参与事件。
    let mut stmt = conn.prepare(
        "SELECT r.id, c.id, c.title, e.id, e.title,
                json_extract(c.payload, '$.life_status'), json_extract(c.payload, '$.death_time'),
                json_extract(c.payload, '$.birth_time'), json_extract(e.payload, '$.world_time')
         FROM relations r JOIN nodes c ON c.id = r.src_id JOIN nodes e ON e.id = r.dst_id
         WHERE r.rel_type = 'participated_in' AND r.deleted_at IS NULL AND c.deleted_at IS NULL AND e.deleted_at IS NULL
           AND c.kind = 'character' AND json_type(e.payload, '$.world_time') = 'integer'
           AND ((json_type(c.payload, '$.death_time') = 'integer' AND json_extract(e.payload, '$.world_time') > json_extract(c.payload, '$.death_time'))
             OR (json_type(c.payload, '$.birth_time') = 'integer' AND json_extract(e.payload, '$.world_time') < json_extract(c.payload, '$.birth_time')))
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([LIMIT], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Option<i64>>(6)?,
            r.get::<_, Option<i64>>(7)?,
            r.get::<_, i64>(8)?,
        ))
    })?;
    for row in rows {
        let (rid, cid, ct, eid, et, death, birth, when) = row?;
        let msg = if death.is_some_and(|d| when > d) {
            format!("「{ct}」死于 {} 之后，却参与了发生在 {when} 的「{et}」", death.unwrap())
        } else {
            format!("「{ct}」出生于 {} ，却参与了更早（{when}）的「{et}」", birth.unwrap_or_default())
        };
        out.push(Finding { code: "timeline_conflict".into(), severity: Severity::Warning, message: msg, node_ids: vec![cid, eid], relation_ids: vec![rid] });
    }

    lap("时间线", &mut timings);

    // 3. 同类型的重名节点。
    let mut stmt = conn.prepare(
        "SELECT k.name_key, n.kind, group_concat(DISTINCT n.id), group_concat(DISTINCT n.title), COUNT(DISTINCT n.id) c
         FROM node_names k JOIN nodes n ON n.id = k.node_id AND n.deleted_at IS NULL
         GROUP BY k.name_key, n.kind HAVING c > 1 LIMIT ?1",
    )?;
    let rows = stmt.query_map([LIMIT], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))?;
    for row in rows {
        let (kind, ids, titles) = row?;
        out.push(Finding {
            code: "duplicate_name".into(),
            severity: Severity::Warning,
            message: format!("多个「{kind}」节点名称（或别名）相同：{}", titles.replace(',', "、")),
            node_ids: ids.split(',').map(str::to_string).collect(),
            relation_ids: vec![],
        });
    }

    // 跨类型重名（例如人物和地点同名），容易让 GM 混淆。
    let mut stmt = conn.prepare(
        "SELECT group_concat(DISTINCT n.id), group_concat(DISTINCT n.title), group_concat(DISTINCT n.kind), COUNT(DISTINCT n.kind) c
         FROM node_names k JOIN nodes n ON n.id = k.node_id AND n.deleted_at IS NULL
         GROUP BY k.name_key HAVING c > 1 LIMIT ?1",
    )?;
    let rows = stmt.query_map([LIMIT], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
    for row in rows {
        let (ids, titles, kinds) = row?;
        out.push(Finding {
            code: "duplicate_name_cross_kind".into(),
            severity: Severity::Warning,
            message: format!("不同类型的节点同名：{}（{}）", titles.replace(',', "、"), kinds.replace(',', "、")),
            node_ids: ids.split(',').map(str::to_string).collect(),
            relation_ids: vec![],
        });
    }

    lap("重名", &mut timings);

    // 4. 指向已删除节点的关系（正常流程不会出现，出现说明数据异常）。
    let mut stmt = conn.prepare(
        // 只查主键索引（NOT EXISTS）与已删除节点的小集合，避免逐条回表读取节点整行。
        "SELECT r.id, r.src_id, r.dst_id FROM relations r
         WHERE r.deleted_at IS NULL AND (
            r.src_id IN (SELECT id FROM nodes WHERE deleted_at IS NOT NULL)
            OR r.dst_id IN (SELECT id FROM nodes WHERE deleted_at IS NOT NULL)
            OR NOT EXISTS (SELECT 1 FROM nodes WHERE id = r.src_id)
            OR NOT EXISTS (SELECT 1 FROM nodes WHERE id = r.dst_id))
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([LIMIT], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
    for row in rows {
        let (rid, a, b) = row?;
        out.push(Finding {
            code: "dangling_relation".into(),
            severity: Severity::Error,
            message: "关系的一端节点已被删除".into(),
            node_ids: vec![a, b],
            relation_ids: vec![rid],
        });
    }
    lap("悬空关系", &mut timings);
    Ok((out, timings))
}

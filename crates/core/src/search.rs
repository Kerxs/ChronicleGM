//! 世界树全文搜索。
//!
//! 使用 FTS5 trigram 分词，支持中文子串搜索。trigram 要求查询至少 3 个字符，
//! 少于 3 个字符（如两个字的人名）时回退到 LIKE 扫描。

use rusqlite::{Connection, params_from_iter, types::Value as SqlValue};
use serde::{Deserialize, Serialize};

use crate::entity::Entity;
use crate::error::Result;
use crate::model::{Node, NodeKind};

#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SearchOptions {
    /// 只搜索这些类型；为空表示全部。
    #[serde(default)]
    pub kinds: Vec<NodeKind>,
    /// 玩家视角：不搜索主持人文本，结果也去掉隐藏内容。
    #[serde(default)]
    pub player_view: bool,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SearchHit {
    pub node: Node,
    /// 是否命中全文索引（false 表示走了 LIKE 回退）。
    pub indexed: bool,
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

pub fn search_nodes(conn: &Connection, query: &str, opts: &SearchOptions) -> Result<Vec<SearchHit>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let limit = opts.limit.clamp(1, 500);
    let mut args: Vec<SqlValue> = Vec::new();
    let kind_filter = if opts.kinds.is_empty() {
        String::new()
    } else {
        let marks = vec!["?"; opts.kinds.len()].join(",");
        format!(" AND n.kind IN ({marks})")
    };
    let indexed = q.chars().count() >= 3;
    let sql = if indexed {
        let phrase = format!("\"{}\"", q.replace('"', "\"\""));
        let expr = if opts.player_view {
            format!("{{title aliases summary player_text}} : {phrase}")
        } else {
            phrase
        };
        args.push(SqlValue::Text(expr));
        format!(
            "SELECT n.* FROM nodes_fts f JOIN nodes n ON n.rowid = f.rowid
             WHERE nodes_fts MATCH ?1 AND n.deleted_at IS NULL{kind_filter}
             ORDER BY bm25(nodes_fts, 10.0, 8.0, 3.0, 1.0, 1.0) LIMIT {limit} OFFSET {}",
            opts.offset.max(0)
        )
    } else {
        args.push(SqlValue::Text(format!("%{}%", escape_like(q))));
        let gm = if opts.player_view { "" } else { " OR n.gm_text LIKE ?1 ESCAPE '\\'" };
        format!(
            "SELECT n.* FROM nodes n
             WHERE n.deleted_at IS NULL AND (n.title LIKE ?1 ESCAPE '\\' OR n.aliases LIKE ?1 ESCAPE '\\'
                OR n.summary LIKE ?1 ESCAPE '\\' OR n.player_text LIKE ?1 ESCAPE '\\'{gm}){kind_filter}
             ORDER BY (n.title LIKE ?1 ESCAPE '\\') DESC, length(n.title) LIMIT {limit} OFFSET {}",
            opts.offset.max(0)
        )
    };
    for k in &opts.kinds {
        args.push(SqlValue::Text(k.as_str().into()));
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), Node::from_row)?;
    let mut out = Vec::new();
    for n in rows {
        let n = n?;
        out.push(SearchHit { node: if opts.player_view { n.player_view() } else { n }, indexed });
    }
    Ok(out)
}

/// 剧本条目搜索（标题、玩家文本，GM 视角下包括主持人文本）。
pub fn search_script_items(
    conn: &Connection,
    script_id: Option<&str>,
    query: &str,
    player_view: bool,
    limit: i64,
) -> Result<Vec<crate::model::ScriptItem>> {
    use crate::model::ScriptItem;
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let limit = limit.clamp(1, 500);
    let sql = if q.chars().count() >= 3 {
        format!(
            "SELECT s.* FROM script_items_fts f JOIN script_items s ON s.rowid = f.rowid
             WHERE script_items_fts MATCH ?1 AND s.deleted_at IS NULL AND (?2 IS NULL OR s.script_id = ?2)
             ORDER BY bm25(script_items_fts, 10.0, 1.0, 1.0) LIMIT {limit}"
        )
    } else {
        let gm = if player_view { "" } else { " OR s.gm_text LIKE ?1 ESCAPE '\\'" };
        format!(
            "SELECT s.* FROM script_items s WHERE s.deleted_at IS NULL AND (?2 IS NULL OR s.script_id = ?2)
             AND (s.title LIKE ?1 ESCAPE '\\' OR s.player_text LIKE ?1 ESCAPE '\\'{gm}) LIMIT {limit}"
        )
    };
    let arg = if q.chars().count() >= 3 {
        let phrase = format!("\"{}\"", q.replace('"', "\"\""));
        if player_view { format!("{{title player_text}} : {phrase}") } else { phrase }
    } else {
        format!("%{}%", escape_like(q))
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params![arg, script_id], ScriptItem::from_row)?;
    let mut out: Vec<ScriptItem> = rows.collect::<rusqlite::Result<_>>()?;
    if player_view {
        for it in &mut out {
            it.gm_text.clear();
        }
    }
    Ok(out)
}

// ===================== 多条件筛选（第 18 项） =====================

/// 一个筛选条件：payload 字段（如 rarity、life_status、world_time），或特殊字段
/// `related`（和某个节点有关系，值是节点 ID）、`under`（在某个节点下面，值是节点 ID）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct FieldCond {
    pub field: String,
    /// eq（等于）/ contains（包含）/ gte（不小于）/ lte（不大于）/ exists（有这个字段）
    pub op: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NodeFilter {
    #[serde(default)]
    pub kinds: Vec<NodeKind>,
    /// 名称、简介里包含的文字（可空）。
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub conds: Vec<FieldCond>,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn field_ok(f: &str) -> bool {
    !f.is_empty() && f.len() <= 40 && f.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// 按条件筛选节点（所有条件同时满足）。
pub fn filter_nodes(conn: &Connection, f: &NodeFilter) -> Result<Vec<Node>> {
    let mut sql = String::from("SELECT n.* FROM nodes n WHERE n.deleted_at IS NULL");
    let mut args: Vec<SqlValue> = Vec::new();
    if !f.kinds.is_empty() {
        sql.push_str(&format!(" AND n.kind IN ({})", vec!["?"; f.kinds.len()].join(",")));
        args.extend(f.kinds.iter().map(|k| SqlValue::Text(k.as_str().into())));
    }
    if !f.text.trim().is_empty() {
        sql.push_str(" AND (n.title LIKE ? ESCAPE '\' OR n.aliases LIKE ? ESCAPE '\' OR n.summary LIKE ? ESCAPE '\')");
        let like = format!("%{}%", escape_like(f.text.trim()));
        args.extend(std::iter::repeat_n(SqlValue::Text(like), 3));
    }
    for c in f.conds.iter().take(8) {
        match c.field.as_str() {
            "related" => {
                sql.push_str(" AND EXISTS (SELECT 1 FROM relations r WHERE r.deleted_at IS NULL AND ((r.src_id = n.id AND r.dst_id = ?) OR (r.dst_id = n.id AND r.src_id = ?)))");
                args.push(SqlValue::Text(c.value.clone()));
                args.push(SqlValue::Text(c.value.clone()));
            }
            "under" => {
                sql.push_str(" AND n.id <> ? AND n.path LIKE (SELECT p.path FROM nodes p WHERE p.id = ?) || '%'");
                args.push(SqlValue::Text(c.value.clone()));
                args.push(SqlValue::Text(c.value.clone()));
            }
            field if field_ok(field) => {
                let x = format!("json_extract(n.payload, '$.{field}')");
                match c.op.as_str() {
                    "exists" => sql.push_str(&format!(" AND {x} IS NOT NULL AND {x} <> ''")),
                    "contains" => {
                        sql.push_str(&format!(" AND CAST({x} AS TEXT) LIKE ? ESCAPE '\'"));
                        args.push(SqlValue::Text(format!("%{}%", escape_like(&c.value))));
                    }
                    "gte" | "lte" => {
                        let v: f64 = c.value.trim().parse().map_err(|_| crate::error::CoreError::Invalid(format!("「{}」需要填数字", c.field)))?;
                        sql.push_str(&format!(" AND CAST({x} AS REAL) {} ?", if c.op == "gte" { ">=" } else { "<=" }));
                        args.push(SqlValue::Real(v));
                    }
                    _ => {
                        sql.push_str(&format!(" AND CAST({x} AS TEXT) = ?"));
                        args.push(SqlValue::Text(c.value.clone()));
                    }
                }
            }
            other => return Err(crate::error::CoreError::Invalid(format!("不能按「{other}」筛选"))),
        }
    }
    let limit = if f.limit <= 0 { 200 } else { f.limit.min(500) };
    sql.push_str(&format!(" ORDER BY n.kind, n.title LIMIT {limit}"));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), Node::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 某类节点 payload 里出现过的字段与常见取值（给筛选界面用；最多看 500 个节点）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct FieldInfo {
    pub field: String,
    /// 出现的节点数。
    pub count: i64,
    /// 是否都是数字（可以按范围筛选）。
    pub numeric: bool,
    /// 常见取值（最多 12 个；数组字段展开）。
    pub samples: Vec<String>,
}

pub fn payload_fields(conn: &Connection, kind: Option<NodeKind>) -> Result<Vec<FieldInfo>> {
    let mut stmt = conn.prepare("SELECT payload FROM nodes WHERE deleted_at IS NULL AND (?1 IS NULL OR kind = ?1) LIMIT 500")?;
    let rows: Vec<String> = stmt.query_map([kind.map(|k| k.as_str())], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let mut map: std::collections::BTreeMap<String, (i64, bool, std::collections::BTreeMap<String, i64>)> = Default::default();
    for p in rows {
        let Ok(serde_json::Value::Object(o)) = serde_json::from_str::<serde_json::Value>(&p) else { continue };
        for (k, v) in o {
            if !field_ok(&k) || k.starts_with("gm_") || crate::payload::gm_fields(kind.unwrap_or(NodeKind::Character)).contains(&k.as_str()) {
                continue;
            }
            let e = map.entry(k).or_insert((0, true, Default::default()));
            e.0 += 1;
            let vals: Vec<serde_json::Value> = match v {
                serde_json::Value::Array(a) => a,
                x => vec![x],
            };
            for x in vals {
                let s = match &x {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    _ => continue,
                };
                if !x.is_number() {
                    e.1 = false;
                }
                if s.chars().count() <= 20 {
                    *e.2.entry(s).or_insert(0) += 1;
                }
            }
        }
    }
    let mut out: Vec<FieldInfo> = map
        .into_iter()
        .map(|(field, (count, numeric, vals))| {
            let mut v: Vec<(String, i64)> = vals.into_iter().collect();
            v.sort_by_key(|x| std::cmp::Reverse(x.1));
            FieldInfo { field, count, numeric, samples: v.into_iter().take(12).map(|x| x.0).collect() }
        })
        .collect();
    out.sort_by_key(|x| std::cmp::Reverse(x.count));
    Ok(out)
}

// ===================== 全局搜索 =====================

/// 全局搜索的一条结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct GlobalHit {
    /// node / script / script_item / campaign / quest / event
    pub kind: String,
    pub id: String,
    pub title: String,
    /// 补充说明（类型、所属剧本、战役名、片段）。
    pub detail: String,
    /// 跳转用：剧本条目的剧本 ID、事件的战役 ID。
    #[serde(default)]
    pub parent: String,
}

fn snip(text: &str, q: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let pos = text.find(q).map(|b| text[..b].chars().count()).unwrap_or(0);
    let start = pos.saturating_sub(20);
    let end = (pos + q.chars().count() + 40).min(chars.len());
    let mut s: String = chars[start..end].iter().collect();
    if start > 0 {
        s = format!("…{s}");
    }
    if end < chars.len() {
        s.push('…');
    }
    s.replace('\n', " ")
}

/// 在整个世界里搜索：世界树节点、剧本与剧本条目、战役、任务、游戏记录。
/// `spoilers` 为 false 时不搜主持人文字，结果里也不含主持人信息与主持人事件。
pub fn global_search(conn: &Connection, query: &str, spoilers: bool, limit_each: i64) -> Result<Vec<GlobalHit>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let lim = limit_each.clamp(1, 50);
    let mut out = vec![];
    for h in search_nodes(conn, q, &SearchOptions { kinds: vec![], player_view: !spoilers, limit: lim, offset: 0 })? {
        let n = h.node;
        let text = if n.summary.is_empty() { n.player_text.clone() } else { n.summary.clone() };
        out.push(GlobalHit { kind: "node".into(), id: n.id, title: n.title, detail: format!("{} · {}", n.kind.as_str(), snip(&text, q)), parent: String::new() });
    }
    let like = format!("%{}%", escape_like(q));
    {
        let mut stmt = conn.prepare("SELECT id, title, genre FROM scripts WHERE deleted_at IS NULL AND (title LIKE ?1 ESCAPE '\\' OR premise LIKE ?1 ESCAPE '\\' OR genre LIKE ?1 ESCAPE '\\') LIMIT ?2")?;
        for r in stmt.query_map(rusqlite::params![like, lim], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))? {
            let (id, title, genre) = r?;
            out.push(GlobalHit { kind: "script".into(), id, title, detail: if genre.is_empty() { "剧本".into() } else { format!("剧本 · {genre}") }, parent: String::new() });
        }
    }
    let script_titles: std::collections::HashMap<String, String> = {
        let mut stmt = conn.prepare("SELECT id, title FROM scripts WHERE deleted_at IS NULL")?;
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.filter_map(|x| x.ok()).collect()
    };
    for it in search_script_items(conn, None, q, !spoilers, lim)? {
        let Some(st) = script_titles.get(&it.script_id) else { continue };
        out.push(GlobalHit { kind: "script_item".into(), detail: format!("{} · {}", st, snip(&it.player_text, q)), id: it.id, title: it.title, parent: it.script_id });
    }
    {
        let mut stmt = conn.prepare("SELECT id, name, turn FROM campaigns WHERE deleted_at IS NULL AND name LIKE ?1 ESCAPE '\\' LIMIT ?2")?;
        for r in stmt.query_map(rusqlite::params![like, lim], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))? {
            let (id, name, turn) = r?;
            out.push(GlobalHit { kind: "campaign".into(), id: id.clone(), title: name, detail: format!("战役 · 第 {turn} 回合"), parent: id });
        }
    }
    {
        // 游戏中新建的任务（战役记录）。
        let mut stmt = conn.prepare("SELECT r.campaign_id, r.id, r.data, c.name FROM campaign_records r JOIN campaigns c ON c.id = r.campaign_id AND c.deleted_at IS NULL WHERE r.kind = 'quest' AND r.data LIKE ?1 ESCAPE '\\' LIMIT ?2")?;
        for r in stmt.query_map(rusqlite::params![like, lim], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))? {
            let (cid, id, data, cname) = r?;
            let v: serde_json::Value = serde_json::from_str(&data).unwrap_or_default();
            out.push(GlobalHit { kind: "quest".into(), id, title: v["title"].as_str().unwrap_or_default().into(), detail: format!("{cname} · {}", v["objective"].as_str().unwrap_or_default()), parent: cid });
        }
    }
    let mut events = crate::archive::search_events(conn, &crate::archive::SearchFilter { query: q.into(), campaign_ids: vec![], kinds: vec![], limit: lim })?;
    if !spoilers {
        events.retain(|e| !crate::spoiler::gm_only_event(&e.kind));
    }
    for e in events {
        out.push(GlobalHit { kind: "event".into(), id: format!("{}:{}", e.campaign_id, e.seq), title: format!("{} · 第 {} 回合", e.campaign_name, e.turn + 1), detail: e.snippet, parent: e.campaign_id });
    }
    Ok(out)
}

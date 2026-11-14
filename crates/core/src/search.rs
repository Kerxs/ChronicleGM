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

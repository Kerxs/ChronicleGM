//! 线索调查板：线索（剧本线索、GM 记录的发现、玩家笔记）、线索之间的关联、玩家自己的推理与证据链。
//!
//! - 推理是玩家的：状态（未定 / 相信 / 否定）由玩家设定，程序**不判断对错**，允许错误推理。
//! - GM 能看到玩家的推理，但提示词要求它不确认、不否认，让证据说话（不替玩家揭晓答案）。
//! - 所有修改都通过 upsert_record / delete_record 效果写入并记入事件日志，随时间线回溯恢复。

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use chronicle_rules::Effect;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::campaign;
use crate::error::{CoreError, Result};
use crate::util::new_id;
use crate::world::World;

pub const LINK_RELATIONS: &[(&str, &str)] = &[("supports", "支持"), ("contradicts", "矛盾"), ("related", "相关")];
pub const HYPOTHESIS_STATUS: &[&str] = &["open", "believed", "rejected"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct BoardClue {
    pub id: String,
    /// script（剧本线索）/ gm（GM 记录的发现）/ player（玩家笔记）
    pub source: String,
    pub title: String,
    /// 玩家可见的内容。
    pub detail: String,
    /// 主持人信息（只在主持人视角给出）。
    pub gm_note: String,
    pub turn: i64,
    /// 在哪里发现的（剧本线索的 found_at / location_name）。
    #[serde(default)]
    pub found_at: String,
    /// 相关人物（名字）。
    #[serde(default)]
    pub related: Vec<String>,
    /// 还没有被任何推理用作证据（未解决的线索）。
    #[serde(default)]
    pub unresolved: bool,
}

/// 还没发现的剧本线索（只在剧透模式给出）：解锁条件与位置，方便主持人掌握进度。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct HiddenClue {
    pub id: String,
    pub title: String,
    pub found_at: String,
    pub related: Vec<String>,
    /// 解锁条件（文字或条件表达式的说明）。
    pub unlock: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ClueLink {
    pub id: String,
    pub a: String,
    pub b: String,
    /// supports / contradicts / related
    pub relation: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Hypothesis {
    pub id: String,
    pub text: String,
    /// 玩家引用的支持证据。
    pub evidence: Vec<String>,
    /// 玩家引用的反对证据。
    pub against: Vec<String>,
    /// open / believed / rejected（玩家自己的判断）
    pub status: String,
    pub turn: i64,
    /// 程序沿"支持"关联找到的间接支持线索（证据链），不含直接引用的。
    pub indirect: Vec<String>,
    /// 与证据有"矛盾"关联的线索（提醒玩家，不代表推理错误）。
    pub conflicts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Board {
    pub clues: Vec<BoardClue>,
    pub links: Vec<ClueLink>,
    pub hypotheses: Vec<Hypothesis>,
    /// 还没发现的剧本线索（剧透模式才有）。
    #[serde(default)]
    pub hidden: Vec<HiddenClue>,
}

/// 线索 payload 里的位置、相关人物（人物写编号时换成名字）、解锁条件。
fn clue_meta(conn: &Connection, p: &Value) -> (String, Vec<String>, String) {
    let found_at = ["found_at", "location_name", "location"].iter().map(|k| s(p, k)).find(|x| !x.is_empty()).unwrap_or_default();
    let found_at = crate::nodes::get_node(conn, &found_at).ok().flatten().map(|n| n.title).unwrap_or(found_at);
    let related: Vec<String> = ["related", "npcs", "npc_ids", "related_npcs"]
        .iter()
        .flat_map(|k| ids(p, k))
        .map(|x| crate::nodes::get_node(conn, &x).ok().flatten().map(|n| n.title).unwrap_or(x))
        .collect();
    let unlock = match p.get("unlock_when").or_else(|| p.get("unlock")) {
        Some(Value::String(t)) => t.clone(),
        Some(v) if v.is_object() => format!("条件：{v}"),
        _ => String::new(),
    };
    (found_at, related, unlock)
}

fn records(conn: &Connection, campaign_id: &str, kind: &str) -> Result<Vec<(String, Value)>> {
    let mut stmt = conn.prepare("SELECT id, data FROM campaign_records WHERE campaign_id = ?1 AND kind = ?2")?;
    let rows = stmt.query_map(params![campaign_id, kind], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = vec![];
    for r in rows {
        let (id, d) = r?;
        out.push((id, serde_json::from_str(&d).unwrap_or(Value::Null)));
    }
    Ok(out)
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn ids(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

pub fn board(conn: &Connection, campaign_id: &str, gm_view: bool) -> Result<Board> {
    let mut clues = vec![];
    // 1. 已揭示的剧本线索。
    {
        let mut stmt = conn.prepare(
            "SELECT r.clue_id, r.turn, i.title, i.player_text, i.gm_text, i.payload FROM clue_reveals r
             LEFT JOIN script_items i ON i.id = r.clue_id WHERE r.campaign_id = ?1 ORDER BY r.turn",
        )?;
        type Row = (String, i64, Option<String>, Option<String>, Option<String>, Option<String>);
        let rows: Vec<Row> = stmt
            .query_map([campaign_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, turn, title, player, gm, payload) in rows {
            let p: Value = payload.and_then(|x| serde_json::from_str(&x).ok()).unwrap_or(Value::Null);
            let (found_at, related, _) = clue_meta(conn, &p);
            clues.push(BoardClue {
                title: title.unwrap_or_else(|| "（未知线索）".into()),
                detail: player.unwrap_or_default(),
                gm_note: if gm_view { gm.unwrap_or_default() } else { String::new() },
                id,
                source: "script".into(),
                turn,
                found_at,
                related,
                unresolved: false,
            });
        }
    }
    // 2. GM 记录的发现与玩家笔记。
    let mut recs = records(conn, campaign_id, "clue")?;
    recs.sort_by_key(|(_, v)| v.get("turn").and_then(Value::as_i64).unwrap_or(0));
    for (id, v) in recs {
        clues.push(BoardClue { id, source: s(&v, "source"), title: s(&v, "title"), detail: s(&v, "detail"), gm_note: String::new(), turn: v.get("turn").and_then(Value::as_i64).unwrap_or(0), found_at: s(&v, "found_at"), related: ids(&v, "related"), unresolved: false });
    }
    let links: Vec<ClueLink> = records(conn, campaign_id, "clue_link")?
        .into_iter()
        .map(|(id, v)| ClueLink { id, a: s(&v, "a"), b: s(&v, "b"), relation: s(&v, "relation"), note: s(&v, "note") })
        .collect();
    let mut hypotheses: Vec<Hypothesis> = records(conn, campaign_id, "hypothesis")?
        .into_iter()
        .map(|(id, v)| Hypothesis {
            id,
            text: s(&v, "text"),
            evidence: ids(&v, "evidence"),
            against: ids(&v, "against"),
            status: s(&v, "status"),
            turn: v.get("turn").and_then(Value::as_i64).unwrap_or(0),
            indirect: vec![],
            conflicts: vec![],
        })
        .collect();
    hypotheses.sort_by_key(|h| h.turn);
    // 3. 证据链：从直接证据出发，沿"支持"关联（无向）找到间接支持；与链上线索"矛盾"的线索单独列出。
    let mut adj: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut contra: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for l in &links {
        let m = if l.relation == "supports" { &mut adj } else if l.relation == "contradicts" { &mut contra } else { continue };
        m.entry(&l.a).or_default().push(&l.b);
        m.entry(&l.b).or_default().push(&l.a);
    }
    for h in &mut hypotheses {
        let direct: BTreeSet<String> = h.evidence.iter().cloned().collect();
        let mut seen: BTreeSet<String> = direct.clone();
        let mut q: VecDeque<String> = direct.iter().cloned().collect();
        while let Some(c) = q.pop_front() {
            for n in adj.get(c.as_str()).into_iter().flatten() {
                if seen.insert(n.to_string()) {
                    q.push_back(n.to_string());
                }
            }
        }
        h.indirect = seen.difference(&direct).cloned().collect();
        let mut conflicts: BTreeSet<String> = BTreeSet::new();
        for c in &seen {
            for n in contra.get(c.as_str()).into_iter().flatten() {
                if !seen.contains(*n) {
                    conflicts.insert(n.to_string());
                }
            }
        }
        h.conflicts = conflicts.into_iter().collect();
    }
    // 4. 未解决的线索：还没被任何推理用作证据（玩家笔记不算线索）。
    let used: BTreeSet<&str> = hypotheses.iter().flat_map(|h| h.evidence.iter().chain(h.against.iter()).chain(h.indirect.iter())).map(String::as_str).collect();
    for c in clues.iter_mut() {
        c.unresolved = c.source != "player" && !used.contains(c.id.as_str());
    }
    // 5. 还没发现的剧本线索（只在剧透模式给出）。
    let mut hidden = vec![];
    if gm_view && let Some(sid) = campaign::get_campaign(conn, campaign_id)?.script_id {
        let known: BTreeSet<String> = clues.iter().map(|c| c.id.clone()).collect();
        for it in crate::scripts::items_by_kind(conn, &sid, crate::model::ScriptItemKind::Clue)? {
            if known.contains(&it.id) {
                continue;
            }
            let (found_at, related, unlock) = clue_meta(conn, &it.payload);
            hidden.push(HiddenClue { id: it.id, title: it.title, found_at, related, unlock });
        }
    }
    Ok(Board { clues, links, hypotheses, hidden })
}

fn turn_of(conn: &Connection, campaign_id: &str) -> Result<i64> {
    Ok(campaign::get_campaign(conn, campaign_id)?.turn)
}

fn apply(world: &mut World, campaign_id: &str, effects: Vec<Effect>, reason: &str) -> Result<()> {
    campaign::apply_effects(world, campaign_id, &effects, reason)?;
    Ok(())
}

fn require_clue(b: &Board, id: &str) -> Result<()> {
    if b.clues.iter().any(|c| c.id == id) { Ok(()) } else { Err(CoreError::Invalid(format!("调查板上没有线索 {id}"))) }
}

/// 玩家笔记（source = player）或 GM 记录的发现（source = gm）。返回新线索 ID。
pub fn add_clue(world: &mut World, campaign_id: &str, title: &str, detail: &str, source: &str) -> Result<String> {
    if title.trim().is_empty() {
        return Err(CoreError::Invalid("线索标题不能为空".into()));
    }
    if !matches!(source, "player" | "gm") {
        return Err(CoreError::Invalid("线索来源只能是 player 或 gm".into()));
    }
    // 同名的发现不重复记录（GM 可能多次提到同一个发现）。
    let b = board(world.conn(), campaign_id, true)?;
    if let Some(c) = b.clues.iter().find(|c| c.title.trim() == title.trim()) {
        return Ok(c.id.clone());
    }
    let id = new_id();
    let turn = turn_of(world.conn(), campaign_id)?;
    let data = json!({ "title": title.trim(), "detail": detail.trim(), "source": source, "turn": turn });
    let why = if source == "gm" { format!("记录发现：{}", title.trim()) } else { format!("调查板：玩家记下「{}」", title.trim()) };
    apply(world, campaign_id, vec![Effect::UpsertRecord { kind: "clue".into(), id: id.clone(), data }], &why)?;
    Ok(id)
}

/// 删除玩家自己的笔记（剧本线索和 GM 记录的发现不能删除）。同时删除与它有关的关联。
pub fn remove_note(world: &mut World, campaign_id: &str, id: &str) -> Result<()> {
    let b = board(world.conn(), campaign_id, true)?;
    match b.clues.iter().find(|c| c.id == id) {
        Some(c) if c.source == "player" => {}
        Some(_) => return Err(CoreError::Invalid("只能删除玩家自己的笔记".into())),
        None => return Err(CoreError::NotFound("线索", id.into())),
    }
    let mut eff = vec![Effect::DeleteRecord { kind: "clue".into(), id: id.into() }];
    for l in b.links.iter().filter(|l| l.a == id || l.b == id) {
        eff.push(Effect::DeleteRecord { kind: "clue_link".into(), id: l.id.clone() });
    }
    apply(world, campaign_id, eff, "调查板：删除笔记")
}

pub fn link(world: &mut World, campaign_id: &str, a: &str, b: &str, relation: &str, note: &str) -> Result<String> {
    if a == b {
        return Err(CoreError::Invalid("不能把线索和自己关联".into()));
    }
    if !LINK_RELATIONS.iter().any(|(k, _)| *k == relation) {
        return Err(CoreError::Invalid(format!("未知的关联类型：{relation}")));
    }
    let bd = board(world.conn(), campaign_id, true)?;
    require_clue(&bd, a)?;
    require_clue(&bd, b)?;
    // 同一对线索只保留一条关联（再次关联即修改）。
    let id = bd.links.iter().find(|l| (l.a == a && l.b == b) || (l.a == b && l.b == a)).map(|l| l.id.clone()).unwrap_or_else(new_id);
    apply(world, campaign_id, vec![Effect::UpsertRecord { kind: "clue_link".into(), id: id.clone(), data: json!({ "a": a, "b": b, "relation": relation, "note": note.trim() }) }], "调查板：关联线索")?;
    Ok(id)
}

pub fn unlink(world: &mut World, campaign_id: &str, link_id: &str) -> Result<()> {
    apply(world, campaign_id, vec![Effect::DeleteRecord { kind: "clue_link".into(), id: link_id.into() }], "调查板：取消关联")
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct HypothesisInput {
    pub text: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub against: Vec<String>,
    #[serde(default)]
    pub status: String,
}

/// 新建或修改一条推理（`id` 为空时新建）。状态是玩家自己的判断，程序不评判对错。
pub fn save_hypothesis(world: &mut World, campaign_id: &str, id: Option<&str>, h: &HypothesisInput) -> Result<String> {
    if h.text.trim().is_empty() {
        return Err(CoreError::Invalid("推理内容不能为空".into()));
    }
    let status = if h.status.is_empty() { "open" } else { h.status.as_str() };
    if !HYPOTHESIS_STATUS.contains(&status) {
        return Err(CoreError::Invalid(format!("未知的推理状态：{status}")));
    }
    let bd = board(world.conn(), campaign_id, true)?;
    for c in h.evidence.iter().chain(&h.against) {
        require_clue(&bd, c)?;
    }
    let (id, turn) = match id {
        Some(i) => {
            let old = bd.hypotheses.iter().find(|x| x.id == i).ok_or_else(|| CoreError::NotFound("推理", i.into()))?;
            (i.to_string(), old.turn)
        }
        None => (new_id(), turn_of(world.conn(), campaign_id)?),
    };
    let data = json!({ "text": h.text.trim(), "evidence": h.evidence, "against": h.against, "status": status, "turn": turn });
    apply(world, campaign_id, vec![Effect::UpsertRecord { kind: "hypothesis".into(), id: id.clone(), data }], "调查板：玩家的推理")?;
    Ok(id)
}

pub fn delete_hypothesis(world: &mut World, campaign_id: &str, id: &str) -> Result<()> {
    apply(world, campaign_id, vec![Effect::DeleteRecord { kind: "hypothesis".into(), id: id.into() }], "调查板：删除推理")
}

/// 给 GM 的调查板摘要：已知线索与玩家的推理（明确标注"玩家的推理，可能是错的"）。
pub fn gm_summary(conn: &Connection, campaign_id: &str) -> Result<String> {
    let b = board(conn, campaign_id, false)?;
    if b.clues.is_empty() && b.hypotheses.is_empty() {
        return Ok(String::new());
    }
    let title = |id: &str| b.clues.iter().find(|c| c.id == id).map(|c| c.title.clone()).unwrap_or_else(|| id.into());
    let mut out = vec![];
    if !b.clues.is_empty() {
        out.push(format!("玩家已掌握的线索：{}", b.clues.iter().map(|c| c.title.as_str()).collect::<Vec<_>>().join("、")));
    }
    for h in &b.hypotheses {
        let st = match h.status.as_str() {
            "believed" => "玩家相信",
            "rejected" => "玩家已否定",
            _ => "玩家还在考虑",
        };
        out.push(format!(
            "玩家的推理（{st}；可能是错的）：{}；依据：{}",
            h.text,
            if h.evidence.is_empty() { "无".into() } else { h.evidence.iter().map(|c| title(c)).collect::<Vec<_>>().join("、") }
        ));
    }
    Ok(out.join("\n"))
}

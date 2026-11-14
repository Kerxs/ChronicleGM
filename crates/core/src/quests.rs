//! 动态任务：剧本中的任务条目（主线 / 支线 / 隐藏）与游戏中由 GM 新建的任务。
//!
//! 任务可以带：前置条件、完成条件、失败条件（规则 DSL，由程序求值）、期限（回合数，从任务开始算起）、
//! 关联的 NPC、地点与线索。每个回合结束时由程序推进（tick）：
//! - 未开始且前置条件满足 → 进行中；进行中且完成条件满足 → 完成；失败条件满足或超过期限 → 失败。
//!
//! 状态变化都通过 set_quest_state 效果写入并记录原因，随时间线回溯恢复。

use chronicle_rules::{Condition, Effect, Tri};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::campaign::{self, CampaignView};
use crate::error::{CoreError, Result};
use crate::model::ScriptItemKind;
use crate::util::new_id;
use crate::world::World;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Ref {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct QuestNote {
    pub turn: i64,
    pub text: String,
    /// 只在主持人视角显示（可能涉及真相或对玩家推理的评价）。
    #[serde(default)]
    pub gm_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct QuestView {
    pub id: String,
    pub title: String,
    /// main / side / hidden
    pub quest_type: String,
    /// script（剧本任务）/ gm（游戏中新建）
    pub source: String,
    /// "" 未开始 / active / done / failed（也可能是剧情设定的其他状态）
    pub status: String,
    pub description: String,
    pub objectives: Vec<String>,
    pub deadline_turns: Option<i64>,
    /// 开始于第几回合（进行中或已结束的任务）。
    pub started_turn: Option<i64>,
    /// 剩余回合（有期限且进行中）。
    pub turns_left: Option<i64>,
    /// 故事时间期限（分钟）与剩余时间（分钟）。
    #[serde(default)]
    pub deadline_minutes: Option<i64>,
    #[serde(default)]
    pub minutes_left: Option<i64>,
    pub npcs: Vec<Ref>,
    pub location: Option<Ref>,
    pub clues: Vec<Ref>,
    /// 关联线索中玩家已经知道的。
    pub known_clues: Vec<String>,
    #[ts(type = "unknown")]
    pub precondition: Option<Value>,
    #[ts(type = "unknown")]
    pub complete_when: Option<Value>,
    #[ts(type = "unknown")]
    pub fail_when: Option<Value>,
    /// 任务变化记录（目标调整、GM 备注等）。
    pub notes: Vec<QuestNote>,
}

struct Def {
    id: String,
    title: String,
    quest_type: String,
    source: String,
    description: String,
    objectives: Vec<String>,
    deadline: Option<i64>,
    /// 故事时间期限（分钟，开始后计算）。
    deadline_minutes: Option<i64>,
    npcs: Vec<String>,
    location: Option<String>,
    clues: Vec<String>,
    pre: Option<Value>,
    done: Option<Value>,
    fail: Option<Value>,
}

fn strs(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn cond(v: &Value, k: &str) -> Option<Value> {
    v.get(k).filter(|x| x.is_object()).cloned()
}

fn defs(conn: &Connection, campaign_id: &str) -> Result<Vec<Def>> {
    let c = campaign::get_campaign(conn, campaign_id)?;
    let mut out = vec![];
    if let Some(sid) = &c.script_id {
        for it in crate::scripts::all_items(conn, sid)?.into_iter().filter(|i| i.kind == ScriptItemKind::Quest) {
            let p = &it.payload;
            out.push(Def {
                id: it.id.clone(),
                title: it.title.clone(),
                quest_type: p.get("quest_type").and_then(Value::as_str).unwrap_or("side").into(),
                source: "script".into(),
                description: it.player_text.clone(),
                objectives: strs(p, "objectives"),
                deadline: p.get("deadline_turns").and_then(Value::as_i64).filter(|d| *d > 0),
                deadline_minutes: deadline_minutes(p),
                npcs: strs(p, "npc_ids"),
                location: p.get("location").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string),
                clues: strs(p, "clue_ids"),
                pre: cond(p, "precondition"),
                done: cond(p, "complete_when"),
                fail: cond(p, "fail_when"),
            });
        }
    }
    let mut stmt = conn.prepare("SELECT id, data FROM campaign_records WHERE campaign_id = ?1 AND kind = 'quest'")?;
    for r in stmt.query_map([campaign_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (id, d) = r?;
        let p: Value = serde_json::from_str(&d).unwrap_or(Value::Null);
        out.push(Def {
            id,
            title: p["title"].as_str().unwrap_or_default().into(),
            quest_type: p["quest_type"].as_str().unwrap_or("side").into(),
            source: "gm".into(),
            description: p["objective"].as_str().unwrap_or_default().into(),
            objectives: strs(&p, "objectives"),
            deadline: p.get("deadline_turns").and_then(Value::as_i64).filter(|d| *d > 0),
            deadline_minutes: deadline_minutes(&p),
            npcs: strs(&p, "npc_ids"),
            location: p.get("location").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string),
            clues: strs(&p, "clue_ids"),
            pre: None,
            done: cond(&p, "complete_when"),
            fail: cond(&p, "fail_when"),
        });
    }
    Ok(out)
}

/// 故事时间期限：`deadline_hours`（小时，可以是小数）或 `deadline_days`（天）。
fn deadline_minutes(p: &Value) -> Option<i64> {
    let h = p.get("deadline_hours").and_then(Value::as_f64).map(|h| h * 60.0);
    let d = p.get("deadline_days").and_then(Value::as_f64).map(|d| d * 1440.0);
    h.or(d).map(|m| m.round() as i64).filter(|m| *m > 0)
}

/// 任务开始时的故事时间（分钟）：开始事件之前最后一次时钟变化的值。
fn started_minutes(conn: &Connection, campaign_id: &str, quest: &str) -> Result<Option<i64>> {
    let mut stmt = conn.prepare(
        "SELECT seq, kind, payload FROM event_log WHERE campaign_id = ?1 AND kind IN ('effects', 'transition') AND instr(payload, ?2) > 0 ORDER BY seq DESC",
    )?;
    let rows: Vec<(i64, String, String)> = stmt.query_map(params![campaign_id, quest], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut start_seq = None;
    for (seq, kind, payload) in rows {
        let v: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
        let changes = if kind == "effects" { &v["changes"] } else { &v["outcome"]["changes"] };
        if let Ok(list) = serde_json::from_value::<Vec<chronicle_rules::StateChange>>(changes.clone())
            && list.iter().any(|c| matches!(c, chronicle_rules::StateChange::SetQuestState { quest: q, new, .. } if q == quest && new == "active"))
        {
            start_seq = Some(seq);
            break;
        }
    }
    let Some(seq) = start_seq else { return Ok(None) };
    let mut stmt = conn.prepare("SELECT payload FROM event_log WHERE campaign_id = ?1 AND kind = 'effects' AND seq <= ?2 AND instr(payload, 'time.minutes') > 0 ORDER BY seq DESC LIMIT 1")?;
    let clock: Option<String> = rusqlite::OptionalExtension::optional(stmt.query_row(params![campaign_id, seq], |r| r.get::<_, String>(0)))?;
    let at = clock
        .and_then(|p| serde_json::from_str::<Value>(&p).ok())
        .and_then(|v| v["changes"].as_array().and_then(|a| a.iter().rev().find(|c| c["key"] == "time.minutes").and_then(|c| c["new"].as_i64())))
        .unwrap_or(crate::schedule::DEFAULT_START);
    Ok(Some(at))
}

/// 任务最近一次进入"进行中"的回合（从事件日志读取）。
fn started_turn(conn: &Connection, campaign_id: &str, quest: &str) -> Result<Option<i64>> {
    let mut stmt = conn.prepare(
        "SELECT turn, kind, payload FROM event_log WHERE campaign_id = ?1 AND kind IN ('effects', 'transition') AND instr(payload, ?2) > 0 ORDER BY seq DESC",
    )?;
    let rows: Vec<(i64, String, String)> = stmt.query_map(params![campaign_id, quest], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
    for (turn, kind, payload) in rows {
        let v: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
        let changes = if kind == "effects" { &v["changes"] } else { &v["outcome"]["changes"] };
        if let Ok(list) = serde_json::from_value::<Vec<chronicle_rules::StateChange>>(changes.clone())
            && list.iter().any(|c| matches!(c, chronicle_rules::StateChange::SetQuestState { quest: q, new, .. } if q == quest && new == "active"))
        {
            return Ok(Some(turn));
        }
    }
    Ok(None)
}

fn title_of(conn: &Connection, id: &str) -> String {
    conn.query_row(
        "SELECT title FROM nodes WHERE id = ?1 UNION ALL SELECT title FROM script_items WHERE id = ?1 UNION ALL SELECT json_extract(data, '$.title') FROM campaign_records WHERE id = ?1 LIMIT 1",
        [id],
        |r| r.get::<_, String>(0),
    )
    .unwrap_or_else(|_| id.to_string())
}

pub fn quests(conn: &Connection, campaign_id: &str, gm_view: bool) -> Result<Vec<QuestView>> {
    let c = campaign::get_campaign(conn, campaign_id)?;
    let st = campaign::campaign_state(conn, campaign_id)?;
    let mut notes_by: std::collections::BTreeMap<String, Vec<QuestNote>> = Default::default();
    {
        let mut stmt = conn.prepare("SELECT data FROM campaign_records WHERE campaign_id = ?1 AND kind = 'quest_note'")?;
        for r in stmt.query_map([campaign_id], |r| r.get::<_, String>(0))? {
            let v: Value = serde_json::from_str(&r?).unwrap_or(Value::Null);
            if let Some(q) = v["quest"].as_str() {
                let gm_only = v["gm_only"].as_bool().unwrap_or(false);
                if gm_only && !gm_view {
                    continue;
                }
                notes_by.entry(q.into()).or_default().push(QuestNote { turn: v["turn"].as_i64().unwrap_or(0), text: v["text"].as_str().unwrap_or_default().into(), gm_only });
            }
        }
    }
    let mut out = vec![];
    for d in defs(conn, campaign_id)? {
        let status = st.quests.get(&d.id).cloned().unwrap_or_default();
        // 玩家视角：隐藏任务在开始之前看不到。
        if !gm_view && d.quest_type == "hidden" && status.is_empty() {
            continue;
        }
        let started = if status.is_empty() { None } else { started_turn(conn, campaign_id, &d.id)? };
        let turns_left = match (d.deadline, started, status.as_str()) {
            (Some(dl), Some(s), "active") => Some((s + dl - c.turn).max(0)),
            _ => None,
        };
        let minutes_left = match (d.deadline_minutes, status.as_str()) {
            (Some(dm), "active") => started_minutes(conn, campaign_id, &d.id)?.map(|s| (dm - (crate::schedule::clock(conn, campaign_id) - s)).max(0)),
            _ => None,
        };
        let mut notes = notes_by.remove(&d.id).unwrap_or_default();
        notes.sort_by_key(|n| n.turn);
        out.push(QuestView {
            title: d.title,
            quest_type: d.quest_type,
            source: d.source,
            status,
            description: d.description,
            objectives: d.objectives,
            deadline_turns: d.deadline,
            started_turn: started,
            turns_left,
            deadline_minutes: d.deadline_minutes,
            minutes_left,
            npcs: d.npcs.iter().map(|i| Ref { id: i.clone(), title: title_of(conn, i) }).collect(),
            location: d.location.as_ref().map(|i| Ref { id: i.clone(), title: title_of(conn, i) }),
            known_clues: d.clues.iter().filter(|i| st.clues.contains(i)).cloned().collect(),
            clues: d.clues.iter().map(|i| Ref { id: i.clone(), title: title_of(conn, i) }).collect(),
            precondition: if gm_view { d.pre } else { None },
            complete_when: if gm_view { d.done } else { None },
            fail_when: if gm_view { d.fail } else { None },
            notes,
            id: d.id,
        });
    }
    // 进行中的在前，其次未开始，最后已结束。
    out.sort_by_key(|q| match q.status.as_str() {
        "active" => 0,
        "" => 1,
        _ => 2,
    });
    Ok(out)
}

fn holds(view: &CampaignView, c: &Option<Value>) -> bool {
    let Some(v) = c else { return false };
    match serde_json::from_value::<Condition>(v.clone()) {
        // 需要检定的条件不在这里掷骰（只在条件确定为真时推进）。
        Ok(c) => matches!(c.preview(view), Ok(Tri::True)),
        Err(_) => false,
    }
}

/// 回合结束时推进任务。返回发生的变化（用于叙述与界面提示）。
pub fn tick(world: &mut World, campaign_id: &str) -> Result<Vec<String>> {
    let c = campaign::get_campaign(world.conn(), campaign_id)?;
    if c.status != "active" {
        return Ok(vec![]);
    }
    let st = campaign::campaign_state(world.conn(), campaign_id)?;
    let mut plan: Vec<(String, String, String)> = vec![];
    {
        let view = CampaignView { conn: world.conn(), campaign_id };
        for d in defs(world.conn(), campaign_id)? {
            let status = st.quests.get(&d.id).cloned().unwrap_or_default();
            match status.as_str() {
                "" if holds(&view, &d.pre) => plan.push((d.id, "active".into(), format!("任务「{}」开始：前置条件已满足", d.title))),
                "active" => {
                    if holds(&view, &d.done) {
                        plan.push((d.id, "done".into(), format!("任务「{}」完成", d.title)));
                    } else if holds(&view, &d.fail) {
                        plan.push((d.id, "failed".into(), format!("任务「{}」失败：失败条件已满足", d.title)));
                    } else if let (Some(dl), Some(s)) = (d.deadline, started_turn(world.conn(), campaign_id, &d.id)?)
                        && c.turn - s >= dl
                    {
                        plan.push((d.id, "failed".into(), format!("任务「{}」失败：超过 {dl} 回合的期限", d.title)));
                    } else if let (Some(dm), Some(s)) = (d.deadline_minutes, started_minutes(world.conn(), campaign_id, &d.id)?)
                        && crate::schedule::clock(world.conn(), campaign_id) - s >= dm
                    {
                        plan.push((d.id, "failed".into(), format!("任务「{}」失败：故事时间超过了 {} 的期限", d.title, crate::worldsim::format_duration(dm).trim_start_matches("约 "))));
                    }
                }
                _ => {}
            }
        }
    }
    let mut notes = vec![];
    for (id, state, why) in plan {
        campaign::apply_effects(world, campaign_id, &[Effect::SetQuestState { quest: id, state }], &why)?;
        notes.push(why);
    }
    Ok(notes)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NewQuest {
    pub title: String,
    #[serde(default)]
    pub objective: String,
    /// main / side / hidden（默认 side）
    #[serde(default)]
    pub quest_type: String,
    #[serde(default)]
    pub deadline_turns: Option<i64>,
    #[serde(default)]
    pub npc_ids: Vec<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub clue_ids: Vec<String>,
}

/// 游戏中新建任务（只属于本战役）并立即开始。同名任务不重复创建。
pub fn add_quest(world: &mut World, campaign_id: &str, q: &NewQuest, reason: &str) -> Result<String> {
    if q.title.trim().is_empty() {
        return Err(CoreError::Invalid("任务名称不能为空".into()));
    }
    let qt = if q.quest_type.is_empty() { "side" } else { q.quest_type.as_str() };
    if !matches!(qt, "main" | "side" | "hidden") {
        return Err(CoreError::Invalid(format!("未知的任务类型：{qt}")));
    }
    // 同名任务已存在（例如剧本里写好的任务在剧情中出现）：不重复创建；还没开始的就让它开始。
    if let Some(d) = defs(world.conn(), campaign_id)?.into_iter().find(|d| d.title.trim() == q.title.trim()) {
        let status = campaign::campaign_state(world.conn(), campaign_id)?.quests.get(&d.id).cloned().unwrap_or_default();
        if status.is_empty() {
            campaign::apply_effects(world, campaign_id, &[Effect::SetQuestState { quest: d.id.clone(), state: "active".into() }], reason)?;
        }
        return Ok(d.id);
    }
    let id = new_id();
    let turn = campaign::get_campaign(world.conn(), campaign_id)?.turn;
    let data = json!({
        "title": q.title.trim(), "objective": q.objective.trim(), "quest_type": qt,
        "deadline_turns": q.deadline_turns.filter(|d| *d > 0), "npc_ids": q.npc_ids, "location": q.location, "clue_ids": q.clue_ids,
        "turn": turn, "source": "gm"
    });
    campaign::apply_effects(
        world,
        campaign_id,
        &[Effect::UpsertRecord { kind: "quest".into(), id: id.clone(), data }, Effect::SetQuestState { quest: id.clone(), state: "active".into() }],
        reason,
    )?;
    Ok(id)
}

/// GM 写的、玩家可见的文字里出现这些词时，很可能在评价玩家推理或暗示真相，改为只在主持人视角显示。
/// （启发式，不能保证；同时提示词也要求 GM 不这样写。）
pub const SPOILER_WORDS: &[&str] = &["真相", "推理", "实际上", "其实是", "真正的", "猜错", "猜对"];

pub fn looks_like_spoiler(text: &str) -> bool {
    SPOILER_WORDS.iter().any(|w| text.contains(w))
}

/// 任务变化：记录一条备注（目标调整、新情况等），显示在任务历史里。
pub fn add_quest_note(world: &mut World, campaign_id: &str, quest: &str, text: &str) -> Result<()> {
    add_quest_note_ex(world, campaign_id, quest, text, false)
}

/// `gm_only` 为真时只在主持人视角显示。
pub fn add_quest_note_ex(world: &mut World, campaign_id: &str, quest: &str, text: &str, gm_only: bool) -> Result<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    if !defs(world.conn(), campaign_id)?.iter().any(|d| d.id == quest) {
        return Err(CoreError::NotFound("任务", quest.into()));
    }
    let turn = campaign::get_campaign(world.conn(), campaign_id)?.turn;
    campaign::apply_effects(
        world,
        campaign_id,
        &[Effect::UpsertRecord { kind: "quest_note".into(), id: new_id(), data: json!({ "quest": quest, "text": text.trim(), "turn": turn, "gm_only": gm_only }) }],
        "任务变化",
    )?;
    Ok(())
}

/// 任务是否存在（剧本任务或本战役新建的任务）。供 GM 校验 set_quest_state 使用。
pub fn quest_exists(conn: &Connection, campaign_id: &str, id: &str) -> Result<bool> {
    Ok(defs(conn, campaign_id)?.iter().any(|d| d.id == id))
}

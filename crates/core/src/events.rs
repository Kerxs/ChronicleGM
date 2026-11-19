//! 动态剧情事件（反剧情纠正改革）：剧本里的任务、关键事件、触发器、转折、结局不再是"必须完成的清单"，
//! 而是有状态的可能性。
//!
//! - 每个事件存成战役记录 `plot_event`（随检查点、分叉、导出一起保存；旧存档第一次用到时从剧本补生成）。
//! - 状态：未发生 / 可触发 / 进行中 / 已完成 / 已失败 / 已绕过 / 已失效 / 被玩家改变。
//! - **终态（完成、失败、绕过、失效、被改变）永远不会回退**：玩家没按预设路线走，事件不会自己"恢复成可触发"。
//! - 事件只有在实际条件满足时才会从"未发生"变成"可触发"；必需的人物死了、必需的物品被毁了，事件自动失效。
//! - 另外两类事实记录：被摧毁的物品 `item_state`（不能凭空再出现一件）、玩家当前目标 `player_goal`。

use std::collections::BTreeMap;

use chronicle_rules::{Condition, Effect, StateView, Tri};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::campaign::{self, CampaignView};
use crate::error::{CoreError, Result};
use crate::model::ScriptItemKind;
use crate::world::World;

pub const EVENT: &str = "plot_event";
pub const ITEM_STATE: &str = "item_state";
pub const PLAYER_GOAL: &str = "player_goal";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum EventStatus {
    /// 未发生
    Pending,
    /// 可触发（条件已经满足，但还没发生）
    Available,
    /// 进行中
    Active,
    /// 已完成
    Done,
    /// 已失败
    Failed,
    /// 已绕过（玩家拒绝、放弃或走了别的路）
    Bypassed,
    /// 已失效（成立条件没有了：关键人物死亡、关键物品被毁……）
    Invalid,
    /// 被玩家改变（玩家用剧本没有预设的方式改写了它）
    Changed,
}

impl EventStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "未发生",
            Self::Available => "可触发",
            Self::Active => "进行中",
            Self::Done => "已完成",
            Self::Failed => "已失败",
            Self::Bypassed => "已绕过",
            Self::Invalid => "已失效",
            Self::Changed => "被玩家改变",
        }
    }
    /// 终态：不会再变。
    pub fn terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Bypassed | Self::Invalid | Self::Changed)
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim() {
            "pending" | "未发生" => Self::Pending,
            "available" | "可触发" => Self::Available,
            "active" | "进行中" => Self::Active,
            "done" | "completed" | "已完成" => Self::Done,
            "failed" | "已失败" => Self::Failed,
            "bypassed" | "refused" | "skipped" | "已绕过" | "已拒绝" => Self::Bypassed,
            "invalid" | "已失效" => Self::Invalid,
            "changed" | "被玩家改变" => Self::Changed,
            _ => return None,
        })
    }
    /// 任务状态（quest_states 里的字符串）对应的事件状态。
    fn from_quest(s: &str) -> Option<Self> {
        Some(match s {
            "" => return None,
            "active" => Self::Active,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "refused" | "bypassed" => Self::Bypassed,
            "invalid" => Self::Invalid,
            "changed" => Self::Changed,
            _ => return None,
        })
    }
    /// 写回任务状态用的字符串。
    fn to_quest(self, refused: bool) -> Option<&'static str> {
        Some(match self {
            Self::Active => "active",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Bypassed if refused => "refused",
            Self::Bypassed => "bypassed",
            Self::Invalid => "invalid",
            Self::Changed => "changed",
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct PlotEvent {
    pub event_id: String,
    /// quest / key_event / trigger / twist / ending
    pub kind: String,
    pub title: String,
    /// 给主持人看的描述（可能含秘密）。
    pub description: String,
    /// 触发条件（规则 DSL）。
    #[ts(type = "unknown")]
    pub prerequisites: Option<Value>,
    pub status: EventStatus,
    /// 事件发生会带来的后果（文字）。
    pub consequences: String,
    pub related_npcs: Vec<String>,
    pub related_locations: Vec<String>,
    pub related_factions: Vec<String>,
    /// 这个事件成立需要的人物（死亡 / 失踪即失效）。
    #[serde(default)]
    pub required_npcs: Vec<String>,
    /// 这个事件成立需要的物品（被摧毁即失效）。
    #[serde(default)]
    pub required_items: Vec<String>,
    /// 创建于第几回合。
    pub created_at: i64,
    #[serde(default)]
    pub updated_turn: i64,
    /// 最近一次状态变化的原因。
    #[serde(default)]
    pub reason: String,
    /// 状态变化记录（回合, 状态, 原因）。
    #[serde(default)]
    pub history: Vec<(i64, EventStatus, String)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ItemState {
    pub item: String,
    pub destroyed: bool,
    pub turn: i64,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct PlayerGoal {
    pub text: String,
    pub turn: i64,
    /// 以前的目标（回合, 目标）。
    #[serde(default)]
    pub history: Vec<(i64, String)>,
}

fn strs(v: &Value, k: &str) -> Vec<String> {
    match v.get(k) {
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).filter(|s| !s.is_empty()).collect(),
        Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
        _ => vec![],
    }
}

/// 条件里要求持有的物品、要求活着的人物。
fn needs(c: &Condition, items: &mut Vec<String>, npcs: &mut Vec<String>) {
    match c {
        Condition::All { of } => of.iter().for_each(|c| needs(c, items, npcs)),
        Condition::HasItem { item, .. } => items.push(item.clone()),
        Condition::Alive { entity } => npcs.push(entity.clone()),
        _ => {}
    }
}

fn list<T: serde::de::DeserializeOwned>(conn: &Connection, campaign_id: &str, kind: &str) -> Result<Vec<T>> {
    let mut stmt = conn.prepare("SELECT data FROM campaign_records WHERE campaign_id = ?1 AND kind = ?2 ORDER BY id")?;
    let rows: Vec<String> = stmt.query_map([campaign_id, kind], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(rows.into_iter().filter_map(|d| serde_json::from_str(&d).ok()).collect())
}

pub fn events(conn: &Connection, campaign_id: &str) -> Result<Vec<PlotEvent>> {
    list(conn, campaign_id, EVENT)
}

pub fn get(conn: &Connection, campaign_id: &str, id: &str) -> Option<PlotEvent> {
    CampaignView { conn, campaign_id }.record(EVENT, id).and_then(|v| serde_json::from_value(v).ok())
}

/// 剧本里能成为事件的条目。
fn seed_from_script(conn: &Connection, campaign_id: &str, turn: i64) -> Result<Vec<PlotEvent>> {
    let c = campaign::get_campaign(conn, campaign_id)?;
    let Some(sid) = c.script_id else { return Ok(vec![]) };
    let st = campaign::campaign_state(conn, campaign_id)?;
    let view = CampaignView { conn, campaign_id };
    let mut out = vec![];
    for it in crate::scripts::all_items(conn, &sid)? {
        let kind = match it.kind {
            ScriptItemKind::Quest => "quest",
            ScriptItemKind::KeyEvent => "key_event",
            ScriptItemKind::Trigger => "trigger",
            ScriptItemKind::Twist => "twist",
            ScriptItemKind::Ending => "ending",
            _ => continue,
        };
        let p = &it.payload;
        let pre = ["precondition", "when", "condition"].iter().find_map(|k| p.get(*k).filter(|v| v.is_object()).cloned());
        let mut items = strs(p, "required_items");
        let mut npcs_req = strs(p, "required_npcs");
        if let Some(c) = pre.as_ref().and_then(|v| serde_json::from_value::<Condition>(v.clone()).ok()) {
            needs(&c, &mut items, &mut npcs_req);
        }
        let related_npcs = {
            let mut v = strs(p, "npc_ids");
            v.extend(strs(p, "npcs"));
            v.dedup();
            v
        };
        // 任务的发布者 / 关键人物：任务没有单独写 required_npcs 时，第一个关联人物算发布者。
        if kind == "quest" && npcs_req.is_empty()
            && let Some(first) = related_npcs.first()
        {
            npcs_req.push(first.clone());
        }
        items.sort();
        items.dedup();
        npcs_req.sort();
        npcs_req.dedup();
        let mut status = EventStatus::Pending;
        if kind == "quest" {
            if let Some(s) = st.quests.get(&it.id).and_then(|s| EventStatus::from_quest(s)) {
                status = s;
            }
        } else if view.record("trigger_fired", &it.id).is_some() {
            status = EventStatus::Done;
        }
        out.push(PlotEvent {
            event_id: it.id.clone(),
            kind: kind.into(),
            title: it.title.clone(),
            description: if it.gm_text.trim().is_empty() { it.player_text.clone() } else { format!("{}\n{}", it.player_text, it.gm_text).trim().to_string() },
            prerequisites: pre,
            status,
            consequences: p.get("consequences").and_then(Value::as_str).unwrap_or_default().to_string(),
            related_npcs,
            related_locations: { let mut v = strs(p, "location"); v.extend(strs(p, "locations")); v },
            related_factions: strs(p, "factions"),
            required_npcs: npcs_req,
            required_items: items,
            created_at: turn,
            updated_turn: turn,
            reason: String::new(),
            history: vec![],
        });
    }
    Ok(out)
}

fn save(world: &mut World, campaign_id: &str, e: &PlotEvent, reason: &str) -> Result<()> {
    campaign::apply_effects(world, campaign_id, &[Effect::UpsertRecord { kind: EVENT.into(), id: e.event_id.clone(), data: serde_json::to_value(e)? }], reason)?;
    Ok(())
}

/// 补齐剧本事件（新战役开局时、旧存档第一次用到时）。已有的事件不动。
pub fn ensure(world: &mut World, campaign_id: &str) -> Result<usize> {
    let turn = campaign::get_campaign(world.conn(), campaign_id)?.turn;
    let have: Vec<String> = events(world.conn(), campaign_id)?.into_iter().map(|e| e.event_id).collect();
    let new: Vec<PlotEvent> = seed_from_script(world.conn(), campaign_id, turn)?.into_iter().filter(|e| !have.contains(&e.event_id)).collect();
    if new.is_empty() {
        return Ok(0);
    }
    let effects: Vec<Effect> = new.iter().map(|e| Effect::UpsertRecord { kind: EVENT.into(), id: e.event_id.clone(), data: serde_json::to_value(e).unwrap_or(Value::Null) }).collect();
    campaign::apply_effects(world, campaign_id, &effects, "整理剧本中的潜在事件")?;
    Ok(new.len())
}

/// 人物现在是否不在了（死亡、失踪），返回说明。
fn gone(view: &CampaignView, conn: &Connection, npc: &str) -> Option<String> {
    let s = view.life_status(npc)?;
    let name = crate::nodes::get_node(conn, npc).ok().flatten().map(|n| n.title).unwrap_or_else(|| npc.to_string());
    match s.as_str() {
        "dead" => Some(format!("{name}已经死亡")),
        "missing" => Some(format!("{name}已经失踪")),
        _ => None,
    }
}

/// 改变一个事件的状态（程序或 GM 提出）。终态不能改；返回是否改了。
fn transition(e: &mut PlotEvent, to: EventStatus, turn: i64, reason: &str) -> bool {
    if e.status == to || e.status.terminal() {
        return false;
    }
    e.status = to;
    e.updated_turn = turn;
    e.reason = reason.chars().take(160).collect();
    e.history.push((turn, to, e.reason.clone()));
    true
}

/// 回合结束时推进事件状态：同步任务 / 触发器的结果；成立条件没了就失效；条件满足就可触发。
/// 返回发生的变化（给叙述与界面提示）。
pub fn tick(world: &mut World, campaign_id: &str) -> Result<Vec<String>> {
    let c = campaign::get_campaign(world.conn(), campaign_id)?;
    if c.status != "active" || c.script_id.is_none() {
        return Ok(vec![]);
    }
    ensure(world, campaign_id)?;
    let turn = c.turn;
    let st = campaign::campaign_state(world.conn(), campaign_id)?;
    let destroyed: Vec<String> = destroyed_items(world.conn(), campaign_id)?;
    let mut changed: Vec<PlotEvent> = vec![];
    let mut quest_writes: Vec<(String, String, String)> = vec![];
    let mut notes = vec![];
    {
        let conn = world.conn();
        let view = CampaignView { conn, campaign_id };
        for mut e in events(conn, campaign_id)? {
            if e.status.terminal() {
                continue;
            }
            let before = e.status;
            // 1. 任务 / 触发器自己的结果。
            if e.kind == "quest" {
                if let Some(s) = st.quests.get(&e.event_id).and_then(|s| EventStatus::from_quest(s)) {
                    transition(&mut e, s, turn, "任务状态变化");
                }
            } else if view.record("trigger_fired", &e.event_id).is_some() {
                transition(&mut e, EventStatus::Done, turn, "条件满足，事件已经发生");
            }
            // 2. 成立条件没了：必需的人物不在了、必需的物品被毁了。
            if !e.status.terminal() {
                let why = e
                    .required_npcs
                    .iter()
                    .find_map(|n| gone(&view, conn, n))
                    .or_else(|| e.required_items.iter().find(|i| destroyed.contains(i)).map(|i| format!("{}已经被摧毁", crate::nodes::get_node(conn, i).ok().flatten().map(|n| n.title).or_else(|| crate::scripts::get_item(conn, i).ok().flatten().map(|it| it.title)).unwrap_or_else(|| i.clone()))));
                if let Some(why) = why {
                    // 进行中的任务因为发布者死亡失效；关键人物本人死亡的事件（例如"某人遇害"）由 GM 判断是否算完成，程序只在它还没发生时让它失效。
                    transition(&mut e, EventStatus::Invalid, turn, &format!("失去成立条件：{why}"));
                    if e.kind == "quest" {
                        quest_writes.push((e.event_id.clone(), "invalid".into(), format!("任务「{}」失效：{why}", e.title)));
                    }
                }
            }
            // 3. 条件满足：未发生 → 可触发（只往前走）。
            if e.status == EventStatus::Pending
                && let Some(c) = e.prerequisites.as_ref().and_then(|v| serde_json::from_value::<Condition>(v.clone()).ok())
                && matches!(c.preview(&view), Ok(Tri::True))
            {
                transition(&mut e, EventStatus::Available, turn, "条件已经满足");
            }
            if e.status != before {
                if e.status.terminal() && e.status != EventStatus::Done {
                    notes.push(format!("「{}」{}：{}", e.title, e.status.label(), e.reason));
                }
                changed.push(e);
            }
        }
    }
    for e in &changed {
        save(world, campaign_id, e, &format!("剧情事件「{}」→ {}", e.title, e.status.label()))?;
    }
    for (q, s, why) in quest_writes {
        campaign::apply_effects(world, campaign_id, &[Effect::SetQuestState { quest: q, state: s }], &why)?;
    }
    Ok(notes)
}

/// GM（或主持人）因为玩家的行动改变事件状态：拒绝委托、绕过、用别的方式改写、失去成立条件。
/// 终态不能改回去；任务会同步写任务状态。`refused` 表示玩家明确拒绝（任务记为 refused）。
pub fn set_status(world: &mut World, campaign_id: &str, id: &str, to: EventStatus, reason: &str, refused: bool) -> Result<bool> {
    ensure(world, campaign_id)?;
    let turn = campaign::get_campaign(world.conn(), campaign_id)?.turn;
    let mut e = get(world.conn(), campaign_id, id).ok_or_else(|| CoreError::NotFound("剧情事件", id.into()))?;
    if e.status == to {
        return Ok(false);
    }
    if e.status.terminal() {
        return Err(CoreError::Invalid(format!("「{}」已经{}，不能再改成{}", e.title, e.status.label(), to.label())));
    }
    if !transition(&mut e, to, turn, reason) {
        return Ok(false);
    }
    save(world, campaign_id, &e, &format!("剧情事件「{}」→ {}：{}", e.title, to.label(), reason))?;
    if e.kind == "quest"
        && let Some(q) = to.to_quest(refused)
    {
        campaign::apply_effects(world, campaign_id, &[Effect::SetQuestState { quest: id.into(), state: q.into() }], reason)?;
    }
    Ok(true)
}

// ---------------- 被摧毁的物品 ----------------

pub fn destroyed_items(conn: &Connection, campaign_id: &str) -> Result<Vec<String>> {
    Ok(list::<ItemState>(conn, campaign_id, ITEM_STATE)?.into_iter().filter(|s| s.destroyed).map(|s| s.item).collect())
}

pub fn is_destroyed(conn: &Connection, campaign_id: &str, item: &str) -> bool {
    CampaignView { conn, campaign_id }.record(ITEM_STATE, item).and_then(|v| serde_json::from_value::<ItemState>(v).ok()).is_some_and(|s| s.destroyed)
}

/// 剧本物品允许重复获得（payload `repeatable: true`，例如普通药水）时，被毁后还可以再得到。
pub fn repeatable(conn: &Connection, item: &str) -> bool {
    crate::scripts::get_item(conn, item).ok().flatten().and_then(|it| it.payload.get("repeatable").and_then(Value::as_bool)).unwrap_or(false)
}

/// 记下物品被摧毁：从所有持有者身上拿走，并标记为不存在。
pub fn destroy_item(world: &mut World, campaign_id: &str, item: &str, reason: &str) -> Result<()> {
    let turn = campaign::get_campaign(world.conn(), campaign_id)?.turn;
    let st = campaign::campaign_state(world.conn(), campaign_id)?;
    let mut eff = vec![];
    for (k, n) in &st.items {
        if let Some((holder, it)) = k.split_once('|')
            && it == item
            && *n > 0
        {
            eff.push(Effect::TakeItem { holder: holder.into(), item: item.into(), qty: *n });
        }
    }
    let s = ItemState { item: item.into(), destroyed: true, turn, reason: reason.chars().take(160).collect() };
    eff.push(Effect::UpsertRecord { kind: ITEM_STATE.into(), id: item.into(), data: serde_json::to_value(&s)? });
    campaign::apply_effects(world, campaign_id, &eff, &format!("物品被摧毁：{reason}"))?;
    Ok(())
}

// ---------------- 玩家目标 ----------------

pub fn player_goal(conn: &Connection, campaign_id: &str) -> Option<PlayerGoal> {
    CampaignView { conn, campaign_id }.record(PLAYER_GOAL, "main").and_then(|v| serde_json::from_value(v).ok()).filter(|g: &PlayerGoal| !g.text.trim().is_empty())
}

pub fn set_player_goal(world: &mut World, campaign_id: &str, text: &str) -> Result<()> {
    let text: String = text.trim().chars().take(120).collect();
    if text.is_empty() {
        return Ok(());
    }
    let turn = campaign::get_campaign(world.conn(), campaign_id)?.turn;
    let mut g = player_goal(world.conn(), campaign_id).unwrap_or_default();
    if g.text == text {
        return Ok(());
    }
    if !g.text.is_empty() {
        g.history.push((g.turn, g.text.clone()));
        if g.history.len() > 10 {
            g.history.remove(0);
        }
    }
    g.text = text;
    g.turn = turn;
    campaign::apply_effects(world, campaign_id, &[Effect::UpsertRecord { kind: PLAYER_GOAL.into(), id: "main".into(), data: serde_json::to_value(&g)? }], "玩家的目标")?;
    Ok(())
}

// ---------------- 给 GM 的文字 ----------------

/// 已确定的事实（不能被叙事改写）：死亡与失踪的人物、被摧毁的物品、已经结束的事件。
pub fn settled_facts(conn: &Connection, campaign_id: &str) -> Result<Vec<String>> {
    let mut out = vec![];
    for s in crate::schedule::all_npc_status(conn, campaign_id)? {
        match s.life_status.as_str() {
            "dead" => out.push(format!("{}已经死亡（不会再出现、说话或行动）", s.name)),
            "missing" => out.push(format!("{}已经失踪（不在任何场景里）", s.name)),
            _ => {}
        }
    }
    for it in list::<ItemState>(conn, campaign_id, ITEM_STATE)?.into_iter().filter(|s| s.destroyed) {
        let name = crate::scripts::get_item(conn, &it.item).ok().flatten().map(|i| i.title).or_else(|| crate::nodes::get_node(conn, &it.item).ok().flatten().map(|n| n.title)).unwrap_or(it.item.clone());
        out.push(format!("{name}已经被摧毁（世界上不再有它，不能再找到或得到一件同样的）"));
    }
    for e in events(conn, campaign_id)?.into_iter().filter(|e| e.status.terminal()) {
        let what = if e.kind == "quest" { "委托" } else { "事件" };
        out.push(format!("{what}「{}」{}{}", e.title, e.status.label(), if e.reason.is_empty() { String::new() } else { format!("（{}）", e.reason) }));
    }
    Ok(out)
}

/// 还没发生的事件（潜在事件，主持人信息）。不含任务（任务另有一节）。返回 (标题, 状态, 描述, ID)。
pub fn potential(conn: &Connection, campaign_id: &str) -> Result<Vec<PlotEvent>> {
    Ok(events(conn, campaign_id)?.into_iter().filter(|e| !e.status.terminal() && e.kind != "quest").collect())
}

/// 已经结束的任务 ID（拒绝、绕过、失效、被改变、完成、失败）。
pub fn closed_quests(conn: &Connection, campaign_id: &str) -> Result<BTreeMap<String, EventStatus>> {
    let mut m = BTreeMap::new();
    for e in events(conn, campaign_id)?.into_iter().filter(|e| e.kind == "quest" && e.status.terminal()) {
        m.insert(e.event_id, e.status);
    }
    for (q, s) in campaign::campaign_state(conn, campaign_id)?.quests {
        if let Some(st) = EventStatus::from_quest(&s).filter(|s| s.terminal()) {
            m.entry(q).or_insert(st);
        }
    }
    Ok(m)
}

/// 界面用：全部事件（含原因）。
pub fn view(conn: &Connection, campaign_id: &str) -> Result<Vec<Value>> {
    Ok(events(conn, campaign_id)?
        .into_iter()
        .map(|e| json!({ "id": e.event_id, "kind": e.kind, "title": e.title, "status": e.status, "label": e.status.label(), "reason": e.reason, "updated_turn": e.updated_turn }))
        .collect())
}

//! GM 控制台的数据（只给主持人看）：当前场景、世界状态与时间、人物的目标情绪与状态、隐藏剧情、线索与触发条件、
//! AI 记忆检索记录与决策依据、剧情冲突、导演建议；以及主持人手动修改游戏状态（写入事件日志，可以回溯）。

use std::collections::BTreeMap;

use chronicle_rules::{Effect, StateView};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::campaign::{self, CampaignView};
use crate::director::{self, Conflict, DirectorNote, DirectorSettings, Pacing};
use crate::error::{CoreError, Result};
use crate::model::ScriptItemKind;
use crate::world::World;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ConsoleEdge {
    pub label: String,
    pub to: String,
    pub availability: String,
    pub condition: String,
    pub used: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ConsoleScene {
    pub title: String,
    pub kind: String,
    pub player_text: String,
    pub gm_text: String,
    pub location: String,
    pub present: Vec<String>,
    pub edges: Vec<ConsoleEdge>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ConsoleNpc {
    pub id: String,
    pub name: String,
    pub role: String,
    pub goals: Vec<String>,
    pub secrets: Vec<String>,
    pub activity: String,
    pub location: String,
    pub life_status: String,
    pub mood: Option<chronicle_rules::effect::Mood>,
    pub bond: Option<chronicle_rules::effect::Bond>,
    pub hp: i64,
    pub max_hp: i64,
    pub level: i64,
    pub present: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ConsoleClue {
    pub id: String,
    pub title: String,
    pub revealed: bool,
    pub player_text: String,
    pub gm_text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ConsoleCondition {
    /// 条件的用途：自动触发 / 自动开始 / 完成 / 失败。
    pub role: String,
    pub owner: String,
    pub condition: String,
    /// 此刻的判定：满足 / 不满足 / 需要检定。
    pub now: String,
    pub fired: bool,
    pub note: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ConsoleHidden {
    pub core_setting: String,
    /// 还没到达的剧情节点：(标题, 主持人信息)。
    pub upcoming: Vec<(String, String)>,
    pub hidden_quests: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct TurnTrace {
    pub turn: i64,
    pub input: String,
    pub intent: String,
    /// GM 说明的决定依据（已换成文字）。
    pub based_on: Vec<String>,
    pub narration: String,
    /// 上下文记录（各部分是否放入、检索到的记忆与设定、采用中的导演建议）。
    #[ts(type = "unknown")]
    pub context: Value,
    /// NPC 回应时检索到的记忆：(人物, 记忆)。
    pub npc_memories: Vec<(String, Vec<String>)>,
    pub rejected: Vec<String>,
    pub conflicts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ManualEdit {
    pub turn: i64,
    pub reason: String,
    pub effects: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct GmConsole {
    pub scene: ConsoleScene,
    pub time: String,
    pub environment: crate::worldsim::Environment,
    pub location: String,
    pub vars: BTreeMap<String, String>,
    pub npcs: Vec<ConsoleNpc>,
    pub clues: Vec<ConsoleClue>,
    pub conditions: Vec<ConsoleCondition>,
    pub hidden: ConsoleHidden,
    pub pacing: Pacing,
    pub conflicts: Vec<Conflict>,
    pub notes: Vec<DirectorNote>,
    pub settings: DirectorSettings,
    pub traces: Vec<TurnTrace>,
    pub manual_edits: Vec<ManualEdit>,
    /// 因果链（决定 → 后果）与伏笔。
    #[serde(default)]
    pub consequences: Vec<crate::story::Consequence>,
    #[serde(default)]
    pub foreshadows: Vec<crate::story::Foreshadow>,
    /// 剧情事件的动态状态（反剧情纠正）。
    #[serde(default)]
    pub events: Vec<crate::events::PlotEvent>,
    /// 玩家当前的目标。
    #[serde(default)]
    pub player_goal: Option<crate::events::PlayerGoal>,
}

fn list_field(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn jstr(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

/// 最近几个回合的 AI 决策记录（按回合分组）。
pub fn traces(conn: &Connection, campaign_id: &str, turns: usize) -> Result<Vec<TurnTrace>> {
    let evs = campaign::recent_events(conn, campaign_id, &["player", "gm_plan", "narration", "context_trace", "npc", "effects_rejected", "conflict"], 400)?;
    let mut by: BTreeMap<i64, TurnTrace> = BTreeMap::new();
    for e in evs.into_iter().rev() {
        let t = by.entry(e.turn).or_insert_with(|| TurnTrace { turn: e.turn, input: String::new(), intent: String::new(), based_on: vec![], narration: String::new(), context: Value::Null, npc_memories: vec![], rejected: vec![], conflicts: vec![] });
        let p = &e.payload;
        match e.kind.as_str() {
            "player" => t.input = p["text"].as_str().unwrap_or_default().into(),
            "gm_plan" => {
                t.intent = p["intent"].as_str().unwrap_or_default().into();
                t.based_on = list_field(p, "based_on");
            }
            "narration" => t.narration = p["text"].as_str().unwrap_or_default().into(),
            "context_trace" => t.context = p.clone(),
            "npc" => t.npc_memories.push((p["name"].as_str().unwrap_or_default().into(), list_field(p, "memories"))),
            "effects_rejected" => t.rejected.extend(p["rejected"].as_array().into_iter().flatten().map(|r| r["reason"].as_str().unwrap_or_default().to_string())),
            _ => t.conflicts.extend(list_field(p, "items")),
        }
    }
    Ok(by.into_values().rev().filter(|t| !t.input.is_empty() || !t.narration.is_empty()).take(turns).collect())
}

pub fn console(conn: &Connection, campaign_id: &str) -> Result<GmConsole> {
    let view = CampaignView { conn, campaign_id };
    let c = campaign::get_campaign(conn, campaign_id)?;
    let script = match &c.script_id {
        Some(s) => crate::scripts::get_script(conn, s)?,
        None => None,
    };
    let items = match &c.script_id {
        Some(s) => crate::scripts::all_items(conn, s)?,
        None => vec![],
    };
    let title_of = |id: &str| -> String {
        conn.query_row(
            "SELECT title FROM script_items WHERE id = ?1 UNION ALL SELECT title FROM plot_nodes WHERE id = ?1 UNION ALL SELECT title FROM nodes WHERE id = ?1 LIMIT 1",
            [id],
            |r| r.get::<_, String>(0),
        )
        .unwrap_or_else(|_| id.to_string())
    };
    let location = crate::worldsim::party_location(conn, campaign_id)?;
    let location_title = location.as_deref().map(title_of).unwrap_or_default();
    let npc_status = crate::schedule::all_npc_status(conn, campaign_id)?;
    let present_ids: Vec<String> = npc_status
        .iter()
        .filter(|s| s.life_status != "dead" && (!s.has_schedule || location.as_deref().is_some_and(|l| crate::schedule::is_at(conn, s, l).unwrap_or(false))))
        .map(|s| s.npc_id.clone())
        .collect();

    // 场景。
    let mut scene = ConsoleScene { location: location_title.clone(), ..Default::default() };
    if let Some(n) = campaign::current_node_conn(conn, campaign_id)? {
        scene.title = n.title;
        scene.kind = n.kind.to_string();
        scene.player_text = n.player_text;
        scene.gm_text = n.gm_text;
        for o in campaign::available_edges_conn(conn, campaign_id)? {
            scene.edges.push(ConsoleEdge {
                label: o.edge.label.clone(),
                to: title_of(&o.edge.to_id),
                availability: match o.availability {
                    chronicle_rules::Tri::True => "满足".into(),
                    chronicle_rules::Tri::False => "不满足".into(),
                    chronicle_rules::Tri::Unknown => "需要检定".into(),
                },
                condition: o.edge.condition.as_ref().map(|c| serde_json::to_string(c).unwrap_or_default()).unwrap_or_else(|| "无".into()),
                used: o.used_up,
            });
        }
    }
    scene.present = npc_status.iter().filter(|s| present_ids.contains(&s.npc_id)).map(|s| s.name.clone()).collect();

    // 人物。
    let cast = match &c.script_id {
        Some(s) => crate::scripts::cast_of(conn, s)?,
        None => vec![],
    };
    let mut npcs = vec![];
    for s in &npc_status {
        let node = crate::nodes::get_node(conn, &s.npc_id)?;
        let cm = cast.iter().find(|x| x.node_id == s.npc_id);
        let mut goals = vec![];
        if let Some(cm) = cm
            && !cm.motive.trim().is_empty()
        {
            goals.push(cm.motive.clone());
        }
        if let Some(n) = &node {
            if let Some(m) = n.payload.get("motivation").and_then(Value::as_str).filter(|m| !m.is_empty()) {
                goals.push(m.to_string());
            }
            goals.extend(list_field(&n.payload, "goals"));
        }
        let mut secrets = node.as_ref().map(|n| list_field(&n.payload, "secrets")).unwrap_or_default();
        if let Some(cm) = cm
            && !cm.secret.trim().is_empty()
        {
            secrets.push(cm.secret.clone());
        }
        if let Some(n) = &node
            && !n.gm_text.trim().is_empty()
        {
            secrets.push(n.gm_text.clone());
        }
        let sh = crate::rpg::sheet(conn, campaign_id, &s.npc_id).ok();
        npcs.push(ConsoleNpc {
            id: s.npc_id.clone(),
            name: s.name.clone(),
            role: cm.map(|x| x.role.clone()).unwrap_or_default(),
            goals,
            secrets,
            activity: s.activity.clone(),
            location: s.location_name.clone(),
            life_status: s.life_status.clone(),
            mood: view.mood(&s.npc_id),
            bond: view.bond(&s.npc_id, "party").map(|(_, b)| b),
            hp: sh.as_ref().map(|x| x.hp).unwrap_or(0),
            max_hp: sh.as_ref().map(|x| x.max_hp).unwrap_or(0),
            level: sh.as_ref().map(|x| x.level).unwrap_or(0),
            present: present_ids.contains(&s.npc_id),
        });
    }

    // 线索与条件。
    let mut clues = vec![];
    let mut conditions = vec![];
    let mut hidden = ConsoleHidden { core_setting: script.as_ref().map(|s| s.core_setting.clone()).unwrap_or_default(), ..Default::default() };
    for it in &items {
        match it.kind {
            ScriptItemKind::Clue => clues.push(ConsoleClue { id: it.id.clone(), title: it.title.clone(), revealed: view.clue_known(&it.id), player_text: it.player_text.clone(), gm_text: it.gm_text.clone() }),
            ScriptItemKind::Trigger | ScriptItemKind::KeyEvent => {
                if let Some(w) = it.payload.get("when").filter(|w| w.is_object()) {
                    conditions.push(ConsoleCondition {
                        role: "自动触发".into(),
                        owner: it.title.clone(),
                        condition: jstr(w),
                        now: director::eval(conn, campaign_id, w),
                        fired: view.record("trigger_fired", &it.id).is_some(),
                        note: it.payload.get("text").and_then(Value::as_str).unwrap_or_default().into(),
                    });
                }
            }
            _ => {}
        }
    }
    for q in crate::quests::quests(conn, campaign_id, true)? {
        if q.quest_type == "hidden" && q.status.is_empty() {
            hidden.hidden_quests.push(q.title.clone());
        }
        for (role, cond) in [("自动开始", &q.precondition), ("完成", &q.complete_when), ("失败", &q.fail_when)] {
            if let Some(w) = cond.as_ref().filter(|w| w.is_object()) {
                conditions.push(ConsoleCondition {
                    role: role.into(),
                    owner: format!("任务「{}」", q.title),
                    condition: jstr(w),
                    now: director::eval(conn, campaign_id, w),
                    fired: match role {
                        "自动开始" => !q.status.is_empty(),
                        "完成" => q.status == "done",
                        _ => q.status == "failed",
                    },
                    note: if q.status.is_empty() { "未开始".into() } else { q.status.clone() },
                });
            }
        }
    }
    if let Some(s) = &c.script_id {
        let visited: Vec<String> = {
            let mut stmt = conn.prepare("SELECT plot_node_id FROM plot_visits WHERE campaign_id = ?1")?;
            stmt.query_map([campaign_id], |r| r.get(0))?.filter_map(|r| r.ok()).collect()
        };
        for n in crate::plot::plot_nodes_of(conn, s)? {
            if !visited.contains(&n.id) {
                hidden.upcoming.push((n.title, n.gm_text));
            }
        }
    }

    // 变量（不含程序内部记录与大段设置）。
    let st = campaign::campaign_state(conn, campaign_id)?;
    let vars = st.vars.iter().filter(|(k, _)| !matches!(k.as_str(), "rules.config" | "director.settings")).map(|(k, v)| (k.clone(), v.to_string())).collect();

    let manual_edits = campaign::recent_events(conn, campaign_id, &["manual_edit"], 30)?
        .into_iter()
        .map(|e| ManualEdit { turn: e.turn, reason: e.payload["reason"].as_str().unwrap_or_default().into(), effects: e.payload["effects"].as_array().into_iter().flatten().map(jstr).collect() })
        .collect();

    Ok(GmConsole {
        events: crate::events::events(conn, campaign_id)?,
        player_goal: crate::events::player_goal(conn, campaign_id),
        scene,
        time: crate::schedule::format_clock(crate::schedule::clock(conn, campaign_id)),
        environment: crate::worldsim::environment(conn, campaign_id)?,
        location: location_title,
        vars,
        npcs,
        clues,
        conditions,
        hidden,
        pacing: director::pacing(conn, campaign_id)?,
        conflicts: director::conflicts(conn, campaign_id)?,
        notes: director::notes(conn, campaign_id, 10)?,
        settings: director::settings(conn, campaign_id),
        traces: traces(conn, campaign_id, 10)?,
        manual_edits,
        consequences: crate::story::consequences(conn, campaign_id)?,
        foreshadows: crate::story::foreshadows(conn, campaign_id)?,
    })
}

/// 主持人手动修改游戏状态：必须写明原因；写入事件日志（可以回溯、参与重放校验），并在控制台里留下记录。
pub fn apply_manual(world: &mut World, campaign_id: &str, effects: &[Effect], reason: &str) -> Result<usize> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(CoreError::Invalid("手动修改需要写明原因".into()));
    }
    if effects.is_empty() {
        return Err(CoreError::Invalid("没有要执行的修改".into()));
    }
    let changes = campaign::apply_effects(world, campaign_id, effects, &format!("主持人手动修改：{reason}"))?;
    campaign::record(world.conn(), campaign_id, "manual_edit", &json!({ "reason": reason, "effects": effects }))?;
    Ok(changes.len())
}

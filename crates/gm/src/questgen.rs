//! 生成任务：根据当前剧情，或根据剧本世界观。生成结果先给玩家确认（附带和已有事实的冲突检查），确认后才加入战役。

use std::sync::Mutex;

use chronicle_core::World;
use chronicle_core::model::ScriptItemKind;
use chronicle_core::quests::{self, NewQuest};
use chronicle_core::{campaign, scripts};
use chronicle_gen::Templates;
use chronicle_llm::{CancelToken, LlmClient};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ask::{GmOptions, ask_limited};
use crate::context::build_gm_context;
use crate::{GmError, Result};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct QuestGenOut {
    pub title: String,
    /// side / main
    #[serde(default)]
    pub quest_type: String,
    pub background: String,
    #[serde(default)]
    pub objectives: Vec<String>,
    pub completion: String,
    pub consequences: String,
    #[serde(default)]
    pub npcs: Vec<String>,
    #[serde(default)]
    pub location: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct QuestProposal {
    pub quest: NewQuest,
    /// 相关人物 / 地点的名字（界面显示）。
    pub npc_names: Vec<String>,
    pub location_name: String,
    /// 和已有事实的冲突或疑点（空 = 没发现）。
    pub conflicts: Vec<String>,
    /// situation / worldview
    pub mode: String,
}

/// 按名称（或别名）在世界树里找节点。
fn find_node(w: &World, title: &str, kinds: &[&str]) -> Option<String> {
    let t = title.trim();
    if t.is_empty() {
        return None;
    }
    let mut stmt = w.conn().prepare("SELECT id, kind, title, aliases FROM nodes WHERE deleted_at IS NULL").ok()?;
    let rows: Vec<(String, String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .ok()?
        .filter_map(|x| x.ok())
        .collect();
    rows.into_iter().find_map(|(id, kind, tt, aliases)| {
        let hit = tt == t || aliases.contains(&format!("\"{t}\""));
        (hit && kinds.contains(&kind.as_str())).then_some(id)
    })
}

#[allow(clippy::too_many_arguments)]
pub fn generate_quest(world: &Mutex<World>, client: &dyn LlmClient, templates: &Templates, opts: &GmOptions, campaign_id: &str, mode: &str, hint: &str, cancel: &CancelToken) -> Result<QuestProposal> {
    let mode = if mode == "worldview" { "worldview" } else { "situation" };
    let (meta, material, existing, play) = {
        let w = world.lock().map_err(|_| GmError::Input("世界文件被占用".into()))?;
        let c = campaign::get_campaign(w.conn(), campaign_id)?;
        let script = match &c.script_id {
            Some(sid) => Some(scripts::require_script(w.conn(), sid)?),
            None => None,
        };
        let meta = script.as_ref().map(chronicle_gen::context::script_meta).unwrap_or_else(|| "（自由沙盒，没有剧本）".into());
        let play = script.as_ref().map(chronicle_core::profile::for_script).unwrap_or_default().guidance();
        let material = if mode == "situation" {
            build_gm_context(&w, campaign_id, hint, 6000)?.text
        } else {
            let mut parts = vec![];
            if let Some(s) = &script {
                parts.push(chronicle_gen::context::script_outline(&w, &s.id, false)?);
                parts.push(chronicle_gen::context::cast_text(&w, &s.id)?);
                let st = campaign::campaign_state(w.conn(), campaign_id)?;
                let pending: Vec<String> = scripts::all_items(w.conn(), &s.id)?
                    .into_iter()
                    .filter(|i| matches!(i.kind, ScriptItemKind::Quest | ScriptItemKind::KeyEvent | ScriptItemKind::Scene) && !st.quests.contains_key(&i.id))
                    .take(20)
                    .map(|i| format!("- {}：{}", i.title, i.player_text))
                    .collect();
                if !pending.is_empty() {
                    parts.push(format!("尚未触发的内容：\n{}", pending.join("\n")));
                }
            }
            if parts.is_empty() {
                parts.push(build_gm_context(&w, campaign_id, hint, 6000)?.text);
            }
            parts.join("\n\n")
        };
        let existing: Vec<String> = quests::quests(w.conn(), campaign_id, true)?
            .into_iter()
            .map(|q| format!("- {}（{}）", q.title, if q.status.is_empty() { "未开始" } else { q.status.as_str() }))
            .collect();
        (meta, material, if existing.is_empty() { "（无）".to_string() } else { existing.join("\n") }, play)
    };
    let mode_text = if mode == "situation" {
        "根据当前剧情：结合玩家此刻所在的场景、刚做过的事、和人物的关系、世界的状态，设计一个顺势而来的任务。"
    } else {
        "根据剧本世界观：结合主线、背景事件、人物和尚未触发的内容，设计一个能丰富世界、和主线有呼应的任务。"
    };
    let hint_text = if hint.trim().is_empty() { "（没有特别要求）" } else { hint.trim() };
    let user = templates.render("quest_generate", serde_json::json!({ "mode_text": mode_text, "meta": meta, "material": material, "existing": existing, "hint": hint_text, "play": play }))?;
    let system = templates.render("system", serde_json::json!({ "schema_text": "" }))?;
    let mut logs = vec![];
    let o = ask_limited::<QuestGenOut>(client, &system, &user, opts, 1500, cancel, &mut logs)?.value;
    if o.title.trim().is_empty() {
        return Err(GmError::Input("模型没有给出任务名称，请重试".into()));
    }
    let w = world.lock().map_err(|_| GmError::Input("世界文件被占用".into()))?;
    Ok(check_proposal(&w, campaign_id, o, mode)?)
}

/// 冲突检查（程序）：同名任务、人物不存在或已死亡、地点不存在。
pub fn check_proposal(w: &World, campaign_id: &str, o: QuestGenOut, mode: &str) -> chronicle_core::Result<QuestProposal> {
    let mut conflicts = vec![];
    if quests::quests(w.conn(), campaign_id, true)?.iter().any(|q| q.title.trim() == o.title.trim()) {
        conflicts.push(format!("已经有同名任务「{}」，确认后不会重复创建", o.title.trim()));
    }
    let st = campaign::campaign_state(w.conn(), campaign_id)?;
    let mut npc_ids = vec![];
    let mut npc_names = vec![];
    for n in o.npcs.iter().map(|x| x.trim()).filter(|x| !x.is_empty()).take(4) {
        match find_node(w, n, &["character"]) {
            Some(id) => {
                if st.entities.get(&format!("{id}|life_status")).and_then(|v| v.as_str()) == Some("dead") {
                    conflicts.push(format!("「{n}」在本战役中已经死亡"));
                }
                npc_ids.push(id);
            }
            None => conflicts.push(format!("人物「{n}」不在世界树里（确认后只作为文字出现）")),
        }
        npc_names.push(n.to_string());
    }
    let loc = find_node(w, &o.location, chronicle_core::worldsim::PLACE_KINDS);
    if !o.location.trim().is_empty() && loc.is_none() {
        conflicts.push(format!("地点「{}」不在世界树里", o.location.trim()));
    }
    let qt = if matches!(o.quest_type.as_str(), "main" | "side") { o.quest_type.clone() } else { "side".into() };
    let mut objectives = o.objectives.iter().map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
    let objective = objectives.next().unwrap_or_default();
    let rest: Vec<String> = objectives.collect();
    Ok(QuestProposal {
        quest: NewQuest {
            title: o.title.trim().chars().take(40).collect(),
            objective: if rest.is_empty() { objective } else { format!("{objective}；{}", rest.join("；")) },
            quest_type: qt,
            deadline_turns: None,
            npc_ids,
            location: loc,
            clue_ids: vec![],
            background: o.background,
            completion: o.completion,
            consequences: o.consequences,
            origin: "generated".into(),
        },
        npc_names,
        location_name: o.location.trim().to_string(),
        conflicts,
        mode: mode.into(),
    })
}

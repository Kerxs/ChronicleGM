//! NPC 的主动行为（第 24 项，模型部分）：每个故事日最多调用一次模型，让几个人物按自己的目标做一件事。
//! 结果只记成世界事件和人物自己的记忆（不直接改状态），之后 GM 和 NPC 的上下文会看到；不会让人物死亡。

use std::sync::Mutex;

use chronicle_core::World;
use chronicle_core::campaign::{self, CampaignView};
use chronicle_core::rules::{Effect, StateView};
use chronicle_gen::Templates;
use chronicle_llm::{CancelToken, LlmClient};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::ask::{GmOptions, ask_limited};
use crate::{GmError, Result};

pub const DAY_VAR: &str = "npcsim.llm_day";

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct NpcActionOut {
    pub npc: String,
    pub action: String,
    #[serde(default)]
    pub public: bool,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct NpcDayOut {
    /// 最多 3 条。
    pub actions: Vec<NpcActionOut>,
}

/// 今天还没推演过就推演一次（先记下"今天推演过"，失败也不重试，保证每天最多一次模型调用）。返回记下的事件文字。
pub fn maybe_run(world: &Mutex<World>, client: &dyn LlmClient, templates: &Templates, opts: &GmOptions, campaign_id: &str, cancel: &CancelToken) -> Result<Option<Vec<String>>> {
    let (day, npcs_text, events_text, names) = {
        let mut w = world.lock().map_err(|_| GmError::Input("世界文件被占用".into()))?;
        let c = campaign::get_campaign(w.conn(), campaign_id)?;
        if c.status != "active" {
            return Ok(None);
        }
        let day = chronicle_core::schedule::clock(w.conn(), campaign_id).div_euclid(1440) + 1;
        let done = CampaignView { conn: w.conn(), campaign_id }.var(DAY_VAR).and_then(|v| v.as_i64()).unwrap_or(0);
        if done >= day {
            return Ok(None);
        }
        let st = campaign::campaign_state(w.conn(), campaign_id)?;
        let view = CampaignView { conn: w.conn(), campaign_id };
        let mut lines = vec![];
        let mut names: Vec<(String, String)> = vec![];
        for s in chronicle_core::schedule::all_npc_status(w.conn(), campaign_id)? {
            if st.entities.get(&format!("{}|life_status", s.npc_id)).and_then(|v| v.as_str()).is_some_and(|l| l != "alive") || s.life_status != "alive" {
                continue;
            }
            let Some(n) = chronicle_core::nodes::get_node(w.conn(), &s.npc_id)? else { continue };
            let goal = ["motivation", "goals", "goal"].iter().find_map(|k| n.payload.get(*k)).map(|v| if let Some(a) = v.as_array() { a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("；") } else { v.as_str().unwrap_or_default().to_string() }).unwrap_or_default();
            if goal.trim().is_empty() {
                continue; // 没有目标的人物不做主动推演
            }
            let m = view.mood(&s.npc_id).unwrap_or_default();
            lines.push(format!("- {}：目标「{}」；恐惧 {} 愤怒 {} 压力 {}；{}", n.title, goal, m.fear, m.anger, m.stress, if s.location_name.is_empty() { "位置不明".to_string() } else { format!("在{}", s.location_name) }));
            names.push((n.title.clone(), n.id.clone()));
            if names.len() >= 8 {
                break;
            }
        }
        // 先记下今天推演过（没人可推演也算）。
        campaign::apply_effects(&mut w, campaign_id, &[Effect::SetVar { key: DAY_VAR.into(), value: json!(day) }], "NPC 每日推演")?;
        if names.is_empty() {
            return Ok(None);
        }
        let events: Vec<String> = chronicle_core::worldsim::world_events(w.conn(), campaign_id, 6)?.into_iter().map(|e| format!("- 第 {} 天：{}", e.day, e.text)).collect();
        (day, lines.join("\n"), if events.is_empty() { "（平静）".to_string() } else { events.join("\n") }, names)
    };
    let user = templates.render("npc_day", json!({ "day": day, "npcs": npcs_text, "events": events_text }))?;
    let mut logs = vec![];
    let out = ask_limited::<NpcDayOut>(client, "你是 TRPG 的世界推演助手。只输出一个 JSON 对象，不要解释。使用简体中文。", &user, opts, 800, cancel, &mut logs)?.value;
    let mut w = world.lock().map_err(|_| GmError::Input("世界文件被占用".into()))?;
    let mut eff = vec![];
    let mut notes = vec![];
    for (i, a) in out.actions.iter().take(3).enumerate() {
        let Some((name, id)) = names.iter().find(|(n, _)| n == a.npc.trim()) else { continue };
        let action: String = a.action.trim().chars().take(80).collect();
        if action.is_empty() || ["死了", "死亡", "被杀", "自杀"].iter().any(|x| action.contains(x)) {
            continue;
        }
        let text = format!("{name}{action}");
        eff.push(Effect::UpsertRecord { kind: "world_event".into(), id: format!("d{day:06}-npc-{i}"), data: json!({ "day": day, "kind": if a.public { "npc_action" } else { "npc_secret" }, "text": text, "factions": [id] }) });
        campaign::add_npc_memory(&w, campaign_id, id, &format!("第 {day} 天，我{action}"), 4)?;
        notes.push(text);
    }
    if !eff.is_empty() {
        campaign::apply_effects(&mut w, campaign_id, &eff, &format!("第 {day} 天：人物的主动行动"))?;
    }
    Ok(Some(notes))
}

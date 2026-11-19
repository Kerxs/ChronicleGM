//! 仪式演出：重要的时刻有专属的演出。等级和场景由程序根据**实际发生的事**判定（不靠 AI）：
//!
//! - 一级 即时反馈：普通检定、物品使用（短动画、轻音效）——由骰子托盘和小卡片负责，这里不产生；
//! - 二级 交互演出：开锁、机关、组合的操作过程——由互动组件负责；
//! - 三级 重要事件：进入新地点、人物首次登场、获得重要物品、发现关键线索、战斗开始、角色濒死、剧情转折；
//! - 四级 史诗演出：开始新剧本（序章）、剧本结局、重大转折（转折 / 结局事件发生）。

use chronicle_core::campaign;
use chronicle_core::rules::Effect;
use chronicle_core::rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::turn::TurnResult;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct RitualCue {
    /// 1–4
    pub level: u8,
    /// prologue / location / npc / item / clue / combat / near_death / turning / ending / event
    pub scene: String,
    pub title: String,
    pub subtitle: String,
    /// 相关的世界树节点（人物肖像、地点图片）。
    #[serde(default)]
    pub node_id: Option<String>,
    /// 音效名（界面合成或用户替换）：prologue / place / npc / item / clue / battle / heartbeat / turning / ending
    pub sound: String,
}

fn cue(level: u8, scene: &str, title: impl Into<String>, subtitle: impl Into<String>, node: Option<String>, sound: &str) -> RitualCue {
    RitualCue { level, scene: scene.into(), title: title.into(), subtitle: subtitle.into(), node_id: node, sound: sound.into() }
}

fn short(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n { format!("{t}…") } else { t }
}

/// 这一回合的演出。`opening` 为开场回合；`spoken_before` 是这一回合之前说过话的人物。
pub fn cues(conn: &Connection, campaign_id: &str, out: &TurnResult, opening: bool, spoken_before: &[String], visited_before: &[String]) -> Vec<RitualCue> {
    let mut v = vec![];
    if opening {
        let c = campaign::get_campaign(conn, campaign_id).ok();
        let script = c.as_ref().and_then(|c| c.script_id.as_deref()).and_then(|s| chronicle_core::scripts::get_script(conn, s).ok().flatten());
        v.push(cue(4, "prologue", script.as_ref().map(|s| s.title.clone()).unwrap_or_else(|| "序章".into()), script.map(|s| short(&s.premise, 60)).unwrap_or_default(), None, "prologue"));
    }
    // 进入新地点（第一次到）。
    if !out.travel.is_empty()
        && let Ok(Some(loc)) = chronicle_core::worldsim::party_location(conn, campaign_id)
        && let Ok(Some(n)) = chronicle_core::nodes::get_node(conn, &loc)
    {
        let first = !visited_before.contains(&loc);
        v.push(cue(if first { 3 } else { 1 }, "location", n.title.clone(), short(if n.summary.is_empty() { &n.player_text } else { &n.summary }, 50), Some(n.id), "place"));
    }
    // 人物首次登场。
    for l in &out.npc_lines {
        if !spoken_before.contains(&l.npc_id) && !v.iter().any(|c| c.node_id.as_deref() == Some(l.npc_id.as_str())) {
            let sub = chronicle_core::nodes::get_node(conn, &l.npc_id).ok().flatten().map(|n| short(n.payload.get("identity").and_then(|x| x.as_str()).filter(|s| !s.is_empty()).unwrap_or(&n.summary), 30)).unwrap_or_default();
            v.push(cue(3, "npc", l.name.clone(), sub, Some(l.npc_id.clone()), "npc"));
        }
    }
    // 关键线索、重要物品。
    for e in &out.applied {
        match e {
            Effect::RevealClue { clue } => {
                if let Ok(Some(it)) = chronicle_core::scripts::get_item(conn, clue) {
                    v.push(cue(3, "clue", it.title.clone(), short(&it.player_text, 50), None, "clue"));
                }
            }
            Effect::GiveItem { item, holder, .. } if *holder == chronicle_core::rpg::actor() => {
                if let Ok(Some(it)) = chronicle_core::scripts::get_item(conn, item)
                    && it.kind == chronicle_core::model::ScriptItemKind::Item
                {
                    v.push(cue(3, "item", it.title.clone(), short(&it.player_text, 50), None, "item"));
                }
            }
            _ => {}
        }
    }
    // 战斗开始。
    if let Some(cb) = &out.combat
        && cb.round <= 1
        && out.rpg_updates.iter().any(|l| l.contains("战斗开始") || l.contains("先攻"))
    {
        let foes: Vec<String> = cb.units.iter().filter(|u| u.side == "enemy").map(|u| u.name.clone()).collect();
        v.push(cue(3, "combat", "战斗开始", foes.join("、"), None, "battle"));
    }
    // 角色濒死。
    if let Ok(s) = chronicle_core::rpg::sheet(conn, campaign_id, &chronicle_core::rpg::actor())
        && s.max_hp > 0
        && (s.hp <= 0 || s.hp * 4 <= s.max_hp)
        && out.rpg_updates.iter().any(|l| l.contains("伤害") || l.contains("倒下") || l.contains("命中"))
    {
        v.push(cue(3, "near_death", if s.hp <= 0 { "你倒下了" } else { "伤势危急" }, format!("生命 {}/{}", s.hp.max(0), s.max_hp), None, "heartbeat"));
    }
    // 剧情转折：走过了剧本分支、转折 / 结局事件发生或被改变。
    if out.transition.as_ref().is_some_and(|t| t.taken)
        && let Some(to) = out.transition.as_ref().and_then(|t| t.to.clone())
        && let Ok(Some(n)) = chronicle_core::plot::get_plot_node(conn, &to)
    {
        let ending = n.kind == chronicle_core::model::PlotNodeKind::Ending;
        v.push(cue(if ending { 4 } else { 3 }, if ending { "ending" } else { "turning" }, n.title.clone(), short(&n.player_text, 60), None, if ending { "ending" } else { "turning" }));
    }
    for q in &out.quest_updates {
        if q.contains("转折") {
            v.push(cue(4, "turning", "命运转折", short(q, 40), None, "turning"));
        }
    }
    if campaign::get_campaign(conn, campaign_id).map(|c| c.status == "ended").unwrap_or(false) && !v.iter().any(|c| c.scene == "ending") {
        v.push(cue(4, "ending", "旅程的终点", String::new(), None, "ending"));
    }
    v.truncate(4);
    v
}

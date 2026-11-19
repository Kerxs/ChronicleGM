//! GM 提议的效果校验。AI 不能直接修改状态：每条效果都要通过这里的规则，才会交给程序执行。

use chronicle_core::World;
use chronicle_core::model::*;
use chronicle_core::rules::Effect;
use chronicle_core::{nodes, plot, scripts};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct RejectedEffect {
    pub effect: Value,
    pub reason: String,
}

/// 由程序管理、AI 不得设置的变量前缀（技能修正、NPC 态度）。
/// 由程序维护的变量：技能修正、NPC 态度、世界时钟、环境、玩家位置与状态（位置只能通过 travel_to 改变）、金钱、规则设置、战斗。
pub const PROTECTED_VAR_PREFIXES: &[&str] = &["skill.", "attitude.", "time.", "env.", "party.", "money.", "rules.", "combat.", "survival.", "stress."];

fn script_item_kind(world: &World, script: Option<&str>, id: &str) -> Option<ScriptItemKind> {
    let it = scripts::get_item(world.conn(), id).ok().flatten()?;
    (Some(it.script_id.as_str()) == script).then_some(it.kind)
}

fn world_entity(world: &World, id: &str) -> Result<Node, String> {
    match nodes::get_node(world.conn(), id) {
        Ok(Some(n)) => Ok(n),
        _ => Err(format!("实体 {id} 不存在")),
    }
}

/// GM 一次提议的关系变化，每项数值最多变化多少（关系应当逐步变化，而不是一句话翻转）。
pub const MAX_BOND_STEP: i64 = 15;
/// GM 一次提议的情绪变化，每项最多多少。
pub const MAX_MOOD_STEP: i64 = 10;

fn check_one(world: &World, script: Option<&str>, e: &Effect) -> Result<(), String> {
    e.validate().map_err(|x| x.to_string())?;
    let not_core = |n: &Node| {
        if n.canon_level == CanonLevel::Core {
            Err(format!("「{}」属于核心设定，GM 不能改变它", n.title))
        } else {
            Ok(())
        }
    };
    match e {
        Effect::Spawn { .. } => Err("GM 不能直接引入内容，新人物/地点请使用 new_entities".into()),
        Effect::SetVar { key, .. } | Effect::IncVar { key, .. } => {
            if PROTECTED_VAR_PREFIXES.iter().any(|p| key.starts_with(p)) {
                Err(format!("变量 {key} 由程序管理，不能由 GM 修改"))
            } else {
                Ok(())
            }
        }
        Effect::SetFlag { .. } => Ok(()),
        Effect::SetQuestState { quest, .. } => match script_item_kind(world, script, quest) {
            Some(ScriptItemKind::Quest) => Ok(()),
            // 游戏中新建的任务（战役记录）。
            _ if world
                .conn()
                .query_row("SELECT 1 FROM campaign_records WHERE kind = 'quest' AND id = ?1", [quest], |_| Ok(()))
                .is_ok() => Ok(()),
            _ => Err(format!("任务 {quest} 不存在")),
        },
        Effect::RevealClue { clue } => match script_item_kind(world, script, clue) {
            Some(ScriptItemKind::Clue) => Ok(()),
            _ => Err(format!("线索 {clue} 不存在于本剧本")),
        },
        Effect::GiveItem { item, .. } | Effect::TakeItem { item, .. } => {
            if script_item_kind(world, script, item) == Some(ScriptItemKind::Item) {
                return Ok(());
            }
            match nodes::get_node(world.conn(), item) {
                Ok(Some(n)) if n.kind == NodeKind::Item => Ok(()),
                _ => Err(format!("物品 {item} 不存在")),
            }
        }
        Effect::SetRelation { a, b, .. } => {
            for id in [a, b] {
                if id != "party" {
                    not_core(&world_entity(world, id)?)?;
                }
            }
            Ok(())
        }
        Effect::SetEntityState { entity, .. } => not_core(&world_entity(world, entity)?),
        Effect::AdjustMood { npc, fear, anger, suspicion, stress, loyalty, .. } => {
            world_entity(world, npc)?;
            if [fear, anger, suspicion, stress, loyalty].iter().any(|v| v.abs() > MAX_MOOD_STEP) {
                return Err(format!("单次情绪变化过大（每项最多 ±{MAX_MOOD_STEP}）"));
            }
            Ok(())
        }
        Effect::UpsertRecord { .. } | Effect::DeleteRecord { .. } => {
            Err("调查板与任务记录由程序管理：新发现请写在 discoveries，新任务请写在 new_quests".into())
        }
        Effect::AdjustBond { from, to, strength, trust, affection, hostility, .. } => {
            for id in [from, to] {
                if id != "party" {
                    world_entity(world, id)?;
                }
            }
            if [strength, trust, affection, hostility].iter().any(|v| v.abs() > MAX_BOND_STEP) {
                return Err(format!("单次关系变化过大（每项最多 ±{MAX_BOND_STEP}）"));
            }
            Ok(())
        }
        Effect::UnlockNode { node } => match plot::get_plot_node(world.conn(), node) {
            Ok(Some(n)) if Some(n.script_id.as_str()) == script => Ok(()),
            _ => Err(format!("剧情节点 {node} 不存在于本剧本")),
        },
    }
}

/// 解析并逐条校验。返回通过的效果与被拒绝的原因。
pub fn validate_effects(world: &World, script: Option<&str>, raw: &[Map<String, Value>]) -> (Vec<Effect>, Vec<RejectedEffect>) {
    let mut ok = vec![];
    let mut rejected = vec![];
    for m in raw.iter().take(12) {
        let v = Value::Object(m.clone());
        match serde_json::from_value::<Effect>(v.clone()) {
            Err(e) => rejected.push(RejectedEffect { effect: v, reason: format!("格式不对：{e}") }),
            Ok(eff) => match check_one(world, script, &eff) {
                Ok(()) => ok.push(eff),
                Err(reason) => rejected.push(RejectedEffect { effect: v, reason }),
            },
        }
    }
    if raw.len() > 12 {
        rejected.push(RejectedEffect { effect: Value::Null, reason: format!("一次最多 12 条效果，多出的 {} 条被忽略", raw.len() - 12) });
    }
    (ok, rejected)
}

/// 和战役事实有关的校验（反剧情纠正）：已经确定的事实不能被 GM 的效果推翻。
/// - 已经结束的任务（拒绝、绕过、失效、被改变、完成、失败）不能改回未开始或进行中；
/// - 已经死亡或失踪的人物不能被改成活着（除非人物设定 `can_revive: true`，例如亡灵、可复活的世界）；
/// - 已经被摧毁的物品不能再给任何人（除非剧本物品 `repeatable: true`）。
pub fn check_facts(world: &World, campaign_id: &str, effects: Vec<Effect>) -> (Vec<Effect>, Vec<RejectedEffect>) {
    use chronicle_core::events;
    use chronicle_core::rules::StateView;
    let conn = world.conn();
    let closed = events::closed_quests(conn, campaign_id).unwrap_or_default();
    let view = chronicle_core::campaign::CampaignView { conn, campaign_id };
    let mut ok = vec![];
    let mut rejected = vec![];
    for e in effects {
        let why: Option<String> = match &e {
            Effect::SetQuestState { quest, state } => closed
                .get(quest)
                .filter(|_| matches!(state.as_str(), "" | "active" | "available"))
                .map(|s| format!("这个任务已经{}，不能改回去", s.label())),
            Effect::SetEntityState { entity, field, value } if field == "life_status" => {
                let now = view.life_status(entity).unwrap_or_default();
                let to = value.as_str().unwrap_or_default();
                let revive = nodes::get_node(conn, entity).ok().flatten().and_then(|n| n.payload.get("can_revive").and_then(Value::as_bool)).unwrap_or(false);
                (matches!(now.as_str(), "dead" | "missing") && !matches!(to, "dead" | "missing") && !revive)
                    .then(|| format!("这个人物已经{}，世界设定不允许复活或凭空回来", if now == "dead" { "死亡" } else { "失踪" }))
            }
            Effect::GiveItem { item, .. } => (events::is_destroyed(conn, campaign_id, item) && !events::repeatable(conn, item)).then(|| "这件物品已经被摧毁，世界上不再有它".to_string()),
            _ => None,
        };
        match why {
            Some(reason) => rejected.push(RejectedEffect { effect: serde_json::to_value(&e).unwrap_or(Value::Null), reason }),
            None => ok.push(e),
        }
    }
    (ok, rejected)
}

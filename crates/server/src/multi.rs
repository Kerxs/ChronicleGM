//! 联机回合：把一个玩家的行动交给单人同一套 GM 回合流程（提示词、结构化方案、规则引擎校验、写库）。
//!
//! - 每个玩家在规则引擎里是自己的行动者 `player:<编号>`（自己的角色卡、背包、金钱、战斗单位），
//!   执行这名玩家的回合时用 [`rpg::with_actor`] 切换，单人模式不受影响（仍是 `party`）；
//! - 行动以「玩家名：行动」进入回合，GM 知道是谁在行动；
//! - 私人 GM 对话产生的整段事件只给本人看（追加一条 `mp_scope` 事件记下范围）；
//! - 战斗中只有轮到的玩家能行动；战斗按钮直接由规则引擎结算（不需要模型）。

use chronicle_core::rules::StateView;
use chronicle_core::{campaign, combat, gamestate, rpg};
use chronicle_gm::{Engine, TurnResult};
use chronicle_llm::CancelToken;
use chronicle_protocol::{CombatIntent, Visibility};
use serde_json::{Value, json};

use crate::game::{GameRoom, QueuedAction, actor_of};

/// 玩家行动在回合里的写法：带上是谁说的（多人时 GM 才知道是谁在行动）。
pub fn turn_input(a: &QueuedAction) -> String {
    let name = a.name.trim();
    if name.is_empty() { a.text.trim().to_string() } else { format!("{name}：{}", a.text.trim()) }
}

/// 战斗中：不是你的回合就不能行动（服务端判断）。
pub fn check_turn(game: &GameRoom, actor: &str) -> Result<(), String> {
    let w = game.world.lock().map_err(|_| "世界文件被占用".to_string())?;
    if let Some(c) = combat::active(w.conn(), &game.campaign_id)
        && let Some(who) = c.current_player()
        && who != actor
    {
        let name = c.units.iter().find(|u| u.actor.as_deref() == Some(who.as_str())).map(|u| u.name.clone()).unwrap_or_default();
        return Err(format!("战斗中，现在轮到{name}行动"));
    }
    Ok(())
}

pub fn run_player_turn(game: &GameRoom, engine: &Engine, a: &QueuedAction, cancel: &CancelToken) -> Result<TurnResult, String> {
    let actor = actor_of(&a.player);
    rpg::with_actor(&actor, || {
        check_turn(game, &actor)?;
        let before = {
            let w = game.world.lock().map_err(|_| "世界文件被占用".to_string())?;
            campaign::last_seq(w.conn(), &game.campaign_id).map_err(|e| e.to_string())?
        };
        let res = chronicle_gm::run_turn(&game.world, engine, &game.campaign_id, &turn_input(a), cancel);
        // 私人 GM 对话：这一回合写下的全部事件（叙述、NPC 台词、掷骰……）只给本人看。
        // 成功和失败都要标：失败的回合也可能已经写下了部分事件。
        if a.private {
            let w = game.world.lock().map_err(|_| "世界文件被占用".to_string())?;
            let after = campaign::last_seq(w.conn(), &game.campaign_id).map_err(|e| e.to_string())?;
            game.add_scope(w.conn(), before + 1, after, Visibility::Personal { player: a.player.clone() })?;
        }
        res.map_err(|e| e.to_string())
    })
}

/// 战场的公开视图：去掉战利品、经验这类只有结算才需要的数据。
pub fn public_combat(c: &combat::Combat) -> Value {
    let mut v = serde_json::to_value(c).unwrap_or(Value::Null);
    if let Some(units) = v.get_mut("units").and_then(|u| u.as_array_mut()) {
        for u in units {
            if let Some(o) = u.as_object_mut() {
                for k in ["loot", "xp", "new_wounds"] {
                    o.remove(k);
                }
            }
        }
    }
    v
}

/// 战斗按钮：规则引擎直接结算（攻击、移动、防御、闪避、技能、用物品、互动、逃跑、结束回合）。
/// 结果写成一条公开的 `combat_log` 事件（行动描述、掷骰、战场公开视图）。
pub fn run_combat_intent(game: &GameRoom, a: &QueuedAction, ci: &CombatIntent) -> Result<(), String> {
    let actor = actor_of(&a.player);
    rpg::with_actor(&actor, || {
        let mut w = game.world.lock().map_err(|_| "世界文件被占用".to_string())?;
        let act = combat::CombatAction { kind: ci.kind.clone(), target: ci.target.clone(), item: ci.item.clone(), x: ci.x, y: ci.y, custom: ci.skill.clone(), part: String::new() };
        let r = combat::act(&mut w, &game.campaign_id, &act).map_err(|e| e.to_string())?;
        let dice: Vec<Value> = r.dice.iter().map(|d| serde_json::to_value(d).unwrap_or(Value::Null)).collect();
        campaign::record(w.conn(), &game.campaign_id, "combat_log", &json!({ "player": a.player, "lines": r.lines, "dice": dice, "combat": public_combat(&r.combat) })).map_err(|e| e.to_string())?;
        let _ = gamestate::sync(&mut w, &game.campaign_id);
        Ok(())
    })
}

/// 玩家加入：登记到队伍（`party.members`），第一次加入时建一张用玩家名字的角色卡。经规则引擎写库。
pub fn ensure_player(game: &GameRoom, player: &str, name: &str) -> Result<(), String> {
    let actor = actor_of(player);
    let mut w = game.world.lock().map_err(|_| "世界文件被占用".to_string())?;
    let cid = game.campaign_id.clone();
    let mut eff = vec![];
    {
        let conn = w.conn();
        let mut members: Vec<String> = match (campaign::CampaignView { conn, campaign_id: &cid }).var("party.members") {
            Some(Value::Array(a)) => a.into_iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
            _ => vec![],
        };
        if !members.contains(&actor) {
            members.push(actor.clone());
            eff.push(chronicle_core::rules::Effect::SetVar { key: "party.members".into(), value: json!(members) });
        }
        if !rpg::has_sheet(conn, &cid, &actor) {
            let mut s = rpg::sheet(conn, &cid, &actor).map_err(|e| e.to_string())?;
            if !name.trim().is_empty() {
                s.name = name.trim().to_string();
            }
            eff.push(rpg::sheet_effect(&s));
        }
    }
    if !eff.is_empty() {
        campaign::apply_effects(&mut w, &cid, &eff, &format!("{} 加入了队伍", name.trim())).map_err(|e| e.to_string())?;
    }
    Ok(())
}

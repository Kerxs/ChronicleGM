//! 游戏状态机：一局游戏处在哪个阶段（大厅设置 → 初始化 → 开场 → 探索 ⇄ 等玩家掷骰 → 战斗 → 战后 → 结局）。
//!
//! - 状态存为战役记录 `game_state/main`，每次迁移都写事件，随时间线回溯，中途退出后能恢复到原来的阶段。
//! - 只有程序能迁移状态：合法迁移写死在 [`allowed`] 里，非法迁移返回错误。AI 的方案不能写战役记录
//!   （`gm::validate` 拒绝 UpsertRecord），所以 AI 无法改状态。
//! - 战斗阶段由战斗引擎的事实推导（[`sync`]）：有进行中的战斗就是战斗，轮到谁由先攻顺序决定。
//! - 旧战役没有状态记录时按事实推导（回合 0 = 设置，已结束 = 结局，有战斗 = 战斗，否则探索）。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::campaign::{self, CampaignView};
use crate::combat;
use crate::error::{CoreError, Result};
use crate::world::World;
use chronicle_rules::{Effect, StateView};

pub const KIND: &str = "game_state";
const ID: &str = "main";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// 新战役：还在选角色、调设置。
    Setup,
    /// 世界初始化进行中（7 步，见 [`InitStep`]）。
    Initializing,
    /// 初始化完成，等待 / 正在讲开场白。
    Opening,
    /// 自由探索、对话（普通 GM 回合）。
    Exploring,
    /// 回合停在检定处，等玩家亲手掷骰。
    AwaitingDice,
    /// 战斗中（细分见 [`CombatPhase`]）。
    Combat,
    /// 战斗刚结束：展示结算，玩家确认后回到探索。
    Aftermath,
    /// 这条时间线已经结束（只能回溯开新时间线）。
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum CombatPhase {
    /// 遭遇转场（界面播放完转场后确认，进入行动）。
    Intro,
    PlayerTurn,
    /// 程序正在结算一个行动。
    Resolving,
    NpcTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
}

/// 初始化的一步（真实的后台工作；进度只按完成的步骤走）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct InitStep {
    pub key: String,
    pub label: String,
    pub status: StepStatus,
    /// 这一步做了什么 / 为什么失败。
    #[serde(default)]
    pub note: String,
    /// 耗时（毫秒）。
    #[serde(default)]
    pub ms: i64,
}

/// 初始化的 7 个步骤（顺序固定）。
pub const INIT_STEPS: [(&str, &str); 7] = [
    ("script", "加载剧本"),
    ("world", "加载世界设定"),
    ("player", "初始化玩家角色"),
    ("npcs", "初始化 NPC"),
    ("relations", "初始化人物关系"),
    ("gm_context", "构建 AI GM 上下文"),
    ("integrity", "检查世界数据和存档"),
];

pub fn fresh_steps() -> Vec<InitStep> {
    INIT_STEPS.iter().map(|(k, l)| InitStep { key: (*k).into(), label: (*l).into(), status: StepStatus::Pending, note: String::new(), ms: 0 }).collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct GameState {
    pub phase: Phase,
    #[serde(default)]
    pub combat: Option<CombatPhase>,
    #[serde(default)]
    pub init: Vec<InitStep>,
    /// 进入这个阶段时的回合数。
    #[serde(default)]
    pub since_turn: i64,
    /// 最近一次迁移的原因。
    #[serde(default)]
    pub reason: String,
    /// 是否是按事实推导出来的（旧战役还没有状态记录）。
    #[serde(default)]
    pub derived: bool,
}

impl GameState {
    pub fn init_done(&self) -> bool {
        !self.init.is_empty() && self.init.iter().all(|s| matches!(s.status, StepStatus::Done | StepStatus::Skipped))
    }
}

/// 合法迁移表。同一阶段内的更新（初始化步骤、战斗细分阶段）也算合法。
pub fn allowed(from: Phase, to: Phase) -> bool {
    use Phase::*;
    if from == to {
        return matches!(from, Initializing | Combat | Exploring);
    }
    if to == Ended {
        return true;
    }
    matches!(
        (from, to),
        (Setup, Initializing)
            | (Setup, Exploring) // 旧流程：不做初始化，直接输入第一个行动
            | (Initializing, Setup) // 取消初始化
            | (Initializing, Opening)
            | (Opening, Exploring)
            | (Opening, AwaitingDice)
            | (Exploring, AwaitingDice)
            | (AwaitingDice, Exploring)
            | (Exploring, Combat)
            | (AwaitingDice, Combat)
            | (Combat, Aftermath)
            | (Combat, Exploring) // 主持人中止战斗
            | (Aftermath, Exploring)
            | (Aftermath, Combat) // 战后紧接着又开战
    )
}

fn read(conn: &Connection, campaign_id: &str) -> Option<GameState> {
    (CampaignView { conn, campaign_id }).record(KIND, ID).and_then(|v| serde_json::from_value(v).ok())
}

/// 当前状态：有记录就用记录（战斗细分阶段按战斗引擎修正），没有就按事实推导。
pub fn get(conn: &Connection, campaign_id: &str) -> Result<GameState> {
    let c = campaign::get_campaign(conn, campaign_id)?;
    let fight = combat::active(conn, campaign_id);
    if c.status == "ended" {
        let mut s = read(conn, campaign_id).unwrap_or_else(|| derived(Phase::Ended, c.turn));
        s.phase = Phase::Ended;
        s.combat = None;
        return Ok(s);
    }
    match read(conn, campaign_id) {
        Some(mut s) => {
            if s.phase == Phase::Combat && s.combat != Some(CombatPhase::Intro) {
                s.combat = Some(match &fight {
                    Some(f) if f.players_turn() => CombatPhase::PlayerTurn,
                    Some(_) => CombatPhase::NpcTurn,
                    None => CombatPhase::Resolving,
                });
            }
            Ok(s)
        }
        None => Ok(match fight {
            Some(f) => GameState { combat: Some(if f.players_turn() { CombatPhase::PlayerTurn } else { CombatPhase::NpcTurn }), ..derived(Phase::Combat, c.turn) },
            None if c.turn == 0 => derived(Phase::Setup, 0),
            None => derived(Phase::Exploring, c.turn),
        }),
    }
}

fn derived(phase: Phase, turn: i64) -> GameState {
    GameState { phase, combat: None, init: vec![], since_turn: turn, reason: String::new(), derived: true }
}

fn save(world: &mut World, campaign_id: &str, s: &GameState, reason: &str) -> Result<()> {
    let mut s = s.clone();
    s.derived = false;
    s.reason = reason.chars().take(80).collect();
    let data = serde_json::to_value(&s).map_err(|e| CoreError::Invalid(e.to_string()))?;
    campaign::apply_effects(world, campaign_id, &[Effect::UpsertRecord { kind: KIND.into(), id: ID.into(), data }], &format!("游戏状态：{reason}"))?;
    Ok(())
}

/// 迁移到 `to`（只有程序调用）。非法迁移返回错误，状态不变。
pub fn transition(world: &mut World, campaign_id: &str, to: Phase, reason: &str) -> Result<GameState> {
    let cur = get(world.conn(), campaign_id)?;
    if !allowed(cur.phase, to) {
        return Err(CoreError::Invalid(format!("游戏状态不能从「{}」变成「{}」", label(cur.phase), label(to))));
    }
    let turn = campaign::get_campaign(world.conn(), campaign_id)?.turn;
    let mut next = GameState { phase: to, since_turn: if cur.phase == to { cur.since_turn } else { turn }, ..cur.clone() };
    next.combat = match to {
        Phase::Combat if cur.phase != Phase::Combat => Some(CombatPhase::Intro),
        Phase::Combat => cur.combat,
        _ => None,
    };
    if to == Phase::Initializing && cur.phase != Phase::Initializing && !cur.init_done() && cur.init.is_empty() {
        next.init = fresh_steps();
    }
    if cur.phase == to && cur.combat == next.combat && cur.init == next.init {
        return Ok(cur);
    }
    save(world, campaign_id, &next, reason)?;
    get(world.conn(), campaign_id)
}

/// 更新一个初始化步骤（只能在初始化阶段）。
pub fn set_step(world: &mut World, campaign_id: &str, key: &str, status: StepStatus, note: &str, ms: i64) -> Result<GameState> {
    let mut s = get(world.conn(), campaign_id)?;
    if s.phase != Phase::Initializing {
        return Err(CoreError::Invalid("现在不在初始化阶段".into()));
    }
    if s.init.is_empty() {
        s.init = fresh_steps();
    }
    let st = s.init.iter_mut().find(|x| x.key == key).ok_or_else(|| CoreError::Invalid(format!("没有初始化步骤「{key}」")))?;
    st.status = status;
    st.note = note.chars().take(200).collect();
    st.ms = ms;
    let label = st.label.clone();
    save(world, campaign_id, &s, &format!("{label}：{note}"))?;
    Ok(s)
}

/// 战斗转场播放完：进入行动（轮到谁按先攻顺序）。
pub fn combat_ready(world: &mut World, campaign_id: &str) -> Result<GameState> {
    let mut s = get(world.conn(), campaign_id)?;
    if s.phase != Phase::Combat {
        return Err(CoreError::Invalid("现在不在战斗中".into()));
    }
    if s.combat == Some(CombatPhase::Intro) {
        s.combat = Some(CombatPhase::Resolving); // get() 会按战斗引擎换成 PlayerTurn / NpcTurn
        save(world, campaign_id, &s, "战斗开始行动")?;
    }
    get(world.conn(), campaign_id)
}

/// 按事实同步状态（每个回合、每次战斗行动之后由程序调用）：
/// 开始了战斗 → 战斗；战斗结束了 → 战后；开场回合讲完 → 探索；战役结束 → 结局。返回同步后的状态。
pub fn sync(world: &mut World, campaign_id: &str) -> Result<GameState> {
    let s = get(world.conn(), campaign_id)?;
    let c = campaign::get_campaign(world.conn(), campaign_id)?;
    if c.status == "ended" {
        return Ok(s); // get() 已经返回结局；结束的战役不能再写记录
    }
    let fighting = combat::active(world.conn(), campaign_id).is_some();
    let target = if fighting {
        Phase::Combat
    } else {
        match s.phase {
            Phase::Combat => Phase::Aftermath,
            // 讲过开场白 / 有了行动，就进入探索。
            Phase::Opening | Phase::Setup if c.turn > 0 => Phase::Exploring,
            Phase::AwaitingDice => Phase::Exploring,
            p => p,
        }
    };
    if target == s.phase && !s.derived {
        return Ok(s);
    }
    if target == s.phase {
        // 旧战役第一次同步：把推导出的状态存下来。
        save(world, campaign_id, &s, "记录当前游戏状态")?;
        return get(world.conn(), campaign_id);
    }
    // 一步到不了时（例如从设置直接进入战斗）先经过探索。
    if !allowed(s.phase, target) && allowed(s.phase, Phase::Exploring) && allowed(Phase::Exploring, target) {
        transition(world, campaign_id, Phase::Exploring, "同步")?;
    }
    transition(world, campaign_id, target, sync_reason(target))
}

fn sync_reason(p: Phase) -> &'static str {
    match p {
        Phase::Combat => "战斗开始",
        Phase::Aftermath => "战斗结束",
        Phase::Exploring => "进入探索",
        Phase::Ended => "时间线结束",
        _ => "同步",
    }
}

pub fn label(p: Phase) -> &'static str {
    match p {
        Phase::Setup => "设置",
        Phase::Initializing => "初始化",
        Phase::Opening => "开场",
        Phase::Exploring => "探索",
        Phase::AwaitingDice => "等待掷骰",
        Phase::Combat => "战斗",
        Phase::Aftermath => "战后",
        Phase::Ended => "结局",
    }
}

/// 状态迁移历史（新的在前），给剧情历史和调试看。
pub fn history(conn: &Connection, campaign_id: &str, limit: usize) -> Result<Vec<serde_json::Value>> {
    let ev = campaign::recent_events(conn, campaign_id, &["effects"], (limit * 4) as i64)?;
    Ok(ev
        .into_iter()
        .filter(|e| e.payload.to_string().contains(KIND))
        .take(limit)
        .map(|e| json!({ "turn": e.turn, "seq": e.seq, "reason": e.payload.get("reason").cloned().unwrap_or_default() }))
        .collect())
}

//! 因果与伏笔（第 26、28 项）：
//! - 决定与后果：GM 记下玩家的重要决定会带来的后果，可以延迟发生（例如被放走的敌人几回合后出现）。
//!   到期的后果交给 GM，GM 标明已经发生后就锁定，不能撤销；全部是战役记录，随时间线回溯。
//! - 伏笔：GM 埋下的伏笔（F 编号）与回收，导演提醒很久没回收的伏笔。
//! - 剧情阶段：按剧情进度与紧张度判断 铺垫 / 发展 / 高潮 / 收束。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::campaign::{self, CampaignView};
use crate::error::{CoreError, Result};
use crate::rules::{Effect, StateView};
use crate::world::World;

pub const CONSEQ: &str = "consequence";
pub const FORESHADOW: &str = "foreshadow";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Consequence {
    pub id: String,
    /// 玩家的决定。
    pub decision: String,
    /// 会带来的后果。
    pub consequence: String,
    pub created_turn: i64,
    /// 到第几回合该发生（立即的后果就是当前回合）。
    pub due_turn: i64,
    /// pending / occurred
    pub status: String,
    #[serde(default)]
    pub occurred_turn: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Foreshadow {
    pub id: String,
    pub text: String,
    pub planted_turn: i64,
    /// open / paid
    pub status: String,
    #[serde(default)]
    pub paid_turn: Option<i64>,
    #[serde(default)]
    pub payoff: String,
}

fn list<T: serde::de::DeserializeOwned>(conn: &Connection, campaign_id: &str, kind: &str) -> Result<Vec<T>> {
    let mut stmt = conn.prepare("SELECT data FROM campaign_records WHERE campaign_id = ?1 AND kind = ?2 ORDER BY id")?;
    let rows: Vec<String> = stmt.query_map([campaign_id, kind], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(rows.into_iter().filter_map(|d| serde_json::from_str(&d).ok()).collect())
}

pub fn consequences(conn: &Connection, campaign_id: &str) -> Result<Vec<Consequence>> {
    list(conn, campaign_id, CONSEQ)
}

pub fn foreshadows(conn: &Connection, campaign_id: &str) -> Result<Vec<Foreshadow>> {
    list(conn, campaign_id, FORESHADOW)
}

fn next_id(conn: &Connection, campaign_id: &str, kind: &str, prefix: &str) -> Result<String> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM campaign_records WHERE campaign_id = ?1 AND kind = ?2", [campaign_id, kind], |r| r.get(0))?;
    Ok(format!("{prefix}{:04}", n + 1))
}

/// 记下一个决定与它的后果（`after_turns` 回合后到期；0 = 这一回合就该体现）。
pub fn add_consequence(world: &mut World, campaign_id: &str, decision: &str, consequence: &str, after_turns: i64) -> Result<Consequence> {
    if consequence.trim().is_empty() {
        return Err(CoreError::Invalid("后果不能为空".into()));
    }
    let turn = campaign::get_campaign(world.conn(), campaign_id)?.turn;
    let c = Consequence {
        id: next_id(world.conn(), campaign_id, CONSEQ, "k")?,
        decision: decision.trim().chars().take(120).collect(),
        consequence: consequence.trim().chars().take(200).collect(),
        created_turn: turn,
        due_turn: turn + after_turns.clamp(0, 200),
        status: "pending".into(),
        occurred_turn: None,
    };
    campaign::apply_effects(world, campaign_id, &[Effect::UpsertRecord { kind: CONSEQ.into(), id: c.id.clone(), data: serde_json::to_value(&c)? }], "记下决定的后果")?;
    Ok(c)
}

/// 后果已经发生：锁定（之后不能撤销或改回未发生）。
pub fn mark_occurred(world: &mut World, campaign_id: &str, id: &str) -> Result<()> {
    let view = CampaignView { conn: world.conn(), campaign_id };
    let mut c: Consequence = view.record(CONSEQ, id).and_then(|v| serde_json::from_value(v).ok()).ok_or_else(|| CoreError::NotFound("后果", id.into()))?;
    if c.status == "occurred" {
        return Ok(());
    }
    c.status = "occurred".into();
    c.occurred_turn = Some(campaign::get_campaign(world.conn(), campaign_id)?.turn);
    campaign::apply_effects(world, campaign_id, &[Effect::UpsertRecord { kind: CONSEQ.into(), id: id.into(), data: serde_json::to_value(&c)? }], "后果已经发生")?;
    Ok(())
}

/// 到期还没发生的后果。
pub fn due(conn: &Connection, campaign_id: &str) -> Result<Vec<Consequence>> {
    let turn = campaign::get_campaign(conn, campaign_id)?.turn;
    Ok(consequences(conn, campaign_id)?.into_iter().filter(|c| c.status == "pending" && c.due_turn <= turn + 1).collect())
}

pub fn plant(world: &mut World, campaign_id: &str, text: &str) -> Result<Foreshadow> {
    let turn = campaign::get_campaign(world.conn(), campaign_id)?.turn;
    let f = Foreshadow { id: next_id(world.conn(), campaign_id, FORESHADOW, "f")?, text: text.trim().chars().take(160).collect(), planted_turn: turn, status: "open".into(), paid_turn: None, payoff: String::new() };
    if f.text.is_empty() {
        return Err(CoreError::Invalid("伏笔不能为空".into()));
    }
    campaign::apply_effects(world, campaign_id, &[Effect::UpsertRecord { kind: FORESHADOW.into(), id: f.id.clone(), data: serde_json::to_value(&f)? }], "埋下伏笔")?;
    Ok(f)
}

pub fn pay_off(world: &mut World, campaign_id: &str, id: &str, how: &str) -> Result<()> {
    let view = CampaignView { conn: world.conn(), campaign_id };
    let mut f: Foreshadow = view.record(FORESHADOW, id).and_then(|v| serde_json::from_value(v).ok()).ok_or_else(|| CoreError::NotFound("伏笔", id.into()))?;
    if f.status == "paid" {
        return Ok(());
    }
    f.status = "paid".into();
    f.paid_turn = Some(campaign::get_campaign(world.conn(), campaign_id)?.turn);
    f.payoff = how.trim().chars().take(160).collect();
    campaign::apply_effects(world, campaign_id, &[Effect::UpsertRecord { kind: FORESHADOW.into(), id: id.into(), data: serde_json::to_value(&f)? }], "回收伏笔")?;
    Ok(())
}

/// 剧情阶段：setup（铺垫）/ development（发展）/ climax（高潮）/ resolution（收束）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct PlotPhase {
    pub phase: String,
    pub label: String,
    /// 剧情进度 0–100（走过的剧情节点 / 全部节点；没有剧情图时按任务完成情况）。
    pub progress: i64,
    pub reason: String,
}

pub fn plot_phase(conn: &Connection, campaign_id: &str, tension: i64) -> Result<PlotPhase> {
    let c = campaign::get_campaign(conn, campaign_id)?;
    let st = campaign::campaign_state(conn, campaign_id)?;
    let (done, total) = match &c.script_id {
        Some(sid) => {
            let total: i64 = conn.query_row("SELECT COUNT(*) FROM plot_nodes WHERE script_id = ?1 AND deleted_at IS NULL", [sid], |r| r.get(0))?;
            (st.visits.len() as i64, total)
        }
        None => (0, 0),
    };
    let (done, total) = if total > 0 {
        (done.min(total), total)
    } else {
        let q = st.quests.len() as i64;
        (st.quests.values().filter(|s| matches!(s.as_str(), "done" | "failed")).count() as i64, q)
    };
    let progress = if total > 0 { done * 100 / total } else { 0 };
    let (phase, label, reason) = if c.status == "ended" {
        ("resolution", "收束", "战役已经结束".to_string())
    } else if progress >= 85 {
        ("resolution", "收束", format!("剧情进度 {progress}%，接近结局"))
    } else if progress >= 55 || (progress >= 35 && tension >= 70) {
        ("climax", "高潮", format!("剧情进度 {progress}%，紧张度 {tension}"))
    } else if progress >= 20 || c.turn >= 8 {
        ("development", "发展", format!("剧情进度 {progress}%，第 {} 回合", c.turn))
    } else {
        ("setup", "铺垫", format!("剧情进度 {progress}%，故事刚开始"))
    };
    Ok(PlotPhase { phase: phase.into(), label: label.into(), progress, reason })
}

/// 给 GM 的说明：到期的后果（必须体现）、还没回收的伏笔（编号）。返回 (文字, 编号 → 记录 ID)。
pub fn gm_text(conn: &Connection, campaign_id: &str) -> Result<(String, Vec<(String, String)>)> {
    let mut lines = vec![];
    let mut refs = vec![];
    let turn = campaign::get_campaign(conn, campaign_id)?.turn;
    let due = due(conn, campaign_id)?;
    if !due.is_empty() {
        lines.push("到期的后果（玩家之前的决定造成的，这一回合或下一回合要自然地让它发生；发生后在 consequences_done 里写编号）：".to_string());
        for (i, c) in due.iter().enumerate() {
            let code = format!("K{}", i + 1);
            lines.push(format!("- {code} 因为「{}」→ {}", c.decision, c.consequence));
            refs.push((code, c.id.clone()));
        }
    }
    let open: Vec<Foreshadow> = foreshadows(conn, campaign_id)?.into_iter().filter(|f| f.status == "open").collect();
    if !open.is_empty() {
        lines.push("已经埋下、还没回收的伏笔（合适时让它们有回响；回收时在 payoffs 里写编号）：".to_string());
        for (i, f) in open.iter().enumerate().take(8) {
            let code = format!("F{}", i + 1);
            lines.push(format!("- {code} {}（第 {} 回合埋下{}）", f.text, f.planted_turn, if turn - f.planted_turn >= 8 { "，已经很久了" } else { "" }));
            refs.push((code, f.id.clone()));
        }
    }
    Ok((lines.join("\n"), refs))
}

pub fn to_json(c: &Consequence) -> Value {
    json!(c)
}

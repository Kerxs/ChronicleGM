//! 检定：按战役的检定规则（`rpg::RuleSet::check`）掷骰并判定成功等级。
//!
//! 随机数来自战役的种子骰（`campaign::roll`，每次都写进事件日志，可以重放）；AI 只能请求检定、看到结果。
//! 成功等级：大成功 / 成功 / 部分成功（成功但有代价）/ 失败 / 大失败。部分成功算作"成功"（发现类效果照常生效），
//! 代价由 GM 叙述。

use serde::{Deserialize, Serialize};

use crate::campaign;
use crate::error::Result;
use crate::profile::CheckRules;
use crate::world::World;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Degree {
    CritSuccess,
    Success,
    Partial,
    Failure,
    CritFailure,
}

impl Degree {
    pub fn label(self) -> &'static str {
        match self {
            Degree::CritSuccess => "大成功",
            Degree::Success => "成功",
            Degree::Partial => "部分成功",
            Degree::Failure => "失败",
            Degree::CritFailure => "大失败",
        }
    }
    /// 算不算成功（部分成功也算，代价由叙述体现）。
    pub fn ok(self) -> bool {
        matches!(self, Degree::CritSuccess | Degree::Success | Degree::Partial)
    }
}

/// 一次检定的完整结果（界面展示每颗骰子与计算过程）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CheckRoll {
    /// target / roll_under / pool
    pub mode: String,
    /// 掷的骰子，例如 "1d20"、"5d6"。
    pub expr: String,
    /// 采用的那一次掷骰的每颗骰子点数。
    pub dice: Vec<i64>,
    /// 优势 / 劣势时另一次掷骰的点数（没有时为空）。
    pub other: Vec<i64>,
    /// 1 = 优势，-1 = 劣势，0 = 普通。
    pub advantage: i8,
    /// 骰面合计（骰池模式为成功的骰子数）。
    pub natural: i64,
    pub modifier: i64,
    /// target：骰面 + 修正；roll_under：骰面；pool：成功数。
    pub total: i64,
    pub dc: i64,
    /// roll_under 模式的目标值（难度 + 修正）。
    pub target: i64,
    pub degree: Degree,
    /// 一句话的计算过程，例如"1d20 掷出 14（优势：14 / 6）+ 修正 3 = 17，难度 15 → 成功"。
    pub explain: String,
}

fn parse_dice(expr: &str) -> (u32, u32) {
    // "NdM"（只取第一组）；解析不了时当作 1d20。
    let e = expr.trim().to_ascii_lowercase();
    let (n, m) = e.split_once('d').unwrap_or(("1", "20"));
    let n: u32 = if n.is_empty() { 1 } else { n.parse().unwrap_or(1) };
    let m: u32 = m.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(20);
    (n.max(1), m.max(2))
}

fn faces(r: &chronicle_rules::dice::RollResult) -> Vec<i64> {
    r.groups.iter().flat_map(|g| g.rolls.iter().map(|x| *x as i64)).collect()
}

/// 按规则掷骰并判定。`advantage`：1 优势 / -1 劣势 / 0 普通（规则关闭优势时忽略）。
pub fn perform(world: &mut World, campaign_id: &str, rules: &CheckRules, modifier: i64, dc: i64, advantage: i8, reason: &str) -> Result<CheckRoll> {
    let adv = if rules.advantage { advantage.signum() } else { 0 };
    match rules.mode.as_str() {
        "pool" => {
            let count = (rules.pool_base + modifier + adv as i64).clamp(1, 40);
            let expr = format!("{count}d{}", rules.pool_sides);
            let r = campaign::roll(world, campaign_id, &expr, &format!("{reason}（骰池）"))?;
            let dice = faces(&r);
            let hits = dice.iter().filter(|d| **d >= rules.pool_success_on).count() as i64;
            let need = dc.clamp(1, 20);
            let degree = if rules.crits && hits == 0 && dice.contains(&1) {
                Degree::CritFailure
            } else if rules.crits && hits >= need && hits as usize == dice.len() && dice.len() >= 2 {
                Degree::CritSuccess
            } else if hits >= need {
                Degree::Success
            } else if rules.partial_band > 0 && hits == need - 1 && hits > 0 {
                Degree::Partial
            } else {
                Degree::Failure
            };
            let explain = format!("掷 {count} 个 d{}（≥{} 为成功）：{:?}，成功 {hits} 个，需要 {need} → {}", rules.pool_sides, rules.pool_success_on, dice, degree.label());
            Ok(CheckRoll { mode: "pool".into(), expr, dice, other: vec![], advantage: adv, natural: hits, modifier, total: hits, dc: need, target: need, degree, explain })
        }
        mode => {
            let expr = rules.die.clone();
            let (n, sides) = parse_dice(&expr);
            let a = campaign::roll(world, campaign_id, &expr, reason)?;
            let (chosen, other) = if adv != 0 {
                let b = campaign::roll(world, campaign_id, &expr, &format!("{reason}（{}）", if adv > 0 { "优势" } else { "劣势" }))?;
                let roll_under = mode == "roll_under";
                // 优势：target 取大、roll_under 取小；劣势相反。
                let a_better = if roll_under { a.total <= b.total } else { a.total >= b.total };
                if (adv > 0) == a_better { (a, faces(&b)) } else { (b, faces(&a)) }
            } else {
                (a, vec![])
            };
            let natural = chosen.total;
            let dice = faces(&chosen);
            let max = n as i64 * sides as i64;
            let min = n as i64;
            let adv_txt = if adv != 0 { format!("（{}：{} / {}）", if adv > 0 { "优势" } else { "劣势" }, natural, other.iter().sum::<i64>()) } else { String::new() };
            if mode == "roll_under" {
                let target = (dc + modifier).clamp(1, max.max(1));
                let crit_s = rules.crits && natural <= (max / 20).max(1);
                let crit_f = rules.crits && natural > max - (max / 25).max(1);
                let band = rules.partial_band * (sides as i64 / 20).max(1);
                let degree = if crit_f {
                    Degree::CritFailure
                } else if crit_s && natural <= target {
                    Degree::CritSuccess
                } else if natural <= target {
                    Degree::Success
                } else if band > 0 && natural <= target + band {
                    Degree::Partial
                } else {
                    Degree::Failure
                };
                let explain = format!("{expr} 掷出 {natural}{adv_txt}，目标 ≤ {target}（难度 {dc} + 修正 {modifier}）→ {}", degree.label());
                return Ok(CheckRoll { mode: "roll_under".into(), expr, dice, other, advantage: adv, natural, modifier, total: natural, dc, target, degree, explain });
            }
            let total = natural + modifier;
            let degree = if rules.crits && natural == max && max > min {
                Degree::CritSuccess
            } else if rules.crits && natural == min && max > min {
                Degree::CritFailure
            } else if total >= dc {
                Degree::Success
            } else if rules.partial_band > 0 && total >= dc - rules.partial_band {
                Degree::Partial
            } else {
                Degree::Failure
            };
            let explain = format!("{expr} 掷出 {natural}{adv_txt} + 修正 {modifier} = {total}，难度 {dc} → {}", degree.label());
            Ok(CheckRoll { mode: "target".into(), expr, dice, other, advantage: adv, natural, modifier, total, dc, target: dc, degree, explain })
        }
    }
}

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

/// 请玩家亲手掷骰时给界面的说明：掷什么骰子、掷几次（优势 / 劣势掷两次）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct DiceAsk {
    /// target / roll_under / pool
    pub mode: String,
    /// 骰子，例如 "1d20"、"5d6"。
    pub expr: String,
    pub count: u32,
    pub sides: u32,
    /// 要掷几次：优势 / 劣势是 2，其他是 1。
    pub rolls: u32,
    pub advantage: i8,
    pub modifier: i64,
    pub dc: i64,
    /// 检定原因（含技能名）。
    pub reason: String,
}

/// 标准难度（d20 刻度：简单 8、普通 12、困难 15、极难 20）的上限；超过的视为已经是本骰子上的数值
/// （旧存档、主持人手写的百分比目标），不再换算。
pub const STANDARD_DC_MAX: i64 = 30;

/// 主骰是不是百分骰（1d100 / d%）。
pub fn is_percentile(die: &str) -> bool {
    let d = die.trim().to_ascii_lowercase();
    d.ends_with("d%") || parse_dice(&d) == (1, 100)
}

/// 实际使用的检定方式：主骰是百分骰时一律按"掷出 ≤ 目标值"判定
/// （以前剧本写了 1d100、检定方式却是 target，结果 d100 和 d20 一样"掷骰 + 修正 ≥ 难度"，几乎必成功）。
pub fn effective_mode(rules: &CheckRules) -> &str {
    if rules.mode == "target" && is_percentile(&rules.die) { "roll_under" } else { rules.mode.as_str() }
}

/// 把标准难度和修正换算到本战役的骰子上。
/// - 百分骰（roll_under）：按 d20 的成功概率换成百分比目标值，例如难度 12、修正 +2 → 55%（掷出 ≤ 55 成功）；
/// - 其他骰子（2d6、1d12……）：按均值和离散程度换算难度与修正，成功概率和 d20 接近；
/// - d20：不变。
///
/// 返回（难度或目标值，修正，换算说明）。
pub fn scale(rules: &CheckRules, dc: i64, modifier: i64) -> (i64, i64, String) {
    let (n, m) = parse_dice(&rules.die);
    match effective_mode(rules) {
        "roll_under" => {
            let max = (n as i64 * m as i64).max(2);
            if dc > STANDARD_DC_MAX {
                return ((dc + modifier).clamp(1, max), 0, String::new());
            }
            let chance = ((21 - dc + modifier) as f64 / 20.0).clamp(0.05, 0.95);
            let target = ((max as f64) * chance).round() as i64;
            (target.clamp(1, max), 0, format!("标准难度 {dc}、修正 {modifier:+} 换算成 {}%", (chance * 100.0).round() as i64))
        }
        "target" if (n, m) != (1, 20) && dc <= STANDARD_DC_MAX => {
            let mean = n as f64 * (m as f64 + 1.0) / 2.0;
            let sd = (n as f64 * ((m * m) as f64 - 1.0) / 12.0).sqrt();
            let f = sd / 5.766;
            let d = (mean + (dc as f64 - 10.5) * f).round() as i64;
            let md = (modifier as f64 * f).round() as i64;
            let d = d.clamp(n as i64 + 1, n as i64 * m as i64);
            (d, md, format!("标准难度 {dc}、修正 {modifier:+} 换算成 {} 的难度 {d}、修正 {md:+}", rules.die))
        }
        _ => (dc, modifier, String::new()),
    }
}

/// 这次检定要掷什么（不掷骰、不碰数据库）：先问玩家亲手掷，再把点数交给 [`perform_with`]。
/// `dc`、`modifier` 是标准刻度，这里换算成本战役的骰子（界面显示的就是实际要比的数）。
pub fn ask_for(rules: &CheckRules, modifier: i64, dc: i64, advantage: i8, reason: &str) -> DiceAsk {
    let adv = if rules.advantage { advantage.signum() } else { 0 };
    if rules.mode == "pool" {
        let count = (rules.pool_base + modifier + adv as i64).clamp(1, 40);
        let sides = rules.pool_sides.clamp(2, 1000);
        return DiceAsk { mode: "pool".into(), expr: format!("{count}d{sides}"), count: count as u32, sides: sides as u32, rolls: 1, advantage: adv, modifier, dc, reason: reason.to_string() };
    }
    let (count, sides) = parse_dice(&rules.die);
    let (dc, modifier, _) = scale(rules, dc, modifier);
    DiceAsk { mode: effective_mode(rules).to_string(), expr: rules.die.clone(), count, sides, rolls: if adv != 0 { 2 } else { 1 }, advantage: adv, modifier, dc, reason: reason.to_string() }
}

/// 检查玩家给的点数：组数、颗数对得上，每颗在 1..=面数 之内，否则不用（交给程序掷）。
pub fn valid_manual(ask: &DiceAsk, got: Option<Vec<Vec<u32>>>) -> Option<Vec<Vec<u32>>> {
    let got = got?;
    let ok = got.len() == ask.rolls as usize && got.iter().all(|g| g.len() == ask.count as usize && g.iter().all(|d| (1..=ask.sides).contains(d)));
    ok.then_some(got)
}

/// 按规则掷骰并判定。`advantage`：1 优势 / -1 劣势 / 0 普通（规则关闭优势时忽略）。
pub fn perform(world: &mut World, campaign_id: &str, rules: &CheckRules, modifier: i64, dc: i64, advantage: i8, reason: &str) -> Result<CheckRoll> {
    perform_with(world, campaign_id, rules, modifier, dc, advantage, reason, None)
}

/// 同 [`perform`]；`manual` 是玩家亲手掷出的点数（见 [`ask_for`]），不合法时由程序掷。
#[allow(clippy::too_many_arguments)]
pub fn perform_with(world: &mut World, campaign_id: &str, rules: &CheckRules, modifier: i64, dc: i64, advantage: i8, reason: &str, manual: Option<Vec<Vec<u32>>>) -> Result<CheckRoll> {
    let hand = valid_manual(&ask_for(rules, modifier, dc, advantage, reason), manual);
    let adv = if rules.advantage { advantage.signum() } else { 0 };
    match rules.mode.as_str() {
        "pool" => {
            let count = (rules.pool_base + modifier + adv as i64).clamp(1, 40);
            let expr = format!("{count}d{}", rules.pool_sides);
            let r = campaign::roll_with(world, campaign_id, &expr, &format!("{reason}（骰池）"), hand.as_ref().map(|h| h[0].as_slice()))?;
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
            let explain = format!("{}掷 {count} 个 d{}（≥{} 为成功）：{:?}，成功 {hits} 个，需要 {need} → {}", if hand.is_some() { "亲手" } else { "" }, rules.pool_sides, rules.pool_success_on, dice, degree.label());
            campaign::annotate_last_roll("check", vec![], hits, Some(need), degree.label());
            Ok(CheckRoll { mode: "pool".into(), expr, dice, other: vec![], advantage: adv, natural: hits, modifier, total: hits, dc: need, target: need, degree, explain })
        }
        _ => {
            let mode = effective_mode(rules);
            let (scaled_dc, scaled_mod, scale_note) = scale(rules, dc, modifier);
            let expr = rules.die.clone();
            let (n, sides) = parse_dice(&expr);
            let a = campaign::roll_with(world, campaign_id, &expr, reason, hand.as_ref().map(|h| h[0].as_slice()))?;
            let (chosen, other) = if adv != 0 {
                let b = campaign::roll_with(world, campaign_id, &expr, &format!("{reason}（{}）", if adv > 0 { "优势" } else { "劣势" }), hand.as_ref().map(|h| h[1].as_slice()))?;
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
            let by_hand = if hand.is_some() { "（亲手掷）" } else { "" };
            let adv_txt = if adv != 0 { format!("（{}：{} / {}）", if adv > 0 { "优势" } else { "劣势" }, natural, other.iter().sum::<i64>()) } else { String::new() };
            if mode == "roll_under" {
                let target = scaled_dc;
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
                campaign::annotate_last_roll("check", vec![], natural, Some(target), degree.label());
                let explain = if scale_note.is_empty() {
                    format!("{expr}{by_hand} 掷出 {natural}{adv_txt}，目标 ≤ {target}（难度 {dc} + 修正 {modifier}）→ {}", degree.label())
                } else {
                    format!("{expr}{by_hand} 掷出 {natural}{adv_txt}，目标 ≤ {target}（{scale_note}）→ {}", degree.label())
                };
                return Ok(CheckRoll { mode: "roll_under".into(), expr, dice, other, advantage: adv, natural, modifier, total: natural, dc, target, degree, explain });
            }
            // 非 d20 的骰子（2d6 等）：难度和修正换算到这种骰子上。
            let (dc, modifier) = (scaled_dc, scaled_mod);
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
            campaign::annotate_last_roll("check", if modifier != 0 { vec![("修正".into(), modifier)] } else { vec![] }, total, Some(dc), degree.label());
            let explain = format!("{expr}{by_hand} 掷出 {natural}{adv_txt} + 修正 {modifier} = {total}，难度 {dc}{} → {}", if scale_note.is_empty() { String::new() } else { format!("（{scale_note}）") }, degree.label());
            Ok(CheckRoll { mode: "target".into(), expr, dice, other, advantage: adv, natural, modifier, total, dc, target: dc, degree, explain })
        }
    }
}

#[cfg(test)]
mod scale_tests {
    use super::*;

    fn rules(mode: &str, die: &str) -> CheckRules {
        CheckRules { mode: mode.into(), die: die.into(), ..Default::default() }
    }

    #[test]
    fn d20_is_unchanged() {
        assert_eq!(scale(&rules("target", "1d20"), 12, 2).0, 12);
        assert_eq!(effective_mode(&rules("target", "1d20")), "target");
    }

    #[test]
    fn d100_becomes_a_percentage() {
        // 写成 target 的百分骰也按 roll_under。
        assert_eq!(effective_mode(&rules("target", "1d100")), "roll_under");
        let (t, m, note) = scale(&rules("roll_under", "1d100"), 12, 2);
        assert_eq!((t, m), (55, 0), "难度 12、修正 +2：d20 成功率 55% → 掷出 ≤ 55");
        assert!(note.contains("55%"));
        // 难的检定百分比低，简单的高；概率夹在 5%–95%。
        assert!(scale(&rules("roll_under", "1d100"), 20, 0).0 < scale(&rules("roll_under", "1d100"), 8, 0).0);
        assert_eq!(scale(&rules("roll_under", "1d100"), 30, 0).0, 5);
        // 已经是百分比的目标（旧存档）不再换算。
        assert_eq!(scale(&rules("roll_under", "1d100"), 60, 5).0, 65);
    }

    #[test]
    fn other_dice_keep_similar_odds() {
        // 2d6：标准难度 12（d20 成功率 45%）→ 8（2d6 掷出 ≥ 8 的概率约 42%）。
        let (d, _, _) = scale(&rules("target", "2d6"), 12, 0);
        assert_eq!(d, 8);
        let (_, md, _) = scale(&rules("target", "2d6"), 12, 5);
        assert_eq!(md, 2, "修正也按比例缩小");
    }

    #[test]
    fn ask_shows_the_real_target() {
        let a = ask_for(&rules("target", "1d100"), 2, 12, 0, "测试");
        assert_eq!((a.mode.as_str(), a.dc, a.modifier), ("roll_under", 55, 0));
    }
}

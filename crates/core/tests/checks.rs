//! 检定：三种方式（target / roll_under / pool）的成功等级、优势劣势、大成功大失败，全部由程序掷骰判定。

mod common;

use chronicle_core::checks::{Degree, perform};
use chronicle_core::profile::CheckRules;
use chronicle_core::*;
use common::*;

fn camp(e: &mut Env) -> String {
    campaign::create_campaign(&mut e.world, None, "检定测试", Some(42)).unwrap().id
}

#[test]
fn target_mode_degrees_follow_the_numbers() {
    let mut e = env();
    let c = camp(&mut e);
    let rules = CheckRules::default(); // 1d20，差 2 以内部分成功，有大成功 / 大失败
    let mut seen = std::collections::HashSet::new();
    for i in 0..300 {
        let r = perform(&mut e.world, &c, &rules, 2, 12, 0, &format!("测试 {i}")).unwrap();
        assert_eq!(r.dice.len(), 1);
        assert_eq!(r.natural, r.dice[0]);
        assert_eq!(r.total, r.natural + 2);
        let expect = if r.natural == 20 {
            Degree::CritSuccess
        } else if r.natural == 1 {
            Degree::CritFailure
        } else if r.total >= 12 {
            Degree::Success
        } else if r.total >= 10 {
            Degree::Partial
        } else {
            Degree::Failure
        };
        assert_eq!(r.degree, expect, "{}", r.explain);
        assert!(r.explain.contains(r.degree.label()));
        seen.insert(r.degree);
    }
    assert_eq!(seen.len(), 5, "300 次里五种等级都出现过");
    // 每次掷骰都写进了事件日志（可重放）。
    let rolls: i64 = e.world.conn().query_row("SELECT COUNT(*) FROM event_log WHERE campaign_id = ?1 AND kind = 'roll'", [&c], |r| r.get(0)).unwrap();
    assert_eq!(rolls, 300);
}

#[test]
fn advantage_keeps_the_better_roll_and_disadvantage_the_worse() {
    let mut e = env();
    let c = camp(&mut e);
    let rules = CheckRules::default();
    for i in 0..100 {
        let a = perform(&mut e.world, &c, &rules, 0, 10, 1, &format!("优势 {i}")).unwrap();
        assert_eq!(a.other.len(), 1);
        assert!(a.natural >= a.other[0], "优势取大：{}", a.explain);
        let d = perform(&mut e.world, &c, &rules, 0, 10, -1, &format!("劣势 {i}")).unwrap();
        assert!(d.natural <= d.other[0], "劣势取小：{}", d.explain);
    }
    let off = CheckRules { advantage: false, ..Default::default() };
    let n = perform(&mut e.world, &c, &off, 0, 10, 1, "关闭优势").unwrap();
    assert!(n.other.is_empty() && n.advantage == 0, "规则关闭优势时忽略请求");
}

#[test]
fn roll_under_and_pool_modes() {
    let mut e = env();
    let c = camp(&mut e);
    let d100 = CheckRules { mode: "roll_under".into(), die: "1d100".into(), ..Default::default() };
    for i in 0..200 {
        let r = perform(&mut e.world, &c, &d100, 10, 40, 0, &format!("百分骰 {i}")).unwrap();
        assert_eq!(r.target, 50);
        if r.natural > 96 {
            assert_eq!(r.degree, Degree::CritFailure);
        } else if r.natural <= 5 {
            assert_eq!(r.degree, Degree::CritSuccess);
        } else if r.natural <= 50 {
            assert_eq!(r.degree, Degree::Success);
        } else if r.natural <= 60 {
            assert_eq!(r.degree, Degree::Partial, "{}", r.explain);
        } else {
            assert_eq!(r.degree, Degree::Failure);
        }
    }
    let pool = CheckRules { mode: "pool".into(), pool_base: 2, pool_sides: 6, pool_success_on: 5, ..Default::default() };
    for i in 0..200 {
        let r = perform(&mut e.world, &c, &pool, 2, 2, 1, &format!("骰池 {i}")).unwrap();
        assert_eq!(r.dice.len(), 5, "基础 2 + 修正 2 + 优势 1");
        let hits = r.dice.iter().filter(|d| **d >= 5).count() as i64;
        assert_eq!(r.natural, hits);
        match r.degree {
            Degree::CritFailure => assert!(hits == 0 && r.dice.contains(&1)),
            Degree::CritSuccess => assert!(hits == 5),
            Degree::Success => assert!(hits >= 2),
            Degree::Partial => assert_eq!(hits, 1),
            Degree::Failure => assert_eq!(hits, 0),
        }
        assert!(r.degree.ok() == (hits >= 1 && !(hits == 0)) || r.degree == Degree::CritFailure);
    }
}

#[test]
fn multi_die_crits_use_max_and_min_sum() {
    let mut e = env();
    let c = camp(&mut e);
    let r2d6 = CheckRules { die: "2d6".into(), partial_band: 0, ..Default::default() };
    for i in 0..300 {
        let r = perform(&mut e.world, &c, &r2d6, 0, 8, 0, &format!("2d6 {i}")).unwrap();
        assert_eq!(r.dice.len(), 2);
        if r.natural == 12 {
            assert_eq!(r.degree, Degree::CritSuccess);
        } else if r.natural == 2 {
            assert_eq!(r.degree, Degree::CritFailure);
        } else {
            assert_eq!(r.degree, if r.total >= r.dc { Degree::Success } else { Degree::Failure }, "没有部分成功（难度按 2d6 换算后比较）");
        }
    }
}

#[test]
fn manual_dice_are_used_when_valid_and_ignored_when_not() {
    use chronicle_core::checks::{ask_for, perform_with};
    let mut e = env();
    let c = camp(&mut e);
    let rules = CheckRules::default();
    // 普通检定：问 1 次 1d20。
    let ask = ask_for(&rules, 3, 15, 0, "聆听 检定：偷听");
    assert_eq!((ask.count, ask.sides, ask.rolls), (1, 20, 1));
    let r = perform_with(&mut e.world, &c, &rules, 3, 15, 0, "聆听 检定：偷听", Some(vec![vec![14]])).unwrap();
    assert_eq!((r.natural, r.total, r.degree), (14, 17, Degree::Success));
    assert!(r.explain.contains("亲手"), "{}", r.explain);
    // 优势：掷两次取高。
    assert_eq!(ask_for(&rules, 0, 10, 1, "x").rolls, 2);
    let r = perform_with(&mut e.world, &c, &rules, 0, 10, 1, "潜行 检定", Some(vec![vec![4], vec![17]])).unwrap();
    assert_eq!((r.natural, r.other.clone()), (17, vec![4]));
    // 不合法的点数（超出骰面、颗数不对）：交给程序掷，不报错。
    let r = perform_with(&mut e.world, &c, &rules, 0, 10, 0, "运动 检定", Some(vec![vec![25]])).unwrap();
    assert!((1..=20).contains(&r.natural) && !r.explain.contains("亲手"));
    let r = perform_with(&mut e.world, &c, &rules, 0, 10, 1, "运动 检定", Some(vec![vec![5]])).unwrap();
    assert!(!r.explain.contains("亲手"));
    // 骰池：颗数随修正变化。
    let pool = CheckRules { mode: "pool".into(), ..CheckRules::default() };
    let ask = ask_for(&pool, 1, 2, 0, "射击 检定");
    assert_eq!(ask.count as i64, (pool.pool_base + 1).clamp(1, 40));
    let faces: Vec<u32> = (0..ask.count).map(|_| ask.sides).collect();
    let r = perform_with(&mut e.world, &c, &pool, 1, 2, 0, "射击 检定", Some(vec![faces])).unwrap();
    assert_eq!(r.natural as u32, ask.count, "全部最大点数都算成功");
    // 掷骰日志里标记了亲手掷。
    let ev = campaign::recent_events(e.world.conn(), &c, &["roll"], 50).unwrap();
    assert!(ev.iter().any(|x| x.payload["manual"] == true));
}

//! 游戏状态机与掷骰记录（改革第 1 阶段）。

mod common;

use chronicle_core::combat::{self, CombatAction, EnemySpec};
use chronicle_core::gamestate::{self, CombatPhase, Phase, StepStatus};
use chronicle_core::rpg::{self, PARTY};
use chronicle_core::*;
use common::*;

fn camp(e: &mut Env) -> String {
    campaign::create_campaign(&mut e.world, None, "状态机", Some(7)).unwrap().id
}

fn wolf() -> Vec<EnemySpec> {
    vec![EnemySpec { name: "灰狼".into(), npc: None, level: 1, count: 1, loot: vec![], boss: false }]
}

#[test]
fn new_campaign_starts_in_setup_and_only_legal_transitions_pass() {
    let mut e = env();
    let c = camp(&mut e);
    let s = gamestate::get(e.world.conn(), &c).unwrap();
    assert_eq!(s.phase, Phase::Setup);
    assert!(s.derived, "没有记录时按事实推导");
    // 非法：设置阶段不能直接进入战斗或开场。
    for bad in [Phase::Combat, Phase::Opening, Phase::Aftermath, Phase::AwaitingDice] {
        let err = gamestate::transition(&mut e.world, &c, bad, "测试").unwrap_err().to_string();
        assert!(err.contains("不能从"), "{err}");
    }
    assert_eq!(gamestate::get(e.world.conn(), &c).unwrap().phase, Phase::Setup, "非法迁移不改状态");
    // 合法：初始化（7 步）→ 开场 → 探索 ⇄ 等待掷骰。
    let s = gamestate::transition(&mut e.world, &c, Phase::Initializing, "开始初始化").unwrap();
    assert_eq!(s.init.len(), 7);
    assert!(s.init.iter().all(|x| x.status == StepStatus::Pending));
    for (k, _) in gamestate::INIT_STEPS {
        gamestate::set_step(&mut e.world, &c, k, StepStatus::Done, "完成", 5).unwrap();
    }
    assert!(gamestate::get(e.world.conn(), &c).unwrap().init_done());
    assert!(gamestate::set_step(&mut e.world, &c, "不存在", StepStatus::Done, "", 0).is_err());
    gamestate::transition(&mut e.world, &c, Phase::Opening, "初始化完成").unwrap();
    assert!(gamestate::set_step(&mut e.world, &c, "script", StepStatus::Done, "", 0).is_err(), "不在初始化阶段不能改步骤");
    gamestate::transition(&mut e.world, &c, Phase::Exploring, "开场讲完").unwrap();
    gamestate::transition(&mut e.world, &c, Phase::AwaitingDice, "等掷骰").unwrap();
    let s = gamestate::transition(&mut e.world, &c, Phase::Exploring, "掷完").unwrap();
    assert_eq!(s.phase, Phase::Exploring);
    assert!(!s.derived);
    assert!(gamestate::allowed(Phase::Combat, Phase::Ended) && !gamestate::allowed(Phase::Ended, Phase::Exploring));
}

#[test]
fn combat_drives_the_state_and_survives_reload() {
    let mut e = env();
    let c = camp(&mut e);
    campaign::advance_turn(e.world.conn(), &c).unwrap();
    let s = gamestate::sync(&mut e.world, &c).unwrap();
    assert_eq!(s.phase, Phase::Exploring, "旧战役第一次同步：回合 > 0 → 探索");
    let rep = combat::start(&mut e.world, &c, &wolf(), &[], "林中遇狼").unwrap();
    assert!(rep.dice.iter().any(|d| d.category == "initiative"), "先攻掷骰有记录：{:?}", rep.dice);
    let s = gamestate::sync(&mut e.world, &c).unwrap();
    assert_eq!((s.phase, s.combat), (Phase::Combat, Some(CombatPhase::Intro)), "开战先播转场");
    // 重新读取（模拟退出再打开）：还在战斗转场。
    let s = gamestate::get(e.world.conn(), &c).unwrap();
    assert_eq!(s.combat, Some(CombatPhase::Intro));
    let s = gamestate::combat_ready(&mut e.world, &c).unwrap();
    let fight = combat::active(e.world.conn(), &c).unwrap();
    assert_eq!(s.combat, Some(if fight.players_turn() { CombatPhase::PlayerTurn } else { CombatPhase::NpcTurn }));
    // 打到结束：每次攻击都有攻击检定记录（含目标防御和结果），命中有伤害骰。
    let mut saw_attack = false;
    for _ in 0..60 {
        let Some(cb) = combat::active(e.world.conn(), &c) else { break };
        let target = cb.units.iter().find(|u| u.side == "enemy" && u.out.is_empty()).map(|u| u.id.clone()).unwrap_or_default();
        let rep = combat::act(&mut e.world, &c, &CombatAction { kind: "attack".into(), target, ..Default::default() })
            .or_else(|_| combat::act(&mut e.world, &c, &CombatAction { kind: "end_turn".into(), ..Default::default() }))
            .unwrap();
        for d in rep.dice.iter().filter(|d| d.category == "attack") {
            saw_attack = true;
            assert_eq!(d.dice.len(), 1);
            assert!(d.target.is_some() && !d.outcome.is_empty(), "{d:?}");
            assert_eq!(d.total, d.natural + d.parts.iter().map(|p| p.1).sum::<i64>(), "总值 = 骰面 + 加值明细：{d:?}");
        }
        let s = gamestate::sync(&mut e.world, &c).unwrap();
        if combat::active(e.world.conn(), &c).is_some() {
            assert_eq!(s.phase, Phase::Combat);
        }
    }
    assert!(saw_attack);
    assert!(combat::active(e.world.conn(), &c).is_none(), "60 步内应当分出胜负");
    let s = gamestate::get(e.world.conn(), &c).unwrap();
    assert_eq!(s.phase, Phase::Aftermath);
    let s = gamestate::transition(&mut e.world, &c, Phase::Exploring, "看完结算").unwrap();
    assert_eq!(s.phase, Phase::Exploring);
}

#[test]
fn state_follows_the_timeline() {
    let mut e = env();
    let c = camp(&mut e);
    gamestate::transition(&mut e.world, &c, Phase::Exploring, "直接开始").unwrap();
    timeline::checkpoint(e.world.conn(), &c, "turn_end").unwrap();
    let cp = timeline::checkpoints(e.world.conn(), &c).unwrap().last().unwrap().id;
    combat::start(&mut e.world, &c, &wolf(), &[], "遇狼").unwrap();
    gamestate::sync(&mut e.world, &c).unwrap();
    assert_eq!(gamestate::get(e.world.conn(), &c).unwrap().phase, Phase::Combat);
    timeline::checkpoint(e.world.conn(), &c, "turn_end").unwrap();
    timeline::end_campaign(&mut e.world, &c, "").unwrap();
    assert_eq!(gamestate::get(e.world.conn(), &c).unwrap().phase, Phase::Ended);
    assert!(gamestate::sync(&mut e.world, &c).is_ok(), "结束的战役同步不报错");
    let b = timeline::fork(&mut e.world, &c, cp, "", false).unwrap();
    assert_eq!(gamestate::get(e.world.conn(), &b.id).unwrap().phase, Phase::Exploring, "回溯到开战之前");
    assert!(timeline::verify(e.world.conn(), &c).unwrap().ok);
}

#[test]
fn check_parts_add_up_and_checks_are_recorded() {
    let mut e = env();
    let c = camp(&mut e);
    let parts = rpg::check_parts(e.world.conn(), &c, PARTY, "察觉");
    assert!(parts.iter().any(|p| p.0.starts_with("属性")));
    assert_eq!(parts.iter().map(|p| p.1).sum::<i64>(), rpg::check_modifier(e.world.conn(), &c, PARTY, "察觉"));
    let rules = rpg::rules(e.world.conn(), &c);
    let (r, dice) = campaign::capture_rolls(|| checks::perform(&mut e.world, &c, &rules.check, 3, 12, 0, "察觉 检定：看看四周").unwrap());
    assert_eq!(dice.len(), 1);
    let d = &dice[0];
    assert_eq!((d.category.as_str(), d.total, d.target, d.natural), ("check", r.total, Some(12), r.natural));
    assert_eq!(d.outcome, r.degree.label());
    // 不在收集中时不收集（不会越积越多）。
    checks::perform(&mut e.world, &c, &rules.check, 0, 10, 0, "x").unwrap();
    let (_, none) = campaign::capture_rolls(|| ());
    assert!(none.is_empty());
}

//! 战斗引擎扩展（改革第 5 阶段）：速度决定行动顺序、闪避、技能、场景交互。全部由程序结算，掷骰有记录。

mod common;

use chronicle_core::combat::{self, Combat, CombatAction, EnemySpec};
use chronicle_core::rules::Effect;
use chronicle_core::*;
use common::*;

fn camp(e: &mut Env, seed: i64) -> String {
    campaign::create_campaign(&mut e.world, None, "战斗扩展", Some(seed)).unwrap().id
}

fn foes(n: i64, level: i64) -> Vec<EnemySpec> {
    vec![EnemySpec { name: "强盗".into(), npc: None, level, count: n, loot: vec![], boss: false }]
}

/// 测试用：直接改战斗记录（摆位置、补行动点），模拟玩家已经走到某处。
fn edit(e: &mut Env, c: &str, f: impl FnOnce(&mut Combat)) {
    let mut cb = combat::active(e.world.conn(), c).unwrap();
    f(&mut cb);
    campaign::apply_effects(&mut e.world, c, &[Effect::UpsertRecord { kind: "combat".into(), id: cb.id.clone(), data: serde_json::to_value(&cb).unwrap() }], "测试摆位").unwrap();
}

/// 一直结束回合，直到轮到玩家（或战斗结束）。
fn until_player(e: &mut Env, c: &str) -> Option<Combat> {
    for _ in 0..10 {
        let cb = combat::active(e.world.conn(), c)?;
        if cb.players_turn() {
            return Some(cb);
        }
    }
    combat::active(e.world.conn(), c)
}

#[test]
fn initiative_uses_speed_and_order_is_sorted() {
    let mut e = env();
    let c = camp(&mut e, 5);
    let rep = combat::start(&mut e.world, &c, &foes(3, 2), &[], "拦路").unwrap();
    let cb = rep.combat;
    let inits: Vec<(i64, i64)> = cb.order.iter().map(|id| cb.units.iter().find(|u| &u.id == id).map(|u| (u.init, u.speed)).unwrap()).collect();
    assert!(inits.windows(2).all(|w| w[0] >= w[1]), "先攻从高到低（同分速度高的先）：{inits:?}");
    let ini: Vec<_> = rep.dice.iter().filter(|d| d.category == "initiative").collect();
    assert_eq!(ini.len(), cb.units.len(), "每个参战者一次先攻掷骰");
    for d in ini {
        assert_eq!(d.parts[0].0, "速度");
        assert_eq!(d.total, d.natural + d.parts[0].1);
    }
}

#[test]
fn props_are_reproducible_and_listed_in_skills() {
    let a = {
        let mut e = env();
        let c = camp(&mut e, 77);
        combat::start(&mut e.world, &c, &foes(1, 1), &[], "x").unwrap().combat.props
    };
    let b = {
        let mut e = env();
        let c = camp(&mut e, 77);
        combat::start(&mut e.world, &c, &foes(1, 1), &[], "x").unwrap().combat.props
    };
    assert!(!a.is_empty() && a.len() <= 3);
    assert_eq!(a, b, "同一种子，物件相同");
    let r = rpg::rules(env().world.conn(), "none");
    let names: Vec<String> = combat::skills(&r).into_iter().map(|s| s.name).collect();
    assert!(names.contains(&"瞄准".to_string()) && names.contains(&"包扎".to_string()));
}

#[test]
fn dodge_contests_the_attack_and_is_recorded() {
    let mut saw_dodge = false;
    for seed in 1..40 {
        let mut e = env();
        let c = camp(&mut e, seed);
        combat::start(&mut e.world, &c, &foes(2, 3), &[], "围攻").unwrap();
        if until_player(&mut e, &c).is_none() {
            continue;
        }
        let rep = combat::act(&mut e.world, &c, &CombatAction { kind: "dodge".into(), ..Default::default() }).unwrap();
        assert!(rep.lines.iter().any(|l| l.contains("准备闪避")));
        // 敌人的攻击若命中，就有闪避检定记录：1d20 + 速度 对抗攻击总值。
        for d in rep.dice.iter().filter(|d| d.category == "dodge") {
            saw_dodge = true;
            assert_eq!(d.parts[0].0, "速度");
            assert!(d.target.is_some() && (d.outcome == "闪开" || d.outcome == "没闪开"));
            assert_eq!(d.outcome == "闪开", d.total >= d.target.unwrap());
        }
        if saw_dodge {
            break;
        }
    }
    assert!(saw_dodge, "40 场里至少有一次敌人命中后触发闪避检定");
}

#[test]
fn skills_aim_and_bandage() {
    let mut e = env();
    let c = camp(&mut e, 3);
    combat::start(&mut e.world, &c, &foes(1, 1), &[], "练习").unwrap();
    until_player(&mut e, &c).expect("轮到玩家");
    edit(&mut e, &c, |cb| {
        let p = cb.units.iter_mut().find(|u| u.side == "party").unwrap();
        p.ap = 5;
        p.hp = 3;
    });
    let rep = combat::act(&mut e.world, &c, &CombatAction { kind: "skill".into(), custom: "瞄准".into(), ..Default::default() }).unwrap();
    let me = rep.combat.units.iter().find(|u| u.side == "party").unwrap();
    assert!(me.statuses.iter().any(|s| s.name == "瞄准" && s.modifier == 3), "{:?}", rep.lines);
    let pid = me.id.clone();
    let rep = combat::act(&mut e.world, &c, &CombatAction { kind: "skill".into(), custom: "包扎".into(), target: pid, ..Default::default() }).unwrap();
    assert!(rep.lines.iter().any(|l| l.contains("包扎") && l.contains("恢复")), "{:?}", rep.lines);
    assert!(rep.dice.iter().any(|d| d.purpose.contains("包扎")));
    assert!(combat::act(&mut e.world, &c, &CombatAction { kind: "skill".into(), custom: "龟派气功".into(), ..Default::default() }).is_err());
}

#[test]
fn interact_with_props_needs_to_be_adjacent_and_follows_rules() {
    let mut done = std::collections::HashSet::new();
    for seed in 1..80 {
        let mut e = env();
        let c = camp(&mut e, seed);
        let cb = combat::start(&mut e.world, &c, &foes(1, 1), &[], "物件").unwrap().combat;
        if until_player(&mut e, &c).is_none() {
            continue;
        }
        for p in cb.props.clone() {
            if done.contains(&p.kind) {
                continue;
            }
            // 太远：拒绝。
            edit(&mut e, &c, |cb| {
                let me = cb.units.iter_mut().find(|u| u.side == "party").unwrap();
                me.ap = 5;
                (me.x, me.y) = (if p.x > 5 { 0 } else { cb.width - 1 }, p.y);
            });
            if let Some(cur) = combat::active(e.world.conn(), &c)
                && let Some(me) = cur.units.iter().find(|u| u.side == "party")
                && (me.x - p.x).abs().max((me.y - p.y).abs()) > 1
            {
                let err = combat::act(&mut e.world, &c, &CombatAction { kind: "interact".into(), target: p.id.clone(), ..Default::default() }).unwrap_err().to_string();
                assert!(err.contains("太远"), "{err}");
            }
            // 走到旁边（敌人也摆到物件旁边，好让火焰 / 重物有目标）。
            edit(&mut e, &c, |cb| {
                let free = |cb: &Combat, x: i64, y: i64| x >= 0 && y >= 0 && x < cb.width && y < cb.height && !cb.obstacles.contains(&(x, y)) && !cb.props.iter().any(|q| (q.x, q.y) == (x, y)) && !cb.units.iter().any(|u| (u.x, u.y) == (x, y));
                let spots: Vec<(i64, i64)> = [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, 1), (-1, 1), (1, -1)].iter().map(|(dx, dy)| (p.x + dx, p.y + dy)).collect();
                let mut it = spots.into_iter();
                if let Some((x, y)) = it.by_ref().find(|&(x, y)| free(cb, x, y)) {
                    let me = cb.units.iter_mut().find(|u| u.side == "party").unwrap();
                    (me.x, me.y) = (x, y);
                }
                if let Some((x, y)) = it.find(|&(x, y)| free(cb, x, y)) {
                    let foe = cb.units.iter_mut().find(|u| u.side == "enemy").unwrap();
                    (foe.x, foe.y, foe.speed) = (x, y, -10); // 速度很低：重物一定砸中，便于检查
                }
            });
            let rep = combat::act(&mut e.world, &c, &CombatAction { kind: "interact".into(), target: p.id.clone(), ..Default::default() }).unwrap();
            let after = combat::active(e.world.conn(), &c).unwrap_or_else(|| rep.combat.clone());
            match p.kind.as_str() {
                "cover" => assert!(after.units.iter().find(|u| u.side == "party").unwrap().statuses.iter().any(|s| s.name == "掩护" && s.ac == 2) || rep.lines.iter().any(|l| l.contains("躲到")), "{:?}", rep.lines),
                "fire" => {
                    assert!(rep.lines.iter().any(|l| l.contains("火焰扑向")), "{:?}", rep.lines);
                    assert!(rep.dice.iter().any(|d| d.category == "damage"));
                    assert!(after.props.iter().find(|q| q.id == p.id).unwrap().used, "油灯只能用一次");
                }
                "drop" => {
                    assert!(rep.lines.iter().any(|l| l.contains("没躲开") || l.contains("躲开了")), "{:?}", rep.lines);
                    assert!(rep.dice.iter().any(|d| d.category == "check" && d.target == Some(12)));
                }
                _ => unreachable!(),
            }
            done.insert(p.kind.clone());
            break;
        }
        if done.len() == 3 {
            break;
        }
    }
    assert_eq!(done.len(), 3, "三种物件都测到：{done:?}");
}

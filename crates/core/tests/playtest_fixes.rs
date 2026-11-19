//! 2026-11-15 试玩反馈的修正：初始装备、弹药消耗、检定技能贴合行动、停滞时推一把。

mod common;

use chronicle_core::combat::{self, CombatAction, EnemySpec};
use chronicle_core::pc::{self, PlayerCharacter, StartItem};
use chronicle_core::rpg::{self, PARTY};
use chronicle_core::*;
use common::*;

fn hunter_kit() -> Vec<StartItem> {
    vec![
        StartItem { name: "猎枪".into(), qty: 1, slot: "weapon".into(), damage: "1d10".into(), range: 8, ammo: "猎枪铅弹".into(), description: "枪托有修补过的裂痕".into(), ..Default::default() },
        StartItem { name: "猎枪铅弹".into(), qty: 3, ..Default::default() },
        StartItem { name: "皮外套".into(), qty: 1, slot: "armor".into(), armor: 1, ..Default::default() },
        StartItem { name: "".into(), qty: 1, ..Default::default() },
        StartItem { name: "怪刀".into(), qty: 1, slot: "weapon".into(), damage: "不是骰子".into(), ..Default::default() },
    ]
}

#[test]
fn start_kit_goes_into_the_bag_once_and_equips_weapon_and_armor() {
    let mut e = env();
    let c = campaign::create_campaign(&mut e.world, None, "猎人", Some(7)).unwrap().id;
    let hunter = PlayerCharacter { name: "老秦".into(), identity: "猎户".into(), abilities: vec!["猎枪射击".into()], equipment: hunter_kit(), ..Default::default() };
    pc::set(&mut e.world, &c, &hunter).unwrap();
    let bag = rpg::inventory(e.world.conn(), &c, PARTY).unwrap();
    let names: Vec<_> = bag.iter().map(|i| (i.item.title.clone(), i.qty)).collect();
    assert!(names.contains(&("猎枪".into(), 1)) && names.contains(&("猎枪铅弹".into(), 3)) && names.contains(&("皮外套".into(), 1)), "{names:?}");
    assert!(!names.iter().any(|(n, _)| n.is_empty()), "没有名字的物品不发放：{names:?}");
    let knife = bag.iter().find(|i| i.item.title == "怪刀").expect("伤害写错的武器按 1d6 发放");
    assert_eq!(knife.item.damage, "1d6");
    let gun = bag.iter().find(|i| i.item.title == "猎枪").unwrap();
    assert!(gun.equipped && gun.item.origin == "campaign");
    assert_eq!(gun.ammo_left, Some(3), "背包里显示剩余弹药");
    assert!(bag.iter().find(|i| i.item.title == "皮外套").unwrap().equipped);
    // 再选一次角色：背包已经有东西，不重复发放。
    pc::set(&mut e.world, &c, &hunter).unwrap();
    assert_eq!(rpg::inventory(e.world.conn(), &c, PARTY).unwrap().len(), bag.len());
}

#[test]
fn each_shot_uses_one_round_and_an_empty_gun_does_not_fire() {
    let mut e = env();
    let c = campaign::create_campaign(&mut e.world, None, "猎人", Some(11)).unwrap().id;
    rpg::grant_start_kit(&mut e.world, &c, &hunter_kit()).unwrap();
    combat::start(&mut e.world, &c, &[EnemySpec { name: "铁皮熊".into(), npc: None, level: 20, count: 1, loot: vec![], boss: false }], &[], "林中遇熊").unwrap();
    let mut lines: Vec<String> = vec![];
    for _ in 0..40 {
        let Some(cb) = combat::active(e.world.conn(), &c) else { break };
        let target = cb.units.iter().find(|u| u.side == "enemy" && u.hp > 0).map(|u| u.id.clone()).unwrap_or_default();
        let rep = combat::act(&mut e.world, &c, &CombatAction { kind: "attack".into(), target, ..Default::default() })
            .or_else(|_| combat::act(&mut e.world, &c, &CombatAction { kind: "end_turn".into(), ..Default::default() }));
        match rep {
            Ok(r) => lines.extend(r.lines),
            Err(_) => break,
        }
        if lines.iter().any(|l| l.contains("打不响")) {
            break;
        }
    }
    let used = lines.iter().filter(|l| l.contains("消耗 1 发猎枪铅弹")).count();
    assert_eq!(used, 3, "三发铅弹各扣一次：{lines:#?}");
    assert!(lines.iter().any(|l| l.contains("没有猎枪铅弹了，打不响")), "{lines:#?}");
    let left = rpg::inventory(e.world.conn(), &c, PARTY).unwrap().into_iter().find(|i| i.item.title == "猎枪").and_then(|i| i.ammo_left);
    assert_eq!(left, Some(0));
}

#[test]
fn action_words_map_to_fitting_skills() {
    assert_eq!(rpg::action_skill("我躲在门后窃听他们的谈话"), Some(("聆听", "insight")));
    assert_eq!(rpg::action_skill("悄悄溜进仓库"), Some(("潜行", "agility")));
    assert_eq!(rpg::action_skill("劝说守卫放我们过去"), Some(("说服", "presence")));
    assert_eq!(rpg::action_skill("向他打招呼"), None);
}

#[test]
fn stall_counter_grows_without_progress() {
    let mut e = env();
    let c = campaign::create_campaign(&mut e.world, None, "停滞", Some(3)).unwrap().id;
    let (since, limit) = director::stall_status(e.world.conn(), &c).unwrap();
    assert_eq!(since, 0);
    assert!((3..=10).contains(&limit));
}

#[test]
fn messy_model_kit_is_normalized() {
    // 实测模型输出：栏位写"主手""腰""背""弹药"，伤害写"无""d0"，射程写 150。
    let raw = vec![
        StartItem { name: "老式猎枪".into(), qty: 1, slot: "主手".into(), damage: "2d6".into(), range: 150, ammo: "猎枪铅弹".into(), ..Default::default() },
        StartItem { name: "猎枪铅弹".into(), qty: 12, slot: "弹药".into(), damage: "2d6".into(), range: 150, ammo: "猎枪铅弹".into(), ..Default::default() },
        StartItem { name: "药箱".into(), qty: 1, slot: "背".into(), damage: "无".into(), armor: 1, ammo: "无".into(), ..Default::default() },
        StartItem { name: "木刀".into(), qty: 1, slot: "腰".into(), damage: "1d4".into(), ..Default::default() },
        StartItem { name: "防水油布".into(), qty: 1, slot: "物品".into(), damage: "d0".into(), ..Default::default() },
        StartItem { name: "皮背心".into(), qty: 1, slot: "身体".into(), armor: 1, ..Default::default() },
    ];
    let k = pc::normalize_kit(&raw);
    let by = |n: &str| k.iter().find(|i| i.name == n).unwrap().clone();
    assert_eq!((by("老式猎枪").slot.as_str(), by("老式猎枪").range, by("老式猎枪").ammo.as_str()), ("weapon", 12, "猎枪铅弹"));
    let ammo = by("猎枪铅弹");
    assert!(ammo.slot.is_empty() && ammo.damage.is_empty() && ammo.ammo.is_empty() && ammo.qty == 12, "{ammo:?}");
    let bag = by("药箱");
    assert!(bag.slot.is_empty() && bag.armor == 0 && bag.damage.is_empty() && bag.ammo.is_empty(), "{bag:?}");
    assert_eq!((by("木刀").slot.as_str(), by("木刀").range), ("weapon", 1));
    assert!(by("防水油布").slot.is_empty() && by("防水油布").damage.is_empty());
    assert_eq!((by("皮背心").slot.as_str(), by("皮背心").armor), ("armor", 1));
}

#[test]
fn missed_shot_written_as_miss_is_not_a_mismatch() {
    let s = vec!["老秦用猎枪攻击野狗：掷出 3+1=4，防御 10，没有命中".to_string()];
    assert!(director::narration_mismatches("铅弹擦着野狗的耳朵飞了过去——没有命中！", &s).is_empty());
    assert!(director::narration_mismatches("你没能击中它。", &s).is_empty());
    assert_eq!(director::narration_mismatches("铅弹正中野狗的前胸。", &s).len(), 1);
}

#[test]
fn target_still_alive_but_narration_kills_it() {
    let s = vec!["老秦用猎枪攻击野狗：掷出 18+1=19，防御 12（掩体 +2），命中，造成 6 点伤害（野狗 生命 5/11）".to_string()];
    assert_eq!(director::narration_mismatches("铅弹击中野狗的肩窝，它轰然倒地，彻底没了声息。", &s).len(), 1);
    assert!(director::narration_mismatches("铅弹击中野狗的肩窝，它哀嚎着踉跄后退。", &s).is_empty());
    let down = vec![s[0].clone(), "野狗倒下了".to_string()];
    assert!(!director::narration_mismatches("它彻底没了声息。", &down).iter().any(|c| c.contains("还活着")));
}

#[test]
fn duplicate_kit_items_are_kept_once_and_miss_written_as_kill_is_flagged() {
    let k = pc::normalize_kit(&[
        StartItem { name: "粗布工作服".into(), qty: 1, slot: "armor".into(), armor: 1, ..Default::default() },
        StartItem { name: "粗布工作服".into(), qty: 1, slot: "armor".into(), armor: 1, ..Default::default() },
        StartItem { name: "卷尺".into(), qty: 1, ..Default::default() },
    ]);
    assert_eq!(k.iter().filter(|i| i.name == "粗布工作服").count(), 1);
    let s = vec!["老秦用猎枪攻击野狗：掷出 9+1=10，防御 12（掩体 +2），没有命中".to_string()];
    assert!(!director::narration_mismatches("它似乎被击中要害，当场倒地不起。", &s).is_empty());
    let alive = vec!["老秦用猎枪攻击野狗：掷出 18+1=19，防御 12，命中，造成 6 点伤害（野狗 生命 5/11）".to_string()];
    assert!(!director::narration_mismatches("它前半身猛地僵直，随即瘫软在地，不再挣扎。", &alive).is_empty());
}

#[test]
fn coins_go_to_the_purse_and_zero_armor_is_not_worn() {
    let mut e = env();
    let c = campaign::create_campaign(&mut e.world, None, "歌手", Some(2)).unwrap().id;
    let money0 = rpg::money(e.world.conn(), &c, PARTY);
    let kit = vec![
        StartItem { name: "铜币".into(), qty: 12, ..Default::default() },
        StartItem { name: "行囊".into(), qty: 1, slot: "armor".into(), armor: 0, ..Default::default() },
        StartItem { name: "旧木箱吉他".into(), qty: 1, ..Default::default() },
    ];
    let notes = rpg::grant_start_kit(&mut e.world, &c, &kit).unwrap();
    assert!(notes.iter().any(|n| n.contains("放进钱包")), "{notes:?}");
    let r = rpg::rules(e.world.conn(), &c);
    let rate = r.currency.iter().position(|x| x == "铜币").map(|i| r.currency_rates[i]).unwrap_or(1);
    assert_eq!(rpg::money(e.world.conn(), &c, PARTY), money0 + 12 * rate);
    let bag = rpg::inventory(e.world.conn(), &c, PARTY).unwrap();
    assert!(!bag.iter().any(|i| i.item.title == "铜币"), "铜币不是背包物品");
    assert!(!bag.iter().find(|i| i.item.title == "行囊").unwrap().equipped, "护甲值 0 的不当护甲穿");
}

#[test]
fn risky_actions_are_recognised() {
    assert_eq!(rpg::action_skill("我趁大家不注意，顺着铁梯往上爬").map(|x| x.0), Some("潜行"));
    assert_eq!(rpg::action_skill("顺着铁梯往上爬").map(|x| x.0), Some("攀爬"));
    assert_eq!(rpg::action_skill("我假装调琴，偷偷观察他").map(|x| x.0), Some("潜行"));
}

#[test]
fn made_up_ammo_names_are_dropped() {
    let k = pc::normalize_kit(&[StartItem { name: "旧木箱吉他".into(), qty: 1, slot: "weapon".into(), damage: "1d6".into(), range: 1, ammo: "string".into(), ..Default::default() }]);
    assert!(k[0].ammo.is_empty(), "弹药不是这套装备里的东西：清空");
    let k = pc::normalize_kit(&hunter_kit());
    assert_eq!(k.iter().find(|i| i.name == "猎枪").unwrap().ammo, "猎枪铅弹", "真有的弹药保留");
    // 旧存档里弹药名乱写：战斗里当作不用弹药（不会"打不响"）。
    let mut e = env();
    let c = campaign::create_campaign(&mut e.world, None, "旧存档", Some(4)).unwrap().id;
    rpg::grant_start_kit(&mut e.world, &c, &hunter_kit()[..1]).unwrap();
    combat::start(&mut e.world, &c, &[EnemySpec { name: "狼".into(), npc: None, level: 1, count: 1, loot: vec![], boss: false }], &[], "x").unwrap();
    let cb = combat::active(e.world.conn(), &c).unwrap();
    assert!(cb.units.iter().find(|u| u.side == "party").unwrap().ammo.is_empty());
}

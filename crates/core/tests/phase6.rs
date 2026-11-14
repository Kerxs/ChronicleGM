//! 战斗、物品与经济、人物成长：全部数值由程序计算，事件来源，随时间线回溯。

mod common;

use chronicle_core::campaign;
use chronicle_core::combat::{self, CombatAction, EnemySpec};
use chronicle_core::rpg::{self, PARTY, RecipeInput, RuleSet};
use chronicle_core::schedule;
use chronicle_core::timeline;
use chronicle_core::*;
use chronicle_rules::StateView;
use common::*;
use serde_json::{Value, json};

fn eff(w: &mut World, c: &str, v: Value) {
    let e: Vec<chronicle_rules::Effect> = serde_json::from_value(v).unwrap();
    campaign::apply_effects(w, c, &e, "测试").unwrap();
}

fn item(w: &mut World, name: &str, p: Value) -> String {
    mk(w, NewNode::new(NodeKind::Item, name).payload(p)).id
}

struct Fx {
    e: Env,
    camp: String,
    sword: String,
    bow: String,
    armor: String,
    potion: String,
    herb: String,
    salve: String,
    smith: String,
    guard: String,
}

fn fx(seed: i64) -> Fx {
    let mut e = env();
    let w = &mut e.world;
    let sword = item(w, "长剑", json!({"value": 150, "rarity": "uncommon", "slot": "weapon", "damage": "1d8", "bonuses": {"运动": 1}}));
    let bow = item(w, "毒箭短弓", json!({"value": 300, "rarity": "rare", "slot": "weapon", "damage": "1d6", "range": 6, "on_hit": {"name": "中毒", "rounds": 3, "dot": "1d2", "modifier": -1}}));
    let armor = item(w, "皮甲", json!({"value": 100, "slot": "armor", "armor": 2}));
    let potion = item(w, "治疗药水", json!({"value": 50, "heal": "2d4+2", "cures": ["中毒"]}));
    let herb = item(w, "草药", json!({"value": 5}));
    let salve = item(w, "药膏", json!({"value": 40, "heal": "1d4", "recipe": {"inputs": [{"item": herb, "qty": 2}], "skill": "医疗", "dc": 10, "minutes": 30}}));
    let smith = mk(
        w,
        NewNode::new(NodeKind::Character, "铁匠阿岩").payload(json!({
            "stats": {"level": 3, "hp": 30, "attrs": {"str": 16}, "skills": {"锻造": 4}},
            "shop": {"stock": [{"item": sword, "qty": 2}, {"item": armor, "qty": 1}, {"item": potion}], "markup": 1.2, "buy_rate": 0.4, "money": 500}
        })),
    )
    .id;
    let guard = mk(w, NewNode::new(NodeKind::Character, "守卫").payload(json!({"stats": {"level": 2, "hp": 14, "ac": 12}}))).id;
    let camp = campaign::create_campaign(w, None, "冒险", Some(seed)).unwrap().id;
    Fx { e, camp, sword, bow, armor, potion, herb, salve, smith, guard }
}

#[test]
fn sheets_checks_equipment_and_validation() {
    let mut f = fx(1);
    let (w, c) = (&mut f.e.world, f.camp.clone());
    // 默认角色卡与 NPC 数值。
    let p = rpg::sheet(w.conn(), &c, PARTY).unwrap();
    assert_eq!((p.level, p.hp, p.attr("str")), (1, 12, 10));
    let s = rpg::sheet(w.conn(), &c, &f.smith).unwrap();
    assert_eq!((s.level, s.max_hp, s.attr("str"), s.skills["锻造"]), (3, 30, 16, 4));
    // 资料校验。
    assert!(w.write(&user(), "n", |x| x.create_node(NewNode::new(NodeKind::Item, "坏物品").payload(json!({"rarity": "神器"})))).is_err());
    assert!(w.write(&user(), "n", |x| x.create_node(NewNode::new(NodeKind::Character, "坏人物").payload(json!({"stats": {"attrs": {"luck": 3}}})))).is_err());
    assert!(w.write(&user(), "n", |x| x.create_node(NewNode::new(NodeKind::Character, "坏商人").payload(json!({"shop": {"stock": [{"qty": 1}]}})))).is_err());
    // 检定修正 = 旧变量 + 技能 + 属性 + 装备 + 状态 − 伤势。
    let mut p = rpg::sheet(w.conn(), &c, PARTY).unwrap();
    p.name = "艾琳".into();
    p.attrs.insert("str".into(), 14);
    p.skills.insert("运动".into(), 2);
    rpg::save_sheet(w, &c, &p, "创建角色").unwrap();
    eff(w, &c, json!([{"type": "set_var", "key": "skill.party.运动", "value": 1}]));
    assert_eq!(rpg::check_modifier(w.conn(), &c, PARTY, "运动"), 1 + 2 + 2);
    rpg::give_item(w, &c, PARTY, &f.sword, 1, "剧情").unwrap();
    rpg::give_item(w, &c, PARTY, &f.armor, 1, "剧情").unwrap();
    assert!(rpg::equip(w, &c, PARTY, &f.potion).is_err(), "药水不能装备");
    assert!(rpg::equip(w, &c, PARTY, &f.bow).is_err(), "没有的东西不能装备");
    rpg::equip(w, &c, PARTY, &f.sword).unwrap();
    rpg::equip(w, &c, PARTY, &f.armor).unwrap();
    assert_eq!(rpg::check_modifier(w.conn(), &c, PARTY, "运动"), 6, "长剑 +1 运动");
    let view = campaign::CampaignView { conn: w.conn(), campaign_id: &c };
    assert_eq!(view.skill_mod(PARTY, "运动"), 6, "规则 DSL 的检定也用角色卡");
    let p = rpg::sheet(w.conn(), &c, PARTY).unwrap();
    assert_eq!(rpg::armor_class(w.conn(), &c, &p), 12, "基础 10 + 皮甲 2");
    assert_eq!(rpg::weapon(w.conn(), &c, &p).1, "1d8");
    let mut p = p;
    p.wounds.push(rpg::Wound { name: "扭伤".into(), penalty: 2, attr: "str".into(), heal_at: 600, source: "测试".into() });
    p.statuses.push(rpg::Status { name: "祝福".into(), modifier: 1, until: Some(10_000), ..Default::default() });
    rpg::update_sheet(w, &c, &p, "wound", "扭伤").unwrap();
    assert_eq!(rpg::check_modifier(w.conn(), &c, PARTY, "运动"), 6 - 2 + 1);
    assert_eq!(rpg::check_modifier(w.conn(), &c, PARTY, "察觉"), 1, "扭伤只影响力量类检定");
    // 时间流逝：伤势痊愈，状态按时间到期，生命恢复。
    let notes = schedule::advance(w, &c, 240, "休息").unwrap();
    assert!(notes.iter().any(|n| n.contains("扭伤") && n.contains("痊愈")), "{notes:?}");
    let p = rpg::sheet(w.conn(), &c, PARTY).unwrap();
    assert!(p.wounds.is_empty() && p.statuses.len() == 1);
}

#[test]
fn shops_money_selling_and_sources() {
    let mut f = fx(2);
    let (w, c) = (&mut f.e.world, f.camp.clone());
    let r = RuleSet::default();
    let sh = rpg::shop(w.conn(), &c, &f.smith).unwrap().unwrap();
    let sw = sh.items.iter().find(|i| i.item.id == f.sword).unwrap();
    assert_eq!((sw.buy_price, sw.stock), (180, Some(2)), "150 × 1.2");
    assert_eq!(sh.items.iter().find(|i| i.item.id == f.potion).unwrap().stock, None, "没写数量的不限量");
    assert!(rpg::buy(w, &c, &f.smith, &f.sword, 1).unwrap_err().to_string().contains("钱不够"));
    rpg::change_money(w, &c, PARTY, 1000, "测试").unwrap();
    assert!(rpg::change_money(w, &c, PARTY, -5000, "测试").is_err(), "不能变成负数");
    let t = rpg::buy(w, &c, &f.smith, &f.sword, 2).unwrap();
    assert!(t.contains("3 金币 6 银币"), "{t}");
    assert_eq!(rpg::money(w.conn(), &c, PARTY), 1000 - 360);
    assert_eq!(rpg::money(w.conn(), &c, &f.smith), 500 + 360);
    assert!(rpg::buy(w, &c, &f.smith, &f.sword, 1).unwrap_err().to_string().contains("只剩 0"));
    assert!(rpg::buy(w, &c, &f.smith, &f.herb, 1).is_err(), "商人不卖的东西");
    let inv = rpg::inventory(w.conn(), &c, PARTY).unwrap();
    assert_eq!(inv[0].source, "购买：铁匠阿岩");
    assert_eq!(inv[0].qty, 2);
    // 卖出：按收购比例，装备的卖光后自动卸下。
    rpg::equip(w, &c, PARTY, &f.sword).unwrap();
    let t = rpg::sell(w, &c, &f.smith, &f.sword, 2).unwrap();
    assert!(t.contains(&rpg::format_money(&r, 120)), "150 × 0.4 × 2：{t}");
    assert!(rpg::equipment(w.conn(), &c, PARTY).is_empty());
    assert!(rpg::sell(w, &c, &f.smith, &f.sword, 1).is_err(), "没有了");
    // 关系好有折扣（信任 100 → 9 折）。
    eff(w, &c, json!([{"type": "adjust_bond", "from": f.smith, "to": "party", "trust": 100, "reason": "老朋友"}]));
    let sh = rpg::shop(w.conn(), &c, &f.smith).unwrap().unwrap();
    assert_eq!(sh.items.iter().find(|i| i.item.id == f.armor).unwrap().buy_price, 108);
    // 使用消耗品。
    rpg::buy(w, &c, &f.smith, &f.potion, 1).unwrap();
    let mut p = rpg::sheet(w.conn(), &c, PARTY).unwrap();
    p.hp = 3;
    p.statuses.push(rpg::Status { name: "中毒".into(), ..Default::default() });
    rpg::update_sheet(w, &c, &p, "wound", "受伤").unwrap();
    let t = rpg::use_item(w, &c, PARTY, &f.potion).unwrap();
    let p = rpg::sheet(w.conn(), &c, PARTY).unwrap();
    assert!(p.hp > 3 && p.statuses.is_empty(), "{t}");
    assert!(rpg::use_item(w, &c, PARTY, &f.potion).is_err(), "用完了");
}

#[test]
fn crafting_consumes_materials_by_program_check() {
    let mut f = fx(3);
    let (w, c) = (&mut f.e.world, f.camp.clone());
    assert!(rpg::craft(w, &c, PARTY, &f.salve).unwrap_err().to_string().contains("材料不够"));
    rpg::give_item(w, &c, PARTY, &f.herb, 20, "采集").unwrap();
    let r = rpg::recipes(w.conn(), &c, PARTY).unwrap();
    assert!(r.iter().any(|(i, ok)| i.id == f.salve && *ok));
    let view_qty = |w: &World, it: &str| campaign::CampaignView { conn: w.conn(), campaign_id: &c }.item_qty(PARTY, it);
    let mut made = 0;
    let mut lost = 0;
    for _ in 0..8 {
        let before = (view_qty(w, &f.herb), schedule::clock(w.conn(), &c));
        let res = rpg::craft(w, &c, PARTY, &f.salve).unwrap();
        assert_eq!(schedule::clock(w.conn(), &c), before.1 + 30, "制作耗时 30 分钟");
        let used = before.0 - view_qty(w, &f.herb);
        if res.success {
            made += 1;
            assert_eq!(used, 2);
        } else if res.materials_lost {
            lost += 1;
            assert_eq!(used, 2);
        } else {
            assert_eq!(used, 0, "差得不多时材料还在");
        }
        assert_eq!(res.success, res.roll + res.modifier >= 10);
    }
    assert_eq!(view_qty(w, &f.salve), made);
    assert!(made > 0, "8 次至少成功一次");
    let _ = lost;
    assert!(rpg::inventory(w.conn(), &c, PARTY).unwrap().iter().any(|i| i.item.id == f.salve && i.source == "制作"));
    assert!(rpg::growth_log(w.conn(), &c, Some(PARTY), 100).unwrap().iter().any(|g| g.kind == "craft"));
}

#[test]
fn experience_levels_points_and_growth_log() {
    let mut f = fx(4);
    let (w, c) = (&mut f.e.world, f.camp.clone());
    let notes = rpg::award_xp(w, &c, PARTY, 350, "击退狼群").unwrap();
    assert!(notes.iter().any(|n| n.contains("升到 2 级")) && notes.iter().any(|n| n.contains("升到 3 级")), "{notes:?}");
    let p = rpg::sheet(w.conn(), &c, PARTY).unwrap();
    assert_eq!((p.level, p.xp, p.unspent_skill, p.max_hp), (3, 350, 4, 12 + 6 * 2));
    rpg::spend_skill(w, &c, PARTY, "潜行").unwrap();
    rpg::spend_skill(w, &c, PARTY, "潜行").unwrap();
    assert_eq!(rpg::sheet(w.conn(), &c, PARTY).unwrap().skills["潜行"], 2);
    assert!(rpg::spend_attr(w, &c, PARTY, "str").is_err(), "3 级还没有属性点");
    rpg::award_xp(w, &c, PARTY, 300, "完成委托").unwrap();
    let p = rpg::sheet(w.conn(), &c, PARTY).unwrap();
    assert_eq!((p.level, p.unspent_attr), (4, 1));
    rpg::spend_attr(w, &c, PARTY, "dex").unwrap();
    assert_eq!(rpg::sheet(w.conn(), &c, PARTY).unwrap().attr("dex"), 11);
    // NPC 也会成长，记录分开。
    rpg::award_xp(w, &c, &f.guard, 100, "陪玩家作战").unwrap();
    assert_eq!(rpg::sheet(w.conn(), &c, &f.guard).unwrap().xp, 100);
    let log = rpg::growth_log(w.conn(), &c, Some(PARTY), 100).unwrap();
    for k in ["xp", "level", "skill", "attr"] {
        assert!(log.iter().any(|g| g.kind == k), "{k}");
    }
    assert!(rpg::growth_log(w.conn(), &c, Some(&f.guard), 10).unwrap().iter().all(|g| g.actor == f.guard));
    // 规则可以自定义经验曲线。
    let r = RuleSet { xp_curve: 1000, ..RuleSet::default() };
    rpg::set_rules(w, &c, &r).unwrap();
    assert_eq!(rpg::xp_for_level(&rpg::rules(w.conn(), &c), 2), 1000);
    assert!(rpg::set_rules(w, &c, &RuleSet { ap_per_turn: 0, ..RuleSet::default() }).is_err());
}

fn fight_to_end(w: &mut World, c: &str) -> combat::Combat {
    for _ in 0..200 {
        let Some(cb) = combat::active(w.conn(), c) else { break };
        assert!(cb.players_turn(), "进行中的战斗只会停在玩家的回合");
        let me = cb.units.iter().find(|u| u.actor.as_deref() == Some(PARTY)).unwrap().clone();
        let target = cb.units.iter().filter(|u| u.side == "enemy" && u.out.is_empty()).min_by_key(|u| combat::dist(&me, u)).unwrap().clone();
        let a = if combat::dist(&me, &target) <= me.range && me.ap < 2 {
            CombatAction { kind: "end_turn".into(), ..Default::default() }
        } else if combat::dist(&me, &target) <= me.range {
            CombatAction { kind: "attack".into(), target: target.id.clone(), ..Default::default() }
        } else {
            CombatAction { kind: "move".into(), target: target.id.clone(), ..Default::default() }
        };
        let rep = combat::act(w, c, &a).unwrap();
        if rep.combat.status != "active" {
            return rep.combat;
        }
    }
    combat::history(w.conn(), c, 1).unwrap().remove(0)
}

#[test]
fn turn_based_combat_with_initiative_ap_positions_statuses_and_rewards() {
    let mut f = fx(5);
    let (w, c) = (&mut f.e.world, f.camp.clone());
    let mut p = rpg::sheet(w.conn(), &c, PARTY).unwrap();
    p.name = "艾琳".into();
    p.max_hp = 40;
    p.hp = 40;
    p.attrs.insert("dex".into(), 16);
    rpg::save_sheet(w, &c, &p, "创建角色").unwrap();
    rpg::give_item(w, &c, PARTY, &f.bow, 1, "剧情").unwrap();
    rpg::give_item(w, &c, PARTY, &f.potion, 1, "剧情").unwrap();
    rpg::equip(w, &c, PARTY, &f.bow).unwrap();
    let rep = combat::start(w, &c, &[EnemySpec { name: "哥布林".into(), npc: None, level: 1, count: 2, loot: vec![RecipeInput { item: f.herb.clone(), qty: 3 }] }], std::slice::from_ref(&f.guard), "林中遇袭").unwrap();
    let cb = rep.combat;
    assert!(rep.lines[0].contains("先攻顺序"));
    assert_eq!(cb.units.len(), 4, "玩家、守卫、两个哥布林");
    let inits: Vec<i64> = cb.order.iter().map(|id| cb.units.iter().find(|u| &u.id == id).unwrap().init).collect();
    assert!(inits.windows(2).all(|w| w[0] >= w[1]), "按先攻从高到低：{inits:?}");
    assert!(cb.players_turn());
    assert!(combat::start(w, &c, &[EnemySpec { name: "x".into(), npc: None, level: 1, count: 1, loot: vec![] }], &[], "").is_err(), "不能同时开两场");
    let me = cb.units.iter().find(|u| u.actor.as_deref() == Some(PARTY)).unwrap();
    assert_eq!((me.range, me.weapon.as_str()), (6, "毒箭短弓"));
    assert_eq!(me.ap, 3);
    // 行动点：攻击 2 点，剩 1 点不够再攻击。
    let gob = cb.units.iter().find(|u| u.side == "enemy").unwrap().id.clone();
    let far = combat::dist(me, cb.units.iter().find(|u| u.id == gob).unwrap());
    if far <= 6 {
        let r1 = combat::act(w, &c, &CombatAction { kind: "attack".into(), target: gob.clone(), ..Default::default() }).unwrap();
        if r1.combat.players_turn() && r1.combat.status == "active" {
            let me2 = r1.combat.units.iter().find(|u| u.actor.as_deref() == Some(PARTY)).unwrap();
            if me2.ap == 1 {
                assert!(combat::act(w, &c, &CombatAction { kind: "attack".into(), target: gob.clone(), ..Default::default() }).unwrap_err().to_string().contains("行动点不够"));
            }
        }
    }
    assert!(combat::act(w, &c, &CombatAction { kind: "dance".into(), ..Default::default() }).is_err());
    let end = fight_to_end(w, &c);
    assert!(matches!(end.status.as_str(), "victory" | "defeat"), "{}", end.status);
    assert!(combat::active(w.conn(), &c).is_none());
    let log = end.log.join("\n");
    assert!(log.contains("攻击"), "{log}");
    let p = rpg::sheet(w.conn(), &c, PARTY).unwrap();
    let me_end = end.units.iter().find(|u| u.actor.as_deref() == Some(PARTY)).unwrap();
    assert_eq!(p.hp, me_end.hp.max(0) + (p.max_hp - 40), "生命值写回角色卡（胜利升级时再加上新增的生命上限）");
    if end.status == "victory" {
        assert!(p.xp >= 100, "两个 1 级哥布林各 50 经验：{}", p.xp);
        assert_eq!(campaign::CampaignView { conn: w.conn(), campaign_id: &c }.item_qty(PARTY, &f.herb), 3, "战利品");
        assert!(end.rewards.iter().any(|r| r.contains("草药")));
    }
    // 守卫（有角色卡的 NPC）的生命值也写回。
    let g = end.units.iter().find(|u| u.actor.as_deref() == Some(f.guard.as_str())).unwrap();
    assert_eq!(rpg::sheet(w.conn(), &c, &f.guard).unwrap().hp, g.hp.max(0));
    timeline::end_campaign(w, &c, "").unwrap();
    assert!(timeline::verify(w.conn(), &c).unwrap().ok, "战斗全程可以重放");
}

#[test]
fn defeat_downs_the_party_with_wounds_and_rest_recovers() {
    let mut f = fx(6);
    let (w, c) = (&mut f.e.world, f.camp.clone());
    let rep = combat::start(w, &c, &[EnemySpec { name: "巨魔".into(), npc: None, level: 12, count: 1, loot: vec![] }], &[], "不自量力").unwrap();
    assert!(rep.combat.status == "active" || rep.combat.status == "defeat");
    let mut end = rep.combat;
    for _ in 0..50 {
        if end.status != "active" {
            break;
        }
        end = combat::act(w, &c, &CombatAction { kind: "defend".into(), ..Default::default() }).unwrap().combat;
    }
    assert_eq!(end.status, "defeat");
    assert!(end.log.iter().any(|l| l.contains("防御姿态")) || end.round == 1);
    let p = rpg::sheet(w.conn(), &c, PARTY).unwrap();
    assert_eq!(p.hp, 0);
    assert!(p.wounds.iter().any(|w| w.name == "重伤"), "倒下时记一处重伤");
    assert!(combat::start(w, &c, &[EnemySpec { name: "x".into(), npc: None, level: 1, count: 1, loot: vec![] }], &[], "").unwrap_err().to_string().contains("倒下"));
    // 休息：每小时恢复 1 点，三天后重伤痊愈。
    schedule::advance(w, &c, 6 * 60, "昏迷").unwrap();
    assert_eq!(rpg::sheet(w.conn(), &c, PARTY).unwrap().hp, 6);
    for _ in 0..3 {
        schedule::advance(w, &c, 1440, "养伤").unwrap();
    }
    let p = rpg::sheet(w.conn(), &c, PARTY).unwrap();
    assert!(p.wounds.is_empty() && p.hp == p.max_hp);
}

#[test]
fn custom_rules_and_actions_and_poison_status() {
    let mut f = fx(7);
    let (w, c) = (&mut f.e.world, f.camp.clone());
    let mut r = RuleSet { ap_per_turn: 5, ..RuleSet::default() };
    r.custom_actions.push(rpg::CustomAction { name: "治愈术".into(), ap: 2, kind: "heal".into(), dice: "1d8".into(), attr: "wis".into(), range: 0, status: None });
    r.custom_actions.push(rpg::CustomAction { name: "眩晕打击".into(), ap: 3, kind: "attack".into(), dice: "1d4".into(), attr: "str".into(), range: 1, status: Some(rpg::Status { name: "眩晕".into(), rounds: Some(1), skip: true, ..Default::default() }) });
    rpg::set_rules(w, &c, &r).unwrap();
    assert!(rpg::set_rules(w, &c, &RuleSet { custom_actions: vec![rpg::CustomAction { name: "x".into(), ap: 1, kind: "飞".into(), dice: String::new(), attr: String::new(), range: 0, status: None }], ..RuleSet::default() }).is_err());
    let mut p = rpg::sheet(w.conn(), &c, PARTY).unwrap();
    p.max_hp = 50;
    p.hp = 20;
    rpg::save_sheet(w, &c, &p, "x").unwrap();
    rpg::give_item(w, &c, PARTY, &f.bow, 1, "x").unwrap();
    rpg::equip(w, &c, PARTY, &f.bow).unwrap();
    let cb = combat::start(w, &c, &[EnemySpec { name: "强盗".into(), npc: None, level: 3, count: 1, loot: vec![] }], &[], "拦路").unwrap().combat;
    if cb.status != "active" {
        return;
    }
    assert_eq!(cb.current().unwrap().ap, 5, "自定义行动点");
    let rep = combat::act(w, &c, &CombatAction { kind: "custom".into(), custom: "治愈术".into(), ..Default::default() }).unwrap();
    assert!(rep.lines.iter().any(|l| l.contains("治愈术")), "{:?}", rep.lines);
    // 射击直到命中，命中附带中毒，之后每轮受到持续伤害。
    let mut poisoned = false;
    for _ in 0..40 {
        let Some(cb) = combat::active(w.conn(), &c) else { break };
        let target = cb.units.iter().find(|u| u.side == "enemy").unwrap().clone();
        if target.statuses.iter().any(|s| s.name == "中毒") {
            poisoned = true;
        }
        let rep = combat::act(w, &c, &CombatAction { kind: "attack".into(), target: target.id.clone(), ..Default::default() }).or_else(|_| combat::act(w, &c, &CombatAction { kind: "end_turn".into(), ..Default::default() })).unwrap();
        if rep.lines.iter().any(|l| l.contains("中毒伤害")) {
            poisoned = true;
        }
    }
    assert!(poisoned, "毒箭命中后目标中毒");
}

#[test]
fn rpg_state_follows_timelines_and_combat_is_reproducible() {
    let run = |seed: i64| {
        let mut f = fx(seed);
        let (w, c) = (&mut f.e.world, f.camp.clone());
        rpg::change_money(w, &c, PARTY, 300, "起始资金").unwrap();
        timeline::checkpoint(w.conn(), &c, "turn_end").unwrap();
        let cp = timeline::checkpoints(w.conn(), &c).unwrap().last().unwrap().id;
        rpg::buy(w, &c, &f.smith, &f.potion, 2).unwrap();
        rpg::award_xp(w, &c, PARTY, 120, "x").unwrap();
        combat::start(w, &c, &[EnemySpec { name: "狼".into(), npc: None, level: 1, count: 2, loot: vec![] }], &[], "狼群").unwrap();
        let end = fight_to_end(w, &c);
        timeline::checkpoint(w.conn(), &c, "turn_end").unwrap();
        timeline::end_campaign(w, &c, "").unwrap();
        let b = timeline::fork(w, &c, cp, "", false).unwrap();
        assert_eq!(rpg::money(w.conn(), &b.id, PARTY), 300);
        assert!(rpg::inventory(w.conn(), &b.id, PARTY).unwrap().is_empty());
        assert_eq!(rpg::sheet(w.conn(), &b.id, PARTY).unwrap().xp, 0);
        assert!(combat::history(w.conn(), &b.id, 10).unwrap().is_empty());
        let cmp = timeline::compare(w.conn(), &[c.clone(), b.id.clone()]).unwrap();
        assert!(cmp.rows.iter().any(|r| r.category == "人物与战斗" && r.key.starts_with("角色卡") && r.differs));
        assert!(cmp.rows.iter().any(|r| r.category == "人物与战斗" && r.key.starts_with("战斗") && r.differs));
        for id in [&c, &b.id] {
            assert!(timeline::verify(w.conn(), id).unwrap().ok);
        }
        let _ = (&f.sword, &f.armor, &f.salve);
        end.log
    };
    let a = run(42);
    let b = run(42);
    assert_eq!(a, b, "同样的种子、同样的行动，战斗过程相同");
}

/// 实测问题回归：买了剑没有装备就徒手作战；战斗中换装备要立即生效。
#[test]
fn acquired_gear_auto_equips_and_mid_combat_swaps_apply() {
    let mut f = fx(8);
    let (w, c) = (&mut f.e.world, f.camp.clone());
    rpg::give_item(w, &c, PARTY, &f.sword, 1, "剧情").unwrap();
    assert_eq!(rpg::equipment(w.conn(), &c, PARTY).get("weapon"), Some(&f.sword), "空栏位自动装备");
    rpg::give_item(w, &c, PARTY, &f.bow, 1, "剧情").unwrap();
    assert_eq!(rpg::equipment(w.conn(), &c, PARTY).get("weapon"), Some(&f.sword), "已有武器时不替换");
    rpg::change_money(w, &c, PARTY, 500, "x").unwrap();
    let t = rpg::buy(w, &c, &f.smith, &f.armor, 1).unwrap();
    assert!(t.contains("已装备"), "{t}");
    let cb = combat::start(w, &c, &[EnemySpec { name: "靶子".into(), npc: None, level: 1, count: 1, loot: vec![] }], &[], "练习").unwrap().combat;
    assert_eq!(cb.units[0].weapon, "长剑");
    rpg::equip(w, &c, PARTY, &f.bow).unwrap();
    let cb = combat::active(w.conn(), &c).unwrap();
    let me = cb.units.iter().find(|u| u.actor.as_deref() == Some(PARTY)).unwrap();
    assert_eq!((me.weapon.as_str(), me.range), ("毒箭短弓", 6));
    assert!(cb.log.iter().any(|l| l.contains("换上了毒箭短弓")));
}

//! 已知问题修复的回归测试：一天内的天气变化、按每段路计算旅行耗时、战争后领土易主、世界事件与死亡对人物情绪和关系的影响、
//! 按故事时间计算的任务期限。

mod common;

use chronicle_core::campaign;
use chronicle_core::quests;
use chronicle_core::schedule;
use chronicle_core::worldsim;
use chronicle_core::*;
use chronicle_rules::StateView;
use common::*;
use serde_json::{Value, json};

fn eff(w: &mut World, c: &str, v: Value) {
    let e: Vec<chronicle_rules::Effect> = serde_json::from_value(v).unwrap();
    campaign::apply_effects(w, c, &e, "测试").unwrap();
}

fn rel(w: &mut World, a: &str, b: &str, t: &str, label: &str, directed: bool) {
    let mut r = NewRelation::new(a, b, t);
    r.label = label.into();
    r.directed = directed;
    w.write(&user(), "r", |x| x.create_relation(r)).unwrap();
}

#[test]
fn weather_changes_within_a_day_and_is_reproducible() {
    let mut changed = 0;
    for day in 1..200 {
        let ws: Vec<&str> = (0..4).map(|s| worldsim::weather_at(5, day, "夏", s * 360)).collect();
        assert_eq!(ws[0], worldsim::weather_for(5, day, "夏"), "凌晨的天气就是当天的基础天气");
        assert_eq!(ws, (0..4).map(|s| worldsim::weather_at(5, day, "夏", s * 360)).collect::<Vec<_>>(), "可复现");
        if ws.windows(2).any(|w| w[0] != w[1]) {
            changed += 1;
        }
    }
    assert!((30..170).contains(&changed), "约有一部分日子天气会在一天内变化：{changed}/199");
}

#[test]
fn each_leg_uses_the_weather_and_daylight_when_it_is_walked() {
    let mut e = env();
    let w = &mut e.world;
    let a = mk(w, NewNode::new(NodeKind::City, "甲城")).id;
    let b = mk(w, NewNode::new(NodeKind::City, "乙城")).id;
    let c = mk(w, NewNode::new(NodeKind::City, "丙城")).id;
    rel(w, &a, &b, "route_to", "12小时", false);
    rel(w, &b, &c, "route_to", "2小时", false);
    let cid = campaign::create_campaign(w, None, "t", Some(1)).unwrap().id;
    eff(w, &cid, json!([{"type": "set_var", "key": "party.location", "value": a}, {"type": "set_var", "key": "env.weather_override", "value": {"day": 1, "weather": "晴"}}]));
    // 08:00 出发：第一段 12 小时白天走完（到 20:00），第二段在夜里走，×1.3。
    let p = worldsim::plan_travel(w.conn(), &cid, &c).unwrap();
    assert_eq!(p.base_minutes, 840);
    assert_eq!(p.minutes, 720 + 156, "{p:?}");
    assert!(p.factor > 1.0 && p.factor < 1.1);
}

fn faction(w: &mut World, name: &str, goals: &[&str], power: Value) -> String {
    mk(w, NewNode::new(NodeKind::Faction, name).payload(json!({ "goals": goals, "power": power }))).id
}

#[test]
fn wars_change_territory_and_move_people() {
    let mut e = env();
    let w = &mut e.world;
    let north = faction(w, "北境", &["扩张"], json!({"military": 95, "stability": 90}));
    let south = faction(w, "南国", &[], json!({"military": 20, "stability": 90}));
    let capital = mk(w, NewNode::new(NodeKind::City, "南都")).id;
    let border = mk(w, NewNode::new(NodeKind::City, "边城")).id;
    let fort = mk(w, NewNode::new(NodeKind::City, "北堡")).id;
    rel(w, &border, &fort, "borders", "", false);
    rel(w, &south, &capital, "rules", "", true);
    rel(w, &south, &border, "rules", "", true);
    rel(w, &north, &fort, "rules", "", true);
    rel(w, &north, &south, "war", "", false);
    // 南国的将军与北境的使者原本是朋友；南国还有一位住在边城的商人。
    let general = mk(w, NewNode::new(NodeKind::Character, "南国将军")).id;
    let envoy = mk(w, NewNode::new(NodeKind::Character, "北境使者")).id;
    let merchant = mk(w, NewNode::new(NodeKind::Character, "边城商人").payload(json!({"schedule": [{"start": "00:00", "end": "23:59", "activity": "看店", "location": border}]}))).id;
    rel(w, &general, &south, "member_of", "", true);
    rel(w, &envoy, &north, "member_of", "", true);
    let cid = campaign::create_campaign(w, None, "战争", Some(4)).unwrap().id;
    eff(w, &cid, json!([{"type": "adjust_bond", "from": general, "to": envoy, "trust": 10, "affection": 10, "reason": "旧识"}]));
    assert_eq!(worldsim::rulers(w.conn(), &cid).unwrap().get(&border), Some(&south));
    for _ in 0..6 {
        schedule::advance(w, &cid, 7 * 1440, "过一周").unwrap();
    }
    let evs = worldsim::world_events(w.conn(), &cid, 500).unwrap();
    assert!(evs.iter().any(|e| e.kind == "peace"), "{:?}", evs.iter().map(|e| &e.text).collect::<Vec<_>>());
    // 世界事件新的在前：第一次占领是最后一条。
    let conquest = evs.iter().rev().find(|e| e.kind == "conquest").expect("停战后占领一个地点");
    assert!(conquest.text.contains("北境占领了边城"), "优先占领与胜方领土接壤的地点：{}", conquest.text);
    assert_eq!(worldsim::rulers(w.conn(), &cid).unwrap().get(&border), Some(&north));
    // 有扩张目标的北境之后可能再次开战，继续占领。
    let m = worldsim::map(w.conn(), &cid, true).unwrap();
    assert_eq!(m.places.iter().find(|p| p.id == border).unwrap().ruler.as_deref(), Some("北境"));
    assert!(m.roads.iter().any(|r| (r.a == border || r.b == border) && r.kind == "边境"));
    // 战败与失去领土让南国成员不安；住在被占领地点的人也不安（情绪随时间平复，所以查事件记录）。
    let log = campaign::events(w.conn(), &cid, 0, 100_000).unwrap();
    let mood_reasons = |npc: &str| -> Vec<String> {
        log.iter()
            .filter(|e| e.kind == "effects")
            .flat_map(|e| e.payload["changes"].as_array().cloned().unwrap_or_default())
            .filter(|c| c["type"] == "adjust_mood" && c["npc"] == npc)
            .map(|c| c["reason"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    let g = mood_reasons(&general);
    assert!(g.iter().any(|r| r.contains("本势力战败")) && g.iter().any(|r| r.contains("失去了领土")), "{g:?}");
    assert!(mood_reasons(&merchant).iter().any(|r| r.contains("所在的地方易主")));
    // 关系变化：每次开战，交战双方成员之间已有的关系变差（开局时已经在交战，那次不算）。
    let wars = evs.iter().filter(|e| e.kind == "war").count() as i64;
    let view = campaign::CampaignView { conn: w.conn(), campaign_id: &cid };
    assert_eq!(view.bond(&general, &envoy).unwrap().1.hostility, (wars * 5).min(100), "开战 {wars} 次");
    let _ = capital;
}

#[test]
fn deaths_affect_friends_and_rivals() {
    let mut e = env();
    let w = &mut e.world;
    let a = mk(w, NewNode::new(NodeKind::Character, "阿甲")).id;
    let b = mk(w, NewNode::new(NodeKind::Character, "阿乙")).id;
    let rival = mk(w, NewNode::new(NodeKind::Character, "仇人")).id;
    let cid = campaign::create_campaign(w, None, "c", Some(1)).unwrap().id;
    eff(w, &cid, json!([
        {"type": "adjust_bond", "from": a, "to": b, "affection": 15, "reason": "朋友"},
        {"type": "adjust_bond", "from": a, "to": b, "affection": 15, "reason": "朋友"},
        {"type": "adjust_bond", "from": rival, "to": b, "hostility": 15, "reason": "旧仇"},
        {"type": "adjust_bond", "from": rival, "to": b, "hostility": 15, "reason": "旧仇"},
        {"type": "adjust_bond", "from": rival, "to": b, "hostility": 15, "reason": "旧仇"},
        {"type": "adjust_mood", "npc": rival, "anger": 30, "stress": 30, "reason": "一直记恨"}
    ]));
    eff(w, &cid, json!([{"type": "set_entity_state", "entity": b, "field": "life_status", "value": "dead"}]));
    let view = campaign::CampaignView { conn: w.conn(), campaign_id: &cid };
    let ma = view.mood(&a).unwrap();
    assert_eq!((ma.anger, ma.stress), (10, 10), "朋友悲伤愤怒");
    let mr = view.mood(&rival).unwrap();
    assert_eq!((mr.anger, mr.stress), (25, 25), "仇人松了口气");
}

#[test]
fn quest_deadline_in_story_time() {
    let mut e = env();
    let u = user();
    let s = e.world.write(&u, "s", |w| w.create_script(NewScript::new("送信"))).unwrap().0;
    let mut it = NewScriptItem::new(&s.id, ScriptItemKind::Quest, "天黑前送信");
    it.payload = json!({"quest_type": "side", "deadline_hours": 3});
    let q = e.world.write(&u, "i", |w| w.create_script_item(it)).unwrap().0.id;
    let w = &mut e.world;
    let cid = campaign::create_campaign(w, Some(&s.id), "c", Some(1)).unwrap().id;
    schedule::advance(w, &cid, 60, "x").unwrap();
    eff(w, &cid, json!([{"type": "set_quest_state", "quest": q, "state": "active"}]));
    schedule::advance(w, &cid, 90, "赶路").unwrap();
    let qv = quests::quests(w.conn(), &cid, false).unwrap().into_iter().find(|x| x.id == q).unwrap();
    assert_eq!((qv.deadline_minutes, qv.minutes_left), (Some(180), Some(90)), "从开始时算起");
    assert!(quests::tick(w, &cid).unwrap().is_empty());
    schedule::advance(w, &cid, 100, "又耽误了").unwrap();
    let notes = quests::tick(w, &cid).unwrap();
    assert!(notes.iter().any(|n| n.contains("故事时间超过了") && n.contains("3 小时")), "{notes:?}");
    assert_eq!(campaign::campaign_state(w.conn(), &cid).unwrap().quests.get(&q).map(String::as_str), Some("failed"));
}

use chronicle_core::combat::{self, CombatAction, EnemySpec};
use chronicle_core::rpg::{self, PARTY};

fn save_combat(w: &mut World, cid: &str, c: &combat::Combat) {
    campaign::apply_effects(w, cid, &[chronicle_rules::Effect::UpsertRecord { kind: "combat".into(), id: c.id.clone(), data: serde_json::to_value(c).unwrap() }], "测试摆位").unwrap();
}

#[test]
fn battlefield_obstacles_cover_morale_and_ranged_enemies() {
    let mut e = env();
    let w = &mut e.world;
    let bow = mk(w, NewNode::new(NodeKind::Item, "长弓").payload(json!({"value": 10, "slot": "weapon", "damage": "1d8", "range": 8}))).id;
    let cid = campaign::create_campaign(w, None, "c", Some(9)).unwrap().id;
    let mut s = rpg::sheet(w.conn(), &cid, PARTY).unwrap();
    s.max_hp = 200;
    s.hp = 200;
    rpg::save_sheet(w, &cid, &s, "x").unwrap();
    rpg::give_item(w, &cid, PARTY, &bow, 1, "x").unwrap();
    let c = combat::start(w, &cid, &[EnemySpec { name: "强盗".into(), npc: None, level: 1, count: 1, loot: vec![], boss: false }, EnemySpec { name: "弓手".into(), npc: None, level: 1, count: 1, loot: vec![], boss: false }], &[], "埋伏").unwrap().combat;
    assert!((3..=5).contains(&c.obstacles.len()), "{:?}", c.obstacles);
    assert!(c.obstacles.iter().all(|&(x, y)| !c.units.iter().any(|u| u.x == x && u.y == y)));
    let archer = c.units.iter().find(|u| u.name == "弓手").unwrap();
    assert_eq!((archer.range, archer.weapon.as_str()), (6, "弓箭"), "名字表明是远程的敌人");
    if c.status != "active" {
        return;
    }
    // 摆位：玩家在 (0,2)，强盗在 (6,2)，障碍物在 (5,2)（挡在中间，紧挨强盗）。
    let mut c = combat::active(w.conn(), &cid).unwrap();
    c.obstacles = vec![(5, 2), (3, 3)];
    let mut ids = vec![];
    for u in c.units.iter_mut() {
        match u.name.as_str() {
            "强盗" => (u.x, u.y) = (6, 2),
            "弓手" => (u.x, u.y) = (9, 5),
            _ if u.actor.as_deref() == Some(PARTY) => (u.x, u.y) = (0, 2),
            _ => {}
        }
        ids.push((u.name.clone(), u.id.clone()));
    }
    save_combat(w, &cid, &c);
    assert!(combat::act(w, &cid, &CombatAction { kind: "move".into(), x: Some(3), y: Some(3), ..Default::default() }).unwrap_err().to_string().contains("障碍物"));
    let bandit = ids.iter().find(|(n, _)| n == "强盗").unwrap().1.clone();
    let rep = combat::act(w, &cid, &CombatAction { kind: "attack".into(), target: bandit.clone(), ..Default::default() }).unwrap();
    assert!(rep.lines[0].contains("掩体 +2"), "{:?}", rep.lines);
    // 士气：敌人生命降到四分之一时检定一次。
    if let Some(mut c) = combat::active(w.conn(), &cid) {
        for u in c.units.iter_mut().filter(|u| u.side == "enemy" && u.out.is_empty()) {
            u.hp = (u.max_hp / 4).max(1);
        }
        save_combat(w, &cid, &c);
        let mut seen = false;
        for _ in 0..4 {
            let Some(c) = combat::active(w.conn(), &cid) else { break };
            if !c.players_turn() {
                break;
            }
            let rep = combat::act(w, &cid, &CombatAction { kind: "end_turn".into(), ..Default::default() }).unwrap();
            if rep.lines.iter().any(|l| l.contains("士气")) {
                seen = true;
                break;
            }
        }
        assert!(seen, "重伤的敌人检定士气");
    }
}

#[test]
fn shops_restock_react_to_war_and_support_barter() {
    let mut e = env();
    let w = &mut e.world;
    let sword = mk(w, NewNode::new(NodeKind::Item, "铁剑").payload(json!({"value": 100, "slot": "weapon", "damage": "1d6"}))).id;
    let herb = mk(w, NewNode::new(NodeKind::Item, "草药").payload(json!({"value": 40, "heal": "1d4"}))).id;
    let merchant = mk(w, NewNode::new(NodeKind::Character, "商人").payload(json!({"shop": {"stock": [{"item": sword, "qty": 2}, {"item": herb, "qty": 5}], "markup": 1.0, "buy_rate": 0.5, "money": 300}}))).id;
    let realm = mk(w, NewNode::new(NodeKind::Faction, "王国")).id;
    let rebels = mk(w, NewNode::new(NodeKind::Faction, "叛军")).id;
    rel(w, &merchant, &realm, "member_of", "", true);
    let cid = campaign::create_campaign(w, None, "c", Some(2)).unwrap().id;
    rpg::change_money(w, &cid, PARTY, 1000, "x").unwrap();
    let price = |w: &World| rpg::shop(w.conn(), &cid, &merchant).unwrap().unwrap().items.iter().find(|i| i.item.id == sword).unwrap().buy_price;
    assert_eq!(price(w), 100);
    // 商人所属势力开战：武器涨价 25%，并说明原因。
    eff(w, &cid, json!([{"type": "set_relation", "a": realm, "b": rebels, "rel_type": "war", "active": true}]));
    assert_eq!(price(w), 125);
    assert!(rpg::shop(w.conn(), &cid, &merchant).unwrap().unwrap().price_notes[0].contains("战时"));
    // 买光后每天补 1 个；本钱慢慢恢复。
    rpg::buy(w, &cid, &merchant, &sword, 2).unwrap();
    let stock = |w: &World| rpg::shop(w.conn(), &cid, &merchant).unwrap().unwrap().items.iter().find(|i| i.item.id == sword).unwrap().stock;
    assert_eq!(stock(w), Some(0));
    schedule::advance(w, &cid, 1440, "过一天").unwrap();
    assert_eq!(stock(w), Some(1));
    schedule::advance(w, &cid, 3 * 1440, "过三天").unwrap();
    assert_eq!(stock(w), Some(2), "不超过设定的数量");
    // 以物易物：一把剑（作价 50）换两份草药……这里换一份（40），对方找回 10。
    let t = rpg::barter(w, &cid, &merchant, &[(sword.clone(), 1)], &[(herb.clone(), 1)]).unwrap();
    assert!(t.contains("对方找回 1 银币"), "{t}");
    let view = campaign::CampaignView { conn: w.conn(), campaign_id: &cid };
    assert_eq!((view.item_qty(PARTY, &sword), view.item_qty(PARTY, &herb)), (1, 1));
    assert!(rpg::barter(w, &cid, &merchant, &[(herb.clone(), 5)], &[(sword.clone(), 1)]).is_err(), "没有那么多草药");
}

//! 沉浸式互动引擎：规则结算、状态持久化、读档与分叉后一致。

mod common;

use chronicle_core::interaction::{self, OpenRequest, PlayerOp};
use chronicle_core::rules::{Effect, StateView};
use chronicle_core::*;
use common::*;
use serde_json::json;

struct Fx {
    e: Env,
    c: String,
    key: String,
    pick: String,
    potion: String,
    coin: String,
}

fn fx() -> Fx {
    let mut e = env();
    let key = mk(&mut e.world, NewNode::new(NodeKind::Item, "铁钥匙").payload(json!({ "unlocks": ["地窖门"] }))).id;
    let pick = mk(&mut e.world, NewNode::new(NodeKind::Item, "撬锁器").payload(json!({ "tags": ["lockpick"] }))).id;
    let potion = mk(&mut e.world, NewNode::new(NodeKind::Item, "草药").payload(json!({ "heal": "2" }))).id;
    let coin = mk(&mut e.world, NewNode::new(NodeKind::Item, "旧金币").payload(json!({}))).id;
    let room = mk(
        &mut e.world,
        NewNode::new(NodeKind::Site, "地窖口").payload(json!({ "objects": [
            { "id": "door", "name": "地窖门", "kind": "door", "lock": { "kind": "key", "dc": 12 } },
            { "id": "safe", "name": "保险箱", "kind": "container", "lock": { "kind": "code", "code": "0427", "tries": 3 }, "contents": [{ "item": coin, "qty": 2 }] },
            { "id": "crate", "name": "旧木箱", "kind": "container", "contents": [{ "item": coin, "qty": 1 }] },
            { "id": "hole", "name": "墙缝", "kind": "wall", "hidden": true, "find_dc": 15 },
            { "id": "levers", "name": "三根拉杆", "kind": "mechanism", "mechanism": {
                "kind": "levers", "labels": ["左", "中", "右"], "modulo": 2, "start": [0, 0, 0], "goal": [1, 1, 1],
                "ops": [ { "id": "a", "label": "拉左边", "add": [1, 1, 0] }, { "id": "b", "label": "拉右边", "add": [0, 1, 1] }, { "id": "c", "label": "拉中间", "add": [0, 1, 0] } ] } }
        ] })),
    )
    .id;
    let c = campaign::create_campaign(&mut e.world, None, "测试团", Some(3)).unwrap().id;
    campaign::apply_effects(&mut e.world, &c, &[Effect::SetVar { key: "party.location".into(), value: json!(room) }], "到达").unwrap();
    Fx { e, c, key, pick, potion, coin }
}

fn open(f: &mut Fx, kind: &str, target: &str) -> interaction::Interaction {
    interaction::open(&mut f.e.world, &f.c, &OpenRequest { kind: kind.into(), target: target.into(), ..Default::default() }).unwrap()
}

fn op(action: &str, dice: Option<u32>) -> PlayerOp {
    PlayerOp { action: action.into(), dice: dice.map(|d| vec![vec![d]]), ..Default::default() }
}

fn qty(f: &Fx, item: &str) -> i64 {
    campaign::CampaignView { conn: f.e.world.conn(), campaign_id: &f.c }.item_qty(&rpg::actor(), item)
}

fn give(f: &mut Fx, item: &str) {
    campaign::apply_effects(&mut f.e.world, &f.c, &[Effect::GiveItem { holder: rpg::actor(), item: item.into(), qty: 1 }], "测试").unwrap();
}

#[test]
fn key_must_match_and_be_carried() {
    let mut f = fx();
    let i = open(&mut f, "lock", "地窖门");
    let key_action = i.available_actions.iter().find(|a| a.id == "key").unwrap();
    assert!(!key_action.available, "身上没有钥匙时不能用钥匙开");
    assert!(interaction::act(&mut f.e.world, &f.c, &i.interaction_id, &op("key", None)).is_err());
    { let k = f.key.clone(); give(&mut f, &k); }
    let i = open(&mut f, "lock", "地窖门");
    assert!(i.available_actions.iter().any(|a| a.id == "key" && a.available && a.label.contains("铁钥匙")));
    let r = interaction::act(&mut f.e.world, &f.c, &i.interaction_id, &op("key", None)).unwrap();
    assert!(r.finished && r.interaction.result.unwrap().success);
    let s = interaction::object_state(f.e.world.conn(), &f.c, &i.target);
    assert!(s.opened, "锁的状态保存下来");
    assert!(interaction::open(&mut f.e.world, &f.c, &OpenRequest { kind: "lock".into(), target: "地窖门".into(), ..Default::default() }).is_err(), "开过的门不能再开锁");
}

#[test]
fn code_lock_alarm_and_success() {
    let mut f = fx();
    let i = open(&mut f, "lock", "保险箱");
    for _ in 0..2 {
        let r = interaction::act(&mut f.e.world, &f.c, &i.interaction_id, &PlayerOp { action: "code".into(), input: json!("1111"), ..Default::default() }).unwrap();
        assert!(!r.finished);
    }
    let r = interaction::act(&mut f.e.world, &f.c, &i.interaction_id, &PlayerOp { action: "code".into(), input: json!("2222"), ..Default::default() }).unwrap();
    assert!(r.finished && !r.interaction.result.as_ref().unwrap().success, "输错三次触发警报");
    let loc = worldsim::party_location(f.e.world.conn(), &f.c).unwrap().unwrap();
    assert_eq!(interaction::alert(f.e.world.conn(), &f.c, &loc).level, 3);
    // 新的一次，正确密码：箱子里的东西归玩家。
    let i = open(&mut f, "lock", "保险箱");
    let r = interaction::act(&mut f.e.world, &f.c, &i.interaction_id, &PlayerOp { action: "code".into(), input: json!("0427"), ..Default::default() }).unwrap();
    assert!(r.interaction.result.unwrap().success);
    assert_eq!(qty(&f, &f.coin.clone()), 2);
}

#[test]
fn picking_uses_the_rules_and_can_break_tools() {
    let mut f = fx();
    { let k = f.pick.clone(); give(&mut f, &k); }
    let i = open(&mut f, "lock", "地窖门");
    // 大失败：撬锁器断了，警戒上升，可以再试。
    let r = interaction::act(&mut f.e.world, &f.c, &i.interaction_id, &op("pick", Some(1))).unwrap();
    assert!(!r.finished && r.roll.is_some());
    assert_eq!(qty(&f, &f.pick.clone()), 0, "大失败弄断工具");
    // 没有工具也可以凑合（难度 +3），掷出 20 照样开。
    let r = interaction::act(&mut f.e.world, &f.c, &i.interaction_id, &op("pick", Some(20))).unwrap();
    assert!(r.finished && r.interaction.result.unwrap().success);
    // 掷骰写进事件日志（亲手掷）。
    let rolls = campaign::recent_events(f.e.world.conn(), &f.c, &["roll"], 5).unwrap();
    assert!(rolls.iter().any(|e| e.payload["manual"].as_bool() == Some(true)));
}

#[test]
fn investigation_finds_contents_and_hidden_things_once() {
    let mut f = fx();
    let i = open(&mut f, "investigate", "旧木箱");
    let r = interaction::act(&mut f.e.world, &f.c, &i.interaction_id, &op("look", Some(18))).unwrap();
    assert!(r.finished);
    assert_eq!(qty(&f, &f.coin.clone()), 1, "没锁的箱子搜到东西");
    assert!(r.lines.iter().any(|l| l.contains("墙缝")), "检定总值够高时发现隐藏对象：{:?}", r.lines);
    // 同样的检查做第二遍：没有新结果（不能反复掷骰刷）。
    let i = open(&mut f, "investigate", "旧木箱");
    let r = interaction::act(&mut f.e.world, &f.c, &i.interaction_id, &op("look", Some(20))).unwrap();
    assert!(!r.interaction.result.unwrap().success && r.roll.is_none());
    assert_eq!(qty(&f, &f.coin.clone()), 1);
    // 玩家视角：隐藏的墙缝现在能看到了。
    let here = interaction::objects_here(f.e.world.conn(), &f.c, false).unwrap();
    assert!(here.iter().any(|o| o.object.name == "墙缝"));
}

#[test]
fn puzzle_state_persists_through_timeline() {
    let mut f = fx();
    let i = open(&mut f, "puzzle", "三根拉杆");
    for a in ["op:a", "op:b"] {
        let r = interaction::act(&mut f.e.world, &f.c, &i.interaction_id, &op(a, None)).unwrap();
        assert!(!r.finished);
    }
    let r = interaction::act(&mut f.e.world, &f.c, &i.interaction_id, &op("op:c", None)).unwrap();
    assert!(r.finished && r.interaction.result.unwrap().success, "{:?}", r.lines);
    let key = r.interaction.target.clone();
    assert!(interaction::object_state(f.e.world.conn(), &f.c, &key).solved);
    let cp = timeline::checkpoint(f.e.world.conn(), &f.c, "测试").unwrap();
    assert!(timeline::verify(f.e.world.conn(), &f.c).unwrap().ok);
    let b = timeline::fork(&mut f.e.world, &f.c, cp, "", false).unwrap();
    assert!(interaction::object_state(f.e.world.conn(), &b.id, &key).solved, "分叉后机关仍是解开的");
}

#[test]
fn pending_interaction_survives_reopen() {
    let mut f = fx();
    let i = open(&mut f, "lock", "地窖门");
    let path = f.e.path();
    let backups = f.e.backups();
    let c = f.c.clone();
    drop(f.e.world);
    let (w, _) = World::open(&path, &backups).unwrap();
    let p = interaction::pending(w.conn(), &c).unwrap();
    assert_eq!(p.len(), 1, "程序重启后待处理的互动还在");
    assert_eq!(p[0].interaction_id, i.interaction_id);
}

#[test]
fn item_use_heals_and_consumes() {
    let mut f = fx();
    { let k = f.potion.clone(); give(&mut f, &k); }
    let mut s = rpg::sheet(f.e.world.conn(), &f.c, &rpg::actor()).unwrap();
    s.hp = 1;
    rpg::save_sheet(&mut f.e.world, &f.c, &s, "受伤").unwrap();
    let i = open(&mut f, "item_use", "");
    let r = interaction::act(&mut f.e.world, &f.c, &i.interaction_id, &PlayerOp { action: "use".into(), input: json!({ "item": f.potion, "on": "self" }), ..Default::default() }).unwrap();
    assert!(r.interaction.result.unwrap().success);
    assert_eq!(qty(&f, &f.potion.clone()), 0);
    assert!(rpg::sheet(f.e.world.conn(), &f.c, &rpg::actor()).unwrap().hp > 1);
    // 不在身上的东西用不了。
    let i = open(&mut f, "item_use", "");
    assert!(interaction::act(&mut f.e.world, &f.c, &i.interaction_id, &PlayerOp { action: "use".into(), input: json!({ "item": f.potion, "on": "self" }), ..Default::default() }).is_err());
}

#[test]
fn stealth_failures_raise_alert_until_discovered() {
    let mut f = fx();
    let i = open(&mut f, "stealth", "");
    let r = interaction::act(&mut f.e.world, &f.c, &i.interaction_id, &op("sneak", Some(1))).unwrap();
    assert!(!r.finished);
    let r = interaction::act(&mut f.e.world, &f.c, &i.interaction_id, &op("sneak", Some(1))).unwrap();
    assert!(r.finished && r.interaction.result.unwrap().degree == "被发现", "两次大失败：被发现");
}

#[test]
fn results_are_handed_to_narration_once() {
    let mut f = fx();
    { let k = f.key.clone(); give(&mut f, &k); }
    let i = open(&mut f, "lock", "地窖门");
    interaction::act(&mut f.e.world, &f.c, &i.interaction_id, &op("key", None)).unwrap();
    let r = interaction::take_results(&mut f.e.world, &f.c).unwrap();
    assert_eq!(r.len(), 1);
    assert!(r[0].contains("地窖门"));
    assert!(interaction::take_results(&mut f.e.world, &f.c).unwrap().is_empty(), "只交付一次");
}

#[test]
fn gm_proposed_objects_cannot_conjure_items() {
    let mut f = fx();
    let o = interaction::add_scene_object(
        &mut f.e.world,
        &f.c,
        interaction::SceneObject { name: "墙角的麻袋".into(), kind: "container".into(), contents: vec![interaction::Content { item: f.coin.clone(), qty: 99 }], ..Default::default() },
    )
    .unwrap();
    assert!(o.contents.is_empty(), "GM 提出的对象里不能凭空放世界物品");
    assert_eq!(o.source, "gm");
}

#[test]
fn timeout_by_actions() {
    let mut f = fx();
    let i = interaction::open(
        &mut f.e.world,
        &f.c,
        &OpenRequest { kind: "puzzle".into(), target: "三根拉杆".into(), timeout: Some(interaction::Timeout { kind: "actions".into(), amount: 2, started: 0, reason: "水位在上涨".into() }), ..Default::default() },
    )
    .unwrap();
    interaction::act(&mut f.e.world, &f.c, &i.interaction_id, &op("op:a", None)).unwrap();
    let r = interaction::act(&mut f.e.world, &f.c, &i.interaction_id, &op("op:a", None)).unwrap();
    assert!(r.finished && r.interaction.status == "expired");
    // 没写原因的限时不启用。
    let i = interaction::open(&mut f.e.world, &f.c, &OpenRequest { kind: "puzzle".into(), target: "三根拉杆".into(), timeout: Some(interaction::Timeout { kind: "real".into(), amount: 5, started: 0, reason: String::new() }), ..Default::default() }).unwrap();
    assert_eq!(i.timeout_policy.kind, "none");
}

#[test]
fn free_action_success_changes_the_object_and_drops_the_card() {
    let mut f = fx();
    let i = open(&mut f, "lock", "地窖门");
    let lines = interaction::change_object(&mut f.e.world, &f.c, "地窖门", "break", "用桌腿撞开").unwrap();
    assert!(lines.iter().any(|l| l.contains("打开")));
    let s = interaction::object_state(f.e.world.conn(), &f.c, &i.target);
    assert!(s.opened && s.broken);
    assert!(interaction::pending(f.e.world.conn(), &f.c).unwrap().is_empty(), "用别的办法解决后，开锁卡片放弃");
}

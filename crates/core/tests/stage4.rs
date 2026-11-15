//! 第四阶段：存档 / 读档（进行中的战役也能从存档点开新时间线）、难度、因果与延迟后果、剧情阶段与伏笔、NPC 与世界模拟。

mod common;

use chronicle_core::*;
use common::*;
use serde_json::json;

#[test]
fn named_and_quick_saves_load_into_new_timelines_while_original_continues() {
    let mut e = env();
    let w = &mut e.world;
    let c = campaign::create_campaign(w, None, "主线", Some(5)).unwrap();
    campaign::apply_effects(w, &c.id, &[rules::Effect::SetVar { key: "gold_found".into(), value: json!(1) }], "x").unwrap();
    let save = timeline::save_point(w, &c.id, "进洞之前").unwrap();
    assert_eq!(save.label, "save:进洞之前");
    campaign::apply_effects(w, &c.id, &[rules::Effect::SetVar { key: "gold_found".into(), value: json!(2) }], "x").unwrap();
    timeline::save_point(w, &c.id, "").unwrap();
    campaign::apply_effects(w, &c.id, &[rules::Effect::SetVar { key: "gold_found".into(), value: json!(3) }], "x").unwrap();
    // 读档（战役还在进行中）：从存档点开出新时间线，状态回到当时。
    let b = timeline::fork(w, &c.id, save.id, "读档", false).unwrap();
    let var = |w: &World, id: &str| campaign::campaign_state(w.conn(), id).unwrap().vars.get("gold_found").cloned();
    assert_eq!(var(w, &b.id), Some(json!(1)));
    assert_eq!(var(w, &c.id), Some(json!(3)), "原时间线不变，还能继续");
    assert_eq!(campaign::get_campaign(w.conn(), &c.id).unwrap().status, "active");
    let q = timeline::last_quicksave(w.conn(), &c.id).unwrap().unwrap();
    let b2 = timeline::fork(w, &c.id, q.id, "快读", false).unwrap();
    assert_eq!(var(w, &b2.id), Some(json!(2)));
    // 结束的时间线不能再存档。
    timeline::end_campaign(w, &c.id, "完").unwrap();
    assert!(timeline::save_point(w, &c.id, "x").is_err());
}

#[test]
fn difficulty_adjusts_checks_and_enemies_explainably_and_is_protected() {
    let mut e = env();
    let w = &mut e.world;
    let c = campaign::create_campaign(w, None, "x", Some(2)).unwrap();
    let mut r = rpg::rules(w.conn(), &c.id);
    assert_eq!(rpg::difficulty_dc(&r, 12), (12, String::new()), "普通难度不调整");
    r.difficulty = "hard".into();
    let (dc, note) = rpg::difficulty_dc(&r, 12);
    assert_eq!(dc, 14);
    assert!(note.contains("困难") && note.contains("+2"));
    assert_eq!(rpg::difficulty_level(&r, 3), 4);
    r.check.mode = "roll_under".into();
    r.check.die = "1d100".into();
    assert_eq!(rpg::difficulty_dc(&r, 50).0, 40, "百分骰：越难目标值越低");
    r.check = Default::default();
    rpg::set_rules(w, &c.id, &r).unwrap();
    assert_eq!(rpg::rules(w.conn(), &c.id).difficulty, "hard");
    r.difficulty = "insane".into();
    assert!(rpg::set_rules(w, &c.id, &r).is_err());
}

#[test]
fn consequences_become_due_lock_when_occurred_and_follow_timelines() {
    let mut e = env();
    let w = &mut e.world;
    let c = campaign::create_campaign(w, None, "x", Some(2)).unwrap();
    let k = story::add_consequence(w, &c.id, "放走了强盗头子", "他会带着同伙回来报复", 3).unwrap();
    assert!(story::due(w.conn(), &c.id).unwrap().is_empty(), "还没到期");
    let save = timeline::save_point(w, &c.id, "放走之后").unwrap();
    for _ in 0..3 {
        w.conn().execute("UPDATE campaigns SET turn = turn + 1 WHERE id = ?1", [&c.id]).unwrap();
    }
    let due = story::due(w.conn(), &c.id).unwrap();
    assert_eq!(due.len(), 1);
    let (text, refs) = story::gm_text(w.conn(), &c.id).unwrap();
    assert!(text.contains("K1") && text.contains("报复"));
    assert_eq!(refs[0].1, k.id);
    story::mark_occurred(w, &c.id, &k.id).unwrap();
    let all = story::consequences(w.conn(), &c.id).unwrap();
    assert_eq!(all[0].status, "occurred");
    assert!(story::due(w.conn(), &c.id).unwrap().is_empty());
    // 读档回到放走之前：那条时间线里后果还没发生。
    let b = timeline::fork(w, &c.id, save.id, "读档", false).unwrap();
    assert_eq!(story::consequences(w.conn(), &b.id).unwrap()[0].status, "pending");
    // 伏笔。
    let f = story::plant(w, &c.id, "井底传来的歌声").unwrap();
    story::pay_off(w, &c.id, &f.id, "歌声来自被困的水妖").unwrap();
    assert_eq!(story::foreshadows(w.conn(), &c.id).unwrap()[0].status, "paid");
    // 剧情阶段。
    let ph = story::plot_phase(w.conn(), &c.id, 20).unwrap();
    assert!(["setup", "development"].contains(&ph.phase.as_str()), "{ph:?}");
}

#[test]
fn daily_npc_sim_meetings_disappearance_collapse_and_defection() {
    let mut e = env();
    let w = &mut e.world;
    let mk = |w: &mut World, n: NewNode| w.write(&user(), "n", |x| x.create_node(n)).unwrap().0.id;
    let town = mk(w, NewNode::new(NodeKind::City, "白石城"));
    let tavern = mk(w, NewNode::new(NodeKind::Site, "酒馆").under(&town));
    let sched = |loc: &str| json!({"schedule": [{"start": "00:00", "end": "23:59", "activity": "在酒馆", "location": loc}]});
    let a = mk(w, NewNode::new(NodeKind::Character, "阿岩").payload(sched(&tavern)));
    let b = mk(w, NewNode::new(NodeKind::Character, "老格").payload(sched(&tavern)));
    let c = mk(w, NewNode::new(NodeKind::Character, "胆小鬼").payload(sched(&tavern)));
    let guild = mk(w, NewNode::new(NodeKind::Faction, "破落商会").payload(json!({"power": {"wealth": 10, "military": 3, "influence": 10, "stability": 2}})));
    w.write(&user(), "r", |x| x.create_relation(NewRelation::new(&a, &guild, "member_of"))).unwrap();
    let camp = campaign::create_campaign(w, None, "x", Some(9)).unwrap();
    campaign::apply_effects(w, &camp.id, &[rules::Effect::AdjustMood { npc: c.clone(), fear: 95, anger: 0, suspicion: 0, stress: 0, loyalty: 0, reason: "被吓坏了".into() }], "x").unwrap();
    // 推进一天。
    worldsim::simulate(w, &camp.id, 8 * 60, 1440 + 8 * 60).unwrap();
    let st = campaign::campaign_state(w.conn(), &camp.id).unwrap();
    assert_eq!(st.entities.get(&format!("{c}|life_status")), Some(&json!("missing")), "极度恐惧的人逃走失踪");
    assert!(npcsim::is_collapsed(w.conn(), &camp.id, &guild), "稳定和军力都很低的势力瓦解");
    assert_eq!(st.relations.get(&format!("{a}|{guild}|member_of")), Some(&false), "瓦解后成员离开");
    let view = campaign::CampaignView { conn: w.conn(), campaign_id: &camp.id };
    use rules::StateView;
    assert!(view.bond(&a, &b).is_some(), "同在酒馆的人碰面，关系有变化");
    let events = worldsim::world_events(w.conn(), &camp.id, 20).unwrap();
    assert!(events.iter().any(|e| e.kind == "collapse") && events.iter().any(|e| e.kind == "disappear"));
    // 再推进一天：已经瓦解的势力不再重复瓦解。
    worldsim::simulate(w, &camp.id, 1440 + 8 * 60, 2 * 1440 + 8 * 60).unwrap();
    assert_eq!(worldsim::world_events(w.conn(), &camp.id, 50).unwrap().iter().filter(|e| e.kind == "collapse").count(), 1);
}

#[test]
fn npc_memories_fade_over_turns() {
    let m = |imp: i64, turn: i64| campaign::NpcMemory { id: format!("{imp}-{turn}"), npc_id: "n".into(), content: String::new(), importance: imp, turn, created_at: 0 };
    let out = npcsim::decayed(vec![m(2, 0), m(5, 0), m(1, 18)], 20);
    assert_eq!(out.iter().map(|x| x.importance).collect::<Vec<_>>(), vec![3, 1], "20 回合前的记忆重要度 -2；淡到 0 的不再想起");
}

#[test]
fn collapsed_faction_leaves_its_places_without_ruler() {
    let mut e = env();
    let w = &mut e.world;
    let mk = |w: &mut World, n: NewNode| w.write(&user(), "n", |x| x.create_node(n)).unwrap().0.id;
    let town = mk(w, NewNode::new(NodeKind::City, "白石城"));
    let guild = mk(w, NewNode::new(NodeKind::Faction, "破落商会").payload(json!({"power": {"wealth": 10, "military": 2, "influence": 10, "stability": 2}})));
    w.write(&user(), "r", |x| x.create_relation(NewRelation::new(&guild, &town, "rules"))).unwrap();
    let c = campaign::create_campaign(w, None, "x", Some(1)).unwrap();
    assert_eq!(worldsim::rulers(w.conn(), &c.id).unwrap().get(&town), Some(&guild));
    worldsim::simulate(w, &c.id, 8 * 60, 1440 + 8 * 60).unwrap();
    assert!(!worldsim::rulers(w.conn(), &c.id).unwrap().contains_key(&town), "瓦解后没人统治");
    assert!(worldsim::world_events(w.conn(), &c.id, 20).unwrap().iter().any(|e| e.kind == "place_change"));
}

#[test]
fn moving_without_progress_is_refused_and_costs_nothing() {
    let mut e = env();
    let w = &mut e.world;
    let c = campaign::create_campaign(w, None, "x", Some(3)).unwrap();
    let mut p = rpg::sheet(w.conn(), &c.id, rpg::PARTY).unwrap();
    p.max_hp = 300;
    p.hp = 300;
    rpg::save_sheet(w, &c.id, &p, "x").unwrap();
    let cb = combat::start(w, &c.id, &[combat::EnemySpec { name: "木桩".into(), npc: None, level: 1, count: 1, loot: vec![], boss: false }], &[], "练习").unwrap().combat;
    let foe = cb.units.iter().find(|u| u.side == "enemy").unwrap().id.clone();
    let mv = combat::CombatAction { kind: "move".into(), target: foe.clone(), ..Default::default() };
    // 一直靠近直到贴身（或行动点用完）。
    for _ in 0..10 {
        let Some(now) = combat::active(w.conn(), &c.id) else { return };
        if !now.players_turn() {
            combat::act(w, &c.id, &combat::CombatAction { kind: "end_turn".into(), ..Default::default() }).ok();
            continue;
        }
        let me = now.units.iter().find(|u| u.side == "party").unwrap();
        let f = now.units.iter().find(|u| u.id == foe).unwrap();
        if combat::dist(me, f) <= 1 {
            let ap = me.ap;
            let err = combat::act(w, &c.id, &mv).unwrap_err().to_string();
            assert!(err.contains("没法再靠近"), "{err}");
            let after = combat::active(w.conn(), &c.id).unwrap();
            assert_eq!(after.units.iter().find(|u| u.side == "party").unwrap().ap, ap, "没有移动就不消耗行动点");
            return;
        }
        if combat::act(w, &c.id, &mv).is_err() {
            combat::act(w, &c.id, &combat::CombatAction { kind: "end_turn".into(), ..Default::default() }).ok();
        }
    }
    panic!("没能靠近目标");
}

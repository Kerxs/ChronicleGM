mod common;

use chronicle_core::campaign::*;
use chronicle_core::rules::{StateView, Tri};
use chronicle_core::*;
use common::*;
use serde_json::json;

struct Story {
    script: String,
    start: String,
    edge_persuade: String,
    edge_bribe: String,
    edge_once: String,
    npc: String,
}

/// 开场 →(说服检定) 接受委托；开场 →(需要 10 金币，花掉) 接受委托；开场 →(只能一次) 自由行动 → 开场
fn build(e: &mut common::Env) -> Story {
    let npc = mk(&mut e.world, NewNode::new(NodeKind::Character, "委托人"));
    let (s, _) = e.world.write(&user(), "s", |w| w.create_script(NewScript::new("遗迹"))).unwrap();
    let node = |e: &mut common::Env, k, t: &str| {
        e.world.write(&user(), "n", |w| w.create_plot_node(NewPlotNode::new(&s.id, k, t))).unwrap().0.id
    };
    let start = node(e, PlotNodeKind::Start, "开场");
    let accept = node(e, PlotNodeKind::Scene, "接受委托");
    let free = node(e, PlotNodeKind::Dynamic, "自由行动");
    let edge = |e: &mut common::Env, from: &str, to: &str, cond: serde_json::Value, eff: serde_json::Value, once: bool| {
        let mut ed = NewPlotEdge::new(&s.id, from, to);
        ed.condition = serde_json::from_value(cond).unwrap();
        ed.effects = serde_json::from_value(eff).unwrap();
        ed.once = once;
        e.world.write(&user(), "e", |w| w.create_plot_edge(ed)).unwrap().0.id
    };
    let npc_id = npc.id.clone();
    let edge_persuade = edge(
        e,
        &start,
        &accept,
        json!({"type":"check","skill":"persuasion","dc":12}),
        json!([{"type":"set_quest_state","quest":"Q-ruins","state":"active"},
               {"type":"set_relation","a":"party","b":npc_id,"rel_type":"trusts"}]),
        false,
    );
    let edge_bribe = edge(
        e,
        &start,
        &accept,
        json!({"type":"var","key":"gold","op":"ge","value":10}),
        json!([{"type":"inc_var","key":"gold","by":-10},{"type":"set_quest_state","quest":"Q-ruins","state":"active"}]),
        false,
    );
    let edge_once = edge(
        e,
        &start,
        &free,
        serde_json::Value::Null,
        json!([{"type":"set_entity_state","entity":npc_id,"field":"life_status","value":"dead"},
               {"type":"reveal_clue","clue":"C-map"}]),
        true,
    );
    edge(e, &free, &start, serde_json::Value::Null, json!([]), false);
    Story { script: s.id, start, edge_persuade, edge_bribe, edge_once, npc: npc.id }
}

#[test]
fn choices_change_state_and_canon_is_untouched() {
    let mut e = env();
    let st = build(&mut e);
    let c = create_campaign(&mut e.world, Some(&st.script), "第一团", Some(42)).unwrap();
    assert_eq!(c.current_plot_node.as_deref(), Some(st.start.as_str()));

    // 预演：检定未知，金币不足，自由行动可走。
    let opts = available_edges(&e.world, &c.id).unwrap();
    let avail = |id: &str| opts.iter().find(|o| o.edge.id == id).unwrap().availability;
    assert_eq!(avail(&st.edge_persuade), Tri::Unknown);
    assert_eq!(avail(&st.edge_bribe), Tri::False);
    assert_eq!(avail(&st.edge_once), Tri::True);

    // 金币不足时强行走贿赂边：条件不满足，不移动，状态不变。
    let out = take_edge(&mut e.world, &c.id, &st.edge_bribe).unwrap();
    assert!(!out.taken);
    assert_eq!(get_campaign(e.world.conn(), &c.id).unwrap().current_plot_node.as_deref(), Some(st.start.as_str()));

    // 走自由行动：NPC 在战役中死亡，但世界正典不变。
    let out = take_edge(&mut e.world, &c.id, &st.edge_once).unwrap();
    assert!(out.taken);
    let ov = entity_overrides(e.world.conn(), &c.id, &st.npc).unwrap();
    assert_eq!(ov.get("life_status"), Some(&json!("dead")));
    assert_eq!(get(&e.world, &st.npc).unwrap().payload, json!({}), "正典未被修改");
    let view = CampaignView { conn: e.world.conn(), campaign_id: &c.id };
    assert!(view.clue_known("C-map"));

    // 回到开场后，once 边已用完。
    let back = available_edges(&e.world, &c.id).unwrap()[0].edge.id.clone();
    take_edge(&mut e.world, &c.id, &back).unwrap();
    let opts = available_edges(&e.world, &c.id).unwrap();
    let once = opts.iter().find(|o| o.edge.id == st.edge_once).unwrap();
    assert!(once.used_up && once.availability == Tri::False);
    assert!(take_edge(&mut e.world, &c.id, &st.edge_once).is_err());

    // 获得金币后贿赂：扣钱、任务激活。
    apply_effects(&mut e.world, &c.id, &serde_json::from_value::<Vec<_>>(json!([{"type":"inc_var","key":"gold","by":15}])).unwrap(), "奖励").unwrap();
    let out = take_edge(&mut e.world, &c.id, &st.edge_bribe).unwrap();
    assert!(out.taken);
    let view = CampaignView { conn: e.world.conn(), campaign_id: &c.id };
    assert_eq!(view.var("gold"), Some(json!(5)));
    assert_eq!(view.quest_state("Q-ruins").as_deref(), Some("active"));
    assert_eq!(view.visit_count(&st.start), 2);

    // 事件日志完整记录且只能追加。
    let log = events(e.world.conn(), &c.id, 0, 100).unwrap();
    let kinds: Vec<_> = log.iter().map(|x| x.kind.as_str()).collect();
    assert_eq!(kinds, ["transition_failed", "transition", "transition", "effects", "transition"]);
    assert!(e.world.conn().execute("UPDATE event_log SET kind = 'x'", []).is_err());
    assert!(e.world.conn().execute("DELETE FROM event_log", []).is_err());
}

#[test]
fn checks_are_rolled_by_program_and_reproducible() {
    let run = |seed: i64| {
        let mut e = env();
        let st = build(&mut e);
        let c = create_campaign(&mut e.world, Some(&st.script), "团", Some(seed)).unwrap();
        let mut results = vec![];
        for _ in 0..5 {
            let out = take_edge(&mut e.world, &c.id, &st.edge_persuade).unwrap();
            results.push((out.checks[0].roll.total, out.taken));
            if out.taken {
                break;
            }
        }
        let r = roll(&mut e.world, &c.id, "3d6", "测试").unwrap();
        (results, r.total)
    };
    assert_eq!(run(7), run(7), "同一种子结果相同");
    // 修正值参与计算。
    let mut e = env();
    let st = build(&mut e);
    let c = create_campaign(&mut e.world, Some(&st.script), "团", Some(1)).unwrap();
    apply_effects(
        &mut e.world,
        &c.id,
        &serde_json::from_value::<Vec<_>>(json!([{"type":"set_var","key":"skill.party.persuasion","value":30}])).unwrap(),
        "加点",
    )
    .unwrap();
    let out = take_edge(&mut e.world, &c.id, &st.edge_persuade).unwrap();
    assert!(out.taken);
    assert_eq!(out.checks[0].modifier, 30);
    assert_eq!(out.checks[0].total, out.checks[0].roll.total + 30);
    let view = CampaignView { conn: e.world.conn(), campaign_id: &c.id };
    assert!(view.relation("party", &st.npc, "trusts"));
}

#[test]
fn failing_effects_roll_back_whole_transition() {
    let mut e = env();
    let (s, _) = e.world.write(&user(), "s", |w| w.create_script(NewScript::new("x"))).unwrap();
    let a = e.world.write(&user(), "n", |w| w.create_plot_node(NewPlotNode::new(&s.id, PlotNodeKind::Start, "a"))).unwrap().0;
    let b = e.world.write(&user(), "n", |w| w.create_plot_node(NewPlotNode::new(&s.id, PlotNodeKind::Scene, "b"))).unwrap().0;
    let mut ed = NewPlotEdge::new(&s.id, &a.id, &b.id);
    ed.effects = serde_json::from_value(json!([{"type":"set_var","key":"touched","value":true},
        {"type":"take_item","item":"key"}])).unwrap();
    let edge = e.world.write(&user(), "e", |w| w.create_plot_edge(ed)).unwrap().0;
    let c = create_campaign(&mut e.world, Some(&s.id), "团", Some(1)).unwrap();
    assert!(take_edge(&mut e.world, &c.id, &edge.id).is_err());
    let after = get_campaign(e.world.conn(), &c.id).unwrap();
    assert_eq!(after.current_plot_node.as_deref(), Some(a.id.as_str()));
    assert_eq!(after.turn, 0);
    let view = CampaignView { conn: e.world.conn(), campaign_id: &c.id };
    assert_eq!(view.var("touched"), None);
    assert!(events(e.world.conn(), &c.id, 0, 10).unwrap().is_empty());
}

#[test]
fn spawn_requires_applied_changeset_and_npc_memory() {
    let mut e = env();
    let npc = mk(&mut e.world, NewNode::new(NodeKind::Character, "守卫"));
    let c = create_campaign(&mut e.world, None, "沙盒", Some(3)).unwrap();
    let spawn = serde_json::from_value::<Vec<_>>(json!([{"type":"spawn","changeset":"NOPE"}])).unwrap();
    assert!(apply_effects(&mut e.world, &c.id, &spawn, "x").is_err());

    add_npc_memory(&e.world, &c.id, &npc.id, "玩家帮他找回了钥匙", 8).unwrap();
    add_npc_memory(&e.world, &c.id, &npc.id, "玩家问过路", 1).unwrap();
    let mem = npc_memories(e.world.conn(), &c.id, &npc.id, 10).unwrap();
    assert_eq!(mem[0].content, "玩家帮他找回了钥匙");
    let city = mk(&mut e.world, NewNode::new(NodeKind::City, "城"));
    assert!(add_npc_memory(&e.world, &c.id, &city.id, "x", 1).is_err());
}

mod common;

use chronicle_core::plot::*;
use chronicle_core::scripts::*;
use chronicle_core::*;
use common::*;
use serde_json::json;

#[test]
fn script_scale_profile_tracks_players_and_hours() {
    let small = scale_profile(2, 2.0);
    let big = scale_profile(6, 12.0);
    assert!(big.scenes.min > small.scenes.max);
    assert!(big.npcs.max > small.npcs.max);
    assert_eq!(small.hidden_quests.min, 0);
    assert!(big.endings.max >= 5);
    for p in [&small, &big] {
        for r in [p.scenes, p.main_quests, p.side_quests, p.hidden_quests, p.npcs, p.clues, p.endings] {
            assert!(r.min <= r.max);
        }
    }
    // 同样的输入得到同样的结果。
    assert_eq!(scale_profile(4, 4.0), scale_profile(4, 4.0));

    let mut e = env();
    let mut s = NewScript::new("雾港疑案");
    s.player_count = 3;
    s.duration_hours = 3.0;
    let (sc, _) = e.world.write(&user(), "s", |w| w.create_script(s)).unwrap();
    let stored: ScaleProfile = serde_json::from_value(sc.scale_profile.clone()).unwrap();
    assert_eq!(stored, scale_profile(3, 3.0));
    let (sc2, _) = e
        .world
        .write(&user(), "u", |w| w.update_script(&sc.id, ScriptPatch { player_count: Some(6), ..Default::default() }, Some(1)))
        .unwrap();
    assert_eq!(serde_json::from_value::<ScaleProfile>(sc2.scale_profile).unwrap(), scale_profile(6, 3.0));
    let mut bad = NewScript::new("x");
    bad.player_count = 0;
    assert!(e.world.write(&user(), "s", |w| w.create_script(bad)).is_err());
}

#[test]
fn script_items_quests_and_cast() {
    let mut e = env();
    let npc = mk(&mut e.world, NewNode::new(NodeKind::Character, "码头老板"));
    let city = mk(&mut e.world, NewNode::new(NodeKind::City, "雾港"));
    let (s, _) = e.world.write(&user(), "s", |w| w.create_script(NewScript::new("雾港疑案"))).unwrap();
    let (ch, _) = e.world.write(&user(), "c", |w| w.create_script_item(NewScriptItem::new(&s.id, ScriptItemKind::Chapter, "第一章"))).unwrap();
    let mut q = NewScriptItem::new(&s.id, ScriptItemKind::Quest, "失踪的货船");
    q.parent_id = Some(ch.id.clone());
    q.payload = json!({"quest_type":"hidden","objectives":["找到货船"]});
    e.world.write(&user(), "q", |w| w.create_script_item(q)).unwrap();
    let mut bad = NewScriptItem::new(&s.id, ScriptItemKind::Quest, "坏任务");
    bad.payload = json!({});
    assert!(e.world.write(&user(), "q", |w| w.create_script_item(bad)).is_err());
    assert_eq!(item_children(e.world.conn(), &s.id, Some(&ch.id)).unwrap().len(), 1);
    assert_eq!(items_by_kind(e.world.conn(), &s.id, ScriptItemKind::Quest).unwrap().len(), 1);

    let cast = NewCastMember {
        script_id: s.id.clone(),
        node_id: npc.id.clone(),
        role: "线人".into(),
        motive: "还债".into(),
        secret: "货船是他卖掉的".into(),
        relation_to_players: "雇主".into(),
        payload: json!({}),
    };
    e.world.write(&user(), "cast", |w| w.add_cast(cast.clone())).unwrap();
    let not_char = NewCastMember { node_id: city.id.clone(), ..cast };
    assert!(e.world.write(&user(), "cast", |w| w.add_cast(not_char)).is_err());
    assert_eq!(cast_of(e.world.conn(), &s.id).unwrap().len(), 1);

    // 删除章节会连同子条目一起删除。
    let (n, _) = e.world.write(&user(), "d", |w| w.delete_script_item(&ch.id, None)).unwrap();
    assert_eq!(n, 2);
    assert!(items_by_kind(e.world.conn(), &s.id, ScriptItemKind::Quest).unwrap().is_empty());
}

#[test]
fn plot_graph_rules() {
    let mut e = env();
    let (s, _) = e.world.write(&user(), "s", |w| w.create_script(NewScript::new("A"))).unwrap();
    let (other, _) = e.world.write(&user(), "s", |w| w.create_script(NewScript::new("B"))).unwrap();
    let mk_node = |e: &mut common::Env, sid: &str, kind, t: &str| {
        e.world.write(&user(), "n", |w| w.create_plot_node(NewPlotNode::new(sid, kind, t))).unwrap().0
    };
    let start = mk_node(&mut e, &s.id, PlotNodeKind::Start, "开场");
    let accept = mk_node(&mut e, &s.id, PlotNodeKind::Scene, "接受委托");
    let end = mk_node(&mut e, &s.id, PlotNodeKind::Ending, "真相");
    let foreign = mk_node(&mut e, &other.id, PlotNodeKind::Scene, "别的剧本");

    let mut edge = NewPlotEdge::new(&s.id, &start.id, &accept.id);
    edge.condition = Some(serde_json::from_value(json!({"type":"flag","name":"x"})).unwrap());
    edge.effects = serde_json::from_value(json!([{"type":"set_quest_state","quest":"Q","state":"active"}])).unwrap();
    e.world.write(&user(), "e", |w| w.create_plot_edge(edge)).unwrap();
    e.world.write(&user(), "e", |w| w.create_plot_edge(NewPlotEdge::new(&s.id, &accept.id, &end.id))).unwrap();
    // 环是允许的（例如回到开场）。
    e.world.write(&user(), "e", |w| w.create_plot_edge(NewPlotEdge::new(&s.id, &accept.id, &start.id))).unwrap();
    // 结局不能有出边；不能跨剧本连线。
    assert!(e.world.write(&user(), "e", |w| w.create_plot_edge(NewPlotEdge::new(&s.id, &end.id, &start.id))).is_err());
    assert!(e.world.write(&user(), "e", |w| w.create_plot_edge(NewPlotEdge::new(&s.id, &start.id, &foreign.id))).is_err());
    // 非法效果被拒绝。
    let mut bad = NewPlotEdge::new(&s.id, &start.id, &end.id);
    bad.effects = serde_json::from_value(json!([{"type":"give_item","item":"k","qty":-1}])).unwrap();
    assert!(e.world.write(&user(), "e", |w| w.create_plot_edge(bad)).is_err());

    assert_eq!(outgoing(e.world.conn(), &accept.id).unwrap().len(), 2);
    assert_eq!(edges_of_script(e.world.conn(), &s.id).unwrap().len(), 3);
    // 删除节点同时删除相连的边。
    e.world.write(&user(), "d", |w| w.delete_plot_node(&accept.id, None)).unwrap();
    assert_eq!(edges_of_script(e.world.conn(), &s.id).unwrap().len(), 0);
    assert_eq!(plot_nodes_of(e.world.conn(), &s.id).unwrap().len(), 2);
}

//! 剧情分支：分析、模拟，以及"状态 → 条件 → 跳转"端到端验收。

mod common;

use chronicle_core::campaign::*;
use chronicle_core::changeset::{self, Mode, NewChangeset, Op};
use chronicle_core::plot_analysis::analyze_plot;
use chronicle_core::rules::{StateView, Tri};
use chronicle_core::sim::*;
use chronicle_core::*;
use common::*;
use serde_json::json;

fn script(e: &mut Env, title: &str) -> Script {
    e.world.write(&user(), "s", |w| w.create_script(NewScript::new(title))).unwrap().0
}

fn pnode(e: &mut Env, s: &str, k: PlotNodeKind, t: &str) -> PlotNode {
    e.world.write(&user(), "n", |w| w.create_plot_node(NewPlotNode::new(s, k, t))).unwrap().0
}

fn edge(e: &mut Env, s: &str, a: &str, b: &str, cond: serde_json::Value, eff: serde_json::Value) -> PlotEdge {
    let mut ed = NewPlotEdge::new(s, a, b);
    ed.condition = serde_json::from_value(cond).unwrap();
    ed.effects = serde_json::from_value(eff).unwrap();
    e.world.write(&user(), "e", |w| w.create_plot_edge(ed)).unwrap().0
}

#[test]
fn analysis_finds_structural_problems() {
    let mut e = env();
    let s = script(&mut e, "测试");
    let a = pnode(&mut e, &s.id, PlotNodeKind::Scene, "没有开场的场景");
    let r = analyze_plot(e.world.conn(), &s.id).unwrap();
    assert!(r.issues.iter().any(|i| i.code == "no_start"));
    assert!(r.issues.iter().any(|i| i.code == "dead_end" && i.node_ids == vec![a.id.clone()]));

    let start = pnode(&mut e, &s.id, PlotNodeKind::Start, "开场");
    let loop_a = pnode(&mut e, &s.id, PlotNodeKind::Scene, "走廊");
    let loop_b = pnode(&mut e, &s.id, PlotNodeKind::Scene, "大厅");
    let end = pnode(&mut e, &s.id, PlotNodeKind::Ending, "结局");
    edge(&mut e, &s.id, &start.id, &loop_a.id, json!(null), json!([]));
    edge(&mut e, &s.id, &loop_a.id, &loop_b.id, json!(null), json!([]));
    edge(&mut e, &s.id, &loop_b.id, &loop_a.id, json!(null), json!([]));
    let bad = edge(&mut e, &s.id, &loop_b.id, &end.id, json!({"type":"quest_state","quest":"不存在的任务","state":"done"}), json!([{"type":"reveal_clue","clue":"也不存在"}]));
    let r = analyze_plot(e.world.conn(), &s.id).unwrap();
    let codes: Vec<_> = r.issues.iter().map(|i| i.code.as_str()).collect();
    assert!(codes.contains(&"unreachable"), "{codes:?}");
    assert!(codes.contains(&"unconditional_cycle"), "{codes:?}");
    assert_eq!(r.issues.iter().filter(|i| i.code == "bad_reference" && i.edge_ids == vec![bad.id.clone()]).count(), 2);
    assert!(codes.contains(&"few_endings"));
    assert_eq!((r.ending_count, r.reachable_endings), (1, 1));
    assert!(!codes.contains(&"no_start") && !codes.contains(&"no_reachable_ending"));
}

#[test]
fn simulation_does_not_touch_database() {
    let mut e = env();
    let s = script(&mut e, "模拟");
    let start = pnode(&mut e, &s.id, PlotNodeKind::Start, "开场");
    let door = pnode(&mut e, &s.id, PlotNodeKind::Scene, "密室");
    let hall = pnode(&mut e, &s.id, PlotNodeKind::Scene, "大厅");
    let locked = edge(&mut e, &s.id, &start.id, &door.id, json!({"type":"has_item","item":"钥匙"}), json!([{"type":"take_item","item":"钥匙"}]));
    let check = edge(&mut e, &s.id, &start.id, &hall.id, json!({"type":"check","skill":"athletics","dc":15}), json!([{"type":"inc_var","key":"fatigue","by":1}]));
    let commits = e.world.commits(100, 0).unwrap().len();

    let st = sim_start(e.world.conn(), &s.id).unwrap();
    assert_eq!(st.current.as_deref(), Some(start.id.as_str()));
    let opts = sim_options(e.world.conn(), &st).unwrap();
    let av = |id: &str| opts.iter().find(|o| o.edge.id == id).unwrap().availability;
    assert_eq!(av(&locked.id), Tri::False);
    assert_eq!(av(&check.id), Tri::Unknown);

    // 检定强制失败：不移动。强制成功：移动并执行效果。
    let fail = sim_step(e.world.conn(), &st, &check.id, CheckMode::Fail, 1).unwrap();
    assert!(!fail.taken);
    assert_eq!(fail.state.current, st.current);
    let pass = sim_step(e.world.conn(), &st, &check.id, CheckMode::Pass, 1).unwrap();
    assert!(pass.taken);
    assert_eq!(pass.state.current.as_deref(), Some(hall.id.as_str()));
    assert_eq!(pass.state.vars["fatigue"], json!(1));

    // 给模拟状态加上钥匙，密室分支打开；走过后钥匙被消耗。
    let mut with_key = st.clone();
    with_key.items.insert("party|钥匙".into(), 1);
    assert_eq!(sim_options(e.world.conn(), &with_key).unwrap().iter().find(|o| o.edge.id == locked.id).unwrap().availability, Tri::True);
    let r = sim_step(e.world.conn(), &with_key, &locked.id, CheckMode::Roll, 7).unwrap();
    assert!(r.taken);
    assert_eq!(r.state.items["party|钥匙"], 0);
    // 真实掷骰可复现。
    let a = sim_step(e.world.conn(), &st, &check.id, CheckMode::Roll, 42).unwrap();
    let b = sim_step(e.world.conn(), &st, &check.id, CheckMode::Roll, 42).unwrap();
    assert_eq!(a.checks, b.checks);
    assert_eq!(e.world.commits(100, 0).unwrap().len(), commits, "模拟不产生任何写入");
    assert!(list_campaigns(e.world.conn()).unwrap().is_empty());
}

/// 验收：一个按 AI 变更集写入的剧本，玩家的不同选择真正改变状态，并决定之后能走哪些分支、到达哪个结局。
#[test]
fn choices_change_state_and_unlock_different_endings() {
    let mut e = env();
    let elder = mk(&mut e.world, NewNode::new(NodeKind::Character, "村长"));
    let ops: Vec<Op> = serde_json::from_value(json!([
        {"op":"create_script","temp_id":"tmp:s","script":{"title":"遗迹委托"}},
        {"op":"create_script_item","temp_id":"tmp:q","item":{"script_id":"tmp:s","kind":"quest","title":"调查遗迹","payload":{"quest_type":"main"}}},
        {"op":"create_script_item","temp_id":"tmp:clue","item":{"script_id":"tmp:s","kind":"clue","title":"石板上的符文"}},
        {"op":"create_plot_node","temp_id":"tmp:start","node":{"script_id":"tmp:s","kind":"start","title":"剧情开场"}},
        {"op":"create_plot_node","temp_id":"tmp:accept","node":{"script_id":"tmp:s","kind":"choice","title":"接受委托"}},
        {"op":"create_plot_node","temp_id":"tmp:refuse","node":{"script_id":"tmp:s","kind":"scene","title":"拒绝委托"}},
        {"op":"create_plot_node","temp_id":"tmp:ruins","node":{"script_id":"tmp:s","kind":"scene","title":"调查遗迹"}},
        {"op":"create_plot_node","temp_id":"tmp:truth","node":{"script_id":"tmp:s","kind":"ending","title":"找到真相"}},
        {"op":"create_plot_node","temp_id":"tmp:hidden","node":{"script_id":"tmp:s","kind":"ending","title":"触发隐藏事件"}},
        {"op":"create_plot_node","temp_id":"tmp:other","node":{"script_id":"tmp:s","kind":"ending","title":"另一条剧情线"}},
        {"op":"create_plot_edge","edge":{"script_id":"tmp:s","from_id":"tmp:start","to_id":"tmp:accept","label":"接受委托",
            "effects":[{"type":"set_quest_state","quest":"tmp:q","state":"active"},{"type":"set_relation","a":"party","b": elder.id,"rel_type":"trusts"}]}},
        {"op":"create_plot_edge","edge":{"script_id":"tmp:s","from_id":"tmp:start","to_id":"tmp:refuse","label":"拒绝委托",
            "effects":[{"type":"set_flag","name":"refused"}]}},
        {"op":"create_plot_edge","edge":{"script_id":"tmp:s","from_id":"tmp:accept","to_id":"tmp:ruins","label":"前往遗迹",
            "condition":{"type":"quest_state","quest":"tmp:q","state":"active"},"effects":[{"type":"reveal_clue","clue":"tmp:clue"}]}},
        {"op":"create_plot_edge","edge":{"script_id":"tmp:s","from_id":"tmp:ruins","to_id":"tmp:truth","label":"解读符文",
            "condition":{"type":"clue_known","clue":"tmp:clue"},"effects":[{"type":"set_quest_state","quest":"tmp:q","state":"done"}]}},
        {"op":"create_plot_edge","edge":{"script_id":"tmp:s","from_id":"tmp:ruins","to_id":"tmp:hidden","label":"触碰祭坛","priority":1,
            "condition":{"type":"all","of":[{"type":"clue_known","clue":"tmp:clue"},{"type":"relation","a":"party","b": elder.id,"rel_type":"trusts"},{"type":"not","cond":{"type":"flag","name":"refused"}}]}}},
        {"op":"create_plot_edge","edge":{"script_id":"tmp:s","from_id":"tmp:refuse","to_id":"tmp:other","label":"离开村庄",
            "condition":{"type":"flag","name":"refused"}}}
    ]))
    .unwrap();
    let cs = changeset::propose(&mut e.world, NewChangeset { job_id: None, scope: "plot".into(), mode: Mode::Append, author: ai(), title: "剧情".into(), ops }).unwrap();
    assert!(cs.validation.ok, "{:#?}", cs.validation);
    changeset::apply(&mut e.world, &cs.id, None, &user()).unwrap();
    let s = scripts::list_scripts(e.world.conn()).unwrap()[0].clone();
    let report = analyze_plot(e.world.conn(), &s.id).unwrap();
    assert!(report.issues.iter().all(|i| i.code != "bad_reference" && i.code != "dead_end"), "{:#?}", report.issues);
    assert_eq!(report.reachable_endings, 3);

    let edge_by_label = |e: &Env, label: &str| {
        chronicle_core::plot::edges_of_script(e.world.conn(), &s.id).unwrap().into_iter().find(|x| x.label == label).unwrap().id
    };
    let node_title = |e: &Env, c: &str| current_node(&e.world, c).unwrap().unwrap().title;

    // 路线一：接受委托 → 遗迹 → 此时"触碰祭坛"（隐藏结局）因信任关系而可用。
    let c1 = create_campaign(&mut e.world, Some(&s.id), "路线一", Some(1)).unwrap();
    let opts = available_edges(&e.world, &c1.id).unwrap();
    assert!(opts.iter().all(|o| o.availability == Tri::True), "开场两个选择都可选");
    { let id = edge_by_label(&e, "接受委托"); take_edge(&mut e.world, &c1.id, &id) }.unwrap();
    let view = CampaignView { conn: e.world.conn(), campaign_id: &c1.id };
    let quest_id = scripts::items_by_kind(e.world.conn(), &s.id, ScriptItemKind::Quest).unwrap()[0].id.clone();
    assert_eq!(view.quest_state(&quest_id).as_deref(), Some("active"));
    { let id = edge_by_label(&e, "前往遗迹"); take_edge(&mut e.world, &c1.id, &id) }.unwrap();
    let opts = available_edges(&e.world, &c1.id).unwrap();
    assert_eq!(opts[0].edge.label, "触碰祭坛", "高优先级的隐藏分支排在前面");
    assert!(opts.iter().all(|o| o.availability == Tri::True));
    { let id = edge_by_label(&e, "触碰祭坛"); take_edge(&mut e.world, &c1.id, &id) }.unwrap();
    assert_eq!(node_title(&e, &c1.id), "触发隐藏事件");

    // 路线二：拒绝委托 → 只能走"离开村庄"；任务从未激活，遗迹的分支永远不会出现。
    let c2 = create_campaign(&mut e.world, Some(&s.id), "路线二", Some(2)).unwrap();
    { let id = edge_by_label(&e, "拒绝委托"); take_edge(&mut e.world, &c2.id, &id) }.unwrap();
    let view = CampaignView { conn: e.world.conn(), campaign_id: &c2.id };
    assert_eq!(view.quest_state(&quest_id), None);
    assert!(!view.relation("party", &elder.id, "trusts"));
    { let id = edge_by_label(&e, "离开村庄"); take_edge(&mut e.world, &c2.id, &id) }.unwrap();
    assert_eq!(node_title(&e, &c2.id), "另一条剧情线");

    // 路线三：在模拟中构造"拒绝过、但后来又接受"的状态：隐藏结局被 not(refused) 挡住，只能找到真相。
    let mut st = sim_start(e.world.conn(), &s.id).unwrap();
    st.vars.insert("flag.refused".into(), json!(true));
    for label in ["接受委托", "前往遗迹"] {
        st = sim_step(e.world.conn(), &st, &edge_by_label(&e, label), CheckMode::Roll, 3).unwrap().state;
    }
    let opts = sim_options(e.world.conn(), &st).unwrap();
    let av = |label: &str| opts.iter().find(|o| o.edge.label == label).unwrap().availability;
    assert_eq!(av("触碰祭坛"), Tri::False);
    assert_eq!(av("解读符文"), Tri::True);

    // 两场战役互不影响，世界正典没有被修改。
    assert_eq!(get(&e.world, &elder.id).unwrap().rev, 1);
    assert_eq!(nodes::relations_of(e.world.conn(), &elder.id).unwrap().len(), 0);
    assert_eq!(events(e.world.conn(), &c1.id, 0, 100).unwrap().len(), 3);
    assert_eq!(events(e.world.conn(), &c2.id, 0, 100).unwrap().len(), 2);
}

#[test]
fn edit_edge_condition_effects_and_once() {
    let mut e = env();
    let s = script(&mut e, "编辑");
    let a = pnode(&mut e, &s.id, PlotNodeKind::Start, "开场");
    let b = pnode(&mut e, &s.id, PlotNodeKind::Ending, "结局");
    let ed = edge(&mut e, &s.id, &a.id, &b.id, json!({"type":"flag","name":"x"}), json!([]));
    let patch = PlotEdgePatch {
        label: Some("新标签".into()),
        condition: Some(None),
        effects: Some(serde_json::from_value(json!([{"type":"inc_var","key":"gold","by":3}])).unwrap()),
        priority: Some(5),
        once: Some(true),
    };
    let (u, _) = e.world.write(&user(), "u", |w| w.update_plot_edge(&ed.id, patch, Some(1))).unwrap();
    assert_eq!((u.label.as_str(), u.condition.is_none(), u.effects.len(), u.priority, u.once, u.rev), ("新标签", true, 1, 5, true, 2));
    // 非法条件被拒绝，且不修改。
    let bad = PlotEdgePatch { condition: Some(Some(serde_json::from_value(json!({"type":"check","skill":"x","dc":1,"dice":"1d0"})).unwrap())), ..Default::default() };
    assert!(e.world.write(&user(), "u", |w| w.update_plot_edge(&ed.id, bad, None)).is_err());
    assert_eq!(chronicle_core::plot::get_edge(e.world.conn(), &ed.id).unwrap().unwrap().rev, 2);
}

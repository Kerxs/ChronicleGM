//! 剧情图生成测试（mock 模型，只验证管线与转换逻辑）。

mod common;

use chronicle_core::campaign;
use chronicle_core::changeset;
use chronicle_core::plot_analysis::analyze_plot;
use chronicle_core::*;
use chronicle_gen::runner::{self, GenOptions};
use chronicle_gen::{Task, Templates};
use chronicle_llm::CancelToken;
use common::Scripted;
use serde_json::json;

fn env() -> (tempfile::TempDir, World) {
    let dir = tempfile::tempdir().unwrap();
    let w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "测试").unwrap();
    (dir, w)
}

fn run(w: &mut World, client: &Scripted, task: Task) -> runner::RunOutcome {
    runner::run(w, client, task, &Templates::builtin(), None, &GenOptions { stream: false, ..Default::default() }, &CancelToken::new()).unwrap()
}

struct Fixture {
    script: String,
    quest: String,
    clue: String,
    ending: String,
}

fn fixture(w: &mut World) -> Fixture {
    let s = w.write(&Author::User, "s", |x| x.create_script(NewScript::new("灯塔之谜"))).unwrap().0;
    let item = |w: &mut World, k: ScriptItemKind, t: &str, p: serde_json::Value| {
        let mut it = NewScriptItem::new(&s.id, k, t);
        it.payload = p;
        w.write(&Author::User, "i", |x| x.create_script_item(it)).unwrap().0.id
    };
    Fixture {
        quest: item(w, ScriptItemKind::Quest, "找到守灯人", json!({"quest_type":"main"})),
        clue: item(w, ScriptItemKind::Clue, "湿脚印", json!({})),
        ending: item(w, ScriptItemKind::Ending, "灯塔重燃", json!({})),
        script: s.id,
    }
}

#[test]
fn plot_from_script_builds_playable_graph() {
    let (_d, mut w) = env();
    let f = fixture(&mut w);
    let client = Scripted::ok(json!({
        "nodes": [
            {"key":"k1","kind":"start","title":"抵达雾港"},
            {"key":"k2","kind":"scene","title":"搜查码头"},
            {"key":"k3","kind":"choice","title":"灯塔门前"},
            {"key":"k4","kind":"ending","title":"灯塔重燃","script_item": f.ending},
            {"key":"k5","kind":"ending","title":"迷失雾中","script_item":"不存在的条目"},
            {"key":"k5","kind":"scene","title":"重复编号"}
        ],
        "edges": [
            {"from":"k1","to":"k2","label":"去码头","effects":[{"type":"set_quest_state","quest": f.quest,"state":"active"}]},
            {"from":"k2","to":"k3","label":"跟随脚印","condition":{"type":"clue_known","clue": f.clue},"effects":[{"type":"teleport"}]},
            {"from":"k2","to":"k3","label":"硬闯","condition":{"type":"check","skill":"athletics","dc":14}},
            {"from":"k2","to":"k2","label":"再搜一遍","once":true,"effects":[{"type":"reveal_clue","clue": f.clue}]},
            {"from":"k3","to":"k4","label":"点燃灯塔","condition":{"type":"all","of":[{"type":"visited","node":"k2"},{"type":"quest_state","quest": f.quest,"state":"active"}]}},
            {"from":"k3","to":"k5","label":"转身离开","condition":{"type":"teleport"}},
            {"from":"k3","to":"k9","label":"去不存在的地方"}
        ]
    }));
    let cs = run(&mut w, &client, Task::PlotFromScript { script_id: f.script.clone(), guidance: String::new() }).result.unwrap();
    let codes: Vec<_> = cs.validation.issues.iter().map(|i| i.code.as_str()).collect();
    for c in ["gen.bad_key", "gen.bad_item_ref", "gen.bad_condition", "gen.bad_effect", "gen.unresolved_edge"] {
        assert!(codes.contains(&c), "缺少 {c}：{codes:?}");
    }
    assert!(!codes.contains(&"gen.structure_no_ending"), "完整的图不应提示结构问题：{codes:?}");
    assert!(cs.validation.ok, "{:#?}", cs.validation);
    // 提示词给的是短编号（Q1/C1）与条件格式说明，不给 26 位 ID（实测模型会抄错）。
    let prompt = &client.requests()[0].messages[1].content;
    assert!(prompt.contains("Q1") && prompt.contains("C1") && prompt.contains("quest_state"));
    assert!(!prompt.contains(&f.quest), "提示词不应出现真实 ID");

    changeset::apply(&mut w, &cs.id, None, &Author::User).unwrap();
    let nodes = plot::plot_nodes_of(w.conn(), &f.script).unwrap();
    assert_eq!(nodes.len(), 5);
    let ending = nodes.iter().find(|n| n.title == "灯塔重燃").unwrap();
    assert_eq!(ending.script_item_ref.as_deref(), Some(f.ending.as_str()));
    let edges = plot::edges_of_script(w.conn(), &f.script).unwrap();
    assert_eq!(edges.len(), 6);
    let light = edges.iter().find(|e| e.label == "点燃灯塔").unwrap();
    let docks = nodes.iter().find(|n| n.title == "搜查码头").unwrap();
    assert!(serde_json::to_string(&light.condition).unwrap().contains(&docks.id), "节点编号被换成真实 ID");
    assert!(edges.iter().find(|e| e.label == "跟随脚印").unwrap().effects.is_empty());
    let report = analyze_plot(w.conn(), &f.script).unwrap();
    assert!(!report.issues.iter().any(|i| i.code == "bad_reference" || i.code == "no_start"), "{:#?}", report.issues);
    assert_eq!(report.reachable_endings, 2);

    // 真正可玩：开场 → 码头（任务激活）→ 再搜一遍拿到线索 → 跟随脚印 → 点燃灯塔。
    let c = campaign::create_campaign(&mut w, Some(&f.script), "试玩", Some(5)).unwrap();
    for label in ["去码头", "再搜一遍", "跟随脚印", "点燃灯塔"] {
        let id = edges.iter().find(|e| e.label == label).unwrap().id.clone();
        assert!(campaign::take_edge(&mut w, &c.id, &id).unwrap().taken, "{label}");
    }
    assert_eq!(campaign::current_node(&w, &c.id).unwrap().unwrap().title, "灯塔重燃");
}

#[test]
fn branches_from_player_action_only_add() {
    let (_d, mut w) = env();
    let f = fixture(&mut w);
    let pn = |w: &mut World, k, t: &str| w.write(&Author::User, "n", |x| x.create_plot_node(NewPlotNode::new(&f.script, k, t))).unwrap().0;
    let start = pn(&mut w, PlotNodeKind::Start, "抵达雾港");
    let docks = pn(&mut w, PlotNodeKind::Scene, "码头");
    let end = pn(&mut w, PlotNodeKind::Ending, "结局");
    w.write(&Author::User, "e", |x| x.create_plot_edge(NewPlotEdge::new(&f.script, &start.id, &docks.id))).unwrap();
    w.write(&Author::User, "e", |x| x.create_plot_edge(NewPlotEdge::new(&f.script, &docks.id, &end.id))).unwrap();

    let client = Scripted::ok(json!({
        "nodes": [
            {"key":"k1","kind":"start","title":"贿赂守卫","gm_text":"守卫收钱后会告密"},
            {"key":"k2","kind":"","title":"夜探仓库"}
        ],
        "edges": [
            {"from": start.id, "to":"k1","label":"掏出金币贿赂守卫","condition":{"type":"var","key":"gold","op":"ge","value":5},
             "effects":[{"type":"inc_var","key":"gold","by":-5},{"type":"set_flag","name":"bribed"}]},
            {"from":"k1","to":"k2","label":"趁夜潜入"},
            {"from":"k2","to": docks.id,"label":"回到码头"},
            {"from":"抵达雾港","to":"夜探仓库","label":"按标题引用的分支"},
            {"from": docks.id,"to":"k2","label":"从码头偷偷修改的分支"}
        ]
    }));
    let task = Task::PlotBranches { script_id: f.script.clone(), from_node_id: start.id.clone(), player_action: "我想贿赂守卫进入仓库".into(), count: 2 };
    let cs = run(&mut w, &client, task).result.unwrap();
    assert!(cs.validation.issues.iter().any(|i| i.code == "gen.edge_not_allowed"));
    assert!(client.requests()[0].messages[1].content.contains("我想贿赂守卫"));
    changeset::apply(&mut w, &cs.id, None, &Author::User).unwrap();

    let nodes = plot::plot_nodes_of(w.conn(), &f.script).unwrap();
    assert_eq!(nodes.iter().filter(|n| n.kind == PlotNodeKind::Start).count(), 1, "新节点不能成为第二个开场");
    assert_eq!(nodes.iter().find(|n| n.title == "夜探仓库").unwrap().kind, PlotNodeKind::Dynamic);
    assert_eq!(plot::outgoing(w.conn(), &docks.id).unwrap().len(), 1, "没有给已有节点偷偷加出边");
    assert!(plot::outgoing(w.conn(), &start.id).unwrap().iter().any(|e| e.label == "按标题引用的分支"), "按标题解析节点（真实联调中模型用标题指代节点）");
    for n in [&start, &docks, &end] {
        assert_eq!(plot::get_plot_node(w.conn(), &n.id).unwrap().unwrap().rev, 1);
    }
    // 新分支真正受状态影响：没钱走不了，有钱才能走，并扣钱。
    let c = campaign::create_campaign(&mut w, Some(&f.script), "试玩", Some(1)).unwrap();
    let bribe = plot::outgoing(w.conn(), &start.id).unwrap().into_iter().find(|e| e.label.contains("贿赂")).unwrap();
    assert!(!campaign::take_edge(&mut w, &c.id, &bribe.id).unwrap().taken);
    let give: Vec<chronicle_core::rules::Effect> = serde_json::from_value(json!([{"type":"set_var","key":"gold","value":8}])).unwrap();
    campaign::apply_effects(&mut w, &c.id, &give, "奖励").unwrap();
    let out = campaign::take_edge(&mut w, &c.id, &bribe.id).unwrap();
    assert!(out.taken);
    assert_eq!(campaign::current_node(&w, &c.id).unwrap().unwrap().title, "贿赂守卫");

    // 结局节点不能再生成分支。
    let r = runner::run(&mut w, &client, Task::PlotBranches { script_id: f.script.clone(), from_node_id: end.id.clone(), player_action: String::new(), count: 2 }, &Templates::builtin(), None, &GenOptions::default(), &CancelToken::new());
    assert!(r.is_err());
}

#[test]
fn broken_graph_is_flagged_before_review() {
    // 真实联调中出现过：开场没有出边，所有结局都到不了。必须在写入前提示。
    let (_d, mut w) = env();
    let f = fixture(&mut w);
    let client = Scripted::ok(json!({
        "nodes": [
            {"key":"k1","kind":"start","title":"村口相遇"},
            {"key":"k2","kind":"choice","title":"村长的请求"},
            {"key":"k3","kind":"scene","title":"接受委托"},
            {"key":"k4","kind":"ending","title":"灯塔重燃"}
        ],
        "edges": [
            {"from":"k2","to":"k3","label":"接受"},
            {"from":"k2","to":"k4","label":"直接结束"}
        ]
    }));
    let cs = run(&mut w, &client, Task::PlotFromScript { script_id: f.script.clone(), guidance: String::new() }).result.unwrap();
    let codes: Vec<_> = cs.validation.issues.iter().map(|i| i.code.as_str()).collect();
    assert!(codes.contains(&"gen.structure_no_ending"), "{codes:?}");
    assert!(codes.contains(&"gen.structure_unreachable"), "{codes:?}");
    assert!(codes.contains(&"gen.structure_dead_end"), "{codes:?}");
    // 结构完整的图不会出现这些提示（见 plot_from_script_builds_playable_graph）。
}

#[test]
fn short_refs_are_mapped_and_invented_ids_never_reach_the_graph() {
    // 真实联调：模型抄错 ID，分支引用了不存在的线索，检查面板才发现。现在写入前就拦下。
    let (_d, mut w) = env();
    let f = fixture(&mut w);
    let client = Scripted::ok(json!({
        "nodes": [
            {"key":"k1","kind":"start","title":"抵达雾港"},
            {"key":"k2","kind":"scene","title":"码头","script_item":"S1"},
            {"key":"k3","kind":"ending","title":"灯塔重燃"}
        ],
        "edges": [
            {"from":"k1","to":"k2","label":"去码头","effects":[{"type":"set_quest_state","quest":"Q1","state":"active"},{"type":"reveal_clue","clue":"01M798FFB5F32ZZ8XZ3BG70DXA"}]},
            {"from":"k2","to":"k3","label":"跟随脚印","condition":{"type":"clue_known","clue":"c1"}},
            {"from":"k2","to":"k3","label":"凭空的线索","condition":{"type":"clue_known","clue":"C9"}},
            {"from":"k2","to":"k3","label":"按标题引用","condition":{"type":"clue_known","clue":"湿脚印"}},
            {"from":"k2","to":"k3","label":"种类不对","effects":[{"type":"reveal_clue","clue":"Q1"}]}
        ]
    }));
    let cs = run(&mut w, &client, Task::PlotFromScript { script_id: f.script.clone(), guidance: String::new() }).result.unwrap();
    let bad: Vec<_> = cs.validation.issues.iter().filter(|i| i.code == "gen.bad_reference").map(|i| i.message.clone()).collect();
    assert_eq!(bad.len(), 3, "编造的 ID、不存在的 C9、把任务当线索：{bad:#?}");
    changeset::apply(&mut w, &cs.id, None, &Author::User).unwrap();
    let edges = plot::edges_of_script(w.conn(), &f.script).unwrap();
    let by = |l: &str| edges.iter().find(|e| e.label == l).unwrap().clone();
    let go = serde_json::to_string(&by("去码头").effects).unwrap();
    assert!(go.contains(&f.quest) && !go.contains("01M798FFB5F32ZZ8XZ3BG70DXA"), "Q1 换成真实 ID，编造的线索被移除：{go}");
    assert!(serde_json::to_string(&by("跟随脚印").condition).unwrap().contains(&f.clue), "小写 c1 也能识别");
    assert!(by("凭空的线索").condition.is_none());
    assert!(serde_json::to_string(&by("按标题引用").condition).unwrap().contains(&f.clue));
    assert!(by("种类不对").effects.is_empty());
    let report = analyze_plot(w.conn(), &f.script).unwrap();
    assert!(!report.issues.iter().any(|i| i.code == "bad_reference"), "写入后的剧情图没有无效引用：{:#?}", report.issues);
}

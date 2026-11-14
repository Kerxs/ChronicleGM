//! 世界树生成任务测试（mock 模型，只验证管线与转换逻辑）。

mod common;

use chronicle_core::changeset::{self, Mode};
use chronicle_core::jobs::{self, JobStatus};
use chronicle_core::*;
use chronicle_gen::runner::{self, GenOptions};
use chronicle_gen::world_tasks::RewriteMode;
use chronicle_gen::{Task, Templates};
use chronicle_llm::CancelToken;
use common::Scripted;
use serde_json::json;

fn env() -> (tempfile::TempDir, World) {
    let dir = tempfile::tempdir().unwrap();
    let w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "测试").unwrap();
    (dir, w)
}

fn opts() -> GenOptions {
    GenOptions { stream: false, ..Default::default() }
}

fn run(w: &mut World, client: &Scripted, task: Task) -> runner::RunOutcome {
    runner::run(w, client, task, &Templates::builtin(), None, &opts(), &CancelToken::new()).unwrap()
}

fn mk(w: &mut World, n: NewNode) -> Node {
    w.write(&Author::User, "t", |x| x.create_node(n)).unwrap().0
}

fn seed_json() -> serde_json::Value {
    json!({
        "name": "潮汐大陆",
        "summary": "被月潮支配的群岛世界",
        "world_rules": ["魔法强弱随月相变化", "新月之夜魔法完全失效"],
        "sections": [
            {"category": "地理区域", "nodes": [
                {"name": "雾港", "kind": "city", "summary": "走私者的港口", "details": {"features": ["终年大雾"]},
                 "children": [{"name": "旧灯塔", "kind": "site", "summary": "废弃的灯塔"}]},
                {"name": "银潮群岛", "kind": "region"}
            ]},
            {"category": "势力与组织", "nodes": [
                {"name": "潮汐教会", "kind": "faction", "details": {"goals": ["控制月潮"], "hidden_agenda": "献祭渔民"}}
            ]},
            {"category": "人物与NPC", "nodes": [
                {"name": "守灯人老莫", "kind": "character", "details": {"secrets": 42}},
                {"name": "神秘旅人", "kind": "wizard"}
            ]}
        ],
        "relations": [
            {"from": "守灯人老莫", "to": "旧灯塔", "rel_type": "located_in"},
            {"from": "潮汐教会", "to": "雾港", "rel_type": "rules", "label": "暗中控制"},
            {"from": "不存在的人", "to": "雾港", "rel_type": "enemy"}
        ]
    })
}

#[test]
fn world_from_one_sentence() {
    let (_d, mut w) = env();
    let client = Scripted::ok(seed_json());
    let task = Task::WorldSeed { idea: "被月潮支配的群岛".into(), genre: "奇幻".into(), era: String::new(), style: String::new(), rules: String::new() };
    let out = run(&mut w, &client, task);
    let cs = out.result.unwrap();
    assert!(cs.validation.ok, "{:#?}", cs.validation);
    assert_eq!(cs.mode, Mode::Append);
    let codes: Vec<_> = cs.validation.issues.iter().map(|i| i.code.as_str()).collect();
    for c in ["gen.promote_rules", "gen.details_dropped", "gen.unknown_kind", "gen.unresolved_relation"] {
        assert!(codes.contains(&c), "缺少 {c}: {codes:?}");
    }
    // 提示词要求使用固定分类与关系类型。
    let prompt = &client.requests()[0].messages[1].content;
    assert!(prompt.contains("科技与魔法体系") && prompt.contains("located_in"));
    assert_eq!(nodes::child_count(w.conn(), None).unwrap(), 0, "确认前不写入");

    changeset::apply(&mut w, &cs.id, None, &Author::User).unwrap();
    let root = &nodes::children(w.conn(), None, 0, 10).unwrap()[0];
    assert_eq!((root.kind, root.title.as_str()), (NodeKind::World, "潮汐大陆"));
    let cats: Vec<String> = nodes::children(w.conn(), Some(&root.id), 0, 20).unwrap().into_iter().map(|n| n.title).collect();
    assert_eq!(cats, ["宇宙与世界背景", "地理区域", "势力与组织", "人物与NPC"]);
    let harbor = &nodes::find_by_name(w.conn(), "雾港", None).unwrap()[0];
    assert_eq!(harbor.payload["features"], json!(["终年大雾"]));
    let tower = &nodes::children(w.conn(), Some(&harbor.id), 0, 10).unwrap()[0];
    assert_eq!(tower.title, "旧灯塔");
    let keeper = &nodes::find_by_name(w.conn(), "守灯人老莫", None).unwrap()[0];
    assert_eq!(keeper.payload, json!({}), "格式错误的资料被丢弃而不是写坏");
    assert_eq!(nodes::find_by_name(w.conn(), "神秘旅人", None).unwrap()[0].kind, NodeKind::Character, "无效类型回落到分类默认类型");
    let rels = nodes::relations_of(w.conn(), &harbor.id).unwrap();
    assert_eq!(rels.len(), 1);
    assert_eq!(rels[0].label, "暗中控制");
    assert_eq!(nodes::relations_of(w.conn(), &keeper.id).unwrap()[0].dst_id, tower.id);
    let rules = nodes::children(w.conn(), Some(&nodes::find_by_name(w.conn(), "宇宙与世界背景", None).unwrap()[0].id), 0, 10).unwrap();
    assert_eq!(rules.len(), 2);
    assert!(rules.iter().all(|r| r.tags == vec!["world_rule".to_string()] && r.canon_level == CanonLevel::Draft));
}

#[test]
fn world_seed_appends_to_existing_root_and_categories() {
    let (_d, mut w) = env();
    let root = mk(&mut w, NewNode::new(NodeKind::World, "已有世界"));
    let geo = mk(&mut w, NewNode::new(NodeKind::Category, "地理区域").under(&root.id));
    let client = Scripted::ok(seed_json());
    let task = Task::WorldSeed { idea: "x".into(), genre: String::new(), era: String::new(), style: String::new(), rules: String::new() };
    let cs = run(&mut w, &client, task).result.unwrap();
    assert!(cs.validation.issues.iter().any(|i| i.code == "gen.existing_root"));
    changeset::apply(&mut w, &cs.id, None, &Author::User).unwrap();
    assert_eq!(nodes::child_count(w.conn(), None).unwrap(), 1, "没有新建第二个根节点");
    assert_eq!(nodes::find_by_name(w.conn(), "地理区域", None).unwrap().len(), 1, "复用已有分类");
    assert_eq!(nodes::child_count(w.conn(), Some(&geo.id)).unwrap(), 2);
}

#[test]
fn expand_node_adds_children_with_context() {
    let (_d, mut w) = env();
    let nation = mk(&mut w, NewNode::new(NodeKind::Nation, "北境王国"));
    let city = mk(&mut w, NewNode::new(NodeKind::City, "白石城").under(&nation.id));
    mk(&mut w, NewNode::new(NodeKind::Site, "王宫").under(&city.id));
    let duke = mk(&mut w, NewNode::new(NodeKind::Character, "铁公爵"));
    let client = Scripted::ok(json!({
        "nodes": [
            {"name": "铁匠街", "kind": "site", "summary": "锻造之声不绝"},
            {"name": "地下水道", "kind": "", "gm_text": "通往王宫的密道"}
        ],
        "relations": [{"from": "铁公爵", "to": "铁匠街", "rel_type": "rules"}, {"from": "铁匠街", "to": "铁匠街", "rel_type": "x"}]
    }));
    let task = Task::ExpandNode { node_id: city.id.clone(), count: 2, kinds: vec![], guidance: String::new() };
    let cs = run(&mut w, &client, task).result.unwrap();
    let prompt = &client.requests()[0].messages[1].content;
    assert!(prompt.contains("北境王国"), "包含祖先");
    assert!(prompt.contains("王宫"), "包含已有子节点");
    changeset::apply(&mut w, &cs.id, None, &Author::User).unwrap();
    let kids = nodes::children(w.conn(), Some(&city.id), 0, 10).unwrap();
    assert_eq!(kids.len(), 3);
    assert_eq!(kids.iter().find(|k| k.title == "地下水道").unwrap().kind, NodeKind::Site, "空类型按父节点推断");
    assert_eq!(nodes::relations_of(w.conn(), &duke.id).unwrap().len(), 1);
    assert_eq!(nodes::get_node(w.conn(), &city.id).unwrap().unwrap().rev, 1, "父节点本身未被修改");
}

#[test]
fn rewrite_modes_and_protection() {
    let (_d, mut w) = env();
    let mut n = NewNode::new(NodeKind::Character, "守灯人老莫").payload(json!({"identity": "守灯人"}));
    n.summary = "原有摘要".into();
    let node = mk(&mut w, n);
    let reply = json!({"summary": "新摘要", "player_text": "一位沉默的老人", "gm_text": "知道船去了哪里",
        "details": {"identity": "前海盗", "secrets": ["曾是幽灵船员"]}});

    // 补全：只填空白字段。
    let client = Scripted::ok(reply.clone());
    let cs = run(&mut w, &client, Task::RewriteNode { node_id: node.id.clone(), mode: RewriteMode::Complete, instruction: String::new() })
        .result
        .unwrap();
    assert!(matches!(cs.mode, Mode::Targeted { .. }));
    changeset::apply(&mut w, &cs.id, None, &Author::User).unwrap();
    let n2 = nodes::get_node(w.conn(), &node.id).unwrap().unwrap();
    assert_eq!(n2.summary, "原有摘要");
    assert_eq!(n2.player_text, "一位沉默的老人");
    assert_eq!(n2.payload["identity"], "守灯人");
    assert_eq!(n2.payload["secrets"], json!(["曾是幽灵船员"]));

    // 重写：替换。
    let client = Scripted::ok(reply.clone());
    let cs = run(&mut w, &client, Task::RewriteNode { node_id: node.id.clone(), mode: RewriteMode::Rewrite, instruction: "更神秘".into() })
        .result
        .unwrap();
    changeset::apply(&mut w, &cs.id, None, &Author::User).unwrap();
    let n3 = nodes::get_node(w.conn(), &node.id).unwrap().unwrap();
    assert_eq!((n3.summary.as_str(), n3.payload["identity"].as_str()), ("新摘要", Some("前海盗")));
    assert!(client.requests()[0].messages[1].content.contains("更神秘"));

    // 补全时已经没有空白：任务失败，世界不变。
    let client = Scripted::ok(reply);
    let out = run(&mut w, &client, Task::RewriteNode { node_id: node.id.clone(), mode: RewriteMode::Complete, instruction: String::new() });
    assert!(out.result.is_err());
    assert_eq!(jobs::get_job(w.conn(), &out.job_id).unwrap().status, JobStatus::Failed);

    // 核心设定不能交给 AI。
    w.write(&Author::User, "core", |x| x.update_node(&node.id, NodePatch { canon_level: Some(CanonLevel::Core), ..Default::default() }, None)).unwrap();
    let client = Scripted::ok(json!({"summary": "x"}));
    let r = runner::run(&mut w, &client, Task::RewriteNode { node_id: node.id.clone(), mode: RewriteMode::Rewrite, instruction: String::new() }, &Templates::builtin(), None, &opts(), &CancelToken::new());
    assert!(r.is_err());
    assert!(client.requests().is_empty(), "受保护节点不会发送给模型");
}

#[test]
fn rewrite_prompt_lists_payload_fields_and_output_limit_fits_context() {
    let (_d, mut w) = env();
    let node = mk(&mut w, NewNode::new(NodeKind::Character, "老莫"));
    let client = Scripted::ok(json!({"summary": "守灯人"}));
    let opts = GenOptions { stream: false, context_length: Some(8000), max_tokens: 24000, ..Default::default() };
    runner::run(&mut w, &client, Task::RewriteNode { node_id: node.id, mode: RewriteMode::Complete, instruction: String::new() }, &Templates::builtin(), None, &opts, &CancelToken::new()).unwrap();
    let req = &client.requests()[0];
    assert!(req.messages[1].content.contains("knowledge（array"), "提示词列出人物资料字段");
    let max = req.max_tokens.unwrap();
    assert!(max < 8000 && max > 3000, "输出上限按上下文剩余空间计算：{max}");
    assert!(chronicle_gen::world_tasks::payload_fields(NodeKind::Faction).contains("hidden_agenda"));
}

#[test]
fn expansion_relations_accept_short_names_of_existing_nodes() {
    // 真实联调：展开节点时模型用"薇拉娜"指已有的"观星祭司 薇拉娜"，关系被丢弃。
    let dir = tempfile::tempdir().unwrap();
    let mut w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "t").unwrap();
    let mk = |w: &mut World, n: NewNode| w.write(&Author::User, "n", |x| x.create_node(n)).unwrap().0;
    let isles = mk(&mut w, NewNode::new(NodeKind::Region, "主岛链"));
    let priest = mk(&mut w, NewNode::new(NodeKind::Character, "观星祭司 薇拉娜"));
    let out = json!({
        "nodes": [{"name": "望月台", "kind": "site", "summary": "观星高台", "player_text": "", "gm_text": "", "aliases": [], "details": {}, "children": []}],
        "relations": [
            {"from": "薇拉娜", "to": "望月台", "rel_type": "located_in", "label": ""},
            {"from": "望月台", "to": "望月台", "rel_type": "allied_with", "label": ""}
        ]
    });
    let (ops, notes, _, _) = chronicle_gen::world_tasks::convert_expand(&w, &isles.id, &[], &out).unwrap();
    let rels: Vec<_> = ops.iter().filter_map(|o| match o { chronicle_core::changeset::Op::CreateRelation { relation, .. } => Some(relation.clone()), _ => None }).collect();
    assert_eq!(rels.len(), 1, "{notes:#?}");
    assert_eq!(rels[0].src_id, priest.id);
    assert!(notes.iter().any(|n| n.code == "self_relation"));
    assert!(!notes.iter().any(|n| n.code == "unresolved_relation"), "{notes:#?}");
}

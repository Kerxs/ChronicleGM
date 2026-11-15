mod common;

use chronicle_core::changeset::*;
use chronicle_core::nodes::*;
use chronicle_core::*;
use common::*;
use serde_json::json;

fn ops(v: serde_json::Value) -> Vec<Op> {
    serde_json::from_value(v).unwrap()
}

fn new_cs(mode: Mode, ops: Vec<Op>) -> NewChangeset {
    NewChangeset { job_id: None, scope: "world".into(), mode, author: ai(), title: "AI 提议".into(), ops }
}

#[test]
fn propose_does_not_write_then_apply_writes_one_commit() {
    let mut e = env();
    let root = mk(&mut e.world, NewNode::new(NodeKind::World, "世界"));
    let o = ops(json!([
        {"op":"create_node","temp_id":"tmp:faction","node":{"kind":"faction","title":"银月议会","parent_id": root.id}},
        {"op":"create_node","temp_id":"tmp:leader","node":{"kind":"character","title":"议长维兰","parent_id":"tmp:faction",
            "payload":{"motivation":"守护古籍","secrets":["曾背叛导师"]}}},
        {"op":"create_relation","relation":{"src_id":"tmp:leader","dst_id":"tmp:faction","rel_type":"member_of"}}
    ]));
    let cs = propose(&mut e.world, new_cs(Mode::Normal, o)).unwrap();
    assert!(cs.validation.ok, "{:?}", cs.validation);
    assert_eq!(child_count(e.world.conn(), Some(&root.id)).unwrap(), 0, "提议阶段不写入");
    let before_commits = e.world.commits(100, 0).unwrap().len();

    let applied = apply(&mut e.world, &cs.id, None, &user()).unwrap();
    assert_eq!(applied.status, ChangesetStatus::Applied);
    let faction = &children(e.world.conn(), Some(&root.id), 0, 10).unwrap()[0];
    let leader = &children(e.world.conn(), Some(&faction.id), 0, 10).unwrap()[0];
    assert_eq!(leader.title, "议长维兰");
    assert_eq!(leader.source, "ai:job1");
    assert_eq!(relations_of(e.world.conn(), &leader.id).unwrap().len(), 1);
    let commits = e.world.commits(100, 0).unwrap();
    assert_eq!(commits.len(), before_commits + 1);
    assert_eq!(commits[0].changeset_id.as_deref(), Some(cs.id.as_str()));

    // 已处理的变更集不能再次应用；整体撤销。
    assert!(apply(&mut e.world, &cs.id, None, &user()).is_err());
    e.world.undo(applied.commit_id.as_deref().unwrap(), &user()).unwrap();
    assert_eq!(child_count(e.world.conn(), Some(&root.id)).unwrap(), 0);
}

#[test]
fn partial_accept_respects_dependencies() {
    let mut e = env();
    let o = ops(json!([
        {"op":"create_node","temp_id":"tmp:a","node":{"kind":"city","title":"甲城"}},
        {"op":"create_node","node":{"kind":"site","title":"甲城钟楼","parent_id":"tmp:a"}},
        {"op":"create_node","node":{"kind":"city","title":"乙城"}}
    ]));
    let cs = propose(&mut e.world, new_cs(Mode::Normal, o)).unwrap();
    // 只接受第 1 条（依赖第 0 条）→ 拒绝，且不写入任何内容。
    let r = apply(&mut e.world, &cs.id, Some(vec![1]), &user());
    assert!(matches!(r, Err(CoreError::Validation(_))), "{r:?}");
    assert_eq!(child_count(e.world.conn(), None).unwrap(), 0);
    // 接受 0 和 2 → 部分应用。
    let done = apply(&mut e.world, &cs.id, Some(vec![0, 2]), &user()).unwrap();
    assert_eq!(done.status, ChangesetStatus::Partial);
    assert_eq!(done.accepted_ops, Some(vec![0, 2]));
    assert_eq!(child_count(e.world.conn(), None).unwrap(), 2);
}

#[test]
fn append_mode_and_targeted_mode() {
    let mut e = env();
    let a = mk(&mut e.world, NewNode::new(NodeKind::City, "甲"));
    let b = mk(&mut e.world, NewNode::new(NodeKind::City, "乙"));
    let o = ops(json!([
        {"op":"create_node","node":{"kind":"city","title":"丙"}},
        {"op":"update_node","id": a.id, "base_rev": 1, "patch":{"summary":"覆盖"}}
    ]));
    let cs = propose(&mut e.world, new_cs(Mode::Append, o)).unwrap();
    assert!(!cs.validation.ok);
    assert_eq!(cs.validation.errors().next().unwrap().code, "append_only");
    assert_eq!(cs.validation.errors().next().unwrap().op, 1);

    let o = ops(json!([
        {"op":"update_node","id": a.id, "base_rev": 1, "patch":{"summary":"重写甲"}},
        {"op":"update_node","id": b.id, "base_rev": 1, "patch":{"summary":"顺手改乙"}}
    ]));
    let cs = propose(&mut e.world, new_cs(Mode::Targeted { targets: vec![a.id.clone()] }, o)).unwrap();
    let errs: Vec<_> = cs.validation.errors().map(|i| (i.op, i.code.clone())).collect();
    assert_eq!(errs, [(1, "out_of_target".to_string())]);
    apply(&mut e.world, &cs.id, Some(vec![0]), &user()).unwrap();
    assert_eq!(get(&e.world, &a.id).unwrap().summary, "重写甲");
    assert_eq!(get(&e.world, &b.id).unwrap().summary, "");
}

#[test]
fn ai_cannot_touch_core_or_locked() {
    let mut e = env();
    let core = mk(&mut e.world, NewNode::new(NodeKind::MagicSystem, "以太法则").canon(CanonLevel::Core));
    let o = ops(json!([
        {"op":"update_node","id": core.id, "base_rev": 1, "patch":{"summary":"魔法其实不存在"}},
        {"op":"delete_node","id": core.id},
        {"op":"update_node","id": core.id, "patch":{"canon_level":"draft"}}
    ]));
    let cs = propose(&mut e.world, new_cs(Mode::Normal, o)).unwrap();
    let codes: Vec<_> = cs.validation.errors().map(|i| i.code.as_str()).collect();
    assert_eq!(codes, ["protected", "protected", "protected"]);
    assert!(apply(&mut e.world, &cs.id, None, &user()).is_err());
    assert_eq!(get(&e.world, &core.id).unwrap().summary, "");
}

#[test]
fn warnings_for_duplicates_and_conflicts() {
    let mut e = env();
    let a = mk(&mut e.world, NewNode::new(NodeKind::Faction, "铁卫"));
    let b = mk(&mut e.world, NewNode::new(NodeKind::Faction, "影刃"));
    e.world.write(&user(), "r", |w| w.create_relation(NewRelation::new(&a.id, &b.id, "ally"))).unwrap();
    let dead = mk(
        &mut e.world,
        NewNode::new(NodeKind::Character, "老国王").payload(json!({"life_status":"dead","death_time":100})),
    );
    let battle = mk(&mut e.world, NewNode::new(NodeKind::Event, "灰烬之战").payload(json!({"world_time":150})));
    let o = ops(json!([
        {"op":"create_node","node":{"kind":"faction","title":"铁 卫"}},
        {"op":"create_relation","relation":{"src_id": a.id, "dst_id": b.id, "rel_type":"enemy"}},
        {"op":"create_relation","relation":{"src_id": dead.id, "dst_id": battle.id, "rel_type":"participated_in"}},
        {"op":"create_node","node":{"kind":"city","title":"新城"}},
        {"op":"create_node","node":{"kind":"city","title":"新城"}}
    ]));
    let cs = propose(&mut e.world, new_cs(Mode::Normal, o)).unwrap();
    assert!(cs.validation.ok, "警告不阻止应用");
    let warns: Vec<_> = cs.validation.issues.iter().map(|i| (i.op, i.code.as_str())).collect();
    assert!(warns.contains(&(0, "possible_duplicate")));
    assert!(warns.contains(&(1, "relation_conflict")));
    assert!(warns.contains(&(2, "timeline_conflict")));
    assert!(warns.contains(&(4, "possible_duplicate")), "同一变更集内的重复也能发现: {warns:?}");
}

#[test]
fn stale_changeset_fails_without_writing() {
    let mut e = env();
    let n = mk(&mut e.world, NewNode::new(NodeKind::City, "城"));
    let o = ops(json!([
        {"op":"create_node","node":{"kind":"site","title":"新地点"}},
        {"op":"update_node","id": n.id, "base_rev": 1, "patch":{"summary":"AI 版本"}}
    ]));
    let cs = propose(&mut e.world, new_cs(Mode::Normal, o)).unwrap();
    assert!(cs.validation.ok);
    // 提议之后用户手动改了这个节点。
    e.world
        .write(&user(), "manual", |w| w.update_node(&n.id, NodePatch { summary: Some("用户版本".into()), ..Default::default() }, None))
        .unwrap();
    let r = apply(&mut e.world, &cs.id, None, &user());
    assert!(matches!(r, Err(CoreError::Validation(ref m)) if m.contains("#1")), "{r:?}");
    assert_eq!(get(&e.world, &n.id).unwrap().summary, "用户版本");
    assert_eq!(child_count(e.world.conn(), None).unwrap(), 1, "第 0 条也没有写入");
    assert_eq!(get_changeset(e.world.conn(), &cs.id).unwrap().unwrap().status, ChangesetStatus::Pending);
}

#[test]
fn revise_and_reject() {
    let mut e = env();
    let o = ops(json!([{"op":"create_node","node":{"kind":"character","title":"x","payload":{"secrets":"bad"}}}]));
    let cs = propose(&mut e.world, new_cs(Mode::Normal, o)).unwrap();
    assert!(!cs.validation.ok);
    let fixed: Op = serde_json::from_value(json!({"op":"create_node","node":{"kind":"character","title":"x","payload":{"secrets":["ok"]}}})).unwrap();
    let cs = revise_op(&mut e.world, &cs.id, 0, fixed).unwrap();
    assert!(cs.validation.ok);
    reject(&mut e.world, &cs.id).unwrap();
    assert!(apply(&mut e.world, &cs.id, None, &user()).is_err());
    assert_eq!(list_changesets(e.world.conn(), Some(ChangesetStatus::Rejected)).unwrap().len(), 1);
    assert_eq!(child_count(e.world.conn(), None).unwrap(), 0);
}

#[test]
fn bad_temp_ids_rejected() {
    let mut e = env();
    let o = ops(json!([
        {"op":"create_node","temp_id":"x","node":{"kind":"city","title":"a"}},
        {"op":"create_node","temp_id":"tmp:d","node":{"kind":"city","title":"b"}},
        {"op":"create_node","temp_id":"tmp:d","node":{"kind":"city","title":"c"}}
    ]));
    let cs = propose(&mut e.world, new_cs(Mode::Normal, o)).unwrap();
    let errs: Vec<_> = cs.validation.errors().map(|i| (i.op, i.code.as_str())).collect();
    assert!(errs.contains(&(0, "bad_temp_id")) && errs.contains(&(2, "bad_temp_id")));
}

#[test]
fn global_and_similar_names_are_flagged() {
    let mut e = env();
    mk(&mut e.world, NewNode::new(NodeKind::Character, "艾琳娜"));
    mk(&mut e.world, NewNode::new(NodeKind::City, "白石港"));
    let o = ops(json!([
        {"op":"create_node","node":{"kind":"character","title":"艾琳娜"}},
        {"op":"create_node","node":{"kind":"character","title":"艾莉娜"}},
        {"op":"create_node","node":{"kind":"character","title":"白石港"}},
        {"op":"create_node","node":{"kind":"site","title":"白石滩"}},
        {"op":"create_node","node":{"kind":"character","title":"阿岩"}}
    ]));
    let cs = propose(&mut e.world, new_cs(Mode::Normal, o)).unwrap();
    let w: Vec<_> = cs.validation.issues.iter().map(|i| (i.op, i.code.as_str())).collect();
    assert!(w.contains(&(0, "duplicate_character")), "{w:?}");
    assert!(w.contains(&(1, "similar_character")), "差一个字：{w:?}");
    assert!(w.contains(&(2, "possible_duplicate")), "跨类型同名：{w:?}");
    assert!(w.contains(&(3, "similar_name")), "{w:?}");
    assert!(!w.iter().any(|(op, _)| *op == 4), "两个字的名字不做近似比较：{w:?}");
    assert!(chronicle_core::util::names_similar("灰烬骑士团长", "灰烬骑土团长"));
    assert!(!chronicle_core::util::names_similar("王都", "王城"));
}

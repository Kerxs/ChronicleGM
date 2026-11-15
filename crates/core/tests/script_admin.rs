//! 剧本管理：删除（级联进回收站）/ 恢复 / 复制（编号重映射）/ 版本快照（对比、恢复）。

mod common;

use chronicle_core::script_admin::{self, campaigns_using};
use chronicle_core::*;
use common::*;
use serde_json::json;

struct Fx {
    e: Env,
    sid: String,
    quest: String,
    start: String,
    edge: String,
}

fn fx() -> Fx {
    let mut e = env();
    let w = &mut e.world;
    let s = w.write(&user(), "s", |x| x.create_script(NewScript::new("护送商队"))).unwrap().0;
    let mk = |w: &mut World, k: ScriptItemKind, t: &str| w.write(&user(), "i", |x| x.create_script_item(NewScriptItem::new(&s.id, k, t))).unwrap().0.id;
    let ch = mk(w, ScriptItemKind::Chapter, "第一章");
    let mut sc = NewScriptItem::new(&s.id, ScriptItemKind::Scene, "山口");
    sc.parent_id = Some(ch.clone());
    w.write(&user(), "i", |x| x.create_script_item(sc)).unwrap();
    let mut q = NewScriptItem::new(&s.id, ScriptItemKind::Quest, "护送");
    q.payload = json!({"quest_type": "main"});
    let quest = w.write(&user(), "i", |x| x.create_script_item(q)).unwrap().0.id;
    let npc = mk_node(w, NodeKind::Character, "老板");
    w.write(&user(), "c", |x| x.add_cast(NewCastMember { script_id: s.id.clone(), node_id: npc, role: "雇主".into(), motive: String::new(), secret: String::new(), relation_to_players: String::new(), payload: json!({}) })).unwrap();
    let a = w.write(&user(), "p", |x| x.create_plot_node(NewPlotNode::new(&s.id, PlotNodeKind::Start, "出发"))).unwrap().0.id;
    let b = w.write(&user(), "p", |x| x.create_plot_node(NewPlotNode::new(&s.id, PlotNodeKind::Scene, "山口"))).unwrap().0.id;
    let mut ed = NewPlotEdge::new(&s.id, &a, &b);
    ed.label = "上路".into();
    ed.effects = serde_json::from_value(json!([{"type": "set_quest_state", "quest": quest, "state": "active"}])).unwrap();
    let edge = w.write(&user(), "e", |x| x.create_plot_edge(ed)).unwrap().0.id;
    Fx { e, sid: s.id, quest, start: a, edge }
}

fn mk_node(w: &mut World, k: NodeKind, t: &str) -> String {
    w.write(&user(), "n", |x| x.create_node(NewNode::new(k, t))).unwrap().0.id
}

#[test]
fn delete_is_cascading_blocked_by_campaigns_and_restorable() {
    let mut f = fx();
    let w = &mut f.e.world;
    let c = campaign::create_campaign(w, Some(&f.sid), "试玩", Some(1)).unwrap();
    let err = w.write(&user(), "d", |x| x.delete_script_cascade(&f.sid)).unwrap_err().to_string();
    assert!(err.contains("试玩"), "有战役在用时不能删：{err}");
    assert_eq!(campaigns_using(w.conn(), &f.sid).unwrap(), vec!["试玩"]);
    timeline::set_archived(w.conn(), &c.id, true).unwrap();
    let n = w.write(&user(), "d", |x| x.delete_script_cascade(&f.sid)).unwrap().0;
    assert_eq!(n, 1 + 3 + 1 + 2 + 1, "剧本 + 3 条目 + 1 角色 + 2 节点 + 1 分支");
    assert!(scripts::get_script(w.conn(), &f.sid).unwrap().is_none());
    assert!(plot::plot_nodes_of(w.conn(), &f.sid).unwrap().is_empty());
    let trash = script_admin::script_trash(w.conn()).unwrap();
    assert_eq!(trash.len(), 1);
    assert_eq!(trash[0].items, 3);
    let r = w.write(&user(), "r", |x| x.restore_script(&f.sid)).unwrap().0;
    assert_eq!(r, n, "同一次删除的全部恢复");
    assert_eq!(scripts::all_items(w.conn(), &f.sid).unwrap().len(), 3);
    assert_eq!(plot::edges_of_script(w.conn(), &f.sid).unwrap().len(), 1);
    assert_eq!(scripts::cast_of(w.conn(), &f.sid).unwrap().len(), 1);
}

#[test]
fn copy_remaps_every_internal_reference() {
    let mut f = fx();
    let w = &mut f.e.world;
    let copy = w.write(&user(), "c", |x| x.copy_script(&f.sid, "")).unwrap().0;
    assert_eq!(copy.title, "护送商队（副本）");
    let b = script_admin::bundle(w.conn(), &copy.id).unwrap();
    assert_eq!((b.items.len(), b.cast.len(), b.nodes.len(), b.edges.len()), (3, 1, 2, 1));
    let old_ids = [f.sid.as_str(), f.quest.as_str(), f.start.as_str(), f.edge.as_str()];
    let text = serde_json::to_string(&serde_json::json!({"i": b.items, "n": b.nodes, "e": b.edges, "c": b.cast})).unwrap();
    for id in old_ids {
        assert!(!text.contains(id), "副本里不能再引用原剧本的编号 {id}");
    }
    let scene = b.items.iter().find(|i| i.title == "山口").unwrap();
    let chapter = b.items.iter().find(|i| i.title == "第一章").unwrap();
    assert_eq!(scene.parent_id.as_deref(), Some(chapter.id.as_str()), "父条目重映射");
    let quest = b.items.iter().find(|i| i.title == "护送").unwrap();
    assert!(serde_json::to_string(&b.edges[0].effects).unwrap().contains(&quest.id), "分支效果里的任务编号重映射");
    // 原剧本不受影响。
    assert_eq!(script_admin::bundle(w.conn(), &f.sid).unwrap().items.len(), 3);
}

#[test]
fn versions_diff_and_restore_with_auto_snapshot() {
    let mut f = fx();
    let w = &mut f.e.world;
    let v = script_admin::create_script_version(w, &f.sid, "初稿", "").unwrap();
    assert_eq!((v.items, v.nodes), (3, 2));
    // 改动：删一个条目、加一个条目、改剧本名。
    w.write(&user(), "d", |x| x.delete_script_item(&f.quest, None)).unwrap();
    w.write(&user(), "i", |x| x.create_script_item(NewScriptItem::new(&f.sid, ScriptItemKind::Ending, "抵达"))).unwrap();
    w.write(&user(), "u", |x| x.update_script(&f.sid, ScriptPatch { title: Some("护送商队二稿".into()), ..Default::default() }, None)).unwrap();
    let d = script_admin::diff_script_version(w.conn(), &v.id).unwrap();
    assert!(d.removed.iter().any(|x| x.contains("护送")), "{d:?}");
    assert!(d.added.iter().any(|x| x.contains("抵达")), "{d:?}");
    assert!(d.changed.iter().any(|x| x.contains("剧本设定")), "{d:?}");
    script_admin::restore_script_version(w, &v.id).unwrap();
    let b = script_admin::bundle(w.conn(), &f.sid).unwrap();
    assert_eq!(b.script.title, "护送商队");
    let titles: Vec<&str> = b.items.iter().map(|i| i.title.as_str()).collect();
    assert!(titles.contains(&"护送") && !titles.contains(&"抵达"), "{titles:?}");
    let vs = script_admin::list_script_versions(w.conn(), &f.sid).unwrap();
    assert_eq!(vs.len(), 2, "恢复前自动保存了一个版本");
    assert!(vs[0].name.contains("自动保存"));
    // 恢复后和快照没有差别。
    let d = script_admin::diff_script_version(w.conn(), &v.id).unwrap();
    assert!(d.added.is_empty() && d.removed.is_empty() && d.changed.is_empty(), "{d:?}");
}

mod common;

use chronicle_core::bundle::*;
use chronicle_core::nodes::*;
use chronicle_core::*;
use common::*;
use serde_json::json;

fn populate(e: &mut common::Env) -> (Node, Node) {
    let root = mk(&mut e.world, NewNode::new(NodeKind::World, "艾尔大陆"));
    let mut npc = NewNode::new(NodeKind::Character, "铁匠").under(&root.id).payload(json!({"secrets":["间谍"]}));
    npc.gm_text = "隐藏信息".into();
    let npc = mk(&mut e.world, npc);
    let city = mk(&mut e.world, NewNode::new(NodeKind::City, "白石城").under(&root.id));
    e.world.write(&user(), "r", |w| w.create_relation(NewRelation::new(&npc.id, &city.id, "located_in"))).unwrap();
    let (s, _) = e.world.write(&user(), "s", |w| w.create_script(NewScript::new("剧本"))).unwrap();
    let a = e.world.write(&user(), "p", |w| w.create_plot_node(NewPlotNode::new(&s.id, PlotNodeKind::Start, "开场"))).unwrap().0;
    let b = e.world.write(&user(), "p", |w| w.create_plot_node(NewPlotNode::new(&s.id, PlotNodeKind::Ending, "结局"))).unwrap().0;
    let mut ed = NewPlotEdge::new(&s.id, &a.id, &b.id);
    ed.condition = serde_json::from_value(json!({"type":"clue_known","clue": npc.id})).unwrap();
    e.world.write(&user(), "e", |w| w.create_plot_edge(ed)).unwrap();
    (root, npc)
}

#[test]
fn export_import_roundtrip_into_new_world() {
    let mut e = env();
    let (_, npc) = populate(&mut e);
    let file = e.dir.path().join("export.cgwpack");
    let m = export_bundle(&e.world, &file, false).unwrap();
    assert_eq!(m.format_version, BUNDLE_FORMAT_VERSION);
    assert_eq!(m.counts["nodes"], 3);
    // 不覆盖已有文件。
    assert!(export_bundle(&e.world, &file, false).is_err());

    let lib = Library::open(&e.dir.path().join("lib")).unwrap();
    let (_, w2, rep) = lib.import_world(&file, None).unwrap();
    assert_eq!(rep.inserted, 3 + 1 + 1 + 2 + 1);
    assert_eq!(w2.name().unwrap(), "测试世界");
    let npc2 = get(&w2, &npc.id).unwrap();
    assert_eq!((npc2.title.as_str(), npc2.gm_text.as_str(), &npc2.payload), (npc.title.as_str(), npc.gm_text.as_str(), &npc.payload));
    assert_eq!(npc2.path, npc.path);
    let (_, d) = read_bundle(&file).unwrap();
    assert_eq!(d.plot_edges[0].condition, serde_json::from_value(json!({"type":"clue_known","clue": npc.id})).unwrap());
}

#[test]
fn merge_strategies() {
    let mut e = env();
    let (root, npc) = populate(&mut e);
    let file = e.dir.path().join("x.cgwpack");
    export_bundle(&e.world, &file, false).unwrap();

    // 修改本地内容后再导入。
    e.world
        .write(&user(), "u", |w| w.update_node(&npc.id, NodePatch { title: Some("本地改名".into()), ..Default::default() }, None))
        .unwrap();

    let skip = import_bundle(&mut e.world, &file, ConflictStrategy::Skip, &user()).unwrap();
    assert_eq!((skip.inserted, skip.skipped), (0, 8));
    assert_eq!(get(&e.world, &npc.id).unwrap().title, "本地改名");
    assert!(skip.commit_id.is_none());

    let ow = import_bundle(&mut e.world, &file, ConflictStrategy::Overwrite, &user()).unwrap();
    assert_eq!(ow.overwritten, 8);
    assert_eq!(get(&e.world, &npc.id).unwrap().title, "铁匠");
    // 覆盖导入可以撤销。
    e.world.undo(ow.commit_id.as_deref().unwrap(), &user()).unwrap();
    assert_eq!(get(&e.world, &npc.id).unwrap().title, "本地改名");

    // 锁定内容不被覆盖。
    e.world.write(&user(), "lock", |w| w.update_node(&npc.id, NodePatch { locked: Some(true), ..Default::default() }, None)).unwrap();
    let ow = import_bundle(&mut e.world, &file, ConflictStrategy::Overwrite, &user()).unwrap();
    assert_eq!(ow.protected, 1);
    assert_eq!(get(&e.world, &npc.id).unwrap().title, "本地改名");

    // 副本导入：全部新 ID，引用一起换新。
    let cp = import_bundle(&mut e.world, &file, ConflictStrategy::Copy, &user()).unwrap();
    assert_eq!(cp.inserted, 8);
    let roots = children(e.world.conn(), None, 0, 10).unwrap();
    assert_eq!(roots.len(), 2);
    let copy_root = roots.iter().find(|n| n.id != root.id).unwrap();
    let copy_kids = children(e.world.conn(), Some(&copy_root.id), 0, 10).unwrap();
    assert_eq!(copy_kids.len(), 2);
    let copy_npc = copy_kids.iter().find(|n| n.kind == NodeKind::Character).unwrap();
    assert_eq!(relations_of(e.world.conn(), &copy_npc.id).unwrap().len(), 1);
    let scripts = chronicle_core::scripts::list_scripts(e.world.conn()).unwrap();
    let copy_script = scripts.iter().find(|s| s.id != scripts[0].id).unwrap();
    let edge = &chronicle_core::plot::edges_of_script(e.world.conn(), &copy_script.id).unwrap()[0];
    assert_eq!(edge.condition, serde_json::from_value(json!({"type":"clue_known","clue": copy_npc.id})).unwrap());
}

#[test]
fn rejects_bad_bundles_atomically() {
    let mut e = env();
    populate(&mut e);
    let before = e.world.commits(100, 0).unwrap().len();
    // 版本过新。
    let r = upgrade_bundle(json!({"format":"chronicle-gm-world","format_version":99}), json!({}));
    assert!(r.unwrap_err().to_string().contains("请升级程序"));
    assert!(upgrade_bundle(json!({"format":"other","format_version":1}), json!({})).is_err());
    // 内容非法（payload 类型错误）：整体拒绝，不写入。
    let file = e.dir.path().join("bad.cgwpack");
    export_bundle(&e.world, &file, false).unwrap();
    let (m, mut d) = read_bundle(&file).unwrap();
    let mut extra = d.nodes[0].clone();
    extra.id = chronicle_core::util::new_id();
    extra.parent_id = None;
    d.nodes.push(extra);
    let ci = d.nodes.iter().position(|n| n.kind == NodeKind::Character).unwrap();
    d.nodes[ci].payload = json!({"secrets": 1});
    assert!(import_data(&mut e.world, &m, d.clone(), ConflictStrategy::Copy, &user()).is_err());
    assert_eq!(e.world.commits(100, 0).unwrap().len(), before);
    // 父节点缺失。
    d.nodes[ci].payload = json!({});
    d.nodes[ci].parent_id = Some(chronicle_core::util::new_id());
    let r = import_data(&mut e.world, &m, d, ConflictStrategy::Copy, &user());
    assert!(r.is_err());
    assert_eq!(e.world.commits(100, 0).unwrap().len(), before);
    // 不是 zip。
    let junk = e.dir.path().join("junk.cgwpack");
    std::fs::write(&junk, b"not a zip").unwrap();
    assert!(read_bundle(&junk).is_err());
}

#[test]
fn version_snapshot_and_restore() {
    let mut e = env();
    let (_, npc) = populate(&mut e);
    let v1 = chronicle_core::versions::create_version(&e.world, "初版", "").unwrap();
    e.world.write(&user(), "d", |w| w.delete_node(&npc.id, None)).unwrap();
    let v2 = chronicle_core::versions::create_version(&e.world, "删除后", "").unwrap();
    assert!(get(&e.world, &npc.id).is_none());

    let (world, backup) = chronicle_core::versions::restore_version(e.world, &v1.id).unwrap();
    assert!(backup.exists(), "回滚前自动备份");
    assert!(get(&world, &npc.id).is_some());
    let names: Vec<_> = chronicle_core::versions::list_versions(world.conn()).unwrap().into_iter().map(|v| v.id).collect();
    assert!(names.contains(&v1.id) && names.contains(&v2.id), "回滚后版本列表保留");
    // 备份文件就是回滚前的状态（节点已删除）。
    let (b, _) = World::open(&backup, &e.dir.path().join("b2")).unwrap();
    assert!(get(&b, &npc.id).is_none());
}

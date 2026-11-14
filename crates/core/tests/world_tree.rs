mod common;

use chronicle_core::nodes::{self, *};
use chronicle_core::*;
use common::*;
use serde_json::json;

#[test]
fn create_children_in_order_and_page() {
    let mut e = env();
    let root = mk(&mut e.world, NewNode::new(NodeKind::World, "世界"));
    let a = mk(&mut e.world, NewNode::new(NodeKind::Region, "A").under(&root.id));
    let c = mk(&mut e.world, NewNode::new(NodeKind::Region, "C").under(&root.id));
    let mut b = NewNode::new(NodeKind::Region, "B").under(&root.id);
    b.position = Position::After(a.id.clone());
    mk(&mut e.world, b);
    let mut z = NewNode::new(NodeKind::Region, "Z").under(&root.id);
    z.position = Position::First;
    mk(&mut e.world, z);
    let titles: Vec<_> = children(e.world.conn(), Some(&root.id), 0, 10).unwrap().into_iter().map(|n| n.title).collect();
    assert_eq!(titles, ["Z", "A", "B", "C"]);
    let page: Vec<_> = children(e.world.conn(), Some(&root.id), 1, 2).unwrap().into_iter().map(|n| n.title).collect();
    assert_eq!(page, ["A", "B"]);
    assert_eq!(child_count(e.world.conn(), Some(&root.id)).unwrap(), 4);
    assert_eq!(child_count(e.world.conn(), None).unwrap(), 1);
    assert_eq!(c.path, format!("/{}/{}/", root.id, c.id));
}

#[test]
fn rejects_invalid_input() {
    let mut e = env();
    let r = e.world.write(&user(), "t", |w| w.create_node(NewNode::new(NodeKind::City, "  ")));
    assert!(r.is_err());
    let r = e.world.write(&user(), "t", |w| {
        w.create_node(NewNode::new(NodeKind::Character, "x").payload(json!({"secrets": 5})))
    });
    assert!(matches!(r, Err(CoreError::Invalid(_))));
    let r = e.world.write(&user(), "t", |w| w.create_node(NewNode::new(NodeKind::City, "x").under("NOPE")));
    assert!(matches!(r, Err(CoreError::NotFound(..))));
    // 失败的写入不留下 commit。
    assert!(e.world.commits(10, 0).unwrap().is_empty());
}

#[test]
fn move_subtree_rewrites_paths() {
    let mut e = env();
    let a = mk(&mut e.world, NewNode::new(NodeKind::Nation, "甲国"));
    let b = mk(&mut e.world, NewNode::new(NodeKind::Nation, "乙国"));
    let city = mk(&mut e.world, NewNode::new(NodeKind::City, "城").under(&a.id));
    let site = mk(&mut e.world, NewNode::new(NodeKind::Site, "塔").under(&city.id));
    e.world.write(&user(), "move", |w| w.move_node(&city.id, Some(&b.id), Position::Last, Some(1))).unwrap();
    let site2 = get(&e.world, &site.id).unwrap();
    assert_eq!(site2.path, format!("/{}/{}/{}/", b.id, city.id, site.id));
    assert_eq!(ancestors(e.world.conn(), &site.id).unwrap().iter().map(|n| n.title.as_str()).collect::<Vec<_>>(), ["乙国", "城"]);
    assert_eq!(subtree(e.world.conn(), &b.id, 100).unwrap().len(), 3);
    assert_eq!(subtree(e.world.conn(), &a.id, 100).unwrap().len(), 1);
    // 不能移到自己的子孙下。
    let r = e.world.write(&user(), "bad", |w| w.move_node(&b.id, Some(&site.id), Position::Last, None));
    assert!(matches!(r, Err(CoreError::Invalid(_))));
}

#[test]
fn delete_and_restore_subtree_with_relations() {
    let mut e = env();
    let a = mk(&mut e.world, NewNode::new(NodeKind::Faction, "教团"));
    let b = mk(&mut e.world, NewNode::new(NodeKind::Character, "主教").under(&a.id));
    let c = mk(&mut e.world, NewNode::new(NodeKind::Character, "旅人"));
    e.world.write(&user(), "rel", |w| w.create_relation(NewRelation::new(&c.id, &b.id, "enemy"))).unwrap();
    let (n, _) = e.world.write(&user(), "del", |w| w.delete_node(&a.id, None)).unwrap();
    assert_eq!(n, 2);
    assert!(get(&e.world, &b.id).is_none());
    assert!(relations_of(e.world.conn(), &c.id).unwrap().is_empty());
    assert_eq!(trash(e.world.conn(), 10).unwrap().len(), 1);
    e.world.write(&user(), "restore", |w| w.restore_node(&a.id)).unwrap();
    assert!(get(&e.world, &b.id).is_some());
    assert_eq!(relations_of(e.world.conn(), &c.id).unwrap().len(), 1);
}

#[test]
fn optimistic_revision_check() {
    let mut e = env();
    let n = mk(&mut e.world, NewNode::new(NodeKind::City, "城"));
    let p = NodePatch { summary: Some("v2".into()), ..Default::default() };
    e.world.write(&user(), "u", |w| w.update_node(&n.id, p.clone(), Some(1))).unwrap();
    let r = e.world.write(&user(), "u", |w| w.update_node(&n.id, p, Some(1)));
    assert!(matches!(r, Err(CoreError::RevConflict { expected: 1, found: 2, .. })));
}

#[test]
fn locked_and_core_protection() {
    let mut e = env();
    let mut nn = NewNode::new(NodeKind::MagicSystem, "魔法体系").canon(CanonLevel::Core);
    nn.locked = true;
    let n = mk(&mut e.world, nn);
    let edit = NodePatch { summary: Some("改".into()), ..Default::default() };
    // 锁定：用户也不能直接改，必须先解锁。
    assert!(matches!(
        e.world.write(&user(), "u", |w| w.update_node(&n.id, edit.clone(), None)),
        Err(CoreError::Protected(_))
    ));
    // AI 不能解锁。
    let unlock = NodePatch { locked: Some(false), ..Default::default() };
    assert!(matches!(e.world.write(&ai(), "u", |w| w.update_node(&n.id, unlock.clone(), None)), Err(CoreError::Protected(_))));
    e.world.write(&user(), "unlock", |w| w.update_node(&n.id, unlock, None)).unwrap();
    // 解锁后：核心设定 AI 仍不能改，用户可以改。
    assert!(matches!(e.world.write(&ai(), "u", |w| w.update_node(&n.id, edit.clone(), None)), Err(CoreError::Protected(_))));
    e.world.write(&user(), "u", |w| w.update_node(&n.id, edit, None)).unwrap();
    // AI 不能删除核心设定，也不能新建核心设定。
    assert!(e.world.write(&ai(), "d", |w| w.delete_node(&n.id, None)).is_err());
    assert!(e.world.write(&ai(), "c", |w| w.create_node(NewNode::new(NodeKind::City, "x").canon(CanonLevel::Core))).is_err());
    // 子树中有锁定节点时整棵子树不能删除。
    let parent = mk(&mut e.world, NewNode::new(NodeKind::Region, "区"));
    let mut child = NewNode::new(NodeKind::City, "锁城").under(&parent.id);
    child.locked = true;
    mk(&mut e.world, child);
    assert!(matches!(e.world.write(&user(), "d", |w| w.delete_node(&parent.id, None)), Err(CoreError::Protected(_))));
    assert!(get(&e.world, &parent.id).is_some());
}

#[test]
fn names_and_player_view() {
    let mut e = env();
    let mut nn = NewNode::new(NodeKind::Character, "艾莉娅·星歌").payload(json!({"identity":"吟游诗人","secrets":["王室私生女"]}));
    nn.aliases = vec!["星歌".into()];
    nn.gm_text = "她其实在为敌国做事".into();
    let n = mk(&mut e.world, nn);
    assert_eq!(find_by_name(e.world.conn(), "艾莉娅 星歌", None).unwrap().len(), 1);
    assert_eq!(find_by_name(e.world.conn(), "星歌", Some(NodeKind::Character)).unwrap().len(), 1);
    assert!(find_by_name(e.world.conn(), "星歌", Some(NodeKind::City)).unwrap().is_empty());
    let pv = n.player_view();
    assert!(pv.gm_text.is_empty());
    assert_eq!(pv.payload, json!({"identity":"吟游诗人"}));
    // 改名后名称索引同步。
    e.world
        .write(&user(), "rename", |w| {
            w.update_node(&n.id, NodePatch { title: Some("艾莉娅".into()), aliases: Some(vec![]), ..Default::default() }, None)
        })
        .unwrap();
    assert!(find_by_name(e.world.conn(), "星歌", None).unwrap().is_empty());
    assert_eq!(nodes::find_by_name(e.world.conn(), "艾莉娅", None).unwrap().len(), 1);
}

#[test]
fn relation_validation() {
    let mut e = env();
    let a = mk(&mut e.world, NewNode::new(NodeKind::Character, "甲"));
    let b = mk(&mut e.world, NewNode::new(NodeKind::Character, "乙"));
    assert!(e.world.write(&user(), "r", |w| w.create_relation(NewRelation::new(&a.id, &a.id, "self"))).is_err());
    assert!(e.world.write(&user(), "r", |w| w.create_relation(NewRelation::new(&a.id, "NOPE", "x"))).is_err());
    let mut r = NewRelation::new(&a.id, &b.id, "ally");
    r.valid_from = Some(10);
    r.valid_to = Some(5);
    assert!(e.world.write(&user(), "r", |w| w.create_relation(r)).is_err());
    let (rel, _) = e.world.write(&user(), "r", |w| w.create_relation(NewRelation::new(&a.id, &b.id, "ally"))).unwrap();
    assert_eq!(relations_between(e.world.conn(), &b.id, &a.id).unwrap().len(), 1);
    e.world.write(&user(), "d", |w| w.delete_relation(&rel.id, Some(1))).unwrap();
    assert!(relations_between(e.world.conn(), &a.id, &b.id).unwrap().is_empty());
}

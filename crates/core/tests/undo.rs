mod common;

use chronicle_core::nodes::*;
use chronicle_core::*;
use common::*;

#[test]
fn undo_and_redo_update() {
    let mut e = env();
    let n = mk(&mut e.world, NewNode::new(NodeKind::City, "旧名"));
    let (_, c) = e
        .world
        .write(&user(), "rename", |w| {
            w.update_node(&n.id, NodePatch { title: Some("新名".into()), ..Default::default() }, None)
        })
        .unwrap();
    let c = c.unwrap();
    let undo = e.world.undo(&c, &user()).unwrap().unwrap();
    assert_eq!(get(&e.world, &n.id).unwrap().title, "旧名");
    assert_eq!(get(&e.world, &n.id).unwrap().rev, 3, "撤销也会递增版本号");
    // 不能重复撤销同一 commit。
    assert!(e.world.undo(&c, &user()).is_err());
    // 撤销"撤销"即重做。
    e.world.undo(&undo, &user()).unwrap();
    assert_eq!(get(&e.world, &n.id).unwrap().title, "新名");
}

#[test]
fn undo_create_removes_and_undo_delete_restores() {
    let mut e = env();
    let (n, c) = e.world.write(&user(), "c", |w| w.create_node(NewNode::new(NodeKind::City, "城"))).unwrap();
    e.world.undo(&c.unwrap(), &user()).unwrap();
    assert!(chronicle_core::entity::Entity::load(e.world.conn(), &n.id).map(|x: Option<Node>| x.is_none()).unwrap());

    let p = mk(&mut e.world, NewNode::new(NodeKind::Region, "区"));
    let k = mk(&mut e.world, NewNode::new(NodeKind::City, "子").under(&p.id));
    let (_, c) = e.world.write(&user(), "d", |w| w.delete_node(&p.id, None)).unwrap();
    assert!(get(&e.world, &k.id).is_none());
    e.world.undo(&c.unwrap(), &user()).unwrap();
    assert!(get(&e.world, &k.id).is_some());
}

#[test]
fn undo_move_restores_descendant_paths() {
    let mut e = env();
    let a = mk(&mut e.world, NewNode::new(NodeKind::Nation, "甲"));
    let b = mk(&mut e.world, NewNode::new(NodeKind::Nation, "乙"));
    let city = mk(&mut e.world, NewNode::new(NodeKind::City, "城").under(&a.id));
    let site = mk(&mut e.world, NewNode::new(NodeKind::Site, "塔").under(&city.id));
    let (_, c) = e.world.write(&user(), "m", |w| w.move_node(&city.id, Some(&b.id), Position::First, None)).unwrap();
    e.world.undo(&c.unwrap(), &user()).unwrap();
    assert_eq!(get(&e.world, &site.id).unwrap().path, site.path);
    assert_eq!(get(&e.world, &city.id).unwrap().parent_id.as_deref(), Some(a.id.as_str()));
}

#[test]
fn undo_conflicts_with_later_edit() {
    let mut e = env();
    let n = mk(&mut e.world, NewNode::new(NodeKind::City, "城"));
    let (_, c1) = e
        .world
        .write(&user(), "e1", |w| w.update_node(&n.id, NodePatch { summary: Some("1".into()), ..Default::default() }, None))
        .unwrap();
    e.world
        .write(&user(), "e2", |w| w.update_node(&n.id, NodePatch { summary: Some("2".into()), ..Default::default() }, None))
        .unwrap();
    let r = e.world.undo(&c1.unwrap(), &user());
    assert!(matches!(r, Err(CoreError::RevConflict { .. })));
    assert_eq!(get(&e.world, &n.id).unwrap().summary, "2", "冲突时不做任何修改");
}

#[test]
fn multi_entity_commit_is_atomic() {
    let mut e = env();
    let a = mk(&mut e.world, NewNode::new(NodeKind::City, "甲"));
    let r = e.world.write(&user(), "batch", |w| {
        w.create_node(NewNode::new(NodeKind::City, "乙"))?;
        w.update_node(&a.id, NodePatch { summary: Some("x".into()), ..Default::default() }, None)?;
        w.create_node(NewNode::new(NodeKind::City, "丙").under("MISSING"))
    });
    assert!(r.is_err());
    assert_eq!(child_count(e.world.conn(), None).unwrap(), 1);
    assert_eq!(get(&e.world, &a.id).unwrap().summary, "");
    assert_eq!(e.world.commits(100, 0).unwrap().len(), 1);
}

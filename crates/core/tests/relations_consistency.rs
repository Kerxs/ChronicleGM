mod common;

use chronicle_core::consistency::check_world;
use chronicle_core::relations::*;
use chronicle_core::*;
use common::*;
use serde_json::json;

#[test]
fn update_relation_and_protection() {
    let mut e = env();
    let a = mk(&mut e.world, NewNode::new(NodeKind::Character, "甲"));
    let b = mk(&mut e.world, NewNode::new(NodeKind::Character, "乙").canon(CanonLevel::Core));
    let (r, _) = e.world.write(&user(), "r", |w| w.create_relation(NewRelation::new(&a.id, &b.id, "friend"))).unwrap();
    let p = RelationPatch { rel_type: Some("rival".into()), label: Some("宿敌".into()), valid_from: Some(Some(10)), ..Default::default() };
    let (r2, _) = e.world.write(&user(), "u", |w| w.update_relation(&r.id, p, Some(1))).unwrap();
    assert_eq!((r2.rel_type.as_str(), r2.label.as_str(), r2.valid_from, r2.rev), ("rival", "宿敌", Some(10), 2));
    let bad = RelationPatch { valid_to: Some(Some(5)), ..Default::default() };
    assert!(e.world.write(&user(), "u", |w| w.update_relation(&r.id, bad, None)).is_err());
    // AI 不能修改涉及核心设定的关系。
    let p = RelationPatch { label: Some("x".into()), ..Default::default() };
    assert!(matches!(e.world.write(&ai(), "u", |w| w.update_relation(&r.id, p, None)), Err(CoreError::Protected(_))));
    assert!(relation_type("member_of").unwrap().directed);
}

#[test]
fn neighborhood_respects_depth_and_limit() {
    let mut e = env();
    let center = mk(&mut e.world, NewNode::new(NodeKind::Character, "中心"));
    let mut first = vec![];
    for i in 0..5 {
        let n = mk(&mut e.world, NewNode::new(NodeKind::Character, format!("一度{i}")));
        e.world.write(&user(), "r", |w| w.create_relation(NewRelation::new(&center.id, &n.id, "friend"))).unwrap();
        first.push(n);
    }
    let far = mk(&mut e.world, NewNode::new(NodeKind::Character, "二度"));
    e.world.write(&user(), "r", |w| w.create_relation(NewRelation::new(&first[0].id, &far.id, "enemy"))).unwrap();
    let g1 = neighborhood(e.world.conn(), &center.id, 1, 100).unwrap();
    assert_eq!((g1.nodes.len(), g1.relations.len(), g1.truncated), (6, 5, false));
    let g2 = neighborhood(e.world.conn(), &center.id, 2, 100).unwrap();
    assert_eq!(g2.nodes.len(), 7);
    let g3 = neighborhood(e.world.conn(), &center.id, 2, 3).unwrap();
    assert_eq!(g3.nodes.len(), 3);
    assert!(g3.truncated);
    assert!(g3.relations.iter().all(|r| g3.nodes.iter().any(|n| n.id == r.src_id) && g3.nodes.iter().any(|n| n.id == r.dst_id)));
    let counts = child_counts(e.world.conn(), std::slice::from_ref(&center.id)).unwrap();
    assert_eq!(counts[0].1, 0);
}

#[test]
fn consistency_check_finds_conflicts() {
    let mut e = env();
    let a = mk(&mut e.world, NewNode::new(NodeKind::Faction, "铁卫"));
    let b = mk(&mut e.world, NewNode::new(NodeKind::Faction, "影刃"));
    e.world.write(&user(), "r", |w| w.create_relation(NewRelation::new(&a.id, &b.id, "ally"))).unwrap();
    e.world.write(&user(), "r", |w| w.create_relation(NewRelation::new(&b.id, &a.id, "enemy"))).unwrap();
    // 不重叠的时间段不算矛盾。
    let c = mk(&mut e.world, NewNode::new(NodeKind::Faction, "银月"));
    let mut r1 = NewRelation::new(&a.id, &c.id, "alliance");
    r1.valid_to = Some(100);
    let mut r2 = NewRelation::new(&a.id, &c.id, "war");
    r2.valid_from = Some(200);
    e.world.write(&user(), "r", |w| { w.create_relation(r1)?; w.create_relation(r2) }).unwrap();

    let king = mk(&mut e.world, NewNode::new(NodeKind::Character, "老国王").payload(json!({"death_time": 100, "birth_time": 20})));
    let late = mk(&mut e.world, NewNode::new(NodeKind::Event, "灰烬之战").payload(json!({"world_time": 150})));
    let early = mk(&mut e.world, NewNode::new(NodeKind::Event, "开国").payload(json!({"world_time": 10})));
    let ok = mk(&mut e.world, NewNode::new(NodeKind::Event, "加冕").payload(json!({"world_time": 50})));
    for ev in [&late, &early, &ok] {
        e.world.write(&user(), "r", |w| w.create_relation(NewRelation::new(&king.id, &ev.id, "participated_in"))).unwrap();
    }
    mk(&mut e.world, NewNode::new(NodeKind::City, "白石城"));
    let mut dup = NewNode::new(NodeKind::City, "灰港");
    dup.aliases = vec!["白石 城".into()];
    mk(&mut e.world, dup);
    mk(&mut e.world, NewNode::new(NodeKind::Character, "白石城"));

    let f = check_world(e.world.conn()).unwrap();
    let codes: Vec<_> = f.iter().map(|x| x.code.as_str()).collect();
    assert_eq!(codes.iter().filter(|c| **c == "relation_conflict").count(), 1, "{f:#?}");
    assert_eq!(codes.iter().filter(|c| **c == "timeline_conflict").count(), 2, "{f:#?}");
    let dups: Vec<_> = f.iter().filter(|x| x.code == "duplicate_name").collect();
    assert_eq!(dups.len(), 1, "不同类型同名不算重复：{dups:#?}");
    assert_eq!(dups[0].node_ids.len(), 2);
    assert!(!codes.contains(&"dangling_relation"));
}

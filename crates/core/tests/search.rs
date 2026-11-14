mod common;

use chronicle_core::search::*;
use chronicle_core::*;
use common::*;

fn titles(hits: &[SearchHit]) -> Vec<String> {
    hits.iter().map(|h| h.node.title.clone()).collect()
}

#[test]
fn chinese_full_text_and_short_fallback() {
    let mut e = env();
    let mut n = NewNode::new(NodeKind::City, "白石城");
    n.summary = "建在古代巨人遗骸之上的要塞".into();
    mk(&mut e.world, n);
    let mut g = NewNode::new(NodeKind::Character, "张三");
    g.gm_text = "实际上是龙族的间谍".into();
    mk(&mut e.world, g);
    mk(&mut e.world, NewNode::new(NodeKind::City, "黑石港"));

    let all = SearchOptions { limit: 10, ..Default::default() };
    let hits = search_nodes(e.world.conn(), "巨人遗骸", &all).unwrap();
    assert_eq!(titles(&hits), ["白石城"]);
    assert!(hits[0].indexed);

    // 两个字：回退 LIKE。
    let hits = search_nodes(e.world.conn(), "石城", &all).unwrap();
    assert_eq!(titles(&hits), ["白石城"]);
    assert!(!hits[0].indexed);
    let hits = search_nodes(e.world.conn(), "张三", &all).unwrap();
    assert_eq!(titles(&hits), ["张三"]);

    // 主持人文本：GM 视角可搜，玩家视角不可搜。
    assert_eq!(titles(&search_nodes(e.world.conn(), "龙族的间谍", &all).unwrap()), ["张三"]);
    let pv = SearchOptions { player_view: true, limit: 10, ..Default::default() };
    assert!(search_nodes(e.world.conn(), "龙族的间谍", &pv).unwrap().is_empty());
    assert!(search_nodes(e.world.conn(), "间谍", &pv).unwrap().is_empty());

    // 类型过滤。
    let cities = SearchOptions { kinds: vec![NodeKind::Character], limit: 10, ..Default::default() };
    assert!(search_nodes(e.world.conn(), "白石城", &cities).unwrap().is_empty());

    // 特殊字符不会破坏查询。
    assert!(search_nodes(e.world.conn(), "\"OR* NEAR(", &all).unwrap().is_empty());
    assert!(search_nodes(e.world.conn(), "%_", &all).unwrap().is_empty());
}

#[test]
fn index_follows_updates_and_deletes() {
    let mut e = env();
    let n = mk(&mut e.world, NewNode::new(NodeKind::City, "翡翠之都"));
    let all = SearchOptions { limit: 10, ..Default::default() };
    e.world
        .write(&user(), "r", |w| w.update_node(&n.id, NodePatch { title: Some("琥珀之都".into()), ..Default::default() }, None))
        .unwrap();
    assert!(search_nodes(e.world.conn(), "翡翠之都", &all).unwrap().is_empty());
    assert_eq!(search_nodes(e.world.conn(), "琥珀之都", &all).unwrap().len(), 1);
    e.world.write(&user(), "d", |w| w.delete_node(&n.id, None)).unwrap();
    assert!(search_nodes(e.world.conn(), "琥珀之都", &all).unwrap().is_empty());
    let (_, c) = e.world.write(&user(), "c", |w| w.create_node(NewNode::new(NodeKind::City, "临时之城"))).unwrap();
    e.world.undo(&c.unwrap(), &user()).unwrap();
    assert!(search_nodes(e.world.conn(), "临时之城", &all).unwrap().is_empty());
}

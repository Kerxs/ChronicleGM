//! 人物关系网络：有向看法、来源事件与历史、世界关系与战役关系合并、筛选与搜索、
//! 关系随时间线回溯恢复并参与重放校验与结局比较。

mod common;

use chronicle_core::campaign;
use chronicle_core::network::{self, NetworkQuery};
use chronicle_core::timeline;
use chronicle_core::*;
use common::*;
use serde_json::json;

fn adj(w: &mut World, c: &str, v: serde_json::Value) {
    let e: Vec<chronicle_rules::Effect> = serde_json::from_value(v).unwrap();
    campaign::apply_effects(w, c, &e, "测试").unwrap();
}

struct Fx {
    e: Env,
    elder: String,
    smith: String,
    guild: String,
    camp: String,
}

fn fx() -> Fx {
    let mut e = env();
    let elder = mk(&mut e.world, NewNode::new(NodeKind::Character, "村长老格")).id;
    let smith = mk(&mut e.world, NewNode::new(NodeKind::Character, "铁匠阿岩")).id;
    let guild = mk(&mut e.world, NewNode::new(NodeKind::Faction, "铁匠行会")).id;
    let _town = mk(&mut e.world, NewNode::new(NodeKind::City, "灰石村")).id;
    e.world.write(&user(), "r", |w| w.create_relation(NewRelation::new(&smith, &guild, "member_of"))).unwrap();
    e.world.write(&user(), "r", |w| w.create_relation(NewRelation::new(&elder, &smith, "friend"))).unwrap();
    let camp = campaign::create_campaign(&mut e.world, None, "关系测试", Some(3)).unwrap().id;
    Fx { e, elder, smith, guild, camp }
}

#[test]
fn bonds_are_directional_with_event_sourced_history() {
    let mut f = fx();
    let (w, c) = (&mut f.e.world, f.camp.clone());
    campaign::record(w.conn(), &c, "player", &json!({"text": "我替村长修好了屋顶"})).unwrap();
    adj(w, &c, json!([{"type": "adjust_bond", "from": f.elder, "to": "party", "kind": "friend", "trust": 15, "affection": 15, "strength": 10, "reason": "玩家帮他修好了屋顶"}]));
    campaign::record(w.conn(), &c, "player", &json!({"text": "我当众揭穿铁匠偷了钥匙"})).unwrap();
    adj(w, &c, json!([
        {"type": "adjust_bond", "from": f.smith, "to": "party", "kind": "enemy", "hostility": 15, "affection": -15, "strength": 10, "reason": "玩家当众揭穿他"},
        {"type": "adjust_bond", "from": "party", "to": f.smith, "trust": -10, "reason": "发现他是小偷"}
    ]));
    // 双方看法不同：铁匠敌视玩家，玩家只是不再信任铁匠。
    let net = network::network(w.conn(), &NetworkQuery { campaign_id: Some(c.clone()), include_world: true, ..Default::default() }).unwrap();
    let e = net.edges.iter().find(|e| e.source == "campaign" && (e.a == f.smith || e.b == f.smith)).unwrap();
    let (smith_view, party_view) = if e.a == f.smith { (e.a_to_b.unwrap(), e.b_to_a.unwrap()) } else { (e.b_to_a.unwrap(), e.a_to_b.unwrap()) };
    assert_eq!((smith_view.hostility, smith_view.affection), (15, -15));
    assert_eq!((party_view.hostility, party_view.trust), (0, 0), "信任下限是 0");
    assert!(!e.directed, "双方都有看法时是双向边");
    let elder_edge = net.edges.iter().find(|e| e.source == "campaign" && (e.a == f.elder || e.b == f.elder)).unwrap();
    assert!(elder_edge.directed, "只有一方的看法时是单向");
    assert!(net.nodes.iter().any(|n| n.id == "party" && n.title == "玩家"));
    // 世界关系也在网络里；城市不是人物网络的节点。
    assert!(net.edges.iter().any(|e| e.source == "world" && e.rel_type == "member_of"));
    assert!(!net.nodes.iter().any(|n| n.title == "灰石村"));
    // 历史：每次变化都有来源事件、原因、前后数值和当时的玩家行动。
    let h = network::bond_history(w.conn(), &c, &f.smith, "party").unwrap();
    assert_eq!(h.len(), 2);
    assert!(h.iter().all(|x| x.seq > 0 && !x.reason.is_empty()));
    let first = h.iter().find(|x| x.from == f.smith).unwrap();
    assert_eq!(first.player_action, "我当众揭穿铁匠偷了钥匙");
    assert_eq!((first.old.hostility, first.new.hostility, first.kind.as_str()), (0, 15, "enemy"));
    // 人物档案里的关系列表。
    let mine = network::bonds_of_person(w.conn(), &c, &f.smith).unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].other_title, "玩家");
    assert!(mine[0].outgoing.is_some() && mine[0].incoming.is_some());
}

#[test]
fn filters_search_and_campaign_overrides() {
    let mut f = fx();
    let (w, c) = (&mut f.e.world, f.camp.clone());
    adj(w, &c, json!([{"type": "adjust_bond", "from": f.elder, "to": "party", "strength": 5, "trust": 5, "reason": "初次见面"}]));
    adj(w, &c, json!([{"type": "set_relation", "a": f.elder, "b": f.smith, "rel_type": "friend", "active": false}]));
    adj(w, &c, json!([{"type": "set_entity_state", "entity": f.smith, "field": "life_status", "value": "dead"}]));
    let q = |extra: NetworkQuery| network::network(w.conn(), &NetworkQuery { campaign_id: Some(c.clone()), include_world: true, ..extra }).unwrap();
    let all = q(NetworkQuery::default());
    let friend = all.edges.iter().find(|e| e.rel_type == "friend" && e.source == "world").unwrap();
    assert!(friend.inactive, "战役中友谊破裂，世界设定不变");
    assert_eq!(all.nodes.iter().find(|n| n.id == f.smith).unwrap().status.as_deref(), Some("dead"));
    // 按关系类型筛选。
    let members = q(NetworkQuery { rel_types: vec!["member_of".into()], ..Default::default() });
    assert!(members.edges.iter().all(|e| e.rel_type == "member_of"));
    // 按强度筛选：强度 5 的关系在门槛 10 时隐藏。
    let strong = q(NetworkQuery { min_strength: 10, ..Default::default() });
    assert!(!strong.edges.iter().any(|e| e.source == "campaign"));
    // 搜索：只取名称匹配的节点及其直接关系对象。
    let s = q(NetworkQuery { search: "行会".into(), ..Default::default() });
    let ids: Vec<&str> = s.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains(&f.guild.as_str()) && ids.contains(&f.smith.as_str()), "带出直接关系对象：{ids:?}");
    assert!(!ids.contains(&f.elder.as_str()));
    // 没有战役时只看世界关系。
    let world_only = network::network(w.conn(), &NetworkQuery { include_world: true, ..Default::default() }).unwrap();
    assert!(world_only.edges.iter().all(|e| e.source == "world"));
}

#[test]
fn bonds_follow_timelines_and_verification() {
    let mut f = fx();
    let (w, c) = (&mut f.e.world, f.camp.clone());
    adj(w, &c, json!([{"type": "adjust_bond", "from": f.elder, "to": "party", "trust": 10, "reason": "第一回合"}]));
    timeline::checkpoint(w.conn(), &c, "turn_end").unwrap();
    let cp = timeline::checkpoints(w.conn(), &c).unwrap().last().unwrap().id;
    adj(w, &c, json!([{"type": "adjust_bond", "from": f.elder, "to": "party", "hostility": 15, "trust": -10, "reason": "第二回合被欺骗"}]));
    timeline::checkpoint(w.conn(), &c, "turn_end").unwrap();
    timeline::end_campaign(w, &c, "测试").unwrap();
    assert!(timeline::verify(w.conn(), &c).unwrap().ok, "关系变化可以从事件重放");
    let b = timeline::fork(w, &c, cp, "", false).unwrap();
    let hist = network::bond_history(w.conn(), &b.id, &f.elder, "party").unwrap();
    assert_eq!(hist.len(), 1, "新时间线只有分叉点之前的关系变化");
    let bonds = network::bonds_of_person(w.conn(), &b.id, &f.elder).unwrap();
    assert_eq!(bonds[0].outgoing.unwrap().trust, 10);
    assert_eq!(bonds[0].outgoing.unwrap().hostility, 0);
    assert_eq!(network::bonds_of_person(w.conn(), &c, &f.elder).unwrap()[0].outgoing.unwrap().hostility, 15, "原时间线不变");
    let cmp = timeline::compare(w.conn(), &[c.clone(), b.id.clone()]).unwrap();
    let row = cmp.rows.iter().find(|r| r.category == "人物关系").unwrap();
    assert!(row.differs && row.key.contains("村长老格 对 玩家"), "{row:?}");
    // 篡改检查点里的关系 → 重放比对发现。
    let id = timeline::checkpoints(w.conn(), &c).unwrap().iter().rev().nth(1).unwrap().id;
    let json: String = w.conn().query_row("SELECT state FROM campaign_checkpoints WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert!(json.contains(",15]"), "检查点里关系的格式：{json}");
    let forged = json.replace(",15]", ",99]");
    w.conn().execute("UPDATE campaign_checkpoints SET state = ?2, checksum = ?3 WHERE id = ?1", rusqlite::params![id, forged, timeline::checksum(&forged)]).unwrap();
    let rep = timeline::verify(w.conn(), &c).unwrap();
    assert!(rep.mismatches.iter().any(|m| m.contains("人物关系")), "{rep:#?}");
}

#[test]
fn large_network_query_is_fast_and_bounded() {
    let mut e = env();
    let mut ids = vec![];
    e.world
        .write(&user(), "bulk", |w| {
            for i in 0..3000 {
                ids.push(w.create_node(NewNode::new(if i % 10 == 0 { NodeKind::Faction } else { NodeKind::Character }, format!("人物{i}")))?.id);
            }
            for i in 0..6000 {
                w.create_relation(NewRelation::new(&ids[i % 3000], &ids[(i * 7 + 1) % 3000], "friend"))?;
            }
            Ok(())
        })
        .unwrap();
    let t = std::time::Instant::now();
    let net = network::network(e.world.conn(), &NetworkQuery { include_world: true, ..Default::default() }).unwrap();
    let ms = t.elapsed().as_millis();
    assert!(net.truncated && net.nodes.len() <= 600, "默认最多 400 个节点（加上关系对象最多再 200）：{}", net.nodes.len());
    assert!(ms < 2000, "{ms} ms");
    let t = std::time::Instant::now();
    let s = network::network(e.world.conn(), &NetworkQuery { include_world: true, search: "人物123".into(), ..Default::default() }).unwrap();
    assert!(s.nodes.iter().any(|n| n.title == "人物123") && s.nodes.len() < 60);
    assert!(t.elapsed().as_millis() < 500);
}

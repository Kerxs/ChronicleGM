//! 第二阶段：剧透过滤、运行监控统计、token 估算、v12→v13 记忆压缩字段。

mod common;

use chronicle_core::library::{Library, LlmRequestRecord};
use chronicle_core::*;
use common::*;
use serde_json::json;

#[test]
fn spoiler_filters_hide_gm_relations_secrets_and_events() {
    let mut e = env();
    let w = &mut e.world;
    let a = w.write(&user(), "n", |x| x.create_node(NewNode::new(NodeKind::Character, "领主").payload(json!({"identity": "领主", "secrets": ["其实是叛徒"]})))).unwrap().0;
    let b = w.write(&user(), "n", |x| x.create_node(NewNode::new(NodeKind::Character, "刺客"))).unwrap().0;
    let mut open = NewRelation::new(&a.id, &b.id, "knows");
    open.label = "认识".into();
    w.write(&user(), "r", |x| x.create_relation(open)).unwrap();
    let mut hidden = NewRelation::new(&a.id, &b.id, "employs");
    hidden.visibility = Visibility::Gm;
    w.write(&user(), "r", |x| x.create_relation(hidden)).unwrap();

    let mut g = relations::neighborhood(w.conn(), &a.id, 1, 50).unwrap();
    assert_eq!(g.relations.len(), 2);
    spoiler::filter_graph(&mut g.nodes, &mut g.relations, false);
    assert_eq!(g.relations.len(), 1, "主持人关系隐藏");
    let lord = g.nodes.iter().find(|n| n.id == a.id).unwrap();
    assert!(lord.payload.get("secrets").is_none(), "秘密字段去掉");
    assert_eq!(lord.payload["identity"], "领主");

    let q = |spoilers| network::NetworkQuery { include_world: true, spoilers, ..Default::default() };
    let rels = |n: &network::Network| n.edges.iter().filter(|e| e.source == "world").count();
    assert_eq!(rels(&network::network(w.conn(), &q(false)).unwrap()), 1);
    assert_eq!(rels(&network::network(w.conn(), &q(true)).unwrap()), 2);

    let c = campaign::create_campaign(w, None, "x", Some(1)).unwrap();
    campaign::record(w.conn(), &c.id, "narration", &json!({"text": "雨夜", "leaks": ["其实是叛徒"]})).unwrap();
    campaign::record(w.conn(), &c.id, "effects_rejected", &json!({"rejected": []})).unwrap();
    let mut ev = campaign::events(w.conn(), &c.id, 0, 100).unwrap();
    spoiler::filter_events(&mut ev, false);
    assert!(ev.iter().all(|e| e.kind != "effects_rejected"));
    assert!(ev.iter().all(|e| e.payload.get("leaks").is_none()), "泄露检查里是秘密原文，也要去掉");
}

#[test]
fn run_stats_aggregate_and_unavailable_metrics_stay_empty() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(dir.path()).unwrap();
    let rec = |lat, ft: Option<i64>, speed: Option<f64>| LlmRequestRecord {
        provider_id: "p".into(), model: "m".into(), task: "narration".into(), ok: true, attempts: 1,
        prompt_tokens: Some(4000), completion_tokens: Some(300), latency_ms: Some(lat), first_token_ms: ft, predicted_per_second: speed,
        context_length: Some(32768), ..Default::default()
    };
    lib.log_request(&rec(10_000, Some(1500), Some(14.0))).unwrap();
    lib.log_request(&rec(20_000, None, Some(10.0))).unwrap();
    lib.log_request(&LlmRequestRecord { provider_id: "t".into(), task: "tts".into(), ok: true, attempts: 1, latency_ms: Some(2000), audio_ms: Some(5000), ..Default::default() }).unwrap();
    lib.log_request(&LlmRequestRecord { provider_id: "p".into(), task: "narration".into(), ok: false, attempts: 3, error: Some("超时".into()), ..Default::default() }).unwrap();
    let st = lib.run_stats(0).unwrap();
    let n = st.iter().find(|s| s.task == "narration").unwrap();
    assert_eq!((n.requests, n.failures), (3, 1));
    assert_eq!(n.latency_ms.avg, Some(15_000.0), "失败的请求不算进耗时");
    assert_eq!((n.first_token_ms.n, n.first_token_ms.avg), (1, Some(1500.0)), "测不到的不参与平均");
    assert_eq!((n.predicted_per_second.min, n.predicted_per_second.max), (Some(10.0), Some(14.0)));
    assert!((n.context_usage.avg.unwrap() - 4000.0 / 32768.0).abs() < 1e-9);
    assert_eq!(n.prompt_per_second.n, 0, "服务没给就是不可用");
    assert_eq!(n.prompt_per_second.avg, None);
    let t = st.iter().find(|s| s.task == "tts").unwrap();
    assert_eq!(t.tts_rtf.avg, Some(0.4));
    assert_eq!(lib.recent_requests(2).unwrap().len(), 2);
}

#[test]
fn token_estimate_counts_cjk_as_one() {
    assert_eq!(util::estimate_tokens("你好世界"), 4);
    assert_eq!(util::estimate_tokens("hello world!"), 3);
    assert_eq!(util::estimate_tokens(""), 0);
}

#[test]
fn multi_condition_filters_on_payload_relations_and_location() {
    let mut e = env();
    let w = &mut e.world;
    let mk = |w: &mut World, k: NodeKind, t: &str, parent: Option<&str>, p: serde_json::Value| {
        let mut n = NewNode::new(k, t).payload(p);
        n.parent_id = parent.map(str::to_string);
        w.write(&user(), "n", |x| x.create_node(n)).unwrap().0.id
    };
    let city = mk(w, NodeKind::City, "白石城", None, json!({}));
    let guild = mk(w, NodeKind::Faction, "盗贼公会", None, json!({}));
    let a = mk(w, NodeKind::Character, "阿岩", Some(&city), json!({"life_status": "alive", "age": 30}));
    let _b = mk(w, NodeKind::Character, "老格", Some(&city), json!({"life_status": "dead", "age": 70}));
    let _c = mk(w, NodeKind::Character, "流浪者", None, json!({"life_status": "alive", "age": 50}));
    mk(w, NodeKind::Item, "霜纹长剑", None, json!({"rarity": "rare", "value": 500}));
    mk(w, NodeKind::Item, "木棍", None, json!({"rarity": "common", "value": 1}));
    w.write(&user(), "r", |x| x.create_relation(NewRelation::new(&a, &guild, "member_of"))).unwrap();
    let f = |kinds: Vec<NodeKind>, conds: Vec<(&str, &str, &str)>| {
        let filter = search::NodeFilter { kinds, conds: conds.into_iter().map(|(f, o, v)| search::FieldCond { field: f.into(), op: o.into(), value: v.into() }).collect(), ..Default::default() };
        search::filter_nodes(w.conn(), &filter).unwrap().into_iter().map(|n| n.title).collect::<Vec<_>>()
    };
    assert_eq!(f(vec![NodeKind::Character], vec![("life_status", "eq", "alive"), ("under", "eq", &city)]), vec!["阿岩"]);
    assert_eq!(f(vec![NodeKind::Character], vec![("age", "gte", "40")]), vec!["流浪者", "老格"]);
    assert_eq!(f(vec![], vec![("related", "eq", &guild)]), vec!["阿岩"]);
    assert_eq!(f(vec![NodeKind::Item], vec![("rarity", "eq", "rare")]), vec!["霜纹长剑"]);
    assert!(search::filter_nodes(w.conn(), &search::NodeFilter { conds: vec![search::FieldCond { field: "x'; DROP TABLE nodes; --".into(), op: "eq".into(), value: "1".into() }], ..Default::default() }).is_err(), "字段名不能注入");
    let fields = search::payload_fields(w.conn(), Some(NodeKind::Item)).unwrap();
    let rarity = fields.iter().find(|x| x.field == "rarity").unwrap();
    assert!(rarity.samples.contains(&"rare".to_string()));
    assert!(fields.iter().find(|x| x.field == "value").unwrap().numeric);
}

#[test]
fn deletions_go_to_trash_and_keep_a_trace() {
    let mut e = env();
    let w = &mut e.world;
    let c = campaign::create_campaign(w, None, "主线", Some(1)).unwrap();
    campaign::add_fact(w.conn(), &c.id, "玩家答应了村长").unwrap();
    campaign::add_fact(w.conn(), &c.id, "玩家欠了铁匠钱").unwrap();
    let facts = campaign::facts_with_seq(w.conn(), &c.id, 10).unwrap();
    campaign::forget_fact(w.conn(), &c.id, facts.iter().find(|f| f.1.contains("铁匠")).unwrap().0).unwrap();
    assert_eq!(campaign::facts(w.conn(), &c.id, 10).unwrap(), vec!["玩家答应了村长"]);
    let npc = w.write(&user(), "n", |x| x.create_node(NewNode::new(NodeKind::Character, "村长"))).unwrap().0.id;
    let m = campaign::add_npc_memory(w, &c.id, &npc, "玩家很有礼貌", 3).unwrap();
    campaign::delete_npc_memory(w.conn(), &c.id, &m.id).unwrap();
    assert!(campaign::npc_memories(w.conn(), &c.id, &npc, 10).unwrap().is_empty());
    assert!(campaign::events(w.conn(), &c.id, 0, 100).unwrap().iter().any(|e| e.kind == "npc_memory_deleted" && e.payload["content"] == "玩家很有礼貌"), "删掉的记忆内容留在事件日志里");
    // 战役进回收站、恢复、永久删除。
    assert_eq!(campaign::trash_campaign(w.conn(), &c.id).unwrap(), 1);
    assert!(campaign::list_campaigns(w.conn()).unwrap().is_empty());
    assert_eq!(campaign::campaign_trash(w.conn()).unwrap().len(), 1);
    campaign::restore_campaign(w.conn(), &c.id).unwrap();
    assert_eq!(campaign::list_campaigns(w.conn()).unwrap().len(), 1);
    assert!(campaign::purge_campaign(w.conn(), &c.id).is_err(), "不在回收站里的不能永久删除");
    campaign::trash_campaign(w.conn(), &c.id).unwrap();
    assert!(campaign::purge_campaign(w.conn(), &c.id).unwrap() > 1);
    let left: i64 = w.conn().query_row("SELECT COUNT(*) FROM event_log WHERE campaign_id = ?1", [&c.id], |r| r.get(0)).unwrap();
    assert_eq!(left, 0);
}

#[test]
fn world_trash_and_restore() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(dir.path()).unwrap();
    let (info, w) = lib.create_world("要删的世界").unwrap();
    drop(w);
    lib.trash_world(&info.id).unwrap();
    assert!(lib.list_worlds().unwrap().is_empty());
    assert_eq!(lib.world_trash().unwrap()[0].name, "要删的世界");
    let back = lib.restore_world(&info.id).unwrap();
    assert_eq!(back.name, "要删的世界");
    assert!(lib.open_world(&info.id).is_ok(), "恢复后能打开");
    assert!(lib.world_trash().unwrap().is_empty());
}

#[test]
fn purge_restores_append_only_guard() {
    let mut e = env();
    let w = &mut e.world;
    let c = campaign::create_campaign(w, None, "x", Some(1)).unwrap();
    let keep = campaign::create_campaign(w, None, "留着", Some(1)).unwrap();
    campaign::record(w.conn(), &keep.id, "player", &json!({"text": "hi"})).unwrap();
    campaign::trash_campaign(w.conn(), &c.id).unwrap();
    campaign::purge_campaign(w.conn(), &c.id).unwrap();
    assert!(w.conn().execute("DELETE FROM event_log WHERE campaign_id = ?1", [&keep.id]).is_err(), "删除后事件日志仍然只能追加");
    assert!(!campaign::events(w.conn(), &keep.id, 0, 10).unwrap().is_empty(), "别的战役不受影响");
}

#[test]
fn campaign_takes_currency_and_rarity_names_from_script_genre() {
    let mut e = env();
    let w = &mut e.world;
    let mut ns = NewScript::new("星港");
    ns.genre = "科幻".into();
    let s = w.write(&user(), "s", |x| x.create_script(ns)).unwrap().0;
    let gp = profile::for_script(&s);
    assert_eq!(gp.currency, vec!["信用点"]);
    let c = campaign::create_campaign(w, Some(&s.id), "x", Some(1)).unwrap();
    let r = rpg::rules(w.conn(), &c.id);
    assert_eq!(r.currency, vec!["信用点"]);
    assert_eq!(rpg::format_money(&r, 250), "250 信用点");
    assert_eq!(rpg::rarity_label(&r, "legendary"), "原型");
    // 默认（奇幻）仍是金银铜。
    let plain = campaign::create_campaign(w, None, "y", Some(1)).unwrap();
    assert_eq!(rpg::rarity_label(&rpg::rules(w.conn(), &plain.id), "rare"), "稀有");
}

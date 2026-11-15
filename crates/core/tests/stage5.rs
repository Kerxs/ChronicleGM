//! 第五阶段：地点环境与检定修正、叙事风格与说话风格、线索追踪、传闻传播与失真、剧情质量检查。

mod common;

use chronicle_core::*;
use common::*;
use serde_json::json;

#[test]
fn local_environment_light_weather_and_check_modifiers() {
    let mut e = env();
    let w = &mut e.world;
    let mk = |w: &mut World, n: NewNode| w.write(&user(), "n", |x| x.create_node(n)).unwrap().0.id;
    let bay = mk(w, NewNode::new(NodeKind::Region, "雾湾"));
    let cave = mk(w, NewNode::new(NodeKind::Site, "潮洞").under(&bay));
    let pier = mk(w, NewNode::new(NodeKind::Site, "码头").under(&bay));
    let c = campaign::create_campaign(w, None, "x", Some(4)).unwrap();
    let noon = 12 * 60;
    let in_cave = worldsim::local_environment(w.conn(), &c.id, Some(&cave), noon).unwrap();
    assert!(in_cave.indoor && in_cave.light == "漆黑", "{in_cave:?}");
    let (m, why) = worldsim::env_check_modifier(&in_cave, "察觉", false);
    assert_eq!(m, -4);
    assert!(why.contains("漆黑"));
    assert_eq!(worldsim::env_check_modifier(&in_cave, "察觉", true).0, -1, "有光源");
    assert_eq!(worldsim::env_check_modifier(&in_cave, "说服", false).0, 0, "说话不受光线影响");
    let at_pier = worldsim::local_environment(w.conn(), &c.id, Some(&pier), noon).unwrap();
    assert!(!at_pier.indoor);
    let night = worldsim::local_environment(w.conn(), &c.id, Some(&pier), 23 * 60).unwrap();
    assert_eq!(night.light, "漆黑", "夜里户外没有光");
    // 同一地区内天气一致。
    assert_eq!(worldsim::local_environment(w.conn(), &c.id, Some(&pier), noon).unwrap().weather, at_pier.weather);
}

#[test]
fn narrative_style_is_described_for_the_gm() {
    let s = profile::Style { tone: "冷硬".into(), detail: 1, dialogue: 3, action: 0, inner: 1, scenery: 2 };
    let d = s.describe();
    assert!(d.contains("风格：冷硬") && d.contains("简洁") && d.contains("对话多") && d.contains("心理少") && d.contains("环境适中"), "{d}");
    assert!(!d.contains("动作"), "没设置的项不写");
    assert_eq!(profile::Style::default().describe(), "");
}

#[test]
fn clues_show_source_people_unlock_and_unresolved_state() {
    let mut e = env();
    let w = &mut e.world;
    let npc = w.write(&user(), "n", |x| x.create_node(NewNode::new(NodeKind::Character, "钟表匠老魏"))).unwrap().0.id;
    let s = w.write(&user(), "s", |x| x.create_script(NewScript::new("钟楼"))).unwrap().0;
    let clue = |w: &mut World, t: &str, p: serde_json::Value| {
        let mut it = NewScriptItem::new(&s.id, ScriptItemKind::Clue, t);
        it.payload = p;
        w.write(&user(), "i", |x| x.create_script_item(it)).unwrap().0.id
    };
    let gear = clue(w, "卡住的齿轮", json!({"found_at": "钟楼顶层", "related": [npc], "unlock_when": "上钟楼并检查大钟"}));
    let _books = clue(w, "假账本", json!({"location_name": "商会", "unlock_when": "翻看商会账本"}));
    let c = campaign::create_campaign(w, Some(&s.id), "x", Some(1)).unwrap();
    campaign::apply_effects(w, &c.id, &[rules::Effect::RevealClue { clue: gear.clone() }], "x").unwrap();
    let b = investigation::board(w.conn(), &c.id, true).unwrap();
    let g = b.clues.iter().find(|x| x.id == gear).unwrap();
    assert_eq!((g.found_at.as_str(), g.related.clone()), ("钟楼顶层", vec!["钟表匠老魏".to_string()]));
    assert!(g.unresolved, "还没被推理用到");
    assert_eq!(b.hidden.len(), 1);
    assert_eq!(b.hidden[0].unlock, "翻看商会账本");
    assert!(investigation::board(w.conn(), &c.id, false).unwrap().hidden.is_empty(), "玩家视角看不到没发现的线索");
    investigation::save_hypothesis(w, &c.id, None, &investigation::HypothesisInput { text: "有人故意卡住了钟".into(), evidence: vec![gear.clone()], ..Default::default() }).unwrap();
    assert!(!investigation::board(w.conn(), &c.id, true).unwrap().clues.iter().find(|x| x.id == gear).unwrap().unresolved, "用作证据后不再是未解决");
}

#[test]
fn rumors_spread_by_place_and_distort_with_lower_credibility() {
    let mut e = env();
    let w = &mut e.world;
    let mk = |w: &mut World, n: NewNode| w.write(&user(), "n", |x| x.create_node(n)).unwrap().0.id;
    let town = mk(w, NewNode::new(NodeKind::City, "白石城"));
    let tavern = mk(w, NewNode::new(NodeKind::Site, "酒馆").under(&town));
    let market = mk(w, NewNode::new(NodeKind::Site, "集市").under(&town));
    let at = |loc: &str| json!({"schedule": [{"start": "00:00", "end": "23:59", "activity": "待着", "location": loc}]});
    let a = mk(w, NewNode::new(NodeKind::Character, "当事人").payload(at(&tavern)));
    let b = mk(w, NewNode::new(NodeKind::Character, "酒客").payload(at(&tavern)));
    let far = mk(w, NewNode::new(NodeKind::Character, "集市小贩").payload(at(&market)));
    let c = campaign::create_campaign(w, None, "x", Some(3)).unwrap();
    let events = vec![("npc_action".to_string(), "当事人在码头打伤了税吏".to_string(), vec![a.clone()])];
    let eff = npcsim::rumor_day(w.conn(), &c.id, 2, 3, &events).unwrap();
    campaign::apply_effects(w, &c.id, &eff, "x").unwrap();
    let ka = npcsim::rumors_known_by(w.conn(), &c.id, &a, 5).unwrap();
    assert_eq!((ka[0].hop, ka[0].credibility), (0, 100), "当事人第一手");
    let kb = npcsim::rumors_known_by(w.conn(), &c.id, &b, 5).unwrap();
    assert_eq!(kb.len(), 1, "同在酒馆的人当天就听说了");
    assert_eq!((kb[0].hop, kb[0].credibility), (1, 85));
    assert!(kb[0].text.starts_with("听说"));
    assert!(npcsim::rumors_known_by(w.conn(), &c.id, &far, 5).unwrap().is_empty(), "不在一起的人没听说");
    // 暗中进行的事不会传开。
    let secret = vec![("npc_secret".to_string(), "秘密".to_string(), vec![a.clone()])];
    assert!(npcsim::rumor_day(w.conn(), &c.id, 3, 3, &secret).unwrap().is_empty() || npcsim::rumors(w.conn(), &c.id).unwrap().iter().all(|r| r.truth != "秘密"));
    assert!(npcsim::distort("x", 3, 1).contains("x"));
}

#[test]
fn script_item_changes_point_to_related_world_nodes() {
    let mut e = env();
    let w = &mut e.world;
    let npc = w.write(&user(), "n", |x| x.create_node(NewNode::new(NodeKind::Character, "钟表匠老魏"))).unwrap().0.id;
    w.write(&user(), "n", |x| x.create_node(NewNode::new(NodeKind::City, "白杨镇"))).unwrap();
    w.write(&user(), "n", |x| x.create_node(NewNode::new(NodeKind::City, "无关城"))).unwrap();
    let s = w.write(&user(), "s", |x| x.create_script(NewScript::new("钟楼"))).unwrap().0;
    let mut it = NewScriptItem::new(&s.id, ScriptItemKind::Scene, "钟楼下");
    it.player_text = "白杨镇的钟楼下围满了人。".into();
    it.payload = json!({"npc": npc});
    let id = w.write(&user(), "i", |x| x.create_script_item(it)).unwrap().0.id;
    let titles: Vec<String> = scripts::related_nodes(w.conn(), &id).unwrap().into_iter().map(|n| n.title).collect();
    assert!(titles.contains(&"白杨镇".to_string()) && titles.contains(&"钟表匠老魏".to_string()), "{titles:?}");
    assert!(!titles.contains(&"无关城".to_string()));
}

#[test]
fn global_search_spans_world_scripts_campaigns_and_hides_gm_text() {
    let mut e = env();
    let w = &mut e.world;
    let mut n = NewNode::new(NodeKind::Character, "灯塔守夜人");
    n.gm_text = "其实他偷走了灯芯石".into();
    w.write(&user(), "n", |x| x.create_node(n)).unwrap();
    let s = w.write(&user(), "s", |x| x.create_script(NewScript::new("灯塔守夜"))).unwrap().0;
    let mut it = NewScriptItem::new(&s.id, ScriptItemKind::Scene, "灯室");
    it.player_text = "灯塔顶上的灯室一片漆黑".into();
    w.write(&user(), "i", |x| x.create_script_item(it)).unwrap();
    let c = campaign::create_campaign(w, Some(&s.id), "灯塔团", Some(1)).unwrap();
    campaign::record(w.conn(), &c.id, "narration", &json!({"text": "你爬上了灯塔"})).unwrap();
    let kinds = |hits: &[search::GlobalHit]| hits.iter().map(|h| h.kind.clone()).collect::<std::collections::BTreeSet<_>>();
    let hits = search::global_search(w.conn(), "灯塔", false, 10).unwrap();
    let k = kinds(&hits);
    for want in ["node", "script", "script_item", "campaign", "event"] {
        assert!(k.contains(want), "缺少 {want}：{k:?}");
    }
    assert!(search::global_search(w.conn(), "灯芯石", false, 10).unwrap().iter().all(|h| h.kind != "node"), "关闭剧透时不搜主持人文字");
    assert!(search::global_search(w.conn(), "灯芯石", true, 10).unwrap().iter().any(|h| h.kind == "node"));
    let ev = hits.iter().find(|h| h.kind == "event").unwrap();
    assert_eq!(ev.parent, c.id);
}

#[test]
fn spoken_moves_switch_location_only_when_clearly_going_there() {
    let mut e = env();
    let w = &mut e.world;
    let mk = |w: &mut World, n: NewNode| w.write(&user(), "n", |x| x.create_node(n)).unwrap().0.id;
    let bay = mk(w, NewNode::new(NodeKind::Region, "雾湾"));
    let town = mk(w, NewNode::new(NodeKind::City, "雾灯镇").under(&bay));
    let pier = mk(w, NewNode::new(NodeKind::Site, "码头").under(&town));
    let tower = mk(w, NewNode::new(NodeKind::Site, "老灯塔").under(&town));
    let c = campaign::create_campaign(w, None, "x", Some(1)).unwrap();
    worldsim::move_nearby(w, &c.id, &pier, "码头").unwrap();
    let got = worldsim::place_from_words(w.conn(), &c.id, "我举着火把走进老灯塔").unwrap();
    assert_eq!(got, Some((tower.clone(), "老灯塔".to_string(), true)));
    assert!(worldsim::place_from_words(w.conn(), &c.id, "我去问莫伦关于老灯塔的事").unwrap().is_none(), "只是提到，不算去");
    assert!(worldsim::place_from_words(w.conn(), &c.id, "老灯塔为什么熄了").unwrap().is_none());
    worldsim::move_nearby(w, &c.id, &tower, "老灯塔").unwrap();
    assert_eq!(worldsim::party_location(w.conn(), &c.id).unwrap(), Some(tower));
}

//! 剧情导演（程序部分）与 GM 控制台：节奏与停滞、重复、建议的采用与有效期、剧情冲突、控制台数据、手动修改与回溯。

mod common;

use chronicle_core::campaign;
use chronicle_core::console;
use chronicle_core::director::{self, DirectorNote, DirectorSettings, Suggestion};
use chronicle_core::timeline;
use chronicle_core::*;
use common::*;
use serde_json::{Value, json};

struct Fx {
    e: Env,
    camp: String,
    clue: String,
    quest: String,
    smith: String,
}

fn fx() -> Fx {
    let mut e = env();
    let u = user();
    let smith = mk(&mut e.world, NewNode::new(NodeKind::Character, "铁匠阿岩").payload(json!({"motivation": "还清赌债", "secrets": ["偷了钥匙"]})));
    let mut ns = NewScript::new("钥匙失窃");
    ns.core_setting = "铁匠偷了钥匙".into();
    let s = e.world.write(&u, "s", |w| w.create_script(ns)).unwrap().0;
    let item = |w: &mut World, k: ScriptItemKind, t: &str, p: Value| {
        let mut it = NewScriptItem::new(&s.id, k, t);
        it.payload = p;
        it.gm_text = format!("{t}的真相");
        w.write(&u, "i", |x| x.create_script_item(it)).unwrap().0.id
    };
    let clue = item(&mut e.world, ScriptItemKind::Clue, "铁砧下的划痕", json!({}));
    let quest = item(&mut e.world, ScriptItemKind::Quest, "找回钥匙", json!({"quest_type": "main", "npc_ids": [smith.id], "complete_when": {"type": "flag", "name": "key_found"}}));
    item(&mut e.world, ScriptItemKind::Trigger, "夜里起雾", json!({"when": {"type": "var", "key": "env.daypart", "op": "eq", "value": "夜晚"}, "text": "雾"}));
    e.world.write(&u, "c", |w| w.add_cast(NewCastMember { script_id: s.id.clone(), node_id: smith.id.clone(), role: "铁匠".into(), motive: "掩盖罪行".into(), secret: String::new(), relation_to_players: String::new(), payload: json!({}) })).unwrap();
    let mut start = NewPlotNode::new(&s.id, PlotNodeKind::Start, "酒馆");
    start.gm_text = "村长在撒谎".into();
    let start = e.world.write(&u, "p", |w| w.create_plot_node(start)).unwrap().0;
    let mut forge = NewPlotNode::new(&s.id, PlotNodeKind::Scene, "铁匠铺");
    forge.gm_text = "钥匙在铁砧下".into();
    let forge = e.world.write(&u, "p", |w| w.create_plot_node(forge)).unwrap().0;
    let mut edge = NewPlotEdge::new(&s.id, &start.id, &forge.id);
    edge.label = "去铁匠铺".into();
    e.world.write(&u, "e", |w| w.create_plot_edge(edge)).unwrap();
    let camp = campaign::create_campaign(&mut e.world, Some(&s.id), "c", Some(3)).unwrap().id;
    Fx { e, camp, clue, quest, smith: smith.id }
}

fn eff(w: &mut World, c: &str, v: Value) {
    let e: Vec<chronicle_rules::Effect> = serde_json::from_value(v).unwrap();
    campaign::apply_effects(w, c, &e, "测试").unwrap();
}

/// 模拟一个没有进展的回合。
fn idle_turn(w: &mut World, c: &str, input: &str) {
    campaign::record(w.conn(), c, "player", &json!({ "text": input })).unwrap();
    campaign::advance_turn(w.conn(), c).unwrap();
    director::record_beat(w, c).unwrap();
}

#[test]
fn stagnation_repetition_and_progress_are_measured_by_program() {
    let mut f = fx();
    let (w, c) = (&mut f.e.world, f.camp.clone());
    for (i, t) in ["我四处看看", "我和酒保聊天气", "我点了一杯酒"].iter().enumerate() {
        idle_turn(w, &c, t);
        assert!(!director::pacing(w.conn(), &c).unwrap().stalled, "第 {} 回合还不算停滞", i + 1);
    }
    idle_turn(w, &c, "我继续喝酒");
    let p = director::pacing(w.conn(), &c).unwrap();
    assert!(p.stalled && p.turns_since_progress >= 4, "{p:?}");
    assert_eq!(director::should_analyze(w.conn(), &c).unwrap().as_deref(), Some("stall"));
    assert!(p.hooks.iter().any(|h| h.kind == "clue" && h.title == "铁砧下的划痕"));
    assert!(p.hooks.iter().any(|h| h.kind == "plot" && h.title.contains("去铁匠铺")));
    assert!(p.hooks.iter().any(|h| h.kind == "npc" && h.title == "铁匠阿岩"));
    assert!(p.hooks.iter().any(|h| h.kind == "trigger"));
    // 有了进展（揭示线索）就不再停滞。
    eff(w, &c, json!([{"type": "reveal_clue", "clue": f.clue}]));
    idle_turn(w, &c, "我仔细看铁砧");
    let p = director::pacing(w.conn(), &c).unwrap();
    assert!(!p.stalled && p.turns_since_progress == 0, "{p:?}");
    assert!(p.recent_progress.iter().any(|x| x.contains("铁砧下的划痕")));
    assert!(!p.hooks.iter().any(|h| h.kind == "clue"), "揭示过的线索不再是钩子");
    // 重复同样的行动。
    for _ in 0..3 {
        idle_turn(w, &c, "我继续搜查酒馆的角落");
    }
    assert!(director::pacing(w.conn(), &c).unwrap().repeating);
    // 关闭自动分析。
    director::set_settings(w, &c, &DirectorSettings { auto_analyze: false, ..Default::default() }).unwrap();
    assert!(director::should_analyze(w.conn(), &c).unwrap().is_none());
    assert!(director::set_settings(w, &c, &DirectorSettings { stall_turns: 0, ..Default::default() }).is_err());
}

#[test]
fn tension_follows_danger_and_deadlines() {
    let mut f = fx();
    let (w, c) = (&mut f.e.world, f.camp.clone());
    let (calm, why) = director::tension_now(w.conn(), &c);
    assert!(calm < 30 && why[0].contains("平静"));
    chronicle_core::combat::start(w, &c, &[chronicle_core::combat::EnemySpec { name: "强盗".into(), npc: None, level: 1, count: 1, loot: vec![], boss: false }], &[], "x").unwrap();
    let (hot, why) = director::tension_now(w.conn(), &c);
    assert!(hot >= calm + 40 && why.iter().any(|x| x.contains("战斗中")), "{hot} {why:?}");
}

fn note(id: &str, turn: i64, texts: &[(&str, bool)]) -> DirectorNote {
    DirectorNote {
        conformity: None,
        deviations: vec![],
        id: id.into(),
        turn,
        trigger: "manual".into(),
        pacing: "raise".into(),
        assessment: "玩家在酒馆里打转".into(),
        tension: 20,
        suggestions: texts
            .iter()
            .enumerate()
            .map(|(i, (t, review))| Suggestion { id: format!("{id}-{}", i + 1), kind: "npc_action".into(), text: t.to_string(), why: "推动剧情".into(), uses: vec![], needs_review: *review, status: "new".into(), adopted_turn: None })
            .collect(),
    }
}

#[test]
fn suggestions_are_adopted_by_gm_and_expire() {
    let mut f = fx();
    let (w, c) = (&mut f.e.world, f.camp.clone());
    director::save_note(w, &c, &note("n00001", 0, &[("让铁匠匆忙离开酒馆", false), ("其实村长才是小偷", true)])).unwrap();
    let (pace, adopted) = director::guidance(w.conn(), &c).unwrap();
    assert!(pace.unwrap().contains("提高紧张度"));
    assert!(adopted.is_empty(), "没采用的建议不进入 GM 上下文");
    director::set_suggestion_status(w, &c, "n00001", "n00001-1", "adopted").unwrap();
    assert_eq!(director::guidance(w.conn(), &c).unwrap().1[0].text, "让铁匠匆忙离开酒馆");
    assert!(director::set_suggestion_status(w, &c, "n00001", "n00001-9", "adopted").is_err());
    assert!(director::set_suggestion_status(w, &c, "n00001", "n00001-2", "强制").is_err());
    for _ in 0..4 {
        idle_turn(w, &c, "x");
    }
    assert!(director::guidance(w.conn(), &c).unwrap().1.is_empty(), "采用的建议 4 回合后过期");
    assert_eq!(director::notes(w.conn(), &c, 5).unwrap().len(), 1);
}

#[test]
fn conflicts_are_reported() {
    let mut f = fx();
    let (w, c) = (&mut f.e.world, f.camp.clone());
    assert!(director::conflicts(w.conn(), &c).unwrap().is_empty());
    eff(w, &c, json!([{"type": "set_entity_state", "entity": f.smith, "field": "life_status", "value": "dead"}]));
    let mut s = chronicle_core::rpg::sheet(w.conn(), &c, "party").unwrap();
    s.hp = 0;
    chronicle_core::rpg::update_sheet(w, &c, &s, "wound", "倒下").unwrap();
    campaign::record(w.conn(), &c, "narration", &json!({ "text": "x", "leaks": ["铁匠偷了钥匙"] })).unwrap();
    campaign::record(w.conn(), &c, "conflict", &json!({ "items": ["结算里所有攻击都没有命中，叙述却写了命中"] })).unwrap();
    let cs = director::conflicts(w.conn(), &c).unwrap();
    let kinds: Vec<&str> = cs.iter().map(|x| x.kind.as_str()).collect();
    for k in ["dead_npc", "party_down", "leak", "narration"] {
        assert!(kinds.contains(&k), "{k}: {cs:?}");
    }
    assert!(cs.iter().any(|x| x.text.contains("找回钥匙") && x.text.contains("铁匠阿岩")));
}

#[test]
fn console_shows_hidden_state_and_manual_edits_are_event_sourced() {
    let mut f = fx();
    let (w, c) = (&mut f.e.world, f.camp.clone());
    campaign::record(w.conn(), &c, "player", &json!({ "text": "我问酒保" })).unwrap();
    campaign::record(w.conn(), &c, "gm_plan", &json!({ "intent": "打听", "based_on": ["M1 玩家答应帮村长"] })).unwrap();
    campaign::record(w.conn(), &c, "context_trace", &json!({ "budget": 6000, "tokens": 900, "sections": [], "memories": ["M1 玩家答应帮村长"], "search_hits": [], "guidance": [] })).unwrap();
    campaign::record(w.conn(), &c, "npc", &json!({ "npc_id": f.smith, "name": "铁匠阿岩", "text": "嗯", "memories": ["玩家上次赊账"] })).unwrap();
    let con = console::console(w.conn(), &c).unwrap();
    assert_eq!(con.scene.title, "酒馆");
    assert_eq!(con.scene.gm_text, "村长在撒谎");
    assert_eq!(con.scene.edges[0].label, "去铁匠铺");
    assert_eq!(con.hidden.core_setting, "铁匠偷了钥匙");
    assert!(con.hidden.upcoming.iter().any(|(t, g)| t == "铁匠铺" && g == "钥匙在铁砧下"));
    let smith = con.npcs.iter().find(|n| n.id == f.smith).unwrap();
    assert!(smith.goals.contains(&"掩盖罪行".to_string()) && smith.goals.contains(&"还清赌债".to_string()));
    assert_eq!(smith.secrets, vec!["偷了钥匙".to_string()]);
    assert!(con.clues.iter().any(|x| x.title == "铁砧下的划痕" && !x.revealed && x.gm_text.contains("真相")));
    assert!(con.conditions.iter().any(|x| x.role == "自动触发" && x.owner == "夜里起雾" && x.now == "不满足"));
    assert!(con.conditions.iter().any(|x| x.role == "完成" && x.owner.contains("找回钥匙")));
    let t = &con.traces[0];
    assert_eq!((t.input.as_str(), t.intent.as_str()), ("我问酒保", "打听"));
    assert_eq!(t.based_on, vec!["M1 玩家答应帮村长".to_string()]);
    assert_eq!(t.npc_memories[0].1, vec!["玩家上次赊账".to_string()]);
    // 手动修改：必须写原因；写入事件日志，随时间线回溯。
    timeline::checkpoint(w.conn(), &c, "turn_end").unwrap();
    let cp = timeline::checkpoints(w.conn(), &c).unwrap().last().unwrap().id;
    let e: Vec<chronicle_rules::Effect> = serde_json::from_value(json!([{"type": "reveal_clue", "clue": f.clue}, {"type": "set_quest_state", "quest": f.quest, "state": "active"}])).unwrap();
    assert!(console::apply_manual(w, &c, &e, " ").is_err());
    assert_eq!(console::apply_manual(w, &c, &e, "玩家上次其实已经看过铁砧").unwrap(), 2);
    let con = console::console(w.conn(), &c).unwrap();
    assert!(con.clues.iter().any(|x| x.title == "铁砧下的划痕" && x.revealed));
    assert_eq!(con.manual_edits[0].reason, "玩家上次其实已经看过铁砧");
    timeline::checkpoint(w.conn(), &c, "turn_end").unwrap();
    timeline::end_campaign(w, &c, "").unwrap();
    assert!(timeline::verify(w.conn(), &c).unwrap().ok);
    let b = timeline::fork(w, &c, cp, "", false).unwrap();
    assert!(!console::console(w.conn(), &b.id).unwrap().clues[0].revealed, "回溯到手动修改之前");
}

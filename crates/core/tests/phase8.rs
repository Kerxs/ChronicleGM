//! Phase 8：战役统计、结局分布、章节、编年史、跨战役搜索、战役 JSON 导入导出、战役完整性检查。

mod common;

use chronicle_core::archive::{self, SearchFilter};
use chronicle_core::campaign;
use chronicle_core::timeline;
use chronicle_core::*;
use common::*;
use serde_json::{Value, json};

struct Fx {
    e: Env,
    script: String,
    camp: String,
    clue: String,
    quest: String,
    smith: String,
    to_forge: String,
    to_good: String,
    to_bad: String,
}

fn fx() -> Fx {
    let mut e = env();
    let u = user();
    let smith = mk(&mut e.world, NewNode::new(NodeKind::Character, "铁匠阿岩"));
    let s = e.world.write(&u, "s", |w| w.create_script(NewScript::new("钥匙失窃"))).unwrap().0;
    let item = |w: &mut World, k: ScriptItemKind, t: &str, p: Value| {
        let mut it = NewScriptItem::new(&s.id, k, t);
        it.payload = p;
        w.write(&u, "i", |x| x.create_script_item(it)).unwrap().0.id
    };
    let clue = item(&mut e.world, ScriptItemKind::Clue, "铁砧下的划痕", json!({}));
    item(&mut e.world, ScriptItemKind::Clue, "酒馆的账本", json!({}));
    let quest = item(&mut e.world, ScriptItemKind::Quest, "找回钥匙", json!({"quest_type": "main"}));
    e.world
        .write(&u, "c", |w| {
            w.add_cast(NewCastMember { script_id: s.id.clone(), node_id: smith.id.clone(), role: "铁匠".into(), motive: String::new(), secret: String::new(), relation_to_players: String::new(), payload: json!({}) })
        })
        .unwrap();
    let node = |w: &mut World, k: PlotNodeKind, t: &str| w.write(&u, "p", |x| x.create_plot_node(NewPlotNode::new(&s.id, k, t))).unwrap().0.id;
    let start = node(&mut e.world, PlotNodeKind::Start, "酒馆");
    let forge = node(&mut e.world, PlotNodeKind::Scene, "铁匠铺");
    let good = node(&mut e.world, PlotNodeKind::Ending, "钥匙物归原主");
    let bad = node(&mut e.world, PlotNodeKind::Ending, "铁匠逃走");
    let edge = |w: &mut World, a: &str, b: &str, l: &str| {
        let mut ed = NewPlotEdge::new(&s.id, a, b);
        ed.label = l.into();
        w.write(&u, "e", |x| x.create_plot_edge(ed)).unwrap().0.id
    };
    let to_forge = edge(&mut e.world, &start, &forge, "去铁匠铺");
    let to_good = edge(&mut e.world, &forge, &good, "说服铁匠");
    let to_bad = edge(&mut e.world, &forge, &bad, "动手抓人");
    let camp = campaign::create_campaign(&mut e.world, Some(&s.id), "第一次冒险", Some(7)).unwrap().id;
    Fx { e, script: s.id, camp, clue, quest, smith: smith.id, to_forge, to_good, to_bad }
}

fn eff(w: &mut World, c: &str, v: Value) {
    let e: Vec<chronicle_rules::Effect> = serde_json::from_value(v).unwrap();
    campaign::apply_effects(w, c, &e, "测试").unwrap();
}

fn turn(w: &mut World, c: &str, input: &str, narration: &str) {
    campaign::record(w.conn(), c, "player", &json!({ "text": input })).unwrap();
    campaign::record(w.conn(), c, "narration", &json!({ "text": narration })).unwrap();
    campaign::advance_turn(w.conn(), c).unwrap();
    timeline::checkpoint(w.conn(), c, "turn_end").unwrap();
}

/// 打完一整局：去铁匠铺、发现线索、任务开始并完成、关系变化、一场胜利的战斗、铁匠死亡、结局。
fn play(f: &mut Fx) {
    let (w, c) = (&mut f.e.world, f.camp.clone());
    timeline::ensure_baseline(w.conn(), &c).unwrap();
    turn(w, &c, "我去铁匠铺看看", "你推开铁匠铺的门，炉火正旺。");
    campaign::take_edge(w, &c, &f.to_forge).unwrap();
    eff(w, &c, json!([{"type": "reveal_clue", "clue": f.clue}, {"type": "set_quest_state", "quest": f.quest, "state": "active"}]));
    turn(w, &c, "我蹲下检查铁砧", "铁砧下有新鲜的划痕，像是有人撬过。");
    eff(w, &c, json!([{"type": "adjust_bond", "from": f.smith, "to": "party", "kind": "敌对", "hostility": 40, "trust": -20, "reason": "被怀疑"}]));
    eff(w, &c, json!([{"type": "upsert_record", "kind": "combat", "id": "cb1", "data": {"id": "cb1", "reason": "铁匠拔刀", "status": "victory", "round": 3,
        "units": [{"side": "enemy"}, {"side": "party"}], "rewards": ["经验 20"]}}]));
    eff(w, &c, json!([{"type": "set_entity_state", "entity": f.smith, "field": "life_status", "value": "dead"}, {"type": "set_quest_state", "quest": f.quest, "state": "done"}]));
    turn(w, &c, "我和铁匠打了起来", "铁匠倒在炉边，再也没有起来。");
    campaign::take_edge(w, &c, &f.to_bad).unwrap();
    timeline::end_campaign(w, &c, "").unwrap();
}

#[test]
fn stats_cover_choices_npc_fates_bonds_quests_combat_and_plot() {
    let mut f = fx();
    play(&mut f);
    let st = archive::campaign_stats(f.e.world.conn(), &f.camp).unwrap();
    assert_eq!(st.status, "ended");
    assert_eq!(st.ending, "铁匠逃走");
    // 剧情跳转也推进回合。
    assert_eq!(st.turns, campaign::get_campaign(f.e.world.conn(), &f.camp).unwrap().turn);
    assert_eq!(st.player_actions, 3);
    // 玩家选择：两次剧情跳转，第二次在有两条出路的分支点。
    assert_eq!(st.choices.len(), 2, "{:?}", st.choices);
    assert_eq!((st.choices[0].label.as_str(), st.choices[0].to.as_str(), st.choices[0].options), ("去铁匠铺", "铁匠铺", 1));
    assert_eq!((st.choices[1].from.as_str(), st.choices[1].label.as_str(), st.choices[1].options), ("铁匠铺", "动手抓人", 2));
    assert_eq!(st.branch_points, 1);
    assert_eq!((st.plot_visited, st.plot_total, st.endings_total), (3, 4, 2));
    // NPC 生死。
    assert_eq!((st.npcs_alive, st.npcs_dead), (0, 1));
    assert_eq!(st.npcs[0].name, "铁匠阿岩");
    let died = st.npcs[0].died_turn.unwrap();
    assert!(died > 0 && died < st.turns, "{died}");
    // 关系变化。
    assert_eq!(st.bond_changes, 1);
    assert_eq!(st.bonds[0].from, "铁匠阿岩");
    assert_eq!(st.bonds[0].to, "队伍");
    assert_eq!(st.bonds[0].last_kind, "敌对");
    assert_eq!(st.bonds[0].delta.1, 0, "信任下限是 0");
    assert_eq!(st.bonds[0].delta.3, 40);
    // 任务、线索、战斗。
    assert_eq!(st.quests.get("done"), Some(&1));
    assert_eq!((st.clues_revealed, st.clues_total), (1, 2));
    assert_eq!(st.combats.len(), 1);
    assert_eq!(st.combat_results.get("victory"), Some(&1));
    assert_eq!((st.combats[0].rounds, st.combats[0].enemies), (3, 1));
    assert!(st.event_kinds.get("player") == Some(&3));
}

#[test]
fn ending_distribution_counts_all_timelines_of_a_script() {
    let mut f = fx();
    play(&mut f);
    // 从开局回溯，走另一个结局。
    let cp0 = timeline::checkpoints(f.e.world.conn(), &f.camp).unwrap()[0].id;
    let b = timeline::fork(&mut f.e.world, &f.camp, cp0, "", false).unwrap().id;
    campaign::take_edge(&mut f.e.world, &b, &f.to_forge).unwrap();
    campaign::take_edge(&mut f.e.world, &b, &f.to_good).unwrap();
    timeline::end_campaign(&mut f.e.world, &b, "").unwrap();
    // 再开一条还没结束的。
    let cp0 = timeline::checkpoints(f.e.world.conn(), &f.camp).unwrap()[0].id;
    timeline::fork(&mut f.e.world, &f.camp, cp0, "第三条", false).unwrap();
    let d = archive::ending_distribution(f.e.world.conn(), &f.script).unwrap();
    assert_eq!((d.total, d.ended, d.active), (3, 2, 1));
    assert_eq!(d.endings.len(), 2);
    assert!(d.endings.iter().any(|e| e.ending == "铁匠逃走" && e.count == 1));
    assert!(d.endings.iter().any(|e| e.ending == "钥匙物归原主" && e.count == 1));
    assert!(d.unreached.is_empty());
    // 档案列表：同一族排在一起，子时间线深度为 1。
    let list = archive::archive_list(f.e.world.conn()).unwrap();
    assert_eq!(list.len(), 3);
    assert_eq!(list[0].campaign.id, f.camp);
    assert_eq!(list[0].children, 2);
    assert!(list[1..].iter().all(|e| e.depth == 1 && e.root_id == f.camp));
    // 原时间线没有被回溯改动。
    let st = archive::campaign_stats(f.e.world.conn(), &f.camp).unwrap();
    assert_eq!((st.ending.as_str(), st.children), ("铁匠逃走", 2));
}

#[test]
fn chapters_auto_from_scenes_then_manual_markers_with_rename_and_remove() {
    let mut f = fx();
    play(&mut f);
    let conn = f.e.world.conn();
    let chs = archive::chapters(conn, &f.camp).unwrap();
    // 没有手动标记和摘要：按进入剧情节点分章（开场 + 两次跳转）。
    assert_eq!(chs.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["开场", "铁匠铺", "铁匠逃走"]);
    assert!(chs.iter().all(|c| c.source == "scene"));
    assert_eq!(chs.last().unwrap().seq_to, campaign::events(conn, &f.camp, 0, 10_000).unwrap().last().unwrap().seq);
    // 手动标记优先。
    let f2 = fx();
    let (w, c) = (&f2.e.world, f2.camp.clone());
    let a = archive::mark_chapter(w.conn(), &c, "序章").unwrap();
    campaign::record(w.conn(), &c, "player", &json!({"text": "出发"})).unwrap();
    let b = archive::mark_chapter(w.conn(), &c, "第二章").unwrap();
    campaign::record(w.conn(), &c, "player", &json!({"text": "抵达"})).unwrap();
    let chs = archive::chapters(w.conn(), &c).unwrap();
    assert_eq!(chs.iter().map(|c| (c.title.as_str(), c.source.as_str())).collect::<Vec<_>>(), [("序章", "manual"), ("第二章", "manual")]);
    assert_eq!(chs[0].seq_to, b - 1);
    archive::rename_chapter(w.conn(), &c, b, "抵达王都").unwrap();
    archive::remove_chapter(w.conn(), &c, a).unwrap();
    let chs = archive::chapters(w.conn(), &c).unwrap();
    assert_eq!(chs.len(), 1);
    assert_eq!(chs[0].title, "抵达王都");
    assert!(archive::mark_chapter(w.conn(), &c, "  ").is_err());
    // 章节标记不改变战役状态：重放校验仍然通过。
    assert!(timeline::verify(w.conn(), &c).unwrap().ok);
}

#[test]
fn chronicle_is_deterministic_and_lists_key_events() {
    let mut f = fx();
    play(&mut f);
    let a = archive::chronicle(f.e.world.conn(), &f.camp).unwrap();
    let b = archive::chronicle(f.e.world.conn(), &f.camp).unwrap();
    assert_eq!(a, b, "同样的存档必须得到同样的编年史");
    let texts: Vec<String> = a.chapters.iter().flat_map(|c| c.entries.iter().map(|e| format!("{}|{}", e.kind, e.text))).collect();
    let has = |k: &str, t: &str| texts.iter().any(|x| x.starts_with(&format!("{k}|")) && x.contains(t));
    assert!(has("action", "我去铁匠铺看看"), "{texts:?}");
    assert!(has("scene", "来到「铁匠铺」"));
    assert!(has("progress", "揭示线索「铁砧下的划痕」"));
    assert!(has("progress", "任务「找回钥匙」→ done"));
    assert!(has("death", "铁匠阿岩 死亡"));
    assert!(has("bond", "铁匠阿岩 与 队伍 的关系：无 → 敌对"));
    assert!(has("ending", "结局：铁匠逃走"));
    assert!(a.markdown.starts_with("# 第一次冒险 编年史"));
    assert!(a.markdown.contains("## 第 2 章 铁匠铺"));
    assert!(a.markdown.contains("- 铁匠阿岩 死亡"));
}

#[test]
fn search_spans_campaigns_with_filters() {
    let mut f = fx();
    play(&mut f);
    let other = campaign::create_campaign(&mut f.e.world, Some(&f.script), "第二次冒险", Some(1)).unwrap().id;
    campaign::record(f.e.world.conn(), &other, "player", &json!({"text": "我又回到铁匠铺"})).unwrap();
    let conn = f.e.world.conn();
    let hits = archive::search_events(conn, &SearchFilter { query: "铁匠铺".into(), ..Default::default() }).unwrap();
    let camps: std::collections::BTreeSet<&str> = hits.iter().map(|h| h.campaign_name.as_str()).collect();
    assert!(camps.contains("第一次冒险") && camps.contains("第二次冒险"), "{hits:?}");
    assert!(hits.iter().any(|h| h.kind == "narration" && h.snippet.contains("铁匠铺的门")));
    let only = archive::search_events(conn, &SearchFilter { query: "铁匠铺".into(), campaign_ids: vec![other.clone()], kinds: vec!["player".into()], limit: 10 }).unwrap();
    assert_eq!(only.len(), 1);
    assert_eq!(only[0].snippet, "我又回到铁匠铺");
    // 通配符按字面匹配。
    assert!(archive::search_events(conn, &SearchFilter { query: "%".into(), ..Default::default() }).unwrap().is_empty());
    assert!(archive::search_events(conn, &SearchFilter { query: " ".into(), ..Default::default() }).unwrap().is_empty());
}

#[test]
fn campaign_json_round_trip_keeps_everything_and_never_overwrites() {
    let mut f = fx();
    play(&mut f);
    let cp0 = timeline::checkpoints(f.e.world.conn(), &f.camp).unwrap()[1].id;
    let child = timeline::fork(&mut f.e.world, &f.camp, cp0, "支线", false).unwrap().id;
    campaign::record(f.e.world.conn(), &child, "player", &json!({"text": "换个做法"})).unwrap();
    archive::mark_chapter(f.e.world.conn(), &child, "另一条路").unwrap();
    let before = archive::campaign_stats(f.e.world.conn(), &f.camp).unwrap();
    let before_events = before.events;

    let ex = archive::export_campaign(f.e.world.conn(), &f.camp, true).unwrap();
    assert_eq!(ex.campaigns.len(), 2);
    assert!(ex.campaigns.iter().all(|d| d.verified));
    // 经过 JSON 文本往返。
    let text = serde_json::to_string_pretty(&ex).unwrap();
    let back: archive::CampaignExport = serde_json::from_str(&text).unwrap();
    let r = archive::import_campaign(&mut f.e.world, &back).unwrap();
    assert_eq!(r.campaign_ids.len(), 2);
    assert!(r.names[0].ends_with("（导入）"), "{:?}", r.names);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let conn = f.e.world.conn();
    // 原战役没有被改动。
    assert_eq!(archive::campaign_stats(conn, &f.camp).unwrap().events, before_events);
    // 导入的战役统计与原来一致（名称、ID 除外）。
    let mut got = archive::campaign_stats(conn, &r.campaign_ids[0]).unwrap();
    let mut want = before.clone();
    for s in [&mut got, &mut want] {
        s.campaign_id.clear();
        s.name.clear();
        s.children = 0;
        for ch in &mut s.choices {
            ch.seq = 0;
        }
    }
    assert_eq!(got, want);
    // 父子关系换成新 ID，分叉位置换成新序号；两条都能通过重放校验。
    let nc = campaign::get_campaign(conn, &r.campaign_ids[1]).unwrap();
    assert_eq!(nc.parent_id.as_deref(), Some(r.campaign_ids[0].as_str()));
    let fs = nc.fork_seq.unwrap();
    let parent_events = campaign::events(conn, &r.campaign_ids[0], 0, 10_000).unwrap();
    assert!(parent_events.iter().any(|e| e.seq == fs), "分叉序号应指向新父时间线中的事件");
    for id in &r.campaign_ids {
        assert!(timeline::verify(conn, id).unwrap().ok);
    }
    assert_eq!(archive::chapters(conn, &r.campaign_ids[1]).unwrap().iter().filter(|c| c.source == "manual").count(), 1);
    // 导入的战役可以再回溯。
    let cps = timeline::checkpoints(conn, &r.campaign_ids[0]).unwrap();
    assert!(!cps.is_empty());
    let g = timeline::fork(&mut f.e.world, &r.campaign_ids[0], cps[0].id, "", false).unwrap();
    assert!(timeline::verify(f.e.world.conn(), &g.id).unwrap().ok);
    // 全部战役通过完整性检查。
    let checks = archive::check_campaigns(f.e.world.conn()).unwrap();
    assert!(checks.iter().all(|c| c.ok), "{checks:?}");
}

#[test]
fn import_rejects_tampered_foreign_or_unknown_files_without_writing() {
    let mut f = fx();
    play(&mut f);
    let ex = archive::export_campaign(f.e.world.conn(), &f.camp, false).unwrap();
    let count = |w: &World| campaign::list_campaigns(w.conn()).unwrap().len();
    let n0 = count(&f.e.world);
    // 被改动。
    let mut bad = ex.clone();
    bad.campaigns[0].tables.get_mut("event_log").unwrap()[0].insert("kind".into(), json!("hacked"));
    assert!(archive::import_campaign(&mut f.e.world, &bad).unwrap_err().to_string().contains("校验值"));
    // 格式不对 / 版本太新。
    let mut bad = ex.clone();
    bad.format = "x".into();
    assert!(archive::import_campaign(&mut f.e.world, &bad).is_err());
    let mut bad = ex.clone();
    bad.format_version = 99;
    assert!(archive::import_campaign(&mut f.e.world, &bad).unwrap_err().to_string().contains("版本"));
    assert_eq!(count(&f.e.world), n0, "失败的导入不能留下任何数据");
    // 另一个没有这个剧本的世界。
    let mut other = env();
    let err = archive::import_campaign(&mut other.world, &ex).unwrap_err().to_string();
    assert!(err.contains("钥匙失窃"), "{err}");
    assert_eq!(count(&other.world), 0);
    // 只导出子时间线时，导入后成为独立战役并给出提示。
    let cp = timeline::checkpoints(f.e.world.conn(), &f.camp).unwrap()[0].id;
    let child = timeline::fork(&mut f.e.world, &f.camp, cp, "孤儿", false).unwrap().id;
    let ex = archive::export_campaign(f.e.world.conn(), &child, false).unwrap();
    let r = archive::import_campaign(&mut f.e.world, &ex).unwrap();
    assert_eq!(r.warnings.len(), 1);
    let c = campaign::get_campaign(f.e.world.conn(), &r.campaign_ids[0]).unwrap();
    assert!(c.parent_id.is_none() && c.fork_seq.is_none());
    assert!(timeline::verify(f.e.world.conn(), &c.id).unwrap().ok);
}

#[test]
fn integrity_check_finds_tampered_checkpoints_and_broken_references() {
    let mut f = fx();
    play(&mut f);
    let conn = f.e.world.conn();
    assert!(archive::check_campaigns(conn).unwrap().iter().all(|c| c.ok));
    // 检查点内容被改（绕过程序直接改数据库）。
    conn.execute("UPDATE campaign_checkpoints SET state = state || ' ' WHERE id = (SELECT MAX(id) FROM campaign_checkpoints) AND campaign_id = ?1", [&f.camp]).unwrap();
    let ck = archive::check_campaigns(conn).unwrap();
    assert!(!ck[0].ok);
    assert!(ck[0].problems.iter().any(|p| p.contains("校验值")), "{:?}", ck[0].problems);
}

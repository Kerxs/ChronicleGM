//! 剧情回溯与多时间线：完整状态恢复、原时间线不被修改、时间线隔离、连续回溯、
//! 可复现的骰子、重放校验与篡改检测、升级前战役的处理。

mod common;

use chronicle_core::campaign::{self, Campaign};
use chronicle_core::timeline::{self, CheckpointState};
use chronicle_core::*;
use common::*;
use serde_json::{Value, json};

struct Fx {
    e: Env,
    script: String,
    npc: String,
    quest: String,
    clue: String,
    item: String,
    /// 起点 → 选择点；选择点 → 结局 A / 结局 B
    to_choice: String,
    to_a: String,
    to_b: String,
}

fn fx() -> Fx {
    let mut e = env();
    let npc = mk(&mut e.world, NewNode::new(NodeKind::Character, "村长老格"));
    let u = user();
    let s = e.world.write(&u, "s", |w| w.create_script(NewScript::new("遗迹委托"))).unwrap().0;
    let item = |w: &mut World, k: ScriptItemKind, t: &str, p: Value| {
        let mut it = NewScriptItem::new(&s.id, k, t);
        it.payload = p;
        w.write(&u, "i", |x| x.create_script_item(it)).unwrap().0.id
    };
    let quest = item(&mut e.world, ScriptItemKind::Quest, "调查遗迹", json!({"quest_type": "main"}));
    let clue = item(&mut e.world, ScriptItemKind::Clue, "石板符文", json!({}));
    let key = item(&mut e.world, ScriptItemKind::Item, "古铜钥匙", json!({}));
    let pn = |w: &mut World, k, t: &str| w.write(&u, "p", |x| x.create_plot_node(NewPlotNode::new(&s.id, k, t))).unwrap().0.id;
    let start = pn(&mut e.world, PlotNodeKind::Start, "酒馆");
    let choice = pn(&mut e.world, PlotNodeKind::Choice, "遗迹入口");
    let end_a = pn(&mut e.world, PlotNodeKind::Ending, "封印加固");
    let end_b = pn(&mut e.world, PlotNodeKind::Ending, "古神苏醒");
    let edge = |w: &mut World, a: &str, b: &str, label: &str, cond: Value, eff: Value| {
        let mut ed = NewPlotEdge::new(&s.id, a, b);
        ed.label = label.into();
        ed.condition = serde_json::from_value(cond).unwrap();
        ed.effects = serde_json::from_value(eff).unwrap();
        w.write(&u, "e", |x| x.create_plot_edge(ed)).unwrap().0.id
    };
    let to_choice = edge(&mut e.world, &start, &choice, "接受委托", Value::Null, json!([{"type": "set_quest_state", "quest": quest, "state": "active"}]));
    let to_a = edge(&mut e.world, &choice, &end_a, "加固封印", json!({"type": "check", "skill": "奥秘", "dc": 2}), json!([{"type": "set_quest_state", "quest": quest, "state": "done"}]));
    let to_b = edge(&mut e.world, &choice, &end_b, "打碎封印", Value::Null, json!([{"type": "set_quest_state", "quest": quest, "state": "failed"}]));
    Fx { e, script: s.id, npc: npc.id, quest, clue, item: key, to_choice, to_a, to_b }
}

fn eff(v: Value) -> Vec<chronicle_rules::Effect> {
    serde_json::from_value(v).unwrap()
}

/// 模拟一个完成的 GM 回合：玩家行动 → 叙述 → 效果 → 回合结束（与 chronicle-gm 的写入顺序一致）。
fn turn(w: &mut World, c: &str, input: &str, effects: Value, extra: impl FnOnce(&mut World)) {
    campaign::record(w.conn(), c, "player", &json!({"text": input})).unwrap();
    campaign::record(w.conn(), c, "narration", &json!({"text": format!("（回应：{input}）")})).unwrap();
    if effects.as_array().is_some_and(|a| !a.is_empty()) {
        campaign::apply_effects(w, c, &eff(effects), "GM 回合").unwrap();
    }
    extra(w);
    campaign::advance_turn(w.conn(), c).unwrap();
    let t = campaign::get_campaign(w.conn(), c).unwrap().turn;
    campaign::record(w.conn(), c, "turn_end", &json!({"turn": t})).unwrap();
    timeline::checkpoint(w.conn(), c, "turn_end").unwrap();
}

/// 原时间线的"指纹"：用来证明回溯没有修改它。
fn fingerprint(w: &World, c: &str) -> String {
    let conn = w.conn();
    let count = |sql: &str| -> i64 { conn.query_row(sql, [c], |r| r.get(0)).unwrap() };
    let st = timeline::capture(conn, c).unwrap();
    let camp = campaign::get_campaign(conn, c).unwrap();
    let evs: Vec<String> = campaign::events(conn, c, 0, 100000).unwrap().into_iter().map(|e| format!("{}{}{}", e.seq, e.kind, e.payload)).collect();
    format!(
        "{}|{}|{}|{}|{}|{:?}|{}",
        serde_json::to_string(&st).unwrap(),
        serde_json::to_string(&camp).unwrap(),
        evs.join(","),
        count("SELECT COUNT(*) FROM npc_memories WHERE campaign_id = ?1"),
        count("SELECT COUNT(*) FROM campaign_checkpoints WHERE campaign_id = ?1"),
        campaign::facts(conn, c, 100).unwrap(),
        count("SELECT COUNT(*) FROM campaign_entities WHERE campaign_id = ?1"),
    )
}

/// 玩四个回合并在第 2 回合后记录"当时的样子"，然后结束战役。
fn play_original(f: &mut Fx) -> (Campaign, CheckpointState, Vec<String>, i64, i64) {
    let w = &mut f.e.world;
    let c = campaign::create_campaign(w, Some(&f.script), "原时间线", Some(42)).unwrap();
    let (npc, quest, clue, item) = (f.npc.clone(), f.quest.clone(), f.clue.clone(), f.item.clone());
    // 回合 1：接受委托（手动推进剧情，随后保存检查点），NPC 记住玩家，记下事实。
    campaign::record(w.conn(), &c.id, "player", &json!({"text": "我接下委托"})).unwrap();
    campaign::take_edge(w, &c.id, &f.to_choice).unwrap();
    campaign::add_npc_memory(w, &c.id, &npc, "冒险者答应了委托", 6).unwrap();
    campaign::add_fact(w.conn(), &c.id, "玩家接受了委托").unwrap();
    campaign::record(w.conn(), &c.id, "turn_end", &json!({})).unwrap();
    timeline::checkpoint(w.conn(), &c.id, "turn_end").unwrap();
    // 回合 2：得到钥匙、知道线索、村长受伤、关系改变、态度变化、临时人物、摘要。
    turn(
        w,
        &c.id,
        "我检查石板",
        json!([
            {"type": "give_item", "holder": "party", "item": item, "qty": 1},
            {"type": "reveal_clue", "clue": clue},
            {"type": "set_entity_state", "entity": npc, "field": "life_status", "value": "wounded"},
            {"type": "set_relation", "a": "party", "b": npc, "rel_type": "trusts", "active": true},
            {"type": "inc_var", "key": format!("attitude.{npc}"), "by": 2},
            {"type": "set_flag", "name": "saw_slab"}
        ]),
        |w| {
            let c2 = campaign::list_campaigns(w.conn()).unwrap()[0].id.clone();
            campaign::add_campaign_entity(w.conn(), &c2, "character", "神秘旅人", "在遗迹门口徘徊", "其实是邪教徒", None).unwrap();
            let s = campaign::add_summary_draft(w.conn(), &c2, "玩家接受委托并找到了钥匙。", 0).unwrap();
            campaign::confirm_summary(w.conn(), &s.id, None).unwrap();
        },
    );
    let at2 = timeline::capture(w.conn(), &c.id).unwrap();
    let facts2 = campaign::facts(w.conn(), &c.id, 100).unwrap();
    let cp2 = timeline::checkpoints(w.conn(), &c.id).unwrap().last().unwrap().id;
    // 回合 3：掷一次骰子（记下结果，用来验证回溯后的骰子可复现），NPC 又记住一件事。
    let mut roll3 = 0;
    turn(w, &c.id, "我在门口等到天黑", json!([{"type": "inc_var", "key": "waited", "by": 1}]), |w| {
        roll3 = campaign::roll(w, &campaign::list_campaigns(w.conn()).unwrap()[0].id, "1d20", "察觉").unwrap().total;
        campaign::add_npc_memory(w, &campaign::list_campaigns(w.conn()).unwrap()[0].id, &npc, "冒险者在门口等到天黑", 3).unwrap();
    });
    campaign::add_fact(w.conn(), &c.id, "这条事实只属于原时间线").unwrap();
    // 回合 4：走向结局 A（带检定的分支）。
    campaign::take_edge(w, &c.id, &f.to_a).unwrap();
    campaign::record(w.conn(), &c.id, "turn_end", &json!({})).unwrap();
    timeline::checkpoint(w.conn(), &c.id, "transition").unwrap();
    let _ = quest;
    (c, at2, facts2, cp2, roll3)
}

#[test]
fn fork_restores_complete_state_and_never_touches_original() {
    let mut f = fx();
    let (c, at2, facts2, cp2, roll3) = play_original(&mut f);
    let w = &mut f.e.world;
    // 战役没结束时不能回溯。
    assert!(timeline::fork(w, &c.id, cp2, "", false).is_err());
    let end = timeline::end_campaign(w, &c.id, "").unwrap();
    assert_eq!(end.label, "ending");
    let ended = campaign::get_campaign(w.conn(), &c.id).unwrap();
    assert_eq!((ended.status.as_str(), ended.ending.as_str()), ("ended", "封印加固"), "结局默认取结局节点名");
    // 结束后不能继续推进。
    assert!(campaign::roll(w, &c.id, "1d20", "x").is_err());
    assert!(campaign::apply_effects(w, &c.id, &eff(json!([{"type": "set_flag", "name": "late"}])), "x").is_err());
    let before = fingerprint(w, &c.id);

    let b = timeline::fork(w, &c.id, cp2, "回到检查石板之后", false).unwrap();
    let conn = w.conn();
    assert_eq!(b.parent_id.as_deref(), Some(c.id.as_str()));
    assert_eq!(b.status, "active");
    assert_eq!(b.turn, at2.turn, "世界时间（回合）恢复");
    // 全部游戏状态与当时一致。
    let now = timeline::capture(conn, &b.id).unwrap();
    assert_eq!(now.core, at2.core, "任务、线索、物品、关系、实体状态、变量、剧情位置、骰子计数");
    assert_eq!(now.core.quests.get(&f.quest).map(String::as_str), Some("active"));
    assert!(now.core.clues.contains(&f.clue));
    assert_eq!(now.core.items.values().copied().collect::<Vec<_>>(), vec![1]);
    assert!(!now.core.vars.contains_key("waited"), "第 3 回合的变化不应出现");
    assert_eq!(campaign::entity_overrides(conn, &b.id, &f.npc).unwrap().get("life_status"), Some(&json!("wounded")));
    // NPC 记忆、事实、临时人物、会话摘要（AI 上下文的来源）恢复到当时。
    let mems = campaign::npc_memories(conn, &b.id, &f.npc, 10).unwrap();
    assert_eq!(mems.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(), vec!["冒险者答应了委托"]);
    assert_eq!(campaign::facts(conn, &b.id, 100).unwrap(), facts2);
    assert_eq!(campaign::campaign_entities(conn, &b.id).unwrap().len(), 1);
    let sums = campaign::summaries(conn, &b.id).unwrap();
    assert_eq!(sums.len(), 1);
    assert!(sums[0].upto_seq <= campaign::events(conn, &b.id, 0, 1000).unwrap().last().unwrap().seq);
    // 历史事件：只有分叉点之前的（外加一条分叉记录），之后的叙述不会出现在新时间线的上下文里。
    let evs = campaign::events(conn, &b.id, 0, 1000).unwrap();
    assert_eq!(evs.last().unwrap().kind, "timeline_fork");
    assert!(!evs.iter().any(|e| e.payload.to_string().contains("等到天黑")));
    // 骰子：新时间线的下一次掷骰与原时间线当时（第 3 回合）的结果完全相同。
    assert_eq!(campaign::roll(w, &b.id, "1d20", "察觉").unwrap().total, roll3);
    // 原时间线一点都没变。
    assert_eq!(fingerprint(w, &c.id), before, "原时间线被修改了");
    // 新旧时间线都通过重放校验。
    for id in [&c.id, &b.id] {
        let rep = timeline::verify(w.conn(), id).unwrap();
        assert!(rep.ok, "{rep:#?}");
        assert!(rep.checkpoints >= 3);
    }
}

#[test]
fn branches_are_isolated_and_endings_can_be_compared() {
    let mut f = fx();
    let (c, _, _, _, _) = play_original(&mut f);
    let w = &mut f.e.world;
    timeline::end_campaign(w, &c.id, "").unwrap();
    let points = timeline::fork_points(w.conn(), &c.id).unwrap();
    assert!(points.iter().any(|p| p.checkpoint.label == "start"));
    assert!(points.iter().any(|p| p.last_action == "我检查石板"), "历史节点带有玩家行动预览");
    // 从"接受委托之后"开出两条分支，各走一个结局。
    let after_accept = points.iter().find(|p| p.checkpoint.turn == 1).unwrap().checkpoint.id;
    let b1 = timeline::fork(w, &c.id, after_accept, "分支一", false).unwrap();
    let b2 = timeline::fork(w, &c.id, after_accept, "分支二", false).unwrap();
    campaign::take_edge(w, &b2.id, &f.to_b).unwrap();
    campaign::add_npc_memory(w, &b2.id, &f.npc, "只在分支二发生：冒险者打碎了封印", 9).unwrap();
    timeline::checkpoint(w.conn(), &b2.id, "transition").unwrap();
    timeline::end_campaign(w, &b2.id, "").unwrap();
    campaign::take_edge(w, &b1.id, &f.to_a).unwrap();
    timeline::checkpoint(w.conn(), &b1.id, "transition").unwrap();
    timeline::end_campaign(w, &b1.id, "封印加固（第二次尝试）").unwrap();
    // 隔离：分支二的记忆不会出现在其他时间线。
    for id in [&c.id, &b1.id] {
        assert!(!campaign::npc_memories(w.conn(), id, &f.npc, 50).unwrap().iter().any(|m| m.content.contains("分支二")));
    }
    // 比较结局。
    let cmp = timeline::compare(w.conn(), &[c.id.clone(), b1.id.clone(), b2.id.clone()]).unwrap();
    let ending = cmp.rows.iter().find(|r| r.category == "结局").unwrap();
    assert_eq!(ending.values, vec![Some("封印加固".into()), Some("封印加固（第二次尝试）".into()), Some("古神苏醒".into())]);
    let quest = cmp.rows.iter().find(|r| r.category == "任务" && r.key == "调查遗迹").unwrap();
    assert_eq!(quest.values, vec![Some("done".into()), Some("done".into()), Some("failed".into())]);
    assert!(quest.differs);
    // 连续回溯：从分支二再回到它的起点，形成三层时间线；每条都通过校验。
    let b2_start = timeline::fork_points(w.conn(), &b2.id).unwrap().into_iter().find(|p| p.checkpoint.label == "start").unwrap().checkpoint.id;
    let b3 = timeline::fork(w, &b2.id, b2_start, "", true).unwrap();
    assert_eq!(b3.parent_id.as_deref(), Some(b2.id.as_str()));
    assert_ne!(b3.rng_seed, b2.rng_seed, "选择换新种子");
    assert_eq!(b3.turn, 0);
    assert!(campaign::events(w.conn(), &b3.id, 0, 100).unwrap().iter().all(|e| e.kind == "timeline_fork"), "回到起点：没有任何之前的事件");
    for id in [&c.id, &b1.id, &b2.id, &b3.id] {
        assert!(timeline::verify(w.conn(), id).unwrap().ok);
    }
    // 重命名与归档。
    timeline::rename(w.conn(), &b1.id, "封印线").unwrap();
    timeline::set_archived(w.conn(), &b1.id, true).unwrap();
    let b1 = campaign::get_campaign(w.conn(), &b1.id).unwrap();
    assert_eq!((b1.name.as_str(), b1.archived), ("封印线", true));
}

#[test]
fn verify_detects_tampering_and_fork_refuses_bad_checkpoints() {
    let mut f = fx();
    let (c, _, _, cp2, _) = play_original(&mut f);
    let w = &mut f.e.world;
    timeline::end_campaign(w, &c.id, "").unwrap();
    assert!(timeline::verify(w.conn(), &c.id).unwrap().ok);
    // 1. 直接改检查点内容 → 校验值不符，回溯拒绝使用。
    w.conn().execute("UPDATE campaign_checkpoints SET state = replace(state, 'active', 'done') WHERE id = ?1", [cp2]).unwrap();
    let rep = timeline::verify(w.conn(), &c.id).unwrap();
    assert_eq!(rep.checksum_failures, vec![cp2]);
    assert!(timeline::fork(w, &c.id, cp2, "", false).is_err());
    // 2. 连校验值一起伪造 → 重放事件后与检查点对不上，被发现。
    let json: String = w.conn().query_row("SELECT state FROM campaign_checkpoints WHERE id = ?1", [cp2], |r| r.get(0)).unwrap();
    w.conn().execute("UPDATE campaign_checkpoints SET checksum = ?2 WHERE id = ?1", rusqlite::params![cp2, timeline::checksum(&json)]).unwrap();
    let rep = timeline::verify(w.conn(), &c.id).unwrap();
    assert!(!rep.ok);
    assert!(rep.mismatches.iter().any(|m| m.contains("任务状态")), "{rep:#?}");
    // 3. 事件日志本身只能追加，不能改写。
    assert!(w.conn().execute("UPDATE event_log SET payload = '{}' WHERE campaign_id = ?1", [&c.id]).is_err());
}

#[test]
fn campaigns_from_before_the_upgrade_get_a_baseline() {
    let mut f = fx();
    let w = &mut f.e.world;
    let c = campaign::create_campaign(w, Some(&f.script), "旧战役", Some(1)).unwrap();
    campaign::take_edge(w, &c.id, &f.to_choice).unwrap();
    // 模拟升级前：没有任何检查点。
    w.conn().execute("DELETE FROM campaign_checkpoints WHERE campaign_id = ?1", [&c.id]).unwrap();
    timeline::end_campaign(w, &c.id, "未完成").unwrap();
    let labels: Vec<String> = timeline::checkpoints(w.conn(), &c.id).unwrap().into_iter().map(|c| c.label).collect();
    assert_eq!(labels, vec!["migrated", "ending"]);
    let rep = timeline::verify(w.conn(), &c.id).unwrap();
    assert!(rep.ok && rep.notes.iter().any(|n| n.contains("升级前")), "{rep:#?}");
}

#[test]
fn checkpoint_storage_stays_small() {
    // 每个回合一个检查点：记忆与摘要只存数量，状态表行数不随回合线性膨胀。
    let mut f = fx();
    let w = &mut f.e.world;
    let c = campaign::create_campaign(w, Some(&f.script), "长战役", Some(1)).unwrap();
    for i in 0..200 {
        turn(w, &c.id, &format!("第{i}回合"), json!([{"type": "inc_var", "key": "steps", "by": 1}]), |_| {});
    }
    let total: i64 = w.conn().query_row("SELECT SUM(length(state)) FROM campaign_checkpoints WHERE campaign_id = ?1", [&c.id], |r| r.get(0)).unwrap();
    assert!(total / 201 < 2000, "平均每个检查点 {} 字节", total / 201);
    timeline::end_campaign(w, &c.id, "").unwrap();
    let t = std::time::Instant::now();
    assert!(timeline::verify(w.conn(), &c.id).unwrap().ok);
    let cp = timeline::checkpoints(w.conn(), &c.id).unwrap()[100].id;
    let b = timeline::fork(w, &c.id, cp, "", false).unwrap();
    assert_eq!(timeline::capture(w.conn(), &b.id).unwrap().core.vars.get("steps").map(String::as_str), Some("100"));
    assert!(t.elapsed().as_secs() < 5);
}

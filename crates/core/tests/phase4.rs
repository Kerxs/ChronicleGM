//! 调查板、动态任务、世界时钟与 NPC 日程、NPC 情绪：功能、事件来源与时间线回溯。

mod common;

use chronicle_core::campaign;
use chronicle_core::investigation::{self, HypothesisInput};
use chronicle_core::quests::{self, NewQuest};
use chronicle_core::schedule;
use chronicle_core::timeline;
use chronicle_core::*;
use common::*;
use serde_json::{Value, json};

struct Fx {
    e: Env,
    script: String,
    smith: String,
    forge: String,
    tavern: String,
    quest: String,
    clue_a: String,
    clue_b: String,
    camp: String,
}

fn fx() -> Fx {
    let mut e = env();
    let u = user();
    let town = mk(&mut e.world, NewNode::new(NodeKind::City, "灰石村"));
    let forge = mk(&mut e.world, NewNode::new(NodeKind::Site, "铁匠铺").under(&town.id));
    let tavern = mk(&mut e.world, NewNode::new(NodeKind::Site, "酒馆").under(&town.id));
    let smith = mk(
        &mut e.world,
        NewNode::new(NodeKind::Character, "铁匠阿岩").payload(json!({
            "identity": "铁匠",
            "schedule": [
                {"start": "08:00", "end": "18:00", "activity": "在铁匠铺打铁", "location": forge.id},
                {"start": "18:00", "end": "22:00", "activity": "在酒馆喝酒", "location": tavern.id},
                {"start": "22:00", "end": "08:00", "activity": "回家睡觉", "location": ""}
            ]
        })),
    );
    let s = e.world.write(&u, "s", |w| w.create_script(NewScript::new("钥匙失窃"))).unwrap().0;
    let item = |w: &mut World, k: ScriptItemKind, t: &str, p: Value, pt: &str| {
        let mut it = NewScriptItem::new(&s.id, k, t);
        it.payload = p;
        it.player_text = pt.into();
        it.gm_text = format!("{t} 的真相（主持人）");
        w.write(&u, "i", |x| x.create_script_item(it)).unwrap().0.id
    };
    let clue_a = item(&mut e.world, ScriptItemKind::Clue, "铁砧下的划痕", json!({}), "铁砧底下有新的划痕");
    let clue_b = item(&mut e.world, ScriptItemKind::Clue, "酒馆账单", json!({}), "铁匠昨晚付了一大笔酒钱");
    let quest = item(
        &mut e.world,
        ScriptItemKind::Quest,
        "找回古铜钥匙",
        json!({
            "quest_type": "main",
            "precondition": {"type": "clue_known", "clue": clue_a},
            "complete_when": {"type": "flag", "name": "key_returned"},
            "fail_when": {"type": "flag", "name": "key_destroyed"},
            "deadline_turns": 3,
            "npc_ids": [smith.id],
            "location": forge.id,
            "clue_ids": [clue_a, clue_b]
        }),
        "找回从遗迹失窃的古铜钥匙",
    );
    item(&mut e.world, ScriptItemKind::Quest, "神秘的委托人", json!({"quest_type": "hidden"}), "");
    e.world.write(&u, "c", |w| w.add_cast(NewCastMember { script_id: s.id.clone(), node_id: smith.id.clone(), role: "铁匠".into(), motive: String::new(), secret: String::new(), relation_to_players: String::new(), payload: json!({}) })).unwrap();
    let camp = campaign::create_campaign(&mut e.world, Some(&s.id), "调查", Some(5)).unwrap().id;
    Fx { e, script: s.id, smith: smith.id, forge: forge.id, tavern: tavern.id, quest, clue_a, clue_b, camp }
}

fn eff(w: &mut World, c: &str, v: Value) {
    let e: Vec<chronicle_rules::Effect> = serde_json::from_value(v).unwrap();
    campaign::apply_effects(w, c, &e, "测试").unwrap();
}

#[test]
fn investigation_board_with_evidence_chains_and_player_owned_hypotheses() {
    let mut f = fx();
    let (w, c) = (&mut f.e.world, f.camp.clone());
    eff(w, &c, json!([{"type": "reveal_clue", "clue": f.clue_a}, {"type": "reveal_clue", "clue": f.clue_b}]));
    let gm_found = investigation::add_clue(w, &c, "铁匠手上的铜绿", "铁匠指缝里有铜绿", "gm").unwrap();
    assert_eq!(investigation::add_clue(w, &c, "铁匠手上的铜绿", "重复", "gm").unwrap(), gm_found, "同名发现不重复记录");
    let note = investigation::add_clue(w, &c, "我的猜测", "也许村长也知情", "player").unwrap();
    investigation::link(w, &c, &f.clue_a, &gm_found, "supports", "都指向铁匠").unwrap();
    investigation::link(w, &c, &gm_found, &f.clue_b, "supports", "").unwrap();
    investigation::link(w, &c, &f.clue_b, &note, "contradicts", "时间对不上").unwrap();
    assert!(investigation::link(w, &c, &f.clue_a, &f.clue_a, "supports", "").is_err());
    assert!(investigation::link(w, &c, &f.clue_a, "不存在", "supports", "").is_err());
    // 玩家的推理：故意写一个错误的推理，系统不评判。
    let wrong = investigation::save_hypothesis(w, &c, None, &HypothesisInput { text: "村长偷了钥匙".into(), evidence: vec![note.clone()], against: vec![], status: "believed".into() }).unwrap();
    let h = investigation::save_hypothesis(w, &c, None, &HypothesisInput { text: "铁匠偷了钥匙".into(), evidence: vec![f.clue_a.clone()], against: vec![], status: "open".into() }).unwrap();
    let b = investigation::board(w.conn(), &c, false).unwrap();
    assert_eq!(b.clues.len(), 4);
    assert!(b.clues.iter().all(|x| x.gm_note.is_empty()), "玩家视角不含主持人信息");
    assert!(investigation::board(w.conn(), &c, true).unwrap().clues.iter().any(|x| x.gm_note.contains("真相")));
    let hh = b.hypotheses.iter().find(|x| x.id == h).unwrap();
    assert_eq!(hh.indirect, { let mut v = vec![gm_found.clone(), f.clue_b.clone()]; v.sort(); v }, "沿支持关联找到间接证据（证据链）");
    assert_eq!(hh.conflicts, vec![note.clone()], "与证据链矛盾的线索被列出");
    assert_eq!(b.hypotheses.iter().find(|x| x.id == wrong).unwrap().status, "believed", "错误的推理照样保留玩家的判断");
    // GM 摘要明确标注推理是玩家的、可能是错的。
    let s = investigation::gm_summary(w.conn(), &c).unwrap();
    assert!(s.contains("玩家相信；可能是错的") && s.contains("村长偷了钥匙"));
    // 只能删除玩家笔记，删除时一并删除相关关联。
    assert!(investigation::remove_note(w, &c, &f.clue_a).is_err());
    investigation::remove_note(w, &c, &note).unwrap();
    assert!(!investigation::board(w.conn(), &c, false).unwrap().links.iter().any(|l| l.a == note || l.b == note));
    // 每次修改都有事件记录。
    let n = campaign::events(w.conn(), &c, 0, 1000).unwrap().iter().filter(|e| e.payload.to_string().contains("upsert_record") || e.payload.to_string().contains("delete_record")).count();
    assert!(n >= 7, "{n}");
}

#[test]
fn quests_advance_by_conditions_deadlines_and_gm_changes() {
    let mut f = fx();
    let (w, c) = (&mut f.e.world, f.camp.clone());
    let board = |w: &World| quests::quests(w.conn(), &c, false).unwrap();
    assert!(!board(w).iter().any(|q| q.title == "神秘的委托人"), "隐藏任务开始前玩家看不到");
    assert!(quests::quests(w.conn(), &c, true).unwrap().iter().any(|q| q.title == "神秘的委托人"));
    assert_eq!(board(w).iter().find(|q| q.id == f.quest).unwrap().status, "");
    // 前置条件满足 → 开始。
    eff(w, &c, json!([{"type": "reveal_clue", "clue": f.clue_a}]));
    assert_eq!(quests::tick(w, &c).unwrap(), vec!["任务「找回古铜钥匙」开始：前置条件已满足".to_string()]);
    let q = board(w).into_iter().find(|q| q.id == f.quest).unwrap();
    assert_eq!((q.status.as_str(), q.turns_left, q.npcs[0].title.as_str(), q.location.as_ref().unwrap().title.as_str()), ("active", Some(3), "铁匠阿岩", "铁匠铺"));
    assert_eq!(q.known_clues, vec![f.clue_a.clone()]);
    // 期限：过了 3 个回合还没完成 → 失败。
    for _ in 0..2 {
        campaign::advance_turn(w.conn(), &c).unwrap();
        assert!(quests::tick(w, &c).unwrap().is_empty());
    }
    campaign::advance_turn(w.conn(), &c).unwrap();
    let notes = quests::tick(w, &c).unwrap();
    assert!(notes[0].contains("超过 3 回合的期限"), "{notes:?}");
    assert_eq!(board(w).into_iter().find(|q| q.id == f.quest).unwrap().status, "failed");
    // GM 新建的任务：立即开始，满足完成条件后完成；任务变化记入历史。
    let gq = quests::add_quest(w, &c, &NewQuest { title: "安抚铁匠".into(), objective: "让铁匠冷静下来".into(), deadline_turns: Some(5), npc_ids: vec![f.smith.clone()], ..Default::default() }, "GM：新的委托").unwrap();
    assert_eq!(quests::add_quest(w, &c, &NewQuest { title: "安抚铁匠".into(), ..Default::default() }, "x").unwrap(), gq, "同名任务不重复创建");
    quests::add_quest_note(w, &c, &gq, "铁匠要求先道歉").unwrap();
    let v = board(w).into_iter().find(|q| q.id == gq).unwrap();
    assert_eq!((v.status.as_str(), v.source.as_str(), v.notes[0].text.as_str()), ("active", "gm", "铁匠要求先道歉"));
    assert!(quests::quest_exists(w.conn(), &c, &gq).unwrap());
    // GM 把剧本里还没开始的任务当作"新任务"提出：不重复创建，而是让剧本任务开始。
    let hidden = quests::quests(w.conn(), &c, true).unwrap().into_iter().find(|q| q.title == "神秘的委托人").unwrap();
    assert_eq!(quests::add_quest(w, &c, &NewQuest { title: "神秘的委托人".into(), ..Default::default() }, "剧情中出现").unwrap(), hidden.id);
    assert_eq!(quests::quests(w.conn(), &c, false).unwrap().into_iter().find(|q| q.id == hidden.id).unwrap().status, "active", "隐藏任务开始后玩家可见");
}

#[test]
fn clock_schedules_presence_offscreen_activity_and_mood_decay() {
    let mut f = fx();
    let (w, c) = (&mut f.e.world, f.camp.clone());
    assert_eq!(schedule::format_clock(schedule::clock(w.conn(), &c)), "第 1 天 08:00（上午）");
    let smith = nodes::get_node(w.conn(), &f.smith).unwrap().unwrap();
    let st = schedule::npc_status(w.conn(), &c, &smith).unwrap();
    assert_eq!((st.activity.as_str(), st.location_name.as_str()), ("在铁匠铺打铁", "铁匠铺"));
    assert!(schedule::is_at(w.conn(), &st, &f.forge).unwrap());
    assert!(!schedule::is_at(w.conn(), &st, &f.tavern).unwrap());
    // 生气的铁匠：情绪随时间平复。
    eff(w, &c, json!([{"type": "adjust_mood", "npc": f.smith, "anger": 60, "fear": 10, "suspicion": 30, "reason": "被当众指责"}]));
    // 推进 11 小时到 19:00：铁匠下班去酒馆喝酒（玩家不在场也会发生），生成一条活动记录。
    let notes = schedule::advance(w, &c, 11 * 60, "时间流逝").unwrap();
    assert_eq!(notes, vec!["铁匠阿岩：在酒馆喝酒（酒馆）".to_string()]);
    assert_eq!(schedule::format_clock(schedule::clock(w.conn(), &c)), "第 1 天 19:00（傍晚）");
    let st = schedule::npc_status(w.conn(), &c, &smith).unwrap();
    assert!(schedule::is_at(w.conn(), &st, &f.tavern).unwrap());
    let m = { use chronicle_rules::StateView; campaign::CampaignView { conn: w.conn(), campaign_id: &c }.mood(&f.smith).unwrap() };
    assert_eq!((m.anger, m.fear, m.suspicion), (5, 0, 8), "11 小时：愤怒 -55、恐惧降到 0、怀疑 -22");
    assert!(campaign::events(w.conn(), &c, 0, 1000).unwrap().iter().any(|e| e.kind == "npc_activity"));
    // 时钟是受保护的状态变量，参与回溯。
    timeline::checkpoint(w.conn(), &c, "turn_end").unwrap();
    timeline::end_campaign(w, &c, "测试").unwrap();
    assert!(timeline::verify(w.conn(), &c).unwrap().ok, "情绪、记录、时钟都能从事件重放");
}

#[test]
fn investigation_quests_and_moods_follow_timelines() {
    let mut f = fx();
    let (w, c) = (&mut f.e.world, f.camp.clone());
    eff(w, &c, json!([{"type": "reveal_clue", "clue": f.clue_a}]));
    investigation::add_clue(w, &c, "早期发现", "", "gm").unwrap();
    quests::tick(w, &c).unwrap();
    timeline::checkpoint(w.conn(), &c, "turn_end").unwrap();
    let cp = timeline::checkpoints(w.conn(), &c).unwrap().last().unwrap().id;
    investigation::save_hypothesis(w, &c, None, &HypothesisInput { text: "之后的推理".into(), ..Default::default() }).unwrap();
    quests::add_quest(w, &c, &NewQuest { title: "之后的任务".into(), ..Default::default() }, "x").unwrap();
    eff(w, &c, json!([{"type": "adjust_mood", "npc": f.smith, "fear": 40, "reason": "之后的惊吓"}]));
    schedule::advance(w, &c, 30, "x").unwrap();
    timeline::checkpoint(w.conn(), &c, "turn_end").unwrap();
    timeline::end_campaign(w, &c, "").unwrap();
    let b = timeline::fork(w, &c, cp, "", false).unwrap();
    let bd = investigation::board(w.conn(), &b.id, false).unwrap();
    assert!(bd.clues.iter().any(|x| x.title == "早期发现") && bd.hypotheses.is_empty());
    assert!(!quests::quests(w.conn(), &b.id, true).unwrap().iter().any(|q| q.title == "之后的任务"));
    assert_eq!(quests::quests(w.conn(), &b.id, false).unwrap().iter().find(|q| q.id == f.quest).unwrap().status, "active");
    assert_eq!(schedule::clock(w.conn(), &b.id), schedule::DEFAULT_START);
    let cmp = timeline::compare(w.conn(), &[c.clone(), b.id.clone()]).unwrap();
    assert!(cmp.rows.iter().any(|r| r.category == "NPC 情绪" && r.differs));
    assert!(cmp.rows.iter().any(|r| r.category == "调查与任务记录" && r.differs));
    for id in [&c, &b.id] {
        assert!(timeline::verify(w.conn(), id).unwrap().ok);
    }
    let _ = (&f.script, &f.clue_b);
}

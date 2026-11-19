//! 世界初始化 7 步（改革第 2 阶段）：真实执行、幂等、失败停在那一步、完成后进入开场。

use std::sync::Mutex;

use chronicle_core::gamestate::{self, Phase, StepStatus};
use chronicle_core::rpg::{self, PARTY};
use chronicle_core::*;
use chronicle_gen::Templates;
use chronicle_gm::GmOptions;
use chronicle_gm::init::run_init;
use chronicle_llm::*;
use serde_json::json;

/// 假模型：装备推断给一把小刀，其余请求回"好"。记下收到几次请求。
struct Mock(Mutex<Vec<String>>);
impl LlmClient for Mock {
    fn chat(&self, req: &ChatRequest, _c: &CancelToken, _p: client::Progress) -> chronicle_llm::Result<ChatResponse> {
        let u = req.messages.last().map(|m| m.content.clone()).unwrap_or_default();
        self.0.lock().unwrap().push(u.chars().take(30).collect());
        let content = if u.contains("随身带着的装备") { json!({"items": [{"name": "船匠小刀", "qty": 1, "slot": "weapon", "damage": "1d4", "range": 1}]}).to_string() } else { "好".into() };
        Ok(ChatResponse { content, finish_reason: Some("stop".into()), usage: Usage::default(), model: None, latency_ms: 1, attempts: 1, used_json_schema: false })
    }
    fn describe(&self) -> (String, String) {
        ("mock".into(), "mock".into())
    }
}

fn demo_campaign(choose_pc: bool) -> (tempfile::TempDir, Mutex<World>, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "初始化").unwrap();
    let sid = demo::populate(&mut w).unwrap();
    let c = campaign::create_campaign(&mut w, Some(&sid), "灯塔", Some(3)).unwrap();
    if choose_pc {
        let pcs = scripts::items_by_kind(w.conn(), &sid, ScriptItemKind::PlayerCharacter).unwrap();
        pc::set(&mut w, &c.id, &pc::from_script_item(&pcs[0])).unwrap();
    }
    (dir, Mutex::new(w), c.id)
}

#[test]
fn seven_real_steps_then_opening_and_rerun_is_idempotent() {
    let (_d, world, c) = demo_campaign(true);
    let mock = Mock(Mutex::default());
    let seen: Mutex<Vec<(String, StepStatus)>> = Mutex::default();
    let on = |s: &gamestate::GameState| {
        for st in &s.init {
            let mut v = seen.lock().unwrap();
            if st.status != StepStatus::Pending && !v.iter().any(|l| l.0 == st.key && l.1 == st.status) {
                v.push((st.key.clone(), st.status));
            }
        }
    };
    let s = run_init(&world, Some(&mock), &Templates::builtin(), &GmOptions::default(), &c, &CancelToken::new(), &on).unwrap();
    assert_eq!(s.phase, Phase::Opening, "{:?}", s.init);
    assert!(s.init_done());
    for st in &s.init {
        assert!(matches!(st.status, StepStatus::Done | StepStatus::Skipped), "{st:?}");
        assert!(!st.note.is_empty(), "每一步写明做了什么：{st:?}");
    }
    // 进度按顺序：每一步先进行中、再完成。
    let order: Vec<String> = seen.lock().unwrap().iter().filter(|x| x.1 == StepStatus::Done).map(|x| x.0.clone()).collect();
    assert_eq!(order, gamestate::INIT_STEPS.iter().map(|(k, _)| k.to_string()).collect::<Vec<_>>());
    let w = world.lock().unwrap();
    let bag = rpg::inventory(w.conn(), &c, PARTY).unwrap();
    assert!(bag.iter().any(|i| i.item.title == "船匠小刀"), "角色没写装备：由模型推断并发放");
    let npc_notes = &s.init.iter().find(|x| x.key == "npcs").unwrap().note;
    assert!(npc_notes.contains("莫伦") || npc_notes.contains("人物"), "{npc_notes}");
    let rel = &s.init.iter().find(|x| x.key == "relations").unwrap().note;
    assert!(rel.contains("阿澜"), "身世里写了阿澜是朋友：{rel}");
    assert!(timeline::checkpoints(w.conn(), &c).unwrap().iter().any(|cp| cp.label == "init"));
    let requests = mock.0.lock().unwrap().len();
    assert_eq!(requests, 2, "一次装备推断 + 一次预热");
    drop(w);
    // 再执行一次：已经在开场，什么都不做（不重复发装备、不重复加关系、不再请求模型）。
    let s2 = run_init(&world, Some(&mock), &Templates::builtin(), &GmOptions::default(), &c, &CancelToken::new(), &|_| {}).unwrap();
    assert_eq!(s2.phase, Phase::Opening);
    assert_eq!(mock.0.lock().unwrap().len(), requests);
    assert_eq!(rpg::inventory(world.lock().unwrap().conn(), &c, PARTY).unwrap().len(), bag.len());
}

#[test]
fn missing_pc_fails_at_step_three_and_retry_continues_from_there() {
    let (_d, world, c) = demo_campaign(false);
    let s = run_init(&world, None, &Templates::builtin(), &GmOptions::default(), &c, &CancelToken::new(), &|_| {}).unwrap();
    assert_eq!(s.phase, Phase::Initializing);
    let player = s.init.iter().find(|x| x.key == "player").unwrap();
    assert_eq!(player.status, StepStatus::Failed);
    assert!(player.note.contains("选择角色"), "{}", player.note);
    assert!(s.init.iter().take(2).all(|x| x.status == StepStatus::Done));
    assert!(s.init.iter().skip(3).all(|x| x.status == StepStatus::Pending), "失败后不继续后面的步骤");
    // 选好角色后重试：前两步不重做，从第 3 步继续；没有模型服务时装备留空并写明。
    {
        let mut w = world.lock().unwrap();
        let sid = campaign::get_campaign(w.conn(), &c).unwrap().script_id.unwrap();
        let pcs = scripts::items_by_kind(w.conn(), &sid, ScriptItemKind::PlayerCharacter).unwrap();
        pc::set(&mut w, &c, &pc::from_script_item(&pcs[1])).unwrap();
    }
    let s = run_init(&world, None, &Templates::builtin(), &GmOptions::default(), &c, &CancelToken::new(), &|_| {}).unwrap();
    assert_eq!(s.phase, Phase::Opening);
    let player = s.init.iter().find(|x| x.key == "player").unwrap();
    assert!(player.note.contains("没有模型服务"), "{}", player.note);
    assert!(s.init.iter().find(|x| x.key == "gm_context").unwrap().note.contains("没有设置模型服务"));
}

#[test]
fn cannot_init_a_campaign_already_in_play() {
    let (_d, world, c) = demo_campaign(true);
    gamestate::transition(&mut world.lock().unwrap(), &c, Phase::Exploring, "直接开始").unwrap();
    let e = run_init(&world, None, &Templates::builtin(), &GmOptions::default(), &c, &CancelToken::new(), &|_| {}).unwrap_err();
    assert!(e.to_string().contains("不能初始化"), "{e}");
}

#[test]
fn combat_narration_only_writes_text_and_records_it() {
    let (_d, world, c) = demo_campaign(true);
    let mock = Mock(Mutex::default());
    let before = {
        let w = world.lock().unwrap();
        (campaign::get_campaign(w.conn(), &c).unwrap().turn, rpg::sheet(w.conn(), &c, PARTY).unwrap().hp)
    };
    let lines = vec!["苇生用徒手攻击走私贩1：掷出 3+1=4，防御 10，没有命中".to_string()];
    let t = chronicle_gm::combat_narrate::narrate_combat(&world, &mock, &Templates::builtin(), &GmOptions::default(), &c, &lines, &CancelToken::new());
    // 假模型回的是"好"（不是 JSON）：描述生成失败，但不改任何状态。
    let w = world.lock().unwrap();
    let after = (campaign::get_campaign(w.conn(), &c).unwrap().turn, rpg::sheet(w.conn(), &c, PARTY).unwrap().hp);
    assert_eq!(before, after, "战斗描述不改回合与数值");
    assert!(mock.0.lock().unwrap().iter().any(|p| p.contains("战斗中的一步")), "请求里带着结算");
    let _ = t;
    // 没有结算行：不请求模型。
    drop(w);
    let n = mock.0.lock().unwrap().len();
    assert_eq!(chronicle_gm::combat_narrate::narrate_combat(&world, &mock, &Templates::builtin(), &GmOptions::default(), &c, &[], &CancelToken::new()).unwrap(), "");
    assert_eq!(mock.0.lock().unwrap().len(), n);
}

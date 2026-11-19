//! 沉浸式互动和 GM 回合：AI 只提出互动，程序校验、结算；结果交给下一回合的叙述；自由输入照常处理。
//! 模型是 mock，只验证程序侧。

use std::sync::Mutex;

use chronicle_core::interaction::{self, PlayerOp};
use chronicle_core::rules::Effect;
use chronicle_core::*;
use chronicle_gen::Templates;
use chronicle_gm::{Engine, GmOptions, run_turn};
use chronicle_llm::*;
use serde_json::{Value, json};

struct Router {
    plan: Mutex<Vec<String>>,
    seen: Mutex<Vec<ChatRequest>>,
}

impl Router {
    fn new(plans: Vec<Value>) -> Self {
        Self { plan: Mutex::new(plans.into_iter().map(|p| p.to_string()).collect()), seen: Mutex::default() }
    }
    fn kind(r: &ChatRequest) -> &'static str {
        let u = &r.messages[1].content;
        if u.contains("规则审查员") {
            "rail"
        } else if u.contains("你现在扮演 TRPG 中的人物") {
            "npc"
        } else if u.contains("程序已经完成了掷骰与规则判定") {
            "narrate"
        } else if u.contains("协助 TRPG 程序理解玩家") {
            "intent"
        } else {
            "plan"
        }
    }
    fn plans(&self) -> Vec<String> {
        self.seen.lock().unwrap().iter().filter(|r| Self::kind(r) == "plan").map(|r| r.messages[1].content.clone()).collect()
    }
}

impl LlmClient for Router {
    fn chat(&self, req: &ChatRequest, _c: &CancelToken, _p: client::Progress) -> chronicle_llm::Result<ChatResponse> {
        self.seen.lock().unwrap().push(req.clone());
        let content = match Self::kind(req) {
            "rail" => json!({"issues": []}).to_string(),
            "narrate" => json!({"narration": "（按结果的叙述）"}).to_string(),
            "npc" => json!({"reply": "嗯。", "attitude_change": 0}).to_string(),
            "intent" => "{}".into(),
            _ => {
                let mut p = self.plan.lock().unwrap();
                if p.len() > 1 { p.remove(0) } else { p[0].clone() }
            }
        };
        Ok(ChatResponse { content, finish_reason: Some("stop".into()), usage: Usage::default(), model: None, latency_ms: 1, attempts: 1, used_json_schema: false })
    }
    fn describe(&self) -> (String, String) {
        ("mock".into(), "mock".into())
    }
}

struct Fx {
    _dir: tempfile::TempDir,
    world: Mutex<World>,
    campaign: String,
    key: String,
}

fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let mut w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "测试").unwrap();
    let u = Author::User;
    let key = w.write(&u, "n", |x| x.create_node(NewNode::new(NodeKind::Item, "铁钥匙").payload(json!({"unlocks": ["地窖门"]})))).unwrap().0.id;
    let room = w
        .write(&u, "n", |x| x.create_node(NewNode::new(NodeKind::Site, "地窖口").payload(json!({"objects": [{"id": "door", "name": "地窖门", "kind": "door", "description": "包着铁皮的木门", "lock": {"kind": "key", "dc": 14}}]}))))
        .unwrap()
        .0
        .id;
    let c = campaign::create_campaign(&mut w, None, "测试团", Some(5)).unwrap().id;
    campaign::apply_effects(&mut w, &c, &[Effect::SetVar { key: "party.location".into(), value: json!(room) }, Effect::GiveItem { holder: rpg::actor(), item: key.clone(), qty: 1 }], "开始").unwrap();
    Fx { _dir: dir, world: Mutex::new(w), campaign: c, key }
}

fn turn(fx: &Fx, gm: &Router, input: &str) -> chronicle_gm::TurnResult {
    run_turn(&fx.world, &Engine { gm, npc: gm, templates: &Templates::builtin(), opts: &GmOptions::default() }, &fx.campaign, input, &CancelToken::new()).unwrap()
}

#[test]
fn gm_proposes_program_resolves_next_turn_narrates() {
    let fx = fixture();
    let gm = Router::new(vec![
        json!({"intent": "开门", "interaction": {"type": "lock", "target": "O1", "title": "打开地窖门", "reason": "门锁着"}, "narration": "你走到门前，掏出钥匙。"}),
        json!({"intent": "下地窖", "narration": "门开了，你走下台阶。"}),
    ]);
    let r = turn(&fx, &gm, "我要打开地窖门");
    assert_eq!(r.interactions.len(), 1, "程序打开了互动");
    let ix = &r.interactions[0];
    assert!(ix.available_actions.iter().any(|a| a.id == "key" && a.available));
    let p = gm.plans()[0].clone();
    assert!(p.contains("这里可以互动的东西") && p.contains("O1 地窖门"), "场景对象交给 GM");
    // 玩家亲自操作。
    {
        let mut w = fx.world.lock().unwrap();
        let a = interaction::act(&mut w, &fx.campaign, &ix.interaction_id, &PlayerOp { action: "key".into(), ..Default::default() }).unwrap();
        assert!(a.finished && a.interaction.result.unwrap().success);
    }
    // 下一回合：结果作为程序事实交给 GM。
    let r2 = turn(&fx, &gm, "〔亲自操作〕打开地窖门：成功");
    let p2 = gm.plans()[1].clone();
    assert!(p2.contains("程序结算：玩家刚亲自完成的互动") && p2.contains("地窖门"), "结果交给叙述");
    assert!(r2.rpg_updates.iter().any(|l| l.contains("地窖门")));
    let _ = &fx.key;
}

#[test]
fn ai_cannot_declare_the_lock_open() {
    let fx = fixture();
    let gm = Router::new(vec![json!({"intent": "开门", "narration": "门一下子就开了。", "effects": [{"type": "upsert_record", "kind": "object_state", "id": "x:door", "data": {"opened": true}}]})]);
    let r = turn(&fx, &gm, "我推门");
    assert!(r.rejected.iter().any(|x| x.reason.contains("由程序管理")), "{:?}", r.rejected);
    let w = fx.world.lock().unwrap();
    let here = interaction::objects_here(w.conn(), &fx.campaign, true).unwrap();
    assert!(!here[0].state.opened, "门还是锁着的");
}

#[test]
fn free_action_bypasses_the_offered_options() {
    let fx = fixture();
    // 已经有一个开锁互动在等，玩家却写了别的办法：GM 照常处理（提出力量检定）。
    {
        let mut w = fx.world.lock().unwrap();
        interaction::open(&mut w, &fx.campaign, &interaction::OpenRequest { kind: "lock".into(), target: "地窖门".into(), ..Default::default() }).unwrap();
    }
    let gm = Router::new(vec![json!({"intent": "撞门", "checks": [{"skill": "运动", "dc": 15, "reason": "用桌腿撞门"}], "narration": "你拆下桌腿，朝门撞去。"})]);
    let r = turn(&fx, &gm, "我把旁边的桌子拆下来，用桌腿撞开门");
    assert_eq!(r.checks.len(), 1, "自由行动照常检定");
    assert!(gm.plans()[0].contains("进行中的互动"), "GM 知道有待处理的互动，但不强迫玩家用它");
}

#[test]
fn opening_turn_has_a_prologue_ritual_and_multiplayer_skips_interactions() {
    let fx = fixture();
    let gm = Router::new(vec![json!({"intent": "开场", "narration": "雾从海上漫进来。"})]);
    let r = turn(&fx, &gm, chronicle_gm::OPENING);
    assert!(r.rituals.iter().any(|c| c.scene == "prologue" && c.level == 4));
    // 联机房间：不打开互动（界面还不支持），也告诉 GM 不要提。
    {
        let mut w = fx.world.lock().unwrap();
        campaign::apply_effects(&mut w, &fx.campaign, &[Effect::SetVar { key: "party.members".into(), value: json!([rpg::actor(), "player:p2"]) }], "联机").unwrap();
    }
    let gm2 = Router::new(vec![json!({"intent": "开门", "interaction": {"type": "lock", "target": "O1"}, "narration": "你走到门前。"})]);
    let r2 = turn(&fx, &gm2, "我开门");
    assert!(r2.interactions.is_empty());
    assert!(gm2.plans()[0].contains("联机模式"));
}

//! 反剧情纠正改革的模拟测试（方案测试 A–F 与剧情纠正检测）。
//! 模型是 mock：只验证程序侧——事实保护、事件状态、上下文标注、检测与重写——不代表真实模型的表现。

use std::sync::Mutex;

use chronicle_core::campaign::{self, CampaignView};
use chronicle_core::events::{self, EventStatus};
use chronicle_core::rules::{Effect, StateView};
use chronicle_core::*;
use chronicle_gen::Templates;
use chronicle_gm::{Engine, GmOptions, run_turn};
use chronicle_llm::*;
use serde_json::{Value, json};

struct Router {
    plan: Mutex<Vec<String>>,
    narration: Mutex<Vec<String>>,
    rail: Mutex<Vec<String>>,
    director: String,
    seen: Mutex<Vec<ChatRequest>>,
}

impl Router {
    fn new(plans: Vec<Value>) -> Self {
        Self {
            plan: Mutex::new(plans.into_iter().map(|p| p.to_string()).collect()),
            narration: Mutex::new(vec![json!({"narration": "（按结果的叙述）"}).to_string()]),
            rail: Mutex::new(vec![json!({"issues": []}).to_string()]),
            director: json!({"assessment": "玩家一直在码头钓鱼", "pacing": "hold", "suggestions": [{"kind": "npc_action", "text": "让村长找上门催玩家去遗迹", "why": "停滞", "uses": [], "touches_canon": false}]}).to_string(),
            seen: Mutex::default(),
        }
    }
    fn kind(r: &ChatRequest) -> &'static str {
        let u = &r.messages[1].content;
        if r.messages[0].content.contains("剧情导演") || u.starts_with("你是这场 TRPG 的剧情导演") {
            "director"
        } else if u.contains("规则审查员") {
            "rail"
        } else if r.messages[0].content.contains("扮演一个人物") || u.contains("你现在扮演 TRPG 中的人物") {
            "npc"
        } else if u.contains("程序已经完成了掷骰与规则判定") {
            "narrate"
        } else if u.contains("协助 TRPG 程序理解玩家") {
            "intent"
        } else {
            "plan"
        }
    }
    fn prompts(&self, kind: &str) -> Vec<String> {
        self.seen.lock().unwrap().iter().filter(|r| Self::kind(r) == kind).map(|r| r.messages[1].content.clone()).collect()
    }
    fn next(q: &Mutex<Vec<String>>) -> String {
        let mut p = q.lock().unwrap();
        if p.len() > 1 { p.remove(0) } else { p[0].clone() }
    }
}

impl LlmClient for Router {
    fn chat(&self, req: &ChatRequest, _c: &CancelToken, _p: client::Progress) -> chronicle_llm::Result<ChatResponse> {
        self.seen.lock().unwrap().push(req.clone());
        let content = match Self::kind(req) {
            "director" => self.director.clone(),
            "rail" => Self::next(&self.rail),
            "narrate" => Self::next(&self.narration),
            "npc" => json!({"reply": "嗯。", "attitude_change": 0}).to_string(),
            "intent" => "{}".into(),
            _ => Self::next(&self.plan),
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
    elder: String,
    quest: String,
    key: String,
    ritual: String,
    edge: String,
}

fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let mut w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "测试").unwrap();
    let u = Author::User;
    let elder = w.write(&u, "n", |x| x.create_node(NewNode::new(NodeKind::Character, "村长老格").payload(json!({"identity": "村长"})))).unwrap().0.id;
    let s = w.write(&u, "s", |x| x.create_script(NewScript::new("遗迹委托"))).unwrap().0;
    let item = |w: &mut World, k: ScriptItemKind, t: &str, p: Value| {
        let mut it = NewScriptItem::new(&s.id, k, t);
        it.payload = p;
        w.write(&u, "i", |x| x.create_script_item(it)).unwrap().0.id
    };
    // 村长发布的主线委托（村长是发布者）。
    let quest = item(&mut w, ScriptItemKind::Quest, "调查遗迹", json!({"quest_type": "main", "npc_ids": [elder]}));
    let key = item(&mut w, ScriptItemKind::Item, "古铜钥匙", json!({}));
    // 关键事件：要用古铜钥匙打开祭坛（钥匙被毁则失效）。
    let ritual = item(&mut w, ScriptItemKind::KeyEvent, "祭坛开启", json!({"when": {"type": "all", "of": [{"type": "has_item", "item": key}, {"type": "flag", "name": "at_altar"}]}, "text": "祭坛打开了"}));
    let c = NewCastMember { script_id: s.id.clone(), node_id: elder.clone(), role: "村长".into(), motive: String::new(), secret: String::new(), relation_to_players: String::new(), payload: json!({}) };
    w.write(&u, "c", |x| x.add_cast(c)).unwrap();
    let tavern = w.write(&u, "p", |x| x.create_plot_node(NewPlotNode::new(&s.id, PlotNodeKind::Start, "酒馆"))).unwrap().0;
    let ruins = w.write(&u, "p", |x| x.create_plot_node(NewPlotNode::new(&s.id, PlotNodeKind::Scene, "遗迹"))).unwrap().0;
    let mut e = NewPlotEdge::new(&s.id, &tavern.id, &ruins.id);
    e.label = "接受委托".into();
    let edge = w.write(&u, "e", |x| x.create_plot_edge(e)).unwrap().0.id;
    let c = campaign::create_campaign(&mut w, Some(&s.id), "测试团", Some(7)).unwrap();
    Fx { _dir: dir, world: Mutex::new(w), campaign: c.id, elder, quest, key, ritual, edge }
}

fn turn(fx: &Fx, gm: &Router, input: &str) -> chronicle_gm::TurnResult {
    run_turn(&fx.world, &Engine { gm, npc: gm, templates: &Templates::builtin(), opts: &GmOptions::default() }, &fx.campaign, input, &CancelToken::new()).unwrap()
}

fn status(fx: &Fx, id: &str) -> EventStatus {
    let w = fx.world.lock().unwrap();
    events::get(w.conn(), &fx.campaign, id).map(|e| e.status).unwrap()
}

fn quest_state(fx: &Fx) -> String {
    let w = fx.world.lock().unwrap();
    campaign::campaign_state(w.conn(), &fx.campaign).unwrap().quests.get(&fx.quest).cloned().unwrap_or_default()
}

/// 测试 A：杀死关键 NPC。死亡保留；他发布的委托失效；GM 不能让他复活；他不再在场。
#[test]
fn a_killing_a_key_npc_sticks() {
    let fx = fixture();
    {
        let mut w = fx.world.lock().unwrap();
        campaign::apply_effects(&mut w, &fx.campaign, &[Effect::SetEntityState { entity: fx.elder.clone(), field: "life_status".into(), value: json!("dead") }], "玩家杀死了村长").unwrap();
    }
    let gm = Router::new(vec![json!({"intent": "离开酒馆", "narration": "你走出酒馆。"})]);
    let r = turn(&fx, &gm, "我离开酒馆");
    assert_eq!(status(&fx, &fx.quest), EventStatus::Invalid, "发布者死了，委托失效");
    assert_eq!(quest_state(&fx), "invalid");
    assert!(r.quest_updates.iter().any(|q| q.contains("已失效")), "{:?}", r.quest_updates);
    let p = gm.prompts("plan")[0].clone();
    assert!(p.contains("已确定的事实") && p.contains("村长老格已经死亡"), "死亡写进已确定的事实");
    assert!(!p.contains("N1 村长老格"), "死者不在在场人物里");
    // GM 想让他复活 → 被拒绝。
    let gm2 = Router::new(vec![json!({"intent": "村长回来了", "narration": "……", "effects": [{"type": "set_entity_state", "entity": fx.elder, "field": "life_status", "value": "alive"}]})]);
    let r2 = turn(&fx, &gm2, "我回到酒馆");
    assert!(r2.rejected.iter().any(|x| x.reason.contains("不允许复活")), "{:?}", r2.rejected);
    let w = fx.world.lock().unwrap();
    assert_eq!(CampaignView { conn: w.conn(), campaign_id: &fx.campaign }.life_status(&fx.elder).as_deref(), Some("dead"));
    // 失效后的委托不会被重新激活。
    drop(w);
    let gm3 = Router::new(vec![json!({"intent": "x", "narration": "……", "effects": [{"type": "set_quest_state", "quest": fx.quest, "state": "active"}]})]);
    let r3 = turn(&fx, &gm3, "我四处看看");
    assert!(r3.rejected.iter().any(|x| x.reason.contains("不能改回去")));
    assert_eq!(quest_state(&fx), "invalid");
}

/// 测试 B：拒绝主线任务。接受拒绝；不再激活、不再出现在选项里、不再列给模型。
#[test]
fn b_refusing_the_main_quest_is_respected() {
    let fx = fixture();
    let gm = Router::new(vec![json!({"intent": "拒绝委托", "player_goal": "去北方找哥哥", "event_updates": [{"event": "Q1", "status": "refused", "reason": "玩家明确说不去遗迹"}], "narration": "村长叹了口气。", "choices": ["收拾行李北上", "去调查遗迹", "在酒馆打听北方的消息"]})]);
    let r = turn(&fx, &gm, "我不去遗迹，我要去北方找我哥");
    assert_eq!(quest_state(&fx), "refused");
    assert_eq!(status(&fx, &fx.quest), EventStatus::Bypassed);
    assert!(!r.choices.iter().any(|c| c.contains("调查遗迹")), "选项里不再出现拒绝过的委托：{:?}", r.choices);
    {
        let w = fx.world.lock().unwrap();
        assert_eq!(events::player_goal(w.conn(), &fx.campaign).unwrap().text, "去北方找哥哥");
    }
    // 下一回合：玩家目标在显眼位置；拒绝的委托在已确定事实里，不再列为可引用的任务。
    let gm2 = Router::new(vec![json!({"intent": "赶路", "narration": "你走上北去的路。", "new_quests": [], "effects": [{"type": "set_quest_state", "quest": "Q1", "state": "active"}]})]);
    let r2 = turn(&fx, &gm2, "我出发");
    let p = gm2.prompts("plan")[0].clone();
    assert!(p.contains("玩家当前的目标") && p.contains("去北方找哥哥"));
    assert!(p.contains("委托「调查遗迹」已绕过"));
    assert!(!p.contains("[quest] 调查遗迹"), "拒绝的委托不再列给模型");
    assert!(r2.rejected.iter().any(|x| x.reason.contains("不能改回去")));
    assert_eq!(quest_state(&fx), "refused", "程序每回合推进任务也不会把它重新激活");
}

/// 测试 C：摧毁关键物品。物品消失并记为摧毁；依赖它的事件失效；GM 不能再给一件。
#[test]
fn c_destroying_a_key_item() {
    let fx = fixture();
    {
        let mut w = fx.world.lock().unwrap();
        campaign::apply_effects(&mut w, &fx.campaign, &[Effect::GiveItem { holder: rpg::actor(), item: fx.key.clone(), qty: 1 }], "初始").unwrap();
    }
    let gm = Router::new(vec![json!({"intent": "把钥匙扔进熔炉", "destroy_items": ["I1"], "narration": "钥匙在炉火里化成了铜水。"})]);
    let r = turn(&fx, &gm, "我把古铜钥匙扔进熔炉");
    assert!(r.rpg_updates.iter().any(|l| l.contains("毁掉")), "{:?}", r.rpg_updates);
    {
        let w = fx.world.lock().unwrap();
        assert!(events::is_destroyed(w.conn(), &fx.campaign, &fx.key));
        assert_eq!(CampaignView { conn: w.conn(), campaign_id: &fx.campaign }.item_qty(&rpg::actor(), &fx.key), 0);
    }
    assert_eq!(status(&fx, &fx.ritual), EventStatus::Invalid, "要用钥匙的事件失效");
    let gm2 = Router::new(vec![json!({"intent": "找到钥匙", "narration": "……", "effects": [{"type": "give_item", "item": "I1", "qty": 1}]})]);
    let r2 = turn(&fx, &gm2, "我在炉灰里翻找");
    assert!(r2.rejected.iter().any(|x| x.reason.contains("已经被摧毁")), "{:?}", r2.rejected);
}

/// 测试 D：非预设的解决方式。程序不强迫玩家走预设分支；上下文把分支标成"只是可能性"。
#[test]
fn d_unscripted_solution_is_not_forced_onto_the_preset_path() {
    let fx = fixture();
    let gm = Router::new(vec![json!({"intent": "雇船绕过遗迹", "off_script": true, "narration": "你在码头雇了一条船。"})]);
    let r = turn(&fx, &gm, "我不走山谷，雇条船从海上绕过去");
    assert!(r.transition.is_none(), "没有提出分支就不推进");
    let p = gm.prompts("plan")[0].clone();
    assert!(p.contains("玩家决定故事往哪里走"), "自由行动规则在最前面");
    assert!(p.contains("剧本预设的分支（只是可能性"));
    let w = fx.world.lock().unwrap();
    let c = campaign::get_campaign(w.conn(), &fx.campaign).unwrap();
    let _ = &fx.edge;
    assert!(c.current_plot_node.is_some());
}

/// 测试 E：完全偏离剧本。很多回合不碰主线也没有"必须推进主线"的程序提示；导演建议默认不自动采用。
#[test]
fn e_long_deviation_has_no_forced_pull_back() {
    let fx = fixture();
    let gm = Router::new(vec![json!({"intent": "钓鱼", "narration": "浮漂在水面轻轻晃动。", "time_passes_minutes": 30})]);
    for _ in 0..14 {
        turn(&fx, &gm, "我在码头钓鱼");
    }
    for p in gm.prompts("plan") {
        assert!(!p.contains("必须让一件事发生") && !p.contains("推进主线或离开这里"), "不再强制推进");
    }
    assert!(gm.prompts("plan").last().unwrap().contains("这本身没有问题"), "很久没变化时只提醒世界照常运转");
    let note = chronicle_gm::director::analyze(&fx.world, &gm, &Templates::builtin(), &GmOptions::default(), &fx.campaign, "stall", &CancelToken::new()).unwrap();
    assert!(note.suggestions.iter().all(|s| s.status != "adopted"), "默认不自动采用");
    let dp = gm.prompts("director").last().cloned().unwrap();
    assert!(dp.contains("因果依据") && dp.contains("玩家决定故事往哪里走"));
}

/// 测试 F：连续改变世界。终态在检查点、分叉和校验后保持，旧剧情不会覆盖新状态。
#[test]
fn f_changes_persist_through_timeline() {
    let fx = fixture();
    let gm = Router::new(vec![
        json!({"intent": "拒绝", "event_updates": [{"event": "Q1", "status": "refused"}], "narration": "你摇了摇头。"}),
        json!({"intent": "改写", "event_updates": [{"event": "V1", "status": "changed", "reason": "玩家把祭坛搬走了"}], "narration": "你们把祭坛撬了下来。"}),
        json!({"intent": "休息", "narration": "你睡了一觉。"}),
    ]);
    turn(&fx, &gm, "我不接");
    turn(&fx, &gm, "我们把祭坛整个撬走");
    turn(&fx, &gm, "睡觉");
    assert_eq!(status(&fx, &fx.ritual), EventStatus::Changed);
    // 触发条件满足也不会再触发（已经被改变）。
    {
        let mut w = fx.world.lock().unwrap();
        campaign::apply_effects(&mut w, &fx.campaign, &[Effect::GiveItem { holder: rpg::actor(), item: fx.key.clone(), qty: 1 }, Effect::SetFlag { name: "at_altar".into(), value: true }], "测试").unwrap();
        schedule::advance(&mut w, &fx.campaign, 60 * 30, "时间流逝").unwrap();
        assert!(CampaignView { conn: w.conn(), campaign_id: &fx.campaign }.record("trigger_fired", &fx.ritual).is_none(), "被改变的事件不再触发");
    }
    let mut w = fx.world.lock().unwrap();
    assert!(timeline::verify(w.conn(), &fx.campaign).unwrap().ok);
    let cps = timeline::checkpoints(w.conn(), &fx.campaign).unwrap();
    let last = cps.last().unwrap().id;
    let b = timeline::fork(&mut w, &fx.campaign, last, "", false).unwrap();
    assert_eq!(events::get(w.conn(), &b.id, &fx.ritual).unwrap().status, EventStatus::Changed, "分叉后保持");
    assert_eq!(campaign::campaign_state(w.conn(), &b.id).unwrap().quests.get(&fx.quest).map(String::as_str), Some("refused"));
}

/// 剧情纠正检测：死者像在场一样说话 → 复核确认 → 重写；状态不回滚。
#[test]
fn rail_check_rewrites_narration_without_rolling_back_state() {
    let fx = fixture();
    {
        let mut w = fx.world.lock().unwrap();
        campaign::apply_effects(&mut w, &fx.campaign, &[Effect::SetEntityState { entity: fx.elder.clone(), field: "life_status".into(), value: json!("dead") }], "玩家杀死了村长").unwrap();
    }
    let gm = Router::new(vec![json!({"intent": "回酒馆", "narration": "村长老格站在门口招手，让你快去遗迹。"})]);
    *gm.rail.lock().unwrap() = vec![json!({"issues": [{"kind": "revive", "quote": "村长老格站在门口招手", "why": "村长已经死了"}]}).to_string(), json!({"issues": []}).to_string()];
    *gm.narration.lock().unwrap() = vec![json!({"narration": "酒馆里空荡荡的，只有村长常坐的那把椅子。"}).to_string()];
    let r = turn(&fx, &gm, "我回到酒馆");
    assert_eq!(r.narration, "酒馆里空荡荡的，只有村长常坐的那把椅子。");
    assert!(r.quality.iter().any(|q| q.contains("剧情纠正")));
    assert!(gm.prompts("rail")[0].contains("村长老格已经死亡"));
    let w = fx.world.lock().unwrap();
    assert_eq!(CampaignView { conn: w.conn(), campaign_id: &fx.campaign }.life_status(&fx.elder).as_deref(), Some("dead"));
    assert!(!campaign::recent_events(w.conn(), &fx.campaign, &["rail_fixed"], 1).unwrap().is_empty());
}

/// 复核认为没问题（有因果依据）时不重写。
#[test]
fn rail_check_lets_causal_visits_through() {
    let fx = fixture();
    let gm = Router::new(vec![json!({"intent": "等人", "narration": "村长恰好推门进来——你昨晚放火烧了他的仓库，他是来算账的。"})]);
    let r = turn(&fx, &gm, "我坐着喝酒");
    assert!(r.narration.contains("来算账"), "有因果依据的来访保留");
    assert_eq!(gm.prompts("rail").len(), 1, "有疑点（恰好）才复核");
    assert!(gm.prompts("narrate").is_empty());
}

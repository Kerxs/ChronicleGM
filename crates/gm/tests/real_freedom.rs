//! 反剧情纠正与沉浸式互动的真实模型测试（默认忽略）。用示例世界「雾湾」跑方案里的测试 A–F 和互动流程，逐回合记录。
//!   set CHRONICLE_LLM_URL=http://127.0.0.1:8080
//!   set CHRONICLE_REAL_OUT=D:\path\to\freedom.json   （可选：逐回合记录）
//!   cargo test -p chronicle-gm --test real_freedom -- --ignored --nocapture --test-threads=1

use std::sync::Mutex;
use std::time::{Duration, Instant};

use chronicle_core::interaction::{self, PlayerOp};
use chronicle_core::rules::{Effect, StateView};
use chronicle_core::*;
use chronicle_gen::Templates;
use chronicle_gm::{Engine, GmOptions, OPENING, run_turn};
use chronicle_llm::discovery::{probe, test_capabilities};
use chronicle_llm::*;
use serde_json::{Value, json};

fn client() -> (OpenAiClient, GmOptions) {
    let url = std::env::var("CHRONICLE_LLM_URL").expect("请设置 CHRONICLE_LLM_URL");
    let svc = probe(&url, None, Duration::from_secs(3)).expect("无法识别模型服务");
    let cfg = ProviderConfig {
        id: "real".into(),
        name: "real".into(),
        base_url: svc.base_url.clone(),
        model: svc.models.first().map(|m| m.id.clone()).unwrap_or_default(),
        server_kind: Some(svc.kind),
        needs_key: false,
        timeout_secs: 1800,
        capabilities: None,
        allow_campaign_content: true,
        thinking: ThinkingMode::Off,
        service: Default::default(),
        media: Default::default(),
    };
    let c = OpenAiClient::new(cfg, None);
    let caps = test_capabilities(&c, svc.models.first().and_then(|m| m.context_length), 0);
    let opts = GmOptions { json_schema: caps.json_schema, thinking: Some(false), temperature: 0.8, max_tokens: 12000, context_budget: 8000, stream: caps.streaming, web_search: None, dice: None };
    (c, opts)
}

struct Run<'a> {
    world: &'a Mutex<World>,
    campaign: String,
    engine: Engine<'a>,
    rows: Vec<Value>,
    label: String,
}

impl Run<'_> {
    fn turn(&mut self, input: &str) -> Option<chronicle_gm::TurnResult> {
        let t = Instant::now();
        let r = run_turn(self.world, &self.engine, &self.campaign, input, &CancelToken::new());
        let secs = t.elapsed().as_secs_f32();
        match r {
            Ok(r) => {
                println!("\n===== [{}] {:.1}s =====\n玩家：{}\nGM：{}", self.label, secs, if input == OPENING { "（开场）" } else { input }, r.narration);
                for l in &r.npc_lines {
                    println!("NPC {}：{}", l.name, l.text);
                }
                println!("可选：{:?}", r.choices);
                if !r.interactions.is_empty() {
                    println!("互动：{:?}", r.interactions.iter().map(|i| (&i.kind, &i.title)).collect::<Vec<_>>());
                }
                for x in r.quality.iter().chain(r.conflicts.iter()) {
                    println!("提示：{x}");
                }
                for x in &r.rejected {
                    println!("拒绝：{}（{}）", x.effect, x.reason);
                }
                self.rows.push(json!({
                    "label": self.label, "input": input, "secs": secs, "narration": r.narration, "choices": r.choices,
                    "npc": r.npc_lines.iter().map(|l| json!({"name": l.name, "text": l.text})).collect::<Vec<_>>(),
                    "interactions": r.interactions.iter().map(|i| json!({"type": i.kind, "title": i.title})).collect::<Vec<_>>(),
                    "rituals": r.rituals.iter().map(|c| json!({"level": c.level, "scene": c.scene, "title": c.title})).collect::<Vec<_>>(),
                    "quality": r.quality, "conflicts": r.conflicts, "quests": r.quest_updates, "rpg": r.rpg_updates,
                    "rejected": r.rejected.iter().map(|x| json!({"effect": x.effect, "reason": x.reason})).collect::<Vec<_>>(),
                    "off_script": r.off_script, "checks": r.checks.len(),
                }));
                Some(r)
            }
            Err(e) => {
                println!("\n===== [{}] 失败：{e}", self.label);
                self.rows.push(json!({"label": self.label, "input": input, "secs": secs, "error": e.to_string()}));
                None
            }
        }
    }
}

fn save(rows: &[Value]) {
    if let Ok(p) = std::env::var("CHRONICLE_REAL_OUT") {
        let mut all: Vec<Value> = std::fs::read_to_string(&p).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        all.extend(rows.iter().cloned());
        std::fs::write(&p, serde_json::to_string_pretty(&all).unwrap()).unwrap();
    }
}

fn demo() -> (tempfile::TempDir, Mutex<World>, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "试玩").unwrap();
    let sid = demo::populate(&mut w).unwrap();
    let c = campaign::create_campaign(&mut w, Some(&sid), "反剧情试玩", Some(11)).unwrap().id;
    (dir, Mutex::new(w), sid, c)
}

fn node_by(w: &Mutex<World>, name: &str) -> String {
    let w = w.lock().unwrap();
    nodes::find_by_name(w.conn(), name, None).unwrap().into_iter().next().unwrap().id
}

fn item_by(w: &Mutex<World>, sid: &str, name: &str) -> String {
    let w = w.lock().unwrap();
    scripts::all_items(w.conn(), sid).unwrap().into_iter().find(|i| i.title == name).unwrap().id
}

/// 测试 A、B、E：杀死守塔人、拒绝主线、长时间偏离。看 GM 有没有复活、再派任务、拉回主线。
#[test]
#[ignore = "需要真实模型服务"]
fn real_refuse_kill_and_deviate() {
    let (gm, opts) = client();
    let templates = Templates::builtin();
    let (_d, world, _sid, c) = demo();
    let mut run = Run { world: &world, campaign: c.clone(), engine: Engine { gm: &gm, npc: &gm, templates: &templates, opts: &opts }, rows: vec![], label: "B 拒绝主线".into() };
    run.turn(OPENING);
    run.turn("灯塔的事跟我没关系。我不去，我要在镇上找个活干，攒钱买一条自己的船。");
    for input in ["我去码头问问有没有船主要招帮手", "我在码头帮人补渔网，挣点零钱", "晚上我去酒馆吃饭，听听别人聊什么", "我在镇上租一间便宜的屋子住下", "第二天一早，我去船坞问问造一条小船要多少钱"] {
        run.label = "E 长时间偏离".into();
        run.turn(input);
    }
    // 测试 A：守塔人死了（假设玩家之前和他起了冲突）。
    let keeper = node_by(&world, "莫伦");
    {
        let mut w = world.lock().unwrap();
        campaign::apply_effects(&mut w, &c, &[Effect::SetEntityState { entity: keeper.clone(), field: "life_status".into(), value: json!("dead") }], "测试：守塔人在冲突中死亡").unwrap();
    }
    run.label = "A 关键人物死亡".into();
    run.turn("我听说守塔人莫伦死了。我去灯塔那边看看。");
    run.turn("我在灯塔里喊：有人吗？莫伦？");
    {
        let w = world.lock().unwrap();
        let st = campaign::CampaignView { conn: w.conn(), campaign_id: &c }.life_status(&keeper);
        println!("\n莫伦的状态：{st:?}");
        assert_eq!(st.as_deref(), Some("dead"), "死亡状态保留");
    }
    save(&run.rows);
}

/// 测试 C、D、F：摧毁关键物品、非预设的解决办法、连续改变世界。
#[test]
#[ignore = "需要真实模型服务"]
fn real_destroy_and_unscripted_solution() {
    let (gm, opts) = client();
    let templates = Templates::builtin();
    let (_d, world, sid, c) = demo();
    let lamp = item_by(&world, &sid, "备用灯芯石");
    let mut run = Run { world: &world, campaign: c.clone(), engine: Engine { gm: &gm, npc: &gm, templates: &templates, opts: &opts }, rows: vec![], label: "C 摧毁关键物品".into() };
    run.turn(OPENING);
    {
        let mut w = world.lock().unwrap();
        campaign::apply_effects(&mut w, &c, &[Effect::GiveItem { holder: rpg::actor(), item: lamp.clone(), qty: 1 }], "测试：玩家已经拿到备用灯芯石").unwrap();
    }
    run.turn("我拿着备用灯芯石走到码头尽头，把它狠狠扔进深海里。");
    run.turn("我回到灯塔，在灯室里到处翻，看看还有没有别的灯芯石。");
    {
        let w = world.lock().unwrap();
        println!("\n灯芯石被摧毁：{}", events::is_destroyed(w.conn(), &c, &lamp));
    }
    run.label = "D 非预设解决".into();
    run.turn("没有灯芯石也行。我把镇上所有的镜子和油灯收集起来，在岬角上搭一个反光的临时信号灯，给船队指路。");
    run.turn("我点燃油灯，调整镜子的角度，对准海面。");
    run.label = "F 连续改变".into();
    run.turn("信号灯亮了之后，我去码头告诉苏桦，以后不用再靠那座老灯塔了。");
    {
        let w = world.lock().unwrap();
        assert!(timeline::verify(w.conn(), &c).unwrap().ok, "状态与日志一致");
        println!("\n事件：{:?}", events::view(w.conn(), &c).unwrap());
    }
    save(&run.rows);
}

/// 沉浸式互动：GM 提出开锁 / 调查 / 机关，程序结算，下一回合按结果叙述；自由输入绕过选项。
#[test]
#[ignore = "需要真实模型服务"]
fn real_interactions() {
    let (gm, opts) = client();
    let templates = Templates::builtin();
    let (_d, world, _sid, c) = demo();
    let tower = node_by(&world, "老灯塔");
    let mut run = Run { world: &world, campaign: c.clone(), engine: Engine { gm: &gm, npc: &gm, templates: &templates, opts: &opts }, rows: vec![], label: "互动".into() };
    run.turn(OPENING);
    {
        let mut w = world.lock().unwrap();
        campaign::apply_effects(&mut w, &c, &[Effect::SetVar { key: "party.location".into(), value: json!(tower) }], "测试：来到老灯塔").unwrap();
    }
    let mut proposed = 0;
    for input in ["灯室的铁门锁着，我想办法把它打开", "我仔细检查桌上的守塔日志", "我去摆弄那个透镜转盘，试着把光对准海面"] {
        if let Some(r) = run.turn(input) {
            for ix in r.interactions {
                proposed += 1;
                // 程序替玩家操作第一个可用的办法（撬锁时小游戏表现 0.8）。
                let a = ix.available_actions.iter().find(|a| a.available && a.id != "give_up").map(|a| a.id.clone()).unwrap_or_default();
                let res = {
                    let mut w = world.lock().unwrap();
                    interaction::act(&mut w, &c, &ix.interaction_id, &PlayerOp { action: a.clone(), performance: Some(0.8), ..Default::default() })
                };
                match res {
                    Ok(x) => {
                        println!("亲自操作 {a}：{:?}", x.lines);
                        if x.finished {
                            run.turn(&format!("〔亲自操作〕{}：{}", x.interaction.title, x.interaction.result.map(|r| r.degree).unwrap_or_default()));
                        }
                    }
                    Err(e) => println!("亲自操作失败：{e}"),
                }
            }
        }
    }
    run.label = "自由行动绕过选项".into();
    run.turn("我不用钥匙也不撬锁，我把旁边的木桌拆下一条腿，用它把门撞开。");
    println!("\nGM 提出的互动：{proposed} 个");
    save(&run.rows);
}

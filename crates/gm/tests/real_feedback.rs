//! 11-15 试玩反馈的真实模型复测（默认忽略）。
//! 检查：开场推断初始装备、AI 生成角色带装备、检定技能贴合行动、点名问 NPC 他亲自回答且台词短、
//! 问弹药按程序数字回答、射击扣弹药、长时间不推进时 GM 推一把。
//!   set CHRONICLE_LLM_URL=http://127.0.0.1:8080
//!   set CHRONICLE_REAL_OUT=D:\path\to\out.json   （可选：逐回合记录）
//!   cargo test -p chronicle-gm --test real_feedback -- --ignored --nocapture --test-threads=1

use std::sync::Mutex;
use std::time::{Duration, Instant};

use chronicle_core::pc::StartItem;
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
    println!("能力：{caps:?}");
    let opts = GmOptions { json_schema: caps.json_schema, thinking: Some(false), temperature: 0.8, max_tokens: 12000, context_budget: 8000, stream: caps.streaming, web_search: None, dice: None };
    (c, opts)
}

fn bag(w: &Mutex<World>, c: &str) -> Vec<String> {
    let w = w.lock().unwrap();
    rpg::inventory(w.conn(), c, rpg::PARTY)
        .unwrap()
        .into_iter()
        .map(|i| format!("{}×{}{}{}", i.item.title, i.qty, if i.equipped { "（装备中）" } else { "" }, i.ammo_left.map(|n| format!("（弹药剩 {n}）")).unwrap_or_default()))
        .collect()
}

/// 跑一组回合：`None` = 点 GM 给的第一个可选行动。
fn play(label: &str, world: &Mutex<World>, campaign: &str, inputs: &[Option<&str>], gm: &OpenAiClient, opts: &GmOptions, opening: bool) -> Vec<Value> {
    let templates = Templates::builtin();
    let engine = Engine { gm, npc: gm, templates: &templates, opts };
    let cancel = CancelToken::new();
    let mut rows = vec![];
    let mut last_choices: Vec<String> = vec![];
    let mut turns: Vec<String> = if opening { vec![OPENING.to_string()] } else { vec![] };
    turns.extend(inputs.iter().map(|i| i.map(str::to_string).unwrap_or_default()));
    for (n, input) in turns.iter().enumerate() {
        let input = if input.is_empty() { last_choices.first().cloned().unwrap_or_else(|| "我看看四周".into()) } else { input.clone() };
        let t = Instant::now();
        let r = run_turn(world, &engine, campaign, &input, &cancel);
        let secs = t.elapsed().as_secs_f32();
        match r {
            Ok(r) => {
                println!("\n===== [{label}] 第 {n} 回合（{secs:.1}s，请求 {} 次）=====", r.requests.len());
                println!("玩家：{}", if input == OPENING { "（开场）".to_string() } else { input.clone() });
                for ck in &r.checks {
                    println!("检定：{} → {:?}（{}）", ck.skill, ck.degree, ck.detail.as_ref().map(|d| d.explain.clone()).unwrap_or_default());
                }
                for c in &r.conflicts {
                    println!("提示：{c}");
                }
                if !r.rpg_updates.is_empty() {
                    println!("结算：{}", r.rpg_updates.join("；"));
                }
                println!("GM：{}", r.narration);
                for l in &r.npc_lines {
                    println!("NPC {}（{} 字）：{}", l.name, l.text.chars().count(), l.text);
                }
                println!("可选行动：{:?}", r.choices);
                rows.push(json!({
                    "label": label, "turn": n, "input": input, "secs": secs, "requests": r.requests.len(),
                    "narration": r.narration, "choices": r.choices, "intent": r.intent,
                    "checks": r.checks.iter().map(|c| json!({"skill": c.skill, "degree": c.degree, "explain": c.detail.as_ref().map(|d| d.explain.clone())})).collect::<Vec<_>>(),
                    "npc": r.npc_lines.iter().map(|l| json!({"name": l.name, "text": l.text, "chars": l.text.chars().count()})).collect::<Vec<_>>(),
                    "conflicts": r.conflicts, "rpg": r.rpg_updates, "travel": r.travel, "quests": r.quest_updates, "discoveries": r.discoveries,
                    "combat": r.combat.as_ref().map(|c| json!({"status": c.status, "log": c.log})),
                }));
                last_choices = r.choices.clone();
            }
            Err(e) => {
                println!("\n===== [{label}] 第 {n} 回合失败（{secs:.1}s）：{e}");
                rows.push(json!({"label": label, "turn": n, "input": input, "secs": secs, "error": e.to_string()}));
            }
        }
    }
    rows
}

fn save(rows: &[Value]) {
    if let Ok(p) = std::env::var("CHRONICLE_REAL_OUT") {
        let mut all: Vec<Value> = std::fs::read_to_string(&p).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        all.extend(rows.iter().cloned());
        std::fs::write(&p, serde_json::to_string_pretty(&all).unwrap()).unwrap();
    }
}

fn demo_world() -> (tempfile::TempDir, Mutex<World>, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "试玩").unwrap();
    let sid = demo::populate(&mut w).unwrap();
    (dir, Mutex::new(w), sid)
}

/// 示例世界的角色没有写装备：开场推断；点名问话、窃听、劝说；之后一直闲逛看节奏推进。
#[test]
#[ignore = "需要真实模型服务"]
fn real_feedback_lighthouse() {
    let (gm, opts) = client();
    let (_dir, world, sid) = demo_world();
    let c = {
        let mut w = world.lock().unwrap();
        let c = campaign::create_campaign(&mut w, Some(&sid), "灯塔", Some(5)).unwrap();
        let pcs = scripts::items_by_kind(w.conn(), &sid, ScriptItemKind::PlayerCharacter).unwrap();
        pc::set(&mut w, &c.id, &pc::from_script_item(&pcs[0])).unwrap();
        c.id
    };
    println!("开场前背包：{:?}", bag(&world, &c));
    let inputs = [
        Some("我问镇长苏桦：灯塔是从什么时候熄的？"),
        Some("我躲在货箱后面，偷听苏桦和阿澜在说什么"),
        Some("阿澜，你觉得灯塔为什么会熄？"),
        Some("我翻翻自己的口袋，看看身上都带了什么"),
        Some("我在码头上走走，看看海"),
        Some("我继续在码头上发呆，看着雾"),
        Some("我坐在木箱上歇一会儿"),
        Some("我数一数码头上的船"),
        Some("我还是在码头上闲逛"),
        Some("我看看天色"),
        Some("我在码头上踱步"),
        Some("我继续在码头上等"),
    ];
    let rows = play("灯塔·反馈复测", &world, &c, &inputs, &gm, &opts, true);
    println!("\n结束时背包：{:?}", bag(&world, &c));
    save(&rows);
    let ok = rows.iter().filter(|r| r.get("error").is_none()).count();
    println!("[灯塔] 成功 {ok}/{} 回合", rows.len());
}

/// 带猎枪和 3 发铅弹的猎人：问弹药、开战射击直到打空。
#[test]
#[ignore = "需要真实模型服务"]
fn real_feedback_hunter_ammo() {
    let (gm, opts) = client();
    let (_dir, world, sid) = demo_world();
    let c = {
        let mut w = world.lock().unwrap();
        let c = campaign::create_campaign(&mut w, Some(&sid), "猎人", Some(9)).unwrap();
        let hunter = pc::PlayerCharacter {
            name: "老秦".into(),
            identity: "雾灯镇的老猎户".into(),
            personality: "话少、警觉".into(),
            abilities: vec!["猎枪射击".into(), "追踪野兽".into()],
            intro: "在岬角林子里打了三十年猎的老猎户。".into(),
            equipment: vec![
                StartItem { name: "猎枪".into(), qty: 1, slot: "weapon".into(), damage: "1d10".into(), range: 8, ammo: "猎枪铅弹".into(), description: "枪托有修补过的裂痕".into(), ..Default::default() },
                StartItem { name: "猎枪铅弹".into(), qty: 3, ..Default::default() },
                StartItem { name: "猎刀".into(), qty: 1, slot: "".into(), ..Default::default() },
            ],
            ..Default::default()
        };
        pc::set(&mut w, &c.id, &hunter).unwrap();
        c.id
    };
    println!("开局背包：{:?}", bag(&world, &c));
    let inputs = [
        Some("我检查一下猎枪，还剩几发铅弹？"),
        Some("码头边的雾里冲出一头发狂的野狗，我端起猎枪朝它开枪"),
        Some("我再开一枪"),
        Some("继续开枪"),
        Some("再开一枪"),
        Some("我还剩几发子弹？"),
    ];
    let rows = play("猎人·弹药", &world, &c, &inputs, &gm, &opts, false);
    println!("\n结束时背包：{:?}", bag(&world, &c));
    save(&rows);
}

/// AI 生成新角色：看是否写了符合身份的初始装备。
#[test]
#[ignore = "需要真实模型服务"]
fn real_feedback_generated_pc_has_kit() {
    let (gm, opts) = client();
    let (_dir, world, sid) = demo_world();
    let templates = Templates::builtin();
    for hint in ["一个带着猎枪的退伍老兵", "镇上药铺的学徒"] {
        let t = Instant::now();
        match chronicle_gm::pcgen::generate_pc(&world, &gm, &templates, &opts, &sid, hint, &CancelToken::new()) {
            Ok(p) => {
                println!("\n[{hint}]（{:.1}s）{}：{}；能力 {:?}", t.elapsed().as_secs_f32(), p.name, p.identity, p.abilities);
                for e in &p.equipment {
                    println!("  - {} ×{} slot={} damage={} range={} armor={} ammo={} {}", e.name, e.qty, e.slot, e.damage, e.range, e.armor, e.ammo, e.description);
                }
                save(&[json!({"label": "生成角色", "hint": hint, "pc": p})]);
            }
            Err(e) => println!("\n[{hint}] 失败：{e}"),
        }
    }
}

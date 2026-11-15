//! 第一阶段真实模型试玩（默认忽略）：一个非悬疑剧本（示例世界「灯塔守夜」）和一个悬疑剧本，各 10 回合以上。
//! 检查：选角色、开场回合、可选行动、检定等级、物品来源与"玩家没有该物品"、逃跑、探案占比。
//!   set CHRONICLE_LLM_URL=http://127.0.0.1:8080
//!   set CHRONICLE_REAL_OUT=D:\path\to\out.json   （可选：逐回合记录）
//!   cargo test -p chronicle-gm --test real_stage1 -- --ignored --nocapture --test-threads=1

use std::sync::Mutex;
use std::time::{Duration, Instant};

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
    let opts = GmOptions { json_schema: caps.json_schema, thinking: Some(false), temperature: 0.8, max_tokens: 12000, context_budget: 8000, stream: caps.streaming, web_search: None };
    (c, opts)
}

const DETECTIVE_WORDS: [&str; 6] = ["线索", "调查", "推理", "嫌疑", "证据", "真相"];

/// 跑一组回合：`None` 表示"点 GM 给的第一个可选行动"。返回逐回合记录。
fn play(label: &str, world: &Mutex<World>, campaign: &str, inputs: &[Option<&str>], gm: &OpenAiClient, opts: &GmOptions) -> Vec<Value> {
    let templates = Templates::builtin();
    let engine = Engine { gm, npc: gm, templates: &templates, opts };
    let cancel = CancelToken::new();
    let mut rows = vec![];
    let mut last_choices: Vec<String> = vec![];
    let mut turns: Vec<String> = vec![OPENING.to_string()];
    for i in inputs {
        turns.push(i.map(str::to_string).unwrap_or_default());
    }
    for (n, input) in turns.iter().enumerate() {
        let input = if input.is_empty() { last_choices.first().cloned().unwrap_or_else(|| "我看看四周".into()) } else { input.clone() };
        let t = Instant::now();
        let r = run_turn(world, &engine, campaign, &input, &cancel);
        let secs = t.elapsed().as_secs_f32();
        match r {
            Ok(r) => {
                let detective = DETECTIVE_WORDS.iter().filter(|w| r.narration.contains(*w) || r.intent.contains(*w)).count();
                println!("\n===== [{label}] 第 {n} 回合（{secs:.1}s，请求 {} 次）=====", r.requests.len());
                println!("玩家：{}", if input == OPENING { "（开场）".to_string() } else { input.clone() });
                for ck in &r.checks {
                    println!("检定：{} → {:?}（{}）", ck.skill, ck.degree, ck.detail.as_ref().map(|d| d.explain.clone()).unwrap_or_default());
                }
                for e in &r.rejected {
                    println!("拒绝：{} —— {}", e.effect, e.reason);
                }
                if !r.missing_items.is_empty() {
                    println!("程序事实：玩家没有 {:?}", r.missing_items);
                }
                if !r.rpg_updates.is_empty() {
                    println!("结算：{}", r.rpg_updates.join("；"));
                }
                println!("GM：{}", r.narration);
                println!("可选行动：{:?}", r.choices);
                rows.push(json!({
                    "label": label, "turn": n, "input": input, "secs": secs, "requests": r.requests.len(),
                    "narration": r.narration, "choices": r.choices, "intent": r.intent,
                    "checks": r.checks.iter().map(|c| json!({"skill": c.skill, "degree": c.degree, "explain": c.detail.as_ref().map(|d| d.explain.clone())})).collect::<Vec<_>>(),
                    "rejected": r.rejected.iter().map(|e| json!({"effect": e.effect, "reason": e.reason})).collect::<Vec<_>>(),
                    "missing_items": r.missing_items, "rpg": r.rpg_updates, "discoveries": r.discoveries, "quests": r.quest_updates,
                    "detective_words": detective, "leaks": r.leaks.len(), "combat": r.combat.as_ref().map(|c| c.status.clone()),
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

#[test]
#[ignore = "需要真实模型服务"]
fn real_non_mystery_lighthouse() {
    let (gm, opts) = client();
    let dir = tempfile::tempdir().unwrap();
    let mut w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "试玩").unwrap();
    let sid = demo::populate(&mut w).unwrap();
    let c = campaign::create_campaign(&mut w, Some(&sid), "灯塔", Some(11)).unwrap();
    let pcs = scripts::items_by_kind(w.conn(), &sid, ScriptItemKind::PlayerCharacter).unwrap();
    pc::set(&mut w, &c.id, &pc::from_script_item(&pcs[0])).unwrap();
    // 一件世界树里的武器，玩家没有：用来检查"玩家没有该物品"。
    w.write(&Author::User, "n", |x| x.create_node(NewNode::new(NodeKind::Item, "霜纹长剑"))).unwrap();
    let world = Mutex::new(w);
    let inputs = [
        None,
        Some("我问镇长苏桦：灯塔是从什么时候熄的？船队还有多久回来？"),
        None,
        Some("我拔出霜纹长剑，砍断挡路的缆绳"),
        Some("我爬上灯室，看看碎掉的灯芯石和灯座"),
        Some("我劝莫伦：船队里有你看着长大的孩子，告诉我备用的灯芯石在哪"),
        None,
        Some("我举着火把走进潮洞，往有蓝光的地方去"),
        Some("我冲向挡路的海妖，用船匠的锤子砸过去"),
        Some("我转身逃跑，往洞口跑"),
        None,
    ];
    let rows = play("非悬疑·灯塔守夜", &world, &c.id, &inputs, &gm, &opts);
    save(&rows);
    let ok = rows.iter().filter(|r| r.get("error").is_none()).count();
    println!("\n[非悬疑] 成功 {ok}/{} 回合", rows.len());
}

#[test]
#[ignore = "需要真实模型服务"]
fn real_mystery_clocktower() {
    let (gm, opts) = client();
    let dir = tempfile::tempdir().unwrap();
    let mut w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "试玩").unwrap();
    let u = Author::User;
    let town = w.write(&u, "n", |x| x.create_node(NewNode::new(NodeKind::City, "白杨镇"))).unwrap().0.id;
    let mk = |w: &mut World, t: &str, p: Value| {
        let mut n = NewNode::new(NodeKind::Character, t).payload(p);
        n.parent_id = Some(town.clone());
        w.write(&u, "n", |x| x.create_node(n)).unwrap().0.id
    };
    let keeper = mk(&mut w, "钟表匠老魏", json!({"identity": "钟楼的钟表匠", "personality": "沉默、谨慎", "secrets": ["案发当晚他在钟楼里修钟，看见了凶手的影子却不敢说"], "knowledge": ["钟楼的钟停在了十一点十分"]}));
    let widow = mk(&mut w, "寡妇岑嫂", json!({"identity": "死者的妻子", "personality": "强硬、悲伤", "secrets": ["她知道丈夫欠了赌债"]}));
    let clerk = mk(&mut w, "账房周默", json!({"identity": "商会的账房", "personality": "圆滑、紧张", "secrets": ["他就是凶手：死者发现他做假账"]}));
    let mut ns = NewScript::new("钟楼命案");
    ns.genre = "悬疑推理".into();
    ns.tone = "阴郁、克制".into();
    ns.premise = "商会会长死在钟楼下，钟停在十一点十分。".into();
    ns.core_setting = "真凶是账房周默：会长发现了他做的假账。".into();
    ns.player_count = 1;
    let s = w.write(&u, "s", |x| x.create_script(ns)).unwrap().0;
    let item = |w: &mut World, k: ScriptItemKind, t: &str, pt: &str, p: Value| {
        let mut it = NewScriptItem::new(&s.id, k, t);
        it.player_text = pt.into();
        it.payload = p;
        w.write(&u, "i", |x| x.create_script_item(it)).unwrap().0.id
    };
    item(&mut w, ScriptItemKind::Opening, "开场", "清晨，白杨镇的钟楼下围满了人。商会会长倒在石阶上，钟楼的大钟停在十一点十分。镇长请你查清这件事。", json!({}));
    item(&mut w, ScriptItemKind::Quest, "查明真凶", "找出杀害会长的人。", json!({"quest_type": "main"}));
    item(&mut w, ScriptItemKind::Clue, "停摆的钟", "钟被人为卡住了齿轮。", json!({}));
    item(&mut w, ScriptItemKind::Clue, "假账本", "商会账本里有几页墨迹不同。", json!({}));
    item(&mut w, ScriptItemKind::Clue, "赌债欠条", "死者欠下的赌债。", json!({}));
    for (nid, role, secret) in [(&keeper, "目击者", "看见了凶手的影子"), (&widow, "家属", "知道赌债"), (&clerk, "凶手", "做假账被发现")] {
        let c = NewCastMember { script_id: s.id.clone(), node_id: nid.clone(), role: role.into(), motive: String::new(), secret: secret.into(), relation_to_players: String::new(), payload: json!({}) };
        w.write(&u, "c", |x| x.add_cast(c)).unwrap();
    }
    let start = w.write(&u, "p", |x| x.create_plot_node(NewPlotNode::new(&s.id, PlotNodeKind::Start, "钟楼下"))).unwrap().0.id;
    let _ = start;
    let c = campaign::create_campaign(&mut w, Some(&s.id), "钟楼", Some(5)).unwrap();
    pc::set(&mut w, &c.id, &pc::PlayerCharacter { name: "温砚".into(), identity: "退休的捕快".into(), personality: "冷静、多疑".into(), abilities: vec!["察觉".into(), "审讯".into()], ..Default::default() }).unwrap();
    let world = Mutex::new(w);
    let inputs = [
        Some("我蹲下查看会长的尸体和周围的石阶"),
        Some("我上钟楼看看那口停住的大钟"),
        None,
        Some("我问钟表匠老魏：昨晚十一点你在哪？"),
        Some("我去会长家，安慰岑嫂，顺便问问会长最近有什么麻烦"),
        None,
        Some("我去商会翻看账本"),
        Some("我把账本拍在周默面前，问他这几页是怎么回事"),
        None,
        Some("我把停摆的钟、假账和老魏看到的影子连起来，指认周默是凶手"),
    ];
    let rows = play("悬疑·钟楼命案", &world, &c.id, &inputs, &gm, &opts);
    save(&rows);
    let ok = rows.iter().filter(|r| r.get("error").is_none()).count();
    println!("\n[悬疑] 成功 {ok}/{} 回合", rows.len());
}

/// 运行监控在真实服务上能拿到哪些数据（流式 / 非流式、服务端分词）。
#[test]
#[ignore = "需要真实模型服务"]
fn real_monitor_metrics() {
    let url = std::env::var("CHRONICLE_LLM_URL").expect("请设置 CHRONICLE_LLM_URL");
    let (gm, _) = client();
    for stream in [false, true] {
        let req = ChatRequest { messages: vec![ChatMessage::user("用一句话介绍灯塔。")], temperature: Some(0.5), max_tokens: Some(80), format: OutputFormat::Text, stream, thinking: Some(false) };
        let r = gm.chat(&req, &CancelToken::new(), &mut |_| {}).unwrap();
        println!("stream={stream} 耗时 {}ms usage={:?}", r.latency_ms, r.usage);
    }
    println!("tokenize: {:?}", chronicle_llm::discovery::tokenize_count(&url, "雾灯镇的老灯塔三十年来从没熄过。", Duration::from_secs(5)));
}

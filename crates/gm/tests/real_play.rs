//! 真实模型试玩（默认忽略）。
//!   set CHRONICLE_LLM_URL=http://127.0.0.1:8080
//!   set CHRONICLE_THINKING=off   （可选：on / off，默认 off）
//!   cargo test -p chronicle-gm --test real_play -- --ignored --nocapture

use std::sync::Mutex;
use std::time::{Duration, Instant};

use chronicle_core::campaign;
use chronicle_core::*;
use chronicle_gen::Templates;
use chronicle_gm::{Engine, GmOptions, run_turn, summarize};
use chronicle_llm::discovery::{probe, test_capabilities};
use chronicle_llm::*;
use serde_json::{Value, json};

#[test]
#[ignore = "需要真实模型服务：设置 CHRONICLE_LLM_URL 后用 --ignored 运行"]
fn real_play_session() {
    let url = std::env::var("CHRONICLE_LLM_URL").expect("请设置 CHRONICLE_LLM_URL");
    let thinking_on = std::env::var("CHRONICLE_THINKING").map(|v| v == "on").unwrap_or(false);
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
        thinking: if thinking_on { ThinkingMode::Auto } else { ThinkingMode::Off },
        service: Default::default(),
        media: Default::default(),
    };
    let client = OpenAiClient::new(cfg, None);
    let caps = test_capabilities(&client, svc.models.first().and_then(|m| m.context_length), 0);
    println!("能力：{caps:?}；思考：{}", if thinking_on { "开启" } else { "关闭" });
    let opts = GmOptions {
        json_schema: caps.json_schema,
        thinking: (!thinking_on).then_some(false),
        temperature: 0.8,
        max_tokens: 12000,
        context_budget: 8000,
    };

    // 小剧本：村长委托调查遗迹。
    let dir = tempfile::tempdir().unwrap();
    let mut w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "联调").unwrap();
    let u = Author::User;
    let mk = |w: &mut World, n: NewNode| w.write(&u, "n", |x| x.create_node(n)).unwrap().0;
    let mut law = NewNode::new(NodeKind::MagicSystem, "潮汐法则").canon(CanonLevel::Core);
    law.summary = "新月之夜所有魔法都会失效".into();
    let law = mk(&mut w, law);
    let mut elder = NewNode::new(NodeKind::Character, "村长老格").payload(json!({
        "identity": "灰石村的村长，六十多岁", "personality": "谨慎、爱面子、说话绕弯子",
        "motivation": "让冒险者去加固遗迹的封印", "secrets": ["二十年前参与过上一次封印仪式，亲手把弟弟留在了遗迹里"],
        "knowledge": ["遗迹在村北的山谷里", "最近夜里能听到山谷传来低语", "上一个进遗迹的猎人没有回来"]
    }));
    elder.summary = "灰石村村长".into();
    let elder = mk(&mut w, elder);
    let mut smith = NewNode::new(NodeKind::Character, "铁匠阿岩").payload(json!({
        "identity": "村里唯一的铁匠", "personality": "粗声粗气但心软",
        "secrets": ["半年前偷偷从遗迹祭坛拿走了一把古铜钥匙，藏在铁砧下面"], "knowledge": ["会修补武器", "知道去遗迹的近路"]
    }));
    smith.summary = "村里的铁匠".into();
    let smith = mk(&mut w, smith);
    let mut ns = NewScript::new("遗迹委托");
    ns.premise = "村长请冒险者调查村北的古代遗迹".into();
    ns.core_setting = "遗迹祭坛封印着沉睡的古神，封印正在减弱；古铜钥匙是加固封印的关键".into();
    let s = w.write(&u, "s", |x| x.create_script(ns)).unwrap().0;
    let item = |w: &mut World, k: ScriptItemKind, t: &str, p: Value, pt: &str| {
        let mut it = NewScriptItem::new(&s.id, k, t);
        it.payload = p;
        it.player_text = pt.into();
        w.write(&u, "i", |x| x.create_script_item(it)).unwrap().0.id
    };
    let quest = item(&mut w, ScriptItemKind::Quest, "调查遗迹", json!({"quest_type":"main"}), "查明山谷里的低语从何而来");
    let clue = item(&mut w, ScriptItemKind::Clue, "石板上的符文", json!({}), "入口石板上刻着古老的符文");
    item(&mut w, ScriptItemKind::Item, "古铜钥匙", json!({}), "一把刻着潮汐纹的古铜钥匙");
    for (n, role, secret) in [(&elder, "委托人", "隐瞒了弟弟的事"), (&smith, "村民", "")] {
        let c = NewCastMember { script_id: s.id.clone(), node_id: n.id.clone(), role: role.into(), motive: String::new(), secret: secret.into(), relation_to_players: String::new(), payload: json!({}) };
        w.write(&u, "c", |x| x.add_cast(c)).unwrap();
    }
    let pn = |w: &mut World, k, t: &str, p: &str, g: &str| {
        let mut n = NewPlotNode::new(&s.id, k, t);
        n.player_text = p.into();
        n.gm_text = g.into();
        w.write(&u, "p", |x| x.create_plot_node(n)).unwrap().0.id
    };
    let tavern = pn(&mut w, PlotNodeKind::Start, "灰石村酒馆", "傍晚的酒馆里人不多，村长老格坐在角落，铁匠阿岩在吧台喝酒。", "村长会请求帮助，但不会主动提起弟弟");
    let ruins = pn(&mut w, PlotNodeKind::Scene, "遗迹入口", "山谷尽头，一座半埋在土里的石门，门边立着一块刻满符文的石板。", "符文是加固封印的咒语，需要察觉或奥秘检定才能看懂");
    let altar = pn(&mut w, PlotNodeKind::Ending, "封印加固", "低语渐渐平息。", "");
    let refuse = pn(&mut w, PlotNodeKind::Ending, "离开灰石村", "你们离开了村子。", "");
    let edge = |w: &mut World, a: &str, b: &str, label: &str, cond: Value, eff: Value| {
        let mut e = NewPlotEdge::new(&s.id, a, b);
        e.label = label.into();
        e.condition = serde_json::from_value(cond).unwrap();
        e.effects = serde_json::from_value(eff).unwrap();
        w.write(&u, "e", |x| x.create_plot_edge(e)).unwrap();
    };
    edge(&mut w, &tavern, &ruins, "接受委托，前往遗迹", Value::Null, json!([{"type":"set_quest_state","quest": quest,"state":"active"}]));
    edge(&mut w, &tavern, &refuse, "拒绝委托离开", Value::Null, json!([]));
    edge(&mut w, &ruins, &altar, "按符文的指示加固封印", json!({"type":"clue_known","clue": clue}), json!([{"type":"set_quest_state","quest": quest,"state":"done"}]));
    let c = campaign::create_campaign(&mut w, Some(&s.id), "联调团", Some(2026)).unwrap();
    let world = Mutex::new(w);
    let templates = Templates::builtin();
    let cancel = CancelToken::new();

    let inputs = [
        "我走到角落，在村长对面坐下，问他找冒险者是为了什么事。",
        "我答应村长的委托，问清楚去遗迹的路，然后出发。",
        "到了遗迹，我仔细观察门边那块石板上的符文。",
        "我想起出发前铁匠神色古怪，回想他是不是从遗迹里拿过什么东西——如果他在这里，我就直接问他。",
    ];
    for (i, input) in inputs.iter().enumerate() {
        let t = Instant::now();
        match run_turn(&world, &Engine { gm: &client, npc: &client, templates: &templates, opts: &opts }, &c.id, input, &cancel) {
            Ok(r) => {
                println!("\n===== 第 {} 回合（{:.1}s，模型请求 {} 次）=====", i + 1, t.elapsed().as_secs_f32(), r.requests.len());
                println!("玩家：{input}");
                println!("意图：{}{}", r.intent, if r.off_script { "（剧本外）" } else { "" });
                for ck in &r.checks {
                    println!("检定：{} {}+{}={} / DC {} → {}", ck.skill, ck.roll, ck.modifier, ck.total, ck.dc, if ck.success { "成功" } else { "失败" });
                }
                for e in &r.applied {
                    println!("执行：{}", serde_json::to_string(e).unwrap());
                }
                for e in &r.rejected {
                    println!("拒绝：{} —— {}", e.effect, e.reason);
                }
                if let Some(tr) = &r.transition {
                    println!("剧情推进：{}", if tr.taken { "成功" } else { "条件不满足" });
                }
                if !r.transition_note.is_empty() {
                    println!("剧情：{}", r.transition_note);
                }
                println!("GM：{}", r.narration);
                if !r.leaks.is_empty() {
                    println!("⚠ 叙述泄露检查：{:?}", r.leaks);
                }
                if !r.redirected_speakers.is_empty() {
                    println!("⤷ GM 替 {:?} 说了话，已删掉并改由本人回答", r.redirected_speakers);
                }
                let quote_chars: Vec<String> = r.narration.chars().filter(|c| "\"“”「」＂'‘’".contains(*c)).map(|c| format!("U+{:04X}", c as u32)).collect();
                if !quote_chars.is_empty() {
                    println!("（叙述中剩余的引号字符：{quote_chars:?}）");
                }
                for n in &r.npc_lines {
                    println!("{}（态度 {:+}）：{}{}", n.name, n.attitude_change, n.text, if n.leaks.is_empty() { String::new() } else { format!("  ⚠ 泄露：{:?}", n.leaks) });
                }
                for e in &r.entities {
                    println!("新内容「{}」：{}", e.name, e.note);
                }
                for f in &r.remembered {
                    println!("记住：{f}");
                }
            }
            Err(e) => println!("\n===== 第 {} 回合失败（{:.1}s）：{e}", i + 1, t.elapsed().as_secs_f32()),
        }
    }
    let t = Instant::now();
    match summarize(&world, &client, &templates, &opts, &c.id, &cancel) {
        Ok(s) => println!("\n===== 会话摘要草稿（{:.1}s）=====\n{}", t.elapsed().as_secs_f32(), s.summary),
        Err(e) => println!("\n摘要失败：{e}"),
    }
    let w = world.lock().unwrap();
    let st = campaign::campaign_state(w.conn(), &c.id).unwrap();
    println!("\n最终状态：回合 {}，节点 {:?}", campaign::get_campaign(w.conn(), &c.id).unwrap().turn, campaign::current_node(&w, &c.id).unwrap().map(|n| n.title));
    println!("任务 {:?}\n线索 {:?}\n物品 {:?}\n变量 {:?}", st.quests, st.clues, st.items, st.vars);
    println!("事件 {} 条；核心设定版本 {}（应为 1）", campaign::events(w.conn(), &c.id, 0, 10000).unwrap().len(), nodes::get_node(w.conn(), &law.id).unwrap().unwrap().rev);
    assert_eq!(nodes::get_node(w.conn(), &law.id).unwrap().unwrap().rev, 1, "核心设定不能被修改");
}

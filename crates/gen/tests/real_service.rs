//! 真实模型服务联调（默认忽略）。
//!
//! 运行方式（先自行启动 llama-server 等服务）：
//!   set CHRONICLE_LLM_URL=http://127.0.0.1:8080
//!   cargo test -p chronicle-gen --test real_service -- --ignored --nocapture
//!
//! 结果打印到控制台，供如实记录；不设置环境变量时测试显示为 ignored，不算通过。

use std::time::{Duration, Instant};

use chronicle_core::changeset;
use chronicle_core::jobs::{self, JobStatus};
use chronicle_core::*;
use chronicle_gen::runner::{self, GenOptions};

/// 设置 CHRONICLE_THINKING=off 时关闭思考（与设置页的"思考模式：关闭"相同）。
fn gen_opts(caps: &chronicle_llm::Capabilities) -> GenOptions {
    let mut o = GenOptions::for_caps(Some(caps));
    if std::env::var("CHRONICLE_THINKING").is_ok_and(|v| v == "off") {
        o.thinking = Some(false);
    }
    o
}
use chronicle_gen::{Task, Templates};
use chronicle_llm::discovery::{probe, test_capabilities};
use chronicle_llm::*;

fn real_client() -> (OpenAiClient, Capabilities) {
    let url = std::env::var("CHRONICLE_LLM_URL").expect("请设置 CHRONICLE_LLM_URL");
    let svc = probe(&url, None, Duration::from_secs(3)).expect("无法识别该地址上的模型服务");
    assert!(svc.usable_for_chat);
    let cfg = ProviderConfig {
        id: "real".into(),
        name: "real".into(),
        base_url: svc.base_url.clone(),
        model: svc.models.first().map(|m| m.id.clone()).unwrap_or_else(|| "default".into()),
        server_kind: Some(svc.kind),
        needs_key: false,
        timeout_secs: 1800,
        capabilities: None,
        allow_campaign_content: true,
        thinking: Default::default(),
        service: Default::default(),
        media: Default::default(),
    };
    let client = OpenAiClient::new(cfg, None);
    let caps = test_capabilities(&client, svc.models.first().and_then(|m| m.context_length), 0);
    println!("能力：{caps:?}");
    assert!(caps.chat);
    (client, caps)
}

fn report(w: &World, out: &runner::RunOutcome, label: &str, secs: f32) -> chronicle_core::changeset::Changeset {
    let job = jobs::get_job(w.conn(), &out.job_id).unwrap();
    println!("{label}：{:?}，用时 {secs:.1}s，请求 {:?}", job.status, out.requests);
    match &out.result {
        Ok(cs) => {
            println!("  变更集：{} 项，校验 ok = {}", cs.ops.len(), cs.validation.ok);
            for i in &cs.validation.issues {
                println!("  [{:?}] #{} {} {}", i.severity, i.op, i.code, i.message);
            }
            cs.clone()
        }
        Err(e) => {
            println!("  错误：{e}\n  原始输出（前 2000 字）：{}", job.raw_output.unwrap_or_default().chars().take(2000).collect::<String>());
            panic!("{label} 失败");
        }
    }
}

#[test]
#[ignore = "需要真实模型服务：设置 CHRONICLE_LLM_URL 后用 --ignored 运行"]
fn real_world_tree() {
    use chronicle_gen::world_tasks::RewriteMode;
    let (client, caps) = real_client();
    let dir = tempfile::tempdir().unwrap();
    let mut w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "联调世界").unwrap();
    let opts = gen_opts(&caps);
    let t = Templates::builtin();
    let cancel = CancelToken::new();

    let t0 = Instant::now();
    let task = Task::WorldSeed {
        idea: "一个被月潮支配的群岛世界，魔法强弱随月相涨落，新月之夜魔法完全失效".into(),
        genre: "奇幻".into(),
        era: "大航海时代".into(),
        style: "神秘".into(),
        rules: String::new(),
    };
    let out = runner::run(&mut w, &client, task, &t, Some(&caps), &opts, &cancel).unwrap();
    let cs = report(&w, &out, "一句话生成世界", t0.elapsed().as_secs_f32());
    changeset::apply(&mut w, &cs.id, None, &Author::User).expect("写入世界失败");
    let root = &nodes::children(w.conn(), None, 0, 10).unwrap()[0];
    println!("世界《{}》：", root.title);
    for c in nodes::children(w.conn(), Some(&root.id), 0, 50).unwrap() {
        let kids: Vec<String> = nodes::children(w.conn(), Some(&c.id), 0, 50).unwrap().into_iter().map(|k| format!("{}[{}]", k.title, k.kind)).collect();
        println!("  - {}：{}", c.title, kids.join("、"));
    }
    let all: i64 = w.conn().query_row("SELECT COUNT(*) FROM nodes WHERE deleted_at IS NULL", [], |r| r.get(0)).unwrap();
    let rels: i64 = w.conn().query_row("SELECT COUNT(*) FROM relations WHERE deleted_at IS NULL", [], |r| r.get(0)).unwrap();
    println!("节点 {all}，关系 {rels}");

    // 找一个城市或地区继续展开。
    let target = ["city", "region", "nation", "site"]
        .iter()
        .find_map(|k| nodes::nodes_by_kind(w.conn(), NodeKind::parse(k).unwrap(), 0, 1).unwrap().into_iter().next())
        .expect("世界中没有地理节点");
    let t1 = Instant::now();
    let task = Task::ExpandNode { node_id: target.id.clone(), count: 4, kinds: vec![], guidance: String::new() };
    let out = runner::run(&mut w, &client, task, &t, Some(&caps), &opts, &cancel).unwrap();
    let cs = report(&w, &out, &format!("展开「{}」", target.title), t1.elapsed().as_secs_f32());
    changeset::apply(&mut w, &cs.id, None, &Author::User).expect("写入失败");
    for k in nodes::children(w.conn(), Some(&target.id), 0, 50).unwrap() {
        println!("  - {}[{}]：{}", k.title, k.kind, k.summary.chars().take(60).collect::<String>());
    }

    // 补全一个人物（如有）。
    if let Some(ch) = nodes::nodes_by_kind(w.conn(), NodeKind::Character, 0, 1).unwrap().into_iter().next() {
        let t2 = Instant::now();
        let task = Task::RewriteNode { node_id: ch.id.clone(), mode: RewriteMode::Complete, instruction: String::new() };
        let out = runner::run(&mut w, &client, task, &t, Some(&caps), &opts, &cancel).unwrap();
        let cs = report(&w, &out, &format!("补全「{}」", ch.title), t2.elapsed().as_secs_f32());
        changeset::apply(&mut w, &cs.id, None, &Author::User).expect("写入失败");
        let after = nodes::get_node(w.conn(), &ch.id).unwrap().unwrap();
        println!("  补全前摘要：{}\n  补全后摘要：{}\n  资料：{}", ch.summary, after.summary, after.payload);
        assert_eq!(after.summary, if ch.summary.is_empty() { after.summary.clone() } else { ch.summary.clone() }, "补全不能覆盖已有摘要");
    }
    let findings = chronicle_core::consistency::check_world(w.conn()).unwrap();
    println!("一致性检查：{} 条", findings.len());
    for f in &findings {
        println!("  {} {}", f.code, f.message);
    }
}

#[test]
#[ignore = "需要真实模型服务：设置 CHRONICLE_LLM_URL 后用 --ignored 运行"]
fn real_outline_and_chapter() {
    let url = std::env::var("CHRONICLE_LLM_URL").expect("请设置 CHRONICLE_LLM_URL");
    let svc = probe(&url, None, Duration::from_secs(3)).expect("无法识别该地址上的模型服务");
    println!("识别结果：{:?} {} 模型 {:?}", svc.kind, svc.base_url, svc.models);
    assert!(svc.usable_for_chat, "这是语音服务，不能用于文本生成");
    let model = svc.models.first().map(|m| m.id.clone()).unwrap_or_else(|| "default".into());
    let ctx = svc.models.first().and_then(|m| m.context_length);
    let cfg = ProviderConfig {
        id: "real".into(),
        name: "real".into(),
        base_url: svc.base_url.clone(),
        model,
        server_kind: Some(svc.kind),
        needs_key: false,
        timeout_secs: 1800,
        capabilities: None,
        allow_campaign_content: true,
        thinking: Default::default(),
        service: Default::default(),
        media: Default::default(),
    };
    let client = OpenAiClient::new(cfg, None);

    let t0 = Instant::now();
    let caps = test_capabilities(&client, ctx, 0);
    println!("能力实测（{:.1}s）：{caps:?}", t0.elapsed().as_secs_f32());
    assert!(caps.chat, "对话测试未通过");

    let dir = tempfile::tempdir().unwrap();
    let mut w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "联调世界").unwrap();
    let mut law = NewNode::new(NodeKind::MagicSystem, "潮汐法则").canon(CanonLevel::Core);
    law.summary = "所有魔法的强弱随月相变化，新月时魔法完全失效".into();
    w.write(&Author::User, "seed", |x| x.create_node(law)).unwrap();
    w.write(&Author::User, "seed", |x| x.create_node(NewNode::new(NodeKind::City, "雾港"))).unwrap();

    let opts = gen_opts(&caps);
    println!("生成参数：{opts:?}");
    let templates = Templates::builtin();
    let task = Task::Outline {
        idea: "雾港的货船在新月之夜集体消失，只留下一盏还亮着的船灯".into(),
        title: None,
        genre: "奇幻悬疑".into(),
        tone: "阴郁".into(),
        player_count: 4,
        duration_hours: 3.0,
        rules: String::new(),
        use_world: true,
    };
    let t1 = Instant::now();
    let out = runner::run(&mut w, &client, task, &templates, Some(&caps), &opts, &CancelToken::new()).unwrap();
    let job = jobs::get_job(w.conn(), &out.job_id).unwrap();
    println!("大纲任务：{:?}，用时 {:.1}s，请求 {:?}", job.status, t1.elapsed().as_secs_f32(), out.requests);
    if job.status != JobStatus::Succeeded {
        println!("错误：{:?}\n原始输出（前 2000 字）：{}", job.error, job.raw_output.unwrap_or_default().chars().take(2000).collect::<String>());
        panic!("大纲生成失败");
    }
    let cs = out.result.unwrap();
    println!("变更集：{} 项，校验 ok = {}", cs.ops.len(), cs.validation.ok);
    for i in &cs.validation.issues {
        println!("  [{:?}] #{} {} {}", i.severity, i.op, i.code, i.message);
    }
    changeset::apply(&mut w, &cs.id, None, &Author::User).expect("应用大纲失败");
    let s = &scripts::list_scripts(w.conn()).unwrap()[0];
    let chapters = scripts::items_by_kind(w.conn(), &s.id, ScriptItemKind::Chapter).unwrap();
    println!("剧本《{}》：章节 {}，任务 {}，结局 {}，人物 {}", s.title, chapters.len(),
        scripts::items_by_kind(w.conn(), &s.id, ScriptItemKind::Quest).unwrap().len(),
        scripts::items_by_kind(w.conn(), &s.id, ScriptItemKind::Ending).unwrap().len(),
        scripts::cast_of(w.conn(), &s.id).unwrap().len());
    for c in &chapters {
        println!("  - {}：{}", c.title, c.player_text.chars().take(80).collect::<String>());
    }

    let t2 = Instant::now();
    let task = Task::ExpandChapter { script_id: s.id.clone(), chapter_id: chapters[0].id.clone(), guidance: String::new() };
    let out = runner::run(&mut w, &client, task, &templates, Some(&caps), &opts, &CancelToken::new()).unwrap();
    let job = jobs::get_job(w.conn(), &out.job_id).unwrap();
    println!("展开第一章：{:?}，用时 {:.1}s", job.status, t2.elapsed().as_secs_f32());
    match out.result {
        Ok(cs) => {
            println!("变更集：{} 项，校验 ok = {}", cs.ops.len(), cs.validation.ok);
            for i in &cs.validation.issues {
                println!("  [{:?}] #{} {} {}", i.severity, i.op, i.code, i.message);
            }
        }
        Err(e) => {
            println!("错误：{e}\n原始输出（前 2000 字）：{}", job.raw_output.unwrap_or_default().chars().take(2000).collect::<String>());
            panic!("展开章节失败");
        }
    }
}

#[test]
#[ignore = "需要真实模型服务：设置 CHRONICLE_LLM_URL 后用 --ignored 运行"]
fn real_plot_graph() {
    use chronicle_core::campaign;
    use chronicle_core::plot_analysis::analyze_plot;
    use serde_json::json;
    let (client, caps) = real_client();
    let dir = tempfile::tempdir().unwrap();
    let mut w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "联调").unwrap();
    // 手工搭一个小剧本（不依赖前面的生成步骤，节省时间）。
    let elder = w.write(&Author::User, "n", |x| x.create_node(NewNode::new(NodeKind::Character, "村长老格"))).unwrap().0;
    let mut ns = NewScript::new("遗迹委托");
    ns.premise = "村长请冒险者调查村外的古代遗迹，遗迹里封印着一个可怕的秘密".into();
    ns.core_setting = "遗迹的祭坛封印着古神的低语，村长其实知道真相，想借冒险者之手重新加固封印".into();
    ns.duration_hours = 3.0;
    let s = w.write(&Author::User, "s", |x| x.create_script(ns)).unwrap().0;
    let item = |w: &mut World, k: ScriptItemKind, t: &str, p: serde_json::Value, g: &str| {
        let mut it = NewScriptItem::new(&s.id, k, t);
        it.payload = p;
        it.gm_text = g.into();
        w.write(&Author::User, "i", |x| x.create_script_item(it)).unwrap().0
    };
    item(&mut w, ScriptItemKind::Opening, "村口", json!({}), "村长在酒馆等待冒险者");
    item(&mut w, ScriptItemKind::Quest, "调查遗迹", json!({"quest_type":"main"}), "");
    item(&mut w, ScriptItemKind::Quest, "村长的秘密", json!({"quest_type":"hidden"}), "村长曾参与上一次封印");
    item(&mut w, ScriptItemKind::Clue, "石板上的符文", json!({}), "符文是加固封印的咒语");
    item(&mut w, ScriptItemKind::Item, "古铜钥匙", json!({}), "打开祭坛密室");
    item(&mut w, ScriptItemKind::Ending, "封印加固", json!({}), "达成条件：读懂符文并使用钥匙");
    item(&mut w, ScriptItemKind::Ending, "古神苏醒", json!({}), "达成条件：在没有符文的情况下打开密室");
    item(&mut w, ScriptItemKind::Ending, "拒绝委托", json!({}), "达成条件：一开始就拒绝");
    w.write(&Author::User, "c", |x| {
        x.add_cast(NewCastMember { script_id: s.id.clone(), node_id: elder.id.clone(), role: "委托人".into(), motive: "加固封印".into(),
            secret: "参与过上一次封印".into(), relation_to_players: "雇主".into(), payload: json!({}) })
    }).unwrap();

    let opts = gen_opts(&caps);
    let t = Templates::builtin();
    let cancel = CancelToken::new();
    let t0 = Instant::now();
    let out = runner::run(&mut w, &client, Task::PlotFromScript { script_id: s.id.clone(), guidance: String::new() }, &t, Some(&caps), &opts, &cancel).unwrap();
    let cs = report(&w, &out, "根据剧本生成剧情图", t0.elapsed().as_secs_f32());
    changeset::apply(&mut w, &cs.id, None, &Author::User).expect("写入剧情图失败");
    let nodes = plot::plot_nodes_of(w.conn(), &s.id).unwrap();
    let edges = plot::edges_of_script(w.conn(), &s.id).unwrap();
    println!("剧情图：{} 个节点，{} 条分支", nodes.len(), edges.len());
    let title = |id: &str| nodes.iter().find(|n| n.id == id).map(|n| format!("[{}]{}", n.kind, n.title)).unwrap_or_default();
    for e in &edges {
        println!("  {} → {}：「{}」 条件={} 效果={}", title(&e.from_id), title(&e.to_id), e.label,
            e.condition.as_ref().map(|c| serde_json::to_string(c).unwrap()).unwrap_or_else(|| "无".into()), serde_json::to_string(&e.effects).unwrap());
    }
    let rep = analyze_plot(w.conn(), &s.id).unwrap();
    println!("检查：结局 {}，可到达 {}；问题 {} 条", rep.ending_count, rep.reachable_endings, rep.issues.len());
    for i in &rep.issues {
        println!("  [{:?}] {} {}", i.severity, i.code, i.message);
    }

    // 自动试玩两遍：每步走第一条可走（或需检定）的分支，记录路线。
    for (round, prefer_last) in [(1, false), (2, true)] {
        let c = campaign::create_campaign(&mut w, Some(&s.id), &format!("自动试玩{round}"), Some(round)).unwrap();
        let mut path = vec![campaign::current_node(&w, &c.id).unwrap().map(|n| n.title).unwrap_or_else(|| "（无开场）".into())];
        for _ in 0..15 {
            let mut options: Vec<_> = campaign::available_edges(&w, &c.id).unwrap().into_iter().filter(|o| o.availability != chronicle_core::rules::Tri::False).collect();
            if prefer_last {
                options.reverse();
            }
            let Some(o) = options.first() else { break };
            let r = campaign::take_edge(&mut w, &c.id, &o.edge.id).unwrap();
            path.push(format!("{}「{}」{}", if r.taken { "→" } else { "×" }, o.edge.label, if r.taken { campaign::current_node(&w, &c.id).unwrap().unwrap().title } else { String::new() }));
            if campaign::current_node(&w, &c.id).unwrap().is_some_and(|n| n.kind == PlotNodeKind::Ending) {
                break;
            }
        }
        let st = campaign::campaign_state(w.conn(), &c.id).unwrap();
        println!("试玩{round}：{}\n  状态：任务 {:?}，线索 {:?}，物品 {:?}，变量 {:?}", path.join(" "), st.quests, st.clues, st.items, st.vars);
    }

    // 按玩家行为生成新分支。
    let from = nodes.iter().find(|n| !matches!(n.kind, PlotNodeKind::Ending)).unwrap();
    let t1 = Instant::now();
    let task = Task::PlotBranches { script_id: s.id.clone(), from_node_id: from.id.clone(), player_action: "玩家不相信村长，想偷偷跟踪他，看他晚上去哪里".into(), count: 2 };
    let out = runner::run(&mut w, &client, task, &t, Some(&caps), &opts, &cancel).unwrap();
    let cs = report(&w, &out, &format!("按玩家行为生成分支（从「{}」）", from.title), t1.elapsed().as_secs_f32());
    changeset::apply(&mut w, &cs.id, None, &Author::User).expect("写入新分支失败");
    for e in plot::outgoing(w.conn(), &from.id).unwrap() {
        let to = plot::get_plot_node(w.conn(), &e.to_id).unwrap().unwrap();
        println!("  从「{}」：「{}」→ [{}]{}", from.title, e.label, to.kind, to.title);
    }
}

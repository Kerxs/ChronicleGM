//! Phase 8 合成性能测试（真实计时，不调用真实模型；需要模型的回合用计数 mock，只测程序侧耗时与请求数）。
//!
//! 用法：cargo run --release -p chronicle-gm --example perf_phase8 -- <结果 JSON 路径> [quick]
//! 各项：NPC 数量、事件数量、大型关系网络、大型世界图谱、连续时间线回溯、数据库并发压力、强杀进程后的存档恢复。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chronicle_core::archive::{self, SearchFilter};
use chronicle_core::campaign;
use chronicle_core::network::{self, NetworkQuery};
use chronicle_core::timeline;
use chronicle_core::*;
use chronicle_gen::Templates;
use chronicle_gm::{Engine, GmOptions, run_turn};
use chronicle_llm::*;
use serde_json::{Value, json};

#[derive(serde::Serialize)]
struct Row {
    section: String,
    case: String,
    metric: String,
    value: f64,
    unit: String,
}

#[derive(Default)]
struct Report {
    rows: Vec<Row>,
}

impl Report {
    fn add(&mut self, section: &str, case: &str, metric: &str, value: f64, unit: &str) {
        println!("[{section}] {case:<28} {metric:<26} {value:>12.2} {unit}");
        self.rows.push(Row { section: section.into(), case: case.into(), metric: metric.into(), value, unit: unit.into() });
    }
}

/// 运行多次取中位数与最大值（毫秒）。
fn time_ms<T>(runs: usize, mut f: impl FnMut() -> T) -> (f64, f64) {
    let mut v: Vec<f64> = (0..runs)
        .map(|_| {
            let t = Instant::now();
            std::hint::black_box(f());
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (v[v.len() / 2], *v.last().unwrap())
}

fn eff(w: &mut World, c: &str, v: Value) {
    let e: Vec<chronicle_core::rules::Effect> = serde_json::from_value(v).unwrap();
    campaign::apply_effects(w, c, &e, "压测").unwrap();
}

// ---------- 计数 mock ----------

struct Counting {
    plan: String,
    requests: AtomicUsize,
    npc_requests: AtomicUsize,
    prompt_chars: AtomicUsize,
}

impl LlmClient for Counting {
    fn chat(&self, req: &ChatRequest, _c: &CancelToken, _p: client::Progress) -> chronicle_llm::Result<ChatResponse> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let chars: usize = req.messages.iter().map(|m| m.content.chars().count()).sum();
        self.prompt_chars.fetch_add(chars, Ordering::SeqCst);
        let sys = &req.messages[0].content;
        let user = &req.messages[1].content;
        let content = if sys.contains("扮演一个人物") {
            self.npc_requests.fetch_add(1, Ordering::SeqCst);
            json!({"reply": "嗯。", "attitude_change": 0, "remember": []}).to_string()
        } else if user.contains("程序判定结果") {
            json!({"narration": "你环顾四周。"}).to_string()
        } else if sys.contains("剧情导演") {
            json!({"assessment": "平稳", "pacing": "hold", "suggestions": []}).to_string()
        } else if user.contains("协助 TRPG 程序理解玩家") {
            "{}".into()
        } else {
            self.plan.clone()
        };
        Ok(ChatResponse { content, finish_reason: Some("stop".into()), usage: Usage::default(), model: None, latency_ms: 0, attempts: 1, used_json_schema: false })
    }
    fn describe(&self) -> (String, String) {
        ("mock".into(), "counting".into())
    }
}

// ---------- 1. NPC 数量 ----------

fn npc_scaling(rep: &mut Report, counts: &[usize]) {
    for &n in counts {
        let dir = tempfile::tempdir().unwrap();
        let mut w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "NPC").unwrap();
        let u = Author::User;
        let s = w.write(&u, "s", |x| x.create_script(NewScript::new("热闹的集市"))).unwrap().0;
        let mut ids = vec![];
        w.write(&u, "npcs", |x| {
            for i in 0..n {
                let node = x.create_node(NewNode::new(NodeKind::Character, format!("商贩{i}")).payload(json!({"identity": "商贩", "secrets": [format!("秘密{i}")]})))?;
                x.add_cast(NewCastMember { script_id: s.id.clone(), node_id: node.id.clone(), role: "商贩".into(), motive: String::new(), secret: String::new(), relation_to_players: String::new(), payload: json!({}) })?;
                ids.push(node.id);
            }
            Ok(())
        })
        .unwrap();
        w.write(&u, "p", |x| x.create_plot_node(NewPlotNode::new(&s.id, PlotNodeKind::Start, "集市"))).unwrap();
        let c = campaign::create_campaign(&mut w, Some(&s.id), "压测", Some(1)).unwrap().id;
        // 规划让所有 NPC 都发言（最坏情况）。
        let plan = json!({"intent": "和大家打招呼", "narration": "集市里人声鼎沸。", "npc_speakers": ids}).to_string();
        let llm = Counting { plan, requests: AtomicUsize::new(0), npc_requests: AtomicUsize::new(0), prompt_chars: AtomicUsize::new(0) };
        let world = Mutex::new(w);
        let tpl = Templates::builtin();
        let opts = GmOptions::default();
        let turns = 5;
        let t = Instant::now();
        for i in 0..turns {
            run_turn(&world, &Engine { gm: &llm, npc: &llm, templates: &tpl, opts: &opts }, &c, &format!("大家好，第 {i} 次"), &CancelToken::new()).unwrap();
        }
        let per_turn = t.elapsed().as_secs_f64() * 1000.0 / turns as f64;
        let case = format!("{n} 个 NPC（全部要求发言）");
        rep.add("npc", &case, "每回合模型请求数", llm.requests.load(Ordering::SeqCst) as f64 / turns as f64, "次");
        rep.add("npc", &case, "每回合 NPC 请求数", llm.npc_requests.load(Ordering::SeqCst) as f64 / turns as f64, "次");
        rep.add("npc", &case, "每回合提示词总长", llm.prompt_chars.load(Ordering::SeqCst) as f64 / turns as f64, "字");
        rep.add("npc", &case, "每回合程序耗时(mock 模型)", per_turn, "ms");
    }
}

// ---------- 2. 事件数量 ----------

fn build_campaign(turns: usize) -> (tempfile::TempDir, World, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "长战役").unwrap();
    let u = Author::User;
    let s = w.write(&u, "s", |x| x.create_script(NewScript::new("漫长旅途"))).unwrap().0;
    let mut npcs = vec![];
    let mut clues = vec![];
    w.write(&u, "n", |x| {
        for i in 0..30 {
            let n = x.create_node(NewNode::new(NodeKind::Character, format!("旅伴{i}")))?;
            x.add_cast(NewCastMember { script_id: s.id.clone(), node_id: n.id.clone(), role: "旅伴".into(), motive: String::new(), secret: String::new(), relation_to_players: String::new(), payload: json!({}) })?;
            npcs.push(n.id);
        }
        for i in 0..200 {
            clues.push(x.create_script_item(NewScriptItem::new(&s.id, ScriptItemKind::Clue, format!("线索{i}")))?.id);
        }
        Ok(())
    })
    .unwrap();
    w.write(&u, "p", |x| x.create_plot_node(NewPlotNode::new(&s.id, PlotNodeKind::Start, "起点"))).unwrap();
    let c = campaign::create_campaign(&mut w, Some(&s.id), "长战役", Some(5)).unwrap().id;
    timeline::ensure_baseline(w.conn(), &c).unwrap();
    for t in 0..turns {
        campaign::record(w.conn(), &c, "player", &json!({"text": format!("第 {t} 回合：我和旅伴{} 聊起远方的灯塔", t % 30)})).unwrap();
        campaign::record(w.conn(), &c, "gm_plan", &json!({"raw": "x".repeat(400)})).unwrap();
        campaign::record(w.conn(), &c, "narration", &json!({"text": format!("夜色渐深，篝火噼啪作响。这是旅途的第 {t} 个夜晚，远处隐约传来海浪声。")})).unwrap();
        campaign::record(w.conn(), &c, "npc", &json!({"npc_id": npcs[t % 30], "text": "灯塔的光已经很久没亮过了。"})).unwrap();
        let mut e = vec![json!({"type": "set_var", "key": "env.day", "value": t as i64})];
        if t % 3 == 0 {
            e.push(json!({"type": "adjust_bond", "from": npcs[t % 30], "to": "party", "trust": 1, "affection": 1, "reason": "同行"}));
        }
        if t % 25 == 0 && t / 25 < clues.len() {
            e.push(json!({"type": "reveal_clue", "clue": clues[t / 25]}));
        }
        eff(&mut w, &c, Value::Array(e));
        campaign::advance_turn(w.conn(), &c).unwrap();
        timeline::checkpoint(w.conn(), &c, "turn_end").unwrap();
        campaign::record(w.conn(), &c, "turn_end", &json!({"turn": t + 1})).unwrap();
    }
    (dir, w, c, s.id)
}

fn event_scaling(rep: &mut Report, turn_counts: &[usize]) {
    for &n in turn_counts {
        let t = Instant::now();
        let (dir, mut w, c, _s) = build_campaign(n);
        let build = t.elapsed().as_secs_f64();
        let events: i64 = w.conn().query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0)).unwrap();
        let case = format!("{n} 回合 / {events} 条事件");
        rep.add("events", &case, "生成存档", build * 1000.0, "ms");
        rep.add("events", &case, "每回合写入(含检查点)", build * 1000.0 / n as f64, "ms");
        let conn = w.conn();
        let runs = if n >= 5000 { 3 } else { 5 };
        let (m, _) = time_ms(runs, || archive::campaign_stats(conn, &c).unwrap());
        rep.add("events", &case, "战役统计", m, "ms");
        let (m, _) = time_ms(runs, || archive::chronicle(conn, &c).unwrap());
        rep.add("events", &case, "编年史", m, "ms");
        let (m, _) = time_ms(runs, || archive::search_events(conn, &SearchFilter { query: "灯塔".into(), limit: 200, ..Default::default() }).unwrap());
        rep.add("events", &case, "全文搜索(200 条)", m, "ms");
        let (m, _) = time_ms(runs, || archive::search_events(conn, &SearchFilter { query: "不存在的词语".into(), limit: 200, ..Default::default() }).unwrap());
        rep.add("events", &case, "全文搜索(无结果,全扫描)", m, "ms");
        let (m, _) = time_ms(runs, || campaign::recent_events(conn, &c, &["player", "narration", "npc"], 12).unwrap());
        rep.add("events", &case, "最近事件(GM 上下文用)", m, "ms");
        let (m, _) = time_ms(runs, || archive::archive_list(conn).unwrap());
        rep.add("events", &case, "档案列表", m, "ms");
        let (m, _) = time_ms(1, || assert!(timeline::verify(conn, &c).unwrap().ok));
        rep.add("events", &case, "重放校验全部检查点", m, "ms");
        let t = Instant::now();
        let ex = archive::export_campaign(conn, &c, false).unwrap();
        let text = serde_json::to_string(&ex).unwrap();
        rep.add("events", &case, "导出 JSON", t.elapsed().as_secs_f64() * 1000.0, "ms");
        rep.add("events", &case, "导出文件大小", text.len() as f64 / 1048576.0, "MB");
        let t = Instant::now();
        let back: archive::CampaignExport = serde_json::from_str(&text).unwrap();
        let r = archive::import_campaign(&mut w, &back).unwrap();
        rep.add("events", &case, "导入 JSON(含重放校验)", t.elapsed().as_secs_f64() * 1000.0, "ms");
        assert_eq!(r.events, ex.campaigns[0].tables["event_log"].len() as i64);
        let size = std::fs::metadata(dir.path().join("w.cgw")).map(|m| m.len()).unwrap_or(0);
        rep.add("events", &case, "数据库文件(导入后,两份)", size as f64 / 1048576.0, "MB");
        // GM 上下文组装（真实函数）。
        let (m, _) = time_ms(runs, || {
            chronicle_gm::context::build_gm_context(&w, &c, "我看看灯塔", GmOptions::default().context_budget).map(|x| x.npcs.len()).unwrap()
        });
        rep.add("events", &case, "GM 上下文组装", m, "ms");
    }
}

// ---------- 3. 大型关系网络 ----------

fn relation_network(rep: &mut Report, people: usize, bonds: usize) {
    let dir = tempfile::tempdir().unwrap();
    let mut w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "关系").unwrap();
    let u = Author::User;
    let mut ids = vec![];
    w.write(&u, "p", |x| {
        for i in 0..people {
            ids.push(x.create_node(NewNode::new(NodeKind::Character, format!("人物{i}")))?.id);
        }
        for i in 0..people {
            x.create_relation(NewRelation::new(&ids[i], &ids[(i * 7 + 1) % people], "朋友"))?;
        }
        Ok(())
    })
    .unwrap();
    let c = campaign::create_campaign(&mut w, None, "关系", Some(1)).unwrap().id;
    let t = Instant::now();
    let mut made = 0;
    let mut k: usize = 0;
    while made < bonds {
        let mut batch = vec![];
        for _ in 0..200 {
            let a = (k * 2_654_435_761) % people;
            let b = (k * 40_503 + 17) % people;
            k += 1;
            if a == b {
                continue;
            }
            batch.push(json!({"type": "adjust_bond", "from": ids[a], "to": ids[b], "kind": if k.is_multiple_of(5) { "敌对" } else { "熟人" }, "strength": 10, "trust": 5, "reason": "压测"}));
            made += 1;
        }
        eff(&mut w, &c, Value::Array(batch));
    }
    let case = format!("{people} 人 / {bonds} 次关系变化");
    rep.add("network", &case, "写入关系变化", t.elapsed().as_secs_f64() * 1000.0, "ms");
    let rows: i64 = w.conn().query_row("SELECT COUNT(*) FROM campaign_bonds WHERE campaign_id = ?1", [&c], |r| r.get(0)).unwrap();
    rep.add("network", &case, "不同关系对", rows as f64, "条");
    let conn = w.conn();
    let q = |search: &str, limit: usize| NetworkQuery { campaign_id: Some(c.clone()), search: search.into(), rel_types: vec![], include_world: true, min_strength: 0, limit, spoilers: true };
    let (m, x) = time_ms(5, || network::network(conn, &q("", 400)).unwrap());
    rep.add("network", &case, "全图查询(上限 400 节点)", m, "ms");
    rep.add("network", &case, "全图查询最大", x, "ms");
    let (m, _) = time_ms(5, || network::network(conn, &q("人物42", 400)).unwrap());
    rep.add("network", &case, "按名称聚焦查询", m, "ms");
    let (m, _) = time_ms(5, || network::bonds_of_person(conn, &c, &ids[42]).unwrap());
    rep.add("network", &case, "某人的全部关系", m, "ms");
    let (m, _) = time_ms(3, || archive::campaign_stats(conn, &c).unwrap());
    rep.add("network", &case, "战役统计(含关系变化汇总)", m, "ms");
}

// ---------- 4. 大型世界图谱 ----------

fn world_graph(rep: &mut Report, n_nodes: usize, n_rel: usize) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("chronicle-perf-graph-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = World::create(&dir.join("w.cgw"), &dir.join("b"), "大世界").unwrap();
    let u = Author::User;
    let t = Instant::now();
    let mut ids = vec![];
    w.write(&u, "g", |x| {
        let root = x.create_node(NewNode::new(NodeKind::World, "大陆"))?;
        let mut parents = vec![root.id.clone()];
        for i in 0..n_nodes {
            let kind = if i % 10 == 0 { NodeKind::Character } else { NodeKind::Site };
            let node = x.create_node(NewNode::new(kind, format!("条目{i}")).under(&parents[i % parents.len()]))?;
            if i < 3000 && i % 15 == 0 {
                parents.push(node.id.clone());
            }
            ids.push(node.id);
        }
        let types = ["位于", "盟友", "敌对", "通往", "隶属"];
        for k in 0..n_rel {
            let a = (k * 2_654_435_761) % ids.len();
            let b = (k * 97 + 13) % ids.len();
            if a != b {
                x.create_relation(NewRelation::new(&ids[a], &ids[b], types[k % types.len()]))?;
            }
        }
        Ok(())
    })
    .unwrap();
    let case = format!("{n_nodes} 节点 / {n_rel} 关系");
    rep.add("graph", &case, "生成世界", t.elapsed().as_secs_f64() * 1000.0, "ms");
    let conn = w.conn();
    let (m, _) = time_ms(10, || nodes::children(conn, None, 0, 300).unwrap());
    rep.add("graph", &case, "顶层 300 个子节点", m, "ms");
    let first = nodes::children(conn, None, 0, 1).unwrap()[0].id.clone();
    let (m, _) = time_ms(10, || nodes::children(conn, Some(&first), 0, 300).unwrap());
    rep.add("graph", &case, "展开一个节点(300 子)", m, "ms");
    let page: Vec<String> = ids.iter().take(300).cloned().collect();
    let (m, _) = time_ms(10, || network::relations_among(conn, &page).unwrap());
    rep.add("graph", &case, "300 个可见节点间的关系", m, "ms");
    let (m, _) = time_ms(10, || relations::neighborhood(conn, &ids[5], 2, 300).unwrap());
    rep.add("graph", &case, "两跳邻域(上限 300)", m, "ms");
    let (m, _) = time_ms(10, || chronicle_core::search::search_nodes(conn, "条目123", &chronicle_core::search::SearchOptions::default()).unwrap());
    rep.add("graph", &case, "节点全文搜索", m, "ms");
    drop(w);
    dir
}

// ---------- 5. 连续时间线回溯 ----------

fn chained_forks(rep: &mut Report, depth: usize) {
    let (_dir, mut w, c0, _) = build_campaign(20);
    let orig_events: i64 = w.conn().query_row("SELECT COUNT(*) FROM event_log WHERE campaign_id = ?1", [&c0], |r| r.get(0)).unwrap();
    let orig_cp: Vec<String> = timeline::checkpoints(w.conn(), &c0).unwrap().into_iter().map(|c| c.checksum).collect();
    let mut cur = c0.clone();
    let mut fork_ms = vec![];
    let mut verify_ms = vec![];
    for d in 0..depth {
        // 每条时间线再玩 3 回合后结束，从倒数第二个检查点回溯。
        for t in 0..3 {
            campaign::record(w.conn(), &cur, "player", &json!({"text": format!("第 {d} 层第 {t} 回合")})).unwrap();
            eff(&mut w, &cur, json!([{"type": "set_var", "key": "env.depth", "value": d as i64}]));
            campaign::advance_turn(w.conn(), &cur).unwrap();
            timeline::checkpoint(w.conn(), &cur, "turn_end").unwrap();
        }
        timeline::end_campaign(&mut w, &cur, &format!("第 {d} 层结局")).unwrap();
        let cps = timeline::checkpoints(w.conn(), &cur).unwrap();
        let cp = cps[cps.len() - 3].id;
        let t = Instant::now();
        let next = timeline::fork(&mut w, &cur, cp, "", false).unwrap().id;
        fork_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        assert!(timeline::verify(w.conn(), &next).unwrap().ok, "第 {d} 层回溯后校验失败");
        verify_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        cur = next;
    }
    let case = format!("连续回溯 {depth} 层");
    rep.add("forks", &case, "第 1 层回溯耗时", fork_ms[0], "ms");
    rep.add("forks", &case, &format!("第 {depth} 层回溯耗时"), *fork_ms.last().unwrap(), "ms");
    rep.add("forks", &case, &format!("第 {depth} 层校验耗时"), *verify_ms.last().unwrap(), "ms");
    let now_events: i64 = w.conn().query_row("SELECT COUNT(*) FROM event_log WHERE campaign_id = ?1", [&c0], |r| r.get(0)).unwrap();
    let now_cp: Vec<String> = timeline::checkpoints(w.conn(), &c0).unwrap().into_iter().map(|c| c.checksum).collect();
    // 原时间线在第一层被继续玩过 3 回合并结束，比较的是"开始回溯之后"不再变化：第 1 层回溯后的快照。
    rep.add("forks", &case, "原时间线事件(回溯前/后)", (now_events - orig_events) as f64, "条新增(仅第1层自身游玩)");
    rep.add("forks", &case, "原时间线旧检查点被改动", orig_cp.iter().zip(&now_cp).filter(|(a, b)| a != b).count() as f64, "个");
    let checks = archive::check_campaigns(w.conn()).unwrap();
    rep.add("forks", &case, "完整性检查通过", checks.iter().filter(|c| c.ok).count() as f64, &format!("/ {} 条时间线", checks.len()));
    let (m, _) = time_ms(1, || archive::check_campaigns(w.conn()).unwrap());
    rep.add("forks", &case, "检查全部时间线", m, "ms");
}

// ---------- 6. 数据库并发压力 ----------

fn db_stress(rep: &mut Report, writes: usize, readers: usize) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("w.cgw");
    let backups = dir.path().join("b");
    let (_d0, w0, c, _) = build_campaign(50);
    // 把 50 回合的存档复制到压测路径（备份 API）。
    w0.conn().execute("VACUUM INTO ?1", [path.to_string_lossy().to_string()]).unwrap();
    drop(w0);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let lat = Arc::new(Mutex::new(Vec::<f64>::new()));
    let errs = Arc::new(AtomicUsize::new(0));
    let ready = Arc::new(std::sync::Barrier::new(readers + 1));
    let mut hs = vec![];
    for i in 0..readers {
        let (stop, lat, errs, path, backups, c, ready) = (stop.clone(), lat.clone(), errs.clone(), path.clone(), backups.clone(), c.clone(), ready.clone());
        hs.push(std::thread::spawn(move || {
            let (w, _) = World::open(&path, &backups).unwrap();
            ready.wait();
            let mut k = 0;
            while !stop.load(Ordering::Relaxed) {
                let t = Instant::now();
                let ok = match (i + k) % 3 {
                    0 => archive::campaign_stats(w.conn(), &c).is_ok(),
                    1 => campaign::recent_events(w.conn(), &c, &["player", "narration"], 12).is_ok(),
                    _ => archive::search_events(w.conn(), &SearchFilter { query: "灯塔".into(), limit: 50, ..Default::default() }).is_ok(),
                };
                if !ok && errs.fetch_add(1, Ordering::Relaxed) == 0 {
                    println!("第一次读取失败");
                }
                lat.lock().unwrap().push(t.elapsed().as_secs_f64() * 1000.0);
                k += 1;
            }
        }));
    }
    let (mut w, _) = World::open(&path, &backups).unwrap();
    ready.wait();
    let t = Instant::now();
    let mut wlat = vec![];
    let mut werr = 0;
    for i in 0..writes {
        let t1 = Instant::now();
        let e: Vec<chronicle_core::rules::Effect> = serde_json::from_value(json!([{"type": "set_var", "key": format!("stress.{}", i % 50), "value": i as i64}])).unwrap();
        if let Err(e) = campaign::apply_effects(&mut w, &c, &e, "压测") {
            if werr == 0 {
                println!("第一次写入失败：{e}");
            }
            werr += 1;
        }
        wlat.push(t1.elapsed().as_secs_f64() * 1000.0);
    }
    let total = t.elapsed().as_secs_f64();
    stop.store(true, Ordering::Relaxed);
    for h in hs {
        h.join().unwrap();
    }
    let pct = |v: &mut Vec<f64>, p: f64| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[((v.len() as f64 - 1.0) * p) as usize]
    };
    let case = format!("{writes} 次写入 + {readers} 个读线程");
    rep.add("stress", &case, "写入吞吐", writes as f64 / total, "次/秒");
    rep.add("stress", &case, "写入 p50", pct(&mut wlat, 0.5), "ms");
    rep.add("stress", &case, "写入 p99", pct(&mut wlat, 0.99), "ms");
    rep.add("stress", &case, "写入失败", werr as f64, "次");
    let mut l = lat.lock().unwrap().clone();
    rep.add("stress", &case, "读取次数", l.len() as f64, "次");
    rep.add("stress", &case, "读取 p50", pct(&mut l, 0.5), "ms");
    rep.add("stress", &case, "读取 p99", pct(&mut l, 0.99), "ms");
    rep.add("stress", &case, "读取失败", errs.load(Ordering::Relaxed) as f64, "次");
    let ok = timeline::verify(w.conn(), &c).map(|r| r.ok).unwrap_or(false);
    rep.add("stress", &case, "压测后重放校验通过", ok as i32 as f64, "(1=是)");
    // 另一个线程反复打开/关闭同一个数据库文件时继续写入（模拟外部工具或第二个实例打开存档）。
    let stop2 = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let opens = Arc::new(AtomicUsize::new(0));
    let h = {
        let (stop2, opens, path, backups) = (stop2.clone(), opens.clone(), path.clone(), backups.clone());
        std::thread::spawn(move || {
            while !stop2.load(Ordering::Relaxed) {
                if World::open(&path, &backups).is_ok() {
                    opens.fetch_add(1, Ordering::Relaxed);
                }
            }
        })
    };
    let mut werr2 = 0;
    let n2 = writes;
    for i in 0..n2 {
        let e: Vec<chronicle_core::rules::Effect> = serde_json::from_value(json!([{"type": "set_var", "key": "stress2", "value": i as i64}])).unwrap();
        if let Err(e) = campaign::apply_effects(&mut w, &c, &e, "压测") {
            if werr2 == 0 {
                println!("反复打开时第一次写入失败：{e}");
            }
            werr2 += 1;
        }
    }
    stop2.store(true, Ordering::Relaxed);
    h.join().unwrap();
    let case2 = format!("{n2} 次写入 + 另一线程反复打开存档");
    rep.add("stress", &case2, "期间打开次数", opens.load(Ordering::Relaxed) as f64, "次");
    rep.add("stress", &case2, "写入失败", werr2 as f64, "次");
}

// ---------- 7. 强杀进程后的存档恢复 ----------

fn crash_child(path: &str) {
    let p = std::path::Path::new(path);
    let (mut w, _) = World::open(p, &p.parent().unwrap().join("b")).unwrap();
    let c = campaign::list_campaigns(w.conn()).unwrap()[0].id.clone();
    let mut t = 0u64;
    loop {
        // 模拟一个 GM 回合的多个短事务（与 run_turn 相同的写入顺序）。
        campaign::record(w.conn(), &c, "player", &json!({"text": format!("崩溃测试 {t}")})).unwrap();
        let e: Vec<chronicle_core::rules::Effect> = serde_json::from_value(json!([{"type": "set_var", "key": "crash.n", "value": t as i64}])).unwrap();
        campaign::apply_effects(&mut w, &c, &e, "崩溃测试").unwrap();
        campaign::record(w.conn(), &c, "narration", &json!({"text": "……"})).unwrap();
        campaign::advance_turn(w.conn(), &c).unwrap();
        timeline::checkpoint(w.conn(), &c, "turn_end").unwrap();
        campaign::record(w.conn(), &c, "turn_end", &json!({"turn": t})).unwrap();
        t += 1;
        if t == 1 {
            println!("ready");
        }
    }
}

fn crash_recovery(rep: &mut Report, rounds: usize) {
    let (_d, w0, _c, _) = build_campaign(10);
    let dir = std::env::temp_dir().join(format!("chronicle-perf-crash-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("w.cgw");
    w0.conn().execute("VACUUM INTO ?1", [path.to_string_lossy().to_string()]).unwrap();
    drop(w0);
    let exe = std::env::current_exe().unwrap();
    let mut ok_rounds = 0;
    let mut incomplete = 0;
    let mut turns_before = 0i64;
    for r in 0..rounds {
        let mut child = std::process::Command::new(&exe).arg("--crash-child").arg(&path).stdout(std::process::Stdio::piped()).spawn().unwrap();
        // 等子进程开始写，再在随机时刻强杀（TerminateProcess，相当于断电/任务管理器结束进程）。
        let mut out = child.stdout.take().unwrap();
        let mut buf = [0u8; 6];
        let _ = std::io::Read::read(&mut out, &mut buf);
        std::thread::sleep(Duration::from_millis(300 + (r as u64 * 137) % 900));
        child.kill().unwrap();
        child.wait().unwrap();
        let t = Instant::now();
        let (w, _) = World::open(&path, &dir.join("b")).unwrap();
        let integ = integrity::check(&w).unwrap();
        let c = campaign::list_campaigns(w.conn()).unwrap()[0].clone();
        let checks = archive::check_campaigns(w.conn()).unwrap();
        let inc = campaign::incomplete_turn(w.conn(), &c.id).unwrap();
        let open_ms = t.elapsed().as_secs_f64() * 1000.0;
        if inc.is_some() {
            incomplete += 1;
        }
        let good = integ.ok && checks.iter().all(|x| x.ok) && c.turn >= turns_before;
        if good {
            ok_rounds += 1;
        } else {
            println!("第 {r} 轮：integrity {:?} checks {:?}", integ.issues, checks.iter().map(|x| &x.problems).collect::<Vec<_>>());
        }
        turns_before = c.turn;
        if r == 0 {
            rep.add("crash", "强杀后重新打开", "打开+完整性+重放校验", open_ms, "ms");
        }
        drop(w);
    }
    let case = format!("强杀写入中的进程 {rounds} 次");
    rep.add("crash", &case, "恢复后完整且校验通过", ok_rounds as f64, &format!("/ {rounds} 轮"));
    rep.add("crash", &case, "检测到中断的回合", incomplete as f64, "轮(会提示用户)");
    rep.add("crash", &case, "最终回合数", turns_before as f64, "回合");
    let _ = std::fs::remove_dir_all(&dir);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--crash-child") {
        crash_child(&args[1]);
        return;
    }
    let out = args.first().cloned().unwrap_or_else(|| "perf_phase8.json".into());
    let quick = args.iter().any(|a| a == "quick");
    let mut rep = Report::default();
    let started = Instant::now();
    npc_scaling(&mut rep, if quick { &[2, 50] } else { &[2, 8, 50, 200, 1000] });
    event_scaling(&mut rep, if quick { &[100] } else { &[100, 1000, 5000] });
    relation_network(&mut rep, if quick { 200 } else { 2000 }, if quick { 2000 } else { 20_000 });
    let gdir = world_graph(&mut rep, if quick { 5000 } else { 50_000 }, if quick { 10_000 } else { 100_000 });
    let _ = std::fs::remove_dir_all(gdir);
    chained_forks(&mut rep, if quick { 5 } else { 30 });
    db_stress(&mut rep, if quick { 500 } else { 5000 }, 4);
    crash_recovery(&mut rep, if quick { 3 } else { 10 });
    let total = started.elapsed().as_secs_f64();
    println!("总耗时 {total:.1} 秒");
    let doc = json!({ "generated_at": util::now_ms(), "quick": quick, "total_seconds": total, "rows": rep.rows });
    std::fs::write(&out, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    println!("结果已写入 {out}");
}

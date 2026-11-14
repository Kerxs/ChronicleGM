//! 长战役性能实测：大世界 + 很多回合时，GM 每回合的上下文组装要多久。
//! 用法：cargo run --release -p chronicle-gm --example bench_campaign -- [节点数] [回合数]
//! 真实计时（中位数 / 最大值），不调用模型。

use std::time::{Duration, Instant};

use chronicle_core::campaign;
use chronicle_core::*;
use chronicle_gm::context::{build_gm_context, build_npc_context, present_npcs};
use serde_json::json;

fn measure<T>(label: &str, runs: usize, target_ms: Option<f64>, mut f: impl FnMut(usize) -> T) {
    let mut times: Vec<Duration> = (0..runs)
        .map(|i| {
            let t = Instant::now();
            std::hint::black_box(f(i));
            t.elapsed()
        })
        .collect();
    times.sort();
    let med = times[times.len() / 2].as_secs_f64() * 1000.0;
    let max = times.last().unwrap().as_secs_f64() * 1000.0;
    let verdict = match target_ms {
        Some(t) if med <= t => format!("达标（目标 < {t} ms）"),
        Some(t) => format!("未达标（目标 < {t} ms）"),
        None => String::new(),
    };
    println!("{label:<30} 中位数 {med:>9.2} ms   最大 {max:>9.2} ms   ×{runs}  {verdict}");
}

fn main() {
    let args: Vec<usize> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let n_nodes = args.first().copied().unwrap_or(50_000);
    let n_turns = args.get(1).copied().unwrap_or(5_000);
    let dir = tempfile::tempdir().unwrap();
    let mut w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "长战役").unwrap();
    let u = Author::User;

    let t = Instant::now();
    let mut chars = vec![];
    w.write(&u, "bench", |x| {
        let root = x.create_node(NewNode::new(NodeKind::World, "大陆"))?;
        let mut parents = vec![root.id.clone()];
        for i in 0..n_nodes {
            let kind = if i % 10 == 0 { NodeKind::Character } else { NodeKind::Site };
            let mut n = NewNode::new(kind, format!("地点或人物{i}")).under(&parents[i % parents.len()]);
            n.summary = format!("第 {i} 个条目的简介，包含一些用于搜索的文字");
            if kind == NodeKind::Character {
                n.payload = json!({"identity": format!("人物{i}"), "secrets": [format!("秘密{i}")], "knowledge": [format!("知道{i}")]});
            }
            let node = x.create_node(n)?;
            if i < 2000 && i % 20 == 0 {
                parents.push(node.id.clone());
            }
            if kind == NodeKind::Character && chars.len() < 30 {
                chars.push(node.id.clone());
            }
        }
        for _ in 0..20 {
            x.create_node(NewNode::new(NodeKind::MagicSystem, "核心法则").canon(CanonLevel::Core))?;
        }
        Ok(())
    })
    .unwrap();
    let s = w.write(&u, "s", |x| x.create_script(NewScript::new("长篇剧本"))).unwrap().0;
    w.write(&u, "c", |x| {
        for id in &chars {
            x.add_cast(NewCastMember { script_id: s.id.clone(), node_id: id.clone(), role: "配角".into(), motive: String::new(), secret: "隐瞒".into(), relation_to_players: String::new(), payload: json!({}) })?;
        }
        let mut prev = x.create_plot_node(NewPlotNode::new(&s.id, PlotNodeKind::Start, "开场"))?;
        for i in 0..200 {
            let n = x.create_plot_node(NewPlotNode::new(&s.id, PlotNodeKind::Scene, format!("场景{i}")))?;
            let mut e = NewPlotEdge::new(&s.id, &prev.id, &n.id);
            e.label = format!("前往场景{i}");
            x.create_plot_edge(e)?;
            prev = n;
        }
        for i in 0..80 {
            let mut it = NewScriptItem::new(&s.id, if i % 2 == 0 { ScriptItemKind::Clue } else { ScriptItemKind::Quest }, format!("条目{i}"));
            it.player_text = format!("条目{i}的玩家可见描述");
            if i % 2 == 1 {
                it.payload = json!({"quest_type": "side"});
            }
            x.create_script_item(it)?;
        }
        Ok(())
    })
    .unwrap();
    println!("建世界：{n_nodes} 节点，用时 {:.1}s", t.elapsed().as_secs_f32());

    // 长战役：每回合 player / roll / effects / gm_narration / turn_end，另有事实、NPC 记忆、摘要。
    let t = Instant::now();
    let c = campaign::create_campaign(&mut w, Some(&s.id), "长团", Some(1)).unwrap();
    {
        let conn = w.conn();
        conn.execute_batch("BEGIN").unwrap();
        for turn in 0..n_turns {
            campaign::record(conn, &c.id, "player", &json!({"text": format!("第{turn}回合玩家行动")})).unwrap();
            campaign::record(conn, &c.id, "roll", &json!({"expr":"1d20+2","total": 12})).unwrap();
            // 与 campaign::roll 一致：每次掷骰占用一个流号（否则重放校验会正确地报告骰子计数对不上）。
            conn.execute("UPDATE campaigns SET roll_counter = roll_counter + 1 WHERE id = ?1", [&c.id]).unwrap();
            campaign::record(conn, &c.id, "gm_narration", &json!({"text": format!("第{turn}回合的叙述，篇幅中等，描写场景与结果。")})).unwrap();
            campaign::record(conn, &c.id, "turn_end", &json!({})).unwrap();
            campaign::advance_turn(conn, &c.id).unwrap();
            chronicle_core::timeline::checkpoint(conn, &c.id, "turn_end").unwrap();
            if turn % 5 == 0 {
                campaign::add_fact(conn, &c.id, &format!("事实{turn}：玩家做过的重要事情")).unwrap();
            }
            if turn % 100 == 99 {
                let sm = campaign::add_summary_draft(conn, &c.id, &format!("截至第{turn}回合的摘要"), turn as i64 * 4).unwrap();
                campaign::confirm_summary(conn, &sm.id, None).unwrap();
            }
        }
        conn.execute_batch("COMMIT").unwrap();
    }
    for (i, npc) in chars.iter().enumerate() {
        for m in 0..(n_turns / 25) {
            campaign::add_npc_memory(&w, &c.id, npc, &format!("NPC{i} 记得第{m}件事"), (m % 5) as i64).unwrap();
        }
    }
    let n_events = campaign::events(w.conn(), &c.id, 0, i64::MAX).unwrap().len();
    println!("建战役：{n_turns} 回合 / {n_events} 条事件，用时 {:.1}s\n", t.elapsed().as_secs_f32());

    let script = scripts::get_script(w.conn(), &s.id).unwrap();
    let cur = campaign::current_node(&w, &c.id).unwrap();
    let npcs = present_npcs(&w, script.as_ref(), cur.as_ref()).unwrap();
    measure("GM 上下文组装（每回合）", 30, Some(200.0), |_| build_gm_context(&w, &c.id, "我四处打听最近发生的事", 8000).unwrap());
    measure("  其中：核心设定查询", 30, None, |_| chronicle_gen::context::core_canon(&w, 30).unwrap());
    measure("  其中：玩家行动相关设定搜索", 30, None, |_| {
        chronicle_core::search::search_nodes(w.conn(), "我四处打听最近发生的事", &chronicle_core::search::SearchOptions { limit: 5, ..Default::default() }).unwrap()
    });
    measure("NPC 上下文组装（每个 NPC）", 30, Some(100.0), |i| build_npc_context(&w, &c.id, &npcs[i % npcs.len()], &npcs, script.as_ref(), cur.as_ref()).unwrap());
    measure("战役状态（任务/线索/变量）", 30, Some(50.0), |_| campaign::campaign_state(w.conn(), &c.id).unwrap());
    measure("可走分支求值", 30, Some(50.0), |_| campaign::available_edges(&w, &c.id).unwrap());
    measure("中断回合检测", 30, Some(20.0), |_| campaign::incomplete_turn(w.conn(), &c.id).unwrap());
    measure("最近 50 条事件", 30, Some(20.0), |_| campaign::recent_events(w.conn(), &c.id, &["player", "gm_narration"], 50).unwrap());
    let ctx = build_gm_context(&w, &c.id, "我四处打听", 8000).unwrap();
    println!("\nGM 上下文长度：{} 字符（预算 8000 token）", ctx.text.chars().count());
    let t = Instant::now();
    let rep = chronicle_core::integrity::check(&w).unwrap();
    println!("完整性检查（全世界）：{:.0} ms，问题 {} 个", t.elapsed().as_secs_f64() * 1000.0, rep.issues.len());
    let t = Instant::now();
    let b = chronicle_core::backup::auto_backup(&w, 10, 0).unwrap().unwrap();
    println!("自动备份：{:.0} ms，{:.1} MB", t.elapsed().as_secs_f64() * 1000.0, b.size as f64 / 1048576.0);

    // 回溯：每回合一个检查点时的体积与耗时。
    use chronicle_core::timeline;
    let (n_cp, bytes): (i64, i64) = w.conn().query_row("SELECT COUNT(*), SUM(length(state)) FROM campaign_checkpoints WHERE campaign_id = ?1", [&c.id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    println!("
检查点：{n_cp} 个，共 {:.1} MB（平均 {} 字节）", bytes as f64 / 1048576.0, bytes / n_cp.max(1));
    let t = Instant::now();
    timeline::end_campaign(&mut w, &c.id, "基准结束").unwrap();
    println!("结束战役（含最终快照）：{:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    let t = Instant::now();
    let rep = timeline::verify(w.conn(), &c.id).unwrap();
    println!("重放校验 {} 条事件 / {} 个检查点：{:.0} ms，结果 {}", rep.events, rep.checkpoints, t.elapsed().as_secs_f64() * 1000.0, if rep.ok { "通过" } else { "不通过" });
    if !rep.ok {
        println!("  {:?}", rep.mismatches.iter().take(3).collect::<Vec<_>>());
    }
    let mid = timeline::checkpoints(w.conn(), &c.id).unwrap()[n_cp as usize / 2].id;
    let t = Instant::now();
    let b1 = timeline::fork(&mut w, &c.id, mid, "", false).unwrap();
    println!("从第 {} 回合回溯开出新时间线：{:.0} ms", b1.turn, t.elapsed().as_secs_f64() * 1000.0);
    timeline::end_campaign(&mut w, &b1.id, "").unwrap();
    let early = timeline::checkpoints(w.conn(), &b1.id).unwrap()[10].id;
    let t = Instant::now();
    let b2 = timeline::fork(&mut w, &b1.id, early, "", false).unwrap();
    println!("再从新时间线回溯（连续回溯）：{:.0} ms，第 {} 回合", t.elapsed().as_secs_f64() * 1000.0, b2.turn);
    measure("回溯后 GM 上下文组装", 10, Some(200.0), |_| build_gm_context(&w, &b1.id, "我四处打听最近发生的事", 8000).unwrap());
}

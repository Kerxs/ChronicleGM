//! 大规模世界性能实测：5 万节点 / 20 万关系。
//! 用法：cargo run --release -p chronicle-gen --example bench_scale -- [节点数] [关系数]
//! 每项操作重复多次，报告中位数与最大值（真实计时，不做推算）。

use std::time::{Duration, Instant};

use chronicle_core::changeset::{self, Mode, Op};
use chronicle_core::search::{SearchOptions, search_nodes};
use chronicle_core::*;
use chronicle_gen::tasks::build_prompt;
use chronicle_gen::{Task, Templates};

const SYL: &[&str] = &["艾", "伦", "索", "维", "塔", "莉", "安", "卡", "洛", "斯", "米", "拉", "诺", "雅", "格", "瑞", "德", "尔", "温", "珂"];

fn name(i: usize) -> String {
    format!("{}{}{}{}", SYL[i % 20], SYL[(i / 20) % 20], SYL[(i / 400) % 20], i)
}

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
    println!("{label:<32} 中位数 {med:>9.2} ms   最大 {max:>9.2} ms   ×{runs}  {verdict}");
}

fn main() {
    let args: Vec<usize> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let target_nodes = args.first().copied().unwrap_or(50_000);
    let target_rels = args.get(1).copied().unwrap_or(200_000);
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = World::open(&dir.path().join("big.cgw"), &dir.path().join("b")).unwrap();

    // 结构：根 → 20 个分类 → 每类 25 组 → 每组若干叶子，凑足目标节点数。
    let t = Instant::now();
    let per_group = (target_nodes.saturating_sub(1 + 20 + 500)).div_ceil(500).max(1);
    let mut ids: Vec<String> = Vec::with_capacity(target_nodes);
    let mut groups = vec![];
    w.write(&Author::User, "bench", |x| {
        let root = x.create_node(NewNode::new(NodeKind::World, "基准世界"))?;
        ids.push(root.id.clone());
        let mut i = 0usize;
        for c in 0..20 {
            let cat = x.create_node(NewNode::new(NodeKind::Category, format!("分类{c}")).under(&root.id))?;
            ids.push(cat.id.clone());
            for _g in 0..25 {
                let g = x.create_node(NewNode::new(NodeKind::Region, name(i)).under(&cat.id))?;
                i += 1;
                ids.push(g.id.clone());
                groups.push(g.id.clone());
                for _ in 0..per_group {
                    if ids.len() >= target_nodes {
                        break;
                    }
                    let mut n = NewNode::new(if i.is_multiple_of(3) { NodeKind::Character } else { NodeKind::City }, name(i)).under(&g.id);
                    n.summary = format!("{}是一个关于{}与{}的地方，传说与潮汐有关", name(i), SYL[i % 20], SYL[(i + 7) % 20]);
                    n.gm_text = format!("秘密编号{i}");
                    let node = x.create_node(n)?;
                    i += 1;
                    ids.push(node.id);
                }
            }
        }
        Ok(())
    })
    .unwrap();
    println!("生成 {} 个节点：{:.1} s", ids.len(), t.elapsed().as_secs_f64());

    let t = Instant::now();
    let types = ["ally", "enemy", "located_in", "member_of", "friend", "trade"];
    let mut seed: u64 = 42;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let n = ids.len() as u64;
    w.write(&Author::User, "bench-rel", |x| {
        let mut made = 0;
        while made < target_rels {
            let a = (rnd() % n) as usize;
            let b = (rnd() % n) as usize;
            if a == b {
                continue;
            }
            x.create_relation(NewRelation::new(&ids[a], &ids[b], types[made % types.len()]))?;
            made += 1;
        }
        Ok(())
    })
    .unwrap();
    println!("生成 {target_rels} 条关系：{:.1} s", t.elapsed().as_secs_f64());
    // 重新打开世界（与程序实际使用一致：打开时会执行 PRAGMA optimize 更新统计信息）。
    let t = Instant::now();
    let path = w.path().to_path_buf();
    drop(w);
    let (mut w, _) = World::open(&path, &dir.path().join("b")).unwrap();
    println!("重新打开世界：{:.2} s", t.elapsed().as_secs_f64());
    let size = std::fs::metadata(w.path()).map(|m| m.len()).unwrap_or(0);
    println!("世界文件大小：{:.1} MB（不含 WAL）\n", size as f64 / 1e6);

    let conn = w.conn();
    measure("展开节点（约 100 个子节点）", 50, Some(50.0), |i| nodes::children(conn, Some(&groups[i * 7 % groups.len()]), 0, 200).unwrap());
    measure("展开根级分类（25 个子节点）", 50, Some(50.0), |i| nodes::children(conn, Some(&ids[1 + (i % 20) * 26]), 0, 200).unwrap());
    measure("子节点计数（200 个节点）", 20, Some(50.0), |i| {
        let batch: Vec<String> = ids.iter().skip(i * 100).take(200).cloned().collect();
        relations::child_counts(conn, &batch).unwrap()
    });
    let opts = SearchOptions { limit: 50, ..Default::default() };
    measure("全文搜索（≥3 字，命中多）", 30, Some(100.0), |_| search_nodes(conn, "与潮汐有关", &opts).unwrap());
    measure("全文搜索（≥3 字，精确名称）", 30, Some(100.0), |i| search_nodes(conn, &name(1000 + i * 37), &opts).unwrap());
    measure("短词搜索（2 字，LIKE 回退）", 30, Some(100.0), |_| search_nodes(conn, "艾伦", &opts).unwrap());
    let pv = SearchOptions { player_view: true, limit: 50, ..Default::default() };
    measure("玩家视角搜索", 30, Some(100.0), |_| search_nodes(conn, "与潮汐有关", &pv).unwrap());
    measure("祖先链", 50, None, |i| nodes::ancestors(conn, &ids[ids.len() - 1 - i]).unwrap());
    measure("节点的全部关系", 50, None, |i| nodes::relations_of(conn, &ids[600 + i]).unwrap());
    measure("关系邻域图（2 层，≤150 节点）", 20, None, |i| relations::neighborhood(conn, &ids[700 + i], 2, 150).unwrap());
    measure("按名称查重", 50, None, |i| nodes::find_by_name(conn, &name(2000 + i), None).unwrap());

    let templates = Templates::builtin();
    measure("上下文组装（展开节点任务）", 20, Some(200.0), |i| {
        let task = Task::ExpandNode { node_id: ids[800 + i].clone(), count: 5, kinds: vec![], guidance: String::new() };
        build_prompt(&w, &task, &templates, 16000, false).unwrap()
    });
    measure("上下文组装（一句话生成世界）", 5, Some(200.0), |_| {
        let task = Task::WorldSeed { idea: "x".into(), genre: String::new(), era: String::new(), style: String::new(), rules: String::new() };
        build_prompt(&w, &task, &templates, 16000, false).unwrap()
    });

    let ops: Vec<Op> = (0..20)
        .map(|k| Op::CreateNode { temp_id: None, node: NewNode::new(NodeKind::City, format!("{}新", name(k * 101))).under(&groups[k]) })
        .collect();
    measure("变更集试运行校验（20 项）", 10, None, |_| changeset::validate(&mut w, &Mode::Append, &Author::Ai { job_id: "b".into() }, &ops).unwrap());
    measure("一致性检查（全世界）", 3, None, |_| consistency::check_world(w.conn()).unwrap().len());
    let (found, timings) = consistency::check_world_profiled(w.conn()).unwrap();
    println!("    分项：{}（发现 {} 条）", timings.iter().map(|(n, ms)| format!("{n} {ms:.0} ms")).collect::<Vec<_>>().join("，"), found.len());
    let mut k = 0;
    measure("导出世界包", 3, Some(10_000.0), |_| {
        k += 1;
        bundle::export_bundle(&w, &dir.path().join(format!("export{k}.cgwpack")), false).unwrap()
    });
    let pack = std::fs::metadata(dir.path().join("export1.cgwpack")).unwrap().len();
    println!("\n导出包大小：{:.1} MB", pack as f64 / 1e6);
    let t = Instant::now();
    let lib = Library::open(&dir.path().join("lib")).unwrap();
    lib.import_world(&dir.path().join("export1.cgwpack"), Some("导入测试")).unwrap();
    println!("导入为新世界：{:.1} s", t.elapsed().as_secs_f64());
}

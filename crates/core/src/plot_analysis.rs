//! 剧情图分析：开场、可达性、死路、结局、引用、无条件循环。只读。

use std::collections::{HashMap, HashSet, VecDeque};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::changeset::Severity;
use crate::entity::Entity;
use crate::error::Result;
use crate::model::*;
use crate::plot::{edges_of_script, plot_nodes_of};
use crate::scripts::require_script;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct PlotIssue {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub node_ids: Vec<String>,
    pub edge_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct PlotReport {
    pub issues: Vec<PlotIssue>,
    pub node_count: usize,
    pub edge_count: usize,
    pub ending_count: usize,
    /// 从开场出发（不考虑条件）能到达的结局数。
    pub reachable_endings: usize,
}

fn issue(code: &str, severity: Severity, message: String, node_ids: Vec<String>, edge_ids: Vec<String>) -> PlotIssue {
    PlotIssue { code: code.into(), severity, message, node_ids, edge_ids }
}

/// 无条件、非一次性边组成的强连通分量（会导致原地打转）。
fn unconditional_cycles(nodes: &[PlotNode], edges: &[PlotEdge]) -> Vec<Vec<String>> {
    let idx: HashMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
    let mut adj = vec![vec![]; nodes.len()];
    for e in edges.iter().filter(|e| e.condition.is_none() && !e.once) {
        if let (Some(&a), Some(&b)) = (idx.get(e.from_id.as_str()), idx.get(e.to_id.as_str())) {
            adj[a].push(b);
        }
    }
    // Tarjan 强连通分量（迭代版）。
    let n = nodes.len();
    let (mut index, mut low, mut on) = (vec![usize::MAX; n], vec![0; n], vec![false; n]);
    let (mut stack, mut out, mut counter) = (vec![], vec![], 0usize);
    for s in 0..n {
        if index[s] != usize::MAX {
            continue;
        }
        let mut work = vec![(s, 0usize)];
        while let Some(&mut (v, ref mut i)) = work.last_mut() {
            if *i == 0 && index[v] == usize::MAX {
                index[v] = counter;
                low[v] = counter;
                counter += 1;
                stack.push(v);
                on[v] = true;
            }
            if *i < adj[v].len() {
                let w = adj[v][*i];
                *i += 1;
                if index[w] == usize::MAX {
                    work.push((w, 0));
                } else if on[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                work.pop();
                if let Some(&(p, _)) = work.last() {
                    low[p] = low[p].min(low[v]);
                }
                if low[v] == index[v] {
                    let mut comp = vec![];
                    while let Some(w) = stack.pop() {
                        on[w] = false;
                        comp.push(nodes[w].id.clone());
                        if w == v {
                            break;
                        }
                    }
                    let self_loop = comp.len() == 1 && adj[v].contains(&v);
                    if comp.len() > 1 || self_loop {
                        out.push(comp);
                    }
                }
            }
        }
    }
    out
}

pub fn analyze_plot(conn: &Connection, script_id: &str) -> Result<PlotReport> {
    let script = require_script(conn, script_id)?;
    let nodes = plot_nodes_of(conn, script_id)?;
    let edges = edges_of_script(conn, script_id)?;
    let mut issues = vec![];
    let title = |id: &str| nodes.iter().find(|n| n.id == id).map(|n| n.title.clone()).unwrap_or_else(|| id.to_string());

    let starts: Vec<&PlotNode> = nodes.iter().filter(|n| n.kind == PlotNodeKind::Start).collect();
    match starts.len() {
        0 if !nodes.is_empty() => issues.push(issue("no_start", Severity::Error, "剧情图没有开场节点".into(), vec![], vec![])),
        0 => {}
        1 => {}
        _ => issues.push(issue(
            "multiple_starts",
            Severity::Warning,
            format!("有 {} 个开场节点，战役会无法自动确定起点", starts.len()),
            starts.iter().map(|n| n.id.clone()).collect(),
            vec![],
        )),
    }

    let mut out_edges: HashMap<&str, Vec<&PlotEdge>> = HashMap::new();
    for e in &edges {
        out_edges.entry(e.from_id.as_str()).or_default().push(e);
    }
    // 可达性（不考虑条件）。
    let mut reach: HashSet<String> = HashSet::new();
    let mut q: VecDeque<String> = starts.iter().map(|n| n.id.clone()).collect();
    while let Some(id) = q.pop_front() {
        if !reach.insert(id.clone()) {
            continue;
        }
        for e in out_edges.get(id.as_str()).into_iter().flatten() {
            q.push_back(e.to_id.clone());
        }
    }
    if !starts.is_empty() {
        let unreachable: Vec<String> = nodes.iter().filter(|n| !reach.contains(&n.id)).map(|n| n.id.clone()).collect();
        if !unreachable.is_empty() {
            issues.push(issue(
                "unreachable",
                Severity::Warning,
                format!("从开场无法到达：{}", unreachable.iter().map(|i| title(i)).collect::<Vec<_>>().join("、")),
                unreachable,
                vec![],
            ));
        }
    }
    // 死路：不是结局、也不是自由行动节点，却没有出边。
    for n in &nodes {
        if !matches!(n.kind, PlotNodeKind::Ending | PlotNodeKind::Dynamic) && !out_edges.contains_key(n.id.as_str()) {
            issues.push(issue("dead_end", Severity::Warning, format!("「{}」没有后续分支，玩家会卡住", n.title), vec![n.id.clone()], vec![]));
        }
    }
    let endings: Vec<&PlotNode> = nodes.iter().filter(|n| n.kind == PlotNodeKind::Ending).collect();
    let reachable_endings = endings.iter().filter(|n| reach.contains(&n.id)).count();
    if !nodes.is_empty() && !starts.is_empty() && reachable_endings == 0 {
        issues.push(issue("no_reachable_ending", Severity::Error, "从开场出发到达不了任何结局".into(), vec![], vec![]));
    }
    if let Ok(scale) = serde_json::from_value::<ScaleProfile>(script.scale_profile.clone())
        && !nodes.is_empty()
        && (endings.len() as u32) < scale.endings.min
    {
        issues.push(issue(
            "few_endings",
            Severity::Warning,
            format!("结局只有 {} 个，按人数与时长建议至少 {} 个", endings.len(), scale.endings.min),
            vec![],
            vec![],
        ));
    }

    // 引用校验：条件与效果中的任务、线索、物品、剧情节点、实体必须存在。
    let items: HashMap<String, ScriptItemKind> = {
        let mut stmt = conn.prepare_cached("SELECT id, kind FROM script_items WHERE script_id = ?1 AND deleted_at IS NULL")?;
        let rows = stmt.query_map([script_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.filter_map(|r| r.ok()).filter_map(|(id, k)| ScriptItemKind::parse(&k).ok().map(|k| (id, k))).collect()
    };
    let node_ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let world_exists = |id: &str| Node::load_active(conn, id).ok().flatten().is_some();
    for e in &edges {
        let mut refs = vec![];
        if let Some(c) = &e.condition {
            c.referenced_ids(&mut refs);
        }
        for eff in &e.effects {
            eff.referenced_ids(&mut refs);
        }
        for (cat, id) in refs {
            let ok = match cat {
                "quest" => items.get(&id) == Some(&ScriptItemKind::Quest),
                "clue" => items.get(&id) == Some(&ScriptItemKind::Clue),
                "item" => items.get(&id) == Some(&ScriptItemKind::Item) || world_exists(&id),
                "plot_node" => node_ids.contains(id.as_str()),
                _ => id == "party" || world_exists(&id) || items.contains_key(&id),
            };
            if !ok {
                issues.push(issue(
                    "bad_reference",
                    Severity::Warning,
                    format!("分支「{} → {}」引用了不存在的{}：{id}", title(&e.from_id), title(&e.to_id), match cat {
                        "quest" => "任务",
                        "clue" => "线索",
                        "item" => "物品",
                        "plot_node" => "剧情节点",
                        _ => "实体",
                    }),
                    vec![e.from_id.clone(), e.to_id.clone()],
                    vec![e.id.clone()],
                ));
            }
        }
    }

    for comp in unconditional_cycles(&nodes, &edges) {
        issues.push(issue(
            "unconditional_cycle",
            Severity::Warning,
            format!("这些节点之间只有无条件的边，可能原地打转：{}", comp.iter().map(|i| title(i)).collect::<Vec<_>>().join("、")),
            comp,
            vec![],
        ));
    }
    Ok(PlotReport { node_count: nodes.len(), edge_count: edges.len(), ending_count: endings.len(), reachable_endings, issues })
}

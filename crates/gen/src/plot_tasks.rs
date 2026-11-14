//! 剧情图生成：从剧本生成剧情分支图；按玩家行为（或无行为时）为节点生成新分支。
//! 模型输出的条件与效果必须能解析为规则 DSL，否则被移除并在审阅台提示。

use std::collections::{HashMap, HashSet};

use chronicle_core::changeset::{Issue, Mode, Op, Severity};
use chronicle_core::model::*;
use chronicle_core::rules::{Condition, Effect};
use chronicle_core::util::normalize_name;
use chronicle_core::{World, plot, scripts};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::context::{self, Context, RenderedContext};
use crate::templates::Templates;
use crate::{GenError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlotNodeOut {
    /// 节点编号，如 k1。
    pub key: String,
    /// start / scene / choice / event / ending / dynamic
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub player_text: String,
    #[serde(default)]
    pub gm_text: String,
    /// 对应的剧本条目编号（可选，如 S1）。
    #[serde(default)]
    pub script_item: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlotEdgeOut {
    pub from: String,
    pub to: String,
    pub label: String,
    /// 条件（规则 DSL 的 JSON 对象），可省略表示总是可走。
    #[serde(default)]
    pub condition: Option<Map<String, Value>>,
    /// 效果列表（规则 DSL 的 JSON 对象）。
    #[serde(default)]
    pub effects: Vec<Map<String, Value>>,
    #[serde(default)]
    pub priority: i64,
    #[serde(default)]
    pub once: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlotOut {
    pub nodes: Vec<PlotNodeOut>,
    pub edges: Vec<PlotEdgeOut>,
}

/// 写给模型看的条件与效果格式说明（与 chronicle-rules 的定义一致）。
pub const DSL_DOC: &str = r#"条件（condition，可嵌套）：
  {"type":"all","of":[...]}  全部满足    {"type":"any","of":[...]}  任一满足    {"type":"not","cond":{...}}
  {"type":"quest_state","quest":"<任务编号，如 Q1>","state":"active|done|failed"}
  {"type":"clue_known","clue":"<线索编号，如 C1>"}
  {"type":"has_item","item":"<物品编号，如 I1>","min_qty":1}
  {"type":"flag","name":"<标志名>"}
  {"type":"var","key":"<变量名>","op":"eq|ne|lt|le|gt|ge|exists","value":10}
  {"type":"relation","a":"party","b":"<人物编号，如 N1>","rel_type":"trusts"}
  {"type":"visited","node":"<新节点 key 或已有节点编号，如 P1>","min_count":1}
  {"type":"check","skill":"<技能>","dc":15}   ← 由程序掷 1d20+技能修正
效果（effects，按顺序执行）：
  {"type":"set_quest_state","quest":"<任务编号，如 Q1>","state":"active"}
  {"type":"reveal_clue","clue":"<线索编号，如 C1>"}
  {"type":"give_item","item":"<物品编号，如 I1>","qty":1}   {"type":"take_item","item":"<物品编号，如 I1>","qty":1}
  {"type":"set_flag","name":"<标志名>","value":true}
  {"type":"set_var","key":"<变量名>","value":...}   {"type":"inc_var","key":"<变量名>","by":1}
  {"type":"set_relation","a":"party","b":"<人物编号，如 N1>","rel_type":"trusts","active":true}
  {"type":"adjust_bond","from":"<人物编号，如 N1>","to":"party","trust":5,"affection":5,"hostility":0,"strength":3,"kind":"friend","reason":"<为什么变化，必填>"}   ← 关系变化，每项最多 ±15
  {"type":"set_entity_state","entity":"<人物编号，如 N1>","field":"life_status","value":"dead"}
  {"type":"unlock_node","node":"<新节点 key 或已有节点编号，如 P1>"}"#;

/// 给模型看的短编号（实测模型抄写 26 位 ID 会抄错：生成的分支引用了不存在的线索）。
/// 剧本条目：任务 Q、线索 C、物品 I、其余 S；剧本人物 N；已有剧情节点 P。
/// 编号由数据库中的顺序决定，生成提示词与转换结果时各算一次，结果相同。
pub struct Refs {
    /// 短编号 → 真实 ID
    pub map: HashMap<String, String>,
    /// 剧本条目 ID → 种类
    pub item_kinds: HashMap<String, ScriptItemKind>,
    /// 规范化标题 → 条目 ID（标题唯一时；用于模型用标题指代条目的情况）
    item_titles: HashMap<String, String>,
    pub cast_ids: HashSet<String>,
    pub items_text: String,
    pub cast_text: String,
    pub nodes_text: String,
    /// 剧情节点 ID → 短编号
    pub node_short: HashMap<String, String>,
}

pub fn refs(world: &World, script_id: &str) -> Result<Refs> {
    let mut r = Refs {
        map: HashMap::new(),
        item_kinds: HashMap::new(),
        item_titles: HashMap::new(),
        cast_ids: HashSet::new(),
        items_text: String::new(),
        cast_text: String::new(),
        nodes_text: String::new(),
        node_short: HashMap::new(),
    };
    fn walk(world: &World, sid: &str, parent: Option<&str>, depth: usize, out: &mut Vec<(usize, ScriptItem)>) -> Result<()> {
        for it in scripts::item_children(world.conn(), sid, parent)? {
            let id = it.id.clone();
            out.push((depth, it));
            walk(world, sid, Some(&id), depth + 1, out)?;
        }
        Ok(())
    }
    let mut items = vec![];
    walk(world, script_id, None, 0, &mut items)?;
    let mut counters: HashMap<char, usize> = HashMap::new();
    let mut lines = vec![];
    let mut dup_titles: HashSet<String> = HashSet::new();
    for (depth, it) in items {
        let prefix = match it.kind {
            ScriptItemKind::Quest => 'Q',
            ScriptItemKind::Clue => 'C',
            ScriptItemKind::Item => 'I',
            _ => 'S',
        };
        let n = counters.entry(prefix).or_insert(0);
        *n += 1;
        let short = format!("{prefix}{n}");
        let brief: String = it.player_text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(60).collect();
        let brief = if brief.is_empty() { String::new() } else { format!("：{brief}") };
        lines.push(format!("{}- {short} [{}] {}{brief}", "  ".repeat(depth), it.kind, it.title));
        let nt = normalize_name(&it.title);
        if r.item_titles.insert(nt.clone(), it.id.clone()).is_some() {
            dup_titles.insert(nt);
        }
        r.item_kinds.insert(it.id.clone(), it.kind);
        r.map.insert(short, it.id);
    }
    for t in dup_titles {
        r.item_titles.remove(&t);
    }
    r.items_text = lines.join("\n");
    let mut cl = vec![];
    for (i, c) in scripts::cast_of(world.conn(), script_id)?.into_iter().enumerate() {
        if let Some(n) = chronicle_core::nodes::get_node(world.conn(), &c.node_id)? {
            let short = format!("N{}", i + 1);
            cl.push(format!("- {short} {} {}", n.title, c.role));
            r.cast_ids.insert(n.id.clone());
            r.map.insert(short, n.id);
        }
    }
    r.cast_text = cl.join("\n");
    let mut nl = vec![];
    for (i, n) in plot::plot_nodes_of(world.conn(), script_id)?.into_iter().enumerate() {
        let short = format!("P{}", i + 1);
        nl.push(format!("- {short} [{}] {}", n.kind, n.title));
        r.node_short.insert(n.id.clone(), short.clone());
        r.map.insert(short, n.id);
    }
    r.nodes_text = nl.join("\n");
    Ok(r)
}

impl Refs {
    /// 短编号（或唯一的条目标题）→ 真实 ID。
    fn lookup(&self, v: &str, allow_title: bool) -> Option<String> {
        let t = v.trim();
        if let Some(id) = self.map.get(&t.to_uppercase()) {
            return Some(id.clone());
        }
        if allow_title {
            return self.item_titles.get(&normalize_name(t)).cloned();
        }
        None
    }

    /// 把 DSL 里的短编号换成真实 ID（任务/线索/物品/人物/剧情节点字段）。
    fn map_value(&self, v: &mut Value) {
        match v {
            Value::Object(o) => {
                for (k, x) in o.iter_mut() {
                    if let Value::String(sv) = x {
                        let allow_title = matches!(k.as_str(), "quest" | "clue" | "item");
                        if matches!(k.as_str(), "quest" | "clue" | "item" | "a" | "b" | "entity" | "node" | "holder" | "from" | "to")
                            && let Some(id) = self.lookup(sv, allow_title)
                        {
                            *sv = id;
                        }
                    } else {
                        self.map_value(x);
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| self.map_value(x)),
            _ => {}
        }
    }

    /// 映射后仍然无效的引用（不存在，或种类不对，如把任务当线索）。
    fn bad_refs(&self, world: &World, v: &Value, new_nodes: &HashSet<String>, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                for (k, x) in o {
                    let want = match k.as_str() {
                        "quest" => Some((ScriptItemKind::Quest, "任务")),
                        "clue" => Some((ScriptItemKind::Clue, "线索")),
                        "item" => Some((ScriptItemKind::Item, "物品")),
                        _ => None,
                    };
                    match (k.as_str(), x, want) {
                        (_, Value::String(id), Some((kind, label))) => {
                            if self.item_kinds.get(id) != Some(&kind) {
                                out.push(format!("{label}「{id}」"));
                            }
                        }
                        ("a" | "b" | "entity" | "from" | "to", Value::String(id), None) => {
                            if id != "party" && !self.cast_ids.contains(id) && chronicle_core::nodes::get_node(world.conn(), id).ok().flatten().is_none() {
                                out.push(format!("人物「{id}」"));
                            }
                        }
                        ("node", Value::String(id), None) => {
                            if !new_nodes.contains(id) && !self.node_short.contains_key(id) {
                                out.push(format!("剧情节点「{id}」"));
                            }
                        }
                        _ => self.bad_refs(world, x, new_nodes, out),
                    }
                }
            }
            Value::Array(a) => a.iter().for_each(|x| self.bad_refs(world, x, new_nodes, out)),
            _ => {}
        }
    }
}

fn base_context(world: &World, s: &Script, refs: &Refs, ctx: &mut Context) -> Result<()> {
    ctx.push("剧本信息", context::script_meta(s), 0);
    ctx.push("世界核心设定（不得矛盾）", context::core_canon(world, 30)?, 1);
    ctx.push("剧本条目（引用时填编号：任务 Q、线索 C、物品 I、其他条目 S）", refs.items_text.clone(), 0);
    ctx.push("剧本人物（引用时填编号 N）", refs.cast_text.clone(), 1);
    Ok(())
}

pub fn build_from_script(world: &World, templates: &Templates, budget: usize, script_id: &str, guidance: &str) -> Result<(String, RenderedContext)> {
    let s = scripts::require_script(world.conn(), script_id)?;
    let r = refs(world, script_id)?;
    let mut ctx = Context::default();
    base_context(world, &s, &r, &mut ctx)?;
    ctx.push("已有剧情节点（可以连接，填编号 P，不要重复）", r.nodes_text.clone(), 2);
    let rendered = ctx.render(budget);
    let user = templates.render("plot_from_script", json!({"guidance": guidance, "dsl": DSL_DOC, "context": rendered.text}))?;
    Ok((user, rendered))
}

pub fn build_branches(world: &World, templates: &Templates, budget: usize, script_id: &str, from: &str, action: &str, count: u32) -> Result<(String, RenderedContext)> {
    let s = scripts::require_script(world.conn(), script_id)?;
    let node = plot::require_plot_node(world.conn(), from)?;
    if node.script_id != s.id {
        return Err(GenError::Input("剧情节点不属于该剧本".into()));
    }
    if node.kind == PlotNodeKind::Ending {
        return Err(GenError::Input("结局节点不能再有分支".into()));
    }
    let r = refs(world, script_id)?;
    let mut ctx = Context::default();
    base_context(world, &s, &r, &mut ctx)?;
    ctx.push("已有剧情节点（可以作为 to 汇回主线，填编号 P）", r.nodes_text.clone(), 1);
    let rendered = ctx.render(budget);
    let existing: Vec<String> = plot::outgoing(world.conn(), from)?
        .iter()
        .map(|e| {
            let to = plot::get_plot_node(world.conn(), &e.to_id).ok().flatten().map(|n| n.title).unwrap_or_default();
            format!("- 「{}」→ {}", e.label, to)
        })
        .collect();
    let short = r.node_short.get(&node.id).cloned().unwrap_or_default();
    let target = format!("{short} [{}] {}（新分支的 from 填 {short}）\n玩家可见：{}\n主持人：{}", node.kind, node.title, node.player_text, node.gm_text);
    let user = templates.render(
        "plot_branches",
        json!({"action": action, "count": count.clamp(1, 6), "target": target,
               "existing": if existing.is_empty() { "（无）".to_string() } else { existing.join("\n") },
               "dsl": DSL_DOC, "context": rendered.text}),
    )?;
    Ok((user, rendered))
}

fn note(op: usize, code: &str, message: String) -> Issue {
    Issue { op, severity: Severity::Warning, code: code.into(), message, related: vec![] }
}

/// 把 DSL 中引用剧情节点 key 的字段换成临时 ID。
fn map_node_refs(v: &mut Value, keys: &HashMap<String, String>) {
    match v {
        Value::Object(o) => {
            let ty = o.get("type").and_then(Value::as_str).map(str::to_string);
            if matches!(ty.as_deref(), Some("visited") | Some("unlock_node"))
                && let Some(Value::String(n)) = o.get_mut("node")
                && let Some(t) = keys.get(n.as_str())
            {
                *n = t.clone();
            }
            for x in o.values_mut() {
                map_node_refs(x, keys);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| map_node_refs(x, keys)),
        _ => {}
    }
}

struct Conv<'a> {
    world: &'a World,
    script_id: String,
    ops: Vec<Op>,
    notes: Vec<Issue>,
    keys: HashMap<String, String>,
    /// 本次新节点：规范化标题 → 临时 ID（标题唯一时）。
    titles: HashMap<String, String>,
    refs: Refs,
}

impl Conv<'_> {
    fn nodes(&mut self, out: &[PlotNodeOut], default_kind: PlotNodeKind, allow_start: bool) -> Result<()> {
        for (i, n) in out.iter().enumerate() {
            let key = n.key.trim().to_string();
            if key.is_empty() || self.keys.contains_key(&key) {
                self.notes.push(note(self.ops.len(), "bad_key", format!("节点「{}」的编号「{}」为空或重复，已忽略", n.title, n.key)));
                continue;
            }
            let mut kind = PlotNodeKind::parse(n.kind.trim()).unwrap_or(default_kind);
            if kind == PlotNodeKind::Start && !allow_start {
                kind = default_kind;
            }
            let mut pn = NewPlotNode::new(&self.script_id, kind, n.title.trim());
            pn.player_text = n.player_text.clone();
            pn.gm_text = n.gm_text.clone();
            if let Some(raw) = n.script_item.as_ref().filter(|s| !s.trim().is_empty()) {
                let it = &self.refs.lookup(raw, true).unwrap_or_else(|| raw.trim().to_string());
                if self.refs.item_kinds.contains_key(it) {
                    pn.script_item_ref = Some(it.clone());
                } else if plot::get_plot_node(self.world.conn(), it).ok().flatten().is_some() {
                    // 实测模型偶尔把剧情节点 ID 填进这里；不是剧本条目，直接忽略，不必提示。
                } else {
                    self.notes.push(note(self.ops.len(), "bad_item_ref", format!("节点「{}」引用的剧本条目 {it} 不存在，已忽略该引用", n.title)));
                }
            }
            let t = format!("tmp:p{i}");
            self.keys.insert(key, t.clone());
            let nt = normalize_name(n.title.trim());
            // 标题重复时记为空字符串，表示不能按标题解析。
            self.titles.entry(nt).and_modify(|v| v.clear()).or_insert_with(|| t.clone());
            self.ops.push(Op::CreatePlotNode { temp_id: Some(t), node: pn });
        }
        Ok(())
    }

    /// 按 key、已有节点 ID、最后按标题解析（实测模型有时用标题指代节点）。
    fn resolve(&self, r: &str) -> Option<String> {
        let r = r.trim();
        if let Some(t) = self.keys.get(r) {
            return Some(t.clone());
        }
        if let Some(id) = self.refs.map.get(&r.to_uppercase()).filter(|id| self.refs.node_short.contains_key(*id)) {
            return Some(id.clone());
        }
        if let Some(n) = plot::get_plot_node(self.world.conn(), r).ok().flatten().filter(|n| n.script_id == self.script_id) {
            return Some(n.id);
        }
        let want = normalize_name(r);
        if let Some(t) = self.titles.get(&want).filter(|t| !t.is_empty()) {
            return Some(t.clone());
        }
        let matches: Vec<PlotNode> = plot::plot_nodes_of(self.world.conn(), &self.script_id)
            .ok()?
            .into_iter()
            .filter(|n| normalize_name(&n.title) == want)
            .collect();
        (matches.len() == 1).then(|| matches[0].id.clone())
    }

    fn edges(&mut self, out: &[PlotEdgeOut], allowed_from: Option<&HashSet<String>>) {
        for e in out {
            let (Some(from), Some(to)) = (self.resolve(&e.from), self.resolve(&e.to)) else {
                self.notes.push(note(self.ops.len(), "unresolved_edge", format!("分支「{}」（{} → {}）的端点不存在，已忽略", e.label, e.from, e.to)));
                continue;
            };
            if let Some(allowed) = allowed_from
                && !allowed.contains(&from)
            {
                self.notes.push(note(self.ops.len(), "edge_not_allowed", format!("分支「{}」不是从当前节点或新节点出发的，已忽略（不修改已有节点）", e.label)));
                continue;
            }
            let mut ed = NewPlotEdge::new(&self.script_id, &from, &to);
            ed.label = e.label.clone();
            ed.priority = e.priority;
            ed.once = e.once;
            let new_nodes: HashSet<String> = self.keys.values().cloned().collect();
            let mut bad = vec![];
            if let Some(c) = &e.condition {
                let mut v = Value::Object(c.clone());
                map_node_refs(&mut v, &self.keys);
                self.refs.map_value(&mut v);
                self.refs.bad_refs(self.world, &v, &new_nodes, &mut bad);
                let parsed = if bad.is_empty() {
                    serde_json::from_value::<Condition>(v).map_err(|x| x.to_string()).and_then(|c| c.validate().map(|_| c).map_err(|x| x.to_string()))
                } else {
                    // 条件引用了不存在的内容：写入前移除并提示。
                    Err(format!("引用了不存在的{}", bad.join("、")))
                };
                match parsed {
                    Ok(c) => ed.condition = Some(c),
                    Err(err) => self.notes.push(Issue {
                        op: self.ops.len(),
                        severity: Severity::Warning,
                        code: if bad.is_empty() { "bad_condition".into() } else { "bad_reference".into() },
                        message: format!("分支「{}」的条件无效，已移除（该分支将总是可走，请检查）：{err}", e.label),
                        related: vec![],
                    }),
                }
            }
            for eff in &e.effects {
                let mut v = Value::Object(eff.clone());
                map_node_refs(&mut v, &self.keys);
                self.refs.map_value(&mut v);
                let mut bad = vec![];
                self.refs.bad_refs(self.world, &v, &new_nodes, &mut bad);
                if !bad.is_empty() {
                    self.notes.push(note(self.ops.len(), "bad_reference", format!("分支「{}」有一条效果引用了不存在的{}，已移除", e.label, bad.join("、"))));
                    continue;
                }
                match serde_json::from_value::<Effect>(v).map_err(|x| x.to_string()).and_then(|f| f.validate().map(|_| f).map_err(|x| x.to_string())) {
                    Ok(f) => ed.effects.push(f),
                    Err(err) => self.notes.push(note(self.ops.len(), "bad_effect", format!("分支「{}」有一条效果格式不对，已移除：{err}", e.label))),
                }
            }
            self.ops.push(Op::CreatePlotEdge { temp_id: None, edge: ed });
        }
    }
}

/// 审阅前的结构检查：把提议的节点与边与已有剧情图合在一起，看开场能否到达结局、新节点是否可达、有没有死路。
/// 实测模型生成的图曾出现"开场没有出边、所有结局都到不了"，必须在写入前让审阅者看到。
fn structure_notes(world: &World, script_id: &str, ops: &[Op], entry: Option<&str>) -> Result<Vec<Issue>> {
    let mut kinds: HashMap<String, (PlotNodeKind, String, bool)> = HashMap::new();
    for n in plot::plot_nodes_of(world.conn(), script_id)? {
        kinds.insert(n.id.clone(), (n.kind, n.title.clone(), false));
    }
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for e in plot::edges_of_script(world.conn(), script_id)? {
        adj.entry(e.from_id).or_default().push(e.to_id);
    }
    for op in ops {
        match op {
            Op::CreatePlotNode { temp_id: Some(t), node } => {
                kinds.insert(t.clone(), (node.kind, node.title.clone(), true));
            }
            Op::CreatePlotEdge { edge, .. } => adj.entry(edge.from_id.clone()).or_default().push(edge.to_id.clone()),
            _ => {}
        }
    }
    let starts: Vec<String> = match entry {
        Some(e) => vec![e.to_string()],
        None => kinds.iter().filter(|(_, (k, _, _))| *k == PlotNodeKind::Start).map(|(id, _)| id.clone()).collect(),
    };
    let mut reach: HashSet<String> = HashSet::new();
    let mut q: Vec<String> = starts.clone();
    while let Some(id) = q.pop() {
        if reach.insert(id.clone()) {
            q.extend(adj.get(&id).cloned().unwrap_or_default());
        }
    }
    let mut notes = vec![];
    let warn = |code: &str, msg: String| Issue { op: 0, severity: Severity::Warning, code: code.into(), message: msg, related: vec![] };
    if entry.is_none() {
        if starts.is_empty() {
            notes.push(warn("structure_no_start", "写入后剧情图没有开场节点".into()));
        } else if !kinds.iter().any(|(id, (k, _, _))| *k == PlotNodeKind::Ending && reach.contains(id)) {
            notes.push(warn("structure_no_ending", "写入后从开场出发到达不了任何结局，请检查分支是否连接完整".into()));
        }
    }
    let unreachable: Vec<&String> = kinds.iter().filter(|(id, (_, _, new))| *new && !reach.contains(*id)).map(|(_, (_, t, _))| t).collect();
    if !unreachable.is_empty() {
        notes.push(warn("structure_unreachable", format!("这些新节点无法到达：{}", unreachable.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("、"))));
    }
    let dead: Vec<&String> = kinds
        .iter()
        .filter(|(id, (k, _, new))| *new && !matches!(k, PlotNodeKind::Ending | PlotNodeKind::Dynamic) && adj.get(*id).is_none_or(|v| v.is_empty()))
        .map(|(_, (_, t, _))| t)
        .collect();
    if !dead.is_empty() {
        notes.push(warn("structure_dead_end", format!("这些新节点没有后续分支（不是结局也不是自由行动）：{}", dead.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("、"))));
    }
    Ok(notes)
}

pub fn convert_from_script(world: &World, script_id: &str, v: &Value) -> Result<(Vec<Op>, Vec<Issue>, Mode, String)> {
    let out: PlotOut = serde_json::from_value(v.clone())?;
    let s = scripts::require_script(world.conn(), script_id)?;
    let has_start = plot::plot_nodes_of(world.conn(), script_id)?.iter().any(|n| n.kind == PlotNodeKind::Start);
    let mut c = Conv { world, script_id: script_id.into(), ops: vec![], notes: vec![], keys: HashMap::new(), titles: HashMap::new(), refs: refs(world, script_id)? };
    if has_start {
        c.notes.push(note(0, "existing_start", "剧本已有开场节点，新生成的开场节点改为普通场景".into()));
    }
    c.nodes(&out.nodes, PlotNodeKind::Scene, !has_start)?;
    c.edges(&out.edges, None);
    let starts = out.nodes.iter().filter(|n| n.kind.trim() == "start").count();
    if !has_start && starts != 1 {
        c.notes.push(note(0, "start_count", format!("模型给出了 {starts} 个开场节点（应为 1 个），请在审阅时检查")));
    }
    c.notes.extend(structure_notes(world, script_id, &c.ops, None)?);
    Ok((c.ops, c.notes, Mode::Append, format!("剧情分支图：{}", s.title)))
}

pub fn convert_branches(world: &World, script_id: &str, from: &str, v: &Value) -> Result<(Vec<Op>, Vec<Issue>, Mode, String)> {
    let out: PlotOut = serde_json::from_value(v.clone())?;
    let node = plot::require_plot_node(world.conn(), from)?;
    let mut c = Conv { world, script_id: script_id.into(), ops: vec![], notes: vec![], keys: HashMap::new(), titles: HashMap::new(), refs: refs(world, script_id)? };
    c.nodes(&out.nodes, PlotNodeKind::Dynamic, false)?;
    let mut allowed: HashSet<String> = c.keys.values().cloned().collect();
    allowed.insert(node.id.clone());
    c.edges(&out.edges, Some(&allowed));
    let from_current = c.ops.iter().any(|o| matches!(o, Op::CreatePlotEdge { edge, .. } if edge.from_id == node.id));
    if !from_current {
        c.notes.push(note(0, "no_branch_from_current", "没有从当前节点出发的新分支，新节点将无法到达".into()));
    } else {
        c.notes.extend(structure_notes(world, script_id, &c.ops, Some(&node.id))?.into_iter().filter(|n| n.code != "structure_no_ending"));
    }
    Ok((c.ops, c.notes, Mode::Append, format!("新分支：{}", node.title)))
}

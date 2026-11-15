//! 世界树生成：一句话生成世界、为节点续生子节点、扩写/重写/补全节点。

use std::collections::HashMap;

use chronicle_core::changeset::{Issue, Mode, Op, Severity};
use chronicle_core::model::*;
use chronicle_core::payload::validate_node_payload;
use chronicle_core::relations::RELATION_TYPES;
use chronicle_core::util::normalize_name;
use chronicle_core::{World, nodes};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::context::{self, Context, RenderedContext};
use crate::templates::Templates;
use crate::{GenError, Result};

/// 世界树第一层分类（名称 → 默认节点类型）。
pub const CATEGORIES: &[(&str, NodeKind)] = &[
    ("宇宙与世界背景", NodeKind::Cosmos),
    ("历史与时间线", NodeKind::Era),
    ("地理区域", NodeKind::Region),
    ("势力与组织", NodeKind::Faction),
    ("种族与文明", NodeKind::Race),
    ("人物与NPC", NodeKind::Character),
    ("文化与社会", NodeKind::Culture),
    ("科技与魔法体系", NodeKind::MagicSystem),
    ("重要事件与传说", NodeKind::Event),
];

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorldLeafOut {
    pub name: String,
    /// 节点类型，例如 city、character、faction。
    pub kind: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub player_text: String,
    #[serde(default)]
    pub gm_text: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// 结构化资料。
    #[serde(default)]
    pub details: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorldNodeOut {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub player_text: String,
    #[serde(default)]
    pub gm_text: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub details: Map<String, Value>,
    #[serde(default)]
    pub children: Vec<WorldLeafOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RelationOut {
    /// 起点节点名称。
    pub from: String,
    /// 终点节点名称。
    pub to: String,
    pub rel_type: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SectionOut {
    pub category: String,
    pub nodes: Vec<WorldNodeOut>,
}

/// 一句话生成世界。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorldSeedOut {
    pub name: String,
    pub summary: String,
    pub world_rules: Vec<String>,
    pub sections: Vec<SectionOut>,
    pub relations: Vec<RelationOut>,
}

/// 为节点续生子节点。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExpansionOut {
    pub nodes: Vec<WorldNodeOut>,
    pub relations: Vec<RelationOut>,
}

/// 扩写 / 重写 / 补全一个节点。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NodeRewriteOut {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub player_text: String,
    #[serde(default)]
    pub gm_text: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub details: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RewriteMode {
    /// 重写：替换文本。
    Rewrite,
    /// 扩写：保留已有信息并补充细节。
    Expand,
    /// 补全：只填写空白字段。
    Complete,
}

fn kinds_text() -> String {
    NodeKind::ALL.iter().filter(|k| !matches!(k, NodeKind::World | NodeKind::Category)).map(|k| k.as_str()).collect::<Vec<_>>().join("、")
}

fn rel_text() -> String {
    RELATION_TYPES.iter().map(|t| format!("{}（{}）", t.id, t.label)).collect::<Vec<_>>().join("、")
}

/// 某类节点结构化资料的字段说明（取自 payload 的 JSON Schema）。
pub fn payload_fields(kind: NodeKind) -> String {
    let schema = chronicle_core::payload::node_payload_schema(kind);
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return "任意（根据节点内容自定）".into();
    };
    props
        .iter()
        .map(|(k, v)| {
            let ty = match v.get("type") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).filter(|s| *s != "null").collect::<Vec<_>>().join("/"),
                _ => "any".into(),
            };
            let desc = v.get("description").and_then(Value::as_str).map(|d| format!("：{d}")).unwrap_or_default();
            format!("{k}（{ty}{desc}）")
        })
        .collect::<Vec<_>>()
        .join("、")
}

fn protected(n: &Node) -> bool {
    n.locked || n.canon_level == CanonLevel::Core
}

/// 节点及其上下文：祖先链、已有子节点、同级、相关节点。
fn node_context(world: &World, n: &Node, ctx: &mut Context) -> Result<()> {
    let chain: Vec<String> = nodes::ancestors(world.conn(), &n.id)?
        .iter()
        .map(|a| format!("- [{}] {}：{}", a.kind, a.title, a.summary.chars().take(120).collect::<String>()))
        .collect();
    ctx.push("祖先节点（从根到父）", chain.join("\n"), 0);
    ctx.push("世界核心设定（不得矛盾）", context::core_canon(world, 40)?, 1);
    let kids: Vec<String> = nodes::children(world.conn(), Some(&n.id), 0, 200)?.iter().map(|c| c.title.clone()).collect();
    if !kids.is_empty() {
        ctx.push("已有子节点（不要重复）", kids.join("、"), 0);
    }
    let sibs: Vec<String> = nodes::children(world.conn(), n.parent_id.as_deref(), 0, 100)?
        .iter()
        .filter(|s| s.id != n.id)
        .map(|s| s.title.clone())
        .collect();
    ctx.push("同级节点", sibs.join("、"), 3);
    let mut rel_lines = vec![];
    for r in nodes::relations_of(world.conn(), &n.id)?.into_iter().take(40) {
        let other = if r.src_id == n.id { &r.dst_id } else { &r.src_id };
        if let Some(o) = nodes::get_node(world.conn(), other)? {
            rel_lines.push(format!("- {} {} {}：{}", if r.src_id == n.id { "→" } else { "←" }, r.rel_type, o.title, o.summary.chars().take(60).collect::<String>()));
        }
    }
    ctx.push("相关节点", rel_lines.join("\n"), 2);
    ctx.push(
        "已有名称",
        context::world_names(world, &[NodeKind::Character, NodeKind::Faction, NodeKind::Nation, NodeKind::City, NodeKind::Site, NodeKind::Region], 40)?,
        4,
    );
    Ok(())
}

/// 一句话生成世界的输入。
pub struct SeedInput<'a> {
    pub idea: &'a str,
    pub genre: &'a str,
    pub era: &'a str,
    pub style: &'a str,
    pub rules: &'a str,
}

/// 根据剧本生成世界树：剧本信息、核心设定、大纲、人物、场景里出现的地点作为依据；已有的世界树节点不要重复。
pub fn build_world_from_script(world: &World, templates: &Templates, budget: usize, script_id: &str, guidance: &str) -> Result<(String, RenderedContext)> {
    let s = chronicle_core::scripts::require_script(world.conn(), script_id)?;
    let mut ctx = Context::default();
    ctx.push("剧本信息", context::script_meta(&s), 0);
    ctx.push("剧本的核心设定与真相（主持人信息，可以写进世界设定，但不要写进玩家可见的文字）", s.core_setting.clone(), 0);
    ctx.push("剧本大纲", context::script_outline(world, script_id, true)?, 1);
    ctx.push("剧本人物（已经在世界树里，不要重复创建，可以在 relations 里引用）", context::cast_text(world, script_id)?, 1);
    ctx.push("世界核心设定（不得矛盾）", context::core_canon(world, 50)?, 1);
    let top: Vec<String> = nodes::children(world.conn(), None, 0, 50)?
        .iter()
        .flat_map(|r| {
            let mut v = vec![format!("- {}", r.title)];
            if let Ok(kids) = nodes::children(world.conn(), Some(&r.id), 0, 50) {
                v.extend(kids.iter().map(|k| format!("  - {}", k.title)));
            }
            v
        })
        .collect();
    ctx.push("已有世界树（不要重复）", top.join("\n"), 2);
    let rendered = ctx.render(budget);
    let cats = CATEGORIES.iter().map(|(c, _)| *c).collect::<Vec<_>>().join("、");
    let idea = format!("为剧本《{}》建立它所在的世界。{}", s.title, s.premise);
    let rules = if guidance.trim().is_empty() {
        "世界树必须服务于这个剧本：剧本里出现或提到的地点、势力、组织、物品、历史事件都要有对应节点，不要写与剧本无关的大段设定".to_string()
    } else {
        format!("世界树必须服务于这个剧本。{guidance}")
    };
    let fields = chronicle_core::profile::for_script(&s).all_node_fields_hint();
    let rules = if fields.is_empty() { rules } else { format!("{rules}。按这个题材，details 里还要写这些字段：{fields}") };
    let user = templates.render(
        "world_seed",
        json!({"idea": idea, "genre": s.genre, "era": "", "style": s.tone, "rules": rules, "categories": cats,
               "kinds": kinds_text(), "rel_types": rel_text(), "context": rendered.text}),
    )?;
    Ok((user, rendered))
}

pub fn build_world_seed(world: &World, templates: &Templates, budget: usize, input: SeedInput) -> Result<(String, RenderedContext)> {
    let SeedInput { idea, genre, era, style, rules } = input;
    if idea.trim().is_empty() {
        return Err(GenError::Input("请填写一句话创意".into()));
    }
    let mut ctx = Context::default();
    ctx.push("世界核心设定（不得矛盾）", context::core_canon(world, 50)?, 1);
    let top: Vec<String> = nodes::children(world.conn(), None, 0, 50)?
        .iter()
        .flat_map(|r| {
            let mut v = vec![format!("- {}", r.title)];
            if let Ok(kids) = nodes::children(world.conn(), Some(&r.id), 0, 50) {
                v.extend(kids.iter().map(|k| format!("  - {}", k.title)));
            }
            v
        })
        .collect();
    ctx.push("已有世界树", top.join("\n"), 2);
    let rendered = ctx.render(budget);
    let cats = CATEGORIES.iter().map(|(c, _)| *c).collect::<Vec<_>>().join("、");
    let fields = chronicle_core::profile::infer(genre, style, idea).all_node_fields_hint();
    let rules = if fields.is_empty() { rules.to_string() } else { format!("{rules}{}按这个题材，details 里还要写这些字段：{fields}", if rules.trim().is_empty() { "" } else { "。" }) };
    let user = templates.render(
        "world_seed",
        json!({"idea": idea, "genre": genre, "era": era, "style": style, "rules": rules, "categories": cats,
               "kinds": kinds_text(), "rel_types": rel_text(), "context": rendered.text}),
    )?;
    Ok((user, rendered))
}

pub fn build_expand(world: &World, templates: &Templates, budget: usize, node_id: &str, count: u32, kinds: &[NodeKind], guidance: &str) -> Result<(String, RenderedContext)> {
    let n = nodes::require_node(world.conn(), node_id)?;
    let mut ctx = Context::default();
    node_context(world, &n, &mut ctx)?;
    let rendered = ctx.render(budget);
    let kinds: Vec<&str> = kinds.iter().map(|k| k.as_str()).collect();
    let fields = chronicle_core::profile::world_profile(world.conn()).map(|p| p.all_node_fields_hint()).unwrap_or_default();
    let guidance = if fields.is_empty() { guidance.to_string() } else { format!("{guidance}（按这个世界的题材，details 里还要写：{fields}）") };
    let user = templates.render(
        "expand_node",
        json!({"target": context::node_detail(&n), "count": count.clamp(1, 20), "kinds": kinds.join("、"),
               "all_kinds": kinds_text(), "rel_types": rel_text(), "guidance": guidance, "context": rendered.text}),
    )?;
    Ok((user, rendered))
}

pub fn build_rewrite(world: &World, templates: &Templates, budget: usize, node_id: &str, mode: RewriteMode, instruction: &str) -> Result<(String, RenderedContext)> {
    let n = nodes::require_node(world.conn(), node_id)?;
    if protected(&n) {
        return Err(GenError::Input(format!("「{}」已锁定或属于核心设定，不能交给 AI 修改", n.title)));
    }
    let mut ctx = Context::default();
    node_context(world, &n, &mut ctx)?;
    let rendered = ctx.render(budget);
    let mode_s = match mode {
        RewriteMode::Rewrite => "rewrite",
        RewriteMode::Expand => "expand",
        RewriteMode::Complete => "complete",
    };
    // 可用字段：通用字段 + 按世界题材补充的字段。
    let extra = chronicle_core::profile::world_profile(world.conn()).map(|p| p.node_fields_hint(n.kind.as_str())).unwrap_or_default();
    let fields = if extra.is_empty() { payload_fields(n.kind) } else { format!("{}、{extra}", payload_fields(n.kind)) };
    let user = templates.render(
        "rewrite_node",
        json!({"target": context::node_detail(&n), "mode": mode_s, "instruction": instruction, "context": rendered.text,
               "fields": fields}),
    )?;
    Ok((user, rendered))
}

// ===================== 转换 =====================

struct Builder {
    ops: Vec<Op>,
    notes: Vec<Issue>,
    seq: usize,
    /// 规范化名称 → 节点 ID（临时 ID 或已有 ID），用于解析关系。
    names: HashMap<String, String>,
}

/// 简介为空时，取玩家可见文字的第一句（最多 60 字）作为简介。
pub(crate) fn summary_from(player_text: &str) -> Option<String> {
    let t = player_text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        return None;
    }
    let end = t.char_indices().find(|(_, c)| matches!(c, '。' | '！' | '？')).map(|(i, c)| i + c.len_utf8()).unwrap_or(t.len());
    Some(t[..end].chars().take(60).collect())
}

/// 结构化资料校验；失败时尝试把"应为数组却给了字符串"的字段包成单元素数组再校验
/// （实测：模型把 magic_system 的规则写成一句话而不是列表，整份资料因此被丢弃）。
pub(crate) fn coerce_payload(kind: NodeKind, payload: &Value) -> std::result::Result<Value, String> {
    let mut fixed = payload.clone();
    // serde 的报错会引用出错的字符串（invalid type: string "……", expected a sequence），据此找到字段。
    for _ in 0..8 {
        let err = match validate_node_payload(kind, &fixed) {
            Ok(()) => return Ok(fixed),
            Err(e) => e.to_string(),
        };
        if !err.contains("expected a sequence") {
            return Err(err);
        }
        let Some(o) = fixed.as_object_mut() else { return Err(err) };
        let key = o.iter().find(|(_, v)| v.as_str().is_some_and(|s| err.contains(&format!("\"{s}\"")))).map(|(k, _)| k.clone());
        let Some(k) = key else { return Err(err) };
        let v = o[&k].clone();
        o.insert(k, Value::Array(vec![v]));
    }
    validate_node_payload(kind, &fixed).map(|_| fixed).map_err(|e| e.to_string())
}

fn note(op: usize, code: &str, message: String) -> Issue {
    Issue { op, severity: Severity::Warning, code: code.into(), message, related: vec![] }
}

impl Builder {
    fn new() -> Self {
        Self { ops: vec![], notes: vec![], seq: 0, names: HashMap::new() }
    }
    fn tmp(&mut self) -> String {
        self.seq += 1;
        format!("tmp:n{}", self.seq)
    }
    // 参数与节点字段一一对应；拆成结构体反而让调用处更难读。
    #[allow(clippy::too_many_arguments)]
    fn node(&mut self, parent: &str, kind: NodeKind, title: &str, summary: &str, p: &str, g: &str, aliases: &[String], details: &Map<String, Value>, tags: Vec<String>) -> String {
        let t = self.tmp();
        let mut n = NewNode::new(kind, title.trim()).under(parent);
        n.summary = summary.trim().into();
        if n.summary.is_empty()
            && let Some(sm) = summary_from(p)
        {
            n.summary = sm;
            self.notes.push(note(self.ops.len(), "summary_derived", format!("「{}」没有简介，已取玩家可见描述的第一句代替，请检查", title.trim())));
        }
        n.player_text = p.into();
        n.gm_text = g.into();
        n.aliases = aliases.iter().filter(|a| !a.trim().is_empty() && a.as_str() != title).cloned().collect();
        n.tags = tags;
        let payload = Value::Object(details.clone());
        if !details.is_empty() {
            match coerce_payload(kind, &payload) {
                Ok(p) => n.payload = p,
                Err(e) => self.notes.push(note(self.ops.len(), "details_dropped", format!("「{title}」的结构化资料格式不对，已忽略：{e}"))),
            }
        }
        self.ops.push(Op::CreateNode { temp_id: Some(t.clone()), node: n });
        self.names.insert(normalize_name(title), t.clone());
        for a in aliases {
            self.names.entry(normalize_name(a)).or_insert_with(|| t.clone());
        }
        t
    }
    fn kind(&mut self, s: &str, fallback: NodeKind, title: &str) -> NodeKind {
        match NodeKind::parse(s.trim()) {
            Ok(NodeKind::World) | Ok(NodeKind::Category) | Err(_) => {
                if !s.trim().is_empty() && NodeKind::parse(s.trim()).is_err() {
                    self.notes.push(note(self.ops.len(), "unknown_kind", format!("「{title}」的类型「{s}」无效，改为 {fallback}")));
                }
                fallback
            }
            Ok(k) => k,
        }
    }
    fn tree(&mut self, parent: &str, n: &WorldNodeOut, fallback: NodeKind) {
        let k = self.kind(&n.kind, fallback, &n.name);
        let id = self.node(parent, k, &n.name, &n.summary, &n.player_text, &n.gm_text, &n.aliases, &n.details, vec![]);
        for c in &n.children {
            let ck = self.kind(&c.kind, k, &c.name);
            self.node(&id, ck, &c.name, &c.summary, &c.player_text, &c.gm_text, &c.aliases, &c.details, vec![]);
        }
    }
    fn resolve(&self, world: &World, name: &str) -> Result<Option<String>> {
        if let Some(id) = self.names.get(&normalize_name(name)) {
            return Ok(Some(id.clone()));
        }
        if let Some(n) = nodes::find_by_name(world.conn(), name, None)?.first() {
            return Ok(Some(n.id.clone()));
        }
        // 实测模型在关系里用简称（"维伦"指"大祭司维伦"）：本次新节点中唯一一个名称包含它（或被它包含）的就算匹配。
        let want = normalize_name(name);
        if want.chars().count() >= 2 {
            let hits: Vec<&String> = self
                .names
                .iter()
                .filter(|(k, _)| k.contains(want.as_str()) || (k.chars().count() >= 2 && want.contains(k.as_str())))
                .map(|(_, id)| id)
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            if hits.len() == 1 {
                return Ok(Some(hits[0].clone()));
            }
            if hits.is_empty() {
                // 已有世界节点里唯一一个标题包含该简称的（"薇拉娜"→"观星祭司 薇拉娜"）。
                let mut stmt = world.conn().prepare(
                    "SELECT id FROM nodes WHERE deleted_at IS NULL AND instr(title, ?1) > 0 LIMIT 2",
                )?;
                let ids: Vec<String> = stmt.query_map([name.trim()], |r| r.get(0))?.filter_map(|r| r.ok()).collect();
                if ids.len() == 1 {
                    return Ok(Some(ids[0].clone()));
                }
            }
        }
        Ok(None)
    }
    fn relations(&mut self, world: &World, rels: &[RelationOut]) -> Result<()> {
        for r in rels {
            match (self.resolve(world, &r.from)?, self.resolve(world, &r.to)?) {
                (Some(a), Some(b)) if a != b => {
                    let mut nr = NewRelation::new(&a, &b, r.rel_type.trim());
                    nr.label = r.label.clone();
                    if let Some(t) = chronicle_core::relations::relation_type(r.rel_type.trim()) {
                        nr.directed = t.directed;
                    }
                    self.ops.push(Op::CreateRelation { temp_id: None, relation: nr });
                }
                (Some(_), Some(_)) => self.notes.push(note(self.ops.len().saturating_sub(1), "self_relation", format!("关系「{} → {}」的两端是同一个节点，已忽略", r.from, r.to))),
                _ => self.notes.push(note(self.ops.len().saturating_sub(1), "unresolved_relation", format!("关系「{} → {}」中有节点找不到，已忽略", r.from, r.to))),
            }
        }
        Ok(())
    }
}

/// 找到或创建根节点下的分类。
fn category(world: &World, b: &mut Builder, root: &str, name: &str, cache: &mut HashMap<String, String>) -> Result<String> {
    if let Some(id) = cache.get(name) {
        return Ok(id.clone());
    }
    let existing = if root.starts_with("tmp:") {
        None
    } else {
        nodes::children(world.conn(), Some(root), 0, 500)?.into_iter().find(|n| normalize_name(&n.title) == normalize_name(name))
    };
    let id = match existing {
        Some(n) => n.id,
        None => {
            let t = b.tmp();
            b.ops.push(Op::CreateNode { temp_id: Some(t.clone()), node: NewNode::new(NodeKind::Category, name).under(root) });
            t
        }
    };
    cache.insert(name.into(), id.clone());
    Ok(id)
}

pub fn convert_world_seed(world: &World, v: &Value) -> Result<(Vec<Op>, Vec<Issue>, Mode, String)> {
    let out: WorldSeedOut = serde_json::from_value(v.clone())?;
    let mut b = Builder::new();
    // 已有世界根节点则在其下追加，否则新建。
    let root = match nodes::children(world.conn(), None, 0, 100)?.into_iter().find(|n| n.kind == NodeKind::World) {
        Some(r) => {
            b.notes.push(note(0, "existing_root", format!("在已有的世界根节点「{}」下追加内容", r.title)));
            r.id
        }
        None => {
            let t = b.tmp();
            let mut n = NewNode::new(NodeKind::World, out.name.trim());
            n.summary = out.summary.clone();
            b.ops.push(Op::CreateNode { temp_id: Some(t.clone()), node: n });
            t
        }
    };
    let mut cats = HashMap::new();
    if !out.world_rules.is_empty() {
        let cosmos = category(world, &mut b, &root, CATEGORIES[0].0, &mut cats)?;
        for (i, rule) in out.world_rules.iter().enumerate() {
            let title = format!("世界规则 {}", i + 1);
            b.node(&cosmos, NodeKind::Custom, &title, rule, rule, "", &[], &Map::new(), vec!["world_rule".into()]);
        }
        b.notes.push(note(0, "promote_rules", "世界规则由 AI 生成，默认是草稿。确认后可在世界树中把它们设为「核心设定」，此后 AI 将不能修改".into()));
    }
    for s in &out.sections {
        let (cat_name, fallback) = CATEGORIES
            .iter()
            .find(|(c, _)| normalize_name(c) == normalize_name(&s.category))
            .map(|(c, k)| (c.to_string(), *k))
            .unwrap_or_else(|| (s.category.trim().to_string(), NodeKind::Custom));
        if cat_name.is_empty() {
            continue;
        }
        let cat = category(world, &mut b, &root, &cat_name, &mut cats)?;
        for n in &s.nodes {
            b.tree(&cat, n, fallback);
        }
    }
    b.relations(world, &out.relations)?;
    let title = format!("生成世界：{}", out.name);
    Ok((b.ops, b.notes, Mode::Append, title))
}

pub fn convert_expand(world: &World, node_id: &str, kinds: &[NodeKind], v: &Value) -> Result<(Vec<Op>, Vec<Issue>, Mode, String)> {
    let out: ExpansionOut = serde_json::from_value(v.clone())?;
    let parent = nodes::require_node(world.conn(), node_id)?;
    let mut b = Builder::new();
    b.names.insert(normalize_name(&parent.title), parent.id.clone());
    let fallback = kinds.first().copied().unwrap_or(match parent.kind {
        NodeKind::World | NodeKind::Category => NodeKind::Custom,
        NodeKind::Nation | NodeKind::Region => NodeKind::City,
        NodeKind::City => NodeKind::Site,
        k => k,
    });
    for n in &out.nodes {
        b.tree(&parent.id, n, fallback);
    }
    b.relations(world, &out.relations)?;
    Ok((b.ops, b.notes, Mode::Append, format!("展开「{}」", parent.title)))
}

pub fn convert_rewrite(world: &World, node_id: &str, mode: RewriteMode, v: &Value) -> Result<(Vec<Op>, Vec<Issue>, Mode, String)> {
    let out: NodeRewriteOut = serde_json::from_value(v.clone())?;
    let n = nodes::require_node(world.conn(), node_id)?;
    let mut notes = vec![];
    let fill = |cur: &str, new: &str| -> Option<String> {
        let new = new.trim();
        if new.is_empty() {
            return None;
        }
        match mode {
            RewriteMode::Complete if !cur.trim().is_empty() => None,
            _ => Some(new.to_string()),
        }
    };
    let mut payload = n.payload.as_object().cloned().unwrap_or_default();
    let mut changed_payload = false;
    for (k, val) in &out.details {
        let empty = match payload.get(k) {
            None | Some(Value::Null) => true,
            Some(Value::String(s)) => s.is_empty(),
            Some(Value::Array(a)) => a.is_empty(),
            Some(Value::Object(o)) => o.is_empty(),
            _ => false,
        };
        if mode != RewriteMode::Complete || empty {
            payload.insert(k.clone(), val.clone());
            changed_payload = true;
        }
    }
    let payload = Value::Object(payload);
    let payload = if changed_payload {
        match coerce_payload(n.kind, &payload) {
            Ok(p) => Some(p),
            Err(e) => {
                notes.push(note(0, "details_dropped", format!("结构化资料格式不对，已忽略：{e}")));
                None
            }
        }
    } else {
        None
    };
    let mut aliases = n.aliases.clone();
    for a in &out.aliases {
        if !a.trim().is_empty() && !aliases.contains(a) && a != &n.title {
            aliases.push(a.clone());
        }
    }
    let mut summary = fill(&n.summary, &out.summary);
    if summary.is_none() && n.summary.trim().is_empty() {
        let pt = if out.player_text.trim().is_empty() { n.player_text.as_str() } else { out.player_text.as_str() };
        if let Some(sm) = summary_from(pt) {
            notes.push(note(0, "summary_derived", "模型没有写简介，已取玩家可见描述的第一句代替，请检查".into()));
            summary = Some(sm);
        }
    }
    let patch = NodePatch {
        summary,
        player_text: fill(&n.player_text, &out.player_text),
        gm_text: fill(&n.gm_text, &out.gm_text),
        aliases: (aliases != n.aliases).then_some(aliases),
        payload,
        ..Default::default()
    };
    if patch.is_empty() {
        return Err(GenError::Output(if mode == RewriteMode::Complete { "没有需要补全的空白内容".into() } else { "模型没有给出任何修改".into() }));
    }
    let verb = match mode {
        RewriteMode::Rewrite => "重写",
        RewriteMode::Expand => "扩写",
        RewriteMode::Complete => "补全",
    };
    Ok((
        vec![Op::UpdateNode { id: n.id.clone(), base_rev: Some(n.rev), patch }],
        notes,
        Mode::Targeted { targets: vec![n.id.clone()] },
        format!("{verb}「{}」", n.title),
    ))
}

#[cfg(test)]
mod coerce_tests {
    use chronicle_core::model::NodeKind;
    use serde_json::json;

    #[test]
    fn wraps_only_the_string_that_should_be_a_list() {
        let p = json!({"identity": "退休的潮汐术士", "secrets": "知晓静默教会的计划"});
        let fixed = super::coerce_payload(NodeKind::Character, &p).unwrap();
        assert_eq!(fixed["identity"], json!("退休的潮汐术士"), "本来就是字符串的字段不动");
        assert_eq!(fixed["secrets"], json!(["知晓静默教会的计划"]));
    }
}

#[cfg(test)]
mod summary_tests {
    #[test]
    fn derives_first_sentence() {
        assert_eq!(super::summary_from("  学院坐落在月心岛北岸。夜里灯火通明。").as_deref(), Some("学院坐落在月心岛北岸。"));
        assert_eq!(super::summary_from(""), None);
        assert_eq!(super::summary_from(&"长".repeat(100)).unwrap().chars().count(), 60);
    }
}

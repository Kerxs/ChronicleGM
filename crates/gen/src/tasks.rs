//! 生成任务：组装提示词，把模型输出转换为待审变更集的操作。
//!
//! 转换只产生 `Op`，不直接写数据库；是否写入由用户在审阅后决定。

use std::collections::HashMap;

use chronicle_core::changeset::{Issue, Mode, NewChangeset, Op, Severity};
use chronicle_core::model::*;
use chronicle_core::util::normalize_name;
use chronicle_core::{World, nodes, scripts};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ts_rs::TS;

use crate::context::{self, Context, RenderedContext};
use crate::outputs::*;
use crate::templates::Templates;
use crate::plot_tasks::{self, PlotOut};
use crate::world_tasks::{self, ExpansionOut, NodeRewriteOut, RewriteMode, WorldSeedOut};
use crate::{GenError, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "task", rename_all = "snake_case")]
#[ts(export)]
pub enum Task {
    /// 从零生成剧本大纲（可参考已有世界观）。
    Outline {
        idea: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        genre: String,
        #[serde(default)]
        tone: String,
        player_count: i64,
        duration_hours: f64,
        #[serde(default)]
        rules: String,
        /// 是否读取当前世界的设定作为参考。
        #[serde(default = "yes")]
        use_world: bool,
    },
    /// 展开一个章节：场景、线索、道具、事件、转折。
    ExpandChapter {
        script_id: String,
        chapter_id: String,
        #[serde(default)]
        guidance: String,
    },
    /// 完善已有人物档案，或新增人物。
    Npcs {
        script_id: String,
        #[serde(default)]
        node_ids: Vec<String>,
        #[serde(default)]
        new_count: u32,
        #[serde(default)]
        guidance: String,
    },
    /// 只重新生成选中的条目。
    Regenerate {
        script_id: String,
        item_ids: Vec<String>,
        instruction: String,
    },
    /// 继续扩展剧本，不覆盖已有内容。
    Extend { script_id: String, instruction: String },
    /// 一句话生成世界树第一层。
    WorldSeed {
        idea: String,
        #[serde(default)]
        genre: String,
        #[serde(default)]
        era: String,
        #[serde(default)]
        style: String,
        #[serde(default)]
        rules: String,
    },
    /// 根据已经写好的剧本生成世界树（地理、势力、世界规则等）。顺序：剧本 → 检查 → 世界树。
    WorldFromScript {
        script_id: String,
        #[serde(default)]
        guidance: String,
    },
    /// 为世界树节点续生子节点。
    ExpandNode {
        node_id: String,
        #[serde(default = "default_count")]
        count: u32,
        #[serde(default)]
        kinds: Vec<NodeKind>,
        #[serde(default)]
        guidance: String,
    },
    /// 扩写 / 重写 / 补全一个世界树节点。
    RewriteNode {
        node_id: String,
        mode: RewriteMode,
        #[serde(default)]
        instruction: String,
    },
    /// 根据剧本生成剧情分支图。
    PlotFromScript {
        script_id: String,
        #[serde(default)]
        guidance: String,
    },
    /// 为剧情节点生成新分支：有玩家行动时按行动生成后续，否则补充若干选择。
    PlotBranches {
        script_id: String,
        from_node_id: String,
        #[serde(default)]
        player_action: String,
        #[serde(default = "default_branch_count")]
        count: u32,
    },
}

fn default_count() -> u32 {
    5
}

fn default_branch_count() -> u32 {
    2
}

fn yes() -> bool {
    true
}

impl Task {
    pub fn kind(&self) -> &'static str {
        match self {
            Task::Outline { .. } => "outline",
            Task::ExpandChapter { .. } => "expand_chapter",
            Task::Npcs { .. } => "npcs",
            Task::Regenerate { .. } => "regenerate",
            Task::Extend { .. } => "extend",
            Task::WorldSeed { .. } => "world_seed",
            Task::WorldFromScript { .. } => "world_from_script",
            Task::ExpandNode { .. } => "expand_node",
            Task::RewriteNode { .. } => "rewrite_node",
            Task::PlotFromScript { .. } => "plot_from_script",
            Task::PlotBranches { .. } => "plot_branches",
        }
    }
    pub fn target(&self) -> String {
        match self {
            Task::Outline { idea, .. } => idea.chars().take(40).collect(),
            Task::ExpandChapter { chapter_id, .. } => chapter_id.clone(),
            Task::Npcs { script_id, .. } | Task::Regenerate { script_id, .. } | Task::Extend { script_id, .. } => {
                script_id.clone()
            }
            Task::WorldSeed { idea, .. } => idea.chars().take(40).collect(),
            Task::WorldFromScript { script_id, .. } => script_id.clone(),
            Task::ExpandNode { node_id, .. } | Task::RewriteNode { node_id, .. } => node_id.clone(),
            Task::PlotFromScript { script_id, .. } => script_id.clone(),
            Task::PlotBranches { from_node_id, .. } => from_node_id.clone(),
        }
    }
    pub fn schema(&self) -> (&'static str, Value) {
        match self {
            Task::Outline { .. } => ("script_outline", schema_of::<ScriptOutline>()),
            Task::ExpandChapter { .. } => ("chapter_expansion", schema_of::<ChapterExpansion>()),
            Task::Npcs { .. } => ("npc_batch", schema_of::<NpcBatch>()),
            Task::Regenerate { .. } => ("rewrites", schema_of::<Rewrites>()),
            Task::Extend { .. } => ("script_extension", schema_of::<ScriptExtension>()),
            Task::WorldSeed { .. } | Task::WorldFromScript { .. } => ("world_seed", schema_of::<WorldSeedOut>()),
            Task::ExpandNode { .. } => ("node_expansion", schema_of::<ExpansionOut>()),
            Task::RewriteNode { .. } => ("node_rewrite", schema_of::<NodeRewriteOut>()),
            Task::PlotFromScript { .. } | Task::PlotBranches { .. } => ("plot_graph", schema_of::<PlotOut>()),
        }
    }
    /// 按任务类型检查输出结构。
    pub fn check_output(&self, v: &Value) -> std::result::Result<(), String> {
        let r = match self {
            Task::Outline { .. } => serde_json::from_value::<ScriptOutline>(v.clone()).map(|o| {
                if o.chapters.is_empty() { Err("chapters 不能为空".to_string()) } else { Ok(()) }
            }),
            Task::ExpandChapter { .. } => serde_json::from_value::<ChapterExpansion>(v.clone()).map(|o| {
                if o.scenes.is_empty() { Err("scenes 不能为空".to_string()) } else { Ok(()) }
            }),
            Task::Npcs { .. } => serde_json::from_value::<NpcBatch>(v.clone()).map(|_| Ok(())),
            Task::Regenerate { .. } => serde_json::from_value::<Rewrites>(v.clone()).map(|_| Ok(())),
            Task::Extend { .. } => serde_json::from_value::<ScriptExtension>(v.clone()).map(|_| Ok(())),
            Task::WorldSeed { .. } | Task::WorldFromScript { .. } => serde_json::from_value::<WorldSeedOut>(v.clone()).map(|o| {
                if o.sections.is_empty() { Err("sections 不能为空".to_string()) } else { Ok(()) }
            }),
            Task::ExpandNode { .. } => serde_json::from_value::<ExpansionOut>(v.clone()).map(|o| {
                if o.nodes.is_empty() { Err("nodes 不能为空".to_string()) } else { Ok(()) }
            }),
            Task::RewriteNode { .. } => serde_json::from_value::<NodeRewriteOut>(v.clone()).map(|_| Ok(())),
            Task::PlotFromScript { .. } | Task::PlotBranches { .. } => serde_json::from_value::<PlotOut>(v.clone()).map(|o| {
                if o.nodes.is_empty() && o.edges.is_empty() { Err("nodes 与 edges 不能都为空".to_string()) } else { Ok(()) }
            }),
        };
        match r {
            Ok(inner) => inner,
            Err(e) => Err(format!("JSON 结构不符合要求：{e}")),
        }
    }
}

/// 章节数量范围：每章约 2–3 个场景。
pub fn chapter_range(p: &ScaleProfile) -> (u32, u32) {
    let min = p.scenes.min.div_ceil(3).max(1);
    (min, p.scenes.max.div_ceil(2).max(min))
}

pub struct Prompt {
    pub system: String,
    pub user: String,
    pub schema_name: &'static str,
    pub schema: Value,
    pub context: RenderedContext,
}

const LOCATION_KINDS: &[NodeKind] = &[NodeKind::Region, NodeKind::Nation, NodeKind::City, NodeKind::Site];

/// 组装提示词。`schema_in_prompt` 为真时把 JSON Schema 写进系统提示（服务不支持结构化输出时）。
pub fn build_prompt(world: &World, task: &Task, templates: &Templates, budget: usize, schema_in_prompt: bool) -> Result<Prompt> {
    let (schema_name, schema) = task.schema();
    let schema_text = if schema_in_prompt { serde_json::to_string(&schema)? } else { String::new() };
    let system = templates.render("system", json!({ "schema_text": schema_text }))?;
    let mut ctx = Context::default();
    let world_prompt = match task {
        Task::WorldSeed { idea, genre, era, style, rules } => {
            Some(world_tasks::build_world_seed(world, templates, budget, world_tasks::SeedInput { idea, genre, era, style, rules })?)
        }
        Task::WorldFromScript { script_id, guidance } => Some(world_tasks::build_world_from_script(world, templates, budget, script_id, guidance)?),
        Task::ExpandNode { node_id, count, kinds, guidance } => {
            Some(world_tasks::build_expand(world, templates, budget, node_id, *count, kinds, guidance)?)
        }
        Task::RewriteNode { node_id, mode, instruction } => {
            Some(world_tasks::build_rewrite(world, templates, budget, node_id, *mode, instruction)?)
        }
        Task::PlotFromScript { script_id, guidance } => Some(plot_tasks::build_from_script(world, templates, budget, script_id, guidance)?),
        Task::PlotBranches { script_id, from_node_id, player_action, count } => {
            Some(plot_tasks::build_branches(world, templates, budget, script_id, from_node_id, player_action, *count)?)
        }
        _ => None,
    };
    if let Some((user, context)) = world_prompt {
        return Ok(Prompt { system, user, schema_name, schema, context });
    }
    let user = match task {
        Task::WorldSeed { .. } | Task::WorldFromScript { .. } | Task::ExpandNode { .. } | Task::RewriteNode { .. } | Task::PlotFromScript { .. } | Task::PlotBranches { .. } => unreachable!(),
        Task::Outline { idea, title, genre, tone, player_count, duration_hours, rules, use_world } => {
            if idea.trim().is_empty() {
                return Err(GenError::Input("请填写剧本创意".into()));
            }
            if *use_world {
                ctx.push("世界核心设定（不得矛盾）", context::core_canon(world, 50)?, 1);
                ctx.push(
                    "已有名称（可以引用，新名称不要重复）",
                    context::world_names(
                        world,
                        &[NodeKind::Character, NodeKind::Faction, NodeKind::Nation, NodeKind::City, NodeKind::Site, NodeKind::Region],
                        60,
                    )?,
                    3,
                );
            }
            let rendered = ctx.render(budget);
            let scale = scale_profile(*player_count, *duration_hours);
            let (cmin, cmax) = chapter_range(&scale);
            let user = templates.render(
                "outline",
                json!({
                    "idea": idea, "title": title, "genre": genre, "tone": tone, "players": player_count,
                    "hours": duration_hours, "rules": rules, "scale": scale, "chapters_min": cmin, "chapters_max": cmax,
                    "npcs_min": scale.npcs.min.min(8), "npcs_max": scale.npcs.max.min(12), "context": rendered.text,
                }),
            )?;
            return Ok(Prompt { system, user, schema_name, schema, context: rendered });
        }
        Task::ExpandChapter { script_id, chapter_id, guidance } => {
            let s = scripts::require_script(world.conn(), script_id)?;
            let ch = scripts::require_item(world.conn(), chapter_id)?;
            if ch.script_id != s.id || ch.kind != ScriptItemKind::Chapter {
                return Err(GenError::Input("目标不是该剧本的章节".into()));
            }
            ctx.push("剧本信息", context::script_meta(&s), 0);
            ctx.push("世界核心设定（不得矛盾）", context::core_canon(world, 30)?, 1);
            ctx.push("剧本大纲", context::script_outline(world, script_id, false)?, 2);
            ctx.push("已有人物", context::cast_text(world, script_id)?, 2);
            ctx.push("已有地点与人物名称", context::world_names(world, &[NodeKind::City, NodeKind::Site, NodeKind::Region, NodeKind::Character], 40)?, 4);
            let rendered = ctx.render(budget);
            let scale: ScaleProfile = serde_json::from_value(s.scale_profile.clone()).unwrap_or_else(|_| scale_profile(s.player_count, s.duration_hours));
            let chapters = scripts::items_by_kind(world.conn(), script_id, ScriptItemKind::Chapter)?.len().max(1) as u32;
            // 线索数量按剧本的调查比重（以前固定"每个场景 1–2 条"，所以所有剧本都像探案）。
            let gp = chronicle_core::profile::for_script(&s);
            let (clues_min, clues_max) = gp.clues_per_chapter();
            let user = templates.render(
                "expand_chapter",
                json!({
                    "target": context::item_detail(&ch), "guidance": guidance, "context": rendered.text,
                    "scenes_min": (scale.scenes.min / chapters).max(1), "scenes_max": scale.scenes.max.div_ceil(chapters).max(2),
                    "clues_min": clues_min, "clues_max": clues_max, "play": gp.guidance(),
                }),
            )?;
            return Ok(Prompt { system, user, schema_name, schema, context: rendered });
        }
        Task::Npcs { script_id, node_ids, new_count, guidance } => {
            let s = scripts::require_script(world.conn(), script_id)?;
            let (targets, _) = npc_targets(world, node_ids)?;
            if targets.is_empty() && *new_count == 0 {
                return Err(GenError::Input("没有可完善的人物（已锁定或核心设定的人物会被跳过），也没有要求新增".into()));
            }
            ctx.push("剧本信息", context::script_meta(&s), 0);
            ctx.push("世界核心设定（不得矛盾）", context::core_canon(world, 30)?, 1);
            ctx.push("剧本大纲", context::script_outline(world, script_id, false)?, 2);
            ctx.push("剧本人物", context::cast_text(world, script_id)?, 2);
            ctx.push("已有名称", context::world_names(world, &[NodeKind::Character, NodeKind::Faction], 60)?, 4);
            let rendered = ctx.render(budget);
            let t: Vec<String> = targets.iter().map(context::node_detail).collect();
            let user = templates.render(
                "npcs",
                json!({ "targets": t.join("\n\n"), "new_count": new_count, "guidance": guidance, "context": rendered.text }),
            )?;
            return Ok(Prompt { system, user, schema_name, schema, context: rendered });
        }
        Task::Regenerate { script_id, item_ids, instruction } => {
            let s = scripts::require_script(world.conn(), script_id)?;
            let (items, _) = regen_targets(world, script_id, item_ids)?;
            if items.is_empty() {
                return Err(GenError::Input("没有可重写的条目（锁定的条目不会被重写）".into()));
            }
            ctx.push("剧本信息", context::script_meta(&s), 0);
            ctx.push("世界核心设定（不得矛盾）", context::core_canon(world, 30)?, 1);
            ctx.push("剧本大纲", context::script_outline(world, script_id, false)?, 2);
            ctx.push("剧本人物", context::cast_text(world, script_id)?, 3);
            let rendered = ctx.render(budget);
            let t: Vec<String> = items.iter().map(context::item_detail).collect();
            let instruction = if instruction.trim().is_empty() { "改写得更具体、更有张力，保持原有设定。" } else { instruction };
            let user = templates.render("regenerate", json!({ "targets": t.join("\n\n"), "instruction": instruction, "context": rendered.text }))?;
            return Ok(Prompt { system, user, schema_name, schema, context: rendered });
        }
        Task::Extend { script_id, instruction } => {
            if instruction.trim().is_empty() {
                return Err(GenError::Input("请填写扩展要求".into()));
            }
            let s = scripts::require_script(world.conn(), script_id)?;
            ctx.push("剧本信息", context::script_meta(&s), 0);
            ctx.push("世界核心设定（不得矛盾）", context::core_canon(world, 30)?, 1);
            ctx.push("剧本大纲（已有内容，不要重复）", context::script_outline(world, script_id, false)?, 0);
            ctx.push("剧本人物", context::cast_text(world, script_id)?, 2);
            ctx.push("已有名称", context::world_names(world, &[NodeKind::Character, NodeKind::City, NodeKind::Site], 40)?, 4);
            let rendered = ctx.render(budget);
            templates.render("extend", json!({ "instruction": instruction, "context": rendered.text }))?
        }
    };
    let rendered = ctx.render(budget);
    Ok(Prompt { system, user, schema_name, schema, context: rendered })
}

fn npc_targets(world: &World, ids: &[String]) -> Result<(Vec<Node>, Vec<Issue>)> {
    let mut out = vec![];
    let mut notes = vec![];
    for id in ids {
        let n = nodes::require_node(world.conn(), id)?;
        if n.kind != NodeKind::Character {
            return Err(GenError::Input(format!("「{}」不是人物", n.title)));
        }
        if n.locked || n.canon_level == CanonLevel::Core {
            notes.push(warn(0, "skipped_protected", format!("「{}」已锁定或属于核心设定，未交给 AI 修改", n.title), vec![n.id.clone()]));
        } else {
            out.push(n);
        }
    }
    Ok((out, notes))
}

fn regen_targets(world: &World, script_id: &str, ids: &[String]) -> Result<(Vec<ScriptItem>, Vec<Issue>)> {
    let mut out = vec![];
    let mut notes = vec![];
    for id in ids {
        let it = scripts::require_item(world.conn(), id)?;
        if it.script_id != script_id {
            return Err(GenError::Input(format!("条目「{}」不属于该剧本", it.title)));
        }
        if it.locked {
            notes.push(warn(0, "skipped_locked", format!("「{}」已锁定，未重写", it.title), vec![it.id.clone()]));
        } else {
            out.push(it);
        }
    }
    Ok((out, notes))
}

fn warn(op: usize, code: &str, message: String, related: Vec<String>) -> Issue {
    Issue { op, severity: Severity::Warning, code: code.into(), message, related }
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().filter(|(_, v)| !v.is_null() && v != &json!("") && v != &json!([])).map(|(k, v)| (k.to_string(), v)).collect())
}

/// 收集操作并生成临时 ID。
struct Ops {
    ops: Vec<Op>,
    notes: Vec<Issue>,
    seq: usize,
    npc_root: Option<String>,
}

impl Ops {
    fn new() -> Self {
        Self { ops: vec![], notes: vec![], seq: 0, npc_root: None }
    }
    fn tmp(&mut self, prefix: &str) -> String {
        self.seq += 1;
        format!("tmp:{prefix}{}", self.seq)
    }
    fn note(&mut self, code: &str, msg: String, related: Vec<String>) {
        let op = self.ops.len().saturating_sub(1);
        self.notes.push(warn(op, code, msg, related));
    }
    // 参数一一对应条目的字段，拆成结构体反而更难读。
    #[allow(clippy::too_many_arguments)]
    fn item(&mut self, script: &str, kind: ScriptItemKind, parent: Option<String>, title: &str, p: &str, g: &str, payload: Value) -> String {
        let t = self.tmp(kind.as_str());
        let mut it = NewScriptItem::new(script, kind, title.trim());
        it.parent_id = parent;
        it.player_text = p.into();
        it.gm_text = g.into();
        it.payload = payload;
        self.ops.push(Op::CreateScriptItem { temp_id: Some(t.clone()), item: it });
        t
    }
    /// 新 NPC 放在根级"人物"分类下（不存在则新建）。
    fn npc_parent(&mut self, world: &World) -> Result<String> {
        if let Some(p) = &self.npc_root {
            return Ok(p.clone());
        }
        let existing = nodes::children(world.conn(), None, 0, 500)?
            .into_iter()
            .find(|n| n.kind == NodeKind::Category && n.title == "人物");
        let id = match existing {
            Some(n) => n.id,
            None => {
                let t = self.tmp("category");
                self.ops.push(Op::CreateNode { temp_id: Some(t.clone()), node: NewNode::new(NodeKind::Category, "人物") });
                t
            }
        };
        self.npc_root = Some(id.clone());
        Ok(id)
    }
    /// 剧本人物：已有同名人物则直接加入剧本角色，否则在世界树中新建。
    fn npc(&mut self, world: &World, script: &str, n: &NpcOutline) -> Result<()> {
        if n.name.trim().is_empty() {
            return Ok(());
        }
        let existing = nodes::find_by_name(world.conn(), &n.name, Some(NodeKind::Character))?;
        let node_id = match existing.first() {
            Some(e) => {
                self.notes.push(warn(self.ops.len(), "reused_npc", format!("使用世界中已有的人物「{}」", e.title), vec![e.id.clone()]));
                e.id.clone()
            }
            None => {
                let parent = self.npc_parent(world)?;
                let t = self.tmp("npc");
                let mut node = NewNode::new(NodeKind::Character, n.name.trim()).under(&parent);
                node.summary = n.identity.clone();
                node.payload = obj(vec![
                    ("identity", json!(n.identity)),
                    ("motivation", json!(n.motivation)),
                    ("secrets", if n.secret.is_empty() { json!([]) } else { json!([n.secret]) }),
                ]);
                self.ops.push(Op::CreateNode { temp_id: Some(t.clone()), node });
                t
            }
        };
        self.ops.push(Op::AddCast {
            temp_id: None,
            cast: NewCastMember {
                script_id: script.into(),
                node_id,
                role: n.role.clone(),
                motive: n.motivation.clone(),
                secret: n.secret.clone(),
                relation_to_players: n.relation_to_players.clone(),
                payload: json!({}),
            },
        });
        Ok(())
    }
}

fn quest_payload(q: &QuestOutline) -> Value {
    let qt = match q.quest_type {
        QuestKind::Main => "main",
        QuestKind::Side => "side",
        QuestKind::Hidden => "hidden",
    };
    json!({"quest_type": qt, "objectives": q.objectives, "rewards": q.rewards})
}

fn ending_gm(e: &EndingOutline) -> String {
    match (e.condition.trim().is_empty(), e.gm_text.trim().is_empty()) {
        (true, _) => e.gm_text.clone(),
        (false, true) => format!("达成条件：{}", e.condition),
        (false, false) => format!("达成条件：{}\n{}", e.condition, e.gm_text),
    }
}

fn check_range(o: &mut Ops, what: &str, n: usize, r: Range) {
    if (n as u32) < r.min || (n as u32) > r.max {
        o.notes.push(warn(0, "scale", format!("{what}数量 {n} 不在建议范围 {}–{} 内", r.min, r.max), vec![]));
    }
}

/// 把模型输出转换为变更集（尚未保存）与生成器说明。
pub fn convert(world: &World, task: &Task, value: &Value, job_id: &str) -> Result<(NewChangeset, Vec<Issue>)> {
    let author = Author::Ai { job_id: job_id.into() };
    let world_result = match task {
        Task::WorldSeed { .. } | Task::WorldFromScript { .. } => Some(world_tasks::convert_world_seed(world, value)?),
        Task::ExpandNode { node_id, kinds, .. } => Some(world_tasks::convert_expand(world, node_id, kinds, value)?),
        Task::RewriteNode { node_id, mode, .. } => Some(world_tasks::convert_rewrite(world, node_id, *mode, value)?),
        Task::PlotFromScript { script_id, .. } => Some(plot_tasks::convert_from_script(world, script_id, value)?),
        Task::PlotBranches { script_id, from_node_id, .. } => Some(plot_tasks::convert_branches(world, script_id, from_node_id, value)?),
        _ => None,
    };
    if let Some((ops, notes, mode, title)) = world_result {
        if ops.is_empty() {
            return Err(GenError::Output("模型没有生成任何可用内容".into()));
        }
        let scope = if matches!(task, Task::PlotFromScript { .. } | Task::PlotBranches { .. }) { "plot" } else { "world" };
        return Ok((NewChangeset { job_id: Some(job_id.into()), scope: scope.into(), mode, author, title, ops }, notes));
    }
    let mut o = Ops::new();
    let (mode, title) = match task {
        Task::WorldSeed { .. } | Task::WorldFromScript { .. } | Task::ExpandNode { .. } | Task::RewriteNode { .. } | Task::PlotFromScript { .. } | Task::PlotBranches { .. } => unreachable!(),
        Task::Outline { title: want_title, genre, tone, player_count, duration_hours, .. } => {
            let out: ScriptOutline = serde_json::from_value(value.clone())?;
            let mut ns = NewScript::new(want_title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| out.title.clone()));
            ns.genre = if out.genre.is_empty() { genre.clone() } else { out.genre.clone() };
            ns.tone = if out.tone.is_empty() { tone.clone() } else { out.tone.clone() };
            ns.premise = out.premise.clone();
            ns.background = out.background.clone();
            ns.core_setting = out.core_setting.clone();
            ns.player_count = *player_count;
            ns.duration_hours = *duration_hours;
            let gp = crate::play::profile_from_play(&out.play, &ns.genre, &ns.tone, &ns.premise);
            ns.game_profile = serde_json::to_value(&gp).unwrap_or_else(|_| json!({}));
            let title = ns.title.clone();
            let s = "tmp:script".to_string();
            o.ops.push(Op::CreateScript { temp_id: Some(s.clone()), script: ns });
            o.item(&s, ScriptItemKind::Opening, None, "开场", &out.opening.player_text, &out.opening.gm_text, json!({}));
            let mut chapter_ids: HashMap<String, String> = HashMap::new();
            for ch in &out.chapters {
                let t = o.item(&s, ScriptItemKind::Chapter, None, &ch.title, &ch.player_text, &ch.gm_text, json!({}));
                chapter_ids.insert(normalize_name(&ch.title), t);
            }
            for q in &out.quests {
                let parent = q.chapter.as_ref().and_then(|c| chapter_ids.get(&normalize_name(c)).cloned());
                o.item(&s, ScriptItemKind::Quest, parent, &q.title, &q.player_text, &q.gm_text, quest_payload(q));
            }
            for e in &out.endings {
                o.item(&s, ScriptItemKind::Ending, None, &e.title, &e.player_text, &ending_gm(e), json!({}));
            }
            for n in &out.npcs {
                o.npc(world, &s, n)?;
            }
            // 可选的玩家角色：身份、性格、能力、身世；与主线的关联给主持人看。
            for pc in out.player_characters.iter().take(4) {
                let gm = format!("与主线的关联：{}", pc.backstory.link_to_plot);
                o.item(&s, ScriptItemKind::PlayerCharacter, None, &pc.name, &pc.player_text, &gm, crate::play::pc_payload(pc));
            }
            if out.player_characters.is_empty() {
                o.note("no_player_characters", "没有生成可选角色：开始游戏时可以让 AI 按剧本生成一个，或者在剧本里手动添加".into(), vec![]);
            }
            let scale = scale_profile(*player_count, *duration_hours);
            let (cmin, cmax) = chapter_range(&scale);
            check_range(&mut o, "章节", out.chapters.len(), Range { min: cmin, max: cmax });
            let count = |k: QuestKind| out.quests.iter().filter(|q| q.quest_type == k).count();
            check_range(&mut o, "主线任务", count(QuestKind::Main), scale.main_quests);
            check_range(&mut o, "支线任务", count(QuestKind::Side), scale.side_quests);
            check_range(&mut o, "隐藏任务", count(QuestKind::Hidden), scale.hidden_quests);
            check_range(&mut o, "结局", out.endings.len(), scale.endings);
            (Mode::Normal, format!("剧本大纲：{title}"))
        }
        Task::ExpandChapter { script_id, chapter_id, .. } => {
            let out: ChapterExpansion = serde_json::from_value(value.clone())?;
            let ch = scripts::require_item(world.conn(), chapter_id)?;
            let mut scene_ids: HashMap<String, String> = HashMap::new();
            for sc in &out.scenes {
                let mut payload = serde_json::Map::new();
                if let Some(loc) = sc.location.as_ref().filter(|l| !l.trim().is_empty()) {
                    let found = nodes::find_by_name(world.conn(), loc, None)?.into_iter().find(|n| LOCATION_KINDS.contains(&n.kind));
                    match found {
                        Some(n) => {
                            payload.insert("location_id".into(), json!(n.id));
                        }
                        None => {
                            payload.insert("location_name".into(), json!(loc));
                        }
                    }
                }
                let mut npc_ids = vec![];
                let mut unknown = vec![];
                for name in &sc.npcs {
                    match nodes::find_by_name(world.conn(), name, Some(NodeKind::Character))?.first() {
                        Some(n) => npc_ids.push(n.id.clone()),
                        None => unknown.push(name.clone()),
                    }
                }
                if !npc_ids.is_empty() {
                    payload.insert("npc_ids".into(), json!(npc_ids));
                }
                if !unknown.is_empty() {
                    payload.insert("npc_names".into(), json!(unknown));
                }
                let t = o.item(script_id, ScriptItemKind::Scene, Some(chapter_id.clone()), &sc.title, &sc.player_text, &sc.gm_text, Value::Object(payload));
                if !unknown.is_empty() {
                    o.note("unknown_npc", format!("场景「{}」提到了世界中不存在的人物：{}", sc.title, unknown.join("、")), vec![]);
                }
                scene_ids.insert(normalize_name(&sc.title), t);
            }
            for c in &out.clues {
                let parent = c
                    .found_in
                    .as_ref()
                    .and_then(|f| scene_ids.get(&normalize_name(f)).cloned())
                    .unwrap_or_else(|| chapter_id.clone());
                o.item(script_id, ScriptItemKind::Clue, Some(parent), &c.title, &c.player_text, &c.gm_text, json!({}));
            }
            for it in &out.items {
                o.item(script_id, ScriptItemKind::Item, Some(chapter_id.clone()), &it.title, &it.player_text, &it.gm_text, json!({}));
            }
            for e in &out.key_events {
                o.item(script_id, ScriptItemKind::KeyEvent, Some(chapter_id.clone()), &e.title, &e.player_text, &e.gm_text, obj(vec![("trigger", json!(e.trigger))]));
            }
            for t in &out.twists {
                o.item(script_id, ScriptItemKind::Twist, Some(chapter_id.clone()), &t.title, &t.player_text, &t.gm_text, json!({}));
            }
            (Mode::Append, format!("展开章节：{}", ch.title))
        }
        Task::Npcs { script_id, node_ids, new_count, .. } => {
            let out: NpcBatch = serde_json::from_value(value.clone())?;
            let (targets, skipped) = npc_targets(world, node_ids)?;
            o.notes.extend(skipped);
            let mut by_name: HashMap<String, &Node> = targets.iter().map(|n| (normalize_name(&n.title), n)).collect();
            let mut created = 0u32;
            for d in &out.npcs {
                let key = normalize_name(&d.name);
                let detail = obj(vec![
                    ("identity", json!(d.identity)),
                    ("appearance", json!(d.appearance)),
                    ("personality", json!(d.personality)),
                    ("motivation", json!(d.motivation)),
                    ("secrets", json!(d.secrets)),
                    ("knowledge", json!(d.knowledge)),
                    ("abilities", json!(d.abilities)),
                ]);
                if let Some(n) = by_name.remove(&key) {
                    // 合并 payload：保留已有字段，用新内容覆盖同名字段。
                    let mut payload = n.payload.as_object().cloned().unwrap_or_default();
                    for (k, v) in detail.as_object().unwrap() {
                        payload.insert(k.clone(), v.clone());
                    }
                    let mut aliases = n.aliases.clone();
                    for a in &d.aliases {
                        if !aliases.contains(a) && a != &n.title {
                            aliases.push(a.clone());
                        }
                    }
                    let patch = NodePatch {
                        summary: (!d.summary.is_empty()).then(|| d.summary.clone()),
                        aliases: Some(aliases),
                        player_text: (!d.player_text.is_empty()).then(|| d.player_text.clone()),
                        gm_text: (!d.gm_text.is_empty()).then(|| d.gm_text.clone()),
                        payload: Some(Value::Object(payload)),
                        ..Default::default()
                    };
                    o.ops.push(Op::UpdateNode { id: n.id.clone(), base_rev: Some(n.rev), patch });
                } else if created < *new_count {
                    created += 1;
                    let parent = o.npc_parent(world)?;
                    let t = o.tmp("npc");
                    let mut node = NewNode::new(NodeKind::Character, d.name.trim()).under(&parent).payload(detail);
                    node.aliases = d.aliases.clone();
                    node.summary = d.summary.clone();
                    node.player_text = d.player_text.clone();
                    node.gm_text = d.gm_text.clone();
                    o.ops.push(Op::CreateNode { temp_id: Some(t.clone()), node });
                    o.ops.push(Op::AddCast {
                        temp_id: None,
                        cast: NewCastMember {
                            script_id: script_id.clone(),
                            node_id: t,
                            role: d.role.clone(),
                            motive: d.motivation.clone(),
                            secret: d.secrets.join("；"),
                            relation_to_players: d.relation_to_players.clone(),
                            payload: json!({}),
                        },
                    });
                } else {
                    o.notes.push(warn(0, "ignored_npc", format!("模型返回了未要求的人物「{}」，已忽略", d.name), vec![]));
                }
            }
            for n in by_name.values() {
                o.notes.push(warn(0, "missing_npc", format!("模型没有返回「{}」的档案", n.title), vec![n.id.clone()]));
            }
            (Mode::Normal, "完善人物档案".to_string())
        }
        Task::Regenerate { script_id, item_ids, .. } => {
            let out: Rewrites = serde_json::from_value(value.clone())?;
            let (items, skipped) = regen_targets(world, script_id, item_ids)?;
            o.notes.extend(skipped);
            let by_id: HashMap<&str, &ScriptItem> = items.iter().map(|i| (i.id.as_str(), i)).collect();
            for r in &out.items {
                match by_id.get(r.id.as_str()) {
                    Some(it) => o.ops.push(Op::UpdateScriptItem {
                        id: it.id.clone(),
                        base_rev: Some(it.rev),
                        patch: ScriptItemPatch {
                            title: (!r.title.trim().is_empty()).then(|| r.title.clone()),
                            player_text: Some(r.player_text.clone()),
                            gm_text: Some(r.gm_text.clone()),
                            ..Default::default()
                        },
                    }),
                    None => o.notes.push(warn(0, "unexpected_item", format!("模型返回了未选中的条目 {}，已忽略", r.id), vec![])),
                }
            }
            let targets = items.iter().map(|i| i.id.clone()).collect();
            (Mode::Targeted { targets }, format!("重写 {} 个条目", out.items.len()))
        }
        Task::Extend { script_id, .. } => {
            let out: ScriptExtension = serde_json::from_value(value.clone())?;
            let mut chapters: HashMap<String, String> = scripts::items_by_kind(world.conn(), script_id, ScriptItemKind::Chapter)?
                .into_iter()
                .map(|c| (normalize_name(&c.title), c.id))
                .collect();
            let mut scenes: HashMap<String, String> = scripts::items_by_kind(world.conn(), script_id, ScriptItemKind::Scene)?
                .into_iter()
                .map(|c| (normalize_name(&c.title), c.id))
                .collect();
            for ch in &out.chapters {
                let t = o.item(script_id, ScriptItemKind::Chapter, None, &ch.title, &ch.player_text, &ch.gm_text, json!({}));
                chapters.insert(normalize_name(&ch.title), t);
            }
            for sc in &out.scenes {
                let parent = chapters.get(&normalize_name(&sc.chapter)).cloned();
                if parent.is_none() {
                    o.notes.push(warn(o.ops.len(), "unknown_chapter", format!("场景「{}」指定的章节「{}」不存在，放在剧本根级", sc.title, sc.chapter), vec![]));
                }
                let t = o.item(script_id, ScriptItemKind::Scene, parent, &sc.title, &sc.player_text, &sc.gm_text, json!({}));
                scenes.insert(normalize_name(&sc.title), t);
            }
            for q in &out.quests {
                let parent = q.chapter.as_ref().and_then(|c| chapters.get(&normalize_name(c)).cloned());
                o.item(script_id, ScriptItemKind::Quest, parent, &q.title, &q.player_text, &q.gm_text, quest_payload(q));
            }
            for c in &out.clues {
                let parent = c.found_in.as_ref().and_then(|f| scenes.get(&normalize_name(f)).cloned());
                o.item(script_id, ScriptItemKind::Clue, parent, &c.title, &c.player_text, &c.gm_text, json!({}));
            }
            for e in &out.endings {
                o.item(script_id, ScriptItemKind::Ending, None, &e.title, &e.player_text, &ending_gm(e), json!({}));
            }
            for n in &out.npcs {
                o.npc(world, script_id, n)?;
            }
            (Mode::Append, "继续扩展剧本".to_string())
        }
    };
    if o.ops.is_empty() {
        return Err(GenError::Output("模型没有生成任何可用内容".into()));
    }
    let scope = if matches!(task, Task::Npcs { .. }) { "world" } else { "script" };
    Ok((NewChangeset { job_id: Some(job_id.into()), scope: scope.into(), mode, author, title, ops: o.ops }, o.notes))
}

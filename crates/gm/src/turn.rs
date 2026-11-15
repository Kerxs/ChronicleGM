//! GM 回合：玩家输入 → 组装上下文 → AI 提议 → 程序掷骰、校验、执行 → AI 按结果叙述 → NPC 回应。
//!
//! 调用模型期间不持有世界锁；所有状态修改都由程序在短事务中完成，并写入只追加的事件日志。

use std::sync::Mutex;

use chronicle_core::campaign::{self, CampaignView, TransitionOutcome};
use chronicle_core::changeset::{self, Mode, NewChangeset, Op};
use chronicle_core::model::*;
use chronicle_core::rules::{Effect, StateView};
use chronicle_core::{World, nodes};
use chronicle_gen::{RequestLog, Templates};
use chronicle_llm::{CancelToken, LlmClient};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ask::{GmOptions, ask_limited};
use crate::context::{NpcCard, build_gm_context, build_npc_context};
use crate::leak::find_leaks;
use crate::outputs::*;
use crate::validate::{RejectedEffect, validate_effects};
use crate::{GmError, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CheckResult {
    pub skill: String,
    pub dc: i64,
    pub roll: i64,
    pub modifier: i64,
    pub total: i64,
    /// 是否成功（部分成功也算成功，代价由叙述体现）。
    pub success: bool,
    pub reason: String,
    /// 成功等级与每颗骰子（旧记录没有这些字段）。
    #[serde(default)]
    pub degree: Option<chronicle_core::checks::Degree>,
    #[serde(default)]
    pub detail: Option<chronicle_core::checks::CheckRoll>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NpcLine {
    pub npc_id: String,
    pub name: String,
    pub text: String,
    pub attitude_change: i64,
    pub leaks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct EntityResult {
    pub name: String,
    pub kind: String,
    /// 成为本战役临时实体后的 ID；被拒绝时为空。
    pub entity_id: Option<String>,
    pub changeset_id: Option<String>,
    pub note: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct TurnResult {
    pub turn: i64,
    pub narration: String,
    /// 叙述中原样出现的主持人信息片段（仅提示，不拦截）。
    pub leaks: Vec<String>,
    pub checks: Vec<CheckResult>,
    pub applied: Vec<Effect>,
    pub rejected: Vec<RejectedEffect>,
    pub transition: Option<TransitionOutcome>,
    pub transition_note: String,
    pub npc_lines: Vec<NpcLine>,
    pub entities: Vec<EntityResult>,
    pub off_script: bool,
    pub intent: String,
    pub remembered: Vec<String>,
    /// 这一回合的联网搜索（GM 请求、程序执行）。
    #[serde(default)]
    pub web_search: Option<serde_json::Value>,
    /// 玩家提到、但背包里没有的物品（程序事实）。
    #[serde(default)]
    pub missing_items: Vec<String>,
    /// 剧情质量提示（第 35 项，只是建议）：人物在场是否合理、可能换了说法的泄露。
    #[serde(default)]
    pub quality: Vec<String>,
    /// GM 给出的可选行动（程序去重、过滤后）。
    #[serde(default)]
    pub choices: Vec<String>,
    /// 回合结束时的故事时间（例如"第 1 天 19:00（傍晚）"）。
    #[serde(default)]
    pub time: String,
    /// 新记入调查板的发现。
    #[serde(default)]
    pub discoveries: Vec<String>,
    /// 任务变化（新任务、开始、完成、失败、备注）。
    #[serde(default)]
    pub quest_updates: Vec<String>,
    /// 时间推进时 NPC 按日程发生的活动变化（玩家不在场也会发生）。
    #[serde(default)]
    pub npc_activity: Vec<String>,
    pub current_node: Option<String>,
    /// GM 在叙述里替这些 NPC 说了话：程序删掉了那些台词，改由 NPC 亲自回答。
    #[serde(default)]
    pub redirected_speakers: Vec<String>,
    /// 叙述与程序结算可能不符的地方（启发式检查，只给主持人看）。
    #[serde(default)]
    pub conflicts: Vec<String>,
    /// GM 说明这一回合的决定依据的事实（已换成文字）。
    #[serde(default)]
    pub based_on: Vec<String>,
    /// 这一回合的旅行（例如"前往「黑木镇」，路上 150 分钟"）。
    #[serde(default)]
    pub travel: String,
    /// 这一回合听说的新地点。
    #[serde(default)]
    pub places_heard: Vec<String>,
    /// 战斗、买卖、制作、物品、奖励的结算说明（由程序计算）。
    #[serde(default)]
    pub rpg_updates: Vec<String>,
    /// 这一回合涉及的战斗（进行中或刚结束）。
    #[serde(default)]
    pub combat: Option<chronicle_core::combat::Combat>,
    #[ts(skip)]
    #[serde(skip)]
    pub requests: Vec<RequestLog>,
}

fn lock(w: &Mutex<World>) -> Result<std::sync::MutexGuard<'_, World>> {
    w.lock().map_err(|_| GmError::Input("世界文件被占用".into()))
}

/// 各类调用的输出上限（token）。关闭思考时足够；开启思考时修正请求会自动关闭思考。
const LIMIT_PLAN: u32 = 3000;
const LIMIT_NARRATION: u32 = 1500;
const LIMIT_NPC: u32 = 800;
const LIMIT_SUMMARY: u32 = 2000;

const SYSTEM: &str = "你是经验丰富的 TRPG 主持人。只输出一个 JSON 对象，不要输出解释或代码块标记。使用简体中文。";
const NPC_SYSTEM: &str = "你在 TRPG 中扮演一个人物。只输出一个 JSON 对象，不要输出解释或代码块标记。使用简体中文。";

/// 找到或创建根级"战役生成内容"分类（新实体先挂在这里，待审阅后由用户整理）。
fn dynamic_root(world: &World) -> Result<Option<String>> {
    Ok(nodes::children(world.conn(), None, 0, 500)?.into_iter().find(|n| n.kind == NodeKind::Category && n.title == "战役生成内容").map(|n| n.id))
}

fn propose_entity(world: &mut World, campaign_id: &str, e: &NewEntity) -> EntityResult {
    let mut res = EntityResult { name: e.name.clone(), kind: e.kind.clone(), entity_id: None, changeset_id: None, note: String::new() };
    let kind = match NodeKind::parse(e.kind.trim()) {
        Ok(k) if !matches!(k, NodeKind::World | NodeKind::Category) => k,
        _ => NodeKind::Custom,
    };
    if e.name.trim().is_empty() {
        res.note = "名称为空，已忽略".into();
        return res;
    }
    if let Ok(dups) = nodes::find_by_name(world.conn(), &e.name, None)
        && let Some(d) = dups.first()
    {
        res.note = format!("世界中已有「{}」，请使用已有内容，没有新建", d.title);
        return res;
    }
    if campaign::campaign_entities(world.conn(), campaign_id).unwrap_or_default().iter().any(|x| x.name == e.name.trim()) {
        res.note = "本战役中已有同名内容，没有新建".into();
        return res;
    }
    let mut ops = vec![];
    let parent = match dynamic_root(world) {
        Ok(Some(id)) => id,
        _ => {
            ops.push(Op::CreateNode { temp_id: Some("tmp:dyn".into()), node: NewNode::new(NodeKind::Category, "战役生成内容") });
            "tmp:dyn".into()
        }
    };
    let mut node = NewNode::new(kind, e.name.trim()).under(&parent);
    node.summary = e.summary.clone();
    node.gm_text = e.gm_text.clone();
    ops.push(Op::CreateNode { temp_id: None, node });
    let author = Author::Ai { job_id: format!("gm:{campaign_id}") };
    let cs = changeset::propose(world, NewChangeset { job_id: None, scope: "campaign".into(), mode: Mode::Append, author, title: format!("战役中出现：{}", e.name.trim()), ops });
    match cs {
        Ok(cs) if cs.validation.ok => match campaign::add_campaign_entity(world.conn(), campaign_id, kind.as_str(), &e.name, &e.summary, &e.gm_text, Some(&cs.id)) {
            Ok(ent) => {
                res.entity_id = Some(ent.id);
                res.changeset_id = Some(cs.id);
                res.note = "已加入本战役；确认后才会写入世界（见审阅台）".into();
            }
            Err(err) => res.note = err.to_string(),
        },
        Ok(cs) => {
            let _ = changeset::reject(world, &cs.id);
            res.note = format!("一致性检查未通过：{}", cs.validation.summary());
        }
        Err(err) => res.note = err.to_string(),
    }
    res
}

/// 回合所需的模型与配置。GM 与 NPC 可以使用不同的模型服务。
pub struct Engine<'a> {
    pub gm: &'a dyn LlmClient,
    pub npc: &'a dyn LlmClient,
    pub templates: &'a Templates,
    pub opts: &'a GmOptions,
}

/// 执行一个 GM 回合。
///
/// 回合的各步骤分别在短事务中完成。正常结束记录 `turn_end`，出错记录 `turn_aborted`（含原因）；
/// 两者都没有（程序在回合中途退出）时，[`campaign::incomplete_turn`] 能检测出来并告诉用户已发生了什么。
pub fn run_turn(world: &Mutex<World>, engine: &Engine, campaign_id: &str, input: &str, cancel: &CancelToken) -> Result<TurnResult> {
    run_turn_with_stages(world, engine, campaign_id, input, cancel, &|_| {})
}

/// 回合进行到的阶段（界面据此显示 GM 正在做什么）：
/// `context` 组装上下文 → `plan` 理解行动 → `resolve` 程序判定 → `narrate` 组织叙述 → `npc` NPC 回应。
pub fn run_turn_with_stages(world: &Mutex<World>, engine: &Engine, campaign_id: &str, input: &str, cancel: &CancelToken, stage: &dyn Fn(&str)) -> Result<TurnResult> {
    let started = {
        let w = lock(world)?;
        // 已结束的战役不能继续游玩（只能回溯开出新时间线）。
        campaign::ensure_active(w.conn(), campaign_id)?;
        true
    };
    let res = run_turn_inner(world, engine, campaign_id, input, cancel, stage);
    if started && let Ok(w) = world.lock() {
        let has_player = campaign::recent_events(w.conn(), campaign_id, &["player"], 1).map(|v| !v.is_empty()).unwrap_or(false);
        let entry = match &res {
            Ok(r) => Some(("turn_end", json!({ "turn": r.turn }))),
            Err(e) if has_player && !matches!(e, GmError::Input(_)) => Some(("turn_aborted", json!({ "error": e.to_string() }))),
            Err(_) => None,
        };
        if let Some((kind, payload)) = entry {
            let _ = campaign::record(w.conn(), campaign_id, kind, &payload);
            // 完成的回合保存检查点（回溯的历史节点）。中断的回合不保存。
            if kind == "turn_end"
                && let Err(e) = chronicle_core::timeline::checkpoint(w.conn(), campaign_id, "turn_end")
            {
                eprintln!("保存检查点失败：{e}");
            }
        }
    }
    res
}

fn script_item_kind(w: &World, script_id: Option<&str>, item: &str) -> bool {
    script_id.is_some_and(|sid| {
        w.conn()
            .query_row("SELECT 1 FROM script_items WHERE id = ?1 AND script_id = ?2 AND kind = 'item' AND deleted_at IS NULL", [item, sid], |_| Ok(()))
            .is_ok()
    })
}

/// 开场回合的输入标记（界面在新战役开始后发送）。
pub const OPENING: &str = "__opening__";

/// 开场回合交给 GM 的指示：先欢迎、介绍剧本，再介绍主角，最后交代眼下的处境，玩家不会一头雾水。
pub fn opening_prompt(script: Option<&chronicle_core::model::Script>, pc: Option<&chronicle_core::pc::PlayerCharacter>) -> String {
    let mut facts: Vec<String> = vec![];
    if let Some(s) = script {
        facts.push(format!("剧本名：《{}》", s.title));
        for (k, v) in [("题材", &s.genre), ("基调", &s.tone), ("故事前提", &s.premise), ("背景", &s.background), ("核心设定", &s.core_setting)] {
            let v = v.trim();
            if !v.is_empty() {
                facts.push(format!("{k}：{}", v.chars().take(300).collect::<String>()));
            }
        }
    }
    if let Some(c) = pc {
        facts.push(format!("主角：{}", c.name));
        for (k, v) in [("身份", &c.identity), ("性格", &c.personality), ("介绍", &c.intro)] {
            let v = v.trim();
            if !v.is_empty() {
                facts.push(format!("{k}：{}", v.chars().take(300).collect::<String>()));
            }
        }
        if !c.abilities.is_empty() {
            facts.push(format!("擅长：{}", c.abilities.join("、")));
        }
        if let Some(b) = c.backstory.as_object() {
            let parts: Vec<String> = b
                .iter()
                .filter_map(|(k, v)| {
                    let t = match v {
                        serde_json::Value::String(x) => x.clone(),
                        serde_json::Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("；"),
                        _ => String::new(),
                    };
                    (!t.trim().is_empty()).then(|| format!("{k}={}", t.chars().take(160).collect::<String>()))
                })
                .collect();
            if !parts.is_empty() {
                facts.push(format!("身世：{}", parts.join("；")));
            }
        }
    }
    let title = script.map(|s| s.title.as_str()).unwrap_or("这个故事");
    let who = pc.map(|c| c.name.as_str()).unwrap_or("玩家的角色");
    format!(
        "（游戏开场。这不是玩家的行动，是你作为 GM 对玩家说的开场白。按下面的顺序讲，用第二人称称呼玩家，层次分明，不要一上来就只写景：\n\
1. 第一句话以「欢迎来到《{title}》」开头。\n\
2. 用两三句话介绍这个剧本：是什么样的世界、什么题材和基调、故事围绕什么展开（只说玩家该知道的，不剧透秘密和真相）。\n\
3. 介绍主角「{who}」的基本情况：身份、来历、擅长什么、和这里的人与事有什么关系。\n\
4. 交代初始剧情：此刻在什么地方、身边有谁、刚刚发生了什么、眼下面对什么问题或悬念。\n\
5. 最后一句把行动交给玩家（例如「你打算怎么做？」）。\n\
不要替玩家行动，不要提出检定。在 choices 里给出 2–4 个此刻可以做的具体的事。\n\
可用的资料：\n{}）",
        facts.join("\n")
    )
}

/// 开场白的程序保证：没有以欢迎开头时，补上「欢迎来到《剧本名》。」。
pub fn ensure_welcome(narration: &str, title: Option<&str>) -> String {
    let t = narration.trim_start();
    let head: String = t.chars().take(12).collect();
    if head.contains("欢迎") {
        return t.to_string();
    }
    match title.map(str::trim).filter(|x| !x.is_empty()) {
        Some(x) => format!("欢迎来到《{x}》。{t}"),
        None => format!("欢迎来到这个故事。{t}"),
    }
}

/// 可选行动：去掉空白、重复、过长的；最多 4 条。
pub fn clean_choices(raw: &[String], narration: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for c in raw {
        let t: String = c.trim().trim_matches(|ch: char| matches!(ch, '-' | '•' | '·' | '"' | '“' | '”')).trim().to_string();
        if t.is_empty() || t.chars().count() > 60 || t == narration.trim() {
            continue;
        }
        let key: String = t.chars().filter(|ch| !ch.is_whitespace() && !ch.is_ascii_punctuation()).collect();
        if out.iter().any(|o| o.chars().filter(|ch| !ch.is_whitespace() && !ch.is_ascii_punctuation()).collect::<String>() == key) {
            continue;
        }
        out.push(t);
        if out.len() == 4 {
            break;
        }
    }
    out
}

fn run_turn_inner(world: &Mutex<World>, engine: &Engine, campaign_id: &str, input: &str, cancel: &CancelToken, stage: &dyn Fn(&str)) -> Result<TurnResult> {
    let Engine { gm, npc, templates, opts } = *engine;
    let input = input.trim();
    if input.is_empty() {
        return Err(GmError::Input("请输入玩家的行动".into()));
    }
    // 开场回合：不是玩家的行动，不记成玩家发言；GM 按剧本的开场与玩家角色讲述开场。
    let opening = input == OPENING;
    let opening_text = if opening {
        let w = lock(world)?;
        let c = campaign::get_campaign(w.conn(), campaign_id)?;
        let script = c.script_id.as_deref().and_then(|sid| chronicle_core::scripts::get_script(w.conn(), sid).ok().flatten());
        opening_prompt(script.as_ref(), chronicle_core::pc::get(w.conn(), campaign_id).as_ref())
    } else {
        String::new()
    };
    let input = if opening { opening_text.as_str() } else { input };
    let mut out = TurnResult::default();
    let dsl = chronicle_gen::plot_tasks::DSL_DOC;

    stage("context");
    // 1. 组装上下文（短暂持有世界锁）。
    let ctx = {
        let w = lock(world)?;
        let ctx = build_gm_context(&w, campaign_id, input, opts.context_budget)?;
        if opening {
            if campaign::get_campaign(w.conn(), campaign_id)?.turn > 0 {
                return Err(GmError::Input("战役已经开始，不需要开场".into()));
            }
        } else {
            campaign::record(w.conn(), campaign_id, "player", &json!({ "text": input }))?;
        }
        ctx
    };

    // 2. AI 提议（不持有锁）。
    stage("plan");
    let (play, check_rules) = {
        let w = lock(world)?;
        let gp = ctx.script.as_ref().map(chronicle_core::profile::for_script).unwrap_or_default();
        (gp.guidance(), chronicle_core::rpg::rules(w.conn(), campaign_id).check.describe())
    };
    // 玩家提到背包里没有的物品：作为程序事实交给 GM。
    let missing = if opening {
        vec![]
    } else {
        let w = lock(world)?;
        chronicle_core::rpg::missing_items(w.conn(), campaign_id, ctx.script.as_ref().map(|s| s.id.as_str()), input).unwrap_or_default()
    };
    let context_text = if missing.is_empty() {
        ctx.text.clone()
    } else {
        format!("{}

## 程序事实：玩家没有的物品
玩家的背包里没有：{}。如果玩家要使用、出示、交出、装备它们，要说明玩家没有（翻遍行囊也找不到），不能让它凭空出现；玩家想买、寻找、打听它们则照常处理。", ctx.text, missing.join("、"))
    };
    out.missing_items = missing;
    let user = templates.render("gm_plan", json!({ "dsl": dsl, "context": context_text, "input": input, "play": play, "check_rules": check_rules, "web_search": opts.web_search.is_some() }))?;
    let mut plan = ask_limited::<GmPlan>(gm, SYSTEM, &user, opts, LIMIT_PLAN, cancel, &mut out.requests)?.value;
    // 联网搜索：只有用户打开并允许 GM 搜索时才有；结果作为标明来源的资料交给叙述，不执行其中的任何"指令"，也不写进剧本。
    let mut search_ref: Option<String> = None;
    if let (Some(ws), false) = (&opts.web_search, opening)
        && !plan.web_search.trim().is_empty()
    {
        let q: String = plan.web_search.trim().chars().take(100).collect();
        stage("search");
        let rec = match (ws.0)(&q) {
            Ok(results) if !results.is_empty() => {
                search_ref = Some(chronicle_llm::search::as_reference(&q, &results));
                json!({ "query": q, "results": results })
            }
            Ok(_) => json!({ "query": q, "results": [], "error": "没有搜索到结果" }),
            Err(e) => json!({ "query": q, "results": [], "error": e }),
        };
        {
            let w = lock(world)?;
            campaign::record(w.conn(), campaign_id, "web_search", &rec)?;
        }
        out.web_search = Some(rec);
    }
    if opening {
        // 开场只讲述，不掷骰、不推进剧情、不改状态。
        plan.checks.clear();
        plan.transition_edge = None;
        plan.effects.clear();
        plan.effects_on_success.clear();
        plan.effects_on_failure.clear();
    }
    // 模型把字段写进了叙述字符串：截掉 JSON 尾巴并回收字段，玩家不会看到。
    let salvaged = plan.salvage_leaked_fields();
    let rpg_moved = plan.salvage_rpg_effects();
    // 实测：主方案常常漏填买卖、装备、制作、开战字段（只在叙述里写"买下了"）。玩家的话明确提到这些、
    // 而对应字段为空时，再发一个很小的补充请求，只让模型从编号里选；不会每个回合都请求。
    let rpg_extra = rpg_followup(gm, templates, opts, cancel, &ctx, input, &mut plan, &mut out)?;
    out.intent = plan.intent.clone();
    out.off_script = plan.off_script;

    // 决定依据：编号换成原文（记忆 M、设定 W，或剧本里的线索、任务、人物等）。
    out.based_on = plan
        .based_on
        .iter()
        .take(8)
        .filter_map(|c| {
            let c = c.trim().to_uppercase();
            ctx.memory_refs.get(&c).cloned().or_else(|| ctx.refs.get(&c).and_then(|id| ctx.names.get(id)).map(|n| format!("{c} {n}")))
        })
        .collect();

    stage("resolve");
    // 3. 程序执行：检定、效果、剧情跳转、新实体、记忆。
    let script_id = ctx.script.as_ref().map(|s| s.id.clone());
    let mut outcome_lines = vec![];
    {
        let mut w = lock(world)?;
        campaign::record(
            w.conn(),
            campaign_id,
            "gm_plan",
            &json!({ "intent": plan.intent, "off_script": plan.off_script, "checks": plan.checks, "transition": plan.transition_edge, "transition_reason": plan.transition_reason,
                     "effects": plan.effects, "on_success": plan.effects_on_success, "on_failure": plan.effects_on_failure,
                     "npc_speakers": plan.npc_speakers, "new_entities": plan.new_entities, "salvaged": salvaged,
                     "combat_start": plan.combat_start, "combat_action": plan.combat_action, "trade": plan.trade, "item_use": plan.item_use, "craft": plan.craft,
                     "rewards": plan.rewards, "travel_to": plan.travel_to, "rpg_moved_from_effects": rpg_moved, "rpg_followup": rpg_extra,
                     "based_on": out.based_on, "based_on_codes": plan.based_on }),
        )?;
        // 没有技能或难度的"检定"（实测："无"，难度 0）不掷骰。
        // 战斗中的攻击由程序掷骰，GM 另提的"命中检定"不掷（实测会多出一次无意义的检定）。
        let fighting = chronicle_core::combat::active(w.conn(), campaign_id).is_some() || !plan.combat_start.is_empty();
        for chk in plan.checks.iter().filter(|c| !fighting && c.dc > 0 && !matches!(c.skill.trim(), "" | "无" | "none" | "无需")).take(3) {
            let skill = chk.skill.trim().to_string();
            let rules = chronicle_core::rpg::rules(w.conn(), campaign_id);
            // 骰池模式的难度是"需要几个成功"（1–6），其他模式是数值难度。
            let dc = if rules.check.mode == "pool" { chk.dc.clamp(1, 6) } else { chk.dc.clamp(1, 100) };
            // 难度设置由玩家决定，程序统一调整（GM 不能为了推剧情改难度），并写进检定说明。
            let (dc, diff_note) = chronicle_core::rpg::difficulty_dc(&rules, dc);
            let mut modifier = CampaignView { conn: w.conn(), campaign_id }.skill_mod("party", &skill);
            // 环境修正（光照、天气）：程序按玩家所在地点计算。玩家提到火把、灯、提灯等光源时视为有光。
            let loc = chronicle_core::worldsim::party_location(w.conn(), campaign_id)?;
            let local = chronicle_core::worldsim::local_environment(w.conn(), campaign_id, loc.as_deref(), chronicle_core::schedule::clock(w.conn(), campaign_id))?;
            let lit = ["火把", "灯", "火光", "蜡烛", "照明", "手电", "光源"].iter().any(|x| input.contains(x));
            let (env_mod, env_note) = chronicle_core::worldsim::env_check_modifier(&local, &skill, lit);
            modifier += env_mod;
            let adv: i8 = match chk.advantage.trim() {
                "advantage" | "优势" => 1,
                "disadvantage" | "劣势" => -1,
                _ => 0,
            };
            let mut roll = chronicle_core::checks::perform(&mut w, campaign_id, &rules.check, modifier, dc, adv, &format!("{skill} 检定：{}", chk.reason))?;
            if !diff_note.is_empty() {
                roll.explain = format!("{}（{diff_note}）", roll.explain);
            }
            if !env_note.is_empty() {
                roll.explain = format!("{}（环境：{env_note}）", roll.explain);
            }
            let res = CheckResult {
                skill,
                dc,
                roll: roll.natural,
                modifier,
                total: roll.total,
                success: roll.degree.ok(),
                reason: chk.reason.clone(),
                degree: Some(roll.degree),
                detail: Some(roll.clone()),
            };
            outcome_lines.push(format!("- {} 检定：{}{}", res.skill, roll.explain, if roll.degree == chronicle_core::checks::Degree::Partial { "（成功，但要付出代价或留下麻烦）" } else { "" }));
            // 界面：玻璃球展开显示骰子与判定（结果已经确定，动画只是演出）。
            stage(&format!("check:{}", serde_json::to_string(&res).unwrap_or_default()));
            out.checks.push(res);
        }
        let all_ok = out.checks.iter().all(|c| c.success);
        let mut raw = plan.effects.clone();
        if !all_ok {
            // 检定失败时，"发现"类效果不能因为模型把它写进了无条件效果就生效（实测：察觉失败仍揭示了线索）。
            raw.retain(|m| {
                let ty = m.get("type").and_then(|t| t.as_str()).unwrap_or("");
                let gated = matches!(ty, "reveal_clue" | "give_item" | "unlock_node");
                if gated {
                    out.rejected.push(RejectedEffect { effect: Value::Object(m.clone()), reason: "检定失败，这个发现不会发生（模型把它放在了无条件效果里）".into() });
                }
                !gated
            });
        }
        if !out.checks.is_empty() {
            raw.extend(if all_ok { plan.effects_on_success.clone() } else { plan.effects_on_failure.clone() });
        } else {
            raw.extend(plan.effects_on_success.clone());
        }
        let raw: Vec<_> = raw.into_iter().map(|m| ctx.map_refs(m)).collect();
        let (effects, mut rejected) = validate_effects(&w, script_id.as_deref(), &raw);
        // 物品来源：GM 用效果直接给玩家的物品，必须是剧本里的物品（剧本奖励），或者本回合检定通过后发现的。
        // 交易、战利品、报酬走各自的程序流程，不经过这里；没有来源的物品不会凭空出现。
        let found_by_check = !out.checks.is_empty() && all_ok;
        let effects: Vec<_> = effects
            .into_iter()
            .filter(|e| {
                let chronicle_core::rules::Effect::GiveItem { holder, item, .. } = e else { return true };
                if holder != chronicle_core::rpg::PARTY || found_by_check || script_item_kind(&w, script_id.as_deref(), item) {
                    return true;
                }
                rejected.push(RejectedEffect {
                    effect: serde_json::to_value(e).unwrap_or(Value::Null),
                    reason: "物品没有合理来源：不是剧本奖励，也不是检定通过后发现的（交易、战利品、报酬请走对应流程）".into(),
                });
                false
            })
            .collect();
        if !effects.is_empty() {
            match campaign::apply_effects(&mut w, campaign_id, &effects, "GM 回合") {
                Ok(_) => out.applied = effects,
                Err(e) => {
                    // 整组效果在一个事务中执行：任何一条失败（如物品不足）都不修改状态。
                    for eff in effects {
                        out.rejected.push(RejectedEffect { effect: serde_json::to_value(&eff).unwrap_or(Value::Null), reason: format!("执行失败，整组未生效：{e}") });
                    }
                }
            }
        }
        out.rejected.extend(rejected);
        if !out.rejected.is_empty() {
            campaign::record(w.conn(), campaign_id, "effects_rejected", &json!({ "rejected": out.rejected }))?;
        }
        for e in &out.applied {
            outcome_lines.push(format!("- 状态变化：{}", serde_json::to_string(e).unwrap_or_default()));
        }
        for r in &out.rejected {
            outcome_lines.push(format!("- 被拒绝的效果（没有发生）：{}（{}）", r.effect, r.reason));
        }

        // 人物成长、物品经济与战斗：GM 只提出意图，数值全部由程序结算。
        let rpg_lines = apply_rpg(&mut w, campaign_id, &ctx, &plan, input, &mut out);
        outcome_lines.extend(rpg_lines.iter().map(|l| format!("- {l}")));
        out.rpg_updates.extend(rpg_lines);

        if let Some(edge) = plan.transition_edge.as_ref().filter(|e| !e.trim().is_empty()) {
            if let Some((_, edge_id, label)) = ctx.resolve_edge(edge).cloned() {
                match campaign::take_edge(&mut w, campaign_id, &edge_id) {
                    Ok(t) => {
                        outcome_lines.push(if t.taken {
                            format!("- 剧情推进：「{label}」成功")
                        } else {
                            format!("- 剧情推进：「{label}」的条件没有满足，剧情没有推进")
                        });
                        out.transition = Some(t);
                    }
                    Err(e) => out.transition_note = format!("剧情推进失败：{e}"),
                }
            } else {
                out.transition_note = format!("提议的分支 {edge} 不是当前可选分支，已忽略");
            }
            if !out.transition_note.is_empty() {
                outcome_lines.push(format!("- {}", out.transition_note));
            }
        }

        for e in plan.new_entities.iter().take(2) {
            let r = propose_entity(&mut w, campaign_id, e);
            outcome_lines.push(format!("- 新内容「{}」：{}", r.name, r.note));
            out.entities.push(r);
        }
        for f in plan.remember.iter().take(5) {
            let f = ctx.humanize(f);
            campaign::add_fact(w.conn(), campaign_id, &f)?;
            out.remembered.push(f);
        }
        // 新发现：自动记入玩家的调查板（只写玩家能感知的内容）。
        for d in plan.discoveries.iter().filter(|d| !d.title.trim().is_empty()).take(3) {
            let (title, detail) = (ctx.humanize(&d.title), ctx.humanize(&d.detail));
            // 发现是给玩家看的：涉及真相或泄露主持人信息的不记入调查板。
            if chronicle_core::quests::looks_like_spoiler(&format!("{title}{detail}")) || !find_leaks(&format!("{title}{detail}"), &ctx.hidden).is_empty() {
                continue;
            }
            chronicle_core::investigation::add_clue(&mut w, campaign_id, &title, &detail, "gm")?;
            out.discoveries.push(title.trim().to_string());
        }
        // 因果与伏笔（开场回合不记）。
        if !opening {
            let (_, refs) = chronicle_core::story::gm_text(w.conn(), campaign_id)?;
            let find = |code: &str| refs.iter().find(|(c, _)| c.eq_ignore_ascii_case(code.trim())).map(|(_, id)| id.clone());
            for code in plan.consequences_done.iter().take(4) {
                if let Some(id) = find(code).filter(|id| id.starts_with('k')) {
                    chronicle_core::story::mark_occurred(&mut w, campaign_id, &id)?;
                    out.quest_updates.push("之前的决定带来了后果".into());
                }
            }
            for code in plan.payoffs.iter().take(3) {
                if let Some(id) = find(code).filter(|id| id.starts_with('f')) {
                    chronicle_core::story::pay_off(&mut w, campaign_id, &id, &plan.intent)?;
                }
            }
            for c in plan.consequences.iter().filter(|c| !c.consequence.trim().is_empty()).take(2) {
                chronicle_core::story::add_consequence(&mut w, campaign_id, &ctx.humanize(&c.decision), &ctx.humanize(&c.consequence), c.after_turns.clamp(0, 30))?;
            }
            for f in plan.foreshadow.iter().filter(|f| !f.trim().is_empty()).take(1) {
                chronicle_core::story::plant(&mut w, campaign_id, &ctx.humanize(f))?;
            }
        }
        // 剧情中出现的新任务（最多 1 个）与任务变化（最多 2 条）。
        for q in plan.new_quests.iter().filter(|q| !q.title.trim().is_empty()).take(1) {
            let nq = chronicle_core::quests::NewQuest {
                title: q.title.clone(),
                objective: ctx.humanize(&q.objective),
                quest_type: if matches!(q.quest_type.as_str(), "main" | "side" | "hidden") { q.quest_type.clone() } else { "side".into() },
                deadline_turns: (q.deadline_turns > 0).then_some(q.deadline_turns.min(100)),
                ..Default::default()
            };
            chronicle_core::quests::add_quest(&mut w, campaign_id, &nq, &format!("GM：剧情中出现新任务「{}」", q.title.trim()))?;
            out.quest_updates.push(format!("新任务「{}」", q.title.trim()));
        }
        for n in plan.quest_notes.iter().take(2) {
            let id = ctx.refs.get(&n.quest.trim().to_uppercase()).cloned().unwrap_or_else(|| n.quest.trim().to_string());
            if chronicle_core::quests::quest_exists(w.conn(), campaign_id, &id)? {
                let text = ctx.humanize(&n.text);
                // 可能评价玩家推理、暗示真相或原样泄露主持人信息的，只给主持人看。
                let gm_only = chronicle_core::quests::looks_like_spoiler(&text) || !find_leaks(&text, &ctx.hidden).is_empty();
                chronicle_core::quests::add_quest_note_ex(&mut w, campaign_id, &id, &text, gm_only)?;
                if !gm_only {
                    out.quest_updates.push(format!("任务变化：{}", text.trim()));
                }
            }
        }
    }

    // 4. 有程序判定结果时，AI 按实际结果重写叙述。
    if let Some(r) = &search_ref {
        outcome_lines.push(format!("- {r}"));
    } else if let Some(e) = out.web_search.as_ref().and_then(|w| w.get("error")).and_then(|e| e.as_str()) {
        outcome_lines.push(format!("- 联网搜索没有结果（{e}）：按你自己的知识叙述，不要假装查到了资料"));
    }
    let needs_narration = !out.checks.is_empty() || out.transition.is_some() || !out.rejected.is_empty() || !out.transition_note.is_empty() || !out.rpg_updates.is_empty() || out.web_search.is_some();
    out.narration = if needs_narration {
        stage("narrate");
        let ctx2 = {
            let w = lock(world)?;
            build_gm_context(&w, campaign_id, input, opts.context_budget)?
        };
        let plan_text = format!("意图：{}；原叙述：{}", plan.intent, plan.narration);
        let user = templates.render("gm_narrate", json!({ "context": ctx2.text, "input": input, "plan": plan_text, "outcome": outcome_lines.join("\n") }))?;
        let n = ask_limited::<Narration>(gm, SYSTEM, &user, opts, LIMIT_NARRATION, cancel, &mut out.requests)?.value;
        if n.choices.len() >= 2 {
            plan.choices = n.choices;
        } else if !n.choices.is_empty() {
            let old = std::mem::take(&mut plan.choices);
            plan.choices = n.choices.into_iter().chain(old).collect();
        }
        n.narration
    } else {
        plan.narration.clone()
    };
    // GM 替在场 NPC 说的台词：从叙述中删掉，改由 NPC 代理亲自回答（NPC 代理有知识隔离）。
    let names: Vec<&str> = ctx.npcs.iter().map(|n| n.node.title.as_str()).collect();
    let mut redirected: Vec<&NpcCard> = vec![];
    // 重复几次：相邻的两段台词偶尔只有第一段被识别。
    for _ in 0..3 {
        let extracted = crate::speech::extract_ventriloquism(&out.narration, &names);
        if extracted.speakers.is_empty() {
            break;
        }
        out.narration = extracted.narration;
        for idx in extracted.speakers {
            let n = &ctx.npcs[idx];
            if !redirected.iter().any(|r| r.node.id == n.node.id) {
                out.redirected_speakers.push(n.node.title.clone());
                redirected.push(n);
            }
        }
    }
    if opening && !out.narration.trim().is_empty() {
        out.narration = ensure_welcome(&out.narration, ctx.script.as_ref().map(|s| s.title.as_str()));
    }
    out.leaks = find_leaks(&out.narration, &ctx.hidden);
    // 剧情质量（只提示，不修改）：换了说法的泄露；已经死去 / 失踪的人物在叙述里出现。
    for p in crate::leak::paraphrase_leaks(&out.narration, &ctx.hidden).into_iter().take(2) {
        out.quality.push(format!("叙述可能换了说法透露了主持人信息：「{p}」"));
    }
    {
        let w = lock(world)?;
        let st = campaign::campaign_state(w.conn(), campaign_id)?;
        for (k, v) in &st.entities {
            if let Some(id) = k.strip_suffix("|life_status")
                && matches!(v.as_str(), Some("dead") | Some("missing"))
                && let Some(n) = chronicle_core::nodes::get_node(w.conn(), id)?
                && n.title.chars().count() >= 2
                && out.narration.contains(&n.title)
                && !["死", "尸", "墓", "遗体", "失踪", "下落不明", "生前", "曾经", "回忆"].iter().any(|x| out.narration.contains(x))
            {
                out.quality.push(format!("「{}」已经{}，叙述里却像在场一样出现", n.title, if v.as_str() == Some("dead") { "死亡" } else { "失踪" }));
            }
        }
        if !out.quality.is_empty() {
            campaign::record(w.conn(), campaign_id, "quality", &json!({ "notes": out.quality }))?;
        }
    }
    // 叙述与程序结算不符（实测：结算是击倒，叙述却写"没能触到"）：记下来给主持人看。
    out.conflicts = chronicle_core::director::narration_mismatches(&out.narration, &out.rpg_updates);
    {
        let w = lock(world)?;
        let revealed: Vec<String> = out.applied.iter().filter_map(|e| if let Effect::RevealClue { clue } = e { Some(clue.clone()) } else { None }).collect();
        for (id, title) in chronicle_core::director::unrevealed_clue_mentions(w.conn(), campaign_id, &out.narration)? {
            if !revealed.contains(&id) {
                out.conflicts.push(format!("叙述里提到了线索「{title}」，但没有揭示（调查板上不会有它；可以在控制台一键揭示）"));
            }
        }
    }
    let en = chronicle_core::director::english_words(&out.narration);
    if !en.is_empty() {
        out.conflicts.push(format!("叙述里夹杂英文：{}", en.join("、")));
    }
    {
        let w = lock(world)?;
        if !out.redirected_speakers.is_empty() {
            campaign::record(w.conn(), campaign_id, "speech_redirected", &json!({ "npcs": out.redirected_speakers }))?;
        }
        campaign::record(w.conn(), campaign_id, "narration", &json!({ "text": out.narration, "leaks": out.leaks, "opening": opening }))?;
        out.choices = clean_choices(&plan.choices, &out.narration);
        // 可选行动不能用到玩家没有的物品。
        let sid = ctx.script.as_ref().map(|s| s.id.clone());
        out.choices.retain(|c| chronicle_core::rpg::missing_items(w.conn(), campaign_id, sid.as_deref(), c).map(|m| m.is_empty()).unwrap_or(true));
        // 模型给的少于 2 条（实测偶尔只有 1 条）：程序按场景补几条简单的（和在场的人说话、去能到的地方、观察四周）。
        if out.choices.len() < 2 && !out.narration.trim().is_empty() {
            let mut extra: Vec<String> = vec![];
            if let Some(n) = ctx.npcs.iter().find(|n| !out.choices.iter().any(|c| c.contains(n.node.title.as_str()))) {
                extra.push(format!("和{}说话", n.node.title));
            }
            if let Some(r) = chronicle_core::worldsim::reachable(w.conn(), campaign_id, 1).ok().and_then(|v| v.into_iter().next()) {
                extra.push(format!("前往{}", r.title));
            }
            extra.push("仔细观察四周".to_string());
            for e in extra {
                if out.choices.len() >= 2 {
                    break;
                }
                if !out.choices.contains(&e) {
                    out.choices.push(e);
                }
            }
        }
        if !out.choices.is_empty() {
            campaign::record(w.conn(), campaign_id, "choices", &json!({ "items": out.choices }))?;
        }
        campaign::record(w.conn(), campaign_id, "context_trace", &serde_json::to_value(&ctx.trace).unwrap_or(Value::Null))?;

    }

    // 5. NPC 回应：每个 NPC 只拿到自己的资料与玩家可见的信息。
    let mut speakers: Vec<&NpcCard> = vec![];
    let requested = plan.npc_speakers.iter().filter_map(|r| ctx.resolve_npc(r));
    for n in requested.chain(redirected) {
        if !speakers.iter().any(|s| s.node.id == n.node.id) && speakers.len() < 2 {
            speakers.push(n);
        }
    }
    if !speakers.is_empty() {
        stage("npc");
    }
    for me in speakers {
        let (nctx, player) = {
            let w = lock(world)?;
            // 实测：不告诉 NPC 玩家是谁时，模型会把叙述里的「你」当成自己，替玩家说话。
            let player = chronicle_core::pc::get(w.conn(), campaign_id)
                .map(|c| if c.identity.trim().is_empty() { c.name } else { format!("{}（{}）", c.name, c.identity.trim()) })
                .unwrap_or_else(|| "玩家的角色".to_string());
            (build_npc_context(&w, campaign_id, me, &ctx.npcs, ctx.script.as_ref(), ctx.current.as_ref())?, player)
        };
        let user = templates.render(
            "npc_reply",
            json!({ "name": nctx.name, "card": nctx.card, "knowledge": nctx.knowledge, "attitude": nctx.attitude,
                    "memories": nctx.memories, "mood": nctx.mood, "scene": nctx.scene, "recent": nctx.recent, "input": input, "player": player }),
        )?;
        // 实测：本地模型偶尔给出无法解析的 NPC 回应。叙述已经写入，不能因为一句台词让整个回合失败：跳过这个人物的台词并记下原因。
        let mut reply = match ask_limited::<NpcReplyOut>(npc, NPC_SYSTEM, &user, opts, LIMIT_NPC, cancel, &mut out.requests) {
            Ok(r) => r.value,
            Err(GmError::Output(e)) => {
                out.conflicts.push(format!("{}的回应没能生成（模型输出无法使用：{e}），这回合略过其台词", me.node.title));
                continue;
            }
            Err(e) => return Err(e),
        };
        reply.salvage_leaked_fields();
        let delta = reply.attitude_change.clamp(-2, 2);
        // 情绪变化：每项每回合最多 ±10，原因写明是对哪句话的反应。
        let md = reply.mood;
        let clamp = |v: i64| v.clamp(-10, 10);
        let mood_eff = (md.fear != 0 || md.anger != 0 || md.suspicion != 0 || md.stress != 0 || md.loyalty != 0).then(|| Effect::AdjustMood {
            npc: me.node.id.clone(),
            fear: clamp(md.fear),
            anger: clamp(md.anger),
            suspicion: clamp(md.suspicion),
            stress: clamp(md.stress),
            loyalty: clamp(md.loyalty),
            reason: format!("对话中的反应：玩家说「{}」", input.chars().take(40).collect::<String>()),
        });
        let leaks = find_leaks(&reply.reply, &nctx.hidden);
        {
            let mut w = lock(world)?;
            if delta != 0 {
                // 态度由程序维护（GM 提议的效果不能直接改它）。
                let eff = Effect::IncVar { key: format!("attitude.{}", me.node.id), by: delta as f64 };
                // 同时记入人物关系（NPC 对玩家的好感与信任），原因写明来自哪句对话。
                let said: String = input.chars().take(40).collect();
                let bond = Effect::AdjustBond {
                    from: me.node.id.clone(),
                    to: "party".into(),
                    kind: String::new(),
                    strength: delta.abs() * 3,
                    trust: delta * 3,
                    affection: delta * 5,
                    hostility: if delta < 0 { -delta * 4 } else { 0 },
                    reason: format!("对话中态度{}：玩家说「{said}」", if delta > 0 { "变好" } else { "变差" }),
                    mutual: false,
                };
                campaign::apply_effects(&mut w, campaign_id, &[eff, bond], &format!("{} 的态度变化", me.node.title))?;
            }
            if let Some(e) = &mood_eff {
                campaign::apply_effects(&mut w, campaign_id, std::slice::from_ref(e), &format!("{} 的情绪变化", me.node.title))?;
            }
            for m in reply.remember.iter().take(2) {
                if !m.trim().is_empty() {
                    campaign::add_npc_memory(&w, campaign_id, &me.node.id, m, 5)?;
                }
            }
            campaign::record(w.conn(), campaign_id, "npc", &json!({ "npc_id": me.node.id, "name": me.node.title, "text": reply.reply, "leaks": leaks, "memories": nctx.memory_list }))?;
        }
        let en = chronicle_core::director::english_words(&reply.reply);
        if !en.is_empty() {
            out.conflicts.push(format!("{}的台词夹杂英文：{}", me.node.title, en.join("、")));
        }
        out.npc_lines.push(NpcLine { npc_id: me.node.id.clone(), name: me.node.title.clone(), text: reply.reply, attitude_change: delta, leaks });
    }

    // 6. 回合结束：推进故事时间（NPC 按日程活动、情绪平复），回合数 +1，再由程序推进任务。
    {
        let mut w = lock(world)?;
        let resolve_place = |r: &str| ctx.refs.get(&r.trim().to_uppercase()).cloned().filter(|id| ctx.refs.iter().any(|(k, v)| k.starts_with('L') && v == id));
        let heard: Vec<String> = plan.places_heard.iter().take(3).filter_map(|r| resolve_place(r)).collect();
        if !heard.is_empty() {
            out.places_heard = chronicle_core::worldsim::hear_of_places(&mut w, campaign_id, &heard)?;
        }
        // 旅行：耗时由程序按路程与天气计算，期间世界照常运转。
        let mut dest = plan.travel_to.as_deref().filter(|t| !t.trim().is_empty()).and_then(resolve_place);
        // 模型没填 travel_to，但玩家明说要去 / 走进某个认识的地点：附近的直接到达，远处的按旅行计算（战斗中、剧情刚推进时不处理）。
        let fighting_now = chronicle_core::combat::active(w.conn(), campaign_id).is_some() || out.combat.as_ref().is_some_and(|c| c.status == "active");
        if dest.is_none() && !opening && !fighting_now && !out.transition.as_ref().is_some_and(|t| t.taken)
            && let Some((id, title, nearby)) = chronicle_core::worldsim::place_from_words(w.conn(), campaign_id, input)?
        {
            if nearby {
                chronicle_core::worldsim::move_nearby(&mut w, campaign_id, &id, &title)?;
                out.travel = format!("来到「{title}」");
            } else {
                dest = Some(id);
            }
        }
        let travelled = match dest {
            Some(to) => match chronicle_core::worldsim::travel(&mut w, campaign_id, &to, "旅行") {
                Ok((p, notes)) => {
                    out.travel = format!("前往「{}」，路上 {}", p.to_title, chronicle_core::worldsim::format_duration(p.minutes));
                    out.npc_activity = notes;
                    true
                }
                Err(e) => {
                    out.travel = format!("没能出发：{e}");
                    false
                }
            },
            None => false,
        };
        if !travelled {
            // 战斗中一回合只过去 1 分钟左右。
            let in_combat = chronicle_core::combat::active(w.conn(), campaign_id).is_some() || out.combat.is_some();
            let minutes = if in_combat { 1 } else { plan.time_passes_minutes.unwrap_or(15).clamp(0, 720) };
            out.npc_activity = chronicle_core::schedule::advance(&mut w, campaign_id, minutes, &format!("故事时间过去 {minutes} 分钟"))?;
        }
        if !out.transition.as_ref().is_some_and(|t| t.taken) {
            campaign::advance_turn(w.conn(), campaign_id)?;
        }
        out.quest_updates.extend(chronicle_core::quests::tick(&mut w, campaign_id)?);
        if !out.conflicts.is_empty() {
            campaign::record(w.conn(), campaign_id, "conflict", &json!({ "items": out.conflicts }))?;
        }
        // 导演：记录这一回合的紧张度与进展（程序计算，不调用模型）。
        chronicle_core::director::record_beat(&mut w, campaign_id)?;
        out.time = chronicle_core::schedule::format_clock(chronicle_core::schedule::clock(w.conn(), campaign_id));
        let c = campaign::get_campaign(w.conn(), campaign_id)?;
        out.turn = c.turn;
        out.current_node = c.current_plot_node;
    }
    Ok(out)
}

/// 玩家的话里有哪些操作意图，主方案却没有填（只看关键词，宁缺毋滥）。
pub fn missing_rpg_intents(ctx: &crate::context::GmContext, input: &str, plan: &GmPlan) -> Vec<&'static str> {
    let has = |ws: &[&str]| ws.iter().any(|w| input.contains(w));
    let code = |p: char| ctx.refs.keys().any(|k| k.starts_with(p) && k[1..].chars().all(|c| c.is_ascii_digit()));
    let in_combat = ctx.text.contains("战斗进行中");
    let mut v = vec![];
    // 只认明确的买卖动作（实测："都卖些什么、多少钱？"这种询价被误判成购买）。
    if plan.trade.is_empty() && has(&["买下", "我买", "买一", "买两", "买几", "买了", "要买", "购买", "卖给", "卖掉", "卖了", "出售", "成交"]) && (code('S') || code('B')) {
        v.push("trade");
    }
    if plan.item_use.as_deref().is_none_or(|s| s.trim().is_empty()) && !in_combat && has(&["装备", "佩", "穿上", "戴上", "换上", "拿出", "喝", "服用", "使用", "用上", "吃下"]) && code('B') {
        v.push("item_use");
    }
    if plan.craft.as_deref().is_none_or(|s| s.trim().is_empty()) && has(&["制作", "做一", "做个", "调制", "熬", "打造", "锻造", "配制", "做成"]) && code('R') {
        v.push("craft");
    }
    if plan.combat_start.is_empty() && !in_combat && has(&["攻击", "扑", "袭", "动手", "砍", "拔剑", "打起来", "开打", "冲上去", "围攻", "埋伏", "冲向", "砸", "劈", "刺向", "揍", "射向", "开枪", "迎战", "拼了"]) {
        v.push("combat_start");
    }
    v
}

#[allow(clippy::too_many_arguments)]
fn rpg_followup(gm: &dyn LlmClient, templates: &Templates, opts: &GmOptions, cancel: &CancelToken, ctx: &crate::context::GmContext, input: &str, plan: &mut GmPlan, out: &mut TurnResult) -> Result<Vec<&'static str>> {
    let missing = missing_rpg_intents(ctx, input, plan);
    if missing.is_empty() {
        return Ok(missing);
    }
    let mut codes: Vec<(String, String)> = ctx
        .refs
        .iter()
        .filter(|(k, _)| matches!(k.chars().next(), Some('S' | 'B' | 'R' | 'N')) && k[1..].chars().all(|c| c.is_ascii_digit()))
        .map(|(k, id)| (k.clone(), ctx.names.get(id).cloned().unwrap_or_default()))
        .collect();
    codes.sort_by_key(|(k, _)| (k.chars().next(), k[1..].parse::<u32>().unwrap_or(0)));
    let label = |k: &str| match k.chars().next() {
        Some('S') => "商品",
        Some('B') => "背包",
        Some('R') => "配方",
        _ => "人物",
    };
    let text = codes.iter().map(|(k, n)| format!("- {k}（{}）{n}", label(k))).collect::<Vec<_>>().join("
");
    let user = templates.render("rpg_intent", json!({ "codes": text, "input": input, "intent": plan.intent }))?;
    // 补充识别失败不影响这一回合。
    let Ok(r) = ask_limited::<crate::outputs::RpgIntentOut>(gm, SYSTEM, &user, opts, LIMIT_NPC, cancel, &mut out.requests) else { return Ok(vec![]) };
    let r = r.value;
    let mut used = vec![];
    // 数量为 0 视为没有决定（实测：询价时补充识别给出了数量 0 的购买）。
    let trades: Vec<_> = r.trade.into_iter().filter(|t| t.qty >= 1).take(3).collect();
    if missing.contains(&"trade") && !trades.is_empty() {
        plan.trade = trades;
        used.push("trade");
    }
    if missing.contains(&"item_use") && r.item_use.as_deref().is_some_and(|s| !s.trim().is_empty()) {
        plan.item_use = r.item_use;
        used.push("item_use");
    }
    if missing.contains(&"craft") && r.craft.as_deref().is_some_and(|s| !s.trim().is_empty()) {
        plan.craft = r.craft;
        used.push("craft");
    }
    if missing.contains(&"combat_start") && !r.combat_start.is_empty() {
        plan.combat_start = r.combat_start;
        used.push("combat_start");
    }
    Ok(used)
}

/// 执行方案里的奖励、买卖、物品、制作与战斗。每一项失败都只影响这一项，原因写进结算说明。
fn apply_rpg(w: &mut World, campaign_id: &str, ctx: &crate::context::GmContext, plan: &GmPlan, input: &str, out: &mut TurnResult) -> Vec<String> {
    use chronicle_core::{combat, rpg};
    let mut lines = vec![];
    let resolve = |code: &str| -> Option<String> {
        let c = code.trim();
        if c.is_empty() {
            return None;
        }
        ctx.refs.get(&c.to_uppercase()).cloned().or_else(|| ctx.names.iter().find(|(_, n)| n.as_str() == c).map(|(id, _)| id.clone()))
    };
    let actor = |code: &str| -> Option<String> {
        let c = code.trim();
        if c.is_empty() || c.eq_ignore_ascii_case("party") || c == "玩家" {
            Some(rpg::PARTY.to_string())
        } else {
            ctx.resolve_npc(c).map(|n| n.node.id.clone())
        }
    };
    let r = rpg::rules(w.conn(), campaign_id);
    for rw in plan.rewards.iter().take(3) {
        let Some(who) = actor(&rw.who) else {
            lines.push(format!("奖励没有发放：不认识「{}」", rw.who));
            continue;
        };
        let reason = ctx.humanize(&rw.reason);
        if rw.xp > 0 {
            let xp = rw.xp.min(r.max_xp_award);
            match rpg::award_xp(w, campaign_id, &who, xp, &reason) {
                Ok(n) => lines.extend(n),
                Err(e) => lines.push(format!("经验没有发放：{e}")),
            }
            if rw.xp > xp {
                lines.push(format!("经验奖励超过上限，按 {xp} 发放"));
            }
        }
        if rw.money != 0 {
            let m = rw.money.clamp(-r.max_money_award, r.max_money_award);
            match rpg::change_money(w, campaign_id, &who, m, &reason) {
                Ok(left) => lines.push(format!("{}{}（{reason}），现有 {}", if m > 0 { "得到 " } else { "付出 " }, rpg::format_money(&r, m.abs()), rpg::format_money(&r, left))),
                Err(e) => lines.push(format!("金钱变化没有发生：{e}")),
            }
        }
    }
    for t in plan.trade.iter().take(3) {
        // 没写商人时（实测会留空）：在场商人里卖这件东西的那位；卖东西时取第一位在场商人。
        let item_id = resolve(&t.item);
        let merchant = ctx.resolve_npc(&t.merchant).map(|n| n.node.id.clone()).or_else(|| {
            ctx.npcs.iter().map(|n| n.node.id.clone()).find(|id| {
                rpg::shop(w.conn(), campaign_id, id).ok().flatten().is_some_and(|s| t.kind == "sell" || item_id.as_ref().is_some_and(|i| s.items.iter().any(|x| &x.item.id == i)))
            })
        });
        let (Some(m), Some(item)) = (merchant, item_id) else {
            lines.push(format!("交易没有发生：不认识商人「{}」或商品「{}」", t.merchant, t.item));
            continue;
        };
        let qty = t.qty.max(1);
        let res = if t.kind == "sell" { rpg::sell(w, campaign_id, &m, &item, qty) } else { rpg::buy(w, campaign_id, &m, &item, qty) };
        lines.push(match res {
            Ok(s) => s,
            Err(e) => format!("交易没有成功：{e}"),
        });
    }
    // 战斗中使用物品要消耗行动点、作用在参战者身上：交给战斗行动（实测：药水被当作战斗外使用，恢复了 0 点又被用掉）。
    let in_combat = combat::active(w.conn(), campaign_id).is_some();
    // 战斗中的 item_use 在下面的战斗部分处理（换装备或转为战斗行动）。
    if let Some(code) = plan.item_use.as_deref().filter(|c| !c.trim().is_empty()).filter(|_| !in_combat) {
        match resolve(code) {
            Some(id) => {
                let slot = rpg::item_info(w.conn(), &id).ok().flatten().map(|i| i.slot).unwrap_or_default();
                let res = if slot.is_empty() { rpg::use_item(w, campaign_id, rpg::PARTY, &id) } else { rpg::equip(w, campaign_id, rpg::PARTY, &id) };
                match res {
                    Ok(s) => lines.push(s),
                    // 制作时模型常把材料也写进 item_use：不提示。
                    Err(_) if plan.craft.as_deref().is_some_and(|c| !c.trim().is_empty()) => {}
                    Err(e) => lines.push(format!("物品没有用上：{e}")),
                }
            }
            None => lines.push(format!("物品没有用上：不认识「{code}」")),
        }
    }
    // 战斗中不制作（实测：模型在战斗回合里把武器编号也写进了 craft）。
    if let Some(code) = plan.craft.as_deref().filter(|c| !c.trim().is_empty()).filter(|_| !in_combat) {
        match resolve(code) {
            Some(id) => match rpg::craft(w, campaign_id, rpg::PARTY, &id) {
                Ok(c) => lines.push(if c.dc > 0 { format!("{}（检定 {}{:+} 对难度 {}）", c.text, c.roll, c.modifier, c.dc) } else { c.text }),
                Err(e) => lines.push(format!("没能制作：{e}")),
            },
            None => lines.push(format!("没能制作：不认识「{code}」")),
        }
    }
    let active = combat::active(w.conn(), campaign_id);
    let mut plan_action = plan.combat_action.clone();
    if active.is_some()
        && plan_action.is_none()
        && let Some(code) = plan.item_use.as_deref().filter(|c| !c.trim().is_empty())
    {
        plan_action = Some(crate::outputs::CombatActionOut { kind: "use_item".into(), target: String::new(), item: code.to_string() });
    }
    // 战斗中装备武器（item_use 是武器）：直接换上。
    if active.is_some()
        && let Some(id) = plan.item_use.as_deref().and_then(resolve)
        && rpg::item_info(w.conn(), &id).ok().flatten().is_some_and(|i| !i.slot.is_empty())
    {
        if !rpg::equipment(w.conn(), campaign_id, rpg::PARTY).values().any(|v| *v == id)
            && let Ok(t) = rpg::equip(w, campaign_id, rpg::PARTY, &id)
        {
            lines.push(t);
        }
        if plan.combat_action.is_none() {
            plan_action = None;
        }
    }
    if active.is_none() && !plan.combat_start.is_empty() {
        let enemies: Vec<combat::EnemySpec> = plan
            .combat_start
            .iter()
            .filter(|e| !e.name.trim().is_empty())
            .map(|e| {
                // 只有敌人的名字确实指这个人物时才让他参战（实测：模型给"野狼"填了铁匠的编号，结果两个铁匠来打玩家）。
                let npc = ctx.resolve_npc(&e.npc).filter(|n| e.name.contains(n.node.title.as_str()) || n.node.title.contains(e.name.trim())).map(|n| n.node.id.clone());
                combat::EnemySpec { name: e.name.trim().to_string(), count: if npc.is_some() { 1 } else { e.count.clamp(1, 6) }, npc, level: e.level.clamp(1, 20), loot: vec![], boss: e.boss }
            })
            .collect();
        // 战斗原因用玩家的原话（实测：模型有时把字段名写进 intent）。
        let reason: String = input.trim().chars().take(30).collect();
        // 同伴：在场、不是敌人的人物。
        let enemy_npcs: Vec<String> = enemies.iter().filter_map(|e| e.npc.clone()).collect();
        let allies: Vec<String> = plan.combat_allies.iter().filter_map(|a| ctx.resolve_npc(a)).map(|n| n.node.id.clone()).filter(|id| !enemy_npcs.contains(id)).take(4).collect();
        match combat::start_locked(w, campaign_id, &enemies, &allies, &reason, &plan.combat_no_escape) {
            Ok(rep) => {
                lines.extend(rep.lines);
                out.combat = Some(rep.combat);
            }
            Err(e) => lines.push(format!("战斗没有开始：{e}")),
        }
    } else if let (Some(_), Some(a)) = (&active, &plan_action) {
        let item = resolve(&a.item).unwrap_or_default();
        // 攻击时指明了背包里的武器（实测：模型会在 item 里写武器编号）：先装备上。
        if a.kind == "attack"
            && !item.is_empty()
            && rpg::item_info(w.conn(), &item).ok().flatten().is_some_and(|i| i.slot == "weapon")
            && !rpg::equipment(w.conn(), campaign_id, rpg::PARTY).values().any(|v| *v == item)
            && let Ok(t) = rpg::equip(w, campaign_id, rpg::PARTY, &item)
        {
            lines.push(t);
        }
        let mut target = resolve(&a.target).and_then(|id| ctx.names.get(&id).cloned()).unwrap_or_else(|| a.target.clone());
        // 目标是"离我最近的那只狼"这类描述时：在名字被提到的敌人里取最近的，都没提到就取最近的敌人。
        if let Some(c) = &active
            && matches!(a.kind.trim(), "attack" | "move")
            && !c.units.iter().any(|u| u.out.is_empty() && (u.id == target || u.name == target))
            && let Some(me) = c.units.iter().find(|u| u.actor.as_deref() == Some(rpg::PARTY))
        {
            let foes: Vec<_> = c.units.iter().filter(|u| u.side == "enemy" && u.out.is_empty()).collect();
            let named: Vec<_> = foes.iter().filter(|u| target.contains(u.name.trim_end_matches(|ch: char| ch.is_ascii_digit()))).collect();
            let pool: Vec<_> = if named.is_empty() { foes.iter().collect() } else { named };
            if let Some(t) = pool.into_iter().min_by_key(|u| combat::dist(me, u)) {
                target = t.id.clone();
            }
        }
        let act = combat::CombatAction { kind: a.kind.trim().to_string(), target, item, x: None, y: None, custom: if a.kind == "custom" { a.item.clone() } else { String::new() } };
        // 一个 GM 回合 = 玩家完整的一个战斗回合：目标太远时先靠近再攻击，行动后自动结束回合。
        let mut res = combat::act(w, campaign_id, &act);
        // 目标太远：在行动点允许的范围内一直靠近，进入射程后攻击。
        for _ in 0..4 {
            if !(act.kind == "attack" && res.as_ref().is_err_and(|e| e.to_string().contains("太远"))) {
                break;
            }
            let mv = combat::CombatAction { kind: "move".into(), target: act.target.clone(), ..Default::default() };
            match combat::act(w, campaign_id, &mv) {
                Ok(rep) => {
                    lines.extend(rep.lines);
                    out.combat = Some(rep.combat);
                    res = combat::act(w, campaign_id, &act);
                }
                Err(_) => {
                    res = combat::active(w.conn(), campaign_id).map(|c| combat::CombatReport { lines: vec![], combat: c }).ok_or_else(|| chronicle_core::CoreError::Invalid("战斗已结束".into()));
                    break;
                }
            }
        }
        if res.as_ref().is_err_and(|e| e.to_string().contains("太远") || e.to_string().contains("行动点不够")) {
            lines.push("这一回合没能接近到可以攻击的距离".into());
            res = combat::active(w.conn(), campaign_id).map(|c| combat::CombatReport { lines: vec![], combat: c }).ok_or_else(|| chronicle_core::CoreError::Invalid("战斗已结束".into()));
        }
        match res {
            Ok(rep) => {
                lines.extend(rep.lines);
                out.combat = Some(rep.combat);
            }
            Err(e) => lines.push(format!("战斗行动没有执行：{e}")),
        }
        if combat::active(w.conn(), campaign_id).is_some_and(|c| c.players_turn())
            && let Ok(rep) = combat::act(w, campaign_id, &combat::CombatAction { kind: "end_turn".into(), ..Default::default() })
        {
            lines.extend(rep.lines);
            out.combat = Some(rep.combat);
        }
    } else if let Some(c) = active {
        out.combat = Some(c);
    }
    // GM 用效果给的物品记下来源。
    for e in &out.applied {
        if let chronicle_core::rules::Effect::GiveItem { holder, item, .. } = e
            && holder == rpg::PARTY
        {
            let _ = campaign::apply_effects(
                w,
                campaign_id,
                &[chronicle_core::rules::Effect::UpsertRecord { kind: "item_source".into(), id: format!("{holder}|{item}"), data: json!({ "source": if script_item_kind(w, ctx.script.as_ref().map(|s| s.id.as_str()), item) { "剧本奖励" } else { "检定后发现" } }) }],
                "物品来源",
            );
        }
    }
    lines
}

/// 生成会话摘要草稿（用户确认后才进入战役记忆）。
pub fn summarize(world: &Mutex<World>, client: &dyn LlmClient, templates: &Templates, opts: &GmOptions, campaign_id: &str, cancel: &CancelToken) -> Result<campaign::SessionSummary> {
    let (log, upto, previous) = {
        let w = lock(world)?;
        let from = campaign::summarized_upto(w.conn(), campaign_id)?;
        let events = campaign::events(w.conn(), campaign_id, from, 2000)?;
        let mut lines = vec![];
        let mut upto = from;
        for e in &events {
            upto = e.seq;
            let text = e.payload.get("text").and_then(Value::as_str).unwrap_or("");
            match e.kind.as_str() {
                "player" => lines.push(format!("[回合 {}] 玩家：{text}", e.turn)),
                "narration" => lines.push(format!("[回合 {}] GM：{text}", e.turn)),
                "npc" => lines.push(format!("[回合 {}] {}：{text}", e.turn, e.payload.get("name").and_then(Value::as_str).unwrap_or(""))),
                "fact" => lines.push(format!("[回合 {}] 记住：{text}", e.turn)),
                "transition" => lines.push(format!("[回合 {}] 剧情推进", e.turn)),
                _ => {}
            }
        }
        if lines.is_empty() {
            return Err(GmError::Input("上次摘要之后还没有新的游戏记录".into()));
        }
        let previous = campaign::summaries(w.conn(), campaign_id)?
            .into_iter()
            .rfind(|s| s.status == "confirmed")
            .map(|s| s.summary)
            .unwrap_or_default();
        (lines.join("\n"), upto, previous)
    };
    let user = templates.render("session_summary", json!({ "log": log, "previous": previous }))?;
    let mut logs = vec![];
    let s = ask_limited::<SummaryOut>(client, SYSTEM, &user, opts, LIMIT_SUMMARY, cancel, &mut logs)?.value;
    let w = lock(world)?;
    Ok(campaign::add_summary_draft(w.conn(), campaign_id, &s.summary, upto)?)
}

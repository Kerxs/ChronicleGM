//! AI 剧情导演：观察玩家的行动与世界状态，判断节奏，提出悬念、转折、人物主动行动等建议。
//!
//! - 程序先算好节奏（紧张度、停滞、重复）和可用的剧情钩子（chronicle_core::director），模型只负责出主意。
//! - 导演只提建议，不修改任何状态；建议存为战役记录，主持人采用后才进入 GM 的上下文。
//! - 不强迫玩家：建议是"可以提供的机会"，GM 的提示词要求玩家不接也没关系。
//! - 不擅自修改已确认设定：提示词禁止；涉及核心或已确认设定、并带有"其实、原来、并非"等改写意味的建议标为需要主持人确认，
//!   不会被自动采用；GM 执行时的效果仍然经过原有的校验（核心设定不可改）。
//! - 在后台以低优先级运行，不阻塞 GM 回复。

use std::sync::Mutex;

use chronicle_core::World;
use chronicle_core::director::{self, DirectorNote, Suggestion};
use chronicle_gen::Templates;
use chronicle_llm::{CancelToken, LlmClient};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{GmError, GmOptions, Result};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SuggestionOut {
    /// hint（给线索或提示）/ complication（制造麻烦）/ twist（剧情转折）/ npc_action（人物主动行动）/ event（发生事件）/ pacing（节奏调整）
    pub kind: String,
    /// 建议的内容：GM 可以怎么做（一两句话）。
    pub text: String,
    /// 为什么现在适合这样做。
    pub why: String,
    /// 依据的事实编号（M 记忆、W 设定、C 线索、Q 任务、N 人物、E 分支、L 地点）。
    #[serde(default)]
    pub uses: Vec<String>,
    /// 这条建议是否需要改变已确认的设定（需要时会交给主持人决定）。
    #[serde(default)]
    pub touches_canon: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DirectorOut {
    /// 对当前局面与节奏的判断（一两句话）。
    pub assessment: String,
    /// raise（提高紧张、推动事情发生）/ hold（保持）/ release（放缓、给玩家喘息）
    pub pacing: String,
    /// 1–3 条建议。
    pub suggestions: Vec<SuggestionOut>,
    /// 剧情符合度 0–100：目前的发展和原剧本、世界观、人物设定、已发生事件的一致程度（偏离剧本本身不是错误，只是记录）。
    #[serde(default)]
    pub conformity: i64,
    /// 偏离点（最多 4 条）：kind 是 script / world / character / event 之一，text 写具体哪里不一致。
    #[serde(default)]
    pub deviations: Vec<DeviationOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DeviationOut {
    pub kind: String,
    pub text: String,
}

const SYSTEM: &str = "你是 TRPG 的剧情导演，负责给主持人出主意。只输出一个 JSON 对象，不要输出解释或代码块标记。使用简体中文。";
const LIMIT: u32 = 1500;
const CHANGE_WORDS: [&str; 8] = ["其实", "原来", "并非", "不是", "实际上", "改为", "变成", "推翻"];

/// 去掉建议文字里的编号说明（实测："……埋线（依据 M3、M2）"），依据另有字段。
fn strip_codes(text: &str) -> String {
    let mut s = text.to_string();
    for (open, close) in [("（依据", "）"), ("(依据", ")")] {
        while let Some(i) = s.find(open) {
            match s[i..].find(close) {
                Some(j) => s.replace_range(i..i + j + close.len(), ""),
                None => break,
            }
        }
    }
    s.trim().to_string()
}

/// 模型给的建议类型五花八门（实测：npc_enters、new_clue、deadline……），归到固定的几类。
fn normalize_kind(k: &str) -> String {
    let k = k.trim().to_lowercase();
    let pick = if k.contains("npc") || k.contains("character") {
        "npc_action"
    } else if k.contains("twist") || k.contains("reveal") {
        "twist"
    } else if k.contains("deadline") || k.contains("complication") || k.contains("time") || k.contains("threat") || k.contains("danger") {
        "complication"
    } else if k.contains("event") {
        "event"
    } else if k.contains("pac") || k.contains("rest") {
        "pacing"
    } else {
        "hint"
    };
    pick.into()
}

fn lock(w: &Mutex<World>) -> Result<std::sync::MutexGuard<'_, World>> {
    w.lock().map_err(|_| GmError::Input("世界文件被占用".into()))
}

/// 已确认的设定（核心正典与已确认的世界节点）的名称。
fn confirmed_titles(world: &World) -> Vec<String> {
    let mut stmt = match world.conn().prepare("SELECT title FROM nodes WHERE deleted_at IS NULL AND (canon_level = 'core' OR status = 'confirmed') LIMIT 2000") {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    stmt.query_map([], |r| r.get::<_, String>(0)).map(|rows| rows.filter_map(|r| r.ok()).filter(|t| t.chars().count() >= 2).collect()).unwrap_or_default()
}

/// 分析一次。trigger：stall / repeat / periodic / manual。结果保存为导演建议（战役已结束时只返回不保存）。
pub fn analyze(world: &Mutex<World>, client: &dyn LlmClient, templates: &Templates, opts: &GmOptions, campaign_id: &str, trigger: &str, cancel: &CancelToken) -> Result<DirectorNote> {
    let (ctx, pacing, recent, confirmed, settings) = {
        let w = lock(world)?;
        let ctx = crate::context::build_gm_context(&w, campaign_id, "", opts.context_budget)?;
        let pacing = director::pacing(w.conn(), campaign_id)?;
        let recent: Vec<String> = chronicle_core::campaign::recent_events(w.conn(), campaign_id, &["player", "narration"], 12)?
            .into_iter()
            .rev()
            .map(|e| format!("[回合 {}] {}：{}", e.turn, if e.kind == "player" { "玩家" } else { "GM" }, e.payload["text"].as_str().unwrap_or("").chars().take(160).collect::<String>()))
            .collect();
        (ctx, pacing, recent, confirmed_titles(&w), director::settings(w.conn(), campaign_id))
    };
    let pace_text = format!(
        "剧情阶段：{}（{}）\n第 {} 回合；紧张度 {}/100（{}）；已经 {} 回合没有剧情进展{}{}\n最近的进展：{}\n紧张度曲线（回合:紧张度）：{}",
        pacing.phase.label,
        pacing.phase.reason,
        pacing.turn,
        pacing.tension,
        pacing.tension_reasons.join("；"),
        pacing.turns_since_progress,
        if pacing.stalled { "，判定为停滞" } else { "" },
        if pacing.repeating { "；玩家最近几次行动几乎一样" } else { "" },
        if pacing.recent_progress.is_empty() { "（暂无）".into() } else { pacing.recent_progress.join("；") },
        pacing.curve.iter().map(|b| format!("{}:{}", b.turn, b.tension)).collect::<Vec<_>>().join(" ")
    );
    let mut hooks = pacing.hooks.iter().take(30).map(|h| format!("- [{}] {}：{}", h.kind, h.title, h.note)).collect::<Vec<_>>().join("\n");
    // 剧情阶段与伏笔：很久没回收的伏笔、快到期的后果也是可以用的钩子。
    let story = {
        let w = lock(world)?;
        let open: Vec<String> = chronicle_core::story::foreshadows(w.conn(), campaign_id)?.into_iter().filter(|f| f.status == "open" && pacing.turn - f.planted_turn >= 6).map(|f| format!("- [伏笔，第 {} 回合埋下] {}", f.planted_turn, f.text)).collect();
        let pending: Vec<String> = chronicle_core::story::consequences(w.conn(), campaign_id)?.into_iter().filter(|c| c.status == "pending").map(|c| format!("- [延迟后果，第 {} 回合到期] 因为「{}」→ {}", c.due_turn, c.decision, c.consequence)).collect();
        [open, pending].concat()
    };
    if !story.is_empty() {
        if !hooks.is_empty() {
            hooks.push('\n');
        }
        hooks.push_str(&story.join("\n"));
    }
    let user = templates.render(
        "director",
        json!({ "context": ctx.text, "pacing": pace_text, "preference": director::pacing_preference(&settings), "hooks": if hooks.is_empty() { "（没有）".to_string() } else { hooks }, "recent": recent.join("\n"), "trigger": trigger,
                 "confirmed": confirmed.iter().take(60).cloned().collect::<Vec<_>>().join("、") }),
    )?;
    let mut logs = vec![];
    let out = crate::ask::ask_limited::<DirectorOut>(client, SYSTEM, &user, opts, LIMIT, cancel, &mut logs)?.value;
    let mut w = lock(world)?;
    let id = director::next_note_id(w.conn(), campaign_id)?;
    let resolve = |c: &str| {
        let c = c.trim().to_uppercase();
        ctx.memory_refs.get(&c).cloned().or_else(|| ctx.refs.get(&c).and_then(|i| ctx.names.get(i)).map(|n| format!("{c} {n}")))
    };
    let mut suggestions: Vec<Suggestion> = out
        .suggestions
        .iter()
        .filter(|s| !s.text.trim().is_empty())
        .take(3)
        .enumerate()
        .map(|(i, s)| {
            let text = strip_codes(&ctx.humanize(s.text.trim()));
            let rewrites_canon = confirmed.iter().any(|t| text.contains(t.as_str())) && CHANGE_WORDS.iter().any(|w| text.contains(w));
            Suggestion {
                id: format!("{id}-{}", i + 1),
                kind: normalize_kind(&s.kind),
                why: strip_codes(&ctx.humanize(s.why.trim())),
                uses: s.uses.iter().take(6).filter_map(|c| resolve(c)).collect(),
                needs_review: s.touches_canon || rewrites_canon,
                status: "new".into(),
                adopted_turn: None,
                text,
            }
        })
        .collect();
    let turn = chronicle_core::campaign::get_campaign(w.conn(), campaign_id)?.turn;
    // 停滞时自动采用第一条不需要确认的建议（可以在设置里关闭）。
    if matches!(trigger, "stall" | "repeat")
        && settings.auto_adopt_on_stall
        && !settings.hint_only
        && let Some(s) = suggestions.iter_mut().find(|s| !s.needs_review)
    {
        s.status = "adopted".into();
        s.adopted_turn = Some(turn);
    }
    let pacing_v = match out.pacing.trim() {
        p @ ("raise" | "hold" | "release") => p.to_string(),
        _ => "hold".into(),
    };
    // 判断写成了一个词（实测：模型把触发原因"stall"写进了判断）时，用程序算的节奏说明代替。
    let assessment = ctx.humanize(out.assessment.trim());
    let assessment = if assessment.chars().count() < 6 || assessment.is_ascii() {
        format!(
            "紧张度 {}/100，已经 {} 回合没有剧情进展{}",
            pacing.tension,
            pacing.turns_since_progress,
            if pacing.repeating { "，玩家在重复同样的行动" } else { "" }
        )
    } else {
        assessment
    };
    let deviations: Vec<chronicle_core::director::Deviation> = out
        .deviations
        .iter()
        .filter(|d| !d.text.trim().is_empty())
        .take(4)
        .map(|d| chronicle_core::director::Deviation {
            kind: if matches!(d.kind.as_str(), "script" | "world" | "character" | "event") { d.kind.clone() } else { "script".into() },
            text: strip_codes(&ctx.humanize(d.text.trim())),
        })
        .collect();
    // 模型没给分数（0）又没有偏离点时不显示分数，避免把"没评估"当成"完全偏离"。
    let conformity = if out.conformity <= 0 && deviations.is_empty() { None } else { Some(out.conformity.clamp(0, 100)) };
    let note = DirectorNote { id, turn, trigger: trigger.into(), pacing: pacing_v, assessment, tension: pacing.tension, suggestions, conformity, deviations };
    if chronicle_core::campaign::get_campaign(w.conn(), campaign_id)?.status != "ended" {
        director::save_note(&mut w, campaign_id, &note)?;
    }
    Ok(note)
}

//! 剧情导演的程序部分（不调用模型）：
//! - 节奏：每回合记录紧张度（战斗、生命、期限、情绪、天气……），连续多回合没有进展就判定为停滞，
//!   玩家反复做同样的事也会提示。
//! - 可用的剧情钩子：没揭示的线索、没开始的任务、没触发的触发器、可走的分支、还没打过交道的人物、听说过没去过的地方。
//! - 剧情冲突提示：死亡人物仍被任务需要、剧情走进死胡同、叙述与程序结算不符、叙述泄露主持人信息、玩家倒下等。
//! - 导演建议（由模型生成，见 chronicle-gm）存为战役记录 `director_note`；主持人采用后才进入 GM 的上下文。
//!   导演只提建议，不直接修改任何状态，也不能改动已确认的设定。

use std::collections::{BTreeMap, HashSet};

use chronicle_rules::{Condition, Effect, StateView, Tri};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::campaign::{self, CampaignView};
use crate::error::{CoreError, Result};
use crate::model::ScriptItemKind;
use crate::world::World;

pub const SETTINGS_VAR: &str = "director.settings";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(default)]
pub struct DirectorSettings {
    /// 停滞或到了间隔时，回合结束后在后台自动分析。
    pub auto_analyze: bool,
    /// 每隔几回合例行分析一次（0 = 只在停滞时分析）。
    pub every_turns: i64,
    /// 连续几回合没有进展算停滞。
    pub stall_turns: i64,
    /// 停滞时自动采用第一条建议（否则等主持人采用）。
    pub auto_adopt_on_stall: bool,
    /// 采用的建议在 GM 上下文里保留几回合。
    pub guidance_turns: i64,
    /// 剧情节奏：0 松弛（给玩家很多自由探索的时间）… 100 紧凑（事情一件接一件）。
    pub pacing_level: i64,
    /// 玩家偏离剧本时只提示主持人，不设法把玩家拉回主线。
    pub hint_only: bool,
}

impl Default for DirectorSettings {
    fn default() -> Self {
        Self { auto_analyze: true, every_turns: 6, stall_turns: 4, auto_adopt_on_stall: true, guidance_turns: 4, pacing_level: 50, hint_only: false }
    }
}

pub fn settings(conn: &Connection, campaign_id: &str) -> DirectorSettings {
    CampaignView { conn, campaign_id }.var(SETTINGS_VAR).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

pub fn set_settings(world: &mut World, campaign_id: &str, s: &DirectorSettings) -> Result<()> {
    if !(1..=50).contains(&s.stall_turns) || !(0..=100).contains(&s.every_turns) || !(1..=50).contains(&s.guidance_turns) || !(0..=100).contains(&s.pacing_level) {
        return Err(CoreError::Invalid("导演设置超出范围（停滞 1–50 回合，间隔 0–100，建议保留 1–50 回合）".into()));
    }
    campaign::apply_effects(world, campaign_id, &[Effect::SetVar { key: SETTINGS_VAR.into(), value: serde_json::to_value(s).unwrap_or(Value::Null) }], "修改导演设置")?;
    Ok(())
}

// ===================== 节奏 =====================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Hook {
    /// clue / quest / trigger / plot / npc / place
    pub kind: String,
    pub id: String,
    pub title: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Beat {
    pub turn: i64,
    pub tension: i64,
    pub progress: Vec<String>,
    /// 处理到的最后一个事件序号。
    #[serde(default)]
    pub seq: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Pacing {
    pub turn: i64,
    /// 0–100。
    pub tension: i64,
    pub tension_reasons: Vec<String>,
    pub turns_since_progress: i64,
    pub stalled: bool,
    /// 玩家最近几次行动几乎一样。
    pub repeating: bool,
    pub recent_progress: Vec<String>,
    pub curve: Vec<Beat>,
    pub hooks: Vec<Hook>,
    /// 剧情阶段：铺垫 / 发展 / 高潮 / 收束。
    #[serde(default = "default_phase")]
    pub phase: crate::story::PlotPhase,
}

fn default_phase() -> crate::story::PlotPhase {
    crate::story::PlotPhase { phase: "setup".into(), label: "铺垫".into(), progress: 0, reason: String::new() }
}

/// 一组状态变化里算"剧情进展"的部分（说明文字）。
pub fn progress_in(payload: &Value, names: &dyn Fn(&str) -> String) -> Vec<String> {
    let mut out = vec![];
    for ch in payload.get("changes").and_then(Value::as_array).into_iter().flatten() {
        let t = ch.get("type").and_then(Value::as_str).unwrap_or_default();
        let s = |k: &str| ch.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        match t {
            "reveal_clue" => out.push(format!("揭示线索「{}」", names(&s("clue")))),
            "set_quest_state" => out.push(format!("任务「{}」→ {}", names(&s("quest")), s("new"))),
            "unlock_node" => out.push(format!("解锁「{}」", names(&s("node")))),
            "set_item_qty" if s("holder") == "party" && ch["new"].as_i64() > ch["old"].as_i64() => out.push(format!("得到「{}」", names(&s("item")))),
            "set_var" if s("key") == "party.location" => out.push("到达新地点".into()),
            "upsert_record" => match s("kind").as_str() {
                "clue" if ch.get("old").is_none_or(Value::is_null) => out.push("新的发现".into()),
                "quest" if ch.get("old").is_none_or(Value::is_null) => out.push("新的任务".into()),
                "quest_note" if ch.get("old").is_none_or(Value::is_null) => out.push("任务有了新情况".into()),
                "combat" if ch["new"]["status"].as_str() != Some("active") && ch["old"]["status"].as_str() == Some("active") => out.push(format!("战斗结束（{}）", ch["new"]["status"].as_str().unwrap_or(""))),
                "combat" if ch.get("old").is_none_or(Value::is_null) => out.push("爆发战斗".into()),
                _ => {}
            },
            _ => {}
        }
    }
    out
}

pub fn names_map(conn: &Connection, campaign_id: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    if let Ok(c) = campaign::get_campaign(conn, campaign_id)
        && let Some(s) = c.script_id
        && let Ok(items) = crate::scripts::all_items(conn, &s)
    {
        for it in items {
            m.insert(it.id, it.title);
        }
    }
    m
}

fn bigrams(s: &str) -> HashSet<(char, char)> {
    let c: Vec<char> = s.chars().filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation() && !"，。！？、；：".contains(*c)).collect();
    c.windows(2).map(|w| (w[0], w[1])).collect()
}

/// 两句话的相似度（字符二元组的 Jaccard 系数）。
pub fn similarity(a: &str, b: &str) -> f64 {
    let (x, y) = (bigrams(a), bigrams(b));
    if x.is_empty() || y.is_empty() {
        return 0.0;
    }
    x.intersection(&y).count() as f64 / x.union(&y).count() as f64
}

/// 当前紧张度与原因（程序计算）。
pub fn tension_now(conn: &Connection, campaign_id: &str) -> (i64, Vec<String>) {
    let mut t = 15;
    let mut why = vec![];
    if let Some(c) = crate::combat::active(conn, campaign_id) {
        t += 40;
        why.push(format!("战斗中（第 {} 轮）", c.round));
    }
    if let Ok(s) = crate::rpg::sheet(conn, campaign_id, crate::rpg::PARTY) {
        if s.hp <= 0 {
            t += 25;
            why.push("玩家已倒下".into());
        } else if s.hp * 2 < s.max_hp {
            t += 15;
            why.push(format!("玩家生命偏低（{}/{}）", s.hp, s.max_hp));
        }
    }
    if let Ok(qs) = crate::quests::quests(conn, campaign_id, true) {
        let urgent: Vec<String> = qs.iter().filter(|q| q.status == "active" && q.turns_left.is_some_and(|n| n <= 2)).map(|q| q.title.clone()).collect();
        if !urgent.is_empty() {
            t += 15;
            why.push(format!("任务期限将到：{}", urgent.join("、")));
        }
    }
    if let Ok(npcs) = crate::schedule::all_npc_status(conn, campaign_id) {
        let view = CampaignView { conn, campaign_id };
        let hot: Vec<String> = npcs.iter().filter(|n| view.mood(&n.npc_id).is_some_and(|m| m.fear >= 70 || m.anger >= 70 || m.suspicion >= 70)).map(|n| n.name.clone()).collect();
        if !hot.is_empty() {
            t += 10;
            why.push(format!("情绪激烈的人物：{}", hot.join("、")));
        }
    }
    if crate::worldsim::environment(conn, campaign_id).is_ok_and(|e| e.bad_weather) {
        t += 5;
        why.push("恶劣天气".into());
    }
    if why.is_empty() {
        why.push("平静：没有冲突、期限或危险".into());
    }
    (t.min(100), why)
}

/// 回合结束时记录这一回合的节拍（紧张度与进展）。由 GM 回合调用。
pub fn record_beat(world: &mut World, campaign_id: &str) -> Result<Beat> {
    let conn = world.conn();
    let turn = campaign::get_campaign(conn, campaign_id)?.turn;
    let (tension, _) = tension_now(conn, campaign_id);
    let names = names_map(conn, campaign_id);
    let name = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_string());
    // 上一个节拍之后的事件。
    let last_seq = beats(conn, campaign_id)?.last().map(|b| b.seq).unwrap_or(0);
    let evs = campaign::events(conn, campaign_id, last_seq, 5000)?;
    let mut progress = vec![];
    let mut max_seq = last_seq;
    for e in &evs {
        max_seq = max_seq.max(e.seq);
        match e.kind.as_str() {
            "effects" => progress.extend(progress_in(&e.payload, &name)),
            "transition" => progress.push("剧情推进".into()),
            // 新的情报（GM 记下的事实）也算进展（实测：玩家和 NPC 谈出了关键信息，却被判为停滞）。
            "fact" => progress.push(format!("新情报：{}", e.payload["text"].as_str().unwrap_or("").chars().take(24).collect::<String>())),
            _ => {}
        }
    }
    progress.dedup();
    // 同一回合多次记录时合并进展。
    let prev = (CampaignView { conn, campaign_id }).record("director_beat", &format!("t{turn:06}")).and_then(|v| serde_json::from_value::<Beat>(v).ok());
    if let Some(p) = prev {
        let mut all = p.progress;
        all.extend(progress);
        progress = all;
        progress.dedup();
    }
    // 记录本身也会产生事件，但它不算进展，下次再扫描也没有影响。
    let beat = Beat { turn, tension, progress, seq: max_seq };
    campaign::apply_effects(world, campaign_id, &[Effect::UpsertRecord { kind: "director_beat".into(), id: format!("t{turn:06}"), data: serde_json::to_value(&beat).unwrap_or(Value::Null) }], "导演：记录节奏")?;
    Ok(beat)
}

fn beats(conn: &Connection, campaign_id: &str) -> Result<Vec<Beat>> {
    let mut stmt = conn.prepare("SELECT data FROM campaign_records WHERE campaign_id = ?1 AND kind = 'director_beat' ORDER BY id")?;
    let rows = stmt.query_map([campaign_id], |r| r.get::<_, String>(0))?;
    Ok(rows.filter_map(|r| r.ok().and_then(|d| serde_json::from_str(&d).ok())).collect())
}

/// 可用的剧情钩子（主持人信息）。
pub fn hooks(conn: &Connection, campaign_id: &str) -> Result<Vec<Hook>> {
    let view = CampaignView { conn, campaign_id };
    let c = campaign::get_campaign(conn, campaign_id)?;
    let mut out = vec![];
    // 未揭示的线索按剧本的调查比重取舍：非探案剧本只留很少几条、排在最后（以前排在最前，导演总建议"去找线索"）。
    let investigation = match &c.script_id {
        Some(sid) => crate::scripts::get_script(conn, sid)?.map(|s| crate::profile::for_script(&s).weights.investigation).unwrap_or(1),
        None => 1,
    };
    let clue_cap = match investigation {
        0 => 0,
        1 => 2,
        2 => 5,
        _ => usize::MAX,
    };
    let mut clue_hooks = vec![];
    if let Some(sid) = &c.script_id {
        for it in crate::scripts::all_items(conn, sid)? {
            match it.kind {
                ScriptItemKind::Clue if !view.clue_known(&it.id) => clue_hooks.push(Hook { kind: "clue".into(), id: it.id, title: it.title, note: it.player_text.chars().take(40).collect() }),
                ScriptItemKind::Trigger | ScriptItemKind::KeyEvent if it.payload.get("when").is_some() && view.record("trigger_fired", &it.id).is_none() => {
                    out.push(Hook { kind: "trigger".into(), id: it.id, title: it.title, note: "还没有触发".into() })
                }
                _ => {}
            }
        }
    }
    for q in crate::quests::quests(conn, campaign_id, true)? {
        if q.status.is_empty() {
            out.push(Hook { kind: "quest".into(), id: q.id, title: q.title, note: if q.quest_type == "hidden" { "隐藏任务，未开始".into() } else { "未开始".into() } });
        }
    }
    for o in campaign::available_edges_conn(conn, campaign_id)? {
        if !o.used_up {
            let to = crate::plot::get_plot_node(conn, &o.edge.to_id)?.map(|n| n.title).unwrap_or_default();
            out.push(Hook { kind: "plot".into(), id: o.edge.id, title: format!("{} → {to}", o.edge.label), note: match o.availability { Tri::True => "条件满足".into(), Tri::False => "条件不满足".into(), Tri::Unknown => "需要检定".into() } });
        }
    }
    let spoken: HashSet<String> = campaign::recent_events(conn, campaign_id, &["npc"], 500)?.into_iter().filter_map(|e| e.payload["npc_id"].as_str().map(str::to_string)).collect();
    for n in crate::schedule::all_npc_status(conn, campaign_id)? {
        if !spoken.contains(&n.npc_id) && n.life_status != "dead" {
            out.push(Hook { kind: "npc".into(), id: n.npc_id, title: n.name, note: if n.activity.is_empty() { "还没打过交道".into() } else { format!("还没打过交道；此刻{}", n.activity) } });
        }
    }
    for p in crate::worldsim::map(conn, campaign_id, false)?.places {
        if p.state == "heard" {
            out.push(Hook { kind: "place".into(), id: p.id, title: p.title, note: "听说过，还没去".into() });
        }
    }
    clue_hooks.truncate(clue_cap);
    if investigation >= 2 {
        // 探案剧本：线索仍是首要推进点。
        clue_hooks.extend(out);
        return Ok(clue_hooks);
    }
    out.extend(clue_hooks);
    Ok(out)
}

pub fn pacing(conn: &Connection, campaign_id: &str) -> Result<Pacing> {
    let st = settings(conn, campaign_id);
    let turn = campaign::get_campaign(conn, campaign_id)?.turn;
    let curve = beats(conn, campaign_id)?;
    let last_progress = curve.iter().rev().find(|b| !b.progress.is_empty()).map(|b| b.turn).unwrap_or(0);
    let first_turn = curve.first().map(|b| b.turn).unwrap_or(turn);
    let turns_since_progress = (turn - last_progress.max(first_turn.saturating_sub(1))).max(0);
    let inputs: Vec<String> = campaign::recent_events(conn, campaign_id, &["player"], 3)?.into_iter().filter_map(|e| e.payload["text"].as_str().map(str::to_string)).collect();
    let repeating = inputs.len() == 3 && similarity(&inputs[0], &inputs[1]) >= 0.6 && similarity(&inputs[1], &inputs[2]) >= 0.6;
    let (tension, tension_reasons) = tension_now(conn, campaign_id);
    let recent_progress = curve.iter().rev().filter(|b| !b.progress.is_empty()).take(3).flat_map(|b| b.progress.iter().map(move |p| format!("第 {} 回合：{p}", b.turn))).collect();
    let in_combat = crate::combat::active(conn, campaign_id).is_some();
    Ok(Pacing {
        turn,
        tension,
        tension_reasons,
        turns_since_progress,
        stalled: !in_combat && curve.len() as i64 >= st.stall_turns && turns_since_progress >= st.stall_turns,
        repeating,
        recent_progress,
        curve: curve.into_iter().rev().take(30).rev().collect(),
        hooks: hooks(conn, campaign_id)?,
        phase: crate::story::plot_phase(conn, campaign_id, tension)?,
    })
}

/// 回合结束后是否需要自动分析：停滞（且这次停滞还没分析过）或到了例行间隔。
pub fn should_analyze(conn: &Connection, campaign_id: &str) -> Result<Option<String>> {
    let st = settings(conn, campaign_id);
    if !st.auto_analyze || campaign::get_campaign(conn, campaign_id)?.status == "ended" {
        return Ok(None);
    }
    let p = pacing(conn, campaign_id)?;
    let last = notes(conn, campaign_id, 1)?.into_iter().next();
    let last_turn = last.as_ref().map(|n| n.turn).unwrap_or(0);
    if (p.stalled || p.repeating) && p.turn - last_turn >= st.stall_turns.min(p.turns_since_progress).max(1) {
        return Ok(Some(if p.stalled { "stall".into() } else { "repeat".into() }));
    }
    if st.every_turns > 0 && p.turn > 0 && p.turn - last_turn >= st.every_turns {
        return Ok(Some("periodic".into()));
    }
    Ok(None)
}

// ===================== 导演建议 =====================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Suggestion {
    pub id: String,
    /// hint（提示）/ complication（麻烦）/ twist（转折）/ npc_action（人物主动行动）/ event（事件）/ pacing（节奏）
    pub kind: String,
    pub text: String,
    pub why: String,
    /// 依据的事实（已换成文字）。
    pub uses: Vec<String>,
    /// 可能触及已确认设定，需要主持人确认。
    pub needs_review: bool,
    /// new / adopted / dismissed / done
    pub status: String,
    #[serde(default)]
    pub adopted_turn: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct DirectorNote {
    pub id: String,
    pub turn: i64,
    /// stall / repeat / periodic / manual
    pub trigger: String,
    /// raise（加快、提高紧张）/ hold（保持）/ release（放缓、给玩家喘息）
    pub pacing: String,
    pub assessment: String,
    pub tension: i64,
    pub suggestions: Vec<Suggestion>,
    /// 剧情符合度 0–100（和原剧本、世界观、人物设定、已发生事件的一致程度；没有评估时为空）。
    #[serde(default)]
    pub conformity: Option<i64>,
    /// 偏离点。
    #[serde(default)]
    pub deviations: Vec<Deviation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Deviation {
    /// script（原剧本）/ world（世界观）/ character（人物设定）/ event（已发生的事件）
    pub kind: String,
    pub text: String,
}

/// 节奏偏好的文字说明（给导演和 GM）。
pub fn pacing_preference(s: &DirectorSettings) -> String {
    let pace = match s.pacing_level {
        0..=20 => "很松弛：让玩家慢慢探索、闲聊、经营，不急着推动事件",
        21..=40 => "偏松弛：留出探索和角色互动的时间，事件间隔长一些",
        41..=60 => "适中：探索和事件交替",
        61..=80 => "偏紧凑：事件来得更快，常有新的压力和时限",
        _ => "很紧凑：事情一件接一件，几乎每回合都有新的推进或威胁",
    };
    let pull = if s.hint_only { "；玩家偏离剧本时只提示主持人，不要设法把玩家拉回主线，顺着玩家的选择发展" } else { "；玩家偏离主线太远时，可以自然地给出回到主线的机会（不强迫）" };
    format!("剧情节奏偏好：{pace}{pull}")
}

pub fn notes(conn: &Connection, campaign_id: &str, limit: usize) -> Result<Vec<DirectorNote>> {
    let mut stmt = conn.prepare("SELECT data FROM campaign_records WHERE campaign_id = ?1 AND kind = 'director_note' ORDER BY id DESC LIMIT ?2")?;
    let rows = stmt.query_map(rusqlite::params![campaign_id, limit as i64], |r| r.get::<_, String>(0))?;
    Ok(rows.filter_map(|r| r.ok().and_then(|d| serde_json::from_str(&d).ok())).collect())
}

pub fn save_note(world: &mut World, campaign_id: &str, note: &DirectorNote) -> Result<()> {
    campaign::apply_effects(world, campaign_id, &[Effect::UpsertRecord { kind: "director_note".into(), id: note.id.clone(), data: serde_json::to_value(note).unwrap_or(Value::Null) }], "导演建议")?;
    Ok(())
}

pub fn next_note_id(conn: &Connection, campaign_id: &str) -> Result<String> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM campaign_records WHERE campaign_id = ?1 AND kind = 'director_note'", [campaign_id], |r| r.get(0))?;
    Ok(format!("n{:05}", n + 1))
}

/// 主持人采用 / 忽略 / 标记完成某条建议。
pub fn set_suggestion_status(world: &mut World, campaign_id: &str, note_id: &str, suggestion_id: &str, status: &str) -> Result<()> {
    if !matches!(status, "adopted" | "dismissed" | "done" | "new") {
        return Err(CoreError::Invalid(format!("未知的状态：{status}")));
    }
    let mut note: DirectorNote = (CampaignView { conn: world.conn(), campaign_id })
        .record("director_note", note_id)
        .and_then(|v| serde_json::from_value(v).ok())
        .ok_or_else(|| CoreError::NotFound("导演建议", note_id.into()))?;
    let turn = campaign::get_campaign(world.conn(), campaign_id)?.turn;
    let s = note.suggestions.iter_mut().find(|s| s.id == suggestion_id).ok_or_else(|| CoreError::NotFound("建议", suggestion_id.into()))?;
    s.status = status.into();
    s.adopted_turn = (status == "adopted").then_some(turn);
    save_note(world, campaign_id, &note)
}

/// 进入 GM 上下文的导演指引：最近一次的节奏判断 + 仍在有效期内的已采用建议。
pub fn guidance(conn: &Connection, campaign_id: &str) -> Result<(Option<String>, Vec<Suggestion>)> {
    let st = settings(conn, campaign_id);
    let turn = campaign::get_campaign(conn, campaign_id)?.turn;
    let ns = notes(conn, campaign_id, 10)?;
    let pace = ns.first().map(|n| {
        format!(
            "{}：{}",
            match n.pacing.as_str() {
                "raise" => "提高紧张度、推动事情发生",
                "release" => "放缓节奏、给玩家喘息和探索的空间",
                _ => "保持当前节奏",
            },
            n.assessment
        )
    });
    let adopted = ns.iter().flat_map(|n| n.suggestions.iter()).filter(|s| s.status == "adopted" && s.adopted_turn.is_some_and(|t| turn - t < st.guidance_turns)).cloned().collect();
    Ok((pace, adopted))
}

// ===================== 剧情冲突提示 =====================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Conflict {
    /// warn / info
    pub severity: String,
    pub kind: String,
    pub text: String,
    pub turn: Option<i64>,
}

/// 叙述与程序结算不符（启发式）：结算里有人倒下而叙述没提到他；攻击没命中而叙述写命中。
pub fn narration_mismatches(narration: &str, settlement: &[String]) -> Vec<String> {
    let mut out = vec![];
    let hit_words = ["砍中", "命中", "击中", "切入", "劈中", "刺中", "斩中", "正中"];
    for l in settlement {
        if let Some(who) = l.strip_suffix("倒下了").or_else(|| l.strip_suffix("死了")) {
            let base = who.trim_end_matches(|c: char| c.is_ascii_digit());
            if !base.is_empty() && !narration.contains(base) && !narration.contains("倒") {
                out.push(format!("结算里「{who}」倒下了，叙述没有提到"));
            }
        }
    }
    let attacks: Vec<&String> = settlement.iter().filter(|l| l.contains("攻击") && l.contains("掷出")).collect();
    if !attacks.is_empty() && attacks.iter().all(|l| l.contains("没有命中") || l.contains("大失手")) && hit_words.iter().any(|w| narration.contains(w)) {
        out.push("结算里所有攻击都没有命中，叙述却写了命中".into());
    }
    if attacks.iter().any(|l| l.contains("倒下") || settlement.iter().any(|s| s.ends_with("倒下了"))) && narration.contains("没能") && narration.contains("触到") {
        out.push("结算里有人被击倒，叙述却写没有击中".into());
    }
    // 结算全部命中，叙述却写落空。
    let miss_words = ["落空", "扑了个空", "没能砍中", "没有砍中", "没能击中", "擦着", "没能触到"];
    if !attacks.is_empty() && attacks.iter().all(|l| l.contains("命中") && !l.contains("没有命中")) && miss_words.iter().any(|w| narration.contains(w)) {
        out.push("结算里的攻击都命中了，叙述却写落空".into());
    }
    // 买卖、制作没有成功，叙述却写成了。
    if settlement.iter().any(|l| l.starts_with("交易没有") || l.starts_with("交易没能")) && ["买下", "成交", "收下了钱", "递给你"].iter().any(|w| narration.contains(w)) && !settlement.iter().any(|l| l.contains("买了")) {
        out.push("结算里交易没有成功，叙述却写成交了".into());
    }
    if settlement.iter().any(|l| l.starts_with("没能制作") || l.contains("制作") && l.contains("失败")) && ["做成了", "制作出", "完成了药膏", "成型"].iter().any(|w| narration.contains(w)) {
        out.push("结算里制作没有成功，叙述却写做成了".into());
    }
    out
}

/// 中文叙述里夹杂的英文单词（实测："daylight""players""sideways"）。编号与常见缩写不算。
pub fn english_words(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if cur.len() >= 4 && cur.chars().any(|c| c.is_ascii_lowercase()) && !matches!(cur.to_uppercase().as_str(), "HTTP" | "JSON") && !out.contains(cur) {
            out.push(cur.clone());
        }
        cur.clear();
    };
    for c in text.chars() {
        if c.is_ascii_alphabetic() {
            cur.push(c);
        } else {
            flush(&mut cur, &mut out);
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// 叙述里提到了还没揭示的线索（标题，或线索描述的大部分字词），却没有用 reveal_clue 揭示。
pub fn unrevealed_clue_mentions(conn: &Connection, campaign_id: &str, narration: &str) -> Result<Vec<(String, String)>> {
    let view = CampaignView { conn, campaign_id };
    let c = campaign::get_campaign(conn, campaign_id)?;
    let Some(sid) = c.script_id else { return Ok(vec![]) };
    let nb = bigrams(narration);
    let mut out = vec![];
    for it in crate::scripts::items_by_kind(conn, &sid, ScriptItemKind::Clue)? {
        if view.clue_known(&it.id) {
            continue;
        }
        let by_title = it.title.chars().count() >= 3 && narration.contains(it.title.as_str());
        let pb = bigrams(&it.player_text);
        let by_text = pb.len() >= 6 && pb.intersection(&nb).count() as f64 / pb.len() as f64 >= 0.6;
        if by_title || by_text {
            out.push((it.id, it.title));
        }
    }
    Ok(out)
}

/// 当前状态下的剧情冲突，加上最近回合记录的叙述问题。
pub fn conflicts(conn: &Connection, campaign_id: &str) -> Result<Vec<Conflict>> {
    let mut out = vec![];
    let c = campaign::get_campaign(conn, campaign_id)?;
    let dead: HashSet<String> = crate::schedule::all_npc_status(conn, campaign_id)?.into_iter().filter(|n| n.life_status == "dead").map(|n| n.npc_id).collect();
    for q in crate::quests::quests(conn, campaign_id, true)? {
        if q.status == "active" || q.status.is_empty() {
            for n in q.npcs.iter().filter(|n| dead.contains(&n.id)) {
                let name = n.title.clone();
                out.push(Conflict { severity: "warn".into(), kind: "dead_npc".into(), text: format!("任务「{}」需要的人物「{name}」已经死亡", q.title), turn: None });
            }
        }
    }
    if let Some(node) = campaign::current_node_conn(conn, campaign_id)? {
        let edges = campaign::available_edges_conn(conn, campaign_id)?;
        if node.kind != crate::model::PlotNodeKind::Ending && node.kind != crate::model::PlotNodeKind::Dynamic && edges.iter().all(|e| e.used_up || e.availability == Tri::False) {
            out.push(Conflict { severity: "warn".into(), kind: "dead_end".into(), text: format!("剧情节点「{}」没有可走的分支（全部走过或条件不满足）", node.title), turn: None });
        }
    }
    if crate::rpg::sheet(conn, campaign_id, crate::rpg::PARTY).is_ok_and(|s| s.hp <= 0) && crate::combat::active(conn, campaign_id).is_none() {
        out.push(Conflict { severity: "warn".into(), kind: "party_down".into(), text: "玩家已经倒下：需要休息、救治或剧情处理".into(), turn: None });
    }
    // 最近的叙述问题。
    for e in campaign::recent_events(conn, campaign_id, &["narration", "conflict", "effects_rejected"], 60)? {
        match e.kind.as_str() {
            "narration" => {
                let leaks = e.payload["leaks"].as_array().map(|a| a.len()).unwrap_or(0);
                if leaks > 0 {
                    out.push(Conflict { severity: "warn".into(), kind: "leak".into(), text: format!("叙述里可能泄露了 {leaks} 处主持人信息"), turn: Some(e.turn) });
                }
            }
            "conflict" => {
                for t in e.payload["items"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    out.push(Conflict { severity: "warn".into(), kind: "narration".into(), text: t.into(), turn: Some(e.turn) });
                }
            }
            _ => {
                let n = e.payload["rejected"].as_array().map(|a| a.len()).unwrap_or(0);
                if n > 0 {
                    out.push(Conflict { severity: "info".into(), kind: "rejected".into(), text: format!("GM 提议的 {n} 条修改被拒绝"), turn: Some(e.turn) });
                }
            }
        }
    }
    let _ = c;
    Ok(out)
}

/// 条件此刻的判定结果（给控制台显示）。
pub fn eval(conn: &Connection, campaign_id: &str, cond: &Value) -> String {
    match serde_json::from_value::<Condition>(cond.clone()) {
        Ok(c) => match c.preview(&CampaignView { conn, campaign_id }) {
            Ok(Tri::True) => "满足".into(),
            Ok(Tri::False) => "不满足".into(),
            Ok(Tri::Unknown) => "需要检定".into(),
            Err(e) => format!("无法判定：{e}"),
        },
        Err(e) => format!("格式错误：{e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_and_mismatches() {
        assert!(similarity("我继续搜查酒馆的角落", "我继续搜查酒馆角落") > 0.6);
        assert!(similarity("我去铁匠铺", "我问村长钥匙的事") < 0.2);
        let s = vec!["艾琳用长剑攻击野狼1：掷出 3+3=6，防御 10，没有命中".to_string()];
        assert_eq!(narration_mismatches("你一剑砍中了野狼", &s).len(), 1);
        assert!(narration_mismatches("你的剑擦着狼毛划过", &s).is_empty());
        let s2 = vec!["艾琳用长剑攻击野狼2：掷出 12+3=15，防御 10，命中，造成 8 点伤害".to_string(), "野狼2倒下了".to_string()];
        assert!(!narration_mismatches("你的剑没能触到它", &s2).is_empty());
        assert!(narration_mismatches("野狼哀嚎着倒在血泊里", &s2).is_empty());
        assert_eq!(english_words("得趁 daylight 赶路，A1 和 HTTP 不算"), vec!["daylight".to_string()]);
        let s3 = vec!["艾琳用长剑攻击野狼1：掷出 15+3=18，防御 10，命中，造成 6 点伤害".to_string()];
        assert!(!narration_mismatches("你的剑落空了", &s3).is_empty());
        let s4 = vec!["交易没有成功：钱不够".to_string()];
        assert!(!narration_mismatches("铁匠收下了钱，把剑递给你", &s4).is_empty());
    }
}

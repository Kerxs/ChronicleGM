//! 战役档案馆：统计、编年史、章节、跨战役搜索、战役 JSON 导入导出、战役完整性检查。
//!
//! 全部从只追加的事件日志与战役表计算（不另存统计结果），所以统计永远与存档一致。
//! 章节标记也写成事件（`chapter` / `chapter_rename` / `chapter_remove`），不改动任何状态。

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use rusqlite::{Connection, params, types::ValueRef};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::campaign::{self, Campaign, EventEntry};
use crate::error::{CoreError, Result};
use crate::timeline;
use crate::util::{new_id, now_ms};
use crate::world::World;

// ===================== 名称解析 =====================

/// 把编号翻成名称（剧本条目、剧情节点、世界节点），带缓存。
pub struct Names<'a> {
    conn: &'a Connection,
    cache: RefCell<HashMap<String, String>>,
}

impl<'a> Names<'a> {
    pub fn new(conn: &'a Connection, campaign_id: &str) -> Self {
        let mut m: HashMap<String, String> = crate::director::names_map(conn, campaign_id).into_iter().collect();
        if let Ok(c) = campaign::get_campaign(conn, campaign_id)
            && let Some(s) = c.script_id
        {
            for n in crate::plot::plot_nodes_of(conn, &s).unwrap_or_default() {
                m.insert(n.id, n.title);
            }
        }
        m.insert("party".into(), "队伍".into());
        Names { conn, cache: RefCell::new(m) }
    }
    pub fn get(&self, id: &str) -> String {
        if id.is_empty() {
            return String::new();
        }
        if let Some(v) = self.cache.borrow().get(id) {
            return v.clone();
        }
        let v = crate::nodes::get_node(self.conn, id).ok().flatten().map(|n| n.title).unwrap_or_else(|| id.chars().take(10).collect());
        self.cache.borrow_mut().insert(id.to_string(), v.clone());
        v
    }
}

fn all_events(conn: &Connection, campaign_id: &str) -> Result<Vec<EventEntry>> {
    let mut out = vec![];
    let mut after = 0;
    loop {
        let page = campaign::events(conn, campaign_id, after, 5000)?;
        let Some(last) = page.last() else { break };
        after = last.seq;
        let n = page.len();
        out.extend(page);
        if n < 5000 {
            break;
        }
    }
    Ok(out)
}

fn changes(e: &EventEntry) -> Vec<&Value> {
    let list = match e.kind.as_str() {
        "effects" => e.payload.get("changes"),
        "transition" => e.payload.pointer("/outcome/changes"),
        _ => None,
    };
    list.and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default()
}

fn s<'v>(v: &'v Value, k: &str) -> &'v str {
    v.get(k).and_then(Value::as_str).unwrap_or_default()
}

// ===================== 统计 =====================

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ChoiceStat {
    pub turn: i64,
    pub seq: i64,
    /// 从哪个剧情节点。
    pub from: String,
    /// 选了哪条路（剧情边的标签）。
    pub label: String,
    pub to: String,
    /// 同一节点当时可选的出路数（大于 1 才算剧情分支）。
    pub options: i64,
    pub passed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NpcFate {
    pub id: String,
    pub name: String,
    /// alive / dead / …
    pub life_status: String,
    /// 死亡回合（从事件日志中找到的）。
    pub died_turn: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct BondDelta {
    pub from: String,
    pub to: String,
    pub changes: i64,
    pub first_kind: String,
    pub last_kind: String,
    /// 净变化：(强度, 信任, 好感, 敌意)
    pub delta: (i64, i64, i64, i64),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CombatStat {
    pub id: String,
    pub reason: String,
    pub status: String,
    pub rounds: i64,
    pub enemies: i64,
    pub rewards: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CampaignStats {
    pub campaign_id: String,
    pub name: String,
    pub status: String,
    pub ending: String,
    pub turns: i64,
    pub events: i64,
    pub player_actions: i64,
    pub rolls: i64,
    /// 游玩时长（毫秒，按事件时间：第一条到最后一条）。
    pub play_ms: i64,
    pub choices: Vec<ChoiceStat>,
    pub transitions_failed: i64,
    /// 剧情节点：到过 / 总数；结局节点：到过 / 总数。
    pub plot_visited: i64,
    pub plot_total: i64,
    pub endings_total: i64,
    pub branch_points: i64,
    pub npcs: Vec<NpcFate>,
    pub npcs_alive: i64,
    pub npcs_dead: i64,
    pub bond_changes: i64,
    pub bonds: Vec<BondDelta>,
    /// 任务状态 → 数量（"" 表示未开始）。
    pub quests: BTreeMap<String, i64>,
    pub clues_revealed: i64,
    pub clues_total: i64,
    pub combats: Vec<CombatStat>,
    /// 战斗结果 → 次数。
    pub combat_results: BTreeMap<String, i64>,
    /// 事件类型 → 数量。
    pub event_kinds: BTreeMap<String, i64>,
    /// 时间线：父时间线名称与子时间线数量。
    pub parent: Option<String>,
    pub children: i64,
}

/// 一个战役（时间线）的统计。
pub fn campaign_stats(conn: &Connection, campaign_id: &str) -> Result<CampaignStats> {
    let c = campaign::get_campaign(conn, campaign_id)?;
    let names = Names::new(conn, campaign_id);
    let evs = all_events(conn, campaign_id)?;
    let mut st = CampaignStats {
        campaign_id: c.id.clone(),
        name: c.name.clone(),
        status: c.status.clone(),
        ending: c.ending.clone(),
        turns: c.turn,
        events: evs.len() as i64,
        ..Default::default()
    };
    if let (Some(a), Some(b)) = (evs.first(), evs.last()) {
        st.play_ms = b.created_at - a.created_at;
    }
    // 剧情图。
    let (plot_nodes, edges) = match &c.script_id {
        Some(s) => (crate::plot::plot_nodes_of(conn, s)?, crate::plot::edges_of_script(conn, s)?),
        None => (vec![], vec![]),
    };
    let mut out_deg: HashMap<&str, i64> = HashMap::new();
    for e in &edges {
        *out_deg.entry(e.from_id.as_str()).or_insert(0) += 1;
    }
    st.plot_total = plot_nodes.len() as i64;
    st.endings_total = plot_nodes.iter().filter(|n| n.kind == crate::model::PlotNodeKind::Ending).count() as i64;
    st.branch_points = out_deg.values().filter(|n| **n > 1).count() as i64;
    st.plot_visited = conn.query_row("SELECT COUNT(*) FROM plot_visits WHERE campaign_id = ?1", [campaign_id], |r| r.get(0))?;

    let mut died: HashMap<String, i64> = HashMap::new();
    let mut bonds: BTreeMap<(String, String), (BondDelta, Value)> = BTreeMap::new();
    for e in &evs {
        *st.event_kinds.entry(e.kind.clone()).or_insert(0) += 1;
        match e.kind.as_str() {
            "player" => st.player_actions += 1,
            "roll" => st.rolls += 1,
            "transition" | "transition_failed" => {
                let passed = e.kind == "transition";
                if !passed {
                    st.transitions_failed += 1;
                }
                let edge_id = s(&e.payload, "edge");
                let edge = edges.iter().find(|x| x.id == edge_id);
                let from = s(&e.payload["outcome"], "from").to_string();
                st.choices.push(ChoiceStat {
                    turn: e.turn,
                    seq: e.seq,
                    from: names.get(&from),
                    label: edge.map(|x| x.label.clone()).unwrap_or_default(),
                    to: edge.map(|x| names.get(&x.to_id)).unwrap_or_default(),
                    options: out_deg.get(from.as_str()).copied().unwrap_or(0),
                    passed,
                });
            }
            _ => {}
        }
        if e.kind == "transition" {
            st.rolls += e.payload.pointer("/outcome/checks").and_then(Value::as_array).map(|a| a.len() as i64).unwrap_or(0);
        }
        for ch in changes(e) {
            match s(ch, "type") {
                "set_entity_field" if s(ch, "field") == "life_status" && ch["value"].as_str() == Some("dead") => {
                    died.entry(s(ch, "entity").to_string()).or_insert(e.turn);
                }
                "adjust_bond" => {
                    st.bond_changes += 1;
                    let key = (s(ch, "from").to_string(), s(ch, "to").to_string());
                    let ent = bonds.entry(key.clone()).or_insert_with(|| {
                        (
                            BondDelta { from: names.get(&key.0), to: names.get(&key.1), first_kind: s(ch, "old_kind").to_string(), ..Default::default() },
                            ch["old"].clone(),
                        )
                    });
                    ent.0.changes += 1;
                    ent.0.last_kind = s(ch, "kind").to_string();
                    let d = |k: &str| ch["new"][k].as_i64().unwrap_or(0) - ent.1[k].as_i64().unwrap_or(0);
                    ent.0.delta = (d("strength"), d("trust"), d("affection"), d("hostility"));
                }
                _ => {}
            }
        }
    }
    let mut bl: Vec<BondDelta> = bonds.into_values().map(|(b, _)| b).collect();
    bl.sort_by_key(|b| -(b.delta.0.abs() + b.delta.1.abs() + b.delta.2.abs() + b.delta.3.abs()));
    st.bonds = bl;

    for n in crate::schedule::all_npc_status(conn, campaign_id)? {
        let dead = n.life_status == "dead";
        if dead {
            st.npcs_dead += 1;
        } else {
            st.npcs_alive += 1;
        }
        st.npcs.push(NpcFate { died_turn: died.get(&n.npc_id).copied(), id: n.npc_id, name: n.name, life_status: n.life_status });
    }
    for q in crate::quests::quests(conn, campaign_id, true)? {
        *st.quests.entry(q.status).or_insert(0) += 1;
    }
    if let Some(sid) = &c.script_id {
        st.clues_total = crate::scripts::all_items(conn, sid)?.iter().filter(|i| i.kind.to_string() == "clue").count() as i64;
    }
    st.clues_revealed = conn.query_row("SELECT COUNT(*) FROM clue_reveals WHERE campaign_id = ?1", [campaign_id], |r| r.get(0))?;
    {
        let mut stmt = conn.prepare("SELECT data FROM campaign_records WHERE campaign_id = ?1 AND kind = 'combat' ORDER BY id")?;
        let rows: Vec<String> = stmt.query_map([campaign_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        for d in rows {
            let v: Value = serde_json::from_str(&d).unwrap_or(Value::Null);
            let cs = CombatStat {
                id: s(&v, "id").into(),
                reason: s(&v, "reason").into(),
                status: s(&v, "status").into(),
                rounds: v["round"].as_i64().unwrap_or(0),
                enemies: v["units"].as_array().map(|u| u.iter().filter(|x| x["side"].as_str() == Some("enemy")).count() as i64).unwrap_or(0),
                rewards: v["rewards"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            };
            *st.combat_results.entry(cs.status.clone()).or_insert(0) += 1;
            st.combats.push(cs);
        }
    }
    st.parent = c.parent_id.as_ref().and_then(|p| campaign::get_campaign(conn, p).ok()).map(|p| p.name);
    st.children = conn.query_row("SELECT COUNT(*) FROM campaigns WHERE parent_id = ?1", [campaign_id], |r| r.get(0))?;
    Ok(st)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct EndingCount {
    pub ending: String,
    pub count: i64,
    pub campaigns: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct EndingDistribution {
    pub script_id: String,
    pub total: i64,
    pub active: i64,
    pub ended: i64,
    pub endings: Vec<EndingCount>,
    /// 剧本里设计了但还没有任何时间线达到的结局节点。
    pub unreached: Vec<String>,
}

/// 同一剧本的所有战役（含回溯出的时间线）的结局分布。
pub fn ending_distribution(conn: &Connection, script_id: &str) -> Result<EndingDistribution> {
    let cs: Vec<Campaign> = campaign::list_campaigns(conn)?.into_iter().filter(|c| c.script_id.as_deref() == Some(script_id)).collect();
    let mut d = EndingDistribution { script_id: script_id.into(), total: cs.len() as i64, ..Default::default() };
    let mut m: BTreeMap<String, EndingCount> = BTreeMap::new();
    for c in &cs {
        if c.status == "ended" {
            d.ended += 1;
            let e = m.entry(c.ending.clone()).or_insert_with(|| EndingCount { ending: c.ending.clone(), ..Default::default() });
            e.count += 1;
            e.campaigns.push(c.name.clone());
        } else {
            d.active += 1;
        }
    }
    d.endings = m.into_values().collect();
    d.endings.sort_by_key(|e| -e.count);
    let reached: Vec<String> = d.endings.iter().map(|e| e.ending.clone()).collect();
    d.unreached = crate::plot::plot_nodes_of(conn, script_id)?
        .into_iter()
        .filter(|n| n.kind == crate::model::PlotNodeKind::Ending && !reached.contains(&n.title))
        .map(|n| n.title)
        .collect();
    Ok(d)
}

// ===================== 章节 =====================

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Chapter {
    pub no: i64,
    pub title: String,
    /// manual（手动标记）/ summary（会话摘要）/ scene（进入剧情节点）/ all（全程）
    pub source: String,
    /// 章节起点事件序号（手动标记用它来改名/删除）。
    pub seq_from: i64,
    pub seq_to: i64,
    pub turn_from: i64,
    pub turn_to: i64,
    pub summary: String,
}

/// 在当前位置（最后一个事件之后）开始新的一章。只追加事件，不改任何状态。
pub fn mark_chapter(conn: &Connection, campaign_id: &str, title: &str) -> Result<i64> {
    let title = title.trim();
    if title.is_empty() {
        return Err(CoreError::Invalid("章节标题不能为空".into()));
    }
    campaign::get_campaign(conn, campaign_id)?;
    campaign::record(conn, campaign_id, "chapter", &json!({ "title": title.chars().take(80).collect::<String>() }))
}

pub fn rename_chapter(conn: &Connection, campaign_id: &str, seq: i64, title: &str) -> Result<()> {
    if title.trim().is_empty() {
        return Err(CoreError::Invalid("章节标题不能为空".into()));
    }
    campaign::record(conn, campaign_id, "chapter_rename", &json!({ "seq": seq, "title": title.trim() }))?;
    Ok(())
}

pub fn remove_chapter(conn: &Connection, campaign_id: &str, seq: i64) -> Result<()> {
    campaign::record(conn, campaign_id, "chapter_remove", &json!({ "seq": seq }))?;
    Ok(())
}

fn chapters_from(conn: &Connection, c: &Campaign, evs: &[EventEntry], names: &Names) -> Result<Vec<Chapter>> {
    // 手动标记（应用改名与删除）。
    let mut manual: BTreeMap<i64, String> = BTreeMap::new();
    for e in evs {
        match e.kind.as_str() {
            "chapter" => {
                manual.insert(e.seq, s(&e.payload, "title").to_string());
            }
            "chapter_rename" => {
                if let Some(k) = e.payload["seq"].as_i64()
                    && let Some(t) = manual.get_mut(&k)
                {
                    *t = s(&e.payload, "title").to_string();
                }
            }
            "chapter_remove" => {
                if let Some(k) = e.payload["seq"].as_i64() {
                    manual.remove(&k);
                }
            }
            _ => {}
        }
    }
    // 起点：(序号, 标题, 来源, 摘要)
    let mut starts: Vec<(i64, String, String, String)> = manual.into_iter().map(|(k, t)| (k, t, "manual".into(), String::new())).collect();
    let sums = campaign::summaries(conn, &c.id)?;
    if starts.is_empty() && !sums.is_empty() {
        let mut from = 0;
        for (i, sm) in sums.iter().enumerate() {
            starts.push((from, format!("第 {} 次游戏", sm.session_no.max(i as i64 + 1)), "summary".into(), sm.summary.clone()));
            from = sm.upto_seq + 1;
        }
        if evs.last().is_some_and(|l| l.seq >= from) && from > 0 {
            starts.push((from, "最近的游戏".into(), "summary".into(), String::new()));
        }
    }
    if starts.is_empty() {
        for e in evs.iter().filter(|e| e.kind == "transition") {
            if let Some(to) = e.payload.pointer("/outcome/to").and_then(Value::as_str) {
                starts.push((e.seq, names.get(to), "scene".into(), String::new()));
            }
        }
        if starts.first().is_some_and(|f| evs.first().is_some_and(|e| e.seq < f.0)) {
            starts.insert(0, (0, "开场".into(), "scene".into(), String::new()));
        }
    }
    if starts.is_empty() {
        starts.push((0, "全程".into(), "all".into(), String::new()));
    }
    starts.sort_by_key(|x| x.0);
    let last_seq = evs.last().map(|e| e.seq).unwrap_or(0);
    let mut out = vec![];
    for (i, (from, title, src, summary)) in starts.iter().enumerate() {
        let to = starts.get(i + 1).map(|n| n.0 - 1).unwrap_or(last_seq);
        let inside: Vec<&EventEntry> = evs.iter().filter(|e| e.seq >= *from && e.seq <= to).collect();
        out.push(Chapter {
            no: i as i64 + 1,
            title: title.clone(),
            source: src.clone(),
            seq_from: *from,
            seq_to: to,
            turn_from: inside.first().map(|e| e.turn).unwrap_or(0),
            turn_to: inside.last().map(|e| e.turn).unwrap_or(0),
            summary: summary.clone(),
        });
    }
    Ok(out)
}

pub fn chapters(conn: &Connection, campaign_id: &str) -> Result<Vec<Chapter>> {
    let c = campaign::get_campaign(conn, campaign_id)?;
    let names = Names::new(conn, campaign_id);
    chapters_from(conn, &c, &all_events(conn, campaign_id)?, &names)
}

// ===================== 编年史 =====================

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ChronicleEntry {
    pub seq: i64,
    pub turn: i64,
    /// action / scene / progress / death / bond / combat / ending / fork / note
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ChronicleChapter {
    pub chapter: Chapter,
    pub entries: Vec<ChronicleEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Chronicle {
    pub campaign_id: String,
    pub title: String,
    pub chapters: Vec<ChronicleChapter>,
    pub markdown: String,
}

fn short(t: &str, n: usize) -> String {
    let t = t.trim().replace('\n', " ");
    if t.chars().count() > n { format!("{}…", t.chars().take(n).collect::<String>()) } else { t }
}

/// 从事件日志确定性地生成战役编年史（不调用模型；同样的存档永远得到同样的结果）。
pub fn chronicle(conn: &Connection, campaign_id: &str) -> Result<Chronicle> {
    let c = campaign::get_campaign(conn, campaign_id)?;
    let names = Names::new(conn, campaign_id);
    let evs = all_events(conn, campaign_id)?;
    let chs = chapters_from(conn, &c, &evs, &names)?;
    let name_fn = |id: &str| names.get(id);
    let mut entries: Vec<ChronicleEntry> = vec![];
    let push = |v: &mut Vec<ChronicleEntry>, e: &EventEntry, kind: &str, text: String| {
        if !text.trim().is_empty() {
            v.push(ChronicleEntry { seq: e.seq, turn: e.turn, kind: kind.into(), text });
        }
    };
    for e in &evs {
        match e.kind.as_str() {
            "player" => push(&mut entries, e, "action", format!("玩家：{}", short(s(&e.payload, "text"), 80))),
            "narration" => push(&mut entries, e, "note", short(s(&e.payload, "text"), 120)),
            "transition" => {
                if let Some(to) = e.payload.pointer("/outcome/to").and_then(Value::as_str) {
                    push(&mut entries, e, "scene", format!("来到「{}」", names.get(to)));
                }
            }
            "campaign_end" => push(&mut entries, e, "ending", format!("结局：{}", s(&e.payload, "ending"))),
            "timeline_fork" => push(
                &mut entries,
                e,
                "fork",
                format!("从时间线「{}」第 {} 回合回溯，开出这条时间线", s(&e.payload, "from_name"), e.payload["from_turn"].as_i64().unwrap_or(0)),
            ),
            "fact" => push(&mut entries, e, "progress", format!("记下：{}", short(s(&e.payload, "text"), 80))),
            _ => {}
        }
        if matches!(e.kind.as_str(), "effects" | "transition") {
            let p = if e.kind == "effects" { e.payload.clone() } else { e.payload["outcome"].clone() };
            for t in crate::director::progress_in(&p, &name_fn) {
                let kind = if t.starts_with("战斗") || t == "爆发战斗" { "combat" } else { "progress" };
                push(&mut entries, e, kind, t);
            }
            for ch in changes(e) {
                match s(ch, "type") {
                    "set_entity_field" if s(ch, "field") == "life_status" && ch["value"].as_str() == Some("dead") => {
                        push(&mut entries, e, "death", format!("{} 死亡", names.get(s(ch, "entity"))));
                    }
                    "adjust_bond" if s(ch, "old_kind") != s(ch, "kind") => push(
                        &mut entries,
                        e,
                        "bond",
                        format!("{} 与 {} 的关系：{} → {}", names.get(s(ch, "from")), names.get(s(ch, "to")), or_none(s(ch, "old_kind")), or_none(s(ch, "kind"))),
                    ),
                    _ => {}
                }
            }
        }
    }
    let mut out = Chronicle { campaign_id: c.id.clone(), title: c.name.clone(), ..Default::default() };
    for ch in chs {
        let es: Vec<ChronicleEntry> = entries.iter().filter(|x| x.seq >= ch.seq_from && x.seq <= ch.seq_to).cloned().collect();
        out.chapters.push(ChronicleChapter { chapter: ch, entries: es });
    }
    out.markdown = render_markdown(&c, &out.chapters);
    Ok(out)
}

fn or_none(s: &str) -> &str {
    if s.is_empty() { "无" } else { s }
}

fn render_markdown(c: &Campaign, chs: &[ChronicleChapter]) -> String {
    let mut md = format!("# {} 编年史\n\n", c.name);
    md.push_str(&format!("- 状态：{}{}\n- 回合：{}\n\n", if c.status == "ended" { "已结束" } else { "进行中" }, if c.ending.is_empty() { String::new() } else { format!("（{}）", c.ending) }, c.turn));
    for ch in chs {
        md.push_str(&format!("## 第 {} 章 {}（回合 {}–{}）\n\n", ch.chapter.no, ch.chapter.title, ch.chapter.turn_from, ch.chapter.turn_to));
        if !ch.chapter.summary.trim().is_empty() {
            md.push_str(&format!("> {}\n\n", ch.chapter.summary.trim().replace('\n', "\n> ")));
        }
        let mut turn = -1;
        for e in &ch.entries {
            if e.kind == "note" {
                continue;
            }
            if e.turn != turn {
                turn = e.turn;
                md.push_str(&format!("**回合 {turn}**\n\n"));
            }
            md.push_str(&format!("- {}\n", e.text));
        }
        md.push('\n');
    }
    md
}

// ===================== 档案列表与搜索 =====================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ArchiveEntry {
    pub campaign: Campaign,
    pub script_title: String,
    pub events: i64,
    pub last_activity: i64,
    pub children: i64,
    /// 根时间线 ID（同一族时间线共用）。
    pub root_id: String,
    pub depth: i64,
}

/// 所有战役（含回溯出的时间线）的档案列表，按"族"排列：根时间线在前，子时间线紧随其后。
pub fn archive_list(conn: &Connection) -> Result<Vec<ArchiveEntry>> {
    let cs = campaign::list_campaigns(conn)?;
    let mut stats: HashMap<String, (i64, i64)> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT campaign_id, COUNT(*), MAX(created_at) FROM event_log GROUP BY campaign_id")?;
        for r in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<i64>>(2)?.unwrap_or(0))))? {
            let (k, n, t) = r?;
            stats.insert(k, (n, t));
        }
    }
    let titles: HashMap<String, String> = {
        let mut stmt = conn.prepare("SELECT id, title FROM scripts")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    };
    let by_id: HashMap<&str, &Campaign> = cs.iter().map(|c| (c.id.as_str(), c)).collect();
    let root_of = |c: &Campaign| -> (String, i64) {
        let (mut cur, mut d) = (c, 0);
        while let Some(p) = cur.parent_id.as_deref().and_then(|p| by_id.get(p)) {
            cur = p;
            d += 1;
            if d > 1000 {
                break;
            }
        }
        (cur.id.clone(), d)
    };
    let mut out: Vec<ArchiveEntry> = cs
        .iter()
        .map(|c| {
            let (root_id, depth) = root_of(c);
            let (events, last) = stats.get(&c.id).copied().unwrap_or((0, c.updated_at));
            ArchiveEntry {
                script_title: c.script_id.as_ref().and_then(|s| titles.get(s).cloned()).unwrap_or_default(),
                events,
                last_activity: last.max(c.updated_at),
                children: cs.iter().filter(|x| x.parent_id.as_deref() == Some(&c.id)).count() as i64,
                root_id,
                depth,
                campaign: c.clone(),
            }
        })
        .collect();
    let root_created: HashMap<String, i64> = cs.iter().map(|c| (c.id.clone(), c.created_at)).collect();
    out.sort_by_key(|e| (std::cmp::Reverse(root_created.get(&e.root_id).copied().unwrap_or(0)), e.root_id.clone(), e.campaign.created_at));
    Ok(out)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SearchFilter {
    pub query: String,
    /// 为空表示全部战役。
    #[serde(default)]
    pub campaign_ids: Vec<String>,
    /// 为空表示所有类型。
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub limit: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct EventHit {
    pub campaign_id: String,
    pub campaign_name: String,
    pub seq: i64,
    pub turn: i64,
    pub kind: String,
    pub snippet: String,
    pub created_at: i64,
}

fn snippet(text: &str, q: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let lower = text.to_lowercase();
    let pos = lower.find(&q.to_lowercase()).map(|b| lower[..b].chars().count()).unwrap_or(0);
    let start = pos.saturating_sub(30);
    let end = (pos + q.chars().count() + 50).min(chars.len());
    let mut s: String = chars[start..end].iter().collect();
    if start > 0 {
        s = format!("…{s}");
    }
    if end < chars.len() {
        s.push('…');
    }
    s
}

/// 在所有战役的事件日志与会话摘要中搜索（玩家行动、叙述、NPC 发言、事实、章节、结局……）。
pub fn search_events(conn: &Connection, f: &SearchFilter) -> Result<Vec<EventHit>> {
    let q = f.query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let limit = if f.limit <= 0 { 200 } else { f.limit.min(2000) };
    let like = format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
    let names: HashMap<String, String> = campaign::list_campaigns(conn)?.into_iter().map(|c| (c.id, c.name)).collect();
    // 内部记录类事件不参与搜索（模型原始输出、上下文追踪等太长且重复）。
    let skip = ["gm_plan", "context_trace", "director_beat", "trigger_fired"];
    let mut sql = String::from("SELECT campaign_id, seq, turn, kind, payload, created_at FROM event_log WHERE payload LIKE ?1 ESCAPE '\\'");
    let mut args: Vec<rusqlite::types::Value> = vec![like.clone().into()];
    if !f.campaign_ids.is_empty() {
        sql.push_str(&format!(" AND campaign_id IN ({})", f.campaign_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",")));
        args.extend(f.campaign_ids.iter().map(|x| x.clone().into()));
    }
    if !f.kinds.is_empty() {
        sql.push_str(&format!(" AND kind IN ({})", f.kinds.iter().map(|_| "?").collect::<Vec<_>>().join(",")));
        args.extend(f.kinds.iter().map(|x| x.clone().into()));
    } else {
        sql.push_str(&format!(" AND kind NOT IN ({})", skip.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(",")));
    }
    sql.push_str(&format!(" ORDER BY seq DESC LIMIT {limit}"));
    let mut out = vec![];
    {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, i64>(5)?))
        })?;
        for r in rows {
            let (cid, seq, turn, kind, payload, at) = r?;
            let v: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
            // 优先在可读字段里找片段。
            let text = ["text", "ending", "title", "reason"].iter().filter_map(|k| v.get(*k).and_then(Value::as_str)).find(|t| t.to_lowercase().contains(&q.to_lowercase())).map(str::to_string);
            let text = text.unwrap_or_else(|| payload.clone());
            out.push(EventHit { campaign_name: names.get(&cid).cloned().unwrap_or_default(), campaign_id: cid, seq, turn, kind, snippet: snippet(&text, q), created_at: at });
        }
    }
    // 会话摘要。
    let mut stmt = conn.prepare("SELECT campaign_id, session_no, summary, created_at FROM session_summaries WHERE summary LIKE ?1 ESCAPE '\\' LIMIT ?2")?;
    let rows = stmt.query_map(params![like, limit], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))?;
    for r in rows {
        let (cid, no, summary, at) = r?;
        if !f.campaign_ids.is_empty() && !f.campaign_ids.contains(&cid) {
            continue;
        }
        if !f.kinds.is_empty() && !f.kinds.iter().any(|k| k == "summary") {
            continue;
        }
        out.push(EventHit { campaign_name: names.get(&cid).cloned().unwrap_or_default(), campaign_id: cid, seq: 0, turn: no, kind: "summary".into(), snippet: snippet(&summary, q), created_at: at });
    }
    Ok(out)
}

// ===================== 战役 JSON 导入导出 =====================

pub const CAMPAIGN_FORMAT: &str = "chronicle-gm-campaign";
pub const CAMPAIGN_FORMAT_VERSION: i64 = 1;

/// 有 campaign_id 列的所有表（按数据库实际结构取，新增表自动包含）。
fn campaign_tables(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '%_fts%' ORDER BY name")?;
    let all: Vec<String> = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let mut out = vec![];
    for t in all {
        if columns(conn, &t)?.iter().any(|c| c == "campaign_id") {
            out.push(t);
        }
    }
    Ok(out)
}

fn columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
    Ok(stmt.query_map([], |r| r.get::<_, String>(1))?.collect::<rusqlite::Result<_>>()?)
}

fn to_json(v: ValueRef) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => json!(i),
        ValueRef::Real(f) => json!(f),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => json!({ "$blob": b.iter().map(|x| format!("{x:02x}")).collect::<String>() }),
    }
}

fn from_json(v: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as V;
    match v {
        Value::Null => V::Null,
        Value::Bool(b) => V::Integer(*b as i64),
        Value::Number(n) => n.as_i64().map(V::Integer).unwrap_or_else(|| V::Real(n.as_f64().unwrap_or(0.0))),
        Value::String(s) => V::Text(s.clone()),
        Value::Object(o) if o.contains_key("$blob") => {
            let h = o["$blob"].as_str().unwrap_or_default();
            V::Blob((0..h.len() / 2).filter_map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).ok()).collect())
        }
        other => V::Text(other.to_string()),
    }
}

fn dump_rows(conn: &Connection, table: &str, campaign_id: &str) -> Result<Vec<Map<String, Value>>> {
    let cols = columns(conn, table)?;
    // 有 rowid 的表按写入顺序（NPC 记忆与临时实体的检查点按"前 N 条"还原，顺序必须保持）；WITHOUT ROWID 表按主键。
    let sql: String = conn.query_row("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1", [table], |r| r.get(0))?;
    let order = if sql.to_uppercase().contains("WITHOUT ROWID") {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
        let mut pk: Vec<(i64, String)> = stmt.query_map([], |r| Ok((r.get::<_, i64>(5)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?.into_iter().filter(|(k, _)| *k > 0).collect();
        pk.sort();
        pk.into_iter().map(|(_, c)| format!("\"{c}\"")).collect::<Vec<_>>().join(",")
    } else {
        "rowid".to_string()
    };
    let mut stmt = conn.prepare(&format!("SELECT * FROM \"{table}\" WHERE campaign_id = ?1 ORDER BY {order}"))?;
    let mut rows = stmt.query([campaign_id])?;
    let mut out = vec![];
    while let Some(r) = rows.next()? {
        let mut m = Map::new();
        for (i, c) in cols.iter().enumerate() {
            if c == "campaign_id" {
                continue;
            }
            m.insert(c.clone(), to_json(r.get_ref(i)?));
        }
        out.push(m);
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CampaignDump {
    pub campaign: Map<String, Value>,
    pub tables: BTreeMap<String, Vec<Map<String, Value>>>,
    /// 导出时的重放校验结果（导入后必须同样通过）。
    pub verified: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CampaignExport {
    pub format: String,
    pub format_version: i64,
    pub app_version: String,
    pub exported_at: i64,
    pub script_id: Option<String>,
    pub script_title: String,
    pub campaigns: Vec<CampaignDump>,
    /// 全部内容的校验值（发现文件被意外改动）。
    pub checksum: String,
}

fn export_checksum(e: &CampaignExport) -> String {
    let mut c = e.clone();
    c.checksum.clear();
    timeline::checksum(&serde_json::to_string(&c.campaigns).unwrap_or_default())
}

/// 导出一个战役；`with_descendants` 时连同从它回溯出的全部后代时间线一起导出。
pub fn export_campaign(conn: &Connection, campaign_id: &str, with_descendants: bool) -> Result<CampaignExport> {
    let root = campaign::get_campaign(conn, campaign_id)?;
    let mut ids = vec![root.id.clone()];
    if with_descendants {
        let all = campaign::list_campaigns(conn)?;
        let mut i = 0;
        while i < ids.len() {
            let cur = ids[i].clone();
            ids.extend(all.iter().filter(|c| c.parent_id.as_deref() == Some(&cur)).map(|c| c.id.clone()));
            i += 1;
        }
    }
    let tables = campaign_tables(conn)?;
    let mut dumps = vec![];
    for id in &ids {
        let campaign = {
            let cols = columns(conn, "campaigns")?;
            let mut stmt = conn.prepare("SELECT * FROM campaigns WHERE id = ?1")?;
            let mut rows = stmt.query([id])?;
            let r = rows.next()?.ok_or_else(|| CoreError::NotFound("战役", id.clone()))?;
            let mut m = Map::new();
            for (i, c) in cols.iter().enumerate() {
                m.insert(c.clone(), to_json(r.get_ref(i)?));
            }
            m
        };
        let mut t = BTreeMap::new();
        for name in &tables {
            t.insert(name.clone(), dump_rows(conn, name, id)?);
        }
        dumps.push(CampaignDump { campaign, tables: t, verified: timeline::verify(conn, id).map(|r| r.ok).unwrap_or(false) });
    }
    let script_title = match &root.script_id {
        Some(s) => conn.query_row("SELECT title FROM scripts WHERE id = ?1", [s], |r| r.get(0)).unwrap_or_default(),
        None => String::new(),
    };
    let mut e = CampaignExport {
        format: CAMPAIGN_FORMAT.into(),
        format_version: CAMPAIGN_FORMAT_VERSION,
        app_version: env!("CARGO_PKG_VERSION").into(),
        exported_at: now_ms(),
        script_id: root.script_id.clone(),
        script_title,
        campaigns: dumps,
        checksum: String::new(),
    };
    e.checksum = export_checksum(&e);
    Ok(e)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ImportResult {
    /// 新战役 ID（与导出文件中的顺序一致）。
    pub campaign_ids: Vec<String>,
    pub names: Vec<String>,
    pub events: i64,
    pub rows: i64,
    pub warnings: Vec<String>,
}

/// 导入战役 JSON：全部换成新 ID（不覆盖任何现有战役），事件序号与检查点重新编号，导入后逐条时间线重放校验，
/// 任何一步失败整体撤销。剧本必须已在这个世界里（战役引用剧本里的节点与条目）。
pub fn import_campaign(world: &mut World, data: &CampaignExport) -> Result<ImportResult> {
    if data.format != CAMPAIGN_FORMAT {
        return Err(CoreError::Invalid("这不是 Chronicle GM 的战役文件".into()));
    }
    if data.format_version > CAMPAIGN_FORMAT_VERSION {
        return Err(CoreError::Invalid(format!("战役文件版本 {} 比本软件支持的 {} 新，请升级软件", data.format_version, CAMPAIGN_FORMAT_VERSION)));
    }
    if !data.checksum.is_empty() && export_checksum(data) != data.checksum {
        return Err(CoreError::Invalid("战役文件校验值不符（文件可能已损坏或被改动），没有导入".into()));
    }
    if data.campaigns.is_empty() {
        return Err(CoreError::Invalid("战役文件里没有战役".into()));
    }
    let tx = world.conn.transaction()?;
    if let Some(sid) = &data.script_id {
        let n: i64 = tx.query_row("SELECT COUNT(*) FROM scripts WHERE id = ?1", [sid], |r| r.get(0))?;
        if n == 0 {
            return Err(CoreError::Invalid(format!("这个世界里没有战役所用的剧本「{}」。请在剧本所在的世界导入，或先导入那个世界", data.script_title)));
        }
    }
    let tables = campaign_tables(&tx)?;
    let camp_cols = columns(&tx, "campaigns")?;
    let existing: Vec<String> = campaign::list_campaigns(&tx)?.into_iter().map(|c| c.name).collect();
    let mut res = ImportResult::default();
    let mut id_map: HashMap<String, String> = HashMap::new();
    for d in &data.campaigns {
        let old = d.campaign.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        id_map.insert(old, new_id());
    }
    let changesets: std::collections::HashSet<String> = {
        let mut stmt = tx.prepare("SELECT id FROM changesets")?;
        stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    // 父时间线先导入（fork_seq 需要换成父时间线的新序号）。
    let mut seq_maps: HashMap<String, BTreeMap<i64, i64>> = HashMap::new();
    for d in &data.campaigns {
        let old_id = d.campaign.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let new = id_map[&old_id].clone();
        let mut camp = d.campaign.clone();
        camp.insert("id".into(), json!(new));
        let parent_old = camp.get("parent_id").and_then(Value::as_str).map(str::to_string);
        match parent_old.as_ref().and_then(|p| id_map.get(p).map(|n| (p, n))) {
            Some((p, n)) => {
                camp.insert("parent_id".into(), json!(n));
                if let Some(fs) = camp.get("fork_seq").and_then(Value::as_i64) {
                    let m = seq_maps.get(p).and_then(|m| m.range(..=fs).next_back().map(|(_, v)| *v));
                    camp.insert("fork_seq".into(), json!(m));
                }
            }
            None => {
                if parent_old.is_some() {
                    res.warnings.push(format!("「{}」的父时间线没有一起导出，导入后作为独立战役", s(&Value::Object(camp.clone()), "name")));
                }
                camp.insert("parent_id".into(), Value::Null);
                camp.insert("fork_seq".into(), Value::Null);
            }
        }
        let mut name = camp.get("name").and_then(Value::as_str).unwrap_or("战役").to_string();
        if existing.contains(&name) {
            name = format!("{name}（导入）");
        }
        camp.insert("name".into(), json!(name));
        if let Some(cp) = camp.get("current_plot_node").and_then(Value::as_str) {
            let n: i64 = tx.query_row("SELECT COUNT(*) FROM plot_nodes WHERE id = ?1", [cp], |r| r.get(0))?;
            if n == 0 {
                return Err(CoreError::Invalid("战役当前所在的剧情节点在这个世界里不存在（剧本版本不同？），没有导入".into()));
            }
        }
        let cols: Vec<&String> = camp_cols.iter().filter(|c| camp.contains_key(c.as_str())).collect();
        tx.execute(
            &format!("INSERT INTO campaigns ({}) VALUES ({})", cols.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(","), cols.iter().map(|_| "?").collect::<Vec<_>>().join(",")),
            rusqlite::params_from_iter(cols.iter().map(|c| from_json(&camp[c.as_str()]))),
        )?;
        res.names.push(name);

        // 事件日志：保持顺序，记录新旧序号。
        let mut seq_map: BTreeMap<i64, i64> = BTreeMap::new();
        for r in d.tables.get("event_log").into_iter().flatten() {
            tx.execute(
                "INSERT INTO event_log (campaign_id, turn, kind, payload, created_at) VALUES (?1,?2,?3,?4,?5)",
                params![new, r["turn"].as_i64().unwrap_or(0), s(&Value::Object(r.clone()), "kind"), s(&Value::Object(r.clone()), "payload"), r["created_at"].as_i64().unwrap_or(0)],
            )?;
            seq_map.insert(r["seq"].as_i64().unwrap_or(0), tx.last_insert_rowid());
            res.events += 1;
        }
        let map_seq = |old: i64| -> i64 { seq_map.range(..=old).next_back().map(|(_, v)| *v).unwrap_or(0) };
        for t in &tables {
            if t == "event_log" {
                continue;
            }
            let Some(rows) = d.tables.get(t) else { continue };
            let tcols = columns(&tx, t)?;
            for r in rows {
                let mut r = r.clone();
                match t.as_str() {
                    "campaign_checkpoints" => {
                        r.remove("id");
                        if let Some(sq) = r.get("seq").and_then(Value::as_i64) {
                            r.insert("seq".into(), json!(map_seq(sq)));
                        }
                        // 先核对导出时的校验值，再把摘要覆盖范围换成新序号、重算校验值。
                        let state = s(&Value::Object(r.clone()), "state").to_string();
                        if timeline::checksum(&state) != s(&Value::Object(r.clone()), "checksum") {
                            return Err(CoreError::Invalid("战役文件里有检查点校验值不符，没有导入".into()));
                        }
                        let mut st: timeline::CheckpointState = serde_json::from_str(&state)?;
                        for sm in &mut st.summaries {
                            sm.upto_seq = map_seq(sm.upto_seq);
                        }
                        let js = serde_json::to_string(&st)?;
                        r.insert("checksum".into(), json!(timeline::checksum(&js)));
                        r.insert("state".into(), json!(js));
                    }
                    "session_summaries" => {
                        r.insert("id".into(), json!(new_id()));
                        if let Some(u) = r.get("upto_seq").and_then(Value::as_i64) {
                            r.insert("upto_seq".into(), json!(map_seq(u)));
                        }
                    }
                    _ => {
                        if r.contains_key("id") && tcols.iter().any(|c| c == "id") && matches!(t.as_str(), "npc_memories" | "campaign_entities") {
                            r.insert("id".into(), json!(new_id()));
                        }
                    }
                }
                if let Some(cs) = r.get("changeset_id").and_then(Value::as_str)
                    && !changesets.contains(cs)
                {
                    r.insert("changeset_id".into(), Value::Null);
                }
                r.insert("campaign_id".into(), json!(new));
                let cols: Vec<&String> = tcols.iter().filter(|c| r.contains_key(c.as_str())).collect();
                tx.execute(
                    &format!("INSERT INTO \"{t}\" ({}) VALUES ({})", cols.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(","), cols.iter().map(|_| "?").collect::<Vec<_>>().join(",")),
                    rusqlite::params_from_iter(cols.iter().map(|c| from_json(&r[c.as_str()]))),
                )?;
                res.rows += 1;
            }
        }
        seq_maps.insert(old_id, seq_map);
        // 校验：导出时通过的时间线，导入后也必须通过。
        let v = timeline::verify(&tx, &new)?;
        if d.verified && !v.ok {
            return Err(CoreError::Invalid(format!("导入后「{}」的重放校验没有通过，已撤销：{}", res.names.last().cloned().unwrap_or_default(), v.mismatches.join("；"))));
        }
        if !d.verified {
            res.warnings.push(format!("「{}」在导出时就没有通过重放校验", res.names.last().cloned().unwrap_or_default()));
        }
        res.campaign_ids.push(new);
    }
    tx.commit()?;
    Ok(res)
}

// ===================== 完整性 =====================

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CampaignCheck {
    pub campaign_id: String,
    pub name: String,
    pub ok: bool,
    pub checkpoints: usize,
    pub events: usize,
    pub problems: Vec<String>,
    pub notes: Vec<String>,
}

/// 检查所有战役：事件重放与检查点一致、校验值、引用（剧本、剧情节点、父时间线）是否存在、事件序号连续性。
pub fn check_campaigns(conn: &Connection) -> Result<Vec<CampaignCheck>> {
    let cs = campaign::list_campaigns(conn)?;
    let ids: std::collections::HashSet<&str> = cs.iter().map(|c| c.id.as_str()).collect();
    let mut out = vec![];
    for c in &cs {
        let mut ck = CampaignCheck { campaign_id: c.id.clone(), name: c.name.clone(), ok: true, ..Default::default() };
        match timeline::verify(conn, &c.id) {
            Ok(v) => {
                ck.checkpoints = v.checkpoints;
                ck.events = v.events;
                ck.notes = v.notes;
                for f in v.checksum_failures {
                    ck.problems.push(format!("检查点 {f} 的校验值不符"));
                }
                ck.problems.extend(v.mismatches);
            }
            Err(e) => ck.problems.push(format!("无法校验：{e}")),
        }
        if let Some(p) = &c.parent_id
            && !ids.contains(p.as_str())
        {
            ck.problems.push("父时间线不存在".into());
        }
        if let Some(s) = &c.script_id {
            let n: i64 = conn.query_row("SELECT COUNT(*) FROM scripts WHERE id = ?1", [s], |r| r.get(0))?;
            if n == 0 {
                ck.problems.push("所用剧本不存在".into());
            }
        }
        if let Some(pn) = &c.current_plot_node {
            let n: i64 = conn.query_row("SELECT COUNT(*) FROM plot_nodes WHERE id = ?1", [pn], |r| r.get(0))?;
            if n == 0 {
                ck.problems.push("当前剧情节点不存在".into());
            }
        }
        let max_turn: i64 = conn.query_row("SELECT COALESCE(MAX(turn), 0) FROM event_log WHERE campaign_id = ?1", [&c.id], |r| r.get(0))?;
        if max_turn > c.turn {
            ck.problems.push(format!("事件日志里的回合（{max_turn}）超过战役当前回合（{}）", c.turn));
        }
        if c.status == "ended" && c.ending.is_empty() {
            ck.notes.push("已结束但没有记录结局".into());
        }
        ck.ok = ck.problems.is_empty();
        out.push(ck);
    }
    Ok(out)
}

//! GM 与 NPC 的上下文组装。
//!
//! GM 可以看到主持人信息（但叙述中不能说出）；NPC 只能看到自己的资料、自己的知识、
//! 自己的记忆与玩家可见的场景——看不到剧本真相、剧情节点的主持人信息、其他人物的秘密。

use std::collections::HashMap;

use chronicle_core::campaign::{self, CampaignView};
use chronicle_core::model::*;
use chronicle_core::rules::StateView;
use chronicle_core::search::{SearchOptions, search_nodes};
use chronicle_core::{World, nodes, plot, scripts};
use chronicle_gen::context::{Context, core_canon, script_meta};
use serde_json::Value;

use crate::Result;

/// 在场 NPC 的资料。
#[derive(Debug, Clone)]
pub struct NpcCard {
    pub node: Node,
    pub cast: Option<CastMember>,
}

impl NpcCard {
    /// 这个人物自己的秘密（只有 GM 与他本人知道）。
    pub fn secrets(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .node
            .payload
            .get("secrets")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        if let Some(c) = &self.cast
            && !c.secret.trim().is_empty()
        {
            v.push(c.secret.clone());
        }
        if !self.node.gm_text.trim().is_empty() {
            v.push(self.node.gm_text.clone());
        }
        v
    }
}

pub struct GmContext {
    pub text: String,
    /// 玩家不应看到的文字（用于泄露检查）。
    pub hidden: Vec<String>,
    pub npcs: Vec<NpcCard>,
    pub script: Option<Script>,
    pub current: Option<PlotNode>,
    /// 当前可选分支：(给模型看的短编号 E1…, 真实 ID, 标签)。
    /// 实测：同一毫秒生成的 ULID 只有末尾几位不同，模型会抄错，所以只给模型看短编号。
    pub edges: Vec<(String, String, String)>,
    /// 用于显示的 ID → 名称。
    pub names: HashMap<String, String>,
    /// 给模型看的短编号（Q1 任务、C1 线索、I1 物品、P1 剧情节点、N1 人物）→ 真实 ID。
    pub refs: HashMap<String, String>,
    /// 这一回合的上下文记录（控制台的"AI 记忆检索记录"）：各部分是否放进了上下文、检索到的记忆与设定。
    pub trace: ContextTrace,
    /// 记忆与设定的短编号（M1 记忆、W1 设定）→ 原文（GM 在 based_on 里引用时换成文字）。
    pub memory_refs: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct TraceSection {
    pub title: String,
    pub tokens: i64,
    /// kept / partial / dropped
    pub status: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ContextTrace {
    pub budget: i64,
    pub tokens: i64,
    pub sections: Vec<TraceSection>,
    /// 检索到的战役记忆（编号：内容）。
    pub memories: Vec<String>,
    /// 按玩家的话检索到的世界设定（编号：标题）。
    pub search_hits: Vec<String>,
    /// 采用中的导演建议。
    pub guidance: Vec<String>,
}

fn str_field<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn list_field(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

/// 当前场景在场的 NPC：剧情节点对应的剧本条目里列出的人物；没有列出时取剧本全部角色。
/// 在场的 NPC：剧情节点指定的人物（或剧本人物表）中，没有死亡、并且按日程此刻在场景地点的人。
/// 没有日程或场景没有地点的人物，按原来的方式视为在场。
pub fn present_npcs_in(world: &World, campaign_id: &str, script: Option<&Script>, current: Option<&PlotNode>) -> Result<Vec<NpcCard>> {
    let all = present_npcs(world, script, current)?;
    // 玩家所在地点：旅行到过的地点优先，否则是剧情节点的场景地点。
    let scene = chronicle_core::worldsim::party_location(world.conn(), campaign_id)?.or_else(|| current.and_then(|n| n.scene_ref.clone()));
    let mut out = vec![];
    for n in all {
        let st = chronicle_core::schedule::npc_status(world.conn(), campaign_id, &n.node)?;
        if st.life_status == "dead" {
            continue;
        }
        if let (true, Some(sc)) = (st.has_schedule, &scene)
            && !chronicle_core::schedule::is_at(world.conn(), &st, sc)?
        {
            continue;
        }
        out.push(n);
    }
    Ok(out)
}

pub fn present_npcs(world: &World, script: Option<&Script>, current: Option<&PlotNode>) -> Result<Vec<NpcCard>> {
    let Some(s) = script else { return Ok(vec![]) };
    let cast = scripts::cast_of(world.conn(), &s.id)?;
    let mut ids: Vec<String> = current
        .and_then(|n| n.script_item_ref.as_ref())
        .and_then(|r| scripts::get_item(world.conn(), r).ok().flatten())
        .map(|it| list_field(&it.payload, "npc_ids"))
        .unwrap_or_default();
    if ids.is_empty() {
        ids = cast.iter().map(|c| c.node_id.clone()).collect();
    }
    let mut out = vec![];
    for id in ids.iter().take(8) {
        if let Some(node) = nodes::get_node(world.conn(), id)?
            && node.kind == NodeKind::Character
        {
            out.push(NpcCard { cast: cast.iter().find(|c| &c.node_id == id).cloned(), node });
        }
    }
    Ok(out)
}

fn attitude_text(v: i64) -> &'static str {
    match v {
        i64::MIN..=-6 => "敌视",
        -5..=-2 => "反感",
        -1..=1 => "中立",
        2..=5 => "友好",
        _ => "信任",
    }
}

pub fn attitude(world: &World, campaign_id: &str, npc_id: &str) -> i64 {
    let view = CampaignView { conn: world.conn(), campaign_id };
    view.var(&format!("attitude.{npc_id}")).and_then(|v| v.as_i64()).unwrap_or(0)
}

fn dialogue_lines(world: &World, campaign_id: &str, n: i64) -> Result<Vec<String>> {
    Ok(campaign::recent_events(world.conn(), campaign_id, &["player", "narration", "npc"], n)?
        .into_iter()
        .map(|e| {
            let text = str_field(&e.payload, "text").to_string();
            match e.kind.as_str() {
                "player" => format!("玩家：{text}"),
                "narration" => format!("GM：{text}"),
                _ => format!("{}：{text}", str_field(&e.payload, "name")),
            }
        })
        .collect())
}

/// 上下文里最多列出的不在场人物数。
const MAX_AWAY: usize = 20;

pub fn build_gm_context(world: &World, campaign_id: &str, input: &str, budget: usize) -> Result<GmContext> {
    let c = campaign::get_campaign(world.conn(), campaign_id)?;
    let script = match &c.script_id {
        Some(id) => scripts::get_script(world.conn(), id)?,
        None => None,
    };
    let current = campaign::current_node(world, campaign_id)?;
    let npcs = present_npcs_in(world, campaign_id, script.as_ref(), current.as_ref())?;
    let mut names: HashMap<String, String> = HashMap::new();
    let mut hidden = vec![];
    let mut ctx = Context::default();

    let mut refs: HashMap<String, String> = HashMap::new();
    if let Some(s) = &script {
        ctx.push("剧本（含主持人信息）", script_meta(s), 0);
        if !s.core_setting.trim().is_empty() {
            hidden.push(s.core_setting.clone());
        }
        let mut lines = vec![];
        let mut counters: HashMap<&str, usize> = HashMap::new();
        for it in scripts::all_items(world.conn(), &s.id)? {
            names.insert(it.id.clone(), it.title.clone());
            let prefix = match it.kind {
                ScriptItemKind::Quest => "Q",
                ScriptItemKind::Clue => "C",
                ScriptItemKind::Item => "I",
                _ => continue,
            };
            let n = counters.entry(prefix).or_insert(0);
            *n += 1;
            let short = format!("{prefix}{n}");
            lines.push(format!("- {short} [{}] {}：{}", it.kind, it.title, it.player_text));
            refs.insert(short, it.id.clone());
        }
        ctx.push("可在效果中引用的任务（Q）、线索（C）、物品（I）——引用时填编号", lines.join("\n"), 0);
        let mut plines = vec![];
        for (i, p) in plot::plot_nodes_of(world.conn(), &s.id)?.into_iter().enumerate() {
            let short = format!("P{}", i + 1);
            plines.push(format!("- {short} [{}] {}", p.kind, p.title));
            refs.insert(short, p.id);
        }
        ctx.push("剧情节点（unlock_node 填编号）", plines.join("\n"), 3);
    }
    ctx.push("世界核心设定（不得违背）", core_canon(world, 30)?, 1);
    // 时间、环境、位置与世界动态（全部由程序计算）。
    {
        use chronicle_core::worldsim;
        let env = worldsim::environment(world.conn(), campaign_id)?;
        ctx.push(
            "当前时间与环境",
            format!(
                "{}；{}季；天气：{}{}{}",
                env.time,
                env.season,
                env.weather,
                if env.travel_factor > 1.0 { format!("（赶路耗时 ×{:.2}）", env.travel_factor) } else { String::new() },
                if env.bad_weather { "；恶劣天气：户外活动的人物会待在家里，叙述要体现天气" } else { "" }
            ),
            0,
        );
        if let Some(loc) = worldsim::party_location(world.conn(), campaign_id)?.and_then(|l| nodes::get_node(world.conn(), &l).ok().flatten()) {
            ctx.push("玩家所在地点", format!("{}——{}", loc.title, if loc.summary.is_empty() { &loc.player_text } else { &loc.summary }), 0);
        }
        let near = worldsim::reachable(world.conn(), campaign_id, 10)?;
        if !near.is_empty() {
            let mut lines = vec![];
            for (i, r) in near.iter().enumerate() {
                let short = format!("L{}", i + 1);
                names.insert(r.id.clone(), r.title.clone());
                refs.insert(short.clone(), r.id.clone());
                let state = match r.state.as_str() {
                    "explored" => "去过",
                    "heard" => "听说过，没去过",
                    _ => "玩家还不知道这个地方",
                };
                lines.push(format!("- {short} {}（{}，{}）", r.title, worldsim::format_duration(((r.base_minutes as f64) * env.travel_factor).round() as i64), state));
            }
            ctx.push("可以前往的地点（玩家出发前往时 travel_to 填编号，耗时由程序计算；NPC 提到某地时把编号写进 places_heard）", lines.join("
"), 1);
        }
        let evs = worldsim::world_events(world.conn(), campaign_id, 6)?;
        if !evs.is_empty() {
            ctx.push(
                "最近世界上发生的事（玩家不在场：只能通过传闻、告示、NPC 闲谈得知，不要让玩家凭空知道）",
                evs.iter().map(|e| format!("- 第 {} 天：{}", e.day, e.text)).collect::<Vec<_>>().join("
"),
                2,
            );
        }
        let fs = worldsim::factions(world.conn(), campaign_id)?;
        if !fs.is_empty() {
            let nm = |id: &String| fs.iter().find(|f| &f.id == id).map(|f| f.name.clone()).unwrap_or_default();
            let lines: Vec<String> = fs
                .iter()
                .map(|f| {
                    let mut rel = vec![];
                    if !f.at_war_with.is_empty() {
                        rel.push(format!("与{}交战", f.at_war_with.iter().map(nm).collect::<Vec<_>>().join("、")));
                    }
                    if !f.allies.is_empty() {
                        rel.push(format!("与{}结盟", f.allies.iter().map(nm).collect::<Vec<_>>().join("、")));
                    }
                    format!(
                        "- {}：目标 {}；财富 {} 军力 {} 影响 {} 稳定 {}{}{}",
                        f.name,
                        if f.goals.is_empty() { "未定".into() } else { f.goals.join("、") },
                        f.power.wealth,
                        f.power.military,
                        f.power.influence,
                        f.power.stability,
                        if rel.is_empty() { String::new() } else { format!("；{}", rel.join("，")) },
                        if f.last_action.is_empty() { String::new() } else { format!("；最近：{}", f.last_action) }
                    )
                })
                .collect();
            ctx.push("势力局势（主持人信息，由程序按天推演）", lines.join("
"), 2);
        }
    }
    // 不在场的人物此刻在做什么（由日程计算）。
    {
        let present: Vec<&str> = npcs.iter().map(|n| n.node.id.as_str()).collect();
        // 最多列 20 人（有日程或已死亡的优先），其余只给人数：NPC 很多时提示词不随人数线性增长（Phase 8 压测：1000 个 NPC 时多出 7000 字）。
        let mut all: Vec<_> = chronicle_core::schedule::all_npc_status(world.conn(), campaign_id)?.into_iter().filter(|s| !present.contains(&s.npc_id.as_str())).collect();
        all.sort_by_key(|s| (s.life_status != "dead" && s.activity.is_empty()) as u8);
        let hidden = all.len().saturating_sub(MAX_AWAY);
        let mut away: Vec<String> = all
            .into_iter()
            .take(MAX_AWAY)
            .map(|s| {
                if s.life_status == "dead" {
                    format!("- {}：已死亡", s.name)
                } else if s.activity.is_empty() {
                    format!("- {}：行踪不明", s.name)
                } else {
                    format!("- {}：{}{}", s.name, s.activity, if s.location_name.is_empty() { String::new() } else { format!("（{}）", s.location_name) })
                }
            })
            .collect();
        if hidden > 0 {
            away.push(format!("- ……另有 {hidden} 人不在场（行踪不明或没有日程）"));
        }
        if !away.is_empty() {
            ctx.push("不在场的人物此刻在做什么（玩家要找他们得去对应地点）", away.join("\n"), 2);
        }
    }
    rpg_context(world, campaign_id, &npcs, &mut ctx, &mut refs, &mut names)?;
    // 任务：进行中的（含期限）与未开始的可见任务。
    {
        let qs = chronicle_core::quests::quests(world.conn(), campaign_id, true)?;
        // 游戏中新建的任务没有剧本编号：给它们 G1、G2…（quest_notes 和 set_quest_state 可以引用）。
        for (i, q) in qs.iter().filter(|q| q.source == "gm").enumerate() {
            refs.insert(format!("G{}", i + 1), q.id.clone());
        }
        let lines: Vec<String> = qs
            .iter()
            .filter(|q| q.status == "active" || q.status.is_empty())
            .map(|q| {
                let short = refs.iter().find(|(_, v)| **v == q.id).map(|(k, _)| k.clone()).unwrap_or_default();
                format!(
                    "- {short} {}「{}」{}{}{}：{}",
                    match q.quest_type.as_str() { "main" => "主线", "hidden" => "隐藏", _ => "支线" },
                    q.title,
                    if q.status == "active" { "进行中" } else { "未开始" },
                    q.turns_left.map(|t| format!("，还剩 {t} 回合")).unwrap_or_default(),
                    q.minutes_left.map(|m| format!("，故事时间还剩{}", chronicle_core::worldsim::format_duration(m).trim_start_matches('约'))).unwrap_or_default(),
                    q.description
                )
            })
            .collect();
        if !lines.is_empty() {
            ctx.push("任务（状态由程序根据条件与期限推进；变化写在 quest_notes）", lines.join("\n"), 1);
        }
    }
    // 调查板：玩家掌握的线索与玩家自己的推理。
    let inv = chronicle_core::investigation::gm_summary(world.conn(), campaign_id)?;
    if !inv.is_empty() {
        ctx.push("调查板（玩家的推理可能是错的：不要确认或否认，不要替玩家揭晓答案，让证据自己说话）", inv, 1);
    }

    if let Some(n) = &current {
        let mut t = format!("「{}」[{}]\n玩家可见：{}\n主持人信息（不可直接说出）：{}", n.title, n.kind, n.player_text, n.gm_text);
        if let Some(loc) = n.scene_ref.as_ref().and_then(|r| nodes::get_node(world.conn(), r).ok().flatten()) {
            t.push_str(&format!("\n地点：{}——{}", loc.title, loc.summary));
        }
        if !n.gm_text.trim().is_empty() {
            hidden.push(n.gm_text.clone());
        }
        ctx.push("当前剧情节点", t, 0);
    } else {
        ctx.push("当前剧情节点", "（自由沙盒，没有预设剧情）".into(), 0);
    }

    let mut edges = vec![];
    let opts = campaign::available_edges(world, campaign_id)?;
    if !opts.is_empty() {
        let mut lines = vec![];
        for (i, o) in opts.iter().enumerate() {
            let short = format!("E{}", i + 1);
            let to = plot::get_plot_node(world.conn(), &o.edge.to_id)?.map(|n| n.title).unwrap_or_default();
            let avail = match (o.used_up, o.availability) {
                (true, _) => "已走过",
                (_, chronicle_core::rules::Tri::True) => "条件满足",
                (_, chronicle_core::rules::Tri::False) => "条件不满足",
                (_, chronicle_core::rules::Tri::Unknown) => "需要检定",
            };
            let cond = o.edge.condition.as_ref().map(|c| serde_json::to_string(c).unwrap_or_default()).unwrap_or_else(|| "无".into());
            lines.push(format!("- {short}「{}」→ {}（{avail}；条件：{cond}）", o.edge.label, to));
            edges.push((short, o.edge.id.clone(), o.edge.label.clone()));
        }
        ctx.push("可选分支（transition_edge 填编号，如 E1；由程序判定条件）", lines.join("\n"), 0);
    }

    let st = campaign::campaign_state(world.conn(), campaign_id)?;
    let name = |id: &str, names: &HashMap<String, String>| names.get(id).cloned().unwrap_or_else(|| id.to_string());
    let mut state = vec![];
    if !st.quests.is_empty() {
        state.push(format!("任务：{}", st.quests.iter().map(|(q, s)| format!("{}={s}", name(q, &names))).collect::<Vec<_>>().join("，")));
    }
    if !st.clues.is_empty() {
        state.push(format!("已知线索：{}", st.clues.iter().map(|c| name(c, &names)).collect::<Vec<_>>().join("，")));
    }
    let items: Vec<String> = st.items.iter().filter(|(_, n)| **n > 0).map(|(k, n)| format!("{}×{n}", name(k.split('|').nth(1).unwrap_or(k), &names))).collect();
    if !items.is_empty() {
        state.push(format!("物品：{}", items.join("，")));
    }
    let vars: Vec<String> = st.vars.iter().filter(|(k, _)| !k.starts_with("attitude.")).map(|(k, v)| format!("{k}={v}")).collect();
    if !vars.is_empty() {
        state.push(format!("标志与变量：{}", vars.join("，")));
    }
    ctx.push("玩家当前状态", if state.is_empty() { "（尚无）".into() } else { state.join("\n") }, 0);

    if !npcs.is_empty() {
        let mut lines = vec![];
        for (i, n) in npcs.iter().enumerate() {
            names.insert(n.node.id.clone(), n.node.title.clone());
            let short = format!("N{}", i + 1);
            refs.insert(short.clone(), n.node.id.clone());
            let role = n.cast.as_ref().map(|c| c.role.clone()).unwrap_or_default();
            let att = attitude(world, campaign_id, &n.node.id);
            let secrets = n.secrets();
            hidden.extend(secrets.iter().cloned());
            // 战役中的人物关系（程序维护的数值），让 GM 知道这个人现在怎么看玩家。
            let bond = {
                use chronicle_core::rules::StateView;
                campaign::CampaignView { conn: world.conn(), campaign_id }.bond(&n.node.id, "party")
            };
            let bond_text = bond
                .map(|(k, b)| format!("（{}信任 {}，好感 {}，敌意 {}）", if k.is_empty() { String::new() } else { format!("{k}，") }, b.trust, b.affection, b.hostility))
                .unwrap_or_default();
            lines.push(format!(
                "- {short} {}（{}）{}；对玩家：{}{bond_text}；秘密（不可直接说出）：{}",
                n.node.title,
                role,
                if n.node.summary.is_empty() { str_field(&n.node.payload, "identity") } else { &n.node.summary },
                attitude_text(att),
                if secrets.is_empty() { "无".into() } else { secrets.join("；") }
            ));
        }
        ctx.push("在场人物（让他们说话请把编号放进 npc_speakers，如 N1；不要替他们写台词）", lines.join("\n"), 0);
    }

    let ents = campaign::campaign_entities(world.conn(), campaign_id)?;
    if !ents.is_empty() {
        for e in &ents {
            if !e.gm_text.trim().is_empty() {
                hidden.push(e.gm_text.clone());
            }
        }
        ctx.push(
            "本场战役中出现的新人物 / 地点",
            ents.iter().map(|e| format!("- [{}] {}：{}", e.kind, e.name, e.summary)).collect::<Vec<_>>().join("\n"),
            2,
        );
    }

    // 战役记忆与检索到的设定带短编号（M、W），GM 在 based_on 里写明这一回合的决定依据了哪些。
    let mut memory_refs: HashMap<String, String> = HashMap::new();
    let mut trace = ContextTrace { budget: budget as i64, ..Default::default() };
    let mut memory = vec![];
    let confirmed: Vec<_> = campaign::summaries(world.conn(), campaign_id)?.into_iter().filter(|s| s.status == "confirmed").collect();
    let mut m = 0;
    for s in confirmed.iter().skip(confirmed.len().saturating_sub(2)) {
        m += 1;
        let text = format!("第 {} 次会话：{}", s.session_no, s.summary);
        memory.push(format!("M{m} {text}"));
        memory_refs.insert(format!("M{m}"), text.clone());
        trace.memories.push(format!("M{m} {text}"));
    }
    for f in campaign::facts(world.conn(), campaign_id, 20)? {
        m += 1;
        memory.push(format!("M{m} {f}"));
        memory_refs.insert(format!("M{m}"), f.clone());
        trace.memories.push(format!("M{m} {f}"));
    }
    ctx.push("战役记忆（需要记住的事实；based_on 可以引用编号）", memory.join("\n"), 1);
    ctx.push("最近的对话", dialogue_lines(world, campaign_id, 8)?.join("\n"), 1);

    if !input.trim().is_empty() {
        let hits = search_nodes(world.conn(), input, &SearchOptions { limit: 5, ..Default::default() })?;
        let lines: Vec<String> = hits
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let code = format!("W{}", i + 1);
                memory_refs.insert(code.clone(), format!("{}：{}", h.node.title, h.node.summary));
                trace.search_hits.push(format!("{code} [{}] {}", h.node.kind, h.node.title));
                format!("- {code} [{}] {}：{}", h.node.kind, h.node.title, h.node.summary)
            })
            .collect();
        ctx.push("与玩家行动相关的设定", lines.join("\n"), 3);
    }

    // 导演指引（主持人信息）：最近的节奏判断与采用中的建议。只是可以提供的机会，不能强迫玩家，也不能违背已确认设定。
    let (pace, adopted) = chronicle_core::director::guidance(world.conn(), campaign_id)?;
    if pace.is_some() || !adopted.is_empty() {
        let mut lines = vec![];
        if let Some(p) = &pace {
            lines.push(format!("节奏：{p}"));
        }
        for s in &adopted {
            lines.push(format!("- {}", s.text));
            hidden.push(s.text.clone());
            trace.guidance.push(s.text.clone());
        }
        ctx.push("导演建议（主持人信息，不要直接说出；在合适时自然地给玩家机会，不要强迫玩家选择，不要违背已确认的设定）", lines.join("\n"), 1);
    }

    let rendered = ctx.render(budget);
    for s in &ctx.sections {
        let status = if rendered.dropped.iter().any(|d| d == &s.title) {
            "dropped"
        } else if rendered.dropped.iter().any(|d| d.starts_with(&s.title) && d.ends_with("（部分）")) {
            "partial"
        } else {
            "kept"
        };
        trace.sections.push(TraceSection { title: s.title.clone(), tokens: chronicle_gen::context::estimate_tokens(&s.body) as i64, status: status.into() });
    }
    trace.tokens = rendered.tokens as i64;
    Ok(GmContext { text: rendered.text, hidden, npcs, script, current, edges, names, refs, trace, memory_refs })
}

/// 玩家角色、背包（B 编号）、在场商人的商品（S 编号）、可制作的物品（R 编号）、战斗状态。数值全部由程序计算。
fn rpg_context(world: &World, campaign_id: &str, npcs: &[NpcCard], ctx: &mut Context, refs: &mut HashMap<String, String>, names: &mut HashMap<String, String>) -> Result<()> {
    use chronicle_core::{combat, rpg};
    let conn = world.conn();
    let r = rpg::rules(conn, campaign_id);
    let s = rpg::sheet(conn, campaign_id, rpg::PARTY)?;
    let attrs: Vec<String> = rpg::ATTRS.iter().map(|(k, l)| format!("{l}{}({:+})", s.attr(k), s.modv(k))).collect();
    let skills: Vec<String> = s.skills.iter().map(|(k, v)| format!("{k}+{v}")).collect();
    let (wname, wdmg, wrange, _) = rpg::weapon(conn, campaign_id, &s);
    let mut lines = vec![
        format!("{}：{} 级（经验 {}/{}），生命 {}/{}，防御 {}，武器 {wname}（{wdmg}，射程 {wrange}）", s.name, s.level, s.xp, rpg::xp_for_level(&r, s.level + 1), s.hp, s.max_hp, rpg::armor_class(conn, campaign_id, &s)),
        format!("属性：{}；技能：{}", attrs.join(" "), if skills.is_empty() { "无".into() } else { skills.join("、") }),
        format!("金钱：{}", rpg::format_money(&r, rpg::money(conn, campaign_id, rpg::PARTY))),
    ];
    if !s.statuses.is_empty() || !s.wounds.is_empty() {
        lines.push(format!("状态与伤势：{}", s.statuses.iter().map(|x| x.name.clone()).chain(s.wounds.iter().map(|w| format!("{}（检定 -{}）", w.name, w.penalty))).collect::<Vec<_>>().join("、")));
    }
    if s.hp <= 0 {
        lines.push("玩家已经倒下，无法行动，需要休息或救治。".into());
    }
    let inv = rpg::inventory(conn, campaign_id, rpg::PARTY)?;
    if !inv.is_empty() {
        let mut bag = vec![];
        for (i, it) in inv.iter().enumerate().take(30) {
            let code = format!("B{}", i + 1);
            refs.insert(code.clone(), it.item.id.clone());
            names.insert(it.item.id.clone(), it.item.title.clone());
            let mut tags = vec![];
            if it.equipped {
                tags.push("已装备".to_string());
            }
            if !it.item.heal.is_empty() {
                tags.push(format!("可使用，恢复 {}", it.item.heal));
            }
            if !it.item.damage.is_empty() {
                tags.push(format!("伤害 {}", it.item.damage));
            }
            bag.push(format!("{code} {}×{}{}", it.item.title, it.qty, if tags.is_empty() { String::new() } else { format!("（{}）", tags.join("，")) }));
        }
        lines.push(format!("背包：{}", bag.join("；")));
    }
    if s.unspent_skill > 0 || s.unspent_attr > 0 {
        lines.push(format!("有未使用的技能点 {}、属性点 {}（玩家在角色页分配）", s.unspent_skill, s.unspent_attr));
    }
    ctx.push("玩家角色（数值由程序计算，叙述不要自己编造数值）", lines.join("
"), 0);

    let mut shop_lines = vec![];
    let mut si = 0;
    for (ni, n) in npcs.iter().enumerate() {
        if let Some(sh) = rpg::shop(conn, campaign_id, &n.node.id)? {
            let goods: Vec<String> = sh
                .items
                .iter()
                .map(|it| {
                    si += 1;
                    let code = format!("S{si}");
                    refs.insert(code.clone(), it.item.id.clone());
                    names.insert(it.item.id.clone(), it.item.title.clone());
                    format!("{code} {} {}{}", it.item.title, rpg::format_money(&r, it.buy_price), it.stock.map(|q| format!("（剩 {q}）")).unwrap_or_default())
                })
                .collect();
            shop_lines.push(format!("N{} {}：{}；收购按价值的 {}%", ni + 1, sh.name, goods.join("，"), (sh.buy_rate * 100.0).round()));
        }
    }
    if !shop_lines.is_empty() {
        ctx.push("在场商人（买卖写在 trade，价格由程序计算）", shop_lines.join("
"), 1);
    }
    let rc = rpg::recipes(conn, campaign_id, rpg::PARTY)?;
    if !rc.is_empty() {
        let lines: Vec<String> = rc
            .iter()
            .take(10)
            .enumerate()
            .map(|(i, (it, ok))| {
                let code = format!("R{}", i + 1);
                refs.insert(code.clone(), it.id.clone());
                names.insert(it.id.clone(), it.title.clone());
                format!("{code} {}（{}）", it.title, if *ok { "材料齐全" } else { "材料不够" })
            })
            .collect();
        ctx.push("可以制作的物品（制作写在 craft，检定由程序掷骰）", lines.join("
"), 2);
    }
    if let Some(c) = combat::active(conn, campaign_id) {
        ctx.push(
            "战斗进行中（玩家的行动写在 combat_action；命中、伤害、敌人的行动全部由程序计算，不要自己决定结果）",
            format!("{}最近：
{}", combat::summary(&c), c.log.iter().rev().take(6).rev().cloned().collect::<Vec<_>>().join("
")),
            0,
        );
    }
    Ok(())
}

/// 给模型看的短编号前缀：任务、线索、物品、人物、剧情节点、分支、G 新任务、地点、背包、商品、配方、记忆、设定。
const KNOWN_CODE_PREFIXES: [char; 13] = ['Q', 'C', 'I', 'N', 'P', 'E', 'G', 'L', 'B', 'S', 'R', 'M', 'W'];

/// NPC 的提示词变量。只包含该 NPC 本人的资料与玩家可见的信息。
impl GmContext {
    /// 把自由文本里的短编号（Q1、C1、N1、G1……）换成名称，再存进记忆、调查板、任务历史（实测模型会把编号写进文字里）。
    pub fn humanize(&self, text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let prev_ok = i == 0 || !chars[i - 1].is_ascii_alphanumeric();
            if c.is_ascii_uppercase() && prev_ok {
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                let next_ok = j >= chars.len() || !chars[j].is_ascii_alphanumeric();
                if j > i + 1 && next_ok {
                    let code: String = chars[i..j].iter().collect();
                    if let Some(name) = self.refs.get(&code).and_then(|id| self.names.get(id)) {
                        out.push_str(name);
                        i = j;
                        // 紧跟着的称呼已经包含在名字里时不重复（实测："N1村长" → "村长老格村长"）。
                        for k in (2..=4).rev() {
                            if i + k <= chars.len() {
                                let next: String = chars[i..i + k].iter().collect();
                                if name.contains(&next) {
                                    i += k;
                                    break;
                                }
                            }
                        }
                        continue;
                    }
                    // 不认识的编号（实测：自由沙盒里模型写出 C1、C2）不写进玩家能看到的文字。
                    if KNOWN_CODE_PREFIXES.contains(&c) {
                        i = j;
                        continue;
                    }
                }
            }
            out.push(c);
            i += 1;
        }
        // 删掉编号后留下的空括号与多余的顿号。
        let mut s = out;
        for (a, b) in [("（）", ""), ("()", ""), ("（、", "（"), ("、）", "）"), ("、、", "、"), ("：，", "："), ("  ", " ")] {
            while s.contains(a) {
                s = s.replace(a, b);
            }
        }
        s.trim().to_string()
    }

    /// 把模型给出的分支编号（或真实 ID）解析为真实 ID。
    pub fn resolve_edge(&self, r: &str) -> Option<&(String, String, String)> {
        let r = r.trim();
        self.edges.iter().find(|(s, id, _)| s.eq_ignore_ascii_case(r) || id == r)
    }
    /// 把效果里的短编号（Q1、C1、I1、P1、N1）换成真实 ID；不认识的值原样保留，交给校验。
    pub fn map_refs(&self, mut m: serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
        for key in ["quest", "clue", "item", "node", "entity", "a", "b", "from", "to", "npc"] {
            if let Some(Value::String(s)) = m.get(key)
                && let Some(id) = self.refs.get(&s.trim().to_uppercase())
            {
                m.insert(key.into(), Value::String(id.clone()));
            }
        }
        m
    }

    /// 把模型给出的 NPC 编号（或真实 ID）解析为在场 NPC。
    pub fn resolve_npc(&self, r: &str) -> Option<&NpcCard> {
        let r = r.trim();
        self.npcs.iter().enumerate().find(|(i, n)| format!("N{}", i + 1).eq_ignore_ascii_case(r) || n.node.id == r || n.node.title == r).map(|(_, n)| n)
    }
}

pub struct NpcContext {
    pub name: String,
    pub card: String,
    pub knowledge: String,
    pub attitude: String,
    pub memories: String,
    /// 情绪与此刻在做的事。
    pub mood: String,
    pub scene: String,
    pub recent: String,
    /// 检索到的这个人物的记忆（控制台的检索记录）。
    pub memory_list: Vec<String>,
    /// 这个 NPC 不应说出的别人的秘密与剧本真相（用于泄露检查）。
    pub hidden: Vec<String>,
}

pub fn build_npc_context(world: &World, campaign_id: &str, me: &NpcCard, all: &[NpcCard], script: Option<&Script>, current: Option<&PlotNode>) -> Result<NpcContext> {
    let p = &me.node.payload;
    let mut card = vec![];
    for (label, v) in [
        ("身份", str_field(p, "identity").to_string()),
        ("外貌", str_field(p, "appearance").to_string()),
        ("性格", str_field(p, "personality").to_string()),
        ("动机", me.cast.as_ref().map(|c| c.motive.clone()).filter(|m| !m.is_empty()).unwrap_or_else(|| str_field(p, "motivation").to_string())),
        ("在故事中的位置", me.cast.as_ref().map(|c| c.role.clone()).unwrap_or_default()),
        ("和玩家的关系", me.cast.as_ref().map(|c| c.relation_to_players.clone()).unwrap_or_default()),
        ("简介", me.node.summary.clone()),
        ("对外的样子", me.node.player_text.clone()),
    ] {
        if !v.trim().is_empty() {
            card.push(format!("- {label}：{v}"));
        }
    }
    let secrets = me.secrets();
    if !secrets.is_empty() {
        card.push(format!("- 你的秘密（不要轻易说出）：{}", secrets.join("；")));
    }
    let abilities = list_field(p, "abilities");
    if !abilities.is_empty() {
        card.push(format!("- 能力：{}", abilities.join("、")));
    }

    let mut knowledge = list_field(p, "knowledge");
    for r in nodes::relations_of(world.conn(), &me.node.id)? {
        if r.visibility != Visibility::Player {
            continue;
        }
        let other_id = if r.src_id == me.node.id { &r.dst_id } else { &r.src_id };
        if let Some(o) = nodes::get_node(world.conn(), other_id)? {
            knowledge.push(format!("你与「{}」的关系：{}{}", o.title, r.rel_type, if r.label.is_empty() { String::new() } else { format!("（{}）", r.label) }));
        }
    }

    // 商人：商品与价格由程序决定，NPC 报价必须一致。
    if let Some(sh) = chronicle_core::rpg::shop(world.conn(), campaign_id, &me.node.id)? {
        let r = chronicle_core::rpg::rules(world.conn(), campaign_id);
        let goods: Vec<String> = sh.items.iter().map(|i| format!("{} {}{}", i.item.title, chronicle_core::rpg::format_money(&r, i.buy_price), i.stock.map(|q| format!("（剩 {q}）")).unwrap_or_default())).collect();
        knowledge.push(format!("你卖的东西和价格（报价必须按这个，不能自己改价）：{}；收购旧货按价值的 {}%", goods.join("，"), (sh.buy_rate * 100.0).round()));
    }
    let att = attitude(world, campaign_id, &me.node.id);
    // 情绪与此刻在做的事（程序计算）。情绪强烈时影响说话与行为。
    let mood_text = {
        use chronicle_core::rules::StateView;
        let m = campaign::CampaignView { conn: world.conn(), campaign_id }.mood(&me.node.id).unwrap_or_default();
        let mut parts = vec![format!("恐惧 {}，愤怒 {}，怀疑 {}，压力 {}，忠诚 {}（0–100）", m.fear, m.anger, m.suspicion, m.stress, m.loyalty)];
        if m.fear >= 70 {
            parts.push("你非常害怕：说话结巴、想逃走或求饶，不敢硬顶。".into());
        }
        if m.anger >= 70 {
            parts.push("你非常愤怒：语气冲、不愿合作，可能出言威胁。".into());
        }
        if m.suspicion >= 70 {
            parts.push("你高度怀疑玩家：不会透露重要信息，会反问和试探。".into());
        }
        if m.stress >= 70 {
            parts.push("你压力很大：心不在焉、容易说漏嘴或情绪失控。".into());
        }
        if m.loyalty >= 70 {
            parts.push("你对玩家很忠诚：愿意冒险帮忙。".into());
        }
        let st = chronicle_core::schedule::npc_status(world.conn(), campaign_id, &me.node)?;
        if !st.activity.is_empty() {
            parts.push(format!("你此刻正在：{}{}", st.activity, if st.location_name.is_empty() { String::new() } else { format!("（{}）", st.location_name) }));
        }
        parts.join("\n")
    };
    let memories: Vec<String> = campaign::npc_memories(world.conn(), campaign_id, &me.node.id, 8)?.into_iter().map(|m| format!("- {}", m.content)).collect();
    let memory_list: Vec<String> = memories.iter().map(|m| m.trim_start_matches("- ").to_string()).collect();

    let mut scene = String::new();
    if let Some(n) = current {
        scene.push_str(&format!("{}：{}", n.title, n.player_text));
    }
    let here = chronicle_core::worldsim::party_location(world.conn(), campaign_id)?.or_else(|| current.and_then(|n| n.scene_ref.clone()));
    {
        if let Some(loc) = here.as_ref().and_then(|r| nodes::get_node(world.conn(), r).ok().flatten()) {
            scene.push_str(&format!("\n地点：{}——{}", loc.title, loc.player_text));
        }
    }
    {
        let env = chronicle_core::worldsim::environment(world.conn(), campaign_id)?;
        scene.push_str(&format!("\n时间：{}，{}季，天气{}", env.time, env.season, env.weather));
    }
    // 最近发生的事：只取玩家可见的叙述与对话。
    let recent = dialogue_lines(world, campaign_id, 4)?;

    let mut hidden = vec![];
    if let Some(s) = script
        && !s.core_setting.trim().is_empty()
    {
        hidden.push(s.core_setting.clone());
    }
    if let Some(n) = current
        && !n.gm_text.trim().is_empty()
    {
        hidden.push(n.gm_text.clone());
    }
    for other in all.iter().filter(|o| o.node.id != me.node.id) {
        hidden.extend(other.secrets());
    }
    for e in campaign::campaign_entities(world.conn(), campaign_id)? {
        if !e.gm_text.trim().is_empty() {
            hidden.push(e.gm_text);
        }
    }

    Ok(NpcContext {
        name: me.node.title.clone(),
        card: card.join("\n"),
        knowledge: if knowledge.is_empty() { "（没有特别的知识）".into() } else { knowledge.iter().map(|k| format!("- {k}")).collect::<Vec<_>>().join("\n") },
        attitude: format!("{}（{att}）", attitude_text(att)),
        mood: mood_text,
        memories: if memories.is_empty() { "（还没有与玩家打过交道）".into() } else { memories.join("\n") },
        scene,
        memory_list,
        recent: recent.join("\n"),
        hidden,
    })
}

/// 游戏页「此刻的画面」：玩家所在地点与在场人物（都是玩家能看到的），再附上它们的封面缩略图。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SceneEntity {
    pub node_id: String,
    pub title: String,
    pub kind: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SceneView {
    pub location: Option<SceneEntity>,
    pub npcs: Vec<SceneEntity>,
    /// 上面这些节点里有图片的封面（没有图片的不在列表里）。
    pub covers: Vec<chronicle_core::images::ImageThumb>,
}

pub fn scene_view(world: &World, campaign_id: &str) -> Result<SceneView> {
    let c = campaign::get_campaign(world.conn(), campaign_id)?;
    let script = match &c.script_id {
        Some(id) => scripts::get_script(world.conn(), id)?,
        None => None,
    };
    let current = campaign::current_node(world, campaign_id)?;
    let here = chronicle_core::worldsim::party_location(world.conn(), campaign_id)?.or_else(|| current.as_ref().and_then(|n| n.scene_ref.clone()));
    let location = match here {
        Some(id) => nodes::get_node(world.conn(), &id)?.filter(|n| n.deleted_at.is_none()).map(|n| SceneEntity { node_id: n.id, title: n.title, kind: n.kind.as_str().into() }),
        None => None,
    };
    let npcs: Vec<SceneEntity> = present_npcs_in(world, campaign_id, script.as_ref(), current.as_ref())?
        .into_iter()
        .map(|n| SceneEntity { title: n.node.title, node_id: n.node.id, kind: n.node.kind.as_str().into() })
        .collect();
    let ids: Vec<String> = location.iter().map(|l| l.node_id.clone()).chain(npcs.iter().map(|n| n.node_id.clone())).collect();
    let covers = chronicle_core::images::cover_thumbs(world.conn(), &ids)?;
    Ok(SceneView { location, npcs, covers })
}

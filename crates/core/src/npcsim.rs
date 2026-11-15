//! NPC 与世界的每日推演（第 24、25 项，程序部分；由 `worldsim::simulate` 在新的一天开始时调用）：
//! - 势力瓦解：稳定和军力都跌到很低时瓦解，之后不再参与推演；
//! - 倒戈：对所属势力的忠诚跌到很低的成员离开势力；
//! - 失踪：极度恐惧的人物逃走、下落不明（不会为了戏剧效果随机死人，死亡只来自战斗或明确的事件）；
//! - NPC 之间的互动：同一地点的人物碰面，关系按已有关系的性质小幅变化（每天最多 3 对，结果由战役种子决定，可以复现）；
//! - 记忆衰减：NPC 记忆的重要度随回合流逝降低，太淡的记忆不再进入 NPC 的上下文（数据库里保留）。
//!
//! 所有变化都是带原因的效果，进入事件日志，随时间线回溯。

use std::collections::HashMap;

use rusqlite::Connection;
use serde_json::json;

use crate::campaign::{self, CampaignView, NpcMemory};
use crate::error::Result;
use crate::rules::{Effect, StateView};

pub const COLLAPSED_PREFIX: &str = "faction.collapsed.";
const HOSTILE: &[&str] = &["enemy", "rival", "war", "hates", "feud"];

fn hash(seed: i64, day: i64, a: &str, b: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ (seed as u64) ^ ((day as u64) << 17);
    for c in a.bytes().chain([0u8]).chain(b.bytes()) {
        h ^= c as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

pub fn is_collapsed(conn: &Connection, campaign_id: &str, faction: &str) -> bool {
    CampaignView { conn, campaign_id }.var(&format!("{COLLAPSED_PREFIX}{faction}")).is_some_and(|v| v.as_bool() == Some(true))
}

/// 一天的推演：返回效果与事件（kind, 文字, 相关 ID）。
/// 一条世界事件：(类型, 文字, 相关 ID)。
pub type DayEvent = (String, String, Vec<String>);

pub fn daily(conn: &Connection, campaign_id: &str, day: i64, seed: i64) -> Result<(Vec<Effect>, Vec<DayEvent>)> {
    let view = CampaignView { conn, campaign_id };
    let mut eff = vec![];
    let mut events = vec![];
    let name = |id: &str| crate::nodes::get_node(conn, id).ok().flatten().map(|n| n.title).unwrap_or_default();
    let st = campaign::campaign_state(conn, campaign_id)?;
    let alive = |id: &str| st.entities.get(&format!("{id}|life_status")).and_then(|v| v.as_str()).is_none_or(|s| s == "alive");

    // 1. 势力瓦解。
    for f in crate::worldsim::factions(conn, campaign_id)? {
        if is_collapsed(conn, campaign_id, &f.id) {
            continue;
        }
        if f.power.stability <= 5 && f.power.military <= 5 {
            eff.push(Effect::SetVar { key: format!("{COLLAPSED_PREFIX}{}", f.id), value: json!(true) });
            events.push(("collapse".to_string(), format!("{}内外交困，彻底瓦解了", f.name), vec![f.id.clone()]));
            // 地点改变：它统治的地方没人管了。
            let mut places: Vec<String> = crate::worldsim::rulers(conn, campaign_id)?.into_iter().filter(|(_, r)| r == &f.id).map(|(p, _)| p).collect();
            places.sort();
            for p in places {
                eff.push(Effect::SetRelation { a: f.id.clone(), b: p.clone(), rel_type: "rules".into(), active: false });
                events.push(("place_change".to_string(), format!("{}瓦解后，{}陷入无人管辖的混乱", f.name, name(&p)), vec![f.id.clone(), p]));
            }
        }
    }

    // 2. 倒戈：和所属势力的关系很差（不信任或敌视）的成员离开；势力瓦解后成员各奔东西。
    //   （注意：情绪里的 loyalty 是对玩家的忠诚，不能用来判断对势力的态度。）
    let mut stmt = conn.prepare(
        "SELECT r.src_id, r.dst_id FROM relations r JOIN nodes n ON n.id = r.src_id AND n.kind = 'character' AND n.deleted_at IS NULL
         JOIN nodes f ON f.id = r.dst_id AND f.kind = 'faction' AND f.deleted_at IS NULL
         WHERE r.deleted_at IS NULL AND r.rel_type IN ('member_of', 'serves')",
    )?;
    let membership: Vec<(String, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(|x| x.ok()).collect();
    for (npc, faction) in &membership {
        if !alive(npc) || st.relations.get(&format!("{npc}|{faction}|member_of")).is_some_and(|a| !a) {
            continue;
        }
        let collapsed = is_collapsed(conn, campaign_id, faction) || eff.iter().any(|e| matches!(e, Effect::SetVar { key, .. } if key == &format!("{COLLAPSED_PREFIX}{faction}")));
        let fed_up = view.bond(npc, faction).is_some_and(|(_, b)| b.trust <= -30 || b.hostility >= 50);
        if collapsed || fed_up {
            eff.push(Effect::SetRelation { a: npc.clone(), b: faction.clone(), rel_type: "member_of".into(), active: false });
            let text = if collapsed { format!("{}瓦解后，{}另谋出路", name(faction), name(npc)) } else { format!("{}对{}心灰意冷，离开了", name(npc), name(faction)) };
            events.push(("defection".to_string(), text, vec![faction.clone(), npc.clone()]));
        }
    }

    // 3. 失踪：极度恐惧的人逃走。
    for s in crate::schedule::all_npc_status(conn, campaign_id)? {
        if !alive(&s.npc_id) {
            continue;
        }
        if let Some(m) = view.mood(&s.npc_id)
            && m.fear >= 90
        {
            eff.push(Effect::SetEntityState { entity: s.npc_id.clone(), field: "life_status".into(), value: json!("missing") });
            events.push(("disappear".to_string(), format!("{}害怕到了极点，连夜逃走，下落不明", s.name), vec![s.npc_id.clone()]));
        }
    }

    // 4. 同一地点的人物碰面（中午的位置）。
    let mut by_place: HashMap<String, Vec<String>> = HashMap::new();
    for s in crate::schedule::all_npc_status_at(conn, campaign_id, (day - 1) * 1440 + 12 * 60)? {
        if let Some(l) = s.location_id.filter(|_| alive(&s.npc_id)) {
            by_place.entry(l).or_default().push(s.npc_id);
        }
    }
    let mut pairs: Vec<(u64, String, String, String)> = vec![];
    for (place, mut ids) in by_place {
        ids.sort();
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                pairs.push((hash(seed, day, &ids[i], &ids[j]), ids[i].clone(), ids[j].clone(), place.clone()));
            }
        }
    }
    pairs.sort();
    for (_, a, b, place) in pairs.into_iter().take(3) {
        let hostile = crate::nodes::relations_of(conn, &a)?.iter().any(|r| (r.src_id == b || r.dst_id == b) && HOSTILE.contains(&r.rel_type.as_str()))
            || view.bond(&a, &b).is_some_and(|(_, x)| x.hostility >= 40);
        let (trust, hostility, text) = if hostile { (-2, 3, format!("{}和{}在{}碰面，不欢而散", name(&a), name(&b), name(&place))) } else { (2, 0, format!("{}和{}在{}碰面，聊了一阵", name(&a), name(&b), name(&place))) };
        eff.push(Effect::AdjustBond { from: a.clone(), to: b.clone(), kind: String::new(), strength: 1, trust, affection: if hostile { 0 } else { 1 }, hostility, reason: format!("第 {day} 天：{text}"), mutual: true });
        if hostile {
            events.push(("npc_meet".to_string(), text, vec![a, b]));
        }
    }
    Ok((eff, events))
}

/// 记忆衰减：每过 10 回合重要度减 1；减到 0 的记忆不再进入上下文（数据库里保留）。
pub fn decayed(memories: Vec<NpcMemory>, now_turn: i64) -> Vec<NpcMemory> {
    let mut out: Vec<NpcMemory> = memories
        .into_iter()
        .map(|mut m| {
            m.importance -= (now_turn - m.turn).max(0) / 10;
            m
        })
        .filter(|m| m.importance > 0)
        .collect();
    out.sort_by(|a, b| b.importance.cmp(&a.importance).then(b.turn.cmp(&a.turn)));
    out
}

/// NPC 之间的关系（给 NPC 自己的上下文：它怎么看其他人）。
pub fn bonds_text(conn: &Connection, campaign_id: &str, npc: &str, limit: usize) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT to_id, trust, affection, hostility FROM campaign_bonds WHERE campaign_id = ?1 AND from_id = ?2 AND to_id != 'party' ORDER BY (ABS(trust) + ABS(affection) + hostility) DESC LIMIT ?3")?;
    let rows: Vec<(String, i64, i64, i64)> = stmt.query_map(rusqlite::params![campaign_id, npc, limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.filter_map(|x| x.ok()).collect();
    Ok(rows
        .into_iter()
        .filter_map(|(to, t, a, h)| {
            let n = crate::nodes::get_node(conn, &to).ok().flatten()?.title;
            let feel = if h >= 40 { "敌视" } else if a >= 30 || t >= 30 { "信任、亲近" } else if t <= -20 { "不信任" } else { "一般" };
            Some(format!("你对「{n}」：{feel}（信任 {t}，好感 {a}，敌意 {h}）"))
        })
        .collect())
}

// ===================== 传闻（第 34 项） =====================

/// 会传开的世界事件类型（暗中进行的 npc_secret 不会）。
const PUBLIC_KINDS: &[&str] = &["npc_action", "war", "peace", "battle", "coup", "conquest", "collapse", "defection", "disappear", "npc_meet", "place_change"];
pub const RUMOR: &str = "rumor";

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct RumorKnown {
    /// 第几手（0 = 亲历或第一手消息）。
    pub hop: i64,
    /// 可信度 0–100。
    pub credibility: i64,
    /// 这个人听到的版本（越传越走样）。
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Rumor {
    pub id: String,
    /// 原本发生的事（主持人可见）。
    pub truth: String,
    pub day: i64,
    pub known_by: std::collections::BTreeMap<String, RumorKnown>,
}

/// 越传越走样：按传的次数换说法，可信度每传一手降 15（最低 10）。
pub fn distort(truth: &str, hop: i64, salt: u64) -> String {
    match hop {
        0 => truth.to_string(),
        1 => format!("听说{truth}"),
        2 => {
            let tails = ["，不过说法不太一样", "，好像还不止这些", "，具体怎么回事没人说得清"];
            format!("有人说{truth}{}", tails[(salt % tails.len() as u64) as usize])
        }
        _ => {
            let tails = ["，传得神乎其神", "，据说背后另有隐情", "，越传越离谱"];
            format!("街头巷尾都在传：{truth}{}", tails[(salt % tails.len() as u64) as usize])
        }
    }
}

pub fn rumors(conn: &Connection, campaign_id: &str) -> Result<Vec<Rumor>> {
    let mut stmt = conn.prepare("SELECT data FROM campaign_records WHERE campaign_id = ?1 AND kind = 'rumor' ORDER BY id DESC LIMIT 30")?;
    let rows: Vec<String> = stmt.query_map([campaign_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(rows.into_iter().filter_map(|d| serde_json::from_str(&d).ok()).collect())
}

/// 一天的传闻：当天的公开事件成为新传闻（先被当事人和所属势力的人知道），已有的传闻在碰面的人之间传开。
pub fn rumor_day(conn: &Connection, campaign_id: &str, day: i64, seed: i64, events: &[DayEvent]) -> Result<Vec<Effect>> {
    let mut list = rumors(conn, campaign_id)?;
    let mut changed: std::collections::BTreeSet<String> = Default::default();
    // 1. 新传闻。
    for (i, (kind, text, ids)) in events.iter().enumerate() {
        if !PUBLIC_KINDS.contains(&kind.as_str()) {
            continue;
        }
        let mut known = std::collections::BTreeMap::new();
        for id in ids {
            let is_faction = crate::nodes::get_node(conn, id)?.is_some_and(|n| n.kind == crate::model::NodeKind::Faction);
            let people: Vec<String> = if is_faction {
                let mut stmt = conn.prepare("SELECT src_id FROM relations WHERE deleted_at IS NULL AND dst_id = ?1 AND rel_type IN ('member_of','serves','leads') LIMIT 6")?;
                stmt.query_map([id], |r| r.get::<_, String>(0))?.filter_map(|x| x.ok()).collect()
            } else {
                vec![id.clone()]
            };
            for p in people {
                known.insert(p, RumorKnown { hop: 0, credibility: 100, text: text.clone() });
            }
        }
        if known.is_empty() {
            continue;
        }
        let r = Rumor { id: format!("d{day:06}-r{i:02}"), truth: text.clone(), day, known_by: known };
        changed.insert(r.id.clone());
        list.push(r);
    }
    // 2. 传开：中午同一地点的人互相说。
    let mut by_place: HashMap<String, Vec<String>> = HashMap::new();
    for s in crate::schedule::all_npc_status_at(conn, campaign_id, (day - 1) * 1440 + 12 * 60)? {
        if let Some(l) = s.location_id {
            by_place.entry(l).or_default().push(s.npc_id);
        }
    }
    for ids in by_place.values() {
        for r in list.iter_mut() {
            let givers: Vec<(String, RumorKnown)> = ids.iter().filter_map(|i| r.known_by.get(i).map(|k| (i.clone(), k.clone()))).collect();
            let Some((g, gk)) = givers.into_iter().min_by_key(|(_, k)| k.hop) else { continue };
            let new: Vec<String> = ids.iter().filter(|i| !r.known_by.contains_key(*i)).cloned().collect();
            for to in &new {
                let hop = gk.hop + 1;
                let salt = hash(seed, day, &g, to.as_str());
                r.known_by.insert(to.clone(), RumorKnown { hop, credibility: (gk.credibility - 15).max(10), text: distort(&r.truth, hop, salt) });
                changed.insert(r.id.clone());
            }
        }
    }
    Ok(list
        .into_iter()
        .filter(|r| changed.contains(&r.id))
        .map(|r| Effect::UpsertRecord { kind: RUMOR.into(), id: r.id.clone(), data: serde_json::to_value(&r).unwrap_or_default() })
        .collect())
}

/// 某个人物知道的传闻（新的在前）。
pub fn rumors_known_by(conn: &Connection, campaign_id: &str, npc: &str, limit: usize) -> Result<Vec<RumorKnown>> {
    Ok(rumors(conn, campaign_id)?.into_iter().filter_map(|r| r.known_by.get(npc).cloned()).take(limit).collect())
}

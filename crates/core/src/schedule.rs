//! 世界时钟与 NPC 日程（程序计算，不调用模型）。
//!
//! - 时钟存在受保护变量 `time.minutes`（从第 1 天 00:00 起的分钟数，默认从第 1 天 08:00 开始），
//!   通过效果修改，因此随检查点回溯、参与重放校验。GM 不能直接改它。
//! - 人物资料里的 `schedule`：`[{"start":"08:00","end":"18:00","activity":"打铁","location":"<地点节点ID>"}]`，
//!   结束时间早于开始时间表示跨过午夜。
//! - 时间推进时：活动发生变化的 NPC 各记一条 `npc_activity` 事件（玩家不在场时 NPC 也在按日程行动）；
//!   NPC 的恐惧、愤怒、压力随时间逐渐平复，怀疑消退得更慢。

use chronicle_rules::{Effect, StateView};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::campaign::{self, CampaignView};
use crate::error::{CoreError, Result};
use crate::model::NodeKind;
use crate::world::World;

pub const CLOCK_VAR: &str = "time.minutes";
pub const DEFAULT_START: i64 = 8 * 60;

pub fn clock(conn: &Connection, campaign_id: &str) -> i64 {
    CampaignView { conn, campaign_id }.var(CLOCK_VAR).and_then(|v| v.as_i64()).unwrap_or(DEFAULT_START)
}

/// "第 2 天 14:30"
pub fn format_clock(minutes: i64) -> String {
    let day = minutes.div_euclid(1440) + 1;
    let m = minutes.rem_euclid(1440);
    let period = match m / 60 {
        0..=4 => "深夜",
        5..=7 => "清晨",
        8..=11 => "上午",
        12..=13 => "中午",
        14..=17 => "下午",
        18..=20 => "傍晚",
        _ => "夜里",
    };
    format!("第 {day} 天 {:02}:{:02}（{period}）", m / 60, m % 60)
}

fn parse_hm(s: &str) -> Option<i64> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (i64, i64) = (h.parse().ok()?, m.parse().ok()?);
    ((0..=24).contains(&h) && (0..60).contains(&m)).then_some(h * 60 + m)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ScheduleEntry {
    pub start: String,
    pub end: String,
    pub activity: String,
    #[serde(default)]
    pub location: String,
    /// 户外活动：恶劣天气（暴雨、暴雪）时改为待在家里（人物资料的 `home`）。
    #[serde(default)]
    pub outdoor: bool,
}

pub fn schedule_of(payload: &Value) -> Vec<ScheduleEntry> {
    payload.get("schedule").and_then(|v| serde_json::from_value::<Vec<ScheduleEntry>>(v.clone()).ok()).unwrap_or_default()
}

/// 检查日程格式（时间写法），用于编辑时提示。
pub fn validate_schedule(entries: &[ScheduleEntry]) -> Result<()> {
    for e in entries {
        if parse_hm(&e.start).is_none() || parse_hm(&e.end).is_none() {
            return Err(CoreError::Invalid(format!("日程时间格式应为 HH:MM：{} - {}", e.start, e.end)));
        }
        if e.activity.trim().is_empty() {
            return Err(CoreError::Invalid("日程的活动不能为空".into()));
        }
    }
    Ok(())
}

/// 某一时刻的日程条目（取第一个覆盖该时刻的条目）。
pub fn entry_at(entries: &[ScheduleEntry], minutes: i64) -> Option<&ScheduleEntry> {
    let t = minutes.rem_euclid(1440);
    entries.iter().find(|e| match (parse_hm(&e.start), parse_hm(&e.end)) {
        (Some(s), Some(en)) if s <= en => t >= s && t < en,
        (Some(s), Some(en)) => t >= s || t < en, // 跨午夜
        _ => false,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NpcStatus {
    pub npc_id: String,
    pub name: String,
    pub activity: String,
    pub location_id: Option<String>,
    pub location_name: String,
    /// alive / dead / ...（战役中的覆盖优先）
    pub life_status: String,
    pub has_schedule: bool,
}

/// 某一时刻实际的（活动, 地点）：日程条目，户外活动遇到恶劣天气时改为待在家里。
pub fn effective_at(conn: &Connection, campaign_id: &str, payload: &Value, minutes: i64) -> Result<Option<(String, String)>> {
    let sched = schedule_of(payload);
    let Some(e) = entry_at(&sched, minutes) else { return Ok(None) };
    if e.outdoor {
        let env = crate::worldsim::environment_at(conn, campaign_id, minutes)?;
        if env.bad_weather {
            let home = payload.get("home").and_then(Value::as_str).unwrap_or_default().to_string();
            return Ok(Some((format!("因{}待在家里（原本：{}）", env.weather, e.activity), home)));
        }
    }
    Ok(Some((e.activity.clone(), e.location.clone())))
}

pub fn npc_status(conn: &Connection, campaign_id: &str, npc: &crate::model::Node) -> Result<NpcStatus> {
    npc_status_at(conn, campaign_id, npc, clock(conn, campaign_id))
}

/// 某个时刻（故事时间，分钟）人物在做什么、在哪里。
pub fn npc_status_at(conn: &Connection, campaign_id: &str, npc: &crate::model::Node, minutes: i64) -> Result<NpcStatus> {
    let life = campaign::entity_overrides(conn, campaign_id, &npc.id)?
        .get("life_status")
        .and_then(|v| v.as_str().map(str::to_string))
        .or_else(|| npc.payload.get("life_status").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| "alive".into());
    let sched = schedule_of(&npc.payload);
    let e = effective_at(conn, campaign_id, &npc.payload, minutes)?;
    let (loc_id, loc_name) = match e.as_ref().map(|e| e.1.trim()).filter(|l| !l.is_empty()) {
        Some(l) => match crate::nodes::get_node(conn, l)? {
            Some(n) => (Some(n.id), n.title),
            None => (None, l.to_string()),
        },
        None => (None, String::new()),
    };
    Ok(NpcStatus {
        npc_id: npc.id.clone(),
        name: npc.title.clone(),
        activity: e.map(|e| e.0).unwrap_or_default(),
        location_id: loc_id,
        location_name: loc_name,
        life_status: life,
        has_schedule: !sched.is_empty(),
    })
}

/// 判断 NPC 是否在某个地点：日程地点就是该地点，或在该地点之内（世界树中的下级）。
pub fn is_at(conn: &Connection, status: &NpcStatus, scene: &str) -> Result<bool> {
    let Some(loc) = &status.location_id else { return Ok(false) };
    if loc == scene {
        return Ok(true);
    }
    Ok(match (crate::nodes::get_node(conn, loc)?, crate::nodes::get_node(conn, scene)?) {
        (Some(l), Some(s)) => l.path.starts_with(&s.path),
        _ => false,
    })
}

/// 参与日程推演的 NPC：剧本人物表；没有剧本时是带日程的世界人物（最多 300 个）。
fn npcs(conn: &Connection, campaign_id: &str) -> Result<Vec<crate::model::Node>> {
    let c = campaign::get_campaign(conn, campaign_id)?;
    let mut out = vec![];
    match &c.script_id {
        Some(s) => {
            for cm in crate::scripts::cast_of(conn, s)? {
                if let Some(n) = crate::nodes::get_node(conn, &cm.node_id)? {
                    out.push(n);
                }
            }
        }
        None => {
            let mut stmt = conn.prepare("SELECT id FROM nodes WHERE kind = ?1 AND deleted_at IS NULL AND instr(payload, '\"schedule\"') > 0 LIMIT 300")?;
            let ids: Vec<String> = stmt.query_map([NodeKind::Character.as_str()], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            for id in ids {
                if let Some(n) = crate::nodes::get_node(conn, &id)? {
                    out.push(n);
                }
            }
        }
    }
    Ok(out)
}

pub fn all_npc_status(conn: &Connection, campaign_id: &str) -> Result<Vec<NpcStatus>> {
    npcs(conn, campaign_id)?.iter().map(|n| npc_status(conn, campaign_id, n)).collect()
}

pub fn all_npc_status_at(conn: &Connection, campaign_id: &str, minutes: i64) -> Result<Vec<NpcStatus>> {
    npcs(conn, campaign_id)?.iter().map(|n| npc_status_at(conn, campaign_id, n, minutes)).collect()
}

/// 推进世界时间（批量、由程序计算）。返回 NPC 活动变化的说明。
pub fn advance(world: &mut World, campaign_id: &str, minutes: i64, reason: &str) -> Result<Vec<String>> {
    let minutes = minutes.clamp(0, 7 * 1440);
    if minutes == 0 {
        return Ok(vec![]);
    }
    let before = clock(world.conn(), campaign_id);
    let after = before + minutes;
    let people = npcs(world.conn(), campaign_id)?;
    let mut effects = vec![Effect::SetVar { key: CLOCK_VAR.into(), value: json!(after) }];
    // 情绪随时间平复：每小时恐惧、愤怒、压力 -5，怀疑 -2。
    let hours = after / 60 - before / 60;
    if hours > 0 {
        let view = CampaignView { conn: world.conn(), campaign_id };
        for n in &people {
            if let Some(m) = view.mood(&n.id)
                && (m.fear > 0 || m.anger > 0 || m.stress > 0 || m.suspicion > 0)
            {
                // 平复量不超过当前值（实测：跳过 7 天时 -5×168 超出了允许的增量范围，整个时间推进失败）。
                effects.push(Effect::AdjustMood {
                    npc: n.id.clone(),
                    fear: -(5 * hours).min(m.fear.max(0)),
                    anger: -(5 * hours).min(m.anger.max(0)),
                    suspicion: -(2 * hours).min(m.suspicion.max(0)),
                    stress: -(5 * hours).min(m.stress.max(0)),
                    loyalty: 0,
                    reason: format!("时间过去了 {hours} 小时，情绪逐渐平复"),
                });
            }
        }
    }
    campaign::apply_effects(world, campaign_id, &effects, reason)?;
    // 世界模拟：环境、势力（按天批量）、触发器。
    let mut world_notes = crate::worldsim::simulate(world, campaign_id, before, after)?;
    // 人物：自然恢复、状态与伤势到期。
    world_notes.extend(crate::rpg::time_passed(world, campaign_id, before, after)?);
    // 日程变化：玩家不在场时 NPC 也按日程活动（每人一条记录）。
    let mut notes = vec![];
    for n in &people {
        if schedule_of(&n.payload).is_empty() {
            continue;
        }
        let (a, b) = (effective_at(world.conn(), campaign_id, &n.payload, before)?, effective_at(world.conn(), campaign_id, &n.payload, after)?);
        if a != b {
            let st = npc_status(world.conn(), campaign_id, n)?;
            if st.life_status == "dead" {
                continue;
            }
            let text = if st.activity.is_empty() { format!("{}结束了之前的活动", n.title) } else { format!("{}：{}{}", n.title, st.activity, if st.location_name.is_empty() { String::new() } else { format!("（{}）", st.location_name) }) };
            campaign::record(
                world.conn(),
                campaign_id,
                "npc_activity",
                &json!({ "npc_id": n.id, "name": n.title, "activity": st.activity, "location_id": st.location_id, "location": st.location_name, "time": after, "text": text }),
            )?;
            notes.push(text);
        }
    }
    notes.extend(world_notes);
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_lookup_handles_midnight_and_format() {
        let s: Vec<ScheduleEntry> = serde_json::from_value(json!([
            {"start": "08:00", "end": "18:00", "activity": "打铁"},
            {"start": "18:00", "end": "22:00", "activity": "喝酒"},
            {"start": "22:00", "end": "08:00", "activity": "睡觉"}
        ]))
        .unwrap();
        assert_eq!(entry_at(&s, 9 * 60).unwrap().activity, "打铁");
        assert_eq!(entry_at(&s, 19 * 60).unwrap().activity, "喝酒");
        assert_eq!(entry_at(&s, 23 * 60).unwrap().activity, "睡觉");
        assert_eq!(entry_at(&s, 1440 + 3 * 60).unwrap().activity, "睡觉", "第二天凌晨");
        assert_eq!(format_clock(DEFAULT_START), "第 1 天 08:00（上午）");
        assert_eq!(format_clock(1440 + 19 * 60 + 5), "第 2 天 19:05（傍晚）");
        assert!(validate_schedule(&s).is_ok());
        let bad: Vec<ScheduleEntry> = serde_json::from_value(json!([{"start": "8点", "end": "18:00", "activity": "x"}])).unwrap();
        assert!(validate_schedule(&bad).is_err());
    }
}

//! 世界模拟（全部由程序计算，确定性，不调用模型）：
//! - 环境：昼夜（由时刻决定）、季节（由天数决定）、天气（由战役种子 + 天数 + 季节决定，可复现）。
//!   环境写入变量 `env.*`，任务与事件的条件可以引用；恶劣天气与夜晚让旅行变慢，户外日程改为待在家里。
//! - 地图：地点的探索状态（未知 / 未探索 / 已探索）、玩家标记、旅行路线与耗时（最短路径）。
//! - 势力：按天批量推进，每个势力每天一个行动（发展、征兵、贸易、作战、政变、结盟、宣战、停战）。
//! - 事件驱动：剧本中的触发器 / 关键事件带条件与效果，时间推进时条件满足就触发。
//!
//! 所有变化都通过效果写入事件日志，随时间线回溯恢复。

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};

use chronicle_rules::{Condition, Effect, StateView, Tri};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::campaign::{self, CampaignView};
use crate::error::{CoreError, Result};
use crate::model::ScriptItemKind;
use crate::world::World;

// ===================== 环境 =====================

pub const SEASONS: [&str; 4] = ["春", "夏", "秋", "冬"];
pub const DAYS_PER_SEASON_VAR: &str = "cal.days_per_season";
pub const WEATHER_OVERRIDE_VAR: &str = "env.weather_override";

/// splitmix64：确定性的伪随机数（同样的种子、天数、对象得到同样的结果）。
pub fn hash64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn str_hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// 0.0–1.0 的确定性随机数。
fn roll(seed: i64, day: i64, salt: &str) -> f64 {
    (hash64((seed as u64) ^ hash64(day as u64) ^ str_hash(salt)) >> 11) as f64 / (1u64 << 53) as f64
}

pub fn season_of(day: i64, days_per_season: i64) -> &'static str {
    SEASONS[((day - 1).max(0) / days_per_season.max(1)).rem_euclid(4) as usize]
}

pub fn daypart(minutes: i64) -> &'static str {
    match minutes.rem_euclid(1440) / 60 {
        5..=6 => "清晨",
        7..=17 => "白天",
        18..=19 => "黄昏",
        _ => "夜晚",
    }
}

/// 季节天气表：(天气, 权重)。
fn weather_table(season: &str) -> &'static [(&'static str, u32)] {
    match season {
        "春" => &[("晴", 40), ("多云", 25), ("小雨", 25), ("暴雨", 5), ("雾", 5)],
        "夏" => &[("晴", 45), ("多云", 20), ("小雨", 15), ("暴雨", 15), ("雾", 5)],
        "秋" => &[("晴", 40), ("多云", 30), ("小雨", 20), ("雾", 10)],
        _ => &[("晴", 30), ("多云", 30), ("雪", 25), ("暴雪", 10), ("雾", 5)],
    }
}

pub fn weather_for(seed: i64, day: i64, season: &str) -> &'static str {
    let t = weather_table(season);
    let total: u32 = t.iter().map(|(_, w)| w).sum();
    let mut r = (roll(seed, day, "weather") * total as f64) as u32;
    for (w, p) in t {
        if r < *p {
            return w;
        }
        r -= p;
    }
    t[0].0
}

/// 一天分四段（0–6、6–12、12–18、18–24 点），每段有 30% 的概率变成天气表里相邻的天气（可复现）。
pub fn weather_at(seed: i64, day: i64, season: &str, minutes_of_day: i64) -> &'static str {
    let t = weather_table(season);
    let base = weather_for(seed, day, season);
    let mut idx = t.iter().position(|(w, _)| *w == base).unwrap_or(0) as i64;
    let seg = (minutes_of_day.rem_euclid(1440) / 360).clamp(0, 3);
    for s in 1..=seg {
        let r = roll(seed, day * 4 + s, "weather-shift");
        if r < 0.15 {
            idx -= 1;
        } else if r < 0.30 {
            idx += 1;
        }
        idx = idx.clamp(0, t.len() as i64 - 1);
    }
    t[idx as usize].0
}

pub fn is_bad_weather(w: &str) -> bool {
    matches!(w, "暴雨" | "暴雪")
}

/// 旅行耗时倍率：天气与夜晚。
pub fn travel_factor(weather: &str, daypart: &str) -> f64 {
    let w = match weather {
        "小雨" | "雾" => 1.25,
        "雪" => 1.5,
        "暴雨" => 1.6,
        "暴雪" => 2.0,
        _ => 1.0,
    };
    let d = if daypart == "夜晚" { 1.3 } else { 1.0 };
    w * d
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Environment {
    pub day: i64,
    pub time: String,
    pub daypart: String,
    pub season: String,
    pub weather: String,
    pub bad_weather: bool,
    pub travel_factor: f64,
}

/// 玩家口头说要去 / 走进的地点（实测：模型常常不填 travel_to，"举着火把走进潮洞"后位置没变）。
/// 返回 (地点 ID, 名称, 是否就在附近)：附近 = 和当前地点在同一个上级之下（同一座城里的另一处、当前地点里面的地方），直接到达；
/// 否则必须是道路能到的地点，由调用方按旅行处理。没有移动的说法、或没提到认识的地点时返回 None。
pub fn place_from_words(conn: &Connection, campaign_id: &str, text: &str) -> Result<Option<(String, String, bool)>> {
    const MOVE: &[&str] = &["走进", "进入", "钻进", "来到", "前往", "去往", "赶往", "回到", "走到", "跑到", "跑进", "进到", "上到", "下到", "爬上", "进去", "去"];
    if !MOVE.iter().any(|v| text.contains(v)) {
        return Ok(None);
    }
    let here = party_location(conn, campaign_id)?;
    let here_node = here.as_deref().and_then(|h| crate::nodes::get_node(conn, h).ok().flatten());
    let mut cands: Vec<(String, String, bool)> = vec![];
    if let Some(h) = &here_node {
        // 附近：当前地点里面的地方，以及同一个上级下面的其他地方。
        let base = h.parent_id.clone().unwrap_or_else(|| h.id.clone());
        let base_path = crate::nodes::get_node(conn, &base)?.map(|n| n.path).unwrap_or_default();
        let mut stmt = conn.prepare("SELECT id, title, path FROM nodes WHERE deleted_at IS NULL AND kind IN ('site','city','region','nation') AND id <> ?1 AND (path LIKE ?2 || '%' OR path LIKE ?3 || '%') LIMIT 200")?;
        let rows: Vec<(String, String, String)> = stmt.query_map(rusqlite::params![h.id, base_path, h.path], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.filter_map(|x| x.ok()).collect();
        for (id, title, _) in rows {
            cands.push((id, title, true));
        }
    }
    for r in reachable(conn, campaign_id, 20)? {
        if !cands.iter().any(|c| c.0 == r.id) {
            cands.push((r.id, r.title, false));
        }
    }
    // 名字最长的优先（"潮洞深处"优先于"潮洞"）。
    // 移动的说法要紧挨在地点名前面（中间最多隔 3 个字，例如"走进那座潮洞"），避免"我去问莫伦潮洞的事"也被当成去潮洞。
    let verb_before = |title: &str| -> bool {
        text.match_indices(title).any(|(i, _)| {
            let before: String = text[..i].chars().rev().take(5).collect::<Vec<_>>().into_iter().rev().collect();
            MOVE.iter().any(|v| before.ends_with(v) || (before.contains(v) && before.chars().count() - before.rfind(v).map(|b| before[..b].chars().count() + v.chars().count()).unwrap_or(0) <= 3))
        })
    };
    cands.retain(|c| c.1.chars().count() >= 2 && text.contains(c.1.as_str()) && Some(&c.0) != here.as_ref() && verb_before(&c.1));
    cands.sort_by_key(|c| std::cmp::Reverse(c.1.chars().count()));
    Ok(cands.into_iter().next())
}

/// 走到附近的地方：直接改位置，不按旅行计算路程（几分钟的事）。
pub fn move_nearby(world: &mut World, campaign_id: &str, to: &str, title: &str) -> Result<()> {
    campaign::apply_effects(world, campaign_id, &[Effect::SetVar { key: PARTY_LOCATION_VAR.into(), value: json!(to) }], &format!("来到「{title}」"))?;
    Ok(())
}

/// 某个地点此刻的环境（第 31 项）：各地区天气不同；室内不受天气影响；光照由时段、天气、室内外决定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct LocalEnv {
    pub location: Option<String>,
    pub weather: String,
    pub indoor: bool,
    /// 明亮 / 昏暗 / 漆黑
    pub light: String,
    /// 对检定的影响说明（给 GM 看）。
    pub effects: Vec<String>,
}

const INDOOR_WORDS: &[&str] = &["屋", "馆", "店", "室", "殿", "厅", "宅", "庙", "铺", "堂", "楼", "塔", "牢", "窟", "洞", "地下", "矿"];
const DARK_WORDS: &[&str] = &["洞", "窟", "地下", "矿", "牢", "墓"];

fn top_region(conn: &Connection, loc: &str) -> String {
    crate::nodes::get_node(conn, loc).ok().flatten().map(|n| n.path.split('/').find(|s| !s.is_empty()).unwrap_or(loc).to_string()).unwrap_or_else(|| loc.to_string())
}

pub fn local_environment(conn: &Connection, campaign_id: &str, location: Option<&str>, minutes: i64) -> Result<LocalEnv> {
    let env = environment_at(conn, campaign_id, minutes)?;
    let c = campaign::get_campaign(conn, campaign_id)?;
    let node = location.and_then(|l| crate::nodes::get_node(conn, l).ok().flatten());
    // 不同的大地区天气不同（同一地区内一致）；剧情指定了天气时以指定为准。
    let overridden = (CampaignView { conn, campaign_id }).var(WEATHER_OVERRIDE_VAR).is_some_and(|v| v.get("day").and_then(Value::as_i64) == Some(env.day));
    let weather = match (&node, overridden) {
        (Some(n), false) => {
            let region = top_region(conn, &n.id);
            let salt = region.bytes().fold(0i64, |h, b| h.wrapping_mul(31).wrapping_add(b as i64));
            weather_at(c.rng_seed ^ salt, env.day, &env.season, minutes).to_string()
        }
        _ => env.weather.clone(),
    };
    let title = node.as_ref().map(|n| n.title.clone()).unwrap_or_default();
    let indoor = node.as_ref().is_some_and(|n| n.payload.get("indoor").and_then(Value::as_bool).unwrap_or_else(|| INDOOR_WORDS.iter().any(|w| n.title.contains(w))));
    let dark_place = DARK_WORDS.iter().any(|w| title.contains(w)) || node.as_ref().is_some_and(|n| n.payload.get("dark").and_then(Value::as_bool) == Some(true));
    let night = matches!(env.daypart.as_str(), "深夜" | "夜晚" | "凌晨");
    let dusk = matches!(env.daypart.as_str(), "傍晚" | "黄昏" | "清晨");
    let gloomy = ["雾", "暴雨", "暴雪", "雷"].iter().any(|w| weather.contains(w));
    let light = if dark_place || (night && !indoor) {
        "漆黑"
    } else if night || dusk || (gloomy && !indoor) {
        "昏暗"
    } else {
        "明亮"
    };
    let mut effects = vec![];
    match light {
        "漆黑" => effects.push("漆黑：察觉、搜索、射击类检定 -4（有光源时 -1）".to_string()),
        "昏暗" => effects.push("昏暗：察觉、搜索、射击类检定 -2".to_string()),
        _ => {}
    }
    if !indoor && is_bad_weather(&weather) {
        effects.push(format!("{weather}：攀爬、运动、射击类检定 -2"));
    }
    Ok(LocalEnv { location: node.map(|n| n.title), weather: if indoor { format!("室内（外面{weather}）") } else { weather }, indoor, light: light.into(), effects })
}

/// 环境对一次检定的修正（程序计算，有原因）。`lit` 表示玩家带着光源。
pub fn env_check_modifier(env: &LocalEnv, skill: &str, lit: bool) -> (i64, String) {
    let sees = ["察觉", "侦查", "搜索", "观察", "感知", "射击", "弓", "远程", "投掷", "追踪"].iter().any(|w| skill.contains(w));
    let body = ["攀爬", "运动", "游泳", "跳跃", "射击", "弓", "远程", "驾驶", "骑"].iter().any(|w| skill.contains(w));
    let mut m = 0;
    let mut why = vec![];
    if sees {
        match (env.light.as_str(), lit) {
            ("漆黑", false) => {
                m -= 4;
                why.push("漆黑 -4");
            }
            ("漆黑", true) => {
                m -= 1;
                why.push("漆黑但有光源 -1");
            }
            ("昏暗", _) => {
                m -= 2;
                why.push("光线昏暗 -2");
            }
            _ => {}
        }
    }
    if body && !env.indoor && is_bad_weather(&env.weather) {
        m -= 2;
        why.push("恶劣天气 -2");
    }
    (m, why.join("，"))
}

pub fn days_per_season(conn: &Connection, campaign_id: &str) -> i64 {
    CampaignView { conn, campaign_id }.var(DAYS_PER_SEASON_VAR).and_then(|v| v.as_i64()).filter(|d| *d > 0).unwrap_or(30)
}

pub fn environment_at(conn: &Connection, campaign_id: &str, minutes: i64) -> Result<Environment> {
    let c = campaign::get_campaign(conn, campaign_id)?;
    let day = minutes.div_euclid(1440) + 1;
    let season = season_of(day, days_per_season(conn, campaign_id));
    // GM 或剧情可以指定某一天的天气（例如剧情需要的暴风雨）。
    let weather = match (CampaignView { conn, campaign_id }).var(WEATHER_OVERRIDE_VAR) {
        Some(v) if v.get("day").and_then(Value::as_i64) == Some(day) => v.get("weather").and_then(Value::as_str).unwrap_or("晴").to_string(),
        _ => weather_at(c.rng_seed, day, season, minutes).to_string(),
    };
    let dp = daypart(minutes);
    Ok(Environment {
        day,
        time: crate::schedule::format_clock(minutes),
        daypart: dp.into(),
        season: season.into(),
        bad_weather: is_bad_weather(&weather),
        travel_factor: travel_factor(&weather, dp),
        weather,
    })
}

pub fn environment(conn: &Connection, campaign_id: &str) -> Result<Environment> {
    environment_at(conn, campaign_id, crate::schedule::clock(conn, campaign_id))
}

fn env_effects(e: &Environment) -> Vec<Effect> {
    vec![
        Effect::SetVar { key: "env.day".into(), value: json!(e.day) },
        Effect::SetVar { key: "env.season".into(), value: json!(e.season) },
        Effect::SetVar { key: "env.daypart".into(), value: json!(e.daypart) },
        Effect::SetVar { key: "env.weather".into(), value: json!(e.weather) },
    ]
}

// ===================== 地图与旅行 =====================

pub const PARTY_LOCATION_VAR: &str = "party.location";
pub const PLACE_KINDS: &[&str] = &["region", "nation", "city", "site"];

/// 玩家此刻所在的地点：旅行到过的地点优先，否则是当前剧情节点的场景地点。
pub fn party_location(conn: &Connection, campaign_id: &str) -> Result<Option<String>> {
    if let Some(Value::String(s)) = (CampaignView { conn, campaign_id }).var(PARTY_LOCATION_VAR)
        && !s.is_empty()
    {
        return Ok(Some(s));
    }
    campaign::current_node_scene(conn, campaign_id)
}

/// 从关系说明或属性里读出路程（分钟）：attrs.minutes，或说明里的"2小时""90分钟""半天""1天"。
pub fn parse_minutes(label: &str, attrs: &Value) -> Option<i64> {
    if let Some(m) = attrs.get("minutes").and_then(Value::as_i64) {
        return Some(m.max(1));
    }
    let l = label.trim();
    if l.contains("半天") {
        return Some(360);
    }
    let num: String = l.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let n: f64 = num.parse().ok()?;
    let rest = &l[num.len()..];
    let m = if rest.starts_with('天') || rest.starts_with("日") {
        n * 1440.0
    } else if rest.starts_with("小时") || rest.starts_with('h') || rest.starts_with('时') {
        n * 60.0
    } else if rest.starts_with("分") || rest.starts_with('m') {
        n
    } else {
        return None;
    };
    Some((m as i64).max(1))
}

struct Edge {
    to: String,
    minutes: i64,
    kind: &'static str,
}

/// 路网：route_to（可单向，默认 2 小时）、borders（双向，默认 4 小时）、地点与所在的城市或上级地点（双向，15 分钟）。
/// 邻接表与地点名称。
type RoadGraph = (HashMap<String, Vec<Edge>>, HashMap<String, String>);

fn road_graph(conn: &Connection) -> Result<RoadGraph> {
    let mut adj: HashMap<String, Vec<Edge>> = HashMap::new();
    let mut titles = HashMap::new();
    let kinds = PLACE_KINDS.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(",");
    {
        let mut stmt = conn.prepare(&format!("SELECT id, parent_id, title, kind FROM nodes WHERE deleted_at IS NULL AND kind IN ({kinds})"))?;
        let rows: Vec<(String, Option<String>, String, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
        let ids: HashSet<&String> = rows.iter().map(|r| &r.0).collect();
        for (id, parent, title, kind) in &rows {
            titles.insert(id.clone(), title.clone());
            // 只有"地点"和它所在的城市（或上级地点）之间是附近（实测：区域、城市之间也连上时，
            // 同一区域的两座城市会经过区域节点只隔 30 分钟）。城市之间要用"通往""接壤"连接。
            if kind == "site"
                && let Some(p) = parent.as_ref().filter(|p| ids.contains(p))
            {
                adj.entry(id.clone()).or_default().push(Edge { to: p.clone(), minutes: 15, kind: "附近" });
                adj.entry(p.clone()).or_default().push(Edge { to: id.clone(), minutes: 15, kind: "附近" });
            }
        }
    }
    let mut stmt = conn.prepare(&format!(
        "SELECT r.src_id, r.dst_id, r.rel_type, r.directed, r.label, r.attrs FROM relations r
         JOIN nodes a ON a.id = r.src_id AND a.deleted_at IS NULL AND a.kind IN ({kinds})
         JOIN nodes b ON b.id = r.dst_id AND b.deleted_at IS NULL AND b.kind IN ({kinds})
         WHERE r.deleted_at IS NULL AND r.rel_type IN ('route_to', 'borders')"
    ))?;
    for r in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, bool>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?)))? {
        let (a, b, t, directed, label, attrs) = r?;
        let attrs: Value = serde_json::from_str(&attrs).unwrap_or(Value::Null);
        let (default, kind) = if t == "route_to" { (120, "道路") } else { (240, "边境") };
        let m = parse_minutes(&label, &attrs).unwrap_or(default);
        adj.entry(a.clone()).or_default().push(Edge { to: b.clone(), minutes: m, kind });
        if !(t == "route_to" && directed) {
            adj.entry(b).or_default().push(Edge { to: a, minutes: m, kind });
        }
    }
    Ok((adj, titles))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct TravelLeg {
    pub to: String,
    pub to_title: String,
    pub minutes: i64,
    /// 道路 / 边境 / 附近
    pub via: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct TravelPlan {
    pub from: Option<String>,
    pub to: String,
    pub to_title: String,
    pub legs: Vec<TravelLeg>,
    /// 路程本身的耗时。
    pub base_minutes: i64,
    /// 天气与昼夜带来的倍率（出发时的环境）。
    pub factor: f64,
    pub minutes: i64,
    pub weather: String,
    pub daypart: String,
}

/// 最短路径（Dijkstra）。没有出发地点时直接到达（耗时 0）。
pub fn plan_travel(conn: &Connection, campaign_id: &str, to: &str) -> Result<TravelPlan> {
    let (adj, titles) = road_graph(conn)?;
    let to_title = titles.get(to).cloned().ok_or_else(|| CoreError::Invalid("目的地不是地点（区域、国家、城市或地点）".into()))?;
    if let Some(n) = crate::nodes::get_node(conn, to)?
        && matches!(n.kind.as_str(), "region" | "nation")
    {
        return Err(CoreError::Invalid(format!("「{to_title}」是区域或国家，请选择其中具体的城市或地点")));
    }
    let env = environment(conn, campaign_id)?;
    let from = party_location(conn, campaign_id)?;
    let Some(src) = from.clone().filter(|f| titles.contains_key(f)) else {
        return Ok(TravelPlan { from, to: to.into(), to_title, legs: vec![], base_minutes: 0, factor: 1.0, minutes: 0, weather: env.weather, daypart: env.daypart });
    };
    let mut dist: HashMap<String, i64> = HashMap::new();
    let mut prev: HashMap<String, (String, i64, &'static str)> = HashMap::new();
    let mut heap = BinaryHeap::new();
    dist.insert(src.clone(), 0);
    heap.push(Reverse((0i64, src.clone())));
    while let Some(Reverse((d, u))) = heap.pop() {
        if u == to {
            break;
        }
        if dist.get(&u).is_some_and(|x| *x < d) {
            continue;
        }
        for e in adj.get(&u).into_iter().flatten() {
            let nd = d + e.minutes;
            if dist.get(&e.to).is_none_or(|x| nd < *x) {
                dist.insert(e.to.clone(), nd);
                prev.insert(e.to.clone(), (u.clone(), e.minutes, e.kind));
                heap.push(Reverse((nd, e.to.clone())));
            }
        }
    }
    let Some(base) = dist.get(to).copied() else {
        return Err(CoreError::Invalid(format!("没有通往「{to_title}」的路线（在世界树中用「通往」关系连接地点，说明里可以写耗时，例如“2小时”）")));
    };
    let mut legs = vec![];
    let mut cur = to.to_string();
    while let Some((p, m, kind)) = prev.get(&cur) {
        legs.push(TravelLeg { to: cur.clone(), to_title: titles.get(&cur).cloned().unwrap_or_default(), minutes: *m, via: kind.to_string() });
        if *p == src {
            break;
        }
        cur = p.clone();
    }
    legs.reverse();
    // 每一段路按走到那里时的天气与昼夜计算（以前只按出发时的环境算整条路）。
    let mut t = crate::schedule::clock(conn, campaign_id);
    let mut total = 0i64;
    for l in &legs {
        let f = environment_at(conn, campaign_id, t)?.travel_factor;
        let m = ((l.minutes as f64) * f).round() as i64;
        total += m;
        t += m;
    }
    let factor = if base > 0 { total as f64 / base as f64 } else { 1.0 };
    Ok(TravelPlan { from: Some(src), to: to.into(), to_title, legs, base_minutes: base, factor, minutes: total, weather: env.weather, daypart: env.daypart })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Reachable {
    pub id: String,
    pub title: String,
    pub kind: String,
    /// 路程耗时（未计天气），分钟。
    pub base_minutes: i64,
    /// unknown / heard / explored
    pub state: String,
}

/// 从当前位置出发能到达的地点，按路程由近到远（最多 limit 个，不含当前位置）。
pub fn reachable(conn: &Connection, campaign_id: &str, limit: usize) -> Result<Vec<Reachable>> {
    let Some(src) = party_location(conn, campaign_id)? else { return Ok(vec![]) };
    let (adj, titles) = road_graph(conn)?;
    if !titles.contains_key(&src) {
        return Ok(vec![]);
    }
    let mut dist: HashMap<String, i64> = HashMap::new();
    let mut done: Vec<(String, i64)> = vec![];
    let mut heap = BinaryHeap::new();
    dist.insert(src.clone(), 0);
    heap.push(Reverse((0i64, src.clone())));
    while let Some(Reverse((d, u))) = heap.pop() {
        if dist.get(&u).is_some_and(|x| *x < d) || done.iter().any(|(x, _)| *x == u) {
            continue;
        }
        if u != src {
            done.push((u.clone(), d));
            if done.len() >= limit {
                break;
            }
        }
        for e in adj.get(&u).into_iter().flatten() {
            let nd = d + e.minutes;
            if dist.get(&e.to).is_none_or(|x| nd < *x) {
                dist.insert(e.to.clone(), nd);
                heap.push(Reverse((nd, e.to.clone())));
            }
        }
    }
    let m = map(conn, campaign_id, true)?;
    let info: HashMap<&str, &MapPlace> = m.places.iter().map(|p| (p.id.as_str(), p)).collect();
    Ok(done
        .into_iter()
        .map(|(id, d)| Reachable {
            title: titles.get(&id).cloned().unwrap_or_default(),
            kind: info.get(id.as_str()).map(|p| p.kind.clone()).unwrap_or_default(),
            state: info.get(id.as_str()).map(|p| p.state.clone()).unwrap_or_else(|| "unknown".into()),
            base_minutes: d,
            id,
        })
        .collect())
}

/// "约 2 小时" / "约 1.5 天"
pub fn format_duration(minutes: i64) -> String {
    if minutes < 60 {
        format!("约 {minutes} 分钟")
    } else if minutes < 1440 {
        let h = minutes as f64 / 60.0;
        if (h - h.round()).abs() < 0.05 { format!("约 {} 小时", h.round()) } else { format!("约 {h:.1} 小时") }
    } else {
        format!("约 {:.1} 天", minutes as f64 / 1440.0)
    }
}

fn place_record(conn: &Connection, campaign_id: &str, id: &str) -> Option<Value> {
    CampaignView { conn, campaign_id }.record("place", id)
}

/// 旅行：按路线耗时推进时间（期间世界照常运转），沿途地点标为已探索，目的地的相邻地点标为"听说过"。
pub fn travel(world: &mut World, campaign_id: &str, to: &str, reason: &str) -> Result<(TravelPlan, Vec<String>)> {
    campaign::ensure_active(world.conn(), campaign_id)?;
    let plan = plan_travel(world.conn(), campaign_id, to)?;
    let turn = campaign::get_campaign(world.conn(), campaign_id)?.turn;
    let mut eff = vec![Effect::SetVar { key: PARTY_LOCATION_VAR.into(), value: json!(to) }];
    // 出发地、沿途、目的地，以及它们的上级地点（区域、国家），都记为已探索。
    let mut visited: Vec<String> = vec![];
    for id in plan.from.iter().cloned().chain(plan.legs.iter().map(|l| l.to.clone())).chain(std::iter::once(to.to_string())) {
        if let Some(n) = crate::nodes::get_node(world.conn(), &id)? {
            for a in n.path.split('/').filter(|s| !s.is_empty()) {
                if !visited.iter().any(|v| v == a)
                    && crate::nodes::get_node(world.conn(), a)?.is_some_and(|x| PLACE_KINDS.contains(&x.kind.as_str()))
                {
                    visited.push(a.to_string());
                }
            }
        }
    }
    for id in visited {
        let old = place_record(world.conn(), campaign_id, &id);
        if old.as_ref().and_then(|v| v["state"].as_str()) != Some("explored") {
            eff.push(Effect::UpsertRecord { kind: "place".into(), id, data: json!({ "state": "explored", "turn": turn }) });
        }
    }
    let (adj, _) = road_graph(world.conn())?;
    for e in adj.get(to).into_iter().flatten() {
        if place_record(world.conn(), campaign_id, &e.to).is_none() && !eff.iter().any(|x| matches!(x, Effect::UpsertRecord { id, .. } if *id == e.to)) {
            eff.push(Effect::UpsertRecord { kind: "place".into(), id: e.to.clone(), data: json!({ "state": "heard", "turn": turn }) });
        }
    }
    let why = format!("{reason}：前往「{}」，路上 {} 分钟", plan.to_title, plan.minutes);
    campaign::apply_effects(world, campaign_id, &eff, &why)?;
    // 每次推进最多 7 天：更长的路程分段推进。
    let mut notes = vec![];
    let mut left = plan.minutes;
    while left > 0 {
        let step = left.min(7 * 1440);
        notes.extend(crate::schedule::advance(world, campaign_id, step, &why)?);
        left -= step;
    }
    Ok((plan, notes))
}

/// 玩家听说了某些地点（例如别人提到）：未知 → 未探索。
pub fn hear_of_places(world: &mut World, campaign_id: &str, ids: &[String]) -> Result<Vec<String>> {
    let turn = campaign::get_campaign(world.conn(), campaign_id)?.turn;
    // 已经知道的地点（包括所在地点及其上级）不再记为"听说"。
    let known: HashSet<String> = map(world.conn(), campaign_id, true)?.places.into_iter().filter(|p| p.state != "unknown").map(|p| p.id).collect();
    let mut eff = vec![];
    let mut names = vec![];
    for id in ids {
        if known.contains(id) || eff.iter().any(|e| matches!(e, Effect::UpsertRecord { id: x, .. } if x == id)) {
            continue;
        }
        if let Some(n) = crate::nodes::get_node(world.conn(), id)?
            && PLACE_KINDS.contains(&n.kind.as_str())
        {
            eff.push(Effect::UpsertRecord { kind: "place".into(), id: id.clone(), data: json!({ "state": "heard", "turn": turn }) });
            names.push(n.title);
        }
    }
    if !eff.is_empty() {
        campaign::apply_effects(world, campaign_id, &eff, "听说了新的地点")?;
    }
    Ok(names)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct MapMarker {
    pub id: String,
    pub place: String,
    pub label: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct MapPlace {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub parent_id: Option<String>,
    /// unknown（未知）/ heard（未探索）/ explored（已探索）
    pub state: String,
    pub here: bool,
    pub markers: Vec<MapMarker>,
    /// 统治这个地点的势力（世界树的"统治"关系，战役中会因战争易主）。
    #[serde(default)]
    pub ruler: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct MapRoad {
    pub a: String,
    pub b: String,
    pub minutes: i64,
    /// 道路 / 边境 / 附近
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct WorldMap {
    pub places: Vec<MapPlace>,
    pub location: Option<String>,
    /// 可见地点之间的道路（画地图用）。
    #[serde(default)]
    pub roads: Vec<MapRoad>,
}

/// 地点的统治者：世界树的"统治"关系，加上战役中的变化（战争易主）。
pub fn rulers(conn: &Connection, campaign_id: &str) -> Result<HashMap<String, String>> {
    let mut base: Vec<(String, String)> = vec![];
    {
        let mut stmt = conn.prepare(
            "SELECT r.src_id, r.dst_id FROM relations r JOIN nodes a ON a.id = r.src_id AND a.kind = 'faction' AND a.deleted_at IS NULL
             WHERE r.deleted_at IS NULL AND r.rel_type = 'rules'",
        )?;
        for r in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            base.push(r?);
        }
    }
    let mut ov: HashMap<(String, String), bool> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT a, b, active FROM relation_overrides WHERE campaign_id = ?1 AND rel_type = 'rules'")?;
        for r in stmt.query_map([campaign_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?)))? {
            let (a, b, act) = r?;
            ov.insert((a, b), act);
        }
    }
    let mut out = HashMap::new();
    for (f, p) in &base {
        if ov.get(&(f.clone(), p.clone())).copied().unwrap_or(true) {
            out.insert(p.clone(), f.clone());
        }
    }
    for ((f, p), act) in &ov {
        if *act {
            out.insert(p.clone(), f.clone());
        }
    }
    Ok(out)
}

/// 地图。玩家视角不显示未知地点；当前地点及其上级视为已探索。
pub fn map(conn: &Connection, campaign_id: &str, gm_view: bool) -> Result<WorldMap> {
    let here = party_location(conn, campaign_id)?;
    let mut ancestors: HashSet<String> = HashSet::new();
    if let Some(h) = &here
        && let Some(n) = crate::nodes::get_node(conn, h)?
    {
        ancestors.extend(n.path.split('/').filter(|s| !s.is_empty()).map(str::to_string));
    }
    let mut markers: BTreeMap<String, Vec<MapMarker>> = BTreeMap::new();
    {
        let mut stmt = conn.prepare("SELECT id, data FROM campaign_records WHERE campaign_id = ?1 AND kind = 'marker'")?;
        for r in stmt.query_map([campaign_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (id, d) = r?;
            let v: Value = serde_json::from_str(&d).unwrap_or(Value::Null);
            let place = v["place"].as_str().unwrap_or_default().to_string();
            markers.entry(place.clone()).or_default().push(MapMarker { id, place, label: v["label"].as_str().unwrap_or_default().into(), note: v["note"].as_str().unwrap_or_default().into() });
        }
    }
    let mut states: HashMap<String, String> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT id, data FROM campaign_records WHERE campaign_id = ?1 AND kind = 'place'")?;
        for r in stmt.query_map([campaign_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (id, d) = r?;
            let v: Value = serde_json::from_str(&d).unwrap_or(Value::Null);
            states.insert(id, v["state"].as_str().unwrap_or("heard").into());
        }
    }
    let kinds = PLACE_KINDS.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(",");
    let ruler_of = rulers(conn, campaign_id)?;
    let fname: HashMap<String, String> = factions_raw(conn)?.into_iter().map(|n| (n.id, n.title)).collect();
    let mut stmt = conn.prepare(&format!("SELECT id, title, kind, parent_id FROM nodes WHERE deleted_at IS NULL AND kind IN ({kinds}) ORDER BY path LIMIT 5000"))?;
    let mut places = vec![];
    for r in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?)))? {
        let (id, title, kind, parent_id) = r?;
        let state = if ancestors.contains(&id) { "explored".to_string() } else { states.get(&id).cloned().unwrap_or_else(|| "unknown".into()) };
        if !gm_view && state == "unknown" {
            continue;
        }
        let ruler = ruler_of.get(&id).and_then(|f| fname.get(f)).cloned();
        places.push(MapPlace { here: here.as_deref() == Some(id.as_str()), markers: markers.remove(&id).unwrap_or_default(), ruler, id, title, kind, parent_id, state });
    }
    let visible: HashSet<&str> = places.iter().map(|p| p.id.as_str()).collect();
    let (adj, _) = road_graph(conn)?;
    let mut roads = vec![];
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for (a, es) in &adj {
        for e in es {
            if visible.contains(a.as_str()) && visible.contains(e.to.as_str()) {
                let k = if *a < e.to { (a.clone(), e.to.clone()) } else { (e.to.clone(), a.clone()) };
                if seen.insert(k.clone()) {
                    roads.push(MapRoad { a: k.0, b: k.1, minutes: e.minutes, kind: e.kind.into() });
                }
            }
        }
    }
    roads.sort_by(|x, y| (&x.a, &x.b).cmp(&(&y.a, &y.b)));
    Ok(WorldMap { places, location: here, roads })
}

pub fn add_marker(world: &mut World, campaign_id: &str, place: &str, label: &str, note: &str) -> Result<String> {
    if label.trim().is_empty() {
        return Err(CoreError::Invalid("标记名称不能为空".into()));
    }
    crate::nodes::get_node(world.conn(), place)?.ok_or_else(|| CoreError::NotFound("地点", place.into()))?;
    let id = crate::util::new_id();
    campaign::apply_effects(world, campaign_id, &[Effect::UpsertRecord { kind: "marker".into(), id: id.clone(), data: json!({ "place": place, "label": label.trim(), "note": note.trim() }) }], "地图：添加标记")?;
    Ok(id)
}

pub fn remove_marker(world: &mut World, campaign_id: &str, id: &str) -> Result<()> {
    campaign::apply_effects(world, campaign_id, &[Effect::DeleteRecord { kind: "marker".into(), id: id.into() }], "地图：删除标记")?;
    Ok(())
}

// ===================== 势力 =====================

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Power {
    pub wealth: i64,
    pub military: i64,
    pub influence: i64,
    pub stability: i64,
}

impl Power {
    fn clamp(mut self) -> Self {
        for v in [&mut self.wealth, &mut self.military, &mut self.influence, &mut self.stability] {
            *v = (*v).clamp(0, 100);
        }
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct FactionState {
    pub id: String,
    pub name: String,
    pub goals: Vec<String>,
    pub power: Power,
    /// 最近一次行动。
    pub last_action: String,
    pub at_war_with: Vec<String>,
    pub allies: Vec<String>,
    pub trade_with: Vec<String>,
}

fn factions_raw(conn: &Connection) -> Result<Vec<crate::model::Node>> {
    let mut stmt = conn.prepare("SELECT id FROM nodes WHERE deleted_at IS NULL AND kind = 'faction' ORDER BY path LIMIT 100")?;
    let ids: Vec<String> = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let mut out = vec![];
    for id in ids {
        if let Some(n) = crate::nodes::get_node(conn, &id)? {
            out.push(n);
        }
    }
    Ok(out)
}

fn initial_power(n: &crate::model::Node) -> Power {
    let p = n.payload.get("power");
    let g = |k: &str| p.and_then(|p| p.get(k)).and_then(Value::as_i64).unwrap_or(50);
    Power { wealth: g("wealth"), military: g("military"), influence: g("influence"), stability: g("stability") }.clamp()
}

/// 势力之间的战争、同盟、贸易关系（一次性读出，避免逐对查询）。语义与 CampaignView::relation 相同：战役中的覆盖优先。
struct FactionRels {
    base: HashSet<(String, String, String)>,
    overrides: HashMap<(String, String, String), bool>,
}

impl FactionRels {
    fn load(conn: &Connection, campaign_id: &str) -> Result<Self> {
        let mut base = HashSet::new();
        let mut stmt = conn.prepare(
            "SELECT r.src_id, r.dst_id, r.rel_type, r.directed FROM relations r
             JOIN nodes a ON a.id = r.src_id AND a.kind = 'faction' AND a.deleted_at IS NULL
             JOIN nodes b ON b.id = r.dst_id AND b.kind = 'faction' AND b.deleted_at IS NULL
             WHERE r.deleted_at IS NULL AND r.rel_type IN ('war', 'alliance', 'trade')",
        )?;
        for r in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, bool>(3)?)))? {
            let (a, b, t, directed) = r?;
            if !directed {
                base.insert((b.clone(), a.clone(), t.clone()));
            }
            base.insert((a, b, t));
        }
        let mut overrides = HashMap::new();
        let mut stmt = conn.prepare("SELECT a, b, rel_type, active FROM relation_overrides WHERE campaign_id = ?1 AND rel_type IN ('war', 'alliance', 'trade')")?;
        for r in stmt.query_map([campaign_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, bool>(3)?)))? {
            let (a, b, t, active) = r?;
            overrides.insert((a, b, t), active);
        }
        Ok(Self { base, overrides })
    }
    fn holds(&self, a: &str, b: &str, t: &str) -> bool {
        let k = (a.to_string(), b.to_string(), t.to_string());
        self.overrides.get(&k).copied().unwrap_or_else(|| self.base.contains(&k))
    }
}

pub fn factions(conn: &Connection, campaign_id: &str) -> Result<Vec<FactionState>> {
    let view = CampaignView { conn, campaign_id };
    let list = factions_raw(conn)?;
    let rels = FactionRels::load(conn, campaign_id)?;
    let mut out = vec![];
    for n in &list {
        let rec = view.record("faction", &n.id);
        let power = rec.as_ref().and_then(|r| serde_json::from_value::<Power>(r["power"].clone()).ok()).unwrap_or_else(|| initial_power(n));
        let rel = |t: &str| -> Vec<String> { list.iter().filter(|o| o.id != n.id && (rels.holds(&n.id, &o.id, t) || rels.holds(&o.id, &n.id, t))).map(|o| o.id.clone()).collect() };
        out.push(FactionState {
            id: n.id.clone(),
            name: n.title.clone(),
            goals: n.payload.get("goals").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            power,
            last_action: rec.as_ref().and_then(|r| r["last_action"].as_str()).unwrap_or_default().into(),
            at_war_with: rel("war"),
            allies: rel("alliance"),
            trade_with: rel("trade"),
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct WorldEvent {
    pub id: String,
    pub day: i64,
    /// battle / war / peace / alliance / coup / trigger / ...
    pub kind: String,
    pub text: String,
    pub factions: Vec<String>,
}

/// (类型, 文字, 相关势力)
type RawEvent = (String, String, Vec<String>);

fn goal_has(f: &FactionState, words: &[&str]) -> bool {
    f.goals.iter().any(|g| words.iter().any(|w| g.contains(w)))
}

/// 一天的势力行动。返回要执行的效果与值得记录的世界事件。
fn faction_day(seed: i64, day: i64, season: &str, weather: &str, fs: &[FactionState], names: &HashMap<String, String>) -> (Vec<Effect>, Vec<RawEvent>) {
    let mut power: BTreeMap<String, Power> = fs.iter().map(|f| (f.id.clone(), f.power)).collect();
    let mut action: BTreeMap<String, String> = BTreeMap::new();
    let mut eff = vec![];
    let mut events = vec![];
    let name = |id: &str| names.get(id).cloned().unwrap_or_default();
    let mut ended_wars: HashSet<(String, String)> = HashSet::new();
    let mut fought: HashSet<(String, String)> = HashSet::new();
    for f in fs {
        let r = roll(seed, day, &f.name);
        let p = power[&f.id];
        let mut np = p;
        // 财政吃紧时稳定下降。
        if p.wealth < 20 {
            np.stability -= 3;
        }
        let act: String = if p.stability < 20 && r < 0.3 {
            np.stability = 50;
            np.influence -= 10;
            events.push(("coup".into(), format!("{}爆发政变，新的掌权者稳住了局势", f.name), vec![f.id.clone()]));
            "政变".into()
        } else if let Some(ally) = fs.iter().find(|o| {
            o.id != f.id && !f.allies.contains(&o.id) && !f.at_war_with.contains(&o.id) && f.at_war_with.iter().any(|e| o.at_war_with.contains(e))
        }) && r < 0.2
        {
            eff.push(Effect::SetRelation { a: f.id.clone(), b: ally.id.clone(), rel_type: "alliance".into(), active: true });
            events.push(("alliance".into(), format!("{}与{}因共同的敌人结盟", f.name, ally.name), vec![f.id.clone(), ally.id.clone()]));
            "结盟".into()
        } else if let Some(enemy) = f.at_war_with.iter().find(|e| !ended_wars.contains(&pair(&f.id, e))) {
            if is_bad_weather(weather) || season == "冬" && r < 0.5 {
                "因天气休整".into()
            } else if !fought.insert(pair(&f.id, enemy)) {
                // 今天对方已经发起过战斗。
                "防守".into()
            } else {
                let ep = power.get(enemy).copied().unwrap_or_default();
                let mine = p.military as f64 + r * 25.0;
                let theirs = ep.military as f64 + roll(seed, day, &format!("{}|{}", name(enemy), f.name)) * 25.0;
                let (winner, loser) = if mine >= theirs { (f.id.clone(), enemy.clone()) } else { (enemy.clone(), f.id.clone()) };
                if let Some(w) = power.get_mut(&winner) {
                    w.influence += 4;
                    w.military -= 2;
                }
                if let Some(l) = power.get_mut(&loser) {
                    l.military -= 8;
                    l.stability -= 4;
                    l.wealth -= 3;
                }
                np = power[&f.id];
                // 相关势力按 [胜方, 败方] 记录，人物的情绪据此变化。
                events.push(("battle".into(), format!("{}与{}交战，{}占了上风", f.name, name(enemy), name(&winner)), vec![winner.clone(), loser.clone()]));
                // 一方军力崩溃时停战。
                if power[&loser].military < 15 {
                    eff.push(Effect::SetRelation { a: f.id.clone(), b: enemy.clone(), rel_type: "war".into(), active: false });
                    eff.push(Effect::SetRelation { a: enemy.clone(), b: f.id.clone(), rel_type: "war".into(), active: false });
                    ended_wars.insert(pair(&f.id, enemy));
                    events.push(("peace".into(), format!("{}无力再战，与{}停战", name(&loser), name(&winner)), vec![winner.clone(), loser.clone()]));
                }
                "作战".into()
            }
        } else if goal_has(f, &["扩张", "征服", "吞并", "统一"]) && p.military >= 60 && r < 0.05 {
            match fs.iter().filter(|o| o.id != f.id && !f.allies.contains(&o.id) && !f.at_war_with.contains(&o.id)).min_by_key(|o| power[&o.id].military) {
                Some(t) => {
                    eff.push(Effect::SetRelation { a: f.id.clone(), b: t.id.clone(), rel_type: "war".into(), active: true });
                    events.push(("war".into(), format!("{}向{}宣战", f.name, t.name), vec![f.id.clone(), t.id.clone()]));
                    "宣战".into()
                }
                None => "发展".into(),
            }
        } else if !f.trade_with.is_empty() {
            np.wealth += if season == "冬" { 1 } else { 3 };
            "贸易".into()
        } else if goal_has(f, &["军", "防御", "征服", "扩张"]) && p.wealth >= 30 {
            np.wealth -= 4;
            np.military += 3;
            "征兵".into()
        } else {
            np.wealth += 2;
            np.stability += 1;
            "发展".into()
        };
        if !matches!(act.as_str(), "作战" | "防守") {
            power.insert(f.id.clone(), np);
        }
        action.insert(f.id.clone(), act);
    }
    for f in fs {
        let p = power[&f.id].clamp();
        if p != f.power || action[&f.id] != f.last_action {
            eff.push(Effect::UpsertRecord { kind: "faction".into(), id: f.id.clone(), data: json!({ "power": p, "last_action": action[&f.id], "day": day }) });
        }
    }
    (eff, events)
}

fn pair(a: &str, b: &str) -> (String, String) {
    if a <= b { (a.into(), b.into()) } else { (b.into(), a.into()) }
}

fn record_events(day: i64, events: Vec<RawEvent>, eff: &mut Vec<Effect>, notes: &mut Vec<String>) {
    for (i, (kind, text, factions)) in events.into_iter().enumerate() {
        let id = format!("d{day:06}-{i:02}-{kind}");
        notes.push(format!("第 {day} 天：{text}"));
        eff.push(Effect::UpsertRecord { kind: "world_event".into(), id, data: json!({ "day": day, "kind": kind, "text": text, "factions": factions }) });
    }
}

// ===================== 事件驱动：剧本中的触发器 =====================

fn triggers(conn: &Connection, campaign_id: &str) -> Result<Vec<(String, String, Value)>> {
    let c = campaign::get_campaign(conn, campaign_id)?;
    let Some(sid) = c.script_id else { return Ok(vec![]) };
    Ok(crate::scripts::all_items(conn, &sid)?
        .into_iter()
        .filter(|i| matches!(i.kind, ScriptItemKind::Trigger | ScriptItemKind::KeyEvent) && i.payload.get("when").is_some_and(Value::is_object))
        .map(|i| (i.id, i.title, i.payload))
        .collect())
}

/// 检查条件满足的触发器（每个默认只触发一次）。
fn fire_triggers(world: &mut World, campaign_id: &str, day: i64, notes: &mut Vec<String>) -> Result<()> {
    for (id, title, p) in triggers(world.conn(), campaign_id)? {
        let once = p.get("once").and_then(Value::as_bool).unwrap_or(true);
        let view = CampaignView { conn: world.conn(), campaign_id };
        if once && view.record("trigger_fired", &id).is_some() {
            continue;
        }
        let Ok(cond) = serde_json::from_value::<Condition>(p["when"].clone()) else { continue };
        if !matches!(cond.preview(&view), Ok(Tri::True)) {
            continue;
        }
        let mut eff: Vec<Effect> = p.get("effects").and_then(|e| serde_json::from_value(e.clone()).ok()).unwrap_or_default();
        let text = p.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()).unwrap_or(&title).to_string();
        eff.push(Effect::UpsertRecord { kind: "trigger_fired".into(), id: id.clone(), data: json!({ "day": day }) });
        let mut evs = vec![];
        record_events(day, vec![("trigger".into(), text, vec![])], &mut evs, notes);
        if let Some(Effect::UpsertRecord { id: eid, .. }) = evs.first_mut() {
            *eid = format!("{eid}-{}", &id[id.len().saturating_sub(6)..]);
        }
        eff.extend(evs);
        campaign::apply_effects(world, campaign_id, &eff, &format!("事件触发：{title}"))?;
    }
    Ok(())
}

/// 时间推进时的世界模拟：更新环境变量；跨过的每一天，势力各行动一次；最后检查触发器。
/// 由 schedule::advance 调用。返回世界事件说明。
pub fn simulate(world: &mut World, campaign_id: &str, before: i64, after: i64) -> Result<Vec<String>> {
    let mut notes = vec![];
    let seed = campaign::get_campaign(world.conn(), campaign_id)?.rng_seed;
    let first_day = before.div_euclid(1440) + 1;
    let last_day = after.div_euclid(1440) + 1;
    // 跨过的每一天（新的一天开始时）推进势力。长时间跳跃最多模拟 365 天。
    for day in (first_day + 1)..=last_day.min(first_day + 365) {
        let env = environment_at(world.conn(), campaign_id, (day - 1) * 1440 + 8 * 60)?;
        let fs = factions(world.conn(), campaign_id)?;
        if fs.is_empty() {
            // 没有势力时：补货与 NPC 推演。
            let mut eff = crate::rpg::restock_effects(world.conn(), campaign_id)?;
            let (more_eff, more_events) = crate::npcsim::daily(world.conn(), campaign_id, day, seed)?;
            eff.extend(more_eff);
            eff.extend(crate::npcsim::rumor_day(world.conn(), campaign_id, day, seed, &more_events)?);
            record_events(day, more_events, &mut eff, &mut notes);
            if !eff.is_empty() {
                campaign::apply_effects(world, campaign_id, &eff, &format!("第 {day} 天：商人补货与人物动向"))?;
            }
            continue;
        }
        // 已经瓦解的势力不再参与推演。
        let fs: Vec<FactionState> = fs.into_iter().filter(|f| !crate::npcsim::is_collapsed(world.conn(), campaign_id, &f.id)).collect();
        let names: HashMap<String, String> = fs.iter().map(|f| (f.id.clone(), f.name.clone())).collect();
        let (mut eff, mut events) = faction_day(seed, day, &env.season, &env.weather, &fs, &names);
        // 停战时胜方占领败方的一个地点（优先与胜方领土接壤或相连的）。
        let conquests = conquests(world.conn(), campaign_id, &events, &names)?;
        for (winner, loser, place, title) in &conquests {
            eff.push(Effect::SetRelation { a: loser.clone(), b: place.clone(), rel_type: "rules".into(), active: false });
            eff.push(Effect::SetRelation { a: winner.clone(), b: place.clone(), rel_type: "rules".into(), active: true });
            events.push(("conquest".into(), format!("{}占领了{}（原属{}）", names.get(winner).cloned().unwrap_or_default(), title, names.get(loser).cloned().unwrap_or_default()), vec![winner.clone(), loser.clone(), place.clone()]));
        }
        eff.extend(npc_reactions_to_events(world.conn(), campaign_id, day, &events, &names)?);
        // NPC 与势力的其他变化：瓦解、倒戈、失踪、人物碰面（程序推演，见 npcsim.rs）。
        let (more_eff, more_events) = crate::npcsim::daily(world.conn(), campaign_id, day, seed)?;
        eff.extend(more_eff);
        events.extend(more_events);
        // 传闻：当天的公开事件开始流传，旧传闻在碰面的人之间传开（越传越走样）。
        eff.extend(crate::npcsim::rumor_day(world.conn(), campaign_id, day, seed, &events)?);
        // 商人每天补货、恢复本钱。
        eff.extend(crate::rpg::restock_effects(world.conn(), campaign_id)?);
        record_events(day, events, &mut eff, &mut notes);
        if !eff.is_empty() {
            campaign::apply_effects(world, campaign_id, &eff, &format!("世界模拟：第 {day} 天"))?;
        }
    }
    let env = environment_at(world.conn(), campaign_id, after)?;
    campaign::apply_effects(world, campaign_id, &env_effects(&env), "环境变化")?;
    fire_triggers(world, campaign_id, env.day, &mut notes)?;
    Ok(notes)
}

/// 停战事件 → 胜方占领败方的一个地点：(胜方, 败方, 地点, 地点名)。
fn conquests(conn: &Connection, campaign_id: &str, events: &[RawEvent], _names: &HashMap<String, String>) -> Result<Vec<(String, String, String, String)>> {
    let mut out = vec![];
    let held = rulers(conn, campaign_id)?;
    let (adj, titles) = road_graph(conn)?;
    for (kind, _, fs) in events {
        if kind != "peace" || fs.len() < 2 {
            continue;
        }
        let (w, l) = (&fs[0], &fs[1]);
        let mut cands: Vec<&String> = held.iter().filter(|(_, f)| *f == l).map(|(p, _)| p).collect();
        cands.sort();
        let near = |p: &String| adj.get(p).into_iter().flatten().any(|e| held.get(&e.to) == Some(w));
        let pick = cands.iter().find(|p| near(p)).or(cands.first()).map(|p| (*p).clone());
        if let Some(p) = pick {
            out.push((w.clone(), l.clone(), p.clone(), titles.get(&p).cloned().unwrap_or_else(|| crate::nodes::get_node(conn, &p).ok().flatten().map(|n| n.title).unwrap_or_default())));
        }
    }
    Ok(out)
}

/// 势力的成员（世界树中"隶属""领导""效忠"该势力的人物）。
fn members(conn: &Connection, faction: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT r.src_id FROM relations r JOIN nodes n ON n.id = r.src_id AND n.kind = 'character' AND n.deleted_at IS NULL
         WHERE r.deleted_at IS NULL AND r.dst_id = ?1 AND r.rel_type IN ('member_of', 'leads', 'serves')",
    )?;
    Ok(stmt.query_map([faction], |r| r.get(0))?.filter_map(|r| r.ok()).collect())
}

/// 世界事件对人物的影响（程序计算，有原因）：本势力被宣战、战败、政变、失去领土时，成员的恐惧与压力上升；
/// 交战双方成员之间已有的关系变差。
fn npc_reactions_to_events(conn: &Connection, campaign_id: &str, day: i64, events: &[RawEvent], names: &HashMap<String, String>) -> Result<Vec<Effect>> {
    let mut eff = vec![];
    let view = CampaignView { conn, campaign_id };
    let mood = |npc: &str, fear: i64, anger: i64, stress: i64, why: String| Effect::AdjustMood { npc: npc.into(), fear, anger, suspicion: 0, stress, loyalty: 0, reason: format!("第 {day} 天：{why}") };
    for (kind, text, fs) in events {
        match (kind.as_str(), fs.as_slice()) {
            ("war", [a, b, ..]) => {
                for m in members(conn, b)? {
                    eff.push(mood(&m, 5, 5, 10, format!("{}向本势力宣战", names.get(a).cloned().unwrap_or_default())));
                }
                // 交战双方成员之间已有的关系变差。
                let (ma, mb) = (members(conn, a)?, members(conn, b)?);
                for x in &ma {
                    for y in &mb {
                        for (from, to) in [(x, y), (y, x)] {
                            if view.bond(from, to).is_some() {
                                eff.push(Effect::AdjustBond { from: from.clone(), to: to.clone(), kind: String::new(), strength: 0, trust: -5, affection: 0, hostility: 5, reason: format!("第 {day} 天：两人所属的势力开战（{text}）"), mutual: false });
                            }
                        }
                    }
                }
            }
            ("battle", [_, loser, ..]) => {
                for m in members(conn, loser)? {
                    eff.push(mood(&m, 5, 0, 5, format!("本势力战败（{text}）")));
                }
            }
            ("coup", [f, ..]) => {
                for m in members(conn, f)? {
                    eff.push(mood(&m, 10, 0, 10, format!("本势力发生政变（{text}）")));
                }
            }
            ("conquest", [_, loser, place, ..]) => {
                for m in members(conn, loser)? {
                    eff.push(mood(&m, 5, 10, 10, format!("本势力失去了领土（{text}）")));
                }
                // 住在被占领地点的人物也会不安。
                for s in crate::schedule::all_npc_status(conn, campaign_id)? {
                    if s.location_id.as_deref().is_some_and(|l| l == place || crate::nodes::get_node(conn, l).ok().flatten().is_some_and(|n| n.path.contains(place.as_str()))) {
                        eff.push(mood(&s.npc_id, 10, 0, 10, format!("所在的地方易主（{text}）")));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(eff)
}

/// 人物死亡时，和他关系好的人悲伤愤怒，和他敌对的人松一口气（情绪变化有原因）。由战斗与状态修改调用。
pub fn reactions_to_death(conn: &Connection, campaign_id: &str, dead: &str) -> Result<Vec<Effect>> {
    let name = crate::nodes::get_node(conn, dead)?.map(|n| n.title).unwrap_or_default();
    let mut stmt = conn.prepare("SELECT from_id, affection, hostility FROM campaign_bonds WHERE campaign_id = ?1 AND to_id = ?2 AND from_id != 'party'")?;
    let rows: Vec<(String, i64, i64)> = stmt.query_map(params![campaign_id, dead], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.filter_map(|r| r.ok()).collect();
    let mut eff = vec![];
    for (who, aff, host) in rows {
        if who == dead {
            continue;
        }
        if aff >= 20 {
            eff.push(Effect::AdjustMood { npc: who, fear: 0, anger: 10, suspicion: 0, stress: 10, loyalty: 0, reason: format!("得知{name}死了") });
        } else if host >= 40 {
            eff.push(Effect::AdjustMood { npc: who, fear: 0, anger: -5, suspicion: 0, stress: -5, loyalty: 0, reason: format!("得知{name}死了（两人敌对）") });
        }
    }
    Ok(eff)
}

pub fn world_events(conn: &Connection, campaign_id: &str, limit: usize) -> Result<Vec<WorldEvent>> {
    let mut stmt = conn.prepare("SELECT id, data FROM campaign_records WHERE campaign_id = ?1 AND kind = 'world_event' ORDER BY id DESC LIMIT ?2")?;
    let rows = stmt.query_map(params![campaign_id, limit as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = vec![];
    for r in rows {
        let (id, d) = r?;
        let v: Value = serde_json::from_str(&d).unwrap_or(Value::Null);
        out.push(WorldEvent {
            id,
            day: v["day"].as_i64().unwrap_or(0),
            kind: v["kind"].as_str().unwrap_or_default().into(),
            text: v["text"].as_str().unwrap_or_default().into(),
            factions: v["factions"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_is_deterministic_and_seasonal() {
        assert_eq!(season_of(1, 30), "春");
        assert_eq!(season_of(31, 30), "夏");
        assert_eq!(season_of(121, 30), "春", "一年之后回到春天");
        assert_eq!(daypart(6 * 60), "清晨");
        assert_eq!(daypart(12 * 60), "白天");
        assert_eq!(daypart(23 * 60), "夜晚");
        assert_eq!(weather_for(42, 7, "冬"), weather_for(42, 7, "冬"));
        let winter: HashSet<&str> = (1..200).map(|d| weather_for(9, d, "冬")).collect();
        assert!(winter.contains("雪") && !winter.contains("暴雨"), "{winter:?}");
        let summer: HashSet<&str> = (1..200).map(|d| weather_for(9, d, "夏")).collect();
        assert!(!summer.contains("雪"));
        assert!(travel_factor("暴雪", "夜晚") > travel_factor("晴", "白天"));
    }

    #[test]
    fn minutes_from_labels() {
        assert_eq!(parse_minutes("2小时", &Value::Null), Some(120));
        assert_eq!(parse_minutes("90分钟", &Value::Null), Some(90));
        assert_eq!(parse_minutes("1.5天", &Value::Null), Some(2160));
        assert_eq!(parse_minutes("半天山路", &Value::Null), Some(360));
        assert_eq!(parse_minutes("山路", &json!({"minutes": 45})), Some(45));
        assert_eq!(parse_minutes("山路", &Value::Null), None);
    }
}

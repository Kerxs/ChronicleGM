//! 人物成长、物品与经济（全部数值由程序计算，AI 只负责描述）：
//! - 角色卡：属性、技能、等级、经验、生命值、伤势、状态。玩家是 `party`，NPC 用人物节点 ID。
//!   NPC 的初始数值来自人物资料的 `stats`；之后的变化记在战役里，不改动世界树。
//! - 规则设置：行动点、攻击消耗、暴击、经验曲线、货币、自定义行动等，按战役保存（`rules.config`）。
//! - 物品：价值、稀有度、装备栏位、伤害、护甲、加成、治疗、命中附带状态、制作配方，写在物品资料里。
//! - 背包沿用战役物品表；装备、来源记在战役记录里；货币是受保护变量 `money.<持有者>`（单位：铜币）。
//! - 商店：人物资料的 `shop`（库存、加价、收购比例、本钱）；买卖、制作、使用物品都由程序结算。
//! - 成长记录：经验、升级、加点、伤势、治愈都记一条（玩家和 NPC 都有）。
//!
//! 所有变化都通过效果写入事件日志，随时间线回溯恢复。

use std::collections::BTreeMap;

use chronicle_rules::{Effect, StateView};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::campaign::{self, CampaignView};
use crate::error::{CoreError, Result};
use crate::world::World;

pub const PARTY: &str = "party";
/// 原来固定的六项属性（没有属性方案时使用）。属性方案见 `RuleSet::attributes` / `attr_defs`。
pub const ATTRS: [(&str, &str); 6] = [("str", "力量"), ("dex", "敏捷"), ("con", "体质"), ("int", "智力"), ("wis", "感知"), ("cha", "魅力")];
pub use crate::profile::{AttrDef, CheckRules};
pub const RARITIES: [(&str, &str); 5] = [("common", "普通"), ("uncommon", "优秀"), ("rare", "稀有"), ("epic", "史诗"), ("legendary", "传说")];

fn normal_difficulty() -> String {
    "normal".into()
}

/// 难度档：名称、检定难度修正、敌人等级修正。
pub const DIFFICULTIES: [(&str, &str, i64, i64); 4] = [("easy", "简单", -2, -1), ("normal", "普通", 0, 0), ("hard", "困难", 2, 1), ("brutal", "残酷", 4, 2)];

fn difficulty_row(r: &RuleSet) -> (&'static str, &'static str, i64, i64) {
    DIFFICULTIES.iter().copied().find(|d| d.0 == r.difficulty).unwrap_or(DIFFICULTIES[1])
}

/// 按难度设置调整检定难度。返回 (调整后的难度, 说明)；普通难度不调整、说明为空。
/// 骰池模式的难度是"需要几个成功"，每 2 点算 1 个成功；百分骰（roll_under）的目标值按 5 倍反向调整（越难目标越低）。
pub fn difficulty_dc(r: &RuleSet, dc: i64) -> (i64, String) {
    let (_, name, off, _) = difficulty_row(r);
    if off == 0 {
        return (dc, String::new());
    }
    let (v, shown) = match r.check.mode.as_str() {
        "pool" => ((dc + off / 2).clamp(1, 6), off / 2),
        // 百分骰是"掷出 ≤ 目标值"：越难，目标值越低。
        "roll_under" => ((dc - off * 5).clamp(1, 100), -off * 5),
        _ => ((dc + off).max(1), off),
    };
    let what = if r.check.mode == "roll_under" { "目标值" } else { "难度" };
    (v, format!("难度设置「{name}」：{what} {shown:+}"))
}

/// 按难度设置调整临时敌人的等级（有角色卡的人物不调整）。
pub fn difficulty_level(r: &RuleSet, level: i64) -> i64 {
    (level + difficulty_row(r).3).clamp(1, 20)
}

/// 稀有度的显示名（按规则里的题材叫法）。
pub fn rarity_label(r: &RuleSet, key: &str) -> String {
    let i = RARITIES.iter().position(|(k, _)| *k == key);
    match i {
        Some(i) => r.rarity_labels.get(i).cloned().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| RARITIES[i].1.to_string()),
        None => key.to_string(),
    }
}
pub const SLOTS: [(&str, &str); 4] = [("weapon", "武器"), ("armor", "护甲"), ("shield", "盾牌"), ("accessory", "饰品")];
pub const RULES_VAR: &str = "rules.config";

// ===================== 规则设置 =====================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CustomAction {
    pub name: String,
    /// 消耗的行动点。
    pub ap: i64,
    /// attack（攻击）/ heal（治疗）/ status（施加状态）
    pub kind: String,
    /// 伤害或治疗的骰子，例如 "2d6"。
    #[serde(default)]
    pub dice: String,
    /// 加上哪个属性的修正（str/dex/...），可以为空。
    #[serde(default)]
    pub attr: String,
    /// 射程（格），0 表示自己。
    #[serde(default)]
    pub range: i64,
    #[serde(default)]
    pub status: Option<Status>,
}

/// 战役的规则设置（自定义规则）。没设置时用默认值。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(default)]
pub struct RuleSet {
    /// 每回合的行动点。
    pub ap_per_turn: i64,
    pub attack_cost: i64,
    pub move_cost: i64,
    /// 一次移动最多几格。
    pub speed: i64,
    /// 自然骰达到这个值为暴击（伤害骰掷两次）。
    pub crit_on: i64,
    /// 升到 n 级需要的累计经验 = xp_curve × n × (n − 1) / 2。
    pub xp_curve: i64,
    pub hp_per_level: i64,
    pub skill_points_per_level: i64,
    /// 每几级获得一个属性点。
    pub attr_point_every: i64,
    /// 一次奖励的经验上限（GM 提议时）。
    pub max_xp_award: i64,
    /// 一次奖励或扣除的金钱上限（铜币，GM 提议时）。
    pub max_money_award: i64,
    /// 每小时自然恢复的生命值。
    pub heal_per_hour: i64,
    /// 玩家生命值降到 0：downed（倒地）/ dead（死亡）。
    pub party_death: String,
    /// 货币名称（从大到小）与进位，例如 ["金币","银币","铜币"]、[100, 10, 1]。
    pub currency: Vec<String>,
    pub currency_rates: Vec<i64>,
    /// 五档稀有度的叫法（按剧本题材；空 = 普通 / 优秀 / 稀有 / 史诗 / 传说）。
    #[serde(default)]
    pub rarity_labels: Vec<String>,
    /// 难度：easy / normal / hard / brutal（玩家设置；GM 不能改，规则变量受保护）。
    #[serde(default = "normal_difficulty")]
    pub difficulty: String,
    /// 卖给商人时按价值的比例收购（商人没有设置时）。
    pub sell_rate: f64,
    /// 技能对应的属性（覆盖默认表），例如 {"锻造": "str"}。
    pub skill_attrs: BTreeMap<String, String>,
    pub custom_actions: Vec<CustomAction>,
    /// 属性方案（随剧本的玩法配置生成，可修改）；空 = 原来的六项属性。
    pub attributes: Vec<AttrDef>,
    /// 检定规则：主骰、成功等级、优势劣势、骰池。
    pub check: CheckRules,
}

impl Default for RuleSet {
    fn default() -> Self {
        Self {
            ap_per_turn: 3,
            attack_cost: 2,
            move_cost: 1,
            speed: 4,
            crit_on: 20,
            xp_curve: 100,
            hp_per_level: 6,
            skill_points_per_level: 2,
            attr_point_every: 4,
            max_xp_award: 200,
            max_money_award: 5000,
            heal_per_hour: 1,
            party_death: "downed".into(),
            currency: vec!["金币".into(), "银币".into(), "铜币".into()],
            rarity_labels: vec![],
            difficulty: normal_difficulty(),
            currency_rates: vec![100, 10, 1],
            sell_rate: 0.5,
            skill_attrs: BTreeMap::new(),
            custom_actions: vec![],
            attributes: vec![],
            check: CheckRules::default(),
        }
    }
}

impl RuleSet {
    pub fn validate(&self) -> Result<()> {
        let pos = |v: i64, what: &str, lo: i64, hi: i64| if (lo..=hi).contains(&v) { Ok(()) } else { Err(CoreError::Invalid(format!("{what} 应在 {lo}–{hi} 之间"))) };
        pos(self.ap_per_turn, "每回合行动点", 1, 20)?;
        pos(self.attack_cost, "攻击消耗", 0, 20)?;
        pos(self.move_cost, "移动消耗", 0, 20)?;
        pos(self.speed, "移动格数", 1, 30)?;
        pos(self.crit_on, "暴击点数", 2, 20)?;
        pos(self.xp_curve, "经验曲线", 1, 100_000)?;
        pos(self.hp_per_level, "每级生命", 0, 100)?;
        if self.currency.is_empty() || self.currency.len() != self.currency_rates.len() || self.currency_rates.last() != Some(&1) {
            return Err(CoreError::Invalid("货币名称与进位数量要一致，最小单位的进位是 1".into()));
        }
        if !matches!(self.party_death.as_str(), "downed" | "dead") {
            return Err(CoreError::Invalid("玩家倒下规则只能是 downed 或 dead".into()));
        }
        let keys = attr_keys(self);
        for (k, a) in &self.skill_attrs {
            if !keys.contains(a) {
                return Err(CoreError::Invalid(format!("技能「{k}」对应的属性 {a} 不存在")));
            }
        }
        self.check.validate()?;
        crate::profile::GameProfile { attributes: self.attributes.clone(), ..Default::default() }.validate()?;
        if !DIFFICULTIES.iter().any(|d| d.0 == self.difficulty) {
            return Err(CoreError::Invalid(format!("未知的难度：{}", self.difficulty)));
        }
        for c in &self.custom_actions {
            if c.name.trim().is_empty() || !matches!(c.kind.as_str(), "attack" | "heal" | "status") {
                return Err(CoreError::Invalid(format!("自定义行动「{}」的类型应为 attack / heal / status", c.name)));
            }
            if !c.dice.is_empty() {
                chronicle_rules::dice::DiceExpr::parse(&c.dice)?;
            }
            pos(c.ap, &format!("「{}」的行动点", c.name), 0, 20)?;
        }
        Ok(())
    }
}

/// 当前的属性方案（没有设置时是原来的六项）。
pub fn attr_defs(r: &RuleSet) -> Vec<AttrDef> {
    if r.attributes.is_empty() { crate::profile::default_attrs() } else { r.attributes.clone() }
}

pub fn attr_keys(r: &RuleSet) -> Vec<String> {
    attr_defs(r).into_iter().map(|a| a.key).collect()
}

pub fn attr_name(r: &RuleSet, key: &str) -> String {
    attr_defs(r).into_iter().find(|a| a.key == key).map(|a| a.name).unwrap_or_else(|| key.to_string())
}

/// 承担某种作用的属性（might / agility / vitality / mind / insight / presence）。
/// 方案里没有这个作用时：原来的六项里有同名 key 就用它，否则用第一个属性。
pub fn role_attr(r: &RuleSet, role: &str) -> String {
    let defs = attr_defs(r);
    if let Some(a) = defs.iter().find(|a| a.role == role) {
        return a.key.clone();
    }
    let legacy = match role {
        "might" => "str",
        "agility" => "dex",
        "vitality" => "con",
        "mind" => "int",
        "insight" => "wis",
        _ => "cha",
    };
    if defs.iter().any(|a| a.key == legacy) { legacy.to_string() } else { defs.first().map(|a| a.key.clone()).unwrap_or_else(|| legacy.to_string()) }
}

/// 把默认数值换到当前属性方案上：同 key 保留；按作用对应原来的六项；其余用基础值。
pub fn adapt_attrs(s: &mut Sheet, r: &RuleSet) {
    if r.attributes.is_empty() {
        return;
    }
    let base = 10 + s.level / 3;
    let old = std::mem::take(&mut s.attrs);
    for a in &r.attributes {
        let legacy = match a.role.as_str() {
            "might" => "str",
            "agility" => "dex",
            "vitality" => "con",
            "mind" => "int",
            "insight" => "wis",
            "presence" => "cha",
            _ => "",
        };
        let v = old.get(&a.key).or_else(|| old.get(legacy)).copied().unwrap_or(base);
        s.attrs.insert(a.key.clone(), v);
    }
}

pub fn rules(conn: &Connection, campaign_id: &str) -> RuleSet {
    CampaignView { conn, campaign_id }.var(RULES_VAR).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

pub fn set_rules(world: &mut World, campaign_id: &str, r: &RuleSet) -> Result<()> {
    r.validate()?;
    campaign::apply_effects(world, campaign_id, &[Effect::SetVar { key: RULES_VAR.into(), value: serde_json::to_value(r).unwrap_or(Value::Null) }], "修改规则设置")?;
    Ok(())
}

/// "1 金币 2 银币 3 铜币"
pub fn format_money(r: &RuleSet, copper: i64) -> String {
    if copper == 0 {
        return format!("0 {}", r.currency.last().cloned().unwrap_or_default());
    }
    let mut left = copper.abs();
    let mut parts = vec![];
    for (name, rate) in r.currency.iter().zip(&r.currency_rates) {
        let n = left / rate.max(&1);
        left %= rate.max(&1);
        if n > 0 {
            parts.push(format!("{n} {name}"));
        }
    }
    format!("{}{}", if copper < 0 { "-" } else { "" }, parts.join(" "))
}

// ===================== 角色卡 =====================

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Status {
    pub name: String,
    /// 战斗中剩余的轮数（None 表示不按轮计算）。
    #[serde(default)]
    pub rounds: Option<i64>,
    /// 到故事时间（分钟）为止（None 表示不按时间计算）。
    #[serde(default)]
    pub until: Option<i64>,
    /// 对检定与攻击的修正。
    #[serde(default)]
    pub modifier: i64,
    /// 对防御的修正。
    #[serde(default)]
    pub ac: i64,
    /// 每轮开始时受到的伤害（骰子），例如中毒 "1d4"。
    #[serde(default)]
    pub dot: String,
    /// 跳过回合（眩晕）。
    #[serde(default)]
    pub skip: bool,
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Wound {
    pub name: String,
    /// 检定惩罚。
    pub penalty: i64,
    /// 影响的属性（为空表示所有检定）。
    #[serde(default)]
    pub attr: String,
    /// 痊愈的故事时间（分钟）。
    pub heal_at: i64,
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Sheet {
    pub actor: String,
    pub name: String,
    pub level: i64,
    pub xp: i64,
    pub hp: i64,
    pub max_hp: i64,
    pub attrs: BTreeMap<String, i64>,
    pub skills: BTreeMap<String, i64>,
    /// 基础防御（未计装备与敏捷）。
    pub base_ac: i64,
    /// 基础攻击加值（未计属性）。
    pub attack: i64,
    /// 徒手或天生武器的伤害。
    pub unarmed: String,
    pub unspent_skill: i64,
    pub unspent_attr: i64,
    pub statuses: Vec<Status>,
    pub wounds: Vec<Wound>,
    /// 击败后给予的经验（NPC 与敌人）。
    #[serde(default)]
    pub xp_value: i64,
}

pub fn attr_mod(v: i64) -> i64 {
    (v - 10).div_euclid(2)
}

impl Sheet {
    pub fn attr(&self, a: &str) -> i64 {
        self.attrs.get(a).copied().unwrap_or(10)
    }
    pub fn modv(&self, a: &str) -> i64 {
        attr_mod(self.attr(a))
    }
    pub fn alive(&self) -> bool {
        self.hp > 0
    }
}

/// 某一等级的模板（没有设定数值的人物和临时敌人）。
pub fn template(actor: &str, name: &str, level: i64) -> Sheet {
    let level = level.clamp(1, 30);
    let a = 10 + level / 3;
    Sheet {
        actor: actor.into(),
        name: name.into(),
        level,
        xp: 0,
        hp: 6 + level * 5,
        max_hp: 6 + level * 5,
        attrs: ATTRS.iter().map(|(k, _)| (k.to_string(), a)).collect(),
        skills: BTreeMap::new(),
        base_ac: 10 + level / 2,
        attack: 1 + level / 2,
        unarmed: format!("1d6+{}", level / 3),
        unspent_skill: 0,
        unspent_attr: 0,
        statuses: vec![],
        wounds: vec![],
        xp_value: 25 + level * 25,
    }
}

/// 玩家默认角色卡：1 级，属性全 10。
pub fn default_party(name: &str) -> Sheet {
    let mut s = template(PARTY, name, 1);
    s.attrs = ATTRS.iter().map(|(k, _)| (k.to_string(), 10)).collect();
    s.hp = 12;
    s.max_hp = 12;
    s.base_ac = 10;
    s.attack = 1;
    s.unarmed = "1d3".into();
    s.xp_value = 0;
    s
}

/// 人物资料 `stats` 的格式：{level, hp, ac, attack, damage, attrs{}, skills{}, xp}，全部可选。
pub fn sheet_from_stats(actor: &str, name: &str, stats: Option<&Value>) -> Sheet {
    let g = |k: &str| stats.and_then(|s| s.get(k)).and_then(Value::as_i64);
    let mut s = template(actor, name, g("level").unwrap_or(1));
    if let Some(hp) = g("hp") {
        s.hp = hp.max(1);
        s.max_hp = hp.max(1);
    }
    if let Some(v) = g("ac") {
        s.base_ac = v;
    }
    if let Some(v) = g("attack") {
        s.attack = v;
    }
    if let Some(v) = g("xp") {
        s.xp_value = v.max(0);
    }
    if let Some(d) = stats.and_then(|s| s.get("damage")).and_then(Value::as_str) {
        s.unarmed = d.into();
    }
    if let Some(m) = stats.and_then(|s| s.get("attrs")).and_then(Value::as_object) {
        for (k, v) in m {
            if let Some(n) = v.as_i64() {
                s.attrs.insert(k.clone(), n);
            }
        }
    }
    if let Some(m) = stats.and_then(|s| s.get("skills")).and_then(Value::as_object) {
        for (k, v) in m {
            if let Some(n) = v.as_i64() {
                s.skills.insert(k.clone(), n);
            }
        }
    }
    s
}

/// 校验人物资料的 `stats`。
pub fn validate_stats(v: &Value) -> Result<()> {
    let m = v.as_object().ok_or_else(|| CoreError::Invalid("人物数值 stats 应为对象".into()))?;
    for (k, x) in m {
        match k.as_str() {
            "level" | "hp" | "ac" | "attack" | "xp" => {
                if !x.as_i64().is_some_and(|n| (0..=10_000).contains(&n)) {
                    return Err(CoreError::Invalid(format!("人物数值 {k} 应为 0–10000 的整数")));
                }
            }
            "damage" => {
                chronicle_rules::dice::DiceExpr::parse(x.as_str().unwrap_or("x")).map_err(|e| CoreError::Invalid(format!("伤害骰写法错误：{e}")))?;
            }
            "attrs" | "skills" => {
                let o = x.as_object().ok_or_else(|| CoreError::Invalid(format!("{k} 应为对象")))?;
                for (n, val) in o {
                    if k == "attrs" && !(1..=24).contains(&n.len()) {
                        return Err(CoreError::Invalid(format!("属性 key「{n}」长度不对")));
                    }
                    if !val.as_i64().is_some_and(|n| (-10..=40).contains(&n)) {
                        return Err(CoreError::Invalid(format!("{n} 的数值应为 -10–40 的整数")));
                    }
                }
            }
            other => return Err(CoreError::Invalid(format!("未知的人物数值项：{other}"))),
        }
    }
    Ok(())
}

/// 角色卡：战役中保存过的优先；否则玩家用默认卡，NPC 用人物资料的 stats（或 1 级模板）。
pub fn sheet(conn: &Connection, campaign_id: &str, actor: &str) -> Result<Sheet> {
    if let Some(v) = (CampaignView { conn, campaign_id }).record("sheet", actor)
        && let Ok(s) = serde_json::from_value::<Sheet>(v)
    {
        return Ok(s);
    }
    let r = rules(conn, campaign_id);
    if actor == PARTY {
        let mut s = default_party("玩家角色");
        adapt_attrs(&mut s, &r);
        return Ok(s);
    }
    let n = crate::nodes::get_node(conn, actor)?.ok_or_else(|| CoreError::NotFound("人物", actor.into()))?;
    let mut s = sheet_from_stats(actor, &n.title, n.payload.get("stats"));
    adapt_attrs(&mut s, &r);
    Ok(s)
}

pub fn has_sheet(conn: &Connection, campaign_id: &str, actor: &str) -> bool {
    (CampaignView { conn, campaign_id }).record("sheet", actor).is_some()
}

pub fn sheet_effect(s: &Sheet) -> Effect {
    Effect::UpsertRecord { kind: "sheet".into(), id: s.actor.clone(), data: serde_json::to_value(s).unwrap_or(Value::Null) }
}

/// 成长记录的效果。
fn growth_effect(conn: &Connection, campaign_id: &str, actor: &str, name: &str, kind: &str, text: &str, seq_offset: usize) -> Effect {
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM campaign_records WHERE campaign_id = ?1 AND kind = 'growth'", [campaign_id], |r| r.get(0))
        .unwrap_or(0);
    let turn = campaign::get_campaign(conn, campaign_id).map(|c| c.turn).unwrap_or(0);
    Effect::UpsertRecord {
        kind: "growth".into(),
        id: format!("g{:07}", n as usize + seq_offset),
        data: json!({ "actor": actor, "name": name, "kind": kind, "text": text, "turn": turn, "minutes": crate::schedule::clock(conn, campaign_id) }),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct GrowthEntry {
    pub id: String,
    pub actor: String,
    pub name: String,
    /// xp / level / skill / attr / wound / heal / status / edit
    pub kind: String,
    pub text: String,
    pub turn: i64,
    pub time: String,
}

pub fn growth_log(conn: &Connection, campaign_id: &str, actor: Option<&str>, limit: usize) -> Result<Vec<GrowthEntry>> {
    let mut stmt = conn.prepare("SELECT id, data FROM campaign_records WHERE campaign_id = ?1 AND kind = 'growth' ORDER BY id DESC")?;
    let mut out = vec![];
    for r in stmt.query_map([campaign_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (id, d) = r?;
        let v: Value = serde_json::from_str(&d).unwrap_or(Value::Null);
        let a = v["actor"].as_str().unwrap_or_default();
        if actor.is_some_and(|x| x != a) {
            continue;
        }
        out.push(GrowthEntry {
            id,
            actor: a.into(),
            name: v["name"].as_str().unwrap_or_default().into(),
            kind: v["kind"].as_str().unwrap_or_default().into(),
            text: v["text"].as_str().unwrap_or_default().into(),
            turn: v["turn"].as_i64().unwrap_or(0),
            time: crate::schedule::format_clock(v["minutes"].as_i64().unwrap_or(0)),
        });
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

/// 保存角色卡（例如玩家在界面上修改名字、属性），同时记一条成长记录。
pub fn save_sheet(world: &mut World, campaign_id: &str, s: &Sheet, note: &str) -> Result<()> {
    if s.name.trim().is_empty() {
        return Err(CoreError::Invalid("角色名不能为空".into()));
    }
    if s.max_hp < 1 || s.hp > s.max_hp || s.level < 1 {
        return Err(CoreError::Invalid("生命值或等级不合理".into()));
    }
    let keys = attr_keys(&rules(world.conn(), campaign_id));
    for (k, v) in &s.attrs {
        if !keys.contains(k) || !(1..=40).contains(v) {
            return Err(CoreError::Invalid(format!("属性 {k} 不在属性方案里，或数值不在 1–40")));
        }
    }
    let g = growth_effect(world.conn(), campaign_id, &s.actor, &s.name, "edit", note, 0);
    campaign::apply_effects(world, campaign_id, &[sheet_effect(s), g], note)?;
    Ok(())
}

/// 技能对应的属性（默认表 + 规则设置里的覆盖）。
pub fn skill_attr(r: &RuleSet, skill: &str) -> String {
    if let Some(a) = r.skill_attrs.get(skill)
        && attr_keys(r).contains(a)
    {
        return a.clone();
    }
    role_attr(r, legacy_skill_role(skill))
}

/// 默认技能表：技能名里的关键词 → 属性作用。
fn legacy_skill_role(skill: &str) -> &'static str {
    let table: [(&[&str], &str); 6] = [
        (&["运动", "攀爬", "游泳", "力量", "搬", "破门", "锻造", "格斗"], "might"),
        (&["潜行", "杂技", "巧手", "开锁", "射击", "闪避", "敏捷", "骑", "偷"], "agility"),
        (&["坚韧", "忍耐", "体质", "耐力"], "vitality"),
        (&["调查", "学识", "奥秘", "历史", "智力", "制作", "炼金", "工艺", "宗教", "自然"], "mind"),
        (&["察觉", "洞察", "医疗", "求生", "感知", "聆听", "察言观色", "追踪"], "insight"),
        (&["游说", "欺瞒", "威吓", "表演", "交涉", "说服", "魅力", "谈判", "讨价还价"], "presence"),
    ];
    table.iter().find(|(ws, _)| ws.iter().any(|w| skill.contains(w))).map(|(_, a)| *a).unwrap_or("insight")
}

/// 检定修正 = 旧的技能变量 + 角色卡技能 + 对应属性修正 + 装备加成 + 状态修正 − 伤势惩罚。
pub fn check_modifier(conn: &Connection, campaign_id: &str, actor: &str, skill: &str) -> i64 {
    let view = CampaignView { conn, campaign_id };
    let legacy = view.var(&format!("skill.{actor}.{skill}")).and_then(|v| v.as_i64()).unwrap_or(0);
    let Ok(s) = sheet(conn, campaign_id, actor) else { return legacy };
    let r = rules(conn, campaign_id);
    let attr = skill_attr(&r, skill);
    let equip_bonus: i64 = equipped_items(conn, campaign_id, actor).iter().map(|i| i.bonuses.get(skill).copied().unwrap_or(0) + i.bonuses.get(&attr).copied().unwrap_or(0)).sum();
    let status: i64 = s.statuses.iter().map(|x| x.modifier).sum();
    let wounds: i64 = s.wounds.iter().filter(|w| w.attr.is_empty() || w.attr == attr).map(|w| w.penalty).sum();
    legacy + s.skills.get(skill).copied().unwrap_or(0) + s.modv(&attr) + equip_bonus + status - wounds
}

// ===================== 成长 =====================

pub fn xp_for_level(r: &RuleSet, level: i64) -> i64 {
    r.xp_curve * level * (level - 1) / 2
}

/// 获得经验（由程序计算升级）。返回说明。
pub fn award_xp(world: &mut World, campaign_id: &str, actor: &str, amount: i64, reason: &str) -> Result<Vec<String>> {
    let r = rules(world.conn(), campaign_id);
    let amount = amount.clamp(0, r.max_xp_award.max(1) * 50);
    if amount == 0 {
        return Ok(vec![]);
    }
    let mut s = sheet(world.conn(), campaign_id, actor)?;
    let mut notes = vec![format!("{}获得 {amount} 点经验（{reason}）", s.name)];
    let mut eff = vec![];
    s.xp += amount;
    eff.push(growth_effect(world.conn(), campaign_id, actor, &s.name, "xp", &notes[0], 0));
    while s.level < 30 && s.xp >= xp_for_level(&r, s.level + 1) {
        s.level += 1;
        let hp = (r.hp_per_level + s.modv(&role_attr(&r, "vitality"))).max(1);
        s.max_hp += hp;
        s.hp += hp;
        s.unspent_skill += r.skill_points_per_level;
        if r.attr_point_every > 0 && s.level % r.attr_point_every == 0 {
            s.unspent_attr += 1;
        }
        if s.level % 2 == 0 {
            s.attack += 1;
        }
        let t = format!("{}升到 {} 级：生命上限 +{hp}，技能点 +{}{}", s.name, s.level, r.skill_points_per_level, if r.attr_point_every > 0 && s.level % r.attr_point_every == 0 { "，属性点 +1" } else { "" });
        eff.push(growth_effect(world.conn(), campaign_id, actor, &s.name, "level", &t, eff.len()));
        notes.push(t);
    }
    eff.insert(0, sheet_effect(&s));
    campaign::apply_effects(world, campaign_id, &eff, &format!("经验：{reason}"))?;
    Ok(notes)
}

/// 使用技能点（+1 级技能）。
pub fn spend_skill(world: &mut World, campaign_id: &str, actor: &str, skill: &str) -> Result<()> {
    let skill = skill.trim();
    if skill.is_empty() {
        return Err(CoreError::Invalid("技能名不能为空".into()));
    }
    let mut s = sheet(world.conn(), campaign_id, actor)?;
    if s.unspent_skill < 1 {
        return Err(CoreError::Invalid("没有可用的技能点".into()));
    }
    s.unspent_skill -= 1;
    let v = s.skills.entry(skill.to_string()).or_insert(0);
    *v += 1;
    let t = format!("{}的「{skill}」提升到 {}", s.name, v);
    let g = growth_effect(world.conn(), campaign_id, actor, &s.name, "skill", &t, 0);
    campaign::apply_effects(world, campaign_id, &[sheet_effect(&s), g], "使用技能点")?;
    Ok(())
}

pub fn spend_attr(world: &mut World, campaign_id: &str, actor: &str, attr: &str) -> Result<()> {
    let r = rules(world.conn(), campaign_id);
    if !attr_keys(&r).iter().any(|k| k == attr) {
        return Err(CoreError::Invalid(format!("未知的属性：{attr}")));
    }
    let label = attr_name(&r, attr);
    let vitality = role_attr(&r, "vitality");
    let mut s = sheet(world.conn(), campaign_id, actor)?;
    if s.unspent_attr < 1 {
        return Err(CoreError::Invalid("没有可用的属性点".into()));
    }
    s.unspent_attr -= 1;
    let v = s.attrs.entry(attr.to_string()).or_insert(10);
    *v += 1;
    let t = format!("{}的{label}提升到 {}", s.name, v);
    if attr == vitality && attr_mod(*v) > attr_mod(*v - 1) {
        s.max_hp += s.level;
        s.hp += s.level;
    }
    let g = growth_effect(world.conn(), campaign_id, actor, &s.name, "attr", &t, 0);
    campaign::apply_effects(world, campaign_id, &[sheet_effect(&s), g], "使用属性点")?;
    Ok(())
}

/// 受伤、加状态等对角色卡的修改，附带成长记录。
pub fn update_sheet(world: &mut World, campaign_id: &str, s: &Sheet, kind: &str, text: &str) -> Result<()> {
    let g = growth_effect(world.conn(), campaign_id, &s.actor, &s.name, kind, text, 0);
    campaign::apply_effects(world, campaign_id, &[sheet_effect(s), g], text)?;
    Ok(())
}

/// 故事时间流逝：自然恢复生命，到期的状态与伤势消失（只处理保存过的角色卡；战斗中不恢复）。
pub fn time_passed(world: &mut World, campaign_id: &str, before: i64, after: i64) -> Result<Vec<String>> {
    if crate::combat::active(world.conn(), campaign_id).is_some() {
        return Ok(vec![]);
    }
    let r = rules(world.conn(), campaign_id);
    let hours = after / 60 - before / 60;
    let mut stmt = world.conn().prepare("SELECT data FROM campaign_records WHERE campaign_id = ?1 AND kind = 'sheet'")?;
    let sheets: Vec<Sheet> = stmt.query_map([campaign_id], |r| r.get::<_, String>(0))?.filter_map(|d| d.ok().and_then(|d| serde_json::from_str(&d).ok())).collect();
    drop(stmt);
    let mut eff = vec![];
    let mut notes = vec![];
    for mut s in sheets {
        let old = s.clone();
        let dead = s.hp <= 0 && is_dead(world.conn(), campaign_id, &s.actor);
        if !dead && hours > 0 && s.hp < s.max_hp {
            // 生命值为 0（倒地）时先恢复到 1。
            s.hp = (s.hp.max(0) + r.heal_per_hour.max(0) * hours).min(s.max_hp);
        }
        s.statuses.retain(|x| x.until.is_none_or(|u| u > after) && x.rounds.is_none());
        let healed: Vec<String> = s.wounds.iter().filter(|w| w.heal_at <= after).map(|w| w.name.clone()).collect();
        s.wounds.retain(|w| w.heal_at > after);
        if s != old {
            eff.push(sheet_effect(&s));
            for w in healed {
                let t = format!("{}的伤势「{w}」痊愈了", s.name);
                eff.push(growth_effect(world.conn(), campaign_id, &s.actor, &s.name, "heal", &t, eff.len()));
                notes.push(t);
            }
        }
    }
    if !eff.is_empty() {
        campaign::apply_effects(world, campaign_id, &eff, "时间流逝：恢复与痊愈")?;
    }
    Ok(notes)
}

fn is_dead(conn: &Connection, campaign_id: &str, actor: &str) -> bool {
    campaign::entity_overrides(conn, campaign_id, actor).ok().and_then(|m| m.get("life_status").and_then(|v| v.as_str().map(str::to_string))).as_deref() == Some("dead")
}

// ===================== 物品 =====================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct RecipeInput {
    pub item: String,
    pub qty: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Recipe {
    pub inputs: Vec<RecipeInput>,
    #[serde(default)]
    pub skill: String,
    #[serde(default)]
    pub dc: i64,
    /// 制作耗时（分钟）。
    #[serde(default)]
    pub minutes: i64,
    /// 一次做出几个。
    #[serde(default = "one")]
    pub qty: i64,
}

fn one() -> i64 {
    1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ItemInfo {
    pub id: String,
    pub title: String,
    pub description: String,
    /// common / uncommon / rare / epic / legendary
    pub rarity: String,
    /// weapon / armor / shield / accessory，空表示不能装备
    pub slot: String,
    /// 价值（铜币）。
    pub value: i64,
    pub damage: String,
    /// 射程（格）：近战 1。
    pub range: i64,
    pub armor: i64,
    /// 加成：技能名或属性（str/dex/...）→ 数值。
    pub bonuses: BTreeMap<String, i64>,
    /// 使用后恢复的生命（骰子），有值表示消耗品。
    pub heal: String,
    /// 使用后解除的状态名。
    pub cures: Vec<String>,
    /// 命中时附带的状态。
    pub on_hit: Option<Status>,
    pub recipe: Option<Recipe>,
    /// 来自世界树（world）还是剧本（script）。
    pub origin: String,
}

const ITEM_KEYS: [&str; 10] = ["value", "price", "rarity", "slot", "damage", "range", "armor", "bonuses", "heal", "recipe"];

/// 校验物品资料里的游戏数值（世界树物品和剧本物品都适用）。
pub fn validate_item_payload(v: &Value) -> Result<()> {
    let _ = ITEM_KEYS;
    for k in ["value", "price", "armor", "range"] {
        if let Some(x) = v.get(k).filter(|x| !x.is_null())
            && !x.as_i64().is_some_and(|n| (0..=100_000_000).contains(&n))
        {
            return Err(CoreError::Invalid(format!("物品的 {k} 应为非负整数")));
        }
    }
    if let Some(r) = v.get("rarity").and_then(Value::as_str)
        && !r.is_empty()
        && !RARITIES.iter().any(|(k, _)| *k == r)
    {
        return Err(CoreError::Invalid(format!("未知的稀有度：{r}")));
    }
    if let Some(s) = v.get("slot").and_then(Value::as_str)
        && !s.is_empty()
        && !SLOTS.iter().any(|(k, _)| *k == s)
    {
        return Err(CoreError::Invalid(format!("未知的装备栏位：{s}")));
    }
    for k in ["damage", "heal"] {
        if let Some(d) = v.get(k).and_then(Value::as_str).filter(|d| !d.is_empty()) {
            chronicle_rules::dice::DiceExpr::parse(d).map_err(|e| CoreError::Invalid(format!("物品的 {k} 骰子写法错误：{e}")))?;
        }
    }
    if let Some(b) = v.get("bonuses").filter(|x| !x.is_null()) {
        let o = b.as_object().ok_or_else(|| CoreError::Invalid("物品加成应为对象".into()))?;
        if o.values().any(|x| !x.as_i64().is_some_and(|n| (-20..=20).contains(&n))) {
            return Err(CoreError::Invalid("物品加成应为 -20–20 的整数".into()));
        }
    }
    if let Some(s) = v.get("on_hit").filter(|x| !x.is_null()) {
        serde_json::from_value::<Status>(s.clone()).map_err(|e| CoreError::Invalid(format!("命中状态格式错误：{e}")))?;
    }
    if let Some(r) = v.get("recipe").filter(|x| !x.is_null()) {
        let r: Recipe = serde_json::from_value(r.clone()).map_err(|e| CoreError::Invalid(format!("制作配方格式错误：{e}")))?;
        if r.inputs.is_empty() || r.inputs.iter().any(|i| i.qty < 1 || i.item.is_empty()) {
            return Err(CoreError::Invalid("制作配方至少需要一种材料，数量至少为 1".into()));
        }
    }
    Ok(())
}

fn item_from(id: &str, title: &str, description: &str, p: &Value, origin: &str) -> ItemInfo {
    let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let i = |k: &str| p.get(k).and_then(Value::as_i64);
    let slot = s("slot");
    ItemInfo {
        id: id.into(),
        title: title.into(),
        description: description.into(),
        rarity: Some(s("rarity")).filter(|r| !r.is_empty()).unwrap_or_else(|| "common".into()),
        value: i("value").or_else(|| i("price")).unwrap_or(0),
        damage: s("damage"),
        range: i("range").unwrap_or(if slot == "weapon" { 1 } else { 0 }),
        armor: i("armor").unwrap_or(0),
        bonuses: p.get("bonuses").and_then(|b| serde_json::from_value(b.clone()).ok()).unwrap_or_default(),
        heal: s("heal"),
        cures: p.get("cures").and_then(|b| serde_json::from_value(b.clone()).ok()).unwrap_or_default(),
        on_hit: p.get("on_hit").and_then(|b| serde_json::from_value(b.clone()).ok()),
        recipe: p.get("recipe").and_then(|b| serde_json::from_value(b.clone()).ok()),
        slot,
        origin: origin.into(),
    }
}

/// 物品资料：剧本物品或世界树物品。
pub fn item_info(conn: &Connection, id: &str) -> Result<Option<ItemInfo>> {
    if let Some(it) = crate::scripts::get_item(conn, id)?
        && it.kind == crate::model::ScriptItemKind::Item
    {
        return Ok(Some(item_from(&it.id, &it.title, &it.player_text, &it.payload, "script")));
    }
    if let Some(n) = crate::nodes::get_node(conn, id)?
        && n.kind == crate::model::NodeKind::Item
    {
        return Ok(Some(item_from(&n.id, &n.title, if n.summary.is_empty() { &n.player_text } else { &n.summary }, &n.payload, "world")));
    }
    Ok(None)
}

/// 表示"拿出来用"的动词：玩家的话里有这些词，提到的物品才需要在背包里。
pub const USE_VERBS: &[&str] = &["用", "拿出", "掏出", "取出", "拔出", "抽出", "举起", "挥", "装上", "装备", "戴上", "穿上", "交给", "递给", "给他", "给她", "出示", "亮出", "喝下", "喝了", "吃下", "吃了", "点燃", "扔出", "射出", "砍", "刺", "插进", "打开", "解开", "念", "吹"];

/// 玩家的话里提到、但背包里没有的物品（按世界树物品与本剧本物品的名称匹配，名称至少两个字）。
/// 程序把"玩家没有这些物品"作为事实交给 GM，GM 不能让玩家凭空拿出来用。
pub fn missing_items(conn: &Connection, campaign_id: &str, script_id: Option<&str>, text: &str) -> Result<Vec<String>> {
    // 只有玩家要"用"它的时候才算（实测：玩家只是查看地上碎掉的灯芯石，也被当成"拿出来用"）。
    if !USE_VERBS.iter().any(|v| text.contains(v)) {
        return Ok(vec![]);
    }
    let mut named: Vec<(String, String)> = vec![];
    let mut stmt = conn.prepare("SELECT id, title FROM nodes WHERE kind = 'item' AND deleted_at IS NULL")?;
    named.extend(stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.filter_map(|x| x.ok()));
    if let Some(sid) = script_id {
        let mut stmt = conn.prepare("SELECT id, title FROM script_items WHERE kind = 'item' AND script_id = ?1 AND deleted_at IS NULL")?;
        named.extend(stmt.query_map([sid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.filter_map(|x| x.ok()));
    }
    let view = CampaignView { conn, campaign_id };
    let held: Vec<String> = inventory(conn, campaign_id, PARTY)?.into_iter().map(|i| i.item.title).collect();
    let mut out: Vec<String> = vec![];
    for (id, title) in named {
        let t = title.trim();
        if t.chars().count() < 2 || !text.contains(t) || out.iter().any(|o| o == t) {
            continue;
        }
        // 同名的另一件物品在背包里也算有。
        if view.item_qty(PARTY, &id) > 0 || held.iter().any(|h| h == t) {
            continue;
        }
        out.push(t.to_string());
    }
    Ok(out)
}

fn require_item(conn: &Connection, id: &str) -> Result<ItemInfo> {
    item_info(conn, id)?.ok_or_else(|| CoreError::NotFound("物品", id.into()))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct InvItem {
    pub item: ItemInfo,
    pub qty: i64,
    /// 来源（购买、制作、战利品、剧情……）。
    pub source: String,
    pub equipped: bool,
}

fn item_source(conn: &Connection, campaign_id: &str, holder: &str, item: &str) -> String {
    (CampaignView { conn, campaign_id }).record("item_source", &format!("{holder}|{item}")).and_then(|v| v["source"].as_str().map(str::to_string)).unwrap_or_default()
}

fn source_effect(holder: &str, item: &str, source: &str) -> Effect {
    Effect::UpsertRecord { kind: "item_source".into(), id: format!("{holder}|{item}"), data: json!({ "source": source }) }
}

pub fn inventory(conn: &Connection, campaign_id: &str, holder: &str) -> Result<Vec<InvItem>> {
    let eq = equipment(conn, campaign_id, holder);
    let mut stmt = conn.prepare("SELECT item, qty FROM inventory WHERE campaign_id = ?1 AND holder = ?2 AND qty > 0 ORDER BY item")?;
    let rows: Vec<(String, i64)> = stmt.query_map([campaign_id, holder], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut out = vec![];
    for (id, qty) in rows {
        let item = item_info(conn, &id)?.unwrap_or_else(|| item_from(&id, &id, "", &Value::Null, "unknown"));
        out.push(InvItem { source: item_source(conn, campaign_id, holder, &id), equipped: eq.values().any(|v| *v == id), item, qty });
    }
    Ok(out)
}

/// 装备：栏位 → 物品 ID。
pub fn equipment(conn: &Connection, campaign_id: &str, holder: &str) -> BTreeMap<String, String> {
    (CampaignView { conn, campaign_id }).record("equip", holder).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

pub fn equipped_items(conn: &Connection, campaign_id: &str, holder: &str) -> Vec<ItemInfo> {
    let view = CampaignView { conn, campaign_id };
    equipment(conn, campaign_id, holder).values().filter(|i| view.item_qty(holder, i) > 0).filter_map(|i| item_info(conn, i).ok().flatten()).collect()
}

pub fn equip(world: &mut World, campaign_id: &str, holder: &str, item: &str) -> Result<String> {
    let it = require_item(world.conn(), item)?;
    if it.slot.is_empty() {
        return Err(CoreError::Invalid(format!("「{}」不能装备", it.title)));
    }
    if (CampaignView { conn: world.conn(), campaign_id }).item_qty(holder, item) < 1 {
        return Err(CoreError::Invalid(format!("没有「{}」", it.title)));
    }
    let mut eq = equipment(world.conn(), campaign_id, holder);
    eq.insert(it.slot.clone(), item.into());
    campaign::apply_effects(world, campaign_id, &[Effect::UpsertRecord { kind: "equip".into(), id: holder.into(), data: json!(eq) }], &format!("装备「{}」", it.title))?;
    crate::combat::refresh_unit(world, campaign_id, holder)?;
    Ok(format!("装备了「{}」", it.title))
}

/// 拿到可装备的物品时，如果对应栏位是空的就自动装备（实测：模型常常忘了写"装备"，导致买了剑却徒手作战）。
fn auto_equip_effect(conn: &Connection, campaign_id: &str, holder: &str, it: &ItemInfo) -> Option<Effect> {
    if it.slot.is_empty() || holder != PARTY {
        return None;
    }
    let mut eq = equipment(conn, campaign_id, holder);
    let view = CampaignView { conn, campaign_id };
    if eq.get(&it.slot).is_some_and(|cur| view.item_qty(holder, cur) > 0) {
        return None;
    }
    eq.insert(it.slot.clone(), it.id.clone());
    Some(Effect::UpsertRecord { kind: "equip".into(), id: holder.into(), data: json!(eq) })
}

pub fn unequip(world: &mut World, campaign_id: &str, holder: &str, slot: &str) -> Result<()> {
    let mut eq = equipment(world.conn(), campaign_id, holder);
    eq.remove(slot);
    campaign::apply_effects(world, campaign_id, &[Effect::UpsertRecord { kind: "equip".into(), id: holder.into(), data: json!(eq) }], "卸下装备")?;
    crate::combat::refresh_unit(world, campaign_id, holder)?;
    Ok(())
}

/// 攻击用的武器（伤害、射程、命中状态）：装备的武器，没有则徒手。
pub fn weapon(conn: &Connection, campaign_id: &str, s: &Sheet) -> (String, String, i64, Option<Status>) {
    match equipped_items(conn, campaign_id, &s.actor).into_iter().find(|i| i.slot == "weapon" && !i.damage.is_empty()) {
        Some(w) => (w.title.clone(), w.damage.clone(), w.range.max(1), w.on_hit.clone()),
        None => ("徒手".into(), s.unarmed.clone(), 1, None),
    }
}

/// 防御值 = 基础 + 敏捷修正 + 护甲与盾牌 + 状态修正。
pub fn armor_class(conn: &Connection, campaign_id: &str, s: &Sheet) -> i64 {
    let gear: i64 = equipped_items(conn, campaign_id, &s.actor).iter().map(|i| i.armor).sum();
    let agi = role_attr(&rules(conn, campaign_id), "agility");
    s.base_ac + s.modv(&agi) + gear + s.statuses.iter().map(|x| x.ac).sum::<i64>()
}

// ===================== 货币与交易 =====================

pub fn money(conn: &Connection, campaign_id: &str, holder: &str) -> i64 {
    CampaignView { conn, campaign_id }.var(&format!("money.{holder}")).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))).unwrap_or(0)
}

fn money_effect(holder: &str, v: i64) -> Effect {
    Effect::SetVar { key: format!("money.{holder}"), value: json!(v) }
}

/// 增减金钱（不能减到负数）。
pub fn change_money(world: &mut World, campaign_id: &str, holder: &str, delta: i64, reason: &str) -> Result<i64> {
    let cur = money(world.conn(), campaign_id, holder);
    if cur + delta < 0 {
        let r = rules(world.conn(), campaign_id);
        return Err(CoreError::Invalid(format!("钱不够：现有 {}，需要 {}", format_money(&r, cur), format_money(&r, -delta))));
    }
    campaign::apply_effects(world, campaign_id, &[money_effect(holder, cur + delta)], reason)?;
    Ok(cur + delta)
}

/// 获得物品（附来源）。
pub fn give_item(world: &mut World, campaign_id: &str, holder: &str, item: &str, qty: i64, source: &str) -> Result<()> {
    let it = require_item(world.conn(), item)?;
    let mut eff = vec![Effect::GiveItem { holder: holder.into(), item: item.into(), qty: qty.max(1) }, source_effect(holder, item, source)];
    let auto = auto_equip_effect(world.conn(), campaign_id, holder, &it);
    let equipped = auto.is_some();
    eff.extend(auto);
    campaign::apply_effects(world, campaign_id, &eff, source)?;
    if equipped {
        crate::combat::refresh_unit(world, campaign_id, holder)?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ShopItem {
    pub item: ItemInfo,
    /// 商人的库存（None 表示不限量）。
    pub stock: Option<i64>,
    /// 玩家买入的单价（铜币）。
    pub buy_price: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Shop {
    pub merchant: String,
    pub name: String,
    pub items: Vec<ShopItem>,
    /// 玩家卖给商人时，按价值的比例收购。
    pub buy_rate: f64,
    pub merchant_money: i64,
    /// 关系带来的折扣（0–0.1）。
    pub discount: f64,
    /// 世界局势对价格的影响（例如"战时武器紧缺，武器与护甲涨价 25%"）。
    #[serde(default)]
    pub price_notes: Vec<String>,
}

struct ShopCfg {
    stock: Vec<(String, Option<i64>)>,
    markup: f64,
    buy_rate: f64,
    money: i64,
}

fn shop_cfg(conn: &Connection, campaign_id: &str, merchant: &str) -> Result<Option<(String, ShopCfg)>> {
    let Some(n) = crate::nodes::get_node(conn, merchant)? else { return Ok(None) };
    let Some(sh) = n.payload.get("shop").filter(|s| s.is_object()) else { return Ok(None) };
    let r = rules(conn, campaign_id);
    let stock = sh
        .get("stock")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| Some((x.get("item")?.as_str()?.to_string(), x.get("qty").and_then(Value::as_i64)))).collect())
        .unwrap_or_default();
    Ok(Some((
        n.title,
        ShopCfg {
            stock,
            markup: sh.get("markup").and_then(Value::as_f64).unwrap_or(1.0),
            buy_rate: sh.get("buy_rate").and_then(Value::as_f64).unwrap_or(r.sell_rate),
            money: sh.get("money").and_then(Value::as_i64).unwrap_or(1000),
        },
    )))
}

/// 校验人物资料的 `shop`。
pub fn validate_shop(v: &Value) -> Result<()> {
    let o = v.as_object().ok_or_else(|| CoreError::Invalid("商店设置应为对象".into()))?;
    for (k, x) in o {
        match k.as_str() {
            "stock" => {
                let a = x.as_array().ok_or_else(|| CoreError::Invalid("商店库存应为列表".into()))?;
                for it in a {
                    if it.get("item").and_then(Value::as_str).is_none_or(str::is_empty) {
                        return Err(CoreError::Invalid("商店库存的每一项都要有 item".into()));
                    }
                    if it.get("qty").is_some_and(|q| !q.is_null() && !q.as_i64().is_some_and(|n| n >= 0)) {
                        return Err(CoreError::Invalid("商店库存数量应为非负整数".into()));
                    }
                }
            }
            "markup" | "buy_rate" => {
                if !x.as_f64().is_some_and(|f| (0.0..=10.0).contains(&f)) {
                    return Err(CoreError::Invalid(format!("商店的 {k} 应为 0–10 之间的数")));
                }
            }
            "money" => {
                if !x.as_i64().is_some_and(|n| n >= 0) {
                    return Err(CoreError::Invalid("商人的本钱应为非负整数".into()));
                }
            }
            other => return Err(CoreError::Invalid(format!("未知的商店设置项：{other}"))),
        }
    }
    Ok(())
}

/// 商人第一次交易时：库存放进商人的背包，本钱记为商人的金钱。
fn ensure_shop_stocked(world: &mut World, campaign_id: &str, merchant: &str, cfg: &ShopCfg) -> Result<()> {
    let view = CampaignView { conn: world.conn(), campaign_id };
    if view.record("shop_init", merchant).is_some() {
        return Ok(());
    }
    let mut eff: Vec<Effect> = cfg.stock.iter().filter_map(|(i, q)| q.map(|q| Effect::GiveItem { holder: merchant.into(), item: i.clone(), qty: q })).filter(|e| !matches!(e, Effect::GiveItem { qty: 0, .. })).collect();
    eff.push(money_effect(merchant, cfg.money));
    eff.push(Effect::UpsertRecord { kind: "shop_init".into(), id: merchant.into(), data: json!({ "turn": campaign::get_campaign(world.conn(), campaign_id)?.turn }) });
    campaign::apply_effects(world, campaign_id, &eff, "商人开张")?;
    Ok(())
}

fn discount(conn: &Connection, campaign_id: &str, merchant: &str) -> f64 {
    let trust = (CampaignView { conn, campaign_id }).bond(merchant, PARTY).map(|(_, b)| b.trust).unwrap_or(0);
    (trust.clamp(0, 100) as f64) / 1000.0
}

/// 世界局势对价格的影响：商人所属势力在打仗时武器、护甲涨价 25%；冬季药品涨价 15%。
fn world_price(conn: &Connection, campaign_id: &str, merchant: &str) -> (bool, bool) {
    let at_war = crate::worldsim::factions(conn, campaign_id).ok().is_some_and(|fs| {
        let mine: Vec<String> = conn
            .prepare("SELECT dst_id FROM relations WHERE deleted_at IS NULL AND src_id = ?1 AND rel_type IN ('member_of', 'leads', 'serves')")
            .and_then(|mut s| s.query_map([merchant], |r| r.get::<_, String>(0)).map(|rows| rows.filter_map(|r| r.ok()).collect()))
            .unwrap_or_default();
        fs.iter().any(|f| mine.contains(&f.id) && !f.at_war_with.is_empty())
    });
    let winter = crate::worldsim::environment(conn, campaign_id).is_ok_and(|e| e.season == "冬");
    (at_war, winter)
}

pub fn shop(conn: &Connection, campaign_id: &str, merchant: &str) -> Result<Option<Shop>> {
    let Some((name, cfg)) = shop_cfg(conn, campaign_id, merchant)? else { return Ok(None) };
    let view = CampaignView { conn, campaign_id };
    let stocked = view.record("shop_init", merchant).is_some();
    let d = discount(conn, campaign_id, merchant);
    let (at_war, winter) = world_price(conn, campaign_id, merchant);
    let mut notes = vec![];
    if at_war {
        notes.push("战时武器紧缺：武器、护甲、盾牌涨价 25%".to_string());
    }
    if winter {
        notes.push("冬季药品紧缺：治疗用品涨价 15%".to_string());
    }
    let mut items = vec![];
    for (id, q) in &cfg.stock {
        let Some(item) = item_info(conn, id)? else { continue };
        let stock = q.map(|q| if stocked { view.item_qty(merchant, id) } else { q });
        let mut f = cfg.markup * (1.0 - d);
        if at_war && matches!(item.slot.as_str(), "weapon" | "armor" | "shield") {
            f *= 1.25;
        }
        if winter && !item.heal.is_empty() {
            f *= 1.15;
        }
        items.push(ShopItem { buy_price: ((item.value as f64) * f).ceil().max(0.0) as i64, item, stock });
    }
    Ok(Some(Shop { merchant: merchant.into(), name, items, buy_rate: cfg.buy_rate, merchant_money: if stocked { money(conn, campaign_id, merchant) } else { cfg.money }, discount: d, price_notes: notes }))
}

/// 所有商人（人物资料里有商店设置的人）。
pub fn merchants(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM nodes WHERE kind = 'character' AND deleted_at IS NULL AND instr(payload, '\"shop\"') > 0 LIMIT 200")?;
    Ok(stmt.query_map([], |r| r.get(0))?.filter_map(|r| r.ok()).collect())
}

/// 每天补货：开张过的商人，有数量的商品每天补 1 个（不超过设定的数量）；本钱每天恢复设定值的 10%（不超过设定值）。
pub fn restock_effects(conn: &Connection, campaign_id: &str) -> Result<Vec<Effect>> {
    let view = CampaignView { conn, campaign_id };
    let mut eff = vec![];
    for m in merchants(conn)? {
        if view.record("shop_init", &m).is_none() {
            continue;
        }
        let Some((_, cfg)) = shop_cfg(conn, campaign_id, &m)? else { continue };
        for (item, q) in &cfg.stock {
            if let Some(q) = q
                && view.item_qty(&m, item) < *q
            {
                eff.push(Effect::GiveItem { holder: m.clone(), item: item.clone(), qty: 1 });
            }
        }
        let cur = money(conn, campaign_id, &m);
        if cur < cfg.money {
            eff.push(money_effect(&m, (cur + (cfg.money / 10).max(1)).min(cfg.money)));
        }
    }
    Ok(eff)
}

/// 以物易物：用背包里的东西换商人的东西，差价用钱补（按收购价与售价计算）。所有变化在一个事务里完成。
pub fn barter(world: &mut World, campaign_id: &str, merchant: &str, give: &[(String, i64)], get: &[(String, i64)]) -> Result<String> {
    if give.is_empty() || get.is_empty() {
        return Err(CoreError::Invalid("以物易物需要至少一件给出的东西和一件换来的东西".into()));
    }
    let (name, cfg) = shop_cfg(world.conn(), campaign_id, merchant)?.ok_or_else(|| CoreError::Invalid("这个人物不是商人".into()))?;
    ensure_shop_stocked(world, campaign_id, merchant, &cfg)?;
    let sh = shop(world.conn(), campaign_id, merchant)?.ok_or_else(|| CoreError::Invalid("这个人物不是商人".into()))?;
    let r = rules(world.conn(), campaign_id);
    let view = CampaignView { conn: world.conn(), campaign_id };
    let mut eff = vec![];
    let (mut give_value, mut get_value) = (0i64, 0i64);
    let mut give_names = vec![];
    let mut get_names = vec![];
    for (item, qty) in give {
        let qty = (*qty).max(1);
        let it = require_item(world.conn(), item)?;
        if view.item_qty(PARTY, item) < qty {
            return Err(CoreError::Invalid(format!("没有那么多「{}」", it.title)));
        }
        give_value += sell_price(&it, cfg.buy_rate) * qty;
        give_names.push(format!("「{}」×{qty}", it.title));
        eff.push(Effect::TakeItem { holder: PARTY.into(), item: item.clone(), qty });
        if cfg.stock.iter().any(|(i, q)| i == item && q.is_some()) {
            eff.push(Effect::GiveItem { holder: merchant.into(), item: item.clone(), qty });
        }
    }
    for (item, qty) in get {
        let qty = (*qty).max(1);
        let si = sh.items.iter().find(|i| &i.item.id == item).ok_or_else(|| CoreError::Invalid(format!("{name}不卖这个")))?;
        if si.stock.is_some_and(|s| s < qty) {
            return Err(CoreError::Invalid(format!("{name}的「{}」只剩 {} 个", si.item.title, si.stock.unwrap_or(0))));
        }
        get_value += si.buy_price * qty;
        get_names.push(format!("「{}」×{qty}", si.item.title));
        if si.stock.is_some() {
            eff.push(Effect::TakeItem { holder: merchant.into(), item: item.clone(), qty });
        }
        eff.push(Effect::GiveItem { holder: PARTY.into(), item: item.clone(), qty });
        eff.push(source_effect(PARTY, item, &format!("以物易物：{name}")));
    }
    let diff = get_value - give_value;
    let (mine, theirs) = (money(world.conn(), campaign_id, PARTY), money(world.conn(), campaign_id, merchant));
    if diff > 0 && mine < diff {
        return Err(CoreError::Invalid(format!("还要补 {}，钱不够（现有 {}）", format_money(&r, diff), format_money(&r, mine))));
    }
    if diff < 0 && theirs < -diff {
        return Err(CoreError::Invalid(format!("{name}要找你 {}，但他只有 {}", format_money(&r, -diff), format_money(&r, theirs))));
    }
    if diff != 0 {
        eff.push(money_effect(PARTY, mine - diff));
        eff.push(money_effect(merchant, theirs + diff));
    }
    let text = format!(
        "用{}和{name}换了{}{}",
        give_names.join("、"),
        get_names.join("、"),
        match diff.cmp(&0) {
            std::cmp::Ordering::Greater => format!("，补了 {}", format_money(&r, diff)),
            std::cmp::Ordering::Less => format!("，对方找回 {}", format_money(&r, -diff)),
            std::cmp::Ordering::Equal => String::new(),
        }
    );
    campaign::apply_effects(world, campaign_id, &eff, &text)?;
    Ok(text)
}

pub fn sell_price(item: &ItemInfo, buy_rate: f64) -> i64 {
    ((item.value as f64) * buy_rate).floor().max(0.0) as i64
}

/// 向商人购买。所有变化在一个事务里完成。
pub fn buy(world: &mut World, campaign_id: &str, merchant: &str, item: &str, qty: i64) -> Result<String> {
    let qty = qty.clamp(1, 999);
    let (name, cfg) = shop_cfg(world.conn(), campaign_id, merchant)?.ok_or_else(|| CoreError::Invalid("这个人物不是商人".into()))?;
    ensure_shop_stocked(world, campaign_id, merchant, &cfg)?;
    let sh = shop(world.conn(), campaign_id, merchant)?.ok_or_else(|| CoreError::Invalid("这个人物不是商人".into()))?;
    let si = sh.items.iter().find(|i| i.item.id == item).ok_or_else(|| CoreError::Invalid(format!("{name}不卖这个")))?;
    if si.stock.is_some_and(|s| s < qty) {
        return Err(CoreError::Invalid(format!("{name}的「{}」只剩 {} 个", si.item.title, si.stock.unwrap_or(0))));
    }
    let r = rules(world.conn(), campaign_id);
    let total = si.buy_price * qty;
    let mine = money(world.conn(), campaign_id, PARTY);
    if mine < total {
        return Err(CoreError::Invalid(format!("钱不够：需要 {}，现有 {}", format_money(&r, total), format_money(&r, mine))));
    }
    let mut eff = vec![money_effect(PARTY, mine - total), money_effect(merchant, sh.merchant_money + total)];
    if si.stock.is_some() {
        eff.push(Effect::TakeItem { holder: merchant.into(), item: item.into(), qty });
    }
    eff.push(Effect::GiveItem { holder: PARTY.into(), item: item.into(), qty });
    eff.push(source_effect(PARTY, item, &format!("购买：{name}")));
    let auto = auto_equip_effect(world.conn(), campaign_id, PARTY, &si.item);
    let text = format!("从{name}那里买了 {qty} 个「{}」，花了 {}{}", si.item.title, format_money(&r, total), if auto.is_some() { "（已装备）" } else { "" });
    eff.extend(auto);
    campaign::apply_effects(world, campaign_id, &eff, &text)?;
    crate::combat::refresh_unit(world, campaign_id, PARTY)?;
    Ok(text)
}

/// 卖给商人。
pub fn sell(world: &mut World, campaign_id: &str, merchant: &str, item: &str, qty: i64) -> Result<String> {
    let qty = qty.clamp(1, 999);
    let (name, cfg) = shop_cfg(world.conn(), campaign_id, merchant)?.ok_or_else(|| CoreError::Invalid("这个人物不是商人".into()))?;
    ensure_shop_stocked(world, campaign_id, merchant, &cfg)?;
    let it = require_item(world.conn(), item)?;
    let view = CampaignView { conn: world.conn(), campaign_id };
    if view.item_qty(PARTY, item) < qty {
        return Err(CoreError::Invalid(format!("没有那么多「{}」", it.title)));
    }
    let r = rules(world.conn(), campaign_id);
    let total = sell_price(&it, cfg.buy_rate) * qty;
    let theirs = money(world.conn(), campaign_id, merchant);
    if theirs < total {
        return Err(CoreError::Invalid(format!("{name}的钱不够（只有 {}）", format_money(&r, theirs))));
    }
    let mine = money(world.conn(), campaign_id, PARTY);
    let mut eff = vec![Effect::TakeItem { holder: PARTY.into(), item: item.into(), qty }, money_effect(PARTY, mine + total), money_effect(merchant, theirs - total)];
    if cfg.stock.iter().any(|(i, q)| i == item && q.is_some()) {
        eff.push(Effect::GiveItem { holder: merchant.into(), item: item.into(), qty });
    }
    // 卖掉的装备自动卸下。
    let mut eq = equipment(world.conn(), campaign_id, PARTY);
    if view.item_qty(PARTY, item) == qty && eq.values().any(|v| v == item) {
        eq.retain(|_, v| v != item);
        eff.push(Effect::UpsertRecord { kind: "equip".into(), id: PARTY.into(), data: json!(eq) });
    }
    let text = format!("把 {qty} 个「{}」卖给{name}，得到 {}", it.title, format_money(&r, total));
    campaign::apply_effects(world, campaign_id, &eff, &text)?;
    Ok(text)
}

/// 使用消耗品（治疗、解除状态）。
pub fn use_item(world: &mut World, campaign_id: &str, actor: &str, item: &str) -> Result<String> {
    let it = require_item(world.conn(), item)?;
    if it.heal.is_empty() && it.cures.is_empty() {
        return Err(CoreError::Invalid(format!("「{}」不能直接使用", it.title)));
    }
    if crate::combat::active(world.conn(), campaign_id).is_some_and(|c| c.units.iter().any(|u| u.actor.as_deref() == Some(actor) && u.out.is_empty())) {
        return Err(CoreError::Invalid("战斗中使用物品要消耗行动点，请作为战斗行动使用".into()));
    }
    if (CampaignView { conn: world.conn(), campaign_id }).item_qty(actor, item) < 1 {
        return Err(CoreError::Invalid(format!("没有「{}」", it.title)));
    }
    let mut s = sheet(world.conn(), campaign_id, actor)?;
    let mut parts = vec![];
    if !it.heal.is_empty() {
        let r = campaign::roll(world, campaign_id, &it.heal, &format!("使用「{}」", it.title))?;
        let before = s.hp;
        s.hp = (s.hp.max(0) + r.total.max(0)).min(s.max_hp);
        parts.push(format!("恢复 {} 点生命（{} → {}）", s.hp - before, before, s.hp));
    }
    if !it.cures.is_empty() {
        let n = s.statuses.len();
        s.statuses.retain(|x| !it.cures.contains(&x.name));
        if s.statuses.len() < n {
            parts.push(format!("解除了{}", it.cures.join("、")));
        }
    }
    let text = format!("{}使用了「{}」：{}", s.name, it.title, if parts.is_empty() { "没有效果".into() } else { parts.join("，") });
    let g = growth_effect(world.conn(), campaign_id, actor, &s.name, "heal", &text, 0);
    campaign::apply_effects(world, campaign_id, &[Effect::TakeItem { holder: actor.into(), item: item.into(), qty: 1 }, sheet_effect(&s), g], &text)?;
    Ok(text)
}

// ===================== 制作 =====================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CraftResult {
    pub success: bool,
    pub roll: i64,
    pub modifier: i64,
    pub dc: i64,
    pub text: String,
    pub materials_lost: bool,
}

/// 可以制作的物品（剧本与世界树中带配方的物品），以及材料是否足够。
pub fn recipes(conn: &Connection, campaign_id: &str, actor: &str) -> Result<Vec<(ItemInfo, bool)>> {
    let view = CampaignView { conn, campaign_id };
    let mut ids: Vec<String> = vec![];
    let c = campaign::get_campaign(conn, campaign_id)?;
    if let Some(s) = &c.script_id {
        ids.extend(crate::scripts::items_by_kind(conn, s, crate::model::ScriptItemKind::Item)?.into_iter().filter(|i| i.payload.get("recipe").is_some_and(Value::is_object)).map(|i| i.id));
    }
    let mut stmt = conn.prepare("SELECT id FROM nodes WHERE kind = 'item' AND deleted_at IS NULL AND instr(payload, '\"recipe\"') > 0 LIMIT 300")?;
    ids.extend(stmt.query_map([], |r| r.get::<_, String>(0))?.filter_map(|r| r.ok()));
    let mut out = vec![];
    for id in ids {
        if let Some(it) = item_info(conn, &id)?
            && let Some(rc) = &it.recipe
        {
            let ok = rc.inputs.iter().all(|i| view.item_qty(actor, &i.item) >= i.qty);
            out.push((it, ok));
        }
    }
    Ok(out)
}

/// 制作：材料齐全时由程序检定；成功消耗材料得到物品；失败且差 5 以上时材料损坏。耗时推进故事时间。
pub fn craft(world: &mut World, campaign_id: &str, actor: &str, item: &str) -> Result<CraftResult> {
    let it = require_item(world.conn(), item)?;
    let rc = it.recipe.clone().ok_or_else(|| CoreError::Invalid(format!("「{}」没有制作配方", it.title)))?;
    let view = CampaignView { conn: world.conn(), campaign_id };
    let mut missing = vec![];
    for i in &rc.inputs {
        let have = view.item_qty(actor, &i.item);
        if have < i.qty {
            let n = item_info(world.conn(), &i.item)?.map(|x| x.title).unwrap_or_else(|| i.item.clone());
            missing.push(format!("{n}（需要 {}，现有 {have}）", i.qty));
        }
    }
    if !missing.is_empty() {
        return Err(CoreError::Invalid(format!("材料不够：{}", missing.join("、"))));
    }
    let (roll, modifier, success) = if rc.dc > 0 {
        let skill = if rc.skill.is_empty() { "制作".to_string() } else { rc.skill.clone() };
        let m = check_modifier(world.conn(), campaign_id, actor, &skill);
        let r = campaign::roll(world, campaign_id, "1d20", &format!("制作「{}」：{skill} 检定", it.title))?;
        (r.total, m, r.total + m >= rc.dc)
    } else {
        (0, 0, true)
    };
    let lost = !success && roll + modifier < rc.dc - 5;
    let mut eff = vec![];
    if success || lost {
        for i in &rc.inputs {
            eff.push(Effect::TakeItem { holder: actor.into(), item: i.item.clone(), qty: i.qty });
        }
    }
    let name = sheet(world.conn(), campaign_id, actor)?.name;
    let text = if success {
        eff.push(Effect::GiveItem { holder: actor.into(), item: item.into(), qty: rc.qty.max(1) });
        eff.push(source_effect(actor, item, "制作"));
        format!("{name}制作出了「{}」×{}", it.title, rc.qty.max(1))
    } else if lost {
        format!("{name}制作「{}」失败，材料损坏了", it.title)
    } else {
        format!("{name}制作「{}」没有成功，材料还在", it.title)
    };
    eff.push(growth_effect(world.conn(), campaign_id, actor, &name, "craft", &text, 0));
    campaign::apply_effects(world, campaign_id, &eff, &text)?;
    if rc.minutes > 0 {
        crate::schedule::advance(world, campaign_id, rc.minutes, &format!("制作「{}」", it.title))?;
    }
    Ok(CraftResult { success, roll, modifier, dc: rc.dc, text, materials_lost: lost })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_levels_and_skill_attrs() {
        let r = RuleSet::default();
        assert_eq!(format_money(&r, 0), "0 铜币");
        assert_eq!(format_money(&r, 1234), "12 金币 3 银币 4 铜币");
        assert_eq!(format_money(&r, 105), "1 金币 5 铜币");
        assert_eq!(xp_for_level(&r, 1), 0);
        assert_eq!(xp_for_level(&r, 2), 100);
        assert_eq!(xp_for_level(&r, 3), 300);
        assert_eq!(skill_attr(&r, "察觉"), "wis");
        assert_eq!(skill_attr(&r, "潜行"), "dex");
        assert_eq!(skill_attr(&r, "锻造"), "str");
        assert_eq!(attr_mod(10), 0);
        assert_eq!(attr_mod(9), -1);
        assert_eq!(attr_mod(15), 2);
        assert!(RuleSet::default().validate().is_ok());
        let bad = RuleSet { currency_rates: vec![100, 10], ..RuleSet::default() };
        assert!(bad.validate().is_err());
    }

    /// 自定义属性方案：默认卡按作用对应；技能按作用找属性；未知属性不能存进角色卡。
    #[test]
    fn custom_attribute_scheme() {
        let r = RuleSet {
            attributes: vec![
                AttrDef { key: "body".into(), name: "体魄".into(), desc: String::new(), role: "might".into() },
                AttrDef { key: "reflex".into(), name: "反应".into(), desc: String::new(), role: "agility".into() },
                AttrDef { key: "tech".into(), name: "技术".into(), desc: String::new(), role: "mind".into() },
            ],
            ..Default::default()
        };
        assert!(r.validate().is_ok());
        let mut s = template("npc", "守卫", 3);
        s.attrs.insert("dex".into(), 16);
        adapt_attrs(&mut s, &r);
        assert_eq!(s.attrs.keys().cloned().collect::<Vec<_>>(), ["body", "reflex", "tech"]);
        assert_eq!(s.attr("reflex"), 16, "敏捷按作用对应到反应");
        assert_eq!(role_attr(&r, "agility"), "reflex");
        assert_eq!(role_attr(&r, "vitality"), "body", "没有体质作用时用第一个属性");
        assert_eq!(skill_attr(&r, "潜行"), "reflex");
        assert_eq!(skill_attr(&r, "炼金"), "tech");
        assert_eq!(attr_name(&r, "tech"), "技术");
        let mut bad = r.clone();
        bad.skill_attrs.insert("开锁".into(), "dex".into());
        assert!(bad.validate().is_err(), "技能指向方案外的属性");
    }

    #[test]
    fn stats_and_items_validate() {
        assert!(validate_stats(&json!({"level": 3, "hp": 20, "attrs": {"str": 14}, "damage": "1d8+2"})).is_ok());
        // 属性方案可以自定义（例如"运气"），只限制 key 长度。
        assert!(validate_stats(&json!({"attrs": {"luck": 14}})).is_ok());
        assert!(validate_stats(&json!({"attrs": {"a_very_long_attribute_key_name": 14}})).is_err());
        assert!(validate_stats(&json!({"damage": "abc"})).is_err());
        assert!(validate_item_payload(&json!({"value": 50, "rarity": "rare", "slot": "weapon", "damage": "1d8"})).is_ok());
        assert!(validate_item_payload(&json!({"rarity": "神器"})).is_err());
        assert!(validate_item_payload(&json!({"recipe": {"inputs": []}})).is_err());
        assert!(validate_shop(&json!({"stock": [{"item": "x", "qty": 3}], "markup": 1.2})).is_ok());
        assert!(validate_shop(&json!({"stock": [{"qty": 3}]})).is_err());
    }
}

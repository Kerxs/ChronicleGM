//! 玩家角色（开新战役时选择或生成）：身份、性格、能力、身世。
//!
//! - 保存为战役记录 `pc/party`（随时间线回溯恢复）；
//! - 角色卡（`rpg` 的 party）名字改成角色名，属性按剧本的属性方案与角色的能力分配（程序计算，可以在角色页修改）；
//! - GM 上下文的核心记忆里会写入角色的身份与身世；`link_to_plot` 只给主持人看。

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::campaign::{self, CampaignView};
use crate::error::{CoreError, Result};
use crate::rpg;
use crate::world::World;
use chronicle_rules::{Effect, StateView};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct PlayerCharacter {
    pub name: String,
    #[serde(default)]
    pub identity: String,
    #[serde(default)]
    pub personality: String,
    #[serde(default)]
    pub abilities: Vec<String>,
    /// 给玩家看的角色介绍。
    #[serde(default)]
    pub intro: String,
    /// 身世：origin / upbringing / occupation / key_events / relations。
    #[serde(default)]
    pub backstory: Value,
    /// 与主线的关联（主持人信息）。
    #[serde(default)]
    pub link_to_plot: String,
    /// 来自剧本里的哪个可选角色（AI 新生成的为空）。
    #[serde(default)]
    pub source_item: Option<String>,
    /// 随身的初始装备（开局时由程序放进背包；武器、护甲自动装备）。
    #[serde(default)]
    pub equipment: Vec<StartItem>,
}

/// 初始装备的一件物品。弹药是单独的一叠物品（例如「猎枪铅弹 ×12」），武器的 `ammo` 写它的名字。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct StartItem {
    pub name: String,
    #[serde(default = "one")]
    pub qty: i64,
    /// weapon / armor / shield / accessory，空 = 不能装备。
    #[serde(default)]
    pub slot: String,
    /// 伤害骰（武器），例如 1d10。
    #[serde(default)]
    pub damage: String,
    /// 射程（格）：近战 1。
    #[serde(default)]
    pub range: i64,
    #[serde(default)]
    pub armor: i64,
    /// common / uncommon / rare / epic / legendary
    #[serde(default)]
    pub rarity: String,
    /// 这件武器用的弹药（另一件物品的名字）；空 = 不用弹药。
    #[serde(default)]
    pub ammo: String,
    #[serde(default)]
    pub description: String,
}

fn one() -> i64 {
    1
}

impl StartItem {
    /// 整理模型写的装备（实测：栏位写成"主手""腰""背"，伤害写"无""d0"，弹药写"无"，射程写 150）：
    /// 有伤害骰的是武器，写了护甲的护具 / 盾是护甲，其余不可装备；弹药本身不是武器。
    pub fn normalized(&self, ammo_names: &[String]) -> StartItem {
        let mut it = self.clone();
        let blank = |s: &str| matches!(s.trim(), "" | "无" | "没有" | "none" | "null" | "-" | "—" | "d0" | "0");
        if blank(&it.damage) {
            it.damage.clear();
        }
        if blank(&it.ammo) {
            it.ammo.clear();
        }
        let is_dice = {
            let d = it.damage.trim().to_ascii_lowercase();
            d.split_once('d').is_some_and(|(n, m)| n.chars().all(|c| c.is_ascii_digit()) && !m.is_empty() && m.chars().take_while(|c| c.is_ascii_digit()).count() > 0 && m.chars().take_while(|c| c.is_ascii_digit()).collect::<String>() != "0")
        };
        let s = it.slot.trim().to_lowercase();
        let is_ammo = ammo_names.iter().any(|a| a.trim() == it.name.trim()) || s.contains("弹药") || s == "ammo";
        it.slot = if is_ammo {
            String::new()
        } else if matches!(s.as_str(), "shield") || s.contains('盾') {
            "shield".into()
        } else if it.armor > 0 && (matches!(s.as_str(), "armor") || ["甲", "护具", "身体", "躯干", "衣"].iter().any(|k| s.contains(k)) || (!is_dice && ["衣", "甲", "袍"].iter().any(|k| it.name.contains(k)))) {
            // 护甲要有护甲值（实测"行囊"写成 armor、护甲 0，被当成护甲穿上了）。
            "armor".into()
        } else if matches!(s.as_str(), "accessory") || ["饰", "戒指", "项链", "护符"].iter().any(|k| s.contains(k)) {
            "accessory".into()
        } else if is_dice || s == "weapon" || ["武器", "主手", "副手", "手"].iter().any(|k| s.contains(k)) && is_dice {
            "weapon".into()
        } else {
            String::new()
        };
        if it.slot != "weapon" {
            it.damage.clear();
            it.range = 0;
            it.ammo.clear();
        } else {
            if !is_dice {
                it.damage = "1d6".into();
            }
            // 射程是格数（近战 1，弓 / 枪一般 6–12）：写成米数的大数按 12 算。
            it.range = if it.range <= 0 { 1 } else if it.range > 30 { 12 } else { it.range };
        }
        if !matches!(it.slot.as_str(), "armor" | "shield") {
            it.armor = 0;
        }
        it.qty = it.qty.clamp(1, 200);
        it
    }
}

/// 整理一整套初始装备（弹药名取自武器的 ammo）。
pub fn normalize_kit(items: &[StartItem]) -> Vec<StartItem> {
    let ammo: Vec<String> = items.iter().map(|i| i.ammo.trim().to_string()).filter(|a| !a.is_empty() && !matches!(a.as_str(), "无" | "没有" | "none")).collect();
    // 同名的只留第一件（实测：开场推断出两件"粗布工作服"）。
    let mut seen: Vec<String> = vec![];
    items
        .iter()
        .filter(|i| !i.name.trim().is_empty())
        .filter(|i| {
            let n = i.name.trim().to_string();
            let fresh = !seen.contains(&n);
            seen.push(n);
            fresh
        })
        .map(|i| {
            let mut n = i.normalized(&ammo);
            // 武器写的弹药必须是这套装备里真有的一项（实测模型给吉他写了 ammo: "string"，战斗里"没有 string 了，打不响"）。
            if !n.ammo.is_empty() && (n.ammo.trim() == n.name.trim() || !items.iter().any(|x| x.name.trim() == n.ammo.trim())) {
                n.ammo.clear();
            }
            n
        })
        .collect()
}

pub const PC_KIND: &str = "pc";

pub fn get(conn: &rusqlite::Connection, campaign_id: &str) -> Option<PlayerCharacter> {
    CampaignView { conn, campaign_id }.record(PC_KIND, &rpg::actor()).and_then(|v| serde_json::from_value(v).ok())
}

/// 从剧本的「可选角色」条目读出角色。
pub fn from_script_item(it: &crate::model::ScriptItem) -> PlayerCharacter {
    let p = &it.payload;
    let s = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    PlayerCharacter {
        name: it.title.clone(),
        identity: s("identity"),
        personality: s("personality"),
        abilities: p.get("abilities").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        intro: it.player_text.clone(),
        backstory: p.get("backstory").cloned().unwrap_or(Value::Null),
        link_to_plot: it.gm_text.trim_start_matches("与主线的关联：").to_string(),
        source_item: Some(it.id.clone()),
        equipment: normalize_kit(&p.get("equipment").and_then(|v| serde_json::from_value::<Vec<StartItem>>(v.clone()).ok()).unwrap_or_default()),
    }
}

/// 按能力分配属性与技能：能力里出现的技能（剧本玩法配置里的技能表）+2，对应属性 +2；
/// 角色身份、能力的文字里提到属性名时该属性 +1。基础 10，每项 8–16。
pub fn derive_stats(r: &rpg::RuleSet, pc: &PlayerCharacter) -> (std::collections::BTreeMap<String, i64>, std::collections::BTreeMap<String, i64>) {
    let defs = rpg::attr_defs(r);
    let mut attrs: std::collections::BTreeMap<String, i64> = defs.iter().map(|a| (a.key.clone(), 10)).collect();
    let mut skills = std::collections::BTreeMap::new();
    let text = format!("{} {} {}", pc.identity, pc.personality, pc.abilities.join(" "));
    for (skill, attr) in &r.skill_attrs {
        if pc.abilities.iter().any(|a| a.contains(skill.as_str()) || skill.contains(a.as_str())) {
            skills.insert(skill.clone(), 2);
            if let Some(v) = attrs.get_mut(attr) {
                *v += 2;
            }
        }
    }
    for a in &pc.abilities {
        // 能力本身不在技能表里时，也记成技能（+1），按默认表找属性。
        let name: String = a.chars().take(8).collect();
        if !name.is_empty() && !skills.contains_key(&name) && skills.len() < 6 {
            skills.insert(name.clone(), 1);
            let attr = rpg::skill_attr(r, &name);
            if let Some(v) = attrs.get_mut(&attr) {
                *v += 1;
            }
        }
    }
    for d in &defs {
        if !d.name.is_empty() && text.contains(&d.name)
            && let Some(v) = attrs.get_mut(&d.key)
        {
            *v += 1;
        }
    }
    for v in attrs.values_mut() {
        *v = (*v).clamp(8, 16);
    }
    (attrs, skills)
}

/// 设定玩家角色：保存记录、改角色卡名字与属性。只能在战役开始时（第 0 回合）设定。
pub fn set(world: &mut World, campaign_id: &str, pc: &PlayerCharacter) -> Result<()> {
    if pc.name.trim().is_empty() {
        return Err(CoreError::Invalid("角色名不能为空".into()));
    }
    let c = campaign::get_campaign(world.conn(), campaign_id)?;
    if c.turn > 0 {
        return Err(CoreError::Invalid("战役已经开始，不能再换角色（可以在角色页修改角色卡）".into()));
    }
    let r = rpg::rules(world.conn(), campaign_id);
    let mut sheet = rpg::sheet(world.conn(), campaign_id, &rpg::actor())?;
    sheet.name = pc.name.trim().chars().take(30).collect();
    let (attrs, skills) = derive_stats(&r, pc);
    sheet.attrs = attrs;
    sheet.skills = skills;
    let data = serde_json::to_value(pc)?;
    campaign::apply_effects(
        world,
        campaign_id,
        &[Effect::UpsertRecord { kind: PC_KIND.into(), id: rpg::actor(), data }, rpg::sheet_effect(&sheet)],
        &format!("选择角色：{}", sheet.name),
    )?;
    // 角色定义里写了随身装备：放进背包（背包已经有东西时不重复发放）。
    if !pc.equipment.is_empty() && rpg::inventory(world.conn(), campaign_id, &rpg::actor())?.is_empty() {
        rpg::grant_start_kit(world, campaign_id, &pc.equipment)?;
    }
    Ok(())
}

/// GM 上下文里的"玩家角色"一节（含主持人信息 link_to_plot，NPC 看不到）。
pub fn context_text(pc: &PlayerCharacter) -> String {
    let b = &pc.backstory;
    let s = |k: &str| b.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let list = |k: &str| b.get(k).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("；")).unwrap_or_default();
    let mut lines = vec![format!("{}：{}；性格：{}；擅长：{}", pc.name, pc.identity, pc.personality, pc.abilities.join("、"))];
    let bs = [("出身", s("origin")), ("成长", s("upbringing")), ("职业", s("occupation")), ("重要经历", list("key_events")), ("初始关系", list("relations"))];
    let bs: Vec<String> = bs.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("{k}：{v}")).collect();
    if !bs.is_empty() {
        lines.push(format!("身世：{}", bs.join("；")));
    }
    if !pc.link_to_plot.is_empty() {
        lines.push(format!("（主持人信息）与主线的关联：{}", pc.link_to_plot));
    }
    lines.join("\n")
}

pub fn to_json(pc: &PlayerCharacter) -> Value {
    json!(pc)
}

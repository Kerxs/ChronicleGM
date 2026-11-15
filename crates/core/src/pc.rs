//! 玩家角色（开新战役时选择或生成）：身份、性格、能力、身世。
//!
//! - 保存为战役记录 `pc/party`（随时间线回溯恢复）；
//! - 角色卡（`rpg` 的 party）名字改成角色名，属性按剧本的属性方案与角色的能力分配（程序计算，可以在角色页修改）；
//! - GM 上下文的核心记忆里会写入角色的身份与身世；`link_to_plot` 只给主持人看。

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::campaign::{self, CampaignView};
use crate::error::{CoreError, Result};
use crate::rpg::{self, PARTY};
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
}

pub const PC_KIND: &str = "pc";

pub fn get(conn: &rusqlite::Connection, campaign_id: &str) -> Option<PlayerCharacter> {
    CampaignView { conn, campaign_id }.record(PC_KIND, PARTY).and_then(|v| serde_json::from_value(v).ok())
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
    let mut sheet = rpg::sheet(world.conn(), campaign_id, PARTY)?;
    sheet.name = pc.name.trim().chars().take(30).collect();
    let (attrs, skills) = derive_stats(&r, pc);
    sheet.attrs = attrs;
    sheet.skills = skills;
    let data = serde_json::to_value(pc)?;
    campaign::apply_effects(
        world,
        campaign_id,
        &[Effect::UpsertRecord { kind: PC_KIND.into(), id: PARTY.into(), data }, rpg::sheet_effect(&sheet)],
        &format!("选择角色：{}", sheet.name),
    )?;
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

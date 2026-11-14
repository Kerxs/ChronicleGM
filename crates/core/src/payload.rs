//! 各类节点 payload 的结构定义与校验。
//!
//! payload 以 JSON 存储，但写入前必须能反序列化为对应类型（类型检查）。
//! 未知字段保留在 `extra` 中，方便扩展。字段名以 `gm_` 开头或列在
//! [`gm_fields`] 中的内容只对主持人可见。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{CoreError, Result};
use crate::model::{NodeKind, QuestType, ScriptItemKind};

type Extra = Map<String, Value>;

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CharacterPayload {
    pub identity: String,
    pub appearance: String,
    pub personality: String,
    pub motivation: String,
    /// 主持人可见：秘密。
    pub secrets: Vec<String>,
    /// NPC 明确知道的事实（AI 扮演时只能使用这些知识）。
    pub knowledge: Vec<String>,
    pub abilities: Vec<String>,
    /// 存活状态：alive / dead / missing / unknown。
    pub life_status: Option<String>,
    pub birth_time: Option<i64>,
    pub death_time: Option<i64>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct EventPayload {
    /// 世界内时间排序键（由历法换算）。
    pub world_time: Option<i64>,
    pub date_label: String,
    pub participants: Vec<String>,
    pub location: Option<String>,
    pub consequences: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct LocationPayload {
    pub climate: String,
    pub population: Option<i64>,
    pub ruler: Option<String>,
    pub features: Vec<String>,
    pub dangers: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct FactionPayload {
    pub goals: Vec<String>,
    pub leader: Option<String>,
    pub resources: Vec<String>,
    pub ideology: String,
    /// 主持人可见：真实目的。
    pub hidden_agenda: String,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SystemPayload {
    /// 体系规则（魔法 / 科技）。
    pub rules: Vec<String>,
    pub limitations: Vec<String>,
    pub costs: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ItemPayload {
    pub item_type: String,
    pub properties: Vec<String>,
    pub owner: Option<String>,
    pub value: Option<i64>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct GenericPayload {
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct QuestPayload {
    pub quest_type: QuestType,
    #[serde(default)]
    pub objectives: Vec<String>,
    #[serde(default)]
    pub rewards: Vec<String>,
    #[serde(default)]
    pub giver: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

fn check<T: serde::de::DeserializeOwned>(v: &Value, what: &str) -> Result<()> {
    if !v.is_object() {
        return Err(CoreError::Invalid(format!("{what} 的 payload 必须是 JSON 对象")));
    }
    serde_json::from_value::<T>(v.clone())
        .map(|_| ())
        .map_err(|e| CoreError::Invalid(format!("{what} 的 payload 格式错误: {e}")))
}

pub fn validate_node_payload(kind: NodeKind, v: &Value) -> Result<()> {
    use NodeKind::*;
    let what = kind.as_str();
    match kind {
        Character => {
            check::<CharacterPayload>(v, what)?;
            // 日程（可选）：时间写法与活动必须有效。
            if let Some(sv) = v.get("schedule").filter(|x| !x.is_null()) {
                let entries: Vec<crate::schedule::ScheduleEntry> =
                    serde_json::from_value(sv.clone()).map_err(|e| CoreError::Invalid(format!("日程格式错误：{e}")))?;
                crate::schedule::validate_schedule(&entries)?;
            }
            if let Some(st) = v.get("stats").filter(|x| !x.is_null()) {
                crate::rpg::validate_stats(st)?;
            }
            if let Some(sh) = v.get("shop").filter(|x| !x.is_null()) {
                crate::rpg::validate_shop(sh)?;
            }
            // 声音（可选）：{instructions, voice_id, seed}。
            if let Some(vc) = v.get("voice").filter(|x| !x.is_null()) {
                let o = vc.as_object().ok_or_else(|| CoreError::Invalid("声音设置应为对象".into()))?;
                for (k, x) in o {
                    let ok = match k.as_str() {
                        "instructions" | "voice_id" => x.is_string(),
                        "seed" => x.is_null() || x.is_i64(),
                        _ => false,
                    };
                    if !ok {
                        return Err(CoreError::Invalid(format!("声音设置的 {k} 格式不对（可用 instructions、voice_id、seed）")));
                    }
                }
            }
            Ok(())
        }
        Event | TimelineEvent | Legend => check::<EventPayload>(v, what),
        Region | Nation | City | Site => check::<LocationPayload>(v, what),
        Faction => {
            check::<FactionPayload>(v, what)?;
            // 势力实力（可选，世界模拟的初始值）：财富、军力、影响、稳定，0–100。
            if let Some(p) = v.get("power").filter(|x| !x.is_null()) {
                let m = p.as_object().ok_or_else(|| CoreError::Invalid("势力实力应为对象".into()))?;
                for (k, x) in m {
                    if !["wealth", "military", "influence", "stability"].contains(&k.as_str()) {
                        return Err(CoreError::Invalid(format!("未知的势力实力项：{k}")));
                    }
                    if !x.as_i64().is_some_and(|n| (0..=100).contains(&n)) {
                        return Err(CoreError::Invalid(format!("势力实力 {k} 应为 0–100 的整数")));
                    }
                }
            }
            Ok(())
        }
        MagicSystem | TechSystem => check::<SystemPayload>(v, what),
        Item => {
            check::<ItemPayload>(v, what)?;
            crate::rpg::validate_item_payload(v)
        }
        World | Cosmos | Era | Race | Culture | Category | Custom => check::<GenericPayload>(v, what),
    }
}

pub fn validate_script_item_payload(kind: ScriptItemKind, v: &Value) -> Result<()> {
    match kind {
        ScriptItemKind::Quest => check::<QuestPayload>(v, "quest"),
        ScriptItemKind::Item => {
            check::<GenericPayload>(v, "item")?;
            crate::rpg::validate_item_payload(v)
        }
        // 触发器 / 关键事件可以带条件与效果（世界模拟在时间推进时检查）。
        ScriptItemKind::Trigger | ScriptItemKind::KeyEvent => {
            check::<GenericPayload>(v, kind.as_str())?;
            if let Some(w) = v.get("when").filter(|x| !x.is_null()) {
                let c: chronicle_rules::Condition = serde_json::from_value(w.clone()).map_err(|e| CoreError::Invalid(format!("触发条件格式错误：{e}")))?;
                c.validate().map_err(|e| CoreError::Invalid(format!("触发条件无效：{e}")))?;
            }
            if let Some(e) = v.get("effects").filter(|x| !x.is_null()) {
                serde_json::from_value::<Vec<chronicle_rules::Effect>>(e.clone()).map_err(|e| CoreError::Invalid(format!("触发效果格式错误：{e}")))?;
            }
            Ok(())
        }
        _ => check::<GenericPayload>(v, kind.as_str()),
    }
}

/// 每种节点的主持人专用 payload 字段。
pub fn gm_fields(kind: NodeKind) -> &'static [&'static str] {
    match kind {
        NodeKind::Character => &["secrets"],
        NodeKind::Faction => &["hidden_agenda"],
        _ => &[],
    }
}

pub fn strip_gm_fields(kind: NodeKind, v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let hidden = gm_fields(kind);
            Value::Object(
                map.iter()
                    .filter(|(k, _)| !k.starts_with("gm_") && !hidden.contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

/// 某类节点 payload 的 JSON Schema（后续提供给模型做结构化输出）。
pub fn node_payload_schema(kind: NodeKind) -> Value {
    use NodeKind::*;
    let schema = match kind {
        Character => schemars::schema_for!(CharacterPayload),
        Event | TimelineEvent | Legend => schemars::schema_for!(EventPayload),
        Region | Nation | City | Site => schemars::schema_for!(LocationPayload),
        Faction => schemars::schema_for!(FactionPayload),
        MagicSystem | TechSystem => schemars::schema_for!(SystemPayload),
        Item => schemars::schema_for!(ItemPayload),
        World | Cosmos | Era | Race | Culture | Category | Custom => schemars::schema_for!(GenericPayload),
    };
    serde_json::to_value(schema).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validates_types() {
        assert!(validate_node_payload(NodeKind::Character, &json!({"secrets":["x"],"custom_field":1})).is_ok());
        assert!(validate_node_payload(NodeKind::Character, &json!({"secrets":"not a list"})).is_err());
        assert!(validate_node_payload(NodeKind::City, &json!([])).is_err());
        assert!(validate_script_item_payload(ScriptItemKind::Quest, &json!({"quest_type":"hidden"})).is_ok());
        assert!(validate_script_item_payload(ScriptItemKind::Quest, &json!({})).is_err());
        assert!(validate_script_item_payload(ScriptItemKind::Quest, &json!({"quest_type":"bogus"})).is_err());
    }

    #[test]
    fn strips_hidden() {
        let v = json!({"identity":"铁匠","secrets":["是间谍"],"gm_note":"x"});
        assert_eq!(strip_gm_fields(NodeKind::Character, &v), json!({"identity":"铁匠"}));
    }

    #[test]
    fn schema_available() {
        let s = node_payload_schema(NodeKind::Character);
        assert!(s.to_string().contains("knowledge"));
    }
}

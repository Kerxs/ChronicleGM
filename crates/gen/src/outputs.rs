//! 模型输出的结构定义。每个结构都会生成 JSON Schema 交给模型，
//! 返回结果必须能反序列化为对应结构才会进入下一步。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// 玩家可见文本与主持人专用文本。
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Texts {
    /// 玩家可见的描述。
    #[serde(default)]
    pub player_text: String,
    /// 只有主持人能看到的信息（秘密、真相、操作提示）。
    #[serde(default)]
    pub gm_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChapterOutline {
    pub title: String,
    /// 本章概要（玩家可见）。
    #[serde(default)]
    pub player_text: String,
    /// 本章主持人笔记。
    #[serde(default)]
    pub gm_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuestKind {
    Main,
    Side,
    Hidden,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QuestOutline {
    pub title: String,
    pub quest_type: QuestKind,
    #[serde(default)]
    pub player_text: String,
    #[serde(default)]
    pub gm_text: String,
    #[serde(default)]
    pub objectives: Vec<String>,
    #[serde(default)]
    pub rewards: Vec<String>,
    /// 所属章节标题（可选）。
    #[serde(default)]
    pub chapter: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NpcOutline {
    pub name: String,
    /// 在本剧本中的定位（如"委托人""幕后黑手"）。
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub identity: String,
    #[serde(default)]
    pub motivation: String,
    /// 主持人可见的秘密。
    #[serde(default)]
    pub secret: String,
    #[serde(default)]
    pub relation_to_players: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EndingOutline {
    pub title: String,
    /// 达成条件（主持人可见）。
    #[serde(default)]
    pub condition: String,
    #[serde(default)]
    pub player_text: String,
    #[serde(default)]
    pub gm_text: String,
}

/// 剧本大纲（第一段生成）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScriptOutline {
    pub title: String,
    #[serde(default)]
    pub genre: String,
    #[serde(default)]
    pub tone: String,
    /// 一句话核心卖点。
    #[serde(default)]
    pub premise: String,
    #[serde(default)]
    pub background: String,
    /// 核心设定与真相（主持人可见）。
    #[serde(default)]
    pub core_setting: String,
    pub opening: Texts,
    pub chapters: Vec<ChapterOutline>,
    // 以下字段在 schema 中为必填：结构化输出时模型不能省略（实测曾省略任务）。
    pub quests: Vec<QuestOutline>,
    pub npcs: Vec<NpcOutline>,
    pub endings: Vec<EndingOutline>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SceneOut {
    pub title: String,
    #[serde(default)]
    pub player_text: String,
    #[serde(default)]
    pub gm_text: String,
    /// 地点名称（尽量使用已有地点）。
    #[serde(default)]
    pub location: Option<String>,
    /// 出场人物名称。
    #[serde(default)]
    pub npcs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ClueOut {
    pub title: String,
    #[serde(default)]
    pub player_text: String,
    #[serde(default)]
    pub gm_text: String,
    /// 在哪个场景中发现（场景标题）。
    #[serde(default)]
    pub found_in: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EventOut {
    pub title: String,
    /// 触发条件（主持人可见）。
    #[serde(default)]
    pub trigger: String,
    #[serde(default)]
    pub player_text: String,
    #[serde(default)]
    pub gm_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SimpleOut {
    pub title: String,
    #[serde(default)]
    pub player_text: String,
    #[serde(default)]
    pub gm_text: String,
}

/// 展开一个章节（分段生成）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChapterExpansion {
    pub scenes: Vec<SceneOut>,
    pub clues: Vec<ClueOut>,
    #[serde(default)]
    pub items: Vec<SimpleOut>,
    #[serde(default)]
    pub key_events: Vec<EventOut>,
    #[serde(default)]
    pub twists: Vec<SimpleOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NpcDetail {
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub identity: String,
    #[serde(default)]
    pub appearance: String,
    #[serde(default)]
    pub personality: String,
    #[serde(default)]
    pub motivation: String,
    /// 主持人可见的秘密。
    #[serde(default)]
    pub secrets: Vec<String>,
    /// 这个人物明确知道的事实。
    #[serde(default)]
    pub knowledge: Vec<String>,
    #[serde(default)]
    pub abilities: Vec<String>,
    #[serde(default)]
    pub player_text: String,
    #[serde(default)]
    pub gm_text: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub relation_to_players: String,
}

/// NPC 档案（完善已有人物或新增人物）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NpcBatch {
    pub npcs: Vec<NpcDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ItemRewrite {
    /// 必须是输入中给出的条目 ID。
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub player_text: String,
    #[serde(default)]
    pub gm_text: String,
}

/// 只重写选中的条目。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Rewrites {
    pub items: Vec<ItemRewrite>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExtSceneOut {
    /// 挂在哪个章节下（已有或本次新建的章节标题）。
    pub chapter: String,
    pub title: String,
    #[serde(default)]
    pub player_text: String,
    #[serde(default)]
    pub gm_text: String,
}

/// 继续扩展剧本：只新增内容。
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ScriptExtension {
    #[serde(default)]
    pub chapters: Vec<ChapterOutline>,
    #[serde(default)]
    pub scenes: Vec<ExtSceneOut>,
    #[serde(default)]
    pub quests: Vec<QuestOutline>,
    #[serde(default)]
    pub npcs: Vec<NpcOutline>,
    #[serde(default)]
    pub clues: Vec<ClueOut>,
    #[serde(default)]
    pub endings: Vec<EndingOutline>,
}

/// 发给模型的 JSON schema：所有对象的全部字段都设为必填（可以填空值），字段顺序与结构体一致。
/// 原因（实测）：llama.cpp 的语法约束允许跳过非必填字段，模型于是常常不写 summary
/// （展开节点、补全后简介为空），或把跳过的字段写进了别的字符串里。
/// 解析时这些字段仍有默认值，不支持 schema 的服务不受影响。
pub fn schema_of<T: JsonSchema>() -> serde_json::Value {
    let mut v = serde_json::to_value(schemars::schema_for!(T)).unwrap_or(serde_json::Value::Null);
    require_all(&mut v);
    v
}

fn require_all(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(o) => {
            if let Some(serde_json::Value::Object(props)) = o.get("properties") {
                let all: Vec<serde_json::Value> = props.keys().map(|k| serde_json::Value::String(k.clone())).collect();
                o.insert("required".into(), serde_json::Value::Array(all));
            }
            for x in o.values_mut() {
                require_all(x);
            }
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(require_all),
        _ => {}
    }
}

#[cfg(test)]
mod schema_tests {
    #[test]
    fn nested_fields_are_required_and_ordered() {
        let s = super::schema_of::<crate::world_tasks::ExpansionOut>();
        let text = s.to_string();
        // WorldNodeOut 的 summary 必须在 required 里（嵌套定义同样处理）。
        let defs = s.get("$defs").or_else(|| s.get("definitions")).expect("有嵌套定义");
        let node = &defs["WorldNodeOut"];
        let req: Vec<&str> = node["required"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
        assert!(req.contains(&"summary") && req.contains(&"children"), "{req:?}");
        let keys: Vec<&String> = node["properties"].as_object().unwrap().keys().collect();
        assert_eq!(keys[0], "name", "字段顺序与结构体一致：{keys:?}");
        assert!(text.contains("\"required\""));
    }
}

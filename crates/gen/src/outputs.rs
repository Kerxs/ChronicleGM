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

/// 玩法比重 0–3。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlayWeights {
    pub combat: u8,
    pub exploration: u8,
    pub social: u8,
    /// 调查 / 推理：只有悬疑、推理、侦探类剧本才给 2–3，其他题材 0–1。
    pub investigation: u8,
    pub survival: u8,
    pub management: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AttrOut {
    /// 英文小写 key，例如 body、reflex、tech。
    pub key: String,
    /// 中文名称。
    pub name: String,
    #[serde(default)]
    pub desc: String,
    /// 作用：might（力量）/ agility（敏捷）/ vitality（体质）/ mind（智力）/ insight（感知）/ presence（魅力），可留空。
    #[serde(default)]
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SkillOut {
    pub skill: String,
    /// 对应的属性 key。
    pub attr: String,
}

/// 剧本的玩法配置。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlayOut {
    pub weights: PlayWeights,
    /// 3–6 个符合题材的属性。
    pub attributes: Vec<AttrOut>,
    /// 常用技能与对应属性（4–10 个）。
    #[serde(default)]
    pub skills: Vec<SkillOut>,
    /// 检定方式：target（掷骰 + 修正 ≥ 难度）/ roll_under（百分骰 ≤ 目标值）/ pool（骰池计成功数）。
    pub check_mode: String,
    /// 主骰，例如 1d20、2d6、1d100（pool 模式写 d6 之类）。
    pub die: String,
    /// 货币名称，从大到小（例如 ["金币","银币","铜币"]、["信用点"]、["两银子","文钱"]），1–3 个。
    #[serde(default)]
    pub currency: Vec<String>,
    /// 每种货币等于多少个最小单位（和 currency 一一对应，最后一个是 1），例如 [100,10,1]。
    #[serde(default)]
    pub currency_rates: Vec<i64>,
    /// 五档稀有度的叫法（从普通到传说），符合题材，例如 ["寻常","上品","珍品","宝物","神兵"]。
    #[serde(default)]
    pub rarity_labels: Vec<String>,
    /// 叙事风格，一句话。
    #[serde(default)]
    pub style: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Backstory {
    /// 出身。
    pub origin: String,
    /// 成长经历。
    pub upbringing: String,
    /// 职业或身份。
    pub occupation: String,
    /// 重要经历（1–3 条）。
    #[serde(default)]
    pub key_events: Vec<String>,
    /// 初始人际关系：与剧本里哪些人物有什么关系（写人物名）。
    #[serde(default)]
    pub relations: Vec<String>,
    /// 与主线的关联（主持人可见，可以包含玩家自己都不知道的伏笔）。
    pub link_to_plot: String,
}

/// 可选的玩家角色（含身世）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PcOutline {
    pub name: String,
    pub identity: String,
    pub personality: String,
    /// 擅长的事（2–4 条）。
    #[serde(default)]
    pub abilities: Vec<String>,
    /// 给玩家看的角色介绍（2–3 句）。
    pub player_text: String,
    pub backstory: Backstory,
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
    /// 2–4 个可选的玩家角色，身世与主线有关联。
    pub player_characters: Vec<PcOutline>,
    /// 玩法配置。
    pub play: PlayOut,
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

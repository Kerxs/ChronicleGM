//! 领域模型：世界树节点、关联、剧本、剧情图。

use chronicle_rules::{Condition, Effect};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{CoreError, Result};

/// 定义一个序列化为 snake_case 字符串的枚举，并提供与数据库文本互转。
macro_rules! str_enum {
    ($(#[$m:meta])* $name:ident { $($var:ident => $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
        #[ts(export)]
        pub enum $name { $(#[serde(rename = $s)] $var),+ }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$var),+];
            pub fn as_str(self) -> &'static str {
                match self { $($name::$var => $s),+ }
            }
            pub fn parse(s: &str) -> Result<Self> {
                match s {
                    $($s => Ok($name::$var),)+
                    other => Err(CoreError::Invalid(format!(concat!(stringify!($name), " 无效值: {}"), other))),
                }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.as_str()) }
        }
    };
}

str_enum!(
    /// 世界树节点类型。
    NodeKind {
        World => "world",
        Cosmos => "cosmos",
        Era => "era",
        TimelineEvent => "timeline_event",
        Region => "region",
        Nation => "nation",
        City => "city",
        Site => "site",
        Faction => "faction",
        Race => "race",
        Character => "character",
        Culture => "culture",
        TechSystem => "tech_system",
        MagicSystem => "magic_system",
        Event => "event",
        Legend => "legend",
        Item => "item",
        Category => "category",
        Custom => "custom",
    }
);

str_enum!(
    /// 正典等级：core 为核心设定，AI 不得修改。
    CanonLevel { Core => "core", Established => "established", Draft => "draft" }
);

str_enum!(ContentStatus { Draft => "draft", Confirmed => "confirmed" });

str_enum!(Visibility { Player => "player", Gm => "gm" });

str_enum!(
    ScriptItemKind {
        Opening => "opening",
        Chapter => "chapter",
        Scene => "scene",
        Quest => "quest",
        Clue => "clue",
        Item => "item",
        LocationRef => "location_ref",
        KeyEvent => "key_event",
        Trigger => "trigger",
        Twist => "twist",
        Ending => "ending",
        Handout => "handout",
        PlayerCharacter => "player_character",
    }
);

str_enum!(QuestType { Main => "main", Side => "side", Hidden => "hidden" });

str_enum!(
    PlotNodeKind {
        Start => "start",
        Scene => "scene",
        Choice => "choice",
        Event => "event",
        Ending => "ending",
        Dynamic => "dynamic",
    }
);

str_enum!(PlotNodeStatus { Planned => "planned", Generated => "generated", Visited => "visited" });

/// 写入者。AI 写入只能经过变更集。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Author {
    User,
    Ai { job_id: String },
    System,
}

impl Author {
    pub fn tag(&self) -> String {
        match self {
            Author::User => "user".into(),
            Author::Ai { job_id } => format!("ai:{job_id}"),
            Author::System => "system".into(),
        }
    }
    pub fn from_tag(s: &str) -> Self {
        match s {
            "user" => Author::User,
            "system" => Author::System,
            other => Author::Ai { job_id: other.strip_prefix("ai:").unwrap_or(other).to_string() },
        }
    }
    pub fn is_ai(&self) -> bool {
        matches!(self, Author::Ai { .. })
    }
}

// ===================== 世界树 =====================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    pub parent_id: Option<String>,
    pub sort_key: String,
    pub path: String,
    pub title: String,
    pub aliases: Vec<String>,
    pub summary: String,
    pub player_text: String,
    pub gm_text: String,
    pub payload: Value,
    pub tags: Vec<String>,
    pub canon_level: CanonLevel,
    pub locked: bool,
    pub status: ContentStatus,
    pub source: String,
    pub rev: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

impl Node {
    /// 玩家视角：去掉主持人文本与 payload 中的隐藏字段。
    pub fn player_view(&self) -> Node {
        let mut n = self.clone();
        n.gm_text.clear();
        n.payload = crate::payload::strip_gm_fields(self.kind, &self.payload);
        n
    }
}

/// 节点插入位置。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    /// 放到最后。
    #[default]
    Last,
    First,
    /// 放到某个兄弟节点之后。
    After(String),
    /// 放到某个兄弟节点之前。
    Before(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NewNode {
    pub kind: NodeKind,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub position: Position,
    pub title: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub player_text: String,
    #[serde(default)]
    pub gm_text: String,
    #[serde(default = "empty_object")]
    pub payload: Value,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_canon")]
    pub canon_level: CanonLevel,
    #[serde(default)]
    pub locked: bool,
    #[serde(default = "default_status")]
    pub status: ContentStatus,
}

pub fn empty_object() -> Value {
    Value::Object(Default::default())
}
fn default_canon() -> CanonLevel {
    CanonLevel::Draft
}
fn default_status() -> ContentStatus {
    ContentStatus::Draft
}

impl NewNode {
    pub fn new(kind: NodeKind, title: impl Into<String>) -> Self {
        Self {
            kind,
            parent_id: None,
            position: Position::Last,
            title: title.into(),
            aliases: vec![],
            summary: String::new(),
            player_text: String::new(),
            gm_text: String::new(),
            payload: empty_object(),
            tags: vec![],
            canon_level: CanonLevel::Draft,
            locked: false,
            status: ContentStatus::Draft,
        }
    }
    pub fn under(mut self, parent: &str) -> Self {
        self.parent_id = Some(parent.to_string());
        self
    }
    pub fn payload(mut self, v: Value) -> Self {
        self.payload = v;
        self
    }
    pub fn canon(mut self, c: CanonLevel) -> Self {
        self.canon_level = c;
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NodePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gm_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canon_level: Option<CanonLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ContentStatus>,
}

impl NodePatch {
    pub fn is_empty(&self) -> bool {
        *self == NodePatch::default()
    }
    /// 是否只是解锁（锁定节点唯一允许的修改）。
    pub fn is_unlock_only(&self) -> bool {
        *self == NodePatch { locked: Some(false), ..Default::default() }
    }
}

// ===================== 关联 =====================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Relation {
    pub id: String,
    pub src_id: String,
    pub dst_id: String,
    pub rel_type: String,
    pub directed: bool,
    pub label: String,
    pub attrs: Value,
    pub visibility: Visibility,
    pub valid_from: Option<i64>,
    pub valid_to: Option<i64>,
    pub source: String,
    pub rev: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NewRelation {
    pub src_id: String,
    pub dst_id: String,
    pub rel_type: String,
    #[serde(default = "default_true")]
    pub directed: bool,
    #[serde(default)]
    pub label: String,
    #[serde(default = "empty_object")]
    pub attrs: Value,
    #[serde(default = "default_visibility")]
    pub visibility: Visibility,
    #[serde(default)]
    pub valid_from: Option<i64>,
    #[serde(default)]
    pub valid_to: Option<i64>,
}

fn default_true() -> bool {
    true
}
fn default_visibility() -> Visibility {
    Visibility::Player
}

impl NewRelation {
    pub fn new(src: &str, dst: &str, rel_type: &str) -> Self {
        Self {
            src_id: src.into(),
            dst_id: dst.into(),
            rel_type: rel_type.into(),
            directed: true,
            label: String::new(),
            attrs: empty_object(),
            visibility: Visibility::Player,
            valid_from: None,
            valid_to: None,
        }
    }
}

// ===================== 剧本 =====================

fn empty_obj() -> Value {
    Value::Object(Default::default())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Script {
    pub id: String,
    pub title: String,
    pub genre: String,
    pub premise: String,
    pub background: String,
    pub core_setting: String,
    pub player_count: i64,
    pub duration_hours: f64,
    pub tone: String,
    pub scale_profile: Value,
    /// 玩法配置（`profile::GameProfile` 的 JSON）；空对象 = 按题材推断。
    #[serde(default = "empty_obj")]
    pub game_profile: Value,
    pub status: ContentStatus,
    pub source: String,
    pub rev: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NewScript {
    pub title: String,
    #[serde(default)]
    pub genre: String,
    #[serde(default)]
    pub premise: String,
    #[serde(default)]
    pub background: String,
    #[serde(default)]
    pub core_setting: String,
    #[serde(default = "default_players")]
    pub player_count: i64,
    #[serde(default = "default_hours")]
    pub duration_hours: f64,
    #[serde(default)]
    pub tone: String,
    /// 玩法配置（`profile::GameProfile` 的 JSON），可以为空对象。
    #[serde(default = "empty_obj")]
    pub game_profile: Value,
}

fn default_players() -> i64 {
    4
}
fn default_hours() -> f64 {
    4.0
}

impl NewScript {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            genre: String::new(),
            premise: String::new(),
            background: String::new(),
            core_setting: String::new(),
            player_count: 4,
            duration_hours: 4.0,
            tone: String::new(),
            game_profile: empty_obj(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ScriptPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub premise: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core_setting: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_hours: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ContentStatus>,
    /// 玩法配置（`profile::GameProfile`），保存前校验。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_profile: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ScriptItem {
    pub id: String,
    pub script_id: String,
    pub kind: ScriptItemKind,
    pub parent_id: Option<String>,
    pub sort_key: String,
    pub title: String,
    pub player_text: String,
    pub gm_text: String,
    pub payload: Value,
    pub locked: bool,
    pub status: ContentStatus,
    pub source: String,
    pub rev: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NewScriptItem {
    pub script_id: String,
    pub kind: ScriptItemKind,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub position: Position,
    pub title: String,
    #[serde(default)]
    pub player_text: String,
    #[serde(default)]
    pub gm_text: String,
    #[serde(default = "empty_object")]
    pub payload: Value,
    #[serde(default)]
    pub locked: bool,
}

impl NewScriptItem {
    pub fn new(script_id: &str, kind: ScriptItemKind, title: impl Into<String>) -> Self {
        Self {
            script_id: script_id.into(),
            kind,
            parent_id: None,
            position: Position::Last,
            title: title.into(),
            player_text: String::new(),
            gm_text: String::new(),
            payload: empty_object(),
            locked: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ScriptItemPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gm_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ContentStatus>,
}

impl ScriptItemPatch {
    pub fn is_unlock_only(&self) -> bool {
        *self == ScriptItemPatch { locked: Some(false), ..Default::default() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CastMember {
    pub id: String,
    pub script_id: String,
    pub node_id: String,
    pub role: String,
    pub motive: String,
    pub secret: String,
    pub relation_to_players: String,
    pub payload: Value,
    pub locked: bool,
    pub source: String,
    pub rev: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NewCastMember {
    pub script_id: String,
    pub node_id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub motive: String,
    #[serde(default)]
    pub secret: String,
    #[serde(default)]
    pub relation_to_players: String,
    #[serde(default = "empty_object")]
    pub payload: Value,
}

// ===================== 剧情图 =====================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct PlotNode {
    pub id: String,
    pub script_id: String,
    pub kind: PlotNodeKind,
    pub title: String,
    pub player_text: String,
    pub gm_text: String,
    pub scene_ref: Option<String>,
    pub script_item_ref: Option<String>,
    pub status: PlotNodeStatus,
    pub payload: Value,
    pub locked: bool,
    pub source: String,
    pub rev: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NewPlotNode {
    pub script_id: String,
    pub kind: PlotNodeKind,
    pub title: String,
    #[serde(default)]
    pub player_text: String,
    #[serde(default)]
    pub gm_text: String,
    #[serde(default)]
    pub scene_ref: Option<String>,
    #[serde(default)]
    pub script_item_ref: Option<String>,
    #[serde(default = "empty_object")]
    pub payload: Value,
}

impl NewPlotNode {
    pub fn new(script_id: &str, kind: PlotNodeKind, title: impl Into<String>) -> Self {
        Self {
            script_id: script_id.into(),
            kind,
            title: title.into(),
            player_text: String::new(),
            gm_text: String::new(),
            scene_ref: None,
            script_item_ref: None,
            payload: empty_object(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct PlotNodePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<PlotNodeKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gm_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<PlotNodeStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
}

impl PlotNodePatch {
    pub fn is_unlock_only(&self) -> bool {
        *self == PlotNodePatch { locked: Some(false), ..Default::default() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct PlotEdge {
    pub id: String,
    pub script_id: String,
    pub from_id: String,
    pub to_id: String,
    pub label: String,
    pub condition: Option<Condition>,
    pub effects: Vec<Effect>,
    pub priority: i64,
    pub once: bool,
    pub locked: bool,
    pub source: String,
    pub rev: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

/// 剧情边的修改（两端不可改；`condition: Some(None)` 表示去掉条件）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlotEdgePatch {
    pub label: Option<String>,
    pub condition: Option<Option<Condition>>,
    pub effects: Option<Vec<Effect>>,
    pub priority: Option<i64>,
    pub once: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NewPlotEdge {
    pub script_id: String,
    pub from_id: String,
    pub to_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub condition: Option<Condition>,
    #[serde(default)]
    pub effects: Vec<Effect>,
    #[serde(default)]
    pub priority: i64,
    #[serde(default)]
    pub once: bool,
}

impl NewPlotEdge {
    pub fn new(script_id: &str, from: &str, to: &str) -> Self {
        Self {
            script_id: script_id.into(),
            from_id: from.into(),
            to_id: to.into(),
            label: String::new(),
            condition: None,
            effects: vec![],
            priority: 0,
            once: false,
        }
    }
}

// ===================== 规模表 =====================

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Range {
    pub min: u32,
    pub max: u32,
}

/// 根据玩家人数与时长给出剧本规模范围（确定性），用于提示词与校验。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ScaleProfile {
    pub scenes: Range,
    pub main_quests: Range,
    pub side_quests: Range,
    pub hidden_quests: Range,
    pub npcs: Range,
    pub clues: Range,
    pub endings: Range,
}

pub fn scale_profile(player_count: i64, duration_hours: f64) -> ScaleProfile {
    let players = player_count.clamp(1, 12) as f64;
    let hours = duration_hours.clamp(0.5, 100.0);
    let r = |lo: f64, hi: f64| {
        let min = lo.round().max(0.0);
        Range { min: min as u32, max: hi.round().max(min) as u32 }
    };
    // 约每 45–60 分钟一个场景；每个场景 2–3 条线索；NPC 随人数与时长增长。
    let scenes = r(hours * 1.0, hours * 1.4 + 1.0);
    ScaleProfile {
        scenes,
        main_quests: r(1.0, (hours / 4.0).ceil().max(1.0)),
        side_quests: r((players / 2.0).floor(), players + hours / 2.0),
        hidden_quests: r(if hours >= 3.0 { 1.0 } else { 0.0 }, (hours / 3.0).ceil()),
        npcs: r(3.0 + hours * 0.8, 4.0 + hours * 1.5 + players * 0.5),
        clues: r(scenes.min as f64 * 2.0, scenes.max as f64 * 3.0),
        endings: r(2.0, if hours >= 6.0 { 5.0 } else { 3.0 }),
    }
}

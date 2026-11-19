//! Chronicle GM 联机协议：客户端 ↔ 权威服务端、服务端 ↔ 房主 GM Agent 的全部消息。
//!
//! - 云服务器和房主主机跑同一个服务端，用同一套协议。
//! - 所有消息都是 JSON（WebSocket 文本帧），带 `t` 字段区分类型；握手时互报 [`PROTOCOL_VERSION`]，不一致直接拒绝。
//! - 客户端**只能提交意图**（行动文字、战斗动作、聊天）；HP、骰子、物品、NPC、任务的任何变化只由服务端产生，
//!   以 [`ServerMsg::Event`] 广播。客户端发来的任何"结果"字段都不存在于协议里。
//! - 每个行动带客户端生成的 `action_id`（去重）和它看到的 `state_version`（过期就拒绝，客户端刷新后重提）。

use serde::{Deserialize, Serialize};

/// 协议版本：不兼容的改动加 1。
pub const PROTOCOL_VERSION: u32 = 1;

/// 默认限制（服务端配置可以调小）。
pub mod limits {
    /// 单条 WebSocket 消息最大字节数。
    pub const MAX_FRAME_BYTES: usize = 16 * 1024;
    /// 行动 / 聊天文字最大字符数。
    pub const MAX_TEXT_CHARS: usize = 600;
    /// 每个房间行动队列的最大长度。
    pub const MAX_QUEUE: usize = 16;
    /// 房间人数上限的硬上限（配置不能超过它）。
    pub const MAX_PLAYERS_HARD: u8 = 8;
    /// 每个连接每秒最多消息数（超过就丢弃并提示）。
    pub const MAX_MSGS_PER_SEC: u32 = 8;
}

pub type PlayerId = String;
pub type RoomId = String;

/// 可见性：所有读库、广播、快照、AI 上下文都按它过滤。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum Visibility {
    /// 房间里所有人可见。
    Public,
    /// 只有这个玩家自己（和 GM）可见：私人剧情、自己的背包变化、私人 GM 对话。
    Personal { player: PlayerId },
    /// 指定的几名玩家可见（私聊、小队密谈）。
    Players { players: Vec<PlayerId> },
    /// 只有 GM 可见（秘密、真相、AI 意图），任何玩家都收不到。
    GmOnly,
}

impl Visibility {
    /// 这个玩家能不能看到。GM 视角（`player = None`）能看到一切。
    pub fn visible_to(&self, player: Option<&str>) -> bool {
        match (self, player) {
            (_, None) => true,
            (Visibility::Public, _) => true,
            (Visibility::Personal { player: p }, Some(me)) => p == me,
            (Visibility::Players { players }, Some(me)) => players.iter().any(|p| p == me),
            (Visibility::GmOnly, Some(_)) => false,
        }
    }
}

/// 玩家在房间里的状态（玩家列表里显示）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerInfo {
    pub id: PlayerId,
    pub name: String,
    /// 扮演的角色名（还没选角色时为空）。
    #[serde(default)]
    pub character: String,
    pub host: bool,
    pub online: bool,
}

/// 房间概况（加入成功、玩家进出、设置变化时发）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomInfo {
    pub id: RoomId,
    pub name: String,
    pub max_players: u8,
    pub players: Vec<PlayerInfo>,
    /// 当前权威 GM 执行节点：cloud / local（房主 Agent）/ none。
    pub gm_mode: String,
    /// GM 节点是否在线可用。
    pub gm_ready: bool,
    /// 当前状态版本（每次提交加 1）。
    pub state_version: u64,
}

/// 客户端发给服务端。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    /// 第一条消息：加入房间。`token` 是断线重连用的会话令牌（第一次加入为空，用 `join_code`）。
    Hello {
        protocol: u32,
        room: RoomId,
        #[serde(default)]
        join_code: String,
        #[serde(default)]
        token: String,
        name: String,
        /// 断线重连：客户端已经看到的最后一个事件序号，服务端补发之后的。
        #[serde(default)]
        last_seq: i64,
    },
    /// 提交一个行动（自由文字，或战斗按钮）。只表达意图，结果由服务端算。
    Action {
        action_id: String,
        state_version: u64,
        #[serde(default)]
        text: String,
        /// 战斗按钮：attack / defend / dodge / skill / use_item / interact / move / flee / end_turn。
        #[serde(default)]
        combat: Option<CombatIntent>,
    },
    /// 聊天：`to` 为空 = 公共频道；否则私聊这些玩家。不经过 GM，不改游戏状态。
    Chat {
        #[serde(default)]
        to: Vec<PlayerId>,
        text: String,
    },
    /// 私人 GM 对话：只有自己和 GM 看得到（例如"我偷偷检查口袋里的信"）。
    PrivateGm { action_id: String, state_version: u64, text: String },
    /// 亲手掷骰时回传点数（服务端再校验点数合法性；不合法就由服务端掷）。
    DiceAnswer { request_id: String, faces: Option<Vec<Vec<u32>>> },
    Ping { nonce: u64 },
    /// 要一份（按权限过滤后的）当前状态快照：自己的角色与背包、队友的公开信息、场景、任务、战斗。
    GetState,
    /// 房主修改 GM 设置：local（房主电脑上的 GM Agent）/ cloud（云端模型）/ auto（Agent 在线用本地，否则云端）。
    /// `cloud.api_key` 为空表示沿用已保存的 Key。只有房主能发，Key 只保存在服务端。
    SetGm {
        mode: String,
        #[serde(default)]
        cloud: Option<CloudGmConfig>,
    },
    /// 查询当前 GM 状态。
    GetGm,
}

/// 云端模型配置（房主填写；服务端保存，Key 不会再发给任何客户端）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudGmConfig {
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "cloud_defaults::temperature")]
    pub temperature: f32,
    #[serde(default = "cloud_defaults::context_length")]
    pub context_length: u32,
    #[serde(default = "cloud_defaults::timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "cloud_defaults::max_tokens")]
    pub max_tokens: u32,
}

mod cloud_defaults {
    pub fn temperature() -> f32 {
        0.8
    }
    pub fn context_length() -> u32 {
        32768
    }
    pub fn timeout_secs() -> u64 {
        120
    }
    pub fn max_tokens() -> u32 {
        4096
    }
}

/// 发给玩家的 GM 状态（不含 Key）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GmStatus {
    /// 房主选的策略：local / cloud / auto。
    pub mode: String,
    /// 现在实际用的节点：local / cloud / none。
    pub active: String,
    pub ready: bool,
    pub agent_online: bool,
    #[serde(default)]
    pub agent_model: String,
    #[serde(default)]
    pub cloud_model: String,
    /// 云端是否已经保存了 Key（只告诉有没有，不给内容）。
    #[serde(default)]
    pub cloud_has_key: bool,
}

/// 战斗意图（只有目标和种类，没有任何数值）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CombatIntent {
    pub kind: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub item: String,
    #[serde(default)]
    pub skill: String,
    #[serde(default)]
    pub x: Option<i64>,
    #[serde(default)]
    pub y: Option<i64>,
}

/// 服务端发给客户端。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    /// 加入成功：自己的身份、会话令牌、房间概况。
    Welcome { player: PlayerId, token: String, room: RoomInfo },
    Room { room: RoomInfo },
    /// 一条已提交的游戏事件（已按可见性过滤）。`seq` 单调递增，客户端据此去重、补发。
    Event { seq: i64, kind: String, payload: serde_json::Value },
    /// 聊天消息（公共或私聊）。
    Chat { from: PlayerId, to: Vec<PlayerId>, text: String, at: i64 },
    /// 行动进入队列 / 开始处理 / 完成 / 被拒绝。
    ActionStatus { action_id: String, status: ActionState, #[serde(default)] reason: String, state_version: u64 },
    /// 请这个玩家亲手掷骰（只发给行动的那名玩家）。
    DiceAsk { request_id: String, ask: serde_json::Value },
    /// GM 处理进度（构思 / 判定 / 叙述……），给玻璃球显示。
    GmStage { stage: String },
    Error { code: ErrorCode, message: String },
    Pong { nonce: u64 },
    /// 状态快照（已按这个玩家的权限过滤）。
    Snapshot { state_version: u64, data: serde_json::Value },
    /// GM 状态变化（Agent 上下线、房主切换模式）。
    Gm { gm: GmStatus },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionState {
    Queued,
    Running,
    Done,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadProtocol,
    BadMessage,
    Unauthorized,
    RoomFull,
    RoomNotFound,
    TooLarge,
    RateLimited,
    QueueFull,
    StaleVersion,
    Duplicate,
    NotYourTurn,
    GmUnavailable,
    /// 没有权限（例如不是房主却要改 GM 设置）。连接不会断开。
    Forbidden,
    Internal,
}

// ===================== 服务端 ↔ 房主 GM Agent =====================

/// GM Agent 发给服务端（Agent 主动连出，房主本机模型端口不暴露到公网）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum AgentMsg {
    /// 注册：房间与房主授权的 Agent 密钥（只在房主手里，不发给其他玩家）。
    Register { protocol: u32, room: RoomId, agent_key: String, model: String },
    /// 任务结果：模型的原始输出文本（结构化解析与规则校验仍在服务端做）。
    TaskResult { task_id: String, ok: bool, #[serde(default)] content: String, #[serde(default)] error: String },
    Heartbeat { busy: bool },
}

/// 服务端发给 GM Agent：一次模型调用（OpenAI 兼容的消息与参数）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum AgentTask {
    Registered { ok: bool, #[serde(default)] reason: String },
    Chat {
        task_id: String,
        messages: Vec<ChatLine>,
        #[serde(default)]
        json_schema: Option<serde_json::Value>,
        max_tokens: u32,
        temperature: f32,
        /// 秒；超时后服务端会把任务改派（或报错）。
        timeout_secs: u32,
    },
    Cancel { task_id: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatLine {
    pub role: String,
    pub content: String,
}

/// 解析一条客户端消息，并做大小与文字长度检查。
pub fn parse_client(raw: &str) -> Result<ClientMsg, ErrorCode> {
    if raw.len() > limits::MAX_FRAME_BYTES {
        return Err(ErrorCode::TooLarge);
    }
    let m: ClientMsg = serde_json::from_str(raw).map_err(|_| ErrorCode::BadMessage)?;
    let too_long = |s: &str| s.chars().count() > limits::MAX_TEXT_CHARS;
    let bad = match &m {
        ClientMsg::Action { text, action_id, .. } => too_long(text) || action_id.is_empty() || action_id.len() > 64,
        ClientMsg::PrivateGm { text, action_id, .. } => too_long(text) || action_id.is_empty() || action_id.len() > 64,
        ClientMsg::Chat { text, to } => too_long(text) || text.trim().is_empty() || to.len() > limits::MAX_PLAYERS_HARD as usize,
        // 用会话令牌重连时名字可以为空；第一次加入必须有名字。
        ClientMsg::Hello { name, token, .. } => (name.trim().is_empty() && token.is_empty()) || name.chars().count() > 24 || token.len() > 128,
        ClientMsg::SetGm { mode, cloud } => {
            !matches!(mode.as_str(), "local" | "cloud" | "auto")
                || cloud.as_ref().is_some_and(|c| c.base_url.len() > 300 || c.model.len() > 120 || c.api_key.len() > 400 || !c.temperature.is_finite())
        }
        _ => false,
    };
    if bad {
        return Err(ErrorCode::BadMessage);
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_rules() {
        let a = Some("a");
        assert!(Visibility::Public.visible_to(a));
        assert!(Visibility::Personal { player: "a".into() }.visible_to(a));
        assert!(!Visibility::Personal { player: "b".into() }.visible_to(a));
        assert!(Visibility::Players { players: vec!["a".into(), "c".into()] }.visible_to(a));
        assert!(!Visibility::Players { players: vec!["b".into()] }.visible_to(a));
        assert!(!Visibility::GmOnly.visible_to(a));
        assert!(Visibility::GmOnly.visible_to(None), "GM 视角看得到一切");
    }

    #[test]
    fn messages_round_trip_and_reject_bad_input() {
        let m = ClientMsg::Action { action_id: "a1".into(), state_version: 3, text: "我推开门".into(), combat: None };
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains("\"t\":\"action\""));
        assert_eq!(parse_client(&s).unwrap(), m);
        assert_eq!(parse_client("{\"t\":\"nope\"}"), Err(ErrorCode::BadMessage));
        let long = serde_json::to_string(&ClientMsg::Chat { to: vec![], text: "字".repeat(limits::MAX_TEXT_CHARS + 1) }).unwrap();
        assert_eq!(parse_client(&long), Err(ErrorCode::BadMessage));
        assert_eq!(parse_client(&"x".repeat(limits::MAX_FRAME_BYTES + 1)), Err(ErrorCode::TooLarge));
        // 协议里没有任何"结果"字段：客户端多塞的 hp / dice 字段被忽略，不会进入消息。
        let forged = r#"{"t":"action","action_id":"x","state_version":1,"text":"攻击","hp":999,"dice":[20]}"#;
        let m = parse_client(forged).unwrap();
        assert!(!serde_json::to_string(&m).unwrap().contains("999"));
    }
}

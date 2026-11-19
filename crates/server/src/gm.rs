//! GM Orchestrator（第 4 阶段）：每个房间一个权威 GM 执行节点。
//!
//! - **Cloud Provider**：服务端用房主配置的云端 OpenAI 兼容接口（Base URL、模型、Key、温度、上下文、超时、输出长度）。
//!   Key 只存在服务端的 `gm.json`（不进 room.json、不出现在任何发给玩家的消息里）。
//! - **Local Provider**：房主电脑上的 GM Agent 主动连上服务端（`/agent`），服务端把一次次模型调用派给它，
//!   Agent 调房主本机的模型再把原始输出传回来。本机模型端口不暴露到公网。
//! - 策略：local（只用房主 Agent）/ cloud（只用云端）/ auto（Agent 在线用本地，否则用云端）；房主可以随时手动切换。
//! - 不管哪种 Provider，提示词、结构化解析、规则校验、写库都在服务端完成（复用 chronicle-gm 的回合流程）。
//!   同一房间同一时刻只有一个 Agent 连接被接受；行动队列本来就是串行的，所以只有一个节点在执行 GM 任务。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chronicle_gen::Templates;
use chronicle_llm::{CancelToken, ChatRequest, ChatResponse, LlmClient, LlmError, OpenAiClient, OutputFormat, ProviderConfig, Usage, client::Progress};
use chronicle_protocol::{AgentTask, ChatLine, GmStatus};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::game::{GameRoom, GmBackend, QueuedAction};

/// 云端模型配置（房主填写）。
pub use chronicle_protocol::CloudGmConfig as CloudConfig;

/// 房间的 GM 设置（存 `gm.json`，只在服务端）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GmSettings {
    /// local / cloud / auto
    pub mode: String,
    #[serde(default)]
    pub cloud: Option<CloudConfig>,
}

/// 等待 Agent 回传结果的模型调用：任务号 → 结果通道。
pub type Pending = Arc<Mutex<HashMap<String, std::sync::mpsc::Sender<Result<String, String>>>>>;

/// 在线的房主 Agent。
pub struct AgentConn {
    pub conn: u64,
    pub model: String,
    pub tx: mpsc::Sender<AgentTask>,
    pub pending: Pending,
}

pub struct RoomGm {
    pub room_id: String,
    path: PathBuf,
    pub settings: Mutex<GmSettings>,
    pub agent: Mutex<Option<AgentConn>>,
    templates: Arc<Templates>,
    next_task: AtomicU64,
}

impl RoomGm {
    pub fn load(room_id: &str, dir: &std::path::Path, default_mode: &str, templates: Arc<Templates>) -> RoomGm {
        let path = dir.join("gm.json");
        let settings = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<GmSettings>(&b).ok()).unwrap_or_else(|| {
            // 还没有 gm.json：用建房时选的模式（room.json 里的 gm_mode）。
            let from_room = std::fs::read(dir.join("room.json")).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()).and_then(|v| v["gm_mode"].as_str().map(str::to_string));
            GmSettings { mode: normalize_mode(from_room.as_deref().unwrap_or(default_mode)), cloud: None }
        });
        RoomGm { room_id: room_id.to_string(), path, settings: Mutex::new(settings), agent: Mutex::new(None), templates, next_task: AtomicU64::new(1) }
    }

    fn save(&self, s: &GmSettings) -> std::io::Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(tmp, &self.path)
    }

    /// 房主修改 GM 设置。`cloud.api_key` 为空表示沿用已保存的 Key。
    pub fn update(&self, mode: &str, cloud: Option<CloudConfig>) -> Result<(), String> {
        let mut s = self.settings.lock().map_err(|_| "内部错误".to_string())?;
        s.mode = normalize_mode(mode);
        if let Some(mut c) = cloud {
            if c.base_url.trim().is_empty() || c.model.trim().is_empty() {
                return Err("云端模型的地址和模型名不能为空".into());
            }
            if !(c.base_url.starts_with("https://") || c.base_url.starts_with("http://")) {
                return Err("云端模型地址要以 http:// 或 https:// 开头".into());
            }
            if c.api_key.trim().is_empty()
                && let Some(old) = &s.cloud
            {
                c.api_key = old.api_key.clone();
            }
            c.temperature = c.temperature.clamp(0.0, 2.0);
            c.timeout_secs = c.timeout_secs.clamp(10, 600);
            c.max_tokens = c.max_tokens.clamp(256, 32000);
            c.context_length = c.context_length.clamp(2048, 1_000_000);
            s.cloud = Some(c);
        }
        self.save(&s).map_err(|e| format!("保存 GM 设置失败：{e}"))
    }

    pub fn agent_online(&self) -> bool {
        self.agent.lock().map(|a| a.is_some()).unwrap_or(false)
    }

    fn cloud_ready(&self) -> bool {
        self.settings.lock().map(|s| s.cloud.as_ref().is_some_and(|c| !c.base_url.is_empty() && !c.model.is_empty())).unwrap_or(false)
    }

    /// 按策略选出这一次用哪个节点。
    pub fn active(&self) -> &'static str {
        let mode = self.settings.lock().map(|s| s.mode.clone()).unwrap_or_default();
        match mode.as_str() {
            "cloud" if self.cloud_ready() => "cloud",
            "local" if self.agent_online() => "local",
            "auto" if self.agent_online() => "local",
            "auto" if self.cloud_ready() => "cloud",
            _ => "none",
        }
    }

    pub fn public(&self) -> GmStatus {
        let s = self.settings.lock().map(|s| s.clone()).unwrap_or_default();
        let agent_model = self.agent.lock().ok().and_then(|a| a.as_ref().map(|x| x.model.clone())).unwrap_or_default();
        let active = self.active().to_string();
        GmStatus {
            mode: s.mode,
            ready: active != "none",
            active,
            agent_online: self.agent_online(),
            agent_model,
            cloud_model: s.cloud.as_ref().map(|c| c.model.clone()).unwrap_or_default(),
            cloud_has_key: s.cloud.as_ref().is_some_and(|c| !c.api_key.is_empty()),
        }
    }

    /// 这一次用的模型客户端与调用参数。
    fn client(self: &Arc<Self>) -> Result<(Box<dyn LlmClient>, chronicle_gm::GmOptions), String> {
        match self.active() {
            "cloud" => {
                let c = self.settings.lock().map_err(|_| "内部错误".to_string())?.cloud.clone().ok_or("云端模型没有配置")?;
                let cfg = ProviderConfig {
                    id: format!("cloud-{}", self.room_id),
                    name: "云端 GM".into(),
                    base_url: c.base_url.clone(),
                    model: c.model.clone(),
                    server_kind: None,
                    needs_key: !c.api_key.is_empty(),
                    timeout_secs: c.timeout_secs,
                    capabilities: None,
                    allow_campaign_content: true,
                    thinking: chronicle_llm::ThinkingMode::Off,
                    service: Default::default(),
                    media: Default::default(),
                };
                let key = (!c.api_key.is_empty()).then(|| c.api_key.clone());
                let opts = options(c.context_length, c.max_tokens, c.temperature, true);
                Ok((Box::new(OpenAiClient::new(cfg, key)), opts))
            }
            "local" => Ok((Box::new(AgentLlm { gm: self.clone(), timeout: Duration::from_secs(240) }), options(32768, 4096, 0.8, true))),
            _ => Err("GM 没有就绪：房主的 GM Agent 不在线，也没有配置云端模型".into()),
        }
    }
}

pub fn normalize_mode(m: &str) -> String {
    match m {
        "cloud" | "local" | "auto" => m.to_string(),
        _ => "auto".to_string(),
    }
}

fn options(ctx: u32, max_tokens: u32, temperature: f32, json_schema: bool) -> chronicle_gm::GmOptions {
    chronicle_gm::GmOptions {
        json_schema,
        thinking: Some(false),
        temperature,
        max_tokens,
        context_budget: (ctx as usize / 3).clamp(1500, 8000),
        stream: false,
        web_search: None,
        dice: None,
    }
}

/// 把模型调用转给房主 Agent 的客户端：在阻塞线程里发任务、等结果（带超时、可取消）。
pub struct AgentLlm {
    gm: Arc<RoomGm>,
    timeout: Duration,
}

impl LlmClient for AgentLlm {
    fn chat(&self, req: &ChatRequest, cancel: &CancelToken, _p: Progress) -> chronicle_llm::Result<ChatResponse> {
        let started = std::time::Instant::now();
        let task_id = format!("t{}", self.gm.next_task.fetch_add(1, Ordering::Relaxed));
        let (tx, pending) = {
            let a = self.gm.agent.lock().map_err(|_| LlmError::Transport("内部错误".into()))?;
            let a = a.as_ref().ok_or_else(|| LlmError::Transport("房主的 GM Agent 已断开".into()))?;
            (a.tx.clone(), a.pending.clone())
        };
        let (rtx, rrx) = std::sync::mpsc::channel();
        pending.lock().map_err(|_| LlmError::Transport("内部错误".into()))?.insert(task_id.clone(), rtx);
        let schema = match &req.format {
            OutputFormat::JsonSchema { schema, .. } => Some(schema.clone()),
            OutputFormat::Text => None,
        };
        let task = AgentTask::Chat {
            task_id: task_id.clone(),
            messages: req.messages.iter().map(|m| ChatLine { role: format!("{:?}", m.role).to_lowercase(), content: m.content.clone() }).collect(),
            json_schema: schema.clone(),
            max_tokens: req.max_tokens.unwrap_or(2048),
            temperature: req.temperature.unwrap_or(0.8),
            timeout_secs: self.timeout.as_secs() as u32,
        };
        if tx.blocking_send(task).is_err() {
            pending.lock().ok().map(|mut p| p.remove(&task_id));
            return Err(LlmError::Transport("房主的 GM Agent 已断开".into()));
        }
        // 等结果：每 200 毫秒看一次是否被取消，总时长不超过超时。
        let deadline = started + self.timeout;
        let out = loop {
            if cancel.is_cancelled() {
                let _ = tx.blocking_send(AgentTask::Cancel { task_id: task_id.clone() });
                break Err(LlmError::Cancelled);
            }
            match rrx.recv_timeout(Duration::from_millis(200)) {
                Ok(Ok(content)) => break Ok(content),
                Ok(Err(e)) => break Err(LlmError::Transport(format!("房主本机模型出错：{e}"))),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if std::time::Instant::now() < deadline => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break Err(LlmError::Transport("房主的 GM Agent 超时没有回应".into())),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break Err(LlmError::Transport("房主的 GM Agent 已断开".into())),
            }
        };
        pending.lock().ok().map(|mut p| p.remove(&task_id));
        let content = out?;
        Ok(ChatResponse { content, finish_reason: Some("stop".into()), usage: Usage::default(), model: None, latency_ms: started.elapsed().as_millis() as u64, attempts: 1, used_json_schema: schema.is_some() })
    }

    fn describe(&self) -> (String, String) {
        ("房主 GM Agent".into(), self.gm.agent.lock().ok().and_then(|a| a.as_ref().map(|x| x.model.clone())).unwrap_or_default())
    }
}

impl GmBackend for Arc<RoomGm> {
    fn ready(&self) -> bool {
        self.active() != "none"
    }
    fn mode(&self) -> String {
        self.active().to_string()
    }
    /// 一个行动 = 一次 GM 回合（复用单人的回合流程：提示词、结构化方案、规则引擎校验、写库）。
    fn take_turn(&self, game: &GameRoom, a: &QueuedAction) -> Result<(), String> {
        let (client, opts) = self.client()?;
        let engine = chronicle_gm::Engine { gm: client.as_ref(), npc: client.as_ref(), templates: &self.templates, opts: &opts };
        let cancel = CancelToken::new();
        crate::multi::run_player_turn(game, &engine, a, &cancel).map(|_| ())
    }
}

/// 所有房间的 GM 节点。
pub struct GmHub {
    pub rooms: Mutex<HashMap<String, Arc<RoomGm>>>,
    pub templates: Arc<Templates>,
}

impl GmHub {
    pub fn new() -> Self {
        GmHub { rooms: Mutex::new(HashMap::new()), templates: Arc::new(Templates::builtin()) }
    }

    pub fn room(&self, room_id: &str, dir: &std::path::Path, default_mode: &str) -> Arc<RoomGm> {
        let mut m = self.rooms.lock().expect("GM 表被占用");
        m.entry(room_id.to_string()).or_insert_with(|| Arc::new(RoomGm::load(room_id, dir, default_mode, self.templates.clone()))).clone()
    }

    pub fn get(&self, room_id: &str) -> Option<Arc<RoomGm>> {
        self.rooms.lock().ok().and_then(|m| m.get(room_id).cloned())
    }
}

impl Default for GmHub {
    fn default() -> Self {
        Self::new()
    }
}

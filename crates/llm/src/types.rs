use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// 附带的图片（`data:image/...;base64,` URL）。有图片时按 OpenAI 兼容格式发送"文字 + 图片"的内容数组；
    /// 没有图片时请求与以前完全相同。需要服务支持看图（能力检测里的"看图"）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}

impl ChatMessage {
    pub fn system(s: impl Into<String>) -> Self {
        Self { role: Role::System, content: s.into(), images: vec![] }
    }
    pub fn user(s: impl Into<String>) -> Self {
        Self { role: Role::User, content: s.into(), images: vec![] }
    }
    pub fn assistant(s: impl Into<String>) -> Self {
        Self { role: Role::Assistant, content: s.into(), images: vec![] }
    }
    /// 带图片的用户消息。
    pub fn user_with_images(s: impl Into<String>, images: Vec<String>) -> Self {
        Self { role: Role::User, content: s.into(), images }
    }
    /// 发给服务的 JSON：没有图片时 `content` 是字符串，有图片时是内容数组。
    pub fn to_wire(&self) -> serde_json::Value {
        if self.images.is_empty() {
            return serde_json::json!({ "role": self.role, "content": self.content });
        }
        let mut parts = vec![serde_json::json!({ "type": "text", "text": self.content })];
        parts.extend(self.images.iter().map(|u| serde_json::json!({ "type": "image_url", "image_url": { "url": u } })));
        serde_json::json!({ "role": self.role, "content": parts })
    }
}

/// 结构化输出要求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputFormat {
    Text,
    /// 要求返回符合 schema 的 JSON。服务不支持时由调用方降级为提示词约束。
    JsonSchema { name: String, schema: Value },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    pub format: OutputFormat,
    /// 使用流式接收（可中途取消、可显示进度）。
    #[serde(default)]
    pub stream: bool,
    /// `Some(false)` 时请求关闭思考（通过 `chat_template_kwargs.enable_thinking`，
    /// llama-server 等支持该参数的服务生效；不支持的服务会忽略）。
    #[serde(default)]
    pub thinking: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Usage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    /// 首个内容 token 到达的时间（毫秒，从发出请求算起；只有流式请求能测到）。
    #[serde(default)]
    pub first_token_ms: Option<u64>,
    /// 服务返回的计时（llama-server 的 timings；其他服务通常没有）。
    #[serde(default)]
    pub prompt_ms: Option<f64>,
    #[serde(default)]
    pub prompt_per_second: Option<f64>,
    #[serde(default)]
    pub predicted_ms: Option<f64>,
    #[serde(default)]
    pub predicted_per_second: Option<f64>,
}

impl Usage {
    /// 读取 llama-server 的 `timings` 对象（没有就不动）。
    pub fn read_timings(&mut self, t: &serde_json::Value) {
        if !t.is_object() {
            return;
        }
        let f = |k: &str| t.get(k).and_then(serde_json::Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0);
        self.prompt_ms = f("prompt_ms").or(self.prompt_ms);
        self.prompt_per_second = f("prompt_per_second").or(self.prompt_per_second);
        self.predicted_ms = f("predicted_ms").or(self.predicted_ms);
        self.predicted_per_second = f("predicted_per_second").or(self.predicted_per_second);
        if self.prompt_tokens.is_none() {
            self.prompt_tokens = t.get("prompt_n").and_then(serde_json::Value::as_u64);
        }
        if self.completion_tokens.is_none() {
            self.completion_tokens = t.get("predicted_n").and_then(serde_json::Value::as_u64);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ChatResponse {
    pub content: String,
    pub finish_reason: Option<String>,
    pub usage: Usage,
    pub model: Option<String>,
    pub latency_ms: u64,
    pub attempts: u32,
    /// 是否实际以 JSON schema 方式请求。
    pub used_json_schema: bool,
}

/// 识别到的服务类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ServerKind {
    LlamaServer,
    Ollama,
    LmStudio,
    KoboldCpp,
    TextGenWebUi,
    Vllm,
    OpenAiCompatible,
    /// 语音合成服务（Breeze TTS），不能用于文本生成。
    BreezeTts,
}

impl ServerKind {
    pub fn label(self) -> &'static str {
        match self {
            ServerKind::LlamaServer => "llama.cpp (llama-server)",
            ServerKind::Ollama => "Ollama",
            ServerKind::LmStudio => "LM Studio",
            ServerKind::KoboldCpp => "KoboldCpp",
            ServerKind::TextGenWebUi => "text-generation-webui",
            ServerKind::Vllm => "vLLM",
            ServerKind::OpenAiCompatible => "OpenAI 兼容服务",
            ServerKind::BreezeTts => "Breeze TTS（语音）",
        }
    }
}

/// 实测得到的模型能力（不预设）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Capabilities {
    pub chat: bool,
    pub streaming: bool,
    pub json_schema: bool,
    /// 能看图（实测：发一张纯红色小图，模型认出了颜色）。旧的检测结果没有这一项。
    #[serde(default)]
    pub vision: bool,
    pub context_length: Option<u64>,
    pub tested_at: i64,
    /// 每项测试的说明（失败原因等）。
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    /// 例如 `http://127.0.0.1:8080/v1`。
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub server_kind: Option<ServerKind>,
    /// 是否需要 API Key（Key 本身存于系统凭据管理器，不在配置里）。
    #[serde(default)]
    pub needs_key: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub capabilities: Option<Capabilities>,
    /// 是否允许向该服务发送战役内容（隐私开关）。本机回环地址默认允许。
    #[serde(default)]
    pub allow_campaign_content: bool,
    /// 思考模式：模型默认，或请求时关闭思考（更快，内容可能简短一些）。
    #[serde(default)]
    pub thinking: ThinkingMode,
    /// 这个服务用来做什么：对话（默认）、语音合成或生成图片。旧配置没有这个字段，按对话处理。
    #[serde(default)]
    pub service: ServiceType,
    /// 语音合成与生成图片的选项（对话服务不用）。
    #[serde(default)]
    pub media: MediaOptions,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ServiceType {
    #[default]
    Chat,
    Speech,
    Image,
    /// 语音识别（语音输入）：OpenAI 兼容 `POST /audio/transcriptions`。
    Transcription,
}

/// 语音合成（OpenAI 兼容 `POST /audio/speech`）与生成图片（OpenAI 兼容 `POST /images/generations`）的选项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct MediaOptions {
    /// 旁白用的声音（例如 OpenAI 的 `alloy`）。为空时不发 `voice` 字段（Breeze 按描述设计声音）。
    #[serde(default)]
    pub voice: String,
    /// 可分给 NPC 的声音（每个 NPC 按人物 ID 固定选一个，听起来不会都一样）。
    #[serde(default)]
    pub voices: Vec<String>,
    /// 音频格式：wav 或 mp3（为空 = wav）。
    #[serde(default)]
    pub audio_format: String,
    /// 是否发送声音描述（`instructions`）。Breeze、gpt-4o-mini-tts 支持；不支持的服务可能报错，可以关掉。
    #[serde(default = "default_true")]
    pub send_instructions: bool,
    /// 图片尺寸，例如 `1024x1024`（为空 = 1024x1024）。
    #[serde(default)]
    pub image_size: String,
}

impl Default for MediaOptions {
    fn default() -> Self {
        Self { voice: String::new(), voices: vec![], audio_format: String::new(), send_instructions: true, image_size: String::new() }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ThinkingMode {
    #[default]
    Auto,
    Off,
}

fn default_timeout() -> u64 {
    600
}

impl ProviderConfig {
    pub fn is_loopback(&self) -> bool {
        let rest = self.base_url.split("://").nth(1).unwrap_or(&self.base_url);
        let host = rest.split(['/', ':']).next().unwrap_or("");
        let host = if rest.starts_with('[') { rest.split(']').next().unwrap_or("").trim_start_matches('[') } else { host };
        matches!(host, "127.0.0.1" | "localhost" | "::1") || host.ends_with(".localhost")
    }
}

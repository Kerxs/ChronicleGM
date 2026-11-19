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

    /// 服务没给计时（云端 API 通常只给 token 数）时，按 token 数和程序自己量的时间估算速度：
    /// 生成速度 = 输出 token ÷（总耗时 − 首字时间）；没有首字时间（非流式）就除以总耗时（会偏低一些）；
    /// 提示词速度 = 输入 token ÷ 首字时间（只有流式请求才有）。llama-server 自己给了计时就不动。
    pub fn estimate_speeds(&mut self, latency_ms: u64) {
        if self.predicted_per_second.is_none()
            && let Some(c) = self.completion_tokens.filter(|c| *c > 0)
        {
            let gen_ms = match self.first_token_ms {
                Some(f) if latency_ms > f => latency_ms - f,
                _ => latency_ms,
            };
            if gen_ms > 0 {
                self.predicted_per_second = Some(c as f64 / (gen_ms as f64 / 1000.0));
                self.predicted_ms.get_or_insert(gen_ms as f64);
            }
        }
        if self.prompt_per_second.is_none()
            && let (Some(p), Some(f)) = (self.prompt_tokens.filter(|p| *p > 0), self.first_token_ms.filter(|f| *f > 0))
        {
            self.prompt_per_second = Some(p as f64 / (f as f64 / 1000.0));
            self.prompt_ms.get_or_insert(f as f64);
        }
    }
}

#[cfg(test)]
mod speed_tests {
    use super::Usage;

    #[test]
    fn cloud_speeds_are_estimated_from_tokens_and_time() {
        // 流式：首字 0.8 秒，总共 4.8 秒，输出 200 token、输入 1600 token。
        let mut u = Usage { prompt_tokens: Some(1600), completion_tokens: Some(200), first_token_ms: Some(800), ..Default::default() };
        u.estimate_speeds(4800);
        assert_eq!(u.predicted_per_second.map(|v| v.round()), Some(50.0));
        assert_eq!(u.prompt_per_second.map(|v| v.round()), Some(2000.0));
        // 非流式：只有总耗时，生成速度按总耗时算，提示词速度拿不到。
        let mut u = Usage { prompt_tokens: Some(1600), completion_tokens: Some(200), ..Default::default() };
        u.estimate_speeds(5000);
        assert_eq!(u.predicted_per_second.map(|v| v.round()), Some(40.0));
        assert_eq!(u.prompt_per_second, None);
        // llama-server 自己给了计时：不覆盖。
        let mut u = Usage { completion_tokens: Some(200), predicted_per_second: Some(77.0), ..Default::default() };
        u.estimate_speeds(5000);
        assert_eq!(u.predicted_per_second, Some(77.0));
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
    /// OpenAI 兼容的语音合成服务（Kokoro-FastAPI、openai-edge-tts 等），不能用于文本生成。
    OpenAiSpeech,
    /// GPT-SoVITS 的 api_v2（`/tts`），不能用于文本生成。
    GptSovits,
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
            ServerKind::OpenAiSpeech => "OpenAI 兼容语音服务",
            ServerKind::GptSovits => "GPT-SoVITS（语音）",
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
    /// 语音接口：空 / `openai` = OpenAI 兼容 `/audio/speech`；`gpt_sovits` = GPT-SoVITS api_v2（`/tts`）。
    /// GPT-SoVITS 的声音写成「参考音频路径|参考音频里说的话|语言」。
    #[serde(default)]
    pub tts_api: String,
    /// 朗读文字的语言（GPT-SoVITS 的 text_lang；为空 = zh）。
    #[serde(default)]
    pub text_lang: String,
}

impl Default for MediaOptions {
    fn default() -> Self {
        Self { voice: String::new(), voices: vec![], audio_format: String::new(), send_instructions: true, image_size: String::new(), tts_api: String::new(), text_lang: String::new() }
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

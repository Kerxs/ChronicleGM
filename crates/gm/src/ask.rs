//! 带结构化输出的单次模型调用：JSON Schema（服务支持时）或提示词约束，解析失败时修正一次。

use chronicle_gen::RequestLog;
use chronicle_gen::parse::extract_json;
use chronicle_llm::{CancelToken, ChatMessage, ChatRequest, LlmClient, LlmError, OutputFormat};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;

use crate::{GmError, Result};

/// 联网搜索（程序提供；GM 方案里写了 web_search 时调用）。返回结果或失败原因。
pub type SearchFn = dyn Fn(&str) -> std::result::Result<Vec<chronicle_llm::search::SearchResult>, String> + Send + Sync;

#[derive(Clone)]
pub struct WebSearch(pub std::sync::Arc<SearchFn>);

impl std::fmt::Debug for WebSearch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WebSearch")
    }
}

/// 玩家亲手掷骰（程序提供；设置里选了"我来掷骰"时才有）：界面请玩家掷骰并返回点数，`None` 交给程序掷。
pub type DiceFn = dyn Fn(&chronicle_core::checks::DiceAsk) -> Option<Vec<Vec<u32>>> + Send + Sync;

#[derive(Clone)]
pub struct ManualDice(pub std::sync::Arc<DiceFn>);

impl std::fmt::Debug for ManualDice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ManualDice")
    }
}

#[derive(Debug, Clone)]
pub struct GmOptions {
    /// 服务支持 response_format json_schema。
    pub json_schema: bool,
    /// `Some(false)` 关闭思考（来自模型服务设置）。
    pub thinking: Option<bool>,
    pub temperature: f32,
    pub max_tokens: u32,
    /// 参考资料预算（token）。
    pub context_budget: usize,
    /// 流式请求（能测到首个 token 时间；服务支持流式时打开）。
    pub stream: bool,
    /// 联网搜索（默认没有；用户在设置里打开并允许 GM 搜索时才有）。
    pub web_search: Option<WebSearch>,
    /// 玩家亲手掷检定骰（默认没有：程序掷）。
    pub dice: Option<ManualDice>,
}

impl Default for GmOptions {
    fn default() -> Self {
        Self { json_schema: false, thinking: None, temperature: 0.8, max_tokens: 8192, context_budget: 6000, stream: false, web_search: None, dice: None }
    }
}

/// 发给模型的 JSON schema（所有字段必填、保持字段顺序，原因见 chronicle_gen::outputs::schema_of）。
pub fn schema_of<T: JsonSchema>() -> serde_json::Value {
    chronicle_gen::outputs::schema_of::<T>()
}

pub struct Answer<T> {
    pub value: T,
    pub raw: String,
}

/// 调用模型并解析为 `T`。请求记录无论成败都追加到 `logs`。
pub fn ask<T: DeserializeOwned + JsonSchema>(
    client: &dyn LlmClient,
    system: &str,
    user: &str,
    opts: &GmOptions,
    cancel: &CancelToken,
    logs: &mut Vec<RequestLog>,
) -> Result<Answer<T>> {
    ask_limited(client, system, user, opts, opts.max_tokens, cancel, logs)
}

/// 同 [`ask`]，并限制这次调用的输出长度。实测模型偶尔会停不下来，直到耗尽上限；
/// 按调用类型设置较小的上限，出问题时很快进入修正重试，而不是等上十分钟。
pub fn ask_limited<T: DeserializeOwned + JsonSchema>(
    client: &dyn LlmClient,
    system: &str,
    user: &str,
    opts: &GmOptions,
    limit: u32,
    cancel: &CancelToken,
    logs: &mut Vec<RequestLog>,
) -> Result<Answer<T>> {
    let schema = schema_of::<T>();
    let mut system = system.to_string();
    if !opts.json_schema {
        system.push_str(&format!("\n\n只输出一个 JSON 对象，必须符合下面的 JSON Schema：\n{schema}"));
    }
    let mut messages = vec![ChatMessage::system(system), ChatMessage::user(user)];
    let mut thinking = opts.thinking;
    for attempt in 0..2 {
        let req = ChatRequest {
            messages: messages.clone(),
            temperature: Some(opts.temperature),
            max_tokens: Some(limit.min(opts.max_tokens)),
            format: if opts.json_schema { OutputFormat::JsonSchema { name: "gm".into(), schema: schema.clone() } } else { OutputFormat::Text },
            stream: opts.stream,
            thinking,
        };
        let resp = match client.chat(&req, cancel, &mut |_| {}) {
            Ok(r) => {
                logs.push(RequestLog::from_response(&r));
                r
            }
            Err(e) => {
                logs.push(RequestLog { ok: false, attempts: 1, error: Some(e.to_string()), ..Default::default() });
                return Err(if matches!(e, LlmError::Cancelled) { GmError::Cancelled } else { GmError::Llm(e) });
            }
        };
        let truncated = resp.finish_reason.as_deref() == Some("length");
        let problem = match extract_json(&resp.content) {
            None => "没有找到完整的 JSON 对象".to_string(),
            Some(text) => match serde_json::from_str::<T>(&text) {
                Ok(v) => return Ok(Answer { value: v, raw: resp.content }),
                Err(e) => format!("JSON 结构不符合要求：{e}"),
            },
        };
        if attempt == 1 {
            return Err(GmError::Output(problem));
        }
        // 输出被长度上限截断时，修正请求关闭思考（实测思考型模型会把上限耗尽）。
        if truncated {
            thinking = Some(false);
        }
        messages.push(ChatMessage::assistant(resp.content.chars().take(4000).collect::<String>()));
        messages.push(ChatMessage::user(format!("你上一次的输出无法使用：{problem}。请只输出一个符合要求的 JSON 对象。")));
    }
    unreachable!()
}

//! OpenAI 兼容的对话接口客户端（llama-server / Ollama / LM Studio / vLLM / 云端均可）。

use std::io::{BufRead, BufReader, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::redact::redact;
use crate::types::*;
use crate::{LlmError, Result};

/// 取消令牌：设置后，正在进行的请求在下一个检查点中止。
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// 进度回调：已接收的字符数。
pub type Progress<'a> = &'a mut dyn FnMut(usize);

/// 抽象的对话接口，方便测试时替换为脚本化实现。
pub trait LlmClient: Send + Sync {
    fn chat(&self, req: &ChatRequest, cancel: &CancelToken, progress: Progress) -> Result<ChatResponse>;
    /// 用于日志与任务记录的描述（不含密钥）。
    fn describe(&self) -> (String, String);
}

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self { max_attempts: 3, base_delay: Duration::from_millis(500), max_delay: Duration::from_secs(8) }
    }
}

pub struct OpenAiClient {
    pub config: ProviderConfig,
    api_key: Option<String>,
    pub retry: RetryPolicy,
    agent: ureq::Agent,
}

pub(crate) fn make_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .timeout_connect(Some(Duration::from_secs(5)))
        .http_status_as_error(false)
        .build()
        .into()
}

pub(crate) fn join_url(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}

fn retryable_status(code: u16) -> bool {
    code == 408 || code == 429 || (500..=599).contains(&code)
}

impl OpenAiClient {
    pub fn new(config: ProviderConfig, api_key: Option<String>) -> Self {
        let agent = make_agent(Duration::from_secs(config.timeout_secs.max(5)));
        Self { config, api_key: api_key.filter(|k| !k.is_empty()), retry: RetryPolicy::default(), agent }
    }

    fn secrets(&self) -> Vec<&str> {
        self.api_key.iter().map(String::as_str).collect()
    }

    fn body(&self, req: &ChatRequest) -> Value {
        let mut b = json!({
            "model": self.config.model,
            "messages": req.messages.iter().map(ChatMessage::to_wire).collect::<Vec<_>>(),
            "stream": req.stream,
        });
        if req.stream {
            // 让服务在流式结束时附带 token 用量。
            b["stream_options"] = json!({"include_usage": true});
        }
        if let Some(t) = req.temperature {
            b["temperature"] = json!(t);
        }
        if let Some(m) = req.max_tokens {
            b["max_tokens"] = json!(m);
        }
        if req.thinking == Some(false) {
            b["chat_template_kwargs"] = json!({"enable_thinking": false});
        }
        if let OutputFormat::JsonSchema { name, schema } = &req.format {
            b["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {"name": name, "schema": schema, "strict": false}
            });
        }
        b
    }

    fn once(&self, req: &ChatRequest, cancel: &CancelToken, progress: Progress) -> Result<ChatResponse> {
        let url = join_url(&self.config.base_url, "chat/completions");
        let mut r = self.agent.post(&url).header("Content-Type", "application/json");
        if let Some(k) = &self.api_key {
            r = r.header("Authorization", &format!("Bearer {k}"));
        }
        let started = Instant::now();
        let body = serde_json::to_vec(&self.body(req)).map_err(|e| LlmError::Config(e.to_string()))?;
        let mut resp = r.send(&body[..]).map_err(|e| self.transport(e))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let mut text = String::new();
            let _ = resp.body_mut().as_reader().take(64 * 1024).read_to_string(&mut text);
            return Err(LlmError::Http { status, body: redact(&text, &self.secrets()) });
        }
        let used_json_schema = matches!(req.format, OutputFormat::JsonSchema { .. });
        let mut out = if req.stream {
            self.read_stream(resp.body_mut().as_reader(), cancel, progress, started)?
        } else {
            let mut text = String::new();
            resp.body_mut()
                .with_config()
                .limit(32 * 1024 * 1024)
                .reader()
                .read_to_string(&mut text)
                .map_err(|e| LlmError::Transport(e.to_string()))?;
            if cancel.is_cancelled() {
                return Err(LlmError::Cancelled);
            }
            let v: Value = serde_json::from_str(&text)
                .map_err(|e| LlmError::BadResponse(format!("响应不是 JSON: {e}")))?;
            let content = v["choices"][0]["message"]["content"]
                .as_str()
                .ok_or_else(|| LlmError::BadResponse("响应缺少 choices[0].message.content".into()))?
                .to_string();
            progress(content.chars().count());
            let mut usage = Usage { prompt_tokens: v["usage"]["prompt_tokens"].as_u64(), completion_tokens: v["usage"]["completion_tokens"].as_u64(), ..Default::default() };
            usage.read_timings(&v["timings"]);
            ChatResponse {
                content,
                finish_reason: v["choices"][0]["finish_reason"].as_str().map(str::to_string),
                usage,
                model: v["model"].as_str().map(str::to_string),
                latency_ms: 0,
                attempts: 1,
                used_json_schema,
            }
        };
        out.latency_ms = started.elapsed().as_millis() as u64;
        out.usage.estimate_speeds(out.latency_ms);
        out.used_json_schema = used_json_schema;
        Ok(out)
    }

    /// 解析 SSE：`data: {...}` 行，`data: [DONE]` 结束。
    fn read_stream(&self, reader: impl Read, cancel: &CancelToken, progress: Progress, started: Instant) -> Result<ChatResponse> {
        let mut content = String::new();
        let mut finish = None;
        let mut model = None;
        let mut usage = Usage::default();
        let mut saw_data = false;
        for line in BufReader::new(reader).lines() {
            if cancel.is_cancelled() {
                return Err(LlmError::Cancelled);
            }
            let line = line.map_err(|e| LlmError::Transport(e.to_string()))?;
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                break;
            }
            saw_data = true;
            let v: Value = serde_json::from_str(data)
                .map_err(|e| LlmError::BadResponse(format!("流式数据无法解析: {e}")))?;
            if let Some(err) = v.get("error") {
                return Err(LlmError::BadResponse(redact(&err.to_string(), &self.secrets())));
            }
            if let Some(delta) = v["choices"][0]["delta"]["content"].as_str() {
                if usage.first_token_ms.is_none() && !delta.is_empty() {
                    usage.first_token_ms = Some(started.elapsed().as_millis() as u64);
                }
                content.push_str(delta);
                progress(content.chars().count());
            }
            usage.read_timings(&v["timings"]);
            if let Some(f) = v["choices"][0]["finish_reason"].as_str() {
                finish = Some(f.to_string());
            }
            if model.is_none() {
                model = v["model"].as_str().map(str::to_string);
            }
            if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
                usage.prompt_tokens = u["prompt_tokens"].as_u64();
                usage.completion_tokens = u["completion_tokens"].as_u64();
            }
        }
        if !saw_data {
            return Err(LlmError::BadResponse("流式响应没有任何数据".into()));
        }
        Ok(ChatResponse { content, finish_reason: finish, usage, model, latency_ms: 0, attempts: 1, used_json_schema: false })
    }

    fn transport(&self, e: ureq::Error) -> LlmError {
        match e {
            ureq::Error::Timeout(_) => LlmError::Timeout,
            other => LlmError::Transport(redact(&other.to_string(), &self.secrets())),
        }
    }
}

impl LlmClient for OpenAiClient {
    fn chat(&self, req: &ChatRequest, cancel: &CancelToken, progress: Progress) -> Result<ChatResponse> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            if cancel.is_cancelled() {
                return Err(LlmError::Cancelled);
            }
            match self.once(req, cancel, progress) {
                Ok(mut r) => {
                    r.attempts = attempt;
                    return Ok(r);
                }
                Err(e) => {
                    let retry = match &e {
                        LlmError::Http { status, .. } => retryable_status(*status),
                        LlmError::Transport(_) | LlmError::Timeout => true,
                        _ => false,
                    };
                    if !retry || attempt >= self.retry.max_attempts {
                        return Err(e);
                    }
                    let delay = (self.retry.base_delay * 2u32.pow(attempt - 1)).min(self.retry.max_delay);
                    let until = Instant::now() + delay;
                    while Instant::now() < until {
                        if cancel.is_cancelled() {
                            return Err(LlmError::Cancelled);
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        }
    }

    fn describe(&self) -> (String, String) {
        (self.config.name.clone(), self.config.model.clone())
    }
}

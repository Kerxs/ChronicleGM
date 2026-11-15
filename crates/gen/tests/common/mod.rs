//! 测试共用：脚本化的 mock 模型（按顺序返回预设文本，不代表真实模型）。
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::Mutex;

use chronicle_llm::*;

pub struct Scripted {
    replies: Mutex<VecDeque<std::result::Result<String, LlmError>>>,
    seen: Mutex<Vec<ChatRequest>>,
    pub reject_schema: bool,
}

impl Scripted {
    pub fn new(replies: Vec<std::result::Result<String, LlmError>>) -> Self {
        Self { replies: Mutex::new(replies.into()), seen: Mutex::default(), reject_schema: false }
    }
    pub fn ok(v: serde_json::Value) -> Self {
        Self::new(vec![Ok(v.to_string())])
    }
    pub fn requests(&self) -> Vec<ChatRequest> {
        self.seen.lock().unwrap().clone()
    }
}

impl LlmClient for Scripted {
    fn chat(&self, req: &ChatRequest, cancel: &CancelToken, progress: client::Progress) -> chronicle_llm::Result<ChatResponse> {
        self.seen.lock().unwrap().push(req.clone());
        if cancel.is_cancelled() {
            return Err(LlmError::Cancelled);
        }
        if self.reject_schema && matches!(req.format, OutputFormat::JsonSchema { .. }) {
            return Err(LlmError::Http { status: 400, body: "unsupported response_format".into() });
        }
        let r = self.replies.lock().unwrap().pop_front().unwrap_or(Err(LlmError::Transport("mock 没有更多回复".into())))?;
        progress(r.chars().count());
        // 以 "__LENGTH__" 开头的回复模拟"输出达到长度上限被截断"。
        let (r, finish) = match r.strip_prefix("__LENGTH__") {
            Some(rest) => (rest.to_string(), "length"),
            None => (r, "stop"),
        };
        Ok(ChatResponse {
            content: r,
            finish_reason: Some(finish.into()),
            usage: Usage { prompt_tokens: Some(100), completion_tokens: Some(50), ..Default::default() },
            model: Some("mock".into()),
            latency_ms: 1,
            attempts: 1,
            used_json_schema: false,
        })
    }
    fn describe(&self) -> (String, String) {
        ("mock".into(), "mock-model".into())
    }
}



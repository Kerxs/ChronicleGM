//! 注意：以下测试全部使用本地 mock 服务器，只验证客户端逻辑，不代表真实模型服务的行为。

mod mock_server;

use std::time::Duration;

use chronicle_llm::discovery::{probe, test_capabilities};
use chronicle_llm::*;
use mock_server::*;
use serde_json::json;

fn config(url: &str) -> ProviderConfig {
    ProviderConfig {
        id: "p1".into(),
        name: "mock".into(),
        base_url: format!("{url}/v1"),
        model: "mock-model".into(),
        server_kind: None,
        needs_key: false,
        timeout_secs: 10,
        capabilities: None,
        allow_campaign_content: true,
        thinking: Default::default(),
        service: Default::default(),
        media: Default::default(),
    }
}

fn fast_retry(mut c: OpenAiClient) -> OpenAiClient {
    c.retry = RetryPolicy { max_attempts: 3, base_delay: Duration::from_millis(10), max_delay: Duration::from_millis(20) };
    c
}

fn req(stream: bool) -> ChatRequest {
    ChatRequest {
        messages: vec![ChatMessage::user("你好")],
        temperature: Some(0.2),
        max_tokens: Some(64),
        format: OutputFormat::Text,
        stream,
        thinking: None,
    }
}

#[test]
fn plain_and_streaming_completion() {
    let srv = MockServer::start(|r, _| {
        if r.json()["stream"] == json!(true) { stream(&["你", "好", "！"], Duration::ZERO) } else { completion("你好！") }
    });
    let c = OpenAiClient::new(config(&srv.url), None);
    let r = c.chat(&req(false), &CancelToken::new(), &mut |_| {}).unwrap();
    assert_eq!(r.content, "你好！");
    assert_eq!(r.usage.completion_tokens, Some(5));
    let mut seen = 0;
    let r = c.chat(&req(true), &CancelToken::new(), &mut |n| seen = n).unwrap();
    assert_eq!(r.content, "你好！");
    assert_eq!(r.finish_reason.as_deref(), Some("stop"));
    assert_eq!(seen, 3);
    let body = srv.requests.lock().unwrap()[0].json();
    assert_eq!(body["model"], "mock-model");
    assert_eq!(body["messages"][0]["role"], "user");
}

#[test]
fn thinking_off_is_sent_only_when_requested() {
    let srv = MockServer::start(|_, _| completion("好"));
    let c = OpenAiClient::new(config(&srv.url), None);
    c.chat(&req(false), &CancelToken::new(), &mut |_| {}).unwrap();
    let mut r = req(false);
    r.thinking = Some(false);
    c.chat(&r, &CancelToken::new(), &mut |_| {}).unwrap();
    let reqs = srv.requests.lock().unwrap();
    assert!(reqs[0].json().get("chat_template_kwargs").is_none());
    assert_eq!(reqs[1].json()["chat_template_kwargs"]["enable_thinking"], json!(false));
}

#[test]
fn retries_on_server_error_but_not_on_client_error() {
    let srv = MockServer::start(|_, n| if n < 2 { Reply::Text(500, "boom".into()) } else { completion("ok") });
    let c = fast_retry(OpenAiClient::new(config(&srv.url), None));
    let r = c.chat(&req(false), &CancelToken::new(), &mut |_| {}).unwrap();
    assert_eq!(r.attempts, 2);

    let srv = MockServer::start(|_, _| Reply::Text(400, "bad request".into()));
    let c = fast_retry(OpenAiClient::new(config(&srv.url), None));
    let e = c.chat(&req(false), &CancelToken::new(), &mut |_| {}).unwrap_err();
    assert!(matches!(e, LlmError::Http { status: 400, .. }));
    assert_eq!(srv.count(), 1);
}

#[test]
fn api_key_sent_but_never_leaked_in_errors() {
    let srv = MockServer::start(|r, _| {
        let auth = r.header("Authorization").unwrap_or("").to_string();
        Reply::Text(401, format!("invalid credentials: {auth}"))
    });
    let c = OpenAiClient::new(config(&srv.url), Some("SECRET-KEY-123456".into()));
    let e = c.chat(&req(false), &CancelToken::new(), &mut |_| {}).unwrap_err();
    let msg = e.to_string();
    assert!(msg.contains("401"));
    assert!(!msg.contains("SECRET-KEY-123456"), "{msg}");
    assert_eq!(srv.requests.lock().unwrap()[0].header("Authorization"), Some("Bearer SECRET-KEY-123456"));
}

#[test]
fn cancel_during_stream() {
    let parts: Vec<&str> = std::iter::repeat_n("字", 200).collect();
    let srv = MockServer::start(move |_, _| stream(&parts, Duration::from_millis(10)));
    let c = OpenAiClient::new(config(&srv.url), None);
    let cancel = CancelToken::new();
    let c2 = cancel.clone();
    let e = c.chat(&req(true), &cancel, &mut |n| if n >= 5 { c2.cancel() }).unwrap_err();
    assert!(matches!(e, LlmError::Cancelled));
}

#[test]
fn unreachable_service_reports_transport_error() {
    let c = fast_retry(OpenAiClient::new(config("http://127.0.0.1:9"), None));
    let e = c.chat(&req(false), &CancelToken::new(), &mut |_| {}).unwrap_err();
    assert!(matches!(e, LlmError::Transport(_) | LlmError::Timeout), "{e:?}");
}

#[test]
fn discovery_identifies_service_types() {
    let t = Duration::from_millis(500);
    let llama = MockServer::start(|r, _| match r.path.as_str() {
        "/props" => Reply::Json(200, json!({"default_generation_settings": {"n_ctx": 32768}, "model_path": "C:\\models\\qwen.gguf"})),
        "/v1/models" => Reply::Json(200, json!({"data": [{"id": "qwen.gguf", "owned_by": "llamacpp", "meta": {"n_ctx_train": 262144}}]})),
        _ => not_found(),
    });
    let s = probe(&llama.url, None, t).unwrap();
    assert_eq!(s.kind, ServerKind::LlamaServer);
    assert_eq!(s.models[0].context_length, Some(32768));
    assert_eq!(s.base_url, format!("{}/v1", llama.url));

    let ollama = MockServer::start(|r, _| match r.path.as_str() {
        "/api/tags" => Reply::Json(200, json!({"models": [{"name": "qwen3:8b"}, {"name": "llama3:8b"}]})),
        _ => not_found(),
    });
    let s = probe(&ollama.url, None, t).unwrap();
    assert_eq!(s.kind, ServerKind::Ollama);
    assert_eq!(s.models.len(), 2);

    let lms = MockServer::start(|r, _| match r.path.as_str() {
        "/v1/models" => Reply::Json(200, json!({"data": [{"id": "qwen3-8b", "owned_by": "organization_owner"}]})),
        _ => not_found(),
    });
    let s = probe(&lms.url, Some(ServerKind::LmStudio), t).unwrap();
    assert_eq!(s.kind, ServerKind::LmStudio);
    // 输入带 /v1 也能识别。
    assert!(probe(&format!("{}/v1", lms.url), None, t).is_some());

    // Breeze TTS（接口形态取自其 docs/server.md）：识别为语音服务，不可用于文本生成。
    let tts = MockServer::start(|r, _| match r.path.as_str() {
        "/health" => Reply::Json(200, json!({"status": "ok", "sample_rate": 24000, "ws_port": 8138})),
        _ => not_found(),
    });
    let s = probe(&tts.url, None, t).unwrap();
    assert_eq!(s.kind, ServerKind::BreezeTts);
    assert!(!s.usable_for_chat);
    assert!(probe(&llama.url, None, t).unwrap().usable_for_chat);

    let other = MockServer::start(|_, _| not_found());
    assert!(probe(&other.url, None, t).is_none());
    assert!(probe("http://127.0.0.1:9", None, t).is_none());
}

#[test]
fn capability_probe_reports_real_results() {
    // 支持 JSON schema 的服务。
    let good = MockServer::start(|r, _| {
        let b = r.json();
        if b["stream"] == json!(true) {
            stream(&["收", "到"], Duration::ZERO)
        } else if b.get("response_format").is_some() {
            completion("<think>好的</think>{\"answer\":\"好\",\"count\":3}")
        } else {
            completion("收到")
        }
    });
    let caps = test_capabilities(&OpenAiClient::new(config(&good.url), None), Some(8192), 1);
    assert!(caps.chat && caps.streaming && caps.json_schema, "{caps:?}");
    assert_eq!(caps.context_length, Some(8192));

    // 不支持 response_format 的服务：返回 400。
    let plain = MockServer::start(|r, _| {
        let b = r.json();
        if b.get("response_format").is_some() {
            Reply::Text(400, "response_format not supported".into())
        } else if b["stream"] == json!(true) {
            stream(&["收到"], Duration::ZERO)
        } else {
            completion("收到")
        }
    });
    let caps = test_capabilities(&OpenAiClient::new(config(&plain.url), None), None, 1);
    assert!(caps.chat && caps.streaming && !caps.json_schema);
    assert!(caps.notes.iter().any(|n| n.contains("HTTP 400")));

    // 对话返回空内容（思考占满 token）：如实报告。
    let empty = MockServer::start(|_, _| completion("<think>想了很久"));
    let caps = test_capabilities(&OpenAiClient::new(config(&empty.url), None), None, 1);
    assert!(!caps.chat);
    assert!(caps.notes[0].contains("为空"));
}

#[test]
fn loopback_detection() {
    let mut c = config("http://127.0.0.1:8080");
    assert!(c.is_loopback());
    c.base_url = "http://localhost:11434/v1".into();
    assert!(c.is_loopback());
    c.base_url = "https://api.example.com/v1".into();
    assert!(!c.is_loopback());
    c.base_url = "http://127.0.0.1.evil.com/v1".into();
    assert!(!c.is_loopback());
}

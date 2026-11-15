//! 自动识别本机正在运行的模型服务，并实测其能力。
//!
//! 只扫描 127.0.0.1 上的常见端口（不扫描局域网），每个请求超时很短。
//! 用户手动填写的地址也走同样的识别流程。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{CancelToken, LlmClient, OpenAiClient, join_url, make_agent};
use crate::types::*;
use crate::{LlmError, strip_thinking};

/// 常见本地服务端口及其默认推测类型。
pub const DEFAULT_PORTS: &[(u16, ServerKind)] = &[
    (8080, ServerKind::LlamaServer),
    (11434, ServerKind::Ollama),
    (1234, ServerKind::LmStudio),
    (5001, ServerKind::KoboldCpp),
    (5000, ServerKind::TextGenWebUi),
    (8000, ServerKind::Vllm),
    (8137, ServerKind::BreezeTts),
    // 语音合成：Kokoro-FastAPI、openai-edge-tts、GPT-SoVITS api_v2 的默认端口（按各项目文档）。
    (8880, ServerKind::OpenAiSpeech),
    (5050, ServerKind::OpenAiSpeech),
    (9880, ServerKind::GptSovits),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct DiscoveredModel {
    pub id: String,
    pub context_length: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct DiscoveredService {
    /// 服务根地址，如 `http://127.0.0.1:8080`。
    pub root_url: String,
    /// OpenAI 兼容接口地址，如 `http://127.0.0.1:8080/v1`。
    pub base_url: String,
    pub kind: ServerKind,
    pub models: Vec<DiscoveredModel>,
    /// 能否用于文本生成（TTS 等服务为 false）。
    pub usable_for_chat: bool,
    /// 语音服务提供的声音（读得到时）。
    #[serde(default)]
    pub voices: Vec<String>,
}

fn get_json(agent: &ureq::Agent, url: &str) -> Option<Value> {
    let mut resp = agent.get(url).call().ok()?;
    if resp.status().as_u16() != 200 {
        return None;
    }
    let text = resp.body_mut().with_config().limit(4 * 1024 * 1024).read_to_string().ok()?;
    serde_json::from_str(&text).ok()
}

/// 识别某个根地址上的服务；不是模型服务时返回 None。
pub fn probe(root_url: &str, hint: Option<ServerKind>, timeout: Duration) -> Option<DiscoveredService> {
    let root = root_url.trim_end_matches('/').trim_end_matches("/v1").to_string();
    let agent = make_agent(timeout);
    let base_url = format!("{root}/v1");

    // Ollama：/api/tags
    if let Some(v) = get_json(&agent, &join_url(&root, "api/tags"))
        && let Some(list) = v["models"].as_array()
    {
        let models = list
            .iter()
            .filter_map(|m| m["name"].as_str().or(m["model"].as_str()))
            .map(|id| DiscoveredModel { id: id.to_string(), context_length: None })
            .collect();
        return Some(DiscoveredService { root_url: root, base_url, kind: ServerKind::Ollama, models, usable_for_chat: true, voices: vec![] });
    }

    // Breeze TTS：/health 返回采样率。
    if let Some(h) = get_json(&agent, &join_url(&root, "health"))
        && h.get("sample_rate").is_some()
    {
        return Some(DiscoveredService { root_url: root, base_url, kind: ServerKind::BreezeTts, models: vec![], usable_for_chat: false, voices: vec![] });
    }

    // Kokoro-FastAPI 等：`GET /v1/audio/voices` 返回 `{"voices": [...]}`。
    if let Some(v) = get_json(&agent, &join_url(&base_url, "audio/voices"))
        && v.get("voices").is_some_and(Value::is_array)
    {
        let voices = parse_voice_list(&v);
        let models = get_json(&agent, &join_url(&base_url, "models"))
            .and_then(|m| m["data"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|m| m["id"].as_str().map(|id| DiscoveredModel { id: id.to_string(), context_length: None }))
            .collect();
        return Some(DiscoveredService { root_url: root, base_url, kind: ServerKind::OpenAiSpeech, models, usable_for_chat: false, voices });
    }

    // GPT-SoVITS api_v2：`GET /tts` 不带参数时返回 400 和 `{"message": "... is required"}`（源码里的参数检查），不会合成。
    if (hint == Some(ServerKind::GptSovits) || hint.is_none())
        && let Ok(mut r) = agent.get(&join_url(&root, "tts")).call()
        && r.status().as_u16() == 400
        && let Ok(t) = r.body_mut().with_config().limit(64 * 1024).read_to_string()
        && let Ok(v) = serde_json::from_str::<Value>(&t)
        && v["message"].as_str().is_some_and(|m| m.contains("required"))
    {
        return Some(DiscoveredService { root_url: root, base_url, kind: ServerKind::GptSovits, models: vec![], usable_for_chat: false, voices: vec![] });
    }

    // llama-server：/props
    let props = get_json(&agent, &join_url(&root, "props")).filter(|v| {
        v.get("default_generation_settings").is_some() || v.get("model_path").is_some() || v.get("n_ctx").is_some()
    });
    let llama_ctx = props.as_ref().and_then(|v| {
        v["default_generation_settings"]["n_ctx"].as_u64().or_else(|| v["n_ctx"].as_u64())
    });

    // OpenAI 兼容：/v1/models
    let models_json = get_json(&agent, &join_url(&base_url, "models"));
    let data = models_json.as_ref().and_then(|v| v["data"].as_array().cloned());
    if props.is_none() && data.is_none() {
        return None;
    }
    let mut models: Vec<DiscoveredModel> = data
        .unwrap_or_default()
        .iter()
        .filter_map(|m| {
            let id = m["id"].as_str()?;
            // 优先使用服务实际开启的上下文（llama-server 的 n_ctx），其次才是训练长度。
            let ctx = m["max_model_len"].as_u64().or(llama_ctx).or_else(|| m["meta"]["n_ctx_train"].as_u64());
            Some(DiscoveredModel { id: id.to_string(), context_length: ctx })
        })
        .collect();
    if models.is_empty()
        && let Some(p) = &props
    {
        let id = p["model_alias"]
            .as_str()
            .or(p["model_path"].as_str())
            .map(|s| s.rsplit(['/', '\\']).next().unwrap_or(s).to_string())
            .unwrap_or_else(|| "default".into());
        models.push(DiscoveredModel { id, context_length: llama_ctx });
    }
    let owned_by = models_json
        .as_ref()
        .and_then(|v| v["data"][0]["owned_by"].as_str().map(str::to_string))
        .unwrap_or_default();
    let kind = if props.is_some() {
        ServerKind::LlamaServer
    } else if owned_by == "vllm" {
        ServerKind::Vllm
    } else if owned_by.eq_ignore_ascii_case("koboldcpp") {
        ServerKind::KoboldCpp
    } else {
        match hint {
            Some(ServerKind::LlamaServer) | Some(ServerKind::Ollama) | None => ServerKind::OpenAiCompatible,
            Some(k) => k,
        }
    };
    // 模型列表里全是语音模型（例如 openai-edge-tts 的 tts-1 / tts-1-hd）：是语音合成服务，不能用来生成文字。
    if props.is_none() && !models.is_empty() && models.iter().all(|m| is_speech_model(&m.id)) {
        return Some(DiscoveredService { root_url: root, base_url, kind: ServerKind::OpenAiSpeech, models, usable_for_chat: false, voices: vec![] });
    }
    Some(DiscoveredService { root_url: root, base_url, kind, models, usable_for_chat: true, voices: vec![] })
}

/// 模型名看起来是语音合成模型。
pub fn is_speech_model(id: &str) -> bool {
    let l = id.to_ascii_lowercase();
    l.starts_with("tts") || l.contains("-tts") || l.contains("_tts") || l.contains("kokoro") || l.contains("speech") || l.contains("sovits")
}

/// 从声音列表的 JSON 里取出声音名：`{"voices": [...]}` 或直接是数组；元素是字符串，或带 id / name / ShortName / voice_id 的对象。
pub fn parse_voice_list(v: &Value) -> Vec<String> {
    let arr = v.get("voices").and_then(Value::as_array).or_else(|| v.as_array()).cloned().unwrap_or_default();
    let mut out: Vec<String> = arr
        .iter()
        .filter_map(|x| match x {
            Value::String(s) => Some(s.clone()),
            Value::Object(o) => ["id", "ShortName", "short_name", "voice_id", "name"].iter().find_map(|k| o.get(*k).and_then(Value::as_str)).map(str::to_string),
            _ => None,
        })
        .filter(|s| !s.trim().is_empty())
        .collect();
    out.dedup();
    out
}

/// 读取语音服务的声音列表（不合成，不计费）：依次试 `/audio/voices`（Kokoro-FastAPI）和 `/voices/all`（openai-edge-tts）。
pub fn list_voices(base_url: &str, api_key: Option<&str>, timeout: Duration) -> Result<Vec<String>, LlmError> {
    let base = if base_url.trim_end_matches('/').ends_with("/v1") { base_url.trim_end_matches('/').to_string() } else { format!("{}/v1", base_url.trim_end_matches('/').trim_end_matches("/v1")) };
    let agent = make_agent(timeout);
    let mut last = String::new();
    for path in ["audio/voices", "voices/all"] {
        let mut req = agent.get(&join_url(&base, path));
        if let Some(k) = api_key.filter(|k| !k.is_empty()) {
            req = req.header("Authorization", &format!("Bearer {k}"));
        }
        match req.call() {
            Ok(mut r) if r.status().as_u16() == 200 => {
                let t = r.body_mut().with_config().limit(8 * 1024 * 1024).read_to_string().map_err(|e| LlmError::Transport(e.to_string()))?;
                if let Ok(v) = serde_json::from_str::<Value>(&t) {
                    let list = parse_voice_list(&v);
                    if !list.is_empty() {
                        return Ok(list);
                    }
                }
            }
            Ok(r) => last = format!("{path}：HTTP {}", r.status().as_u16()),
            Err(e) => last = format!("{path}：{e}"),
        }
    }
    Err(LlmError::Transport(if last.is_empty() { "服务没有提供声音列表".into() } else { format!("服务没有提供声音列表（{last}）") }))
}

/// 并行扫描默认端口与用户追加的地址。
pub fn discover(extra_urls: &[String], timeout: Duration) -> Vec<DiscoveredService> {
    let mut targets: Vec<(String, Option<ServerKind>)> =
        DEFAULT_PORTS.iter().map(|(p, k)| (format!("http://127.0.0.1:{p}"), Some(*k))).collect();
    for u in extra_urls {
        let u = u.trim().trim_end_matches('/').to_string();
        if !u.is_empty() && !targets.iter().any(|(t, _)| *t == u) {
            targets.push((u, None));
        }
    }
    let mut found: Vec<DiscoveredService> = std::thread::scope(|s| {
        let handles: Vec<_> = targets.iter().map(|(u, k)| s.spawn(move || probe(u, *k, timeout))).collect();
        handles.into_iter().filter_map(|h| h.join().ok().flatten()).collect()
    });
    found.sort_by(|a, b| a.root_url.cmp(&b.root_url));
    found.dedup_by(|a, b| a.root_url == b.root_url);
    found
}

/// Ollama 模型的上下文长度：POST /api/show。
fn ollama_context(root: &str, model: &str, timeout: Duration) -> Option<u64> {
    let agent = make_agent(timeout);
    let body = serde_json::to_vec(&json!({"model": model})).ok()?;
    let mut resp = agent.post(&join_url(root, "api/show")).header("Content-Type", "application/json").send(&body[..]).ok()?;
    if resp.status().as_u16() != 200 {
        return None;
    }
    let v: Value = serde_json::from_str(&resp.body_mut().read_to_string().ok()?).ok()?;
    v["model_info"].as_object()?.iter().find(|(k, _)| k.ends_with(".context_length")).and_then(|(_, v)| v.as_u64())
}

/// 对某个服务上的某个模型实测能力：普通对话、流式、JSON schema 输出、上下文长度。
/// 每一项都以真实请求结果为准；失败原因写入 `notes`。
pub fn test_capabilities(client: &OpenAiClient, discovered_ctx: Option<u64>, now_ms: i64) -> Capabilities {
    let cancel = CancelToken::new();
    let mut caps = Capabilities { tested_at: now_ms, context_length: discovered_ctx, ..Default::default() };
    // 按配置的思考模式测试，结果与实际使用一致。
    let off = (client.config.thinking == ThinkingMode::Off).then_some(false);
    let ping = |stream: bool| ChatRequest {
        messages: vec![ChatMessage::user("请只回复两个字：收到")],
        temperature: Some(0.0),
        max_tokens: Some(1024),
        format: OutputFormat::Text,
        stream,
        thinking: off,
    };
    match client.chat(&ping(false), &cancel, &mut |_| {}) {
        Ok(r) if !strip_thinking(&r.content).trim().is_empty() => caps.chat = true,
        Ok(r) => caps.notes.push(format!(
            "对话：返回内容为空（finish_reason = {:?}，可能是思考过程占满了 token）",
            r.finish_reason
        )),
        Err(e) => caps.notes.push(format!("对话：{e}")),
    }
    if !caps.chat {
        return caps;
    }
    match client.chat(&ping(true), &cancel, &mut |_| {}) {
        Ok(r) if !strip_thinking(&r.content).trim().is_empty() => caps.streaming = true,
        Ok(_) => caps.notes.push("流式：没有收到内容".into()),
        Err(e) => caps.notes.push(format!("流式：{e}")),
    }
    let schema = json!({
        "type": "object",
        "properties": {"answer": {"type": "string"}, "count": {"type": "integer"}},
        "required": ["answer", "count"]
    });
    let req = ChatRequest {
        messages: vec![ChatMessage::user("用 JSON 回答：answer 填“好”，count 填 3。")],
        temperature: Some(0.0),
        max_tokens: Some(1024),
        format: OutputFormat::JsonSchema { name: "probe".into(), schema },
        stream: false,
        thinking: off,
    };
    match client.chat(&req, &cancel, &mut |_| {}) {
        Ok(r) => match serde_json::from_str::<Value>(strip_thinking(&r.content).trim()) {
            Ok(v) if v["answer"].is_string() && v["count"].is_i64() => caps.json_schema = true,
            Ok(v) => caps.notes.push(format!("JSON schema：返回的 JSON 不符合要求：{}", truncate(&v.to_string(), 200))),
            Err(_) => caps.notes.push(format!("JSON schema：返回的不是纯 JSON：{}", truncate(&r.content, 200))),
        },
        Err(LlmError::Http { status, .. }) => caps.notes.push(format!("JSON schema：服务不支持（HTTP {status}）")),
        Err(e) => caps.notes.push(format!("JSON schema：{e}")),
    }
    // 看图：一张 16×16 的纯红色 PNG，问颜色。服务不支持图片时一般直接返回 HTTP 错误。
    let red = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAF0lEQVR4nGN4ICBAEmIY1TCqQWDYagAA6ggAEK2bvLwAAAAASUVORK5CYII=";
    let req = ChatRequest {
        messages: vec![ChatMessage::user_with_images("这张图片是什么颜色？只回答一个颜色词。", vec![red.into()])],
        temperature: Some(0.0),
        max_tokens: Some(256),
        format: OutputFormat::Text,
        stream: false,
        thinking: off,
    };
    match client.chat(&req, &cancel, &mut |_| {}) {
        Ok(r) => {
            let a = strip_thinking(&r.content).to_lowercase();
            if a.contains('红') || a.contains("red") {
                caps.vision = true;
            } else {
                caps.notes.push(format!("看图：没有认出图片颜色（回答：{}）", truncate(a.trim(), 60)));
            }
        }
        Err(LlmError::Http { status, .. }) => caps.notes.push(format!("看图：服务不支持图片输入（HTTP {status}）")),
        Err(e) => caps.notes.push(format!("看图：{e}")),
    }
    if caps.context_length.is_none() && client.config.server_kind == Some(ServerKind::Ollama) {
        let root = client.config.base_url.trim_end_matches('/').trim_end_matches("/v1");
        caps.context_length = ollama_context(root, &client.config.model, Duration::from_secs(5));
    }
    caps
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n).collect::<String>() + "…" }
}

/// 云端或本机服务的模型列表（`GET {base}/models`，带 API Key）。只读，不计费。
pub fn list_models(base_url: &str, api_key: Option<&str>, timeout: Duration) -> Result<Vec<String>, crate::LlmError> {
    let url = crate::tts::api_url(base_url, "models");
    let mut req = crate::client::make_agent(timeout).get(&url);
    if let Some(k) = api_key.filter(|k| !k.is_empty()) {
        req = req.header("Authorization", &format!("Bearer {k}"));
    }
    let mut resp = req.call().map_err(|e| crate::LlmError::Transport(e.to_string()))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().with_config().limit(8 * 1024 * 1024).read_to_string().map_err(|e| crate::LlmError::Transport(e.to_string()))?;
    if !(200..300).contains(&status) {
        return Err(crate::LlmError::Http { status, body: text.chars().take(2000).collect() });
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| crate::LlmError::Transport(format!("模型列表不是 JSON：{e}")))?;
    let mut ids: Vec<String> = v["data"].as_array().or_else(|| v["models"].as_array()).map(|a| a.iter().filter_map(|m| m["id"].as_str().or_else(|| m["name"].as_str()).map(str::to_string)).collect()).unwrap_or_default();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// llama-server 的服务端分词：返回 token 数。服务不支持（不是 llama-server、请求失败）时返回 None。
/// 地址只需要到端口（/tokenize 在根路径；写了 /v1 也会去掉）。
pub fn tokenize_count(base_url: &str, text: &str, timeout: Duration) -> Option<usize> {
    let root = base_url.trim_end_matches('/').trim_end_matches("/v1");
    let agent = make_agent(timeout);
    let body = serde_json::to_vec(&json!({ "content": text, "add_special": false })).ok()?;
    let mut resp = agent.post(&format!("{root}/tokenize")).header("Content-Type", "application/json").send(&body[..]).ok()?;
    if !(200..300).contains(&resp.status().as_u16()) {
        return None;
    }
    let text = resp.body_mut().with_config().limit(64 * 1024 * 1024).read_to_string().ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("tokens").and_then(Value::as_array).map(|a| a.len())
}

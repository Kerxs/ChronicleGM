//! 语音输入：OpenAI 兼容的 `POST /audio/transcriptions`（本机 whisper 服务或云端）。
//! 请求是 multipart/form-data：`file`（录音）、`model`、`language`、`response_format=json`；返回 `{"text": "..."}`。
//! 没有真实服务的测试只用本地模拟服务器验证请求格式与解析。

use std::time::Duration;

use serde_json::Value;

use crate::LlmError;
use crate::client::make_agent;
use crate::redact::redact;
use crate::tts::api_url;

#[derive(Debug, Clone, Default)]
pub struct TranscribeTarget {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
}

fn ext_of(mime: &str) -> &'static str {
    match mime.split(';').next().unwrap_or("").trim() {
        "audio/webm" => "webm",
        "audio/ogg" => "ogg",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => "m4a",
        _ => "webm",
    }
}

/// 把录音转成文字。
pub fn transcribe(t: &TranscribeTarget, audio: &[u8], mime: &str, language: &str, timeout: Duration) -> Result<String, LlmError> {
    if audio.is_empty() {
        return Err(LlmError::Config("录音是空的".into()));
    }
    if audio.len() > 25 * 1024 * 1024 {
        return Err(LlmError::Config("录音太长（超过 25 MB）".into()));
    }
    let boundary = format!("----chronicle{:x}", audio.len() as u64 ^ 0x9e37_79b9_7f4a_7c15);
    let mut body: Vec<u8> = vec![];
    let mut field = |name: &str, value: &str| {
        body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
    };
    if !t.model.trim().is_empty() {
        field("model", t.model.trim());
    }
    if !language.trim().is_empty() {
        field("language", language.trim());
    }
    field("response_format", "json");
    let mime_clean = mime.split(';').next().unwrap_or("audio/webm").trim();
    body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"speech.{}\"\r\nContent-Type: {mime_clean}\r\n\r\n", ext_of(mime)).as_bytes());
    body.extend_from_slice(audio);
    body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
    let agent = make_agent(timeout);
    let mut req = agent.post(&api_url(&t.base_url, "audio/transcriptions")).header("Content-Type", &format!("multipart/form-data; boundary={boundary}"));
    let secrets: Vec<&str> = t.api_key.as_deref().into_iter().collect();
    if let Some(k) = t.api_key.as_deref().filter(|k| !k.is_empty()) {
        req = req.header("Authorization", &format!("Bearer {k}"));
    }
    let mut resp = req.send(&body[..]).map_err(|e| LlmError::Transport(redact(&e.to_string(), &secrets)))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().with_config().limit(4 * 1024 * 1024).read_to_string().map_err(|e| LlmError::Transport(e.to_string()))?;
    if !(200..300).contains(&status) {
        return Err(LlmError::Http { status, body: redact(&text, &secrets) });
    }
    // 有的服务即使要了 json 也直接返回纯文本。
    let out = match serde_json::from_str::<Value>(&text) {
        Ok(v) => v.get("text").and_then(Value::as_str).map(str::to_string).ok_or_else(|| LlmError::BadResponse("识别结果里没有 text".into()))?,
        Err(_) => text,
    };
    Ok(out.trim().to_string())
}

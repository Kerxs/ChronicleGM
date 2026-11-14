//! 生成图片：OpenAI 兼容的 `POST /images/generations`。
//!
//! 按 OpenAI 的接口文档实现：请求 `{model, prompt, n: 1, size}`，返回 `data[0].b64_json` 或 `data[0].url`。
//! 也兼容一些服务把结果放在 `images[0].url`（如硅基流动）。返回地址时再下载一次（不带 API Key，地址本身就是授权）。
//! 是否真的能用取决于你选的服务与模型；没有真实 Key 的测试只用本地模拟服务器验证。

use std::io::Read;
use std::time::Duration;

use base64::Engine as _;

use crate::client::make_agent;
use crate::tts::api_url;
use crate::{CancelToken, LlmError};

#[derive(Debug, Clone, Default)]
pub struct ImageTarget {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    /// 例如 `1024x1024`；为空时用 `1024x1024`。
    pub size: String,
}

#[derive(Debug, Clone)]
pub struct GeneratedImage {
    pub mime: &'static str,
    pub bytes: Vec<u8>,
    /// 服务改写后的提示词（有的模型会返回 `revised_prompt`）。
    pub revised_prompt: Option<String>,
}

/// 单张图片的上限：与节点图片相同（12 MB）。
pub const MAX_IMAGE_BYTES: u64 = 12 * 1024 * 1024;

fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if b.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn read_body(resp: &mut ureq::http::Response<ureq::Body>, limit: u64) -> Result<Vec<u8>, LlmError> {
    let mut out = Vec::new();
    resp.body_mut().with_config().limit(limit).reader().read_to_end(&mut out).map_err(|e| LlmError::Transport(e.to_string()))?;
    Ok(out)
}

pub fn generate(target: &ImageTarget, prompt: &str, timeout: Duration, cancel: &CancelToken) -> Result<GeneratedImage, LlmError> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(LlmError::Config("没有图片描述".into()));
    }
    if target.model.trim().is_empty() {
        return Err(LlmError::Config("生图服务没有填写模型".into()));
    }
    let size = if target.size.trim().is_empty() { "1024x1024" } else { target.size.trim() };
    let body = serde_json::json!({ "model": target.model.trim(), "prompt": prompt, "n": 1, "size": size });
    let agent = make_agent(timeout);
    let url = api_url(&target.base_url, "images/generations");
    let mut attempt = 0;
    let mut resp = loop {
        if cancel.is_cancelled() {
            return Err(LlmError::Cancelled);
        }
        let mut req = agent.post(&url).header("Content-Type", "application/json");
        if let Some(k) = target.api_key.as_deref().filter(|k| !k.is_empty()) {
            req = req.header("Authorization", &format!("Bearer {k}"));
        }
        let resp = req.send(body.to_string().as_bytes()).map_err(|e| LlmError::Transport(e.to_string()))?;
        let status = resp.status().as_u16();
        // 限流与临时故障重试两次（生图通常按次收费，不多重试）。
        if (status == 429 || (500..600).contains(&status)) && attempt < 2 {
            attempt += 1;
            std::thread::sleep(Duration::from_secs(2 * attempt));
            continue;
        }
        break resp;
    };
    let status = resp.status().as_u16();
    let raw = read_body(&mut resp, 64 * 1024 * 1024)?;
    if !(200..300).contains(&status) {
        return Err(LlmError::Http { status, body: String::from_utf8_lossy(&raw[..raw.len().min(16 * 1024)]).into_owned() });
    }
    if cancel.is_cancelled() {
        return Err(LlmError::Cancelled);
    }
    let v: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| LlmError::Transport(format!("生图服务返回的不是 JSON：{e}")))?;
    let item = v["data"].get(0).or_else(|| v["images"].get(0)).ok_or_else(|| LlmError::Transport(format!("生图服务没有返回图片：{}", truncate(&v.to_string()))))?;
    let revised_prompt = item["revised_prompt"].as_str().map(str::to_string);
    let bytes = if let Some(b64) = item["b64_json"].as_str() {
        base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|_| LlmError::Transport("图片数据无法解码".into()))?
    } else if let Some(u) = item["url"].as_str() {
        if !(u.starts_with("https://") || u.starts_with("http://")) {
            return Err(LlmError::Transport("生图服务返回的图片地址无效".into()));
        }
        let mut r = agent.get(u).call().map_err(|e| LlmError::Transport(format!("下载生成的图片失败：{e}")))?;
        let st = r.status().as_u16();
        if !(200..300).contains(&st) {
            return Err(LlmError::Http { status: st, body: "下载生成的图片失败".into() });
        }
        read_body(&mut r, MAX_IMAGE_BYTES + 1)?
    } else {
        return Err(LlmError::Transport(format!("生图服务返回的格式不认识：{}", truncate(&item.to_string()))));
    };
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(LlmError::Transport("生成的图片超过 12 MB".into()));
    }
    let mime = sniff(&bytes).ok_or_else(|| LlmError::Transport("生成的内容不是 PNG / JPEG / WebP 图片".into()))?;
    Ok(GeneratedImage { mime, bytes, revised_prompt })
}

fn truncate(s: &str) -> String {
    s.chars().take(300).collect()
}

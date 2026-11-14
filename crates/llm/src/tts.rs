//! 语音合成客户端（OpenAI 兼容的 `POST /v1/audio/speech`）。
//!
//! 按 Breeze TTS 自带的接口文档（docs/server.md）实现，并在本机实测：
//! - JSON 请求：`input`（文字）、`instructions`（声音描述）、`voice`（保存的声音，可省略）、`response_format: "wav"`、`seed`；
//! - 返回流式 WAV（24 kHz、16 位、单声道），头部的长度字段先写成最大值——这里读完后改成真实长度；
//! - `409` 表示所有生成槽都占满了，需要退避重试（文档要求）。

use std::io::Read;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::client::{join_url, make_agent};
use crate::LlmError;
use crate::CancelToken;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct VoiceSpec {
    /// 声音描述或语气（Breeze 的 instructions），例如"沉稳的中年男声，语速稍慢"。
    #[serde(default)]
    pub instructions: String,
    /// 服务里保存的声音 ID（可以为空：按描述设计声音）。
    #[serde(default)]
    pub voice_id: String,
    /// 随机种子：同样的描述 + 同样的种子，声音更稳定。
    #[serde(default)]
    pub seed: Option<i64>,
}

/// 语音服务：本机 Breeze，或云端 OpenAI 兼容的 `/audio/speech`（OpenAI、硅基流动等）。
#[derive(Debug, Clone, Default)]
pub struct SpeechTarget {
    /// 例如 `http://127.0.0.1:8137`（Breeze）或 `https://api.openai.com/v1`。
    pub base_url: String,
    pub api_key: Option<String>,
    /// 云端服务的模型（例如 `gpt-4o-mini-tts`）；为空时不发（Breeze 不需要）。
    pub model: String,
    /// 没有为某个角色指定声音时用的默认声音（OpenAI 要求必填 `voice`）。
    pub default_voice: String,
    /// `wav`（默认）或 `mp3`。
    pub format: String,
    /// 是否发送声音描述 `instructions`。
    pub send_instructions: bool,
    /// 是否发送 Breeze 专有的 `seed`（只对本机服务）。
    pub send_seed: bool,
}

impl SpeechTarget {
    /// 本机 Breeze（与以前的请求完全相同）。
    pub fn local(base_url: &str) -> Self {
        Self { base_url: base_url.into(), send_instructions: true, send_seed: true, ..Default::default() }
    }
}

/// 合成结果。
#[derive(Debug, Clone)]
pub struct Audio {
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

/// 地址只写到端口（如 Breeze 的 `http://127.0.0.1:8137`）时补上 `/v1`；写了路径（`/v1`、`/api/v3` 等）就直接接在后面。
pub fn api_url(base_url: &str, path: &str) -> String {
    let b = base_url.trim_end_matches('/');
    let after_scheme = b.split("://").nth(1).unwrap_or(b);
    if after_scheme.contains('/') { join_url(b, path) } else { join_url(&join_url(b, "v1"), path) }
}

/// 按文件头判断音频格式。
pub fn sniff_audio(b: &[u8]) -> Option<&'static str> {
    if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WAVE" {
        Some("audio/wav")
    } else if b.starts_with(b"ID3") || (b.len() >= 2 && b[0] == 0xff && (b[1] & 0xe0) == 0xe0) {
        Some("audio/mpeg")
    } else if b.starts_with(b"OggS") {
        Some("audio/ogg")
    } else if b.starts_with(b"fLaC") {
        Some("audio/flac")
    } else {
        None
    }
}

/// 合成一段语音，返回完整的 WAV 文件内容（本机 Breeze，保留原来的接口）。
pub fn synthesize(base_url: &str, text: &str, voice: &VoiceSpec, timeout: Duration, cancel: &CancelToken) -> Result<Vec<u8>, LlmError> {
    let a = synthesize_to(&SpeechTarget::local(base_url), text, voice, timeout, cancel)?;
    if a.mime != "audio/wav" {
        return Err(LlmError::Transport(format!("语音服务返回的不是 WAV（{}）", a.mime)));
    }
    Ok(a.bytes)
}

/// 合成一段语音。WAV 会修正流式输出的长度字段；其他格式（mp3 等）原样返回。
pub fn synthesize_to(target: &SpeechTarget, text: &str, voice: &VoiceSpec, timeout: Duration, cancel: &CancelToken) -> Result<Audio, LlmError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(LlmError::Config("没有要朗读的文字".into()));
    }
    let agent = make_agent(timeout);
    let url = api_url(&target.base_url, "audio/speech");
    let format = if target.format.trim().is_empty() { "wav" } else { target.format.trim() };
    let mut body = serde_json::json!({ "input": text, "response_format": format });
    if !target.model.trim().is_empty() {
        body["model"] = target.model.trim().into();
    }
    if target.send_instructions && !voice.instructions.trim().is_empty() {
        body["instructions"] = voice.instructions.trim().into();
    }
    let v = if voice.voice_id.trim().is_empty() { target.default_voice.trim() } else { voice.voice_id.trim() };
    if !v.is_empty() {
        body["voice"] = v.into();
    }
    if target.send_seed
        && let Some(s) = voice.seed
    {
        body["seed"] = s.into();
    }
    let payload = serde_json::to_vec(&body).map_err(|e| LlmError::Config(e.to_string()))?;
    let mut wait = Duration::from_millis(400);
    for attempt in 0..12 {
        if cancel.is_cancelled() {
            return Err(LlmError::Cancelled);
        }
        let mut req = agent.post(&url).header("Content-Type", "application/json");
        if let Some(k) = target.api_key.as_deref().filter(|k| !k.is_empty()) {
            req = req.header("Authorization", &format!("Bearer {k}"));
        }
        let mut resp = req.send(&payload[..]).map_err(|e| LlmError::Transport(e.to_string()))?;
        let status = resp.status().as_u16();
        // 409：Breeze 的生成槽占满；429 / 5xx：云端限流或临时故障。都退避后重试。
        let busy = status == 409 || status == 429;
        let server = (500..600).contains(&status);
        if (busy && attempt < 11) || (server && attempt < 2) {
            std::thread::sleep(wait);
            wait = (wait * 3 / 2).min(Duration::from_secs(4));
            continue;
        }
        if !(200..300).contains(&status) {
            let mut t = String::new();
            let _ = resp.body_mut().as_reader().take(16 * 1024).read_to_string(&mut t);
            return Err(LlmError::Http { status, body: t });
        }
        let mut out = Vec::new();
        let mut reader = resp.body_mut().with_config().limit(256 * 1024 * 1024).reader();
        let mut buf = [0u8; 32 * 1024];
        loop {
            if cancel.is_cancelled() {
                return Err(LlmError::Cancelled);
            }
            let n = reader.read(&mut buf).map_err(|e| LlmError::Transport(e.to_string()))?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        return match sniff_audio(&out) {
            Some("audio/wav") => Ok(Audio { mime: "audio/wav", bytes: fix_wav_lengths(out)? }),
            Some(m) => Ok(Audio { mime: m, bytes: out }),
            None => Err(LlmError::Transport(format!("语音服务返回的不是音频（{} 字节）：{}", out.len(), String::from_utf8_lossy(&out[..out.len().min(200)])))),
        };
    }
    Err(LlmError::Http { status: 409, body: "语音服务一直忙".into() })
}

/// 流式 WAV 的长度字段是占位的最大值：改成真实长度，播放器才能正确显示时长。
pub fn fix_wav_lengths(mut wav: Vec<u8>) -> Result<Vec<u8>, LlmError> {
    if wav.len() < 44 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return Err(LlmError::Transport(format!("语音服务返回的不是 WAV（{} 字节）", wav.len())));
    }
    let riff = (wav.len() - 8) as u32;
    wav[4..8].copy_from_slice(&riff.to_le_bytes());
    // 找到 data 块。
    let mut i = 12;
    while i + 8 <= wav.len() {
        let id = &wav[i..i + 4];
        let size_pos = i + 4;
        if id == b"data" {
            let data = (wav.len() - (i + 8)) as u32;
            wav[size_pos..size_pos + 4].copy_from_slice(&data.to_le_bytes());
            return Ok(wav);
        }
        let size = u32::from_le_bytes([wav[size_pos], wav[size_pos + 1], wav[size_pos + 2], wav[size_pos + 3]]) as usize;
        i = i + 8 + size + (size & 1);
    }
    Err(LlmError::Transport("WAV 里没有音频数据".into()))
}

/// WAV 的时长（秒）。
pub fn wav_seconds(wav: &[u8]) -> f64 {
    if wav.len() < 44 {
        return 0.0;
    }
    let rate = u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]).max(1) as f64;
    let block = u16::from_le_bytes([wav[32], wav[33]]).max(1) as f64;
    ((wav.len() - 44) as f64 / block) / rate
}

/// 语音服务是否就绪（`GET /health`）。
pub fn health(base_url: &str, timeout: Duration) -> bool {
    let root = base_url.trim_end_matches('/').trim_end_matches("/v1");
    make_agent(timeout).get(&join_url(root, "health")).call().is_ok_and(|r| r.status().as_u16() == 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn streamed_wav(samples: usize) -> Vec<u8> {
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&u32::MAX.to_le_bytes());
        w.extend_from_slice(b"WAVE");
        w.extend_from_slice(b"fmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&24000u32.to_le_bytes());
        w.extend_from_slice(&48000u32.to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&u32::MAX.to_le_bytes());
        w.extend(std::iter::repeat_n(0u8, samples * 2));
        w
    }

    #[test]
    fn streamed_wav_lengths_are_fixed() {
        let w = fix_wav_lengths(streamed_wav(24000)).unwrap();
        assert_eq!(u32::from_le_bytes([w[4], w[5], w[6], w[7]]) as usize, w.len() - 8);
        assert_eq!(u32::from_le_bytes([w[40], w[41], w[42], w[43]]), 48000);
        assert!((wav_seconds(&w) - 1.0).abs() < 1e-6);
        assert!(fix_wav_lengths(b"not a wav file at all, definitely not one!!!!".to_vec()).is_err());
    }
}

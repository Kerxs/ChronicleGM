//! 语音：GM 叙述与 NPC 台词的朗读。
//!
//! - 请求走调度中心（P2 语音）：同样的文字和声音只合成一次（去重），队列有上限（默认 16），不会无限堆积；
//!   托管的语音模型按需启动。
//! - 服务：本机 Breeze，或云端 OpenAI 兼容的语音合成（`SpeechTarget`）。
//! - 缓存：按"文字 + 声音 + 服务"的哈希存成音频文件（WAV 或 MP3），超过上限（默认 500 MB）时删除最久没用的。
//! - 声音：旁白用设置里的声音；每个 NPC 用人物资料里的 `voice`，没有设置时按身份与性格生成描述、按人物 ID 固定种子，
//!   同一个人每次的声音尽量一致。
//! - 朗读前去掉台词里括号中的动作描写（"（压低声音）"），这些不该被读出来。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use chronicle_llm::tts::{self, SpeechTarget, VoiceSpec};
use chronicle_llm::CancelToken;
use chronicle_runtime::{ModelManager, Priority, SchedError, Scheduler, Ticket};
use serde::Serialize;

pub const DEFAULT_CACHE_MB: u64 = 500;
pub const NARRATOR_KEY: &str = "tts.narrator";

#[derive(Debug, Clone, Serialize)]
pub struct SpeakResult {
    /// data:audio/wav;base64,… 或 data:audio/mpeg;base64,…
    pub data_url: String,
    pub cached: bool,
    pub seconds: f64,
    pub ms: u64,
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// 去掉括号里的动作与神态描写、说话人前缀和多余空白。
pub fn speakable(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in text.chars() {
        match c {
            '（' | '(' | '【' | '[' => depth += 1,
            '）' | ')' | '】' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let out = out.trim().trim_matches(|c| c == '"' || c == '“' || c == '”' || c == '「' || c == '」').trim().to_string();
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// NPC 的声音：资料里的 `voice` 优先，否则按身份与性格描述，按人物 ID 固定种子。
pub fn npc_voice(node: &chronicle_core::Node) -> VoiceSpec {
    if let Some(v) = node.payload.get("voice").and_then(|v| serde_json::from_value::<VoiceSpec>(v.clone()).ok())
        && (!v.instructions.trim().is_empty() || !v.voice_id.trim().is_empty())
    {
        return VoiceSpec { seed: v.seed.or(Some((fnv(&node.id) % 100_000) as i64)), ..v };
    }
    let s = |k: &str| node.payload.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let mut desc = vec![];
    for k in ["identity", "personality", "appearance"] {
        let v = s(k);
        if !v.is_empty() {
            desc.push(v.chars().take(40).collect::<String>());
        }
    }
    let instructions = if desc.is_empty() { format!("{}，说普通话，语气自然", node.title) } else { format!("{}，说普通话，语气符合身份", desc.join("，")) };
    VoiceSpec { instructions, voice_id: String::new(), seed: Some((fnv(&node.id) % 100_000) as i64) }
}

pub fn default_narrator() -> VoiceSpec {
    VoiceSpec { instructions: "沉稳的中文旁白，吐字清楚，语速适中，带一点讲故事的感觉".into(), voice_id: String::new(), seed: Some(7) }
}

const AUDIO_EXT: &[&str] = &["wav", "mp3", "ogg", "flac"];

fn cache_path(dir: &Path, text: &str, voice: &VoiceSpec, target: &SpeechTarget) -> PathBuf {
    // 本机 Breeze 的键与以前相同（已有缓存继续有效）；云端服务再加上地址、模型、默认声音与格式。
    let mut key = format!("{}|{}|{}|{:?}", text, voice.instructions, voice.voice_id, voice.seed);
    if target.api_key.is_some() || !target.model.is_empty() || !target.default_voice.is_empty() || !target.format.is_empty() {
        key.push_str(&format!("|{}|{}|{}|{}", target.base_url, target.model, target.default_voice, target.format));
    }
    let ext = if target.format.trim() == "mp3" { "mp3" } else { "wav" };
    dir.join(format!("{:016x}.{ext}", fnv(&key)))
}

/// 云端语音的 NPC 声音：人物没有指定声音时，从服务的声音列表里按人物 ID 固定选一个。
pub fn pick_voice(spec: &mut VoiceSpec, node_id: &str, voices: &[String]) {
    if spec.voice_id.trim().is_empty() && !voices.is_empty() {
        spec.voice_id = voices[(fnv(node_id) % voices.len() as u64) as usize].clone();
    }
}

/// 缓存超过上限时删除最久没用的文件。返回删除的个数。
pub fn evict(dir: &Path, max_bytes: u64) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let mut files: Vec<(PathBuf, u64, SystemTime)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((e.path(), m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
        })
        .filter(|(p, _, _)| p.extension().is_some_and(|x| AUDIO_EXT.iter().any(|e| x == *e)))
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort_by_key(|f| f.2);
    let mut n = 0;
    for (p, len, _) in files {
        if total <= max_bytes {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= len;
            n += 1;
        }
    }
    n
}

pub fn cache_stats(dir: &Path) -> (usize, u64) {
    std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).filter_map(|e| e.metadata().ok()).fold((0, 0), |(n, b), m| (n + 1, b + m.len())))
        .unwrap_or((0, 0))
}

fn to_result(audio: &[u8], cached: bool, started: Instant) -> SpeakResult {
    use base64::Engine;
    let mime = tts::sniff_audio(audio).unwrap_or("audio/wav");
    // MP3 等格式的时长由界面播放时得到，这里只算 WAV。
    let seconds = if mime == "audio/wav" { tts::wav_seconds(audio) } else { 0.0 };
    SpeakResult { data_url: format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(audio)), cached, seconds, ms: started.elapsed().as_millis() as u64 }
}

/// 合成（或从缓存取）一段语音。
#[allow(clippy::too_many_arguments)]
pub fn speak(sched: &std::sync::Arc<Scheduler>, models: &ModelManager, cache_dir: &Path, max_cache_bytes: u64, target: &SpeechTarget, text: &str, voice: &VoiceSpec, cancel: &CancelToken) -> Result<SpeakResult, String> {
    let started = Instant::now();
    let text = speakable(text);
    if text.is_empty() {
        return Err("没有可以朗读的文字".into());
    }
    let _ = std::fs::create_dir_all(cache_dir);
    let path = cache_path(cache_dir, &text, voice, target);
    if let Ok(wav) = std::fs::read(&path) {
        // 更新修改时间，作为"最近使用"。
        let _ = std::fs::OpenOptions::new().append(true).open(&path).and_then(|f| f.set_modified(SystemTime::now()));
        return Ok(to_result(&wav, true, started));
    }
    let service = chronicle_runtime::client::service_key(&target.base_url);
    let key = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    let ticket = Ticket::new(Priority::P2, "voice", service.clone()).label(text.chars().take(20).collect::<String>()).dedup(key).queue_timeout(Duration::from_secs(120));
    let permit = match sched.acquire(ticket, cancel) {
        Ok(p) => p,
        // 同样的内容正在合成：等它写进缓存。
        Err(SchedError::Duplicate) => {
            for _ in 0..240 {
                if let Ok(wav) = std::fs::read(&path) {
                    return Ok(to_result(&wav, true, started));
                }
                if cancel.is_cancelled() {
                    return Err("已取消".into());
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            return Err("等待同样内容的语音超时".into());
        }
        Err(SchedError::QueueFull(_)) => return Err("语音队列已满（最多 16 条排队），请稍后再试".into()),
        Err(e) => return Err(e.to_string()),
    };
    if let Err(e) = chronicle_runtime::client::BeforeRequest::ensure(models, &service, cancel) {
        permit.finish(Some(e.clone()));
        return Err(format!("语音服务没有启动：{e}"));
    }
    match tts::synthesize_to(target, &text, voice, Duration::from_secs(300), cancel).map(|a| a.bytes) {
        Ok(wav) => {
            chronicle_runtime::client::BeforeRequest::touch(models, &service);
            permit.finish(None);
            let tmp = path.with_extension("part");
            if std::fs::write(&tmp, &wav).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
            evict(cache_dir, max_cache_bytes);
            Ok(to_result(&wav, false, started))
        }
        Err(e) => {
            permit.finish(Some(e.to_string()));
            Err(format!("语音合成失败：{e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_directions_are_not_spoken() {
        assert_eq!(speakable("（压低声音）那把钥匙……(左右看看)不见了。"), "那把钥匙……不见了。");
        assert_eq!(speakable("「你好」"), "你好");
        assert_eq!(speakable("（叹气）"), "");
    }

    #[test]
    fn cache_evicts_oldest_over_limit() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(d.path().join(format!("{i}.wav")), vec![0u8; 1000]).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(evict(d.path(), 2500), 3);
        assert!(d.path().join("4.wav").exists() && !d.path().join("0.wav").exists());
        assert_eq!(cache_stats(d.path()).0, 2);
    }

    /// 本机 Breeze 的缓存键与以前相同（已有缓存继续有效）；云端服务换了模型或格式就是另一份缓存。
    #[test]
    fn cache_keys_keep_local_and_separate_cloud() {
        let d = Path::new("c");
        let v = default_narrator();
        let local = cache_path(d, "你好", &v, &SpeechTarget::local("http://127.0.0.1:8137"));
        let key = format!("{}|{}|{}|{:?}", "你好", v.instructions, v.voice_id, v.seed);
        assert_eq!(local, d.join(format!("{:016x}.wav", fnv(&key))), "与旧版本的文件名相同");
        let cloud = SpeechTarget { base_url: "https://api.openai.com/v1".into(), api_key: Some("k".into()), model: "gpt-4o-mini-tts".into(), format: "mp3".into(), ..Default::default() };
        let c1 = cache_path(d, "你好", &v, &cloud);
        assert!(c1.extension().is_some_and(|e| e == "mp3"));
        assert_ne!(c1.file_stem(), local.file_stem());
        let c2 = cache_path(d, "你好", &v, &SpeechTarget { model: "tts-1".into(), ..cloud.clone() });
        assert_ne!(c1, c2);
    }

    #[test]
    fn npc_voice_is_picked_stably_from_list() {
        let voices = vec!["alloy".to_string(), "echo".to_string(), "nova".to_string()];
        let mut a = VoiceSpec::default();
        pick_voice(&mut a, "npc-1", &voices);
        let mut b = VoiceSpec::default();
        pick_voice(&mut b, "npc-1", &voices);
        assert_eq!(a.voice_id, b.voice_id, "同一个人每次一样");
        assert!(voices.contains(&a.voice_id));
        let mut own = VoiceSpec { voice_id: "shimmer".into(), ..Default::default() };
        pick_voice(&mut own, "npc-1", &voices);
        assert_eq!(own.voice_id, "shimmer", "人物自己的声音优先");
        let mut none = VoiceSpec::default();
        pick_voice(&mut none, "npc-1", &[]);
        assert!(none.voice_id.is_empty());
    }
}

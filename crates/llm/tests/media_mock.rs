//! 云端语音合成、生成图片、模型列表的客户端测试。
//! 注意：全部使用本地 mock 服务器，只验证请求格式与对各种响应的处理，不代表任何真实云端服务的行为。

mod mock_server;

use std::time::Duration;

use base64::Engine;
use chronicle_llm::discovery::list_models;
use chronicle_llm::image::{ImageTarget, generate};
use chronicle_llm::tts::{SpeechTarget, VoiceSpec, api_url, synthesize, synthesize_to};
use chronicle_llm::{CancelToken, LlmError};
use mock_server::*;
use serde_json::json;

const T: Duration = Duration::from_secs(10);

fn wav() -> Vec<u8> {
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&u32::MAX.to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&[1, 0, 1, 0]);
    w.extend_from_slice(&24000u32.to_le_bytes());
    w.extend_from_slice(&48000u32.to_le_bytes());
    w.extend_from_slice(&[2, 0, 16, 0]);
    w.extend_from_slice(b"data");
    w.extend_from_slice(&u32::MAX.to_le_bytes());
    w.extend(std::iter::repeat_n(0u8, 4800));
    w
}

fn png() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAF0lEQVR4nGN4ICBAEmIY1TCqQWDYagAA6ggAEK2bvLwAAAAASUVORK5CYII=")
        .unwrap()
}

#[test]
fn api_url_adds_v1_only_when_there_is_no_path() {
    assert_eq!(api_url("http://127.0.0.1:8137", "audio/speech"), "http://127.0.0.1:8137/v1/audio/speech");
    assert_eq!(api_url("http://127.0.0.1:8137/v1/", "audio/speech"), "http://127.0.0.1:8137/v1/audio/speech");
    assert_eq!(api_url("https://api.openai.com/v1", "images/generations"), "https://api.openai.com/v1/images/generations");
    assert_eq!(api_url("https://ark.cn-beijing.volces.com/api/v3", "models"), "https://ark.cn-beijing.volces.com/api/v3/models");
}

/// 本机 Breeze：请求与以前完全相同（没有 model、没有 Authorization，带 seed 与 instructions）。
#[test]
fn local_breeze_request_is_unchanged() {
    let srv = MockServer::start(|_, _| Reply::Bytes(200, "audio/wav".into(), wav()));
    let v = VoiceSpec { instructions: "沉稳的旁白".into(), voice_id: String::new(), seed: Some(7) };
    let out = synthesize(&srv.url, "你好", &v, T, &CancelToken::new()).unwrap();
    assert_eq!(u32::from_le_bytes([out[4], out[5], out[6], out[7]]) as usize, out.len() - 8, "WAV 长度已修正");
    let r = &srv.requests.lock().unwrap()[0];
    assert_eq!(r.path, "/v1/audio/speech");
    assert!(r.header("authorization").is_none());
    let b = r.json();
    assert_eq!(b, json!({ "input": "你好", "response_format": "wav", "instructions": "沉稳的旁白", "seed": 7 }));
}

/// 云端（OpenAI 兼容）：带 Key、model、默认声音、mp3；不发 Breeze 专有的 seed；返回 mp3 原样交回。
#[test]
fn cloud_speech_sends_key_model_voice_and_returns_mp3() {
    let mut mp3 = b"ID3\x04\x00\x00\x00\x00\x00\x00".to_vec();
    mp3.extend(std::iter::repeat_n(0u8, 100));
    let body = mp3.clone();
    let srv = MockServer::start(move |_, _| Reply::Bytes(200, "audio/mpeg".into(), body.clone()));
    let target = SpeechTarget {
        base_url: format!("{}/v1", srv.url),
        api_key: Some("sk-test".into()),
        model: "gpt-4o-mini-tts".into(),
        default_voice: "alloy".into(),
        format: "mp3".into(),
        send_instructions: false,
        send_seed: false,
        ..Default::default()
    };
    let v = VoiceSpec { instructions: "不会发出去".into(), voice_id: String::new(), seed: Some(1) };
    let a = synthesize_to(&target, "你好", &v, T, &CancelToken::new()).unwrap();
    assert_eq!((a.mime, a.bytes), ("audio/mpeg", mp3));
    // 角色自己的声音优先于默认声音。
    let v2 = VoiceSpec { instructions: String::new(), voice_id: "nova".into(), seed: None };
    synthesize_to(&target, "再见", &v2, T, &CancelToken::new()).unwrap();
    let reqs = srv.requests.lock().unwrap();
    assert_eq!(reqs[0].header("authorization"), Some("Bearer sk-test"));
    assert_eq!(reqs[0].json(), json!({ "input": "你好", "response_format": "mp3", "model": "gpt-4o-mini-tts", "voice": "alloy" }));
    assert_eq!(reqs[1].json()["voice"], "nova");
}

#[test]
fn cloud_speech_errors_are_reported_not_hidden() {
    let srv = MockServer::start(|_, _| Reply::Json(401, json!({"error": {"message": "Incorrect API key"}})));
    let t = SpeechTarget { base_url: srv.url.clone(), api_key: Some("bad".into()), ..Default::default() };
    match synthesize_to(&t, "你好", &VoiceSpec::default(), T, &CancelToken::new()) {
        Err(LlmError::Http { status: 401, body }) => assert!(body.contains("Incorrect API key")),
        other => panic!("应该是 401：{other:?}"),
    }
    assert_eq!(srv.count(), 1, "401 不重试");
    // 返回的不是音频（例如 JSON 错误但状态码 200）。
    let srv = MockServer::start(|_, _| Reply::Json(200, json!({"ok": true})));
    let t = SpeechTarget { base_url: srv.url.clone(), ..Default::default() };
    assert!(matches!(synthesize_to(&t, "你好", &VoiceSpec::default(), T, &CancelToken::new()), Err(LlmError::Transport(_))));
}

#[test]
fn speech_retries_rate_limit_then_succeeds() {
    let srv = MockServer::start(|_, n| if n < 3 { Reply::Text(429, "slow down".into()) } else { Reply::Bytes(200, "audio/wav".into(), wav()) });
    let t = SpeechTarget { base_url: srv.url.clone(), api_key: Some("k".into()), ..Default::default() };
    let a = synthesize_to(&t, "你好", &VoiceSpec::default(), T, &CancelToken::new()).unwrap();
    assert_eq!(a.mime, "audio/wav");
    assert_eq!(srv.count(), 3);
}

/// 生图：OpenAI 格式（b64_json）。
#[test]
fn image_b64_json() {
    let b64 = base64::engine::general_purpose::STANDARD.encode(png());
    let srv = MockServer::start(move |_, _| Reply::Json(200, json!({ "created": 1, "data": [{ "b64_json": b64, "revised_prompt": "改写后的描述" }] })));
    let t = ImageTarget { base_url: format!("{}/v1", srv.url), api_key: Some("sk-img".into()), model: "gpt-image-1".into(), size: "1024x1536".into() };
    let img = generate(&t, "雾中的港口", T, &CancelToken::new()).unwrap();
    assert_eq!((img.mime, img.bytes), ("image/png", png()));
    assert_eq!(img.revised_prompt.as_deref(), Some("改写后的描述"));
    let r = &srv.requests.lock().unwrap()[0];
    assert_eq!(r.path, "/v1/images/generations");
    assert_eq!(r.header("authorization"), Some("Bearer sk-img"));
    assert_eq!(r.json(), json!({ "model": "gpt-image-1", "prompt": "雾中的港口", "n": 1, "size": "1024x1536" }));
}

/// 生图：返回图片地址（data[].url 或 images[].url），再下载；下载请求不带 Key。
#[test]
fn image_url_is_downloaded_without_key() {
    let srv = MockServer::start(|r, _| {
        if r.path.starts_with("/files/") {
            Reply::Bytes(200, "image/png".into(), png())
        } else {
            Reply::Json(200, json!({ "images": [{ "url": format!("http://{}/files/a.png", r.header("host").unwrap_or("")) }] }))
        }
    });
    let t = ImageTarget { base_url: format!("{}/v1", srv.url), api_key: Some("sk".into()), model: "Kwai-Kolors/Kolors".into(), size: String::new() };
    let img = generate(&t, "灯塔", T, &CancelToken::new()).unwrap();
    assert_eq!(img.mime, "image/png");
    let reqs = srv.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0].json()["size"], "1024x1024", "默认尺寸");
    assert!(reqs[1].header("authorization").is_none(), "下载图片不带 API Key");
}

#[test]
fn image_rejects_bad_input_and_bad_output() {
    let t = ImageTarget { base_url: "http://127.0.0.1:1".into(), model: String::new(), ..Default::default() };
    assert!(matches!(generate(&t, "x", T, &CancelToken::new()), Err(LlmError::Config(_))), "没填模型时不发请求");
    let srv = MockServer::start(|_, _| Reply::Json(200, json!({ "data": [{ "b64_json": base64::engine::general_purpose::STANDARD.encode(b"not an image") }] })));
    let t = ImageTarget { base_url: srv.url.clone(), model: "m".into(), ..Default::default() };
    assert!(matches!(generate(&t, "x", T, &CancelToken::new()), Err(LlmError::Transport(_))), "不是图片");
    assert!(matches!(generate(&t, "  ", T, &CancelToken::new()), Err(LlmError::Config(_))), "空描述");
    let srv = MockServer::start(|_, _| Reply::Json(400, json!({ "error": { "message": "content policy" } })));
    let t = ImageTarget { base_url: srv.url.clone(), model: "m".into(), ..Default::default() };
    match generate(&t, "x", T, &CancelToken::new()) {
        Err(LlmError::Http { status: 400, body }) => assert!(body.contains("content policy")),
        other => panic!("应该是 400：{other:?}"),
    }
}

#[test]
fn models_list_with_key() {
    let srv = MockServer::start(|_, _| Reply::Json(200, json!({ "object": "list", "data": [{ "id": "gpt-4o" }, { "id": "deepseek-chat" }, { "id": "gpt-4o" }] })));
    let ids = list_models(&format!("{}/v1", srv.url), Some("sk-x"), T).unwrap();
    assert_eq!(ids, vec!["deepseek-chat".to_string(), "gpt-4o".to_string()]);
    let r = &srv.requests.lock().unwrap()[0];
    assert_eq!((r.path.as_str(), r.header("authorization")), ("/v1/models", Some("Bearer sk-x")));
    let srv = MockServer::start(|_, _| Reply::Json(401, json!({ "error": "bad key" })));
    assert!(matches!(list_models(&srv.url, Some("x"), T), Err(LlmError::Http { status: 401, .. })));
}

/// 语音识别：multipart 请求格式与解析（mock，不代表真实服务）。
#[test]
fn transcription_sends_multipart_and_reads_text() {
    use chronicle_llm::transcribe::*;
    let srv = MockServer::start(|r, _| {
        if r.path.ends_with("/audio/transcriptions") {
            let ct = r.header("Content-Type").unwrap_or("").to_string();
            assert!(ct.starts_with("multipart/form-data; boundary="), "{ct}");
            assert!(r.body.contains("name=\"model\"") && r.body.contains("whisper-1"));
            assert!(r.body.contains("filename=\"speech.webm\""));
            assert!(r.body.contains("name=\"language\"\r\n\r\nzh"));
            return Reply::Json(200, serde_json::json!({"text": " 我推开酒馆的门 "}));
        }
        Reply::Json(404, serde_json::json!({}))
    });
    let t = TranscribeTarget { base_url: format!("{}/v1", srv.url), api_key: Some("k".into()), model: "whisper-1".into() };
    let text = transcribe(&t, b"fake-audio", "audio/webm;codecs=opus", "zh", std::time::Duration::from_secs(3)).unwrap();
    assert_eq!(text, "我推开酒馆的门");
    assert!(transcribe(&t, b"", "audio/webm", "zh", std::time::Duration::from_secs(1)).is_err());
}

/// GPT-SoVITS api_v2：POST /tts，参数名按它的源码（text、text_lang、ref_audio_path、prompt_text、prompt_lang、media_type）。
#[test]
fn gpt_sovits_request_follows_api_v2() {
    use chronicle_llm::tts::SpeechApi;
    let srv = MockServer::start(|_, _| Reply::Bytes(200, "audio/wav".into(), wav()));
    let target = SpeechTarget { api: SpeechApi::GptSovits, base_url: srv.url.clone(), default_voice: r"D:/ref/旁白.wav|今天天气很好。|zh".into(), send_seed: true, ..Default::default() };
    let v = VoiceSpec { instructions: "不会发出去".into(), voice_id: String::new(), seed: Some(3) };
    let a = synthesize_to(&target, "欢迎来到雾湾。", &v, T, &CancelToken::new()).unwrap();
    assert_eq!(a.mime, "audio/wav");
    let r = &srv.requests.lock().unwrap()[0];
    assert_eq!((r.method.as_str(), r.path.as_str()), ("POST", "/tts"));
    assert_eq!(
        r.json(),
        json!({ "text": "欢迎来到雾湾。", "text_lang": "zh", "ref_audio_path": r"D:/ref/旁白.wav", "prompt_text": "今天天气很好。", "prompt_lang": "zh", "media_type": "wav", "streaming_mode": false, "seed": 3 })
    );
    // 地址写成 /v1 也去掉；NPC 的声音（voice_id）覆盖默认参考音频。
    let srv2 = MockServer::start(|_, _| Reply::Bytes(200, "audio/wav".into(), wav()));
    let t2 = SpeechTarget { api: SpeechApi::GptSovits, base_url: format!("{}/v1", srv2.url), default_voice: "a.wav".into(), text_lang: "ja".into(), ..Default::default() };
    let v2 = VoiceSpec { voice_id: "npc.wav".into(), ..Default::default() };
    synthesize_to(&t2, "こんにちは", &v2, T, &CancelToken::new()).unwrap();
    let b = srv2.requests.lock().unwrap()[0].json();
    assert_eq!(srv2.requests.lock().unwrap()[0].path, "/tts");
    assert_eq!((b["ref_audio_path"].as_str(), b["prompt_text"].as_str(), b["prompt_lang"].as_str(), b["text_lang"].as_str()), (Some("npc.wav"), Some(""), Some("zh"), Some("ja")));
    assert!(b.get("seed").is_none());
}

#[test]
fn gpt_sovits_without_reference_audio_or_with_400_reports_why() {
    use chronicle_llm::tts::SpeechApi;
    let t = SpeechTarget { api: SpeechApi::GptSovits, base_url: "http://127.0.0.1:9".into(), ..Default::default() };
    let e = synthesize_to(&t, "你好", &VoiceSpec::default(), T, &CancelToken::new()).unwrap_err();
    assert!(e.to_string().contains("参考音频"), "{e}");
    let srv = MockServer::start(|_, _| Reply::Json(400, json!({ "message": "tts failed", "Exception": "ref audio not found" })));
    let t = SpeechTarget { api: SpeechApi::GptSovits, base_url: srv.url.clone(), default_voice: "missing.wav".into(), ..Default::default() };
    match synthesize_to(&t, "你好", &VoiceSpec::default(), T, &CancelToken::new()).unwrap_err() {
        LlmError::Http { status, body } => assert!(status == 400 && body.contains("ref audio not found")),
        e => panic!("{e:?}"),
    }
}

#[test]
fn sovits_voice_string_parses_path_text_and_language() {
    use chronicle_llm::tts::parse_sovits_voice;
    let v = parse_sovits_voice(r#""D:\声音 样本.wav" | 你好呀 | ja"#);
    assert_eq!((v.ref_audio_path.as_str(), v.prompt_text.as_str(), v.prompt_lang.as_str()), (r"D:\声音 样本.wav", "你好呀", "ja"));
    assert_eq!(parse_sovits_voice("a.wav").prompt_lang, "zh");
}

/// 声音列表：Kokoro-FastAPI 的 /v1/audio/voices（对象带 id），没有时退到 openai-edge-tts 的 /v1/voices/all。
#[test]
fn voice_lists_are_read_from_kokoro_or_edge_endpoints() {
    use chronicle_llm::discovery::list_voices;
    let kokoro = MockServer::start(|r, _| {
        if r.path == "/v1/audio/voices" { Reply::Json(200, json!({ "voices": [{ "id": "zf_xiaobei" }, { "id": "zm_yunxi" }] })) } else { Reply::Json(404, json!({})) }
    });
    assert_eq!(list_voices(&kokoro.url, None, T).unwrap(), vec!["zf_xiaobei", "zm_yunxi"]);
    let edge = MockServer::start(|r, _| {
        if r.path == "/v1/voices/all" { Reply::Json(200, json!([{ "ShortName": "zh-CN-XiaoxiaoNeural" }, { "name": "zh-CN-YunxiNeural" }])) } else { Reply::Json(404, json!({})) }
    });
    assert_eq!(list_voices(&format!("{}/v1", edge.url), Some("k"), T).unwrap(), vec!["zh-CN-XiaoxiaoNeural", "zh-CN-YunxiNeural"]);
    assert_eq!(edge.requests.lock().unwrap().last().unwrap().header("authorization"), Some("Bearer k"));
    let none = MockServer::start(|_, _| Reply::Json(404, json!({})));
    assert!(list_voices(&none.url, None, T).is_err());
}

/// 识别语音服务：Kokoro（有声音列表）、GPT-SoVITS（GET /tts 返回 400 + message）、只有 tts 模型的 OpenAI 兼容服务；都不能用于文本生成。
#[test]
fn discovery_recognizes_speech_services() {
    use chronicle_llm::ServerKind;
    use chronicle_llm::discovery::probe;
    let kokoro = MockServer::start(|r, _| match r.path.as_str() {
        "/v1/audio/voices" => Reply::Json(200, json!({ "voices": ["zf_xiaobei", "af_bella"] })),
        "/v1/models" => Reply::Json(200, json!({ "data": [{ "id": "kokoro" }] })),
        _ => Reply::Json(404, json!({})),
    });
    let s = probe(&kokoro.url, Some(ServerKind::OpenAiSpeech), T).unwrap();
    assert_eq!((s.kind, s.usable_for_chat), (ServerKind::OpenAiSpeech, false));
    assert_eq!(s.voices, vec!["zf_xiaobei", "af_bella"]);
    assert_eq!(s.models[0].id, "kokoro");

    let sovits = MockServer::start(|r, _| if r.path.starts_with("/tts") { Reply::Json(400, json!({ "message": "text is required" })) } else { Reply::Json(404, json!({})) });
    let s = probe(&sovits.url, Some(ServerKind::GptSovits), T).unwrap();
    assert_eq!((s.kind, s.usable_for_chat), (ServerKind::GptSovits, false));
    assert!(sovits.requests.lock().unwrap().iter().all(|r| r.method == "GET"), "识别时不发合成请求");

    let edge = MockServer::start(|r, _| if r.path == "/v1/models" { Reply::Json(200, json!({ "data": [{ "id": "tts-1" }, { "id": "tts-1-hd" }] })) } else { Reply::Json(404, json!({})) });
    let s = probe(&edge.url, Some(ServerKind::OpenAiSpeech), T).unwrap();
    assert_eq!((s.kind, s.usable_for_chat), (ServerKind::OpenAiSpeech, false));

    // 普通语言模型服务不受影响。
    let chat = MockServer::start(|r, _| if r.path == "/v1/models" { Reply::Json(200, json!({ "data": [{ "id": "qwen3-8b" }] })) } else { Reply::Json(404, json!({})) });
    assert!(probe(&chat.url, None, T).unwrap().usable_for_chat);
}

/// 就绪检查：有 /health 的看 200；没有 /health 的服务（GPT-SoVITS 返回 404）也算在运行；连不上算没就绪。
#[test]
fn speech_health_accepts_services_without_health_endpoint() {
    use chronicle_llm::tts::health;
    let ok = MockServer::start(|_, _| Reply::Json(200, json!({ "status": "ok" })));
    assert!(health(&ok.url, T));
    let no_health = MockServer::start(|_, _| Reply::Json(404, json!({ "detail": "Not Found" })));
    assert!(health(&no_health.url, T));
    let broken = MockServer::start(|_, _| Reply::Json(503, json!({})));
    assert!(!health(&broken.url, T));
    assert!(!health("http://127.0.0.1:9", Duration::from_millis(500)));
}

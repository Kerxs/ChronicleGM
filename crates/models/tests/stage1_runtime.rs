//! 阶段 1：llama.cpp 运行环境（模拟 GitHub Releases，不真实下载）。
//! 检查更新、按硬件选构建、下载校验、解压、自检、切换、保留上一个版本并能回滚、自检失败不影响当前版本、损坏检测。

mod common;

use std::path::Path;
use std::sync::atomic::AtomicBool;

use chronicle_models::hw::Hardware;
use chronicle_models::runtime_mgr::{self, Backend, Health, RuntimeMgr};
use common::*;
use serde_json::json;

fn exe() -> &'static str {
    runtime_mgr::server_exe_name()
}

/// 模拟的自检：llama-server 文件存在且内容是 "ok" 才算通过。
fn fake_verify(p: &Path) -> Result<String, String> {
    match std::fs::read_to_string(p) {
        Ok(s) if s.starts_with("ok") => Ok(format!("version: {} (mock)", s.trim())),
        Ok(_) => Err("自检失败：缺少 CUDA 运行库".into()),
        Err(e) => Err(e.to_string()),
    }
}

fn publish(m: &Mock, tag: &str, server_body: &str, with_digest: bool) {
    let main = make_zip(&[(&format!("llama-{tag}-bin-win-cuda-12.4-x64/{}", exe()), server_body.as_bytes()), (&format!("llama-{tag}-bin-win-cuda-12.4-x64/ggml-cuda.dll"), b"dll")]);
    let rt = make_zip(&[("cudart64_12.dll", b"cudart"), ("cublas64_12.dll", b"cublas")]);
    let cpu = make_zip(&[(exe(), b"ok-cpu")]);
    let names = [(format!("llama-{tag}-bin-win-cuda-12.4-x64.zip"), main), ("cudart-llama-bin-win-cuda-12.4-x64.zip".to_string(), rt), (format!("llama-{tag}-bin-win-cpu-x64.zip"), cpu)];
    let mut assets = vec![];
    for (n, b) in names {
        let path = format!("/dl/{tag}/{n}");
        assets.push(json!({ "name": n, "size": b.len(), "browser_download_url": m.url(&path), "digest": if with_digest { json!(format!("sha256:{}", sha256(&b))) } else { json!(null) } }));
        m.file(&path, b);
    }
    m.json("/repos/ggml-org/llama.cpp/releases", &json!([{ "tag_name": tag, "published_at": "2026-11-01T00:00:00Z", "draft": false, "prerelease": false, "assets": assets }]));
}

fn nvidia() -> Hardware {
    Hardware { os: "windows".into(), arch: "x64".into(), nvidia: vec![Default::default()], cuda: Some((13, 0)), vulkan: true, ..Default::default() }
}

#[test]
fn install_update_rollback_and_damage_detection() {
    let m = Mock::start();
    let dir = tempfile::tempdir().unwrap();
    let mut rm = RuntimeMgr::new(dir.path(), chronicle_models::no_log());
    rm.api = m.base.clone();
    let stop = AtomicBool::new(false);
    assert_eq!(rm.health(None), Health::NotInstalled);

    // 首次安装：选 CUDA 12.4 构建并带上 cudart；校验 SHA256；运行库被放到 llama-server 同一目录。
    publish(&m, "b7000", "ok-7000", true);
    let (rel, newer) = rm.check_update().unwrap();
    assert!(newer);
    let plan = runtime_mgr::choose(&rel, &nvidia(), None).unwrap();
    assert_eq!(plan.backend, Backend::Cuda { major: 12, minor: 4 });
    assert_eq!(plan.assets.len(), 2);
    let mut seen = 0;
    let inst = rm.install(&plan, &stop, |_, _| seen += 1, &fake_verify).unwrap();
    assert!(seen > 0, "有进度回报");
    let bin = rm.runtimes_dir().join(&inst.dir);
    assert!(bin.join(exe()).exists() && bin.join("cudart64_12.dll").exists() && bin.join("ggml-cuda.dll").exists());
    assert!(matches!(rm.health(Some(&fake_verify)), Health::Ok { .. }));
    assert!(!dir.path().join("downloads").read_dir().unwrap().any(|_| true), "装好后删除下载的压缩包");

    // 模型、配置不在 runtimes/ 里，更新时不受影响。
    std::fs::create_dir_all(dir.path().join("models")).unwrap();
    std::fs::write(dir.path().join("models/keep.gguf"), b"model").unwrap();

    // 更新到新版本：上一个版本保留，可以回滚。
    publish(&m, "b7100", "ok-7100", true);
    let (rel, newer) = rm.check_update().unwrap();
    assert!(newer);
    let inst2 = rm.install(&runtime_mgr::choose(&rel, &nvidia(), None).unwrap(), &stop, |_, _| {}, &fake_verify).unwrap();
    let s = rm.state();
    assert_eq!(s.current.as_ref().unwrap().tag, "b7100");
    assert_eq!(s.previous.as_ref().unwrap().tag, "b7000");
    assert!(dir.path().join("models/keep.gguf").exists(), "更新不删模型");
    let back = rm.rollback().unwrap();
    assert_eq!(back.tag, "b7000");
    assert_eq!(rm.state().previous.unwrap().tag, inst2.tag);
    rm.rollback().unwrap();

    // 新版本自检失败（比如缺运行库）：不切换，当前版本照常可用，临时目录清理干净。
    publish(&m, "b7200", "broken", true);
    let (rel, _) = rm.check_update().unwrap();
    let err = rm.install(&runtime_mgr::choose(&rel, &nvidia(), None).unwrap(), &stop, |_, _| {}, &fake_verify).unwrap_err();
    assert!(err.contains("自检失败"), "{err}");
    assert_eq!(rm.state().current.unwrap().tag, "b7100");
    assert!(!rm.runtimes_dir().read_dir().unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with('.')), "没有残留临时目录");

    // 校验值不对：拒绝安装。
    publish(&m, "b7300", "ok-7300", false);
    let (mut rel, _) = rm.check_update().unwrap();
    for a in rel.assets.iter_mut() {
        a.digest = Some(format!("sha256:{}", "0".repeat(64)));
    }
    let err = rm.install(&runtime_mgr::choose(&rel, &nvidia(), None).unwrap(), &stop, |_, _| {}, &fake_verify).unwrap_err();
    assert!(err.contains("SHA256"), "{err}");
    assert_eq!(rm.state().current.unwrap().tag, "b7100");

    // 损坏检测：llama-server 被删掉 → 损坏；重装后恢复。
    let cur = rm.state().current.unwrap();
    std::fs::remove_file(rm.exe_of(&cur)).unwrap();
    assert!(matches!(rm.health(None), Health::Broken { .. }));
    publish(&m, "b7100", "ok-7100", true);
    let (rel, newer) = rm.check_update().unwrap();
    assert!(!newer, "同一版本不算更新");
    rm.install(&runtime_mgr::choose(&rel, &nvidia(), None).unwrap(), &stop, |_, _| {}, &fake_verify).unwrap();
    assert!(matches!(rm.health(Some(&fake_verify)), Health::Ok { .. }));
    // 只保留当前和上一个版本。
    let dirs: Vec<_> = rm.runtimes_dir().read_dir().unwrap().flatten().collect();
    assert!(dirs.len() <= 2, "{dirs:?}");
}

#[test]
fn interrupted_download_resumes_with_range() {
    let m = Mock::start();
    let dir = tempfile::tempdir().unwrap();
    let body: Vec<u8> = (0..600_000u32).map(|i| (i % 251) as u8).collect();
    m.route("/big.bin", Route { body: body.clone(), content_type: "application/octet-stream".into(), cut_after: Some(200_000), ..Default::default() });
    let dest = dir.path().join("big.bin");
    let stop = AtomicBool::new(false);
    let req = chronicle_models::net::Request { url: &m.url("/big.bin"), dest: &dest, token: None, cookie: None, expected_size: Some(body.len() as u64), sha256: Some(&sha256(&body)), stop: &stop };
    let mut tries = 0;
    loop {
        tries += 1;
        match chronicle_models::net::fetch(&req, |_| {}) {
            Ok(()) => break,
            Err(e) => assert!(e.retryable(), "{e}"),
        }
        assert!(tries < 10);
    }
    assert_eq!(std::fs::read(&dest).unwrap(), body);
    assert!(tries >= 3, "断了几次，每次从断点继续");
    let ranges: Vec<_> = m.hits.lock().unwrap().iter().filter_map(|h| h.1.clone()).collect();
    assert!(ranges.contains(&"200000".to_string()), "续传带 Range：{ranges:?}");
}

/// 真实自检：本机已有的 llama-server（`CHRONICLE_TEST_LLAMA` 指定路径；没有就跳过）。
#[test]
#[ignore]
fn real_llama_server_self_check() {
    let Ok(p) = std::env::var("CHRONICLE_TEST_LLAMA") else { return };
    let v = runtime_mgr::verify_exe(Path::new(&p)).unwrap();
    eprintln!("自检通过：{v}");
    assert!(v.contains("version"));
    let hw = chronicle_models::hw::detect();
    eprintln!("硬件：{hw:?}");
}

/// 真实推理（阶段 5）：用本机已有的 llama-server 和模型文件，按自动参数启动，发一次对话，读运行状态。
/// 需要 CHRONICLE_TEST_LLAMA 和 CHRONICLE_TEST_GGUF；不下载任何东西。
#[test]
#[ignore]
fn real_tuned_server_runs_and_reports_stats() {
    let (Ok(exe), Ok(model)) = (std::env::var("CHRONICLE_TEST_LLAMA"), std::env::var("CHRONICLE_TEST_GGUF")) else { return };
    let info = chronicle_llm::gguf::read_gguf(Path::new(&model)).unwrap();
    let hw = chronicle_models::hw::detect();
    let port = 18977;
    let input = chronicle_models::tune::TuneInput { model: model.clone(), mmproj: None, port, ctx: 0, vram_reserve_mb: 1024, gpu_layers: None, threads: None, cache_type: None, flash_attn: None, batch: None };
    let t = chronicle_models::tune::tune(&input, &info, info.file_size, 0, &hw);
    eprintln!("自动参数：GPU 层 {}/{}，上下文 {}，KV {}，FA {}，线程 {}，批 {}，估计显存 {} MB / 内存 {} MB；{:?}", t.gpu_layers, t.total_layers, t.ctx, t.cache_type, t.flash_attn, t.threads, t.batch, t.est_vram_mb, t.est_ram_mb, t.notes);
    assert!(t.ok, "{:?}", t.problems);
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _child = Kill(std::process::Command::new(&exe).args(&t.args).current_dir(Path::new(&exe).parent().unwrap()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap());
    let started = std::time::Instant::now();
    let mut ok = false;
    while started.elapsed() < std::time::Duration::from_secs(240) {
        if chronicle_models::tune::server_stats(port).healthy {
            ok = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    eprintln!("加载耗时 {:.1} 秒", started.elapsed().as_secs_f64());
    {
        assert!(ok, "llama-server 没有就绪");
        let body = serde_json::json!({ "messages": [{ "role": "user", "content": "用一句话描述一座海边的灯塔。" }], "max_tokens": 64, "temperature": 0.7, "chat_template_kwargs": { "enable_thinking": false } });
        let mut r = ureq::post(&format!("http://127.0.0.1:{port}/v1/chat/completions")).header("content-type", "application/json").send(body.to_string()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.body_mut().read_to_string().unwrap()).unwrap();
        eprintln!("回复：{}", v["choices"][0]["message"]["content"].as_str().unwrap_or_default());
        let s = chronicle_models::tune::server_stats(port);
        let raw = ureq::get(&format!("http://127.0.0.1:{port}/slots")).call().ok().and_then(|mut r| r.body_mut().read_to_string().ok()).unwrap_or_default();
        eprintln!("状态：{s:?}（/slots {} 字节）", raw.len());
        assert!(s.gen_tps.unwrap_or(0.0) > 0.0 && s.ctx == Some(t.ctx));
        assert!(s.ctx_used.unwrap_or(0) > 0, "读得到上下文用量");
    }
}

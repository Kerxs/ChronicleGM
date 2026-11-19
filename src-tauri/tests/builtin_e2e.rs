//! 阶段 6：游戏系统接入内置模型的端到端测试（真实模型，不下载任何东西）。
//!
//! 和应用里同一条路径：模型库添加本地 GGUF → 自动参数 → 生成托管配置 `builtin-llm` 交给 ModelManager
//! → 游戏的模型请求经调度中心发出，用到时按需启动 llama-server（只监听 127.0.0.1）→ 跑真实 GM 回合 → 读运行状态 → 停止。
//!
//! 需要环境变量 CHRONICLE_TEST_LLAMA（llama-server 路径）和 CHRONICLE_TEST_GGUF（模型文件）；没有就跳过。
//! 运行：cargo test -p chronicle-gm-app --test builtin_e2e -- --ignored --nocapture

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chronicle_core::{World, campaign};
use chronicle_gen::Templates;
use chronicle_gm::{Engine, GmOptions, run_turn};
use chronicle_llm::{CancelToken, LlmClient, OpenAiClient, ProviderConfig};
use chronicle_models::library::Library;
use chronicle_models::tune::{self, TuneInput};
use chronicle_runtime::{BeforeRequest, ModelManager, ModelProfile, ModelRole, PerfMode, Priority, ScheduledClient, Scheduler};

#[test]
#[ignore]
fn game_turns_run_on_builtin_model_started_on_demand() {
    let (Ok(exe), Ok(gguf)) = (std::env::var("CHRONICLE_TEST_LLAMA"), std::env::var("CHRONICLE_TEST_GGUF")) else {
        eprintln!("没有设置 CHRONICLE_TEST_LLAMA / CHRONICLE_TEST_GGUF，跳过");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    // 1. 模型库：添加本地已有的 GGUF（不复制）。
    let mut lib = Library::open(&dir.path().join("llama"), &dir.path().join("models"), chronicle_models::no_log());
    let m = lib.add_file(Path::new(&gguf), None).unwrap();
    println!("模型库：{}（{}，{}，上下文 {:?}）", m.name, m.quant.clone().unwrap_or_default(), m.arch.clone().unwrap_or_default(), m.context_length);
    // 2. 自动参数。
    let hw = chronicle_models::hw::detect();
    let port = 18979;
    let info = chronicle_llm::gguf::read_gguf(Path::new(&m.path)).unwrap();
    let t = tune::tune(&TuneInput { model: m.path.clone(), mmproj: None, port, ctx: 0, vram_reserve_mb: 1024, gpu_layers: None, threads: None, cache_type: None, flash_attn: None, batch: None }, &info, m.size, 0, &hw);
    assert!(t.ok, "{:?}", t.problems);
    println!("自动参数：GPU 层 {}/{}，上下文 {}，KV {}，FA {}，线程 {}；{:?}", t.gpu_layers, t.total_layers, t.ctx, t.cache_type, t.flash_attn, t.threads, t.notes);
    // 3. 托管配置交给 ModelManager（和应用里的 builtin-llm 一样）。
    let sched = Scheduler::new(PerfMode::Balanced);
    let logs = dir.path().join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    let mm = ModelManager::new(&logs, &sched, |s| println!("[托管] {s}"));
    let exe_dir = Path::new(&exe).parent().unwrap().display().to_string();
    mm.set_profiles(vec![ModelProfile { id: "builtin-llm".into(), name: "内置模型".into(), role: ModelRole::Llm, exe: exe.clone(), args: t.args.clone(), cwd: exe_dir, port, health_path: "/health".into(), start_timeout_secs: 300, auto_restart: true, idle_unload_secs: None, exclusive_group: "llm".into(), gpu_reserve_mb: 0 }]);
    assert!(mm.status().iter().all(|p| p.pid.is_none()), "还没启动");
    // 4. 游戏的模型客户端：经调度中心，用到时按需启动。
    let base = format!("http://127.0.0.1:{port}/v1");
    let cfg = ProviderConfig { id: "builtin-llama".into(), name: "内置模型".into(), base_url: base.clone(), model: m.name.clone(), server_kind: Some(chronicle_llm::types::ServerKind::LlamaServer), needs_key: false, timeout_secs: 600, capabilities: None, allow_campaign_content: true, thinking: chronicle_llm::ThinkingMode::Off, service: Default::default(), media: Default::default() };
    let before: Arc<dyn BeforeRequest> = mm.clone();
    let client = ScheduledClient::new(Arc::new(OpenAiClient::new(cfg, None)), sched.clone(), &base, Priority::P1, "gm").with_before(Some(before));
    let opts = GmOptions { json_schema: true, thinking: Some(false), temperature: 0.8, max_tokens: 4096, context_budget: (t.ctx as usize / 3).clamp(1500, 6000), stream: false, web_search: None, dice: None };
    // 5. 示例世界上跑两个真实 GM 回合。
    let mut w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "内置模型联调").unwrap();
    chronicle_core::demo::populate(&mut w).unwrap();
    let script = chronicle_core::scripts::list_scripts(w.conn()).unwrap().into_iter().next().map(|s| s.id);
    let c = campaign::create_campaign(&mut w, script.as_deref(), "内置模型团", None).unwrap();
    let world = Mutex::new(w);
    let templates = Templates::builtin();
    let cancel = CancelToken::new();
    // 不管成功失败，结束时都停掉模型进程。
    struct Stop(Arc<ModelManager>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.stop_all();
        }
    }
    let guard = Stop(mm.clone());
    {
        for (i, input) in ["我环顾四周，看看这里都有些什么人。", "我走向最近的人，问他最近这一带有没有发生什么怪事。"].iter().enumerate() {
            let started = Instant::now();
            let r = run_turn(&world, &Engine { gm: &client as &dyn LlmClient, npc: &client, templates: &templates, opts: &opts }, &c.id, input, &cancel).unwrap_or_else(|e| panic!("第 {} 回合失败：{e}", i + 1));
            println!("\n—— 第 {} 回合（{:.1} 秒）——\n玩家：{input}\nGM：{}", i + 1, started.elapsed().as_secs_f32(), r.narration);
            for l in &r.npc_lines {
                println!("{}：{}", l.name, l.text);
            }
            assert!(!r.narration.trim().is_empty());
        }
        let p = mm.status().into_iter().find(|p| p.profile.id == "builtin-llm").unwrap();
        assert!(p.pid.is_some(), "游戏请求按需启动了内置模型");
        let s = tune::server_stats(port);
        println!("\n运行状态：生成 {:?} tokens/s，读提示 {:?} tokens/s，上下文 {:?}/{:?}，加载耗时 {:?} ms", s.gen_tps, s.prompt_tps, s.ctx_used, s.ctx, p.last_load_ms);
        assert!(s.healthy && s.ctx == Some(t.ctx));
        // 只监听本机。
        assert!(t.args.windows(2).any(|w| w[0] == "--host" && w[1] == "127.0.0.1"));
    }
    drop(guard);
    assert!(mm.status().iter().all(|p| p.pid.is_none()), "停止后没有残留进程");
}

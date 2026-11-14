//! 模型托管：用测试专用的假模型服务（真实进程）验证启动、健康检查、停止、崩溃重启、
//! 外部服务不接管、按需加载、空闲释放、互斥组、启动超时。

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chronicle_llm::*;
use chronicle_runtime::*;

const FAKE: &str = env!("CARGO_BIN_EXE_fake_model");

fn profile(id: &str, port: u16, start_delay_ms: u64) -> ModelProfile {
    ModelProfile {
        id: id.into(),
        name: format!("假模型{id}"),
        role: ModelRole::Llm,
        exe: FAKE.into(),
        args: vec![port.to_string(), start_delay_ms.to_string(), "0".into()],
        cwd: String::new(),
        port,
        health_path: "/health".into(),
        start_timeout_secs: 20,
        auto_restart: true,
        idle_unload_secs: Some(0),
        exclusive_group: String::new(),
        gpu_reserve_mb: 0,
    }
}

fn setup(profiles: Vec<ModelProfile>) -> (tempfile::TempDir, Arc<Scheduler>, Arc<ModelManager>) {
    let dir = tempfile::tempdir().unwrap();
    let sched = Scheduler::new(PerfMode::Balanced);
    let m = ModelManager::new(dir.path(), &sched, |msg| eprintln!("[托管] {msg}"));
    m.set_profiles(profiles);
    (dir, sched, m)
}

fn state(m: &ModelManager, id: &str) -> ProcState {
    m.status().into_iter().find(|s| s.profile.id == id).unwrap().state
}

fn wait_for(mut f: impl FnMut() -> bool, secs: u64) -> bool {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn get(port: u16, path: &str) -> Option<u16> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(2))).http_status_as_error(false).build().into();
    agent.get(&format!("http://127.0.0.1:{port}{path}")).call().ok().map(|r| r.status().as_u16())
}

#[test]
fn start_waits_for_health_then_stop_releases_port() {
    let (_d, _s, m) = setup(vec![profile("a", 18101, 1200)]);
    let t = Instant::now();
    m.start("a", &CancelToken::new()).unwrap();
    assert!(t.elapsed() >= Duration::from_millis(1100), "要等到健康检查通过（假服务 1.2 秒后才就绪）");
    let st = m.status().remove(0);
    assert_eq!(st.state, ProcState::Running);
    assert!(st.pid.is_some() && st.last_load_ms.unwrap() >= 1100);
    assert_eq!(get(18101, "/health"), Some(200));
    m.stop("a").unwrap();
    assert_eq!(state(&m, "a"), ProcState::Stopped);
    assert!(wait_for(|| get(18101, "/health").is_none(), 5), "停止后端口应释放");
}

#[test]
fn crash_is_detected_and_restarted() {
    let (_d, _s, m) = setup(vec![profile("b", 18102, 0)]);
    m.start("b", &CancelToken::new()).unwrap();
    let pid1 = m.status()[0].pid.unwrap();
    let _ = get(18102, "/crash");
    assert!(wait_for(|| { let s = &m.status()[0]; s.state == ProcState::Running && s.pid.is_some_and(|p| p != pid1) }, 15), "崩溃后应自动重启：{:?}", m.status()[0]);
    assert_eq!(m.status()[0].restarts, 1);
    m.stop("b").unwrap();
}

#[test]
fn repeated_crashes_stop_restarting() {
    let (_d, _s, m) = setup(vec![profile("c", 18103, 0)]);
    m.start("c", &CancelToken::new()).unwrap();
    for _ in 0..4 {
        assert!(wait_for(|| state(&m, "c") == ProcState::Running, 15));
        let _ = get(18103, "/crash");
        assert!(wait_for(|| state(&m, "c") != ProcState::Running, 5));
    }
    assert!(wait_for(|| matches!(state(&m, "c"), ProcState::Crashed(_)), 5), "5 分钟内第 4 次崩溃后不再重启：{:?}", state(&m, "c"));
    assert_eq!(m.status()[0].restarts, 3);
}

#[test]
fn missing_exe_and_start_timeout_fail_cleanly() {
    let mut bad = profile("d", 18104, 0);
    bad.exe = "D:/不存在/llama-server.exe".into();
    let mut slow = profile("e", 18105, 60_000);
    slow.start_timeout_secs = 2;
    let (_d, _s, m) = setup(vec![bad, slow]);
    assert!(m.start("d", &CancelToken::new()).unwrap_err().contains("找不到程序"));
    assert!(matches!(state(&m, "d"), ProcState::Failed(_)));
    let err = m.start("e", &CancelToken::new()).unwrap_err();
    assert!(err.contains("超过"), "{err}");
    assert!(m.status().iter().find(|s| s.profile.id == "e").unwrap().pid.is_none(), "超时后进程被结束");
    assert!(wait_for(|| get(18105, "/health").is_none(), 5));
}

#[test]
fn external_service_is_never_taken_over() {
    struct KillOnDrop(std::process::Child);
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _ext = KillOnDrop(Command::new(FAKE).args(["18106", "0", "0"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    assert!(wait_for(|| get(18106, "/health") == Some(200), 5));
    let (_d, _s, m) = setup(vec![profile("f", 18106, 0)]);
    m.start("f", &CancelToken::new()).unwrap();
    assert_eq!(state(&m, "f"), ProcState::External);
    assert!(m.stop("f").is_err(), "外部服务不能被停止");
    m.stop_all();
    assert_eq!(get(18106, "/health"), Some(200), "外部服务仍在运行");
}

fn client(port: u16) -> Arc<dyn LlmClient> {
    Arc::new(OpenAiClient::new(
        ProviderConfig {
            id: "p".into(),
            name: "p".into(),
            base_url: format!("http://127.0.0.1:{port}"),
            model: "fake".into(),
            server_kind: None,
            needs_key: false,
            timeout_secs: 30,
            capabilities: None,
            allow_campaign_content: true,
            thinking: ThinkingMode::Auto,
            service: Default::default(),
            media: Default::default(),
        },
        None,
    ))
}

fn req() -> ChatRequest {
    ChatRequest { messages: vec![ChatMessage::user("hi")], temperature: None, max_tokens: Some(8), format: OutputFormat::Text, stream: false, thinking: None }
}

#[test]
fn request_loads_model_on_demand_and_idle_releases_it() {
    let mut p = profile("g", 18107, 800);
    p.idle_unload_secs = Some(2);
    let (_d, sched, m) = setup(vec![p]);
    assert_eq!(state(&m, "g"), ProcState::Stopped);
    let before: Arc<dyn BeforeRequest> = m.clone();
    let c = ScheduledClient::new(client(18107), sched.clone(), "http://127.0.0.1:18107", Priority::P0, "gm_plan").with_before(Some(before));
    let r = c.chat(&req(), &CancelToken::new(), &mut |_| {}).unwrap();
    assert!(r.content.contains("ok"));
    assert_eq!(state(&m, "g"), ProcState::Running, "请求触发了按需加载");
    assert!(sched.snapshot().recent.iter().any(|t| t.kind == "gm_plan" && t.state == TaskState::Done));
    assert!(wait_for(|| state(&m, "g") == ProcState::Stopped, 10), "空闲 2 秒后释放");
    assert!(wait_for(|| get(18107, "/health").is_none(), 5));
}

#[test]
fn exclusive_group_swaps_models_without_interrupting_requests() {
    let mut llm = profile("llm", 18108, 0);
    llm.exclusive_group = "gpu".into();
    let mut tts = profile("tts", 18109, 0);
    tts.role = ModelRole::Tts;
    tts.exclusive_group = "gpu".into();
    tts.args[2] = "600".into(); // TTS 每个请求要 0.6 秒
    let (_d, sched, m) = setup(vec![llm, tts]);
    let before: Arc<dyn BeforeRequest> = m.clone();
    let c_llm = ScheduledClient::new(client(18108), sched.clone(), "http://127.0.0.1:18108", Priority::P0, "gm").with_before(Some(before.clone()));
    let c_tts = Arc::new(ScheduledClient::new(client(18109), sched.clone(), "http://127.0.0.1:18109", Priority::P2, "tts").with_before(Some(before)));
    c_tts.chat(&req(), &CancelToken::new(), &mut |_| {}).unwrap();
    assert_eq!(state(&m, "tts"), ProcState::Running);
    // TTS 请求进行中时要求 LLM：等 TTS 请求结束才换下它，请求不会被打断。
    let t2 = c_tts.clone();
    let h = std::thread::spawn(move || t2.chat(&req(), &CancelToken::new(), &mut |_| {}));
    std::thread::sleep(Duration::from_millis(200));
    c_llm.chat(&req(), &CancelToken::new(), &mut |_| {}).unwrap();
    assert!(h.join().unwrap().is_ok(), "进行中的 TTS 请求正常完成");
    assert_eq!(state(&m, "llm"), ProcState::Running);
    assert_eq!(state(&m, "tts"), ProcState::Stopped, "同组只保留一个");
    m.stop_all();
}

#[test]
fn foreground_request_is_sent_while_background_request_is_in_flight() {
    // 真实 HTTP：后台生成请求要 1.5 秒；同一服务的 GM 请求应立即发出，而不是排在后面。
    let mut p = profile("h", 18110, 0);
    p.args[2] = "1500".into();
    let (_d, sched, m) = setup(vec![p]);
    m.start("h", &CancelToken::new()).unwrap();
    let before: Arc<dyn BeforeRequest> = m.clone();
    let bg = Arc::new(ScheduledClient::new(client(18110), sched.clone(), "http://127.0.0.1:18110", Priority::P5, "gen.outline").with_before(Some(before.clone())));
    let gm = ScheduledClient::new(client(18110), sched.clone(), "http://127.0.0.1:18110", Priority::P0, "gm").with_before(Some(before));
    let b = bg.clone();
    let h = std::thread::spawn(move || b.chat(&req(), &CancelToken::new(), &mut |_| {}));
    std::thread::sleep(Duration::from_millis(200));
    gm.chat(&req(), &CancelToken::new(), &mut |_| {}).unwrap();
    h.join().unwrap().unwrap();
    let snap = sched.snapshot();
    let gm_task = snap.recent.iter().find(|t| t.kind == "gm").unwrap();
    assert!(gm_task.wait_ms().unwrap() < 200, "GM 请求排队 {} ms", gm_task.wait_ms().unwrap());
    m.stop_all();
}

/// 托管模型在请求前崩溃、正在自动重启：请求等它恢复后重试一次，而不是直接失败（实测：显存不足崩溃后 GM 回合失败）。
#[test]
fn request_during_crash_restart_is_retried() {
    let (_d, sched, m) = setup(vec![profile("r", 18111, 1500)]);
    m.start("r", &CancelToken::new()).unwrap();
    let before: Arc<dyn BeforeRequest> = m.clone();
    let c = ScheduledClient::new(client(18111), sched.clone(), "http://127.0.0.1:18111", Priority::P0, "gm").with_before(Some(before));
    let _ = get(18111, "/crash");
    // 等服务真的停了或正在重新加载（返回 503）再发请求：第一次失败，之后等重启完成再重试。
    assert!(wait_for(|| get(18111, "/health") != Some(200), 5));
    let t = Instant::now();
    let r = c.chat(&req(), &CancelToken::new(), &mut |_| {});
    assert!(r.is_ok(), "{r:?}");
    assert!(t.elapsed() >= Duration::from_millis(500), "等待了重启：{:?}", t.elapsed());
    assert_eq!(m.status()[0].restarts, 1);
    m.stop_all();
}

#[test]
fn gpu_reserve_adds_fit_target_only_when_asked() {
    let mut p = profile("x", 1, 0);
    p.args = vec!["-m".into(), "a.gguf".into(), "--fit".into(), "on".into()];
    assert_eq!(p.effective_args(), p.args, "默认不改动启动参数");
    p.gpu_reserve_mb = 1536;
    assert_eq!(&p.effective_args()[4..], &["--fit-target".to_string(), "2560".to_string()]);
    p.args.extend(["--fit-target".into(), "3000".into()]);
    assert_eq!(p.effective_args(), p.args, "自己写了 --fit-target 时不加");
    p.args = vec!["-ngl".into(), "99".into()];
    assert_eq!(p.effective_args(), p.args, "没有 --fit on 时不加");
}

/// 设置了显存预留的语言模型启动前，先启动托管的语音模型。
#[test]
fn llm_with_reserve_starts_tts_first() {
    let mut llm = profile("l2", 18112, 300);
    llm.gpu_reserve_mb = 1024;
    let mut tts = profile("t2", 18113, 300);
    tts.role = ModelRole::Tts;
    let (_d, sched, m) = setup(vec![llm, tts]);
    let before: Arc<dyn BeforeRequest> = m.clone();
    let c = ScheduledClient::new(client(18112), sched.clone(), "http://127.0.0.1:18112", Priority::P0, "gm").with_before(Some(before));
    c.chat(&req(), &CancelToken::new(), &mut |_| {}).unwrap();
    let st = m.status();
    let get_ = |id: &str| st.iter().find(|s| s.profile.id == id).unwrap().clone();
    assert_eq!(get_("t2").state, ProcState::Running, "语音模型先启动了");
    assert!(get_("t2").started_at.unwrap() <= get_("l2").started_at.unwrap(), "语音模型先于语言模型");
    m.stop_all();
    // 没有设置预留的语言模型不会带起语音模型。
    let llm3 = profile("l3", 18114, 0);
    let mut tts3 = profile("t3", 18115, 0);
    tts3.role = ModelRole::Tts;
    let (_d2, _s2, m2) = setup(vec![llm3, tts3]);
    m2.start("l3", &CancelToken::new()).unwrap();
    assert_eq!(m2.status().iter().find(|s| s.profile.id == "t3").unwrap().state, ProcState::Stopped);
    m2.stop_all();
}

/// 软件被强行结束（任务管理器、崩溃、断电式退出，Drop 不会执行）时，托管的模型进程也必须结束，
/// 否则它继续占着端口和显存，重启后只能当成"外部服务"，无法停止或切换。
#[test]
fn managed_models_die_when_host_process_is_killed() {
    use std::io::{BufRead, BufReader};
    let port = 18_391;
    let dir = tempfile::tempdir().unwrap();
    let mut host = Command::new(env!("CARGO_BIN_EXE_manager_host"))
        .args([FAKE, &port.to_string(), dir.path().to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(host.stdout.take().unwrap()).read_line(&mut line).unwrap();
    assert!(line.starts_with("ready"), "{line}");
    assert_eq!(get(port, "/health"), Some(200), "模型应已启动");
    host.kill().unwrap(); // TerminateProcess：不执行任何清理代码
    host.wait().unwrap();
    assert!(wait_for(|| get(port, "/health").is_none(), 10), "宿主进程被强杀后，托管的模型进程仍在运行（孤儿进程）");
}

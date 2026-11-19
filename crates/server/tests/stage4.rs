//! 第 4 阶段：AI GM 双模式。
//! - 房主电脑上的 GM Agent 主动连服务端，替服务端调房主本机的模型（这里本机模型是一个模拟的 OpenAI 兼容服务）；
//! - 云端模式：服务端直接调云端模型，Key 只存在服务端；
//! - 同一房间只有一个 GM 节点；Agent 断开后 GM 不就绪、行动被拒绝；只有房主能切换；设置重启后还在。
//!
//! 模型都是模拟的（按请求类型返回预设内容），只验证程序侧的链路、权限与隔离，不代表真实模型质量。

mod common;

use std::net::SocketAddr;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use chronicle_protocol::{ActionState, ClientMsg, CloudGmConfig, ErrorCode, GmStatus, PROTOCOL_VERSION, ServerMsg};
use chronicle_server::agent::{AgentConfig, AgentExit, run_agent};
use chronicle_server::{CreateRoom, ServerConfig, create_room, start};
use common::*;
use serde_json::{Value, json};

// ---------- 模拟的 OpenAI 兼容模型服务 ----------

#[derive(Default)]
struct MockModel {
    auth: Mutex<Vec<String>>,
    calls: Mutex<usize>,
}

fn answer(body: &Value) -> String {
    let msgs = body["messages"].as_array().cloned().unwrap_or_default();
    let sys = msgs.first().and_then(|m| m["content"].as_str()).unwrap_or_default().to_string();
    let user = msgs.get(1).and_then(|m| m["content"].as_str()).unwrap_or_default().to_string();
    if sys.contains("剧情导演") {
        json!({"assessment": "平稳", "pacing": "hold", "suggestions": []}).to_string()
    } else if sys.contains("扮演一个人物") {
        json!({"reply": "嗯。", "attitude_change": 0, "remember": []}).to_string()
    } else if user.contains("程序判定结果") {
        json!({"narration": "海风灌进来，灯塔里的旧梯子吱呀作响。"}).to_string()
    } else if user.contains("协助 TRPG 程序理解玩家") {
        "{}".into()
    } else if user.contains("会话摘要") {
        json!({"summary": "大家进了灯塔。"}).to_string()
    } else {
        json!({"intent": "行动", "narration": "海风灌进来，灯塔里的旧梯子吱呀作响。"}).to_string()
    }
}

async fn completions(State(m): State<Arc<MockModel>>, headers: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
    m.auth.lock().unwrap().push(headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string());
    *m.calls.lock().unwrap() += 1;
    Json(json!({ "model": "mock", "choices": [{ "index": 0, "message": { "role": "assistant", "content": answer(&body) }, "finish_reason": "stop" }] }))
}

async fn mock_model() -> (SocketAddr, Arc<MockModel>) {
    let m = Arc::new(MockModel::default());
    let app = axum::Router::new().route("/v1/chat/completions", post(completions)).with_state(m.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    (addr, m)
}

fn provider(model_addr: SocketAddr) -> chronicle_llm::ProviderConfig {
    chronicle_llm::ProviderConfig {
        id: "local".into(),
        name: "本机".into(),
        base_url: format!("http://{model_addr}/v1"),
        model: "mock-local".into(),
        server_kind: None,
        needs_key: true,
        timeout_secs: 30,
        capabilities: None,
        allow_campaign_content: true,
        thinking: chronicle_llm::ThinkingMode::Off,
        service: Default::default(),
        media: Default::default(),
    }
}

// ---------- 小工具 ----------

async fn join(addr: SocketAddr, room: &str, code: &str, token: &str, name: &str) -> (Ws, String) {
    let mut ws = connect(addr).await;
    send(&mut ws, &ClientMsg::Hello { protocol: PROTOCOL_VERSION, room: room.into(), join_code: code.into(), token: token.into(), name: name.into(), last_seq: 0 }).await;
    match recv(&mut ws).await {
        Some(ServerMsg::Welcome { player, .. }) => (ws, player),
        other => panic!("{name} 没能加入：{other:?}"),
    }
}

/// 读消息直到出现满足条件的 GM 状态（最多约 10 秒），同时把读到的全部消息记下来。
async fn wait_gm(ws: &mut Ws, log: &mut Vec<ServerMsg>, want: impl Fn(&GmStatus) -> bool) -> GmStatus {
    for _ in 0..40 {
        match tokio::time::timeout(std::time::Duration::from_millis(250), recv(ws)).await {
            Ok(Some(m)) => {
                let hit = if let ServerMsg::Gm { gm } = &m { want(gm).then(|| gm.clone()) } else { None };
                log.push(m);
                if let Some(g) = hit {
                    return g;
                }
            }
            _ => {
                send(ws, &ClientMsg::GetGm).await;
            }
        }
    }
    panic!("没有等到期望的 GM 状态：{log:?}");
}

async fn until_done(ws: &mut Ws, log: &mut Vec<ServerMsg>, action: &str) -> (ActionState, String) {
    for _ in 0..200 {
        let Ok(Some(m)) = tokio::time::timeout(std::time::Duration::from_secs(30), recv_long(ws)).await else { break };
        let done = if let ServerMsg::ActionStatus { action_id, status, reason, .. } = &m { (action_id == action && matches!(status, ActionState::Done | ActionState::Rejected)).then(|| (*status, reason.clone())) } else { None };
        log.push(m);
        if let Some(d) = done {
            return d;
        }
    }
    panic!("行动 {action} 没有结束：{log:?}");
}

/// GM 回合要调好几次模型，等得比 `recv` 久一点。
async fn recv_long(ws: &mut Ws) -> Option<ServerMsg> {
    loop {
        if let Some(m) = recv(ws).await {
            return Some(m);
        }
    }
}

async fn drain(ws: &mut Ws, log: &mut Vec<ServerMsg>) {
    while let Ok(Some(m)) = tokio::time::timeout(std::time::Duration::from_millis(400), recv(ws)).await {
        log.push(m);
    }
}

fn narrations(v: &[ServerMsg]) -> Vec<String> {
    v.iter().filter_map(|m| if let ServerMsg::Event { kind, payload, .. } = m { (kind == "narration").then(|| payload["text"].as_str().unwrap_or_default().to_string()) } else { None }).collect()
}

fn agent(addr: SocketAddr, room: &str, key: &str, model: SocketAddr) -> AgentConfig {
    AgentConfig { server: format!("ws://{addr}/agent"), room: room.into(), agent_key: key.into(), provider: provider(model), api_key: Some("local-only-key".into()) }
}

fn quiet() -> Arc<dyn Fn(&str) + Send + Sync> {
    Arc::new(|_: &str| {})
}

// ---------- 测试 ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_agent_runs_turns_and_only_one_gm_node_per_room() {
    let s = server(|_| {}).await;
    let (maddr, model) = mock_model().await;
    let r = create_room(&s.hub, CreateRoom { name: "灯塔".into(), max_players: Some(4), host_name: "房主".into(), gm_mode: Some("local".into()), template: None }).await.unwrap();
    let (mut h, _) = join(s.addr, &r.room_id, "", &r.host_token, "").await;
    let (mut a, _) = join(s.addr, &r.room_id, &r.join_code, "", "阿澜").await;
    let (mut lh, mut la) = (vec![], vec![]);
    drain(&mut h, &mut lh).await;
    drain(&mut a, &mut la).await;

    // Agent 不在线：GM 不就绪，行动被拒绝并说明原因。
    send(&mut h, &ClientMsg::GetGm).await;
    let g = wait_gm(&mut h, &mut lh, |_| true).await;
    assert!(!g.ready && !g.agent_online && g.active == "none");
    send(&mut h, &ClientMsg::Action { action_id: "h0".into(), state_version: 0, text: "我推开门".into(), combat: None }).await;
    let (st, reason) = until_done(&mut h, &mut lh, "h0").await;
    assert_eq!(st, ActionState::Rejected);
    assert!(reason.contains("GM"), "{reason}");

    // 密钥不对：直接被拒绝，不会一直重连。
    let bad = run_agent(agent(s.addr, &r.room_id, "wrong-key", maddr), Arc::new(AtomicBool::new(false)), quiet()).await;
    assert!(matches!(bad, AgentExit::Rejected(ref m) if m.contains("密钥")), "{bad:?}");

    // 正确的密钥：上线后房间里所有人都看到 GM 就绪（本地）。
    let stop = Arc::new(AtomicBool::new(false));
    let agent_task = tokio::spawn(run_agent(agent(s.addr, &r.room_id, &r.agent_key, maddr), stop.clone(), quiet()));
    let g = wait_gm(&mut a, &mut la, |g| g.ready).await;
    assert_eq!(g.active, "local");
    assert_eq!(g.agent_model, "mock-local");

    // 第二个 Agent：拒绝（同一房间只有一个权威 GM 节点）。
    let second = run_agent(agent(s.addr, &r.room_id, &r.agent_key, maddr), Arc::new(AtomicBool::new(false)), quiet()).await;
    assert!(matches!(second, AgentExit::Rejected(ref m) if m.contains("已经有")), "{second:?}");

    // 公开行动：完整 GM 回合经 Agent 调本机模型，叙述广播给所有人。
    let v = s.hub.games.read().await.get(&r.room_id).unwrap().version();
    send(&mut h, &ClientMsg::Action { action_id: "h1".into(), state_version: v, text: "我推开灯塔的门".into(), combat: None }).await;
    let (st, reason) = until_done(&mut h, &mut lh, "h1").await;
    assert_eq!(st, ActionState::Done, "{reason}");
    drain(&mut a, &mut la).await;
    assert!(narrations(&la).iter().any(|t| t.contains("旧梯子")), "阿澜也收到 GM 叙述：{:?}", narrations(&la));
    assert!(*model.calls.lock().unwrap() > 0, "模型调用经 Agent 到了房主本机的模型");
    assert!(model.auth.lock().unwrap().iter().all(|a| a == "Bearer local-only-key"), "本机模型的 Key 只在房主电脑上用");

    // 私人 GM 对话：整段回合只给本人。
    let v = s.hub.games.read().await.get(&r.room_id).unwrap().version();
    let before = narrations(&lh).len();
    let mut la2 = vec![];
    send(&mut a, &ClientMsg::PrivateGm { action_id: "a1".into(), state_version: v, text: "我偷偷摸了摸口袋里的信".into() }).await;
    let (st, reason) = until_done(&mut a, &mut la2, "a1").await;
    assert_eq!(st, ActionState::Done, "{reason}");
    assert!(!narrations(&la2).is_empty(), "阿澜收到自己的私人叙述");
    drain(&mut h, &mut lh).await;
    assert_eq!(narrations(&lh).len(), before, "房主收不到阿澜私人 GM 对话的叙述：{:?}", &lh[lh.len().saturating_sub(12)..]);

    // Agent 下线：GM 不就绪，行动被拒绝。
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(agent_task.await.unwrap(), AgentExit::Stopped);
    let g = wait_gm(&mut h, &mut lh, |g| !g.ready).await;
    assert!(!g.agent_online);
    let v = s.hub.games.read().await.get(&r.room_id).unwrap().version();
    send(&mut h, &ClientMsg::Action { action_id: "h2".into(), state_version: v, text: "我往上爬".into(), combat: None }).await;
    assert_eq!(until_done(&mut h, &mut lh, "h2").await.0, ActionState::Rejected);

    // 重连补发全部历史：阿澜的私人回合（她说的话、GM 的回应）仍然不给房主。
    drop(h);
    let (mut h2, _) = join(s.addr, &r.room_id, "", &r.host_token, "").await;
    let mut replay = vec![];
    drain(&mut h2, &mut replay).await;
    let text = serde_json::to_string(&replay).unwrap();
    assert!(text.contains("推开灯塔的门") && !text.contains("口袋里的信"), "补发也按私人范围过滤");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cloud_mode_key_stays_on_server_and_only_host_can_switch() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = || ServerConfig { bind: "127.0.0.1:0".parse().unwrap(), data_dir: dir.path().to_path_buf(), ..Default::default() };
    let (addr, task, hub) = start(cfg()).await.unwrap();
    let (maddr, model) = mock_model().await;
    let r = create_room(&hub, CreateRoom { name: "云端".into(), max_players: Some(4), host_name: "房主".into(), gm_mode: None, template: None }).await.unwrap();
    let (mut h, _) = join(addr, &r.room_id, "", &r.host_token, "").await;
    let (mut a, _) = join(addr, &r.room_id, &r.join_code, "", "阿澜").await;
    let (mut lh, mut la) = (vec![], vec![]);
    drain(&mut h, &mut lh).await;
    drain(&mut a, &mut la).await;
    const KEY: &str = "sk-cloud-very-secret-123";
    let cloud = CloudGmConfig { base_url: format!("http://{maddr}/v1"), model: "mock-cloud".into(), api_key: KEY.into(), temperature: 0.7, context_length: 16384, timeout_secs: 30, max_tokens: 2048 };

    // 不是房主：不能改（服务端判断），连接也不会被断开。
    send(&mut a, &ClientMsg::SetGm { mode: "cloud".into(), cloud: Some(cloud.clone()) }).await;
    drain(&mut a, &mut la).await;
    assert!(la.iter().any(|m| matches!(m, ServerMsg::Error { code: ErrorCode::Forbidden, .. })), "{la:?}");
    send(&mut a, &ClientMsg::Ping { nonce: 7 }).await;
    assert!(matches!(recv(&mut a).await, Some(ServerMsg::Pong { nonce: 7 })), "非房主改设置后连接还在");

    // 房主切到云端：所有人看到 GM 就绪（云端），只知道"有 Key"，看不到 Key。
    send(&mut h, &ClientMsg::SetGm { mode: "cloud".into(), cloud: Some(cloud.clone()) }).await;
    let g = wait_gm(&mut a, &mut la, |g| g.ready).await;
    assert_eq!((g.active.as_str(), g.cloud_model.as_str(), g.cloud_has_key), ("cloud", "mock-cloud", true));

    // 云端回合：服务端带 Key 调云端模型。
    let v = hub.games.read().await.get(&r.room_id).unwrap().version();
    send(&mut a, &ClientMsg::Action { action_id: "a1".into(), state_version: v, text: "我点亮油灯".into(), combat: None }).await;
    let (st, reason) = until_done(&mut a, &mut la, "a1").await;
    assert_eq!(st, ActionState::Done, "{reason}");
    assert!(model.auth.lock().unwrap().iter().all(|x| x == &format!("Bearer {KEY}")) && *model.calls.lock().unwrap() > 0);

    // 再保存一次但 Key 留空：沿用原来的 Key。
    send(&mut h, &ClientMsg::SetGm { mode: "cloud".into(), cloud: Some(CloudGmConfig { api_key: String::new(), ..cloud.clone() }) }).await;
    let g = wait_gm(&mut h, &mut lh, |g| g.cloud_has_key).await;
    assert!(g.ready);

    // auto：没有 Agent 时用云端。
    send(&mut h, &ClientMsg::SetGm { mode: "auto".into(), cloud: None }).await;
    let g = wait_gm(&mut h, &mut lh, |g| g.mode == "auto").await;
    assert_eq!(g.active, "cloud");
    // auto + Agent 上线：改用房主本机。
    let stop = Arc::new(AtomicBool::new(false));
    let agent_task = tokio::spawn(run_agent(agent(addr, &r.room_id, &r.agent_key, maddr), stop.clone(), quiet()));
    let g = wait_gm(&mut h, &mut lh, |g| g.agent_online).await;
    assert_eq!(g.active, "local");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    agent_task.await.unwrap();
    let g = wait_gm(&mut h, &mut lh, |g| !g.agent_online).await;
    assert_eq!(g.active, "cloud", "Agent 下线后自动回到云端");

    // Key 绝不出现在任何发给玩家的消息里，也不在 room.json 里；只在服务端的 gm.json。
    drain(&mut h, &mut lh).await;
    drain(&mut a, &mut la).await;
    for log in [&lh, &la] {
        assert!(!serde_json::to_string(log).unwrap().contains(KEY), "Key 泄露给了玩家");
    }
    let room_dir = dir.path().join("rooms").join(&r.room_id);
    assert!(!std::fs::read_to_string(room_dir.join("room.json")).unwrap().contains(KEY));
    assert!(std::fs::read_to_string(room_dir.join("gm.json")).unwrap().contains(KEY));

    // 服务端重启：GM 设置还在。
    task.abort();
    drop(h);
    drop(a);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let (addr2, _t2, _hub2) = start(cfg()).await.unwrap();
    let (mut h2, _) = join(addr2, &r.room_id, "", &r.host_token, "").await;
    let mut l2 = vec![];
    let g = wait_gm(&mut h2, &mut l2, |_| true).await;
    assert_eq!((g.mode.as_str(), g.active.as_str(), g.cloud_has_key), ("auto", "cloud", true));
}

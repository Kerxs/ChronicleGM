//! 压力测试：开 N 个房间、每间 P 名玩家，持续发聊天和行动，统计延迟、错误和服务端内存。
//!
//! 本地：`cargo run --release -p chronicle-server --example stress -- --local --rooms 20 --players 4 --seconds 60`
//!   （进程内启动服务端，GM 用脚本 GM，验证服务端本身的承载）
//! 远程：`cargo run --release -p chronicle-server --example stress -- --http https://域名 --token 管理员令牌 --rooms 10 --seconds 60`
//!   （真实服务器；房间的 GM 没就绪，行动会被拒绝——测的是连接、聊天、队列与广播）

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use chronicle_protocol::{ClientMsg, PROTOCOL_VERSION, ServerMsg};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

#[derive(Default)]
struct Stats {
    sent: AtomicU64,
    recv: AtomicU64,
    errors: AtomicU64,
    chat_rtt_us: AtomicU64,
    chat_rtt_n: AtomicU64,
    chat_rtt_max_us: AtomicU64,
    actions_done: AtomicU64,
    actions_rejected: AtomicU64,
}

fn arg(args: &[String], k: &str) -> Option<String> {
    args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let rooms: usize = arg(&args, "--rooms").and_then(|v| v.parse().ok()).unwrap_or(10);
    let players: usize = arg(&args, "--players").and_then(|v| v.parse().ok()).unwrap_or(4);
    let seconds: u64 = arg(&args, "--seconds").and_then(|v| v.parse().ok()).unwrap_or(30);
    let local = args.iter().any(|a| a == "--local");

    // 服务端：本地进程内启动，或用远程地址。
    let (http, ws, token, hub) = if local {
        let dir = std::env::temp_dir().join(format!("chronicle-stress-{}", std::process::id()));
        let cfg = chronicle_server::ServerConfig { bind: "127.0.0.1:0".parse().unwrap(), data_dir: dir, max_rooms: rooms + 5, max_connections: rooms * players + 50, ..Default::default() };
        let gm: chronicle_server::GmFor = Arc::new(|_: &str| Arc::new(chronicle_server::ScriptedGm) as Arc<dyn chronicle_server::GmBackend>);
        let (addr, _t, hub) = chronicle_server::start_with_gm(cfg, gm).await.unwrap();
        (format!("http://{addr}"), format!("ws://{addr}"), String::new(), Some(hub))
    } else {
        let http = arg(&args, "--http").expect("--http https://域名").trim_end_matches('/').to_string();
        let ws = http.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
        (http, ws, arg(&args, "--token").unwrap_or_default(), None)
    };

    // 建房
    let mut created = vec![];
    for i in 0..rooms {
        let body = serde_json::json!({ "name": format!("压测{i}"), "max_players": players, "host_name": "房主" }).to_string();
        let (url, tok) = (format!("{http}/rooms"), token.clone());
        let r = tokio::task::spawn_blocking(move || {
            let mut req = ureq::post(&url).header("content-type", "application/json");
            if !tok.is_empty() {
                req = req.header("authorization", &format!("Bearer {tok}"));
            }
            req.send(body.as_str()).map_err(|e| e.to_string()).and_then(|mut r| r.body_mut().read_to_string().map_err(|e| e.to_string()))
        })
        .await
        .unwrap();
        match r.ok().and_then(|t| serde_json::from_str::<chronicle_server::CreatedRoom>(&t).ok()) {
            Some(c) => created.push(c),
            None => eprintln!("第 {i} 个房间没建成"),
        }
    }
    println!("建好 {} 个房间，每间 {players} 人，持续 {seconds} 秒", created.len());

    let stats = Arc::new(Stats::default());
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut tasks = vec![];
    for c in &created {
        for p in 0..players {
            let (ws, room, code, host_token, stats) = (ws.clone(), c.room_id.clone(), c.join_code.clone(), c.host_token.clone(), stats.clone());
            tasks.push(tokio::spawn(async move {
                let Ok((mut sock, _)) = tokio_tungstenite::connect_async(format!("{ws}/ws")).await else {
                    stats.errors.fetch_add(1, Ordering::Relaxed);
                    return;
                };
                let hello = if p == 0 { ClientMsg::Hello { protocol: PROTOCOL_VERSION, room: room.clone(), join_code: String::new(), token: host_token, name: String::new(), last_seq: 0 } } else { ClientMsg::Hello { protocol: PROTOCOL_VERSION, room: room.clone(), join_code: code, token: String::new(), name: format!("玩家{p}"), last_seq: 0 } };
                let _ = sock.send(Message::Text(serde_json::to_string(&hello).unwrap().into())).await;
                let mut me = String::new();
                let mut version = 0u64;
                let mut chat_sent: Option<Instant> = None;
                let mut tick = tokio::time::interval(Duration::from_millis(1000 + p as u64 * 37));
                let mut n = 0u64;
                while Instant::now() < deadline {
                    tokio::select! {
                        _ = tick.tick() => {
                            if me.is_empty() { continue; }
                            n += 1;
                            let m = if n.is_multiple_of(5) {
                                ClientMsg::Action { action_id: format!("{me}-{n}"), state_version: version, text: format!("第 {n} 步"), combat: None }
                            } else {
                                chat_sent = Some(Instant::now());
                                ClientMsg::Chat { to: vec![me.clone()], text: format!("测试 {n}") }
                            };
                            if sock.send(Message::Text(serde_json::to_string(&m).unwrap().into())).await.is_err() { stats.errors.fetch_add(1, Ordering::Relaxed); break; }
                            stats.sent.fetch_add(1, Ordering::Relaxed);
                        }
                        m = sock.next() => {
                            let Some(Ok(Message::Text(t))) = m else { if m.is_none() { break; } continue; };
                            stats.recv.fetch_add(1, Ordering::Relaxed);
                            match serde_json::from_str::<ServerMsg>(&t) {
                                Ok(ServerMsg::Welcome { player, room, .. }) => { me = player; version = room.state_version; }
                                Ok(ServerMsg::Room { room }) => version = version.max(room.state_version),
                                Ok(ServerMsg::Chat { from, .. }) if from == me => {
                                    if let Some(s) = chat_sent.take() {
                                        let us = s.elapsed().as_micros() as u64;
                                        stats.chat_rtt_us.fetch_add(us, Ordering::Relaxed);
                                        stats.chat_rtt_n.fetch_add(1, Ordering::Relaxed);
                                        stats.chat_rtt_max_us.fetch_max(us, Ordering::Relaxed);
                                    }
                                }
                                Ok(ServerMsg::ActionStatus { status, state_version, .. }) => {
                                    version = version.max(state_version);
                                    match status {
                                        chronicle_protocol::ActionState::Done => { stats.actions_done.fetch_add(1, Ordering::Relaxed); }
                                        chronicle_protocol::ActionState::Rejected => { stats.actions_rejected.fetch_add(1, Ordering::Relaxed); }
                                        _ => {}
                                    }
                                }
                                Ok(ServerMsg::Error { code, message }) if !matches!(code, chronicle_protocol::ErrorCode::StaleVersion | chronicle_protocol::ErrorCode::Duplicate | chronicle_protocol::ErrorCode::GmUnavailable) => {
                                    stats.errors.fetch_add(1, Ordering::Relaxed);
                                    if stats.errors.load(Ordering::Relaxed) <= 5 { eprintln!("服务端错误 {code:?}：{message}"); }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                let _ = sock.close(None).await;
            }));
        }
    }
    for t in tasks {
        let _ = t.await;
    }
    let s = &stats;
    let n = s.chat_rtt_n.load(Ordering::Relaxed).max(1);
    println!("连接 {}，发送 {} 条，收到 {} 条，错误 {}", created.len() * players, s.sent.load(Ordering::Relaxed), s.recv.load(Ordering::Relaxed), s.errors.load(Ordering::Relaxed));
    println!("聊天往返：平均 {:.1} ms，最慢 {:.1} ms（{} 次）", s.chat_rtt_us.load(Ordering::Relaxed) as f64 / n as f64 / 1000.0, s.chat_rtt_max_us.load(Ordering::Relaxed) as f64 / 1000.0, n);
    println!("行动：完成 {}，拒绝 {}", s.actions_done.load(Ordering::Relaxed), s.actions_rejected.load(Ordering::Relaxed));
    let health = tokio::task::spawn_blocking(move || ureq::get(&format!("{http}/health")).call().ok().and_then(|mut r| r.body_mut().read_to_string().ok())).await.unwrap();
    println!("服务端健康：{}", health.unwrap_or_else(|| "（读不到）".into()));
    drop(hub);
}

//! 第 1 阶段：服务端骨架——健康检查、握手、协议版本、消息大小、限流、连接数上限。

mod common;

use chronicle_protocol::{ClientMsg, ErrorCode, PROTOCOL_VERSION, ServerMsg};
use common::*;

fn hello(protocol: u32) -> ClientMsg {
    ClientMsg::Hello { protocol, room: "nope".into(), join_code: String::new(), token: String::new(), name: "测试".into(), last_seq: 0 }
}

#[tokio::test]
async fn health_reports_state() {
    let s = server(|_| {}).await;
    let addr = s.addr;
    let body: serde_json::Value = tokio::task::spawn_blocking(move || ureq::get(&format!("http://{addr}/health")).call().unwrap().body_mut().read_to_string().map(|s| serde_json::from_str::<serde_json::Value>(&s).unwrap()).unwrap()).await.unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["protocol"], PROTOCOL_VERSION);
    assert_eq!(body["rooms"], 0);
    assert!(body["uptime_secs"].is_number() && body["rss_mb"].is_number());
}

#[tokio::test]
async fn handshake_checks_protocol_and_room() {
    let s = server(|_| {}).await;
    let mut ws = connect(s.addr).await;
    send(&mut ws, &hello(PROTOCOL_VERSION + 1)).await;
    assert!(matches!(recv(&mut ws).await, Some(ServerMsg::Error { code: ErrorCode::BadProtocol, .. })));
    let mut ws = connect(s.addr).await;
    send(&mut ws, &hello(PROTOCOL_VERSION)).await;
    assert!(matches!(recv(&mut ws).await, Some(ServerMsg::Error { code: ErrorCode::RoomNotFound, .. })));
    // 没加入房间就提交行动：拒绝。
    send(&mut ws, &ClientMsg::Action { action_id: "a".into(), state_version: 0, text: "x".into(), combat: None }).await;
    assert!(matches!(recv(&mut ws).await, Some(ServerMsg::Error { code: ErrorCode::Unauthorized, .. })));
    send(&mut ws, &ClientMsg::Ping { nonce: 7 }).await;
    assert_eq!(recv(&mut ws).await, Some(ServerMsg::Pong { nonce: 7 }));
}

#[tokio::test]
async fn oversized_and_flooding_messages_are_refused() {
    let s = server(|c| c.max_msgs_per_sec = 3).await;
    let mut ws = connect(s.addr).await;
    send(&mut ws, &hello(PROTOCOL_VERSION)).await;
    let _ = recv(&mut ws).await;
    // 刷屏：一秒内第 4 条起被限流。
    let mut limited = false;
    for i in 0..6 {
        send(&mut ws, &ClientMsg::Ping { nonce: i }).await;
    }
    for _ in 0..6 {
        if let Some(ServerMsg::Error { code: ErrorCode::RateLimited, .. }) = recv(&mut ws).await {
            limited = true;
        }
    }
    assert!(limited, "刷屏被限流");
    // 超大消息：服务端直接断开连接（不读进内存）。
    let big = "x".repeat(chronicle_protocol::limits::MAX_FRAME_BYTES * 4);
    let _ = send_raw(&mut ws, big).await;
    assert!(recv(&mut ws).await.is_none(), "超过大小限制的连接被断开");
}

#[tokio::test]
async fn connection_cap_is_enforced() {
    let s = server(|c| c.max_connections = 2).await;
    let mut a = connect(s.addr).await;
    let mut b = connect(s.addr).await;
    send(&mut a, &ClientMsg::Ping { nonce: 1 }).await;
    send(&mut b, &ClientMsg::Ping { nonce: 1 }).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let third = tokio_tungstenite::connect_async(format!("ws://{}/ws", s.addr)).await;
    assert!(third.is_err(), "第 3 个连接被拒绝");
}

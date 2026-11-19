//! 服务端这一侧的 GM Agent 连接（`/agent`）。
//!
//! 房主电脑上的 GM Agent 主动连上来（房主的本机模型端口不暴露到公网），用建房时拿到的 Agent 密钥注册。
//! 同一房间同一时刻只接受一个 Agent（唯一的权威 GM 节点）；45 秒没有心跳就当作断开，
//! 断开时正在等结果的模型调用立即失败（行动被拒绝并说明原因，不会卡住队列）。

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chronicle_protocol::{AgentMsg, AgentTask, PROTOCOL_VERSION};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use crate::gm::{AgentConn, Pending};
use crate::{Shared, rooms};

/// Agent 回传的是模型原始输出，比玩家消息大得多，单独放宽到 1 MB。
const AGENT_FRAME_BYTES: usize = 1 << 20;
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(45);

pub async fn upgrade(State(hub): State<Shared>, ws: WebSocketUpgrade) -> Response {
    if hub.gmhub.is_none() {
        return (StatusCode::NOT_FOUND, "这个服务端没有启用 AI GM").into_response();
    }
    if hub.stats.connections.load(Ordering::Relaxed) >= hub.cfg.max_connections {
        hub.stats.rejected.fetch_add(1, Ordering::Relaxed);
        return (StatusCode::SERVICE_UNAVAILABLE, "连接数已满").into_response();
    }
    ws.max_message_size(AGENT_FRAME_BYTES).max_frame_size(AGENT_FRAME_BYTES).on_upgrade(move |s| agent_loop(hub, s))
}

async fn send(sink: &mut futures_util::stream::SplitSink<WebSocket, Message>, t: &AgentTask) -> bool {
    match serde_json::to_string(t) {
        Ok(s) => sink.send(Message::Text(s.into())).await.is_ok(),
        Err(_) => false,
    }
}

async fn agent_loop(hub: Shared, socket: WebSocket) {
    hub.stats.connections.fetch_add(1, Ordering::Relaxed);
    agent_session(&hub, socket).await;
    hub.stats.connections.fetch_sub(1, Ordering::Relaxed);
}

async fn agent_session(hub: &Shared, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let Some(gmhub) = hub.gmhub.clone() else { return };
    // 第一条必须是注册消息（10 秒内）。
    let first = tokio::time::timeout(Duration::from_secs(10), stream.next()).await.ok().flatten();
    let reg = match first {
        Some(Ok(Message::Text(t))) => serde_json::from_str::<AgentMsg>(&t).ok(),
        _ => None,
    };
    let Some(AgentMsg::Register { protocol, room: room_id, agent_key, model }) = reg else {
        send(&mut sink, &AgentTask::Registered { ok: false, reason: "第一条消息必须是注册".into() }).await;
        return;
    };
    let reject = |reason: &str| AgentTask::Registered { ok: false, reason: reason.to_string() };
    if protocol != PROTOCOL_VERSION {
        send(&mut sink, &reject(&format!("协议版本不一致（服务端 {PROTOCOL_VERSION}，Agent {protocol}），请更新软件"))).await;
        return;
    }
    let Some(room) = hub.rooms.read().await.get(&room_id).cloned() else {
        send(&mut sink, &reject("房间不存在")).await;
        return;
    };
    let key_ok = {
        let m = room.meta.lock().await;
        !agent_key.is_empty() && !m.agent_key_hash.is_empty() && rooms::hash_secret(&agent_key) == m.agent_key_hash
    };
    if !key_ok {
        hub.stats.rejected.fetch_add(1, Ordering::Relaxed);
        hub.log.warn(&format!("房间 {room_id} 的 GM Agent 密钥不对，已拒绝"));
        send(&mut sink, &reject("Agent 密钥不对")).await;
        return;
    }
    let rgm = gmhub.room(&room_id, &room.dir, "auto");
    let conn = hub.next_conn.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::channel::<AgentTask>(32);
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let installed = match rgm.agent.lock() {
        Ok(mut slot) if slot.is_none() => {
            *slot = Some(AgentConn { conn, model: model.chars().take(80).collect(), tx: tx.clone(), pending: pending.clone() });
            true
        }
        _ => false,
    };
    if !installed {
        send(&mut sink, &reject("这个房间已经有一个 GM Agent 在线（同一房间只能有一个 GM 节点）")).await;
        return;
    }
    if !send(&mut sink, &AgentTask::Registered { ok: true, reason: String::new() }).await {
        clear(&rgm, conn);
        return;
    }
    hub.log.info(&format!("房间 {room_id} 的 GM Agent 上线（模型 {model}）"));
    hub.notify_gm(&room_id).await;
    drop(tx);

    let writer = tokio::spawn(async move {
        while let Some(t) = rx.recv().await {
            if !send(&mut sink, &t).await {
                break;
            }
        }
        let _ = sink.close().await;
    });
    loop {
        let msg = match tokio::time::timeout(HEARTBEAT_TIMEOUT, stream.next()).await {
            Ok(Some(Ok(m))) => m,
            Ok(_) => break,
            Err(_) => {
                hub.log.warn(&format!("房间 {room_id} 的 GM Agent 心跳超时"));
                break;
            }
        };
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
        match serde_json::from_str::<AgentMsg>(&text) {
            Ok(AgentMsg::TaskResult { task_id, ok, content, error }) => {
                let waiter = pending.lock().ok().and_then(|mut p| p.remove(&task_id));
                if let Some(w) = waiter {
                    let _ = w.send(if ok { Ok(content) } else { Err(error) });
                }
            }
            Ok(AgentMsg::Heartbeat { .. }) | Ok(AgentMsg::Register { .. }) => {}
            Err(_) => {
                hub.stats.rejected.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    clear(&rgm, conn);
    // 正在等结果的调用立即失败。
    if let Ok(mut p) = pending.lock() {
        for (_, w) in p.drain() {
            let _ = w.send(Err("房主的 GM Agent 已断开".into()));
        }
    }
    writer.abort();
    hub.log.info(&format!("房间 {room_id} 的 GM Agent 下线"));
    hub.notify_gm(&room_id).await;
}

/// 只清掉自己这条连接（不影响之后重新连上的新 Agent）。
fn clear(rgm: &crate::gm::RoomGm, conn: u64) {
    if let Ok(mut slot) = rgm.agent.lock()
        && slot.as_ref().is_some_and(|a| a.conn == conn)
    {
        *slot = None;
    }
}

//! 测试工具：在临时目录、随机端口启动服务端；WebSocket 客户端收发 JSON。
#![allow(dead_code)]

use std::net::SocketAddr;

use chronicle_protocol::{ClientMsg, ServerMsg};
use chronicle_server::{ServerConfig, Shared, start};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

pub struct TestServer {
    pub addr: SocketAddr,
    pub hub: Shared,
    pub _dir: tempfile::TempDir,
}

pub async fn server(tweak: impl FnOnce(&mut ServerConfig)) -> TestServer {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = ServerConfig { bind: "127.0.0.1:0".parse().unwrap(), data_dir: dir.path().to_path_buf(), ..Default::default() };
    tweak(&mut cfg);
    let (addr, _task, hub) = start(cfg).await.unwrap();
    TestServer { addr, hub, _dir: dir }
}

pub type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub async fn connect(addr: SocketAddr) -> Ws {
    tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.unwrap().0
}

pub async fn send(ws: &mut Ws, m: &ClientMsg) {
    ws.send(Message::Text(serde_json::to_string(m).unwrap().into())).await.unwrap();
}

pub async fn send_raw(ws: &mut Ws, s: String) -> bool {
    ws.send(Message::Text(s.into())).await.is_ok()
}

/// 收下一条服务端消息（2 秒超时）。
pub async fn recv(ws: &mut Ws) -> Option<ServerMsg> {
    loop {
        let m = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next()).await.ok()??.ok()?;
        match m {
            Message::Text(t) => return serde_json::from_str(&t).ok(),
            Message::Close(_) => return None,
            _ => continue,
        }
    }
}

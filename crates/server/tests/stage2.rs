//! 第 2 阶段：房间、身份、权限、聊天。

mod common;

use chronicle_protocol::{ClientMsg, ErrorCode, PROTOCOL_VERSION, ServerMsg};
use chronicle_server::{CreateRoom, create_room};
use common::*;

fn hello(room: &str, code: &str, token: &str, name: &str) -> ClientMsg {
    ClientMsg::Hello { protocol: PROTOCOL_VERSION, room: room.into(), join_code: code.into(), token: token.into(), name: name.into(), last_seq: 0 }
}

/// 加入房间，返回（连接，玩家 ID，令牌）。会把 Welcome 之后的消息先读掉。
async fn join(addr: std::net::SocketAddr, room: &str, code: &str, token: &str, name: &str) -> (Ws, String, String) {
    let mut ws = connect(addr).await;
    send(&mut ws, &hello(room, code, token, name)).await;
    match recv(&mut ws).await {
        Some(ServerMsg::Welcome { player, token: t, .. }) => (ws, player, if t.is_empty() { token.to_string() } else { t }),
        other => panic!("{name} 没能加入：{other:?}"),
    }
}

/// 读消息直到满足条件（最多读 20 条）。
async fn until(ws: &mut Ws, f: impl Fn(&ServerMsg) -> bool) -> Option<ServerMsg> {
    for _ in 0..20 {
        let m = recv(ws).await?;
        if f(&m) {
            return Some(m);
        }
    }
    None
}

/// 把当前已经到达的消息全部读掉，返回它们。
async fn drain(ws: &mut Ws) -> Vec<ServerMsg> {
    let mut v = vec![];
    while let Ok(Some(m)) = tokio::time::timeout(std::time::Duration::from_millis(300), recv(ws)).await {
        v.push(m);
    }
    v
}

#[tokio::test]
async fn four_players_join_fifth_is_refused_and_bad_code_rejected() {
    let s = server(|c| c.max_players = 4).await;
    let r = create_room(&s.hub, CreateRoom { name: "雾湾".into(), max_players: None, host_name: "房主".into(), gm_mode: None, template: None }).await.unwrap();
    let (_h, host, _) = join(s.addr, &r.room_id, "", &r.host_token, "").await;
    assert_eq!(host, r.host_player, "房主用令牌进入");
    let mut ws = connect(s.addr).await;
    send(&mut ws, &hello(&r.room_id, "WRONG1", "", "路人")).await;
    assert!(matches!(recv(&mut ws).await, Some(ServerMsg::Error { code: ErrorCode::Unauthorized, .. })));
    let (_a, _, _) = join(s.addr, &r.room_id, &r.join_code, "", "阿澜").await;
    let (_b, _, _) = join(s.addr, &r.room_id, &r.join_code.to_lowercase(), "", "柯拉").await;
    let (mut c, _, _) = join(s.addr, &r.room_id, &r.join_code, "", "苇生").await;
    let mut e = connect(s.addr).await;
    send(&mut e, &hello(&r.room_id, &r.join_code, "", "第五人")).await;
    assert!(matches!(recv(&mut e).await, Some(ServerMsg::Error { code: ErrorCode::RoomFull, .. })));
    let info = until(&mut c, |m| matches!(m, ServerMsg::Room { .. })).await;
    if let Some(ServerMsg::Room { room }) = info {
        assert_eq!(room.players.len(), 4);
        assert!(room.players.iter().filter(|p| p.host).count() == 1);
    }
}

#[tokio::test]
async fn private_chat_never_reaches_others_and_history_is_filtered_on_reconnect() {
    let s = server(|_| {}).await;
    let r = create_room(&s.hub, CreateRoom { name: "密谈".into(), max_players: Some(4), host_name: "房主".into(), gm_mode: None, template: None }).await.unwrap();
    let (mut h, host, _) = join(s.addr, &r.room_id, "", &r.host_token, "").await;
    let (mut a, alan, alan_token) = join(s.addr, &r.room_id, &r.join_code, "", "阿澜").await;
    let (mut b, _kola, _) = join(s.addr, &r.room_id, &r.join_code, "", "柯拉").await;
    drain(&mut h).await;
    drain(&mut a).await;
    drain(&mut b).await;
    send(&mut h, &ClientMsg::Chat { to: vec![], text: "大家好".into() }).await;
    send(&mut h, &ClientMsg::Chat { to: vec![alan.clone()], text: "只告诉阿澜：钥匙在铁砧下".into() }).await;
    let got_a = drain(&mut a).await;
    let got_b = drain(&mut b).await;
    let texts = |v: &[ServerMsg]| v.iter().filter_map(|m| if let ServerMsg::Chat { text, .. } = m { Some(text.clone()) } else { None }).collect::<Vec<_>>();
    assert_eq!(texts(&got_a), vec!["大家好", "只告诉阿澜：钥匙在铁砧下"]);
    assert_eq!(texts(&got_b), vec!["大家好"], "柯拉收不到私聊");
    // 私聊对象不存在：拒绝。
    send(&mut h, &ClientMsg::Chat { to: vec!["p_nobody".into()], text: "x".into() }).await;
    assert!(until(&mut h, |m| matches!(m, ServerMsg::Error { code: ErrorCode::BadMessage, .. })).await.is_some());
    // 阿澜断线后用令牌重连：同一个身份，历史里有私聊；旧连接被踢掉。
    let (mut a2, alan2, _) = join(s.addr, &r.room_id, "", &alan_token, "").await;
    assert_eq!(alan2, alan);
    assert!(until(&mut a, |m| matches!(m, ServerMsg::Error { code: ErrorCode::Unauthorized, .. })).await.is_some(), "旧连接收到被顶下线的通知");
    let hist = texts(&drain(&mut a2).await);
    assert!(hist.contains(&"只告诉阿澜：钥匙在铁砧下".to_string()));
    // 柯拉重连看到的历史里没有私聊。
    let (mut b2, _, _) = join(s.addr, &r.room_id, &r.join_code, "", "旁观者").await;
    let hist_b = texts(&drain(&mut b2).await);
    assert!(hist_b.contains(&"大家好".to_string()) && !hist_b.iter().any(|t| t.contains("铁砧")), "{hist_b:?}");
    let _ = host;
}

#[tokio::test]
async fn rooms_and_tokens_survive_a_server_restart() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = |d: &std::path::Path| chronicle_server::ServerConfig { bind: "127.0.0.1:0".parse().unwrap(), data_dir: d.to_path_buf(), ..Default::default() };
    let (addr, task, hub) = chronicle_server::start(cfg(dir.path())).await.unwrap();
    let r = create_room(&hub, CreateRoom { name: "重启".into(), max_players: None, host_name: "房主".into(), gm_mode: Some("cloud".into()), template: None }).await.unwrap();
    let (mut a, alan, token) = join(addr, &r.room_id, &r.join_code, "", "阿澜").await;
    send(&mut a, &ClientMsg::Chat { to: vec![], text: "重启前的话".into() }).await;
    drain(&mut a).await;
    task.abort();
    drop(a);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let (addr2, _t2, hub2) = chronicle_server::start(cfg(dir.path())).await.unwrap();
    assert_eq!(hub2.rooms.read().await.len(), 1, "房间从磁盘恢复");
    let (mut a2, alan2, _) = join(addr2, &r.room_id, "", &token, "").await;
    assert_eq!(alan2, alan, "令牌重启后仍然有效");
    let hist = drain(&mut a2).await;
    assert!(hist.iter().any(|m| matches!(m, ServerMsg::Chat { text, .. } if text == "重启前的话")));
    // 磁盘上只有令牌哈希。
    let meta = std::fs::read_to_string(dir.path().join("rooms").join(&r.room_id).join("room.json")).unwrap();
    assert!(!meta.contains(&token) && !meta.contains(&r.host_token) && !meta.contains(&r.agent_key));
}

#[tokio::test]
async fn http_room_creation_needs_admin_token_when_configured() {
    let s = server(|c| c.admin_token = Some("s3cret".into())).await;
    let addr = s.addr;
    let body = r#"{"name":"云房间","host_name":"房主"}"#;
    let (no, yes) = tokio::task::spawn_blocking(move || {
        let no = ureq::post(&format!("http://{addr}/rooms")).header("content-type", "application/json").send(body).map(|r| r.status().as_u16()).unwrap_or_else(|e| match e {
            ureq::Error::StatusCode(c) => c,
            _ => 0,
        });
        let yes = ureq::post(&format!("http://{addr}/rooms")).header("content-type", "application/json").header("authorization", "Bearer s3cret").send(body).map(|r| r.status().as_u16()).unwrap_or(0);
        (no, yes)
    })
    .await
    .unwrap();
    assert_eq!(no, 401);
    assert_eq!(yes, 200);
    assert_eq!(s.hub.rooms.read().await.len(), 1);
}

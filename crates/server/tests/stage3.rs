//! 第 3 阶段：行动队列、去重、状态版本、按权限广播、断线补发、重启恢复、过滤后的快照。

mod common;

use std::sync::Arc;

use chronicle_protocol::{ActionState, ClientMsg, ErrorCode, PROTOCOL_VERSION, ServerMsg};
use chronicle_server::{CreateRoom, GmBackend, ScriptedGm, ServerConfig, create_room, start_with_gm};
use common::*;

fn gm() -> chronicle_server::GmFor {
    Arc::new(|_: &str| Arc::new(ScriptedGm) as Arc<dyn GmBackend>)
}

async fn boot(dir: &std::path::Path) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>, chronicle_server::Shared) {
    start_with_gm(ServerConfig { bind: "127.0.0.1:0".parse().unwrap(), data_dir: dir.to_path_buf(), ..Default::default() }, gm()).await.unwrap()
}

async fn join(addr: std::net::SocketAddr, room: &str, code: &str, token: &str, name: &str, last_seq: i64) -> (Ws, String, String) {
    let mut ws = connect(addr).await;
    send(&mut ws, &ClientMsg::Hello { protocol: PROTOCOL_VERSION, room: room.into(), join_code: code.into(), token: token.into(), name: name.into(), last_seq }).await;
    match recv(&mut ws).await {
        Some(ServerMsg::Welcome { player, token: t, .. }) => (ws, player, if t.is_empty() { token.to_string() } else { t }),
        other => panic!("{name} 没能加入：{other:?}"),
    }
}

async fn drain(ws: &mut Ws) -> Vec<ServerMsg> {
    let mut v = vec![];
    while let Ok(Some(m)) = tokio::time::timeout(std::time::Duration::from_millis(400), recv(ws)).await {
        v.push(m);
    }
    v
}

/// 等这个行动结束（完成或被拒绝），返回它之前收到的全部消息。
async fn until_done(ws: &mut Ws, action: &str) -> (ActionState, Vec<ServerMsg>) {
    let mut seen = vec![];
    for _ in 0..60 {
        let Some(m) = recv(ws).await else { break };
        if let ServerMsg::ActionStatus { action_id, status, .. } = &m
            && action_id == action
            && matches!(status, ActionState::Done | ActionState::Rejected)
        {
            return (*status, seen);
        }
        seen.push(m);
    }
    panic!("行动 {action} 没有结束：{seen:?}");
}

fn act(id: &str, v: u64, text: &str) -> ClientMsg {
    ClientMsg::Action { action_id: id.into(), state_version: v, text: text.into(), combat: None }
}

fn narrations(v: &[ServerMsg]) -> Vec<String> {
    v.iter().filter_map(|m| if let ServerMsg::Event { kind, payload, .. } = m { (kind == "narration").then(|| payload["text"].as_str().unwrap_or_default().to_string()) } else { None }).collect()
}

#[tokio::test]
async fn actions_are_queued_resolved_by_server_and_broadcast_per_visibility() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, _t, hub) = boot(dir.path()).await;
    let r = create_room(&hub, CreateRoom { name: "队列".into(), max_players: Some(4), host_name: "房主".into(), gm_mode: None, template: None }).await.unwrap();
    let (mut h, _host, _) = join(addr, &r.room_id, "", &r.host_token, "", 0).await;
    let (mut a, _alan, _) = join(addr, &r.room_id, &r.join_code, "", "阿澜", 0).await;
    drain(&mut h).await;
    drain(&mut a).await;
    // 公开行动：所有人都收到叙述和服务端掷骰。
    send(&mut h, &act("h1", 0, "我推开灯塔的门")).await;
    let (st, seen_h) = until_done(&mut h, "h1").await;
    assert_eq!(st, ActionState::Done);
    let seen_a = drain(&mut a).await;
    assert!(narrations(&seen_h).iter().any(|t| t.contains("推开灯塔的门")));
    assert!(narrations(&seen_a).iter().any(|t| t.contains("推开灯塔的门")), "其他玩家也收到公开叙述");
    assert!(seen_a.iter().any(|m| matches!(m, ServerMsg::Event { kind, .. } if kind == "roll")), "骰子由服务端掷，结果广播");
    // 私人 GM 对话：只有本人收到。
    send(&mut a, &ClientMsg::PrivateGm { action_id: "a1".into(), state_version: 1, text: "我偷偷看口袋里的信".into() }).await;
    let (st, seen_a) = until_done(&mut a, "a1").await;
    assert_eq!(st, ActionState::Done);
    assert!(narrations(&seen_a).iter().any(|t| t.contains("口袋里的信")));
    let seen_h = drain(&mut h).await;
    assert!(!narrations(&seen_h).iter().any(|t| t.contains("口袋里的信")), "房主收不到阿澜的私人 GM 对话");
    assert!(!seen_h.iter().any(|m| matches!(m, ServerMsg::Event { kind, .. } if kind == "mp_action")), "私人行动的记录也不给别人");
    // GM 专用事件（AI 方案）和原始状态差异绝不发给玩家。
    {
        let games = hub.games.read().await;
        let g = games.get(&r.room_id).unwrap();
        let w = g.world.lock().unwrap();
        chronicle_core::campaign::record(w.conn(), &g.campaign_id, "gm_plan", &serde_json::json!({ "secret": "真凶是镇长" })).unwrap();
    }
    send(&mut h, &act("h2", 2, "我看看四周")).await;
    let (_, seen_h) = until_done(&mut h, "h2").await;
    let seen_a = drain(&mut a).await;
    for v in [&seen_h, &seen_a] {
        assert!(!v.iter().any(|m| matches!(m, ServerMsg::Event { kind, .. } if kind == "gm_plan" || kind == "effects")), "GM 专用事件不广播");
        assert!(!serde_json::to_string(v).unwrap().contains("真凶"));
    }
}

#[tokio::test]
async fn duplicates_stale_versions_and_double_submits_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, _t, hub) = boot(dir.path()).await;
    let r = create_room(&hub, CreateRoom { name: "规矩".into(), max_players: Some(4), host_name: "房主".into(), gm_mode: None, template: None }).await.unwrap();
    let (mut h, _, _) = join(addr, &r.room_id, "", &r.host_token, "", 0).await;
    drain(&mut h).await;
    send(&mut h, &act("x1", 0, "第一步")).await;
    assert_eq!(until_done(&mut h, "x1").await.0, ActionState::Done);
    // 同一个 action_id 再提交：拒绝（不会执行两次）。
    send(&mut h, &act("x1", 1, "第一步（重发）")).await;
    assert_eq!(until_done(&mut h, "x1").await.0, ActionState::Rejected);
    // 用过期的状态版本提交：拒绝。
    send(&mut h, &act("x2", 0, "按旧局面行动")).await;
    let (st, seen) = until_done(&mut h, "x2").await;
    assert_eq!(st, ActionState::Rejected);
    assert!(seen.iter().any(|m| matches!(m, ServerMsg::Error { code: ErrorCode::StaleVersion, .. })) || drain(&mut h).await.iter().any(|m| matches!(m, ServerMsg::Error { code: ErrorCode::StaleVersion, .. })));
}

#[tokio::test]
async fn reconnect_replays_missed_events_and_restart_keeps_state_and_dedupe() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, task, hub) = boot(dir.path()).await;
    let r = create_room(&hub, CreateRoom { name: "恢复".into(), max_players: Some(4), host_name: "房主".into(), gm_mode: None, template: None }).await.unwrap();
    let (mut h, _, _) = join(addr, &r.room_id, "", &r.host_token, "", 0).await;
    let (mut a, _alan, alan_token) = join(addr, &r.room_id, &r.join_code, "", "阿澜", 0).await;
    drain(&mut h).await;
    let seen = drain(&mut a).await;
    let mut last_seq = seen.iter().filter_map(|m| if let ServerMsg::Event { seq, .. } = m { Some(*seq) } else { None }).max().unwrap_or(0);
    send(&mut a, &act("a1", 0, "阿澜先走一步")).await;
    let (_, seen) = until_done(&mut a, "a1").await;
    last_seq = seen.iter().filter_map(|m| if let ServerMsg::Event { seq, .. } = m { Some(*seq) } else { None }).max().unwrap_or(last_seq);
    // 阿澜断线期间房主行动。
    drop(a);
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    send(&mut h, &act("h1", 1, "房主点亮了油灯")).await;
    assert_eq!(until_done(&mut h, "h1").await.0, ActionState::Done);
    // 重连带上看到的最后序号：只补发错过的事件。
    let (mut a2, _, _) = join(addr, &r.room_id, "", &alan_token, "", last_seq).await;
    let replay = drain(&mut a2).await;
    let n = narrations(&replay);
    assert!(n.iter().any(|t| t.contains("点亮了油灯")), "补发断线期间的叙述：{n:?}");
    assert!(!n.iter().any(|t| t.contains("阿澜先走一步")), "已经看过的不重复发");
    // 服务端重启：状态版本、去重记录都还在。
    task.abort();
    drop(h);
    drop(a2);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let (addr2, _t2, hub2) = boot(dir.path()).await;
    assert_eq!(hub2.games.read().await.get(&r.room_id).unwrap().version(), 2, "已提交的两个行动");
    let (mut h2, _, _) = join(addr2, &r.room_id, "", &r.host_token, "", 0).await;
    drain(&mut h2).await;
    send(&mut h2, &act("h1", 2, "重启后重发同一个行动")).await;
    assert_eq!(until_done(&mut h2, "h1").await.0, ActionState::Rejected, "重启后仍能识别重复的行动");
}

#[tokio::test]
async fn snapshots_show_my_private_data_but_not_other_players() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, _t, hub) = boot(dir.path()).await;
    let r = create_room(&hub, CreateRoom { name: "背包".into(), max_players: Some(4), host_name: "房主".into(), gm_mode: None, template: None }).await.unwrap();
    let (mut h, host, _) = join(addr, &r.room_id, "", &r.host_token, "", 0).await;
    let (mut a, alan, _) = join(addr, &r.room_id, &r.join_code, "", "阿澜", 0).await;
    // 服务端给阿澜一件私人物品（经规则引擎：带来源、写事件日志）。
    {
        let games = hub.games.read().await;
        let g = games.get(&r.room_id).unwrap();
        let mut w = g.world.lock().unwrap();
        let kit = vec![chronicle_core::pc::StartItem { name: "密信".into(), qty: 1, ..Default::default() }];
        chronicle_core::rpg::grant_start_kit_for(&mut w, &g.campaign_id, &chronicle_server::game::actor_of(&alan), &kit).unwrap();
    }
    drain(&mut h).await;
    drain(&mut a).await;
    send(&mut a, &ClientMsg::GetState).await;
    send(&mut h, &ClientMsg::GetState).await;
    let snap = |v: Vec<ServerMsg>| v.into_iter().find_map(|m| if let ServerMsg::Snapshot { data, .. } = m { Some(data) } else { None }).unwrap();
    let sa = snap(drain(&mut a).await);
    let sh = snap(drain(&mut h).await);
    assert!(sa["me"]["inventory"].to_string().contains("密信"));
    assert!(!sh.to_string().contains("密信"), "房主的快照里没有阿澜的背包");
    assert!(sh["party"].as_array().unwrap().iter().any(|p| p["player"] == alan.as_str()), "队友的公开信息在");
    let _ = host;
}

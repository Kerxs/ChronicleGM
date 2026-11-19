//! 第 5 阶段：多人 GM 回合与战斗。
//! - 每个玩家是规则引擎里自己的行动者：自己的角色卡（用玩家名字）、背包、战斗单位；
//! - 战斗：所有在场玩家都进战场，各控一个单位；只有轮到的玩家能行动（服务端判断），战斗按钮由规则引擎结算；
//! - 战场广播不带战利品、经验这类结算数据；被拒绝的行动不推进状态版本（重启后也一致）。

mod common;

use std::sync::Arc;

use chronicle_core::{combat, rpg};
use chronicle_protocol::{ActionState, ClientMsg, CombatIntent, PROTOCOL_VERSION, ServerMsg};
use chronicle_server::game::{GameRoom, GmBackend, QueuedAction, actor_of};
use chronicle_server::{CreateRoom, GmFor, ServerConfig, create_room, start_with_gm};
use common::*;

/// 测试 GM：给"当前行动的玩家"一件物品（验证服务端用对了行动者），并写一段公开叙述。
struct KitGm;
impl GmBackend for KitGm {
    fn ready(&self) -> bool {
        true
    }
    fn mode(&self) -> String {
        "scripted".into()
    }
    fn take_turn(&self, game: &GameRoom, a: &QueuedAction) -> Result<(), String> {
        let mut w = game.world.lock().unwrap();
        let item = if a.text.contains("火把") { "火把" } else { "绳子" };
        let me = rpg::actor();
        rpg::grant_start_kit_for(&mut w, &game.campaign_id, &me, &[chronicle_core::pc::StartItem { name: item.into(), qty: 1, ..Default::default() }]).map_err(|e| e.to_string())?;
        chronicle_core::campaign::record(w.conn(), &game.campaign_id, "narration", &serde_json::json!({ "text": format!("{}拿到了{item}", a.name) })).map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn gm() -> GmFor {
    Arc::new(|_: &str| Arc::new(KitGm) as Arc<dyn GmBackend>)
}

async fn boot(dir: &std::path::Path) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>, chronicle_server::Shared) {
    start_with_gm(ServerConfig { bind: "127.0.0.1:0".parse().unwrap(), data_dir: dir.to_path_buf(), ..Default::default() }, gm()).await.unwrap()
}

async fn join(addr: std::net::SocketAddr, room: &str, code: &str, token: &str, name: &str) -> (Ws, String) {
    let mut ws = connect(addr).await;
    send(&mut ws, &ClientMsg::Hello { protocol: PROTOCOL_VERSION, room: room.into(), join_code: code.into(), token: token.into(), name: name.into(), last_seq: 0 }).await;
    match recv(&mut ws).await {
        Some(ServerMsg::Welcome { player, .. }) => (ws, player),
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

async fn until_done(ws: &mut Ws, action: &str) -> (ActionState, String, Vec<ServerMsg>) {
    let mut seen = vec![];
    for _ in 0..80 {
        let Some(m) = recv(ws).await else { break };
        if let ServerMsg::ActionStatus { action_id, status, reason, .. } = &m
            && action_id == action
            && matches!(status, ActionState::Done | ActionState::Rejected)
        {
            return (*status, reason.clone(), seen);
        }
        seen.push(m);
    }
    panic!("行动 {action} 没有结束：{seen:?}");
}

async fn snapshot(ws: &mut Ws) -> serde_json::Value {
    send(ws, &ClientMsg::GetState).await;
    drain(ws).await.into_iter().find_map(|m| if let ServerMsg::Snapshot { data, .. } = m { Some(data) } else { None }).unwrap()
}

async fn version(hub: &chronicle_server::Shared, room: &str) -> u64 {
    hub.games.read().await.get(room).unwrap().version()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_player_has_own_sheet_and_inventory() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, _t, hub) = boot(dir.path()).await;
    let r = create_room(&hub, CreateRoom { name: "分开".into(), max_players: Some(4), host_name: "房主".into(), gm_mode: None, template: None }).await.unwrap();
    let (mut h, host) = join(addr, &r.room_id, "", &r.host_token, "").await;
    let (mut a, alan) = join(addr, &r.room_id, &r.join_code, "", "阿澜").await;
    drain(&mut h).await;
    drain(&mut a).await;
    // 角色卡用各自的名字。
    let sh = snapshot(&mut h).await;
    let sa = snapshot(&mut a).await;
    assert_eq!(sh["me"]["sheet"]["name"], "房主");
    assert_eq!(sa["me"]["sheet"]["name"], "阿澜");
    assert_eq!(sa["me"]["actor"], actor_of(&alan));
    // 各自行动拿到的物品进各自的背包。
    send(&mut h, &ClientMsg::Action { action_id: "h1".into(), state_version: version(&hub, &r.room_id).await, text: "我捡起火把".into(), combat: None }).await;
    assert_eq!(until_done(&mut h, "h1").await.0, ActionState::Done);
    send(&mut a, &ClientMsg::Action { action_id: "a1".into(), state_version: version(&hub, &r.room_id).await, text: "我捡起绳子".into(), combat: None }).await;
    assert_eq!(until_done(&mut a, "a1").await.0, ActionState::Done);
    drain(&mut h).await;
    let sh = snapshot(&mut h).await;
    let sa = snapshot(&mut a).await;
    let bag = |s: &serde_json::Value| s["me"]["inventory"].to_string();
    assert!(bag(&sh).contains("火把") && !bag(&sh).contains("绳子"), "{}", bag(&sh));
    assert!(bag(&sa).contains("绳子") && !bag(&sa).contains("火把"), "{}", bag(&sa));
    // 单人的 party 没有被动到。
    let games = hub.games.read().await;
    let g = games.get(&r.room_id).unwrap();
    let w = g.world.lock().unwrap();
    assert!(rpg::inventory(w.conn(), &g.campaign_id, rpg::PARTY).unwrap().is_empty());
    let _ = host;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn combat_turn_order_is_enforced_and_each_player_controls_own_unit() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, task, hub) = boot(dir.path()).await;
    let r = create_room(&hub, CreateRoom { name: "战斗".into(), max_players: Some(4), host_name: "房主".into(), gm_mode: None, template: None }).await.unwrap();
    let (mut h, host) = join(addr, &r.room_id, "", &r.host_token, "").await;
    let (mut a, alan) = join(addr, &r.room_id, &r.join_code, "", "阿澜").await;
    drain(&mut h).await;
    drain(&mut a).await;
    // 服务端开战（第 5 阶段由 GM 回合开战；这里直接调规则引擎）。所有在场玩家都进战场。
    let current = {
        let games = hub.games.read().await;
        let g = games.get(&r.room_id).unwrap();
        let mut w = g.world.lock().unwrap();
        let enemy: combat::EnemySpec = serde_json::from_value(serde_json::json!({ "name": "海盗", "level": 1 })).unwrap();
        let rep = rpg::with_actor(&actor_of(&host), || combat::start(&mut w, &g.campaign_id, &[enemy], &[], "海盗冲上了码头")).unwrap();
        let players: Vec<_> = rep.combat.units.iter().filter_map(|u| u.actor.clone()).filter(|x| rpg::is_player_actor(x)).collect();
        assert!(players.contains(&actor_of(&host)) && players.contains(&actor_of(&alan)), "两名玩家都进了战场：{players:?}");
        rep.combat.current_player().expect("开战后轮到某个玩家")
    };
    let (mut now_ws, mut other_ws, now_id) = if current == actor_of(&host) { (h, a, "h") } else { (a, h, "a") };
    // 不是你的回合：自由行动和战斗按钮都被拒绝。
    let v = version(&hub, &r.room_id).await;
    send(&mut other_ws, &ClientMsg::Action { action_id: "x1".into(), state_version: v, text: "我冲上去".into(), combat: None }).await;
    let (st, reason, _) = until_done(&mut other_ws, "x1").await;
    assert_eq!(st, ActionState::Rejected);
    assert!(reason.contains("轮到"), "{reason}");
    send(&mut other_ws, &ClientMsg::Action { action_id: "x2".into(), state_version: v, text: String::new(), combat: Some(CombatIntent { kind: "end_turn".into(), target: String::new(), item: String::new(), skill: String::new(), x: None, y: None }) }).await;
    assert_eq!(until_done(&mut other_ws, "x2").await.0, ActionState::Rejected);
    assert_eq!(version(&hub, &r.room_id).await, v, "被拒绝的行动不推进状态版本");
    // 轮到的玩家：防御（规则引擎结算），结果广播给所有人，不带战利品 / 经验。
    send(&mut now_ws, &ClientMsg::Action { action_id: format!("{now_id}1"), state_version: v, text: String::new(), combat: Some(CombatIntent { kind: "defend".into(), target: String::new(), item: String::new(), skill: String::new(), x: None, y: None }) }).await;
    let (st, reason, _) = until_done(&mut now_ws, &format!("{now_id}1")).await;
    assert_eq!(st, ActionState::Done, "{reason}");
    let seen = drain(&mut other_ws).await;
    let log = seen.iter().find_map(|m| if let ServerMsg::Event { kind, payload, .. } = m { (kind == "combat_log").then(|| payload.clone()) } else { None }).expect("另一名玩家收到战斗记录");
    assert!(log["lines"].to_string().contains("防御"), "{log}");
    assert!(log["combat"]["units"].as_array().unwrap().iter().all(|u| u.get("loot").is_none() && u.get("xp").is_none()));
    assert_eq!(version(&hub, &r.room_id).await, v + 1);
    // 重启后状态版本仍是成功提交的数量（被拒绝的不算）。
    task.abort();
    drop(now_ws);
    drop(other_ws);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let (_addr2, _t2, hub2) = boot(dir.path()).await;
    assert_eq!(version(&hub2, &r.room_id).await, v + 1);
}

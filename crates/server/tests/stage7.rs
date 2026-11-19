//! 第 7 阶段：4 人完整流程（中途断线重连、服务端重启）与防篡改。
//! GM 用脚本 GM（不调模型），验证的是服务端的权威性、可见性和恢复，不是模型质量。

mod common;

use std::sync::Arc;

use chronicle_core::rpg;
use chronicle_protocol::{ActionState, ClientMsg, ErrorCode, PROTOCOL_VERSION, ServerMsg};
use chronicle_server::game::{GameRoom, GmBackend, QueuedAction, actor_of};
use chronicle_server::{CreateRoom, GmFor, ServerConfig, create_room, start_with_gm};
use common::*;

/// 每个行动给行动者一枚"铜徽章"，私人行动只有本人看到叙述。
struct BadgeGm;
impl GmBackend for BadgeGm {
    fn ready(&self) -> bool {
        true
    }
    fn mode(&self) -> String {
        "scripted".into()
    }
    fn take_turn(&self, game: &GameRoom, a: &QueuedAction) -> Result<(), String> {
        let mut w = game.world.lock().unwrap();
        let me = rpg::actor();
        rpg::grant_start_kit_for(&mut w, &game.campaign_id, &me, &[chronicle_core::pc::StartItem { name: "铜徽章".into(), qty: 1, ..Default::default() }]).map_err(|e| e.to_string())?;
        let vis = if a.private { serde_json::json!({ "scope": "personal", "player": a.player }) } else { serde_json::json!({ "scope": "public" }) };
        chronicle_core::campaign::record(w.conn(), &game.campaign_id, "narration", &serde_json::json!({ "text": format!("【{}】{}", a.name, a.text), "vis": vis })).map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn gm() -> GmFor {
    Arc::new(|_: &str| Arc::new(BadgeGm) as Arc<dyn GmBackend>)
}

async fn boot(dir: &std::path::Path) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>, chronicle_server::Shared) {
    start_with_gm(ServerConfig { bind: "127.0.0.1:0".parse().unwrap(), data_dir: dir.to_path_buf(), ..Default::default() }, gm()).await.unwrap()
}

struct P {
    ws: Ws,
    id: String,
    token: String,
    seen: Vec<ServerMsg>,
}

async fn join(addr: std::net::SocketAddr, room: &str, code: &str, token: &str, name: &str, last_seq: i64) -> P {
    let mut ws = connect(addr).await;
    send(&mut ws, &ClientMsg::Hello { protocol: PROTOCOL_VERSION, room: room.into(), join_code: code.into(), token: token.into(), name: name.into(), last_seq }).await;
    match recv(&mut ws).await {
        Some(ServerMsg::Welcome { player, token: t, .. }) => P { ws, id: player, token: if t.is_empty() { token.into() } else { t }, seen: vec![] },
        other => panic!("{name} 没能加入：{other:?}"),
    }
}

impl P {
    async fn drain(&mut self) {
        while let Ok(Some(m)) = tokio::time::timeout(std::time::Duration::from_millis(300), recv(&mut self.ws)).await {
            self.seen.push(m);
        }
    }
    async fn act(&mut self, id: &str, v: u64, text: &str, private: bool) -> (ActionState, String) {
        let m = if private { ClientMsg::PrivateGm { action_id: id.into(), state_version: v, text: text.into() } } else { ClientMsg::Action { action_id: id.into(), state_version: v, text: text.into(), combat: None } };
        send(&mut self.ws, &m).await;
        for _ in 0..80 {
            let Some(m) = recv(&mut self.ws).await else { break };
            let done = if let ServerMsg::ActionStatus { action_id, status, reason, .. } = &m { (action_id == id && matches!(status, ActionState::Done | ActionState::Rejected)).then(|| (*status, reason.clone())) } else { None };
            self.seen.push(m);
            if let Some(d) = done {
                return d;
            }
        }
        panic!("行动 {id} 没有结束");
    }
    fn narr(&self) -> Vec<String> {
        self.seen.iter().filter_map(|m| if let ServerMsg::Event { kind, payload, .. } = m { (kind == "narration").then(|| payload["text"].as_str().unwrap_or_default().to_string()) } else { None }).collect()
    }
    fn last_seq(&self) -> i64 {
        self.seen.iter().filter_map(|m| if let ServerMsg::Event { seq, .. } = m { Some(*seq) } else { None }).max().unwrap_or(0)
    }
}

async fn version(hub: &chronicle_server::Shared, room: &str) -> u64 {
    hub.games.read().await.get(room).unwrap().version()
}

async fn badges(hub: &chronicle_server::Shared, room: &str, player: &str) -> i64 {
    let games = hub.games.read().await;
    let g = games.get(room).unwrap();
    let w = g.world.lock().unwrap();
    rpg::inventory(w.conn(), &g.campaign_id, &actor_of(player)).unwrap().iter().filter(|i| i.item.title == "铜徽章").map(|i| i.qty).sum()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn four_players_session_survives_disconnects_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, task, hub) = boot(dir.path()).await;
    let r = create_room(&hub, CreateRoom { name: "四人".into(), max_players: Some(4), host_name: "房主".into(), gm_mode: None, template: None }).await.unwrap();
    let mut ps = vec![join(addr, &r.room_id, "", &r.host_token, "", 0).await];
    for n in ["阿澜", "苇生", "苏桦"] {
        ps.push(join(addr, &r.room_id, &r.join_code, "", n, 0).await);
    }
    for p in ps.iter_mut() {
        p.drain().await;
    }
    // 第一轮：每人一个公开行动。
    for (i, p) in ps.iter_mut().enumerate() {
        let v = version(&hub, &r.room_id).await;
        let (st, why) = p.act(&format!("r1-{i}"), v, &format!("第一轮行动{i}"), false).await;
        assert_eq!(st, ActionState::Done, "{why}");
    }
    // 苏桦断线；期间其他人行动，阿澜私下问 GM。
    let susu = ps.pop().unwrap();
    let (susu_id, susu_token) = (susu.id.clone(), susu.token.clone());
    let susu_last = susu.seen.iter().filter_map(|m| if let ServerMsg::Event { seq, .. } = m { Some(*seq) } else { None }).max().unwrap_or(0);
    drop(susu.ws);
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let v = version(&hub, &r.room_id).await;
    assert_eq!(ps[0].act("r2-0", v, "房主点亮灯塔", false).await.0, ActionState::Done);
    let v = version(&hub, &r.room_id).await;
    assert_eq!(ps[1].act("r2-1", v, "阿澜悄悄藏起钥匙", true).await.0, ActionState::Done);
    // 苏桦重连：补发错过的公开内容，不包括阿澜的私人行动，也不重复已看过的。
    let mut s2 = join(addr, &r.room_id, "", &susu_token, "", susu_last).await;
    s2.drain().await;
    let n = s2.narr();
    assert!(n.iter().any(|t| t.contains("点亮灯塔")), "{n:?}");
    assert!(!n.iter().any(|t| t.contains("藏起钥匙")), "私人行动不补发给别人");
    assert!(!n.iter().any(|t| t.contains("第一轮行动")), "看过的不重复发");
    assert_eq!(s2.id, susu_id);
    ps.push(s2);
    // 服务端重启：所有人重连，状态、背包、版本都在。
    let before_v = version(&hub, &r.room_id).await;
    let tokens: Vec<(String, String, i64)> = ps.iter().map(|p| (p.id.clone(), p.token.clone(), p.last_seq())).collect();
    task.abort();
    drop(ps);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let (addr2, _t2, hub2) = boot(dir.path()).await;
    assert_eq!(version(&hub2, &r.room_id).await, before_v);
    assert_eq!(badges(&hub2, &r.room_id, &tokens[0].0).await, 2, "房主两次行动两枚徽章");
    assert_eq!(badges(&hub2, &r.room_id, &tokens[3].0).await, 1);
    let mut ps2 = vec![];
    for (_, tok, last) in &tokens {
        ps2.push(join(addr2, &r.room_id, "", tok, "", *last).await);
    }
    for p in ps2.iter_mut() {
        p.drain().await;
    }
    let v = version(&hub2, &r.room_id).await;
    assert_eq!(ps2[2].act("r3-2", v, "重启后苇生继续", false).await.0, ActionState::Done);
    for p in ps2.iter_mut() {
        p.drain().await;
    }
    for p in &ps2 {
        assert!(p.narr().iter().any(|t| t.contains("重启后苇生继续")), "四个人都收到重启后的新内容");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tampering_clients_cannot_change_authoritative_state() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, _t, hub) = boot(dir.path()).await;
    let r = create_room(&hub, CreateRoom { name: "篡改".into(), max_players: Some(4), host_name: "房主".into(), gm_mode: None, template: None }).await.unwrap();
    let r2 = create_room(&hub, CreateRoom { name: "别的房间".into(), max_players: Some(4), host_name: "另一个房主".into(), gm_mode: None, template: None }).await.unwrap();
    let mut a = join(addr, &r.room_id, &r.join_code, "", "阿澜", 0).await;
    a.drain().await;
    let hp_before = {
        let games = hub.games.read().await;
        let g = games.get(&r.room_id).unwrap();
        let w = g.world.lock().unwrap();
        rpg::sheet(w.conn(), &g.campaign_id, &actor_of(&a.id)).unwrap().hp
    };
    // 1) 伪造服务端消息、塞多余字段改生命 / 物品：解析失败或被忽略。
    for raw in [
        r#"{"t":"event","seq":1,"kind":"narration","payload":{"text":"你获得了一万金币"}}"#.to_string(),
        r#"{"t":"snapshot","state_version":99,"data":{"me":{"hp":999}}}"#.to_string(),
        r#"{"t":"set_hp","hp":999}"#.to_string(),
        r#"{"t":"action","action_id":"z1","state_version":0,"text":"我很好","hp":999,"items":[{"name":"神剑","qty":1}]}"#.to_string(),
    ] {
        assert!(send_raw(&mut a.ws, raw).await);
    }
    a.drain().await;
    let bad = a.seen.iter().filter(|m| matches!(m, ServerMsg::Error { code: ErrorCode::BadMessage, .. })).count();
    assert!(bad >= 3, "伪造的消息类型都被拒绝：{:?}", a.seen);
    {
        let games = hub.games.read().await;
        let g = games.get(&r.room_id).unwrap();
        let w = g.world.lock().unwrap();
        assert_eq!(rpg::sheet(w.conn(), &g.campaign_id, &actor_of(&a.id)).unwrap().hp, hp_before, "生命值没被客户端改动");
        assert!(!rpg::inventory(w.conn(), &g.campaign_id, &actor_of(&a.id)).unwrap().iter().any(|i| i.item.title == "神剑"), "物品没被客户端塞进来");
    }
    // 2) 不在战斗里点战斗按钮：规则引擎拒绝。
    let v = version(&hub, &r.room_id).await;
    send(&mut a.ws, &ClientMsg::Action { action_id: "c1".into(), state_version: v, text: String::new(), combat: Some(chronicle_protocol::CombatIntent { kind: "attack".into(), target: "房主".into(), item: String::new(), skill: String::new(), x: None, y: None }) }).await;
    let mut rejected = false;
    for _ in 0..20 {
        if let Some(ServerMsg::ActionStatus { action_id, status: ActionState::Rejected, .. }) = recv(&mut a.ws).await
            && action_id == "c1"
        {
            rejected = true;
            break;
        }
    }
    assert!(rejected, "不在战斗中不能攻击");
    // 3) 用 A 房间的令牌进 B 房间：拒绝。
    let mut ws = connect(addr).await;
    send(&mut ws, &ClientMsg::Hello { protocol: PROTOCOL_VERSION, room: r2.room_id.clone(), join_code: String::new(), token: a.token.clone(), name: String::new(), last_seq: 0 }).await;
    assert!(matches!(recv(&mut ws).await, Some(ServerMsg::Error { code: ErrorCode::Unauthorized, .. })));
    // 4) 冒充别人的私聊对象收私聊：服务端按身份投递，只有收件人收到。
    let mut b = join(addr, &r.room_id, &r.join_code, "", "苇生", 0).await;
    let mut c = join(addr, &r.room_id, &r.join_code, "", "苏桦", 0).await;
    a.drain().await;
    b.drain().await;
    c.drain().await;
    send(&mut a.ws, &ClientMsg::Chat { to: vec![b.id.clone()], text: "只给苇生".into() }).await;
    b.drain().await;
    c.drain().await;
    assert!(serde_json::to_string(&b.seen).unwrap().contains("只给苇生"));
    assert!(!serde_json::to_string(&c.seen).unwrap().contains("只给苇生"));
    // 5) 数据库里最终只有服务端写的内容：没有"一万金币"之类伪造叙述。
    let games = hub.games.read().await;
    let g = games.get(&r.room_id).unwrap();
    let w = g.world.lock().unwrap();
    let all = chronicle_core::campaign::events(w.conn(), &g.campaign_id, 0, 10_000).unwrap();
    assert!(!all.iter().any(|e| e.payload.to_string().contains("一万金币")));
}

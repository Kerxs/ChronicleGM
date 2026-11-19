//! Chronicle GM 联机服务端（权威服务端）。云服务器和房主主机跑的是同一个程序：
//! - 云服务器：`chronicle-server` 进程，Nginx 反代 TLS 与 WebSocket；
//! - 房主主机：桌面端在本机直接调用 [`start`] 内嵌启动，监听局域网地址。
//!
//! 关键游戏状态只在这里改：客户端只能提交意图，结果由规则引擎（chronicle-core）算出、写库、再广播。

pub mod agent;
mod agent_ws;
pub mod config;
pub mod game;
pub mod gm;
pub mod monitor;
pub mod multi;
pub mod rooms;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chronicle_protocol::{ActionState, ClientMsg, ErrorCode, PROTOCOL_VERSION, PlayerId, ServerMsg, parse_client};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{RwLock, mpsc};

pub use config::ServerConfig;
pub use game::{GameRoom, GmBackend, QueuedAction, ScriptedGm};
use monitor::{Log, RateLimit, Stats};
pub use rooms::{CreateRoom, CreatedRoom, Room};

/// 还没有接入 GM 时的占位：永远不就绪（行动会被拒绝并说明原因）。
pub struct NoGm;
impl GmBackend for NoGm {
    fn ready(&self) -> bool {
        false
    }
    fn take_turn(&self, _g: &GameRoom, _a: &QueuedAction) -> Result<(), String> {
        Err("GM 还没有就绪".into())
    }
    fn mode(&self) -> String {
        "none".into()
    }
}

/// 服务端共享状态。
pub struct Hub {
    pub cfg: ServerConfig,
    pub stats: Stats,
    pub log: Log,
    pub rooms: RwLock<HashMap<String, Arc<Room>>>,
    pub games: RwLock<HashMap<String, Arc<GameRoom>>>,
    /// 每个房间的 GM 执行节点（第 4 阶段由 GM Orchestrator 按房间策略提供）。
    pub gm_for: GmFor,
    /// GM Orchestrator（真实 GM：本地 Agent / 云端）。测试用脚本 GM 时为空。
    pub gmhub: Option<Arc<gm::GmHub>>,
    pub(crate) next_conn: AtomicU64,
}

pub type Shared = Arc<Hub>;

/// 房间号 → 这个房间的 GM 执行节点。
pub type GmFor = Arc<dyn Fn(&str) -> Arc<dyn GmBackend> + Send + Sync>;

impl Hub {
    pub fn rooms_dir(&self) -> std::path::PathBuf {
        self.cfg.data_dir.join("rooms")
    }

    /// 房间的 GM 节点：（显示用的模式，是否就绪）。
    pub fn gm_state(&self, room: &str) -> (String, bool) {
        let g = (self.gm_for)(room);
        (g.mode(), g.ready())
    }

    /// 房间的 GM 状态（给玩家看，不含 Key）。
    pub fn gm_status(&self, room: &str) -> chronicle_protocol::GmStatus {
        match self.gmhub.as_ref().and_then(|h| h.get(room)) {
            Some(g) => g.public(),
            None => {
                let (mode, ready) = self.gm_state(room);
                chronicle_protocol::GmStatus { mode: mode.clone(), active: mode, ready, ..Default::default() }
            }
        }
    }

    /// GM 变化（Agent 上下线、房主切换）后通知房间里所有人。
    pub async fn notify_gm(&self, room_id: &str) {
        let Some(room) = self.rooms.read().await.get(room_id).cloned() else { return };
        let (mode, ready) = self.gm_state(room_id);
        let info = room.info(&mode, ready).await;
        room.broadcast_filtered(&chronicle_protocol::Visibility::Public, &ServerMsg::Gm { gm: self.gm_status(room_id) }).await;
        room.broadcast_filtered(&chronicle_protocol::Visibility::Public, &ServerMsg::Room { room: info }).await;
    }

    /// 给房间挂上游戏（世界 + 战役 + 行动队列工作任务）。
    async fn attach_game(self: &Arc<Self>, room: Arc<Room>, template: Option<&std::path::Path>) -> Result<(), String> {
        let id = room.meta.lock().await.id.clone();
        let (game, rx) = GameRoom::open_or_create(&room.dir, template, self.cfg.queue_len)?;
        room.state_version.store(game.version(), Ordering::SeqCst);
        let gm = (self.gm_for)(&id);
        tokio::spawn(game::run_queue(room, game.clone(), gm, rx));
        self.games.write().await.insert(id, game);
        Ok(())
    }
}

/// 启动服务端：每个房间的 GM 由 GM Orchestrator 按房主设置提供（房主电脑上的 GM Agent / 云端模型）。
/// 返回实际监听地址、运行任务和共享状态。
pub async fn start(cfg: ServerConfig) -> std::io::Result<(SocketAddr, tokio::task::JoinHandle<()>, Shared)> {
    let gmhub = Arc::new(gm::GmHub::new());
    let rooms_dir = cfg.data_dir.join("rooms");
    let h = gmhub.clone();
    let gm_for: GmFor = Arc::new(move |id: &str| Arc::new(h.room(id, &rooms_dir.join(id), "auto")) as Arc<dyn GmBackend>);
    boot(cfg, gm_for, Some(gmhub)).await
}

/// 启动服务端，并指定每个房间的 GM 执行节点（测试、脚本 GM）。
pub async fn start_with_gm(cfg: ServerConfig, gm_for: GmFor) -> std::io::Result<(SocketAddr, tokio::task::JoinHandle<()>, Shared)> {
    boot(cfg, gm_for, None).await
}

async fn boot(cfg: ServerConfig, gm_for: GmFor, gmhub: Option<Arc<gm::GmHub>>) -> std::io::Result<(SocketAddr, tokio::task::JoinHandle<()>, Shared)> {
    std::fs::create_dir_all(cfg.data_dir.join("rooms"))?;
    let log = Log::new(&cfg.data_dir.join("logs"), cfg.log_max_bytes);
    let hub: Shared = Arc::new(Hub { stats: Stats::default(), log, rooms: RwLock::new(HashMap::new()), games: RwLock::new(HashMap::new()), gm_for, gmhub, cfg, next_conn: AtomicU64::new(1) });
    // 服务端重启：从磁盘恢复全部房间和它们的游戏状态（已提交的都在世界文件里）。
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(hub.rooms_dir()) {
        for e in rd.flatten() {
            if let Some(r) = Room::load(&e.path()) {
                let r = Arc::new(r);
                let id = r.meta.lock().await.id.clone();
                hub.rooms.write().await.insert(id.clone(), r.clone());
                if let Err(err) = hub.attach_game(r, None).await {
                    hub.log.warn(&format!("房间 {id} 的游戏状态没能恢复：{err}"));
                }
                n += 1;
            }
        }
    }
    let app = router(hub.clone());
    let listener = tokio::net::TcpListener::bind(hub.cfg.bind).await?;
    let addr = listener.local_addr()?;
    hub.log.info(&format!("服务端启动：{addr}，数据目录 {}，恢复了 {n} 个房间", hub.cfg.data_dir.display()));
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await;
    });
    Ok((addr, task, hub))
}

pub fn router(hub: Shared) -> Router {
    Router::new().route("/health", get(health)).route("/ws", get(ws_upgrade)).route("/agent", get(agent_ws::upgrade)).route("/rooms", post(create_room_http)).with_state(hub)
}

/// 创建房间（房主主机模式由桌面端直接调用；云服务器经 HTTP 调用）。
pub async fn create_room(hub: &Shared, req: CreateRoom) -> Result<CreatedRoom, String> {
    if req.name.trim().is_empty() || req.host_name.trim().is_empty() {
        return Err("房间名和房主名不能为空".into());
    }
    let rss = monitor::rss_mb();
    if rss > 0 && rss >= hub.cfg.memory_soft_limit_mb {
        hub.log.warn(&format!("内存 {rss} MB 超过告警线，拒绝新房间"));
        return Err("服务器内存紧张，暂时不能开新房间".into());
    }
    let mut map = hub.rooms.write().await;
    if map.len() >= hub.cfg.max_rooms {
        return Err(format!("房间数已达上限（{}）", hub.cfg.max_rooms));
    }
    let (room, created) = rooms::create_room_files(&hub.rooms_dir(), &req, hub.cfg.max_players).map_err(|e| format!("创建房间失败：{e}"))?;
    map.insert(created.room_id.clone(), room.clone());
    drop(map);
    hub.attach_game(room, req.template.as_deref()).await?;
    hub.log.info(&format!("创建房间 {}（{}）", created.room_id, req.name.trim()));
    Ok(created)
}

async fn create_room_http(State(hub): State<Shared>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(req): Json<CreateRoom>) -> Response {
    // 云服务器必须配置管理员令牌；没配置时只允许本机（房主主机 / 部署在本机的管理脚本）创建。
    let bearer = headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).unwrap_or("");
    let allowed = match &hub.cfg.admin_token {
        Some(t) => !t.is_empty() && rooms::hash_secret(bearer) == rooms::hash_secret(t),
        // 经反向代理转发来的请求（带 X-Forwarded-For / X-Real-IP）不算本机。
        None => peer.ip().is_loopback() && !headers.contains_key("x-forwarded-for") && !headers.contains_key("x-real-ip"),
    };
    if !allowed {
        hub.stats.rejected.fetch_add(1, Ordering::Relaxed);
        return (StatusCode::UNAUTHORIZED, "需要管理员令牌").into_response();
    }
    match create_room(&hub, req).await {
        Ok(c) => (StatusCode::OK, Json(c)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

async fn health(State(hub): State<Shared>) -> Response {
    let rooms = hub.rooms.read().await.len();
    let body = hub.stats.health(rooms, hub.cfg.memory_soft_limit_mb);
    let code = if body["ok"].as_bool().unwrap_or(false) { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    (code, Json(body)).into_response()
}

async fn ws_upgrade(State(hub): State<Shared>, ws: WebSocketUpgrade) -> Response {
    // 连接数上限：超过直接拒绝（保护 2GB 内存）。
    if hub.stats.connections.load(Ordering::Relaxed) >= hub.cfg.max_connections {
        hub.stats.rejected.fetch_add(1, Ordering::Relaxed);
        return (StatusCode::SERVICE_UNAVAILABLE, "连接数已满").into_response();
    }
    let max = hub.cfg.max_frame_bytes;
    ws.max_message_size(max).max_frame_size(max).on_upgrade(move |socket| client_loop(hub, socket))
}

type Sink = futures_util::stream::SplitSink<WebSocket, Message>;

async fn send(sink: &mut Sink, m: &ServerMsg) -> bool {
    match serde_json::to_string(m) {
        Ok(s) => sink.send(Message::Text(s.into())).await.is_ok(),
        Err(_) => false,
    }
}

fn err(code: ErrorCode, message: &str) -> ServerMsg {
    ServerMsg::Error { code, message: message.to_string() }
}

/// 握手：第一条消息必须是 Hello；校验协议、房间、身份。成功返回（房间、玩家、新令牌）。
async fn handshake(hub: &Shared, sink: &mut Sink, first: Option<Result<Message, axum::Error>>) -> Option<(Arc<Room>, PlayerId, Option<String>, i64)> {
    let parsed = match first {
        Some(Ok(Message::Text(t))) => parse_client(&t),
        _ => Err(ErrorCode::BadMessage),
    };
    let (protocol, room, join_code, token, name, last_seq) = match parsed {
        Ok(ClientMsg::Hello { protocol, room, join_code, token, name, last_seq }) => (protocol, room, join_code, token, name, last_seq),
        _ => {
            send(sink, &err(ErrorCode::BadMessage, "第一条消息必须是 hello")).await;
            return None;
        }
    };
    if protocol != PROTOCOL_VERSION {
        send(sink, &err(ErrorCode::BadProtocol, &format!("协议版本不一致（服务端 {PROTOCOL_VERSION}，客户端 {protocol}），请更新软件"))).await;
        return None;
    }
    let Some(room) = hub.rooms.read().await.get(&room).cloned() else {
        send(sink, &err(ErrorCode::RoomNotFound, "房间不存在")).await;
        return None;
    };
    match room.authenticate(&token, &join_code, &name).await {
        Ok((pid, new_token)) => Some((room, pid, new_token, last_seq)),
        Err((code, msg)) => {
            hub.stats.rejected.fetch_add(1, Ordering::Relaxed);
            send(sink, &err(code, &msg)).await;
            None
        }
    }
}

async fn client_loop(hub: Shared, socket: WebSocket) {
    hub.stats.connections.fetch_add(1, Ordering::Relaxed);
    let (mut sink, mut stream) = socket.split();
    // 10 秒内不发 Hello 就断开（防止空连接占资源）。
    let first = tokio::time::timeout(Duration::from_secs(10), stream.next()).await.ok().flatten();
    let Some((room, pid, new_token, last_seq)) = handshake(&hub, &mut sink, first).await else {
        // 握手失败后仍可回心跳，但不能做任何房间操作；读完剩余消息直到对方断开。
        idle_loop(&hub, &mut sink, &mut stream).await;
        hub.stats.connections.fetch_sub(1, Ordering::Relaxed);
        return;
    };
    // 出站消息走通道 + 写任务：广播不会被某个慢客户端卡住。
    let conn = hub.next_conn.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::channel::<ServerMsg>(256);
    let old = room.online.lock().await.insert(pid.clone(), rooms::Online { conn, tx: tx.clone() });
    if let Some(old) = old {
        // 同一玩家在别处重新连上：通知旧连接并让它断开。
        let _ = old.tx.try_send(err(ErrorCode::Unauthorized, "你在别的地方重新连上了，这里的连接已断开"));
    }
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            let kick = matches!(&m, ServerMsg::Error { code: ErrorCode::Unauthorized, .. });
            if !send(&mut sink, &m).await || kick {
                break;
            }
        }
        let _ = sink.close().await;
    });
    let token = new_token.unwrap_or_default();
    let room_id = room.meta.lock().await.id.clone();
    let game = hub.games.read().await.get(&room_id).cloned();
    // 玩家登记到队伍、第一次加入时建角色卡（自己的角色卡、背包、战斗单位）。
    if let Some(g) = &game {
        let name = room.meta.lock().await.players.iter().find(|p| p.id == pid).map(|p| p.name.clone()).unwrap_or_default();
        let (g2, p2) = (g.clone(), pid.clone());
        if let Ok(Err(e)) = tokio::task::spawn_blocking(move || multi::ensure_player(&g2, &p2, &name)).await {
            hub.log.warn(&format!("玩家 {pid} 登记到队伍失败：{e}"));
        }
    }
    let (gm_mode, gm_ready) = hub.gm_state(&room_id);
    let info = room.info(&gm_mode, gm_ready).await;
    let _ = tx.send(ServerMsg::Welcome { player: pid.clone(), token, room: info.clone() }).await;
    let _ = tx.send(ServerMsg::Gm { gm: hub.gm_status(&room_id) }).await;
    for m in room.chat_history(&pid).await {
        let _ = tx.send(m).await;
    }
    // 断线重连：补发客户端没看到的事件（只发这名玩家看得到的，最多 500 条）。
    if let Some(g) = &game {
        for m in g.events_for(Some(&pid), last_seq.max(0), 500) {
            let _ = tx.send(m).await;
        }
    }
    room.broadcast_filtered(&chronicle_protocol::Visibility::Public, &ServerMsg::Room { room: info }).await;
    hub.log.info(&format!("玩家 {pid} 进入房间"));

    let mut rate = RateLimit::new(hub.cfg.max_msgs_per_sec);
    while let Some(Ok(msg)) = stream.next().await {
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Binary(_) => {
                let _ = tx.try_send(err(ErrorCode::BadMessage, "只接受文本消息"));
                continue;
            }
        };
        hub.stats.messages.fetch_add(1, Ordering::Relaxed);
        if !rate.allow() {
            hub.stats.rejected.fetch_add(1, Ordering::Relaxed);
            let _ = tx.try_send(err(ErrorCode::RateLimited, "消息太频繁，慢一点"));
            continue;
        }
        match parse_client(&text) {
            Ok(ClientMsg::Ping { nonce }) => {
                let _ = tx.try_send(ServerMsg::Pong { nonce });
            }
            Ok(ClientMsg::Chat { to, text }) => {
                if let Err((code, m)) = room.chat(&pid, to, text).await {
                    let _ = tx.try_send(err(code, &m));
                }
            }
            Ok(m @ (ClientMsg::Action { .. } | ClientMsg::PrivateGm { .. })) => {
                let (action_id, state_version, text, private, combat) = match m {
                    ClientMsg::Action { action_id, state_version, text, combat } => (action_id, state_version, text, false, combat),
                    ClientMsg::PrivateGm { action_id, state_version, text } => (action_id, state_version, text, true, None),
                    _ => unreachable!(),
                };
                let Some(g) = &game else {
                    let _ = tx.try_send(err(ErrorCode::Internal, "这个房间的游戏没有加载"));
                    continue;
                };
                if text.trim().is_empty() && combat.is_none() {
                    let _ = tx.try_send(err(ErrorCode::BadMessage, "行动不能为空"));
                    continue;
                }
                let name = room.meta.lock().await.players.iter().find(|p| p.id == pid).map(|p| p.name.clone()).unwrap_or_default();
                let qa = QueuedAction { action_id: action_id.clone(), player: pid.clone(), name, text, private, combat };
                match g.submit(qa, state_version) {
                    Ok(()) => {
                        let _ = tx.try_send(ServerMsg::ActionStatus { action_id, status: ActionState::Queued, reason: String::new(), state_version: g.version() });
                    }
                    Err((code, m)) => {
                        let _ = tx.try_send(ServerMsg::ActionStatus { action_id, status: ActionState::Rejected, reason: m.clone(), state_version: g.version() });
                        let _ = tx.try_send(err(code, &m));
                    }
                }
            }
            Ok(ClientMsg::GetState) => {
                if let Some(g) = &game {
                    let players: Vec<(String, String)> = room.meta.lock().await.players.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
                    let (g2, me) = (g.clone(), pid.clone());
                    let data = tokio::task::spawn_blocking(move || g2.snapshot(&me, &players)).await.unwrap_or_default();
                    let _ = tx.try_send(ServerMsg::Snapshot { state_version: g.version(), data });
                }
            }
            Ok(ClientMsg::GetGm) => {
                let _ = tx.try_send(ServerMsg::Gm { gm: hub.gm_status(&room_id) });
            }
            Ok(ClientMsg::SetGm { mode, cloud }) => {
                // 只有房主能改 GM 设置（服务端判断，不靠客户端隐藏按钮）。
                let is_host = room.meta.lock().await.players.iter().any(|p| p.id == pid && p.host);
                let Some(gh) = hub.gmhub.as_ref().filter(|_| is_host).map(|h| h.room(&room_id, &room.dir, "auto")) else {
                    let code = if is_host { ErrorCode::GmUnavailable } else { ErrorCode::Forbidden };
                    let _ = tx.try_send(err(code, if is_host { "这个服务端没有启用 AI GM" } else { "只有房主能修改 GM 设置" }));
                    continue;
                };
                match gh.update(&mode, cloud) {
                    Ok(()) => {
                        hub.log.info(&format!("房间 {room_id} 的 GM 设置改为 {mode}"));
                        hub.notify_gm(&room_id).await;
                    }
                    Err(m) => {
                        let _ = tx.try_send(err(ErrorCode::BadMessage, &m));
                    }
                }
            }
            Ok(ClientMsg::DiceAnswer { .. }) => {}
            Ok(ClientMsg::Hello { .. }) => {
                let _ = tx.try_send(err(ErrorCode::BadMessage, "已经在房间里了"));
            }
            Err(code) => {
                let _ = tx.try_send(err(code, "消息无法解析或超出限制"));
            }
        }
    }
    // 断开：只移除自己这条连接（重连后的新连接不受影响），再通知其他人。
    {
        let mut online = room.online.lock().await;
        if online.get(&pid).is_some_and(|o| o.conn == conn) {
            online.remove(&pid);
        }
    }
    drop(tx);
    let _ = writer.await;
    let (gm_mode, gm_ready) = hub.gm_state(&room_id);
    let info = room.info(&gm_mode, gm_ready).await;
    room.broadcast_filtered(&chronicle_protocol::Visibility::Public, &ServerMsg::Room { room: info }).await;
    hub.stats.connections.fetch_sub(1, Ordering::Relaxed);
}

/// 握手失败的连接：只回心跳和错误，直到对方断开。
async fn idle_loop(hub: &Shared, sink: &mut Sink, stream: &mut futures_util::stream::SplitStream<WebSocket>) {
    let mut rate = RateLimit::new(hub.cfg.max_msgs_per_sec);
    while let Some(Ok(msg)) = stream.next().await {
        let Message::Text(text) = msg else {
            if matches!(msg, Message::Close(_)) {
                break;
            }
            continue;
        };
        hub.stats.messages.fetch_add(1, Ordering::Relaxed);
        if !rate.allow() {
            hub.stats.rejected.fetch_add(1, Ordering::Relaxed);
            send(sink, &err(ErrorCode::RateLimited, "消息太频繁，慢一点")).await;
            continue;
        }
        match parse_client(&text) {
            Ok(ClientMsg::Ping { nonce }) => {
                send(sink, &ServerMsg::Pong { nonce }).await;
            }
            Ok(_) => {
                send(sink, &err(ErrorCode::Unauthorized, "还没有加入房间")).await;
            }
            Err(code) => {
                send(sink, &err(code, "消息无法解析或超出限制")).await;
            }
        }
    }
}

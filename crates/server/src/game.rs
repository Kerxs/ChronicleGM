//! 房间的游戏状态与行动处理（第 3 阶段）。
//!
//! 每个房间有自己的世界文件（与单人存档同一格式）和一个战役。所有状态变化都走 chronicle-core 的事件溯源
//! （`apply_effects` → 事件日志），所以服务端重启后读库即恢复，回溯、存档格式与单人一致。
//!
//! 行动的处理顺序（任务书要求）：身份验证 → 权限验证 → 状态版本验证 → 行动队列 → GM 理解 → 规则引擎验证
//! → 数据库事务提交 → 事件广播。前三步在连接线程里做（[`GameRoom::submit`]），后面在房间的单一工作任务里按顺序执行。

use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use chronicle_core::World;
use chronicle_core::campaign::{self, EventEntry};
use chronicle_protocol::{ActionState, ErrorCode, PlayerId, ServerMsg, Visibility};
use serde_json::json;
use tokio::sync::mpsc;

use crate::rooms::Room;

/// 玩家在规则引擎里的身份（角色卡、背包都按它存）。单人模式仍然是 `party`。
pub fn actor_of(player: &str) -> String {
    format!("player:{player}")
}

/// 队列里的一个行动。
#[derive(Debug, Clone)]
pub struct QueuedAction {
    pub action_id: String,
    pub player: PlayerId,
    /// 玩家名字（多人回合里 GM 要知道是谁在行动）。
    pub name: String,
    pub text: String,
    /// 私人 GM 对话：结果只给这名玩家看。
    pub private: bool,
    pub combat: Option<chronicle_protocol::CombatIntent>,
}

/// GM 执行节点（第 4 阶段接入本地 / 云端 GM；测试用脚本 GM）。只能通过 core 的接口改状态。
pub trait GmBackend: Send + Sync {
    /// 现在能不能接任务（本地 Agent 在线、云端已配置……）。
    fn ready(&self) -> bool;
    /// 处理一个行动。所有状态修改都必须经过 chronicle-core（规则引擎校验 + 事件日志）。
    fn take_turn(&self, game: &GameRoom, action: &QueuedAction) -> Result<(), String>;
    /// 显示用：cloud / local / manual / scripted。
    fn mode(&self) -> String;
}

/// 房间的游戏部分。
pub struct GameRoom {
    pub world: Arc<StdMutex<World>>,
    pub campaign_id: String,
    pub state_version: AtomicU64,
    /// 已经广播过的最后一个事件序号。
    pub broadcast_seq: AtomicU64,
    recent_actions: StdMutex<(HashSet<String>, VecDeque<String>)>,
    pending: StdMutex<HashSet<PlayerId>>,
    queue_tx: mpsc::Sender<QueuedAction>,
    /// 私人回合的事件范围（序号起止 + 可见性）。事件日志只能追加，所以用一条 `mp_scope` 事件记下范围，而不是改旧事件。
    scopes: StdMutex<Vec<(i64, i64, Visibility)>>,
}

/// 事件 → 可见性。原则：拿不准就不给玩家看。
/// - GM 专用事件（AI 方案、被拒绝的修改、导演建议……）只给 GM；
/// - `effects`（原始状态差异，里面有别人的背包、NPC 秘密数值）不广播，玩家通过过滤后的快照拿状态；
/// - 带 `vis` 字段的事件（私人 GM 对话、私人剧情）按字段；
/// - 其余（叙述、NPC 台词、玩家行动、掷骰、可选行动）公开。
/// - 落在私人回合范围（`mp_scope`）里的事件按范围的可见性；
pub fn event_visibility(e: &EventEntry, scopes: &[(i64, i64, Visibility)]) -> Visibility {
    if chronicle_core::spoiler::gm_only_event(&e.kind) || matches!(e.kind.as_str(), "effects" | "effects_rejected" | "gm_plan" | "game_state" | "mp_scope" | "mp_commit") {
        return Visibility::GmOnly;
    }
    if let Some(v) = e.payload.get("vis").and_then(|v| serde_json::from_value::<Visibility>(v.clone()).ok()) {
        return v;
    }
    if let Some((_, _, v)) = scopes.iter().find(|(a, b, _)| (*a..=*b).contains(&e.seq)) {
        return v.clone();
    }
    // 白名单：只有明确是给玩家看的事件才公开；其余（事实、摘要、联网搜索、上下文……）一律只给 GM。
    if PUBLIC_KINDS.contains(&e.kind.as_str()) { Visibility::Public } else { Visibility::GmOnly }
}

/// 公开给玩家的事件类型（叙述、NPC 台词、玩家行动、掷骰、可选行动、战斗记录、回合边界、剧情跳转与结局）。
pub const PUBLIC_KINDS: &[&str] = &["narration", "npc", "player", "roll", "choices", "combat_log", "mp_action", "turn_end", "turn_aborted", "transition", "ending"];

fn scope_of(e: &EventEntry) -> Option<(i64, i64, Visibility)> {
    let from = e.payload.get("from")?.as_i64()?;
    let to = e.payload.get("to")?.as_i64()?;
    let v = serde_json::from_value::<Visibility>(e.payload.get("scope_vis")?.clone()).ok()?;
    Some((from, to, v))
}

/// 给玩家看的事件内容：去掉 GM 才能看的字段（泄露检查结果、记忆编号）。
pub fn player_payload(e: &EventEntry) -> serde_json::Value {
    let mut p = e.payload.clone();
    if let Some(o) = p.as_object_mut() {
        for k in ["leaks", "memories", "vis"] {
            o.remove(k);
        }
    }
    p
}

impl GameRoom {
    /// 打开或新建房间的世界：已有 `world.cgw` 就打开（服务端重启）；否则从模板世界复制，或用示例世界。
    pub fn open_or_create(dir: &Path, template: Option<&Path>, queue_len: usize) -> Result<(Arc<GameRoom>, mpsc::Receiver<QueuedAction>), String> {
        let path = dir.join("world.cgw");
        let backups = dir.join("backups");
        let fresh = !path.exists();
        if fresh && let Some(t) = template {
            std::fs::copy(t, &path).map_err(|e| format!("复制世界文件失败：{e}"))?;
        }
        let mut world = if path.exists() {
            World::open(&path, &backups).map_err(|e| format!("打开世界失败：{e}"))?.0
        } else {
            let mut w = World::create(&path, &backups, "联机世界").map_err(|e| format!("创建世界失败：{e}"))?;
            chronicle_core::demo::populate(&mut w).map_err(|e| format!("准备示例世界失败：{e}"))?;
            w
        };
        // 房间的战役：记在世界里名为「联机房间」的战役（已有就沿用）。
        let existing = campaign::list_campaigns(world.conn()).map_err(|e| e.to_string())?.into_iter().find(|c| c.name == "联机房间");
        let campaign_id = match existing {
            Some(c) => c.id,
            None => {
                let script = chronicle_core::scripts::list_scripts(world.conn()).ok().and_then(|v| v.into_iter().next()).map(|s| s.id);
                campaign::create_campaign(&mut world, script.as_deref(), "联机房间", None).map_err(|e| e.to_string())?.id
            }
        };
        // 恢复：状态版本 = 成功提交的联机行动数（`mp_commit`）；最近的行动编号（含被拒绝的）用来去重（防止重启前后重复执行）。
        let done = campaign::recent_events(world.conn(), &campaign_id, &["mp_action"], 512).map_err(|e| e.to_string())?;
        let version = campaign::events(world.conn(), &campaign_id, 0, i64::MAX).map(|v| v.iter().filter(|e| e.kind == "mp_commit").count() as u64).unwrap_or(0);
        let last_seq = campaign::events(world.conn(), &campaign_id, 0, i64::MAX).ok().and_then(|v| v.last().map(|e| e.seq)).unwrap_or(0);
        let scopes: Vec<_> = campaign::events(world.conn(), &campaign_id, 0, i64::MAX).unwrap_or_default().iter().filter(|e| e.kind == "mp_scope").filter_map(scope_of).collect();
        let mut ids = (HashSet::new(), VecDeque::new());
        for e in done.iter().rev() {
            if let Some(a) = e.payload.get("action_id").and_then(|v| v.as_str()) {
                ids.0.insert(a.to_string());
                ids.1.push_back(a.to_string());
            }
        }
        let (tx, rx) = mpsc::channel(queue_len.max(1));
        let g = GameRoom {
            world: Arc::new(StdMutex::new(world)),
            campaign_id,
            state_version: AtomicU64::new(version),
            broadcast_seq: AtomicU64::new(last_seq.max(0) as u64),
            recent_actions: StdMutex::new(ids),
            pending: StdMutex::new(HashSet::new()),
            queue_tx: tx,
            scopes: StdMutex::new(scopes),
        };
        Ok((Arc::new(g), rx))
    }

    /// 把序号 `from..=to` 的事件标成只给某些人看（追加一条 `mp_scope` 事件，重启后据此恢复）。调用方持有世界锁。
    pub fn add_scope(&self, conn: &chronicle_core::rusqlite::Connection, from: i64, to: i64, vis: Visibility) -> Result<(), String> {
        if to < from {
            return Ok(());
        }
        campaign::record(conn, &self.campaign_id, "mp_scope", &json!({ "from": from, "to": to, "scope_vis": vis })).map_err(|e| e.to_string())?;
        if let Ok(mut s) = self.scopes.lock() {
            s.push((from, to, vis));
        }
        Ok(())
    }

    fn scopes(&self) -> Vec<(i64, i64, Visibility)> {
        self.scopes.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn version(&self) -> u64 {
        self.state_version.load(Ordering::SeqCst)
    }

    /// 前三步（连接线程里）：去重 → 状态版本 → 每人同时只能有一个行动 → 入队。返回错误码与说明。
    pub fn submit(&self, a: QueuedAction, client_version: u64) -> Result<(), (ErrorCode, String)> {
        {
            let ids = self.recent_actions.lock().map_err(|_| (ErrorCode::Internal, "内部错误".to_string()))?;
            if ids.0.contains(&a.action_id) {
                return Err((ErrorCode::Duplicate, "这个行动已经处理过了".into()));
            }
        }
        let now = self.version();
        if client_version < now {
            return Err((ErrorCode::StaleVersion, format!("局面已经变了（你看到的是第 {client_version} 版，现在是第 {now} 版），请看完最新情况再行动")));
        }
        {
            let mut p = self.pending.lock().map_err(|_| (ErrorCode::Internal, "内部错误".to_string()))?;
            if p.contains(&a.player) {
                return Err((ErrorCode::Duplicate, "你的上一个行动还在处理".into()));
            }
            p.insert(a.player.clone());
        }
        let player = a.player.clone();
        if self.queue_tx.try_send(a).is_err() {
            self.pending.lock().map(|mut p| p.remove(&player)).ok();
            return Err((ErrorCode::QueueFull, "行动太多，排不下了，稍后再试".into()));
        }
        Ok(())
    }

    fn remember(&self, action_id: &str) {
        if let Ok(mut ids) = self.recent_actions.lock() {
            if ids.0.insert(action_id.to_string()) {
                ids.1.push_back(action_id.to_string());
            }
            while ids.1.len() > 512 {
                if let Some(old) = ids.1.pop_front() {
                    ids.0.remove(&old);
                }
            }
        }
    }

    /// 这名玩家能看到的、序号大于 `after` 的事件（重连补发、提交后广播都用它）。
    pub fn events_for(&self, player: Option<&str>, after: i64, limit: i64) -> Vec<ServerMsg> {
        let scopes = self.scopes();
        let Ok(w) = self.world.lock() else { return vec![] };
        campaign::events(w.conn(), &self.campaign_id, after, limit)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| event_visibility(e, &scopes).visible_to(player))
            .map(|e| ServerMsg::Event { seq: e.seq, kind: e.kind.clone(), payload: player_payload(&e) })
            .collect()
    }

    /// 按权限过滤的状态快照：自己的角色与背包、队友的公开信息、场景、游戏阶段、进行中的战斗。
    pub fn snapshot(&self, player: &str, players: &[(PlayerId, String)]) -> serde_json::Value {
        let Ok(w) = self.world.lock() else { return json!({}) };
        let conn = w.conn();
        let cid = &self.campaign_id;
        let me = actor_of(player);
        let sheet = chronicle_core::rpg::sheet(conn, cid, &me).ok();
        let bag: Vec<serde_json::Value> = chronicle_core::rpg::inventory(conn, cid, &me)
            .unwrap_or_default()
            .into_iter()
            .map(|i| json!({ "id": i.item.id, "name": i.item.title, "qty": i.qty, "equipped": i.equipped, "ammo_left": i.ammo_left }))
            .collect();
        let party: Vec<serde_json::Value> = players
            .iter()
            .map(|(pid, name)| {
                let s = chronicle_core::rpg::sheet(conn, cid, &actor_of(pid)).ok();
                // 队友只公开名字、生命值和状态名（不公开背包、属性细节）。
                json!({ "player": pid, "name": name, "hp": s.as_ref().map(|x| x.hp), "max_hp": s.as_ref().map(|x| x.max_hp), "statuses": s.map(|x| x.statuses.into_iter().map(|st| st.name).collect::<Vec<_>>()).unwrap_or_default() })
            })
            .collect();
        let state = chronicle_core::gamestate::get(conn, cid).ok();
        let c = campaign::get_campaign(conn, cid).ok();
        let scene = c.as_ref().and_then(|c| c.current_plot_node.as_deref()).and_then(|id| chronicle_core::plot::get_plot_node(conn, id).ok().flatten()).map(|n| json!({ "title": n.title, "text": n.player_text }));
        json!({
            "turn": c.map(|c| c.turn),
            "phase": state.map(|s| serde_json::to_value(s.phase).unwrap_or_default()),
            "scene": scene,
            "me": { "actor": me, "sheet": sheet, "inventory": bag, "money": chronicle_core::rpg::money(conn, cid, &actor_of(player)) },
            "party": party,
            "combat": chronicle_core::combat::active(conn, cid),
        })
    }
}

/// 房间的行动工作任务：按顺序执行队列里的行动（同一房间同一时刻只有一个行动在改状态）。
pub async fn run_queue(room: Arc<Room>, game: Arc<GameRoom>, gm: Arc<dyn GmBackend>, mut rx: mpsc::Receiver<QueuedAction>) {
    while let Some(a) = rx.recv().await {
        let version = game.version();
        room.send_to(&a.player, ServerMsg::ActionStatus { action_id: a.action_id.clone(), status: ActionState::Running, reason: String::new(), state_version: version }).await;
        let result = if a.combat.is_some() {
            // 战斗按钮：规则引擎直接结算，不需要模型（GM 不在线也能打完这场仗）。
            let (g, act) = (game.clone(), a.clone());
            tokio::task::spawn_blocking(move || {
                {
                    let w = g.world.lock().map_err(|_| "世界文件被占用".to_string())?;
                    campaign::record(w.conn(), &g.campaign_id, "mp_action", &json!({ "action_id": act.action_id, "player": act.player, "combat": act.combat.as_ref().map(|c| c.kind.clone()), "vis": { "scope": "public" } })).map_err(|e| e.to_string())?;
                }
                let ci = act.combat.clone().unwrap_or_else(|| unreachable!());
                crate::multi::run_combat_intent(&g, &act, &ci)
            })
            .await
            .unwrap_or_else(|e| Err(format!("战斗结算崩溃：{e}")))
        } else if gm.ready() {
            let (g, gmb, act) = (game.clone(), gm.clone(), a.clone());
            // GM 回合（模型调用、规则结算、写库）在阻塞线程里做，不占异步工作线程。
            tokio::task::spawn_blocking(move || {
                // 先记下"这个行动开始处理了"（带 action_id，重启后用于去重），再交给 GM。
                {
                    let w = g.world.lock().map_err(|_| "世界文件被占用".to_string())?;
                    let vis = if act.private { json!({ "scope": "personal", "player": act.player }) } else { json!({ "scope": "public" }) };
                    campaign::record(w.conn(), &g.campaign_id, "mp_action", &json!({ "action_id": act.action_id, "player": act.player, "text": act.text, "vis": vis })).map_err(|e| e.to_string())?;
                }
                // 以这名玩家的身份执行（自己的角色卡、背包、战斗单位）；战斗中不是他的回合就拒绝。
                let actor = actor_of(&act.player);
                chronicle_core::rpg::with_actor(&actor, || {
                    crate::multi::check_turn(&g, &actor)?;
                    gmb.take_turn(&g, &act)
                })
            })
            .await
            .unwrap_or_else(|e| Err(format!("GM 任务崩溃：{e}")))
        } else {
            Err("GM 还没有就绪（房主的 GM Agent 不在线，或云端模型没有配置）".into())
        };
        game.remember(&a.action_id);
        game.pending.lock().map(|mut p| p.remove(&a.player)).ok();
        let (status, reason) = match &result {
            Ok(()) => {
                if let Ok(w) = game.world.lock() {
                    let _ = campaign::record(w.conn(), &game.campaign_id, "mp_commit", &json!({ "action_id": a.action_id }));
                }
                game.state_version.fetch_add(1, Ordering::SeqCst);
                room.state_version.store(game.version(), Ordering::SeqCst);
                (ActionState::Done, String::new())
            }
            Err(e) => (ActionState::Rejected, e.clone()),
        };
        // 提交后广播：每人只收到自己看得到的新事件。
        broadcast_new_events(&room, &game).await;
        let v = game.version();
        room.send_to(&a.player, ServerMsg::ActionStatus { action_id: a.action_id, status, reason, state_version: v }).await;
        room.broadcast_filtered(&Visibility::Public, &ServerMsg::Room { room: room.info(&gm.mode(), gm.ready()).await }).await;
    }
}

/// 把序号大于上次广播的事件按每个在线玩家的权限过滤后发出去。
pub async fn broadcast_new_events(room: &Room, game: &GameRoom) {
    let after = game.broadcast_seq.load(Ordering::SeqCst) as i64;
    let all: Vec<EventEntry> = {
        let Ok(w) = game.world.lock() else { return };
        campaign::events(w.conn(), &game.campaign_id, after, 1000).unwrap_or_default()
    };
    let Some(last) = all.last().map(|e| e.seq) else { return };
    game.broadcast_seq.store(last.max(0) as u64, Ordering::SeqCst);
    let scopes = game.scopes();
    let online = room.online.lock().await;
    for (pid, o) in online.iter() {
        for e in all.iter().filter(|e| event_visibility(e, &scopes).visible_to(Some(pid))) {
            let _ = o.tx.try_send(ServerMsg::Event { seq: e.seq, kind: e.kind.clone(), payload: player_payload(e) });
        }
    }
}

/// 测试与"手动模式"用：不调模型，只把玩家的话记成一段叙述（私人对话只给本人），并做一次服务端掷骰。
pub struct ScriptedGm;

impl GmBackend for ScriptedGm {
    fn ready(&self) -> bool {
        true
    }
    fn mode(&self) -> String {
        "scripted".into()
    }
    fn take_turn(&self, game: &GameRoom, a: &QueuedAction) -> Result<(), String> {
        let mut w = game.world.lock().map_err(|_| "世界文件被占用".to_string())?;
        let roll = campaign::roll(&mut w, &game.campaign_id, "1d20", &format!("{} 的行动", a.player)).map_err(|e| e.to_string())?;
        let vis = if a.private { json!({ "scope": "personal", "player": a.player }) } else { json!({ "scope": "public" }) };
        campaign::record(w.conn(), &game.campaign_id, "narration", &json!({ "text": format!("（GM）{}——掷出 {}", a.text, roll.total), "vis": vis })).map_err(|e| e.to_string())?;
        Ok(())
    }
}

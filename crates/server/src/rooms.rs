//! 房间、身份与聊天。
//!
//! - 房间元数据存在 `data/rooms/<房间>/room.json`（服务端重启后恢复）；会话令牌和 Agent 密钥只存哈希。
//! - 加入：第一次用房间的加入码，成功后拿到会话令牌；断线重连用令牌（同一玩家只保留最新的一条连接）。
//! - 聊天：公共频道发给所有人；私聊只发给发送者和指定玩家；历史按可见性过滤后补发，存 `chat.jsonl`（只保留最近 300 条在内存）。
//! - 聊天不经过 GM、不改游戏状态。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chronicle_protocol::{ErrorCode, PlayerId, PlayerInfo, RoomInfo, ServerMsg, Visibility};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, mpsc};

const CHAT_KEEP: usize = 300;

pub fn random_token(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    getrandom::getrandom(&mut b).expect("系统随机数不可用");
    hex::encode(b)
}

pub fn hash_secret(s: &str) -> String {
    hex::encode(Sha256::digest(format!("chronicle-gm:{s}").as_bytes()))
}

/// 6 位加入码（去掉容易看错的 0/O/1/I）。
pub fn join_code() -> String {
    const ABC: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut b = [0u8; 6];
    getrandom::getrandom(&mut b).expect("系统随机数不可用");
    b.iter().map(|x| ABC[(*x as usize) % ABC.len()] as char).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerMeta {
    pub id: PlayerId,
    pub name: String,
    pub token_hash: String,
    #[serde(default)]
    pub character: String,
    #[serde(default)]
    pub host: bool,
    pub joined_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomMeta {
    pub id: String,
    pub name: String,
    pub max_players: u8,
    pub join_code: String,
    pub created_at: i64,
    pub players: Vec<PlayerMeta>,
    /// 房主 GM Agent 的密钥哈希（Local Provider 注册时校验）。
    pub agent_key_hash: String,
    /// GM 策略：cloud / local / manual。
    #[serde(default = "default_gm")]
    pub gm_mode: String,
}

fn default_gm() -> String {
    "local".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRecord {
    pub from: PlayerId,
    pub to: Vec<PlayerId>,
    pub text: String,
    pub at: i64,
}

impl ChatRecord {
    fn visibility(&self) -> Visibility {
        if self.to.is_empty() {
            Visibility::Public
        } else {
            let mut p = self.to.clone();
            p.push(self.from.clone());
            Visibility::Players { players: p }
        }
    }
}

/// 一条在线连接：给它发消息的通道和连接编号（重连时用来区分新旧连接）。
pub struct Online {
    pub conn: u64,
    pub tx: mpsc::Sender<ServerMsg>,
}

pub struct Room {
    pub dir: PathBuf,
    pub meta: Mutex<RoomMeta>,
    pub online: Mutex<HashMap<PlayerId, Online>>,
    pub chat: Mutex<VecDeque<ChatRecord>>,
    /// 当前状态版本（第 3 阶段起由事件提交推进）。
    pub state_version: std::sync::atomic::AtomicU64,
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl Room {
    pub fn save_meta(dir: &Path, m: &RoomMeta) -> std::io::Result<()> {
        // 先写临时文件再改名：写到一半断电不会留下半个 room.json。
        let tmp = dir.join("room.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(m)?)?;
        std::fs::rename(tmp, dir.join("room.json"))
    }

    pub fn load(dir: &Path) -> Option<Room> {
        let meta: RoomMeta = serde_json::from_slice(&std::fs::read(dir.join("room.json")).ok()?).ok()?;
        let mut chat = VecDeque::new();
        if let Ok(s) = std::fs::read_to_string(dir.join("chat.jsonl")) {
            for line in s.lines() {
                if let Ok(c) = serde_json::from_str::<ChatRecord>(line) {
                    chat.push_back(c);
                    if chat.len() > CHAT_KEEP {
                        chat.pop_front();
                    }
                }
            }
        }
        Some(Room { dir: dir.to_path_buf(), meta: Mutex::new(meta), online: Mutex::new(HashMap::new()), chat: Mutex::new(chat), state_version: Default::default() })
    }

    pub async fn info(&self, gm_mode: &str, gm_ready: bool) -> RoomInfo {
        let m = self.meta.lock().await;
        let online = self.online.lock().await;
        RoomInfo {
            id: m.id.clone(),
            name: m.name.clone(),
            max_players: m.max_players,
            players: m.players.iter().map(|p| PlayerInfo { id: p.id.clone(), name: p.name.clone(), character: p.character.clone(), host: p.host, online: online.contains_key(&p.id) }).collect(),
            gm_mode: gm_mode.to_string(),
            gm_ready,
            state_version: self.state_version.load(std::sync::atomic::Ordering::SeqCst),
        }
    }

    /// 身份验证：令牌重连，或加入码加入（新玩家）。返回（玩家，新令牌——只在新加入时有）。
    pub async fn authenticate(&self, token: &str, code: &str, name: &str) -> Result<(PlayerId, Option<String>), (ErrorCode, String)> {
        let mut m = self.meta.lock().await;
        if !token.is_empty() {
            let h = hash_secret(token);
            return match m.players.iter().find(|p| p.token_hash == h) {
                Some(p) => Ok((p.id.clone(), None)),
                None => Err((ErrorCode::Unauthorized, "会话令牌无效（可能已被移出房间），请用加入码重新加入".into())),
            };
        }
        if code.trim().to_uppercase() != m.join_code {
            return Err((ErrorCode::Unauthorized, "加入码不对".into()));
        }
        if m.players.len() >= m.max_players as usize {
            return Err((ErrorCode::RoomFull, format!("房间已满（{} 人）", m.max_players)));
        }
        let mut nm: String = name.trim().chars().take(24).collect();
        if m.players.iter().any(|p| p.name == nm) {
            nm = format!("{nm}{}", m.players.len() + 1);
        }
        let token = random_token(24);
        let id = format!("p{}", random_token(4));
        m.players.push(PlayerMeta { id: id.clone(), name: nm, token_hash: hash_secret(&token), character: String::new(), host: false, joined_at: now() });
        Self::save_meta(&self.dir, &m).map_err(|e| (ErrorCode::Internal, format!("保存房间失败：{e}")))?;
        Ok((id, Some(token)))
    }

    /// 发给房间里所有在线的人（每人一份，按可见性过滤）。
    pub async fn broadcast_filtered(&self, vis: &Visibility, msg: &ServerMsg) {
        let online = self.online.lock().await;
        for (pid, o) in online.iter() {
            if vis.visible_to(Some(pid)) {
                // 对方处理不过来（通道满）就丢掉这条，客户端靠事件序号补发，不能让一个慢客户端拖住整个房间。
                let _ = o.tx.try_send(msg.clone());
            }
        }
    }

    pub async fn send_to(&self, pid: &str, msg: ServerMsg) {
        if let Some(o) = self.online.lock().await.get(pid) {
            let _ = o.tx.try_send(msg);
        }
    }

    /// 聊天：校验收件人，记录并投递。
    pub async fn chat(&self, from: &str, to: Vec<PlayerId>, text: String) -> Result<(), (ErrorCode, String)> {
        {
            let m = self.meta.lock().await;
            if let Some(bad) = to.iter().find(|t| !m.players.iter().any(|p| &p.id == *t)) {
                return Err((ErrorCode::BadMessage, format!("没有这个玩家：{bad}")));
            }
        }
        let rec = ChatRecord { from: from.to_string(), to: to.into_iter().filter(|t| t != from).collect(), text: text.trim().to_string(), at: now() };
        if let Ok(line) = serde_json::to_string(&rec) {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join("chat.jsonl")) {
                let _ = writeln!(f, "{line}");
            }
        }
        let msg = ServerMsg::Chat { from: rec.from.clone(), to: rec.to.clone(), text: rec.text.clone(), at: rec.at };
        let vis = rec.visibility();
        {
            let mut c = self.chat.lock().await;
            c.push_back(rec);
            if c.len() > CHAT_KEEP {
                c.pop_front();
            }
        }
        self.broadcast_filtered(&vis, &msg).await;
        Ok(())
    }

    /// 这个玩家能看到的聊天历史（重连时补发）。
    pub async fn chat_history(&self, pid: &str) -> Vec<ServerMsg> {
        self.chat
            .lock()
            .await
            .iter()
            .filter(|c| c.visibility().visible_to(Some(pid)))
            .map(|c| ServerMsg::Chat { from: c.from.clone(), to: c.to.clone(), text: c.text.clone(), at: c.at })
            .collect()
    }
}

/// 创建房间的请求（云服务器上要管理员令牌；房主主机由桌面端直接调用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRoom {
    pub name: String,
    #[serde(default)]
    pub max_players: Option<u8>,
    pub host_name: String,
    #[serde(default)]
    pub gm_mode: Option<String>,
    /// 房主主机模式：用房主本机的世界文件作为模板（复制一份给房间）。HTTP 请求里不接受这个字段。
    #[serde(skip)]
    pub template: Option<std::path::PathBuf>,
}

/// 创建结果：这些密钥只返回这一次（服务端只存哈希）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatedRoom {
    pub room_id: String,
    pub join_code: String,
    pub host_player: PlayerId,
    pub host_token: String,
    /// 房主 GM Agent 的密钥：只交给房主本人，不会发给其他玩家。
    pub agent_key: String,
}

pub fn create_room_files(rooms_dir: &Path, req: &CreateRoom, max_players_cap: u8) -> std::io::Result<(Arc<Room>, CreatedRoom)> {
    let id = format!("r{}", random_token(5));
    let dir = rooms_dir.join(&id);
    std::fs::create_dir_all(&dir)?;
    let host_token = random_token(24);
    let agent_key = random_token(24);
    let host_id = format!("p{}", random_token(4));
    let gm = match req.gm_mode.as_deref() {
        Some(m @ ("cloud" | "local" | "auto" | "manual")) => m.to_string(),
        _ => default_gm(),
    };
    let meta = RoomMeta {
        id: id.clone(),
        name: req.name.trim().chars().take(40).collect(),
        max_players: req.max_players.unwrap_or(max_players_cap).clamp(1, max_players_cap),
        join_code: join_code(),
        created_at: now(),
        players: vec![PlayerMeta { id: host_id.clone(), name: req.host_name.trim().chars().take(24).collect(), token_hash: hash_secret(&host_token), character: String::new(), host: true, joined_at: now() }],
        agent_key_hash: hash_secret(&agent_key),
        gm_mode: gm,
    };
    Room::save_meta(&dir, &meta)?;
    let created = CreatedRoom { room_id: id, join_code: meta.join_code.clone(), host_player: host_id, host_token, agent_key };
    let room = Room { dir, meta: Mutex::new(meta), online: Mutex::new(HashMap::new()), chat: Mutex::new(VecDeque::new()), state_version: Default::default() };
    Ok((Arc::new(room), created))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_and_codes() {
        let t = random_token(24);
        assert_eq!(t.len(), 48);
        assert_ne!(t, random_token(24));
        assert_ne!(hash_secret(&t), t, "只存哈希");
        let c = join_code();
        assert_eq!(c.len(), 6);
        assert!(!c.contains(['0', 'O', '1', 'I']));
    }
}

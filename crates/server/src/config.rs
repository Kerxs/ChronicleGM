//! 服务端配置：命令行参数 > 环境变量 > 默认值。默认值按 2 核 2GB 的小服务器设定（宁可保守）。

use std::net::SocketAddr;
use std::path::PathBuf;

use chronicle_protocol::limits;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// 监听地址。云服务器默认只听本机（由 Nginx 反代 TLS）；房主主机模式由桌面端设为局域网地址。
    pub bind: SocketAddr,
    /// 数据目录：房间存档（世界文件）、日志。
    pub data_dir: PathBuf,
    /// 每个房间默认人数上限（2–4，可配置，不超过协议硬上限）。
    pub max_players: u8,
    /// 同时开放的房间上限。
    pub max_rooms: usize,
    /// 同时在线的连接上限（超过直接拒绝，保护内存）。
    pub max_connections: usize,
    /// 每个房间行动队列长度。
    pub queue_len: usize,
    /// 单条消息最大字节数。
    pub max_frame_bytes: usize,
    /// 每个连接每秒最多消息数。
    pub max_msgs_per_sec: u32,
    /// 单个日志文件上限（字节），超过就轮转，保留 3 份。
    pub log_max_bytes: u64,
    /// 常驻内存告警线（MB）：超过时拒绝新房间并写日志。
    pub memory_soft_limit_mb: u64,
    /// 通过 HTTP 创建房间要带的管理员令牌（云服务器必须设置；不设置时只允许本机创建）。
    pub admin_token: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:7788".parse().unwrap(),
            data_dir: PathBuf::from("./chronicle-data"),
            max_players: 4,
            max_rooms: 20,
            max_connections: 120,
            queue_len: limits::MAX_QUEUE,
            max_frame_bytes: limits::MAX_FRAME_BYTES,
            max_msgs_per_sec: limits::MAX_MSGS_PER_SEC,
            log_max_bytes: 5 * 1024 * 1024,
            memory_soft_limit_mb: 600,
            admin_token: None,
        }
    }
}

impl ServerConfig {
    /// 读环境变量与命令行（`--bind 0.0.0.0:7788 --data /var/lib/chronicle --max-players 4`）。
    pub fn from_env_and_args(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut c = Self::default();
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        if let Some(v) = env("CHRONICLE_BIND") {
            c.bind = v.parse().map_err(|_| format!("CHRONICLE_BIND 不是有效地址：{v}"))?;
        }
        if let Some(v) = env("CHRONICLE_DATA") {
            c.data_dir = PathBuf::from(v);
        }
        if let Some(v) = env("CHRONICLE_MAX_PLAYERS") {
            c.max_players = v.parse().map_err(|_| format!("CHRONICLE_MAX_PLAYERS 不是数字：{v}"))?;
        }
        if let Some(v) = env("CHRONICLE_MAX_ROOMS") {
            c.max_rooms = v.parse().map_err(|_| format!("CHRONICLE_MAX_ROOMS 不是数字：{v}"))?;
        }
        if let Some(v) = env("CHRONICLE_ADMIN_TOKEN") {
            c.admin_token = Some(v);
        }
        if let Some(v) = env("CHRONICLE_MEMORY_LIMIT_MB") {
            c.memory_soft_limit_mb = v.parse().map_err(|_| format!("CHRONICLE_MEMORY_LIMIT_MB 不是数字：{v}"))?;
        }
        let mut it = args.into_iter().skip(1);
        while let Some(a) = it.next() {
            let mut val = |name: &str| it.next().ok_or_else(|| format!("{name} 后面缺少值"));
            match a.as_str() {
                "--bind" => c.bind = val("--bind")?.parse().map_err(|_| "--bind 不是有效地址".to_string())?,
                "--data" => c.data_dir = PathBuf::from(val("--data")?),
                "--max-players" => c.max_players = val("--max-players")?.parse().map_err(|_| "--max-players 不是数字".to_string())?,
                "--max-rooms" => c.max_rooms = val("--max-rooms")?.parse().map_err(|_| "--max-rooms 不是数字".to_string())?,
                "--help" | "-h" => return Err(HELP.to_string()),
                other => return Err(format!("不认识的参数：{other}\n{HELP}")),
            }
        }
        c.max_players = c.max_players.clamp(1, limits::MAX_PLAYERS_HARD);
        c.max_rooms = c.max_rooms.max(1);
        Ok(c)
    }
}

pub const HELP: &str = "chronicle-server：Chronicle GM 联机服务端（云服务器与房主主机共用）
  --bind 地址:端口     默认 127.0.0.1:7788（云服务器由 Nginx 反代）
  --data 目录          存档与日志目录，默认 ./chronicle-data
  --max-players N      每房间人数上限，默认 4
  --max-rooms N        房间上限，默认 20
环境变量：CHRONICLE_BIND / CHRONICLE_DATA / CHRONICLE_MAX_PLAYERS / CHRONICLE_MAX_ROOMS / CHRONICLE_MEMORY_LIMIT_MB
  CHRONICLE_ADMIN_TOKEN（云服务器上创建房间要带这个令牌；不设置时只允许本机创建）";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_override_defaults_and_are_clamped() {
        let c = ServerConfig::from_env_and_args(["x", "--bind", "0.0.0.0:9000", "--max-players", "99"].map(String::from)).unwrap();
        assert_eq!(c.bind.port(), 9000);
        assert_eq!(c.max_players, limits::MAX_PLAYERS_HARD, "人数不超过协议硬上限");
        assert!(ServerConfig::from_env_and_args(["x", "--bogus"].map(String::from)).is_err());
    }
}

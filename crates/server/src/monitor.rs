//! 运行监控：常驻内存、连接数、房间数、运行时长（/health 返回），以及大小受限、自动轮转的日志。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// 本进程的常驻内存（MB）。Linux 读 /proc/self/status（零依赖、最准）；其他平台返回 0（只用于本机房主模式）。
pub fn rss_mb() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(s) = fs::read_to_string("/proc/self/status")
            && let Some(line) = s.lines().find(|l| l.starts_with("VmRSS:"))
        {
            let kb: u64 = line.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
            return kb / 1024;
        }
        0
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

pub struct Stats {
    pub started: Instant,
    pub connections: AtomicUsize,
    pub messages: AtomicUsize,
    pub rejected: AtomicUsize,
}

impl Default for Stats {
    fn default() -> Self {
        Self { started: Instant::now(), connections: AtomicUsize::new(0), messages: AtomicUsize::new(0), rejected: AtomicUsize::new(0) }
    }
}

impl Stats {
    pub fn health(&self, rooms: usize, soft_limit_mb: u64) -> serde_json::Value {
        let rss = rss_mb();
        serde_json::json!({
            "ok": rss == 0 || rss < soft_limit_mb,
            "version": env!("CARGO_PKG_VERSION"),
            "protocol": chronicle_protocol::PROTOCOL_VERSION,
            "uptime_secs": self.started.elapsed().as_secs(),
            "rooms": rooms,
            "connections": self.connections.load(Ordering::Relaxed),
            "messages": self.messages.load(Ordering::Relaxed),
            "rejected": self.rejected.load(Ordering::Relaxed),
            "rss_mb": rss,
            "memory_soft_limit_mb": soft_limit_mb,
        })
    }
}

/// 简单的文件日志：写满 `max_bytes` 就把 server.log → server.log.1 → .2，最多保留 3 份。
pub struct Log {
    path: PathBuf,
    max_bytes: u64,
    lock: Mutex<()>,
}

impl Log {
    pub fn new(dir: &Path, max_bytes: u64) -> Self {
        let _ = fs::create_dir_all(dir);
        Self { path: dir.join("server.log"), max_bytes: max_bytes.max(64 * 1024), lock: Mutex::new(()) }
    }

    pub fn write(&self, level: &str, msg: &str) {
        let Ok(_g) = self.lock.lock() else { return };
        if fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0) >= self.max_bytes {
            for i in (1..3).rev() {
                let _ = fs::rename(self.path.with_extension(format!("log.{i}")), self.path.with_extension(format!("log.{}", i + 1)));
            }
            let _ = fs::rename(&self.path, self.path.with_extension("log.1"));
        }
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        // 一行最多 500 字，防止异常输入把日志撑爆。
        let msg: String = msg.chars().take(500).collect();
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{secs} [{level}] {msg}");
        }
        eprintln!("[{level}] {msg}");
    }

    pub fn info(&self, msg: &str) {
        self.write("info", msg);
    }
    pub fn warn(&self, msg: &str) {
        self.write("warn", msg);
    }
}

/// 每个连接的限流（令牌桶，每秒补满）。
pub struct RateLimit {
    per_sec: u32,
    tokens: u32,
    window: Instant,
}

impl RateLimit {
    pub fn new(per_sec: u32) -> Self {
        Self { per_sec: per_sec.max(1), tokens: per_sec.max(1), window: Instant::now() }
    }
    /// 这条消息能不能放行。
    pub fn allow(&mut self) -> bool {
        if self.window.elapsed().as_millis() >= 1000 {
            self.window = Instant::now();
            self.tokens = self.per_sec;
        }
        if self.tokens == 0 {
            return false;
        }
        self.tokens -= 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_rotates_and_keeps_three() {
        let dir = std::env::temp_dir().join(format!("chronicle-log-{}", std::process::id() as u128 * 1000 + std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos() as u128));
        let log = Log::new(&dir, 64 * 1024);
        for i in 0..4000 {
            log.info(&format!("第 {i} 行 {}", "x".repeat(60)));
        }
        assert!(dir.join("server.log").exists());
        assert!(dir.join("server.log.1").exists());
        assert!(!dir.join("server.log.4").exists(), "最多保留 3 份旧日志");
        let total: u64 = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.metadata().unwrap().len()).sum();
        assert!(total < 64 * 1024 * 5, "日志总大小受限：{total}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rate_limit_blocks_bursts() {
        let mut r = RateLimit::new(3);
        assert!(r.allow() && r.allow() && r.allow());
        assert!(!r.allow(), "一秒内第 4 条被拒");
    }
}

//! 单文件下载：HTTP Range 断点续传、大小检查、SHA256 校验、进度回调、可中止。
//! 运行环境安装和下载管理器共用。下载写到 `<目标>.part`，全部校验通过后才改名成目标文件。

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq)]
pub enum NetError {
    /// 被调用方中止（暂停或取消）；`.part` 文件保留，可以续传。
    Stopped,
    Http(u16, String),
    Io(String),
    /// 下载完成但大小或校验值不对（`.part` 已删除，需要重新下载）。
    Corrupt(String),
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetError::Stopped => write!(f, "已停止"),
            NetError::Http(401, _) | NetError::Http(403, _) => write!(f, "没有访问权限（私有仓库需要访问令牌）"),
            NetError::Http(404, _) => write!(f, "文件不存在（404）"),
            NetError::Http(c, m) => write!(f, "服务器返回 HTTP {c}：{m}"),
            NetError::Io(e) => write!(f, "{e}"),
            NetError::Corrupt(e) => write!(f, "文件不完整或已损坏：{e}"),
        }
    }
}

impl NetError {
    /// 值得自动重试的错误（网络中断、服务器临时错误）。
    pub fn retryable(&self) -> bool {
        match self {
            NetError::Io(_) => true,
            NetError::Http(c, _) => *c == 408 || *c == 429 || *c >= 500,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
    /// 字节 / 秒（最近几秒的平均）。
    pub speed: f64,
}

impl Progress {
    /// 剩余秒数估算。
    pub fn eta_secs(&self) -> Option<u64> {
        let t = self.total?;
        (self.speed > 1.0).then(|| ((t.saturating_sub(self.downloaded)) as f64 / self.speed) as u64)
    }
}

pub struct Request<'a> {
    pub url: &'a str,
    pub dest: &'a Path,
    /// 访问令牌（私有仓库）；只放在请求头里，不写日志。
    pub token: Option<&'a str>,
    /// 魔搭私有仓库用登录得到的 Cookie（同样只放请求头）。
    pub cookie: Option<&'a str>,
    pub expected_size: Option<u64>,
    /// 十六进制 SHA256（仓库提供时）。
    pub sha256: Option<&'a str>,
    pub stop: &'a AtomicBool,
}

pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_connect(Some(Duration::from_secs(15))).timeout_recv_response(Some(Duration::from_secs(30))).timeout_recv_body(None).user_agent("ChronicleGM-ModelManager").build().into()
}

pub fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

/// 下载到 `dest`。已有 `.part` 时从断点继续（服务器不支持 Range 就从头下）。
pub fn fetch(req: &Request, mut on_progress: impl FnMut(Progress)) -> Result<(), NetError> {
    if let Some(dir) = req.dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| NetError::Io(format!("建目录失败：{e}")))?;
    }
    let part = part_path(req.dest);
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if let Some(n) = req.expected_size
        && have > n
    {
        let _ = std::fs::remove_file(&part);
        have = 0;
    }
    let need_more = req.expected_size.is_none_or(|n| have < n);
    if need_more {
        let mut r = agent().get(req.url);
        if have > 0 {
            r = r.header("Range", &format!("bytes={have}-"));
        }
        if let Some(t) = req.token.filter(|t| !t.is_empty()) {
            r = r.header("Authorization", &format!("Bearer {t}"));
        }
        if let Some(c) = req.cookie.filter(|c| !c.is_empty()) {
            r = r.header("Cookie", c);
        }
        let resp = match r.call() {
            Ok(x) => x,
            // 416：已经下完了（.part 和服务器上的一样大）。
            Err(ureq::Error::StatusCode(416)) if have > 0 => return finish(req, &part, have, &mut on_progress),
            Err(ureq::Error::StatusCode(c)) => return Err(NetError::Http(c, String::new())),
            Err(e) => return Err(NetError::Io(format!("网络错误：{e}"))),
        };
        let status = resp.status().as_u16();
        let resumed = status == 206;
        if !resumed {
            have = 0;
        }
        let len = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
        let total = req.expected_size.or(len.map(|l| l + have));
        let mut f = OpenOptions::new().create(true).write(true).truncate(!resumed).open(&part).map_err(|e| NetError::Io(format!("写临时文件失败：{e}")))?;
        if resumed {
            f.seek(SeekFrom::End(0)).map_err(|e| NetError::Io(e.to_string()))?;
        }
        let mut body = resp.into_body().into_reader();
        let mut buf = vec![0u8; 256 * 1024];
        let mut window: (Instant, u64) = (Instant::now(), have);
        let mut speed = 0.0;
        let mut last_report = Instant::now() - Duration::from_secs(1);
        loop {
            if req.stop.load(Ordering::Relaxed) {
                let _ = f.flush();
                return Err(NetError::Stopped);
            }
            let n = body.read(&mut buf).map_err(|e| NetError::Io(format!("下载中断：{e}")))?;
            if n == 0 {
                break;
            }
            f.write_all(&buf[..n]).map_err(|e| NetError::Io(format!("写磁盘失败（空间不够？）：{e}")))?;
            have += n as u64;
            let dt = window.0.elapsed().as_secs_f64();
            if dt >= 1.0 {
                let inst = (have - window.1) as f64 / dt;
                speed = if speed == 0.0 { inst } else { speed * 0.6 + inst * 0.4 };
                window = (Instant::now(), have);
            }
            if last_report.elapsed() >= Duration::from_millis(250) {
                last_report = Instant::now();
                on_progress(Progress { downloaded: have, total, speed });
            }
        }
        f.flush().map_err(|e| NetError::Io(e.to_string()))?;
    }
    finish(req, &part, have, &mut on_progress)
}

fn finish(req: &Request, part: &Path, have: u64, on_progress: &mut impl FnMut(Progress)) -> Result<(), NetError> {
    if let Some(n) = req.expected_size
        && have != n
    {
        if have > n {
            let _ = std::fs::remove_file(part);
        }
        return Err(if have > n { NetError::Corrupt(format!("大小 {have} 字节，应为 {n}")) } else { NetError::Io(format!("连接提前结束（{have}/{n} 字节），可以继续下载")) });
    }
    if let Some(want) = req.sha256.filter(|s| !s.is_empty()) {
        let got = sha256_file(part).map_err(|e| NetError::Io(e.to_string()))?;
        if !got.eq_ignore_ascii_case(want.trim_start_matches("sha256:")) {
            let _ = std::fs::remove_file(part);
            return Err(NetError::Corrupt("SHA256 校验值不一致".into()));
        }
    }
    on_progress(Progress { downloaded: have, total: Some(have), speed: 0.0 });
    std::fs::rename(part, req.dest).map_err(|e| NetError::Io(format!("移动文件失败：{e}")))?;
    Ok(())
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// GET 一个 JSON（仓库 API、Releases 列表）。
pub fn get_json(url: &str, token: Option<&str>) -> Result<serde_json::Value, NetError> {
    get_json_with(url, token, None)
}

pub fn get_json_with(url: &str, token: Option<&str>, cookie: Option<&str>) -> Result<serde_json::Value, NetError> {
    let mut r = agent().get(url).header("Accept", "application/json");
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        r = r.header("Authorization", &format!("Bearer {t}"));
    }
    if let Some(c) = cookie.filter(|c| !c.is_empty()) {
        r = r.header("Cookie", c);
    }
    match r.call() {
        Ok(mut resp) => {
            let s = resp.body_mut().with_config().limit(32 * 1024 * 1024).read_to_string().map_err(|e| NetError::Io(e.to_string()))?;
            serde_json::from_str(&s).map_err(|e| NetError::Io(format!("回复不是 JSON：{e}")))
        }
        Err(ureq::Error::StatusCode(c)) => Err(NetError::Http(c, String::new())),
        Err(e) => Err(NetError::Io(format!("网络错误：{e}"))),
    }
}

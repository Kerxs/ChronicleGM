//! 测试用的小 HTTP 服务器：按路径返回内容，支持 Range、访问令牌、限速、中途断开。
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct Route {
    pub body: Vec<u8>,
    pub content_type: String,
    /// 需要的令牌（Bearer）。
    pub token: Option<String>,
    /// 每次请求最多发多少字节后断开（模拟网络中断）。
    pub cut_after: Option<usize>,
    /// 每 64KB 停顿的毫秒数（模拟慢速网络，便于测试暂停）。
    pub throttle_ms: u64,
    /// 返回这个状态码（例如 500），不发内容。
    pub status: Option<u16>,
    /// 需要的 Cookie（魔搭私有仓库）。
    pub cookie: Option<String>,
    /// 额外的回复头。
    pub headers: Vec<(String, String)>,
}

/// 一次请求：（路径，Range 起点，Authorization）。
pub type Hit = (String, Option<String>, Option<String>);

#[derive(Clone, Default)]
pub struct Mock {
    pub routes: Arc<Mutex<HashMap<String, Route>>>,
    /// （路径，Range 起点，Authorization）。
    pub hits: Arc<Mutex<Vec<Hit>>>,
    pub served: Arc<AtomicUsize>,
    pub base: String,
}

impl Mock {
    pub fn start() -> Mock {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let m = Mock { base: format!("http://{}", l.local_addr().unwrap()), ..Default::default() };
        let me = m.clone();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let me = me.clone();
                std::thread::spawn(move || me.handle(s));
            }
        });
        m
    }

    pub fn route(&self, path: &str, r: Route) {
        self.routes.lock().unwrap().insert(path.to_string(), r);
    }
    pub fn json(&self, path: &str, v: &serde_json::Value) {
        self.route(path, Route { body: v.to_string().into_bytes(), content_type: "application/json".into(), ..Default::default() });
    }
    pub fn file(&self, path: &str, body: Vec<u8>) {
        self.route(path, Route { body, content_type: "application/octet-stream".into(), ..Default::default() });
    }
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
    pub fn hits_for(&self, prefix: &str) -> usize {
        self.hits.lock().unwrap().iter().filter(|h| h.0.starts_with(prefix)).count()
    }

    fn handle(&self, mut s: std::net::TcpStream) {
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut line = String::new();
        if r.read_line(&mut line).is_err() {
            return;
        }
        let full = line.split_whitespace().nth(1).unwrap_or("/").to_string();
        let mut range = None;
        let mut auth = None;
        let mut cookie = None;
        let mut clen = 0usize;
        loop {
            let mut h = String::new();
            if r.read_line(&mut h).is_err() || h.trim().is_empty() {
                break;
            }
            let lower = h.to_ascii_lowercase();
            if let Some(v) = lower.strip_prefix("range: bytes=") {
                range = v.trim().trim_end_matches('-').parse::<usize>().ok();
            }
            if lower.starts_with("authorization:") {
                auth = Some(h["authorization:".len()..].trim().to_string());
            }
            if lower.starts_with("cookie:") {
                cookie = Some(h["cookie:".len()..].trim().to_string());
            }
            if let Some(v) = lower.strip_prefix("content-length:") {
                clen = v.trim().parse().unwrap_or(0);
            }
        }
        if clen > 0 {
            let mut body = vec![0u8; clen];
            let _ = r.read_exact(&mut body);
        }

        self.hits.lock().unwrap().push((full.clone(), range.map(|x| x.to_string()), auth.clone()));
        let path = full.clone();
        let route = {
            let routes = self.routes.lock().unwrap();
            routes.get(&path).cloned().or_else(|| routes.get(path.split('?').next().unwrap_or("")).cloned())
        };
        let Some(rt) = route else {
            let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            return;
        };
        if let Some(st) = rt.status {
            let _ = s.write_all(format!("HTTP/1.1 {st} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").as_bytes());
            return;
        }
        if (rt.token.as_ref().is_some_and(|t| auth.as_deref() != Some(&format!("Bearer {t}")))) || (rt.cookie.as_ref().is_some_and(|c| !cookie.as_deref().unwrap_or("").contains(c.as_str()))) {
            let _ = s.write_all(b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            return;
        }
        let start = range.unwrap_or(0);
        if start >= rt.body.len() && start > 0 {
            let _ = s.write_all(b"HTTP/1.1 416 Range Not Satisfiable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            return;
        }
        let part = &rt.body[start..];
        let head = if range.is_some() {
            format!("HTTP/1.1 206 Partial Content\r\ncontent-type: {}\r\ncontent-length: {}\r\ncontent-range: bytes {}-{}/{}\r\nconnection: close\r\n\r\n", rt.content_type, part.len(), start, rt.body.len() - 1, rt.body.len())
        } else {
            format!("HTTP/1.1 200 OK\r\ncontent-type: {}\r\ncontent-length: {}\r\naccept-ranges: bytes\r\nconnection: close\r\n{}\r\n", rt.content_type, part.len(), rt.headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect::<String>())
        };
        if s.write_all(head.as_bytes()).is_err() {
            return;
        }
        let limit = rt.cut_after.unwrap_or(usize::MAX).min(part.len());
        for chunk in part[..limit].chunks(64 * 1024) {
            if s.write_all(chunk).is_err() {
                return;
            }
            self.served.fetch_add(chunk.len(), Ordering::Relaxed);
            if rt.throttle_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(rt.throttle_ms));
            }
        }
        let _ = s.flush();
        let _ = s.shutdown(std::net::Shutdown::Both);
    }
}

/// 生成一个 zip（文件名 → 内容）。
pub fn make_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        for (n, b) in files {
            z.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(b).unwrap();
        }
        z.finish().unwrap();
    }
    buf.into_inner()
}

pub fn sha256(b: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(b))
}

/// 一个最小的合法 GGUF 文件（只有头和几个元数据）。
pub fn gguf_bytes(arch: &str, ctx: u64, pad: usize) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("m.gguf");
    chronicle_llm::gguf::tests::write_fixture(&p, arch, ctx, 15, None);
    let mut b = std::fs::read(&p).unwrap();
    b.extend(std::iter::repeat_n(0u8, pad));
    b
}

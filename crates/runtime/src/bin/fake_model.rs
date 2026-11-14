//! 测试用的假模型服务（只在测试中使用，不会打包进应用）。
//! 用法：fake_model <端口> [启动延迟毫秒] [回复延迟毫秒]
//! - GET /health：启动延迟过后返回 200 {"status":"ok"}，之前返回 503
//! - GET /crash：进程以退出码 3 退出（模拟崩溃）
//! - POST /v1/chat/completions：等待回复延迟后返回固定回复
//! - GET /v1/models：返回一个模型

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let port: u16 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(18080);
    let start_delay = Duration::from_millis(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0));
    let reply_delay = Duration::from_millis(args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0));
    let started = Instant::now();
    let listener = TcpListener::bind(("127.0.0.1", port)).expect("端口被占用");
    println!("fake_model listening on {port}");
    for stream in listener.incoming().flatten() {
        let ready = started.elapsed() >= start_delay;
        std::thread::spawn(move || handle(stream, ready, reply_delay));
    }
}

fn handle(mut s: TcpStream, ready: bool, reply_delay: Duration) {
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut line = String::new();
    if r.read_line(&mut line).is_err() {
        return;
    }
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).is_err() || h == "\r\n" || h.is_empty() {
            break;
        }
        if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; len];
    let _ = r.read_exact(&mut body);
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
    let (code, text) = match path.as_str() {
        "/crash" => std::process::exit(3),
        "/health" if ready => (200, r#"{"status":"ok"}"#.to_string()),
        "/health" => (503, r#"{"status":"loading model"}"#.to_string()),
        "/v1/models" => (200, r#"{"data":[{"id":"fake"}]}"#.to_string()),
        // 和 llama-server 一样：加载完成之前，对话请求返回 503 "Loading model"。
        p if p.ends_with("/chat/completions") && !ready => (503, r#"{"error":{"message":"Loading model","type":"unavailable_error","code":503}}"#.to_string()),
        p if p.ends_with("/chat/completions") => {
            std::thread::sleep(reply_delay);
            (200, r#"{"choices":[{"message":{"role":"assistant","content":"{\"ok\":true}"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#.to_string())
        }
        _ => (404, "{}".to_string()),
    };
    let _ = write!(s, "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
}

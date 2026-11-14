//! 测试用的本地 HTTP 模拟服务器（mock）。
//! 用它测试的结果只说明客户端按约定处理各种响应，不代表任何真实模型服务的行为。

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

pub enum Reply {
    Json(u16, serde_json::Value),
    Text(u16, String),
    /// SSE：逐块发送，每块之间可延时。
    Sse(Vec<String>, Duration),
    /// 二进制内容（音频、图片）。
    Bytes(u16, String, Vec<u8>),
}

pub struct MockServer {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut headers = vec![];
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        reader.read_line(&mut h).ok()?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().unwrap_or(0);
            }
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).ok()?;
    Some(Request { method, path, headers, body: String::from_utf8_lossy(&body).into_owned() })
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

impl MockServer {
    pub fn start<F>(handler: F) -> MockServer
    where
        F: Fn(&Request, usize) -> Reply + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests: Arc<Mutex<Vec<Request>>> = Arc::default();
        let reqs = requests.clone();
        let handler = Arc::new(handler);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let handler = handler.clone();
                let reqs = reqs.clone();
                thread::spawn(move || {
                    let Some(req) = read_request(&mut stream) else { return };
                    let n = {
                        let mut r = reqs.lock().unwrap();
                        r.push(req.clone());
                        r.len()
                    };
                    match handler(&req, n) {
                        Reply::Json(code, v) => {
                            let body = v.to_string();
                            let _ = write!(
                                stream,
                                "HTTP/1.1 {code} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                reason(code),
                                body.len()
                            );
                        }
                        Reply::Text(code, body) => {
                            let _ = write!(
                                stream,
                                "HTTP/1.1 {code} {}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                reason(code),
                                body.len()
                            );
                        }
                        Reply::Bytes(code, ctype, body) => {
                            let _ = write!(stream, "HTTP/1.1 {code} {}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reason(code), body.len());
                            let _ = stream.write_all(&body);
                        }
                        Reply::Sse(chunks, delay) => {
                            let _ = write!(
                                stream,
                                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
                            );
                            for c in chunks {
                                if write!(stream, "data: {c}\n\n").is_err() {
                                    return;
                                }
                                let _ = stream.flush();
                                thread::sleep(delay);
                            }
                        }
                    }
                    let _ = stream.flush();
                });
            }
        });
        MockServer { url, requests }
    }

    pub fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

pub fn not_found() -> Reply {
    Reply::Text(404, "not found".into())
}

/// OpenAI 风格的非流式回复。
pub fn completion(content: &str) -> Reply {
    Reply::Json(
        200,
        serde_json::json!({
            "model": "mock-model",
            "choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        }),
    )
}

/// OpenAI 风格的流式回复。
pub fn stream(parts: &[&str], delay: Duration) -> Reply {
    let mut chunks: Vec<String> = parts
        .iter()
        .map(|p| serde_json::json!({"model":"mock-model","choices":[{"delta":{"content":p}}]}).to_string())
        .collect();
    chunks.push(serde_json::json!({"choices":[{"delta":{},"finish_reason":"stop"}]}).to_string());
    chunks.push("[DONE]".into());
    Reply::Sse(chunks, delay)
}

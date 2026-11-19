//! 房主电脑上的 GM Agent（Local Provider 的另一半）。
//!
//! - 主动连服务端的 `/agent`（云服务器用 `wss://`，经 Nginx TLS；房主主机模式是本机回环），
//!   所以房主本机的模型端口永远不用暴露到公网；
//! - 用建房时拿到的 Agent 密钥注册，之后只做一件事：收到一次模型调用就交给房主本机的模型，把原始输出传回去；
//!   提示词、结构化解析、规则校验、写库全在服务端；
//! - 模型的 API Key（如果本机服务要 Key）只在房主电脑上，不发给服务端；
//! - 每 15 秒心跳；断线后 1→30 秒退避重连；密钥不对 / 已有 Agent 在线这类错误直接停止并说明原因。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chronicle_llm::{CancelToken, ChatMessage, ChatRequest, LlmClient, OpenAiClient, OutputFormat, ProviderConfig, Role};
use chronicle_protocol::{AgentMsg, AgentTask, PROTOCOL_VERSION};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone)]
pub struct AgentConfig {
    /// 例如 `wss://example.com/agent` 或 `ws://127.0.0.1:7788/agent`。
    pub server: String,
    pub room: String,
    pub agent_key: String,
    /// 房主本机的模型服务（和单人模式用的是同一份配置）。
    pub provider: ProviderConfig,
    pub api_key: Option<String>,
}

/// Agent 停下的原因。
#[derive(Debug, Clone, PartialEq)]
pub enum AgentExit {
    /// 调用方要求停止。
    Stopped,
    /// 服务端拒绝（密钥不对、已有 Agent、协议不一致……），重连也没用。
    Rejected(String),
}

/// 一直运行直到 `stop` 被置位或被服务端拒绝。`status` 回调报告连接状态（界面显示用）。
pub async fn run_agent(cfg: AgentConfig, stop: Arc<AtomicBool>, status: Arc<dyn Fn(&str) + Send + Sync>) -> AgentExit {
    let client: Arc<dyn LlmClient> = Arc::new(OpenAiClient::new(cfg.provider.clone(), cfg.api_key.clone()));
    let mut backoff = 1u64;
    loop {
        if stop.load(Ordering::Relaxed) {
            return AgentExit::Stopped;
        }
        status("连接中");
        match session(&cfg, client.clone(), &stop, &status).await {
            Ok(()) => return AgentExit::Stopped,
            Err(SessionEnd::Rejected(r)) => {
                status(&format!("被拒绝：{r}"));
                return AgentExit::Rejected(r);
            }
            Err(SessionEnd::Lost(e)) => {
                status(&format!("连接断开（{e}），{backoff} 秒后重连"));
            }
            Err(SessionEnd::Connected) => backoff = 1,
        }
        for _ in 0..backoff * 10 {
            if stop.load(Ordering::Relaxed) {
                return AgentExit::Stopped;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        backoff = (backoff * 2).min(30);
    }
}

enum SessionEnd {
    Rejected(String),
    Lost(String),
    /// 注册成功过、之后才断开：重置退避。
    Connected,
}

async fn session(cfg: &AgentConfig, client: Arc<dyn LlmClient>, stop: &Arc<AtomicBool>, status: &Arc<dyn Fn(&str) + Send + Sync>) -> Result<(), SessionEnd> {
    let (ws, _) = tokio_tungstenite::connect_async(cfg.server.as_str()).await.map_err(|e| SessionEnd::Lost(e.to_string()))?;
    let (mut sink, mut stream) = ws.split();
    let model = cfg.provider.model.clone();
    let reg = AgentMsg::Register { protocol: PROTOCOL_VERSION, room: cfg.room.clone(), agent_key: cfg.agent_key.clone(), model };
    sink.send(Message::Text(serde_json::to_string(&reg).unwrap_or_default().into())).await.map_err(|e| SessionEnd::Lost(e.to_string()))?;
    match tokio::time::timeout(Duration::from_secs(10), stream.next()).await {
        Ok(Some(Ok(Message::Text(t)))) => match serde_json::from_str::<AgentTask>(&t) {
            Ok(AgentTask::Registered { ok: true, .. }) => {}
            Ok(AgentTask::Registered { ok: false, reason }) => return Err(SessionEnd::Rejected(reason)),
            _ => return Err(SessionEnd::Lost("注册回复无法识别".into())),
        },
        _ => return Err(SessionEnd::Lost("注册没有回复".into())),
    }
    status("在线");
    let (out_tx, mut out_rx) = mpsc::channel::<AgentMsg>(32);
    let writer = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            let Ok(s) = serde_json::to_string(&m) else { continue };
            if sink.send(Message::Text(s.into())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    let running: Arc<Mutex<HashMap<String, CancelToken>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut beat = tokio::time::interval(Duration::from_secs(15));
    let end = loop {
        if stop.load(Ordering::Relaxed) {
            break Ok(());
        }
        tokio::select! {
            _ = beat.tick() => {
                let busy = running.lock().map(|r| !r.is_empty()).unwrap_or(false);
                if out_tx.send(AgentMsg::Heartbeat { busy }).await.is_err() {
                    break Err(SessionEnd::Connected);
                }
            }
            m = tokio::time::timeout(Duration::from_millis(500), stream.next()) => {
                let m = match m {
                    Err(_) => continue,
                    Ok(Some(Ok(m))) => m,
                    Ok(_) => break Err(SessionEnd::Connected),
                };
                let Message::Text(t) = m else {
                    if matches!(m, Message::Close(_)) { break Err(SessionEnd::Connected); }
                    continue;
                };
                match serde_json::from_str::<AgentTask>(&t) {
                    Ok(AgentTask::Chat { task_id, messages, json_schema, max_tokens, temperature, timeout_secs }) => {
                        let cancel = CancelToken::new();
                        running.lock().ok().map(|mut r| r.insert(task_id.clone(), cancel.clone()));
                        let (client, out, running) = (client.clone(), out_tx.clone(), running.clone());
                        tokio::spawn(async move {
                            let req = ChatRequest {
                                messages: messages.into_iter().map(|l| ChatMessage { role: role_of(&l.role), content: l.content, images: vec![] }).collect(),
                                temperature: Some(temperature),
                                max_tokens: Some(max_tokens),
                                format: match json_schema { Some(schema) => OutputFormat::JsonSchema { name: "output".into(), schema }, None => OutputFormat::Text },
                                stream: false,
                                thinking: Some(false),
                            };
                            let c2 = cancel.clone();
                            let call = tokio::task::spawn_blocking(move || client.chat(&req, &c2, &mut |_| {}));
                            let res = match tokio::time::timeout(Duration::from_secs(timeout_secs.max(10) as u64), call).await {
                                Ok(Ok(Ok(r))) => AgentMsg::TaskResult { task_id: task_id.clone(), ok: true, content: r.content, error: String::new() },
                                Ok(Ok(Err(e))) => AgentMsg::TaskResult { task_id: task_id.clone(), ok: false, content: String::new(), error: e.to_string() },
                                Ok(Err(e)) => AgentMsg::TaskResult { task_id: task_id.clone(), ok: false, content: String::new(), error: format!("模型调用崩溃：{e}") },
                                Err(_) => { cancel.cancel(); AgentMsg::TaskResult { task_id: task_id.clone(), ok: false, content: String::new(), error: "本机模型超时".into() } }
                            };
                            running.lock().ok().map(|mut r| r.remove(&task_id));
                            let _ = out.send(res).await;
                        });
                    }
                    Ok(AgentTask::Cancel { task_id }) => {
                        if let Some(c) = running.lock().ok().and_then(|r| r.get(&task_id).cloned()) {
                            c.cancel();
                        }
                    }
                    _ => {}
                }
            }
        }
    };
    if let Ok(r) = running.lock() {
        r.values().for_each(|c| c.cancel());
    }
    drop(out_tx);
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
    end
}

fn role_of(s: &str) -> Role {
    match s {
        "system" => Role::System,
        "assistant" => Role::Assistant,
        _ => Role::User,
    }
}

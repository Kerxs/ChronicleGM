//! 联机（2.0）：桌面端的房主主机模式与 GM Agent。
//!
//! - 房主主机模式：在本机内嵌启动同一个联机服务端（`chronicle-server` 库），世界用当前打开世界的快照；
//!   勾选局域网时监听 0.0.0.0，否则只监听本机；
//! - GM Agent：用设置里配置好的模型服务替服务端调本机模型。连本机服务端（房主主机）或云服务器（`wss://域名/agent`），
//!   都是由这台电脑主动连出去，本机模型端口不会暴露；模型的 API Key 只在这台电脑上；
//! - 云服务器开房：带管理员令牌 POST /rooms（经 Rust 发请求，没有浏览器跨域问题）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{AppState, Res, data_dir, err};

struct HostServer {
    port: u16,
    task: tauri::async_runtime::TokioJoinHandle<()>,
    hub: chronicle_server::Shared,
}

struct AgentRun {
    stop: Arc<AtomicBool>,
    room: String,
}

#[derive(Default)]
pub struct MpState {
    host: tauri::async_runtime::Mutex<Option<HostServer>>,
    agent: Mutex<Option<AgentRun>>,
    agent_status: Arc<Mutex<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStart {
    world_id: Option<String>,
    lan: bool,
    port: u16,
    room_name: String,
    host_name: String,
    max_players: u8,
    provider_id: Option<String>,
}

#[derive(Serialize)]
pub struct HostInfo {
    port: u16,
    /// 朋友可以用的地址（局域网 IP:端口）。
    addresses: Vec<String>,
    room_id: String,
    join_code: String,
    host_token: String,
    agent_key: String,
}

/// 本机的局域网地址（UDP "连接"公网地址只是让系统选出口网卡，不发任何数据）。
fn lan_ip() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then(|| ip.to_string())
}

/// 当前世界的一致快照（`VACUUM INTO`，不受正在写入影响），作为联机房间的世界模板。
fn world_snapshot(state: &AppState, world_id: &str) -> Res<std::path::PathBuf> {
    let dir = data_dir().join("multiplayer").join("templates");
    std::fs::create_dir_all(&dir).map_err(err)?;
    let path = dir.join(format!("{world_id}-{}.cgw", chrono_ms()));
    let w = state.world(world_id)?;
    let w = w.lock().map_err(|_| "世界文件被占用".to_string())?;
    w.conn().execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()]).map_err(err)?;
    Ok(path)
}

fn chrono_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

#[tauri::command]
pub async fn mp_host_start(app: AppHandle, mp: State<'_, MpState>, input: HostStart) -> Res<HostInfo> {
    if input.room_name.trim().is_empty() || input.host_name.trim().is_empty() {
        return Err("房间名和你的名字不能为空".into());
    }
    let state = app.state::<AppState>();
    let template = match input.world_id.as_deref().filter(|s| !s.is_empty()) {
        Some(id) => Some(world_snapshot(&state, id)?),
        None => None,
    };
    let mut host = mp.host.lock().await;
    if host.is_none() {
        let bind = if input.lan { format!("0.0.0.0:{}", input.port) } else { format!("127.0.0.1:{}", input.port) };
        let cfg = chronicle_server::ServerConfig { bind: bind.parse().map_err(err)?, data_dir: data_dir().join("multiplayer").join("host"), max_players: input.max_players.clamp(2, 4), ..Default::default() };
        let (addr, task, hub) = chronicle_server::start(cfg).await.map_err(|e| format!("启动联机服务失败（端口 {} 可能被占用）：{e}", input.port))?;
        *host = Some(HostServer { port: addr.port(), task, hub });
    }
    let h = host.as_ref().ok_or("联机服务没有启动")?;
    let created = chronicle_server::create_room(
        &h.hub,
        chronicle_server::CreateRoom { name: input.room_name.trim().into(), max_players: Some(input.max_players), host_name: input.host_name.trim().into(), gm_mode: Some("local".into()), template },
    )
    .await?;
    let port = h.port;
    drop(host);
    if let Some(pid) = input.provider_id.as_deref().filter(|s| !s.is_empty()) {
        start_agent(&app, &mp, format!("ws://127.0.0.1:{port}/agent"), created.room_id.clone(), created.agent_key.clone(), pid)?;
    }
    let addresses = if input.lan { lan_ip().map(|ip| vec![format!("{ip}:{port}")]).unwrap_or_default() } else { vec![format!("127.0.0.1:{port}")] };
    Ok(HostInfo { port, addresses, room_id: created.room_id, join_code: created.join_code, host_token: created.host_token, agent_key: created.agent_key })
}

#[tauri::command]
pub async fn mp_host_stop(mp: State<'_, MpState>) -> Res<()> {
    stop_agent(&mp);
    if let Some(h) = mp.host.lock().await.take() {
        h.hub.log.info("房主关闭了联机服务");
        h.task.abort();
    }
    Ok(())
}

#[derive(Serialize)]
pub struct MpStatus {
    hosting: bool,
    port: Option<u16>,
    agent_room: Option<String>,
    agent_status: String,
}

#[tauri::command]
pub async fn mp_status(mp: State<'_, MpState>) -> Res<MpStatus> {
    let host = mp.host.lock().await;
    let agent_room = mp.agent.lock().map_err(err)?.as_ref().map(|a| a.room.clone());
    Ok(MpStatus { hosting: host.is_some(), port: host.as_ref().map(|h| h.port), agent_room, agent_status: mp.agent_status.lock().map(|s| s.clone()).unwrap_or_default() })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudCreate {
    url: String,
    admin_token: String,
    room_name: String,
    host_name: String,
    max_players: u8,
    gm_mode: String,
}

#[tauri::command]
pub async fn mp_cloud_create_room(input: CloudCreate) -> Res<chronicle_server::CreatedRoom> {
    let url = format!("{}/rooms", input.url.trim().trim_end_matches('/'));
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("服务器地址要以 https:// 开头".into());
    }
    let body = serde_json::json!({ "name": input.room_name, "max_players": input.max_players, "host_name": input.host_name, "gm_mode": input.gm_mode }).to_string();
    let token = input.admin_token;
    tauri::async_runtime::spawn_blocking(move || {
        let resp = ureq::post(&url).header("content-type", "application/json").header("authorization", &format!("Bearer {token}")).send(body.as_str());
        match resp {
            Ok(mut r) => {
                let text = r.body_mut().read_to_string().map_err(err)?;
                serde_json::from_str::<chronicle_server::CreatedRoom>(&text).map_err(|_| format!("服务器回复无法识别：{}", text.chars().take(200).collect::<String>()))
            }
            Err(ureq::Error::StatusCode(401)) => Err("管理员令牌不对".into()),
            Err(ureq::Error::StatusCode(c)) => Err(format!("服务器拒绝了建房（HTTP {c}）")),
            Err(e) => Err(format!("连不上服务器：{e}")),
        }
    })
    .await
    .map_err(err)?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStart {
    server: String,
    room: String,
    agent_key: String,
    provider_id: String,
}

#[tauri::command]
pub fn mp_agent_start(app: AppHandle, mp: State<'_, MpState>, input: AgentStart) -> Res<()> {
    start_agent(&app, &mp, input.server, input.room, input.agent_key, &input.provider_id)
}

#[tauri::command]
pub fn mp_agent_stop(mp: State<'_, MpState>) -> Res<()> {
    stop_agent(&mp);
    Ok(())
}

fn stop_agent(mp: &MpState) {
    if let Ok(mut a) = mp.agent.lock()
        && let Some(run) = a.take()
    {
        run.stop.store(true, Ordering::Relaxed);
    }
}

fn start_agent(app: &AppHandle, mp: &MpState, server: String, room: String, agent_key: String, provider_id: &str) -> Res<()> {
    let state = app.state::<AppState>();
    let provider = state.provider(provider_id)?;
    let api_key = if provider.needs_key { state.secrets.get(&provider.id)? } else { None };
    if provider.needs_key && api_key.is_none() {
        return Err(format!("「{}」需要 API Key，请先在设置中填写", provider.name));
    }
    stop_agent(mp);
    let stop = Arc::new(AtomicBool::new(false));
    *mp.agent.lock().map_err(err)? = Some(AgentRun { stop: stop.clone(), room: room.clone() });
    let status = mp.agent_status.clone();
    let app2 = app.clone();
    let report: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |s: &str| {
        if let Ok(mut x) = status.lock() {
            *x = s.to_string();
        }
        let _ = app2.emit("mp-agent", s.to_string());
    });
    let cfg = chronicle_server::agent::AgentConfig { server, room, agent_key, provider, api_key };
    tauri::async_runtime::spawn(async move {
        let exit = chronicle_server::agent::run_agent(cfg, stop, report.clone()).await;
        if let chronicle_server::agent::AgentExit::Rejected(r) = exit {
            report(&format!("已停止：{r}"));
        }
    });
    Ok(())
}

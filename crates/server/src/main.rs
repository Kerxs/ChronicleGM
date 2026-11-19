//! chronicle-server：Chronicle GM 联机服务端（云服务器部署用）。
//!
//! `chronicle-server agent ...` 在房主电脑上以命令行方式运行 GM Agent（桌面端内置同样的功能）。

use chronicle_server::{ServerConfig, start};

fn main() {
    if std::env::args().nth(1).as_deref() == Some("agent") {
        run_agent_cli();
        return;
    }
    let cfg = match ServerConfig::from_env_and_args(std::env::args()) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    // 2 核机器：两个工作线程就够了（模型推理不在这台机器上跑）。
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("无法启动运行时");
    rt.block_on(async move {
        let (addr, task, hub) = match start(cfg).await {
            Ok(x) => x,
            Err(e) => {
                eprintln!("启动失败：{e}");
                std::process::exit(1);
            }
        };
        println!("chronicle-server 已在 {addr} 监听");
        tokio::select! {
            _ = task => {}
            _ = tokio::signal::ctrl_c() => {
                hub.log.info("收到退出信号，服务端停止（已提交的状态都在存档里）");
            }
        }
    });
}

/// `chronicle-server agent --server wss://域名/agent --room 房间号 --key Agent密钥 --base-url http://127.0.0.1:8080/v1 --model 模型名`
/// 本机模型的 Key（如果需要）从环境变量 `CHRONICLE_AGENT_API_KEY` 读，不写在命令行里。
fn run_agent_cli() {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned().unwrap_or_default();
    let (server, room, key, base_url, model) = (get("--server"), get("--room"), get("--key"), get("--base-url"), get("--model"));
    if server.is_empty() || room.is_empty() || key.is_empty() || base_url.is_empty() {
        eprintln!("用法：chronicle-server agent --server wss://域名/agent --room 房间号 --key Agent密钥 --base-url http://127.0.0.1:8080/v1 [--model 模型名]");
        std::process::exit(2);
    }
    let api_key = std::env::var("CHRONICLE_AGENT_API_KEY").ok().filter(|k| !k.is_empty());
    let provider = chronicle_llm::ProviderConfig {
        id: "agent".into(),
        name: "房主本机模型".into(),
        base_url,
        model: if model.is_empty() { "local".into() } else { model },
        server_kind: None,
        needs_key: api_key.is_some(),
        timeout_secs: 600,
        capabilities: None,
        allow_campaign_content: true,
        thinking: chronicle_llm::ThinkingMode::Off,
        service: Default::default(),
        media: Default::default(),
    };
    let cfg = chronicle_server::agent::AgentConfig { server, room, agent_key: key, provider, api_key };
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("无法启动运行时");
    let exit = rt.block_on(async move {
        let s2 = stop.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            s2.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        chronicle_server::agent::run_agent(cfg, stop, std::sync::Arc::new(|s: &str| println!("[GM Agent] {s}"))).await
    });
    if let chronicle_server::agent::AgentExit::Rejected(r) = exit {
        eprintln!("服务端拒绝了 GM Agent：{r}");
        std::process::exit(1);
    }
}

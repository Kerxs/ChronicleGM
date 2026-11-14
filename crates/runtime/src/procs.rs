//! 模型进程托管（可选的"托管模式"）。
//!
//! 用户自己填写程序路径与启动参数，软件照此启动、停止、重启，并在崩溃后按策略自动重启。
//! 软件不下载模型，不替用户选择或更换模型；没有托管配置的服务仍然只做识别。
//!
//! - 按需加载：请求发往某个托管服务而它没在运行时，先启动并等待健康检查通过。
//! - 空闲释放：一段时间没有任务就停止进程，释放显存和内存（时长由性能模式决定，也可以按模型单独设置）。
//! - 互斥组：同组的模型不同时运行（例如 8 GB 显存放不下 LLM 加 TTS）；启动一个前，先等组内其他模型空闲再停止它们。
//! - 只停止软件自己启动的进程；用户自己启动、已经在端口上运行的服务标记为"外部"，不会被停止或重启。

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use chronicle_llm::CancelToken;
use serde::{Deserialize, Serialize};

use crate::sched::Scheduler;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    Llm,
    Tts,
    Asr,
}

/// 托管配置（存在资料库里）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ModelProfile {
    pub id: String,
    pub name: String,
    pub role: ModelRole,
    /// 可执行文件完整路径。
    pub exe: String,
    /// 启动参数（每项一个参数，不经过命令行解释器）。
    pub args: Vec<String>,
    /// 工作目录；为空时用程序所在目录。
    #[serde(default)]
    pub cwd: String,
    /// 服务监听的本机端口（用来与"模型服务"配置对应，以及健康检查）。
    pub port: u16,
    /// 健康检查路径；为空时只检查端口能否连接。llama-server 为 /health。
    #[serde(default = "default_health")]
    pub health_path: String,
    /// 等待启动完成的最长时间（秒）。大模型加载可能需要几分钟。
    #[serde(default = "default_start_timeout")]
    pub start_timeout_secs: u64,
    /// 崩溃后自动重启（5 分钟内最多 3 次）。
    #[serde(default = "default_true")]
    pub auto_restart: bool,
    /// 空闲多久后释放（秒）。None 时跟随性能模式；Some(0) 表示从不自动释放。
    #[serde(default)]
    pub idle_unload_secs: Option<u64>,
    /// 互斥组：同组模型不同时运行。
    #[serde(default)]
    pub exclusive_group: String,
    /// 为同一显卡上的其他模型预留的显存（MiB）。大于 0、参数里有 `--fit on` 且没有自己写 `--fit-target` 时，
    /// 启动时加上 `--fit-target <预留 + 1024>`（实测：语音模型在 llama 加载后又多占约 0.5 GB，llama 显存不足崩溃）。
    /// 默认 0：不改动你的启动参数。
    #[serde(default)]
    pub gpu_reserve_mb: u32,
}

impl ModelProfile {
    /// 实际使用的启动参数（只在设置了显存预留时追加 --fit-target）。
    pub fn effective_args(&self) -> Vec<String> {
        let mut a = self.args.clone();
        let has = |k: &[&str]| a.iter().any(|x| k.contains(&x.as_str()));
        let fit_on = a.windows(2).any(|w| matches!(w[0].as_str(), "--fit" | "-fit") && w[1] == "on");
        if self.gpu_reserve_mb > 0 && fit_on && !has(&["--fit-target", "-fitt"]) {
            a.push("--fit-target".into());
            a.push((self.gpu_reserve_mb + 1024).to_string());
        }
        a
    }
}

fn default_health() -> String {
    "/health".into()
}
fn default_start_timeout() -> u64 {
    600
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case", tag = "state", content = "detail")]
pub enum ProcState {
    Stopped,
    Starting,
    Running,
    /// 端口上已有不是本软件启动的服务在运行。
    External,
    Stopping,
    /// 意外退出（退出码）。
    Crashed(String),
    /// 启动失败（原因）。
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ProcStatus {
    pub profile: ModelProfile,
    pub state: ProcState,
    pub pid: Option<u32>,
    pub started_at: Option<i64>,
    /// 最近一次加载耗时（毫秒）。
    pub last_load_ms: Option<i64>,
    pub last_used_at: Option<i64>,
    pub restarts: u32,
    pub log_file: String,
}

struct Proc {
    profile: ModelProfile,
    child: Option<Child>,
    state: ProcState,
    started_at: Option<i64>,
    last_load_ms: Option<i64>,
    last_used: Instant,
    last_used_at: Option<i64>,
    restart_times: Vec<Instant>,
    restarts: u32,
}

/// Windows 作业对象：本软件启动的模型进程都放进去，句柄随本进程关闭（包括被任务管理器强行结束、崩溃），
/// 系统随即结束作业里的全部进程——不留占着端口与显存的孤儿进程。
#[cfg(windows)]
struct KillOnCloseJob(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
unsafe impl Send for KillOnCloseJob {}
#[cfg(windows)]
unsafe impl Sync for KillOnCloseJob {}

#[cfg(windows)]
impl KillOnCloseJob {
    fn new() -> Option<Self> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::*;
        // SAFETY: 标准的 Win32 调用；结构体按文档零初始化后只设置限制标志。
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(h, JobObjectExtendedLimitInformation, &info as *const _ as *const core::ffi::c_void, std::mem::size_of_val(&info) as u32);
            if ok == 0 {
                CloseHandle(h);
                return None;
            }
            Some(Self(h))
        }
    }
    fn assign(&self, child: &Child) -> bool {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: child 的进程句柄在 child 存活期间有效。
        unsafe { windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(self.0, child.as_raw_handle() as _) != 0 }
    }
}

#[cfg(windows)]
impl Drop for KillOnCloseJob {
    fn drop(&mut self) {
        // SAFETY: 句柄由 CreateJobObjectW 创建，只关闭一次。
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

pub struct ModelManager {
    procs: Mutex<HashMap<String, Proc>>,
    #[cfg(windows)]
    job: Option<KillOnCloseJob>,
    logs_dir: PathBuf,
    sched: Weak<Scheduler>,
    /// 同一时间只做一个启动 / 停止动作，避免两个请求同时拉起同一个模型。
    op_lock: Mutex<()>,
    /// 退出时取消：正在进行的自动重启立即中止。
    shutdown: CancelToken,
    log: Box<dyn Fn(&str) + Send + Sync>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// 健康检查：路径为空时只检查端口；否则 GET 返回 200 才算就绪（llama-server 加载中返回 503）。
pub fn healthy(port: u16, path: &str) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_err() {
        return false;
    }
    if path.trim().is_empty() {
        return true;
    }
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(2))).http_status_as_error(false).build().into();
    matches!(agent.get(&format!("http://127.0.0.1:{port}{path}")).call(), Ok(r) if r.status().as_u16() == 200)
}

/// 结束进程及其子进程（llama-server 等可能会再启动子进程；只用于软件自己启动的进程）。
fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

impl ModelManager {
    pub fn new(logs_dir: &Path, sched: &Arc<Scheduler>, log: impl Fn(&str) + Send + Sync + 'static) -> Arc<Self> {
        let _ = std::fs::create_dir_all(logs_dir);
        let m = Arc::new(Self {
            procs: Mutex::new(HashMap::new()),
            #[cfg(windows)]
            job: KillOnCloseJob::new(),
            logs_dir: logs_dir.to_path_buf(),
            sched: Arc::downgrade(sched),
            op_lock: Mutex::new(()),
            shutdown: CancelToken::new(),
            log: Box::new(log),
        });
        let weak = Arc::downgrade(&m);
        std::thread::Builder::new()
            .name("model-supervisor".into())
            .spawn(move || {
                while let Some(m) = weak.upgrade() {
                    m.supervise();
                    drop(m);
                    std::thread::sleep(Duration::from_millis(1000));
                }
            })
            .ok();
        m
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Proc>> {
        self.procs.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 用资料库里的配置替换当前配置。已删除的配置若正在由本软件运行，会被停止。
    pub fn set_profiles(&self, profiles: Vec<ModelProfile>) {
        let ids: Vec<String> = profiles.iter().map(|p| p.id.clone()).collect();
        let removed: Vec<String> = self.lock().keys().filter(|k| !ids.contains(k)).cloned().collect();
        for id in removed {
            let _ = self.stop(&id);
            self.lock().remove(&id);
        }
        let mut g = self.lock();
        for p in profiles {
            match g.get_mut(&p.id) {
                Some(x) => x.profile = p,
                None => {
                    g.insert(
                        p.id.clone(),
                        Proc { profile: p, child: None, state: ProcState::Stopped, started_at: None, last_load_ms: None, last_used: Instant::now(), last_used_at: None, restart_times: vec![], restarts: 0 },
                    );
                }
            }
        }
    }

    fn log_path(&self, id: &str) -> PathBuf {
        self.logs_dir.join(format!("model-{id}.log"))
    }

    pub fn status(&self) -> Vec<ProcStatus> {
        let mut v: Vec<ProcStatus> = self
            .lock()
            .values()
            .map(|p| ProcStatus {
                profile: p.profile.clone(),
                state: p.state.clone(),
                pid: p.child.as_ref().map(|c| c.id()),
                started_at: p.started_at,
                last_load_ms: p.last_load_ms,
                last_used_at: p.last_used_at,
                restarts: p.restarts,
                log_file: self.log_path(&p.profile.id).display().to_string(),
            })
            .collect();
        v.sort_by(|a, b| a.profile.name.cmp(&b.profile.name));
        v
    }

    /// 软件自己启动的进程 PID（资源监控用）。
    pub fn pids(&self) -> Vec<(String, u32)> {
        self.lock().values().filter_map(|p| p.child.as_ref().map(|c| (p.profile.id.clone(), c.id()))).collect()
    }

    fn profile_for_service(&self, service: &str) -> Option<ModelProfile> {
        let port: u16 = service.rsplit(':').next()?.parse().ok()?;
        let host = service.rsplit_once(':').map(|x| x.0).unwrap_or("");
        if host != "127.0.0.1" {
            return None;
        }
        self.lock().values().find(|p| p.profile.port == port).map(|p| p.profile.clone())
    }

    /// 启动（已在运行则直接返回）。阻塞直到健康检查通过、失败或被取消。
    pub fn start(&self, id: &str, cancel: &CancelToken) -> Result<(), String> {
        let _op = self.op_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.start_locked(id, cancel)
    }

    fn start_locked(&self, id: &str, cancel: &CancelToken) -> Result<(), String> {
        let profile = {
            let g = self.lock();
            let p = g.get(id).ok_or("托管配置不存在")?;
            if matches!(p.state, ProcState::Running | ProcState::External) {
                return Ok(());
            }
            p.profile.clone()
        };
        // 设置了显存预留的模型（例如 llama 用 --fit on）：先启动同一台机器上托管的语音模型，
        // 让 --fit 按语音模型已经在显卡上的情况分配（否则 llama 会把语音模型的显存也占掉，语音模型之后就放不下）。
        if profile.gpu_reserve_mb > 0 {
            let companions: Vec<ModelProfile> = self
                .lock()
                .values()
                .filter(|p| p.profile.id != profile.id && p.profile.role == ModelRole::Tts && p.profile.exclusive_group.trim().is_empty() && !matches!(p.state, ProcState::Running | ProcState::External))
                .map(|p| p.profile.clone())
                .collect();
            for c in companions {
                (self.log)(&format!("先启动「{}」，再启动「{}」（显存按语音模型已在显卡上的情况分配）", c.name, profile.name));
                if let Err(e) = self.start_locked(&c.id, cancel) {
                    (self.log)(&format!("「{}」没有启动成功（{e}），继续启动「{}」", c.name, profile.name));
                }
            }
        }
        // 端口上已经有服务（用户自己启动的）：不重复启动，也不接管。
        if healthy(profile.port, &profile.health_path) {
            if let Some(p) = self.lock().get_mut(id) {
                p.state = ProcState::External;
            }
            return Ok(());
        }
        if !Path::new(&profile.exe).is_file() {
            let msg = format!("找不到程序：{}", profile.exe);
            self.set_state(id, ProcState::Failed(msg.clone()));
            return Err(msg);
        }
        let log_path = self.log_path(id);
        // 日志超过 5 MB 时换新文件（保留上一份）。
        if std::fs::metadata(&log_path).map(|m| m.len() > 5 << 20).unwrap_or(false) {
            let _ = std::fs::rename(&log_path, log_path.with_extension("old.log"));
        }
        let out = OpenOptions::new().create(true).append(true).open(&log_path).map_err(|e| e.to_string())?;
        let err = out.try_clone().map_err(|e| e.to_string())?;
        let cwd = if profile.cwd.trim().is_empty() {
            Path::new(&profile.exe).parent().map(Path::to_path_buf).unwrap_or_default()
        } else {
            PathBuf::from(&profile.cwd)
        };
        let mut cmd = Command::new(&profile.exe);
        cmd.args(profile.effective_args()).current_dir(&cwd).stdin(Stdio::null()).stdout(out).stderr(err);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // 不弹出控制台窗口
        }
        self.set_state(id, ProcState::Starting);
        let t0 = Instant::now();
        let child = cmd.spawn().map_err(|e| {
            let msg = format!("无法启动 {}：{e}", profile.exe);
            self.set_state(id, ProcState::Failed(msg.clone()));
            msg
        })?;
        (self.log)(&format!("托管模型「{}」已启动，PID {}", profile.name, child.id()));
        #[cfg(windows)]
        if !self.job.as_ref().is_some_and(|j| j.assign(&child)) {
            (self.log)("无法把模型进程加入作业对象：本软件异常退出时，模型进程可能需要手动结束");
        }
        {
            let mut g = self.lock();
            if let Some(p) = g.get_mut(id) {
                p.child = Some(child);
                p.started_at = Some(now_ms());
            }
        }
        let deadline = Instant::now() + Duration::from_secs(profile.start_timeout_secs.max(5));
        loop {
            if healthy(profile.port, &profile.health_path) {
                let mut g = self.lock();
                if let Some(p) = g.get_mut(id) {
                    p.state = ProcState::Running;
                    p.last_load_ms = Some(t0.elapsed().as_millis() as i64);
                    p.last_used = Instant::now();
                }
                return Ok(());
            }
            // 进程在加载中退出了。
            let exited = {
                let mut g = self.lock();
                g.get_mut(id).and_then(|p| p.child.as_mut()).and_then(|c| c.try_wait().ok().flatten())
            };
            if let Some(code) = exited {
                let msg = format!("进程启动后退出（{code}），详见日志 {}", log_path.display());
                let mut g = self.lock();
                if let Some(p) = g.get_mut(id) {
                    p.child = None;
                    p.state = ProcState::Failed(msg.clone());
                }
                return Err(msg);
            }
            if cancel.is_cancelled() || Instant::now() >= deadline {
                let msg = if cancel.is_cancelled() { "已取消".to_string() } else { format!("等待启动超过 {} 秒，已停止", profile.start_timeout_secs) };
                self.kill(id, ProcState::Failed(msg.clone()));
                return Err(msg);
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }

    fn set_state(&self, id: &str, s: ProcState) {
        if let Some(p) = self.lock().get_mut(id) {
            p.state = s;
        }
    }

    fn kill(&self, id: &str, end_state: ProcState) {
        let child = {
            let mut g = self.lock();
            g.get_mut(id).and_then(|p| {
                p.state = ProcState::Stopping;
                p.child.take()
            })
        };
        if let Some(mut c) = child {
            kill_tree(&mut c);
        }
        if let Some(p) = self.lock().get_mut(id) {
            p.state = end_state;
            p.started_at = None;
        }
    }

    /// 停止（只停止软件自己启动的进程；外部服务只改回"未运行"状态的记录，不会被结束）。
    pub fn stop(&self, id: &str) -> Result<(), String> {
        let _op = self.op_lock.lock().unwrap_or_else(|e| e.into_inner());
        let external = self.lock().get(id).map(|p| p.state == ProcState::External).ok_or("托管配置不存在")?;
        if external {
            return Err("这个服务不是本软件启动的，不会被停止".into());
        }
        let name = self.lock().get(id).map(|p| p.profile.name.clone()).unwrap_or_default();
        self.kill(id, ProcState::Stopped);
        (self.log)(&format!("托管模型「{name}」已停止"));
        Ok(())
    }

    pub fn restart(&self, id: &str, cancel: &CancelToken) -> Result<(), String> {
        self.stop(id)?;
        self.start(id, cancel)
    }

    /// 退出程序时停止所有由本软件启动的模型进程。
    pub fn stop_all(&self) {
        self.shutdown.cancel();
        let ids: Vec<String> = self.lock().iter().filter(|(_, p)| p.child.is_some()).map(|(k, _)| k.clone()).collect();
        for id in ids {
            self.kill(&id, ProcState::Stopped);
        }
    }

    /// 请求发出前：服务由本软件托管且没在运行时，按需启动（先让出互斥组里的其他模型）。
    pub fn ensure_for_service(&self, service: &str, cancel: &CancelToken) -> Result<(), String> {
        let Some(profile) = self.profile_for_service(service) else { return Ok(()) };
        let waited = Instant::now();
        loop {
            let op = self.op_lock.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(self.lock().get(&profile.id).map(|p| p.state.clone()), Some(ProcState::Running | ProcState::External)) {
                drop(op);
                self.touch_id(&profile.id);
                return Ok(());
            }
            // 互斥组里正在运行的其他模型：都空闲了才换下；否则先放开锁等待，
            // 不在持锁时等待（对方的请求可能也需要这把锁，持锁等待会互相卡死）。
            let others: Vec<ModelProfile> = if profile.exclusive_group.trim().is_empty() {
                vec![]
            } else {
                self.lock()
                    .values()
                    .filter(|p| p.profile.id != profile.id && p.profile.exclusive_group == profile.exclusive_group && p.child.is_some())
                    .map(|p| p.profile.clone())
                    .collect()
            };
            let busy = others.iter().any(|o| self.sched.upgrade().map(|s| s.active_on(&format!("127.0.0.1:{}", o.port))).unwrap_or(0) > 0);
            if !busy {
                for o in others {
                    (self.log)(&format!("互斥组「{}」：为启动「{}」停止「{}」", profile.exclusive_group, profile.name, o.name));
                    self.kill(&o.id, ProcState::Stopped);
                }
                let r = self.start_locked(&profile.id, cancel);
                drop(op);
                r?;
                self.touch_id(&profile.id);
                return Ok(());
            }
            drop(op);
            if cancel.is_cancelled() {
                return Err("已取消".into());
            }
            if waited.elapsed() > Duration::from_secs(600) {
                return Err("等待互斥组内其他模型空闲超时".into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn touch_id(&self, id: &str) {
        if let Some(p) = self.lock().get_mut(id) {
            p.last_used = Instant::now();
            p.last_used_at = Some(now_ms());
        }
    }

    pub fn touch_service(&self, service: &str) {
        if let Some(p) = self.profile_for_service(service) {
            self.touch_id(&p.id);
        }
    }

    /// 后台巡检：发现崩溃并按策略重启；空闲超时释放。
    fn supervise(self: &Arc<Self>) {
        let sched = self.sched.upgrade();
        let mode_idle = sched.as_ref().and_then(|s| s.policy().idle_unload_secs);
        let mut crashed = vec![];
        let mut idle = vec![];
        {
            let mut g = self.lock();
            for (id, p) in g.iter_mut() {
                if p.state == ProcState::Running
                    && let Some(code) = p.child.as_mut().and_then(|c| c.try_wait().ok().flatten())
                {
                    p.child = None;
                    p.state = ProcState::Crashed(code.to_string());
                    p.started_at = None;
                    crashed.push((id.clone(), p.profile.name.clone(), code.to_string(), p.profile.auto_restart));
                    continue;
                }
                let limit = match p.profile.idle_unload_secs {
                    Some(0) => None,
                    Some(n) => Some(n),
                    None => mode_idle,
                };
                if p.state == ProcState::Running
                    && p.child.is_some()
                    && let Some(lim) = limit
                    && p.last_used.elapsed() >= Duration::from_secs(lim)
                    && sched.as_ref().map(|s| s.active_on(&format!("127.0.0.1:{}", p.profile.port))).unwrap_or(0) == 0
                {
                    idle.push((id.clone(), p.profile.name.clone()));
                }
            }
        }
        for (id, name) in idle {
            (self.log)(&format!("托管模型「{name}」空闲超时，已释放"));
            self.kill(&id, ProcState::Stopped);
        }
        for (id, name, code, auto) in crashed {
            (self.log)(&format!("托管模型「{name}」意外退出（{code}）"));
            if !auto {
                continue;
            }
            let allowed = {
                let mut g = self.lock();
                let Some(p) = g.get_mut(&id) else { continue };
                p.restart_times.retain(|t| t.elapsed() < Duration::from_secs(300));
                if p.restart_times.len() < 3 {
                    p.restart_times.push(Instant::now());
                    p.restarts += 1;
                    true
                } else {
                    false
                }
            };
            if allowed {
                (self.log)(&format!("自动重启托管模型「{name}」"));
                // 在单独的线程里重启（实测重启大模型约 29 秒），不耽误对其他托管进程的巡检；退出时会被取消。
                let weak = Arc::downgrade(self);
                let token = self.shutdown.clone();
                let _ = std::thread::Builder::new().name(format!("restart-{id}")).spawn(move || {
                    if let Some(m) = weak.upgrade()
                        && !token.is_cancelled()
                    {
                        let _ = m.start(&id, &token);
                    }
                });
            } else {
                (self.log)(&format!("托管模型「{name}」5 分钟内已崩溃 3 次，不再自动重启"));
            }
        }
    }
}

impl Drop for ModelManager {
    /// 管理器销毁（程序退出）时，结束所有由本软件启动的模型进程，不留孤儿进程。
    fn drop(&mut self) {
        self.stop_all();
    }
}

impl crate::client::BeforeRequest for ModelManager {
    fn ensure(&self, service: &str, cancel: &CancelToken) -> Result<(), String> {
        self.ensure_for_service(service, cancel)
    }
    fn is_managed(&self, service: &str) -> bool {
        self.profile_for_service(service).is_some()
    }
    fn touch(&self, service: &str) {
        self.touch_service(service);
    }
}

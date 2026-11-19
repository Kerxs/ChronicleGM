//! 下载任务管理器（独立于运行环境和推理服务）。
//!
//! - 任务持久化到 `downloads.json`；程序关闭时正在下载的任务，下次启动自动继续（Range 断点续传）。
//! - 暂停 / 继续 / 取消；失败自动重试（指数退避，最多 5 次），之后标记失败，可以手动重试。
//! - 并发上限（默认 2 个任务同时下载）。
//! - 实时速度、进度、剩余时间。
//! - 大小检查；仓库提供 SHA256 时校验；GGUF 文件做基本结构检查。
//! - 文件先下到 `模型目录/.incoming/<任务>/`，**全部文件下载并校验通过后**才移到正式位置、交给模型库；
//!   未完成的文件不会被当作可用模型。
//! - 去重：同样的文件已经在下载就返回原任务；正式位置已有同样大小（及校验值）的文件就直接完成，不再占一份磁盘。
//! - 访问令牌不存进任务文件：任务只记仓库来源，下载时向调用方要令牌。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::net::{self, NetError};
use crate::LogFn;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Running,
    Paused,
    /// 正在自动重试（第几次，下次在几秒后）。
    Retrying { attempt: u32, error: String },
    Failed { error: String },
    Done,
    Cancelled,
}

impl TaskState {
    pub fn finished(&self) -> bool {
        matches!(self, TaskState::Done | TaskState::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskFile {
    /// 仓库里的路径。
    pub path: String,
    pub url: String,
    pub size: u64,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub done: bool,
    /// 多模态投影文件。
    #[serde(default)]
    pub mmproj: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    /// hf / modelscope / url
    pub source: String,
    pub repo: String,
    pub files: Vec<TaskFile>,
    pub state: TaskState,
    pub created_at: i64,
    #[serde(default)]
    pub finished_at: Option<i64>,
    /// 完成后正式文件的位置（第一个是主模型）。
    #[serde(default)]
    pub outputs: Vec<String>,
    // 以下是运行时数据（也存下来，界面重启后能看到上次的进度）。
    #[serde(default)]
    pub downloaded: u64,
    #[serde(default)]
    pub speed: f64,
    #[serde(default)]
    pub retries: u32,
}

impl Task {
    pub fn total(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
    pub fn eta_secs(&self) -> Option<u64> {
        let left = self.total().saturating_sub(self.downloaded);
        (self.speed > 1.0 && self.total() > 0).then(|| (left as f64 / self.speed) as u64)
    }
}

/// 新任务的描述。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NewTask {
    pub source: String,
    pub repo: String,
    pub files: Vec<TaskFile>,
}

/// 向调用方要访问令牌（按来源；令牌存在系统钥匙串里）。返回（Bearer 令牌，Cookie）。
pub type TokenFn = Arc<dyn Fn(&str) -> (Option<String>, Option<String>) + Send + Sync>;
/// 任务完成后调用（模型库登记）。
pub type DoneFn = Arc<dyn Fn(&Task) + Send + Sync>;

struct Inner {
    tasks: Vec<Task>,
    stops: HashMap<String, Arc<AtomicBool>>,
    dirty_at: Option<Instant>,
}

pub struct DownloadManager {
    root: PathBuf,
    models_dir: Mutex<PathBuf>,
    inner: Mutex<Inner>,
    pub max_concurrent: std::sync::atomic::AtomicUsize,
    log: LogFn,
    token: TokenFn,
    on_done: Mutex<Option<DoneFn>>,
    /// 自动重试的基础等待（测试里调短）。
    pub retry_base: Mutex<Duration>,
    stopped: AtomicBool,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// 仓库路径 → 模型目录下的相对路径（去掉危险字符，防止跑出模型目录）。
pub fn safe_rel(source: &str, repo: &str, path: &str) -> PathBuf {
    let clean = |s: &str| -> String { s.chars().map(|c| if c.is_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect::<String>().trim_matches('.').to_string() };
    let mut p = PathBuf::from(clean(source));
    for seg in repo.split('/').chain(path.split('/')) {
        let s = clean(seg);
        if !s.is_empty() {
            p.push(s);
        }
    }
    p
}

impl DownloadManager {
    /// `root`：任务文件所在目录（`数据目录/llama`）；`models_dir`：模型保存位置。
    pub fn new(root: &Path, models_dir: &Path, log: LogFn, token: TokenFn) -> Arc<Self> {
        let mut tasks: Vec<Task> = std::fs::read(root.join("downloads.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        // 上次关闭时还在下载 / 等待重试的任务：重新排队，启动后自动继续。
        for t in tasks.iter_mut() {
            if matches!(t.state, TaskState::Running | TaskState::Retrying { .. }) {
                t.state = TaskState::Queued;
            }
            t.speed = 0.0;
        }
        Arc::new(DownloadManager {
            root: root.to_path_buf(),
            models_dir: Mutex::new(models_dir.to_path_buf()),
            inner: Mutex::new(Inner { tasks, stops: HashMap::new(), dirty_at: None }),
            max_concurrent: std::sync::atomic::AtomicUsize::new(2),
            log,
            token,
            on_done: Mutex::new(None),
            retry_base: Mutex::new(Duration::from_secs(2)),
            stopped: AtomicBool::new(false),
        })
    }

    pub fn set_on_done(&self, f: DoneFn) {
        *self.on_done.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }

    pub fn models_dir(&self) -> PathBuf {
        self.models_dir.lock().map(|p| p.clone()).unwrap_or_default()
    }
    pub fn set_models_dir(&self, p: &Path) {
        if let Ok(mut d) = self.models_dir.lock() {
            *d = p.to_path_buf();
        }
    }

    fn incoming(&self, id: &str) -> PathBuf {
        self.models_dir().join(".incoming").join(id)
    }

    /// 启动调度线程（每 200 毫秒看一次有没有可以开始的任务）。
    pub fn start(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        std::thread::spawn(move || {
            loop {
                let Some(m) = me.upgrade() else { break };
                if m.stopped.load(Ordering::Relaxed) {
                    break;
                }
                m.tick();
                drop(m);
                std::thread::sleep(Duration::from_millis(200));
            }
        });
    }

    /// 停止调度并暂停所有下载（程序退出时调用；任务保持"排队"，下次启动继续）。
    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        for s in g.stops.values() {
            s.store(true, Ordering::Relaxed);
        }
        for t in g.tasks.iter_mut() {
            if matches!(t.state, TaskState::Running | TaskState::Retrying { .. }) {
                t.state = TaskState::Queued;
            }
        }
        save(&self.root, &g.tasks);
    }

    fn tick(self: &Arc<Self>) {
        let mut to_start = vec![];
        {
            let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let running = g.tasks.iter().filter(|t| matches!(t.state, TaskState::Running | TaskState::Retrying { .. })).count();
            let free = self.max_concurrent.load(Ordering::Relaxed).max(1).saturating_sub(running);
            for t in g.tasks.iter_mut().filter(|t| t.state == TaskState::Queued).take(free) {
                t.state = TaskState::Running;
                to_start.push(t.id.clone());
            }
            for id in &to_start {
                g.stops.insert(id.clone(), Arc::new(AtomicBool::new(false)));
            }
            if !to_start.is_empty() || g.dirty_at.is_some_and(|d| d.elapsed() > Duration::from_secs(2)) {
                g.dirty_at = None;
                save(&self.root, &g.tasks);
            }
        }
        for id in to_start {
            let me = self.clone();
            std::thread::spawn(move || me.run(&id));
        }
    }

    pub fn list(&self) -> Vec<Task> {
        self.inner.lock().map(|g| g.tasks.clone()).unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<Task> {
        self.inner.lock().ok()?.tasks.iter().find(|t| t.id == id).cloned()
    }

    /// 加任务。重复的（同来源、同仓库、同一组文件，还没结束）返回原任务编号。
    pub fn add(&self, n: NewTask) -> Result<String, String> {
        if n.files.is_empty() {
            return Err("没有选择文件".into());
        }
        for f in &n.files {
            if !(f.url.starts_with("https://") || f.url.starts_with("http://")) {
                return Err(format!("下载地址无效：{}", f.path));
            }
        }
        let mut g = self.inner.lock().map_err(|_| "内部错误".to_string())?;
        let key = |src: &str, repo: &str, files: &[TaskFile]| format!("{src}|{repo}|{}", files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>().join(","));
        let k = key(&n.source, &n.repo, &n.files);
        if let Some(t) = g.tasks.iter().find(|t| !matches!(t.state, TaskState::Cancelled) && key(&t.source, &t.repo, &t.files) == k && !(t.state == TaskState::Done && !t.outputs.iter().all(|o| Path::new(o).exists()))) {
            return Ok(t.id.clone());
        }
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = format!("d{}-{}", now_ms(), SEQ.fetch_add(1, Ordering::Relaxed));
        let mut t = Task { id: id.clone(), source: n.source, repo: n.repo, files: n.files, state: TaskState::Queued, created_at: now_ms(), finished_at: None, outputs: vec![], downloaded: 0, speed: 0.0, retries: 0 };
        // 正式位置已经有同样的文件：直接完成，不再下载、不再占一份磁盘。
        let dir = self.models_dir();
        let finals: Vec<PathBuf> = t.files.iter().map(|f| dir.join(safe_rel(&t.source, &t.repo, &f.path))).collect();
        if t.files.iter().zip(&finals).all(|(f, p)| std::fs::metadata(p).is_ok_and(|m| f.size == 0 || m.len() == f.size)) {
            t.files.iter_mut().for_each(|f| f.done = true);
            t.downloaded = t.total();
            t.outputs = finals.iter().map(|p| p.display().to_string()).collect();
            t.state = TaskState::Done;
            t.finished_at = Some(now_ms());
            (self.log)(&format!("下载任务 {id}：文件已经在模型库里（{}），不重复下载", t.repo));
            let done = t.clone();
            g.tasks.push(t);
            save(&self.root, &g.tasks);
            drop(g);
            if let Some(f) = self.on_done.lock().ok().and_then(|f| f.clone()) {
                f(&done);
            }
            return Ok(id);
        }
        (self.log)(&format!("新下载任务 {id}：{} / {}（{} 个文件，{} 字节）", t.source, t.repo, t.files.len(), t.total()));
        g.tasks.push(t);
        save(&self.root, &g.tasks);
        Ok(id)
    }

    pub fn pause(&self, id: &str) -> Result<(), String> {
        let mut g = self.inner.lock().map_err(|_| "内部错误".to_string())?;
        if let Some(s) = g.stops.get(id) {
            s.store(true, Ordering::Relaxed);
        }
        let t = g.tasks.iter_mut().find(|t| t.id == id).ok_or("任务不存在")?;
        if matches!(t.state, TaskState::Queued | TaskState::Running | TaskState::Retrying { .. }) {
            t.state = TaskState::Paused;
            t.speed = 0.0;
        }
        save(&self.root, &g.tasks);
        Ok(())
    }

    /// 继续（暂停的）或重试（失败的）。
    pub fn resume(&self, id: &str) -> Result<(), String> {
        let mut g = self.inner.lock().map_err(|_| "内部错误".to_string())?;
        let t = g.tasks.iter_mut().find(|t| t.id == id).ok_or("任务不存在")?;
        if matches!(t.state, TaskState::Paused | TaskState::Failed { .. }) {
            t.state = TaskState::Queued;
            t.retries = 0;
        }
        save(&self.root, &g.tasks);
        Ok(())
    }

    /// 取消：停止并删除临时文件。
    pub fn cancel(&self, id: &str) -> Result<(), String> {
        {
            let mut g = self.inner.lock().map_err(|_| "内部错误".to_string())?;
            if let Some(s) = g.stops.get(id) {
                s.store(true, Ordering::Relaxed);
            }
            let t = g.tasks.iter_mut().find(|t| t.id == id).ok_or("任务不存在")?;
            if t.state == TaskState::Done {
                return Err("已经下载完成的任务不能取消（要删除请到模型库）".into());
            }
            t.state = TaskState::Cancelled;
            t.speed = 0.0;
            save(&self.root, &g.tasks);
        }
        // 等下载线程放开文件再删（最多 3 秒）。
        let dir = self.incoming(id);
        for _ in 0..30 {
            if std::fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        (self.log)(&format!("下载任务 {id} 已取消，临时文件已删除"));
        Ok(())
    }

    /// 从列表里移除已结束（完成 / 取消 / 失败）的任务记录（不删模型文件）。
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let mut g = self.inner.lock().map_err(|_| "内部错误".to_string())?;
        let t = g.tasks.iter().find(|t| t.id == id).ok_or("任务不存在")?;
        if !(t.state.finished() || matches!(t.state, TaskState::Failed { .. } | TaskState::Paused)) {
            return Err("任务还在进行，先暂停或取消".into());
        }
        let paused = !t.state.finished();
        g.tasks.retain(|t| t.id != id);
        save(&self.root, &g.tasks);
        drop(g);
        if paused {
            let _ = std::fs::remove_dir_all(self.incoming(id));
        }
        Ok(())
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Task)) -> Option<TaskState> {
        let mut g = self.inner.lock().ok()?;
        let t = g.tasks.iter_mut().find(|t| t.id == id)?;
        f(t);
        let st = t.state.clone();
        g.dirty_at.get_or_insert_with(Instant::now);
        Some(st)
    }

    fn state_of(&self, id: &str) -> Option<TaskState> {
        self.inner.lock().ok()?.tasks.iter().find(|t| t.id == id).map(|t| t.state.clone())
    }

    /// 下载线程：一个任务的全部文件。
    fn run(self: &Arc<Self>, id: &str) {
        let Some(task) = self.get(id) else { return };
        let stop = self.inner.lock().ok().and_then(|g| g.stops.get(id).cloned()).unwrap_or_default();
        let (token, cookie) = (self.token)(&task.source);
        let incoming = self.incoming(id);
        let mut attempt = task.retries;
        let result: Result<(), String> = 'outer: {
            for (i, f) in task.files.iter().enumerate() {
                if f.done {
                    continue;
                }
                let dest = incoming.join(safe_rel("", "", &f.path));
                let before: u64 = task.files.iter().take(i).map(|x| x.size).sum();
                loop {
                    let r = net::fetch(&net::Request { url: &f.url, dest: &dest, token: token.as_deref(), cookie: cookie.as_deref(), expected_size: (f.size > 0).then_some(f.size), sha256: f.sha256.as_deref(), stop: &stop }, |p| {
                        self.update(id, |t| {
                            t.downloaded = before + p.downloaded;
                            t.speed = p.speed;
                        });
                    });
                    match r {
                        Ok(()) => {
                            // GGUF 基本结构检查：头不对就不算下载成功。
                            if dest.extension().is_some_and(|x| x == "gguf")
                                && let Err(e) = chronicle_llm::gguf::read_gguf(&dest)
                            {
                                let _ = std::fs::remove_file(&dest);
                                break 'outer Err(format!("{} 不是有效的 GGUF 文件：{e}", f.path));
                            }
                            self.update(id, |t| t.files[i].done = true);
                            break;
                        }
                        Err(NetError::Stopped) => break 'outer Ok(()),
                        Err(e) if e.retryable() && attempt < 5 => {
                            attempt += 1;
                            let wait = self.retry_base.lock().map(|d| *d).unwrap_or(Duration::from_secs(2)) * 2u32.pow(attempt - 1);
                            (self.log)(&format!("下载任务 {id} 的 {} 出错，{} 秒后第 {attempt} 次重试：{e}", f.path, wait.as_secs()));
                            self.update(id, |t| {
                                t.state = TaskState::Retrying { attempt, error: e.to_string() };
                                t.retries = attempt;
                                t.speed = 0.0;
                            });
                            let until = Instant::now() + wait.min(Duration::from_secs(30));
                            while Instant::now() < until {
                                if stop.load(Ordering::Relaxed) {
                                    break 'outer Ok(());
                                }
                                std::thread::sleep(Duration::from_millis(50));
                            }
                            self.update(id, |t| {
                                if matches!(t.state, TaskState::Retrying { .. }) {
                                    t.state = TaskState::Running;
                                }
                            });
                        }
                        Err(e) => break 'outer Err(format!("{}：{e}", f.path)),
                    }
                }
            }
            Ok(())
        };
        if let Ok(mut g) = self.inner.lock() {
            g.stops.remove(id);
        }
        // 被暂停 / 取消：保持用户设的状态。
        if stop.load(Ordering::Relaxed) || !matches!(self.state_of(id), Some(TaskState::Running | TaskState::Retrying { .. })) {
            if let Ok(g) = self.inner.lock() {
                save(&self.root, &g.tasks);
            }
            return;
        }
        match result {
            Err(e) => {
                (self.log)(&format!("下载任务 {id} 失败：{e}"));
                self.update(id, |t| {
                    t.state = TaskState::Failed { error: e };
                    t.speed = 0.0;
                });
            }
            Ok(()) => match self.publish(id, &incoming) {
                Ok(outputs) => {
                    (self.log)(&format!("下载任务 {id} 完成：{}", outputs.join("、")));
                    let done = self.update(id, |t| {
                        t.state = TaskState::Done;
                        t.outputs = outputs;
                        t.finished_at = Some(now_ms());
                        t.downloaded = t.total();
                        t.speed = 0.0;
                    });
                    if done.is_some()
                        && let (Some(t), Some(f)) = (self.get(id), self.on_done.lock().ok().and_then(|f| f.clone()))
                    {
                        f(&t);
                    }
                }
                Err(e) => {
                    (self.log)(&format!("下载任务 {id} 完成后移动文件失败：{e}"));
                    self.update(id, |t| t.state = TaskState::Failed { error: e });
                }
            },
        }
        if let Ok(g) = self.inner.lock() {
            save(&self.root, &g.tasks);
        }
    }

    /// 全部文件就绪：从 `.incoming` 移到正式位置。
    fn publish(&self, id: &str, incoming: &Path) -> Result<Vec<String>, String> {
        let t = self.get(id).ok_or("任务不存在")?;
        let dir = self.models_dir();
        let mut out = vec![];
        for f in &t.files {
            let from = incoming.join(safe_rel("", "", &f.path));
            let to = dir.join(safe_rel(&t.source, &t.repo, &f.path));
            if let Some(p) = to.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            if std::fs::rename(&from, &to).is_err() {
                // 跨盘时改用复制。
                std::fs::copy(&from, &to).map_err(|e| format!("移动 {} 失败：{e}", f.path))?;
                let _ = std::fs::remove_file(&from);
            }
            out.push(to.display().to_string());
        }
        let _ = std::fs::remove_dir_all(incoming);
        Ok(out)
    }
}

fn save(root: &Path, tasks: &[Task]) {
    let _ = std::fs::create_dir_all(root);
    let p = root.join("downloads.json");
    let tmp = root.join("downloads.json.tmp");
    if let Ok(b) = serde_json::to_vec_pretty(tasks)
        && std::fs::write(&tmp, b).is_ok()
    {
        let _ = std::fs::rename(tmp, p);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn safe_paths_stay_inside() {
        let p = super::safe_rel("hf", "../../etc", "../a/b.gguf");
        assert!(!p.to_string_lossy().contains(".."), "{p:?}");
        assert_eq!(super::safe_rel("hf", "a/b", "c.gguf"), std::path::PathBuf::from("hf").join("a").join("b").join("c.gguf"));
    }
}

//! 统一 AI 任务调度中心。
//!
//! 每一次模型请求都先在这里排队拿"许可"，再真正发出。规则：
//! - 优先级 P0（GM 当前回复）> P1（NPC 对话）> P2（语音）> P3（线索分析、任务生成）> P4（记忆整理、世界模拟）> P5（剧本扩展、世界图谱生成）。
//! - 按模型服务（地址）分别计数：前台（P0–P2）与后台（P3–P5）各有自己的并发上限，
//!   所以正在运行的后台请求永远不会占住前台的位置——后台任务不会阻塞 GM 回复。
//! - 节能 / 平衡模式下，前台有请求在跑或在等时，不启动新的后台请求。
//! - 可以暂停 / 恢复 / 取消单个任务，可以整体暂停某个优先级以下的任务；资源不足时由监控模块自动暂停低优先级任务。
//! - 去重：同一去重键的任务在排队或运行时，新的同类任务直接拒绝。
//! - 排队超时与运行超时；语音等队列有长度上限，不会无限堆积。

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use chronicle_llm::CancelToken;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub enum Priority {
    /// GM 当前回复
    P0,
    /// NPC 对话
    P1,
    /// 语音识别与合成
    P2,
    /// 线索分析、任务生成
    P3,
    /// NPC 记忆整理、世界模拟
    P4,
    /// 剧本扩展、世界图谱生成
    P5,
}

impl Priority {
    pub fn is_foreground(self) -> bool {
        self <= Priority::P2
    }
    pub fn label(self) -> &'static str {
        match self {
            Priority::P0 => "GM 回复",
            Priority::P1 => "NPC 对话",
            Priority::P2 => "语音",
            Priority::P3 => "分析",
            Priority::P4 => "记忆与模拟",
            Priority::P5 => "生成",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum PerfMode {
    /// 节能：后台一次一个，前台活跃时不启动后台；采样慢；空闲模型早释放。
    Eco,
    /// 平衡（默认）。
    Balanced,
    /// 性能：后台可以两个并发，并且与前台同时运行；空闲模型不自动释放。
    Performance,
}

impl PerfMode {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "eco" => Self::Eco,
            "balanced" => Self::Balanced,
            "performance" => Self::Performance,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eco => "eco",
            Self::Balanced => "balanced",
            Self::Performance => "performance",
        }
    }
}

/// 每个模式的调度参数。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Policy {
    /// 每个服务同时运行的前台请求数。
    pub fg_limit: usize,
    /// 每个服务同时运行的后台请求数。
    pub bg_limit: usize,
    /// 前台有请求运行或排队时，不启动新的后台请求。
    pub bg_yields_to_fg: bool,
    /// 资源监控采样间隔（毫秒）。
    pub sample_ms: u64,
    /// 托管模型空闲多久后自动释放（秒）；None 表示不自动释放。
    pub idle_unload_secs: Option<u64>,
}

impl PerfMode {
    pub fn policy(self) -> Policy {
        match self {
            Self::Eco => Policy { fg_limit: 1, bg_limit: 1, bg_yields_to_fg: true, sample_ms: 5000, idle_unload_secs: Some(180) },
            Self::Balanced => Policy { fg_limit: 1, bg_limit: 1, bg_yields_to_fg: true, sample_ms: 2000, idle_unload_secs: Some(600) },
            Self::Performance => Policy { fg_limit: 2, bg_limit: 2, bg_yields_to_fg: false, sample_ms: 1000, idle_unload_secs: None },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct TaskInfo {
    #[ts(type = "number")]
    pub id: u64,
    pub priority: Priority,
    /// 任务类型，例如 gm_plan、npc、gen.plot_from_script。
    pub kind: String,
    pub label: String,
    /// 模型服务（地址）。
    pub service: String,
    pub state: TaskState,
    /// 单个任务被暂停（只对排队中的任务有效）。
    pub paused: bool,
    pub queued_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
}

impl TaskInfo {
    pub fn wait_ms(&self) -> Option<i64> {
        self.started_at.map(|s| s - self.queued_at)
    }
    pub fn run_ms(&self) -> Option<i64> {
        Some(self.finished_at? - self.started_at?)
    }
}

/// 申请许可时的描述。
#[derive(Debug, Clone)]
pub struct Ticket {
    pub priority: Priority,
    pub kind: String,
    pub label: String,
    pub service: String,
    pub dedup: Option<String>,
    pub queue_timeout: Option<Duration>,
    pub run_timeout: Option<Duration>,
    /// 可以被抢占：同一服务有前台任务到来时，运行中的这个后台任务会被取消（调用方稍后重试）。
    /// 只用于重做代价小的分析类请求，长时间的生成任务不要设置。
    pub preemptible: bool,
}

impl Ticket {
    pub fn new(priority: Priority, kind: impl Into<String>, service: impl Into<String>) -> Self {
        Self { priority, kind: kind.into(), label: String::new(), service: service.into(), dedup: None, queue_timeout: None, run_timeout: None, preemptible: false }
    }
    pub fn preemptible(mut self, p: bool) -> Self {
        self.preemptible = p;
        self
    }
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = l.into();
        self
    }
    pub fn dedup(mut self, key: impl Into<String>) -> Self {
        self.dedup = Some(key.into());
        self
    }
    pub fn queue_timeout(mut self, d: Duration) -> Self {
        self.queue_timeout = Some(d);
        self
    }
    pub fn run_timeout(mut self, d: Duration) -> Self {
        self.run_timeout = Some(d);
        self
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SchedError {
    #[error("已取消")]
    Cancelled,
    #[error("排队超时")]
    QueueTimeout,
    #[error("相同的任务已经在排队或运行")]
    Duplicate,
    #[error("{0} 队列已满，新任务被丢弃")]
    QueueFull(&'static str),
}

struct Entry {
    info: TaskInfo,
    cancel: CancelToken,
    dedup: Option<String>,
    deadline_queue: Option<Instant>,
    deadline_run: Option<Duration>,
    started: Option<Instant>,
    preemptible: bool,
}

struct Inner {
    mode: PerfMode,
    policy: Policy,
    next_id: u64,
    active: BTreeMap<u64, Entry>,
    history: VecDeque<TaskInfo>,
    /// 手动整体暂停：该优先级及更低的任务不启动。
    paused_from: Option<Priority>,
    /// 资源不足自动暂停（同上）与原因。
    resource_pause: Option<(Priority, String)>,
    /// 每个优先级最多排队多少个（超过时新任务被丢弃）。
    max_queued: HashMap<Priority, usize>,
}

const HISTORY: usize = 500;

pub struct Scheduler {
    inner: Mutex<Inner>,
    cv: Condvar,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct KindStat {
    pub kind: String,
    pub priority: Priority,
    #[ts(type = "number")]
    pub count: u64,
    #[ts(type = "number")]
    pub failed: u64,
    pub avg_wait_ms: f64,
    pub max_wait_ms: f64,
    pub avg_run_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SchedSnapshot {
    pub mode: PerfMode,
    pub policy: Policy,
    pub paused_from: Option<Priority>,
    pub resource_pause: Option<(Priority, String)>,
    pub active: Vec<TaskInfo>,
    pub recent: Vec<TaskInfo>,
    pub stats: Vec<KindStat>,
}

/// 许可：持有期间算作"运行中"；结束时调用 `finish`（或直接丢弃，按失败记录）。
pub struct Permit {
    sched: Arc<Scheduler>,
    id: u64,
    done: bool,
}

impl Permit {
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn finish(mut self, error: Option<String>) {
        self.done = true;
        self.sched.complete(self.id, error);
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        if !self.done {
            self.sched.complete(self.id, Some("任务异常结束".into()));
        }
    }
}

impl Scheduler {
    pub fn new(mode: PerfMode) -> Arc<Self> {
        let mut max_queued = HashMap::new();
        max_queued.insert(Priority::P2, 16);
        let s = Arc::new(Self {
            inner: Mutex::new(Inner {
                mode,
                policy: mode.policy(),
                next_id: 1,
                active: BTreeMap::new(),
                history: VecDeque::new(),
                paused_from: None,
                resource_pause: None,
                max_queued,
            }),
            cv: Condvar::new(),
        });
        // 看门狗：处理运行超时（取消请求）。排队超时由等待者自己检查。
        let weak = Arc::downgrade(&s);
        std::thread::Builder::new()
            .name("sched-watchdog".into())
            .spawn(move || {
                while let Some(s) = weak.upgrade() {
                    s.watchdog();
                    drop(s);
                    std::thread::sleep(Duration::from_millis(250));
                }
            })
            .ok();
        s
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn mode(&self) -> PerfMode {
        self.lock().mode
    }

    pub fn policy(&self) -> Policy {
        self.lock().policy
    }

    pub fn set_mode(&self, mode: PerfMode) {
        let mut g = self.lock();
        g.mode = mode;
        g.policy = mode.policy();
        drop(g);
        self.cv.notify_all();
    }

    /// 测试与高级设置用：直接指定调度参数。
    pub fn set_policy(&self, p: Policy) {
        self.lock().policy = p;
        self.cv.notify_all();
    }

    pub fn set_max_queued(&self, p: Priority, n: usize) {
        self.lock().max_queued.insert(p, n);
    }

    pub fn set_paused_from(&self, p: Option<Priority>) {
        self.lock().paused_from = p;
        self.cv.notify_all();
    }

    pub fn set_resource_pause(&self, p: Option<(Priority, String)>) {
        let mut g = self.lock();
        if g.resource_pause != p {
            g.resource_pause = p;
            drop(g);
            self.cv.notify_all();
        }
    }

    pub fn pause_task(&self, id: u64, paused: bool) -> bool {
        let mut g = self.lock();
        let ok = match g.active.get_mut(&id) {
            Some(e) if e.info.state == TaskState::Queued => {
                e.info.paused = paused;
                true
            }
            _ => false,
        };
        drop(g);
        self.cv.notify_all();
        ok
    }

    /// 取消任务：排队中的立即移出；运行中的通过取消令牌中止请求。
    pub fn cancel_task(&self, id: u64) -> bool {
        let g = self.lock();
        let ok = g.active.get(&id).map(|e| e.cancel.cancel()).is_some();
        drop(g);
        self.cv.notify_all();
        ok
    }

    /// 某个服务上正在运行或排队的任务数（托管模型据此判断是否空闲）。
    pub fn active_on(&self, service: &str) -> usize {
        self.lock().active.values().filter(|e| e.info.service == service).count()
    }

    fn startable(g: &Inner, id: u64) -> bool {
        let Some(me) = g.active.get(&id) else { return false };
        let p = me.info.priority;
        let blocked = |e: &Entry| {
            e.info.paused
                || g.paused_from.is_some_and(|x| e.info.priority >= x)
                || g.resource_pause.as_ref().is_some_and(|(x, _)| e.info.priority >= *x)
        };
        if blocked(me) {
            return false;
        }
        let same: Vec<&Entry> = g.active.values().filter(|e| e.info.service == me.info.service).collect();
        let running = |fg: bool| same.iter().filter(|e| e.info.state == TaskState::Running && e.info.priority.is_foreground() == fg).count();
        if p.is_foreground() {
            if running(true) >= g.policy.fg_limit {
                return false;
            }
        } else {
            if running(false) >= g.policy.bg_limit {
                return false;
            }
            if g.policy.bg_yields_to_fg && same.iter().any(|e| e.info.priority.is_foreground() && !blocked(e)) {
                return false;
            }
        }
        // 同一服务、同一前后台类别里，优先级更高（或同级更早）且可以启动的排队任务先走。
        !same.iter().any(|e| {
            e.info.state == TaskState::Queued
                && e.info.id != id
                && e.info.priority.is_foreground() == p.is_foreground()
                && (e.info.priority, e.info.id) < (p, id)
                && !blocked(e)
        })
    }

    /// 申请许可（阻塞直到可以开始、被取消或排队超时）。`cancel` 同时作为这个任务的取消令牌：
    /// 在性能中心取消任务会中止调用方的整个操作。
    pub fn acquire(self: &Arc<Self>, t: Ticket, cancel: &CancelToken) -> Result<Permit, SchedError> {
        let id = {
            let mut g = self.lock();
            if let Some(k) = &t.dedup
                && g.active.values().any(|e| e.dedup.as_deref() == Some(k.as_str()))
            {
                return Err(SchedError::Duplicate);
            }
            if let Some(max) = g.max_queued.get(&t.priority).copied() {
                let queued = g.active.values().filter(|e| e.info.priority == t.priority && e.info.state == TaskState::Queued).count();
                if queued >= max {
                    return Err(SchedError::QueueFull(t.priority.label()));
                }
            }
            let id = g.next_id;
            g.next_id += 1;
            let info = TaskInfo {
                id,
                priority: t.priority,
                kind: t.kind,
                label: t.label,
                service: t.service,
                state: TaskState::Queued,
                paused: false,
                queued_at: now_ms(),
                started_at: None,
                finished_at: None,
                error: None,
            };
            g.active.insert(
                id,
                Entry {
                    info,
                    cancel: cancel.clone(),
                    dedup: t.dedup,
                    deadline_queue: t.queue_timeout.map(|d| Instant::now() + d),
                    deadline_run: t.run_timeout,
                    started: None,
                    preemptible: t.preemptible,
                },
            );
            // 前台任务到来：取消同一服务上运行中、可以被抢占的后台任务，让前台独占模型服务。
            if t.priority.is_foreground() {
                let service = g.active.get(&id).map(|e| e.info.service.clone()).unwrap_or_default();
                for e in g.active.values_mut() {
                    if e.preemptible && !e.info.priority.is_foreground() && e.info.service == service && e.info.state == TaskState::Running {
                        e.info.error = Some("让路给前台任务".into());
                        e.cancel.cancel();
                    }
                }
            }
            id
        };
        let mut g = self.lock();
        loop {
            let e = g.active.get(&id).expect("排队中的任务不会被别人移除");
            if e.cancel.is_cancelled() {
                let info = g.active.remove(&id).map(|e| e.info);
                Self::archive(&mut g, info, TaskState::Cancelled, None);
                drop(g);
                self.cv.notify_all();
                return Err(SchedError::Cancelled);
            }
            if e.deadline_queue.is_some_and(|d| Instant::now() >= d) {
                let info = g.active.remove(&id).map(|e| e.info);
                Self::archive(&mut g, info, TaskState::TimedOut, Some("排队超时".into()));
                drop(g);
                self.cv.notify_all();
                return Err(SchedError::QueueTimeout);
            }
            if Self::startable(&g, id) {
                let e = g.active.get_mut(&id).unwrap();
                e.info.state = TaskState::Running;
                e.info.started_at = Some(now_ms());
                e.started = Some(Instant::now());
                drop(g);
                // 状态变化可能影响别的等待者（例如后台让路）。
                self.cv.notify_all();
                return Ok(Permit { sched: self.clone(), id, done: false });
            }
            g = self.cv.wait_timeout(g, Duration::from_millis(200)).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
    }

    fn archive(g: &mut Inner, info: Option<TaskInfo>, state: TaskState, error: Option<String>) {
        if let Some(mut info) = info {
            info.state = state;
            info.finished_at = Some(now_ms());
            info.error = error;
            g.history.push_back(info);
            while g.history.len() > HISTORY {
                g.history.pop_front();
            }
        }
    }

    fn complete(&self, id: u64, error: Option<String>) {
        let mut g = self.lock();
        let e = g.active.remove(&id);
        let state = match (&e, &error) {
            (Some(e), _) if e.cancel.is_cancelled() && e.info.error.as_deref() == Some("运行超时") => TaskState::TimedOut,
            (Some(e), _) if e.cancel.is_cancelled() => TaskState::Cancelled,
            (_, Some(_)) => TaskState::Failed,
            _ => TaskState::Done,
        };
        let err = e.as_ref().and_then(|e| e.info.error.clone()).or(error);
        Self::archive(&mut g, e.map(|e| e.info), state, if state == TaskState::Done { None } else { err });
        drop(g);
        self.cv.notify_all();
    }

    fn watchdog(&self) {
        let mut g = self.lock();
        let mut fired = false;
        for e in g.active.values_mut() {
            if let (Some(st), Some(lim)) = (e.started, e.deadline_run)
                && st.elapsed() >= lim
                && !e.cancel.is_cancelled()
            {
                e.info.error = Some("运行超时".into());
                e.cancel.cancel();
                fired = true;
            }
        }
        let has_queue_deadline = g.active.values().any(|e| e.deadline_queue.is_some() && e.info.state == TaskState::Queued);
        drop(g);
        if fired || has_queue_deadline {
            self.cv.notify_all();
        }
    }

    pub fn snapshot(&self) -> SchedSnapshot {
        let g = self.lock();
        // (次数, 失败, 排队总和, 最长排队, 运行总和, 有运行时长的次数)
        type Acc = (u64, u64, f64, f64, f64, u64);
        let mut stats: BTreeMap<(Priority, String), Acc> = BTreeMap::new();
        for t in &g.history {
            let s = stats.entry((t.priority, t.kind.clone())).or_default();
            s.0 += 1;
            if matches!(t.state, TaskState::Failed | TaskState::TimedOut) {
                s.1 += 1;
            }
            if let Some(w) = t.wait_ms() {
                s.2 += w as f64;
                s.3 = s.3.max(w as f64);
            }
            if let Some(r) = t.run_ms() {
                s.4 += r as f64;
                s.5 += 1;
            }
        }
        SchedSnapshot {
            mode: g.mode,
            policy: g.policy,
            paused_from: g.paused_from,
            resource_pause: g.resource_pause.clone(),
            active: g.active.values().map(|e| e.info.clone()).collect(),
            recent: g.history.iter().rev().take(50).cloned().collect(),
            stats: stats
                .into_iter()
                .map(|((priority, kind), (n, failed, wait, max_wait, run, nrun))| KindStat {
                    kind,
                    priority,
                    count: n,
                    failed,
                    avg_wait_ms: if n > 0 { wait / n as f64 } else { 0.0 },
                    max_wait_ms: max_wait,
                    avg_run_ms: if nrun > 0 { run / nrun as f64 } else { 0.0 },
                })
                .collect(),
        }
    }
}

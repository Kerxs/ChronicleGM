//! 经过调度中心的模型客户端：先拿许可（必要时先让托管模型启动），再发请求。
//! 实现同一个 `LlmClient` 接口，GM 与生成管线不需要改动。

use std::sync::Arc;

use chronicle_llm::client::Progress;
use chronicle_llm::{ChatRequest, ChatResponse, LlmClient};
use chronicle_llm::{CancelToken, LlmError, Result};

use crate::sched::{Priority, SchedError, Scheduler, Ticket};

/// 请求发出前的准备（托管模式下按需启动模型）。
pub trait BeforeRequest: Send + Sync {
    fn ensure(&self, service: &str, cancel: &CancelToken) -> std::result::Result<(), String>;
    /// 这个服务是否由本软件托管（托管的服务崩溃重启期间，请求可以等它恢复后重试）。
    fn is_managed(&self, _service: &str) -> bool {
        false
    }
    /// 每次请求结束后调用（用于记录最近使用时间）。
    fn touch(&self, _service: &str) {}
}

pub struct ScheduledClient {
    inner: Arc<dyn LlmClient>,
    sched: Arc<Scheduler>,
    before: Option<Arc<dyn BeforeRequest>>,
    service: String,
    priority: Priority,
    kind: String,
    label: String,
    dedup: Option<String>,
    preemptible: bool,
}

/// 由服务地址得到调度用的服务键（同一个服务的不同路由共享并发限制）。
pub fn service_key(base_url: &str) -> String {
    let s = base_url.trim().trim_end_matches('/');
    let s = s.strip_prefix("http://").or_else(|| s.strip_prefix("https://")).unwrap_or(s);
    let host = s.split('/').next().unwrap_or(s).to_lowercase();
    host.replace("localhost", "127.0.0.1")
}

impl ScheduledClient {
    pub fn new(inner: Arc<dyn LlmClient>, sched: Arc<Scheduler>, base_url: &str, priority: Priority, kind: impl Into<String>) -> Self {
        Self { inner, sched, before: None, service: service_key(base_url), priority, kind: kind.into(), label: String::new(), dedup: None, preemptible: false }
    }
    pub fn with_before(mut self, b: Option<Arc<dyn BeforeRequest>>) -> Self {
        self.before = b;
        self
    }
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = l.into();
        self
    }
    /// 可以被前台任务抢占（只用于重做代价小的分析请求）。
    pub fn preemptible(mut self, p: bool) -> Self {
        self.preemptible = p;
        self
    }
    pub fn dedup(mut self, k: Option<String>) -> Self {
        self.dedup = k;
        self
    }
    /// 同一个底层客户端，换一个优先级与任务类型（例如 GM 与 NPC 共用一个服务）。
    pub fn retag(&self, priority: Priority, kind: impl Into<String>) -> Self {
        Self {
            inner: self.inner.clone(),
            sched: self.sched.clone(),
            before: self.before.clone(),
            service: self.service.clone(),
            priority,
            kind: kind.into(),
            label: self.label.clone(),
            dedup: None,
            preemptible: self.preemptible,
        }
    }
}

impl LlmClient for ScheduledClient {
    fn chat(&self, req: &ChatRequest, cancel: &CancelToken, progress: Progress) -> Result<ChatResponse> {
        let mut t = Ticket::new(self.priority, self.kind.clone(), self.service.clone()).label(self.label.clone()).preemptible(self.preemptible);
        if let Some(d) = &self.dedup {
            t = t.dedup(d.clone());
        }
        let permit = self.sched.acquire(t, cancel).map_err(|e| match e {
            SchedError::Cancelled => LlmError::Cancelled,
            SchedError::QueueTimeout => LlmError::Timeout,
            other => LlmError::Config(other.to_string()),
        })?;
        // 拿到许可后再确保模型在运行：排队期间不占用启动时间，也不会在别的任务运行时被换下。
        if let Some(b) = &self.before
            && let Err(e) = b.ensure(&self.service, cancel)
        {
            permit.finish(Some(e.clone()));
            return Err(if cancel.is_cancelled() { LlmError::Cancelled } else { LlmError::Transport(e) });
        }
        let mut r = self.inner.chat(req, cancel, &mut *progress);
        // 托管的模型在请求中途崩溃、正在自动重启（实测：显存不足崩溃后返回 503 "Loading model"）：
        // 等巡检发现并重启完成，再重试一次。
        if let (Err(e), Some(b)) = (&r, &self.before)
            && b.is_managed(&self.service)
            && !cancel.is_cancelled()
            && matches!(e, LlmError::Transport(_) | LlmError::Timeout | LlmError::Http { status: 502..=504, .. })
        {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            if b.ensure(&self.service, cancel).is_ok() {
                r = self.inner.chat(req, cancel, progress);
            }
        }
        if let Some(b) = &self.before {
            b.touch(&self.service);
        }
        permit.finish(r.as_ref().err().map(|e| e.to_string()));
        r
    }

    fn describe(&self) -> (String, String) {
        self.inner.describe()
    }
}

#[cfg(test)]
mod tests {
    use super::service_key;

    #[test]
    fn service_keys_normalize_urls() {
        assert_eq!(service_key("http://localhost:8080/v1/"), "127.0.0.1:8080");
        assert_eq!(service_key("http://127.0.0.1:8080"), "127.0.0.1:8080");
        assert_eq!(service_key("https://api.example.com/v1"), "api.example.com");
    }
}

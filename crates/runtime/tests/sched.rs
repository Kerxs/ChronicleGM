//! 调度中心：优先级、前台不被后台阻塞、暂停 / 恢复 / 取消、去重、超时、队列上限、资源暂停。

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chronicle_llm::CancelToken;
use chronicle_runtime::*;

const SVC: &str = "127.0.0.1:8080";

/// 在线程里申请许可、持有 `hold` 后释放；把开始顺序记进 `order`。
fn spawn_task(s: &Arc<Scheduler>, p: Priority, name: &str, hold: Duration, order: &Arc<Mutex<Vec<String>>>) -> thread::JoinHandle<Result<(), SchedError>> {
    let (s, name, order) = (s.clone(), name.to_string(), order.clone());
    thread::spawn(move || {
        let permit = s.acquire(Ticket::new(p, name.clone(), SVC), &CancelToken::new())?;
        order.lock().unwrap().push(name);
        thread::sleep(hold);
        permit.finish(None);
        Ok(())
    })
}

fn wait_until(mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(5), "等待超时");
        thread::sleep(Duration::from_millis(5));
    }
}

fn running(s: &Scheduler) -> Vec<String> {
    s.snapshot().active.into_iter().filter(|t| t.state == TaskState::Running).map(|t| t.kind).collect()
}

#[test]
fn foreground_is_not_blocked_by_running_background() {
    let s = Scheduler::new(PerfMode::Balanced);
    let order = Arc::new(Mutex::new(vec![]));
    // 一个很长的后台生成正在跑。
    let bg = spawn_task(&s, Priority::P5, "gen", Duration::from_millis(800), &order);
    wait_until(|| running(&s) == ["gen"]);
    // GM 回复应立即拿到许可，而不是等后台结束。
    let t = Instant::now();
    let permit = s.acquire(Ticket::new(Priority::P0, "gm", SVC), &CancelToken::new()).unwrap();
    assert!(t.elapsed() < Duration::from_millis(150), "前台等待了 {:?}", t.elapsed());
    permit.finish(None);
    bg.join().unwrap().unwrap();
}

#[test]
fn background_yields_while_foreground_active_in_balanced_mode() {
    let s = Scheduler::new(PerfMode::Balanced);
    let order = Arc::new(Mutex::new(vec![]));
    let gm = s.acquire(Ticket::new(Priority::P0, "gm", SVC), &CancelToken::new()).unwrap();
    let bg = spawn_task(&s, Priority::P5, "gen", Duration::from_millis(10), &order);
    thread::sleep(Duration::from_millis(300));
    assert!(order.lock().unwrap().is_empty(), "前台运行时后台不应启动");
    gm.finish(None);
    bg.join().unwrap().unwrap();
    assert_eq!(*order.lock().unwrap(), ["gen"]);

    // 性能模式：后台可以与前台同时运行。
    s.set_mode(PerfMode::Performance);
    let gm = s.acquire(Ticket::new(Priority::P0, "gm", SVC), &CancelToken::new()).unwrap();
    let t = Instant::now();
    let p = s.acquire(Ticket::new(Priority::P5, "gen2", SVC), &CancelToken::new()).unwrap();
    assert!(t.elapsed() < Duration::from_millis(150));
    p.finish(None);
    gm.finish(None);
}

#[test]
fn queued_tasks_start_by_priority_then_fifo() {
    let s = Scheduler::new(PerfMode::Balanced);
    let order = Arc::new(Mutex::new(vec![]));
    // 先占住后台唯一的位置，再按乱序加入排队。
    let first = s.acquire(Ticket::new(Priority::P3, "hold", SVC), &CancelToken::new()).unwrap();
    let mut hs = vec![];
    for (p, n) in [(Priority::P5, "p5a"), (Priority::P4, "p4"), (Priority::P5, "p5b"), (Priority::P3, "p3")] {
        hs.push(spawn_task(&s, p, n, Duration::from_millis(5), &order));
        thread::sleep(Duration::from_millis(30));
    }
    first.finish(None);
    for h in hs {
        h.join().unwrap().unwrap();
    }
    assert_eq!(*order.lock().unwrap(), ["p3", "p4", "p5a", "p5b"]);
}

#[test]
fn different_services_do_not_block_each_other() {
    let s = Scheduler::new(PerfMode::Eco);
    let a = s.acquire(Ticket::new(Priority::P5, "gen", "127.0.0.1:8080"), &CancelToken::new()).unwrap();
    let t = Instant::now();
    let b = s.acquire(Ticket::new(Priority::P5, "tts", "127.0.0.1:8137"), &CancelToken::new()).unwrap();
    assert!(t.elapsed() < Duration::from_millis(150));
    a.finish(None);
    b.finish(None);
}

#[test]
fn pause_resume_and_cancel() {
    let s = Scheduler::new(PerfMode::Balanced);
    let order = Arc::new(Mutex::new(vec![]));
    s.set_paused_from(Some(Priority::P4));
    let h = spawn_task(&s, Priority::P5, "gen", Duration::from_millis(5), &order);
    thread::sleep(Duration::from_millis(250));
    assert!(order.lock().unwrap().is_empty(), "整体暂停时 P4 及以下不启动");
    s.set_paused_from(None);
    h.join().unwrap().unwrap();

    // 单个任务暂停 / 恢复。
    let blocker = s.acquire(Ticket::new(Priority::P3, "hold", SVC), &CancelToken::new()).unwrap();
    let h = spawn_task(&s, Priority::P5, "later", Duration::from_millis(5), &order);
    wait_until(|| s.snapshot().active.iter().any(|t| t.kind == "later"));
    let id = s.snapshot().active.iter().find(|t| t.kind == "later").unwrap().id;
    assert!(s.pause_task(id, true));
    blocker.finish(None);
    thread::sleep(Duration::from_millis(250));
    assert!(!order.lock().unwrap().contains(&"later".to_string()), "暂停的任务不启动");
    assert!(s.pause_task(id, false));
    h.join().unwrap().unwrap();

    // 取消排队中的任务：立即返回 Cancelled，并记入历史。
    let blocker = s.acquire(Ticket::new(Priority::P3, "hold", SVC), &CancelToken::new()).unwrap();
    let s2 = s.clone();
    let h = thread::spawn(move || s2.acquire(Ticket::new(Priority::P5, "doomed", SVC), &CancelToken::new()).map(|p| p.finish(None)));
    wait_until(|| s.snapshot().active.iter().any(|t| t.kind == "doomed"));
    let id = s.snapshot().active.iter().find(|t| t.kind == "doomed").unwrap().id;
    assert!(s.cancel_task(id));
    assert_eq!(h.join().unwrap(), Err(SchedError::Cancelled));
    blocker.finish(None);
    assert!(s.snapshot().recent.iter().any(|t| t.kind == "doomed" && t.state == TaskState::Cancelled));
}

#[test]
fn cancelling_a_running_task_cancels_the_callers_token() {
    let s = Scheduler::new(PerfMode::Balanced);
    let token = CancelToken::new();
    let p = s.acquire(Ticket::new(Priority::P5, "gen", SVC), &token).unwrap();
    s.cancel_task(p.id());
    assert!(token.is_cancelled(), "正在运行的请求会通过取消令牌中止");
    p.finish(Some("已取消".into()));
    assert_eq!(s.snapshot().recent[0].state, TaskState::Cancelled);
}

#[test]
fn dedup_queue_limit_and_timeouts() {
    let s = Scheduler::new(PerfMode::Balanced);
    let a = s.acquire(Ticket::new(Priority::P5, "gen", SVC).dedup("plot:S1"), &CancelToken::new()).unwrap();
    assert_eq!(s.acquire(Ticket::new(Priority::P5, "gen", SVC).dedup("plot:S1"), &CancelToken::new()).err(), Some(SchedError::Duplicate));
    a.finish(None);

    // 排队超时。
    let hold = s.acquire(Ticket::new(Priority::P5, "hold", SVC), &CancelToken::new()).unwrap();
    let t = Instant::now();
    let r = s.acquire(Ticket::new(Priority::P5, "late", SVC).queue_timeout(Duration::from_millis(300)), &CancelToken::new());
    assert_eq!(r.err(), Some(SchedError::QueueTimeout));
    assert!(t.elapsed() < Duration::from_millis(1000));
    hold.finish(None);

    // 运行超时：看门狗取消令牌，记为 TimedOut。
    let token = CancelToken::new();
    let p = s.acquire(Ticket::new(Priority::P5, "slow", SVC).run_timeout(Duration::from_millis(300)), &token).unwrap();
    wait_until(|| token.is_cancelled());
    p.finish(Some("已取消".into()));
    assert_eq!(s.snapshot().recent[0].state, TaskState::TimedOut);

    // 语音队列有上限：超过的新任务被丢弃，不会无限堆积。
    s.set_max_queued(Priority::P2, 2);
    let voice_hold = s.acquire(Ticket::new(Priority::P2, "tts", "127.0.0.1:8137"), &CancelToken::new()).unwrap();
    let mut hs = vec![];
    for i in 0..2 {
        let s2 = s.clone();
        hs.push(thread::spawn(move || s2.acquire(Ticket::new(Priority::P2, format!("tts{i}"), "127.0.0.1:8137"), &CancelToken::new()).map(|p| p.finish(None))));
    }
    wait_until(|| s.snapshot().active.iter().filter(|t| t.state == TaskState::Queued).count() == 2);
    assert!(matches!(s.acquire(Ticket::new(Priority::P2, "tts9", "127.0.0.1:8137"), &CancelToken::new()).err(), Some(SchedError::QueueFull(_))));
    voice_hold.finish(None);
    for h in hs {
        h.join().unwrap().unwrap();
    }
}

#[test]
fn resource_pressure_pauses_low_priority_only() {
    let s = Scheduler::new(PerfMode::Balanced);
    s.set_resource_pause(Some((Priority::P4, "可用内存不足 1.5 GB".into())));
    let order = Arc::new(Mutex::new(vec![]));
    let low = spawn_task(&s, Priority::P5, "gen", Duration::from_millis(5), &order);
    thread::sleep(Duration::from_millis(250));
    // 前台与 P3 不受影响。
    s.acquire(Ticket::new(Priority::P0, "gm", SVC), &CancelToken::new()).unwrap().finish(None);
    assert!(order.lock().unwrap().is_empty());
    assert!(s.snapshot().resource_pause.is_some());
    s.set_resource_pause(None);
    low.join().unwrap().unwrap();
}

#[test]
fn stats_record_wait_and_run_times() {
    let s = Scheduler::new(PerfMode::Balanced);
    for _ in 0..3 {
        let p = s.acquire(Ticket::new(Priority::P0, "gm_plan", SVC), &CancelToken::new()).unwrap();
        thread::sleep(Duration::from_millis(20));
        p.finish(None);
    }
    let p = s.acquire(Ticket::new(Priority::P0, "gm_plan", SVC), &CancelToken::new()).unwrap();
    p.finish(Some("HTTP 500".into()));
    let st = s.snapshot().stats.into_iter().find(|k| k.kind == "gm_plan").unwrap();
    assert_eq!((st.count, st.failed), (4, 1));
    assert!(st.avg_run_ms >= 10.0, "{st:?}");
}

#[test]
fn many_concurrent_requests_respect_limits() {
    // 压力：60 个任务同时申请，任何时刻每个服务的前台 ≤ 1、后台 ≤ 1。
    let s = Scheduler::new(PerfMode::Balanced);
    let peak = Arc::new(Mutex::new((0usize, 0usize)));
    let now = Arc::new(Mutex::new((0usize, 0usize)));
    let hs: Vec<_> = (0..60)
        .map(|i| {
            let (s, peak, now) = (s.clone(), peak.clone(), now.clone());
            thread::spawn(move || {
                let p = if i % 3 == 0 { Priority::P0 } else { Priority::P5 };
                let permit = s.acquire(Ticket::new(p, "x", SVC), &CancelToken::new()).unwrap();
                {
                    let mut n = now.lock().unwrap();
                    if p.is_foreground() { n.0 += 1 } else { n.1 += 1 }
                    let mut pk = peak.lock().unwrap();
                    pk.0 = pk.0.max(n.0);
                    pk.1 = pk.1.max(n.1);
                }
                thread::sleep(Duration::from_millis(3));
                {
                    let mut n = now.lock().unwrap();
                    if p.is_foreground() { n.0 -= 1 } else { n.1 -= 1 }
                }
                permit.finish(None);
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    assert_eq!(*peak.lock().unwrap(), (1, 1));
    assert!(s.snapshot().active.is_empty());
}

/// 前台任务到来时，运行中、可以被抢占的后台任务被取消；不能抢占的后台任务继续运行。
#[test]
fn foreground_preempts_preemptible_background_only() {
    let s = Scheduler::new(PerfMode::Balanced);
    let bg_cancel = CancelToken::new();
    let bg = s.acquire(Ticket::new(Priority::P3, "director", SVC).preemptible(true), &bg_cancel).unwrap();
    let other_svc_cancel = CancelToken::new();
    let other = s.acquire(Ticket::new(Priority::P3, "director", "127.0.0.1:9999").preemptible(true), &other_svc_cancel).unwrap();
    let fg = s.acquire(Ticket::new(Priority::P0, "gm", SVC), &CancelToken::new()).unwrap();
    assert!(bg_cancel.is_cancelled(), "同一服务上的可抢占后台任务被取消");
    assert!(!other_svc_cancel.is_cancelled(), "别的服务不受影响");
    bg.finish(Some("cancelled".into()));
    other.finish(None);
    fg.finish(None);
    let hist = s.snapshot().recent;
    assert!(hist.iter().any(|t| t.kind == "director" && t.error.as_deref() == Some("让路给前台任务")), "{hist:?}");
    // 不能抢占的后台任务（例如长时间的生成）继续运行。
    let gen_cancel = CancelToken::new();
    let generation = s.acquire(Ticket::new(Priority::P5, "gen", SVC), &gen_cancel).unwrap();
    let fg = s.acquire(Ticket::new(Priority::P0, "gm", SVC), &CancelToken::new()).unwrap();
    assert!(!gen_cancel.is_cancelled());
    fg.finish(None);
    generation.finish(None);
}

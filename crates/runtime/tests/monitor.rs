//! 资源监控在本机真实采样（不是模拟数据）。
use std::time::{Duration, Instant};

use chronicle_runtime::*;

#[test]
fn samples_real_system_resources() {
    let dir = tempfile::tempdir().unwrap();
    let sched = Scheduler::new(PerfMode::Performance); // 1 秒采样
    let models = ModelManager::new(dir.path(), &sched, |_| {});
    let mon = Monitor::start(dir.path().to_path_buf(), &sched, &models);
    let t = Instant::now();
    while mon.history(10).len() < 2 {
        assert!(t.elapsed() < Duration::from_secs(15), "没有采样");
        std::thread::sleep(Duration::from_millis(200));
    }
    let s = mon.latest().unwrap();
    println!("{s:#?}");
    assert!(s.ram_total_mb > 1024 && s.ram_available_mb <= s.ram_total_mb);
    assert!(s.app.mem_mb > 0 && s.app.pids.contains(&std::process::id()));
    assert!(s.disk_free_mb.is_some(), "数据目录所在磁盘");
    // 有 NVIDIA 显卡就应读到显存；没有就要给出说明，而不是编造数据。
    assert!(!s.gpus.is_empty() || s.gpu_note.is_some());
}

//! 测试用：在独立进程里托管一个模型，打印 "ready <模型 PID>" 后一直等待（测试会强杀本进程，检查模型是否随之结束）。
//! 用法：manager_host <模型程序> <端口> <日志目录>

use chronicle_runtime::*;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let port: u16 = a[1].parse().unwrap();
    let sched = Scheduler::new(PerfMode::Balanced);
    let m = ModelManager::new(std::path::Path::new(&a[2]), &sched, |msg| eprintln!("[托管] {msg}"));
    m.set_profiles(vec![ModelProfile {
        id: "m".into(),
        name: "假模型".into(),
        role: ModelRole::Llm,
        exe: a[0].clone(),
        args: vec![port.to_string(), "0".into(), "0".into()],
        cwd: String::new(),
        port,
        health_path: "/health".into(),
        start_timeout_secs: 20,
        auto_restart: false,
        idle_unload_secs: Some(0),
        exclusive_group: String::new(),
        gpu_reserve_mb: 0,
    }]);
    m.start("m", &chronicle_llm::CancelToken::new()).unwrap();
    let pid = m.status()[0].pid.unwrap_or(0);
    println!("ready {pid}");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

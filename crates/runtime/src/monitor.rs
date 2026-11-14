//! 资源监控：CPU、内存、磁盘、本程序与托管模型进程的占用、显卡（NVIDIA 用 nvidia-smi；
//! 其他显卡用 Windows 的"GPU 适配器内存"性能计数器与注册表里的显存大小，不区分厂商）。
//! 按性能模式的间隔采样，保留最近 600 个样本；内存不足时让调度中心暂停低优先级任务。
//! 显存只用于显示：llama-server 加载时会按可用显存分配，显存占满是正常现象，不作为暂停依据。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sysinfo::{Disks, Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::procs::ModelManager;
use crate::sched::{Priority, Scheduler};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ProcUsage {
    pub name: String,
    pub pids: Vec<u32>,
    pub mem_mb: u64,
    /// 私有内存（Windows 的 PrivateUsage；判断内存是否够用、是否在涨要看这个，不是工作集）。
    #[serde(default)]
    pub private_mb: u64,
    pub cpu_percent: f32,
    pub read_kbps: f64,
    pub write_kbps: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct GpuSample {
    pub name: String,
    pub util_percent: f32,
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
    pub temp_c: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ResourceSample {
    pub at: i64,
    pub cpu_percent: f32,
    pub ram_total_mb: u64,
    pub ram_available_mb: u64,
    /// 本程序（含界面 WebView2 子进程）。
    pub app: ProcUsage,
    /// 软件托管的模型进程（含子进程）。
    pub models: Vec<ProcUsage>,
    /// 没有 NVIDIA 显卡或 nvidia-smi 不可用时为 None（不编造数据）。
    pub gpus: Vec<GpuSample>,
    pub gpu_note: Option<String>,
    pub disk_free_mb: Option<u64>,
    pub disk_total_mb: Option<u64>,
}

/// 内存不足时暂停低优先级任务的阈值（MB）。
pub const RAM_PAUSE_P4: u64 = 2048;
pub const RAM_PAUSE_P3: u64 = 1024;

pub fn pressure_pause(ram_available_mb: u64) -> Option<(Priority, String)> {
    if ram_available_mb < RAM_PAUSE_P3 {
        Some((Priority::P3, format!("可用内存只有 {ram_available_mb} MB，已暂停分析、记忆整理与生成任务")))
    } else if ram_available_mb < RAM_PAUSE_P4 {
        Some((Priority::P4, format!("可用内存只有 {ram_available_mb} MB，已暂停记忆整理与生成任务")))
    } else {
        None
    }
}

/// 解析 `nvidia-smi --query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu --format=csv,noheader,nounits`。
pub fn parse_nvidia_smi(out: &str) -> Vec<GpuSample> {
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').map(str::trim).collect();
            if f.len() < 4 {
                return None;
            }
            Some(GpuSample {
                name: f[0].to_string(),
                util_percent: f[1].parse().ok()?,
                mem_used_mb: f[2].parse().ok()?,
                mem_total_mb: f[3].parse().ok()?,
                temp_c: f.get(4).and_then(|t| t.parse().ok()),
            })
        })
        .collect()
}

/// 解析 `typeperf` 的 CSV：返回 (计数器路径, 数值)。
pub fn parse_typeperf(out: &str) -> Vec<(String, f64)> {
    let mut lines = out.lines().filter(|l| l.starts_with('"'));
    let (Some(head), Some(vals)) = (lines.next(), lines.next()) else { return vec![] };
    let split = |l: &str| l.trim().trim_matches('"').split("\",\"").map(str::to_string).collect::<Vec<_>>();
    let (h, v) = (split(head), split(vals));
    h.into_iter().zip(v).skip(1).filter_map(|(k, x)| x.trim().parse::<f64>().ok().map(|n| (k, n))).collect()
}

/// 解析 `reg query ... /s /v HardwareInformation.qwMemorySize`：返回 (注册表键, 显存字节数)。
pub fn parse_reg_memory(out: &str) -> Vec<(String, u64)> {
    let mut key = String::new();
    let mut res = vec![];
    for l in out.lines() {
        let t = l.trim();
        if t.starts_with("HKEY_") {
            key = t.to_string();
        } else if t.starts_with("HardwareInformation.qwMemorySize")
            && let Some(hex) = t.split_whitespace().last()
            && let Ok(n) = u64::from_str_radix(hex.trim_start_matches("0x"), 16)
        {
            res.push((key.clone(), n));
        }
    }
    res
}

fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

const VIDEO_CLASS: &str = r"HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";

/// 不是 NVIDIA（或没有 nvidia-smi）时：用 Windows 性能计数器读显存占用与 3D 引擎占用，用注册表读显存大小。
fn query_gpus_windows() -> Result<Vec<GpuSample>, String> {
    let out = hidden(Command::new("typeperf").args([r"\GPU Adapter Memory(*)\Dedicated Usage", r"\GPU Engine(*engtype_3D)\Utilization Percentage", "-sc", "1"]))
        .output()
        .map_err(|_| "无法读取显卡数据（没有 nvidia-smi，Windows 性能计数器也不可用）".to_string())?;
    let vals = parse_typeperf(&String::from_utf8_lossy(&out.stdout));
    let used = vals.iter().filter(|(k, _)| k.contains("Dedicated Usage")).map(|(_, v)| *v).fold(0.0, f64::max);
    if vals.is_empty() {
        return Err("无法读取显卡数据（没有 nvidia-smi，Windows 性能计数器也不可用）".into());
    }
    let util = vals.iter().filter(|(k, _)| k.contains("Utilization")).map(|(_, v)| *v).sum::<f64>().min(100.0);
    let reg = hidden(Command::new("reg").args(["query", VIDEO_CLASS, "/s", "/v", "HardwareInformation.qwMemorySize"])).output().ok();
    let mems = reg.map(|o| parse_reg_memory(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default();
    let (key, total) = mems.into_iter().max_by_key(|(_, n)| *n).unwrap_or_default();
    let name = if key.is_empty() {
        "显卡".to_string()
    } else {
        hidden(Command::new("reg").args(["query", &key, "/v", "DriverDesc"]))
            .output()
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().find(|l| l.contains("DriverDesc")).and_then(|l| l.split("REG_SZ").nth(1)).map(|s| s.trim().to_string()))
            .unwrap_or_else(|| "显卡".into())
    };
    Ok(vec![GpuSample { name, util_percent: util as f32, mem_used_mb: (used as u64) >> 20, mem_total_mb: total >> 20, temp_c: None }])
}

fn query_gpus() -> Result<(Vec<GpuSample>, bool), String> {
    match query_gpus_nvidia() {
        Ok(g) if !g.is_empty() => Ok((g, false)),
        _ => query_gpus_windows().map(|g| (g, true)),
    }
}

fn query_gpus_nvidia() -> Result<Vec<GpuSample>, String> {
    let mut cmd = Command::new("nvidia-smi");
    cmd.args(["--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu", "--format=csv,noheader,nounits"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|_| "未找到 nvidia-smi".to_string())?;
    if !out.status.success() {
        return Err("nvidia-smi 运行失败".into());
    }
    Ok(parse_nvidia_smi(&String::from_utf8_lossy(&out.stdout)))
}

pub struct Monitor {
    samples: Mutex<VecDeque<ResourceSample>>,
    /// 正在运行的托管模型的内存：加载完成后的第一次采样（基线）与之后的峰值（私有内存）。
    model_mem: Mutex<std::collections::HashMap<String, ModelMem>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ModelMem {
    /// 进入"运行中"后第一次采样的私有内存（MiB）。
    pub baseline_mb: u64,
    pub peak_mb: u64,
    pub current_mb: u64,
}

/// 更新托管模型的内存记录：只记运行中的模型；停止后清掉（下次启动重新记基线）。
pub fn track_model_mem(map: &mut std::collections::HashMap<String, ModelMem>, running: &[String], usage: &[ProcUsage]) {
    map.retain(|id, _| running.contains(id));
    for u in usage.iter().filter(|u| running.contains(&u.name) && u.private_mb > 0) {
        let e = map.entry(u.name.clone()).or_insert(ModelMem { baseline_mb: u.private_mb, peak_mb: 0, current_mb: 0 });
        e.peak_mb = e.peak_mb.max(u.private_mb);
        e.current_mb = u.private_mb;
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// 进程及其全部子孙进程。
fn tree(sys: &System, root: Pid) -> Vec<Pid> {
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        let parent = out[i];
        for (pid, p) in sys.processes() {
            if p.parent() == Some(parent) && !out.contains(pid) {
                out.push(*pid);
            }
        }
        i += 1;
    }
    out
}

fn usage(sys: &System, name: &str, root: Pid, secs: f64) -> ProcUsage {
    usage_excluding(sys, name, root, secs, &[])
}

/// 进程树的占用，去掉 `exclude` 里的进程（本程序的占用不应包含它启动的模型进程）。
fn usage_excluding(sys: &System, name: &str, root: Pid, secs: f64, exclude: &[u32]) -> ProcUsage {
    let pids: Vec<Pid> = tree(sys, root).into_iter().filter(|p| !exclude.contains(&p.as_u32())).collect();
    let mut u = ProcUsage { name: name.to_string(), ..Default::default() };
    for pid in &pids {
        if let Some(p) = sys.process(*pid) {
            u.pids.push(pid.as_u32());
            u.mem_mb += p.memory() >> 20;
            u.private_mb += p.virtual_memory() >> 20;
            u.cpu_percent += p.cpu_usage();
            let d = p.disk_usage();
            if secs > 0.0 {
                u.read_kbps += d.read_bytes as f64 / 1024.0 / secs;
                u.write_kbps += d.written_bytes as f64 / 1024.0 / secs;
            }
        }
    }
    u
}

impl Monitor {
    /// 启动采样线程。`data_dir` 用来统计所在磁盘的剩余空间。
    pub fn start(data_dir: PathBuf, sched: &Arc<Scheduler>, models: &Arc<ModelManager>) -> Arc<Self> {
        let m = Arc::new(Self { samples: Mutex::new(VecDeque::new()), model_mem: Mutex::new(Default::default()) });
        let weak: Weak<Self> = Arc::downgrade(&m);
        let sched = Arc::downgrade(sched);
        let models = Arc::downgrade(models);
        std::thread::Builder::new()
            .name("resource-monitor".into())
            .spawn(move || {
                // 系统信息不可用时监控线程退出，界面显示"无数据"，不影响其他功能。
                let Ok(mut sys) = System::new() else { return };
                let Ok(mut disks) = Disks::new_with_refreshed_list() else { return };
                let me = Pid::from_u32(std::process::id());
                let mut last = Instant::now();
                let mut gpu_err: Option<String> = None;
                let mut gpu_checked = Instant::now() - Duration::from_secs(3600);
                let mut gpu_fallback = false;
                let mut gpus = vec![];
                while let Some(mon) = weak.upgrade() {
                    let interval = sched.upgrade().map(|s| s.policy().sample_ms).unwrap_or(2000);
                    sys.refresh_cpu_usage();
                    sys.refresh_memory();
                    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_memory().with_cpu().with_disk_usage());
                    let secs = last.elapsed().as_secs_f64();
                    last = Instant::now();
                    // 读不到时每 5 分钟才重试一次；用 Windows 计数器时（一次约 1.5 秒）每 10 秒读一次。
                    let due = match (&gpu_err, gpu_fallback) {
                        (Some(_), _) => gpu_checked.elapsed() > Duration::from_secs(300),
                        (None, true) => gpu_checked.elapsed() > Duration::from_secs(10),
                        (None, false) => true,
                    };
                    if due {
                        gpu_checked = Instant::now();
                        match query_gpus() {
                            Ok((g, fallback)) => {
                                gpus = g;
                                gpu_fallback = fallback;
                                gpu_err = None;
                            }
                            Err(e) => {
                                gpus.clear();
                                gpu_err = Some(e);
                            }
                        }
                    }
                    disks.refresh(true);
                    let disk = disks
                        .iter()
                        .filter(|d| data_dir.starts_with(d.mount_point()))
                        .max_by_key(|d| d.mount_point().as_os_str().len());
                    let model_usage: Vec<ProcUsage> = models
                        .upgrade()
                        .map(|mm| mm.pids().into_iter().map(|(id, pid)| usage(&sys, &id, Pid::from_u32(pid), secs)).collect())
                        .unwrap_or_default();
                    if let Some(mm) = models.upgrade() {
                        let running: Vec<String> = mm.status().into_iter().filter(|s| matches!(s.state, crate::procs::ProcState::Running)).map(|s| s.profile.id).collect();
                        track_model_mem(&mut mon.model_mem.lock().unwrap_or_else(|e| e.into_inner()), &running, &model_usage);
                    }
                    let ram_available_mb = sys.available_memory() >> 20;
                    let sample = ResourceSample {
                        at: now_ms(),
                        cpu_percent: sys.global_cpu_usage(),
                        ram_total_mb: sys.total_memory() >> 20,
                        ram_available_mb,
                        // 托管的模型是本程序的子进程：单独统计，不计入本程序（Phase 8 长测发现本程序内存被算成 22 GB）。
                        app: usage_excluding(&sys, "Chronicle GM", me, secs, &model_usage.iter().flat_map(|u| u.pids.iter().copied()).collect::<Vec<_>>()),
                        models: model_usage,
                        gpus: gpus.clone(),
                        gpu_note: if gpu_fallback && gpu_err.is_none() { Some("显存数据来自 Windows 性能计数器（不显示温度）".to_string()) } else { gpu_err.clone() },
                        disk_free_mb: disk.map(|d| d.available_space() >> 20),
                        disk_total_mb: disk.map(|d| d.total_space() >> 20),
                    };
                    if let Some(s) = sched.upgrade() {
                        s.set_resource_pause(pressure_pause(ram_available_mb));
                    }
                    {
                        let mut g = mon.samples.lock().unwrap_or_else(|e| e.into_inner());
                        g.push_back(sample);
                        while g.len() > 600 {
                            g.pop_front();
                        }
                    }
                    drop(mon);
                    std::thread::sleep(Duration::from_millis(interval));
                }
            })
            .ok();
        m
    }

    pub fn latest(&self) -> Option<ResourceSample> {
        self.samples.lock().unwrap_or_else(|e| e.into_inner()).back().cloned()
    }

    /// 正在运行的托管模型的内存记录（配置 ID → 基线 / 峰值 / 当前）。
    pub fn model_mem(&self) -> std::collections::HashMap<String, ModelMem> {
        self.model_mem.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 最近 `n` 个样本（旧的在前）。
    pub fn history(&self, n: usize) -> Vec<ResourceSample> {
        let g = self.samples.lock().unwrap_or_else(|e| e.into_inner());
        g.iter().skip(g.len().saturating_sub(n)).cloned().collect()
    }
}

/// 物理内存总量（MiB）。
pub fn total_ram_mb() -> u64 {
    match System::new() {
        Ok(mut sys) => {
            sys.refresh_memory();
            sys.total_memory() >> 20
        }
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 本机实测（需要 Windows）：cargo test -p chronicle-runtime -- --ignored windows_gpu_fallback --nocapture
    #[test]
    #[ignore]
    fn windows_gpu_fallback_reads_this_machine() {
        let g = query_gpus_windows().unwrap();
        println!("{g:?}");
        assert!(g[0].mem_total_mb > 0);
    }

    #[test]
    fn parses_windows_gpu_counters_and_registry() {
        // 本机实测的输出格式。
        let tp = "\"(PDH-CSV 4.0)\",\"\\\\KERXS\\GPU Adapter Memory(luid_0x00000000_0x0000A95E_phys_0)\\Dedicated Usage\",\"\\\\KERXS\\GPU Adapter Memory(luid_0x00000000_0x0000E197_phys_0)\\Dedicated Usage\"\n\"11/13/2026 22:05:58.986\",\"1361096704.000000\",\"0.000000\"\n";
        let v = parse_typeperf(tp);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].1 as u64) >> 20, 1298);
        let reg = "HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}\\0000\n    HardwareInformation.qwMemorySize    REG_QWORD    0x1ffc00000\n\nEnd of search: 1 match(es) found.\n";
        let m = parse_reg_memory(reg);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].1 >> 20, 8188);
        assert!(m[0].0.ends_with("0000"));
    }

    #[test]
    fn parses_nvidia_smi_csv() {
        let g = parse_nvidia_smi("NVIDIA GeForce RTX 4070 Laptop GPU, 42, 7672, 8188, 61\n");
        assert_eq!(g.len(), 1);
        assert_eq!((g[0].mem_used_mb, g[0].mem_total_mb), (7672, 8188));
        assert_eq!(g[0].temp_c, Some(61.0));
        assert!(parse_nvidia_smi("garbage").is_empty());
    }

    #[test]
    fn memory_pressure_thresholds() {
        assert_eq!(pressure_pause(8000), None);
        assert_eq!(pressure_pause(1500).map(|x| x.0), Some(Priority::P4));
        assert_eq!(pressure_pause(500).map(|x| x.0), Some(Priority::P3));
    }

    /// 基线是进入运行中后的第一次采样；峰值只升不降；停止后清掉，下次启动重新记。
    #[test]
    fn model_mem_baseline_peak_and_reset() {
        let u = |mb| vec![ProcUsage { name: "llama".into(), private_mb: mb, ..Default::default() }];
        let run = vec!["llama".to_string()];
        let mut m = std::collections::HashMap::new();
        track_model_mem(&mut m, &[], &u(1000));
        assert!(m.is_empty(), "没进入运行中（还在加载）时不记");
        track_model_mem(&mut m, &run, &u(25466));
        track_model_mem(&mut m, &run, &u(27000));
        track_model_mem(&mut m, &run, &u(26000));
        assert_eq!(m["llama"], ModelMem { baseline_mb: 25466, peak_mb: 27000, current_mb: 26000 });
        track_model_mem(&mut m, &[], &[]);
        assert!(m.is_empty());
        track_model_mem(&mut m, &run, &u(25000));
        assert_eq!(m["llama"].baseline_mb, 25000);
    }
}


//! 硬件检测：系统、CPU 架构、内存、NVIDIA 显卡（驱动、支持的最高 CUDA 版本、显存）、Vulkan。
//! 检测结果用来选 llama.cpp 构建和自动参数。检测失败的项按"没有"处理，不报错。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Gpu {
    pub name: String,
    pub driver: String,
    pub vram_mb: u64,
    pub vram_free_mb: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Hardware {
    /// windows / linux / macos
    pub os: String,
    /// x64 / arm64
    pub arch: String,
    pub cpu_threads: usize,
    pub ram_mb: u64,
    pub nvidia: Vec<Gpu>,
    /// 驱动支持的最高 CUDA 版本（nvidia-smi 表头里的 "CUDA Version"）。
    pub cuda: Option<(u32, u32)>,
    pub vulkan: bool,
}

impl Hardware {
    pub fn vram_total_mb(&self) -> u64 {
        self.nvidia.iter().map(|g| g.vram_mb).max().unwrap_or(0)
    }
    pub fn vram_free_mb(&self) -> u64 {
        self.nvidia.iter().map(|g| g.vram_free_mb).max().unwrap_or(0)
    }
}

pub fn detect() -> Hardware {
    let os = match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "macos",
        _ => "linux",
    }
    .to_string();
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        _ => "x64",
    }
    .to_string();
    let cpu_threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let (nvidia, cuda) = nvidia_smi();
    Hardware { os, arch, cpu_threads, ram_mb: chronicle_ram_mb(), nvidia, cuda, vulkan: has_vulkan() }
}

fn chronicle_ram_mb() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(s) = std::fs::read_to_string("/proc/meminfo")
            && let Some(l) = s.lines().find(|l| l.starts_with("MemTotal:"))
        {
            return l.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) / 1024;
        }
        0
    }
    #[cfg(windows)]
    {
        // wmic 在新系统上可能没有；用 PowerShell 读总内存。
        let out = hidden("powershell", &["-NoProfile", "-Command", "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory"]);
        out.trim().parse::<u64>().map(|b| b / 1024 / 1024).unwrap_or(0)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        0
    }
}

/// 运行命令（Windows 上不弹出控制台窗口），返回标准输出；失败返回空字符串。
pub fn hidden(cmd: &str, args: &[&str]) -> String {
    let mut c = std::process::Command::new(cmd);
    c.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c.output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

fn nvidia_smi() -> (Vec<Gpu>, Option<(u32, u32)>) {
    let q = hidden("nvidia-smi", &["--query-gpu=name,driver_version,memory.total,memory.free", "--format=csv,noheader,nounits"]);
    let gpus = parse_gpus(&q);
    if gpus.is_empty() {
        return (gpus, None);
    }
    (gpus, parse_cuda_version(&hidden("nvidia-smi", &[])))
}

pub fn parse_gpus(csv: &str) -> Vec<Gpu> {
    csv.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').map(str::trim).collect();
            (f.len() >= 4).then(|| Gpu { name: f[0].into(), driver: f[1].into(), vram_mb: f[2].parse().unwrap_or(0), vram_free_mb: f[3].parse().unwrap_or(0) })
        })
        .collect()
}

/// nvidia-smi 表头：`| NVIDIA-SMI 580.88   Driver Version: 580.88   CUDA Version: 13.0 |`；
/// 新驱动写成 `KMD Version: 621.24   CUDA UMD Version: 13.5`。
pub fn parse_cuda_version(head: &str) -> Option<(u32, u32)> {
    let (i, key) = ["CUDA UMD Version:", "CUDA Version:"].iter().find_map(|k| head.find(k).map(|i| (i, *k)))?;
    let v = head[i + key.len()..].split_whitespace().next()?;
    let mut p = v.split('.');
    Some((p.next()?.parse().ok()?, p.next().and_then(|x| x.parse().ok()).unwrap_or(0)))
}

fn has_vulkan() -> bool {
    #[cfg(windows)]
    {
        let sys = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        std::path::Path::new(&sys).join("System32").join("vulkan-1.dll").exists()
    }
    #[cfg(not(windows))]
    {
        ["/usr/lib/x86_64-linux-gnu/libvulkan.so.1", "/usr/lib/libvulkan.so.1", "/usr/lib64/libvulkan.so.1", "/usr/lib/aarch64-linux-gnu/libvulkan.so.1"].iter().any(|p| std::path::Path::new(p).exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nvidia_smi() {
        let g = parse_gpus("NVIDIA GeForce RTX 4060 Laptop GPU, 580.88, 8188, 7412\n");
        assert_eq!(g[0].vram_mb, 8188);
        assert_eq!(g[0].vram_free_mb, 7412);
        assert_eq!(parse_cuda_version("| NVIDIA-SMI 580.88   Driver Version: 580.88   CUDA Version: 13.0     |"), Some((13, 0)));
        assert_eq!(parse_cuda_version("| NVIDIA-SMI 621.24   KMD Version: 621.24   CUDA UMD Version: 13.5     |"), Some((13, 5)));
        assert_eq!(parse_cuda_version("no gpu"), None);
    }
}

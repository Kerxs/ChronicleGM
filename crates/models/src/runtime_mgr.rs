//! llama.cpp 运行环境管理：从 ggml-org/llama.cpp 官方 GitHub Releases 取预编译构建。
//!
//! - 按硬件选构建：NVIDIA 显卡选驱动支持的最高 CUDA 构建（Windows 上同时下载对应的 cudart 运行库），
//!   其次 Vulkan，最后 CPU；用户也可以指定后端。
//! - 安装：下载（断点续传、有 digest 时校验 SHA256）→ 解压到临时目录 → 找到 llama-server → 自检（`--version`）
//!   → 自检通过才移到 `runtimes/<版本>-<后端>/` 并切换为当前版本。任何一步失败都不影响正在用的版本。
//! - 保留上一个版本，更新失败或新版本有问题可以回滚；更旧的版本自动清理。
//! - 只动 `runtimes/` 和 `downloads/`，不碰模型、配置和存档。
//! - 损坏检测：当前版本的目录或 llama-server 不见了、自检失败 → 状态为"损坏"，可以一键重装。

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::hw::Hardware;
use crate::net::{self, NetError, Progress};
use crate::LogFn;

pub const GITHUB_API: &str = "https://api.github.com";
pub const REPO: &str = "ggml-org/llama.cpp";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub url: String,
    /// GitHub 提供的 `sha256:...`（较新的发布才有）。
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub tag: String,
    pub published_at: String,
    pub assets: Vec<Asset>,
}

impl Release {
    /// `b7123` → 7123（用来比较新旧）。
    pub fn build_number(&self) -> u64 {
        build_number(&self.tag)
    }
}

pub fn build_number(tag: &str) -> u64 {
    tag.trim_start_matches(['b', 'v']).split(|c: char| !c.is_ascii_digit()).next().and_then(|s| s.parse().ok()).unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Backend {
    Cpu,
    Cuda { major: u32, minor: u32 },
    Vulkan,
    /// macOS 构建自带 Metal。
    Metal,
}

impl Backend {
    pub fn label(&self) -> String {
        match self {
            Backend::Cpu => "cpu".into(),
            Backend::Cuda { major, minor } => format!("cuda-{major}.{minor}"),
            Backend::Vulkan => "vulkan".into(),
            Backend::Metal => "metal".into(),
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Backend::Cpu => "cpu",
            Backend::Cuda { .. } => "cuda",
            Backend::Vulkan => "vulkan",
            Backend::Metal => "metal",
        }
    }
}

/// 资源文件名 → （系统、架构、后端）。不认识或不支持的构建（HIP、SYCL、OpenCL……）返回 None。
pub fn classify(name: &str) -> Option<(String, String, Backend)> {
    let n = name.to_ascii_lowercase();
    if !n.starts_with("llama-") || !(n.ends_with(".zip") || n.ends_with(".tar.gz")) || !n.contains("-bin-") {
        return None;
    }
    let os = if n.contains("-win-") {
        "windows"
    } else if n.contains("-macos-") {
        "macos"
    } else if n.contains("-ubuntu-") || n.contains("-linux-") {
        "linux"
    } else {
        return None;
    };
    let arch = if n.contains("arm64") || n.contains("aarch64") {
        "arm64"
    } else if n.contains("x64") || n.contains("x86_64") {
        "x64"
    } else {
        return None;
    };
    for bad in ["hip", "sycl", "opencl", "kompute", "musa", "rocm", "cann", "s390x", "openvino", "radeon", "xcframework"] {
        if n.contains(bad) {
            return None;
        }
    }
    let backend = if let Some(i) = n.find("-cuda-") {
        let v = &n[i + 6..];
        let mut p = v.split(['.', '-']);
        let major = p.next()?.parse().ok()?;
        let minor = p.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        Backend::Cuda { major, minor }
    } else if n.contains("vulkan") {
        Backend::Vulkan
    } else if os == "macos" {
        Backend::Metal
    } else {
        Backend::Cpu
    };
    Some((os.into(), arch.into(), backend))
}

/// 要装的构建：主包 + （Windows CUDA）cudart 运行库包。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub tag: String,
    pub backend: Backend,
    pub assets: Vec<Asset>,
    /// 为什么选它（给用户看）。
    pub reason: String,
}

impl Plan {
    pub fn total_size(&self) -> u64 {
        self.assets.iter().map(|a| a.size).sum()
    }
}

/// 按硬件选构建。`prefer`：cpu / cuda / vulkan（用户指定，没有对应构建时退回自动）。
pub fn choose(rel: &Release, hw: &Hardware, prefer: Option<&str>) -> Option<Plan> {
    let builds: Vec<(&Asset, Backend)> = rel.assets.iter().filter_map(|a| classify(&a.name).filter(|(os, arch, _)| *os == hw.os && *arch == hw.arch).map(|(_, _, b)| (a, b))).collect();
    let cudart = |major: u32, minor: u32| rel.assets.iter().find(|a| a.name.to_ascii_lowercase().starts_with("cudart-") && a.name.contains(&format!("cuda-{major}.{minor}")) && a.name.contains(&hw.arch));
    let pick_cuda = || -> Option<Plan> {
        let drv = hw.cuda?;
        let mut c: Vec<(&Asset, Backend)> = builds.iter().copied().filter(|(_, b)| matches!(b, Backend::Cuda { major, minor } if (*major, *minor) <= drv)).collect();
        c.sort_by_key(|(_, b)| match b {
            Backend::Cuda { major, minor } => (*major, *minor),
            _ => (0, 0),
        });
        let (a, b) = c.pop()?;
        let mut assets = vec![a.clone()];
        if let Backend::Cuda { major, minor } = b
            && hw.os == "windows"
            && let Some(rt) = cudart(major, minor)
        {
            assets.push(rt.clone());
        }
        Some(Plan { tag: rel.tag.clone(), backend: b, assets, reason: format!("检测到 NVIDIA 显卡（驱动支持 CUDA {}.{}），用 {} 构建", drv.0, drv.1, b.label()) })
    };
    let pick = |kind: &str, reason: &str| builds.iter().find(|(_, b)| b.kind() == kind).map(|(a, b)| Plan { tag: rel.tag.clone(), backend: *b, assets: vec![(*a).clone()], reason: reason.into() });
    let auto = || {
        pick_cuda()
            .or_else(|| if hw.vulkan || !hw.nvidia.is_empty() { pick("vulkan", "显卡支持 Vulkan，用 Vulkan 构建") } else { None })
            .or_else(|| pick("metal", "macOS，用自带 Metal 的构建"))
            .or_else(|| pick("cpu", "没有可用的显卡加速，用 CPU 构建"))
    };
    match prefer {
        Some("cuda") => pick_cuda().or_else(auto),
        Some(k @ ("vulkan" | "cpu" | "metal")) => pick(k, &format!("按你的选择用 {k} 构建")).or_else(auto),
        _ => auto(),
    }
}

pub fn parse_releases(v: &serde_json::Value) -> Vec<Release> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter(|r| !r["draft"].as_bool().unwrap_or(false) && !r["prerelease"].as_bool().unwrap_or(false))
                .map(|r| Release {
                    tag: r["tag_name"].as_str().unwrap_or_default().into(),
                    published_at: r["published_at"].as_str().unwrap_or_default().into(),
                    assets: r["assets"]
                        .as_array()
                        .map(|xs| xs.iter().map(|x| Asset { name: x["name"].as_str().unwrap_or_default().into(), size: x["size"].as_u64().unwrap_or(0), url: x["browser_download_url"].as_str().unwrap_or_default().into(), digest: x["digest"].as_str().map(str::to_string) }).collect())
                        .unwrap_or_default(),
                })
                .filter(|r| !r.tag.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

// ===================== 安装状态 =====================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Installed {
    pub tag: String,
    pub backend: Backend,
    /// 相对 `runtimes/` 的目录名。
    pub dir: String,
    pub installed_at: i64,
    /// `llama-server --version` 的输出（第一行）。
    #[serde(default)]
    pub version_line: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RuntimeState {
    pub current: Option<Installed>,
    pub previous: Option<Installed>,
    #[serde(default = "yes")]
    pub auto_check: bool,
    #[serde(default)]
    pub last_check: i64,
    /// 上次检查看到的最新版本。
    #[serde(default)]
    pub latest_seen: Option<String>,
    /// 用户指定的后端（为空 = 自动）。
    #[serde(default)]
    pub prefer: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Health {
    NotInstalled,
    Ok { exe: String },
    Broken { reason: String },
}

/// 自检：运行 `llama-server --version`，返回版本行。测试里换成别的实现。
pub type Verify<'a> = &'a dyn Fn(&Path) -> Result<String, String>;

pub fn server_exe_name() -> &'static str {
    if cfg!(windows) { "llama-server.exe" } else { "llama-server" }
}

/// 真正的自检：30 秒内 `--version` 正常退出并输出版本。
pub fn verify_exe(exe: &Path) -> Result<String, String> {
    let mut c = std::process::Command::new(exe);
    c.arg("--version").stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    if let Some(d) = exe.parent() {
        c.current_dir(d);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let child = c.spawn().map_err(|e| format!("无法运行 llama-server：{e}"))?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let out = rx.recv_timeout(Duration::from_secs(30)).map_err(|_| "llama-server --version 30 秒没有结束".to_string())?.map_err(|e| e.to_string())?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let line = text.lines().find(|l| l.contains("version")).unwrap_or("").trim().to_string();
    if out.status.success() && !line.is_empty() {
        Ok(line)
    } else {
        // 常见原因：缺少 CUDA 运行库或显卡驱动太旧。
        Err(format!("llama-server 自检失败（退出码 {:?}）：{}", out.status.code(), text.lines().rev().take(3).collect::<Vec<_>>().join(" | ")))
    }
}

pub struct RuntimeMgr {
    pub root: PathBuf,
    pub api: String,
    log: LogFn,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl RuntimeMgr {
    /// `root` 是 `数据目录/llama`。
    pub fn new(root: &Path, log: LogFn) -> Self {
        RuntimeMgr { root: root.to_path_buf(), api: std::env::var("CHRONICLE_LLAMA_API").unwrap_or_else(|_| GITHUB_API.into()), log }
    }

    fn state_path(&self) -> PathBuf {
        self.root.join("runtime.json")
    }
    pub fn runtimes_dir(&self) -> PathBuf {
        self.root.join("runtimes")
    }

    pub fn state(&self) -> RuntimeState {
        std::fs::read(self.state_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(RuntimeState { auto_check: true, ..Default::default() })
    }

    pub fn save_state(&self, s: &RuntimeState) -> Result<(), String> {
        std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let tmp = self.state_path().with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(tmp, self.state_path()).map_err(|e| e.to_string())
    }

    pub fn exe_of(&self, i: &Installed) -> PathBuf {
        self.runtimes_dir().join(&i.dir).join(server_exe_name())
    }

    /// 当前 llama-server 的路径（没有安装或已损坏时为 None）。
    pub fn current_exe(&self) -> Option<PathBuf> {
        self.state().current.map(|c| self.exe_of(&c)).filter(|p| p.exists())
    }

    /// 运行环境是否完好。`deep` 时额外跑一次自检。
    pub fn health(&self, deep: Option<Verify>) -> Health {
        let Some(cur) = self.state().current else { return Health::NotInstalled };
        let exe = self.exe_of(&cur);
        if !exe.exists() {
            return Health::Broken { reason: format!("找不到 {}（文件被删除或移动了）", exe.display()) };
        }
        if let Some(v) = deep
            && let Err(e) = v(&exe)
        {
            return Health::Broken { reason: e };
        }
        Health::Ok { exe: exe.display().to_string() }
    }

    /// 读官方 Releases（最近 `n` 个正式版本）。
    pub fn releases(&self, n: usize) -> Result<Vec<Release>, NetError> {
        let v = net::get_json(&format!("{}/repos/{REPO}/releases?per_page={}", self.api.trim_end_matches('/'), n.clamp(1, 30)), None)?;
        Ok(parse_releases(&v))
    }

    /// 检查更新：返回（最新版本，是否比当前新）。记下检查时间。
    pub fn check_update(&self) -> Result<(Release, bool), NetError> {
        let rel = self.releases(1)?.into_iter().next().ok_or_else(|| NetError::Io("没有找到正式发布的版本".into()))?;
        let mut s = self.state();
        s.last_check = now_ms();
        s.latest_seen = Some(rel.tag.clone());
        let newer = s.current.as_ref().is_none_or(|c| rel.build_number() > build_number(&c.tag));
        let _ = self.save_state(&s);
        (self.log)(&format!("llama.cpp 检查更新：最新 {}，当前 {}", rel.tag, s.current.as_ref().map(|c| c.tag.as_str()).unwrap_or("未安装")));
        Ok((rel, newer))
    }

    /// 自动检查是否到时间了（开启且距上次超过一天）。
    pub fn due_for_check(&self) -> bool {
        let s = self.state();
        s.auto_check && now_ms() - s.last_check > 24 * 3600 * 1000
    }

    /// 安装（首次安装、更新、重装都走这里）。失败时当前版本保持不变。
    pub fn install(&self, plan: &Plan, stop: &AtomicBool, mut progress: impl FnMut(&str, Progress), verify: Verify) -> Result<Installed, String> {
        let dl = self.root.join("downloads");
        let rts = self.runtimes_dir();
        std::fs::create_dir_all(&dl).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&rts).map_err(|e| e.to_string())?;
        (self.log)(&format!("安装 llama.cpp {}（{}）：{}", plan.tag, plan.backend.label(), plan.reason));
        // 1. 下载（断点续传；GitHub 给了 digest 就校验）。
        let mut files = vec![];
        for a in &plan.assets {
            let dest = dl.join(&a.name);
            if !dest.exists() {
                let sha = a.digest.as_deref().and_then(|d| d.strip_prefix("sha256:"));
                let mut tries = 0;
                loop {
                    let r = net::fetch(&net::Request { url: &a.url, dest: &dest, token: None, cookie: None, expected_size: (a.size > 0).then_some(a.size), sha256: sha, stop }, |p| progress(&a.name, p));
                    match r {
                        Ok(()) => break,
                        Err(e) if e.retryable() && tries < 3 => {
                            tries += 1;
                            (self.log)(&format!("下载 {} 出错，第 {tries} 次重试：{e}", a.name));
                            std::thread::sleep(Duration::from_secs(2u64.pow(tries)));
                        }
                        Err(e) => {
                            (self.log)(&format!("下载 {} 失败：{e}", a.name));
                            return Err(format!("下载 {} 失败：{e}", a.name));
                        }
                    }
                }
            }
            files.push(dest);
        }
        // 2. 解压到临时目录。
        let name = format!("{}-{}", plan.tag, plan.backend.label());
        let staging = rts.join(format!(".staging-{name}"));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
        let fail = |e: String| {
            let _ = std::fs::remove_dir_all(&staging);
            (self.log)(&format!("安装 llama.cpp {name} 失败：{e}"));
            e
        };
        for f in &files {
            extract(f, &staging).map_err(|e| fail(format!("解压 {} 失败：{e}", f.display())))?;
        }
        // 3. 找到 llama-server，把散落在其他目录的运行库（cudart 等）放到同一目录。
        let exe = find_file(&staging, server_exe_name()).ok_or_else(|| fail("压缩包里没有 llama-server".into()))?;
        let bin = exe.parent().map(Path::to_path_buf).unwrap_or(staging.clone());
        gather_libs(&staging, &bin).map_err(|e| fail(e.to_string()))?;
        // 4. 自检通过才算装好。
        let version_line = verify(&bin.join(server_exe_name())).map_err(fail)?;
        // 5. 移到正式目录（同名旧目录先挪开，成功后删除）。
        let final_dir = rts.join(&name);
        let old = rts.join(format!(".old-{name}"));
        let _ = std::fs::remove_dir_all(&old);
        if final_dir.exists() {
            std::fs::rename(&final_dir, &old).map_err(|e| fail(format!("替换旧目录失败：{e}")))?;
        }
        if let Err(e) = std::fs::rename(&bin, &final_dir) {
            let _ = std::fs::rename(&old, &final_dir);
            return Err(fail(format!("移动到正式目录失败：{e}")));
        }
        let _ = std::fs::remove_dir_all(&old);
        let _ = std::fs::remove_dir_all(&staging);
        // 6. 切换当前版本，保留上一个版本，清理更旧的。
        let inst = Installed { tag: plan.tag.clone(), backend: plan.backend, dir: name.clone(), installed_at: now_ms(), version_line };
        let mut s = self.state();
        if s.current.as_ref().is_some_and(|c| c.dir != inst.dir) {
            s.previous = s.current.take();
        }
        s.current = Some(inst.clone());
        s.prefer = s.prefer.clone();
        self.save_state(&s)?;
        self.prune(&s);
        for f in files {
            let _ = std::fs::remove_file(f);
        }
        (self.log)(&format!("llama.cpp {name} 安装完成：{}", inst.version_line));
        Ok(inst)
    }

    /// 回滚到上一个版本（上一个版本还完好时）。
    pub fn rollback(&self) -> Result<Installed, String> {
        let mut s = self.state();
        let prev = s.previous.clone().ok_or("没有可以回滚的上一个版本")?;
        if !self.exe_of(&prev).exists() {
            return Err("上一个版本的文件已经不在了".into());
        }
        s.previous = s.current.take();
        s.current = Some(prev.clone());
        self.save_state(&s)?;
        (self.log)(&format!("llama.cpp 回滚到 {}", prev.dir));
        Ok(prev)
    }

    /// 只保留当前和上一个版本的目录（以及临时目录以外的其他东西不动）。
    fn prune(&self, s: &RuntimeState) {
        let keep: Vec<String> = [&s.current, &s.previous].iter().filter_map(|x| x.as_ref().map(|i| i.dir.clone())).collect();
        if let Ok(rd) = std::fs::read_dir(self.runtimes_dir()) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if e.path().is_dir() && !keep.contains(&n) {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
    }
}

/// 解压 zip / tar.gz（防止路径穿越：只接受压缩包内的相对路径）。
pub fn extract(archive: &Path, to: &Path) -> Result<(), String> {
    let name = archive.to_string_lossy().to_ascii_lowercase();
    if name.ends_with(".zip") {
        let f = std::fs::File::open(archive).map_err(|e| e.to_string())?;
        let mut z = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
        for i in 0..z.len() {
            let mut e = z.by_index(i).map_err(|e| e.to_string())?;
            let Some(rel) = e.enclosed_name() else { continue };
            let out = to.join(rel);
            if e.is_dir() {
                std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
                continue;
            }
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            let mut w = std::fs::File::create(&out).map_err(|e| e.to_string())?;
            std::io::copy(&mut e, &mut w).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            if let Some(mode) = e.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode));
            }
        }
        Ok(())
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        let f = std::fs::File::open(archive).map_err(|e| e.to_string())?;
        let mut t = tar::Archive::new(flate2::read::GzDecoder::new(f));
        // tar 的 unpack 会拒绝 ".." 和绝对路径。
        t.unpack(to).map_err(|e| e.to_string())
    } else {
        Err("不认识的压缩格式".into())
    }
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut subdirs = vec![];
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            subdirs.push(p);
        } else if p.file_name().is_some_and(|n| n == name) {
            return Some(p);
        }
    }
    subdirs.into_iter().find_map(|d| find_file(&d, name))
}

/// 把 `root` 下（不在 `bin` 里的）动态库复制到 `bin`（Windows 的 cudart 包解压在另一处）。
fn gather_libs(root: &Path, bin: &Path) -> std::io::Result<()> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        if d == bin {
            continue;
        }
        for e in std::fs::read_dir(&d)?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "dll" || x == "so" || x == "dylib") || p.to_string_lossy().contains(".so.") {
                let target = bin.join(p.file_name().unwrap_or_default());
                if !target.exists() {
                    std::fs::copy(&p, target)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(names: &[&str]) -> Release {
        Release { tag: "b7100".into(), published_at: String::new(), assets: names.iter().map(|n| Asset { name: n.to_string(), size: 1, url: format!("https://x/{n}"), digest: None }).collect() }
    }
    fn hw(os: &str, cuda: Option<(u32, u32)>, vulkan: bool) -> Hardware {
        Hardware { os: os.into(), arch: "x64".into(), nvidia: if cuda.is_some() { vec![Default::default()] } else { vec![] }, cuda, vulkan, ..Default::default() }
    }
    const NAMES: &[&str] = &[
        "llama-b7100-bin-win-cpu-x64.zip",
        "llama-b7100-bin-win-cpu-arm64.zip",
        "llama-b7100-bin-win-cuda-12.4-x64.zip",
        "llama-b7100-bin-win-cuda-13.1-x64.zip",
        "cudart-llama-bin-win-cuda-12.4-x64.zip",
        "cudart-llama-bin-win-cuda-13.1-x64.zip",
        "llama-b7100-bin-win-vulkan-x64.zip",
        "llama-b7100-bin-win-hip-radeon-x64.zip",
        "llama-b7100-bin-win-sycl-x64.zip",
        "llama-b7100-bin-ubuntu-x64.tar.gz",
        "llama-b7100-bin-ubuntu-vulkan-x64.tar.gz",
        "llama-b7100-bin-macos-arm64.zip",
        "llama-b7100-xcframework.zip",
    ];

    #[test]
    fn picks_highest_cuda_the_driver_supports_with_its_runtime() {
        let p = choose(&rel(NAMES), &hw("windows", Some((13, 0)), true), None).unwrap();
        assert_eq!(p.backend, Backend::Cuda { major: 12, minor: 4 }, "驱动只支持 13.0，不能用 13.1");
        assert_eq!(p.assets.len(), 2);
        assert!(p.assets[1].name.starts_with("cudart-") && p.assets[1].name.contains("12.4"));
        let p = choose(&rel(NAMES), &hw("windows", Some((13, 2)), true), None).unwrap();
        assert_eq!(p.backend, Backend::Cuda { major: 13, minor: 1 });
    }

    #[test]
    fn falls_back_to_vulkan_then_cpu_and_respects_preference() {
        assert_eq!(choose(&rel(NAMES), &hw("windows", None, true), None).unwrap().backend, Backend::Vulkan);
        assert_eq!(choose(&rel(NAMES), &hw("windows", None, false), None).unwrap().backend, Backend::Cpu);
        assert_eq!(choose(&rel(NAMES), &hw("windows", Some((13, 0)), true), Some("cpu")).unwrap().backend, Backend::Cpu);
        assert_eq!(choose(&rel(NAMES), &hw("linux", None, true), None).unwrap().assets[0].name, "llama-b7100-bin-ubuntu-vulkan-x64.tar.gz");
        let mut mac = hw("macos", None, false);
        mac.arch = "arm64".into();
        assert_eq!(choose(&rel(NAMES), &mac, None).unwrap().backend, Backend::Metal);
        assert_eq!(classify("llama-b7100-bin-win-hip-radeon-x64.zip"), None, "不支持的后端不选");
    }

    #[test]
    fn build_numbers_compare() {
        assert!(build_number("b7100") > build_number("b6999"));
        assert_eq!(build_number("v1.2"), 1);
    }
}

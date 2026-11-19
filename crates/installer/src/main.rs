//! Chronicle GM 自研安装 / 卸载程序。
//!
//! - 安装：把嵌入的程序解压到安装目录（默认沿用已安装的位置，否则 `%LOCALAPPDATA%\Programs\Chronicle GM`），
//!   复制自己为 `uninstall.exe`，建开始菜单 / 桌面快捷方式，写入「应用和功能」里的卸载信息（当前用户，不需要管理员）。
//! - 卸载：`uninstall.exe`（或带 `--uninstall`）——删快捷方式、卸载信息和安装目录；可选同时删除用户数据（默认保留）。
//! - 静默更新：带 `/S`、`/P` 或 `/UPDATE`（自动更新器调用安装包时用）时不显示界面，装到原来的位置后重新打开程序。
//! - 运行前检查 WebView2（界面需要它）；没有时用系统对话框询问是否下载安装（来自 Microsoft 官方地址）。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

static PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.xz"));
const PAYLOAD_SIZE: &str = include_str!(concat!(env!("OUT_DIR"), "/payload_size.txt"));
const APP_EXE: &str = "chronicle-gm-app.exe";
const PRODUCT: &str = "Chronicle GM";

/// 卸载项和快捷方式用的名字。和以前的 NSIS 安装包同名：从旧版升级时「应用和功能」里不会出现两项。
/// 测试时用环境变量 CHRONICLE_SETUP_NAME 换一个名字，不碰真实安装的那一份。
fn product_name() -> String {
    std::env::var("CHRONICLE_SETUP_NAME").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| PRODUCT.to_string())
}

fn uninstall_key() -> String {
    format!(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\{}", product_name())
}
const VERSION: &str = env!("CARGO_PKG_VERSION");

static BUSY: AtomicBool = AtomicBool::new(false);

fn hidden(exe: &str) -> std::process::Command {
    let mut c = std::process::Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}

fn local_appdata() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn reg_query(key: &str, value: &str) -> Option<String> {
    let out = hidden("reg").args(["query", key, "/v", value]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let line = text.lines().find(|l| l.trim_start().starts_with(value))?;
    let mut parts = line.trim().splitn(3, "    ");
    parts.next();
    parts.next();
    Some(parts.next()?.trim().trim_matches('"').to_string())
}

fn reg_set(name: &str, kind: &str, data: &str) -> Result<(), String> {
    let key = uninstall_key();
    let out = hidden("reg").args(["add", key.as_str(), "/v", name, "/t", kind, "/d", data, "/f"]).output().map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(format!("写入卸载信息失败：{}", String::from_utf8_lossy(&out.stderr).trim())) }
}

/// 已安装的位置（读卸载信息；旧版 NSIS 安装的也认得）。
fn installed_dir() -> Option<PathBuf> {
    let p = reg_query(&uninstall_key(), "InstallLocation").map(PathBuf::from).or_else(|| {
        reg_query(&uninstall_key(), "UninstallString").and_then(|s| PathBuf::from(s.trim_matches('"')).parent().map(Path::to_path_buf))
    })?;
    p.join(APP_EXE).exists().then_some(p)
}

fn default_dir() -> PathBuf {
    installed_dir().unwrap_or_else(|| local_appdata().join("Programs").join(product_name()))
}

/// 用户数据位置（和主程序一致：默认 `%LOCALAPPDATA%\ChronicleGM`，迁移过的看指针文件）。
fn data_dir() -> PathBuf {
    std::fs::read_to_string(local_appdata().join("ChronicleGM-location.txt")).ok().map(|s| PathBuf::from(s.trim())).filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| local_appdata().join("ChronicleGM"))
}

fn uninstall_mode() -> bool {
    let me = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase())).unwrap_or_default();
    me.starts_with("uninstall") || std::env::args().any(|a| a == "--uninstall")
}

fn silent_mode() -> bool {
    std::env::args().skip(1).any(|a| matches!(a.to_ascii_uppercase().as_str(), "/S" | "/P" | "/UPDATE" | "--SILENT"))
}

/// 程序是否还开着（只看安装目录里的那一份）。
fn app_running(dir: &Path) -> bool {
    let target = dir.join(APP_EXE).to_string_lossy().to_ascii_lowercase();
    let out = hidden("powershell").args(["-NoProfile", "-Command", "Get-Process chronicle-gm-app -ErrorAction SilentlyContinue | ForEach-Object { $_.Path }"]).output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.trim().to_ascii_lowercase() == target)).unwrap_or(false)
}

fn stop_app(dir: &Path) {
    let target = dir.join(APP_EXE).to_string_lossy().replace('\'', "''");
    let _ = hidden("powershell")
        .args(["-NoProfile", "-Command", &format!("Get-Process chronicle-gm-app -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -eq '{target}' }} | Stop-Process -Force")])
        .output();
    std::thread::sleep(std::time::Duration::from_millis(800));
}

fn shortcut(lnk: &Path, target: &Path) -> Result<(), String> {
    if let Some(p) = lnk.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let ps = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.WorkingDirectory='{}';$s.IconLocation='{},0';$s.Save()",
        lnk.display().to_string().replace('\'', "''"),
        target.display().to_string().replace('\'', "''"),
        target.parent().map(|p| p.display().to_string()).unwrap_or_default().replace('\'', "''"),
        target.display().to_string().replace('\'', "''"),
    );
    let out = hidden("powershell").args(["-NoProfile", "-Command", &ps]).output().map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(format!("创建快捷方式失败：{}", String::from_utf8_lossy(&out.stderr).trim())) }
}

fn start_menu_lnk() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default().join(r"Microsoft\Windows\Start Menu\Programs").join(format!("{}.lnk", product_name()))
}

fn desktop_lnk() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default().join("Desktop").join(format!("{}.lnk", product_name()))
}

fn progress(app: Option<&AppHandle>, stage: &str, pct: f64) {
    if let Some(a) = app {
        let _ = a.emit("progress", serde_json::json!({ "stage": stage, "pct": pct }));
    }
}

/// 安装（界面和静默更新共用）。
fn do_install(app: Option<&AppHandle>, dir: &Path, desktop: bool, start_menu: bool) -> Result<(), String> {
    if PAYLOAD.is_empty() {
        return Err("这个安装程序是开发调试版，没有包含要安装的程序".into());
    }
    let total: u64 = PAYLOAD_SIZE.trim().parse().unwrap_or(0);
    progress(app, "准备安装位置", 2.0);
    std::fs::create_dir_all(dir).map_err(|e| format!("建不了安装目录 {}：{e}", dir.display()))?;
    if app_running(dir) {
        progress(app, "关闭正在运行的 Chronicle GM", 4.0);
        stop_app(dir);
    }
    // 解压到临时文件，完整写完再替换，避免装到一半留下坏文件。
    let exe = dir.join(APP_EXE);
    let tmp = dir.join(format!("{APP_EXE}.new"));
    {
        // xz 解压直接写进文件；写的时候顺便报告进度。
        struct Counting<'a> {
            f: std::fs::File,
            done: u64,
            total: u64,
            last: u64,
            app: Option<&'a AppHandle>,
        }
        impl std::io::Write for Counting<'_> {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                let n = self.f.write(b)?;
                self.done += n as u64;
                if self.total > 0 && self.done - self.last > 512 * 1024 {
                    self.last = self.done;
                    progress(self.app, "复制程序文件", 5.0 + 75.0 * self.done as f64 / self.total as f64);
                }
                Ok(n)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.f.flush()
            }
        }
        let f = std::fs::File::create(&tmp).map_err(|e| format!("写入失败（目录没有权限？）：{e}"))?;
        let mut out = std::io::BufWriter::with_capacity(1 << 20, Counting { f, done: 0, total, last: 0, app });
        lzma_rs::xz_decompress(&mut std::io::BufReader::new(PAYLOAD), &mut out).map_err(|e| format!("安装包损坏或写入失败：{e:?}"))?;
        std::io::Write::flush(&mut out).map_err(|e| format!("写入失败（磁盘空间不够？）：{e}"))?;
        let written = out.get_ref().done;
        if total > 0 && written != total {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("安装包不完整（{written}/{total} 字节）"));
        }
    }
    let mut tries = 0;
    loop {
        let _ = std::fs::remove_file(&exe);
        match std::fs::rename(&tmp, &exe) {
            Ok(()) => break,
            Err(_) if tries < 10 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            Err(e) => return Err(format!("替换程序文件失败（Chronicle GM 是否还开着？）：{e}")),
        }
    }
    progress(app, "写入卸载程序", 84.0);
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let un = dir.join("uninstall.exe");
    if me != un {
        std::fs::copy(&me, &un).map_err(|e| format!("复制卸载程序失败：{e}"))?;
    }
    progress(app, "创建快捷方式", 90.0);
    if start_menu {
        shortcut(&start_menu_lnk(), &exe)?;
    }
    if desktop {
        shortcut(&desktop_lnk(), &exe)?;
    }
    progress(app, "登记到「应用和功能」", 95.0);
    reg_set("DisplayName", "REG_SZ", &product_name())?;
    reg_set("DisplayVersion", "REG_SZ", VERSION)?;
    reg_set("Publisher", "REG_SZ", "Chronicle GM")?;
    reg_set("DisplayIcon", "REG_SZ", &exe.display().to_string())?;
    reg_set("InstallLocation", "REG_SZ", &dir.display().to_string())?;
    reg_set("UninstallString", "REG_SZ", &format!("\"{}\"", un.display()))?;
    reg_set("QuietUninstallString", "REG_SZ", &format!("\"{}\" /S", un.display()))?;
    reg_set("NoModify", "REG_DWORD", "1")?;
    reg_set("NoRepair", "REG_DWORD", "1")?;
    reg_set("EstimatedSize", "REG_DWORD", &((total / 1024).max(1)).to_string())?;
    progress(app, "完成", 100.0);
    Ok(())
}

fn do_uninstall(app: Option<&AppHandle>, remove_data: bool) -> Result<(), String> {
    let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).ok_or("找不到安装目录")?;
    progress(app, "关闭正在运行的 Chronicle GM", 10.0);
    if app_running(&dir) {
        stop_app(&dir);
    }
    progress(app, "删除快捷方式", 30.0);
    for l in [start_menu_lnk(), desktop_lnk()] {
        let _ = std::fs::remove_file(l);
    }
    progress(app, "从「应用和功能」移除", 45.0);
    let _ = hidden("reg").args(["delete", uninstall_key().as_str(), "/f"]).output();
    progress(app, "删除程序文件", 60.0);
    let _ = std::fs::remove_file(dir.join(APP_EXE));
    if remove_data {
        progress(app, "删除用户数据", 80.0);
        let d = data_dir();
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_file(local_appdata().join("ChronicleGM-location.txt"));
        // WebView2 的缓存（界面设置、本机存储）。
        let _ = std::fs::remove_dir_all(local_appdata().join("com.chroniclegm.desktop"));
    }
    progress(app, "完成", 100.0);
    Ok(())
}

/// 卸载程序删不了正在运行的自己：退出后由一个隐藏的 cmd 稍等片刻再删除安装目录。
fn schedule_self_delete() {
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        let mut c = hidden("cmd");
        // cmd 不认 Rust 默认的 \" 转义：整条命令原样传过去。
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.raw_arg(format!("/C ping 127.0.0.1 -n 3 > nul & rmdir /S /Q \"{}\"", dir.display()));
        }
        let _ = c.spawn();
    }
}

// ===================== 界面命令 =====================

#[derive(Serialize)]
struct Info {
    mode: &'static str,
    version: &'static str,
    default_dir: String,
    installed: bool,
    size_mb: f64,
    data_dir: String,
}

#[tauri::command]
fn info() -> Info {
    let size: f64 = PAYLOAD_SIZE.trim().parse().unwrap_or(0.0);
    Info {
        mode: if uninstall_mode() { "uninstall" } else { "install" },
        version: VERSION,
        default_dir: default_dir().display().to_string(),
        installed: installed_dir().is_some(),
        size_mb: (size / 1048576.0 * 10.0).round() / 10.0,
        data_dir: data_dir().display().to_string(),
    }
}

#[tauri::command]
async fn install(app: AppHandle, dir: String, desktop: bool, start_menu: bool) -> Result<(), String> {
    if BUSY.swap(true, Ordering::SeqCst) {
        return Err("正在进行".into());
    }
    let r = tauri::async_runtime::spawn_blocking(move || {
        let d = PathBuf::from(dir.trim());
        if d.as_os_str().is_empty() {
            return Err("请选择安装位置".to_string());
        }
        do_install(Some(&app), &d, desktop, start_menu)
    })
    .await
    .map_err(|e| e.to_string())?;
    BUSY.store(false, Ordering::SeqCst);
    r
}

#[tauri::command]
async fn uninstall(app: AppHandle, remove_data: bool) -> Result<(), String> {
    if BUSY.swap(true, Ordering::SeqCst) {
        return Err("正在进行".into());
    }
    let r = tauri::async_runtime::spawn_blocking(move || do_uninstall(Some(&app), remove_data)).await.map_err(|e| e.to_string())?;
    BUSY.store(false, Ordering::SeqCst);
    r
}

#[tauri::command]
fn launch(dir: String) -> Result<(), String> {
    let exe = PathBuf::from(dir.trim()).join(APP_EXE);
    std::process::Command::new(&exe).current_dir(exe.parent().unwrap_or(Path::new("."))).spawn().map_err(|e| format!("打开失败：{e}"))?;
    Ok(())
}

#[tauri::command]
fn finish(app: AppHandle, self_delete: bool) {
    if self_delete {
        schedule_self_delete();
    }
    app.exit(0);
}

// ===================== WebView2 =====================

fn has_webview2() -> bool {
    const GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    [format!(r"HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{GUID}"), format!(r"HKLM\SOFTWARE\Microsoft\EdgeUpdate\Clients\{GUID}"), format!(r"HKCU\Software\Microsoft\EdgeUpdate\Clients\{GUID}")]
        .iter()
        .any(|k| reg_query(k, "pv").is_some_and(|v| !v.is_empty() && v != "0.0.0.0"))
}

/// 没有 WebView2：用系统对话框询问，同意后从 Microsoft 官方地址下载安装程序并静默安装。
fn ensure_webview2() -> bool {
    if has_webview2() {
        return true;
    }
    let ask = "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.MessageBox]::Show('Chronicle GM 需要 Microsoft Edge WebView2 运行库（Windows 10/11 通常已自带）。是否现在从 Microsoft 官网下载并安装？（约 2 MB）', 'Chronicle GM 安装程序', 'YesNo', 'Question')";
    let yes = hidden("powershell").args(["-NoProfile", "-Command", ask]).output().map(|o| String::from_utf8_lossy(&o.stdout).contains("Yes")).unwrap_or(false);
    if !yes {
        return false;
    }
    let path = std::env::temp_dir().join("MicrosoftEdgeWebview2Setup.exe");
    let ok = ureq::get("https://go.microsoft.com/fwlink/p/?LinkId=2124703")
        .call()
        .ok()
        .and_then(|mut r| r.body_mut().with_config().limit(50 * 1024 * 1024).read_to_vec().ok())
        .and_then(|b| std::fs::write(&path, b).ok())
        .is_some();
    ok && std::process::Command::new(&path).args(["/silent", "/install"]).status().map(|s| s.success()).unwrap_or(false) && has_webview2()
}

fn main() {
    // 静默模式（自动更新）：不显示界面，装到原来的位置，装完重新打开程序。
    if silent_mode() {
        let code = if uninstall_mode() {
            let r = do_uninstall(None, false);
            schedule_self_delete();
            r
        } else {
            let dir = default_dir();
            let r = do_install(None, &dir, false, true);
            if r.is_ok() {
                let exe = dir.join(APP_EXE);
                let _ = std::process::Command::new(&exe).current_dir(&dir).spawn();
            }
            r
        };
        std::process::exit(if code.is_ok() { 0 } else { 1 });
    }
    if !ensure_webview2() {
        std::process::exit(1);
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![info, install, uninstall, launch, finish])
        .setup(|app| {
            if uninstall_mode()
                && let Some(w) = app.get_webview_window("main")
            {
                let _ = w.set_title("卸载 Chronicle GM");
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("安装程序启动失败");
}

//! 软件的基础设置：系统托盘、关闭窗口时退出还是转到后台、开机自启（可以启动后直接在后台）、打开数据文件夹。
//!
//! - 后台运行：关窗口只是隐藏窗口，下载、联机房主服务、本地模型照常运行；托盘菜单「退出」才真正退出（同样会清理子进程）。
//! - 开机自启：写当前用户的 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`（不需要管理员权限）。

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, State, Window, WindowEvent};

use crate::{AppState, Res, applog, data_dir, err};

static CLOSE_TO_TRAY: AtomicBool = AtomicBool::new(false);
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_NAME: &str = "ChronicleGM";

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// 启动时：读设置、建托盘；带 `--minimized` 启动（开机自启）且开了后台运行时，窗口先不显示。
pub fn init(app: &App) {
    let on = app.state::<AppState>().lib().ok().and_then(|l| l.setting("ui.close_to_tray").ok().flatten()).is_some_and(|v| v == "1");
    CLOSE_TO_TRAY.store(on, Ordering::Relaxed);
    if let Err(e) = build_tray(app) {
        applog::warn(&format!("系统托盘创建失败：{e}"));
    }
    if on && std::env::args().any(|a| a == "--minimized")
        && let Some(w) = app.get_webview_window("main")
    {
        let _ = w.hide();
    }
}

fn build_tray(app: &App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出 Chronicle GM", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let mut b = TrayIconBuilder::with_id("main").tooltip("Chronicle GM").menu(&menu).show_menu_on_left_click(false);
    if let Some(icon) = app.default_window_icon() {
        b = b.icon(icon.clone());
    }
    b.on_menu_event(|app, e| match e.id.as_ref() {
        "show" => show_main(app),
        "quit" => app.exit(0),
        _ => {}
    })
    .on_tray_icon_event(|tray, e| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            show_main(tray.app_handle());
        }
    })
    .build(app)?;
    Ok(())
}

/// 关窗口：开了后台运行就只隐藏窗口。
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event
        && window.label() == "main"
        && CLOSE_TO_TRAY.load(Ordering::Relaxed)
    {
        api.prevent_close();
        let _ = window.hide();
    }
}

#[derive(Serialize)]
pub struct SysSettings {
    close_to_tray: bool,
    autostart: bool,
    start_minimized: bool,
    data_dir: String,
}

fn hidden_cmd(exe: &str) -> std::process::Command {
    let mut c = std::process::Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}

/// 读开机自启项：（有没有，是否带 --minimized）。
fn read_autostart() -> (bool, bool) {
    if !cfg!(windows) {
        return (false, false);
    }
    match hidden_cmd("reg").args(["query", RUN_KEY, "/v", RUN_NAME]).output() {
        Ok(o) if o.status.success() => (true, String::from_utf8_lossy(&o.stdout).contains("--minimized")),
        _ => (false, false),
    }
}

#[tauri::command]
pub fn sys_settings() -> SysSettings {
    let (autostart, start_minimized) = read_autostart();
    SysSettings { close_to_tray: CLOSE_TO_TRAY.load(Ordering::Relaxed), autostart, start_minimized, data_dir: data_dir().display().to_string() }
}

#[tauri::command]
pub fn sys_set_close_to_tray(state: State<AppState>, on: bool) -> Res<()> {
    state.lib()?.set_setting("ui.close_to_tray", if on { "1" } else { "0" }).map_err(err)?;
    CLOSE_TO_TRAY.store(on, Ordering::Relaxed);
    Ok(())
}

/// 开机自启（Windows）：写 / 删当前用户的启动项。`minimized`：启动后直接在后台（需要同时开着后台运行）。
#[tauri::command]
pub fn sys_set_autostart(on: bool, minimized: bool) -> Res<()> {
    if !cfg!(windows) {
        return Err("开机自启目前只支持 Windows".into());
    }
    let out = if on {
        let exe = std::env::current_exe().map_err(err)?;
        let cmd = format!("\"{}\"{}", exe.display(), if minimized { " --minimized" } else { "" });
        hidden_cmd("reg").args(["add", RUN_KEY, "/v", RUN_NAME, "/t", "REG_SZ", "/d", &cmd, "/f"]).output()
    } else {
        if !read_autostart().0 {
            return Ok(());
        }
        hidden_cmd("reg").args(["delete", RUN_KEY, "/v", RUN_NAME, "/f"]).output()
    }
    .map_err(err)?;
    if !out.status.success() {
        return Err(format!("修改开机启动项失败：{}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    applog::info(&format!("开机自启：{}{}", if on { "开" } else { "关" }, if on && minimized { "（启动后在后台）" } else { "" }));
    Ok(())
}

/// 用资源管理器打开数据文件夹（只允许这几个固定位置）。
#[tauri::command]
pub fn sys_open_folder(kind: String) -> Res<()> {
    let dir = match kind.as_str() {
        "data" => data_dir(),
        "logs" => data_dir().join("logs"),
        "prompts" => crate::prompts::prepare_dir()?,
        "models" => data_dir().join("models"),
        _ => return Err("未知的文件夹".into()),
    };
    std::fs::create_dir_all(&dir).map_err(err)?;
    let opener = if cfg!(windows) { "explorer" } else if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    std::process::Command::new(opener).arg(&dir).spawn().map_err(err)?;
    Ok(())
}

/// 真正退出（界面上的「退出软件」；开着后台运行时关窗口不会退出）。
#[tauri::command]
pub fn sys_quit(app: AppHandle) {
    app.exit(0);
}

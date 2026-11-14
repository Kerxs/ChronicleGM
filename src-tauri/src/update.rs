//! 自动更新：从 GitHub Releases 取 `latest.json`，有新版本时下载安装包、校验签名（minisign，公钥在 tauri.conf.json），
//! 然后运行安装程序（NSIS，passive 模式）并重启。
//!
//! - 只检查和下载；安装要用户在界面上点「立即更新」。
//! - 下载的安装包必须通过签名校验才会运行（由 tauri-plugin-updater 完成），签名私钥不在仓库里。
//! - 测试时可以用环境变量 `CHRONICLE_UPDATE_ENDPOINT` 换成别的地址（例如本机的模拟发布）。
//! - 用户数据（`%LOCALAPPDATA%\ChronicleGM`）不在安装目录里，更新不会动它。

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use super::{AppState, Res, applog};

#[derive(Default)]
pub struct Pending(pub Mutex<Option<Update>>);

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub current: String,
    pub version: String,
    pub date: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
}

fn dry_run() -> bool {
    std::env::var("CHRONICLE_UPDATE_DRY_RUN").ok().as_deref() == Some("1")
}

fn updater(app: &AppHandle) -> Res<tauri_plugin_updater::Updater> {
    let mut b = app.updater_builder();
    if dry_run() {
        // 测试：把已发布的版本当作新版本（验证真实的下载与签名校验），可以换一个公钥做反向测试。只下载，不安装。
        b = b.version_comparator(|_, _| true);
        if let Ok(k) = std::env::var("CHRONICLE_UPDATE_PUBKEY")
            && !k.trim().is_empty()
        {
            b = b.pubkey(k.trim());
        }
    }
    if let Ok(url) = std::env::var("CHRONICLE_UPDATE_ENDPOINT")
        && !url.trim().is_empty()
    {
        let u = url.trim().parse().map_err(|e| format!("更新地址无效：{e}"))?;
        b = b.endpoints(vec![u]).map_err(|e| e.to_string())?;
    }
    b.build().map_err(|e| e.to_string())
}

/// 检查更新：有新版本返回版本信息，已是最新返回 None。
#[tauri::command]
pub async fn update_check(app: AppHandle) -> Res<Option<UpdateInfo>> {
    let current = app.package_info().version.to_string();
    let found = updater(&app)?.check().await.map_err(|e| format!("检查更新失败：{e}"))?;
    let pending = app.state::<Pending>();
    let mut slot = pending.0.lock().map_err(|e| e.to_string())?;
    match found {
        Some(u) => {
            let info = UpdateInfo { current, version: u.version.clone(), date: u.date.map(|d| d.to_string()), notes: u.body.clone() };
            applog::info(&format!("发现新版本 {}（当前 {}）", info.version, info.current));
            *slot = Some(u);
            Ok(Some(info))
        }
        None => {
            *slot = None;
            Ok(None)
        }
    }
}

/// 下载并安装上一次检查到的新版本（进度通过 `update-progress` 事件发给界面），然后重启。
/// 安装前先停止托管的模型进程。
#[tauri::command]
pub async fn update_install(app: AppHandle) -> Res<()> {
    if dry_run() {
        return Err("测试模式（CHRONICLE_UPDATE_DRY_RUN）只下载与校验，不安装".into());
    }
    let update = app.state::<Pending>().0.lock().map_err(|e| e.to_string())?.take().ok_or("没有待安装的更新，请先检查更新")?;
    let version = update.version.clone();
    let mut downloaded: u64 = 0;
    let emitter = app.clone();
    let bytes = update
        .download(
            move |chunk, total| {
                downloaded += chunk as u64;
                let _ = emitter.emit("update-progress", Progress { downloaded, total });
            },
            || {},
        )
        .await
        .map_err(|e| format!("下载或校验签名失败：{e}"))?;
    applog::info(&format!("新版本 {version} 已下载并通过签名校验（{} KB），开始安装", bytes.len() / 1024));
    app.state::<AppState>().models.stop_all();
    update.install(bytes).map_err(|e| format!("安装失败：{e}"))?;
    app.restart();
}

/// 只下载并校验签名，不安装（测试用：环境变量 `CHRONICLE_UPDATE_DRY_RUN=1` 时 `update_install` 走这里）。
#[tauri::command]
pub async fn update_download_only(app: AppHandle) -> Res<u64> {
    if !dry_run() {
        return Err("只在测试时可用".into());
    }
    let update = app.state::<Pending>().0.lock().map_err(|e| e.to_string())?.take().ok_or("没有待安装的更新")?;
    let bytes = update.download(|_, _| {}, || {}).await.map_err(|e| format!("下载或校验签名失败：{e}"))?;
    Ok(bytes.len() as u64)
}

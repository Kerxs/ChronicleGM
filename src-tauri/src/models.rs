//! 内置 llama.cpp 与模型管理的命令（薄层：逻辑在 chronicle-models）。
//!
//! - 运行环境：检查更新、安装 / 更新 / 重装（后台线程，进度事件 `llm-runtime`）、回滚、损坏检测、自动检查。
//! - 模型仓库：Hugging Face / 魔搭的搜索与详情；访问令牌存在系统钥匙串（`repo-token-<来源>`），不写日志。
//! - 下载管理：任务持久化、断点续传，完成后自动加入模型库。
//! - 模型库：添加 / 删除（删文件由界面二次确认）/ 默认模型 / mmproj / 保存位置 / 完整性检查。
//! - 推理服务：按自动参数生成托管配置 `builtin-llm`，交给已有的 ModelManager 启停（只监听 127.0.0.1），
//!   并登记为模型服务"内置模型"；游戏照常经调度中心调用，用到时按需启动。
//!
//! 所有耗时操作都在后台线程，不占界面线程。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use chronicle_llm::CancelToken;
use chronicle_llm::secrets::SecretStore;
use chronicle_models::download::{DownloadManager, NewTask, Task, TaskFile};
use chronicle_models::hw::Hardware;
use chronicle_models::library::{Integrity, Library, LibraryData, ModelEntry};
use chronicle_models::repo::hf::HuggingFace;
use chronicle_models::repo::modelscope::ModelScope;
use chronicle_models::repo::{ModelDetail, ModelSummary, RepoAdapter};
use chronicle_models::runtime_mgr::{self, Health, RuntimeMgr, RuntimeState};
use chronicle_models::tune::{self, ServerStats, TuneInput, Tuned};
use chronicle_runtime::{ModelProfile, ModelRole, ProcStatus};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{AppState, Res, applog, data_dir, err};

pub const BUILTIN_ID: &str = "builtin-llm";
pub const BUILTIN_PROVIDER: &str = "builtin-llama";

/// 内置模型的托管配置（交给 ModelManager；perf::sync_profiles 合并进去）。
pub fn builtin_profile() -> Option<ModelProfile> {
    BUILTIN.get().and_then(|m| m.lock().ok().and_then(|g| g.clone()))
}
static BUILTIN: OnceLock<Mutex<Option<ModelProfile>>> = OnceLock::new();

fn set_builtin(p: Option<ModelProfile>) {
    let m = BUILTIN.get_or_init(|| Mutex::new(None));
    if let Ok(mut g) = m.lock() {
        *g = p;
    }
}

/// 推理设置（`llama/serve.json`）：用哪个模型、端口、手动覆盖的参数。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServeConfig {
    pub model_id: Option<String>,
    pub port: u16,
    #[serde(default)]
    pub ctx: u64,
    #[serde(default)]
    pub gpu_layers: Option<u32>,
    #[serde(default)]
    pub threads: Option<usize>,
    #[serde(default)]
    pub cache_type: Option<String>,
    #[serde(default)]
    pub flash_attn: Option<bool>,
    #[serde(default = "reserve")]
    pub vram_reserve_mb: u64,
}
fn reserve() -> u64 {
    1024
}

pub struct ModelsState {
    root: PathBuf,
    rt: RuntimeMgr,
    pub dl: Arc<DownloadManager>,
    lib: Arc<Mutex<Library>>,
    hf: HuggingFace,
    ms: ModelScope,
    install_stop: Mutex<Option<Arc<AtomicBool>>>,
    install_state: Arc<Mutex<serde_json::Value>>,
    hw: Mutex<Option<Hardware>>,
}

fn log() -> chronicle_models::LogFn {
    Arc::new(|s: &str| applog::info(&format!("[模型管理] {s}")))
}

fn token_key(source: &str) -> String {
    format!("repo-token-{source}")
}

impl ModelsState {
    pub fn new(secrets: Arc<dyn SecretStore>) -> ModelsState {
        let root = data_dir().join("llama");
        let lib = Library::open(&root, &data_dir().join("models"), log());
        let models_dir = lib.models_dir();
        let lib = Arc::new(Mutex::new(lib));
        let ms = ModelScope::default();
        let ms_base = ms.base.clone();
        let tok = move |source: &str| -> (Option<String>, Option<String>) {
            let t = secrets.get(&token_key(source)).ok().flatten();
            match source {
                // 魔搭私有仓库：令牌换 Cookie。
                "modelscope" => (None, t.and_then(|t| ModelScope { base: ms_base.clone() }.login(&t).ok())),
                _ => (t, None),
            }
        };
        let dl = DownloadManager::new(&root, &models_dir, log(), Arc::new(tok));
        let lib2 = lib.clone();
        dl.set_on_done(Arc::new(move |t: &Task| {
            if let Ok(mut l) = lib2.lock()
                && let Err(e) = l.add_from_task(t)
            {
                applog::warn(&format!("[模型管理] 下载完成但加入模型库失败：{e}"));
            }
        }));
        dl.start();
        ModelsState { rt: RuntimeMgr::new(&root, log()), root, dl, lib, hf: HuggingFace::default(), ms, install_stop: Mutex::new(None), install_state: Arc::default(), hw: Mutex::new(None) }
    }

    fn serve_cfg(&self) -> ServeConfig {
        std::fs::read(self.root.join("serve.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(ServeConfig { port: 0, vram_reserve_mb: 1024, ..Default::default() })
    }
    fn save_serve(&self, c: &ServeConfig) -> Res<()> {
        std::fs::create_dir_all(&self.root).map_err(err)?;
        std::fs::write(self.root.join("serve.json"), serde_json::to_vec_pretty(c).map_err(err)?).map_err(err)
    }

    fn hardware(&self) -> Hardware {
        let mut g = self.hw.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            *g = Some(chronicle_models::hw::detect());
        }
        g.clone().unwrap_or_default()
    }

    fn repo(&self, source: &str) -> Res<&dyn RepoAdapter> {
        match source {
            "hf" => Ok(&self.hf),
            "modelscope" => Ok(&self.ms),
            _ => Err("未知的模型仓库".into()),
        }
    }

    /// 启动时：恢复内置模型的托管配置（不启动进程，用到时按需启动）。
    pub fn restore(&self) {
        let c = self.serve_cfg();
        if let Some(id) = c.model_id.clone()
            && c.port > 0
            && let Ok(t) = self.plan(&id, &c, false)
        {
            set_builtin(Some(self.profile(&t, c.port)));
        }
    }

    fn plan(&self, model_id: &str, c: &ServeConfig, fresh_hw: bool) -> Res<Tuned> {
        let m = self.lib.lock().map_err(err)?.get(model_id).cloned().ok_or("模型不在模型库里")?;
        let info = chronicle_llm::gguf::read_gguf(Path::new(&m.path)).map_err(|e| format!("读不了模型文件：{e}"))?;
        let proj = m.mmproj.as_deref().and_then(|p| std::fs::metadata(p).ok()).map(|x| x.len()).unwrap_or(0);
        if fresh_hw {
            *self.hw.lock().map_err(err)? = None;
        }
        let input = TuneInput { model: m.path.clone(), mmproj: m.mmproj.clone(), port: c.port, ctx: c.ctx, vram_reserve_mb: c.vram_reserve_mb, gpu_layers: c.gpu_layers, threads: c.threads, cache_type: c.cache_type.clone(), flash_attn: c.flash_attn, batch: None };
        Ok(tune::tune(&input, &info, m.size.max(info.file_size), proj, &self.hardware()))
    }

    fn profile(&self, t: &Tuned, port: u16) -> ModelProfile {
        let exe = self.rt.current_exe().unwrap_or_default();
        ModelProfile {
            id: BUILTIN_ID.into(),
            name: "内置模型（llama.cpp）".into(),
            role: ModelRole::Llm,
            cwd: exe.parent().map(|p| p.display().to_string()).unwrap_or_default(),
            exe: exe.display().to_string(),
            args: t.args.clone(),
            port,
            health_path: "/health".into(),
            start_timeout_secs: 600,
            auto_restart: true,
            idle_unload_secs: None,
            exclusive_group: "llm".into(),
            gpu_reserve_mb: 0,
        }
    }
}

/// 找一个本机空闲端口（从 18080 开始）。
fn free_port() -> u16 {
    (18080..18180).find(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok()).unwrap_or(18080)
}

// ===================== 运行环境 =====================

#[tauri::command]
pub fn llm_hw(ms: State<ModelsState>, refresh: bool) -> Hardware {
    if refresh && let Ok(mut g) = ms.hw.lock() {
        *g = None;
    }
    ms.hardware()
}

#[derive(Serialize)]
pub struct RuntimeView {
    state: RuntimeState,
    health: Health,
    install: serde_json::Value,
    root: String,
}

#[tauri::command]
pub async fn llm_runtime_status(app: AppHandle, deep: bool) -> Res<RuntimeView> {
    tauri::async_runtime::spawn_blocking(move || {
        let ms = app.state::<ModelsState>();
        let health = if deep { ms.rt.health(Some(&runtime_mgr::verify_exe)) } else { ms.rt.health(None) };
        Ok(RuntimeView { state: ms.rt.state(), health, install: ms.install_state.lock().map(|v| v.clone()).unwrap_or_default(), root: ms.rt.runtimes_dir().display().to_string() })
    })
    .await
    .map_err(err)?
}

#[derive(Serialize)]
pub struct UpdateView {
    tag: String,
    published_at: String,
    newer: bool,
    plan: Option<runtime_mgr::Plan>,
}

#[tauri::command]
pub async fn llm_runtime_check(app: AppHandle) -> Res<UpdateView> {
    tauri::async_runtime::spawn_blocking(move || {
        let ms = app.state::<ModelsState>();
        let (rel, newer) = ms.rt.check_update().map_err(|e| format!("检查 llama.cpp 更新失败：{e}"))?;
        let prefer = ms.rt.state().prefer;
        let plan = runtime_mgr::choose(&rel, &ms.hardware(), prefer.as_deref());
        Ok(UpdateView { tag: rel.tag, published_at: rel.published_at, newer, plan })
    })
    .await
    .map_err(err)?
}

/// 安装最新版本（首次安装、更新、重装都用它）。后台进行，进度事件 `llm-runtime`。
#[tauri::command]
pub fn llm_runtime_install(app: AppHandle, ms: State<ModelsState>, prefer: Option<String>) -> Res<()> {
    let mut g = ms.install_stop.lock().map_err(err)?;
    if g.is_some() {
        return Err("正在安装，请等它完成".into());
    }
    let stop = Arc::new(AtomicBool::new(false));
    *g = Some(stop.clone());
    drop(g);
    let state = ms.install_state.clone();
    std::thread::spawn(move || {
        let ms = app.state::<ModelsState>();
        let emit = |v: serde_json::Value| {
            if let Ok(mut s) = state.lock() {
                *s = v.clone();
            }
            let _ = app.emit("llm-runtime", v);
        };
        emit(serde_json::json!({ "stage": "check" }));
        let res = (|| -> Result<runtime_mgr::Installed, String> {
            let rel = ms.rt.releases(1).map_err(|e| format!("读取 llama.cpp 发布列表失败：{e}"))?.into_iter().next().ok_or("没有找到正式版本")?;
            if prefer.is_some() {
                let mut s = ms.rt.state();
                s.prefer = prefer.clone().filter(|p| p != "auto");
                ms.rt.save_state(&s)?;
            }
            let plan = runtime_mgr::choose(&rel, &ms.hardware(), ms.rt.state().prefer.as_deref()).ok_or("这个版本没有适合本机的构建")?;
            emit(serde_json::json!({ "stage": "download", "tag": plan.tag, "backend": plan.backend.label(), "reason": plan.reason, "total": plan.total_size() }));
            ms.rt.install(&plan, &stop, |file, p| emit(serde_json::json!({ "stage": "download", "tag": plan.tag, "file": file, "downloaded": p.downloaded, "total": p.total, "speed": p.speed, "eta": p.eta_secs() })), &runtime_mgr::verify_exe)
        })();
        match res {
            Ok(i) => {
                emit(serde_json::json!({ "stage": "done", "tag": i.tag, "version": i.version_line }));
                // 内置模型的托管配置指向新版本。
                ms.restore();
                crate::perf::sync_profiles(&app.state::<AppState>());
            }
            Err(e) => emit(serde_json::json!({ "stage": "error", "error": e })),
        }
        if let Ok(mut g) = ms.install_stop.lock() {
            *g = None;
        }
    });
    Ok(())
}

#[tauri::command]
pub fn llm_runtime_cancel(ms: State<ModelsState>) -> Res<()> {
    if let Some(s) = ms.install_stop.lock().map_err(err)?.as_ref() {
        s.store(true, Ordering::Relaxed);
    }
    Ok(())
}

#[tauri::command]
pub fn llm_runtime_rollback(app: AppHandle, ms: State<ModelsState>) -> Res<runtime_mgr::Installed> {
    let i = ms.rt.rollback()?;
    ms.restore();
    crate::perf::sync_profiles(&app.state::<AppState>());
    Ok(i)
}

#[tauri::command]
pub fn llm_runtime_settings(ms: State<ModelsState>, auto_check: bool, prefer: Option<String>) -> Res<()> {
    let mut s = ms.rt.state();
    s.auto_check = auto_check;
    s.prefer = prefer.filter(|p| !p.is_empty() && p != "auto");
    ms.rt.save_state(&s)
}

/// 启动后：到时间就在后台检查一次 llama.cpp 更新，有新版本发 `llm-update` 事件提示（不自动安装）。
pub fn auto_check(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let ms = app.state::<ModelsState>();
        if ms.rt.state().current.is_none() || !ms.rt.due_for_check() {
            return;
        }
        match ms.rt.check_update() {
            Ok((rel, true)) => {
                let _ = app.emit("llm-update", rel.tag);
            }
            Ok(_) => {}
            Err(e) => applog::warn(&format!("[模型管理] 自动检查 llama.cpp 更新失败：{e}")),
        }
    });
}

// ===================== 模型仓库 =====================

fn token_of(app: &AppHandle, source: &str) -> Option<String> {
    app.state::<AppState>().secrets.get(&token_key(source)).ok().flatten()
}

#[tauri::command]
pub async fn llm_repo_search(app: AppHandle, source: String, query: String) -> Res<Vec<ModelSummary>> {
    tauri::async_runtime::spawn_blocking(move || {
        let ms = app.state::<ModelsState>();
        let tok = token_of(&app, &source);
        ms.repo(&source)?.search(&query, 30, tok.as_deref()).map_err(|e| e.to_string())
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub async fn llm_repo_details(app: AppHandle, source: String, id: String) -> Res<ModelDetail> {
    tauri::async_runtime::spawn_blocking(move || {
        let ms = app.state::<ModelsState>();
        let tok = token_of(&app, &source);
        ms.repo(&source)?.details(&id, tok.as_deref()).map_err(|e| e.to_string())
    })
    .await
    .map_err(err)?
}

/// 设置 / 清除仓库访问令牌（存系统钥匙串）。返回是否已设置。
#[tauri::command]
pub fn llm_repo_token(state: State<AppState>, source: String, token: Option<String>) -> Res<bool> {
    match token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        Some(t) => {
            state.secrets.set(&token_key(&source), &t)?;
            applog::info(&format!("[模型管理] 已设置 {source} 访问令牌"));
            Ok(true)
        }
        None => {
            if token_of_state(&state, &source) {
                state.secrets.delete(&token_key(&source))?;
            }
            Ok(false)
        }
    }
}

fn token_of_state(state: &AppState, source: &str) -> bool {
    state.secrets.get(&token_key(source)).ok().flatten().is_some()
}

#[tauri::command]
pub fn llm_repo_token_status(state: State<AppState>) -> serde_json::Value {
    serde_json::json!({ "hf": token_of_state(&state, "hf"), "modelscope": token_of_state(&state, "modelscope") })
}

// ===================== 下载 =====================

#[tauri::command]
pub fn llm_download_add(ms: State<ModelsState>, source: String, repo: String, files: Vec<TaskFile>) -> Res<String> {
    ms.dl.add(NewTask { source, repo, files })
}

/// 一键下载默认模型（Rocinante-X-12B Q4_K_M），先查仓库拿到大小和校验值。
#[tauri::command]
pub async fn llm_download_default(app: AppHandle) -> Res<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let ms = app.state::<ModelsState>();
        let d = ms.hf.details(chronicle_models::DEFAULT_MODEL_REPO, token_of(&app, "hf").as_deref()).map_err(|e| format!("查询默认模型失败：{e}"))?;
        let f = d.files.iter().find(|f| f.path == chronicle_models::DEFAULT_MODEL_FILE).ok_or("仓库里没有找到默认模型文件")?;
        ms.dl.add(NewTask { source: "hf".into(), repo: chronicle_models::DEFAULT_MODEL_REPO.into(), files: vec![TaskFile { path: f.path.clone(), url: f.url.clone(), size: f.size, sha256: f.sha256.clone(), done: false, mmproj: false }] })
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub fn llm_downloads(ms: State<ModelsState>) -> Vec<Task> {
    ms.dl.list()
}

#[tauri::command]
pub fn llm_download_op(ms: State<ModelsState>, id: String, op: String) -> Res<()> {
    match op.as_str() {
        "pause" => ms.dl.pause(&id),
        "resume" => ms.dl.resume(&id),
        "cancel" => ms.dl.cancel(&id),
        "remove" => ms.dl.remove(&id),
        _ => Err("未知操作".into()),
    }
}

#[tauri::command]
pub fn llm_download_settings(ms: State<ModelsState>, max_concurrent: usize) {
    ms.dl.max_concurrent.store(max_concurrent.clamp(1, 4), Ordering::Relaxed);
}

// ===================== 模型库 =====================

#[derive(Serialize)]
pub struct LibraryView {
    data: LibraryData,
    in_use: Option<String>,
}

fn in_use(app: &AppHandle) -> Option<String> {
    let st = app.state::<AppState>().models.status();
    let running = st.iter().find(|p| p.profile.id == BUILTIN_ID).is_some_and(|p| p.pid.is_some());
    if !running {
        return None;
    }
    let ms = app.state::<ModelsState>();
    let id = ms.serve_cfg().model_id?;
    ms.lib.lock().ok()?.get(&id).map(|m| m.path.clone())
}

#[tauri::command]
pub fn llm_library(app: AppHandle, ms: State<ModelsState>) -> Res<LibraryView> {
    Ok(LibraryView { data: ms.lib.lock().map_err(err)?.data.clone(), in_use: in_use(&app) })
}

#[tauri::command]
pub async fn llm_library_add(app: AppHandle, path: String, mmproj: Option<String>) -> Res<ModelEntry> {
    tauri::async_runtime::spawn_blocking(move || app.state::<ModelsState>().lib.lock().map_err(err)?.add_file(Path::new(&path), mmproj.as_deref().map(Path::new))).await.map_err(err)?
}

/// 删除记录或文件。`delete_files` 必须由界面二次确认后才传 true。
#[tauri::command]
pub fn llm_library_remove(app: AppHandle, ms: State<ModelsState>, id: String, delete_files: bool) -> Res<Vec<String>> {
    let busy: Vec<String> = in_use(&app).into_iter().collect();
    let r = ms.lib.lock().map_err(err)?.remove(&id, delete_files, &busy)?;
    let mut c = ms.serve_cfg();
    if c.model_id.as_deref() == Some(&id) {
        c.model_id = None;
        ms.save_serve(&c)?;
        set_builtin(None);
        crate::perf::sync_profiles(&app.state::<AppState>());
    }
    Ok(r)
}

#[tauri::command]
pub fn llm_library_default(ms: State<ModelsState>, id: String) -> Res<()> {
    ms.lib.lock().map_err(err)?.set_default(&id)
}

#[tauri::command]
pub fn llm_library_mmproj(ms: State<ModelsState>, id: String, path: Option<String>) -> Res<()> {
    ms.lib.lock().map_err(err)?.link_mmproj(&id, path.as_deref().map(Path::new))
}

#[tauri::command]
pub async fn llm_library_info(app: AppHandle, id: String) -> Res<(chronicle_llm::gguf::GgufInfo, Integrity)> {
    tauri::async_runtime::spawn_blocking(move || {
        let ms = app.state::<ModelsState>();
        let l = ms.lib.lock().map_err(err)?;
        Ok((l.info(&id).unwrap_or_default(), l.check(&id)))
    })
    .await
    .map_err(err)?
}

/// 修改模型保存位置（有下载在进行时不允许）。
#[tauri::command]
pub async fn llm_library_move(app: AppHandle, dir: String) -> Res<usize> {
    tauri::async_runtime::spawn_blocking(move || {
        let ms = app.state::<ModelsState>();
        if ms.dl.list().iter().any(|t| matches!(t.state, chronicle_models::download::TaskState::Running | chronicle_models::download::TaskState::Queued | chronicle_models::download::TaskState::Retrying { .. })) {
            return Err("有下载正在进行，先暂停或等它完成".to_string());
        }
        if in_use(&app).is_some() {
            return Err("内置模型正在运行，先停止再移动".to_string());
        }
        let n = ms.lib.lock().map_err(err)?.move_dir(Path::new(dir.trim()))?;
        ms.dl.set_models_dir(Path::new(dir.trim()));
        ms.restore();
        crate::perf::sync_profiles(&app.state::<AppState>());
        Ok(n)
    })
    .await
    .map_err(err)?
}

// ===================== 推理服务 =====================

#[derive(Deserialize, Default)]
pub struct ServeOverrides {
    #[serde(default)]
    ctx: u64,
    #[serde(default)]
    gpu_layers: Option<u32>,
    #[serde(default)]
    threads: Option<usize>,
    #[serde(default)]
    cache_type: Option<String>,
    #[serde(default)]
    flash_attn: Option<bool>,
    #[serde(default)]
    vram_reserve_mb: Option<u64>,
}

fn merged(ms: &ModelsState, o: &ServeOverrides, model_id: &str) -> ServeConfig {
    let old = ms.serve_cfg();
    ServeConfig {
        model_id: Some(model_id.into()),
        port: if old.port > 0 { old.port } else { free_port() },
        ctx: o.ctx,
        gpu_layers: o.gpu_layers,
        threads: o.threads,
        cache_type: o.cache_type.clone().filter(|s| !s.is_empty() && s != "auto"),
        flash_attn: o.flash_attn,
        vram_reserve_mb: o.vram_reserve_mb.unwrap_or(old.vram_reserve_mb.max(512)),
    }
}

/// 预览自动参数（不启动）。
#[tauri::command]
pub async fn llm_tune(app: AppHandle, model_id: String, overrides: Option<ServeOverrides>) -> Res<Tuned> {
    tauri::async_runtime::spawn_blocking(move || {
        let ms = app.state::<ModelsState>();
        let c = merged(&ms, &overrides.unwrap_or_default(), &model_id);
        ms.plan(&model_id, &c, true)
    })
    .await
    .map_err(err)?
}

/// 启动（或切换到）某个模型：检查运行环境和兼容性 → 生成托管配置 → 启动并等健康检查 → 登记成模型服务。
#[tauri::command]
pub async fn llm_serve_start(app: AppHandle, model_id: String, overrides: Option<ServeOverrides>) -> Res<Tuned> {
    tauri::async_runtime::spawn_blocking(move || {
        let ms = app.state::<ModelsState>();
        let state = app.state::<AppState>();
        match ms.rt.health(None) {
            Health::Ok { .. } => {}
            Health::NotInstalled => return Err("还没有安装 llama.cpp 运行环境，先到「运行环境」页安装".to_string()),
            Health::Broken { reason } => return Err(format!("llama.cpp 运行环境损坏：{reason}。请到「运行环境」页重新安装")),
        }
        let c = merged(&ms, &overrides.unwrap_or_default(), &model_id);
        let t = ms.plan(&model_id, &c, true)?;
        if !t.ok {
            return Err(format!("不能启动：{}", t.problems.join("；")));
        }
        // 先停掉旧的（切换模型），再换上新配置。
        let _ = state.models.stop(BUILTIN_ID);
        ms.save_serve(&c)?;
        set_builtin(Some(ms.profile(&t, c.port)));
        crate::perf::sync_profiles(&state);
        let m = ms.lib.lock().map_err(err)?.get(&model_id).cloned().ok_or("模型不存在")?;
        applog::info(&format!("[模型管理] 启动内置模型 {}（端口 {}，GPU 层 {}/{}，上下文 {}，KV {}）", m.name, c.port, t.gpu_layers, t.total_layers, t.ctx, t.cache_type));
        state.models.start(BUILTIN_ID, &CancelToken::new())?;
        register_provider(&state, &m, c.port, t.ctx)?;
        Ok(t)
    })
    .await
    .map_err(err)?
}

/// 登记 / 更新"内置模型"服务；还没有默认路由时设为默认（以内置为主）。
fn register_provider(state: &AppState, m: &ModelEntry, port: u16, ctx: u64) -> Res<()> {
    let lib = state.lib()?;
    let cfg = chronicle_llm::ProviderConfig {
        id: BUILTIN_PROVIDER.into(),
        name: "内置模型".into(),
        base_url: format!("http://127.0.0.1:{port}/v1"),
        model: m.name.clone(),
        server_kind: Some(chronicle_llm::types::ServerKind::LlamaServer),
        needs_key: false,
        timeout_secs: 600,
        capabilities: None,
        allow_campaign_content: true,
        thinking: Default::default(),
        service: Default::default(),
        media: Default::default(),
    };
    lib.save_provider(BUILTIN_PROVIDER, &serde_json::to_value(&cfg).map_err(err)?).map_err(err)?;
    if lib.route_for("default").map_err(err)?.is_none() {
        lib.set_route("default", Some(BUILTIN_PROVIDER)).map_err(err)?;
    }
    let _ = ctx;
    Ok(())
}

#[tauri::command]
pub async fn llm_serve_stop(app: AppHandle) -> Res<()> {
    tauri::async_runtime::spawn_blocking(move || app.state::<AppState>().models.stop(BUILTIN_ID)).await.map_err(err)?
}

#[derive(Serialize)]
pub struct ServeView {
    config: ServeConfig,
    model: Option<ModelEntry>,
    process: Option<ProcStatus>,
    runtime: Option<runtime_mgr::Installed>,
    stats: ServerStats,
    /// 进程的 CPU / 内存与显卡（来自资源监控）。
    resources: Option<chronicle_runtime::ResourceSample>,
    tuned: Option<Tuned>,
}

#[tauri::command]
pub async fn llm_serve_status(app: AppHandle) -> Res<ServeView> {
    tauri::async_runtime::spawn_blocking(move || {
        let ms = app.state::<ModelsState>();
        let state = app.state::<AppState>();
        let config = ms.serve_cfg();
        let model = config.model_id.as_deref().and_then(|id| ms.lib.lock().ok()?.get(id).cloned());
        let process = state.models.status().into_iter().find(|p| p.profile.id == BUILTIN_ID);
        let running = process.as_ref().is_some_and(|p| p.pid.is_some());
        let stats = if running && config.port > 0 { tune::server_stats(config.port) } else { ServerStats::default() };
        let tuned = config.model_id.as_deref().and_then(|id| ms.plan(id, &config, false).ok());
        Ok(ServeView { runtime: ms.rt.state().current, config, model, process, stats, resources: state.monitor.latest(), tuned })
    })
    .await
    .map_err(err)?
}

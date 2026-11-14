//! 性能中心与托管模式的命令（薄层：逻辑在 chronicle-runtime）。

use std::time::Instant;

use chronicle_llm::CancelToken;
use chronicle_runtime::{MemEstimate, ModelProfile, PerfMode, Priority, ProcStatus, ResourceSample, SchedSnapshot, estimate_llama};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use super::{AppState, Res, err, lock};

/// 界面上报的图谱渲染耗时（由前端在渲染完成后测得）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderStat {
    pub view: String,
    pub nodes: usize,
    pub edges: usize,
    pub ms: f64,
    pub at: i64,
}

#[derive(Serialize)]
pub struct PerfSnapshot {
    sched: SchedSnapshot,
    resources: Option<ResourceSample>,
    /// 最近的资源样本（画趋势用，旧的在前）。
    history: Vec<ResourceSample>,
    models: Vec<ProcStatus>,
    render: Vec<RenderStat>,
    /// 语音合成的队列与缓存（语音识别尚未接入，如实显示）。
    voice_note: String,
}

#[tauri::command]
pub fn perf_snapshot(state: State<AppState>) -> Res<PerfSnapshot> {
    Ok(PerfSnapshot {
        sched: state.sched.snapshot(),
        resources: state.monitor.latest(),
        history: state.monitor.history(120),
        models: state.models.status(),
        render: state.render_stats.lock().map_err(err)?.iter().rev().take(30).cloned().collect(),
        voice_note: {
            let snap = state.sched.snapshot();
            let queued = snap.active.iter().filter(|t| t.kind == "voice").count();
            let (files, bytes) = crate::voice::cache_stats(&crate::voice_cache_dir());
            format!("语音合成（P2）：排队与进行中 {queued} 条（排队上限 16，同样的内容只合成一次）；缓存 {files} 条，{:.1} MB（上限 500 MB）。语音识别尚未接入。", bytes as f64 / 1048576.0)
        },
    })
}

#[tauri::command]
pub fn set_perf_mode(state: State<AppState>, mode: String) -> Res<()> {
    let m = PerfMode::parse(&mode).ok_or("未知的性能模式")?;
    state.sched.set_mode(m);
    state.lib()?.set_setting("perf_mode", m.as_str()).map_err(err)
}

/// 整体暂停：该优先级及更低的任务不再启动（正在运行的请求不受影响）。None 表示恢复。
#[tauri::command]
pub fn pause_from(state: State<AppState>, priority: Option<Priority>) {
    state.sched.set_paused_from(priority);
}

#[tauri::command]
pub fn task_pause(state: State<AppState>, id: u64, paused: bool) -> bool {
    state.sched.pause_task(id, paused)
}

#[tauri::command]
pub fn task_cancel(state: State<AppState>, id: u64) -> bool {
    state.sched.cancel_task(id)
}

#[tauri::command]
pub fn report_render(state: State<AppState>, view: String, nodes: usize, edges: usize, ms: f64) -> Res<()> {
    let mut g = state.render_stats.lock().map_err(err)?;
    g.push_back(RenderStat { view, nodes, edges, ms, at: chronicle_core::util::now_ms() });
    while g.len() > 200 {
        g.pop_front();
    }
    Ok(())
}

// ===================== 托管模式 =====================

fn load_profiles(state: &AppState) -> Res<Vec<ModelProfile>> {
    state.lib()?.managed_models().map_err(err)?.into_iter().map(|v| serde_json::from_value(v).map_err(err)).collect()
}

/// 启动时与配置变更后：把资料库里的托管配置交给管理器。
pub fn sync_profiles(state: &AppState) {
    match load_profiles(state) {
        Ok(p) => state.models.set_profiles(p),
        Err(e) => crate::applog::warn(&format!("读取托管配置失败：{e}")),
    }
}

#[tauri::command]
pub fn list_managed_models(state: State<AppState>) -> Res<Vec<ProcStatus>> {
    Ok(state.models.status())
}

#[tauri::command]
pub fn save_managed_model(state: State<AppState>, profile: ModelProfile) -> Res<()> {
    if profile.name.trim().is_empty() || profile.exe.trim().is_empty() {
        return Err("名称和程序路径不能为空".into());
    }
    if profile.port == 0 {
        return Err("请填写服务端口".into());
    }
    let others = load_profiles(&state)?;
    if let Some(o) = others.iter().find(|o| o.id != profile.id && o.port == profile.port) {
        return Err(format!("端口 {} 已被「{}」使用", profile.port, o.name));
    }
    state.lib()?.save_managed_model(&profile.id, &serde_json::to_value(&profile).map_err(err)?).map_err(err)?;
    sync_profiles(&state);
    Ok(())
}

#[tauri::command]
pub fn delete_managed_model(state: State<AppState>, id: String) -> Res<()> {
    let _ = state.models.stop(&id);
    state.lib()?.delete_managed_model(&id).map_err(err)?;
    sync_profiles(&state);
    Ok(())
}

/// 启动 / 重启可能要几分钟（大模型加载），在后台线程等待。
#[tauri::command]
pub async fn model_start(app: AppHandle, id: String, restart: bool) -> Res<()> {
    let models = app.state::<AppState>().models.clone();
    tauri::async_runtime::spawn_blocking(move || if restart { models.restart(&id, &CancelToken::new()) } else { models.start(&id, &CancelToken::new()) })
        .await
        .map_err(err)?
}

#[tauri::command]
pub fn model_stop(state: State<AppState>, id: String) -> Res<()> {
    state.models.stop(&id)
}

/// 按启动参数估算显存 / 内存需求（读取 GGUF 元数据，只读）。
#[tauri::command]
pub fn estimate_model(args: Vec<String>) -> Option<MemEstimate> {
    estimate_llama(&args)
}

// ===================== 数据库性能 =====================

#[derive(Serialize)]
pub struct DbProbe {
    file_mb: f64,
    wal_mb: f64,
    nodes: i64,
    events: i64,
    /// 各项查询的实测耗时（毫秒）。
    timings: Vec<(String, f64)>,
}

#[tauri::command]
pub fn db_probe(state: State<AppState>, world_id: String) -> Res<DbProbe> {
    let w = state.world(&world_id)?;
    let w = lock(&w)?;
    let size = |p: std::path::PathBuf| std::fs::metadata(p).map(|m| m.len() as f64 / 1048576.0).unwrap_or(0.0);
    let path = w.path().to_path_buf();
    let mut timings = vec![];
    let mut time = |label: &str, f: &mut dyn FnMut() -> Result<(), String>| -> Res<()> {
        let t = Instant::now();
        f()?;
        timings.push((label.to_string(), t.elapsed().as_secs_f64() * 1000.0));
        Ok(())
    };
    let conn = w.conn();
    let mut nodes = 0i64;
    let mut events = 0i64;
    time("统计节点数", &mut || {
        nodes = conn.query_row("SELECT COUNT(*) FROM nodes WHERE deleted_at IS NULL", [], |r| r.get(0)).map_err(err)?;
        Ok(())
    })?;
    time("统计事件数", &mut || {
        events = conn.query_row("SELECT COUNT(*) FROM event_log", [], |r| r.get(0)).map_err(err)?;
        Ok(())
    })?;
    time("展开根节点", &mut || chronicle_core::nodes::children(conn, None, 0, 200).map(|_| ()).map_err(err))?;
    time("全文搜索", &mut || {
        chronicle_core::search::search_nodes(conn, "城市", &chronicle_core::search::SearchOptions { limit: 20, ..Default::default() }).map(|_| ()).map_err(err)
    })?;
    if let Some(c) = chronicle_core::campaign::list_campaigns(conn).map_err(err)?.first() {
        let id = c.id.clone();
        time("组装 GM 上下文（最近的战役）", &mut || chronicle_gm::context::build_gm_context(&w, &id, "四处看看", 8000).map(|_| ()).map_err(err))?;
    }
    time("SQLite 快速自检", &mut || {
        let r: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0)).map_err(err)?;
        if r == "ok" { Ok(()) } else { Err(format!("自检发现问题：{r}")) }
    })?;
    Ok(DbProbe { file_mb: size(path.clone()), wal_mb: size(path.with_extension("cgw-wal")), nodes, events, timings })
}

/// 应用退出：停止所有由本软件启动的模型进程。
pub fn on_exit(app: &AppHandle) {
    if let Some(st) = app.try_state::<AppState>() {
        st.models.stop_all();
    }
}


/// 内存建议：按编辑中的托管配置（可以还没保存）、其他托管配置与正在运行模型的实测内存计算。只给建议，不改配置。
#[tauri::command]
pub fn memory_advice(state: State<AppState>, profile: ModelProfile) -> Res<Option<chronicle_runtime::MemoryAdvice>> {
    let mut all = load_profiles(&state)?;
    all.retain(|p| p.id != profile.id);
    let all_saved_args = |state: &AppState, id: &str| state.models.status().into_iter().find(|s| s.profile.id == id).map(|s| s.profile.args);
    all.push(profile.clone());
    let sample = state.monitor.latest();
    let mut measured = state.monitor.model_mem();
    // 参数改过（与保存的、也就是运行中用的不同）时，这个配置自己的实测值不再适用。
    if all_saved_args(&state, &profile.id).is_some_and(|a| a != profile.args) {
        measured.remove(&profile.id);
    }
    let total = match &sample {
        Some(s) => s.ram_total_mb,
        None => chronicle_runtime::monitor::total_ram_mb(),
    };
    if total == 0 {
        return Ok(None);
    }
    Ok(chronicle_runtime::memory_advice(&profile, &all, &measured, total))
}

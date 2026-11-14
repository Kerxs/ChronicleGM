//! Tauri 外壳：只把界面请求转发给 chronicle-core / chronicle-gen / chronicle-llm，不包含业务规则。

mod applog;
mod perf;
mod secrets;
mod update;
mod voice;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chronicle_core::changeset::{self, Changeset, ChangesetStatus, Op};
use chronicle_core::jobs::{self, GenJob};
use chronicle_core::library::{LlmRequestRecord, ProviderStats};
use chronicle_core::model::*;
use chronicle_core::{CommitInfo, Library, World, WorldInfo, nodes, scripts};
use chronicle_gen::runner::{self, GenOptions};
use chronicle_gen::{Task, Templates};
use chronicle_llm::discovery::{self, DiscoveredService};
use chronicle_llm::gguf::{self, ScanResult};
use chronicle_llm::secrets::SecretStore;
use chronicle_llm::{CancelToken, Capabilities, OpenAiClient, ProviderConfig};
use chronicle_runtime::{BeforeRequest, ModelManager, Monitor, PerfMode, Priority, ScheduledClient, Scheduler};
use perf::RenderStat;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

/// 界面层收到的错误一律是已脱敏的字符串。
type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

struct AppState {
    /// 资料库；启动时打不开则为 None，界面显示 `startup_error`。
    library: Mutex<Option<Library>>,
    startup_error: Option<String>,
    /// 已打开的世界（同一个世界只打开一个连接）。
    worlds: Mutex<HashMap<String, Arc<Mutex<World>>>>,
    templates: Arc<Templates>,
    running: Mutex<HashMap<String, CancelToken>>,
    secrets: Arc<dyn SecretStore>,
    /// 统一 AI 任务调度中心（所有模型请求都经过它）。
    sched: Arc<Scheduler>,
    /// 托管模式：按用户配置启停模型进程。
    models: Arc<ModelManager>,
    monitor: Arc<Monitor>,
    /// 正在进行的生成任务（去重：同一目标的同类生成不重复启动）。
    gen_keys: Arc<Mutex<HashSet<String>>>,
    /// 界面上报的图谱渲染耗时。
    render_stats: Mutex<VecDeque<RenderStat>>,
}

impl AppState {
    fn lib(&self) -> Res<LibGuard<'_>> {
        let g = self.library.lock().map_err(|_| "资料库被占用（上一次操作异常中断）".to_string())?;
        if g.is_none() {
            return Err(self.startup_error.clone().unwrap_or_else(|| "资料库未打开".into()));
        }
        Ok(LibGuard(g))
    }

    fn world(&self, id: &str) -> Res<Arc<Mutex<World>>> {
        let mut map = self.worlds.lock().map_err(err)?;
        if let Some(w) = map.get(id) {
            return Ok(w.clone());
        }
        let w = self.lib()?.open_world(id).map_err(err)?;
        // 上次退出时仍在运行的任务标记为中断，可重试。
        jobs::recover_interrupted(w.conn()).map_err(err)?;
        // 自动备份：距上次超过一天才备份，只保留最近 10 份自动备份。失败不影响打开，但会记录。
        if let Err(e) = chronicle_core::backup::auto_backup(&w, AUTO_BACKUP_KEEP, AUTO_BACKUP_INTERVAL_MS) {
            applog::error(&format!("自动备份失败（{id}）：{e}"));
        }
        let w = Arc::new(Mutex::new(w));
        map.insert(id.to_string(), w.clone());
        Ok(w)
    }

    fn provider(&self, id: &str) -> Res<ProviderConfig> {
        let v = self.lib()?.provider(id).map_err(err)?.ok_or("模型服务配置不存在")?;
        serde_json::from_value(v).map_err(err)
    }

    /// 包装成经过调度中心的客户端（托管模式下按需启动模型）。
    fn scheduled(&self, client: OpenAiClient, cfg: &ProviderConfig, priority: Priority, kind: &str) -> ScheduledClient {
        let before: Arc<dyn BeforeRequest> = self.models.clone();
        ScheduledClient::new(Arc::new(client), self.sched.clone(), &cfg.base_url, priority, kind).with_before(Some(before))
    }

    fn client(&self, cfg: &ProviderConfig) -> Res<OpenAiClient> {
        let key = if cfg.needs_key { self.secrets.get(&cfg.id)? } else { None };
        if cfg.needs_key && key.is_none() {
            return Err(format!("「{}」需要 API Key，请先在设置中填写", cfg.name));
        }
        Ok(OpenAiClient::new(cfg.clone(), key))
    }
}

const AUTO_BACKUP_KEEP: usize = 10;
const AUTO_BACKUP_INTERVAL_MS: i64 = 24 * 3600 * 1000;

/// 独占世界文件执行替换操作（回滚版本、从备份还原）。有生成任务运行或世界正被使用时拒绝。
fn replace_world(
    state: &AppState,
    world_id: String,
    f: impl FnOnce(World) -> chronicle_core::Result<(World, PathBuf)>,
) -> Res<String> {
    if !state.running.lock().map_err(err)?.is_empty() {
        return Err("有生成任务正在运行，请等待完成或取消后再操作".into());
    }
    let mut map = state.worlds.lock().map_err(err)?;
    let arc = map.remove(&world_id).ok_or("世界尚未打开")?;
    let world = match Arc::try_unwrap(arc) {
        Ok(m) => m.into_inner().map_err(|_| "世界文件被占用".to_string())?,
        Err(arc) => {
            map.insert(world_id, arc);
            return Err("世界文件正在被使用，请稍后再试".into());
        }
    };
    match f(world) {
        Ok((w, backup)) => {
            map.insert(world_id, Arc::new(Mutex::new(w)));
            Ok(backup.display().to_string())
        }
        Err(e) => {
            // 替换在复制文件之前失败不会改动文件；重新打开原文件。
            drop(map);
            let _ = state.world(&world_id);
            Err(e.to_string())
        }
    }
}

struct LibGuard<'a>(MutexGuard<'a, Option<Library>>);

impl std::ops::Deref for LibGuard<'_> {
    type Target = Library;
    fn deref(&self) -> &Library {
        self.0.as_ref().expect("lib() 已检查")
    }
}

impl std::ops::DerefMut for LibGuard<'_> {
    fn deref_mut(&mut self) -> &mut Library {
        self.0.as_mut().expect("lib() 已检查")
    }
}

fn lock(w: &Arc<Mutex<World>>) -> Res<MutexGuard<'_, World>> {
    w.lock().map_err(|_| "世界文件被占用（上一次操作异常中断）".to_string())
}

/// 应用数据目录：默认 `%LOCALAPPDATA%\ChronicleGM`，可用环境变量 `CHRONICLE_DATA_DIR` 覆盖（开发与测试用）。
fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CHRONICLE_DATA_DIR") {
        return PathBuf::from(d);
    }
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("ChronicleGM")
}

// ===================== 资料库与世界 =====================

#[derive(Serialize)]
struct AppInfo {
    version: &'static str,
    data_dir: String,
    template_version: String,
    templates_overridden: Vec<String>,
}

/// 启动失败时的说明（资料库打不开等）；正常为 None。
#[tauri::command]
fn startup_error(state: State<AppState>) -> Option<String> {
    state.startup_error.clone()
}

/// 界面偏好（只允许 `ui.` 开头的键，例如是否看过使用引导、侧栏是否收起）。存在资料库里，跟着数据目录走。
#[tauri::command]
fn ui_pref(state: State<AppState>, key: String) -> Res<Option<String>> {
    if !key.starts_with("ui.") {
        return Err("只能读取界面偏好".into());
    }
    state.lib()?.setting(&key).map_err(err)
}

#[tauri::command]
fn set_ui_pref(state: State<AppState>, key: String, value: String) -> Res<()> {
    if !key.starts_with("ui.") || value.len() > 200 {
        return Err("只能保存界面偏好".into());
    }
    state.lib()?.set_setting(&key, &value).map_err(err)
}

#[tauri::command]
fn app_info(state: State<AppState>) -> Res<AppInfo> {
    let lib = state.lib()?;
    Ok(AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        data_dir: lib.data_dir().display().to_string(),
        template_version: state.templates.version.clone(),
        templates_overridden: state.templates.overridden.clone(),
    })
}

#[tauri::command]
fn list_worlds(state: State<AppState>) -> Res<Vec<WorldInfo>> {
    state.lib()?.list_worlds().map_err(err)
}

#[tauri::command]
fn create_world(state: State<AppState>, name: String) -> Res<WorldInfo> {
    let (info, w) = state.lib()?.create_world(&name).map_err(err)?;
    state.worlds.lock().map_err(err)?.insert(info.id.clone(), Arc::new(Mutex::new(w)));
    Ok(info)
}

#[derive(Serialize)]
struct WorldOverview {
    info: WorldInfo,
    root_nodes: i64,
    node_count: i64,
    script_count: usize,
    pending_changesets: usize,
}

#[tauri::command]
fn world_overview(state: State<AppState>, world_id: String) -> Res<WorldOverview> {
    let info = state.lib()?.get_world(&world_id).map_err(err)?;
    let w = state.world(&world_id)?;
    let w = lock(&w)?;
    let conn = w.conn();
    Ok(WorldOverview {
        root_nodes: nodes::child_count(conn, None).map_err(err)?,
        node_count: conn
            .query_row("SELECT COUNT(*) FROM nodes WHERE deleted_at IS NULL", [], |r| r.get(0))
            .map_err(err)?,
        script_count: scripts::list_scripts(conn).map_err(err)?.len(),
        pending_changesets: changeset::list_changesets(conn, Some(ChangesetStatus::Pending)).map_err(err)?.len(),
        info,
    })
}

#[tauri::command]
fn list_commits(state: State<AppState>, world_id: String, limit: i64) -> Res<Vec<CommitInfo>> {
    let w = state.world(&world_id)?;
    lock(&w)?.commits(limit.clamp(1, 500), 0).map_err(err)
}

#[tauri::command]
fn undo_commit(state: State<AppState>, world_id: String, commit_id: String) -> Res<Option<String>> {
    let w = state.world(&world_id)?;
    lock(&w)?.undo(&commit_id, &Author::User).map_err(err)
}

// ===================== 剧本 =====================

#[tauri::command]
fn list_scripts(state: State<AppState>, world_id: String) -> Res<Vec<Script>> {
    let w = state.world(&world_id)?;
    scripts::list_scripts(lock(&w)?.conn()).map_err(err)
}

#[derive(Serialize)]
struct CastView {
    cast: CastMember,
    name: String,
    node_rev: i64,
}

#[derive(Serialize)]
struct ScriptView {
    script: Script,
    /// 全部条目（扁平列表，按父节点与排序键排列；界面据此组装树）。
    items: Vec<ScriptItem>,
    cast: Vec<CastView>,
}

#[tauri::command]
fn get_script(state: State<AppState>, world_id: String, script_id: String) -> Res<ScriptView> {
    let w = state.world(&world_id)?;
    let w = lock(&w)?;
    let conn = w.conn();
    let script = scripts::require_script(conn, &script_id).map_err(err)?;
    let mut items = vec![];
    fn walk(conn: &chronicle_core::rusqlite::Connection, sid: &str, parent: Option<&str>, out: &mut Vec<ScriptItem>) -> Res<()> {
        for it in scripts::item_children(conn, sid, parent).map_err(err)? {
            let id = it.id.clone();
            out.push(it);
            walk(conn, sid, Some(&id), out)?;
        }
        Ok(())
    }
    walk(conn, &script_id, None, &mut items)?;
    let mut cast = vec![];
    for c in scripts::cast_of(conn, &script_id).map_err(err)? {
        if let Some(n) = nodes::get_node(conn, &c.node_id).map_err(err)? {
            cast.push(CastView { name: n.title, node_rev: n.rev, cast: c });
        }
    }
    Ok(ScriptView { script, items, cast })
}

#[tauri::command]
fn create_script(state: State<AppState>, world_id: String, script: NewScript) -> Res<Script> {
    let w = state.world(&world_id)?;
    Ok(lock(&w)?.write(&Author::User, "新建剧本", |x| x.create_script(script)).map_err(err)?.0)
}

#[tauri::command]
fn update_script(state: State<AppState>, world_id: String, id: String, patch: ScriptPatch, rev: i64) -> Res<Script> {
    let w = state.world(&world_id)?;
    Ok(lock(&w)?.write(&Author::User, "编辑剧本", |x| x.update_script(&id, patch, Some(rev))).map_err(err)?.0)
}

#[tauri::command]
fn create_script_item(state: State<AppState>, world_id: String, item: NewScriptItem) -> Res<ScriptItem> {
    let w = state.world(&world_id)?;
    Ok(lock(&w)?.write(&Author::User, "新建条目", |x| x.create_script_item(item)).map_err(err)?.0)
}

#[tauri::command]
fn update_script_item(state: State<AppState>, world_id: String, id: String, patch: ScriptItemPatch, rev: i64) -> Res<ScriptItem> {
    let w = state.world(&world_id)?;
    Ok(lock(&w)?.write(&Author::User, "编辑条目", |x| x.update_script_item(&id, patch, Some(rev))).map_err(err)?.0)
}

#[tauri::command]
fn delete_script_item(state: State<AppState>, world_id: String, id: String, rev: i64) -> Res<usize> {
    let w = state.world(&world_id)?;
    Ok(lock(&w)?.write(&Author::User, "删除条目", |x| x.delete_script_item(&id, Some(rev))).map_err(err)?.0)
}

#[tauri::command]
fn search_script_items(state: State<AppState>, world_id: String, script_id: Option<String>, query: String) -> Res<Vec<ScriptItem>> {
    let w = state.world(&world_id)?;
    chronicle_core::search::search_script_items(lock(&w)?.conn(), script_id.as_deref(), &query, false, 100).map_err(err)
}

#[tauri::command]
fn get_node(state: State<AppState>, world_id: String, id: String) -> Res<Option<Node>> {
    let w = state.world(&world_id)?;
    nodes::get_node(lock(&w)?.conn(), &id).map_err(err)
}

// ===================== 世界树 =====================

#[derive(Serialize)]
struct TreeNode {
    node: Node,
    child_count: i64,
}

#[tauri::command]
fn tree_children(state: State<AppState>, world_id: String, parent_id: Option<String>, offset: i64, limit: i64) -> Res<Vec<TreeNode>> {
    let w = state.world(&world_id)?;
    let w = lock(&w)?;
    let kids = nodes::children(w.conn(), parent_id.as_deref(), offset.max(0), limit.clamp(1, 1000)).map_err(err)?;
    let ids: Vec<String> = kids.iter().map(|n| n.id.clone()).collect();
    let counts = chronicle_core::relations::child_counts(w.conn(), &ids).map_err(err)?;
    Ok(kids.into_iter().zip(counts).map(|(node, (_, c))| TreeNode { node, child_count: c }).collect())
}

#[derive(Serialize)]
struct RelationView {
    relation: Relation,
    other_id: String,
    other_title: String,
    other_kind: NodeKind,
    outgoing: bool,
}

#[derive(Serialize)]
struct NodeDetail {
    node: Node,
    ancestors: Vec<Node>,
    relations: Vec<RelationView>,
    child_count: i64,
}

#[tauri::command]
fn node_detail(state: State<AppState>, world_id: String, id: String) -> Res<NodeDetail> {
    let w = state.world(&world_id)?;
    let w = lock(&w)?;
    let c = w.conn();
    let node = nodes::require_node(c, &id).map_err(err)?;
    let mut relations = vec![];
    for r in nodes::relations_of(c, &id).map_err(err)? {
        let outgoing = r.src_id == id;
        let other = if outgoing { &r.dst_id } else { &r.src_id };
        if let Some(o) = nodes::get_node(c, other).map_err(err)? {
            relations.push(RelationView { other_id: o.id, other_title: o.title, other_kind: o.kind, outgoing, relation: r });
        }
    }
    Ok(NodeDetail {
        ancestors: nodes::ancestors(c, &id).map_err(err)?,
        child_count: nodes::child_count(c, Some(&id)).map_err(err)?,
        node,
        relations,
    })
}

#[tauri::command]
fn create_node(state: State<AppState>, world_id: String, node: NewNode) -> Res<Node> {
    let w = state.world(&world_id)?;
    Ok(lock(&w)?.write(&Author::User, "新建节点", |x| x.create_node(node)).map_err(err)?.0)
}

#[tauri::command]
fn update_node(state: State<AppState>, world_id: String, id: String, patch: NodePatch, rev: i64) -> Res<Node> {
    let w = state.world(&world_id)?;
    Ok(lock(&w)?.write(&Author::User, "编辑节点", |x| x.update_node(&id, patch, Some(rev))).map_err(err)?.0)
}

#[tauri::command]
fn move_node(state: State<AppState>, world_id: String, id: String, new_parent: Option<String>, position: Position, rev: i64) -> Res<Node> {
    let w = state.world(&world_id)?;
    Ok(lock(&w)?
        .write(&Author::User, "移动节点", |x| x.move_node(&id, new_parent.as_deref(), position, Some(rev)))
        .map_err(err)?
        .0)
}

#[tauri::command]
fn delete_node(state: State<AppState>, world_id: String, id: String, rev: i64) -> Res<usize> {
    let w = state.world(&world_id)?;
    Ok(lock(&w)?.write(&Author::User, "删除节点", |x| x.delete_node(&id, Some(rev))).map_err(err)?.0)
}

#[tauri::command]
fn restore_node(state: State<AppState>, world_id: String, id: String) -> Res<usize> {
    let w = state.world(&world_id)?;
    Ok(lock(&w)?.write(&Author::User, "从回收站恢复", |x| x.restore_node(&id)).map_err(err)?.0)
}

#[tauri::command]
fn list_trash(state: State<AppState>, world_id: String) -> Res<Vec<Node>> {
    let w = state.world(&world_id)?;
    nodes::trash(lock(&w)?.conn(), 200).map_err(err)
}

#[tauri::command]
fn create_relation(state: State<AppState>, world_id: String, relation: NewRelation) -> Res<Relation> {
    let w = state.world(&world_id)?;
    Ok(lock(&w)?.write(&Author::User, "新建关系", |x| x.create_relation(relation)).map_err(err)?.0)
}

#[tauri::command]
fn update_relation(state: State<AppState>, world_id: String, id: String, patch: chronicle_core::relations::RelationPatch, rev: i64) -> Res<Relation> {
    let w = state.world(&world_id)?;
    Ok(lock(&w)?.write(&Author::User, "编辑关系", |x| x.update_relation(&id, patch, Some(rev))).map_err(err)?.0)
}

#[tauri::command]
fn delete_relation(state: State<AppState>, world_id: String, id: String, rev: i64) -> Res<()> {
    let w = state.world(&world_id)?;
    lock(&w)?.write(&Author::User, "删除关系", |x| x.delete_relation(&id, Some(rev))).map_err(err)?;
    Ok(())
}

#[tauri::command]
fn relation_types() -> Vec<chronicle_core::relations::RelationType> {
    chronicle_core::relations::RELATION_TYPES.to_vec()
}

#[tauri::command]
fn search_nodes(state: State<AppState>, world_id: String, query: String, options: chronicle_core::search::SearchOptions) -> Res<Vec<chronicle_core::search::SearchHit>> {
    let w = state.world(&world_id)?;
    chronicle_core::search::search_nodes(lock(&w)?.conn(), &query, &options).map_err(err)
}

#[tauri::command]
fn node_graph(state: State<AppState>, world_id: String, center: String, depth: u32, limit: usize) -> Res<chronicle_core::relations::Graph> {
    let w = state.world(&world_id)?;
    chronicle_core::relations::neighborhood(lock(&w)?.conn(), &center, depth.clamp(1, 3), limit.clamp(5, 300)).map_err(err)
}

#[tauri::command]
fn check_consistency(state: State<AppState>, world_id: String) -> Res<Vec<chronicle_core::consistency::Finding>> {
    let w = state.world(&world_id)?;
    chronicle_core::consistency::check_world(lock(&w)?.conn()).map_err(err)
}

#[tauri::command]
fn kind_counts(state: State<AppState>, world_id: String) -> Res<Vec<(String, i64)>> {
    let w = state.world(&world_id)?;
    chronicle_core::relations::kind_counts(lock(&w)?.conn()).map_err(err)
}

// ===================== 版本与导入导出 =====================

#[tauri::command]
fn list_versions(state: State<AppState>, world_id: String) -> Res<Vec<chronicle_core::versions::WorldVersion>> {
    let w = state.world(&world_id)?;
    chronicle_core::versions::list_versions(lock(&w)?.conn()).map_err(err)
}

#[tauri::command]
fn create_version(state: State<AppState>, world_id: String, name: String, note: String) -> Res<chronicle_core::versions::WorldVersion> {
    let w = state.world(&world_id)?;
    chronicle_core::versions::create_version(&*lock(&w)?, &name, &note).map_err(err)
}

/// 回滚到版本：需要独占世界文件，有生成任务运行时拒绝。返回回滚前备份的路径。
#[tauri::command]
fn restore_version(state: State<AppState>, world_id: String, version_id: String) -> Res<String> {
    replace_world(&state, world_id, |w| chronicle_core::versions::restore_version(w, &version_id))
}

#[tauri::command]
fn list_backups(state: State<AppState>, world_id: String) -> Res<Vec<chronicle_core::backup::BackupInfo>> {
    let w = state.world(&world_id)?;
    chronicle_core::backup::list_backups(&*lock(&w)?).map_err(err)
}

/// 从备份还原：当前状态先另存为回滚前备份。返回该备份路径。
#[tauri::command]
fn restore_backup(state: State<AppState>, world_id: String, file_name: String) -> Res<String> {
    replace_world(&state, world_id, |w| chronicle_core::backup::restore_backup(w, &file_name))
}

/// 立即备份（不受一天一次的限制，同样参与轮换）。
#[tauri::command]
fn backup_now(state: State<AppState>, world_id: String) -> Res<Option<chronicle_core::backup::BackupInfo>> {
    let w = state.world(&world_id)?;
    chronicle_core::backup::auto_backup(&*lock(&w)?, AUTO_BACKUP_KEEP, 0).map_err(err)
}

#[tauri::command]
fn check_integrity(state: State<AppState>, world_id: String) -> Res<chronicle_core::integrity::IntegrityReport> {
    let w = state.world(&world_id)?;
    chronicle_core::integrity::check(&*lock(&w)?).map_err(err)
}

#[tauri::command]
fn repair_integrity(state: State<AppState>, world_id: String) -> Res<chronicle_core::integrity::RepairReport> {
    let w = state.world(&world_id)?;
    chronicle_core::integrity::repair(&mut *lock(&w)?).map_err(err)
}

#[tauri::command]
fn export_world(state: State<AppState>, world_id: String, path: String, include_deleted: bool) -> Res<chronicle_core::bundle::BundleManifest> {
    let w = state.world(&world_id)?;
    chronicle_core::bundle::export_bundle(&*lock(&w)?, &PathBuf::from(path), include_deleted).map_err(err)
}

#[tauri::command]
fn import_into_world(state: State<AppState>, world_id: String, path: String, strategy: chronicle_core::bundle::ConflictStrategy) -> Res<chronicle_core::bundle::ImportReport> {
    let w = state.world(&world_id)?;
    chronicle_core::bundle::import_bundle(&mut *lock(&w)?, &PathBuf::from(path), strategy, &Author::User).map_err(err)
}

#[tauri::command]
fn import_as_new_world(state: State<AppState>, path: String, name: Option<String>) -> Res<WorldInfo> {
    let (info, w, _) = state.lib()?.import_world(&PathBuf::from(path), name.as_deref()).map_err(err)?;
    state.worlds.lock().map_err(err)?.insert(info.id.clone(), Arc::new(Mutex::new(w)));
    Ok(info)
}

// ===================== 剧情图 =====================

#[derive(Serialize)]
struct PlotGraph {
    nodes: Vec<PlotNode>,
    edges: Vec<PlotEdge>,
}

#[tauri::command]
fn plot_graph(state: State<AppState>, world_id: String, script_id: String) -> Res<PlotGraph> {
    let w = state.world(&world_id)?;
    let w = lock(&w)?;
    Ok(PlotGraph {
        nodes: chronicle_core::plot::plot_nodes_of(w.conn(), &script_id).map_err(err)?,
        edges: chronicle_core::plot::edges_of_script(w.conn(), &script_id).map_err(err)?,
    })
}

#[tauri::command]
fn create_plot_node(state: State<AppState>, world_id: String, node: NewPlotNode) -> Res<PlotNode> {
    let w = state.world(&world_id)?;
    Ok(lock(&w)?.write(&Author::User, "新建剧情节点", |x| x.create_plot_node(node)).map_err(err)?.0)
}

#[tauri::command]
fn update_plot_node(state: State<AppState>, world_id: String, id: String, patch: PlotNodePatch, rev: i64) -> Res<PlotNode> {
    let w = state.world(&world_id)?;
    Ok(lock(&w)?.write(&Author::User, "编辑剧情节点", |x| x.update_plot_node(&id, patch, Some(rev))).map_err(err)?.0)
}

#[tauri::command]
fn delete_plot_node(state: State<AppState>, world_id: String, id: String, rev: i64) -> Res<()> {
    let w = state.world(&world_id)?;
    lock(&w)?.write(&Author::User, "删除剧情节点", |x| x.delete_plot_node(&id, Some(rev))).map_err(err)?;
    Ok(())
}

#[tauri::command]
fn create_plot_edge(state: State<AppState>, world_id: String, edge: NewPlotEdge) -> Res<PlotEdge> {
    let w = state.world(&world_id)?;
    Ok(lock(&w)?.write(&Author::User, "新建剧情分支", |x| x.create_plot_edge(edge)).map_err(err)?.0)
}

#[derive(Deserialize)]
struct EdgeUpdate {
    label: String,
    condition: Option<chronicle_core::rules::Condition>,
    effects: Vec<chronicle_core::rules::Effect>,
    priority: i64,
    once: bool,
}

#[tauri::command]
fn update_plot_edge(state: State<AppState>, world_id: String, id: String, edge: EdgeUpdate, rev: i64) -> Res<PlotEdge> {
    let w = state.world(&world_id)?;
    Ok(lock(&w)?
        .write(&Author::User, "编辑剧情分支", |x| {
            let patch = PlotEdgePatch {
                label: Some(edge.label),
                condition: Some(edge.condition),
                effects: Some(edge.effects),
                priority: Some(edge.priority),
                once: Some(edge.once),
            };
            x.update_plot_edge(&id, patch, Some(rev))
        })
        .map_err(err)?
        .0)
}

#[tauri::command]
fn delete_plot_edge(state: State<AppState>, world_id: String, id: String, rev: i64) -> Res<()> {
    let w = state.world(&world_id)?;
    lock(&w)?.write(&Author::User, "删除剧情分支", |x| x.delete_plot_edge(&id, Some(rev))).map_err(err)?;
    Ok(())
}

#[tauri::command]
fn analyze_plot(state: State<AppState>, world_id: String, script_id: String) -> Res<chronicle_core::plot_analysis::PlotReport> {
    let w = state.world(&world_id)?;
    chronicle_core::plot_analysis::analyze_plot(lock(&w)?.conn(), &script_id).map_err(err)
}

#[tauri::command]
fn sim_start(state: State<AppState>, world_id: String, script_id: String) -> Res<chronicle_core::sim::SimState> {
    let w = state.world(&world_id)?;
    chronicle_core::sim::sim_start(lock(&w)?.conn(), &script_id).map_err(err)
}

#[tauri::command]
fn sim_options(state: State<AppState>, world_id: String, sim: chronicle_core::sim::SimState) -> Res<Vec<chronicle_core::campaign::EdgeOption>> {
    let w = state.world(&world_id)?;
    chronicle_core::sim::sim_options(lock(&w)?.conn(), &sim).map_err(err)
}

#[tauri::command]
fn sim_step(state: State<AppState>, world_id: String, sim: chronicle_core::sim::SimState, edge_id: String, mode: chronicle_core::sim::CheckMode, seed: u64) -> Res<chronicle_core::sim::SimStep> {
    let w = state.world(&world_id)?;
    chronicle_core::sim::sim_step(lock(&w)?.conn(), &sim, &edge_id, mode, seed).map_err(err)
}

// ===================== 试玩（战役） =====================

#[tauri::command]
fn list_campaigns(state: State<AppState>, world_id: String) -> Res<Vec<chronicle_core::campaign::Campaign>> {
    let w = state.world(&world_id)?;
    chronicle_core::campaign::list_campaigns(lock(&w)?.conn()).map_err(err)
}

#[tauri::command]
fn create_campaign(state: State<AppState>, world_id: String, script_id: Option<String>, name: String) -> Res<chronicle_core::campaign::Campaign> {
    let w = state.world(&world_id)?;
    chronicle_core::campaign::create_campaign(&mut *lock(&w)?, script_id.as_deref(), &name, None).map_err(err)
}

#[derive(Serialize)]
struct CampaignStatus {
    campaign: chronicle_core::campaign::Campaign,
    current: Option<PlotNode>,
    options: Vec<chronicle_core::campaign::EdgeOption>,
    state: chronicle_core::campaign::CampaignState,
    events: Vec<chronicle_core::campaign::EventEntry>,
    /// 上次程序意外退出时没有完成的回合。
    incomplete: Option<chronicle_core::campaign::IncompleteTurn>,
}

#[tauri::command]
fn campaign_status(state: State<AppState>, world_id: String, campaign_id: String) -> Res<CampaignStatus> {
    let w = state.world(&world_id)?;
    let w = lock(&w)?;
    let c = chronicle_core::campaign::get_campaign(w.conn(), &campaign_id).map_err(err)?;
    let all = chronicle_core::campaign::events(w.conn(), &campaign_id, 0, 10_000).map_err(err)?;
    let events = all.into_iter().rev().take(50).collect::<Vec<_>>().into_iter().rev().collect();
    Ok(CampaignStatus {
        current: chronicle_core::campaign::current_node(&w, &campaign_id).map_err(err)?,
        options: chronicle_core::campaign::available_edges(&w, &campaign_id).map_err(err)?,
        state: chronicle_core::campaign::campaign_state(w.conn(), &campaign_id).map_err(err)?,
        events,
        incomplete: chronicle_core::campaign::incomplete_turn(w.conn(), &campaign_id).map_err(err)?,
        campaign: c,
    })
}

/// 确认已知晓中断的回合：记录 turn_aborted，此后不再提示。已执行的效果保留在日志中。
#[tauri::command]
fn dismiss_incomplete_turn(state: State<AppState>, world_id: String, campaign_id: String) -> Res<()> {
    let w = state.world(&world_id)?;
    let w = lock(&w)?;
    if chronicle_core::campaign::incomplete_turn(w.conn(), &campaign_id).map_err(err)?.is_some() {
        chronicle_core::campaign::record(w.conn(), &campaign_id, "turn_aborted", &serde_json::json!({ "error": "程序意外退出，回合未完成" })).map_err(err)?;
    }
    Ok(())
}

#[tauri::command]
fn campaign_take_edge(state: State<AppState>, world_id: String, campaign_id: String, edge_id: String) -> Res<chronicle_core::campaign::TransitionOutcome> {
    let w = state.world(&world_id)?;
    let mut w = lock(&w)?;
    let out = chronicle_core::campaign::take_edge(&mut w, &campaign_id, &edge_id).map_err(err)?;
    // 手动推进剧情也是可以回溯到的历史节点。
    chronicle_core::timeline::checkpoint(w.conn(), &campaign_id, "transition").map_err(err)?;
    Ok(out)
}

/// 把世界中已有的人物加入剧本人物表（不经过 AI；可撤销）。
#[tauri::command]
fn add_cast_member(state: State<AppState>, world_id: String, script_id: String, node_id: String, role: String) -> Res<chronicle_core::model::CastMember> {
    let w = state.world(&world_id)?;
    let mut w = lock(&w)?;
    let node = nodes::get_node(w.conn(), &node_id).map_err(err)?.ok_or("人物不存在")?;
    if node.kind != NodeKind::Character {
        return Err(format!("「{}」不是人物", node.title));
    }
    if scripts::cast_of(w.conn(), &script_id).map_err(err)?.iter().any(|c| c.node_id == node_id) {
        return Err(format!("「{}」已经在人物表里", node.title));
    }
    let c = NewCastMember { script_id, node_id, role, motive: String::new(), secret: String::new(), relation_to_players: String::new(), payload: serde_json::json!({}) };
    w.write(&Author::User, &format!("加入人物「{}」", node.title), |x| x.add_cast(c)).map(|(c, _)| c).map_err(err)
}

// ===================== 调查、任务、NPC =====================

#[tauri::command]
fn investigation_board(state: State<AppState>, world_id: String, campaign_id: String, gm_view: bool) -> Res<chronicle_core::investigation::Board> {
    let w = state.world(&world_id)?;
    chronicle_core::investigation::board(lock(&w)?.conn(), &campaign_id, gm_view).map_err(err)
}

#[tauri::command]
fn board_add_note(state: State<AppState>, world_id: String, campaign_id: String, title: String, detail: String) -> Res<String> {
    let w = state.world(&world_id)?;
    chronicle_core::investigation::add_clue(&mut *lock(&w)?, &campaign_id, &title, &detail, "player").map_err(err)
}

#[tauri::command]
fn board_remove_note(state: State<AppState>, world_id: String, campaign_id: String, id: String) -> Res<()> {
    let w = state.world(&world_id)?;
    chronicle_core::investigation::remove_note(&mut *lock(&w)?, &campaign_id, &id).map_err(err)
}

#[tauri::command]
fn board_link(state: State<AppState>, world_id: String, campaign_id: String, a: String, b: String, relation: String, note: String) -> Res<String> {
    let w = state.world(&world_id)?;
    chronicle_core::investigation::link(&mut *lock(&w)?, &campaign_id, &a, &b, &relation, &note).map_err(err)
}

#[tauri::command]
fn board_unlink(state: State<AppState>, world_id: String, campaign_id: String, id: String) -> Res<()> {
    let w = state.world(&world_id)?;
    chronicle_core::investigation::unlink(&mut *lock(&w)?, &campaign_id, &id).map_err(err)
}

#[tauri::command]
fn board_save_hypothesis(state: State<AppState>, world_id: String, campaign_id: String, id: Option<String>, input: chronicle_core::investigation::HypothesisInput) -> Res<String> {
    let w = state.world(&world_id)?;
    chronicle_core::investigation::save_hypothesis(&mut *lock(&w)?, &campaign_id, id.as_deref(), &input).map_err(err)
}

#[tauri::command]
fn board_delete_hypothesis(state: State<AppState>, world_id: String, campaign_id: String, id: String) -> Res<()> {
    let w = state.world(&world_id)?;
    chronicle_core::investigation::delete_hypothesis(&mut *lock(&w)?, &campaign_id, &id).map_err(err)
}

#[tauri::command]
fn quest_list(state: State<AppState>, world_id: String, campaign_id: String, gm_view: bool) -> Res<Vec<chronicle_core::quests::QuestView>> {
    let w = state.world(&world_id)?;
    chronicle_core::quests::quests(lock(&w)?.conn(), &campaign_id, gm_view).map_err(err)
}

#[derive(Serialize)]
struct NpcView {
    status: chronicle_core::schedule::NpcStatus,
    mood: Option<chronicle_core::rules::effect::Mood>,
    bond_to_party: Option<chronicle_core::rules::effect::Bond>,
}

#[derive(Serialize)]
struct NpcStates {
    time: String,
    minutes: i64,
    npcs: Vec<NpcView>,
}

#[tauri::command]
fn npc_states(state: State<AppState>, world_id: String, campaign_id: String) -> Res<NpcStates> {
    use chronicle_core::rules::StateView;
    let w = state.world(&world_id)?;
    let w = lock(&w)?;
    let conn = w.conn();
    let view = chronicle_core::campaign::CampaignView { conn, campaign_id: &campaign_id };
    let minutes = chronicle_core::schedule::clock(conn, &campaign_id);
    let npcs = chronicle_core::schedule::all_npc_status(conn, &campaign_id)
        .map_err(err)?
        .into_iter()
        .map(|s| NpcView { mood: view.mood(&s.npc_id), bond_to_party: view.bond(&s.npc_id, "party").map(|(_, b)| b), status: s })
        .collect();
    Ok(NpcStates { time: chronicle_core::schedule::format_clock(minutes), minutes, npcs })
}

/// 手动推进故事时间（例如"等到晚上"）：NPC 按日程活动、情绪平复、任务按条件推进。
#[tauri::command]
fn advance_story_time(state: State<AppState>, world_id: String, campaign_id: String, minutes: i64) -> Res<Vec<String>> {
    let w = state.world(&world_id)?;
    let mut w = lock(&w)?;
    chronicle_core::campaign::ensure_active(w.conn(), &campaign_id).map_err(err)?;
    let mut notes = chronicle_core::schedule::advance(&mut w, &campaign_id, minutes, &format!("玩家等待 {minutes} 分钟")).map_err(err)?;
    notes.extend(chronicle_core::quests::tick(&mut w, &campaign_id).map_err(err)?);
    Ok(notes)
}

// ===================== 语音 =====================

pub(crate) fn voice_cache_dir() -> PathBuf {
    data_dir().join("voice_cache")
}

/// 朗读用的语音服务。
struct SpeechRoute {
    target: chronicle_llm::tts::SpeechTarget,
    /// 可分给 NPC 的声音（云端服务）。
    voices: Vec<String>,
    /// 不是本机服务（没有 /health，可能计费）。
    cloud: bool,
}

fn is_speech_provider(cfg: &ProviderConfig) -> bool {
    cfg.service == chronicle_llm::types::ServiceType::Speech || cfg.server_kind == Some(chronicle_llm::types::ServerKind::BreezeTts)
}

fn speech_route_for(state: &AppState, cfg: ProviderConfig) -> Res<SpeechRoute> {
    let cloud = !cfg.is_loopback();
    // 朗读的是叙述与台词（战役内容）：发到云端前要用户在设置里允许。
    if cloud && !cfg.allow_campaign_content {
        return Err(format!("「{}」是云端服务，还没有允许发送战役内容（在「模型服务」里打开该服务的「允许发送战役内容」）", cfg.name));
    }
    let key = if cfg.needs_key { state.secrets.get(&cfg.id)? } else { None };
    if cfg.needs_key && key.is_none() {
        return Err(format!("「{}」需要 API Key，请先在设置中填写", cfg.name));
    }
    let breeze = cfg.server_kind == Some(chronicle_llm::types::ServerKind::BreezeTts);
    let target = if breeze && !cloud {
        chronicle_llm::tts::SpeechTarget::local(&cfg.base_url)
    } else {
        chronicle_llm::tts::SpeechTarget {
            base_url: cfg.base_url.clone(),
            api_key: key,
            model: if breeze { String::new() } else { cfg.model.clone() },
            default_voice: cfg.media.voice.clone(),
            format: cfg.media.audio_format.clone(),
            send_instructions: cfg.media.send_instructions,
            send_seed: !cloud,
        }
    };
    Ok(SpeechRoute { target, voices: cfg.media.voices.clone(), cloud })
}

fn speech_route(state: &AppState) -> Res<SpeechRoute> {
    // 语音不回退到默认路由（那是对话模型）；没有单独设置时，用第一个语音服务。
    // 先取出路由再查服务：`if let` 条件里的资料库锁会一直持有到块结束，块里再取锁会自己卡死（实测：语音请求一直不返回）。
    let routed = state.lib()?.route_exact("tts").map_err(err)?;
    if let Some(id) = routed {
        return speech_route_for(state, state.provider(&id)?);
    }
    let all = state.lib()?.providers().map_err(err)?;
    for v in all {
        if let Ok(cfg) = serde_json::from_value::<ProviderConfig>(v)
            && is_speech_provider(&cfg)
        {
            return speech_route_for(state, cfg);
        }
    }
    // 托管的语音模型：直接用它的端口。
    if let Some(p) = state.models.status().into_iter().map(|s| s.profile).find(|p| p.role == chronicle_runtime::ModelRole::Tts) {
        return Ok(SpeechRoute { target: chronicle_llm::tts::SpeechTarget::local(&format!("http://127.0.0.1:{}", p.port)), voices: vec![], cloud: false });
    }
    Err("还没有设置语音服务：请在「模型服务」里添加语音服务（本机 Breeze TTS，或云端的语音合成），或在性能中心托管语音模型".into())
}

fn narrator_voice(state: &AppState) -> chronicle_llm::tts::VoiceSpec {
    state.lib().ok().and_then(|l| l.setting(voice::NARRATOR_KEY).ok().flatten()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(voice::default_narrator)
}

/// 朗读一段文字：speaker 为人物 ID（NPC 的声音）或空（旁白）。结果是可以直接播放的 WAV。
#[tauri::command]
async fn voice_speak(app: AppHandle, world_id: String, speaker: Option<String>, text: String) -> Res<voice::SpeakResult> {
    let state = app.state::<AppState>();
    let route = speech_route(&state)?;
    let spec = match speaker.as_deref().filter(|s| !s.is_empty()) {
        Some(id) => {
            let w = state.world(&world_id)?;
            let node = chronicle_core::nodes::get_node(lock(&w)?.conn(), id).map_err(err)?.ok_or("人物不存在")?;
            let mut v = voice::npc_voice(&node);
            voice::pick_voice(&mut v, id, &route.voices);
            v
        }
        None => narrator_voice(&state),
    };
    let base = route.target;
    let (sched, models) = (state.sched.clone(), state.models.clone());
    let cache = voice_cache_dir();
    tauri::async_runtime::spawn_blocking(move || voice::speak(&sched, &models, &cache, voice::DEFAULT_CACHE_MB * 1024 * 1024, &base, &text, &spec, &CancelToken::new()))
        .await
        .map_err(err)?
}

#[derive(Serialize)]
struct VoiceStatus {
    configured: bool,
    base_url: String,
    healthy: bool,
    /// 云端服务：没有健康检查（不发请求，避免计费），只表示已配置。
    cloud: bool,
    /// 没配置或配置不可用时的原因。
    problem: Option<String>,
    cache_files: usize,
    cache_mb: f64,
    queued: usize,
    narrator: chronicle_llm::tts::VoiceSpec,
}

#[tauri::command]
async fn voice_status(app: AppHandle) -> Res<VoiceStatus> {
    let state = app.state::<AppState>();
    let route = speech_route(&state);
    let (files, bytes) = voice::cache_stats(&voice_cache_dir());
    let queued = state.sched.snapshot().active.iter().filter(|t| t.kind == "voice").count();
    let narrator = narrator_voice(&state);
    let (base, cloud, problem) = match &route {
        Ok(r) => (Some(r.target.base_url.clone()), r.cloud, None),
        Err(e) => (None, false, Some(e.clone())),
    };
    let b = base.clone();
    let healthy = match (b, cloud) {
        (Some(_), true) => true,
        (Some(u), false) => tauri::async_runtime::spawn_blocking(move || chronicle_llm::tts::health(&u, std::time::Duration::from_millis(800))).await.unwrap_or(false),
        (None, _) => false,
    };
    Ok(VoiceStatus { configured: base.is_some(), base_url: base.unwrap_or_default(), healthy, cloud, problem, cache_files: files, cache_mb: bytes as f64 / 1048576.0, queued, narrator })
}

#[tauri::command]
fn voice_set_narrator(state: State<AppState>, voice: chronicle_llm::tts::VoiceSpec) -> Res<()> {
    state.lib()?.set_setting(voice::NARRATOR_KEY, &serde_json::to_string(&voice).map_err(err)?).map_err(err)
}

#[tauri::command]
fn voice_clear_cache() -> Res<usize> {
    Ok(voice::evict(&voice_cache_dir(), 0))
}

// ===================== 剧情导演与 GM 控制台（Phase 7） =====================

/// 控制台数据（只给主持人看）。
#[tauri::command]
fn gm_console(state: State<AppState>, world_id: String, campaign_id: String) -> Res<chronicle_core::console::GmConsole> {
    let w = state.world(&world_id)?;
    chronicle_core::console::console(lock(&w)?.conn(), &campaign_id).map_err(err)
}

/// 主持人手动修改游戏状态（必须写明原因，写入事件日志）。
#[tauri::command]
fn console_apply(state: State<AppState>, world_id: String, campaign_id: String, effects: Vec<chronicle_core::rules::Effect>, reason: String) -> Res<usize> {
    let w = state.world(&world_id)?;
    let mut w = lock(&w)?;
    chronicle_core::console::apply_manual(&mut w, &campaign_id, &effects, &reason).map_err(err)
}

#[tauri::command]
fn director_set_status(state: State<AppState>, world_id: String, campaign_id: String, note_id: String, suggestion_id: String, status: String) -> Res<()> {
    let w = state.world(&world_id)?;
    let mut w = lock(&w)?;
    chronicle_core::director::set_suggestion_status(&mut w, &campaign_id, &note_id, &suggestion_id, &status).map_err(err)
}

#[tauri::command]
fn director_set_settings(state: State<AppState>, world_id: String, campaign_id: String, settings: chronicle_core::director::DirectorSettings) -> Res<()> {
    let w = state.world(&world_id)?;
    let mut w = lock(&w)?;
    chronicle_core::director::set_settings(&mut w, &campaign_id, &settings).map_err(err)
}

/// 导演用"分析"任务的模型服务（没有单独设置时用 GM 叙述的服务），低优先级排队，不阻塞 GM 回复。
/// 自动分析可以被抢占：玩家的下一回合到来时让路，之后再重新分析（实测：两路请求同时跑会让 GM 回复变慢）。
fn director_client(state: &AppState, preemptible: bool) -> Res<(ProviderConfig, ScheduledClient, chronicle_gm::GmOptions)> {
    let (cfg, client, opts) = task_provider(state, "analysis").or_else(|_| task_provider(state, "narration"))?;
    let client = state.scheduled(client, &cfg, Priority::P3, "director").preemptible(preemptible);
    Ok((cfg, client, opts))
}

/// 主持人要求导演立即分析。
#[tauri::command]
async fn director_analyze(app: AppHandle, world_id: String, campaign_id: String) -> Res<chronicle_core::director::DirectorNote> {
    let state = app.state::<AppState>();
    let (_cfg, client, opts) = director_client(&state, false)?;
    let world = state.world(&world_id)?;
    let templates = state.templates.clone();
    tauri::async_runtime::spawn_blocking(move || chronicle_gm::director::analyze(&world, &client, &templates, &opts, &campaign_id, "manual", &CancelToken::new()))
        .await
        .map_err(err)?
        .map_err(|e| e.to_string())
}

#[derive(Clone, Serialize)]
struct DirectorEvent {
    world_id: String,
    campaign_id: String,
    note_id: String,
    trigger: String,
    adopted: usize,
}

/// 回合结束后：停滞、重复或到了例行间隔时，在后台分析（低优先级，不阻塞 GM 回复），完成后通知界面。
fn maybe_run_director(app: &AppHandle, world_id: &str, campaign_id: &str) {
    let state = app.state::<AppState>();
    let Ok(world) = state.world(world_id) else { return };
    let trigger = match world.lock().ok().and_then(|w| chronicle_core::director::should_analyze(w.conn(), campaign_id).ok().flatten()) {
        Some(t) => t,
        None => return,
    };
    let Ok((_cfg, client, opts)) = director_client(&state, true) else { return };
    let templates = state.templates.clone();
    let (app, world_id, campaign_id) = (app.clone(), world_id.to_string(), campaign_id.to_string());
    std::thread::spawn(move || match chronicle_gm::director::analyze(&world, &client, &templates, &opts, &campaign_id, &trigger, &CancelToken::new()) {
        Ok(note) => {
            let adopted = note.suggestions.iter().filter(|s| s.status == "adopted").count();
            let _ = app.emit("director-note", DirectorEvent { world_id, campaign_id, note_id: note.id, trigger, adopted });
        }
        // 被前台任务抢占：不算错误，下一回合结束后会重新分析（停滞状态还在）。
        Err(chronicle_gm::GmError::Cancelled) => applog::info(&format!("导演分析让路给 GM 回合（战役 {campaign_id}），稍后重新分析")),
        Err(e) => applog::error(&format!("导演分析失败（战役 {campaign_id}）：{e}")),
    });
}

// ===================== 战斗、物品与成长（Phase 6） =====================

#[derive(Serialize)]
struct MerchantView {
    id: String,
    name: String,
    shop: chronicle_core::rpg::Shop,
}

#[derive(Serialize)]
struct RpgOverview {
    sheet: chronicle_core::rpg::Sheet,
    ac: i64,
    weapon: String,
    rules: chronicle_core::rpg::RuleSet,
    money: i64,
    money_text: String,
    inventory: Vec<chronicle_core::rpg::InvItem>,
    equipment: std::collections::BTreeMap<String, String>,
    growth: Vec<chronicle_core::rpg::GrowthEntry>,
    recipes: Vec<(chronicle_core::rpg::ItemInfo, bool)>,
    merchants: Vec<MerchantView>,
    combat: Option<chronicle_core::combat::Combat>,
    combats: Vec<chronicle_core::combat::Combat>,
    /// 每件物品卖给每个商人的价格：商人 ID → 物品 ID → 价格。
    sell_prices: std::collections::BTreeMap<String, std::collections::BTreeMap<String, i64>>,
    next_level_xp: i64,
    /// 配方材料的名称：物品 ID → 名称。
    item_names: std::collections::BTreeMap<String, String>,
}

/// 角色页：角色卡、背包与装备、金钱、成长记录、配方、商人、战斗。
#[tauri::command]
fn rpg_overview(state: State<AppState>, world_id: String, campaign_id: String, actor: Option<String>) -> Res<RpgOverview> {
    use chronicle_core::{combat, rpg};
    let w = state.world(&world_id)?;
    let w = lock(&w)?;
    let conn = w.conn();
    let actor = actor.unwrap_or_else(|| rpg::PARTY.into());
    let sheet = rpg::sheet(conn, &campaign_id, &actor).map_err(err)?;
    let r = rpg::rules(conn, &campaign_id);
    let money = rpg::money(conn, &campaign_id, &actor);
    let inventory = rpg::inventory(conn, &campaign_id, &actor).map_err(err)?;
    let mut merchants = vec![];
    let mut sell_prices = std::collections::BTreeMap::new();
    let ids: Vec<String> = {
        let mut stmt = conn.prepare("SELECT id FROM nodes WHERE kind = 'character' AND deleted_at IS NULL AND instr(payload, '\"shop\"') > 0 LIMIT 50").map_err(|e| e.to_string())?;
        stmt.query_map([], |r| r.get(0)).map_err(|e| e.to_string())?.filter_map(|r| r.ok()).collect()
    };
    for id in ids {
        if let Some(shop) = rpg::shop(conn, &campaign_id, &id).map_err(err)? {
            sell_prices.insert(id.clone(), inventory.iter().map(|i| (i.item.id.clone(), rpg::sell_price(&i.item, shop.buy_rate))).collect());
            merchants.push(MerchantView { id, name: shop.name.clone(), shop });
        }
    }
    let recipes = rpg::recipes(conn, &campaign_id, &actor).map_err(err)?;
    let mut item_names = std::collections::BTreeMap::new();
    for (it, _) in &recipes {
        for i in it.recipe.iter().flat_map(|r| r.inputs.iter()) {
            if let Some(x) = rpg::item_info(conn, &i.item).map_err(err)? {
                item_names.insert(i.item.clone(), x.title);
            }
        }
    }
    Ok(RpgOverview {
        item_names,
        ac: rpg::armor_class(conn, &campaign_id, &sheet),
        weapon: rpg::weapon(conn, &campaign_id, &sheet).0,
        next_level_xp: rpg::xp_for_level(&r, sheet.level + 1),
        money_text: rpg::format_money(&r, money),
        money,
        equipment: rpg::equipment(conn, &campaign_id, &actor),
        growth: rpg::growth_log(conn, &campaign_id, None, 100).map_err(err)?,
        recipes,
        combat: combat::active(conn, &campaign_id),
        combats: combat::history(conn, &campaign_id, 10).map_err(err)?,
        inventory,
        merchants,
        sell_prices,
        rules: r,
        sheet,
    })
}

#[tauri::command]
fn rpg_save_sheet(state: State<AppState>, world_id: String, campaign_id: String, sheet: chronicle_core::rpg::Sheet) -> Res<()> {
    let w = state.world(&world_id)?;
    let mut w = lock(&w)?;
    chronicle_core::campaign::ensure_active(w.conn(), &campaign_id).map_err(err)?;
    chronicle_core::rpg::save_sheet(&mut w, &campaign_id, &sheet, "在角色页修改角色卡").map_err(err)
}

/// 角色页的各种操作：加点、装备、使用、买卖、制作、主持人调整金钱与经验。
#[derive(Deserialize)]
struct RpgActionInput {
    kind: String,
    #[serde(default)]
    target: String,
    #[serde(default)]
    item: String,
    #[serde(default)]
    qty: i64,
    #[serde(default)]
    reason: String,
    /// 以物易物时换来的东西。
    #[serde(default)]
    item2: String,
    #[serde(default)]
    qty2: i64,
}

#[tauri::command]
fn rpg_action(state: State<AppState>, world_id: String, campaign_id: String, input: RpgActionInput) -> Res<String> {
    use chronicle_core::rpg;
    let RpgActionInput { kind, target, item, qty, reason, item2, qty2 } = input;
    let w = state.world(&world_id)?;
    let mut w = lock(&w)?;
    chronicle_core::campaign::ensure_active(w.conn(), &campaign_id).map_err(err)?;
    let p = rpg::PARTY;
    let res = match kind.as_str() {
        "spend_skill" => rpg::spend_skill(&mut w, &campaign_id, p, &target).map(|_| format!("「{target}」+1")),
        "spend_attr" => rpg::spend_attr(&mut w, &campaign_id, p, &target).map(|_| "属性 +1".to_string()),
        "equip" => rpg::equip(&mut w, &campaign_id, p, &item),
        "unequip" => rpg::unequip(&mut w, &campaign_id, p, &target).map(|_| "已卸下".to_string()),
        "use" => rpg::use_item(&mut w, &campaign_id, p, &item),
        "buy" => rpg::buy(&mut w, &campaign_id, &target, &item, qty),
        "sell" => rpg::sell(&mut w, &campaign_id, &target, &item, qty),
        "craft" => rpg::craft(&mut w, &campaign_id, p, &item).map(|c| c.text),
        "barter" => rpg::barter(&mut w, &campaign_id, &target, &[(item, qty.max(1))], &[(item2, qty2.max(1))]),
        "money" => rpg::change_money(&mut w, &campaign_id, if target.is_empty() { p } else { &target }, qty, if reason.is_empty() { "主持人调整" } else { &reason }).map(|m| format!("现有 {m}")),
        "xp" => rpg::award_xp(&mut w, &campaign_id, if target.is_empty() { p } else { &target }, qty, if reason.is_empty() { "主持人奖励" } else { &reason }).map(|n| n.join("；")),
        "give" => rpg::give_item(&mut w, &campaign_id, if target.is_empty() { p } else { &target }, &item, qty, if reason.is_empty() { "主持人给予" } else { &reason }).map(|_| "已给予".to_string()),
        other => return Err(format!("未知的操作：{other}")),
    };
    res.map_err(err)
}

#[tauri::command]
fn rpg_set_rules(state: State<AppState>, world_id: String, campaign_id: String, rules: chronicle_core::rpg::RuleSet) -> Res<()> {
    let w = state.world(&world_id)?;
    let mut w = lock(&w)?;
    chronicle_core::campaign::ensure_active(w.conn(), &campaign_id).map_err(err)?;
    chronicle_core::rpg::set_rules(&mut w, &campaign_id, &rules).map_err(err)
}

#[tauri::command]
fn combat_start(state: State<AppState>, world_id: String, campaign_id: String, enemies: Vec<chronicle_core::combat::EnemySpec>, allies: Vec<String>, reason: String) -> Res<chronicle_core::combat::CombatReport> {
    let w = state.world(&world_id)?;
    let mut w = lock(&w)?;
    chronicle_core::combat::start(&mut w, &campaign_id, &enemies, &allies, if reason.is_empty() { "遭遇战" } else { &reason }).map_err(err)
}

#[tauri::command]
fn combat_act(state: State<AppState>, world_id: String, campaign_id: String, action: chronicle_core::combat::CombatAction) -> Res<chronicle_core::combat::CombatReport> {
    let w = state.world(&world_id)?;
    let mut w = lock(&w)?;
    chronicle_core::combat::act(&mut w, &campaign_id, &action).map_err(err)
}

#[tauri::command]
fn combat_abort(state: State<AppState>, world_id: String, campaign_id: String, reason: String) -> Res<chronicle_core::combat::CombatReport> {
    let w = state.world(&world_id)?;
    let mut w = lock(&w)?;
    chronicle_core::combat::abort(&mut w, &campaign_id, if reason.is_empty() { "主持人中止" } else { &reason }).map_err(err)
}

// ===================== 世界模拟（Phase 5） =====================

#[derive(Serialize)]
struct WorldSimOverview {
    environment: chronicle_core::worldsim::Environment,
    map: chronicle_core::worldsim::WorldMap,
    reachable: Vec<chronicle_core::worldsim::Reachable>,
    factions: Vec<chronicle_core::worldsim::FactionState>,
    events: Vec<chronicle_core::worldsim::WorldEvent>,
}

/// 世界页：环境、地图（玩家视角不含未知地点）、附近地点、势力（仅主持人视角）、世界事件。
#[tauri::command]
fn world_sim_overview(state: State<AppState>, world_id: String, campaign_id: String, gm_view: bool) -> Res<WorldSimOverview> {
    use chronicle_core::worldsim as ws;
    let w = state.world(&world_id)?;
    let w = lock(&w)?;
    let conn = w.conn();
    let mut reachable = ws::reachable(conn, &campaign_id, 30).map_err(err)?;
    if !gm_view {
        reachable.retain(|r| r.state != "unknown");
    }
    Ok(WorldSimOverview {
        environment: ws::environment(conn, &campaign_id).map_err(err)?,
        map: ws::map(conn, &campaign_id, gm_view).map_err(err)?,
        reachable,
        factions: if gm_view { ws::factions(conn, &campaign_id).map_err(err)? } else { vec![] },
        events: ws::world_events(conn, &campaign_id, 100).map_err(err)?,
    })
}

#[tauri::command]
fn plan_travel(state: State<AppState>, world_id: String, campaign_id: String, to: String) -> Res<chronicle_core::worldsim::TravelPlan> {
    let w = state.world(&world_id)?;
    chronicle_core::worldsim::plan_travel(lock(&w)?.conn(), &campaign_id, &to).map_err(err)
}

#[derive(Serialize)]
struct TravelDone {
    plan: chronicle_core::worldsim::TravelPlan,
    notes: Vec<String>,
}

/// 玩家在世界页直接旅行：按路程推进时间，期间世界照常运转，任务按条件推进。
#[tauri::command]
fn travel_to(state: State<AppState>, world_id: String, campaign_id: String, to: String) -> Res<TravelDone> {
    let w = state.world(&world_id)?;
    let mut w = lock(&w)?;
    let (plan, mut notes) = chronicle_core::worldsim::travel(&mut w, &campaign_id, &to, "玩家旅行").map_err(err)?;
    notes.extend(chronicle_core::quests::tick(&mut w, &campaign_id).map_err(err)?);
    Ok(TravelDone { plan, notes })
}

#[tauri::command]
fn add_map_marker(state: State<AppState>, world_id: String, campaign_id: String, place: String, label: String, note: String) -> Res<String> {
    let w = state.world(&world_id)?;
    let mut w = lock(&w)?;
    chronicle_core::campaign::ensure_active(w.conn(), &campaign_id).map_err(err)?;
    chronicle_core::worldsim::add_marker(&mut w, &campaign_id, &place, &label, &note).map_err(err)
}

#[tauri::command]
fn remove_map_marker(state: State<AppState>, world_id: String, campaign_id: String, id: String) -> Res<()> {
    let w = state.world(&world_id)?;
    let mut w = lock(&w)?;
    chronicle_core::campaign::ensure_active(w.conn(), &campaign_id).map_err(err)?;
    chronicle_core::worldsim::remove_marker(&mut w, &campaign_id, &id).map_err(err)
}

/// 主持人指定今天的天气（剧情需要时），写入事件日志。
#[tauri::command]
fn set_weather_today(state: State<AppState>, world_id: String, campaign_id: String, weather: String) -> Res<()> {
    let w = state.world(&world_id)?;
    let mut w = lock(&w)?;
    chronicle_core::campaign::ensure_active(w.conn(), &campaign_id).map_err(err)?;
    let day = chronicle_core::worldsim::environment(w.conn(), &campaign_id).map_err(err)?.day;
    let value = if weather.is_empty() { serde_json::Value::Null } else { serde_json::json!({ "day": day, "weather": weather }) };
    chronicle_core::campaign::apply_effects(
        &mut w,
        &campaign_id,
        &[chronicle_core::rules::Effect::SetVar { key: chronicle_core::worldsim::WEATHER_OVERRIDE_VAR.into(), value }],
        "主持人指定天气",
    )
    .map_err(err)?;
    Ok(())
}

#[tauri::command]
fn find_places(state: State<AppState>, world_id: String, query: String) -> Res<Vec<Node>> {
    let w = state.world(&world_id)?;
    chronicle_core::nodes::find_places(lock(&w)?.conn(), &query, 200).map_err(err)
}

// ===================== 人物关系网络与世界图谱 =====================

#[tauri::command]
fn relation_network(state: State<AppState>, world_id: String, query: chronicle_core::network::NetworkQuery) -> Res<chronicle_core::network::Network> {
    let w = state.world(&world_id)?;
    chronicle_core::network::network(lock(&w)?.conn(), &query).map_err(err)
}

#[tauri::command]
fn bond_history(state: State<AppState>, world_id: String, campaign_id: String, a: String, b: String) -> Res<Vec<chronicle_core::network::BondChange>> {
    let w = state.world(&world_id)?;
    chronicle_core::network::bond_history(lock(&w)?.conn(), &campaign_id, &a, &b).map_err(err)
}

#[tauri::command]
fn person_bonds(state: State<AppState>, world_id: String, campaign_id: String, id: String) -> Res<Vec<chronicle_core::network::BondSummary>> {
    let w = state.world(&world_id)?;
    chronicle_core::network::bonds_of_person(lock(&w)?.conn(), &campaign_id, &id).map_err(err)
}

#[tauri::command]
fn relations_among(state: State<AppState>, world_id: String, ids: Vec<String>) -> Res<Vec<Relation>> {
    let w = state.world(&world_id)?;
    chronicle_core::network::relations_among(lock(&w)?.conn(), &ids).map_err(err)
}

// ===================== 剧情回溯与多时间线 =====================

#[tauri::command]
fn end_campaign(state: State<AppState>, world_id: String, campaign_id: String, ending: String) -> Res<chronicle_core::timeline::Checkpoint> {
    let w = state.world(&world_id)?;
    chronicle_core::timeline::end_campaign(&mut *lock(&w)?, &campaign_id, &ending).map_err(err)
}

#[tauri::command]
fn timeline_fork_points(state: State<AppState>, world_id: String, campaign_id: String) -> Res<Vec<chronicle_core::timeline::ForkPoint>> {
    let w = state.world(&world_id)?;
    chronicle_core::timeline::fork_points(lock(&w)?.conn(), &campaign_id).map_err(err)
}

#[tauri::command]
fn timeline_fork(state: State<AppState>, world_id: String, campaign_id: String, checkpoint_id: i64, name: String, reseed: bool) -> Res<chronicle_core::campaign::Campaign> {
    let w = state.world(&world_id)?;
    chronicle_core::timeline::fork(&mut *lock(&w)?, &campaign_id, checkpoint_id, &name, reseed).map_err(err)
}

#[tauri::command]
fn timeline_verify(state: State<AppState>, world_id: String, campaign_id: String) -> Res<chronicle_core::timeline::VerifyReport> {
    let w = state.world(&world_id)?;
    chronicle_core::timeline::verify(lock(&w)?.conn(), &campaign_id).map_err(err)
}

#[tauri::command]
fn timeline_rename(state: State<AppState>, world_id: String, campaign_id: String, name: String) -> Res<()> {
    let w = state.world(&world_id)?;
    chronicle_core::timeline::rename(lock(&w)?.conn(), &campaign_id, &name).map_err(err)
}

#[tauri::command]
fn timeline_archive(state: State<AppState>, world_id: String, campaign_id: String, archived: bool) -> Res<()> {
    let w = state.world(&world_id)?;
    chronicle_core::timeline::set_archived(lock(&w)?.conn(), &campaign_id, archived).map_err(err)
}

#[tauri::command]
fn timeline_compare(state: State<AppState>, world_id: String, campaign_ids: Vec<String>) -> Res<chronicle_core::timeline::TimelineComparison> {
    if campaign_ids.len() < 2 {
        return Err("至少选择两条时间线".into());
    }
    let w = state.world(&world_id)?;
    chronicle_core::timeline::compare(lock(&w)?.conn(), &campaign_ids).map_err(err)
}

// ===================== 节点图片 =====================

#[tauri::command]
fn node_images(state: State<AppState>, world_id: String, node_id: String) -> Res<Vec<chronicle_core::images::NodeImage>> {
    let w = state.world(&world_id)?;
    chronicle_core::images::list_images(lock(&w)?.conn(), &node_id).map_err(err)
}

/// 添加图片：`data` 是 base64（界面已缩放到长边不超过 2048 像素）。格式按文件头判断。
#[tauri::command]
fn node_image_add(state: State<AppState>, world_id: String, node_id: String, data: String, width: i64, height: i64, caption: String) -> Res<chronicle_core::images::NodeImage> {
    use base64::Engine;
    let b64 = data.split_once(";base64,").map(|(_, b)| b).unwrap_or(&data);
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|_| "图片数据无法解码".to_string())?;
    let w = state.world(&world_id)?;
    chronicle_core::images::add_image(lock(&w)?.conn(), &node_id, &bytes, width, height, &caption).map_err(err)
}

/// 图片的 data URL（显示用）。
#[tauri::command]
fn node_image_data(state: State<AppState>, world_id: String, image_id: String) -> Res<String> {
    let w = state.world(&world_id)?;
    chronicle_core::images::data_url(lock(&w)?.conn(), &image_id).map_err(err)
}

#[tauri::command]
fn node_image_caption(state: State<AppState>, world_id: String, image_id: String, caption: String) -> Res<()> {
    let w = state.world(&world_id)?;
    chronicle_core::images::set_caption(lock(&w)?.conn(), &image_id, &caption).map_err(err)
}

#[tauri::command]
fn node_image_cover(state: State<AppState>, world_id: String, image_id: String) -> Res<()> {
    let w = state.world(&world_id)?;
    chronicle_core::images::set_cover(lock(&w)?.conn(), &image_id).map_err(err)
}

#[tauri::command]
fn node_image_delete(state: State<AppState>, world_id: String, image_id: String) -> Res<()> {
    let w = state.world(&world_id)?;
    chronicle_core::images::delete_image(lock(&w)?.conn(), &image_id).map_err(err)
}

#[tauri::command]
fn node_covers(state: State<AppState>, world_id: String, node_ids: Vec<String>) -> Res<Vec<(String, String)>> {
    let w = state.world(&world_id)?;
    chronicle_core::images::covers(lock(&w)?.conn(), &node_ids).map_err(err)
}

/// 封面缩略图（没有图片的节点不在结果里；`thumb` 为空表示还没生成）。
#[tauri::command]
fn node_cover_thumbs(state: State<AppState>, world_id: String, node_ids: Vec<String>) -> Res<Vec<chronicle_core::images::ImageThumb>> {
    let w = state.world(&world_id)?;
    chronicle_core::images::cover_thumbs(lock(&w)?.conn(), &node_ids).map_err(err)
}

#[tauri::command]
fn node_image_thumbs(state: State<AppState>, world_id: String, node_id: String) -> Res<Vec<chronicle_core::images::ImageThumb>> {
    let w = state.world(&world_id)?;
    chronicle_core::images::node_thumbs(lock(&w)?.conn(), &node_id).map_err(err)
}

/// 保存界面生成的缩略图（data URL 或 base64）。
#[tauri::command]
fn node_image_set_thumb(state: State<AppState>, world_id: String, image_id: String, data: String) -> Res<()> {
    use base64::Engine;
    let b64 = data.split_once(";base64,").map(|(_, b)| b).unwrap_or(&data);
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|_| "缩略图数据无法解码".to_string())?;
    let w = state.world(&world_id)?;
    chronicle_core::images::set_thumb(lock(&w)?.conn(), &image_id, &bytes).map_err(err)
}

#[tauri::command]
fn illustrated_nodes(state: State<AppState>, world_id: String) -> Res<Vec<chronicle_core::images::IllustratedNode>> {
    let w = state.world(&world_id)?;
    chronicle_core::images::illustrated_nodes(lock(&w)?.conn()).map_err(err)
}

/// 游戏页「此刻的画面」：所在地点与在场人物，以及它们的封面。
#[tauri::command]
fn scene_view(state: State<AppState>, world_id: String, campaign_id: String) -> Res<chronicle_gm::context::SceneView> {
    let w = state.world(&world_id)?;
    chronicle_gm::context::scene_view(&*lock(&w)?, &campaign_id).map_err(err)
}

/// 让视觉模型描述一张节点图片（只返回文字，由用户决定是否填进节点；不写任何数据）。
#[tauri::command]
async fn describe_node_image(app: AppHandle, world_id: String, image_id: String) -> Res<String> {
    let state = app.state::<AppState>();
    let (cfg, client) = gen_provider(&state)?;
    if !cfg.capabilities.as_ref().is_some_and(|c| c.vision) {
        return Err("当前模型服务没有通过「看图」测试（或还没测过）。请到「模型服务」页点「重新测试」".into());
    }
    let w = state.world(&world_id)?;
    let (url, title, kind) = {
        let g = lock(&w)?;
        let img = chronicle_core::images::get_image(g.conn(), &image_id).map_err(err)?;
        let node = chronicle_core::nodes::get_node(g.conn(), &img.node_id).map_err(err)?.ok_or("节点不存在")?;
        (chronicle_core::images::data_url(g.conn(), &image_id).map_err(err)?, node.title, node.kind.to_string())
    };
    let thinking = (cfg.thinking == chronicle_llm::ThinkingMode::Off).then_some(false);
    let client = state.scheduled(client, &cfg, Priority::P1, "vision");
    let prompt = format!(
        "这是 TRPG 世界设定里「{title}」（类型：{kind}）的图片。请用简体中文写一段 150–300 字的描述，可直接用作设定：
         - 人物：外貌、年龄感、服饰、神态、随身物品；
- 地点：建筑与地貌、气候与氛围、显眼的细节；
- 地图：能看清的地名、方位与相对位置；
- 物品：外形、材质、纹饰。
         只写图片里看得到的，看不清的不要编造。不要用 Markdown 标题。"
    );
    let req = chronicle_llm::ChatRequest {
        messages: vec![chronicle_llm::ChatMessage::user_with_images(prompt, vec![url])],
        temperature: Some(0.3),
        max_tokens: Some(1200),
        format: chronicle_llm::OutputFormat::Text,
        stream: false,
        thinking,
    };
    let res = tauri::async_runtime::spawn_blocking(move || chronicle_llm::LlmClient::chat(&client, &req, &chronicle_llm::CancelToken::new(), &mut |_: usize| {}))
        .await
        .map_err(err)?
        .map_err(err)?;
    // 模型有时仍会加 Markdown 标题和加粗（实测："# 雾港"）：去掉，留下可以直接填进设定的纯文字。
    let text = chronicle_llm::strip_thinking(&res.content)
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .map(|l| l.replace("**", ""))
        .collect::<Vec<_>>()
        .join("
")
        .trim()
        .to_string();
    if text.is_empty() {
        return Err("模型没有返回描述".into());
    }
    Ok(text)
}

// ===================== 战役档案馆 =====================

#[tauri::command]
fn archive_list(state: State<AppState>, world_id: String) -> Res<Vec<chronicle_core::archive::ArchiveEntry>> {
    let w = state.world(&world_id)?;
    chronicle_core::archive::archive_list(lock(&w)?.conn()).map_err(err)
}

#[tauri::command]
fn archive_stats(state: State<AppState>, world_id: String, campaign_id: String) -> Res<chronicle_core::archive::CampaignStats> {
    let w = state.world(&world_id)?;
    chronicle_core::archive::campaign_stats(lock(&w)?.conn(), &campaign_id).map_err(err)
}

#[tauri::command]
fn archive_endings(state: State<AppState>, world_id: String, script_id: String) -> Res<chronicle_core::archive::EndingDistribution> {
    let w = state.world(&world_id)?;
    chronicle_core::archive::ending_distribution(lock(&w)?.conn(), &script_id).map_err(err)
}

#[tauri::command]
fn archive_chapters(state: State<AppState>, world_id: String, campaign_id: String) -> Res<Vec<chronicle_core::archive::Chapter>> {
    let w = state.world(&world_id)?;
    chronicle_core::archive::chapters(lock(&w)?.conn(), &campaign_id).map_err(err)
}

#[tauri::command]
fn archive_mark_chapter(state: State<AppState>, world_id: String, campaign_id: String, title: String) -> Res<i64> {
    let w = state.world(&world_id)?;
    chronicle_core::archive::mark_chapter(lock(&w)?.conn(), &campaign_id, &title).map_err(err)
}

#[tauri::command]
fn archive_rename_chapter(state: State<AppState>, world_id: String, campaign_id: String, seq: i64, title: String) -> Res<()> {
    let w = state.world(&world_id)?;
    chronicle_core::archive::rename_chapter(lock(&w)?.conn(), &campaign_id, seq, &title).map_err(err)
}

#[tauri::command]
fn archive_remove_chapter(state: State<AppState>, world_id: String, campaign_id: String, seq: i64) -> Res<()> {
    let w = state.world(&world_id)?;
    chronicle_core::archive::remove_chapter(lock(&w)?.conn(), &campaign_id, seq).map_err(err)
}

#[tauri::command]
fn archive_chronicle(state: State<AppState>, world_id: String, campaign_id: String) -> Res<chronicle_core::archive::Chronicle> {
    let w = state.world(&world_id)?;
    chronicle_core::archive::chronicle(lock(&w)?.conn(), &campaign_id).map_err(err)
}

/// 把编年史保存为 Markdown 文件。
#[tauri::command]
fn archive_save_chronicle(state: State<AppState>, world_id: String, campaign_id: String, path: String) -> Res<()> {
    let w = state.world(&world_id)?;
    let c = chronicle_core::archive::chronicle(lock(&w)?.conn(), &campaign_id).map_err(err)?;
    std::fs::write(&path, c.markdown).map_err(err)
}

#[tauri::command]
fn archive_search(state: State<AppState>, world_id: String, filter: chronicle_core::archive::SearchFilter) -> Res<Vec<chronicle_core::archive::EventHit>> {
    let w = state.world(&world_id)?;
    chronicle_core::archive::search_events(lock(&w)?.conn(), &filter).map_err(err)
}

/// 导出战役（可连同后代时间线）为 JSON 文件。返回导出的时间线数量。
#[tauri::command]
fn archive_export(state: State<AppState>, world_id: String, campaign_id: String, path: String, with_descendants: bool) -> Res<usize> {
    let w = state.world(&world_id)?;
    let ex = chronicle_core::archive::export_campaign(lock(&w)?.conn(), &campaign_id, with_descendants).map_err(err)?;
    let n = ex.campaigns.len();
    let text = serde_json::to_string_pretty(&ex).map_err(err)?;
    // 先写临时文件再改名，避免写到一半留下损坏的文件。
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, text).map_err(err)?;
    std::fs::rename(&tmp, &path).map_err(err)?;
    Ok(n)
}

/// 导入战役 JSON（全部换新 ID，不覆盖现有战役）。导入前先做一次备份。
#[tauri::command]
fn archive_import(state: State<AppState>, world_id: String, path: String) -> Res<chronicle_core::archive::ImportResult> {
    let text = std::fs::read_to_string(&path).map_err(err)?;
    let data: chronicle_core::archive::CampaignExport = serde_json::from_str(&text).map_err(|e| format!("无法解析战役文件：{e}"))?;
    let w = state.world(&world_id)?;
    let mut g = lock(&w)?;
    chronicle_core::backup::auto_backup(&g, AUTO_BACKUP_KEEP, 0).map_err(err)?;
    chronicle_core::archive::import_campaign(&mut g, &data).map_err(err)
}

#[tauri::command]
fn archive_check(state: State<AppState>, world_id: String) -> Res<Vec<chronicle_core::archive::CampaignCheck>> {
    let w = state.world(&world_id)?;
    chronicle_core::archive::check_campaigns(lock(&w)?.conn()).map_err(err)
}

// ===================== AI GM =====================

/// 选择某个任务使用的模型服务（检查隐私开关），并给出 GM 调用参数。
fn task_provider(state: &AppState, task: &str) -> Res<(ProviderConfig, OpenAiClient, chronicle_gm::GmOptions)> {
    let id = state.lib()?.route_for(task).map_err(err)?.ok_or("还没有设置模型服务，请先到「模型服务」页面配置")?;
    let cfg = state.provider(&id)?;
    if !cfg.is_loopback() && !cfg.allow_campaign_content {
        return Err(format!("「{}」不是本机服务，且尚未允许发送战役内容。请在「模型服务」中确认后打开该开关", cfg.name));
    }
    let client = state.client(&cfg)?;
    let caps = cfg.capabilities.clone().unwrap_or_default();
    let ctx = caps.context_length.unwrap_or(8192) as usize;
    let opts = chronicle_gm::GmOptions {
        json_schema: caps.json_schema,
        thinking: (cfg.thinking == chronicle_llm::ThinkingMode::Off).then_some(false),
        temperature: 0.8,
        max_tokens: (ctx / 2).clamp(1024, 16000) as u32,
        context_budget: (ctx / 3).clamp(1500, 8000),
    };
    Ok((cfg, client, opts))
}

fn log_requests(state: &AppState, cfg: &ProviderConfig, task: &str, logs: &[chronicle_gen::RequestLog]) {
    if let Ok(lib) = state.lib() {
        for l in logs {
            let _ = lib.log_request(&LlmRequestRecord {
                provider_id: cfg.id.clone(),
                model: cfg.model.clone(),
                task: task.into(),
                job_id: None,
                ok: l.ok,
                attempts: l.attempts as i64,
                prompt_tokens: l.prompt_tokens.map(|v| v as i64),
                completion_tokens: l.completion_tokens.map(|v| v as i64),
                latency_ms: l.latency_ms.map(|v| v as i64),
                error: l.error.clone(),
            });
        }
    }
}

/// 执行一个 GM 回合（耗时较长，在后台线程中运行；调用模型期间不占用世界文件）。
#[tauri::command]
async fn gm_turn(app: AppHandle, world_id: String, campaign_id: String, input: String) -> Res<chronicle_gm::TurnResult> {
    let state = app.state::<AppState>();
    let (gcfg, gm, opts) = task_provider(&state, "narration")?;
    let (npc_cfg, npc, _) = task_provider(&state, "npc")?;
    let gm = state.scheduled(gm, &gcfg, Priority::P0, "gm");
    let npc = state.scheduled(npc, &npc_cfg, Priority::P1, "npc");
    let world = state.world(&world_id)?;
    let templates = state.templates.clone();
    let cancel = CancelToken::new();
    state.running.lock().map_err(err)?.insert(format!("gm:{campaign_id}"), cancel.clone());
    let cid = campaign_id.clone();
    let app2 = app.clone();
    let res = tauri::async_runtime::spawn_blocking(move || {
        let engine = chronicle_gm::Engine { gm: &gm, npc: &npc, templates: &templates, opts: &opts };
        // 回合阶段推送给界面（GM 玻璃球下方显示"理解你的行动 / 程序判定 / 组织叙述 / NPC 回应"）。
        let stage = |s: &str| {
            let _ = app2.emit("gm-stage", serde_json::json!({ "campaign_id": cid, "stage": s }));
        };
        chronicle_gm::run_turn_with_stages(&world, &engine, &cid, &input, &cancel, &stage)
    })
        .await
        .map_err(err)?;
    if let Ok(mut r) = state.running.lock() {
        r.remove(&format!("gm:{campaign_id}"));
    }
    match res {
        Ok(r) => {
            // GM 与 NPC 的请求一起记在 GM 叙述的服务名下。
            log_requests(&state, &gcfg, "narration", &r.requests);
            maybe_run_director(&app, &world_id, &campaign_id);
            Ok(r)
        }
        Err(e) => {
            if !matches!(e, chronicle_gm::GmError::Cancelled | chronicle_gm::GmError::Input(_)) {
                applog::error(&format!("GM 回合失败（战役 {campaign_id}）：{e}"));
            }
            Err(e.to_string())
        }
    }
}

#[tauri::command]
fn cancel_gm_turn(state: State<AppState>, campaign_id: String) -> Res<bool> {
    Ok(match state.running.lock().map_err(err)?.get(&format!("gm:{campaign_id}")) {
        Some(c) => {
            c.cancel();
            true
        }
        None => false,
    })
}

#[tauri::command]
fn campaign_log(state: State<AppState>, world_id: String, campaign_id: String, after_seq: i64) -> Res<Vec<chronicle_core::campaign::EventEntry>> {
    let w = state.world(&world_id)?;
    chronicle_core::campaign::events(lock(&w)?.conn(), &campaign_id, after_seq, 5000).map_err(err)
}

#[derive(Serialize)]
struct CampaignMemory {
    summaries: Vec<chronicle_core::campaign::SessionSummary>,
    facts: Vec<String>,
    entities: Vec<chronicle_core::campaign::CampaignEntity>,
}

#[tauri::command]
fn campaign_memory(state: State<AppState>, world_id: String, campaign_id: String) -> Res<CampaignMemory> {
    let w = state.world(&world_id)?;
    let w = lock(&w)?;
    Ok(CampaignMemory {
        summaries: chronicle_core::campaign::summaries(w.conn(), &campaign_id).map_err(err)?,
        facts: chronicle_core::campaign::facts(w.conn(), &campaign_id, 100).map_err(err)?,
        entities: chronicle_core::campaign::campaign_entities(w.conn(), &campaign_id).map_err(err)?,
    })
}

#[tauri::command]
async fn gm_summarize(app: AppHandle, world_id: String, campaign_id: String) -> Res<chronicle_core::campaign::SessionSummary> {
    let state = app.state::<AppState>();
    let (cfg, client, opts) = task_provider(&state, "summary")?;
    let client = state.scheduled(client, &cfg, Priority::P4, "summary");
    let world = state.world(&world_id)?;
    let templates = state.templates.clone();
    tauri::async_runtime::spawn_blocking(move || chronicle_gm::summarize(&world, &client, &templates, &opts, &campaign_id, &CancelToken::new()))
        .await
        .map_err(err)?
        .map_err(err)
}

#[tauri::command]
fn confirm_summary(state: State<AppState>, world_id: String, summary_id: String, text: Option<String>) -> Res<()> {
    let w = state.world(&world_id)?;
    chronicle_core::campaign::confirm_summary(lock(&w)?.conn(), &summary_id, text.as_deref()).map_err(err)
}

// ===================== 变更集（审阅台） =====================

#[tauri::command]
fn list_changesets(state: State<AppState>, world_id: String, status: Option<ChangesetStatus>) -> Res<Vec<Changeset>> {
    let w = state.world(&world_id)?;
    changeset::list_changesets(lock(&w)?.conn(), status).map_err(err)
}

#[tauri::command]
fn apply_changeset(state: State<AppState>, world_id: String, id: String, accepted: Option<Vec<usize>>) -> Res<Changeset> {
    let w = state.world(&world_id)?;
    changeset::apply(&mut *lock(&w)?, &id, accepted, &Author::User).map_err(err)
}

#[tauri::command]
fn reject_changeset(state: State<AppState>, world_id: String, id: String) -> Res<()> {
    let w = state.world(&world_id)?;
    changeset::reject(&mut *lock(&w)?, &id).map_err(err)
}

#[tauri::command]
fn revise_changeset_op(state: State<AppState>, world_id: String, id: String, index: usize, op: Op) -> Res<Changeset> {
    let w = state.world(&world_id)?;
    changeset::revise_op(&mut *lock(&w)?, &id, index, op).map_err(err)
}

// ===================== 生成任务 =====================

#[derive(Clone, Serialize)]
struct ProgressEvent {
    world_id: String,
    job_id: String,
    chars: usize,
}

#[derive(Clone, Serialize)]
struct FinishedEvent {
    world_id: String,
    job_id: String,
    ok: bool,
    error: Option<String>,
    changeset_id: Option<String>,
}

const GEN_TASK_ROUTE: &str = "script_gen";

/// 选择用于剧本生成的模型服务，并检查隐私开关。
fn gen_provider(state: &AppState) -> Res<(ProviderConfig, OpenAiClient)> {
    let id = state.lib()?.route_for(GEN_TASK_ROUTE).map_err(err)?.ok_or("还没有设置用于剧本生成的模型服务，请先到「模型服务」页面配置")?;
    let cfg = state.provider(&id)?;
    if !cfg.is_loopback() && !cfg.allow_campaign_content {
        return Err(format!(
            "「{}」不是本机服务，且尚未允许发送世界与剧本内容。请在「模型服务」中确认后打开该开关",
            cfg.name
        ));
    }
    let client = state.client(&cfg)?;
    Ok((cfg, client))
}

/// 生成任务去重键：同一目标的同类生成。
fn gen_key(world_id: &str, task: &Task) -> String {
    format!("{world_id}|{}|{}", task.kind(), task.target())
}

/// 占用去重键；已有同类任务在进行时拒绝。
fn claim_gen(state: &AppState, key: &str) -> Res<()> {
    let mut g = state.gen_keys.lock().map_err(err)?;
    if !g.insert(key.to_string()) {
        return Err("相同的生成任务已经在排队或进行中".into());
    }
    Ok(())
}

fn release_gen(keys: &Arc<Mutex<HashSet<String>>>, key: &str) {
    if let Ok(mut g) = keys.lock() {
        g.remove(key);
    }
}

// 参数分别是任务运行所需的独立部件，合成结构体只会多一层拆装。
#[allow(clippy::too_many_arguments)]
fn spawn_job(app: AppHandle, state: &AppState, world_id: String, prepared: runner::Prepared, cfg: ProviderConfig, client: OpenAiClient, opts: GenOptions, key: Option<String>) -> Res<String> {
    let job_id = prepared.job_id.clone();
    let cancel = CancelToken::new();
    state.running.lock().map_err(err)?.insert(job_id.clone(), cancel.clone());
    let world = state.world(&world_id)?;
    let kind = format!("gen.{}", prepared.task.kind());
    let client = state.scheduled(client, &cfg, Priority::P5, &kind).label(prepared.task.target().chars().take(40).collect::<String>());
    let keys = state.gen_keys.clone();
    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        let mut last = Instant::now() - Duration::from_secs(1);
        let (res, logs) = runner::call_model(&client, &prepared, &opts, &cancel, &mut |chars| {
            if last.elapsed() > Duration::from_millis(250) {
                last = Instant::now();
                let _ = app.emit("gen-progress", ProgressEvent { world_id: world_id.clone(), job_id: prepared.job_id.clone(), chars });
            }
        });
        let outcome: Result<Changeset, String> = match lock(&world) {
            Err(e) => Err(e),
            Ok(mut w) => match res {
                Ok(out) => runner::finalize(&mut w, &prepared, &out).map_err(err),
                Err((e, raw)) => {
                    let _ = runner::fail(&w, &prepared.job_id, &e, raw.as_deref());
                    Err(e.to_string())
                }
            },
        };
        if let Ok(lib) = st.lib() {
            for l in &logs {
                let _ = lib.log_request(&LlmRequestRecord {
                    provider_id: cfg.id.clone(),
                    model: cfg.model.clone(),
                    task: GEN_TASK_ROUTE.into(),
                    job_id: Some(prepared.job_id.clone()),
                    ok: l.ok,
                    attempts: l.attempts as i64,
                    prompt_tokens: l.prompt_tokens.map(|v| v as i64),
                    completion_tokens: l.completion_tokens.map(|v| v as i64),
                    latency_ms: l.latency_ms.map(|v| v as i64),
                    error: l.error.clone(),
                });
            }
        }
        if let Ok(mut r) = st.running.lock() {
            r.remove(&prepared.job_id);
        }
        if let Some(k) = &key {
            release_gen(&keys, k);
        }
        let _ = app.emit(
            "gen-finished",
            FinishedEvent {
                world_id,
                job_id: prepared.job_id.clone(),
                ok: outcome.is_ok(),
                changeset_id: outcome.as_ref().ok().map(|c| c.id.clone()),
                error: outcome.err(),
            },
        );
    });
    Ok(job_id)
}

use tauri::Manager;

#[tauri::command]
fn start_generation(app: AppHandle, state: State<AppState>, world_id: String, task: Task, images: Option<Vec<String>>) -> Res<String> {
    let (cfg, client) = gen_provider(&state)?;
    let mut opts = GenOptions::for_caps(cfg.capabilities.as_ref());
    opts.thinking = (cfg.thinking == chronicle_llm::ThinkingMode::Off).then_some(false);
    let w = state.world(&world_id)?;
    let key = gen_key(&world_id, &task);
    claim_gen(&state, &key)?;
    let prepared = match chronicle_gen::prepare_with_images(&mut *lock(&w)?, task, &state.templates, cfg.capabilities.as_ref(), &opts, (&cfg.name, &cfg.model), images.unwrap_or_default()) {
        Ok(p) => p,
        Err(e) => {
            release_gen(&state.gen_keys, &key);
            return Err(err(e));
        }
    };
    spawn_job(app, &state, world_id, prepared, cfg, client, opts, Some(key))
}

/// 为剧情图中所有"死路"节点各生成后续分支（每个死路一个任务，依次排队执行）。返回任务 ID。
#[tauri::command]
fn fill_dead_ends(app: AppHandle, state: State<AppState>, world_id: String, script_id: String) -> Res<Vec<String>> {
    let (cfg, _) = gen_provider(&state)?;
    let mut opts = GenOptions::for_caps(cfg.capabilities.as_ref());
    opts.thinking = (cfg.thinking == chronicle_llm::ThinkingMode::Off).then_some(false);
    let w = state.world(&world_id)?;
    let report = chronicle_core::plot_analysis::analyze_plot(lock(&w)?.conn(), &script_id).map_err(err)?;
    let dead: Vec<String> = report.issues.iter().filter(|i| i.code == "dead_end").flat_map(|i| i.node_ids.clone()).collect();
    if dead.is_empty() {
        return Err("剧情图里没有死路节点".into());
    }
    let mut ids = vec![];
    for node in dead.into_iter().take(10) {
        let task = Task::PlotBranches { script_id: script_id.clone(), from_node_id: node, player_action: String::new(), count: 2 };
        let key = gen_key(&world_id, &task);
        if claim_gen(&state, &key).is_err() {
            continue; // 这个节点已经在生成中
        }
        let prepared = match runner::prepare(&mut *lock(&w)?, task, &state.templates, cfg.capabilities.as_ref(), &opts, (&cfg.name, &cfg.model)) {
            Ok(p) => p,
            Err(e) => {
                release_gen(&state.gen_keys, &key);
                return Err(err(e));
            }
        };
        ids.push(spawn_job(app.clone(), &state, world_id.clone(), prepared, cfg.clone(), state.client(&cfg)?, opts.clone(), Some(key))?);
    }
    Ok(ids)
}

#[tauri::command]
fn retry_job(app: AppHandle, state: State<AppState>, world_id: String, job_id: String) -> Res<String> {
    let (cfg, client) = gen_provider(&state)?;
    let mut opts = GenOptions::for_caps(cfg.capabilities.as_ref());
    opts.thinking = (cfg.thinking == chronicle_llm::ThinkingMode::Off).then_some(false);
    let w = state.world(&world_id)?;
    let prepared =
        runner::prepare_retry(&mut *lock(&w)?, &job_id, &state.templates, cfg.capabilities.as_ref(), &opts, (&cfg.name, &cfg.model))
            .map_err(err)?;
    spawn_job(app, &state, world_id, prepared, cfg, client, opts, None)
}

#[tauri::command]
fn cancel_job(state: State<AppState>, job_id: String) -> Res<bool> {
    Ok(match state.running.lock().map_err(err)?.get(&job_id) {
        Some(c) => {
            c.cancel();
            true
        }
        None => false,
    })
}

#[tauri::command]
fn list_jobs(state: State<AppState>, world_id: String) -> Res<Vec<GenJob>> {
    let w = state.world(&world_id)?;
    jobs::list_jobs(lock(&w)?.conn(), None, 100).map_err(err)
}

// ===================== 模型服务 =====================

#[derive(Serialize)]
struct ProviderView {
    config: ProviderConfig,
    has_key: bool,
    loopback: bool,
}

#[tauri::command]
fn discover_services(extra_urls: Vec<String>) -> Vec<DiscoveredService> {
    discovery::discover(&extra_urls, Duration::from_millis(600))
}

#[tauri::command]
fn scan_model_folder(folder: String) -> Res<ScanResult> {
    let p = PathBuf::from(folder.trim());
    if !p.is_dir() {
        return Err("文件夹不存在".into());
    }
    gguf::scan_folder(&p, 3).map_err(err)
}

#[tauri::command]
fn list_providers(state: State<AppState>) -> Res<Vec<ProviderView>> {
    let list = state.lib()?.providers().map_err(err)?;
    let mut out = vec![];
    for v in list {
        let config: ProviderConfig = serde_json::from_value(v).map_err(err)?;
        let has_key = config.needs_key && state.secrets.get(&config.id).ok().flatten().is_some();
        out.push(ProviderView { loopback: config.is_loopback(), config, has_key });
    }
    Ok(out)
}

#[derive(Deserialize)]
struct SaveProvider {
    config: ProviderConfig,
    /// 新的 API Key；为空表示不修改。
    api_key: Option<String>,
}

#[tauri::command]
fn save_provider(state: State<AppState>, input: SaveProvider) -> Res<ProviderConfig> {
    let mut cfg = input.config;
    if cfg.name.trim().is_empty() || cfg.base_url.trim().is_empty() || cfg.model.trim().is_empty() {
        return Err("名称、地址和模型名都不能为空".into());
    }
    if !(cfg.base_url.starts_with("http://") || cfg.base_url.starts_with("https://")) {
        return Err("地址必须以 http:// 或 https:// 开头".into());
    }
    if cfg.id.is_empty() {
        cfg.id = chronicle_core::util::new_id();
    }
    if cfg.is_loopback() && !cfg.needs_key {
        // 本机服务默认允许发送内容。
        cfg.allow_campaign_content = true;
    }
    if let Some(k) = input.api_key.filter(|k| !k.trim().is_empty()) {
        state.secrets.set(&cfg.id, k.trim())?;
        cfg.needs_key = true;
    }
    let lib = state.lib()?;
    lib.save_provider(&cfg.id, &serde_json::to_value(&cfg).map_err(err)?).map_err(err)?;
    if cfg.service == chronicle_llm::types::ServiceType::Chat && cfg.server_kind != Some(chronicle_llm::types::ServerKind::BreezeTts) && lib.route_for("default").map_err(err)?.is_none() {
        lib.set_route("default", Some(&cfg.id)).map_err(err)?;
    }
    Ok(cfg)
}

#[tauri::command]
fn delete_provider(state: State<AppState>, id: String) -> Res<()> {
    state.secrets.delete(&id)?;
    state.lib()?.delete_provider(&id).map_err(err)
}

/// 实测能力并保存结果（会向该服务发送几条很短的测试请求）。
#[tauri::command]
async fn test_provider(state: State<'_, AppState>, id: String, context_hint: Option<u64>) -> Res<Capabilities> {
    let mut cfg = state.provider(&id)?;
    if cfg.service != chronicle_llm::types::ServiceType::Chat {
        let caps = test_media_provider(&state, &cfg).await?;
        cfg.capabilities = Some(caps.clone());
        state.lib()?.save_provider(&cfg.id, &serde_json::to_value(&cfg).map_err(err)?).map_err(err)?;
        return Ok(caps);
    }
    let client = state.client(&cfg)?;
    let hint = context_hint.or(cfg.capabilities.as_ref().and_then(|c| c.context_length));
    // 检测请求也走调度中心（前台 P1）：不和正在进行的 GM 回复抢模型服务，并在性能中心里可见；托管的模型按需启动。
    let (sched, models, service) = (state.sched.clone(), state.models.clone(), chronicle_runtime::client::service_key(&cfg.base_url));
    let caps = tauri::async_runtime::spawn_blocking(move || -> Res<Capabilities> {
        let cancel = CancelToken::new();
        let permit = sched.acquire(chronicle_runtime::Ticket::new(Priority::P1, "probe", service.clone()).label("检测能力"), &cancel).map_err(|e| e.to_string())?;
        if let Err(e) = chronicle_runtime::client::BeforeRequest::ensure(models.as_ref(), &service, &cancel) {
            permit.finish(Some(e.clone()));
            return Err(e);
        }
        let caps = discovery::test_capabilities(&client, hint, chronicle_core::util::now_ms());
        permit.finish(None);
        Ok(caps)
    })
    .await
    .map_err(err)??;
    cfg.capabilities = Some(caps.clone());
    state.lib()?.save_provider(&cfg.id, &serde_json::to_value(&cfg).map_err(err)?).map_err(err)?;
    Ok(caps)
}

/// 语音服务：合成"测试"两个字；生图服务：读取模型列表并确认模型在列表里（不生成图片，避免计费）。
async fn test_media_provider(state: &AppState, cfg: &ProviderConfig) -> Res<Capabilities> {
    let key = if cfg.needs_key { state.secrets.get(&cfg.id)? } else { None };
    let cfg2 = cfg.clone();
    let mut notes = vec![];
    let ok = tauri::async_runtime::spawn_blocking(move || -> Res<bool> {
        let t = std::time::Duration::from_secs(cfg2.timeout_secs.clamp(10, 120));
        match cfg2.service {
            chronicle_llm::types::ServiceType::Speech => {
                let target = chronicle_llm::tts::SpeechTarget {
                    base_url: cfg2.base_url.clone(),
                    api_key: key,
                    model: cfg2.model.clone(),
                    default_voice: cfg2.media.voice.clone(),
                    format: cfg2.media.audio_format.clone(),
                    send_instructions: false,
                    send_seed: cfg2.is_loopback(),
                };
                let a = chronicle_llm::tts::synthesize_to(&target, "测试", &chronicle_llm::tts::VoiceSpec::default(), t, &CancelToken::new()).map_err(|e| e.to_string())?;
                Ok(!a.bytes.is_empty())
            }
            _ => {
                let models = discovery::list_models(&cfg2.base_url, key.as_deref(), t).map_err(|e| e.to_string())?;
                Ok(models.is_empty() || models.iter().any(|m| m == &cfg2.model))
            }
        }
    })
    .await
    .map_err(err)?;
    let ok = match ok {
        Ok(v) => {
            if !v {
                notes.push(format!("模型列表里没有「{}」，请检查模型名", cfg.model));
            }
            v
        }
        Err(e) => {
            notes.push(e);
            false
        }
    };
    if cfg.service == chronicle_llm::types::ServiceType::Image {
        notes.push("生图服务只检查了 API Key 与模型列表；真正生成一张图要在节点的「图片」页签里点「AI 生成图片」（会计费）".into());
    }
    Ok(Capabilities { chat: ok, streaming: false, json_schema: false, vision: false, context_length: None, tested_at: chronicle_core::util::now_ms(), notes })
}

/// 读取服务的模型列表（不计费）。`api_key` 为空时用已保存的 Key。
#[tauri::command]
async fn provider_models(state: State<'_, AppState>, id: Option<String>, base_url: String, api_key: Option<String>) -> Res<Vec<String>> {
    let key = match api_key.filter(|k| !k.trim().is_empty()) {
        Some(k) => Some(k.trim().to_string()),
        None => match id {
            Some(id) => state.secrets.get(&id)?,
            None => None,
        },
    };
    tauri::async_runtime::spawn_blocking(move || discovery::list_models(&base_url, key.as_deref(), std::time::Duration::from_secs(20)).map_err(|e| e.to_string())).await.map_err(err)?
}

#[derive(Serialize)]
struct GeneratedImageOut {
    data_url: String,
    revised_prompt: Option<String>,
    provider: String,
    ms: u64,
}

/// 用生图服务生成一张图片（不保存；界面预览后由用户决定是否存到节点）。
#[tauri::command]
async fn generate_image(state: State<'_, AppState>, prompt: String, size: Option<String>) -> Res<GeneratedImageOut> {
    let routed = state.lib()?.route_exact("image").map_err(err)?;
    let cfg = match routed {
        Some(id) => state.provider(&id)?,
        None => {
            let all = state.lib()?.providers().map_err(err)?;
            all.into_iter()
                .filter_map(|v| serde_json::from_value::<ProviderConfig>(v).ok())
                .find(|c| c.service == chronicle_llm::types::ServiceType::Image)
                .ok_or("还没有设置生图服务：请在「模型服务」里添加（服务类型选「生成图片」）")?
        }
    };
    if !cfg.is_loopback() && !cfg.allow_campaign_content {
        return Err(format!("「{}」是云端服务，还没有允许发送内容（在「模型服务」里打开该服务的「允许发送战役内容」）", cfg.name));
    }
    let key = if cfg.needs_key { state.secrets.get(&cfg.id)? } else { None };
    if cfg.needs_key && key.is_none() {
        return Err(format!("「{}」需要 API Key，请先在设置中填写", cfg.name));
    }
    let target = chronicle_llm::image::ImageTarget {
        base_url: cfg.base_url.clone(),
        api_key: key,
        model: cfg.model.clone(),
        size: size.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| cfg.media.image_size.clone()),
    };
    let timeout = std::time::Duration::from_secs(cfg.timeout_secs.clamp(30, 600));
    // 走调度中心（P5 生成，与世界生成同级），在性能中心里可见。
    let (sched, service) = (state.sched.clone(), chronicle_runtime::client::service_key(&cfg.base_url));
    let started = std::time::Instant::now();
    let img = tauri::async_runtime::spawn_blocking(move || -> Res<chronicle_llm::image::GeneratedImage> {
        let cancel = CancelToken::new();
        let permit = sched.acquire(chronicle_runtime::Ticket::new(Priority::P5, "image", service).label(prompt.chars().take(20).collect::<String>()), &cancel).map_err(|e| e.to_string())?;
        let r = chronicle_llm::image::generate(&target, &prompt, timeout, &cancel).map_err(|e| e.to_string());
        permit.finish(r.as_ref().err().cloned());
        r
    })
    .await
    .map_err(err)??;
    use base64::Engine;
    applog::info(&format!("生成图片：{}，{} KB，{} ms", cfg.name, img.bytes.len() / 1024, started.elapsed().as_millis()));
    Ok(GeneratedImageOut {
        data_url: format!("data:{};base64,{}", img.mime, base64::engine::general_purpose::STANDARD.encode(&img.bytes)),
        revised_prompt: img.revised_prompt,
        provider: cfg.name,
        ms: started.elapsed().as_millis() as u64,
    })
}

#[tauri::command]
fn list_routes(state: State<AppState>) -> Res<Vec<(String, String)>> {
    state.lib()?.routes().map_err(err)
}

#[tauri::command]
fn set_route(state: State<AppState>, task: String, provider_id: Option<String>) -> Res<()> {
    state.lib()?.set_route(&task, provider_id.as_deref()).map_err(err)
}

#[tauri::command]
fn provider_stats(state: State<AppState>) -> Res<Vec<ProviderStats>> {
    state.lib()?.request_stats().map_err(err)
}

pub fn run() {
    let dir = data_dir();
    applog::init(&dir);
    // 资料库打不开时不直接退出：界面显示原因、数据目录和日志位置。
    let (library, open_error) = match Library::open(&dir) {
        Ok(l) => (Some(l), None),
        Err(e) => {
            let msg = format!(
                "无法打开资料库（{}）：{e}

你的世界文件没有被修改。如果刚升级过程序，升级前的备份在数据目录的 backups 文件夹中。详细信息见日志：{}",
                dir.join("library.db").display(),
                applog::log_path().map(|p| p.display().to_string()).unwrap_or_default()
            );
            applog::error(&msg);
            (None, Some(msg))
        }
    };
    // 用户可在数据目录的 prompts 文件夹中放同名模板覆盖内置模板。
    let templates = match Templates::load(Some(&dir.join("prompts"))) {
        Ok(t) => t,
        Err(e) => {
            applog::warn(&format!("自定义模板有误，改用内置模板: {e}"));
            Templates::builtin()
        }
    };
    // 调度中心、托管模型、资源监控。性能模式从设置读取（默认平衡）。
    let mode = library.as_ref().and_then(|l| l.setting("perf_mode").ok().flatten()).and_then(|m| PerfMode::parse(&m)).unwrap_or(PerfMode::Balanced);
    let sched = Scheduler::new(mode);
    let models = ModelManager::new(&dir.join("logs"), &sched, applog::info);
    let monitor = Monitor::start(dir.clone(), &sched, &models);
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(update::Pending::default())
        .manage(AppState {
            library: Mutex::new(library),
            startup_error: open_error,
            worlds: Mutex::default(),
            templates: Arc::new(templates),
            running: Mutex::default(),
            secrets: Arc::new(secrets::KeyringSecrets),
            sched,
            models,
            monitor,
            gen_keys: Arc::default(),
            render_stats: Mutex::default(),
        })
        .setup(|app| {
            perf::sync_profiles(&app.state::<AppState>());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            voice_speak,
            voice_status,
            voice_set_narrator,
            voice_clear_cache,
            gm_console,
            console_apply,
            director_set_status,
            director_set_settings,
            director_analyze,
            rpg_overview,
            rpg_save_sheet,
            rpg_action,
            rpg_set_rules,
            combat_start,
            combat_act,
            combat_abort,
            world_sim_overview,
            plan_travel,
            travel_to,
            add_map_marker,
            remove_map_marker,
            set_weather_today,
            app_info,
            ui_pref,
            update::update_check,
            update::update_install,
            update::update_download_only,
            set_ui_pref,
            startup_error,
            perf::perf_snapshot,
            perf::set_perf_mode,
            perf::pause_from,
            perf::task_pause,
            perf::task_cancel,
            perf::report_render,
            perf::list_managed_models,
            perf::save_managed_model,
            perf::delete_managed_model,
            perf::model_start,
            perf::model_stop,
            perf::estimate_model,
            perf::memory_advice,
            perf::db_probe,
            list_worlds,
            create_world,
            world_overview,
            list_commits,
            undo_commit,
            list_scripts,
            get_script,
            create_script,
            update_script,
            create_script_item,
            update_script_item,
            delete_script_item,
            search_script_items,
            get_node,
            tree_children,
            node_detail,
            create_node,
            update_node,
            move_node,
            delete_node,
            restore_node,
            list_trash,
            create_relation,
            update_relation,
            delete_relation,
            relation_types,
            search_nodes,
            node_graph,
            check_consistency,
            kind_counts,
            list_versions,
            create_version,
            restore_version,
            list_backups,
            restore_backup,
            backup_now,
            check_integrity,
            repair_integrity,
            dismiss_incomplete_turn,
            export_world,
            import_into_world,
            import_as_new_world,
            plot_graph,
            create_plot_node,
            update_plot_node,
            delete_plot_node,
            create_plot_edge,
            update_plot_edge,
            delete_plot_edge,
            analyze_plot,
            sim_start,
            sim_options,
            sim_step,
            list_campaigns,
            create_campaign,
            campaign_status,
            campaign_take_edge,
            end_campaign,
            relation_network,
            investigation_board,
            board_add_note,
            board_remove_note,
            board_link,
            board_unlink,
            board_save_hypothesis,
            board_delete_hypothesis,
            quest_list,
            npc_states,
            advance_story_time,
            find_places,
            add_cast_member,
            bond_history,
            person_bonds,
            relations_among,
            timeline_fork_points,
            timeline_fork,
            timeline_verify,
            timeline_rename,
            timeline_archive,
            timeline_compare,
            node_images,
            node_image_add,
            node_image_data,
            node_image_caption,
            node_image_cover,
            node_image_delete,
            node_covers,
            node_cover_thumbs,
            node_image_thumbs,
            node_image_set_thumb,
            illustrated_nodes,
            scene_view,
            describe_node_image,
            provider_models,
            generate_image,
            archive_list,
            archive_stats,
            archive_endings,
            archive_chapters,
            archive_mark_chapter,
            archive_rename_chapter,
            archive_remove_chapter,
            archive_chronicle,
            archive_save_chronicle,
            archive_search,
            archive_export,
            archive_import,
            archive_check,
            gm_turn,
            cancel_gm_turn,
            campaign_log,
            campaign_memory,
            gm_summarize,
            confirm_summary,
            list_changesets,
            apply_changeset,
            reject_changeset,
            revise_changeset_op,
            start_generation,
            retry_job,
            fill_dead_ends,
            cancel_job,
            list_jobs,
            discover_services,
            scan_model_folder,
            list_providers,
            save_provider,
            delete_provider,
            test_provider,
            list_routes,
            set_route,
            provider_stats,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            applog::error(&format!("启动 Chronicle GM 失败：{e}"));
            std::process::exit(1);
        });
    // 退出时停止由本软件启动的模型进程，不留孤儿进程。
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            perf::on_exit(handle);
        }
    });
}

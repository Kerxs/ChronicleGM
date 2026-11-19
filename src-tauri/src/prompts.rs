//! 提示词在线更新：不更新软件也能换上新的提示词。
//!
//! - 提示词包是一个 JSON（`chronicle_gen::PromptPack`），放在 GitHub 上固定的发布 `prompts` 里；
//!   可以用环境变量 `CHRONICLE_PROMPT_ENDPOINT` 换地址（测试用）。
//! - 下载后先校验（校验值、模板名、每个模板能编译），版本号比正在用的新才换上；换上后立即生效（下一回合起），不用重启。
//! - 保存在 `数据目录/prompt-pack.json`，旧的留一份 `prompt-pack.prev.json`；可以一键改回软件自带的版本。
//! - 版本号不比软件自带的新时（软件更新后自带了更新的提示词）自动不用提示词包。
//! - 用户自己放在 `数据目录/prompts/` 里的模板始终优先。

use std::path::PathBuf;
use std::sync::Arc;

use chronicle_gen::templates::{PROMPT_VERSION, PromptPack};
use chronicle_gen::Templates;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::{AppState, Res, applog, data_dir, err};

const DEFAULT_ENDPOINT: &str = "https://github.com/Kerxs/ChronicleGM/releases/download/prompts/prompt-pack.json";

fn pack_path() -> PathBuf {
    data_dir().join("prompt-pack.json")
}

/// 读本地保存的提示词包（不完整、校验不过的当作没有）。
pub fn saved_pack() -> Option<PromptPack> {
    let p: PromptPack = serde_json::from_slice(&std::fs::read(pack_path()).ok()?).ok()?;
    match p.validate() {
        Ok(()) => Some(p),
        Err(e) => {
            applog::warn(&format!("本地提示词包不能用，改用自带提示词：{e}"));
            None
        }
    }
}

/// 按「自带 → 提示词包 → 用户覆盖」加载模板。
pub fn load_templates() -> Templates {
    let pack = saved_pack();
    match Templates::load_with(pack.as_ref(), Some(&data_dir().join("prompts"))) {
        Ok(t) => t,
        Err(e) => {
            applog::warn(&format!("自定义模板或提示词包有误，改用自带提示词：{e}"));
            Templates::builtin()
        }
    }
}

fn reload(state: &AppState) {
    let t = load_templates();
    applog::info(&format!("提示词已重新加载：{}", t.version));
    state.set_templates(Arc::new(t));
}

#[derive(Serialize)]
pub struct PromptInfo {
    version: String,
    builtin: u32,
    pack: Option<u32>,
    pack_date: Option<String>,
    overridden: Vec<String>,
    auto_check: bool,
}

#[tauri::command]
pub fn prompt_info(state: State<AppState>) -> Res<PromptInfo> {
    let t = state.tpl();
    let pack = saved_pack().filter(|p| Some(p.version) == t.pack_version);
    Ok(PromptInfo {
        version: t.version.clone(),
        builtin: PROMPT_VERSION,
        pack: t.pack_version,
        pack_date: pack.map(|p| p.date).filter(|d| !d.is_empty()),
        overridden: t.overridden.clone(),
        auto_check: state.lib()?.setting("ui.prompt_auto").map_err(err)?.as_deref() != Some("0"),
    })
}

#[derive(Serialize)]
pub struct CheckResult {
    updated: bool,
    version: u32,
    message: String,
}

/// 下载并检查；比正在用的新就换上。
pub fn check(state: &AppState) -> Res<CheckResult> {
    let url = std::env::var("CHRONICLE_PROMPT_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.into());
    let text = match ureq::get(&url).header("User-Agent", "ChronicleGM").call() {
        Ok(mut r) => r.body_mut().with_config().limit(8 * 1024 * 1024).read_to_string().map_err(err)?,
        Err(ureq::Error::StatusCode(404)) => return Ok(CheckResult { updated: false, version: current_version(state), message: "还没有发布在线提示词更新".into() }),
        Err(e) => return Err(format!("检查提示词更新失败：{e}")),
    };
    let pack: PromptPack = serde_json::from_str(&text).map_err(|e| format!("提示词包格式不对：{e}"))?;
    pack.validate().map_err(|e| e.to_string())?;
    let now = current_version(state);
    if pack.version <= now {
        return Ok(CheckResult { updated: false, version: now, message: format!("提示词已经是最新（v{now}）") });
    }
    let path = pack_path();
    if path.exists() {
        let _ = std::fs::copy(&path, data_dir().join("prompt-pack.prev.json"));
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &text).map_err(err)?;
    std::fs::rename(&tmp, &path).map_err(err)?;
    reload(state);
    applog::info(&format!("提示词更新到 v{}（{}）", pack.version, pack.date));
    Ok(CheckResult { updated: true, version: pack.version, message: format!("提示词已更新到 v{}{}，下一回合起生效", pack.version, if pack.notes.is_empty() { String::new() } else { format!("：{}", pack.notes) }) })
}

fn current_version(state: &AppState) -> u32 {
    state.tpl().pack_version.unwrap_or(PROMPT_VERSION)
}

#[tauri::command]
pub async fn prompt_update_check(app: AppHandle) -> Res<CheckResult> {
    tauri::async_runtime::spawn_blocking(move || check(&app.state::<AppState>())).await.map_err(err)?
}

#[tauri::command]
pub fn prompt_set_auto(state: State<AppState>, on: bool) -> Res<()> {
    state.lib()?.set_setting("ui.prompt_auto", if on { "1" } else { "0" }).map_err(err)
}

/// 不用在线提示词包，改回软件自带的。
#[tauri::command]
pub fn prompt_reset(state: State<AppState>) -> Res<()> {
    let p = pack_path();
    if p.exists() {
        std::fs::rename(&p, data_dir().join("prompt-pack.prev.json")).map_err(err)?;
    }
    reload(&state);
    Ok(())
}

/// 自定义提示词文件夹：把软件自带的模板放进「自带模板（参考）」子文件夹（每次打开都刷新），并写一份说明。
/// 只有直接放在 prompts 文件夹里的 `<名字>.j2` 才会覆盖自带模板，子文件夹里的只是参考，不会生效。
pub fn prepare_dir() -> Res<PathBuf> {
    let dir = data_dir().join("prompts");
    let refdir = dir.join("自带模板（参考）");
    std::fs::create_dir_all(&refdir).map_err(err)?;
    for (name, src) in chronicle_gen::templates::builtin_sources() {
        std::fs::write(refdir.join(format!("{name}.j2")), src).map_err(err)?;
    }
    let readme = format!(
        "自定义提示词

        1. 「自带模板（参考）」里是软件当前自带的全部提示词（v{PROMPT_VERSION}），每次打开这个文件夹都会刷新，改它们不会生效。
        2. 想修改某个提示词：把它复制到这一层（和这份说明放在一起），再修改。文件名不要改，例如 gm_plan.j2。
        3. 改完在「设置 → 通用 → 更新」里点「重新加载提示词」，下一回合起生效，不用重启。
        4. 想恢复：删掉这一层里对应的 .j2 文件，再重新加载。
        5. 模板有语法错误时软件会改用自带提示词，并在日志里写明原因。

        常用：gm_plan（GM 理解行动、决定检定和效果）、gm_narrate（按结果写叙述）、npc_reply（NPC 回应）、director（剧情导演）、free_play（自由行动规则）。
"
    );
    std::fs::write(dir.join("说明.txt"), readme).map_err(err)?;
    Ok(dir)
}

/// 重新加载提示词（用户改了自定义模板后）。返回正在使用的版本和自定义的模板。
#[tauri::command]
pub fn prompt_reload(state: State<AppState>) -> Res<PromptInfo> {
    reload(&state);
    prompt_info(state)
}

/// 启动后：开着自动检查且距上次超过一天，就在后台检查一次。
pub fn auto_check(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(8));
        let state = app.state::<AppState>();
        let Ok(lib) = state.lib() else { return };
        if lib.setting("ui.prompt_auto").ok().flatten().as_deref() == Some("0") {
            return;
        }
        let now = chronicle_core::util::now_ms();
        let last: i64 = lib.setting("ui.prompt_last_check").ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(0);
        if now - last < 24 * 3600 * 1000 {
            return;
        }
        let _ = lib.set_setting("ui.prompt_last_check", &now.to_string());
        drop(lib);
        match check(&state) {
            Ok(r) if r.updated => applog::info(&r.message),
            Ok(_) => {}
            Err(e) => applog::warn(&format!("自动检查提示词更新：{e}")),
        }
    });
}

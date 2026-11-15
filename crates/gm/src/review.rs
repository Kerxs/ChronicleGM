//! 剧本的 AI 逻辑检查：人物设定是否前后一致、剧情是否连贯、任务与结局是否有达成路径、可选角色是否与主线有关联。
//! 只返回问题与修复建议，不修改剧本。

use std::sync::Mutex;

use chronicle_core::World;
use chronicle_core::scripts::{self, ScriptFinding};
use chronicle_gen::Templates;
use chronicle_llm::{CancelToken, LlmClient};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::ask::{GmOptions, ask_limited};
use crate::{GmError, Result};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ReviewIssue {
    /// error（明显矛盾、会让剧情走不下去）/ warning（逻辑薄弱）/ info（可以更好）
    pub severity: String,
    /// 出问题的地方（条目名称）。
    #[serde(default)]
    pub location: String,
    pub problem: String,
    #[serde(default)]
    pub suggestion: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ReviewOut {
    /// 发现的问题，最多 10 条；没有问题就返回空数组。
    pub issues: Vec<ReviewIssue>,
}

fn lock(w: &Mutex<World>) -> Result<std::sync::MutexGuard<'_, World>> {
    w.lock().map_err(|_| GmError::Input("世界文件被占用".into()))
}

pub fn review_script(world: &Mutex<World>, client: &dyn LlmClient, templates: &Templates, opts: &GmOptions, script_id: &str, cancel: &CancelToken) -> Result<Vec<ScriptFinding>> {
    let (meta, outline, pcs, cast) = {
        let w = lock(world)?;
        let s = scripts::require_script(w.conn(), script_id)?;
        let meta = chronicle_gen::context::script_meta(&s);
        let outline = chronicle_gen::context::script_outline(&w, script_id, true)?;
        let pcs: Vec<String> = scripts::items_by_kind(w.conn(), script_id, chronicle_core::model::ScriptItemKind::PlayerCharacter)?
            .iter()
            .map(|p| format!("- {}：{}；身世：{}；{}", p.title, p.player_text, p.payload.get("backstory").map(|b| b.to_string()).unwrap_or_default(), p.gm_text))
            .collect();
        let cast = chronicle_gen::context::cast_text(&w, script_id)?;
        (meta, outline, pcs.join("\n"), cast)
    };
    let user = templates.render("script_review", serde_json::json!({ "meta": meta, "outline": outline, "pcs": pcs, "cast": cast }))?;
    let system = templates.render("system", serde_json::json!({ "schema_text": "" }))?;
    let mut logs = vec![];
    let ans = ask_limited::<ReviewOut>(client, &system, &user, opts, 2500, cancel, &mut logs)?;
    Ok(ans
        .value
        .issues
        .into_iter()
        .take(10)
        .map(|i| ScriptFinding {
            severity: if matches!(i.severity.as_str(), "error" | "warning" | "info") { i.severity } else { "warning".into() },
            message: if i.location.trim().is_empty() { i.problem } else { format!("{}：{}", i.location.trim(), i.problem) },
            suggestion: i.suggestion,
            item_id: None,
            node_id: None,
        })
        .collect())
}

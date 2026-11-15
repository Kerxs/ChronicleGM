//! 上下文压缩：把「最近的对话」窗口之前、还没有摘要过的游戏记录，在后台整理成增量摘要。
//!
//! - 只压缩对话与叙述；物品、骰子、决定等事实另有程序记录（战役状态、事件日志、记忆事实），原样保留；
//! - 达到阈值（按 token 计）才压缩；token 数优先用服务端分词实测（llama-server 的 /tokenize），否则估算；
//! - 记录压缩前后的 token 数；失败时什么都不改（原始记录一直保留）。

use std::sync::Mutex;

use chronicle_core::World;
use chronicle_core::campaign::{self, CampaignView, SessionSummary};
use chronicle_core::rules::{Effect, StateView};
use chronicle_gen::Templates;
use chronicle_llm::{CancelToken, LlmClient};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ask::{GmOptions, ask_limited};
use crate::outputs::SummaryOut;
use crate::{GmError, Result};

pub const SETTINGS_VAR: &str = "memory.settings";
/// 「最近的对话」窗口：GM 上下文里原样保留的最近几条（与 context.rs 一致）。
pub const SHORT_WINDOW: usize = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(default)]
pub struct MemorySettings {
    /// 达到阈值时自动压缩。
    pub auto: bool,
    /// 窗口之前还没摘要的记录超过多少 token 时压缩。
    pub threshold_tokens: i64,
}

impl Default for MemorySettings {
    fn default() -> Self {
        Self { auto: true, threshold_tokens: 1500 }
    }
}

pub fn settings(world: &World, campaign_id: &str) -> MemorySettings {
    CampaignView { conn: world.conn(), campaign_id }.var(SETTINGS_VAR).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

pub fn set_settings(world: &mut World, campaign_id: &str, s: &MemorySettings) -> chronicle_core::Result<()> {
    if !(300..=20000).contains(&s.threshold_tokens) {
        return Err(chronicle_core::CoreError::Invalid("压缩阈值需在 300–20000 token".into()));
    }
    campaign::apply_effects(world, campaign_id, &[Effect::SetVar { key: SETTINGS_VAR.into(), value: serde_json::to_value(s).unwrap_or(Value::Null) }], "修改记忆压缩设置")?;
    Ok(())
}

/// 待压缩的记录：上次摘要之后、最近窗口之前的对话。返回 (起点, 终点, 文字行)。
pub fn pending(world: &World, campaign_id: &str) -> chronicle_core::Result<(i64, i64, Vec<String>)> {
    let from = campaign::summarized_upto(world.conn(), campaign_id)?;
    let events = campaign::events(world.conn(), campaign_id, from, 5000)?;
    let dialogue: Vec<_> = events.iter().filter(|e| matches!(e.kind.as_str(), "player" | "narration" | "npc")).collect();
    if dialogue.len() <= SHORT_WINDOW {
        return Ok((from, from, vec![]));
    }
    let cut = dialogue[dialogue.len() - SHORT_WINDOW].seq; // 窗口里最早的一条
    let mut lines = vec![];
    let mut upto = from;
    for e in events.iter().filter(|e| e.seq < cut) {
        upto = e.seq;
        let text = e.payload.get("text").and_then(Value::as_str).unwrap_or("");
        match e.kind.as_str() {
            "player" => lines.push(format!("[回合 {}] 玩家：{text}", e.turn)),
            "narration" => lines.push(format!("[回合 {}] GM：{text}", e.turn)),
            "npc" => lines.push(format!("[回合 {}] {}：{text}", e.turn, e.payload.get("name").and_then(Value::as_str).unwrap_or(""))),
            _ => {}
        }
    }
    Ok((from, upto, lines))
}

/// 达到阈值时压缩一次（`force` 忽略阈值与开关，用于"重新生成"）。没到阈值返回 None。
/// `count` 计算 token 数，返回 (数量, 是否实测)。
#[allow(clippy::too_many_arguments)]
pub fn maybe_compress(world: &Mutex<World>, client: &dyn LlmClient, templates: &Templates, opts: &GmOptions, campaign_id: &str, force: bool, count: &dyn Fn(&str) -> (usize, bool), cancel: &CancelToken) -> Result<Option<SessionSummary>> {
    let (from, upto, lines, previous, cfg) = {
        let w = world.lock().map_err(|_| GmError::Input("世界文件被占用".into()))?;
        let cfg = settings(&w, campaign_id);
        if !cfg.auto && !force {
            return Ok(None);
        }
        let (from, upto, lines) = pending(&w, campaign_id)?;
        let previous = campaign::summaries(w.conn(), campaign_id)?.into_iter().rfind(|s| s.status == "confirmed" || s.status == "auto").map(|s| s.summary).unwrap_or_default();
        (from, upto, lines, previous, cfg)
    };
    if lines.is_empty() {
        return if force { Err(GmError::Input("最近窗口之前没有需要压缩的记录".into())) } else { Ok(None) };
    }
    let log = lines.join("\n");
    let (before, measured) = count(&log);
    if !force && (before as i64) < cfg.threshold_tokens {
        return Ok(None);
    }
    let user = templates.render("session_summary", json!({ "log": log, "previous": previous }))?;
    let mut logs = vec![];
    let s = ask_limited::<SummaryOut>(client, "你是 TRPG 的记录员，负责把游戏记录整理成简洁的摘要。只输出一个 JSON 对象。使用简体中文。", &user, opts, 1500, cancel, &mut logs)?.value;
    if s.summary.trim().is_empty() {
        return Err(GmError::Input("模型返回了空摘要，原始记录保持不变".into()));
    }
    let (after, _) = count(&s.summary);
    let w = world.lock().map_err(|_| GmError::Input("世界文件被占用".into()))?;
    // 期间有人手动摘要或压缩过：放弃这次结果，避免重叠。
    if campaign::summarized_upto(w.conn(), campaign_id)? != from {
        return Ok(None);
    }
    let method = if measured { "measured" } else { "estimated" };
    Ok(Some(campaign::add_auto_summary(w.conn(), campaign_id, &s.summary, from, upto, before as i64, after as i64, method)?))
}

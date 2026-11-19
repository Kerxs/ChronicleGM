//! 战斗描述（改革第 6 阶段）：战斗场景里玩家点按钮行动时，规则引擎先结算（`combat::act`），
//! 再请模型按结算写 2–3 句描述。模型只写文字，不碰任何状态；写完记成一条叙述事件（剧情历史里能看到）。
//! 界面异步调用：动画和下一步操作不等它。

use std::sync::Mutex;

use chronicle_core::{World, campaign};
use chronicle_gen::Templates;
use chronicle_llm::{CancelToken, LlmClient};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::ask::{GmOptions, ask_limited};
use crate::{GmError, Result};

#[derive(Debug, Deserialize, JsonSchema)]
struct CombatNarration {
    /// 2–3 句战斗描述。
    narration: String,
}

/// 按结算行写战斗描述，记入剧情历史并返回文字。没有结算行时返回空字符串。
pub fn narrate_combat(world: &Mutex<World>, client: &dyn LlmClient, templates: &Templates, opts: &GmOptions, campaign_id: &str, lines: &[String], cancel: &CancelToken) -> Result<String> {
    let lines: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).take(16).collect();
    if lines.is_empty() {
        return Ok(String::new());
    }
    let (scene, player, fighters) = {
        let w = world.lock().map_err(|_| GmError::Input("世界文件被占用".into()))?;
        // 参战者和他们手里的武器（实测：没有枪的战斗里，模型把「瞄准」写成了举枪、子弹擦过耳边）。
        let fighters = chronicle_core::combat::active(w.conn(), campaign_id)
            .or_else(|| chronicle_core::combat::history(w.conn(), campaign_id, 1).ok().and_then(|v| v.into_iter().next()))
            .map(|c| c.units.iter().map(|u| format!("{}（{}，{}）", u.name, if u.weapon.is_empty() || u.weapon == "武器" { "近身兵器".to_string() } else { u.weapon.clone() }, if u.out.is_empty() { "还在战斗" } else { "已倒下" })).collect::<Vec<_>>().join("、"))
            .unwrap_or_default();
        let c = campaign::get_campaign(w.conn(), campaign_id)?;
        let scene = c
            .current_plot_node
            .as_deref()
            .and_then(|id| chronicle_core::plot::get_plot_node(w.conn(), id).ok().flatten())
            .map(|n| format!("{}：{}", n.title, n.player_text))
            .unwrap_or_else(|| "（战斗现场）".into());
        let player = chronicle_core::pc::get(w.conn(), campaign_id).map(|p| format!("{}（{}）", p.name, p.identity)).unwrap_or_else(|| "玩家的角色".into());
        (scene, player, fighters)
    };
    let user = templates.render("combat_narrate", json!({ "scene": scene, "player": player, "fighters": fighters, "lines": lines.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n") }))?;
    let system = templates.render("system", json!({ "schema_text": "" }))?;
    let mut logs = vec![];
    let out = ask_limited::<CombatNarration>(client, &system, &user, opts, 600, cancel, &mut logs)?.value;
    let text = out.narration.trim().to_string();
    if text.is_empty() {
        return Ok(text);
    }
    let w = world.lock().map_err(|_| GmError::Input("世界文件被占用".into()))?;
    campaign::record(w.conn(), campaign_id, "narration", &json!({ "text": text, "leaks": [], "combat": true }))?;
    Ok(text)
}

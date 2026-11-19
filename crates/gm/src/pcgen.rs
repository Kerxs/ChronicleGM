//! 按剧本让 AI 生成一个新的玩家角色（带身世）。不写入剧本，由界面确认后用 `pc::set` 设定到战役。

use std::sync::Mutex;

use chronicle_core::World;
use chronicle_core::model::ScriptItemKind;
use chronicle_core::pc::PlayerCharacter;
use chronicle_core::scripts;
use chronicle_gen::Templates;
use chronicle_gen::outputs::PcOutline;
use chronicle_llm::{CancelToken, LlmClient};

use crate::ask::{GmOptions, ask_limited};
use crate::{GmError, Result};

pub fn generate_pc(world: &Mutex<World>, client: &dyn LlmClient, templates: &Templates, opts: &GmOptions, script_id: &str, hint: &str, cancel: &CancelToken) -> Result<PlayerCharacter> {
    let (meta, outline, cast, pcs, play, taken) = {
        let w = world.lock().map_err(|_| GmError::Input("世界文件被占用".into()))?;
        let s = scripts::require_script(w.conn(), script_id)?;
        let meta = chronicle_gen::context::script_meta(&s);
        let outline = chronicle_gen::context::script_outline(&w, script_id, false)?;
        let cast = chronicle_gen::context::cast_text(&w, script_id)?;
        let items = scripts::items_by_kind(w.conn(), script_id, ScriptItemKind::PlayerCharacter)?;
        let pcs: Vec<String> = items.iter().map(|p| format!("- {}：{}", p.title, p.player_text)).collect();
        let profile = chronicle_core::profile::for_script(&s);
        let mut taken: Vec<String> = items.iter().map(|p| p.title.clone()).collect();
        taken.extend(cast.lines().filter_map(|l| l.trim_start_matches("- ").split(['：', ':', '（']).next()).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()));
        (meta, outline, cast, if pcs.is_empty() { "（无）".to_string() } else { pcs.join("\n") }, profile.guidance(), taken)
    };
    let hint = if hint.trim().is_empty() { "（没有特别要求）" } else { hint.trim() };
    let user = templates.render("pc_generate", serde_json::json!({ "meta": meta, "outline": outline, "cast": cast, "pcs": pcs, "hint": hint, "play": play }))?;
    let system = templates.render("system", serde_json::json!({ "schema_text": "" }))?;
    let mut logs = vec![];
    let o = ask_limited::<PcOutline>(client, &system, &user, opts, 1800, cancel, &mut logs)?.value;
    if o.name.trim().is_empty() {
        return Err(GmError::Input("模型没有给出角色名，请重试".into()));
    }
    let mut name = o.name.trim().to_string();
    if taken.iter().any(|t| t == &name) {
        name = format!("{name}（新）");
    }
    let payload = chronicle_gen::play::pc_payload(&o);
    Ok(PlayerCharacter {
        name,
        identity: o.identity,
        personality: o.personality,
        abilities: o.abilities.into_iter().filter(|a| !a.trim().is_empty()).take(4).collect(),
        intro: o.player_text,
        backstory: payload.get("backstory").cloned().unwrap_or_default(),
        link_to_plot: o.backstory.link_to_plot,
        source_item: None,
        equipment: chronicle_core::pc::normalize_kit(&serde_json::from_value::<Vec<chronicle_core::pc::StartItem>>(serde_json::to_value(&o.equipment).unwrap_or_default()).unwrap_or_default()),
    })
}

/// 角色没有写初始装备（旧剧本、手动添加的角色）：按设定让 AI 列出随身装备，由程序检查后放进背包。
/// 背包里已经有东西时不做。返回发放的物品说明。
pub fn infer_start_kit(world: &Mutex<World>, client: &dyn LlmClient, templates: &Templates, opts: &GmOptions, campaign_id: &str, cancel: &CancelToken, logs: &mut Vec<chronicle_gen::RequestLog>) -> Result<Vec<String>> {
    let (pc, setting) = {
        let w = world.lock().map_err(|_| GmError::Input("世界文件被占用".into()))?;
        let Some(pc) = chronicle_core::pc::get(w.conn(), campaign_id) else { return Ok(vec![]) };
        if !pc.equipment.is_empty() || !chronicle_core::rpg::inventory(w.conn(), campaign_id, &chronicle_core::rpg::actor())?.is_empty() {
            return Ok(vec![]);
        }
        let c = chronicle_core::campaign::get_campaign(w.conn(), campaign_id)?;
        let setting = c
            .script_id
            .as_deref()
            .and_then(|sid| scripts::get_script(w.conn(), sid).ok().flatten())
            .map(|s| format!("《{}》 题材：{}；{}", s.title, s.genre, s.premise.chars().take(200).collect::<String>()))
            .unwrap_or_else(|| "（自由沙盒）".into());
        (pc, setting)
    };
    let user = templates.render("pc_kit", serde_json::json!({ "pc": chronicle_core::pc::context_text(&pc) + "\n介绍：" + &pc.intro, "setting": setting }))?;
    let system = templates.render("system", serde_json::json!({ "schema_text": "" }))?;
    let o = ask_limited::<chronicle_gen::outputs::KitOut>(client, &system, &user, opts, 1200, cancel, logs)?.value;
    let items: Vec<chronicle_core::pc::StartItem> = serde_json::from_value(serde_json::to_value(&o.items).unwrap_or_default()).unwrap_or_default();
    if items.is_empty() {
        return Ok(vec![]);
    }
    let mut w = world.lock().map_err(|_| GmError::Input("世界文件被占用".into()))?;
    Ok(chronicle_core::rpg::grant_start_kit(&mut w, campaign_id, &items)?)
}

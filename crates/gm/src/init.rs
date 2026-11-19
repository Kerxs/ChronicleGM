//! 世界初始化（改革第 2 阶段）：确认游戏设置后依次执行 7 个真实步骤，每一步做完才算进度。
//!
//! 1 加载剧本 → 2 加载世界设定 → 3 初始化玩家角色 → 4 初始化 NPC → 5 初始化人物关系 → 6 构建 AI GM 上下文 → 7 检查世界数据和存档。
//!
//! - 每一步都幂等：重复执行不会多发装备、不会重复建卡或重复加关系（中途失败后重试只做没完成的步骤）。
//! - 进度只按完成的步骤走（`gamestate::set_step`），每次变化通过回调推给界面；没有假进度。
//! - 第 3 步在角色没有写初始装备时会调用模型推断；第 6 步会向模型服务发一次很短的请求预热提示词缓存
//!   （连不上时这一步照样完成，并写明原因）。没有模型服务时 `client` 传 None。

use std::sync::Mutex;
use std::time::Instant;

use chronicle_core::gamestate::{self, GameState, Phase, StepStatus};
use chronicle_core::model::ScriptItemKind;
use chronicle_core::rpg;
use chronicle_core::{World, campaign, scripts};
use chronicle_gen::Templates;
use chronicle_llm::{CancelToken, ChatMessage, ChatRequest, LlmClient, OutputFormat};
use chronicle_core::rules::Effect;
use serde_json::json;

use crate::ask::GmOptions;
use crate::{GmError, Result};

fn lock(w: &Mutex<World>) -> Result<std::sync::MutexGuard<'_, World>> {
    w.lock().map_err(|_| GmError::Input("世界文件被占用".into()))
}

/// 一步的结果：完成 / 跳过（附说明）。失败用 Err。
type StepOut = std::result::Result<(StepStatus, String), String>;

/// 执行初始化。`on_change` 在每次步骤状态变化时调用（界面进度）。
/// 返回最终状态：全部完成时已进入「开场」；某一步失败时停在「初始化」，那一步标为失败。
pub fn run_init(world: &Mutex<World>, client: Option<&dyn LlmClient>, templates: &Templates, opts: &GmOptions, campaign_id: &str, cancel: &CancelToken, on_change: &dyn Fn(&GameState)) -> Result<GameState> {
    {
        let mut w = lock(world)?;
        let s = gamestate::get(w.conn(), campaign_id)?;
        match s.phase {
            Phase::Setup => {
                let s = gamestate::transition(&mut w, campaign_id, Phase::Initializing, "开始初始化世界")?;
                on_change(&s);
            }
            Phase::Initializing => {}
            // 已经初始化过（例如重复点击）：不重复做。
            Phase::Opening => return Ok(s),
            p => return Err(GmError::Input(format!("现在是「{}」阶段，不能初始化", gamestate::label(p)))),
        }
    }
    for (key, _) in gamestate::INIT_STEPS {
        if cancel.is_cancelled() {
            return Err(GmError::Cancelled);
        }
        {
            let w = lock(world)?;
            let s = gamestate::get(w.conn(), campaign_id)?;
            if s.init.iter().any(|x| x.key == key && matches!(x.status, StepStatus::Done | StepStatus::Skipped)) {
                continue;
            }
        }
        {
            let mut w = lock(world)?;
            let s = gamestate::set_step(&mut w, campaign_id, key, StepStatus::Running, "进行中", 0)?;
            on_change(&s);
        }
        let t = Instant::now();
        let out = match key {
            "script" => step_script(world, campaign_id),
            "world" => step_world(world, campaign_id),
            "player" => step_player(world, client, templates, opts, campaign_id, cancel),
            "npcs" => step_npcs(world, campaign_id),
            "relations" => step_relations(world, campaign_id),
            "gm_context" => step_gm_context(world, client, templates, opts, campaign_id, cancel),
            "integrity" => step_integrity(world, campaign_id),
            _ => Err(format!("未知步骤 {key}")),
        };
        let ms = t.elapsed().as_millis() as i64;
        let mut w = lock(world)?;
        match out {
            Ok((st, note)) => {
                let s = gamestate::set_step(&mut w, campaign_id, key, st, &note, ms)?;
                on_change(&s);
            }
            Err(e) => {
                let s = gamestate::set_step(&mut w, campaign_id, key, StepStatus::Failed, &e, ms)?;
                on_change(&s);
                return Ok(s);
            }
        }
    }
    let mut w = lock(world)?;
    let s = gamestate::transition(&mut w, campaign_id, Phase::Opening, "世界初始化完成")?;
    on_change(&s);
    Ok(s)
}

fn script_id(w: &World, campaign_id: &str) -> std::result::Result<Option<String>, String> {
    Ok(campaign::get_campaign(w.conn(), campaign_id).map_err(|e| e.to_string())?.script_id)
}

/// 1. 加载剧本：读剧本与条目，做结构检查（问题只提示，缺了就没法玩的错误才算失败）。
fn step_script(world: &Mutex<World>, campaign_id: &str) -> StepOut {
    let w = world.lock().map_err(|_| "世界文件被占用".to_string())?;
    let Some(sid) = script_id(&w, campaign_id)? else {
        return Ok((StepStatus::Skipped, "自由沙盒：不使用剧本".into()));
    };
    let s = scripts::require_script(w.conn(), &sid).map_err(|e| e.to_string())?;
    let count = |k: ScriptItemKind| scripts::items_by_kind(w.conn(), &sid, k).map(|v| v.len()).unwrap_or(0);
    let (chapters, scenes, quests) = (count(ScriptItemKind::Chapter), count(ScriptItemKind::Scene), count(ScriptItemKind::Quest));
    let findings = scripts::check_script(w.conn(), &sid).map_err(|e| e.to_string())?;
    let errors: Vec<_> = findings.iter().filter(|f| f.severity == "error").map(|f| f.message.clone()).collect();
    if !errors.is_empty() {
        return Err(format!("剧本《{}》有 {} 个必须修正的问题：{}", s.title, errors.len(), errors.iter().take(2).cloned().collect::<Vec<_>>().join("；")));
    }
    let warn = findings.iter().filter(|f| f.severity == "warning").count();
    Ok((StepStatus::Done, format!("《{}》：{chapters} 章、{scenes} 个场景、{quests} 个任务{}", s.title, if warn > 0 { format!("（{warn} 条建议可在剧本页查看）") } else { String::new() })))
}

/// 2. 加载世界设定：统计世界树（核心设定、地点、人物、势力），确定玩家的起点。
fn step_world(world: &Mutex<World>, campaign_id: &str) -> StepOut {
    let w = world.lock().map_err(|_| "世界文件被占用".to_string())?;
    let q = |sql: &str| w.conn().query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0);
    let total = q("SELECT COUNT(*) FROM nodes WHERE deleted_at IS NULL");
    let core = q("SELECT COUNT(*) FROM nodes WHERE deleted_at IS NULL AND canon_level = 'core'");
    let places = q("SELECT COUNT(*) FROM nodes WHERE deleted_at IS NULL AND kind IN ('location','region','site','city','place')");
    let people = q("SELECT COUNT(*) FROM nodes WHERE deleted_at IS NULL AND kind = 'character'");
    if total == 0 {
        return Err("世界树是空的：先在世界树里添加设定，或打开示例世界".into());
    }
    let start = chronicle_core::worldsim::party_location(w.conn(), campaign_id).ok().flatten().and_then(|id| chronicle_core::nodes::get_node(w.conn(), &id).ok().flatten()).map(|n| n.title);
    Ok((StepStatus::Done, format!("{total} 个设定（核心 {core}、地点 {places}、人物 {people}）{}", start.map(|s| format!("；起点：{s}")).unwrap_or_default())))
}

/// 3. 初始化玩家角色：角色卡写进存档，发放初始装备（角色没写装备时请模型按身世推断）。
fn step_player(world: &Mutex<World>, client: Option<&dyn LlmClient>, templates: &Templates, opts: &GmOptions, campaign_id: &str, cancel: &CancelToken) -> StepOut {
    let (pc, has_kit) = {
        let mut w = world.lock().map_err(|_| "世界文件被占用".to_string())?;
        let Some(pc) = chronicle_core::pc::get(w.conn(), campaign_id) else {
            return Err("还没有选择角色：先选一个角色或让 AI 生成一个".into());
        };
        if !rpg::has_sheet(w.conn(), campaign_id, &rpg::actor()) {
            let s = rpg::sheet(w.conn(), campaign_id, &rpg::actor()).map_err(|e| e.to_string())?;
            campaign::apply_effects(&mut w, campaign_id, &[rpg::sheet_effect(&s)], "初始化：玩家角色卡").map_err(|e| e.to_string())?;
        }
        if !pc.equipment.is_empty() && rpg::inventory(w.conn(), campaign_id, &rpg::actor()).map_err(|e| e.to_string())?.is_empty() {
            rpg::grant_start_kit(&mut w, campaign_id, &pc.equipment).map_err(|e| e.to_string())?;
        }
        let has = !rpg::inventory(w.conn(), campaign_id, &rpg::actor()).map_err(|e| e.to_string())?.is_empty();
        (pc, has)
    };
    let mut kit_note = String::new();
    if !has_kit {
        match client {
            Some(c) => match crate::pcgen::infer_start_kit(world, c, templates, opts, campaign_id, cancel, &mut vec![]) {
                Ok(items) if !items.is_empty() => kit_note = format!("；AI 按身世推断的装备：{}", items.join("、")),
                Ok(_) => kit_note = "；没有推断出装备".into(),
                Err(GmError::Cancelled) => return Err("已取消".into()),
                Err(e) => kit_note = format!("；没能推断初始装备（{e}），可以在角色页手动添加"),
            },
            None => kit_note = "；没有模型服务，初始装备留空（可以在角色页添加）".into(),
        }
    }
    let w = world.lock().map_err(|_| "世界文件被占用".to_string())?;
    let s = rpg::sheet(w.conn(), campaign_id, &rpg::actor()).map_err(|e| e.to_string())?;
    let bag = rpg::inventory(w.conn(), campaign_id, &rpg::actor()).map_err(|e| e.to_string())?.into_iter().map(|i| i.item.title).collect::<Vec<_>>();
    Ok((StepStatus::Done, format!("{}（{}）生命 {}/{}，背包 {} 件{}", pc.name, pc.identity, s.hp, s.max_hp, bag.len(), kit_note)))
}

/// 4. 初始化 NPC：给剧本里的出场人物建立角色卡（已有的不动）。
fn step_npcs(world: &Mutex<World>, campaign_id: &str) -> StepOut {
    let mut w = world.lock().map_err(|_| "世界文件被占用".to_string())?;
    let Some(sid) = script_id(&w, campaign_id)? else {
        return Ok((StepStatus::Skipped, "自由沙盒：人物在游戏中出现时再建立".into()));
    };
    let cast = scripts::cast_of(w.conn(), &sid).map_err(|e| e.to_string())?;
    let mut created = vec![];
    let mut effects = vec![];
    for m in &cast {
        if rpg::has_sheet(w.conn(), campaign_id, &m.node_id) {
            continue;
        }
        if let Ok(s) = rpg::sheet(w.conn(), campaign_id, &m.node_id) {
            created.push(s.name.clone());
            effects.push(rpg::sheet_effect(&s));
        }
    }
    if !effects.is_empty() {
        campaign::apply_effects(&mut w, campaign_id, &effects, "初始化：人物角色卡").map_err(|e| e.to_string())?;
    }
    Ok((StepStatus::Done, if created.is_empty() { format!("{} 个人物都已就绪", cast.len()) } else { format!("{} 个人物，新建角色卡：{}", cast.len(), created.join("、")) }))
}

/// 关系描述里的词 → 初始关系（信任、好感、敌意）。
fn bond_from_text(t: &str) -> (i64, i64, i64) {
    let has = |ws: &[&str]| ws.iter().any(|w| t.contains(w));
    if has(&["仇", "敌", "恨", "对手", "宿怨"]) {
        (-10, -10, 25)
    } else if has(&["家人", "父", "母", "兄", "姐", "弟", "妹", "恋人", "爱人", "挚友"]) {
        (25, 30, 0)
    } else if has(&["朋友", "好友", "师父", "师傅", "恩人", "伙伴", "一起长大", "人情"]) {
        (20, 20, 0)
    } else if has(&["怀疑", "提防", "不信任", "看不起"]) {
        (-10, -5, 5)
    } else {
        (5, 5, 0)
    }
}

/// 5. 初始化人物关系：按角色身世与剧本写的"与玩家的关系"建立玩家与人物的初始关系（只做一次）。
fn step_relations(world: &Mutex<World>, campaign_id: &str) -> StepOut {
    let mut w = world.lock().map_err(|_| "世界文件被占用".to_string())?;
    let marked = chronicle_core::rules::StateView::record(&chronicle_core::campaign::CampaignView { conn: w.conn(), campaign_id }, "init", "relations").is_some();
    let Some(sid) = script_id(&w, campaign_id)? else {
        return Ok((StepStatus::Skipped, "自由沙盒：关系在游戏中建立".into()));
    };
    let cast = scripts::cast_of(w.conn(), &sid).map_err(|e| e.to_string())?;
    let ids: Vec<String> = cast.iter().map(|m| m.node_id.clone()).collect();
    let world_rel = chronicle_core::network::relations_among(w.conn(), &ids).map(|v| v.len()).unwrap_or(0);
    if marked {
        return Ok((StepStatus::Done, format!("已经建立过；人物之间有 {world_rel} 条设定关系")));
    }
    let pc = chronicle_core::pc::get(w.conn(), campaign_id);
    let pc_rel: Vec<String> = pc.as_ref().and_then(|p| p.backstory.get("relations")).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let mut effects = vec![];
    let mut made = vec![];
    for m in &cast {
        let Ok(Some(node)) = chronicle_core::nodes::get_node(w.conn(), &m.node_id) else { continue };
        // 角色身世里写到这个人（"阿澜：一起长大的朋友"），或剧本写了他与玩家的关系。
        let from_pc = pc_rel.iter().find(|r| r.contains(node.title.as_str()));
        let text = from_pc.cloned().unwrap_or_else(|| m.relation_to_players.clone());
        if text.trim().is_empty() {
            continue;
        }
        let (trust, affection, hostility) = bond_from_text(&text);
        effects.push(Effect::AdjustBond {
            from: rpg::actor(),
            to: m.node_id.clone(),
            kind: String::new(),
            strength: 20,
            trust,
            affection,
            hostility,
            reason: format!("初始关系：{}", text.chars().take(40).collect::<String>()),
            mutual: true,
        });
        made.push(node.title.clone());
    }
    effects.push(Effect::UpsertRecord { kind: "init".into(), id: "relations".into(), data: json!({ "bonds": made }) });
    campaign::apply_effects(&mut w, campaign_id, &effects, "初始化：人物关系").map_err(|e| e.to_string())?;
    Ok((StepStatus::Done, format!("玩家与 {} 个人物建立了初始关系{}；人物之间有 {world_rel} 条设定关系", made.len(), if made.is_empty() { String::new() } else { format!("（{}）", made.join("、")) })))
}

/// 6. 构建 AI GM 上下文：真实组装一次 GM 上下文；有模型服务时发一次很短的请求预热提示词缓存。
fn step_gm_context(world: &Mutex<World>, client: Option<&dyn LlmClient>, templates: &Templates, opts: &GmOptions, campaign_id: &str, cancel: &CancelToken) -> StepOut {
    let t = Instant::now();
    let ctx = {
        let w = world.lock().map_err(|_| "世界文件被占用".to_string())?;
        crate::context::build_gm_context(&w, campaign_id, "", opts.context_budget).map_err(|e| e.to_string())?
    };
    let build_ms = t.elapsed().as_millis();
    let chars = ctx.text.chars().count();
    let mut note = format!("上下文 {chars} 字，在场人物 {} 个，用时 {build_ms} ms", ctx.npcs.len());
    if let Some(c) = client {
        let system = templates.render("system", json!({ "schema_text": "" })).map_err(|e| e.to_string())?;
        let req = ChatRequest {
            messages: vec![ChatMessage::system(system), ChatMessage::user(format!("{}\n\n（初始化预热：只回复「好」）", ctx.text))],
            temperature: Some(0.0),
            max_tokens: Some(4),
            format: OutputFormat::Text,
            stream: false,
            thinking: Some(false),
        };
        let t = Instant::now();
        match c.chat(&req, cancel, &mut |_| {}) {
            Ok(_) => note.push_str(&format!("；模型已预热（{:.1} 秒）", t.elapsed().as_secs_f32())),
            Err(e) if cancel.is_cancelled() => return Err(format!("已取消（{e}）")),
            Err(e) => note.push_str(&format!("；模型服务暂时连不上（{e}），开场时再连接")),
        }
    } else {
        note.push_str("；没有设置模型服务");
    }
    Ok((StepStatus::Done, note))
}

/// 7. 检查世界数据和存档：完整性检查，写第一个检查点（之后可以回溯到这里）。
fn step_integrity(world: &Mutex<World>, campaign_id: &str) -> StepOut {
    let w = world.lock().map_err(|_| "世界文件被占用".to_string())?;
    let rep = chronicle_core::integrity::check(&w).map_err(|e| e.to_string())?;
    let verify = chronicle_core::timeline::verify(w.conn(), campaign_id).map_err(|e| e.to_string())?;
    if !verify.ok {
        return Err("存档的事件重放校验没有通过：请在「时间线」里检查".into());
    }
    chronicle_core::timeline::checkpoint(w.conn(), campaign_id, "init").map_err(|e| e.to_string())?;
    Ok((
        StepStatus::Done,
        if rep.ok { "世界数据完整，存档校验通过，已写入初始检查点".into() } else { format!("存档校验通过，已写入初始检查点；世界数据有 {} 个小问题（可在世界工具修复）", rep.issues.len()) },
    ))
}

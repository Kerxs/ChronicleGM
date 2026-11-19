//! 沉浸式互动引擎：让玩家亲自操作（掷骰、开锁、调查、用物品、机关、对话、潜行、医疗、生存、仪式……），
//! 而不是只用文字告诉 AI 自己做了什么。
//!
//! 约束（和反剧情纠正同时实施）：
//! - **AI 只能提出互动，不能宣布结果**：成败由这里的规则引擎结算（检定、库存、对象状态），状态变化全部经过 `apply_effects`。
//! - **互动选项不是限制**：会话进行中玩家随时可以自由输入，GM 照常分析；离开地点、开始别的互动时会话自动放弃。
//! - **全部持久化**：会话存成战役记录 `interaction`，对象状态 `object_state`，随检查点、分叉、读档一起保存；程序重启后待处理的互动还在。
//! - 每一步都写事件日志（`interaction`），结算结果交给下一次 GM 叙述（`take_results`）。

use std::collections::{BTreeMap, HashSet, VecDeque};

use chronicle_rules::{Effect, StateView};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::campaign::{self, CampaignView};
use crate::checks::{CheckRoll, Degree};
use crate::error::{CoreError, Result};
use crate::rpg;
use crate::world::World;

pub const INTERACTION: &str = "interaction";
pub const OBJECT_STATE: &str = "object_state";
pub const SCENE_OBJECT: &str = "scene_object";
pub const ALERT: &str = "alert";

/// 互动类型。
pub const KINDS: &[(&str, &str)] = &[
    ("check", "检定"),
    ("lock", "开锁"),
    ("item_use", "使用物品"),
    ("investigate", "调查"),
    ("puzzle", "机关谜题"),
    ("social", "交涉"),
    ("combine", "物品组合"),
    ("stealth", "潜行"),
    ("medical", "医疗"),
    ("survival", "生存"),
    ("ritual", "仪式"),
];

pub fn kind_label(k: &str) -> &'static str {
    KINDS.iter().find(|(x, _)| *x == k).map(|(_, l)| *l).unwrap_or("互动")
}

/// 默认的呈现方式：embedded 嵌在对话流里 / local 展开局部操作区 / fullscreen 全屏 / ritual 仪式演出。
pub fn default_presentation(k: &str) -> &'static str {
    match k {
        "check" | "item_use" | "investigate" | "survival" => "embedded",
        "ritual" => "ritual",
        _ => "local",
    }
}

// ============================ 场景对象 ============================

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(default)]
pub struct Lock {
    /// key（钥匙锁）/ pick（只能撬）/ code（密码锁）/ electronic（电子锁）/ bar（门闩，只能破坏或从另一边开）
    pub kind: String,
    /// 能打开它的钥匙（物品 ID 或名字）。
    pub key_item: String,
    /// 密码（密码锁）。
    pub code: String,
    /// 撬锁 / 破解难度。
    pub dc: i64,
    /// 强行破坏的难度。
    pub break_dc: i64,
    /// 备用入口（找到时的说明），空 = 没有。
    pub alt_entry: String,
    /// 密码输错几次触发警报（默认 3）。
    pub tries: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Content {
    pub item: String,
    #[serde(default = "one")]
    pub qty: i64,
}

fn one() -> i64 {
    1
}

/// 状态机谜题：状态是一组 0..modulo 的数，操作把一些位置加上增量（取模）；达到目标即解开。
/// 拉杆（模 2）、旋钮 / 齿轮（模 n）、压力板、图案排列、电路开关都能这样表示。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(default)]
pub struct Puzzle {
    /// levers / gears / plates / pattern / circuit / dials / symbols / combo
    pub kind: String,
    /// 每个位置的名字（例如"左拉杆"）。
    pub labels: Vec<String>,
    /// 每个取值的名字（例如符号），空 = 显示数字。
    pub symbols: Vec<String>,
    pub modulo: i64,
    pub start: Vec<i64>,
    pub goal: Vec<i64>,
    pub ops: Vec<PuzzleOp>,
    /// 步数上限（0 = 不限）；超过即失败。
    pub max_steps: i64,
    /// 多阶段谜题：解开这一阶段后进入下一阶段。
    pub stages: Vec<Puzzle>,
    /// 给玩家看的提示（可以为空）。
    pub hint: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(default)]
pub struct PuzzleOp {
    pub id: String,
    pub label: String,
    /// 每个位置的增量（长度等于状态长度）。
    pub add: Vec<i64>,
    /// 需要的物品（例如"把齿轮装上去"），空 = 不需要。
    pub needs: String,
}

/// 特殊剧情互动（题材无关，由剧本或世界设定声明）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(default)]
pub struct RitualDef {
    /// materials（材料组合）/ resist（精神抵抗）/ dream（梦境探索）/ ability（能力发动）/ symbols（符号破解）
    pub kind: String,
    pub title: String,
    pub materials: Vec<Content>,
    /// 检定用的技能（为空时按类型：抵抗用 意志，梦境用 感知，能力用 奥秘）。
    pub skill: String,
    pub dc: i64,
    /// 抵抗 / 梦境的轮数。
    pub rounds: i64,
    /// 能力发动的消耗（变量名与数值，例如 "mana" 3）。
    pub cost_var: String,
    pub cost: i64,
    /// 符号破解：符号与可选的意思；answer 是正确的译文顺序。
    pub symbols: Vec<String>,
    pub meanings: Vec<String>,
    pub answer: Vec<String>,
    pub success_text: String,
    pub fail_text: String,
    /// 成功后的效果（剧本作者写的，由程序执行）。
    #[ts(type = "unknown[]")]
    pub effects: Vec<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(default)]
pub struct SceneObject {
    pub id: String,
    pub name: String,
    /// door / container / mechanism / body / document / furniture / window / wall / floor / altar / device / other
    pub kind: String,
    /// 所在地点（世界树节点 ID）。
    pub location: String,
    pub description: String,
    /// 隐藏对象：调查达到 find_dc 才会被发现。
    pub hidden: bool,
    pub find_dc: i64,
    pub lock: Option<Lock>,
    pub contents: Vec<Content>,
    /// 调查成功能发现的线索（剧本线索 ID）。
    pub clues: Vec<String>,
    /// 调查难度（默认 10）。
    pub search_dc: i64,
    /// 有帮助的工具标签（例如 light、magnifier）。
    pub tool: String,
    pub mechanism: Option<Puzzle>,
    pub ritual: Option<RitualDef>,
    /// 主持人笔记（不给玩家看）。
    pub notes: String,
    /// script（地点设定里的）/ gm（游戏中由 GM 提出、程序校验后加入）
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(default)]
pub struct ObjectState {
    pub opened: bool,
    pub broken: bool,
    pub discovered: bool,
    /// 搜查过的最高程度：0 没有 / 1 普通 / 2 仔细 / 3 用工具。
    pub searched: i64,
    pub looted: bool,
    pub code_fails: i64,
    pub solved: bool,
    pub alarm: bool,
    /// 用在它上面的物品。
    pub used_items: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ObjectView {
    /// 对象键：`地点ID:对象ID`。
    pub key: String,
    pub object: SceneObject,
    pub state: ObjectState,
    /// 现在能做的互动（类型）。
    pub can: Vec<String>,
}

fn obj_key(o: &SceneObject) -> String {
    format!("{}:{}", o.location, o.id)
}

fn location_objects(conn: &Connection, campaign_id: &str, loc: &str) -> Result<Vec<SceneObject>> {
    let mut out = vec![];
    if let Some(n) = crate::nodes::get_node(conn, loc)?
        && let Some(arr) = n.payload.get("objects").and_then(Value::as_array)
    {
        for (i, v) in arr.iter().enumerate() {
            let Ok(mut o) = serde_json::from_value::<SceneObject>(v.clone()) else { continue };
            if o.name.trim().is_empty() {
                continue;
            }
            if o.id.is_empty() {
                o.id = format!("f{}", i + 1);
            }
            o.location = loc.to_string();
            o.source = "script".into();
            out.push(o);
        }
    }
    let mut stmt = conn.prepare("SELECT data FROM campaign_records WHERE campaign_id = ?1 AND kind = ?2 ORDER BY id")?;
    let rows: Vec<String> = stmt.query_map([campaign_id, SCENE_OBJECT], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    out.extend(rows.into_iter().filter_map(|d| serde_json::from_str::<SceneObject>(&d).ok()).filter(|o| o.location == loc));
    Ok(out)
}

pub fn object_state(conn: &Connection, campaign_id: &str, key: &str) -> ObjectState {
    CampaignView { conn, campaign_id }.record(OBJECT_STATE, key).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn state_effect(key: &str, s: &ObjectState) -> Effect {
    Effect::UpsertRecord { kind: OBJECT_STATE.into(), id: key.into(), data: serde_json::to_value(s).unwrap_or(Value::Null) }
}

fn can_do(o: &SceneObject, s: &ObjectState) -> Vec<String> {
    let mut v = vec!["investigate".to_string()];
    if o.lock.is_some() && !s.opened && !s.broken {
        v.push("lock".into());
    }
    if o.mechanism.is_some() && !s.solved {
        v.push("puzzle".into());
    }
    if o.ritual.is_some() && !s.solved {
        v.push("ritual".into());
    }
    v.push("item_use".into());
    v
}

/// 玩家所在地点的对象（`gm_view` 为假时不含还没发现的隐藏对象）。
pub fn objects_here(conn: &Connection, campaign_id: &str, gm_view: bool) -> Result<Vec<ObjectView>> {
    let Some(loc) = crate::worldsim::party_location(conn, campaign_id)? else { return Ok(vec![]) };
    let mut out = vec![];
    for o in location_objects(conn, campaign_id, &loc)? {
        let key = obj_key(&o);
        let state = object_state(conn, campaign_id, &key);
        if o.hidden && !state.discovered && !gm_view {
            continue;
        }
        let can = can_do(&o, &state);
        let mut object = o;
        if !gm_view {
            object.notes.clear();
            if let Some(l) = object.lock.as_mut() {
                l.code.clear();
                l.alt_entry.clear();
            }
            object.clues.clear();
        }
        out.push(ObjectView { key, object, state, can });
    }
    Ok(out)
}

fn find_object(conn: &Connection, campaign_id: &str, target: &str) -> Result<Option<(SceneObject, ObjectState)>> {
    let t = target.trim();
    if t.is_empty() {
        return Ok(None);
    }
    let Some(loc) = crate::worldsim::party_location(conn, campaign_id)? else { return Ok(None) };
    for o in location_objects(conn, campaign_id, &loc)? {
        let key = obj_key(&o);
        if key == t || o.id == t || o.name == t || (t.chars().count() >= 2 && o.name.contains(t)) {
            let st = object_state(conn, campaign_id, &key);
            return Ok(Some((o, st)));
        }
    }
    Ok(None)
}

/// GM 提出的临时对象（剧本里没有、但场景合理存在的东西，例如"墙角的旧木箱"）。程序校验后加入：
/// 只能放在玩家所在地点；里面的东西只能是剧本里存在、没有被摧毁的物品；不能凭空造线索。
pub fn add_scene_object(world: &mut World, campaign_id: &str, mut o: SceneObject) -> Result<SceneObject> {
    let conn = world.conn();
    let loc = crate::worldsim::party_location(conn, campaign_id)?.ok_or_else(|| CoreError::Invalid("玩家现在不在任何地点，不能添加场景对象".into()))?;
    o.name = o.name.trim().chars().take(30).collect();
    if o.name.is_empty() {
        return Err(CoreError::Invalid("场景对象要有名字".into()));
    }
    if location_objects(conn, campaign_id, &loc)?.iter().any(|x| x.name == o.name) {
        return Err(CoreError::Invalid(format!("这里已经有「{}」了", o.name)));
    }
    o.location = loc;
    o.source = "gm".into();
    o.id = format!("g{}", crate::util::new_id().to_lowercase().chars().rev().take(8).collect::<String>());
    if let Some(l) = o.lock.as_mut() {
        l.dc = l.dc.clamp(5, 30);
        l.break_dc = if l.break_dc <= 0 { l.dc + 2 } else { l.break_dc.clamp(5, 35) };
    }
    o.contents.retain(|c| {
        crate::scripts::get_item(conn, &c.item).ok().flatten().is_some_and(|it| it.kind == crate::model::ScriptItemKind::Item) && !crate::events::is_destroyed(conn, campaign_id, &c.item)
    });
    o.clues.clear();
    o.ritual = None;
    if let Some(p) = &o.mechanism {
        validate_puzzle(p)?;
    }
    campaign::apply_effects(world, campaign_id, &[Effect::UpsertRecord { kind: SCENE_OBJECT.into(), id: o.id.clone(), data: serde_json::to_value(&o)? }], &format!("场景里的「{}」", o.name))?;
    Ok(o)
}

// ============================ 谜题 ============================

fn apply_op(state: &[i64], op: &PuzzleOp, m: i64) -> Vec<i64> {
    state.iter().enumerate().map(|(i, v)| (v + op.add.get(i).copied().unwrap_or(0)).rem_euclid(m)).collect()
}

/// 检查谜题合法且可解（广度优先搜索，状态数有上限）。返回最少步数。
pub fn validate_puzzle(p: &Puzzle) -> Result<i64> {
    let n = p.start.len();
    if n == 0 || n > 12 || p.goal.len() != n || p.modulo < 2 || p.modulo > 12 || p.ops.is_empty() || p.ops.len() > 16 {
        return Err(CoreError::Invalid("谜题格式不对：需要 1–12 个位置、取值 2–12 种、1–16 个操作，目标和初始状态一样长".into()));
    }
    if p.ops.iter().any(|o| o.add.len() != n) {
        return Err(CoreError::Invalid("谜题的每个操作都要给出每个位置的变化".into()));
    }
    if (p.modulo as f64).powi(n as i32) > 300_000.0 {
        return Err(CoreError::Invalid("谜题太大".into()));
    }
    let norm = |v: &[i64]| v.iter().map(|x| x.rem_euclid(p.modulo)).collect::<Vec<_>>();
    let (start, goal) = (norm(&p.start), norm(&p.goal));
    let mut seen: HashSet<Vec<i64>> = HashSet::from([start.clone()]);
    let mut q = VecDeque::from([(start, 0i64)]);
    let mut steps = None;
    while let Some((s, d)) = q.pop_front() {
        if s == goal {
            steps = Some(d);
            break;
        }
        for op in &p.ops {
            let t = apply_op(&s, op, p.modulo);
            if seen.insert(t.clone()) {
                q.push_back((t, d + 1));
            }
        }
    }
    let steps = steps.ok_or_else(|| CoreError::Invalid("这个谜题无解".into()))?;
    if p.max_steps > 0 && steps > p.max_steps {
        return Err(CoreError::Invalid(format!("这个谜题至少要 {steps} 步，超过了步数上限 {}", p.max_steps)));
    }
    for s in &p.stages {
        validate_puzzle(s)?;
    }
    Ok(steps)
}

// ============================ 会话 ============================

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(default)]
pub struct ActionDef {
    pub id: String,
    pub label: String,
    /// 掷骰用的技能（为空表示不掷骰）。
    pub skill: String,
    pub dc: i64,
    /// 需要的物品或工具标签（为空表示不需要）。
    pub needs: String,
    /// 玩家需要输入什么：code（密码）/ item（选物品）/ items（选多个物品）/ amount（数额）/ answer（排列答案）/ ""
    pub input: String,
    pub note: String,
    /// 现在能不能做（缺工具、缺物品时为假，并在 note 写原因）。
    pub available: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(default)]
pub struct Timeout {
    /// none / real（真实秒数）/ game（故事分钟）/ actions（行动次数）
    pub kind: String,
    pub amount: i64,
    /// 开始时间（真实毫秒 / 故事分钟）。
    pub started: i64,
    /// 为什么限时（GM 写明，程序认可才启用）。
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(default)]
pub struct Step {
    pub action: String,
    pub label: String,
    pub roll: Option<CheckRoll>,
    pub text: String,
    pub ok: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(default)]
pub struct Outcome {
    pub success: bool,
    /// 成功等级（大成功 / 成功 / 部分成功 / 失败 / 大失败 / 放弃 / 超时）。
    pub degree: String,
    pub summary: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(default)]
pub struct Interaction {
    pub interaction_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub description: String,
    /// 对象键、人物 ID 或空。
    pub target: String,
    pub target_name: String,
    pub actor: String,
    pub required_items: Vec<String>,
    pub required_attributes: Vec<String>,
    pub difficulty: i64,
    pub available_actions: Vec<ActionDef>,
    /// 规则说明（给玩家看）。
    pub rules: String,
    pub possible_consequences: Vec<String>,
    /// embedded / local / fullscreen / ritual
    pub presentation: String,
    pub timeout_policy: Timeout,
    /// pending / resolved / abandoned / expired
    pub status: String,
    pub steps: Vec<Step>,
    pub result: Option<Outcome>,
    pub created_turn: i64,
    /// 类型自己的状态（谜题状态、潜行加值、抵抗轮数……）。
    #[ts(type = "unknown")]
    pub data: Value,
    /// 结果已经交给 GM 叙述。
    pub consumed: bool,
}

impl Interaction {
    pub fn open(&self) -> bool {
        self.status == "pending"
    }
}

/// 打开互动的请求（GM 提出、或玩家在界面上点场景对象）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(default)]
pub struct OpenRequest {
    #[serde(rename = "type")]
    pub kind: String,
    /// 对象（键、ID、名字）或人物 ID。
    pub target: String,
    pub title: String,
    pub reason: String,
    /// 检定：技能与难度。
    pub skill: String,
    pub dc: i64,
    /// 限时（GM 写明原因时才启用）。
    pub timeout: Option<Timeout>,
    /// GM 提出的临时对象（场景合理存在、剧本里没有的东西）。
    pub new_object: Option<SceneObject>,
    /// GM 提出的谜题（检查可解后才用）。
    pub puzzle: Option<Puzzle>,
}

/// 玩家的一步操作。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(default)]
pub struct PlayerOp {
    pub action: String,
    /// 密码、物品 ID、物品列表、数额、答案……
    #[ts(type = "unknown")]
    pub input: Value,
    /// 亲手掷骰（实体骰子或界面点击）的点数。
    pub dice: Option<Vec<Vec<u32>>>,
    /// 小游戏的表现（0–1，例如撬锁时找准位置的程度），换算成 −3…+3 的修正。
    pub performance: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ActResult {
    pub interaction: Interaction,
    /// 这一步发生了什么（交给叙述）。
    pub lines: Vec<String>,
    pub finished: bool,
    /// 这一步的检定（界面播放骰子）。
    pub roll: Option<CheckRoll>,
}

fn list(conn: &Connection, campaign_id: &str) -> Result<Vec<Interaction>> {
    let mut stmt = conn.prepare("SELECT data FROM campaign_records WHERE campaign_id = ?1 AND kind = ?2 ORDER BY id")?;
    let rows: Vec<String> = stmt.query_map([campaign_id, INTERACTION], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(rows.into_iter().filter_map(|d| serde_json::from_str(&d).ok()).collect())
}

/// 待处理的互动（当前行动者的）。
pub fn pending(conn: &Connection, campaign_id: &str) -> Result<Vec<Interaction>> {
    let me = rpg::actor();
    Ok(list(conn, campaign_id)?.into_iter().filter(|i| i.open() && i.actor == me).collect())
}

/// 最近的互动（含已结束的），给界面显示记录。
pub fn recent(conn: &Connection, campaign_id: &str, n: usize) -> Result<Vec<Interaction>> {
    let mut v = list(conn, campaign_id)?;
    v.reverse();
    v.truncate(n);
    Ok(v)
}

pub fn get(conn: &Connection, campaign_id: &str, id: &str) -> Option<Interaction> {
    CampaignView { conn, campaign_id }.record(INTERACTION, id).and_then(|v| serde_json::from_value(v).ok())
}

fn save_effect(i: &Interaction) -> Effect {
    Effect::UpsertRecord { kind: INTERACTION.into(), id: i.interaction_id.clone(), data: serde_json::to_value(i).unwrap_or(Value::Null) }
}

fn item_name(conn: &Connection, id: &str) -> String {
    rpg::item_info(conn, id).ok().flatten().map(|i| i.title).unwrap_or_else(|| id.to_string())
}

/// 行动者背包里带某个标签（或就是这件物品）的第一件东西。
fn find_tool(conn: &Connection, campaign_id: &str, actor: &str, tag: &str) -> Option<rpg::ItemInfo> {
    if tag.is_empty() {
        return None;
    }
    rpg::inventory(conn, campaign_id, actor).ok()?.into_iter().map(|i| i.item).find(|it| it.tags.iter().any(|t| t == tag) || it.id == tag || it.title == tag)
}

fn has_item(conn: &Connection, campaign_id: &str, actor: &str, id: &str) -> bool {
    CampaignView { conn, campaign_id }.item_qty(actor, id) > 0
}

/// 钥匙：背包里能打开这把锁的物品。
fn key_for(conn: &Connection, campaign_id: &str, actor: &str, o: &SceneObject) -> Option<rpg::ItemInfo> {
    let l = o.lock.as_ref()?;
    rpg::inventory(conn, campaign_id, actor).ok()?.into_iter().map(|i| i.item).find(|it| {
        (!l.key_item.is_empty() && (it.id == l.key_item || it.title == l.key_item)) || it.unlocks.iter().any(|u| *u == o.id || *u == o.name || *u == obj_key(o))
    })
}

fn action(id: &str, label: &str, skill: &str, dc: i64) -> ActionDef {
    ActionDef { id: id.into(), label: label.into(), skill: skill.into(), dc, available: true, ..Default::default() }
}

fn ritual_skill(r: &RitualDef) -> String {
    if !r.skill.is_empty() {
        return r.skill.clone();
    }
    match r.kind.as_str() {
        "resist" => "意志",
        "dream" => "感知",
        "symbols" => "调查",
        _ => "奥秘",
    }
    .into()
}

/// 按类型、对象和玩家现在的条件生成可做的操作（缺工具的标为不可用并写明原因）。
fn build_actions(conn: &Connection, campaign_id: &str, actor: &str, kind: &str, obj: Option<&(SceneObject, ObjectState)>, data: &Value, req: &OpenRequest) -> Vec<ActionDef> {
    let mut v = vec![];
    match kind {
        "check" => v.push(action("roll", "掷骰", if req.skill.is_empty() { "运气" } else { &req.skill }, req.dc.max(5))),
        "lock" => {
            if let Some((o, _)) = obj
                && let Some(l) = &o.lock
            {
                let key = key_for(conn, campaign_id, actor, o);
                let mut a = action("key", "用钥匙开", "", 0);
                match (&key, l.kind.as_str()) {
                    (Some(k), _) => a.label = format!("用「{}」开", k.title),
                    (None, _) => {
                        a.available = false;
                        a.note = "身上没有能开这把锁的钥匙".into();
                    }
                }
                v.push(a);
                if l.kind != "code" && l.kind != "bar" {
                    let mut p = action("pick", "撬锁", "开锁", l.dc.max(8));
                    p.needs = "lockpick".into();
                    if find_tool(conn, campaign_id, actor, "lockpick").is_none() {
                        p.note = "没有撬锁工具：只能用手边的东西凑合（难度 +3）".into();
                    }
                    v.push(p);
                }
                if l.kind == "code" {
                    let mut c = action("code", "输入密码", "", 0);
                    c.input = "code".into();
                    c.note = format!("输错 {} 次会触发警报", if l.tries > 0 { l.tries } else { 3 });
                    v.push(c);
                }
                if l.kind == "electronic" {
                    let mut e = action("hack", "破解电子锁", "电子", l.dc.max(10));
                    e.needs = "hacking".into();
                    v.push(e);
                }
                v.push(action("break", "强行破坏", "运动", if l.break_dc > 0 { l.break_dc } else { l.dc + 2 }));
                v.push(action("alt", "寻找别的入口", "调查", 12));
            }
        }
        "investigate" => {
            let dc = obj.map(|(o, _)| if o.search_dc > 0 { o.search_dc } else { 10 }).unwrap_or(10);
            v.push(action("look", "普通检查", "调查", dc));
            let mut c = action("careful", "仔细检查（多花时间）", "调查", dc);
            c.note = "花 30 分钟，检定 +2".into();
            v.push(c);
            let tool = obj.map(|(o, _)| o.tool.clone()).unwrap_or_default();
            let mut t = action("tool", "用工具检查", "调查", dc);
            t.needs = if tool.is_empty() { "light".into() } else { tool.clone() };
            if find_tool(conn, campaign_id, actor, &t.needs).is_none() {
                t.available = false;
                t.note = "身上没有合适的工具".into();
            } else {
                t.note = "有工具帮忙（优势）".into();
            }
            v.push(t);
        }
        "item_use" => {
            let mut a = action("use", "使用物品", "", 0);
            a.input = "item".into();
            v.push(a);
        }
        "combine" => {
            let mut a = action("combine", "组合", "", 0);
            a.input = "items".into();
            v.push(a);
        }
        "puzzle" => {
            if let Some(p) = puzzle_now(data) {
                for op in &p.ops {
                    let mut a = action(&format!("op:{}", op.id), &op.label, "", 0);
                    if !op.needs.is_empty() && !has_item(conn, campaign_id, actor, &op.needs) && find_tool(conn, campaign_id, actor, &op.needs).is_none() {
                        a.available = false;
                        a.note = format!("需要「{}」", item_name(conn, &op.needs));
                    }
                    v.push(a);
                }
                v.push(action("reset", "复位", "", 0));
            }
        }
        "social" => {
            for (id, label, skill) in [("ask", "询问", ""), ("persuade", "说服", "说服"), ("threaten", "威胁", "威吓"), ("deceive", "欺骗", "欺瞒"), ("interrogate", "盘问", "洞察"), ("negotiate", "谈判", "交涉"), ("leverage", "利用交情", "说服")] {
                v.push(action(id, label, skill, if skill.is_empty() { 0 } else { 12 }));
            }
            let mut b = action("bribe", "贿赂", "说服", 10);
            b.input = "amount".into();
            v.push(b);
        }
        "stealth" => {
            let lvl = data.get("level").and_then(Value::as_i64).unwrap_or(0);
            v.push(action("observe", "观察守卫", "感知", 12));
            v.push(action("hide", "找掩体躲藏", "潜行", 10 + lvl * 2));
            let mut d = action("distract", "制造声响引开注意", "欺瞒", 12);
            d.input = "item".into();
            d.note = "可以扔出一件东西".into();
            v.push(d);
            v.push(action("disable", "关闭监控 / 机关", "电子", 14));
            v.push(action("route", "规划路线", "调查", 12));
            v.push(action("sneak", "潜行通过", "潜行", 12 + lvl * 2));
        }
        "medical" => {
            for (id, label, skill, dc, need) in [("first_aid", "急救", "医疗", 12, ""), ("bandage", "包扎止血", "医疗", 10, "bandage"), ("extract", "取出异物", "医疗", 14, "tweezers"), ("antidote", "解毒", "医疗", 12, "medkit"), ("stabilize", "稳住伤势", "医疗", 10, "")] {
                let mut a = action(id, label, skill, dc);
                a.needs = need.into();
                if !need.is_empty() && find_tool(conn, campaign_id, actor, need).is_none() {
                    a.note = "没有合适的工具（难度 +4）".into();
                }
                v.push(a);
            }
        }
        "survival" => {
            v.push(action("forage", "寻找食物", "生存", 12));
            v.push(action("water", "寻找水源", "生存", 10));
            let mut e = action("eat", "吃东西", "", 0);
            e.needs = "food".into();
            e.input = "item".into();
            v.push(e);
            let mut d = action("drink", "喝水", "", 0);
            d.needs = "water".into();
            d.input = "item".into();
            v.push(d);
        }
        "ritual" => {
            if let Some((o, _)) = obj
                && let Some(r) = &o.ritual
            {
                match r.kind.as_str() {
                    "materials" => {
                        let mut a = action("perform", "放上材料，举行仪式", &if r.dc > 0 { ritual_skill(r) } else { String::new() }, r.dc);
                        a.input = "items".into();
                        v.push(a);
                    }
                    "symbols" => {
                        let mut a = action("decode", "破解符号", "", 0);
                        a.input = "answer".into();
                        v.push(a);
                        v.push(action("study", "研究符号（检定）", &ritual_skill(r), r.dc.max(10)));
                    }
                    "ability" => v.push(action("invoke", "发动", &ritual_skill(r), r.dc.max(10))),
                    _ => v.push(action("endure", if r.kind == "dream" { "在梦境里前进" } else { "抵抗" }, &ritual_skill(r), r.dc.max(10))),
                }
            }
        }
        _ => {}
    }
    v.push(action("give_up", "放弃", "", 0));
    v
}

fn puzzle_now(data: &Value) -> Option<Puzzle> {
    let p: Puzzle = serde_json::from_value(data.get("puzzle")?.clone()).ok()?;
    let stage = data.get("stage").and_then(Value::as_u64).unwrap_or(0) as usize;
    if stage == 0 { Some(p) } else { p.stages.get(stage - 1).cloned() }
}

fn rules_text(kind: &str) -> String {
    match kind {
        "check" => "程序掷骰，结果按本战役的检定规则判定。",
        "lock" => "钥匙要在身上；撬锁、破解、破坏都要检定，失败可能弄出声响或弄断工具；密码输错太多次会触发警报。",
        "investigate" => "调查结果取决于你的能力、工具和这里实际有什么——不保证每次都有发现。同样的检查做第二遍不会有新结果。",
        "item_use" => "物品要在你身上，效果按物品和对象的设定判定。",
        "combine" => "要有对应的配方和材料。",
        "puzzle" => "每一步都会改变机关的状态，达到目标就解开；有步数限制时超过即失败。",
        "social" => "对方按自己的性格、情绪、利益和对你的看法回应；检定决定你的说法有没有效果，具体怎么说还是由你和对方决定。",
        "stealth" => "警戒分四级：未察觉、怀疑、搜索、发现。失败会提高警戒，被发现后的事按世界规则处理。",
        "medical" => "不同的伤需要不同的工具和本事。",
        "survival" => "食物和水会随时间消耗，太低会有惩罚。",
        "ritual" => "按剧本设定的规则进行，成败由程序判定。",
        _ => "",
    }
    .into()
}

/// 打开一个互动。程序按对象、物品和规则校验并生成可做的操作；同一个目标已有待处理的互动时先放弃旧的。
pub fn open(world: &mut World, campaign_id: &str, req: &OpenRequest) -> Result<Interaction> {
    let kind = req.kind.trim();
    if !KINDS.iter().any(|(k, _)| *k == kind) {
        return Err(CoreError::Invalid(format!("不认识的互动类型：{kind}")));
    }
    let actor = rpg::actor();
    if let Some(no) = &req.new_object
        && find_object(world.conn(), campaign_id, &no.name)?.is_none()
    {
        add_scene_object(world, campaign_id, no.clone())?;
    }
    let conn = world.conn();
    let obj = match kind {
        "lock" | "puzzle" | "ritual" => Some(find_object(conn, campaign_id, if req.target.is_empty() { req.new_object.as_ref().map(|o| o.name.as_str()).unwrap_or("") } else { &req.target })?.ok_or_else(|| CoreError::Invalid(format!("这里没有「{}」", req.target)))?),
        "investigate" | "item_use" => find_object(conn, campaign_id, if req.target.is_empty() { req.new_object.as_ref().map(|o| o.name.as_str()).unwrap_or("") } else { &req.target })?,
        _ => None,
    };
    let mut data = json!({});
    let (target, target_name) = match kind {
        "lock" => {
            let (o, s) = obj.as_ref().unwrap();
            if o.lock.is_none() {
                return Err(CoreError::Invalid(format!("「{}」没有锁", o.name)));
            }
            if s.opened || s.broken {
                return Err(CoreError::Invalid(format!("「{}」已经开了", o.name)));
            }
            (obj_key(o), o.name.clone())
        }
        "puzzle" => {
            let (o, s) = obj.as_ref().unwrap();
            let p = o.mechanism.clone().or_else(|| req.puzzle.clone()).ok_or_else(|| CoreError::Invalid(format!("「{}」不是机关", o.name)))?;
            validate_puzzle(&p)?;
            if s.solved {
                return Err(CoreError::Invalid(format!("「{}」已经解开了", o.name)));
            }
            data = json!({ "puzzle": p, "state": p.start, "stage": 0, "steps": 0 });
            (obj_key(o), o.name.clone())
        }
        "ritual" => {
            let (o, s) = obj.as_ref().unwrap();
            if o.ritual.is_none() {
                return Err(CoreError::Invalid(format!("「{}」上没有可以进行的仪式", o.name)));
            }
            if s.solved {
                return Err(CoreError::Invalid(format!("「{}」的仪式已经完成了", o.name)));
            }
            // 符号破解：给玩家看符号和可选的意思（正确答案混在里面，顺序打乱），不给答案。
            let r = o.ritual.clone().unwrap_or_default();
            let mut meanings: Vec<String> = r.meanings.clone();
            for a in &r.answer {
                if !meanings.contains(a) {
                    meanings.push(a.clone());
                }
            }
            meanings.sort_by_key(|m| crate::timeline::checksum(m));
            data = json!({ "round": 0, "passes": 0, "fails": 0, "symbols": r.symbols, "meanings": meanings });
            (obj_key(o), o.name.clone())
        }
        "social" | "medical" => {
            let t = req.target.trim();
            if t.is_empty() || t == "self" || t == actor || kind == "medical" && t == "自己" {
                if kind == "social" {
                    return Err(CoreError::Invalid("交涉要选一个在场的人物".into()));
                }
                (actor.clone(), "自己".into())
            } else {
                let n = crate::nodes::get_node(conn, t)?.or_else(|| crate::nodes::find_by_name(conn, t, Some(crate::model::NodeKind::Character)).ok().and_then(|v| v.into_iter().next())).ok_or_else(|| CoreError::Invalid(format!("没有找到人物「{t}」")))?;
                if matches!(CampaignView { conn, campaign_id }.life_status(&n.id).as_deref(), Some("dead" | "missing")) && kind == "social" {
                    return Err(CoreError::Invalid(format!("{}已经不在了", n.title)));
                }
                (n.id.clone(), n.title.clone())
            }
        }
        "stealth" => {
            let loc = crate::worldsim::party_location(conn, campaign_id)?.unwrap_or_default();
            let a = alert(conn, campaign_id, &loc);
            data = json!({ "level": a.level, "bonus": 0, "location": loc });
            (loc.clone(), crate::nodes::get_node(conn, &loc)?.map(|n| n.title).unwrap_or_else(|| "这里".into()))
        }
        "survival" => {
            if !rpg::rules(conn, campaign_id).survival {
                return Err(CoreError::Invalid("这个剧本没有开启生存规则".into()));
            }
            (String::new(), String::new())
        }
        _ => match &obj {
            Some((o, _)) => (obj_key(o), o.name.clone()),
            None => (String::new(), String::new()),
        },
    };
    // 同一个目标的旧会话放弃。
    let mut eff = vec![];
    for mut old in pending(conn, campaign_id)?.into_iter().filter(|i| i.target == target && i.kind == kind) {
        old.status = "abandoned".into();
        old.result = Some(Outcome { success: false, degree: "放弃".into(), summary: "开始了新的同类互动".into() });
        old.consumed = true;
        eff.push(save_effect(&old));
    }
    let turn = campaign::get_campaign(conn, campaign_id)?.turn;
    // 限时：只在 GM 写明原因时启用（普通探索和对话不限时）。
    let timeout = match &req.timeout {
        Some(t) if matches!(t.kind.as_str(), "real" | "game" | "actions") && t.amount > 0 && t.reason.trim().chars().count() >= 4 && !matches!(kind, "social" | "investigate") => Timeout {
            kind: t.kind.clone(),
            amount: t.amount.clamp(1, 600),
            started: if t.kind == "real" { crate::util::now_ms() } else if t.kind == "game" { crate::schedule::clock(conn, campaign_id) } else { 0 },
            reason: t.reason.trim().chars().take(80).collect(),
        },
        _ => Timeout { kind: "none".into(), ..Default::default() },
    };
    let actions = build_actions(conn, campaign_id, &actor, kind, obj.as_ref(), &data, req);
    let dc = actions.iter().filter(|a| a.dc > 0).map(|a| a.dc).min().unwrap_or(0);
    let title = if req.title.trim().is_empty() { format!("{}{}", kind_label(kind), if target_name.is_empty() { String::new() } else { format!("：{target_name}") }) } else { req.title.trim().chars().take(40).collect() };
    let consequences = match kind {
        "lock" => vec!["撬锁失败会弄出声响".into(), "大失败会弄断工具".into()],
        "stealth" => vec!["失败会提高警戒".into(), "被发现后可能发生冲突".into()],
        "puzzle" => vec!["超过步数可能触发机关".into()],
        _ => vec![],
    };
    let i = Interaction {
        interaction_id: format!("x{:04}-{}", list(conn, campaign_id)?.len() + 1, &crate::util::new_id()[20..]),
        kind: kind.into(),
        title,
        description: obj.as_ref().map(|(o, _)| o.description.clone()).unwrap_or_default(),
        target,
        target_name,
        actor: actor.clone(),
        required_items: obj.as_ref().and_then(|(o, _)| o.lock.as_ref().map(|l| l.key_item.clone())).filter(|s| !s.is_empty()).into_iter().collect(),
        required_attributes: actions.iter().filter(|a| !a.skill.is_empty()).map(|a| a.skill.clone()).collect::<std::collections::BTreeSet<_>>().into_iter().collect(),
        difficulty: dc,
        available_actions: actions,
        rules: rules_text(kind),
        possible_consequences: consequences,
        presentation: default_presentation(kind).into(),
        timeout_policy: timeout,
        status: "pending".into(),
        steps: vec![],
        result: None,
        created_turn: turn,
        data,
        consumed: false,
    };
    eff.push(save_effect(&i));
    campaign::apply_effects(world, campaign_id, &eff, &format!("开始互动：{}", i.title))?;
    campaign::record(world.conn(), campaign_id, "interaction", &json!({ "id": i.interaction_id, "type": i.kind, "title": i.title, "stage": "open" }))?;
    Ok(i)
}

// ============================ 结算 ============================

struct Ctx<'a> {
    world: &'a mut World,
    campaign_id: &'a str,
    actor: String,
    eff: Vec<Effect>,
    lines: Vec<String>,
    roll: Option<CheckRoll>,
}

impl Ctx<'_> {
    fn conn(&self) -> &Connection {
        self.world.conn()
    }
    /// 程序掷骰（技能 + 难度设置 + 环境），可以用玩家亲手掷的点数。
    fn check(&mut self, skill: &str, dc: i64, adv: i8, extra: i64, reason: &str, dice: Option<Vec<Vec<u32>>>) -> Result<CheckRoll> {
        let rules = rpg::rules(self.conn(), self.campaign_id);
        let dc = if rules.check.mode == "pool" { dc.clamp(1, 6) } else { dc.clamp(1, 100) };
        let (dc, diff_note) = rpg::difficulty_dc(&rules, dc);
        let mut modifier = rpg::check_modifier(self.conn(), self.campaign_id, &self.actor, skill) + extra;
        let loc = crate::worldsim::party_location(self.conn(), self.campaign_id)?;
        let local = crate::worldsim::local_environment(self.conn(), self.campaign_id, loc.as_deref(), crate::schedule::clock(self.conn(), self.campaign_id))?;
        let lit = find_tool(self.conn(), self.campaign_id, &self.actor, "light").is_some();
        let (env, env_note) = crate::worldsim::env_check_modifier(&local, skill, lit);
        modifier += env;
        let reason = format!("{skill} 检定：{reason}");
        let mut r = crate::checks::perform_with(self.world, self.campaign_id, &rules.check, modifier, dc, adv, &reason, dice)?;
        if !diff_note.is_empty() {
            r.explain = format!("{}（{diff_note}）", r.explain);
        }
        if !env_note.is_empty() {
            r.explain = format!("{}（环境：{env_note}）", r.explain);
        }
        self.lines.push(format!("{}：{}（{}）", reason, r.degree.label(), r.explain));
        self.roll = Some(r.clone());
        Ok(r)
    }
    fn line(&mut self, s: impl Into<String>) {
        self.lines.push(s.into());
    }
}

/// 警戒状态（潜行）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(default)]
pub struct Alert {
    /// 0 未察觉 / 1 怀疑 / 2 搜索 / 3 发现
    pub level: i64,
    pub points: i64,
    /// 最近一次变化的故事时间（分钟）。
    pub at: i64,
}

pub const ALERT_LABELS: [&str; 4] = ["未察觉", "怀疑", "搜索", "发现"];

pub fn alert(conn: &Connection, campaign_id: &str, loc: &str) -> Alert {
    let mut a: Alert = CampaignView { conn, campaign_id }.record(ALERT, loc).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    // 随时间平息：每 30 分钟降 1 点（已经"发现"的不自动平息）。
    if a.level < 3 && a.points > 0 {
        let now = crate::schedule::clock(conn, campaign_id);
        let calm = ((now - a.at) / 30).max(0);
        a.points = (a.points - calm).max(0);
        a.level = level_of(a.points);
    }
    a
}

fn level_of(points: i64) -> i64 {
    match points {
        i64::MIN..=1 => 0,
        2..=3 => 1,
        4..=5 => 2,
        _ => 3,
    }
}

fn raise_alert(c: &mut Ctx, loc: &str, by: i64) -> Alert {
    if loc.is_empty() || by == 0 {
        return Alert::default();
    }
    let mut a = alert(c.conn(), c.campaign_id, loc);
    let before = a.level;
    a.points = (a.points + by).clamp(0, 8);
    a.level = level_of(a.points).max(if by > 0 { before } else { 0 });
    a.at = crate::schedule::clock(c.conn(), c.campaign_id);
    c.eff.push(Effect::UpsertRecord { kind: ALERT.into(), id: loc.into(), data: serde_json::to_value(&a).unwrap_or(Value::Null) });
    if a.level != before {
        c.line(format!("警戒：{} → {}", ALERT_LABELS[before as usize], ALERT_LABELS[a.level as usize]));
    }
    a
}

/// 打开对象：标记打开；容器里的东西归玩家；对象上的线索揭示。
fn open_object(c: &mut Ctx, key: &str, how: &str) -> Result<()> {
    let Some((o, mut s)) = find_object(c.conn(), c.campaign_id, key)? else { return Ok(()) };
    s.opened = true;
    if how == "break" {
        s.broken = true;
    }
    c.line(format!("「{}」打开了", o.name));
    if !s.looted && !o.contents.is_empty() {
        for it in &o.contents {
            if crate::events::is_destroyed(c.conn(), c.campaign_id, &it.item) && !crate::events::repeatable(c.conn(), &it.item) {
                continue;
            }
            c.eff.push(Effect::GiveItem { holder: c.actor.clone(), item: it.item.clone(), qty: it.qty.max(1) });
            c.eff.push(Effect::UpsertRecord { kind: "item_source".into(), id: format!("{}|{}", c.actor, it.item), data: json!({ "source": format!("从「{}」里拿到", o.name) }) });
            let n = item_name(c.conn(), &it.item);
            c.line(format!("从「{}」里拿到「{n}」×{}", o.name, it.qty.max(1)));
        }
        s.looted = true;
    }
    c.eff.push(state_effect(key, &s));
    Ok(())
}

fn reveal_clues(c: &mut Ctx, o: &SceneObject, all: bool) {
    let view = CampaignView { conn: c.conn(), campaign_id: c.campaign_id };
    let new: Vec<String> = o.clues.iter().filter(|id| !view.clue_known(id)).cloned().collect();
    for id in new.iter().take(if all { usize::MAX } else { 1 }) {
        let title = crate::scripts::get_item(c.conn(), id).ok().flatten().map(|i| i.title).unwrap_or_default();
        c.eff.push(Effect::RevealClue { clue: id.clone() });
        c.line(format!("发现线索「{title}」"));
    }
}

fn perf_mod(p: Option<f64>) -> i64 {
    p.map(|x| ((x.clamp(0.0, 1.0) - 0.5) * 6.0).round() as i64).unwrap_or(0)
}

fn finish(i: &mut Interaction, success: bool, degree: &str, summary: &str) {
    i.status = "resolved".into();
    i.result = Some(Outcome { success, degree: degree.into(), summary: summary.chars().take(200).collect() });
}

/// 玩家做一步操作。返回这一步的结果（结束时结果会交给下一次 GM 叙述）。
pub fn act(world: &mut World, campaign_id: &str, id: &str, op: &PlayerOp) -> Result<ActResult> {
    let mut i = get(world.conn(), campaign_id, id).ok_or_else(|| CoreError::NotFound("互动", id.into()))?;
    if !i.open() {
        return Err(CoreError::Invalid("这个互动已经结束了".into()));
    }
    let actor = i.actor.clone();
    let mut c = Ctx { world, campaign_id, actor: actor.clone(), eff: vec![], lines: vec![], roll: None };
    // 限时：真实时间 / 故事时间 / 行动次数。
    let expired = match i.timeout_policy.kind.as_str() {
        "real" => crate::util::now_ms() - i.timeout_policy.started > i.timeout_policy.amount * 1000,
        "game" => crate::schedule::clock(c.conn(), campaign_id) - i.timeout_policy.started > i.timeout_policy.amount,
        "actions" => i.steps.len() as i64 >= i.timeout_policy.amount,
        _ => false,
    };
    if expired || op.action == "timeout" {
        i.status = "expired".into();
        i.result = Some(Outcome { success: false, degree: "超时".into(), summary: format!("时间到了：{}", i.timeout_policy.reason) });
        c.line(format!("「{}」时间到了（{}）", i.title, i.timeout_policy.reason));
        return commit(c, i, true);
    }
    if op.action == "give_up" {
        i.status = "abandoned".into();
        i.result = Some(Outcome { success: false, degree: "放弃".into(), summary: "玩家放弃了".into() });
        c.line(format!("放弃了「{}」", i.title));
        return commit(c, i, true);
    }
    let a = i.available_actions.iter().find(|a| a.id == op.action).cloned().ok_or_else(|| CoreError::Invalid(format!("不能这样做：{}", op.action)))?;
    if !a.available {
        return Err(CoreError::Invalid(if a.note.is_empty() { "现在不能这样做".into() } else { a.note.clone() }));
    }
    let mut step = Step { action: a.id.clone(), label: a.label.clone(), ..Default::default() };
    let done = match i.kind.as_str() {
        "check" => {
            let r = c.check(&a.skill, a.dc, 0, 0, &i.title, op.dice.clone())?;
            step.ok = r.degree.ok();
            let summary = format!("{}：{}", i.title, r.degree.label());
            finish(&mut i, r.degree.ok(), r.degree.label(), &summary);
            true
        }
        "lock" => act_lock(&mut c, &mut i, &a, op, &mut step)?,
        "investigate" => act_investigate(&mut c, &mut i, &a, op, &mut step)?,
        "item_use" => act_item_use(&mut c, &mut i, op, &mut step)?,
        "combine" => act_combine(&mut c, &mut i, op, &mut step)?,
        "puzzle" => act_puzzle(&mut c, &mut i, &a, &mut step)?,
        "social" => act_social(&mut c, &mut i, &a, op, &mut step)?,
        "stealth" => act_stealth(&mut c, &mut i, &a, op, &mut step)?,
        "medical" => act_medical(&mut c, &mut i, &a, op, &mut step)?,
        "survival" => act_survival(&mut c, &mut i, &a, op, &mut step)?,
        "ritual" => act_ritual(&mut c, &mut i, &a, op, &mut step)?,
        _ => return Err(CoreError::Invalid("不认识的互动类型".into())),
    };
    step.roll = c.roll.clone();
    step.text = c.lines.join("；");
    i.steps.push(step);
    // 行动次数用完：没成功就算超时。
    if !done && i.timeout_policy.kind == "actions" && i.steps.len() as i64 >= i.timeout_policy.amount {
        i.status = "expired".into();
        i.result = Some(Outcome { success: false, degree: "超时".into(), summary: format!("机会用完了：{}", i.timeout_policy.reason) });
        c.line("机会用完了");
        return commit(c, i, true);
    }
    // 操作改变了条件（例如拿到了工具）：重新生成可做的操作。
    if !done {
        let obj = find_object(c.conn(), campaign_id, &i.target)?;
        let req = OpenRequest { kind: i.kind.clone(), skill: i.available_actions.first().map(|a| a.skill.clone()).unwrap_or_default(), dc: i.difficulty, ..Default::default() };
        i.available_actions = build_actions(c.conn(), campaign_id, &actor, &i.kind, obj.as_ref(), &i.data, &req);
    }
    commit(c, i, done)
}

fn commit(mut c: Ctx, i: Interaction, finished: bool) -> Result<ActResult> {
    c.eff.push(save_effect(&i));
    let reason = format!("互动「{}」：{}", i.title, c.lines.first().cloned().unwrap_or_default());
    let eff = std::mem::take(&mut c.eff);
    campaign::apply_effects(c.world, c.campaign_id, &eff, &reason)?;
    campaign::record(c.world.conn(), c.campaign_id, "interaction", &json!({ "id": i.interaction_id, "type": i.kind, "title": i.title, "lines": c.lines, "status": i.status, "result": i.result }))?;
    Ok(ActResult { interaction: i, lines: c.lines, finished, roll: c.roll })
}

fn act_lock(c: &mut Ctx, i: &mut Interaction, a: &ActionDef, op: &PlayerOp, step: &mut Step) -> Result<bool> {
    let Some((o, mut s)) = find_object(c.conn(), c.campaign_id, &i.target)? else { return Err(CoreError::Invalid("对象不见了".into())) };
    let l = o.lock.clone().unwrap_or_default();
    let key = i.target.clone();
    let loc = o.location.clone();
    match a.id.as_str() {
        "key" => {
            let k = key_for(c.conn(), c.campaign_id, &c.actor, &o).ok_or_else(|| CoreError::Invalid("身上没有能开这把锁的钥匙".into()))?;
            c.line(format!("用「{}」打开了「{}」的锁", k.title, o.name));
            open_object(c, &key, "key")?;
            step.ok = true;
            finish(i, true, "成功", &format!("用钥匙打开了「{}」", o.name));
            Ok(true)
        }
        "pick" => {
            let tool = find_tool(c.conn(), c.campaign_id, &c.actor, "lockpick");
            let dc = a.dc + if tool.is_none() { 3 } else { 0 };
            let r = c.check("开锁", dc, 0, perf_mod(op.performance), &format!("撬开「{}」", o.name), op.dice.clone())?;
            step.ok = r.degree.ok();
            if r.degree.ok() {
                open_object(c, &key, "pick")?;
                finish(i, true, r.degree.label(), &format!("撬开了「{}」{}", o.name, if r.degree == Degree::Partial { "，但弄出了一点声响" } else { "" }));
                if r.degree == Degree::Partial {
                    raise_alert(c, &loc, 1);
                }
                return Ok(true);
            }
            raise_alert(c, &loc, if r.degree == Degree::CritFailure { 2 } else { 1 });
            if r.degree == Degree::CritFailure
                && let Some(t) = &tool
            {
                c.eff.push(Effect::TakeItem { holder: c.actor.clone(), item: t.id.clone(), qty: 1 });
                c.line(format!("「{}」断了", t.title));
            }
            c.line("锁没有开，可以再试，或者换个办法");
            Ok(false)
        }
        "code" => {
            let got = op.input.as_str().map(|s| s.trim().to_string()).or_else(|| op.input.get("code").and_then(Value::as_str).map(|s| s.trim().to_string())).unwrap_or_default();
            if got.is_empty() {
                return Err(CoreError::Invalid("请输入密码".into()));
            }
            if !l.code.is_empty() && got == l.code.trim() {
                c.line(format!("密码正确，「{}」开了", o.name));
                open_object(c, &key, "code")?;
                step.ok = true;
                finish(i, true, "成功", &format!("输入正确的密码打开了「{}」", o.name));
                return Ok(true);
            }
            s.code_fails += 1;
            let tries = if l.tries > 0 { l.tries } else { 3 };
            c.line(format!("密码不对（已经错了 {} 次，共 {tries} 次机会）", s.code_fails));
            if s.code_fails >= tries {
                s.alarm = true;
                c.eff.push(state_effect(&key, &s));
                raise_alert(c, &loc, 6);
                c.line("警报响了");
                finish(i, false, "失败", &format!("「{}」的密码输错太多次，触发了警报", o.name));
                return Ok(true);
            }
            c.eff.push(state_effect(&key, &s));
            Ok(false)
        }
        "hack" => {
            let tool = find_tool(c.conn(), c.campaign_id, &c.actor, "hacking");
            let r = c.check("电子", a.dc, if tool.is_some() { 1 } else { 0 }, perf_mod(op.performance), &format!("破解「{}」", o.name), op.dice.clone())?;
            step.ok = r.degree.ok();
            if r.degree.ok() {
                open_object(c, &key, "hack")?;
                finish(i, true, r.degree.label(), &format!("破解了「{}」", o.name));
                return Ok(true);
            }
            if r.degree == Degree::CritFailure {
                s.alarm = true;
                c.eff.push(state_effect(&key, &s));
                raise_alert(c, &loc, 6);
                finish(i, false, "大失败", &format!("破解「{}」时触发了警报", o.name));
                return Ok(true);
            }
            raise_alert(c, &loc, 1);
            Ok(false)
        }
        "break" => {
            let r = c.check("运动", a.dc, 0, 0, &format!("强行破坏「{}」", o.name), op.dice.clone())?;
            step.ok = r.degree.ok();
            raise_alert(c, &loc, if r.degree.ok() { 2 } else { 3 });
            if r.degree.ok() {
                open_object(c, &key, "break")?;
                finish(i, true, r.degree.label(), &format!("砸开了「{}」（动静很大）", o.name));
                return Ok(true);
            }
            c.line(format!("「{}」很结实，没能弄开", o.name));
            Ok(false)
        }
        "alt" => {
            let r = c.check("调查", a.dc, 0, 0, &format!("在「{}」附近找别的入口", o.name), op.dice.clone())?;
            step.ok = r.degree.ok() && !l.alt_entry.is_empty();
            if step.ok {
                c.line(format!("找到了别的入口：{}", l.alt_entry));
                s.opened = true;
                c.eff.push(state_effect(&key, &s));
                finish(i, true, r.degree.label(), &format!("绕过「{}」：{}", o.name, l.alt_entry));
                return Ok(true);
            }
            c.line(if l.alt_entry.is_empty() { "附近没有别的入口".to_string() } else { "没有找到别的入口".to_string() });
            // 这个办法试过了：从可选操作里拿掉（避免反复掷骰刷结果）。
            i.available_actions.retain(|x| x.id != "alt");
            Ok(false)
        }
        _ => Err(CoreError::Invalid("不能这样做".into())),
    }
}

fn act_investigate(c: &mut Ctx, i: &mut Interaction, a: &ActionDef, op: &PlayerOp, step: &mut Step) -> Result<bool> {
    let level = match a.id.as_str() {
        "careful" => 2,
        "tool" => 3,
        _ => 1,
    };
    let target = find_object(c.conn(), c.campaign_id, &i.target)?;
    if let Some((o, s)) = &target
        && s.searched >= level
    {
        c.line(format!("「{}」已经这样检查过了，没有新的发现", o.name));
        step.ok = false;
        finish(i, false, "没有新发现", &format!("「{}」已经检查过了", o.name));
        return Ok(true);
    }
    let (extra, adv) = match level {
        2 => (2, 0),
        3 => (0, 1),
        _ => (0, 0),
    };
    let what = target.as_ref().map(|(o, _)| format!("「{}」", o.name)).unwrap_or_else(|| "四周".into());
    let r = c.check("调查", a.dc, adv, extra, &format!("检查{what}"), op.dice.clone())?;
    step.ok = r.degree.ok();
    if level == 2 {
        crate::schedule::advance(c.world, c.campaign_id, 30, "仔细检查")?;
    }
    let mut found = false;
    if let Some((o, mut s)) = target {
        s.searched = s.searched.max(level);
        if r.degree.ok() {
            let before = c.lines.len();
            reveal_clues(c, &o, r.degree == Degree::CritSuccess || r.degree == Degree::Success);
            if o.lock.is_none() && !s.looted && !o.contents.is_empty() {
                c.eff.push(state_effect(&i.target, &s));
                open_object(c, &i.target.clone(), "search")?;
                s = object_state(c.conn(), c.campaign_id, &i.target);
                s.looted = true;
                s.searched = s.searched.max(level);
            }
            found = c.lines.len() > before;
            if !found {
                c.line(format!("{what}没有什么特别的"));
            }
        }
        c.eff.push(state_effect(&i.target, &s));
    }
    // 隐藏的对象：检定总值达到 find_dc 就发现（大成功一定发现）。
    if r.degree.ok()
        && let Some(loc) = crate::worldsim::party_location(c.conn(), c.campaign_id)?
    {
        for h in location_objects(c.conn(), c.campaign_id, &loc)?.into_iter().filter(|o| o.hidden) {
            let k = obj_key(&h);
            let mut hs = object_state(c.conn(), c.campaign_id, &k);
            if !hs.discovered && (r.total >= h.find_dc.max(10) || r.degree == Degree::CritSuccess) {
                hs.discovered = true;
                c.eff.push(state_effect(&k, &hs));
                c.line(format!("发现了「{}」", h.name));
                found = true;
            }
        }
    }
    // 探索四周：可能发现相邻的、玩家还不知道的地点（隐藏地点）。
    if i.target.is_empty()
        && r.degree.ok()
        && let Some(here) = crate::worldsim::party_location(c.conn(), c.campaign_id)?
    {
        let m = crate::worldsim::map(c.conn(), c.campaign_id, true)?;
        let unknown: Vec<String> = m
            .roads
            .iter()
            .filter_map(|rd| if rd.a == here { Some(rd.b.clone()) } else if rd.b == here { Some(rd.a.clone()) } else { None })
            .filter(|id| m.places.iter().any(|p| &p.id == id && p.state == "unknown"))
            .take(if r.degree == Degree::CritSuccess { 2 } else { 1 })
            .collect();
        if !unknown.is_empty() {
            let pending = std::mem::take(&mut c.eff);
            if !pending.is_empty() {
                campaign::apply_effects(c.world, c.campaign_id, &pending, "调查")?;
            }
            for name in crate::worldsim::hear_of_places(c.world, c.campaign_id, &unknown)? {
                c.line(format!("发现了通往「{name}」的路"));
                found = true;
            }
        }
    }
    if !r.degree.ok() {
        c.line(format!("没有在{what}发现什么"));
    }
    finish(i, found, r.degree.label(), &format!("检查{what}：{}", if found { "有发现" } else { "没有发现" }));
    Ok(true)
}

fn input_item(op: &PlayerOp) -> String {
    op.input.as_str().map(str::to_string).or_else(|| op.input.get("item").and_then(Value::as_str).map(str::to_string)).unwrap_or_default()
}

fn act_item_use(c: &mut Ctx, i: &mut Interaction, op: &PlayerOp, step: &mut Step) -> Result<bool> {
    let item = input_item(op);
    if item.is_empty() || !has_item(c.conn(), c.campaign_id, &c.actor, &item) {
        return Err(CoreError::Invalid("这件东西不在你身上".into()));
    }
    let it = rpg::item_info(c.conn(), &item)?.ok_or_else(|| CoreError::Invalid("没有这件物品".into()))?;
    let on = op.input.get("on").and_then(Value::as_str).unwrap_or(i.target.as_str()).to_string();
    // 用在另一件物品上 = 组合。
    if let Some(other) = op.input.get("with").and_then(Value::as_str) {
        let op2 = PlayerOp { input: json!([item, other]), ..op.clone() };
        return act_combine(c, i, &op2, step);
    }
    // 用在自己身上：消耗品。
    if on.is_empty() || on == "self" || on == c.actor {
        if it.heal.is_empty() && it.cures.is_empty() {
            c.line(format!("「{}」不能这样用", it.title));
            finish(i, false, "没有效果", &format!("「{}」没有明显的效果", it.title));
            return Ok(true);
        }
        let text = rpg::use_item(c.world, c.campaign_id, &c.actor.clone(), &item)?;
        c.line(text.clone());
        step.ok = true;
        finish(i, true, "成功", &text);
        return Ok(true);
    }
    // 用在场景对象上：钥匙开锁、按 uses_on 判定。
    if let Some((o, mut s)) = find_object(c.conn(), c.campaign_id, &on)? {
        let key = obj_key(&o);
        let is_key = o.lock.as_ref().is_some_and(|l| !l.key_item.is_empty() && (l.key_item == it.id || l.key_item == it.title)) || it.unlocks.iter().any(|u| *u == o.id || *u == o.name || *u == key);
        if is_key && !s.opened {
            c.line(format!("用「{}」打开了「{}」", it.title, o.name));
            open_object(c, &key, "key")?;
            step.ok = true;
            finish(i, true, "成功", &format!("用「{}」打开了「{}」", it.title, o.name));
            return Ok(true);
        }
        let fits = it.uses_on.iter().any(|u| *u == o.id || *u == o.name || *u == o.kind || *u == key);
        if fits {
            s.used_items.push(it.id.clone());
            c.eff.push(state_effect(&key, &s));
            c.line(format!("把「{}」用在了「{}」上", it.title, o.name));
            step.ok = true;
            finish(i, true, "成功", &format!("把「{}」用在了「{}」上", it.title, o.name));
        } else {
            c.line(format!("「{}」用在「{}」上没有明显的效果", it.title, o.name));
            finish(i, false, "没有效果", &format!("「{}」对「{}」没有明显的效果", it.title, o.name));
        }
        return Ok(true);
    }
    // 用在人物身上：交给对方。
    if let Some(n) = crate::nodes::get_node(c.conn(), &on)?
        && n.kind == crate::model::NodeKind::Character
    {
        c.eff.push(Effect::TakeItem { holder: c.actor.clone(), item: item.clone(), qty: 1 });
        c.eff.push(Effect::GiveItem { holder: n.id.clone(), item: item.clone(), qty: 1 });
        c.line(format!("把「{}」交给了{}", it.title, n.title));
        step.ok = true;
        finish(i, true, "成功", &format!("把「{}」交给了{}", it.title, n.title));
        return Ok(true);
    }
    Err(CoreError::Invalid("没有找到要用在什么上".into()))
}

fn act_combine(c: &mut Ctx, i: &mut Interaction, op: &PlayerOp, step: &mut Step) -> Result<bool> {
    let picked: Vec<String> = op.input.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    if picked.len() < 2 {
        return Err(CoreError::Invalid("至少选两件东西".into()));
    }
    for p in &picked {
        if !has_item(c.conn(), c.campaign_id, &c.actor, p) {
            return Err(CoreError::Invalid(format!("「{}」不在你身上", item_name(c.conn(), p))));
        }
    }
    // 找材料正好是这几件东西的配方。
    let mut want: Vec<String> = picked.clone();
    want.sort();
    let found = rpg::recipes(c.conn(), c.campaign_id, &c.actor)?.into_iter().find(|(it, _)| {
        let mut ins: Vec<String> = it.recipe.as_ref().map(|r| r.inputs.iter().map(|x| x.item.clone()).collect()).unwrap_or_default();
        ins.sort();
        ins == want
    });
    let Some((target, ok)) = found else {
        let names: Vec<String> = picked.iter().map(|p| item_name(c.conn(), p)).collect();
        c.line(format!("{}组合不出什么", names.join("和")));
        finish(i, false, "没有配方", &format!("{}组合不出什么", names.join("和")));
        return Ok(true);
    };
    if !ok {
        c.line(format!("做「{}」的材料数量不够", target.title));
        finish(i, false, "材料不够", &format!("做「{}」的材料数量不够", target.title));
        return Ok(true);
    }
    // 制作自己会提交效果：先把这一步之前积累的效果提交。
    let pending = std::mem::take(&mut c.eff);
    if !pending.is_empty() {
        campaign::apply_effects(c.world, c.campaign_id, &pending, "互动")?;
    }
    let r = rpg::craft(c.world, c.campaign_id, &c.actor.clone(), &target.id)?;
    c.line(r.text.clone());
    step.ok = r.success;
    finish(i, r.success, if r.success { "成功" } else { "失败" }, &r.text);
    Ok(true)
}

fn act_puzzle(c: &mut Ctx, i: &mut Interaction, a: &ActionDef, step: &mut Step) -> Result<bool> {
    let p = puzzle_now(&i.data).ok_or_else(|| CoreError::Invalid("机关数据坏了".into()))?;
    let mut state: Vec<i64> = serde_json::from_value(i.data["state"].clone()).unwrap_or_else(|_| p.start.clone());
    let mut steps = i.data["steps"].as_i64().unwrap_or(0);
    if a.id == "reset" {
        state = p.start.clone();
        steps = 0;
        c.line("机关复位了");
    } else if let Some(opid) = a.id.strip_prefix("op:") {
        let o = p.ops.iter().find(|x| x.id == opid).ok_or_else(|| CoreError::Invalid("没有这个操作".into()))?;
        state = apply_op(&state, o, p.modulo);
        steps += 1;
        c.line(format!("{}（第 {steps} 步）", o.label));
    }
    i.data["state"] = json!(state);
    i.data["steps"] = json!(steps);
    let goal: Vec<i64> = p.goal.iter().map(|x| x.rem_euclid(p.modulo)).collect();
    if state == goal {
        let stage = i.data["stage"].as_u64().unwrap_or(0) as usize;
        let full: Puzzle = serde_json::from_value(i.data["puzzle"].clone()).unwrap_or_default();
        if stage < full.stages.len() {
            let next = &full.stages[stage];
            i.data["stage"] = json!(stage + 1);
            i.data["state"] = json!(next.start);
            i.data["steps"] = json!(0);
            c.line(format!("第 {} 阶段解开了，机关继续转动", stage + 1));
            step.ok = true;
            return Ok(false);
        }
        let key = i.target.clone();
        if let Some((o, mut s)) = find_object(c.conn(), c.campaign_id, &key)? {
            s.solved = true;
            c.eff.push(state_effect(&key, &s));
            c.line(format!("「{}」解开了", o.name));
            if o.lock.is_some() || !o.contents.is_empty() {
                open_object(c, &key, "puzzle")?;
            }
            reveal_clues(c, &o, true);
        }
        step.ok = true;
        finish(i, true, "成功", &format!("解开了「{}」", i.target_name));
        return Ok(true);
    }
    if p.max_steps > 0 && steps >= p.max_steps {
        let loc = find_object(c.conn(), c.campaign_id, &i.target)?.map(|(o, _)| o.location).unwrap_or_default();
        raise_alert(c, &loc, 2);
        c.line("步数用完，机关卡住了");
        finish(i, false, "失败", &format!("「{}」的机关卡住了", i.target_name));
        return Ok(true);
    }
    Ok(false)
}

fn act_social(c: &mut Ctx, i: &mut Interaction, a: &ActionDef, op: &PlayerOp, step: &mut Step) -> Result<bool> {
    let npc = i.target.clone();
    let view = CampaignView { conn: c.conn(), campaign_id: c.campaign_id };
    let bond = view.bond(&npc, "party").map(|(_, b)| b).unwrap_or_default();
    let mood = view.mood(&npc).unwrap_or_default();
    let att = view.var(&format!("attitude.{npc}")).and_then(|v| v.as_i64()).unwrap_or(0);
    if a.id == "ask" {
        c.line(format!("向{}发问", i.target_name));
        step.ok = true;
        finish(i, true, "询问", &format!("向{}发问（由对方自己回答）", i.target_name));
        return Ok(true);
    }
    // 对方的看法、情绪影响检定。
    let mut extra = att.clamp(-3, 3) + (bond.trust - bond.hostility) / 25;
    match a.id.as_str() {
        "threaten" => extra += mood.fear / 25 - mood.anger / 30,
        "deceive" => extra -= mood.suspicion / 20,
        "leverage" => extra += (bond.trust + bond.affection) / 20,
        _ => {}
    }
    let mut amount = 0;
    if a.id == "bribe" {
        amount = op.input.as_i64().or_else(|| op.input.get("amount").and_then(Value::as_i64)).unwrap_or(0);
        if amount <= 0 {
            return Err(CoreError::Invalid("贿赂要给出数额".into()));
        }
        let have = rpg::money(c.conn(), c.campaign_id, &c.actor);
        if have < amount {
            return Err(CoreError::Invalid("钱不够".into()));
        }
        extra += ((amount as f64).log10().floor() as i64).clamp(0, 4);
    }
    let r = c.check(&a.skill, a.dc, 0, extra, &format!("{}{}", a.label, i.target_name), op.dice.clone())?;
    step.ok = r.degree.ok();
    let mut mood_d = (0, 0, 0);
    let mut att_d = 0;
    match (a.id.as_str(), r.degree.ok()) {
        ("persuade" | "negotiate" | "leverage", true) => att_d = 1,
        ("threaten", true) => {
            mood_d = (6, 2, 0);
            att_d = -1
        }
        ("threaten", false) => {
            mood_d = (0, 6, 2);
            att_d = -1
        }
        ("deceive", false) => {
            mood_d = (0, 3, 8);
            att_d = -2
        }
        ("bribe", true) => {
            c.eff.push(Effect::SetVar { key: format!("money.{}", c.actor), value: json!(rpg::money(c.conn(), c.campaign_id, &c.actor) - amount) });
            c.line(format!("付出 {}", rpg::format_money(&rpg::rules(c.conn(), c.campaign_id), amount)));
            att_d = 1
        }
        ("bribe", false) if r.degree == Degree::CritFailure => att_d = -2,
        ("interrogate", false) => mood_d = (0, 2, 3),
        _ => {}
    }
    if att_d != 0 {
        c.eff.push(Effect::SetVar { key: format!("attitude.{npc}"), value: json!((att + att_d).clamp(-10, 10)) });
    }
    if mood_d != (0, 0, 0) {
        c.eff.push(Effect::AdjustMood { npc: npc.clone(), fear: mood_d.0, anger: mood_d.1, suspicion: mood_d.2, stress: 0, loyalty: 0, reason: format!("{}：{}", a.label, r.degree.label()) });
    }
    let summary = format!("{}{}：{}（对方怎么回应由对方决定）", a.label, i.target_name, r.degree.label());
    finish(i, r.degree.ok(), r.degree.label(), &summary);
    Ok(true)
}

fn act_stealth(c: &mut Ctx, i: &mut Interaction, a: &ActionDef, op: &PlayerOp, step: &mut Step) -> Result<bool> {
    let loc = i.data["location"].as_str().unwrap_or_default().to_string();
    let bonus = i.data["bonus"].as_i64().unwrap_or(0);
    let mut extra = 0;
    if a.id == "distract" {
        let it = input_item(op);
        if !it.is_empty() && has_item(c.conn(), c.campaign_id, &c.actor, &it) {
            c.eff.push(Effect::TakeItem { holder: c.actor.clone(), item: it.clone(), qty: 1 });
            c.line(format!("扔出了「{}」", item_name(c.conn(), &it)));
            extra += 2;
        }
    }
    if a.id == "sneak" || a.id == "hide" {
        extra += bonus;
    }
    let r = c.check(&a.skill, a.dc, 0, extra, &a.label, op.dice.clone())?;
    step.ok = r.degree.ok();
    let by = match (a.id.as_str(), r.degree) {
        (_, Degree::CritSuccess) => -2,
        ("distract" | "disable", d) if d.ok() => -2,
        ("hide", d) if d.ok() => -1,
        (_, d) if d.ok() => 0,
        (_, Degree::CritFailure) => 4,
        _ => 2,
    };
    let al = raise_alert(c, &loc, by);
    let level = if by == 0 { alert(c.conn(), c.campaign_id, &loc).level } else { al.level };
    i.data["level"] = json!(level);
    if r.degree.ok() && matches!(a.id.as_str(), "observe" | "route") {
        i.data["bonus"] = json!((bonus + 2).min(4));
        let guards = crate::nodes::get_node(c.conn(), &loc)?.and_then(|n| n.payload.get("guards").and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| "看清了巡逻的规律和视线的死角".into());
        c.line(format!("{guards}（之后潜行 +2）"));
    }
    if level >= 3 {
        finish(i, false, "被发现", &format!("在{}被发现了", i.target_name));
        return Ok(true);
    }
    if a.id == "sneak" && r.degree.ok() {
        finish(i, true, r.degree.label(), &format!("悄悄通过了{}", i.target_name));
        return Ok(true);
    }
    Ok(false)
}

fn act_medical(c: &mut Ctx, i: &mut Interaction, a: &ActionDef, op: &PlayerOp, step: &mut Step) -> Result<bool> {
    let patient = i.target.clone();
    let tool = find_tool(c.conn(), c.campaign_id, &c.actor, &a.needs);
    let extra = if !a.needs.is_empty() && tool.is_none() { -4 } else { 0 };
    let r = c.check("医疗", a.dc, 0, extra, &format!("{}（{}）", a.label, i.target_name), op.dice.clone())?;
    step.ok = r.degree.ok();
    if !rpg::has_sheet(c.conn(), c.campaign_id, &patient) && patient != c.actor {
        finish(i, r.degree.ok(), r.degree.label(), &format!("给{}{}：{}", i.target_name, a.label, r.degree.label()));
        return Ok(true);
    }
    let mut s = rpg::sheet(c.conn(), c.campaign_id, &patient)?;
    let mut text = vec![];
    if r.degree.ok() {
        match a.id.as_str() {
            "first_aid" => {
                let heal = 2 + (r.total - a.dc).clamp(0, 6);
                let before = s.hp;
                s.hp = (s.hp + heal).min(s.max_hp);
                text.push(format!("恢复 {} 点生命（{}/{}）", s.hp - before, s.hp, s.max_hp));
            }
            "bandage" => {
                let n = s.wounds.len();
                s.wounds.retain(|w| !w.name.contains("出血") && !w.name.contains("流血"));
                s.statuses.retain(|w| !w.name.contains("出血") && !w.name.contains("流血"));
                text.push(if s.wounds.len() < n { "止住了出血".into() } else { "包扎好了伤口".into() });
                s.hp = (s.hp + 1).min(s.max_hp);
            }
            "extract" => {
                let n = s.wounds.len();
                s.wounds.retain(|w| !w.name.contains("异物") && !w.name.contains("箭") && !w.name.contains("弹片"));
                text.push(if s.wounds.len() < n { "取出了异物".into() } else { "没有找到需要取出的东西".into() });
            }
            "antidote" => {
                let n = s.statuses.len();
                s.statuses.retain(|w| !w.name.contains("中毒") && !w.name.contains("毒"));
                text.push(if s.statuses.len() < n { "毒解了".into() } else { "没有中毒".into() });
            }
            "stabilize" => {
                if s.hp <= 0 {
                    s.hp = 1;
                    text.push("稳住了伤势，人醒过来了".into());
                } else {
                    text.push("伤势稳定".into());
                }
            }
            _ => {}
        }
        if let Some(t) = &tool
            && matches!(a.id.as_str(), "bandage" | "antidote")
        {
            c.eff.push(Effect::TakeItem { holder: c.actor.clone(), item: t.id.clone(), qty: 1 });
            text.push(format!("用掉了「{}」", t.title));
        }
        c.eff.push(rpg::sheet_effect(&s));
    } else if r.degree == Degree::CritFailure {
        s.hp = (s.hp - 1).max(0);
        c.eff.push(rpg::sheet_effect(&s));
        text.push("弄疼了伤口（−1 生命）".into());
    } else {
        text.push("没有处理好".into());
    }
    for t in &text {
        c.line(t.clone());
    }
    finish(i, r.degree.ok(), r.degree.label(), &format!("给{}{}：{}", i.target_name, a.label, text.join("，")));
    Ok(true)
}

pub const FOOD_VAR: &str = "survival.food";
pub const WATER_VAR: &str = "survival.water";

fn survival_get(conn: &Connection, campaign_id: &str, k: &str) -> i64 {
    CampaignView { conn, campaign_id }.var(k).and_then(|v| v.as_i64()).unwrap_or(80)
}

fn act_survival(c: &mut Ctx, i: &mut Interaction, a: &ActionDef, op: &PlayerOp, step: &mut Step) -> Result<bool> {
    let (food, water) = (survival_get(c.conn(), c.campaign_id, FOOD_VAR), survival_get(c.conn(), c.campaign_id, WATER_VAR));
    let set = |k: &str, v: i64| Effect::SetVar { key: k.into(), value: json!(v.clamp(0, 100)) };
    match a.id.as_str() {
        "forage" | "water" => {
            let r = c.check("生存", a.dc, 0, 0, &a.label, op.dice.clone())?;
            step.ok = r.degree.ok();
            crate::schedule::advance(c.world, c.campaign_id, 60, &a.label)?;
            let gain = match r.degree {
                Degree::CritSuccess => 50,
                Degree::Success => 30,
                Degree::Partial => 15,
                _ => 0,
            };
            let (k, cur, name) = if a.id == "forage" { (FOOD_VAR, food, "食物") } else { (WATER_VAR, water, "水") };
            c.eff.push(set(k, cur + gain));
            c.line(if gain > 0 { format!("找到了一些{name}（{name} {} → {}）", cur, (cur + gain).min(100)) } else { format!("没有找到{name}，花了一个小时") });
            finish(i, gain > 0, r.degree.label(), &format!("{}：{}", a.label, r.degree.label()));
        }
        "eat" | "drink" => {
            let it = input_item(op);
            let info = rpg::item_info(c.conn(), &it)?.filter(|x| x.tags.iter().any(|t| t == &a.needs)).ok_or_else(|| CoreError::Invalid(format!("要选一件{}", if a.id == "eat" { "吃的" } else { "喝的" })))?;
            if !has_item(c.conn(), c.campaign_id, &c.actor, &it) {
                return Err(CoreError::Invalid("这件东西不在你身上".into()));
            }
            c.eff.push(Effect::TakeItem { holder: c.actor.clone(), item: it.clone(), qty: 1 });
            let (k, cur) = if a.id == "eat" { (FOOD_VAR, food) } else { (WATER_VAR, water) };
            c.eff.push(set(k, cur + 40));
            c.line(format!("{}了「{}」", if a.id == "eat" { "吃" } else { "喝" }, info.title));
            step.ok = true;
            finish(i, true, "成功", &format!("{}了「{}」", if a.id == "eat" { "吃" } else { "喝" }, info.title));
        }
        _ => return Err(CoreError::Invalid("不能这样做".into())),
    }
    Ok(true)
}

/// 生存规则：故事时间推进时消耗食物和水（每小时各 −2 / −3），太低时加上状态。
pub fn survival_tick(world: &mut World, campaign_id: &str, minutes: i64) -> Result<Vec<String>> {
    if minutes <= 0 || !rpg::rules(world.conn(), campaign_id).survival {
        return Ok(vec![]);
    }
    let conn = world.conn();
    let hours = minutes as f64 / 60.0;
    let food = survival_get(conn, campaign_id, FOOD_VAR) - (hours * 2.0).round() as i64;
    let water = survival_get(conn, campaign_id, WATER_VAR) - (hours * 3.0).round() as i64;
    let mut notes = vec![];
    if food <= 20 {
        notes.push("很饿了：需要吃东西".to_string());
    }
    if water <= 20 {
        notes.push("很渴了：需要喝水".to_string());
    }
    campaign::apply_effects(world, campaign_id, &[Effect::SetVar { key: FOOD_VAR.into(), value: json!(food.clamp(0, 100)) }, Effect::SetVar { key: WATER_VAR.into(), value: json!(water.clamp(0, 100)) }], "生存消耗")?;
    Ok(notes)
}

fn act_ritual(c: &mut Ctx, i: &mut Interaction, a: &ActionDef, op: &PlayerOp, step: &mut Step) -> Result<bool> {
    let (o, mut s) = find_object(c.conn(), c.campaign_id, &i.target)?.ok_or_else(|| CoreError::Invalid("对象不见了".into()))?;
    let r = o.ritual.clone().unwrap_or_default();
    let title = if r.title.is_empty() { format!("「{}」的仪式", o.name) } else { r.title.clone() };
    let success = |c: &mut Ctx, s: &mut ObjectState| -> Result<()> {
        s.solved = true;
        c.eff.push(state_effect(&obj_key(&o), s));
        let effects: Vec<Effect> = r.effects.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect();
        c.eff.extend(effects);
        c.line(if r.success_text.is_empty() { format!("{title}完成了") } else { r.success_text.clone() });
        Ok(())
    };
    match a.id.as_str() {
        "perform" => {
            let picked: Vec<String> = op.input.as_array().map(|x| x.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
            let mut need: Vec<String> = r.materials.iter().map(|m| m.item.clone()).collect();
            let mut got = picked.clone();
            need.sort();
            got.sort();
            for m in &r.materials {
                if (CampaignView { conn: c.conn(), campaign_id: c.campaign_id }).item_qty(&c.actor, &m.item) < m.qty {
                    return Err(CoreError::Invalid(format!("缺少「{}」", item_name(c.conn(), &m.item))));
                }
            }
            for m in &r.materials {
                c.eff.push(Effect::TakeItem { holder: c.actor.clone(), item: m.item.clone(), qty: m.qty });
            }
            let right = need == got;
            let ok = if right && r.dc > 0 { c.check(&ritual_skill(&r), r.dc, 0, 0, &title, op.dice.clone())?.degree.ok() } else { right };
            step.ok = ok;
            if ok {
                success(c, &mut s)?;
            } else {
                c.line(if r.fail_text.is_empty() { "材料燃尽了，什么也没有发生".to_string() } else { r.fail_text.clone() });
            }
            finish(i, ok, if ok { "成功" } else { "失败" }, &format!("{title}：{}", if ok { "成功" } else { "失败" }));
            Ok(true)
        }
        "decode" => {
            let ans: Vec<String> = op.input.as_array().map(|x| x.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
            let ok = !r.answer.is_empty() && ans == r.answer;
            step.ok = ok;
            if ok {
                success(c, &mut s)?;
                finish(i, true, "成功", &format!("破解了{title}"));
                return Ok(true);
            }
            let tries = i.data["tries"].as_i64().unwrap_or(0) + 1;
            i.data["tries"] = json!(tries);
            c.line(format!("译文不对（第 {tries} 次）"));
            if tries >= 3 {
                finish(i, false, "失败", &format!("没能破解{title}"));
                return Ok(true);
            }
            Ok(false)
        }
        "study" => {
            let rr = c.check(&ritual_skill(&r), a.dc, 0, 0, &format!("研究{title}的符号"), op.dice.clone())?;
            step.ok = rr.degree.ok();
            if rr.degree.ok() && !r.answer.is_empty() {
                let n = i.data["hints"].as_i64().unwrap_or(0) as usize;
                if let (Some(sym), Some(mean)) = (r.symbols.get(n), r.answer.get(n)) {
                    c.line(format!("看懂了一个符号：「{sym}」的意思是「{mean}」"));
                    i.data["hints"] = json!(n + 1);
                }
            } else {
                c.line("还是看不懂这些符号");
            }
            i.available_actions.retain(|x| x.id != "study" || rr.degree.ok());
            Ok(false)
        }
        "invoke" => {
            if !r.cost_var.is_empty() && r.cost > 0 {
                let key = format!("{}.{}", r.cost_var, c.actor);
                let have = CampaignView { conn: c.conn(), campaign_id: c.campaign_id }.var(&key).and_then(|v| v.as_i64()).unwrap_or(0);
                if have < r.cost {
                    return Err(CoreError::Invalid(format!("{} 不够（需要 {}，现有 {have}）", r.cost_var, r.cost)));
                }
                c.eff.push(Effect::SetVar { key, value: json!(have - r.cost) });
            }
            let rr = c.check(&ritual_skill(&r), a.dc, 0, 0, &title, op.dice.clone())?;
            step.ok = rr.degree.ok();
            if rr.degree.ok() {
                success(c, &mut s)?;
            } else {
                c.line(if r.fail_text.is_empty() { "力量没有回应".to_string() } else { r.fail_text.clone() });
            }
            finish(i, rr.degree.ok(), rr.degree.label(), &format!("{title}：{}", rr.degree.label()));
            Ok(true)
        }
        _ => {
            // 抵抗 / 梦境：分几轮，每轮一次检定；失败累积压力，成功过半即通过。
            let rounds = r.rounds.clamp(1, 7);
            let rr = c.check(&ritual_skill(&r), a.dc, 0, 0, &format!("{title}（第 {} 轮）", i.data["round"].as_i64().unwrap_or(0) + 1), op.dice.clone())?;
            step.ok = rr.degree.ok();
            let round = i.data["round"].as_i64().unwrap_or(0) + 1;
            let passes = i.data["passes"].as_i64().unwrap_or(0) + rr.degree.ok() as i64;
            let fails = i.data["fails"].as_i64().unwrap_or(0) + (!rr.degree.ok()) as i64;
            i.data["round"] = json!(round);
            i.data["passes"] = json!(passes);
            i.data["fails"] = json!(fails);
            if !rr.degree.ok() {
                let key = format!("stress.{}", c.actor);
                let cur = CampaignView { conn: c.conn(), campaign_id: c.campaign_id }.var(&key).and_then(|v| v.as_i64()).unwrap_or(0);
                c.eff.push(Effect::SetVar { key, value: json!(cur + 1) });
                c.line("精神受到冲击（压力 +1）");
            }
            let need = rounds / 2 + 1;
            if passes >= need {
                success(c, &mut s)?;
                finish(i, true, "成功", &format!("{title}：撑过来了"));
                return Ok(true);
            }
            if fails > rounds - need || round >= rounds {
                c.line(if r.fail_text.is_empty() { "没能撑住".to_string() } else { r.fail_text.clone() });
                finish(i, false, "失败", &format!("{title}：没能撑住"));
                return Ok(true);
            }
            Ok(false)
        }
    }
}

/// 放弃一个互动（玩家离开、开始了别的事）。
pub fn abandon(world: &mut World, campaign_id: &str, id: &str, reason: &str) -> Result<()> {
    let Some(mut i) = get(world.conn(), campaign_id, id) else { return Ok(()) };
    if !i.open() {
        return Ok(());
    }
    i.status = "abandoned".into();
    i.result = Some(Outcome { success: false, degree: "放弃".into(), summary: reason.chars().take(80).collect() });
    i.consumed = true;
    campaign::apply_effects(world, campaign_id, &[save_effect(&i)], &format!("放弃互动「{}」：{reason}", i.title))?;
    Ok(())
}

/// 回合结束时：玩家离开了地点的互动放弃；故事时间限时到了的超时。
pub fn tick(world: &mut World, campaign_id: &str) -> Result<Vec<String>> {
    let loc = crate::worldsim::party_location(world.conn(), campaign_id)?.unwrap_or_default();
    let now = crate::schedule::clock(world.conn(), campaign_id);
    let mut notes = vec![];
    for i in pending(world.conn(), campaign_id)? {
        let here = i.target.is_empty() || i.target.starts_with(&format!("{loc}:")) || i.target == loc || !i.target.contains(':') && i.kind != "stealth";
        if !here {
            abandon(world, campaign_id, &i.interaction_id, "离开了这里")?;
            continue;
        }
        if i.timeout_policy.kind == "game" && now - i.timeout_policy.started > i.timeout_policy.amount {
            let mut x = i.clone();
            x.status = "expired".into();
            x.result = Some(Outcome { success: false, degree: "超时".into(), summary: format!("时间到了：{}", x.timeout_policy.reason) });
            campaign::apply_effects(world, campaign_id, &[save_effect(&x)], "互动超时")?;
            notes.push(format!("「{}」时间到了", x.title));
        }
    }
    Ok(notes)
}

/// 已经结束、还没交给 GM 叙述的互动结果（取出后标记已交付）。
pub fn take_results(world: &mut World, campaign_id: &str) -> Result<Vec<String>> {
    let mut out = vec![];
    let mut eff = vec![];
    for mut i in list(world.conn(), campaign_id)?.into_iter().filter(|i| !i.open() && !i.consumed && i.actor == rpg::actor()) {
        let steps: Vec<String> = i.steps.iter().map(|s| format!("{}：{}", s.label, s.text)).collect();
        let r = i.result.clone().unwrap_or_default();
        out.push(format!("互动「{}」{}：{}。过程：{}", i.title, r.degree, r.summary, steps.join(" → ")));
        i.consumed = true;
        eff.push(save_effect(&i));
    }
    if !eff.is_empty() {
        campaign::apply_effects(world, campaign_id, &eff, "互动结果交给叙述")?;
    }
    Ok(out)
}

/// 给 GM 的文字：玩家所在地点的场景对象（O 编号）与待处理的互动。返回（文字，O 编号 → 对象键）。
pub fn gm_text(conn: &Connection, campaign_id: &str) -> Result<(String, BTreeMap<String, String>)> {
    let mut lines = vec![];
    let mut refs = BTreeMap::new();
    for (n, v) in objects_here(conn, campaign_id, true)?.into_iter().enumerate() {
        let code = format!("O{}", n + 1);
        let o = &v.object;
        let mut tags = vec![o.kind.clone()];
        if o.hidden && !v.state.discovered {
            tags.push(format!("隐藏，调查达到 {} 才会发现", o.find_dc.max(10)));
        }
        if let Some(l) = &o.lock {
            tags.push(if v.state.opened || v.state.broken { "已打开".into() } else { format!("锁着（{}锁）", match l.kind.as_str() { "code" => "密码", "electronic" => "电子", "bar" => "门闩", _ => "钥匙" }) });
        }
        if o.mechanism.is_some() {
            tags.push(if v.state.solved { "机关已解开".into() } else { "机关".into() });
        }
        if o.ritual.is_some() {
            tags.push("可以进行仪式".into());
        }
        if v.state.searched > 0 {
            tags.push("搜过".into());
        }
        lines.push(format!("- {code} {}（{}）：{}{}", o.name, tags.join("，"), o.description, if o.notes.is_empty() { String::new() } else { format!("［主持人：{}］", o.notes) }));
        refs.insert(code, v.key.clone());
    }
    for i in pending(conn, campaign_id)? {
        lines.push(format!("- 进行中的互动：{}（玩家可以继续操作，也可以做别的事）", i.title));
    }
    let loc = crate::worldsim::party_location(conn, campaign_id)?.unwrap_or_default();
    let a = alert(conn, campaign_id, &loc);
    if a.level > 0 {
        lines.push(format!("- 这里的警戒：{}", ALERT_LABELS[a.level.clamp(0, 3) as usize]));
    }
    if rpg::rules(conn, campaign_id).survival {
        lines.push(format!("- 生存：食物 {}/100，水 {}/100", survival_get(conn, campaign_id, FOOD_VAR), survival_get(conn, campaign_id, WATER_VAR)));
    }
    Ok((lines.join("\n"), refs))
}

/// 玩家用自己的办法（不是互动卡片）改变了场景对象，而且这一回合的检定成功了：程序改对象状态。
/// change：open（打开，里面的东西归玩家）/ break（砸坏并打开）/ search（记为搜过）。同一个对象上待处理的互动随之放弃。
pub fn change_object(world: &mut World, campaign_id: &str, target: &str, change: &str, why: &str) -> Result<Vec<String>> {
    let (o, s) = find_object(world.conn(), campaign_id, target)?.ok_or_else(|| CoreError::Invalid(format!("这里没有「{target}」")))?;
    let key = obj_key(&o);
    let actor = rpg::actor();
    let mut c = Ctx { world, campaign_id, actor, eff: vec![], lines: vec![], roll: None };
    match change {
        "open" | "break" => {
            if s.opened && change == "open" {
                return Ok(vec![format!("「{}」本来就开着", o.name)]);
            }
            open_object(&mut c, &key, change)?;
            if change == "break" {
                raise_alert(&mut c, &o.location, 2);
            }
        }
        "search" => {
            let mut s = s;
            s.searched = s.searched.max(1);
            c.eff.push(state_effect(&key, &s));
            c.line(format!("「{}」搜过了", o.name));
        }
        other => return Err(CoreError::Invalid(format!("不认识的变化：{other}"))),
    }
    for mut i in pending(c.conn(), campaign_id)?.into_iter().filter(|i| i.target == key) {
        i.status = "abandoned".into();
        i.result = Some(Outcome { success: false, degree: "放弃".into(), summary: "玩家用别的办法解决了".into() });
        i.consumed = true;
        c.eff.push(save_effect(&i));
    }
    let eff = std::mem::take(&mut c.eff);
    let lines = std::mem::take(&mut c.lines);
    campaign::apply_effects(c.world, campaign_id, &eff, &format!("场景对象「{}」：{why}", o.name))?;
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levers() -> Puzzle {
        Puzzle {
            kind: "levers".into(),
            labels: vec!["左".into(), "中".into(), "右".into()],
            modulo: 2,
            start: vec![0, 0, 0],
            goal: vec![1, 1, 1],
            ops: vec![
                PuzzleOp { id: "a".into(), label: "拉左边".into(), add: vec![1, 1, 0], needs: String::new() },
                PuzzleOp { id: "b".into(), label: "拉右边".into(), add: vec![0, 1, 1], needs: String::new() },
                PuzzleOp { id: "c".into(), label: "拉中间".into(), add: vec![0, 1, 0], needs: String::new() },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn puzzle_solvability() {
        assert_eq!(validate_puzzle(&levers()).unwrap(), 3);
        let mut p = levers();
        p.max_steps = 2;
        assert!(validate_puzzle(&p).is_err(), "步数不够");
        let mut q = levers();
        q.ops = vec![PuzzleOp { id: "x".into(), label: "x".into(), add: vec![1, 1, 1], needs: String::new() }];
        q.goal = vec![1, 0, 1];
        assert!(validate_puzzle(&q).is_err(), "无解");
    }

    #[test]
    fn alert_levels() {
        assert_eq!(level_of(0), 0);
        assert_eq!(level_of(2), 1);
        assert_eq!(level_of(5), 2);
        assert_eq!(level_of(6), 3);
        assert_eq!(perf_mod(Some(1.0)), 3);
        assert_eq!(perf_mod(Some(0.0)), -3);
        assert_eq!(perf_mod(None), 0);
    }
}

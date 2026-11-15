//! 战役状态：与世界正典分离的可变状态，以及剧情推进的程序判定。
//!
//! 玩家选择 → 程序求值条件（含掷骰）→ 程序执行效果 → 写入状态与只追加的事件日志。
//! 世界正典（nodes/relations）在这里永远不会被修改；战役内的变化写入覆盖表。

use std::collections::BTreeMap;

use chronicle_rules::{
    CheckRecord, DiceExpr, RollResult, SeededDice, StateChange, StateView, Tri, resolve_effects,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::entity::{Entity, js};
use crate::error::{CoreError, Result};
use crate::model::{PlotEdge, PlotNodeKind};
use crate::plot;
use crate::util::{new_id, now_ms};
use crate::world::World;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Campaign {
    pub id: String,
    pub script_id: Option<String>,
    pub name: String,
    pub base_commit: Option<String>,
    pub rng_seed: i64,
    pub roll_counter: i64,
    pub turn: i64,
    pub current_plot_node: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// 回溯产生的时间线：父时间线与分叉位置。
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub fork_seq: Option<i64>,
    #[serde(default)]
    pub fork_turn: Option<i64>,
    /// active / ended
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub ended_at: Option<i64>,
    #[serde(default)]
    pub ending: String,
}

fn default_status() -> String {
    "active".into()
}

/// 已结束的战役不能再推进，只能查看或从历史节点开出新时间线。
pub fn ensure_active(conn: &Connection, campaign_id: &str) -> Result<()> {
    if get_campaign(conn, campaign_id)?.status != "active" {
        return Err(CoreError::Invalid("战役已结束，只能查看，或从历史节点开出新的时间线继续".into()));
    }
    Ok(())
}

fn campaign_row(r: &rusqlite::Row) -> rusqlite::Result<Campaign> {
    Ok(Campaign {
        id: r.get("id")?,
        script_id: r.get("script_id")?,
        name: r.get("name")?,
        base_commit: r.get("base_commit")?,
        rng_seed: r.get("rng_seed")?,
        roll_counter: r.get("roll_counter")?,
        turn: r.get("turn")?,
        current_plot_node: r.get("current_plot_node")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
        parent_id: r.get("parent_id")?,
        fork_seq: r.get("fork_seq")?,
        fork_turn: r.get("fork_turn")?,
        status: r.get("status")?,
        archived: r.get::<_, i64>("archived")? != 0,
        ended_at: r.get("ended_at")?,
        ending: r.get("ending")?,
    })
}

pub fn get_campaign(conn: &Connection, id: &str) -> Result<Campaign> {
    conn.query_row("SELECT * FROM campaigns WHERE id = ?1", [id], campaign_row)
        .optional()?
        .ok_or_else(|| CoreError::NotFound("战役", id.into()))
}

pub fn list_campaigns(conn: &Connection) -> Result<Vec<Campaign>> {
    let mut stmt = conn.prepare("SELECT * FROM campaigns WHERE deleted_at IS NULL ORDER BY created_at")?;
    let rows = stmt.query_map([], campaign_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 新建战役。若剧本恰有一个开场节点，则从该节点开始。
pub fn create_campaign(world: &mut World, script_id: Option<&str>, name: &str, seed: Option<i64>) -> Result<Campaign> {
    if name.trim().is_empty() {
        return Err(CoreError::Invalid("战役名称不能为空".into()));
    }
    let start = match script_id {
        Some(s) => {
            crate::scripts::require_script(&world.conn, s)?;
            let starts: Vec<_> =
                plot::plot_nodes_of(&world.conn, s)?.into_iter().filter(|n| n.kind == PlotNodeKind::Start).collect();
            if starts.len() == 1 { Some(starts[0].id.clone()) } else { None }
        }
        None => None,
    };
    let seed = seed.unwrap_or_else(|| {
        let u = ulid::Ulid::generate();
        (u.random() as i64) ^ now_ms()
    });
    let now = now_ms();
    let c = Campaign {
        id: new_id(),
        script_id: script_id.map(str::to_string),
        name: name.trim().into(),
        base_commit: world.head_commit()?,
        rng_seed: seed,
        roll_counter: 0,
        turn: 0,
        current_plot_node: start,
        created_at: now,
        updated_at: now,
        parent_id: None,
        fork_seq: None,
        fork_turn: None,
        status: default_status(),
        archived: false,
        ended_at: None,
        ending: String::new(),
    };
    world.conn.execute(
        "INSERT INTO campaigns (id, script_id, name, base_commit, rng_seed, roll_counter, turn, current_plot_node,
            created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            c.id, c.script_id, c.name, c.base_commit, c.rng_seed, c.roll_counter, c.turn, c.current_plot_node,
            c.created_at, c.updated_at
        ],
    )?;
    if let Some(n) = &c.current_plot_node {
        world.conn.execute(
            "INSERT INTO plot_visits (campaign_id, plot_node_id, count, first_turn) VALUES (?1, ?2, 1, 0)",
            params![c.id, n],
        )?;
    }
    // 剧本的玩法配置：属性方案、技能对应、检定规则复制进战役规则（之后可以单独修改）。
    if let Some(sid) = script_id {
        let script = crate::scripts::require_script(&world.conn, sid)?;
        let gp = crate::profile::for_script(&script);
        if !gp.attributes.is_empty() || !gp.skills.is_empty() || gp.check != crate::profile::CheckRules::default() || !gp.currency.is_empty() || !gp.rarity_labels.is_empty() {
            let mut r = crate::rpg::RuleSet { attributes: gp.attributes.clone(), skill_attrs: gp.skills.clone(), check: gp.check.clone(), rarity_labels: gp.rarity_labels.clone(), ..Default::default() };
            if !gp.currency.is_empty() {
                r.currency = gp.currency.clone();
                r.currency_rates = gp.currency_rates.clone();
            }
            if r.validate().is_ok() {
                apply_effects(world, &c.id, &[chronicle_rules::Effect::SetVar { key: crate::rpg::RULES_VAR.into(), value: serde_json::to_value(&r)? }], "按剧本的玩法配置设置规则")?;
            }
        }
    }
    // 起点检查点：回溯可以回到战役开始。
    crate::timeline::checkpoint(&world.conn, &c.id, "start")?;
    Ok(c)
}

/// 战役状态视图：先读战役覆盖，再回落到世界正典。
pub struct CampaignView<'a> {
    pub conn: &'a Connection,
    pub campaign_id: &'a str,
}

impl StateView for CampaignView<'_> {
    /// 检定修正由角色卡计算（技能、属性、装备、状态、伤势），兼容旧的 `skill.<actor>.<skill>` 变量。
    fn skill_mod(&self, actor: &str, skill: &str) -> i64 {
        crate::rpg::check_modifier(self.conn, self.campaign_id, actor, skill)
    }
    fn mood(&self, npc: &str) -> Option<chronicle_rules::effect::Mood> {
        self.conn
            .query_row(
                "SELECT fear, anger, suspicion, stress, loyalty FROM campaign_moods WHERE campaign_id = ?1 AND npc_id = ?2",
                [self.campaign_id, npc],
                |r| Ok(chronicle_rules::effect::Mood { fear: r.get(0)?, anger: r.get(1)?, suspicion: r.get(2)?, stress: r.get(3)?, loyalty: r.get(4)? }),
            )
            .optional()
            .ok()
            .flatten()
    }
    fn record(&self, kind: &str, id: &str) -> Option<Value> {
        self.conn
            .query_row(
                "SELECT data FROM campaign_records WHERE campaign_id = ?1 AND kind = ?2 AND id = ?3",
                [self.campaign_id, kind, id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
    }
    fn bond(&self, from: &str, to: &str) -> Option<(String, chronicle_rules::effect::Bond)> {
        self.conn
            .query_row(
                "SELECT kind, strength, trust, affection, hostility FROM campaign_bonds WHERE campaign_id = ?1 AND from_id = ?2 AND to_id = ?3",
                [self.campaign_id, from, to],
                |r| Ok((r.get(0)?, chronicle_rules::effect::Bond { strength: r.get(1)?, trust: r.get(2)?, affection: r.get(3)?, hostility: r.get(4)? })),
            )
            .optional()
            .ok()
            .flatten()
    }
    fn var(&self, key: &str) -> Option<Value> {
        self.conn
            .query_row(
                "SELECT value FROM state_vars WHERE campaign_id = ?1 AND key = ?2",
                [self.campaign_id, key],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
    }
    fn quest_state(&self, quest: &str) -> Option<String> {
        self.conn
            .query_row(
                "SELECT state FROM quest_states WHERE campaign_id = ?1 AND quest_id = ?2",
                [self.campaign_id, quest],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }
    fn item_qty(&self, holder: &str, item: &str) -> i64 {
        self.conn
            .query_row(
                "SELECT qty FROM inventory WHERE campaign_id = ?1 AND holder = ?2 AND item = ?3",
                [self.campaign_id, holder, item],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
            .unwrap_or(0)
    }
    fn clue_known(&self, clue: &str) -> bool {
        self.conn
            .query_row(
                "SELECT 1 FROM clue_reveals WHERE campaign_id = ?1 AND clue_id = ?2",
                [self.campaign_id, clue],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }
    fn relation(&self, a: &str, b: &str, rel_type: &str) -> bool {
        let ov: Option<bool> = self
            .conn
            .query_row(
                "SELECT active FROM relation_overrides WHERE campaign_id = ?1 AND a = ?2 AND b = ?3 AND rel_type = ?4",
                [self.campaign_id, a, b, rel_type],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten();
        if let Some(v) = ov {
            return v;
        }
        self.conn
            .query_row(
                "SELECT 1 FROM relations WHERE src_id = ?1 AND dst_id = ?2 AND rel_type = ?3 AND deleted_at IS NULL
                 UNION ALL
                 SELECT 1 FROM relations WHERE src_id = ?2 AND dst_id = ?1 AND rel_type = ?3 AND directed = 0 AND deleted_at IS NULL
                 LIMIT 1",
                [a, b, rel_type],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }
    fn visit_count(&self, node: &str) -> i64 {
        self.conn
            .query_row(
                "SELECT count FROM plot_visits WHERE campaign_id = ?1 AND plot_node_id = ?2",
                [self.campaign_id, node],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
            .unwrap_or(0)
    }
}

fn append_event(conn: &Connection, campaign_id: &str, turn: i64, kind: &str, payload: &Value) -> Result<i64> {
    conn.execute(
        "INSERT INTO event_log (campaign_id, turn, kind, payload, created_at) VALUES (?1,?2,?3,?4,?5)",
        params![campaign_id, turn, kind, js(payload), now_ms()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 把已解析的状态变化写入战役状态表（调用方负责事务）。
pub fn apply_changes(conn: &Connection, campaign_id: &str, turn: i64, changes: &[StateChange]) -> Result<()> {
    for ch in changes {
        match ch {
            StateChange::SetVar { key, new, .. } => {
                conn.execute(
                    "INSERT INTO state_vars (campaign_id, key, value) VALUES (?1,?2,?3)
                     ON CONFLICT(campaign_id, key) DO UPDATE SET value = excluded.value",
                    params![campaign_id, key, js(new)],
                )?;
            }
            StateChange::SetQuestState { quest, new, .. } => {
                conn.execute(
                    "INSERT INTO quest_states (campaign_id, quest_id, state) VALUES (?1,?2,?3)
                     ON CONFLICT(campaign_id, quest_id) DO UPDATE SET state = excluded.state",
                    params![campaign_id, quest, new],
                )?;
            }
            StateChange::SetItemQty { holder, item, new, .. } => {
                conn.execute(
                    "INSERT INTO inventory (campaign_id, holder, item, qty) VALUES (?1,?2,?3,?4)
                     ON CONFLICT(campaign_id, holder, item) DO UPDATE SET qty = excluded.qty",
                    params![campaign_id, holder, item, new],
                )?;
            }
            StateChange::RevealClue { clue } => {
                conn.execute(
                    "INSERT OR IGNORE INTO clue_reveals (campaign_id, clue_id, turn) VALUES (?1,?2,?3)",
                    params![campaign_id, clue, turn],
                )?;
            }
            StateChange::SetRelation { a, b, rel_type, active } => {
                conn.execute(
                    "INSERT INTO relation_overrides (campaign_id, a, b, rel_type, active) VALUES (?1,?2,?3,?4,?5)
                     ON CONFLICT(campaign_id, a, b, rel_type) DO UPDATE SET active = excluded.active",
                    params![campaign_id, a, b, rel_type, active],
                )?;
            }
            StateChange::SetEntityField { entity, field, value } => {
                conn.execute(
                    "INSERT INTO entity_overrides (campaign_id, entity_id, field, value) VALUES (?1,?2,?3,?4)
                     ON CONFLICT(campaign_id, entity_id, field) DO UPDATE SET value = excluded.value",
                    params![campaign_id, entity, field, js(value)],
                )?;
            }
            StateChange::UnlockNode { node } => {
                plot::require_plot_node(conn, node)?;
                conn.execute(
                    "INSERT OR IGNORE INTO plot_unlocks (campaign_id, plot_node_id) VALUES (?1,?2)",
                    params![campaign_id, node],
                )?;
            }
            StateChange::AdjustBond { from, to, kind, new, .. } => {
                conn.execute(
                    "INSERT INTO campaign_bonds (campaign_id, from_id, to_id, kind, strength, trust, affection, hostility) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                     ON CONFLICT(campaign_id, from_id, to_id) DO UPDATE SET kind = excluded.kind, strength = excluded.strength,
                        trust = excluded.trust, affection = excluded.affection, hostility = excluded.hostility",
                    params![campaign_id, from, to, kind, new.strength, new.trust, new.affection, new.hostility],
                )?;
            }
            StateChange::AdjustMood { npc, new, .. } => {
                conn.execute(
                    "INSERT INTO campaign_moods (campaign_id, npc_id, fear, anger, suspicion, stress, loyalty) VALUES (?1,?2,?3,?4,?5,?6,?7)
                     ON CONFLICT(campaign_id, npc_id) DO UPDATE SET fear = excluded.fear, anger = excluded.anger,
                        suspicion = excluded.suspicion, stress = excluded.stress, loyalty = excluded.loyalty",
                    params![campaign_id, npc, new.fear, new.anger, new.suspicion, new.stress, new.loyalty],
                )?;
            }
            StateChange::UpsertRecord { kind, id, new, .. } => {
                conn.execute(
                    "INSERT INTO campaign_records (campaign_id, kind, id, data) VALUES (?1,?2,?3,?4)
                     ON CONFLICT(campaign_id, kind, id) DO UPDATE SET data = excluded.data",
                    params![campaign_id, kind, id, js(new)],
                )?;
            }
            StateChange::DeleteRecord { kind, id, .. } => {
                conn.execute("DELETE FROM campaign_records WHERE campaign_id = ?1 AND kind = ?2 AND id = ?3", params![campaign_id, kind, id])?;
            }
            StateChange::Spawn { changeset } => {
                if !crate::changeset::changeset_is_applied(conn, changeset)? {
                    return Err(CoreError::Invalid(format!("变更集 {changeset} 尚未批准，不能引入战役")));
                }
            }
        }
    }
    Ok(())
}

fn next_stream(conn: &Connection, campaign_id: &str) -> Result<(i64, i64)> {
    let (seed, counter): (i64, i64) = conn.query_row(
        "SELECT rng_seed, roll_counter FROM campaigns WHERE id = ?1",
        [campaign_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    conn.execute("UPDATE campaigns SET roll_counter = roll_counter + 1 WHERE id = ?1", [campaign_id])?;
    Ok((seed, counter))
}

/// 程序掷骰：使用战役种子与递增流号，结果写入事件日志，可复现。
pub fn roll(world: &mut World, campaign_id: &str, expr: &str, reason: &str) -> Result<RollResult> {
    let e = DiceExpr::parse(expr)?;
    let tx = world.conn.transaction()?;
    ensure_active(&tx, campaign_id)?;
    let c = get_campaign(&tx, campaign_id)?;
    let (seed, stream) = next_stream(&tx, campaign_id)?;
    let r = e.roll(&mut SeededDice::new(seed as u64, stream as u64));
    append_event(
        &tx,
        campaign_id,
        c.turn,
        "roll",
        &serde_json::json!({"reason": reason, "stream": stream, "result": r}),
    )?;
    tx.commit()?;
    Ok(r)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct EdgeOption {
    pub edge: PlotEdge,
    /// 预演结果：需要检定的为 Unknown。
    pub availability: Tri,
    /// `once` 边已经走过。
    pub used_up: bool,
}

fn edge_used(conn: &Connection, campaign_id: &str, edge_id: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM edge_uses WHERE campaign_id = ?1 AND edge_id = ?2",
            [campaign_id, edge_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// 当前节点的出边及其可用性（不掷骰，不修改状态）。
pub fn available_edges(world: &World, campaign_id: &str) -> Result<Vec<EdgeOption>> {
    available_edges_conn(&world.conn, campaign_id)
}

/// 当前剧情节点（只读连接版）。
pub fn current_node_conn(conn: &Connection, campaign_id: &str) -> Result<Option<crate::model::PlotNode>> {
    match get_campaign(conn, campaign_id)?.current_plot_node {
        Some(id) => crate::model::PlotNode::load(conn, &id),
        None => Ok(None),
    }
}

/// 当前可选分支（只读连接版）。
pub fn available_edges_conn(conn: &Connection, campaign_id: &str) -> Result<Vec<EdgeOption>> {
    let world_conn = conn;
    let c = get_campaign(world_conn, campaign_id)?;
    let Some(cur) = c.current_plot_node else {
        return Ok(vec![]);
    };
    let view = CampaignView { conn: world_conn, campaign_id };
    let mut out = Vec::new();
    for edge in plot::outgoing(world_conn, &cur)? {
        let used_up = edge.once && edge_used(world_conn, campaign_id, &edge.id)?;
        let availability = match (&edge.condition, used_up) {
            (_, true) => Tri::False,
            (None, false) => Tri::True,
            (Some(cond), false) => cond.preview(&view)?,
        };
        out.push(EdgeOption { edge, availability, used_up });
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct TransitionOutcome {
    pub taken: bool,
    pub checks: Vec<CheckRecord>,
    pub changes: Vec<StateChange>,
    pub from: String,
    pub to: Option<String>,
    pub turn: i64,
}

/// 尝试沿一条剧情边推进。条件由程序求值（检定由程序掷骰），
/// 通过则执行效果、移动到目标节点；不通过只记录检定结果。整个过程一个事务。
pub fn take_edge(world: &mut World, campaign_id: &str, edge_id: &str) -> Result<TransitionOutcome> {
    let tx = world.conn.transaction()?;
    ensure_active(&tx, campaign_id)?;
    let c = get_campaign(&tx, campaign_id)?;
    let edge = plot::get_edge(&tx, edge_id)?.ok_or_else(|| CoreError::NotFound("剧情边", edge_id.into()))?;
    let cur = c.current_plot_node.clone().ok_or_else(|| CoreError::Invalid("战役尚未进入剧情".into()))?;
    if edge.from_id != cur {
        return Err(CoreError::Invalid("这条剧情边不是从当前节点出发的".into()));
    }
    if edge.once && edge_used(&tx, campaign_id, edge_id)? {
        return Err(CoreError::Invalid("这条剧情边只能走一次，已经走过".into()));
    }
    let to_node = plot::require_plot_node(&tx, &edge.to_id)?;
    let turn = c.turn + 1;

    // 用掉的骰子流号写进事件，回溯校验时据此重放骰子计数。
    let mut used_stream: Option<i64> = None;
    let (passed, checks) = match &edge.condition {
        None => (true, vec![]),
        Some(cond) => {
            let res = if cond.has_checks() {
                let (seed, stream) = next_stream(&tx, campaign_id)?;
                used_stream = Some(stream);
                let view = CampaignView { conn: &tx, campaign_id };
                cond.eval(&view, &mut SeededDice::new(seed as u64, stream as u64))?
            } else {
                let view = CampaignView { conn: &tx, campaign_id };
                cond.eval(&view, &mut chronicle_rules::dice::FixedDice::new(vec![]))?
            };
            (res.passed, res.checks)
        }
    };

    let mut changes = Vec::new();
    if passed {
        changes = {
            let view = CampaignView { conn: &tx, campaign_id };
            resolve_effects(&edge.effects, &view)?
        };
        apply_changes(&tx, campaign_id, turn, &changes)?;
        tx.execute(
            "INSERT INTO plot_visits (campaign_id, plot_node_id, count, first_turn) VALUES (?1,?2,1,?3)
             ON CONFLICT(campaign_id, plot_node_id) DO UPDATE SET count = count + 1",
            params![campaign_id, to_node.id, turn],
        )?;
        tx.execute(
            "INSERT INTO edge_uses (campaign_id, edge_id, count) VALUES (?1,?2,1)
             ON CONFLICT(campaign_id, edge_id) DO UPDATE SET count = count + 1",
            params![campaign_id, edge_id],
        )?;
        tx.execute(
            "UPDATE campaigns SET current_plot_node = ?2, turn = ?3, updated_at = ?4 WHERE id = ?1",
            params![campaign_id, to_node.id, turn, now_ms()],
        )?;
    } else {
        tx.execute(
            "UPDATE campaigns SET turn = ?2, updated_at = ?3 WHERE id = ?1",
            params![campaign_id, turn, now_ms()],
        )?;
    }
    let outcome = TransitionOutcome {
        taken: passed,
        checks,
        changes,
        from: cur,
        to: passed.then(|| to_node.id.clone()),
        turn,
    };
    append_event(
        &tx,
        campaign_id,
        turn,
        if passed { "transition" } else { "transition_failed" },
        &serde_json::json!({"edge": edge_id, "outcome": outcome, "stream": used_stream}),
    )?;
    tx.commit()?;
    Ok(outcome)
}

/// 直接执行一组效果（例如 GM 程序判定后的结果），不涉及剧情跳转。
pub fn apply_effects(world: &mut World, campaign_id: &str, effects: &[chronicle_rules::Effect], reason: &str) -> Result<Vec<StateChange>> {
    let tx = world.conn.transaction()?;
    ensure_active(&tx, campaign_id)?;
    let c = get_campaign(&tx, campaign_id)?;
    let changes = {
        let view = CampaignView { conn: &tx, campaign_id };
        resolve_effects(effects, &view)?
    };
    apply_changes(&tx, campaign_id, c.turn, &changes)?;
    append_event(&tx, campaign_id, c.turn, "effects", &serde_json::json!({"reason": reason, "changes": changes}))?;
    tx.commit()?;
    // 人物死亡（不论来自战斗、GM 还是主持人手动修改）：和他有关系的人物情绪随之变化，作为另一组效果记录。
    let dead: Vec<String> = changes
        .iter()
        .filter_map(|ch| match ch {
            StateChange::SetEntityField { entity, field, value } if field == "life_status" && value.as_str() == Some("dead") => Some(entity.clone()),
            _ => None,
        })
        .collect();
    for d in dead {
        let extra = crate::worldsim::reactions_to_death(&world.conn, campaign_id, &d)?;
        if !extra.is_empty() {
            let name = crate::nodes::get_node(&world.conn, &d)?.map(|n| n.title).unwrap_or_default();
            apply_effects(world, campaign_id, &extra, &format!("{name}死亡的影响"))?;
        }
    }
    Ok(changes)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct EventEntry {
    pub seq: i64,
    pub turn: i64,
    pub kind: String,
    pub payload: Value,
    pub created_at: i64,
}

pub fn log_event(world: &World, campaign_id: &str, kind: &str, payload: &Value) -> Result<i64> {
    let c = get_campaign(&world.conn, campaign_id)?;
    append_event(&world.conn, campaign_id, c.turn, kind, payload)
}

pub fn events(conn: &Connection, campaign_id: &str, after_seq: i64, limit: i64) -> Result<Vec<EventEntry>> {
    let mut stmt = conn.prepare(
        "SELECT seq, turn, kind, payload, created_at FROM event_log WHERE campaign_id = ?1 AND seq > ?2
         ORDER BY seq LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![campaign_id, after_seq, limit], |r| {
        Ok(EventEntry {
            seq: r.get(0)?,
            turn: r.get(1)?,
            kind: r.get(2)?,
            payload: crate::entity::jget(r, "payload")?,
            created_at: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 战役内对某实体的覆盖字段（如 NPC 已死亡）。
pub fn entity_overrides(conn: &Connection, campaign_id: &str, entity_id: &str) -> Result<BTreeMap<String, Value>> {
    let mut stmt =
        conn.prepare("SELECT field, value FROM entity_overrides WHERE campaign_id = ?1 AND entity_id = ?2")?;
    let rows = stmt.query_map([campaign_id, entity_id], |r| Ok((r.get::<_, String>(0)?, crate::entity::jget(r, "value")?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NpcMemory {
    pub id: String,
    pub npc_id: String,
    pub content: String,
    pub importance: i64,
    pub turn: i64,
    pub created_at: i64,
}

pub fn add_npc_memory(world: &World, campaign_id: &str, npc_id: &str, content: &str, importance: i64) -> Result<NpcMemory> {
    let c = get_campaign(&world.conn, campaign_id)?;
    let node = crate::nodes::require_node(&world.conn, npc_id)?;
    if node.kind != crate::model::NodeKind::Character {
        return Err(CoreError::Invalid(format!("「{}」不是人物节点", node.title)));
    }
    if content.trim().is_empty() {
        return Err(CoreError::Invalid("记忆内容不能为空".into()));
    }
    let m = NpcMemory {
        id: new_id(),
        npc_id: npc_id.into(),
        content: content.trim().into(),
        importance: importance.clamp(1, 10),
        turn: c.turn,
        created_at: now_ms(),
    };
    world.conn.execute(
        "INSERT INTO npc_memories (id, campaign_id, npc_id, content, importance, turn, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![m.id, campaign_id, m.npc_id, m.content, m.importance, m.turn, m.created_at],
    )?;
    Ok(m)
}

/// 按重要度与时间取 NPC 记忆。
pub fn npc_memories(conn: &Connection, campaign_id: &str, npc_id: &str, limit: i64) -> Result<Vec<NpcMemory>> {
    let mut stmt = conn.prepare(
        "SELECT id, npc_id, content, importance, turn, created_at FROM npc_memories
         WHERE campaign_id = ?1 AND npc_id = ?2 ORDER BY importance DESC, turn DESC LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![campaign_id, npc_id, limit], |r| {
        Ok(NpcMemory {
            id: r.get(0)?,
            npc_id: r.get(1)?,
            content: r.get(2)?,
            importance: r.get(3)?,
            turn: r.get(4)?,
            created_at: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 便于测试：读取当前剧情节点实体。
pub fn current_node(world: &World, campaign_id: &str) -> Result<Option<crate::model::PlotNode>> {
    let c = get_campaign(&world.conn, campaign_id)?;
    match c.current_plot_node {
        Some(id) => crate::model::PlotNode::load(&world.conn, &id),
        None => Ok(None),
    }
}

/// 当前剧情节点的场景地点（世界树节点 ID）。
pub fn current_node_scene(conn: &Connection, campaign_id: &str) -> Result<Option<String>> {
    let c = get_campaign(conn, campaign_id)?;
    let Some(id) = c.current_plot_node else { return Ok(None) };
    Ok(conn
        .query_row("SELECT scene_ref FROM plot_nodes WHERE id = ?1", [id], |r| r.get::<_, Option<String>>(0))
        .optional()?
        .flatten())
}

/// 战役当前状态的完整快照（供界面显示）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CampaignState {
    pub vars: BTreeMap<String, Value>,
    pub quests: BTreeMap<String, String>,
    /// 键为 `持有者|物品`。
    pub items: BTreeMap<String, i64>,
    pub clues: Vec<String>,
    /// 键为 `a|b|类型`。
    pub relations: BTreeMap<String, bool>,
    /// 键为 `实体|字段`。
    pub entities: BTreeMap<String, Value>,
    pub visits: BTreeMap<String, i64>,
}

pub fn campaign_state(conn: &Connection, campaign_id: &str) -> Result<CampaignState> {
    get_campaign(conn, campaign_id)?;
    let mut s = CampaignState::default();
    let q = |sql: &str| -> Result<Vec<(String, String)>> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([campaign_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    };
    for (k, v) in q("SELECT key, value FROM state_vars WHERE campaign_id = ?1")? {
        s.vars.insert(k, serde_json::from_str(&v).unwrap_or(Value::Null));
    }
    for (k, v) in q("SELECT quest_id, state FROM quest_states WHERE campaign_id = ?1")? {
        s.quests.insert(k, v);
    }
    for (k, v) in q("SELECT holder || '|' || item, CAST(qty AS TEXT) FROM inventory WHERE campaign_id = ?1")? {
        s.items.insert(k, v.parse().unwrap_or(0));
    }
    s.clues = q("SELECT clue_id, '' FROM clue_reveals WHERE campaign_id = ?1 ORDER BY turn")?.into_iter().map(|(k, _)| k).collect();
    for (k, v) in q("SELECT a || '|' || b || '|' || rel_type, CAST(active AS TEXT) FROM relation_overrides WHERE campaign_id = ?1")? {
        s.relations.insert(k, v == "1");
    }
    for (k, v) in q("SELECT entity_id || '|' || field, value FROM entity_overrides WHERE campaign_id = ?1")? {
        s.entities.insert(k, serde_json::from_str(&v).unwrap_or(Value::Null));
    }
    for (k, v) in q("SELECT plot_node_id, CAST(count AS TEXT) FROM plot_visits WHERE campaign_id = ?1")? {
        s.visits.insert(k, v.parse().unwrap_or(0));
    }
    Ok(s)
}

// ===================== GM 整合：回合、事实记忆、临时实体、会话摘要 =====================

/// 记录一条战役事件（玩家输入、叙述、NPC 发言等），返回序号。
pub fn record(conn: &Connection, campaign_id: &str, kind: &str, payload: &Value) -> Result<i64> {
    let c = get_campaign(conn, campaign_id)?;
    append_event(conn, campaign_id, c.turn, kind, payload)
}

/// 没有剧情跳转的回合结束时调用：回合数 +1。
pub fn advance_turn(conn: &Connection, campaign_id: &str) -> Result<i64> {
    conn.execute(
        "UPDATE campaigns SET turn = turn + 1, updated_at = ?2 WHERE id = ?1",
        params![campaign_id, now_ms()],
    )?;
    Ok(get_campaign(conn, campaign_id)?.turn)
}

/// 最近的若干条指定类型事件（按时间正序）。
pub fn recent_events(conn: &Connection, campaign_id: &str, kinds: &[&str], limit: i64) -> Result<Vec<EventEntry>> {
    let marks = vec!["?"; kinds.len()].join(",");
    let sql = format!(
        "SELECT seq, turn, kind, payload, created_at FROM event_log WHERE campaign_id = ?1 AND kind IN ({marks})
         ORDER BY seq DESC LIMIT {}",
        limit.clamp(1, 500)
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut args: Vec<&dyn rusqlite::ToSql> = vec![&campaign_id];
    for k in kinds {
        args.push(k);
    }
    let rows = stmt.query_map(args.as_slice(), |r| {
        Ok(EventEntry {
            seq: r.get(0)?,
            turn: r.get(1)?,
            kind: r.get(2)?,
            payload: crate::entity::jget(r, "payload")?,
            created_at: r.get(4)?,
        })
    })?;
    let mut out: Vec<EventEntry> = rows.collect::<rusqlite::Result<_>>()?;
    out.reverse();
    Ok(out)
}

// ===================== 删除（第 21 项） =====================

/// 回收站里的战役。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct DeletedCampaign {
    pub id: String,
    pub name: String,
    pub turn: i64,
    pub deleted_at: i64,
}

/// 删除战役：进回收站（数据都还在，可以恢复）。从它分出去的时间线一起进回收站。
pub fn trash_campaign(conn: &Connection, campaign_id: &str) -> Result<usize> {
    get_campaign(conn, campaign_id)?;
    let now = now_ms();
    let n = conn.execute(
        "WITH RECURSIVE t(id) AS (SELECT ?1 UNION SELECT c.id FROM campaigns c JOIN t ON c.parent_id = t.id)
         UPDATE campaigns SET deleted_at = ?2 WHERE id IN (SELECT id FROM t) AND deleted_at IS NULL",
        params![campaign_id, now],
    )?;
    Ok(n)
}

pub fn restore_campaign(conn: &Connection, campaign_id: &str) -> Result<usize> {
    let ts: Option<i64> = conn.query_row("SELECT deleted_at FROM campaigns WHERE id = ?1", [campaign_id], |r| r.get(0)).map_err(|_| CoreError::NotFound("战役", campaign_id.into()))?;
    let Some(ts) = ts else { return Ok(0) };
    Ok(conn.execute("UPDATE campaigns SET deleted_at = NULL WHERE deleted_at = ?1", [ts])?)
}

pub fn campaign_trash(conn: &Connection) -> Result<Vec<DeletedCampaign>> {
    let mut stmt = conn.prepare("SELECT id, name, turn, deleted_at FROM campaigns WHERE deleted_at IS NOT NULL ORDER BY deleted_at DESC")?;
    let rows = stmt.query_map([], |r| Ok(DeletedCampaign { id: r.get(0)?, name: r.get(1)?, turn: r.get(2)?, deleted_at: r.get(3)? }))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 永久删除回收站里的一个战役：删掉所有带 campaign_id 的表里它的数据（不能恢复）。
pub fn purge_campaign(conn: &Connection, campaign_id: &str) -> Result<usize> {
    let deleted: Option<i64> = conn.query_row("SELECT deleted_at FROM campaigns WHERE id = ?1", [campaign_id], |r| r.get(0)).map_err(|_| CoreError::NotFound("战役", campaign_id.into()))?;
    if deleted.is_none() {
        return Err(CoreError::Invalid("只能永久删除回收站里的战役".into()));
    }
    let children: i64 = conn.query_row("SELECT COUNT(*) FROM campaigns WHERE parent_id = ?1", [campaign_id], |r| r.get(0))?;
    if children > 0 {
        return Err(CoreError::Invalid("还有从它分出去的时间线，先永久删除那些时间线".into()));
    }
    let tables: Vec<String> = {
        let mut stmt = conn.prepare("SELECT m.name FROM sqlite_master m WHERE m.type = 'table' AND m.name <> 'campaigns' AND EXISTS (SELECT 1 FROM pragma_table_info(m.name) p WHERE p.name = 'campaign_id')")?;
        stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    // 事件日志平时只能追加（触发器禁止删除）。永久删除整个战役是用户明确要求的操作：
    // 只在这个事务里暂时去掉禁止删除的触发器，删完立刻按原样恢复。
    let trigger: Option<String> = conn.query_row("SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'event_log_no_delete'", [], |r| r.get(0)).ok();
    conn.execute_batch("SAVEPOINT purge_campaign")?;
    let res = (|| -> Result<usize> {
        if trigger.is_some() {
            conn.execute_batch("DROP TRIGGER event_log_no_delete")?;
        }
        let mut n = 0;
        for t in &tables {
            n += conn.execute(&format!("DELETE FROM \"{t}\" WHERE campaign_id = ?1"), [campaign_id])?;
        }
        n += conn.execute("DELETE FROM campaigns WHERE id = ?1", [campaign_id])?;
        if let Some(sql) = &trigger {
            conn.execute_batch(sql)?;
        }
        Ok(n)
    })();
    match res {
        Ok(n) => {
            conn.execute_batch("RELEASE purge_campaign")?;
            Ok(n)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO purge_campaign; RELEASE purge_campaign");
            Err(e)
        }
    }
}

/// 删除一条 NPC 记忆：内容记进事件日志（可以追溯），再从记忆表删掉。
pub fn delete_npc_memory(conn: &Connection, campaign_id: &str, memory_id: &str) -> Result<()> {
    let (npc, content): (String, String) = conn
        .query_row("SELECT npc_id, content FROM npc_memories WHERE id = ?1 AND campaign_id = ?2", [memory_id, campaign_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|_| CoreError::NotFound("NPC 记忆", memory_id.into()))?;
    record(conn, campaign_id, "npc_memory_deleted", &serde_json::json!({ "id": memory_id, "npc_id": npc, "content": content }))?;
    conn.execute("DELETE FROM npc_memories WHERE id = ?1", [memory_id])?;
    Ok(())
}

/// 忘掉一条战役事实（事件日志里原来的记录保留，另记一条"忘掉"）。
pub fn forget_fact(conn: &Connection, campaign_id: &str, fact_seq: i64) -> Result<()> {
    let kind: String = conn
        .query_row("SELECT kind FROM event_log WHERE campaign_id = ?1 AND seq = ?2", params![campaign_id, fact_seq], |r| r.get(0))
        .map_err(|_| CoreError::NotFound("事实", fact_seq.to_string()))?;
    if kind != "fact" {
        return Err(CoreError::Invalid("这不是一条事实".into()));
    }
    record(conn, campaign_id, "fact_forgotten", &serde_json::json!({ "seq": fact_seq }))?;
    Ok(())
}

/// 记住的事实（带序号，已经忘掉的不算）。
pub fn facts_with_seq(conn: &Connection, campaign_id: &str, limit: i64) -> Result<Vec<(i64, String)>> {
    let forgotten: std::collections::HashSet<i64> = recent_events(conn, campaign_id, &["fact_forgotten"], 10_000)?.into_iter().filter_map(|e| e.payload.get("seq").and_then(Value::as_i64)).collect();
    Ok(recent_events(conn, campaign_id, &["fact"], limit + forgotten.len() as i64)?
        .into_iter()
        .filter(|e| !forgotten.contains(&e.seq))
        .filter_map(|e| e.payload.get("text").and_then(Value::as_str).map(|t| (e.seq, t.to_string())))
        .collect())
}

/// 记住一条战役事实（例如"玩家答应了村长的请求"）。
pub fn add_fact(conn: &Connection, campaign_id: &str, text: &str) -> Result<()> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(());
    }
    record(conn, campaign_id, "fact", &serde_json::json!({ "text": t.chars().take(300).collect::<String>() }))?;
    Ok(())
}

pub fn facts(conn: &Connection, campaign_id: &str, limit: i64) -> Result<Vec<String>> {
    Ok(facts_with_seq(conn, campaign_id, limit)?.into_iter().map(|x| x.1).collect())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CampaignEntity {
    pub id: String,
    pub campaign_id: String,
    pub kind: String,
    pub name: String,
    pub summary: String,
    pub gm_text: String,
    /// 对应的待审变更集；用户确认后成为世界正典。
    pub changeset_id: Option<String>,
    pub created_turn: i64,
    pub created_at: i64,
}

pub fn add_campaign_entity(conn: &Connection, campaign_id: &str, kind: &str, name: &str, summary: &str, gm_text: &str, changeset_id: Option<&str>) -> Result<CampaignEntity> {
    let c = get_campaign(conn, campaign_id)?;
    if name.trim().is_empty() {
        return Err(CoreError::Invalid("名称不能为空".into()));
    }
    let e = CampaignEntity {
        id: new_id(),
        campaign_id: campaign_id.into(),
        kind: kind.into(),
        name: name.trim().into(),
        summary: summary.into(),
        gm_text: gm_text.into(),
        changeset_id: changeset_id.map(str::to_string),
        created_turn: c.turn,
        created_at: now_ms(),
    };
    conn.execute(
        "INSERT INTO campaign_entities (id, campaign_id, kind, name, summary, gm_text, changeset_id, created_turn, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![e.id, e.campaign_id, e.kind, e.name, e.summary, e.gm_text, e.changeset_id, e.created_turn, e.created_at],
    )?;
    Ok(e)
}

pub fn campaign_entities(conn: &Connection, campaign_id: &str) -> Result<Vec<CampaignEntity>> {
    let mut stmt = conn.prepare("SELECT * FROM campaign_entities WHERE campaign_id = ?1 ORDER BY created_turn, created_at")?;
    let rows = stmt.query_map([campaign_id], |r| {
        Ok(CampaignEntity {
            id: r.get("id")?,
            campaign_id: r.get("campaign_id")?,
            kind: r.get("kind")?,
            name: r.get("name")?,
            summary: r.get("summary")?,
            gm_text: r.get("gm_text")?,
            changeset_id: r.get("changeset_id")?,
            created_turn: r.get("created_turn")?,
            created_at: r.get("created_at")?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SessionSummary {
    pub id: String,
    pub session_no: i64,
    pub summary: String,
    /// draft（待确认）/ confirmed（已确认，GM 会读取）/ auto（自动压缩的，GM 会读取，可以确认、重新生成或删除）
    pub status: String,
    pub upto_seq: i64,
    pub created_at: i64,
    /// manual（手动生成）/ auto（上下文压缩）
    #[serde(default)]
    pub source: String,
    /// 压缩前的游戏记录 token 数 / 摘要的 token 数。
    #[serde(default)]
    pub tokens_before: Option<i64>,
    #[serde(default)]
    pub tokens_after: Option<i64>,
    /// measured（服务端分词实测）/ estimated（估算）
    #[serde(default)]
    pub token_method: String,
    /// 覆盖的事件范围起点（不含）。
    #[serde(default)]
    pub from_seq: i64,
}

fn summary_row(r: &rusqlite::Row) -> rusqlite::Result<SessionSummary> {
    Ok(SessionSummary {
        id: r.get("id")?,
        session_no: r.get("session_no")?,
        summary: r.get("summary")?,
        status: r.get("status")?,
        upto_seq: r.get("upto_seq")?,
        created_at: r.get("created_at")?,
        source: r.get("source")?,
        tokens_before: r.get("tokens_before")?,
        tokens_after: r.get("tokens_after")?,
        token_method: r.get("token_method")?,
        from_seq: r.get("from_seq")?,
    })
}

pub fn summaries(conn: &Connection, campaign_id: &str) -> Result<Vec<SessionSummary>> {
    let mut stmt = conn.prepare("SELECT * FROM session_summaries WHERE campaign_id = ?1 ORDER BY session_no")?;
    let rows = stmt.query_map([campaign_id], summary_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 已确认摘要覆盖到的最后一条事件序号（之后的事件需要进入下一次摘要）。
pub fn summarized_upto(conn: &Connection, campaign_id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(upto_seq), 0) FROM session_summaries WHERE campaign_id = ?1 AND status IN ('confirmed', 'auto')",
        [campaign_id],
        |r| r.get(0),
    )?)
}

pub fn add_summary_draft(conn: &Connection, campaign_id: &str, summary: &str, upto_seq: i64) -> Result<SessionSummary> {
    get_campaign(conn, campaign_id)?;
    // 只保留一份未确认的草稿。
    conn.execute("DELETE FROM session_summaries WHERE campaign_id = ?1 AND status = 'draft'", [campaign_id])?;
    let no: i64 = conn.query_row("SELECT COALESCE(MAX(session_no), 0) + 1 FROM session_summaries WHERE campaign_id = ?1", [campaign_id], |r| r.get(0))?;
    let id = new_id();
    conn.execute(
        "INSERT INTO session_summaries (id, campaign_id, session_no, summary, status, created_at, upto_seq) VALUES (?1,?2,?3,?4,'draft',?5,?6)",
        params![id, campaign_id, no, summary.trim(), now_ms(), upto_seq],
    )?;
    Ok(conn.query_row("SELECT * FROM session_summaries WHERE id = ?1", [&id], summary_row)?)
}

/// 上下文压缩写入的自动摘要（GM 直接使用；玩家可以确认、重新生成或删除）。
#[allow(clippy::too_many_arguments)]
pub fn add_auto_summary(conn: &Connection, campaign_id: &str, summary: &str, from_seq: i64, upto_seq: i64, tokens_before: i64, tokens_after: i64, method: &str) -> Result<SessionSummary> {
    get_campaign(conn, campaign_id)?;
    let no: i64 = conn.query_row("SELECT COALESCE(MAX(session_no), 0) + 1 FROM session_summaries WHERE campaign_id = ?1", [campaign_id], |r| r.get(0))?;
    let id = new_id();
    conn.execute(
        "INSERT INTO session_summaries (id, campaign_id, session_no, summary, status, created_at, upto_seq, source, tokens_before, tokens_after, token_method, from_seq)
         VALUES (?1,?2,?3,?4,'auto',?5,?6,'auto',?7,?8,?9,?10)",
        params![id, campaign_id, no, summary.trim(), now_ms(), upto_seq, tokens_before, tokens_after, method, from_seq],
    )?;
    Ok(conn.query_row("SELECT * FROM session_summaries WHERE id = ?1", [&id], summary_row)?)
}

/// 删除一份草稿或自动摘要（只能删最新的自动摘要，避免中间留下空洞；原始游戏记录一直保留）。
pub fn delete_summary(conn: &Connection, campaign_id: &str, summary_id: &str) -> Result<()> {
    let s = conn
        .query_row("SELECT * FROM session_summaries WHERE id = ?1 AND campaign_id = ?2", [summary_id, campaign_id], summary_row)
        .map_err(|_| CoreError::NotFound("摘要", summary_id.into()))?;
    if s.status == "auto" || s.status == "confirmed" {
        let newer: i64 = conn.query_row("SELECT COUNT(*) FROM session_summaries WHERE campaign_id = ?1 AND status IN ('auto','confirmed') AND upto_seq > ?2", params![campaign_id, s.upto_seq], |r| r.get(0))?;
        if newer > 0 {
            return Err(CoreError::Invalid("只能删除最新的一份摘要".into()));
        }
    }
    if s.status == "confirmed" {
        record(conn, campaign_id, "summary_deleted", &serde_json::json!({ "session_no": s.session_no, "summary": s.summary, "upto_seq": s.upto_seq }))?;
    }
    conn.execute("DELETE FROM session_summaries WHERE id = ?1", [summary_id])?;
    Ok(())
}

/// 用户确认（可修改文字）后，摘要才会被 GM 作为战役记忆使用。
pub fn confirm_summary(conn: &Connection, summary_id: &str, text: Option<&str>) -> Result<()> {
    let n = match text {
        Some(t) if !t.trim().is_empty() => conn.execute(
            "UPDATE session_summaries SET status = 'confirmed', summary = ?2 WHERE id = ?1 AND status IN ('draft', 'auto')",
            params![summary_id, t.trim()],
        )?,
        _ => conn.execute("UPDATE session_summaries SET status = 'confirmed' WHERE id = ?1 AND status IN ('draft', 'auto')", [summary_id])?,
    };
    if n == 0 {
        return Err(CoreError::Invalid("摘要不存在或已经确认".into()));
    }
    Ok(())
}

/// 中断的回合：有玩家输入（`player` 事件），之后却没有回合结束或中断的记录（例如程序崩溃）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct IncompleteTurn {
    pub seq: i64,
    pub input: String,
    /// 这个回合里已经发生的事件类型（如 roll、effects、transition）。
    pub happened: Vec<String>,
}

pub fn incomplete_turn(conn: &Connection, campaign_id: &str) -> Result<Option<IncompleteTurn>> {
    let last_player: Option<(i64, String)> = conn
        .query_row(
            "SELECT seq, payload FROM event_log WHERE campaign_id = ?1 AND kind = 'player' ORDER BY seq DESC LIMIT 1",
            [campaign_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((seq, payload)) = last_player else { return Ok(None) };
    let closed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM event_log WHERE campaign_id = ?1 AND seq > ?2 AND kind IN ('turn_end', 'turn_aborted')",
        params![campaign_id, seq],
        |r| r.get(0),
    )?;
    if closed > 0 {
        return Ok(None);
    }
    let mut stmt = conn.prepare("SELECT kind FROM event_log WHERE campaign_id = ?1 AND seq > ?2 ORDER BY seq")?;
    let happened = stmt.query_map(params![campaign_id, seq], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let input = serde_json::from_str::<Value>(&payload).ok().and_then(|v| v.get("text").and_then(Value::as_str).map(str::to_string)).unwrap_or_default();
    Ok(Some(IncompleteTurn { seq, input, happened }))
}

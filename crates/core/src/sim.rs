//! 剧情模拟：在内存中的状态上预演分支，不写数据库、不创建战役。
//! 用于设计剧情时检查"在某种状态下哪些分支可走、走了会改变什么"。

use std::collections::{BTreeMap, BTreeSet};

use chronicle_rules::{CheckRecord, SeededDice, StateChange, StateView, Tri, resolve_effects};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::campaign::EdgeOption;
use crate::error::{CoreError, Result};
use crate::model::PlotNodeKind;
use crate::plot;

/// 模拟状态。键均为字符串，便于界面编辑：物品 `持有者|物品`，关系 `a|b|类型`，实体字段 `实体|字段`。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SimState {
    #[serde(default)]
    pub current: Option<String>,
    #[serde(default)]
    pub vars: BTreeMap<String, Value>,
    #[serde(default)]
    pub quests: BTreeMap<String, String>,
    #[serde(default)]
    pub items: BTreeMap<String, i64>,
    #[serde(default)]
    pub clues: BTreeSet<String>,
    #[serde(default)]
    pub relations: BTreeMap<String, bool>,
    #[serde(default)]
    pub entities: BTreeMap<String, Value>,
    #[serde(default)]
    pub visits: BTreeMap<String, i64>,
    /// 人物关系（"from|to" → (类型, 数值)）。
    #[serde(default)]
    pub bonds: BTreeMap<String, (String, chronicle_rules::effect::Bond)>,
    #[serde(default)]
    pub moods: BTreeMap<String, chronicle_rules::effect::Mood>,
    #[serde(default)]
    pub records: BTreeMap<String, Value>,
    #[serde(default)]
    pub used_edges: BTreeMap<String, i64>,
    #[serde(default)]
    pub unlocked: BTreeSet<String>,
    #[serde(default)]
    pub rolls: u64,
    #[serde(default)]
    pub history: Vec<String>,
}

struct SimView<'a> {
    conn: &'a Connection,
    s: &'a SimState,
}

impl StateView for SimView<'_> {
    fn mood(&self, npc: &str) -> Option<chronicle_rules::effect::Mood> {
        self.s.moods.get(npc).copied()
    }
    fn record(&self, kind: &str, id: &str) -> Option<Value> {
        self.s.records.get(&format!("{kind}|{id}")).cloned()
    }
    fn bond(&self, from: &str, to: &str) -> Option<(String, chronicle_rules::effect::Bond)> {
        self.s.bonds.get(&format!("{from}|{to}")).cloned()
    }
    fn var(&self, key: &str) -> Option<Value> {
        self.s.vars.get(key).cloned()
    }
    fn quest_state(&self, quest: &str) -> Option<String> {
        self.s.quests.get(quest).cloned()
    }
    fn item_qty(&self, holder: &str, item: &str) -> i64 {
        self.s.items.get(&format!("{holder}|{item}")).copied().unwrap_or(0)
    }
    fn clue_known(&self, clue: &str) -> bool {
        self.s.clues.contains(clue)
    }
    fn relation(&self, a: &str, b: &str, rel_type: &str) -> bool {
        if let Some(v) = self.s.relations.get(&format!("{a}|{b}|{rel_type}")) {
            return *v;
        }
        // 回落到世界正典中的关系。
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
        self.s.visits.get(node).copied().unwrap_or(0)
    }
}

fn apply(s: &mut SimState, changes: &[StateChange]) {
    for ch in changes {
        match ch {
            StateChange::SetVar { key, new, .. } => {
                s.vars.insert(key.clone(), new.clone());
            }
            StateChange::SetQuestState { quest, new, .. } => {
                s.quests.insert(quest.clone(), new.clone());
            }
            StateChange::SetItemQty { holder, item, new, .. } => {
                s.items.insert(format!("{holder}|{item}"), *new);
            }
            StateChange::RevealClue { clue } => {
                s.clues.insert(clue.clone());
            }
            StateChange::SetRelation { a, b, rel_type, active } => {
                s.relations.insert(format!("{a}|{b}|{rel_type}"), *active);
            }
            StateChange::SetEntityField { entity, field, value } => {
                s.entities.insert(format!("{entity}|{field}"), value.clone());
            }
            StateChange::UnlockNode { node } => {
                s.unlocked.insert(node.clone());
            }
            StateChange::AdjustBond { from, to, kind, new, .. } => {
                s.bonds.insert(format!("{from}|{to}"), (kind.clone(), *new));
            }
            StateChange::AdjustMood { npc, new, .. } => {
                s.moods.insert(npc.clone(), *new);
            }
            StateChange::UpsertRecord { kind, id, new, .. } => {
                s.records.insert(format!("{kind}|{id}"), new.clone());
            }
            StateChange::DeleteRecord { kind, id, .. } => {
                s.records.remove(&format!("{kind}|{id}"));
            }
            StateChange::Spawn { .. } => {}
        }
    }
}

/// 新的模拟状态：从剧本唯一的开场节点开始。
pub fn sim_start(conn: &Connection, script_id: &str) -> Result<SimState> {
    let starts: Vec<_> = plot::plot_nodes_of(conn, script_id)?.into_iter().filter(|n| n.kind == PlotNodeKind::Start).collect();
    let mut s = SimState::default();
    if let [only] = starts.as_slice() {
        s.current = Some(only.id.clone());
        s.visits.insert(only.id.clone(), 1);
    }
    Ok(s)
}

/// 当前节点的出边及其在该状态下的可用性。
pub fn sim_options(conn: &Connection, s: &SimState) -> Result<Vec<EdgeOption>> {
    let Some(cur) = &s.current else { return Ok(vec![]) };
    let view = SimView { conn, s };
    let mut out = vec![];
    for edge in plot::outgoing(conn, cur)? {
        let used_up = edge.once && s.used_edges.contains_key(&edge.id);
        let availability = match (&edge.condition, used_up) {
            (_, true) => Tri::False,
            (None, false) => Tri::True,
            (Some(c), false) => c.preview(&view)?,
        };
        out.push(EdgeOption { edge, availability, used_up });
    }
    Ok(out)
}

/// 检定在模拟中的处理方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CheckMode {
    /// 按种子真实掷骰。
    Roll,
    /// 所有检定视为成功。
    Pass,
    /// 所有检定视为失败。
    Fail,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SimStep {
    pub state: SimState,
    pub taken: bool,
    pub checks: Vec<CheckRecord>,
    pub changes: Vec<StateChange>,
}

/// 在模拟状态上走一条边（不修改数据库）。条件不满足时状态只记录这次尝试。
pub fn sim_step(conn: &Connection, s: &SimState, edge_id: &str, mode: CheckMode, seed: u64) -> Result<SimStep> {
    let edge = plot::get_edge(conn, edge_id)?.ok_or_else(|| CoreError::NotFound("剧情边", edge_id.into()))?;
    if s.current.as_deref() != Some(edge.from_id.as_str()) {
        return Err(CoreError::Invalid("这条分支不是从当前节点出发的".into()));
    }
    if edge.once && s.used_edges.contains_key(&edge.id) {
        return Err(CoreError::Invalid("这条分支只能走一次，已经走过".into()));
    }
    let to = plot::require_plot_node(conn, &edge.to_id)?;
    let mut next = s.clone();
    let (passed, checks) = {
        let view = SimView { conn, s };
        match (&edge.condition, mode) {
            (None, _) => (true, vec![]),
            (Some(c), CheckMode::Roll) => {
                next.rolls += 1;
                let r = c.eval(&view, &mut SeededDice::new(seed, s.rolls))?;
                (r.passed, r.checks)
            }
            (Some(c), m) => (c.eval_forced(&view, m == CheckMode::Pass)?, vec![]),
        }
    };
    let label = if edge.label.is_empty() { to.title.clone() } else { edge.label.clone() };
    if !passed {
        next.history.push(format!("尝试「{label}」：条件不满足"));
        return Ok(SimStep { state: next, taken: false, checks, changes: vec![] });
    }
    let changes = {
        let view = SimView { conn, s: &next };
        resolve_effects(&edge.effects, &view)?
    };
    apply(&mut next, &changes);
    *next.used_edges.entry(edge.id.clone()).or_default() += 1;
    *next.visits.entry(to.id.clone()).or_default() += 1;
    next.current = Some(to.id.clone());
    next.history.push(format!("「{label}」→ {}", to.title));
    Ok(SimStep { state: next, taken: true, checks, changes })
}

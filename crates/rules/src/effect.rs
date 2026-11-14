//! 效果：剧情跳转或事件触发后对战役状态的修改。
//!
//! 效果先被 `resolve` 成确定的 `StateChange` 列表（读取当前状态计算新值），
//! 再由存储层在事务中写入。AI 只能提议效果，执行一律由程序完成。

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::condition::flag_key;
use crate::{RulesError, StateView};

fn default_true() -> bool {
    true
}
fn default_one() -> i64 {
    1
}
fn default_party() -> String {
    "party".into()
}

/// 人物关系的数值：`from` 对 `to` 的看法（双方可以不同）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Bond {
    /// 关系强度 0–100（羁绊有多深，不论好坏）。
    pub strength: i64,
    /// 信任 0–100。
    pub trust: i64,
    /// 好感 -100–100。
    pub affection: i64,
    /// 敌意 0–100。
    pub hostility: i64,
}

impl Bond {
    /// 叠加增量并限制在各自范围内。
    pub fn adjusted(self, strength: i64, trust: i64, affection: i64, hostility: i64) -> Bond {
        Bond {
            strength: (self.strength + strength).clamp(0, 100),
            trust: (self.trust + trust).clamp(0, 100),
            affection: (self.affection + affection).clamp(-100, 100),
            hostility: (self.hostility + hostility).clamp(0, 100),
        }
    }
}

/// NPC 的心理状态（各 0–100）。对玩家的信任在人物关系（Bond）里。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Mood {
    pub fear: i64,
    pub anger: i64,
    pub suspicion: i64,
    pub stress: i64,
    pub loyalty: i64,
}

impl Mood {
    pub fn adjusted(self, fear: i64, anger: i64, suspicion: i64, stress: i64, loyalty: i64) -> Mood {
        let c = |v: i64| v.clamp(0, 100);
        Mood { fear: c(self.fear + fear), anger: c(self.anger + anger), suspicion: c(self.suspicion + suspicion), stress: c(self.stress + stress), loyalty: c(self.loyalty + loyalty) }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Effect {
    SetVar { key: String, value: Value },
    IncVar { key: String, by: f64 },
    SetFlag {
        name: String,
        #[serde(default = "default_true")]
        value: bool,
    },
    SetQuestState { quest: String, state: String },
    GiveItem {
        #[serde(default = "default_party")]
        holder: String,
        item: String,
        #[serde(default = "default_one")]
        qty: i64,
    },
    TakeItem {
        #[serde(default = "default_party")]
        holder: String,
        item: String,
        #[serde(default = "default_one")]
        qty: i64,
    },
    RevealClue { clue: String },
    SetRelation {
        a: String,
        b: String,
        rel_type: String,
        #[serde(default = "default_true")]
        active: bool,
    },
    /// 战役内覆盖实体字段（如 NPC 死亡），不修改世界正典。
    SetEntityState { entity: String, field: String, value: Value },
    UnlockNode { node: String },
    /// 调整人物关系：`from` 对 `to` 的看法按增量变化（必须写明原因，变化记录在事件日志里）。
    /// `mutual` 为真时 `to` 对 `from` 的看法同样变化。`kind` 不为空时同时设定关系类型（朋友、敌人、雇佣……）。
    AdjustBond {
        from: String,
        to: String,
        #[serde(default)]
        kind: String,
        #[serde(default)]
        strength: i64,
        #[serde(default)]
        trust: i64,
        #[serde(default)]
        affection: i64,
        #[serde(default)]
        hostility: i64,
        reason: String,
        #[serde(default)]
        mutual: bool,
    },
    /// 引用一个已批准的变更集，把其中内容带入当前战役。
    Spawn { changeset: String },
    /// 调整 NPC 情绪（增量，必须写明原因）。
    AdjustMood {
        npc: String,
        #[serde(default)]
        fear: i64,
        #[serde(default)]
        anger: i64,
        #[serde(default)]
        suspicion: i64,
        #[serde(default)]
        stress: i64,
        #[serde(default)]
        loyalty: i64,
        reason: String,
    },
    /// 写入一条战役记录（调查板的线索、关联、推理，动态任务等）。整条替换。
    UpsertRecord { kind: String, id: String, data: Value },
    /// 删除一条战役记录。
    DeleteRecord { kind: String, id: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StateChange {
    SetVar { key: String, old: Option<Value>, new: Value },
    SetQuestState { quest: String, old: Option<String>, new: String },
    SetItemQty { holder: String, item: String, old: i64, new: i64 },
    RevealClue { clue: String },
    SetRelation { a: String, b: String, rel_type: String, active: bool },
    SetEntityField { entity: String, field: String, value: Value },
    UnlockNode { node: String },
    Spawn { changeset: String },
    /// 人物关系变化（前后完整数值与原因，便于查看关系的形成历史）。
    AdjustBond { from: String, to: String, old_kind: String, kind: String, old: Bond, new: Bond, reason: String },
    /// NPC 情绪变化（前后数值与原因）。
    AdjustMood { npc: String, old: Mood, new: Mood, reason: String },
    UpsertRecord { kind: String, id: String, old: Option<Value>, new: Value },
    DeleteRecord { kind: String, id: String, old: Option<Value> },
}

impl Effect {
    /// 效果引用的实体（任务、物品、线索、剧情节点、实体、关系两端），供引用校验使用。
    pub fn referenced_ids(&self, out: &mut Vec<(&'static str, String)>) {
        match self {
            Effect::SetQuestState { quest, .. } => out.push(("quest", quest.clone())),
            Effect::GiveItem { item, .. } | Effect::TakeItem { item, .. } => out.push(("item", item.clone())),
            Effect::RevealClue { clue } => out.push(("clue", clue.clone())),
            Effect::UnlockNode { node } => out.push(("plot_node", node.clone())),
            Effect::SetEntityState { entity, .. } => out.push(("entity", entity.clone())),
            Effect::AdjustBond { from, to, .. } => {
                for x in [from, to] {
                    if x != "party" {
                        out.push(("entity", x.clone()));
                    }
                }
            }
            Effect::SetRelation { a, b, .. } => {
                out.push(("entity", a.clone()));
                out.push(("entity", b.clone()));
            }
            _ => {}
        }
    }

    pub fn validate(&self) -> Result<(), RulesError> {
        let empty = |s: &str, what: &str| {
            if s.trim().is_empty() {
                Err(RulesError::Invalid(format!("{what} 不能为空")))
            } else {
                Ok(())
            }
        };
        match self {
            Effect::IncVar { by, .. } if !by.is_finite() => Err(RulesError::Invalid("增量必须是有限数".into())),
            Effect::SetVar { key, .. } | Effect::IncVar { key, .. } => empty(key, "变量名"),
            Effect::GiveItem { item, qty, .. } | Effect::TakeItem { item, qty, .. } => {
                empty(item, "物品")?;
                if *qty <= 0 {
                    return Err(RulesError::Invalid("物品数量必须为正".into()));
                }
                Ok(())
            }
            Effect::SetFlag { name, .. } => empty(name, "标志名"),
            Effect::SetQuestState { quest, state } => {
                empty(quest, "任务")?;
                empty(state, "任务状态")
            }
            Effect::RevealClue { clue } => empty(clue, "线索"),
            Effect::SetRelation { a, b, rel_type, .. } => {
                empty(a, "关系一方")?;
                empty(b, "关系一方")?;
                empty(rel_type, "关系类型")
            }
            Effect::SetEntityState { entity, field, .. } => {
                empty(entity, "实体")?;
                empty(field, "字段")
            }
            Effect::UnlockNode { node } => empty(node, "剧情节点"),
            Effect::Spawn { changeset } => empty(changeset, "变更集"),
            Effect::AdjustMood { npc, reason, fear, anger, suspicion, stress, loyalty } => {
                empty(npc, "人物")?;
                empty(reason, "情绪变化的原因")?;
                if [fear, anger, suspicion, stress, loyalty].iter().any(|v| v.abs() > 200) {
                    return Err(RulesError::Invalid("情绪数值的增量超出范围".into()));
                }
                Ok(())
            }
            Effect::UpsertRecord { kind, id, data } => {
                empty(kind, "记录类型")?;
                empty(id, "记录 ID")?;
                if !data.is_object() {
                    return Err(RulesError::Invalid("记录内容必须是对象".into()));
                }
                Ok(())
            }
            Effect::DeleteRecord { kind, id } => {
                empty(kind, "记录类型")?;
                empty(id, "记录 ID")
            }
            Effect::AdjustBond { from, to, reason, strength, trust, affection, hostility, .. } => {
                empty(from, "关系一方")?;
                empty(to, "关系一方")?;
                if from == to {
                    return Err(RulesError::Invalid("关系的两端不能是同一个人".into()));
                }
                empty(reason, "关系变化的原因")?;
                if [strength, trust, affection, hostility].iter().any(|v| v.abs() > 200) {
                    return Err(RulesError::Invalid("关系数值的增量超出范围".into()));
                }
                Ok(())
            }
        }
    }
}

/// 在基础状态之上叠加尚未提交的变化，使后续效果能看到前面效果的结果。
struct Overlay<'a> {
    base: &'a dyn StateView,
    vars: HashMap<String, Value>,
    quests: HashMap<String, String>,
    items: HashMap<(String, String), i64>,
    clues: HashSet<String>,
    relations: HashMap<(String, String, String), bool>,
    bonds: HashMap<(String, String), (String, Bond)>,
    moods: HashMap<String, Mood>,
    records: HashMap<(String, String), Option<Value>>,
}

impl StateView for Overlay<'_> {
    fn mood(&self, npc: &str) -> Option<Mood> {
        self.moods.get(npc).copied().or_else(|| self.base.mood(npc))
    }
    fn record(&self, kind: &str, id: &str) -> Option<Value> {
        match self.records.get(&(kind.to_string(), id.to_string())) {
            Some(v) => v.clone(),
            None => self.base.record(kind, id),
        }
    }
    fn bond(&self, from: &str, to: &str) -> Option<(String, Bond)> {
        self.bonds.get(&(from.to_string(), to.to_string())).cloned().or_else(|| self.base.bond(from, to))
    }
    fn var(&self, key: &str) -> Option<Value> {
        self.vars.get(key).cloned().or_else(|| self.base.var(key))
    }
    fn quest_state(&self, quest: &str) -> Option<String> {
        self.quests.get(quest).cloned().or_else(|| self.base.quest_state(quest))
    }
    fn item_qty(&self, holder: &str, item: &str) -> i64 {
        self.items
            .get(&(holder.to_string(), item.to_string()))
            .copied()
            .unwrap_or_else(|| self.base.item_qty(holder, item))
    }
    fn clue_known(&self, clue: &str) -> bool {
        self.clues.contains(clue) || self.base.clue_known(clue)
    }
    fn relation(&self, a: &str, b: &str, rel_type: &str) -> bool {
        self.relations
            .get(&(a.to_string(), b.to_string(), rel_type.to_string()))
            .copied()
            .unwrap_or_else(|| self.base.relation(a, b, rel_type))
    }
    fn visit_count(&self, node: &str) -> i64 {
        self.base.visit_count(node)
    }
    fn skill_mod(&self, actor: &str, skill: &str) -> i64 {
        match self.var(&format!("skill.{actor}.{skill}")) {
            Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
            _ => self.base.skill_mod(actor, skill),
        }
    }
}

/// 把效果列表按顺序解析为确定的状态变化。任何一个效果非法则整体失败，不产生部分结果。
pub fn resolve_effects(effects: &[Effect], view: &dyn StateView) -> Result<Vec<StateChange>, RulesError> {
    let mut ov = Overlay {
        base: view,
        vars: HashMap::new(),
        quests: HashMap::new(),
        items: HashMap::new(),
        clues: HashSet::new(),
        relations: HashMap::new(),
        bonds: HashMap::new(),
        moods: HashMap::new(),
        records: HashMap::new(),
    };
    let mut out = Vec::with_capacity(effects.len());
    for e in effects {
        e.validate()?;
        match e {
            Effect::SetVar { key, value } => {
                let old = ov.var(key);
                ov.vars.insert(key.clone(), value.clone());
                out.push(StateChange::SetVar { key: key.clone(), old, new: value.clone() });
            }
            Effect::IncVar { key, by } => {
                let old = ov.var(key);
                let base = match &old {
                    None | Some(Value::Null) => 0.0,
                    Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
                    Some(other) => {
                        return Err(RulesError::Invalid(format!("变量 {key} 不是数字: {other}")));
                    }
                };
                let sum = base + by;
                // 整数保持整数表示。
                let new = if sum.fract() == 0.0 && sum.abs() < 9.0e15 {
                    Value::from(sum as i64)
                } else {
                    serde_json::Number::from_f64(sum)
                        .map(Value::Number)
                        .ok_or_else(|| RulesError::Invalid("数值溢出".into()))?
                };
                ov.vars.insert(key.clone(), new.clone());
                out.push(StateChange::SetVar { key: key.clone(), old, new });
            }
            Effect::SetFlag { name, value } => {
                let key = flag_key(name);
                let old = ov.var(&key);
                let new = Value::Bool(*value);
                ov.vars.insert(key.clone(), new.clone());
                out.push(StateChange::SetVar { key, old, new });
            }
            Effect::SetQuestState { quest, state } => {
                let old = ov.quest_state(quest);
                ov.quests.insert(quest.clone(), state.clone());
                out.push(StateChange::SetQuestState { quest: quest.clone(), old, new: state.clone() });
            }
            Effect::GiveItem { holder, item, qty } | Effect::TakeItem { holder, item, qty } => {
                let old = ov.item_qty(holder, item);
                let new = if matches!(e, Effect::GiveItem { .. }) {
                    old.checked_add(*qty).ok_or_else(|| RulesError::Invalid("物品数量溢出".into()))?
                } else {
                    if old < *qty {
                        return Err(RulesError::InsufficientItem {
                            holder: holder.clone(),
                            item: item.clone(),
                            have: old,
                            need: *qty,
                        });
                    }
                    old - qty
                };
                ov.items.insert((holder.clone(), item.clone()), new);
                out.push(StateChange::SetItemQty { holder: holder.clone(), item: item.clone(), old, new });
            }
            Effect::RevealClue { clue } => {
                ov.clues.insert(clue.clone());
                out.push(StateChange::RevealClue { clue: clue.clone() });
            }
            Effect::SetRelation { a, b, rel_type, active } => {
                ov.relations.insert((a.clone(), b.clone(), rel_type.clone()), *active);
                out.push(StateChange::SetRelation {
                    a: a.clone(),
                    b: b.clone(),
                    rel_type: rel_type.clone(),
                    active: *active,
                });
            }
            Effect::SetEntityState { entity, field, value } => out.push(StateChange::SetEntityField {
                entity: entity.clone(),
                field: field.clone(),
                value: value.clone(),
            }),
            Effect::UnlockNode { node } => out.push(StateChange::UnlockNode { node: node.clone() }),
            Effect::Spawn { changeset } => out.push(StateChange::Spawn { changeset: changeset.clone() }),
            Effect::AdjustMood { npc, fear, anger, suspicion, stress, loyalty, reason } => {
                let old = ov.mood(npc).unwrap_or_default();
                let new = old.adjusted(*fear, *anger, *suspicion, *stress, *loyalty);
                ov.moods.insert(npc.clone(), new);
                out.push(StateChange::AdjustMood { npc: npc.clone(), old, new, reason: reason.trim().to_string() });
            }
            Effect::UpsertRecord { kind, id, data } => {
                let old = ov.record(kind, id);
                ov.records.insert((kind.clone(), id.clone()), Some(data.clone()));
                out.push(StateChange::UpsertRecord { kind: kind.clone(), id: id.clone(), old, new: data.clone() });
            }
            Effect::DeleteRecord { kind, id } => {
                let old = ov.record(kind, id);
                ov.records.insert((kind.clone(), id.clone()), None);
                out.push(StateChange::DeleteRecord { kind: kind.clone(), id: id.clone(), old });
            }
            Effect::AdjustBond { from, to, kind, strength, trust, affection, hostility, reason, mutual } => {
                let mut pairs = vec![(from.clone(), to.clone())];
                if *mutual {
                    pairs.push((to.clone(), from.clone()));
                }
                for (a, b) in pairs {
                    let (old_kind, old) = ov.bond(&a, &b).unwrap_or_default();
                    let new = old.adjusted(*strength, *trust, *affection, *hostility);
                    let new_kind = if kind.trim().is_empty() { old_kind.clone() } else { kind.trim().to_string() };
                    ov.bonds.insert((a.clone(), b.clone()), (new_kind.clone(), new));
                    out.push(StateChange::AdjustBond { from: a, to: b, old_kind, kind: new_kind, old, new, reason: reason.trim().to_string() });
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod mood_record_tests {
    use super::*;
    use crate::testing::MapState;

    #[test]
    fn moods_clamp_and_records_see_earlier_effects() {
        let st = MapState::default();
        let e: Vec<Effect> = serde_json::from_value(serde_json::json!([
            {"type": "adjust_mood", "npc": "n1", "fear": 70, "anger": -5, "reason": "玩家拔剑"},
            {"type": "adjust_mood", "npc": "n1", "fear": 50, "reason": "又被威胁"},
            {"type": "upsert_record", "kind": "clue", "id": "c1", "data": {"title": "湿脚印"}},
            {"type": "upsert_record", "kind": "clue", "id": "c1", "data": {"title": "湿脚印（更新）"}},
            {"type": "delete_record", "kind": "clue", "id": "c1"}
        ])).unwrap();
        let ch = resolve_effects(&e, &st).unwrap();
        match &ch[1] {
            StateChange::AdjustMood { old, new, .. } => assert_eq!((old.fear, new.fear, new.anger), (70, 100, 0)),
            other => panic!("{other:?}"),
        }
        match &ch[3] {
            StateChange::UpsertRecord { old, .. } => assert_eq!(old.as_ref().unwrap()["title"], "湿脚印"),
            other => panic!("{other:?}"),
        }
        match &ch[4] {
            StateChange::DeleteRecord { old, .. } => assert_eq!(old.as_ref().unwrap()["title"], "湿脚印（更新）"),
            other => panic!("{other:?}"),
        }
        let bad: Effect = serde_json::from_value(serde_json::json!({"type": "adjust_mood", "npc": "n1", "fear": 5, "reason": ""})).unwrap();
        assert!(resolve_effects(&[bad], &st).is_err(), "情绪变化必须有原因");
    }
}

#[cfg(test)]
mod bond_tests {
    use super::*;
    use crate::testing::MapState;

    #[test]
    fn adjust_bond_is_directional_clamped_and_needs_reason() {
        let st = MapState::default();
        let e: Effect = serde_json::from_value(serde_json::json!({
            "type": "adjust_bond", "from": "npc1", "to": "party", "kind": "friend", "trust": 30, "affection": 150, "reason": "玩家救了他"
        })).unwrap();
        let ch = resolve_effects(&[e.clone(), e], &st).unwrap();
        // 第二次在第一次的结果上叠加（能看到前面效果的结果），并限制在范围内。
        match &ch[1] {
            StateChange::AdjustBond { from, to, old, new, kind, .. } => {
                assert_eq!((from.as_str(), to.as_str(), kind.as_str()), ("npc1", "party", "friend"));
                assert_eq!(old.trust, 30);
                assert_eq!((new.trust, new.affection), (60, 100));
            }
            other => panic!("{other:?}"),
        }
        let mutual: Effect = serde_json::from_value(serde_json::json!({"type": "adjust_bond", "from": "a", "to": "b", "hostility": 20, "reason": "争吵", "mutual": true})).unwrap();
        let ch = resolve_effects(&[mutual], &st).unwrap();
        assert_eq!(ch.len(), 2, "双向：双方的看法都变化");
        let no_reason: Effect = serde_json::from_value(serde_json::json!({"type": "adjust_bond", "from": "a", "to": "b", "trust": 5, "reason": " "})).unwrap();
        assert!(resolve_effects(&[no_reason], &st).is_err(), "关系变化必须有原因");
        let same: Effect = serde_json::from_value(serde_json::json!({"type": "adjust_bond", "from": "a", "to": "a", "trust": 5, "reason": "x"})).unwrap();
        assert!(resolve_effects(&[same], &st).is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MapState;
    use serde_json::json;

    fn effects(v: Value) -> Vec<Effect> {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn sequential_effects_see_previous_results() {
        let mut s = MapState::default();
        s.vars.insert("gold".into(), json!(5));
        let ch = resolve_effects(
            &effects(json!([
                {"type":"inc_var","key":"gold","by":10},
                {"type":"inc_var","key":"gold","by":-3},
                {"type":"give_item","item":"key"},
                {"type":"take_item","item":"key"},
                {"type":"set_flag","name":"met_king"}
            ])),
            &s,
        )
        .unwrap();
        assert_eq!(ch[0], StateChange::SetVar { key: "gold".into(), old: Some(json!(5)), new: json!(15) });
        assert_eq!(ch[1], StateChange::SetVar { key: "gold".into(), old: Some(json!(15)), new: json!(12) });
        assert_eq!(
            ch[3],
            StateChange::SetItemQty { holder: "party".into(), item: "key".into(), old: 1, new: 0 }
        );
        assert_eq!(
            ch[4],
            StateChange::SetVar { key: "flag.met_king".into(), old: None, new: json!(true) }
        );
    }

    #[test]
    fn insufficient_item_fails_whole_batch() {
        let s = MapState::default();
        let r = resolve_effects(
            &effects(json!([{"type":"set_var","key":"x","value":1},{"type":"take_item","item":"key"}])),
            &s,
        );
        assert!(matches!(r, Err(RulesError::InsufficientItem { .. })));
    }

    #[test]
    fn inc_non_number_fails() {
        let mut s = MapState::default();
        s.vars.insert("name".into(), json!("abc"));
        assert!(resolve_effects(&effects(json!([{"type":"inc_var","key":"name","by":1}])), &s).is_err());
    }

    #[test]
    fn invalid_effects_rejected() {
        let s = MapState::default();
        assert!(resolve_effects(&effects(json!([{"type":"give_item","item":"k","qty":0}])), &s).is_err());
        assert!(resolve_effects(&effects(json!([{"type":"set_var","key":" ","value":1}])), &s).is_err());
    }
}

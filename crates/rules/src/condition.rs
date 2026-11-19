//! 剧情条件 DSL：JSON AST，由程序求值。
//!
//! 示例：
//! ```json
//! {"type":"all","of":[
//!   {"type":"quest_state","quest":"Q1","state":"done"},
//!   {"type":"any","of":[{"type":"flag","name":"met_king"},{"type":"check","skill":"persuasion","dc":15}]}
//! ]}
//! ```

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::dice::{DiceExpr, DiceSource, RollResult};
use crate::{RulesError, StateView};

pub const MAX_DEPTH: usize = 32;
pub const MAX_NODES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// 字符串包含子串，或数组包含元素。
    Contains,
    /// 变量存在（忽略 value）。
    Exists,
}

fn default_one() -> i64 {
    1
}
fn default_party() -> String {
    "party".into()
}
fn default_d20() -> String {
    "1d20".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Condition {
    Const { value: bool },
    All { of: Vec<Condition> },
    Any { of: Vec<Condition> },
    Not { cond: Box<Condition> },
    Var {
        key: String,
        op: Cmp,
        #[serde(default)]
        value: Value,
    },
    Flag { name: String },
    QuestState { quest: String, state: String },
    HasItem {
        #[serde(default = "default_party")]
        holder: String,
        item: String,
        #[serde(default = "default_one")]
        min_qty: i64,
    },
    ClueKnown { clue: String },
    /// 人物（或其他实体）还活着：战役里没有被记为死亡 / 失踪。剧情事件常用它表达"这个人必须在"。
    Alive { entity: String },
    Relation { a: String, b: String, rel_type: String },
    Visited {
        node: String,
        #[serde(default = "default_one")]
        min_count: i64,
    },
    /// 技能检定：程序掷 `dice` + 技能修正，与 `dc` 比较。
    Check {
        #[serde(default = "default_party")]
        actor: String,
        skill: String,
        dc: i64,
        #[serde(default = "default_d20")]
        dice: String,
    },
}

/// 三值逻辑结果：检定未掷骰时为 Unknown。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Tri {
    True,
    False,
    Unknown,
}

impl From<bool> for Tri {
    fn from(b: bool) -> Self {
        if b { Tri::True } else { Tri::False }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CheckRecord {
    pub actor: String,
    pub skill: String,
    pub dc: i64,
    pub modifier: i64,
    pub roll: RollResult,
    pub total: i64,
    pub success: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct EvalResult {
    pub passed: bool,
    pub checks: Vec<CheckRecord>,
}

impl Condition {
    /// 结构校验：深度、节点数、骰子表达式。
    pub fn validate(&self) -> Result<(), RulesError> {
        let mut count = 0usize;
        self.validate_inner(1, &mut count)
    }

    fn validate_inner(&self, depth: usize, count: &mut usize) -> Result<(), RulesError> {
        *count += 1;
        if depth > MAX_DEPTH {
            return Err(RulesError::Invalid(format!("条件嵌套超过 {MAX_DEPTH} 层")));
        }
        if *count > MAX_NODES {
            return Err(RulesError::Invalid(format!("条件节点超过 {MAX_NODES} 个")));
        }
        match self {
            Condition::All { of } | Condition::Any { of } => {
                for c in of {
                    c.validate_inner(depth + 1, count)?;
                }
            }
            Condition::Not { cond } => cond.validate_inner(depth + 1, count)?,
            Condition::Check { dice, .. } => {
                DiceExpr::parse(dice)?;
            }
            Condition::Var { key, .. } if key.is_empty() => {
                return Err(RulesError::Invalid("变量名不能为空".into()));
            }
            _ => {}
        }
        Ok(())
    }

    /// 是否包含需要掷骰的检定。
    pub fn has_checks(&self) -> bool {
        match self {
            Condition::Check { .. } => true,
            Condition::All { of } | Condition::Any { of } => of.iter().any(|c| c.has_checks()),
            Condition::Not { cond } => cond.has_checks(),
            _ => false,
        }
    }

    /// 实际求值：遇到检定时由程序掷骰，结果记录在 `checks` 中。短路求值。
    pub fn eval(
        &self,
        view: &dyn StateView,
        dice: &mut dyn DiceSource,
    ) -> Result<EvalResult, RulesError> {
        self.validate()?;
        let mut checks = Vec::new();
        let passed = self.eval_inner(view, dice, &mut checks)?;
        Ok(EvalResult { passed, checks })
    }

    fn eval_inner(
        &self,
        view: &dyn StateView,
        dice: &mut dyn DiceSource,
        checks: &mut Vec<CheckRecord>,
    ) -> Result<bool, RulesError> {
        Ok(match self {
            Condition::All { of } => {
                for c in of {
                    if !c.eval_inner(view, dice, checks)? {
                        return Ok(false);
                    }
                }
                true
            }
            Condition::Any { of } => {
                for c in of {
                    if c.eval_inner(view, dice, checks)? {
                        return Ok(true);
                    }
                }
                false
            }
            Condition::Not { cond } => !cond.eval_inner(view, dice, checks)?,
            Condition::Check { actor, skill, dc, dice: expr } => {
                let e = DiceExpr::parse(expr)?;
                let roll = e.roll(dice);
                let modifier = view.skill_mod(actor, skill);
                let total = roll.total + modifier;
                let success = total >= *dc;
                checks.push(CheckRecord {
                    actor: actor.clone(),
                    skill: skill.clone(),
                    dc: *dc,
                    modifier,
                    roll,
                    total,
                    success,
                });
                success
            }
            other => other.eval_static_leaf(view).expect("非检定叶子节点"),
        })
    }

    /// 模拟用：不掷骰，所有检定按 `checks_pass` 视为成功或失败。
    pub fn eval_forced(&self, view: &dyn StateView, checks_pass: bool) -> Result<bool, RulesError> {
        self.validate()?;
        Ok(self.forced_inner(view, checks_pass))
    }

    fn forced_inner(&self, view: &dyn StateView, pass: bool) -> bool {
        match self {
            Condition::All { of } => of.iter().all(|c| c.forced_inner(view, pass)),
            Condition::Any { of } => of.iter().any(|c| c.forced_inner(view, pass)),
            Condition::Not { cond } => !cond.forced_inner(view, pass),
            Condition::Check { .. } => pass,
            other => other.eval_static_leaf(view).expect("非检定叶子节点"),
        }
    }

    /// 预演求值（不掷骰）：检定返回 Unknown，使用 Kleene 三值逻辑。
    pub fn preview(&self, view: &dyn StateView) -> Result<Tri, RulesError> {
        self.validate()?;
        Ok(self.preview_inner(view))
    }

    fn preview_inner(&self, view: &dyn StateView) -> Tri {
        match self {
            Condition::All { of } => {
                let mut unknown = false;
                for c in of {
                    match c.preview_inner(view) {
                        Tri::False => return Tri::False,
                        Tri::Unknown => unknown = true,
                        Tri::True => {}
                    }
                }
                if unknown { Tri::Unknown } else { Tri::True }
            }
            Condition::Any { of } => {
                let mut unknown = false;
                for c in of {
                    match c.preview_inner(view) {
                        Tri::True => return Tri::True,
                        Tri::Unknown => unknown = true,
                        Tri::False => {}
                    }
                }
                if unknown { Tri::Unknown } else { Tri::False }
            }
            Condition::Not { cond } => match cond.preview_inner(view) {
                Tri::True => Tri::False,
                Tri::False => Tri::True,
                Tri::Unknown => Tri::Unknown,
            },
            Condition::Check { .. } => Tri::Unknown,
            other => other.eval_static_leaf(view).expect("非检定叶子节点").into(),
        }
    }

    /// 不需要随机数的叶子节点求值；组合节点与检定返回 None。
    fn eval_static_leaf(&self, view: &dyn StateView) -> Option<bool> {
        Some(match self {
            Condition::Const { value } => *value,
            Condition::Var { key, op, value } => compare(view.var(key).as_ref(), *op, value),
            Condition::Flag { name } => truthy(view.var(&flag_key(name)).as_ref()),
            Condition::QuestState { quest, state } => {
                view.quest_state(quest).as_deref() == Some(state.as_str())
            }
            Condition::HasItem { holder, item, min_qty } => view.item_qty(holder, item) >= *min_qty,
            Condition::ClueKnown { clue } => view.clue_known(clue),
            Condition::Alive { entity } => !matches!(view.life_status(entity).as_deref(), Some("dead" | "missing" | "destroyed")),
            Condition::Relation { a, b, rel_type } => view.relation(a, b, rel_type),
            Condition::Visited { node, min_count } => view.visit_count(node) >= *min_count,
            Condition::All { .. } | Condition::Any { .. } | Condition::Not { .. } | Condition::Check { .. } => {
                return None;
            }
        })
    }

    /// 收集条件中引用的实体（类别, ID）：任务、物品、线索、剧情节点、关系两端，供引用校验使用。
    pub fn referenced_ids(&self, out: &mut Vec<(&'static str, String)>) {
        match self {
            Condition::All { of } | Condition::Any { of } => of.iter().for_each(|c| c.referenced_ids(out)),
            Condition::Not { cond } => cond.referenced_ids(out),
            Condition::QuestState { quest, .. } => out.push(("quest", quest.clone())),
            Condition::HasItem { item, .. } => out.push(("item", item.clone())),
            Condition::ClueKnown { clue } => out.push(("clue", clue.clone())),
            Condition::Alive { entity } => out.push(("entity", entity.clone())),
            Condition::Visited { node, .. } => out.push(("plot_node", node.clone())),
            Condition::Relation { a, b, .. } => {
                out.push(("entity", a.clone()));
                out.push(("entity", b.clone()));
            }
            _ => {}
        }
    }
}

/// 标志位以变量 `flag.<name>` 存储。
pub fn flag_key(name: &str) -> String {
    format!("flag.{name}")
}

pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn compare(actual: Option<&Value>, op: Cmp, expected: &Value) -> bool {
    if op == Cmp::Exists {
        return actual.is_some_and(|v| !v.is_null());
    }
    let Some(actual) = actual else {
        // 变量不存在：只有 Ne 成立（与任何值都不相等）。
        return op == Cmp::Ne;
    };
    match op {
        Cmp::Eq => values_eq(actual, expected),
        Cmp::Ne => !values_eq(actual, expected),
        Cmp::Lt | Cmp::Le | Cmp::Gt | Cmp::Ge => {
            let ord = match (actual, expected) {
                (Value::Number(a), Value::Number(b)) => {
                    a.as_f64().zip(b.as_f64()).and_then(|(a, b)| a.partial_cmp(&b))
                }
                (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
                _ => None,
            };
            match ord {
                None => false,
                Some(o) => match op {
                    Cmp::Lt => o.is_lt(),
                    Cmp::Le => o.is_le(),
                    Cmp::Gt => o.is_gt(),
                    Cmp::Ge => o.is_ge(),
                    _ => unreachable!(),
                },
            }
        }
        Cmp::Contains => match (actual, expected) {
            (Value::String(a), Value::String(b)) => a.contains(b.as_str()),
            (Value::Array(a), e) => a.iter().any(|x| values_eq(x, e)),
            _ => false,
        },
        Cmp::Exists => unreachable!(),
    }
}

/// 数字按数值比较（1 == 1.0），其余按 JSON 相等。
fn values_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dice::FixedDice;
    use crate::testing::MapState;
    use serde_json::json;

    fn c(v: Value) -> Condition {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn leaf_conditions() {
        let mut s = MapState::default();
        s.vars.insert("gold".into(), json!(12));
        s.vars.insert("flag.met_king".into(), json!(true));
        s.quests.insert("Q1".into(), "done".into());
        s.items.insert(("party".into(), "key".into()), 1);
        s.clues.insert("C1".into());
        s.visits.insert("P1".into(), 2);
        s.relations.insert(("A".into(), "B".into(), "ally".into()));

        let ok = |v| c(v).preview(&s).unwrap();
        assert_eq!(ok(json!({"type":"var","key":"gold","op":"ge","value":10})), Tri::True);
        assert_eq!(ok(json!({"type":"var","key":"gold","op":"lt","value":10})), Tri::False);
        assert_eq!(ok(json!({"type":"var","key":"gold","op":"eq","value":12.0})), Tri::True);
        assert_eq!(ok(json!({"type":"var","key":"none","op":"ne","value":1})), Tri::True);
        assert_eq!(ok(json!({"type":"var","key":"none","op":"exists"})), Tri::False);
        assert_eq!(ok(json!({"type":"flag","name":"met_king"})), Tri::True);
        assert_eq!(ok(json!({"type":"flag","name":"other"})), Tri::False);
        assert_eq!(ok(json!({"type":"quest_state","quest":"Q1","state":"done"})), Tri::True);
        assert_eq!(ok(json!({"type":"has_item","item":"key"})), Tri::True);
        assert_eq!(ok(json!({"type":"has_item","item":"key","min_qty":2})), Tri::False);
        assert_eq!(ok(json!({"type":"clue_known","clue":"C1"})), Tri::True);
        assert_eq!(ok(json!({"type":"visited","node":"P1","min_count":2})), Tri::True);
        assert_eq!(ok(json!({"type":"relation","a":"A","b":"B","rel_type":"ally"})), Tri::True);
    }

    #[test]
    fn checks_roll_via_program() {
        let mut s = MapState::default();
        s.vars.insert("skill.party.persuasion".into(), json!(3));
        let cond = c(json!({"type":"check","skill":"persuasion","dc":15}));
        assert_eq!(cond.preview(&s).unwrap(), Tri::Unknown);
        let r = cond.eval(&s, &mut FixedDice::new(vec![12])).unwrap();
        assert!(r.passed);
        assert_eq!(r.checks[0].total, 15);
        let r = cond.eval(&s, &mut FixedDice::new(vec![11])).unwrap();
        assert!(!r.passed);
    }

    #[test]
    fn kleene_logic_in_preview() {
        let s = MapState::default();
        let unknown = json!({"type":"check","skill":"x","dc":10});
        let f = json!({"type":"const","value":false});
        let t = json!({"type":"const","value":true});
        assert_eq!(c(json!({"type":"all","of":[unknown, f]})).preview(&s).unwrap(), Tri::False);
        assert_eq!(c(json!({"type":"any","of":[unknown, t]})).preview(&s).unwrap(), Tri::True);
        assert_eq!(c(json!({"type":"all","of":[unknown, t]})).preview(&s).unwrap(), Tri::Unknown);
    }

    #[test]
    fn forced_checks_for_simulation() {
        let s = MapState::default();
        let cond = c(json!({"type":"all","of":[{"type":"check","skill":"x","dc":30},{"type":"const","value":true}]}));
        assert!(cond.eval_forced(&s, true).unwrap());
        assert!(!cond.eval_forced(&s, false).unwrap());
        let mut refs = vec![];
        c(json!({"type":"any","of":[{"type":"quest_state","quest":"Q","state":"done"},{"type":"visited","node":"P"}]})).referenced_ids(&mut refs);
        assert_eq!(refs, vec![("quest", "Q".to_string()), ("plot_node", "P".to_string())]);
    }

    #[test]
    fn rejects_too_deep() {
        let mut v = json!({"type":"const","value":true});
        for _ in 0..40 {
            v = json!({"type":"not","cond":v});
        }
        assert!(c(v).validate().is_err());
    }

    #[test]
    fn rejects_unknown_type() {
        let r: Result<Condition, _> = serde_json::from_value(json!({"type":"exec","cmd":"rm"}));
        assert!(r.is_err());
    }
}

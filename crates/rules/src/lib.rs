//! Chronicle GM 规则层：骰子、剧情条件 DSL、效果解析。
//!
//! 本 crate 不依赖数据库，也不调用 AI：所有判定都是确定性的程序逻辑，
//! 通过 [`StateView`] 读取状态，通过 [`DiceSource`](dice::DiceSource) 获取随机数。

pub mod condition;
pub mod dice;
pub mod effect;

pub use condition::{CheckRecord, Cmp, Condition, EvalResult, Tri};
pub use dice::{DiceExpr, DiceSource, RollResult, SeededDice};
pub use effect::{Effect, StateChange, resolve_effects};

use serde_json::Value;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum RulesError {
    #[error("骰子表达式错误: {0}")]
    Dice(String),
    #[error("规则数据无效: {0}")]
    Invalid(String),
    #[error("{holder} 的物品 {item} 不足：拥有 {have}，需要 {need}")]
    InsufficientItem { holder: String, item: String, have: i64, need: i64 },
}

/// 规则求值所需的只读状态接口，由存储层（战役状态）实现。
pub trait StateView {
    fn var(&self, key: &str) -> Option<Value>;
    fn quest_state(&self, quest: &str) -> Option<String>;
    fn item_qty(&self, holder: &str, item: &str) -> i64;
    fn clue_known(&self, clue: &str) -> bool;
    fn relation(&self, a: &str, b: &str, rel_type: &str) -> bool;
    fn visit_count(&self, node: &str) -> i64;
    /// 人物关系：`from` 对 `to` 的关系类型与数值；没有记录时为 None。
    fn bond(&self, _from: &str, _to: &str) -> Option<(String, effect::Bond)> {
        None
    }
    /// NPC 情绪；没有记录时为 None。
    fn mood(&self, _npc: &str) -> Option<effect::Mood> {
        None
    }
    /// 实体的生存状态（alive / dead / missing / destroyed）；没有记录时为 None（视为存活）。
    fn life_status(&self, _entity: &str) -> Option<String> {
        None
    }
    /// 战役记录（调查板、动态任务等）。
    fn record(&self, _kind: &str, _id: &str) -> Option<Value> {
        None
    }
    /// 技能修正，默认读取变量 `skill.<actor>.<skill>`。
    fn skill_mod(&self, actor: &str, skill: &str) -> i64 {
        match self.var(&format!("skill.{actor}.{skill}")) {
            Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
            _ => 0,
        }
    }
}

/// 内存状态实现，用于测试与离线预演。
pub mod testing {
    use std::collections::{HashMap, HashSet};

    use serde_json::Value;

    use crate::StateView;

    #[derive(Debug, Default, Clone)]
    pub struct MapState {
        pub vars: HashMap<String, Value>,
        pub quests: HashMap<String, String>,
        pub items: HashMap<(String, String), i64>,
        pub clues: HashSet<String>,
        pub relations: HashSet<(String, String, String)>,
        pub visits: HashMap<String, i64>,
    }

    impl StateView for MapState {
        fn var(&self, key: &str) -> Option<Value> {
            self.vars.get(key).cloned()
        }
        fn quest_state(&self, quest: &str) -> Option<String> {
            self.quests.get(quest).cloned()
        }
        fn item_qty(&self, holder: &str, item: &str) -> i64 {
            self.items.get(&(holder.to_string(), item.to_string())).copied().unwrap_or(0)
        }
        fn clue_known(&self, clue: &str) -> bool {
            self.clues.contains(clue)
        }
        fn relation(&self, a: &str, b: &str, rel_type: &str) -> bool {
            self.relations.contains(&(a.to_string(), b.to_string(), rel_type.to_string()))
        }
        fn visit_count(&self, node: &str) -> i64 {
            self.visits.get(node).copied().unwrap_or(0)
        }
    }
}

//! 属性测试：条件 DSL 与骰子。

use chronicle_rules::dice::{DiceExpr, FixedDice, SeededDice};
use chronicle_rules::testing::MapState;
use chronicle_rules::{Condition, Tri};
use proptest::prelude::*;
use serde_json::json;

fn leaf() -> impl Strategy<Value = Condition> {
    prop_oneof![
        any::<bool>().prop_map(|value| Condition::Const { value }),
        "[ab]".prop_map(|name| Condition::Flag { name }),
        (0i64..20).prop_map(|v| Condition::Var {
            key: "gold".into(),
            op: chronicle_rules::Cmp::Ge,
            value: json!(v)
        }),
        "[QR]".prop_map(|quest| Condition::QuestState { quest, state: "done".into() }),
    ]
}

/// 不含检定的条件树。
fn cond_tree() -> impl Strategy<Value = Condition> {
    leaf().prop_recursive(5, 64, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(|of| Condition::All { of }),
            prop::collection::vec(inner.clone(), 0..4).prop_map(|of| Condition::Any { of }),
            inner.prop_map(|c| Condition::Not { cond: Box::new(c) }),
        ]
    })
}

fn state() -> impl Strategy<Value = MapState> {
    (any::<bool>(), any::<bool>(), 0i64..20, any::<bool>()).prop_map(|(a, b, gold, q)| {
        let mut s = MapState::default();
        s.vars.insert("flag.a".into(), json!(a));
        s.vars.insert("flag.b".into(), json!(b));
        s.vars.insert("gold".into(), json!(gold));
        if q {
            s.quests.insert("Q".into(), "done".into());
        }
        s
    })
}

proptest! {
    #[test]
    fn preview_matches_eval_without_checks(c in cond_tree(), s in state()) {
        let eval = c.eval(&s, &mut FixedDice::new(vec![])).unwrap();
        prop_assert!(eval.checks.is_empty());
        prop_assert_eq!(c.preview(&s).unwrap(), Tri::from(eval.passed));
    }

    #[test]
    fn double_negation(c in cond_tree(), s in state()) {
        let nn = Condition::Not { cond: Box::new(Condition::Not { cond: Box::new(c.clone()) }) };
        prop_assert_eq!(nn.preview(&s).unwrap(), c.preview(&s).unwrap());
    }

    #[test]
    fn json_roundtrip(c in cond_tree()) {
        let text = serde_json::to_string(&c).unwrap();
        let back: Condition = serde_json::from_str(&text).unwrap();
        prop_assert_eq!(back, c);
    }

    #[test]
    fn roll_within_bounds(count in 1u32..20, sides in 1u32..100, k in 0u32..20, konst in -50i64..50, seed in any::<u64>()) {
        let keep = if k > 0 && k <= count { format!("kh{k}") } else { String::new() };
        let expr = format!("{count}d{sides}{keep}{konst:+}");
        let e = DiceExpr::parse(&expr).unwrap();
        let (lo, hi) = e.bounds();
        let r = e.roll(&mut SeededDice::new(seed, 0));
        prop_assert!(r.total >= lo && r.total <= hi, "{} -> {} not in [{}, {}]", expr, r.total, lo, hi);
        for g in &r.groups {
            for v in &g.rolls {
                prop_assert!(*v >= 1 && *v <= sides);
            }
        }
    }
}

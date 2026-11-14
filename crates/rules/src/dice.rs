//! 骰子表达式解析与掷骰。
//!
//! 语法：`term (('+'|'-') term)*`，term 为整数常量或 `[N]d(M|%)[kh K|kl K]`。
//! 例：`1d20+5`、`2d20kh1`（优势）、`2d20kl1`（劣势）、`d%`、`4d6kh3`、`-2+1d4`。
//! 结果由程序计算；随机源可注入，便于复现与测试。

use rand_chacha::ChaCha8Rng;
use rand_core::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};

use crate::RulesError;

pub const MAX_DICE_PER_TERM: u32 = 1000;
pub const MAX_SIDES: u32 = 10_000;
pub const MAX_TERMS: usize = 50;

/// 随机源：返回 1..=sides 的均匀整数。
pub trait DiceSource {
    fn roll_die(&mut self, sides: u32) -> u32;
}

/// 基于 ChaCha8 的可复现随机源：同一 (seed, stream) 产生同一序列。
pub struct SeededDice {
    rng: ChaCha8Rng,
}

impl SeededDice {
    pub fn new(seed: u64, stream: u64) -> Self {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        rng.set_stream(stream);
        Self { rng }
    }
}

impl DiceSource for SeededDice {
    fn roll_die(&mut self, sides: u32) -> u32 {
        uniform(&mut self.rng, sides)
    }
}

/// 拒绝采样，避免取模偏差。
fn uniform<R: Rng>(rng: &mut R, sides: u32) -> u32 {
    debug_assert!(sides >= 1);
    let n = sides as u64;
    let zone = u64::MAX - (u64::MAX % n);
    loop {
        let v = rng.next_u64();
        if v < zone {
            return (v % n) as u32 + 1;
        }
    }
}

/// 固定序列随机源（测试用）。
pub struct FixedDice {
    values: Vec<u32>,
    pos: usize,
}

impl FixedDice {
    pub fn new(values: Vec<u32>) -> Self {
        Self { values, pos: 0 }
    }
}

impl DiceSource for FixedDice {
    fn roll_die(&mut self, sides: u32) -> u32 {
        let v = self.values.get(self.pos).copied().unwrap_or(1);
        self.pos += 1;
        v.clamp(1, sides)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Keep {
    Highest(u32),
    Lowest(u32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Term {
    Const { value: i64 },
    Dice { count: u32, sides: u32, keep: Option<Keep> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SignedTerm {
    pub negative: bool,
    pub term: Term,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct DiceExpr {
    pub source: String,
    pub terms: Vec<SignedTerm>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct DieGroup {
    pub count: u32,
    pub sides: u32,
    pub negative: bool,
    /// 全部掷出的点数。
    pub rolls: Vec<u32>,
    /// 被保留（计入总和）的下标。
    pub kept: Vec<usize>,
    pub subtotal: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct RollResult {
    pub expr: String,
    pub total: i64,
    pub groups: Vec<DieGroup>,
    pub constant: i64,
}

impl DiceExpr {
    pub fn parse(input: &str) -> Result<Self, RulesError> {
        let s: Vec<char> = input
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        if s.is_empty() {
            return Err(RulesError::Dice("空表达式".into()));
        }
        let mut p = Parser { s: &s, i: 0 };
        let mut terms = Vec::new();
        let mut negative = false;
        if p.peek() == Some('-') {
            negative = true;
            p.i += 1;
        } else if p.peek() == Some('+') {
            p.i += 1;
        }
        loop {
            let term = p.term()?;
            terms.push(SignedTerm { negative, term });
            if terms.len() > MAX_TERMS {
                return Err(RulesError::Dice(format!("项数超过 {MAX_TERMS}")));
            }
            match p.peek() {
                None => break,
                Some('+') => negative = false,
                Some('-') => negative = true,
                Some(c) => return Err(RulesError::Dice(format!("无法识别的字符 '{c}'"))),
            }
            p.i += 1;
        }
        Ok(Self { source: input.trim().to_string(), terms })
    }

    /// 理论最小值与最大值。
    pub fn bounds(&self) -> (i64, i64) {
        let (mut lo, mut hi) = (0i64, 0i64);
        for t in &self.terms {
            let (a, b) = match &t.term {
                Term::Const { value } => (*value, *value),
                Term::Dice { count, sides, keep } => {
                    let n = match keep {
                        Some(Keep::Highest(k)) | Some(Keep::Lowest(k)) => (*k).min(*count),
                        None => *count,
                    } as i64;
                    (n, n * *sides as i64)
                }
            };
            if t.negative {
                lo -= b;
                hi -= a;
            } else {
                lo += a;
                hi += b;
            }
        }
        (lo, hi)
    }

    pub fn roll(&self, src: &mut dyn DiceSource) -> RollResult {
        let mut total = 0i64;
        let mut constant = 0i64;
        let mut groups = Vec::new();
        for t in &self.terms {
            let sign = if t.negative { -1 } else { 1 };
            match &t.term {
                Term::Const { value } => {
                    constant += sign * value;
                    total += sign * value;
                }
                Term::Dice { count, sides, keep } => {
                    let rolls: Vec<u32> = (0..*count).map(|_| src.roll_die(*sides)).collect();
                    let mut idx: Vec<usize> = (0..rolls.len()).collect();
                    let kept: Vec<usize> = match keep {
                        None => idx,
                        Some(Keep::Highest(k)) => {
                            idx.sort_by(|a, b| rolls[*b].cmp(&rolls[*a]).then(a.cmp(b)));
                            let mut v: Vec<usize> = idx.into_iter().take(*k as usize).collect();
                            v.sort();
                            v
                        }
                        Some(Keep::Lowest(k)) => {
                            idx.sort_by(|a, b| rolls[*a].cmp(&rolls[*b]).then(a.cmp(b)));
                            let mut v: Vec<usize> = idx.into_iter().take(*k as usize).collect();
                            v.sort();
                            v
                        }
                    };
                    let subtotal: i64 = kept.iter().map(|i| rolls[*i] as i64).sum::<i64>() * sign;
                    total += subtotal;
                    groups.push(DieGroup {
                        count: *count,
                        sides: *sides,
                        negative: t.negative,
                        rolls,
                        kept,
                        subtotal,
                    });
                }
            }
        }
        RollResult { expr: self.source.clone(), total, groups, constant }
    }
}

struct Parser<'a> {
    s: &'a [char],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn number(&mut self) -> Result<Option<u64>, RulesError> {
        let start = self.i;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.i += 1;
        }
        if start == self.i {
            return Ok(None);
        }
        let text: String = self.s[start..self.i].iter().collect();
        text.parse::<u64>()
            .map(Some)
            .map_err(|_| RulesError::Dice(format!("数字过大: {text}")))
    }

    fn term(&mut self) -> Result<Term, RulesError> {
        let n = self.number()?;
        if self.peek() != Some('d') {
            return match n {
                Some(v) if v <= i64::MAX as u64 / 2 => Ok(Term::Const { value: v as i64 }),
                Some(_) => Err(RulesError::Dice("常量过大".into())),
                None => Err(RulesError::Dice(format!("位置 {} 需要数字或骰子", self.i))),
            };
        }
        self.i += 1; // 'd'
        let count = n.unwrap_or(1);
        let sides = if self.peek() == Some('%') {
            self.i += 1;
            100
        } else {
            self.number()?.ok_or_else(|| RulesError::Dice("骰子缺少面数".into()))?
        };
        if count == 0 || count > MAX_DICE_PER_TERM as u64 {
            return Err(RulesError::Dice(format!("骰子个数需在 1..={MAX_DICE_PER_TERM}")));
        }
        if sides == 0 || sides > MAX_SIDES as u64 {
            return Err(RulesError::Dice(format!("骰子面数需在 1..={MAX_SIDES}")));
        }
        let keep = if self.peek() == Some('k') {
            self.i += 1;
            let kind = self.peek();
            self.i += 1;
            let k = self.number()?.unwrap_or(1);
            if k == 0 || k > count {
                return Err(RulesError::Dice("保留个数需在 1..=骰子个数".into()));
            }
            match kind {
                Some('h') => Some(Keep::Highest(k as u32)),
                Some('l') => Some(Keep::Lowest(k as u32)),
                _ => return Err(RulesError::Dice("k 之后需要 h 或 l".into())),
            }
        } else {
            None
        };
        Ok(Term::Dice { count: count as u32, sides: sides as u32, keep })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_forms() {
        let e = DiceExpr::parse("1d20 + 5").unwrap();
        assert_eq!(e.bounds(), (6, 25));
        assert_eq!(DiceExpr::parse("d%").unwrap().bounds(), (1, 100));
        assert_eq!(DiceExpr::parse("2d20kh1").unwrap().bounds(), (1, 20));
        assert_eq!(DiceExpr::parse("4d6kh3").unwrap().bounds(), (3, 18));
        assert_eq!(DiceExpr::parse("-2+1d4").unwrap().bounds(), (-1, 2));
        assert_eq!(DiceExpr::parse("1d6-1d4").unwrap().bounds(), (-3, 5));
    }

    #[test]
    fn rejects_bad_input() {
        for bad in ["", "d", "1d0", "0d6", "1d6+", "2d20kh3", "1d6x", "99999d6", "1d6kq1"] {
            assert!(DiceExpr::parse(bad).is_err(), "应拒绝 {bad:?}");
        }
    }

    #[test]
    fn keep_highest_and_lowest() {
        let e = DiceExpr::parse("2d20kh1").unwrap();
        let r = e.roll(&mut FixedDice::new(vec![3, 17]));
        assert_eq!(r.total, 17);
        assert_eq!(r.groups[0].kept, vec![1]);
        let e = DiceExpr::parse("2d20kl1").unwrap();
        assert_eq!(e.roll(&mut FixedDice::new(vec![3, 17])).total, 3);
        let e = DiceExpr::parse("4d6kh3+2").unwrap();
        assert_eq!(e.roll(&mut FixedDice::new(vec![1, 6, 4, 5])).total, 17);
    }

    #[test]
    fn seeded_is_reproducible() {
        let e = DiceExpr::parse("10d20").unwrap();
        let a = e.roll(&mut SeededDice::new(42, 7));
        let b = e.roll(&mut SeededDice::new(42, 7));
        let c = e.roll(&mut SeededDice::new(42, 8));
        assert_eq!(a, b);
        assert_ne!(a.groups[0].rolls, c.groups[0].rolls);
    }
}

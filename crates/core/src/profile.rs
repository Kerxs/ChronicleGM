//! 剧本「玩法配置」：玩法比重、属性方案、检定规则、叙事风格。
//!
//! 由 AI 随剧本大纲一起生成（可以修改）；旧剧本没有时按题材关键词推断一份。
//! 用途：
//! - 生成：调查比重决定每章写多少线索（以前固定"每个场景 1–2 条"，所以所有剧本都像探案）；
//! - GM / 导演：按比重推进，不再把所有场景都引向调查；
//! - 战役：属性方案与检定规则在开新战役时复制进战役规则（`rpg::RuleSet`），之后可单独修改；
//! - 界面：调查比重为 0 时不显示「调查」页签。

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// 各类玩法的比重，0（没有）～3（核心玩法）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[ts(export)]
pub struct Weights {
    #[serde(default = "two")]
    pub combat: u8,
    #[serde(default = "two")]
    pub exploration: u8,
    #[serde(default = "two")]
    pub social: u8,
    #[serde(default = "one")]
    pub investigation: u8,
    #[serde(default = "one")]
    pub survival: u8,
    #[serde(default)]
    pub management: u8,
}

fn one() -> u8 {
    1
}
fn two() -> u8 {
    2
}

impl Default for Weights {
    fn default() -> Self {
        Self { combat: 2, exploration: 2, social: 2, investigation: 1, survival: 1, management: 0 }
    }
}

impl Weights {
    pub fn clamp(mut self) -> Self {
        for v in [&mut self.combat, &mut self.exploration, &mut self.social, &mut self.investigation, &mut self.survival, &mut self.management] {
            *v = (*v).min(3);
        }
        self
    }
    /// 给提示词看的一句话，例如"战斗 2、探索 3、社交 2、调查 0、生存 1、经营 0"。
    pub fn describe(&self) -> String {
        format!(
            "战斗 {}、探索 {}、社交 {}、调查 {}、生存 {}、经营 {}（0 = 没有，3 = 核心玩法）",
            self.combat, self.exploration, self.social, self.investigation, self.survival, self.management
        )
    }
}

/// 一个属性：程序用 key 计算，界面和提示词显示 name。`role` 让战斗等规则找到对应的属性
/// （might 力量 / agility 敏捷 / vitality 体质 / mind 智力 / insight 感知 / presence 魅力）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[ts(export)]
pub struct AttrDef {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub desc: String,
    #[serde(default)]
    pub role: String,
}

pub const ROLES: [&str; 6] = ["might", "agility", "vitality", "mind", "insight", "presence"];

/// 原来固定的六项属性（旧战役、没有属性方案时使用）。
pub fn default_attrs() -> Vec<AttrDef> {
    [("str", "力量", "might"), ("dex", "敏捷", "agility"), ("con", "体质", "vitality"), ("int", "智力", "mind"), ("wis", "感知", "insight"), ("cha", "魅力", "presence")]
        .iter()
        .map(|(k, n, r)| AttrDef { key: k.to_string(), name: n.to_string(), desc: String::new(), role: r.to_string() })
        .collect()
}

/// 检定规则。所有随机数与计算都由程序完成。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[ts(export)]
pub struct CheckRules {
    /// `target`：掷骰 + 修正 ≥ 难度为成功（d20 类）；
    /// `roll_under`：掷骰 ≤ 目标值为成功（d100 类，目标值 = 难度 + 修正）；
    /// `pool`：掷若干个骰子，统计达到 `pool_success_on` 的个数，个数 ≥ 难度为成功。
    #[serde(default = "target_mode")]
    pub mode: String,
    /// 主骰：target / roll_under 模式下的骰子，例如 "1d20"、"2d6"、"1d100"。
    #[serde(default = "d20")]
    pub die: String,
    /// 骰池：基础骰子数（再加上修正值个骰子）、面数、几点算成功。
    #[serde(default = "pool_base")]
    pub pool_base: i64,
    #[serde(default = "pool_sides")]
    pub pool_sides: i64,
    #[serde(default = "pool_success")]
    pub pool_success_on: i64,
    /// 差几点以内算"部分成功"（成功但有代价）。0 = 不用部分成功。
    #[serde(default = "partial_band")]
    pub partial_band: i64,
    /// 是否使用大成功 / 大失败（target：骰面最大 / 最小值；roll_under：≤ 5% / ≥ 96%；pool：全部成功 / 一个都没有且出现 1）。
    #[serde(default = "yes")]
    pub crits: bool,
    /// 是否使用优势 / 劣势（多掷一次取高 / 取低；骰池模式为多 / 少一个骰子）。
    #[serde(default = "yes")]
    pub advantage: bool,
}

fn target_mode() -> String {
    "target".into()
}
fn d20() -> String {
    "1d20".into()
}
fn pool_base() -> i64 {
    2
}
fn pool_sides() -> i64 {
    6
}
fn pool_success() -> i64 {
    5
}
fn partial_band() -> i64 {
    2
}
fn yes() -> bool {
    true
}

impl Default for CheckRules {
    fn default() -> Self {
        Self { mode: target_mode(), die: d20(), pool_base: 2, pool_sides: 6, pool_success_on: 5, partial_band: 2, crits: true, advantage: true }
    }
}

impl CheckRules {
    pub fn validate(&self) -> Result<()> {
        match self.mode.as_str() {
            "target" | "roll_under" => {
                chronicle_rules::dice::DiceExpr::parse(&self.die)?;
            }
            "pool" => {
                if !(1..=20).contains(&self.pool_base) || !(2..=100).contains(&self.pool_sides) || !(1..=self.pool_sides).contains(&self.pool_success_on) {
                    return Err(CoreError::Invalid("骰池设置不合理（基础骰子 1–20 个，面数 2–100，成功点数不超过面数）".into()));
                }
            }
            m => return Err(CoreError::Invalid(format!("未知的检定方式：{m}（应为 target / roll_under / pool）"))),
        }
        if !(0..=20).contains(&self.partial_band) {
            return Err(CoreError::Invalid("部分成功的范围应为 0–20".into()));
        }
        Ok(())
    }
    /// 给提示词看的说明。
    pub fn describe(&self) -> String {
        let base = match self.mode.as_str() {
            "roll_under" => format!("{} 掷出 ≤ 目标值为成功（目标值 = 难度 + 修正）", self.die),
            "pool" => format!("掷 {}+修正 个 d{}，每个 ≥ {} 算一个成功，成功数 ≥ 难度为成功（难度一般 1–4）", self.pool_base, self.pool_sides, self.pool_success_on),
            _ => format!("{} + 修正 ≥ 难度为成功", self.die),
        };
        let mut s = base;
        if self.partial_band > 0 {
            s.push_str(&format!("；差 {} 以内为部分成功（成功但有代价）", self.partial_band));
        }
        if self.crits {
            s.push_str("；有大成功与大失败");
        }
        s
    }
}

/// 叙事风格：风格、详细程度、描写比例。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[ts(export)]
pub struct Style {
    /// 风格，例如"冷硬悬疑""轻松冒险""史诗奇幻"。
    #[serde(default)]
    pub tone: String,
    /// 描写详细程度 1（简洁）～3（细致）；0 = 默认。
    #[serde(default)]
    pub detail: u8,
    /// 描写比例（各 0–3，0 = 默认）：对话、动作、心理、环境。
    #[serde(default)]
    pub dialogue: u8,
    #[serde(default)]
    pub action: u8,
    #[serde(default)]
    pub inner: u8,
    #[serde(default)]
    pub scenery: u8,
}

impl Style {
    /// 给 GM 的叙事要求（没设置的项不写）。
    pub fn describe(&self) -> String {
        let mut out = vec![];
        if !self.tone.trim().is_empty() {
            out.push(format!("风格：{}", self.tone.trim()));
        }
        match self.detail {
            1 => out.push("描写简洁（2–3 句）".to_string()),
            3 => out.push("描写细致（可以 5–8 句）".to_string()),
            _ => {}
        }
        let lv = |v: u8| ["", "少", "适中", "多"][v.min(3) as usize];
        let ratios: Vec<String> = [("对话", self.dialogue), ("动作", self.action), ("心理", self.inner), ("环境", self.scenery)]
            .iter()
            .filter(|(_, v)| *v > 0)
            .map(|(k, v)| format!("{k}{}", lv(*v)))
            .collect();
        if !ratios.is_empty() {
            out.push(format!("描写比例：{}", ratios.join("、")));
        }
        out.join("；")
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct GameProfile {
    #[serde(default)]
    pub weights: Weights,
    /// 属性方案；空 = 使用原来的六项属性。
    #[serde(default)]
    pub attributes: Vec<AttrDef>,
    /// 技能对应的属性 key（覆盖默认表），例如 {"驾驶": "agi"}。
    #[serde(default)]
    pub skills: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub check: CheckRules,
    #[serde(default)]
    pub style: Style,
    /// 来源：ai（随大纲生成）/ inferred（按题材推断）/ user（手动修改）。
    #[serde(default)]
    pub source: String,
    /// 货币名称（从大到小）与进位；空 = 金币 / 银币 / 铜币。
    #[serde(default)]
    pub currency: Vec<String>,
    #[serde(default)]
    pub currency_rates: Vec<i64>,
    /// 五档稀有度的叫法（普通 → 传说）；空 = 普通 / 优秀 / 稀有 / 史诗 / 传说。
    #[serde(default)]
    pub rarity_labels: Vec<String>,
    /// 按题材给世界树节点补充的资料字段：节点类型 → ["字段名：说明"]（例如武侠的人物写门派、武功）。
    #[serde(default)]
    pub node_fields: std::collections::BTreeMap<String, Vec<String>>,
}

/// 按题材推断世界树节点要多写的资料字段（第 23 项：节点结构随题材变化，不再只有固定的几种）。
pub fn infer_node_fields(text: &str) -> std::collections::BTreeMap<String, Vec<String>> {
    let has = |ws: &[&str]| ws.iter().any(|w| text.contains(w));
    let mut m: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    let mut add = |k: &str, v: &[&str]| m.entry(k.to_string()).or_default().extend(v.iter().map(|x| x.to_string()));
    if has(&["武侠", "仙侠", "江湖", "修仙"]) {
        add("character", &["sect：所属门派", "martial_arts：会的武功（数组）", "realm：武功或修为境界"]);
        add("faction", &["sect_rules：门规", "signature_skill：镇派绝学"]);
        add("item", &["grade：品阶"]);
    }
    if has(&["科幻", "赛博", "太空", "星际", "机甲"]) {
        add("character", &["implants：义体改造（数组）", "employer：所属公司或组织"]);
        add("faction", &["tech_level：科技水平"]);
        add("site", &["security：安保等级"]);
    }
    if has(&["克苏鲁", "恐怖", "惊悚", "神秘学"]) {
        add("character", &["sanity：精神状态", "forbidden_knowledge：接触过的禁忌知识（数组，主持人信息）"]);
        add("site", &["omens：不祥的征兆（数组）"]);
    }
    if has(&["奇幻", "魔法", "精灵", "地下城"]) {
        add("character", &["race：种族", "class：职业"]);
        add("item", &["enchantment：附魔或魔力"]);
    }
    if has(&["废土", "末日", "丧尸", "生存"]) {
        add("site", &["supplies：物资情况", "danger：危险程度"]);
        add("character", &["condition：身体与补给状况"]);
    }
    if has(&["经营", "商会", "领地", "种田"]) {
        add("faction", &["industries：产业（数组）", "treasury：资金状况"]);
        add("site", &["economy：经济状况"]);
    }
    if has(&["悬疑", "推理", "侦探", "凶案"]) {
        add("character", &["alibi：不在场证明", "motive：可能的动机（主持人信息）"]);
    }
    m
}

impl GameProfile {
    /// 某类节点按题材建议多写的字段（给生成提示词用）；没有返回空字符串。
    pub fn node_fields_hint(&self, kind: &str) -> String {
        self.node_fields.get(kind).filter(|v| !v.is_empty()).map(|v| v.join("、")).unwrap_or_default()
    }

    /// 全部类型的字段建议（给世界生成用）。
    pub fn all_node_fields_hint(&self) -> String {
        self.node_fields.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("{k}：{}", v.join("、"))).collect::<Vec<_>>().join("；")
    }
}

/// 世界的题材配置：取最近修改的剧本的玩法配置（世界里没有剧本时为空）。
pub fn world_profile(conn: &rusqlite::Connection) -> Option<GameProfile> {
    let s: Option<crate::model::Script> = conn
        .query_row("SELECT * FROM scripts WHERE deleted_at IS NULL ORDER BY updated_at DESC LIMIT 1", [], <crate::model::Script as crate::entity::Entity>::from_row)
        .ok();
    s.map(|s| for_script(&s))
}

/// 按题材推断货币与稀有度叫法（科幻用信用点、现代用元、武侠用两和文……）。
pub fn infer_economy(text: &str) -> (Vec<String>, Vec<i64>, Vec<String>) {
    let has = |ws: &[&str]| ws.iter().any(|w| text.contains(w));
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    if has(&["科幻", "太空", "赛博", "星际", "未来", "机甲"]) {
        (s(&["信用点"]), vec![1], s(&["标准", "改良", "精工", "军规", "原型"]))
    } else if has(&["现代", "都市", "校园", "当代", "职场"]) {
        (s(&["元"]), vec![1], s(&["普通", "不错", "少见", "珍贵", "绝版"]))
    } else if has(&["武侠", "仙侠", "江湖", "古代", "宫廷", "宫斗", "朝廷"]) {
        (s(&["两银子", "文钱"]), vec![1000, 1], s(&["寻常", "上品", "珍品", "宝物", "神兵"]))
    } else if has(&["废土", "末日", "丧尸"]) {
        (s(&["瓶盖"]), vec![1], s(&["破旧", "能用", "完好", "稀缺", "战前珍品"]))
    } else {
        (vec![], vec![], vec![])
    }
}

impl GameProfile {
    pub fn validate(&self) -> Result<()> {
        self.check.validate()?;
        let mut seen = std::collections::HashSet::new();
        if self.attributes.len() > 12 {
            return Err(CoreError::Invalid("属性最多 12 项".into()));
        }
        for a in &self.attributes {
            let k = a.key.trim();
            if k.is_empty() || a.name.trim().is_empty() {
                return Err(CoreError::Invalid("属性的 key 和名称不能为空".into()));
            }
            if !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(CoreError::Invalid(format!("属性 key「{k}」只能用英文字母、数字和下划线")));
            }
            if !seen.insert(k.to_string()) {
                return Err(CoreError::Invalid(format!("属性 key「{k}」重复")));
            }
            if !a.role.is_empty() && !ROLES.contains(&a.role.as_str()) {
                return Err(CoreError::Invalid(format!("属性「{}」的作用 {} 不认识", a.name, a.role)));
            }
        }
        if !self.currency.is_empty() && self.currency.len() != self.currency_rates.len() {
            return Err(CoreError::Invalid("货币名称和进位的数量要一样".into()));
        }
        if self.currency.len() > 4 || self.currency_rates.iter().any(|r| *r < 1) {
            return Err(CoreError::Invalid("货币最多 4 种，进位至少是 1".into()));
        }
        if !self.rarity_labels.is_empty() && self.rarity_labels.len() != 5 {
            return Err(CoreError::Invalid("稀有度要写满 5 档（从普通到传说）".into()));
        }
        let keys: Vec<&str> = self.attrs_or_default_keys();
        for (skill, attr) in &self.skills {
            if !keys.contains(&attr.as_str()) {
                return Err(CoreError::Invalid(format!("技能「{skill}」对应的属性 {attr} 不在属性方案里")));
            }
        }
        Ok(())
    }

    fn attrs_or_default_keys(&self) -> Vec<&str> {
        if self.attributes.is_empty() { vec!["str", "dex", "con", "int", "wis", "cha"] } else { self.attributes.iter().map(|a| a.key.as_str()).collect() }
    }

    /// 生成时每章要写多少线索（按调查比重）。
    pub fn clues_per_chapter(&self) -> (u32, u32) {
        match self.weights.investigation {
            0 => (0, 1),
            1 => (1, 2),
            2 => (2, 4),
            _ => (4, 8),
        }
    }

    /// 给 GM / 生成提示词看的玩法说明。
    pub fn guidance(&self) -> String {
        let w = &self.weights;
        let mut s = format!("玩法比重：{}。", w.describe());
        if w.investigation <= 1 {
            s.push_str("这不是以探案为主的剧本：不要把场景都引向找线索、查真相，按其他玩法推进（冒险、战斗、交流、探索等）；");
        }
        if w.combat >= 3 {
            s.push_str("战斗是核心玩法，冲突可以较快升级为战斗；");
        } else if w.combat == 0 {
            s.push_str("尽量用非战斗方式解决冲突；");
        }
        if w.social >= 3 {
            s.push_str("人物交流、关系与谈判是核心；");
        }
        if w.exploration >= 3 {
            s.push_str("探索新地点、发现奇观是核心；");
        }
        if w.survival >= 2 {
            s.push_str("资源、伤势、环境压力要有分量；");
        }
        if w.management >= 2 {
            s.push_str("经营、建设与资源管理是重要玩法；");
        }
        let style = self.style.describe();
        if !style.is_empty() {
            s.push_str(&format!("叙事要求：{style}；"));
        }
        s.trim_end_matches('；').to_string() + "。"
    }
}

/// 按题材 / 基调 / 创意里的关键词推断一份玩法配置（旧剧本、AI 没给出时使用）。
pub fn infer(genre: &str, tone: &str, premise: &str) -> GameProfile {
    let text = format!("{genre} {tone} {premise}");
    let has = |ws: &[&str]| ws.iter().any(|w| text.contains(w));
    let mut w = Weights { combat: 2, exploration: 2, social: 2, investigation: 1, survival: 1, management: 0 };
    if has(&["悬疑", "推理", "侦探", "探案", "谋杀", "凶案", "破案", "密室", "调查", "克苏鲁", "侦破"]) {
        w.investigation = 3;
        w.combat = w.combat.min(1);
    }
    if has(&["战争", "武侠", "动作", "战斗", "军", "地下城", "厮杀", "竞技", "格斗"]) {
        w.combat = 3;
    }
    if has(&["生存", "末日", "废土", "荒野", "求生", "丧尸"]) {
        w.survival = 3;
    }
    if has(&["经营", "种田", "建设", "商会", "领地", "开店"]) {
        w.management = 3;
    }
    if has(&["恋爱", "宫廷", "宫斗", "社交", "政治", "日常", "校园"]) {
        w.social = 3;
        w.combat = w.combat.min(1);
    }
    if has(&["冒险", "探索", "航海", "寻宝", "奇幻", "科幻", "太空"]) {
        w.exploration = 3;
    }
    let (currency, currency_rates, rarity_labels) = infer_economy(&text);
    let node_fields = infer_node_fields(&text);
    GameProfile { weights: w, style: Style { tone: tone.trim().to_string(), ..Default::default() }, source: "inferred".into(), currency, currency_rates, rarity_labels, node_fields, ..Default::default() }
}

/// 剧本保存的配置（`scripts.game_profile`），为空或无法解析时按题材推断。
pub fn for_script(s: &crate::model::Script) -> GameProfile {
    match serde_json::from_value::<GameProfile>(s.game_profile.clone()) {
        Ok(mut p) if !p.source.is_empty() => {
            if p.node_fields.is_empty() {
                p.node_fields = infer_node_fields(&format!("{} {} {}", s.genre, s.tone, s.premise));
            }
            p
        }
        _ => infer(&s.genre, &s.tone, &s.premise),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_by_genre() {
        let m = infer("推理悬疑", "冷硬", "雨夜的密室谋杀");
        assert_eq!(m.weights.investigation, 3);
        assert_eq!(m.clues_per_chapter(), (4, 8));
        let a = infer("奇幻冒险", "轻松", "护送商队穿越山脉");
        assert_eq!(a.weights.investigation, 1, "非悬疑题材调查比重低");
        assert_eq!(a.weights.exploration, 3);
        assert_eq!(a.clues_per_chapter(), (1, 2));
        assert!(a.guidance().contains("不是以探案为主"));
        assert!(!m.guidance().contains("不是以探案为主"));
        let w = infer("武侠", "", "江湖恩怨");
        assert_eq!(w.weights.combat, 3);
    }

    #[test]
    fn validate_attributes_and_checks() {
        let mut p = GameProfile { attributes: vec![AttrDef { key: "body".into(), name: "体魄".into(), desc: String::new(), role: "might".into() }], ..Default::default() };
        p.skills.insert("搏斗".into(), "body".into());
        assert!(p.validate().is_ok());
        p.skills.insert("黑客".into(), "tech".into());
        assert!(p.validate().is_err(), "技能指向不存在的属性");
        p.skills.remove("黑客");
        p.attributes.push(AttrDef { key: "body".into(), name: "重复".into(), desc: String::new(), role: String::new() });
        assert!(p.validate().is_err(), "重复 key");
        let mut c = CheckRules { mode: "pool".into(), ..Default::default() };
        assert!(c.validate().is_ok());
        c.pool_success_on = 9;
        assert!(c.validate().is_err());
        assert!(CheckRules { mode: "roll_under".into(), die: "1d100".into(), ..Default::default() }.validate().is_ok());
        assert!(CheckRules { mode: "x".into(), ..Default::default() }.validate().is_err());
        assert!(CheckRules { die: "abc".into(), ..Default::default() }.validate().is_err());
    }

    #[test]
    fn old_json_without_fields_gets_defaults() {
        let p: GameProfile = serde_json::from_str(r#"{"weights":{"investigation":0},"source":"user"}"#).unwrap();
        assert_eq!(p.weights.investigation, 0);
        assert_eq!(p.weights.combat, 2);
        assert_eq!(p.check, CheckRules::default());
    }
}

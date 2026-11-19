//! 把模型给出的玩法配置（`PlayOut`）整理成合法的 `GameProfile`：
//! 属性 key 改成英文、去重，作用不认识的清空，技能只保留指向已有属性的，检定方式与主骰不合法时用默认值。
//! 整理后仍不合法时，退回按题材推断的配置（只保留模型给的玩法比重）。

use chronicle_core::profile::{self, AttrDef, CheckRules, GameProfile, Style, Weights};

use crate::outputs::{PcOutline, PlayOut};

fn ascii_key(raw: &str, role: &str, i: usize) -> String {
    let k: String = raw.trim().to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(16).collect();
    if !k.is_empty() && k.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return k;
    }
    if !role.is_empty() { role.to_string() } else { format!("attr{}", i + 1) }
}

pub fn profile_from_play(p: &PlayOut, genre: &str, tone: &str, premise: &str) -> GameProfile {
    let w = &p.weights;
    let weights = Weights { combat: w.combat, exploration: w.exploration, social: w.social, investigation: w.investigation, survival: w.survival, management: w.management }.clamp();
    let mut attrs: Vec<AttrDef> = vec![];
    for (i, a) in p.attributes.iter().take(8).enumerate() {
        let role = if profile::ROLES.contains(&a.role.trim()) { a.role.trim().to_string() } else { String::new() };
        let mut key = ascii_key(&a.key, &role, i);
        while attrs.iter().any(|x| x.key == key) {
            key.push('2');
        }
        // 一个作用只给第一个属性（其余清空），避免战斗规则找错属性。
        let role = if !role.is_empty() && attrs.iter().any(|x| x.role == role) { String::new() } else { role };
        if a.name.trim().is_empty() {
            continue;
        }
        attrs.push(AttrDef { key, name: a.name.trim().chars().take(12).collect(), desc: a.desc.trim().chars().take(80).collect(), role });
    }
    if attrs.len() < 2 {
        attrs.clear(); // 太少就用原来的六项
    }
    let keys: Vec<String> = if attrs.is_empty() { profile::default_attrs().into_iter().map(|a| a.key).collect() } else { attrs.iter().map(|a| a.key.clone()).collect() };
    let mut skills = std::collections::BTreeMap::new();
    for s in p.skills.iter().take(16) {
        let attr = s.attr.trim().to_ascii_lowercase();
        // 模型常把属性写成中文名：按名称找回 key。
        let attr = if keys.contains(&attr) { Some(attr) } else { attrs.iter().find(|a| a.name == s.attr.trim()).map(|a| a.key.clone()) };
        if let Some(a) = attr
            && !s.skill.trim().is_empty()
        {
            skills.insert(s.skill.trim().to_string(), a);
        }
    }
    let mut check = CheckRules::default();
    match p.check_mode.trim() {
        "roll_under" => {
            check.mode = "roll_under".into();
            check.die = "1d100".into();
        }
        "pool" => {
            check.mode = "pool".into();
            let sides: i64 = p.die.trim().trim_start_matches(|c: char| c.is_ascii_digit()).trim_start_matches(['d', 'D']).parse().unwrap_or(6);
            check.pool_sides = sides.clamp(4, 20);
            check.pool_success_on = (check.pool_sides * 5 / 6).max(2);
        }
        _ => {
            if !p.die.trim().is_empty() && (CheckRules { die: p.die.trim().to_string(), ..Default::default() }).validate().is_ok() {
                check.die = p.die.trim().to_string();
            }
        }
    }
    // 货币与稀有度叫法：模型给的不完整（数量对不上、进位不对）就按题材推断。
    let names: Vec<String> = p.currency.iter().map(|c| c.trim().chars().take(8).collect::<String>()).filter(|c| !c.is_empty()).take(3).collect();
    let rates_ok = names.len() == p.currency_rates.len() && p.currency_rates.last() == Some(&1) && p.currency_rates.windows(2).all(|w| w[0] > w[1]);
    let (inf_c, inf_r, inf_l) = profile::infer_economy(&format!("{genre} {tone} {premise}"));
    let (currency, currency_rates) = if !names.is_empty() && rates_ok { (names, p.currency_rates.clone()) } else { (inf_c, inf_r) };
    let labels: Vec<String> = p.rarity_labels.iter().map(|c| c.trim().chars().take(6).collect::<String>()).collect();
    let rarity_labels = if labels.len() == 5 && labels.iter().all(|l| !l.is_empty()) { labels } else { inf_l };
    let node_fields = profile::infer_node_fields(&format!("{genre} {tone} {premise}"));
    let gp = GameProfile { weights, attributes: attrs, skills, check, style: Style { tone: p.style.trim().chars().take(40).collect(), ..Default::default() }, source: "ai".into(), currency, currency_rates, rarity_labels, node_fields };
    if gp.validate().is_ok() {
        return gp;
    }
    let mut fallback = profile::infer(genre, tone, premise);
    fallback.weights = weights;
    fallback.source = "ai".into();
    fallback
}

/// 可选角色的条目 payload：身份、性格、能力、身世。
pub fn pc_payload(pc: &PcOutline) -> serde_json::Value {
    serde_json::json!({
        "identity": pc.identity,
        "personality": pc.personality,
        "abilities": pc.abilities,
        "backstory": {
            "origin": pc.backstory.origin,
            "upbringing": pc.backstory.upbringing,
            "occupation": pc.backstory.occupation,
            "key_events": pc.backstory.key_events,
            "relations": pc.backstory.relations,
        },
        "equipment": pc.equipment,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outputs::{AttrOut, PlayWeights, SkillOut};

    fn play(attrs: Vec<(&str, &str, &str)>, mode: &str, die: &str) -> PlayOut {
        PlayOut {
            weights: PlayWeights { combat: 5, exploration: 3, social: 2, investigation: 0, survival: 1, management: 0 },
            attributes: attrs.into_iter().map(|(k, n, r)| AttrOut { key: k.into(), name: n.into(), desc: String::new(), role: r.into() }).collect(),
            skills: vec![SkillOut { skill: "驾驶".into(), attr: "反应".into() }, SkillOut { skill: "黑客".into(), attr: "nonexist".into() }],
            check_mode: mode.into(),
            die: die.into(),
            currency: vec![],
            currency_rates: vec![],
            rarity_labels: vec![],
            style: "赛博朋克".into(),
        }
    }

    #[test]
    fn economy_from_model_or_genre() {
        let mut p = play(vec![("a", "体魄", "might"), ("b", "反应", "agility")], "target", "1d20");
        p.currency = vec!["金".into(), "银".into()];
        p.currency_rates = vec![10, 2]; // 最后一个不是 1：不合法
        let gp = profile_from_play(&p, "科幻", "", "");
        assert_eq!(gp.currency, vec!["信用点"], "不合法时按题材推断");
        p.currency_rates = vec![10, 1];
        p.rarity_labels = vec!["寻常".into(), "上品".into(), "珍品".into(), "宝物".into(), "神兵".into()];
        let gp = profile_from_play(&p, "武侠", "", "");
        assert_eq!((gp.currency.clone(), gp.currency_rates.clone()), (vec!["金".to_string(), "银".to_string()], vec![10, 1]));
        assert_eq!(gp.rarity_labels[4], "神兵");
        assert!(gp.validate().is_ok());
    }

    #[test]
    fn sanitizes_model_output() {
        let p = profile_from_play(&play(vec![("体魄", "体魄", "might"), ("reflex", "反应", "agility"), ("Tech", "技术", "mind"), ("x", "", "")], "target", "2d6"), "科幻", "", "");
        assert_eq!(p.weights.combat, 3, "比重截到 0–3");
        assert_eq!(p.attributes.iter().map(|a| a.key.as_str()).collect::<Vec<_>>(), ["might", "reflex", "tech"], "中文 key 改用作用名，大写转小写，空名称丢弃");
        assert_eq!(p.skills.get("驾驶").map(String::as_str), Some("reflex"), "中文属性名找回 key");
        assert!(!p.skills.contains_key("黑客"), "指向不存在属性的技能丢弃");
        assert_eq!(p.check.die, "2d6");
        assert_eq!(p.source, "ai");
        assert!(p.validate().is_ok());
        let pool = profile_from_play(&play(vec![("a", "甲", ""), ("b", "乙", "")], "pool", "d10"), "", "", "");
        assert_eq!((pool.check.mode.as_str(), pool.check.pool_sides), ("pool", 10));
        let bad_die = profile_from_play(&play(vec![("a", "甲", ""), ("b", "乙", "")], "target", "很多骰子"), "", "", "");
        assert_eq!(bad_die.check.die, "1d20");
        let few = profile_from_play(&play(vec![("a", "甲", "")], "target", "1d20"), "", "", "");
        assert!(few.attributes.is_empty(), "少于 2 个属性时用原来的六项");
    }
}

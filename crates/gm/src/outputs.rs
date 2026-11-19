//! 模型输出结构（GM 规划、叙述、NPC 回应、会话摘要）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CheckRequest {
    /// 技能名，例如 察觉、游说、运动。
    pub skill: String,
    /// 难度，通常 8–25。
    pub dc: i64,
    /// 为什么需要这次检定。
    #[serde(default)]
    pub reason: String,
    /// 有利条件写 advantage，不利条件写 disadvantage，普通情况留空。
    #[serde(default)]
    pub advantage: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Discovery {
    pub title: String,
    /// 玩家看到或得知的内容（不要写真相）。
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QuestOut {
    pub title: String,
    #[serde(default)]
    pub objective: String,
    /// main / side / hidden
    #[serde(default)]
    pub quest_type: String,
    /// 期限（回合数），没有就填 0。
    #[serde(default)]
    pub deadline_turns: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QuestNoteOut {
    pub quest: String,
    pub text: String,
}

/// NPC 这次交流后的情绪变化（每项 -10 到 10）。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
pub struct MoodDelta {
    #[serde(default)]
    pub fear: i64,
    #[serde(default)]
    pub anger: i64,
    #[serde(default)]
    pub suspicion: i64,
    #[serde(default)]
    pub stress: i64,
    #[serde(default)]
    pub loyalty: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NewEntity {
    /// character / city / site / item / faction / event
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub summary: String,
    /// 主持人可见的信息。
    #[serde(default)]
    pub gm_text: String,
}

/// 玩家的重要决定会带来的后果（可以延迟发生）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConsequenceOut {
    /// 玩家的决定（一句话）。
    pub decision: String,
    /// 会带来的后果（一句话）。
    pub consequence: String,
    /// 几回合后发生（0 = 马上，最多 30）。
    #[serde(default)]
    pub after_turns: i64,
}

/// 遭遇的敌人（数值由程序生成或取自人物的角色卡）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnemyOut {
    pub name: String,
    /// 对应的在场人物编号（如 N1）；普通敌人留空。
    #[serde(default)]
    pub npc: String,
    /// 等级 1–20（普通人 1，老练士兵 3，怪物头目 5 以上）。
    #[serde(default)]
    pub level: i64,
    #[serde(default)]
    pub count: i64,
    /// 是否首领（首领战无法逃跑）。
    #[serde(default)]
    pub boss: bool,
}

/// 战斗中玩家这一回合的行动（结果由程序计算）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CombatActionOut {
    /// attack（攻击）/ move（移动靠近 target）/ defend（防御）/ dodge（闪避，到下次行动前）/ skill（技能，技能名写在 item）/ use_item（使用物品）/ interact（场景交互，物件名写在 target）/ flee（撤退）/ end_turn（结束回合）
    pub kind: String,
    /// 目标参战者的名字。
    #[serde(default)]
    pub target: String,
    /// 使用的物品编号（如 B1）或自定义行动名称。
    #[serde(default)]
    pub item: String,
}

/// 买卖（价格由程序计算）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TradeOut {
    /// buy（买）/ sell（卖）
    pub kind: String,
    /// 商人编号（如 N1）。
    pub merchant: String,
    /// 商品编号（买填 S1，卖填背包编号 B1）。
    pub item: String,
    #[serde(default)]
    pub qty: i64,
}

/// 奖励（经验、金钱；数量有上限，由程序结算）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RewardOut {
    /// party（玩家）或人物编号（如 N1）。
    pub who: String,
    #[serde(default)]
    pub xp: i64,
    /// 金钱变化（铜币，可以为负，例如付钱）。
    #[serde(default)]
    pub money: i64,
    pub reason: String,
}

/// 补充识别：玩家这句话里的买卖、使用物品、制作、开战（主方案漏填这些字段时才请求）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RpgIntentOut {
    #[serde(default)]
    pub trade: Vec<TradeOut>,
    #[serde(default)]
    pub item_use: Option<String>,
    #[serde(default)]
    pub craft: Option<String>,
    #[serde(default)]
    pub combat_start: Vec<EnemyOut>,
}

/// GM 对玩家行动的处理方案（由程序校验与执行）。
/// 字段顺序即生成顺序：先决定谁说话、要做哪些判定，最后才写叙述（实测模型先写叙述时会替 NPC 说话）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GmPlan {
    /// 用中文一句话概括玩家想做什么（必填）。
    pub intent: String,
    /// 需要亲自回应玩家的在场 NPC 编号（如 N1）；玩家在和某个 NPC 说话时必须填写。
    #[serde(default)]
    pub npc_speakers: Vec<String>,
    /// 玩家做了剧本没有预设的事。
    #[serde(default)]
    pub off_script: bool,
    /// 如果要推进剧情：引用玩家原话说明他明确做出了哪个选择；玩家只是询问、观察或犹豫时留空。
    /// （实测：模型会在玩家只是提问时就推进剧情；先写理由再填编号可以减少误判。）
    #[serde(default)]
    pub transition_reason: String,
    /// 玩家明确选择的可选分支编号（如 E1）；没有明确选择时留空。
    #[serde(default)]
    pub transition_edge: Option<String>,
    #[serde(default)]
    pub checks: Vec<CheckRequest>,
    #[serde(default)]
    pub effects: Vec<Map<String, Value>>,
    #[serde(default)]
    pub effects_on_success: Vec<Map<String, Value>>,
    #[serde(default)]
    pub effects_on_failure: Vec<Map<String, Value>>,
    #[serde(default)]
    pub new_entities: Vec<NewEntity>,
    /// 玩家这一回合获得的、剧本线索之外的新发现（自动记入调查板），最多 3 条。
    #[serde(default)]
    pub discoveries: Vec<Discovery>,
    /// 剧情中自然产生的新任务（很少需要），最多 1 个。
    #[serde(default)]
    pub new_quests: Vec<QuestOut>,
    /// 已有任务的变化（目标调整、新情况），最多 2 条；quest 填任务编号。
    #[serde(default)]
    pub quest_notes: Vec<QuestNoteOut>,
    /// 开始战斗：遭遇的敌人（只有冲突确实爆发时才填，已经在战斗中时留空）。
    #[serde(default)]
    pub combat_start: Vec<EnemyOut>,
    /// 这场战斗无法逃跑的原因（场地封锁、退路被切断等），没有就留空。
    #[serde(default)]
    pub combat_no_escape: String,
    /// 开战时和玩家并肩作战的在场人物编号（如 N2），没有就留空。
    #[serde(default)]
    pub combat_allies: Vec<String>,
    /// 战斗中玩家这一回合的行动；不在战斗中时留空。
    #[serde(default)]
    pub combat_action: Option<CombatActionOut>,
    /// 玩家与商人的买卖，最多 3 笔。
    #[serde(default)]
    pub trade: Vec<TradeOut>,
    /// 战斗外使用或装备的物品编号（如 B1）。
    #[serde(default)]
    pub item_use: Option<String>,
    /// 制作的物品编号（如 R1）。
    #[serde(default)]
    pub craft: Option<String>,
    /// 经验、金钱奖励，最多 3 条。
    #[serde(default)]
    pub rewards: Vec<RewardOut>,
    /// 玩家这一回合出发前往的地点编号（如 L1）；没有出发时留空。耗时由程序按路程计算。
    #[serde(default)]
    pub travel_to: Option<String>,
    /// 玩家这一回合听说的地点编号（NPC 提到、看到告示等），最多 3 个。
    #[serde(default)]
    pub places_heard: Vec<String>,
    /// 这一回合在故事里大约过去了多少分钟（0–720；不确定时写 15）。
    #[serde(default)]
    pub time_passes_minutes: Option<i64>,
    #[serde(default)]
    pub remember: Vec<String>,
    /// 这一回合的决定依据了哪些记忆、设定或剧本内容（填编号，如 M1、W2、C1），没有就留空。
    #[serde(default)]
    pub based_on: Vec<String>,
    /// 玩家做出会影响以后的重要决定时（放走敌人、得罪势力、许下承诺……），记下它的后果，最多 2 条；平常留空。
    #[serde(default)]
    pub consequences: Vec<ConsequenceOut>,
    /// 这一回合已经让它发生的到期后果编号（如 K1）。
    #[serde(default)]
    pub consequences_done: Vec<String>,
    /// 这一回合埋下的伏笔（以后要回收的暗示），最多 1 条；平常留空。
    #[serde(default)]
    pub foreshadow: Vec<String>,
    /// 这一回合回收的伏笔编号（如 F1）。
    #[serde(default)]
    pub payoffs: Vec<String>,
    /// 需要查现实世界的资料时（例如真实的历史、地理、技术细节），写要搜索的内容；不需要就留空。
    #[serde(default)]
    pub web_search: String,
    /// 给玩家看的叙述（不包含 NPC 的台词）。
    pub narration: String,
    /// 给玩家的 2–4 个可选行动：符合此刻的场景、彼此不同、每回合都不一样，每条一句话（玩家也可以自由行动）。
    #[serde(default)]
    pub choices: Vec<String>,
}

const PLAN_KEYS: [&str; 32] = [
    "intent", "npc_speakers", "off_script", "transition_reason", "transition_edge", "checks",
    "effects", "effects_on_success", "effects_on_failure", "new_entities", "discoveries", "new_quests", "quest_notes",
    "combat_start", "combat_no_escape", "combat_allies", "combat_action", "trade", "item_use", "craft", "rewards", "travel_to", "places_heard", "time_passes_minutes", "remember", "based_on", "consequences", "consequences_done", "foreshadow", "payoffs", "web_search", "choices",
];

/// 模型有时把后面的字段写进了字符串末尾（实测：`……压低了声音。","npc_speakers":["N1"]}`）。
/// 找到 `",` + `"字段名":` 这样的尾巴就截掉，返回截断后的文本与能解析出的字段。
pub fn strip_leaked_fields(text: &str, keys: &[&str]) -> Option<(String, serde_json::Map<String, Value>)> {
    let (text_end, fields_start) = keys
        .iter()
        .filter_map(|k| {
            let pat = format!("\"{k}\"");
            text.find(&pat).and_then(|pos| {
                // 要求前面是 `",` 加可选空白，才算 JSON 字段而不是正文里的引号。
                let head = text[..pos].trim_end();
                head.strip_suffix(',').map(|h| h.trim_end()).filter(|h| h.ends_with('"')).map(|h| (h.len() - 1, pos))
            })
        })
        .min_by_key(|(c, _)| *c)?;
    let tail = text[fields_start..].trim_end();
    let candidates = [format!("{{{tail}"), format!("{{{tail}}}"), format!("{{{}}}", tail.trim_end_matches(['}', ',']))];
    let parsed = candidates.iter().find_map(|c| serde_json::from_str(c).ok()).unwrap_or_default();
    Some((text[..text_end].trim_end().to_string(), parsed))
}

impl GmPlan {
    /// 截掉叙述里的字段尾巴，并把能解析出的字段补回空着的字段。返回是否做了修正。
    pub fn salvage_leaked_fields(&mut self) -> bool {
        let Some((narration, m)) = strip_leaked_fields(&self.narration, &PLAN_KEYS) else { return false };
        self.narration = narration;
        if self.npc_speakers.is_empty()
            && let Some(v) = m.get("npc_speakers").and_then(|v| serde_json::from_value(v.clone()).ok())
        {
            self.npc_speakers = v;
        }
        if self.transition_reason.trim().is_empty()
            && let Some(v) = m.get("transition_reason").and_then(|v| v.as_str())
        {
            self.transition_reason = v.to_string();
        }
        if self.transition_edge.as_deref().is_none_or(|e| e.trim().is_empty())
            && let Some(v) = m.get("transition_edge").and_then(|v| v.as_str())
        {
            self.transition_edge = Some(v.to_string());
        }
        true
    }
}

impl NpcReplyOut {
    /// 同上：NPC 台词末尾混进了 attitude_change / remember（实测第 1 回合）。
    pub fn salvage_leaked_fields(&mut self) -> bool {
        let Some((reply, m)) = strip_leaked_fields(&self.reply, &["attitude_change", "mood", "remember"]) else { return false };
        self.reply = reply;
        if self.attitude_change == 0
            && let Some(v) = m.get("attitude_change").and_then(|v| v.as_i64())
        {
            self.attitude_change = v;
        }
        if self.remember.is_empty()
            && let Some(v) = m.get("remember").and_then(|v| serde_json::from_value(v.clone()).ok())
        {
            self.remember = v;
        }
        true
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Narration {
    pub narration: String,
    /// 按实际结果重新给出的 2–4 个可选行动。
    #[serde(default)]
    pub choices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NpcReplyOut {
    pub reply: String,
    #[serde(default)]
    pub attitude_change: i64,
    /// 情绪变化（恐惧、愤怒、怀疑、压力、忠诚）。
    #[serde(default)]
    pub mood: MoodDelta,
    #[serde(default)]
    pub remember: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SummaryOut {
    pub summary: String,
}

#[cfg(test)]
mod tests {
    use super::GmPlan;

    fn plan(narration: &str) -> GmPlan {
        serde_json::from_value(serde_json::json!({"intent": "x", "narration": narration})).unwrap()
    }

    #[test]
    fn salvages_fields_leaked_into_narration() {
        // 第 1 回合真实输出的形状。
        let mut p = plan("他打量了你片刻，然后压低了声音。\",
  \"npc_speakers\": [\"N1\"],
  \"transition_edge\": \"E1\",
  \"transition_reason\": \"玩家只是询问\"");
        assert!(p.salvage_leaked_fields());
        assert_eq!(p.narration, "他打量了你片刻，然后压低了声音。");
        assert_eq!(p.npc_speakers, vec!["N1"]);
        assert_eq!(p.transition_edge.as_deref(), Some("E1"));
        // 第 4 回合真实输出的形状。
        let mut p = plan("动作显得有些僵硬。\",\"npc_speakers\":[\"N2\"]}");
        assert!(p.salvage_leaked_fields());
        assert_eq!(p.narration, "动作显得有些僵硬。");
        assert_eq!(p.npc_speakers, vec!["N2"]);
        // 正常叙述不动。
        // NPC 台词（真实试玩第 1 回合的形状）。
        let mut r: super::NpcReplyOut = serde_json::from_value(serde_json::json!({"reply": "我顿了顿，\"得有人去把封印再加固加固。\", \"attitude_change\": 1, \"remember\": []"})).unwrap();
        assert!(r.salvage_leaked_fields());
        assert_eq!(r.reply, "我顿了顿，\"得有人去把封印再加固加固。");
        assert_eq!(r.attitude_change, 1);
        let mut p = plan("石板上刻着\"潮汐\"，你想起了\"intent\"这个词。");
        assert!(!p.salvage_leaked_fields());
    }

    #[test]
    fn plan_schema_requires_speakers_before_narration() {
        let s = crate::ask::schema_of::<super::GmPlan>();
        let req: Vec<&str> = s["required"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
        for f in ["intent", "npc_speakers", "transition_reason", "narration"] {
            assert!(req.contains(&f), "{f} 应为必填：{req:?}");
        }
        // 发给模型的字段顺序必须与结构体一致（llama.cpp 按 schema 顺序生成）：先决定谁说话，最后写叙述。
        // 曾经因为 serde_json 没开 preserve_order，字段按字母排序，narration 排在 npc_speakers 前面。
        let keys: Vec<&String> = s["properties"].as_object().unwrap().keys().collect();
        assert_eq!(keys.first().map(|k| k.as_str()), Some("intent"));
        // 叙述之后才是可选行动（按叙述的结果给选项）。
        let tail: Vec<&str> = keys.iter().rev().take(2).rev().map(|k| k.as_str()).collect();
        assert_eq!(tail, ["narration", "choices"]);
    }
}


const RPG_EFFECT_TYPES: [&str; 21] = [
    "check", "skill_check", "roll",
    "trade", "buy", "sell", "purchase", "combat_start", "start_combat", "encounter", "combat", "combat_action", "attack", "craft", "item_use", "use_item", "equip", "reward", "rewards", "award_xp", "give_money",
];

fn s_of(m: &Map<String, Value>, keys: &[&str]) -> String {
    keys.iter().find_map(|k| m.get(*k).and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_i64().map(|n| n.to_string())))).unwrap_or_default()
}

fn i_of(m: &Map<String, Value>, keys: &[&str]) -> i64 {
    keys.iter().find_map(|k| m.get(*k).and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))).unwrap_or(0)
}

impl GmPlan {
    /// 实测：模型会把买卖、开战、制作写成 effects 里的效果类型（例如 `{"type":"trade","seller":"N1","items":[...]}`），
    /// 而不是写进对应的字段。这里把它们移到正确的字段（字段已经有内容时不覆盖），返回移动了几条。
    pub fn salvage_rpg_effects(&mut self) -> usize {
        let mut moved = 0;
        for list in [&mut self.effects, &mut self.effects_on_success, &mut self.effects_on_failure] {
            let (rpg, rest): (Vec<_>, Vec<_>) = std::mem::take(list).into_iter().partition(|m| m.get("type").and_then(Value::as_str).is_some_and(|t| RPG_EFFECT_TYPES.contains(&t)));
            *list = rest;
            for m in rpg {
                moved += 1;
                let ty = m.get("type").and_then(Value::as_str).unwrap_or_default().to_string();
                match ty.as_str() {
                    // 实测：模型把检定写成了效果（{"type":"check",...}），移到 checks 里，由程序掷骰。
                    "check" | "skill_check" | "roll" => {
                        let skill = s_of(&m, &["skill", "ability", "name"]);
                        let dc = i_of(&m, &["dc", "difficulty", "target"]);
                        if !skill.is_empty() && dc > 0 && self.checks.len() < 3 {
                            self.checks.push(CheckRequest { skill, dc, reason: s_of(&m, &["reason", "why"]), advantage: s_of(&m, &["advantage"]) });
                        }
                    }
                    "trade" | "buy" | "sell" | "purchase" => {
                        let kind = if ty == "sell" || s_of(&m, &["kind", "action"]) == "sell" { "sell" } else { "buy" };
                        let merchant = s_of(&m, &["merchant", "seller", "buyer", "npc", "from", "to", "with"]);
                        let items: Vec<(String, i64)> = match m.get("items").and_then(Value::as_array) {
                            Some(a) => a
                                .iter()
                                .filter_map(|x| match x {
                                    Value::String(s) => Some((s.clone(), 1)),
                                    Value::Object(o) => Some((s_of(o, &["item", "id", "code", "name"]), i_of(o, &["qty", "count", "quantity"]).max(1))),
                                    _ => None,
                                })
                                .collect(),
                            None => vec![(s_of(&m, &["item", "id", "code"]), i_of(&m, &["qty", "count", "quantity"]).max(1))],
                        };
                        for (item, qty) in items.into_iter().filter(|(i, _)| !i.is_empty()) {
                            if self.trade.len() < 3 {
                                self.trade.push(TradeOut { kind: kind.into(), merchant: merchant.clone(), item, qty });
                            }
                        }
                    }
                    "combat_start" | "start_combat" | "encounter" | "combat" => {
                        if self.combat_start.is_empty() {
                            let one = |o: &Map<String, Value>| EnemyOut { name: s_of(o, &["name", "enemy"]), npc: s_of(o, &["npc"]), level: i_of(o, &["level"]).max(1), count: i_of(o, &["count", "qty", "number"]).max(1), boss: o.get("boss").and_then(Value::as_bool).unwrap_or(false) };
                            match m.get("enemies").and_then(Value::as_array) {
                                Some(a) => self.combat_start.extend(a.iter().filter_map(|x| x.as_object().map(one))),
                                None => self.combat_start.push(one(&m)),
                            }
                            self.combat_start.retain(|e| !e.name.trim().is_empty());
                        }
                    }
                    "combat_action" | "attack" => {
                        if self.combat_action.is_none() {
                            let kind = if ty == "attack" { "attack".to_string() } else { s_of(&m, &["kind", "action"]) };
                            self.combat_action = Some(CombatActionOut { kind: if kind.is_empty() { "attack".into() } else { kind }, target: s_of(&m, &["target", "enemy"]), item: s_of(&m, &["item"]) });
                        }
                    }
                    "craft" => {
                        if self.craft.as_deref().is_none_or(|c| c.trim().is_empty()) {
                            self.craft = Some(s_of(&m, &["item", "recipe", "id", "code"]));
                        }
                    }
                    "item_use" | "use_item" | "equip" => {
                        if self.item_use.as_deref().is_none_or(|c| c.trim().is_empty()) {
                            self.item_use = Some(s_of(&m, &["item", "id", "code"]));
                        }
                    }
                    _ => {
                        // reward / rewards / award_xp / give_money
                        if self.rewards.len() < 3 {
                            let amount = i_of(&m, &["amount", "value"]);
                            self.rewards.push(RewardOut {
                                who: Some(s_of(&m, &["who", "target", "to", "holder"])).filter(|s| !s.is_empty()).unwrap_or_else(|| "party".into()),
                                xp: if ty == "award_xp" { i_of(&m, &["xp"]).max(amount) } else { i_of(&m, &["xp"]) },
                                money: if ty == "give_money" { i_of(&m, &["money"]).max(amount) } else { i_of(&m, &["money"]) },
                                reason: s_of(&m, &["reason"]),
                            });
                        }
                    }
                }
            }
        }
        moved
    }
}

#[cfg(test)]
mod rpg_salvage_tests {
    use super::*;

    /// 实测（Phase 6 第 1 轮联调）中模型写出的原样内容。
    #[test]
    fn misplaced_rpg_effects_move_to_their_fields() {
        let mut p: GmPlan = serde_json::from_value(serde_json::json!({
            "intent": "x", "narration": "y",
            "effects": [{"type": "trade", "seller": "N1", "items": [{"id": "S1", "qty": 1}, {"id": "S2", "qty": 1}]}, {"type": "set_flag", "name": "met"}, {"type": "craft", "recipe": "R1"}],
            "effects_on_success": [{"type": "combat_start", "enemies": [{"name": "野狼", "level": 0, "qty": 2}]}]
        }))
        .unwrap();
        assert_eq!(p.salvage_rpg_effects(), 3);
        assert_eq!(p.effects.len(), 1, "普通效果保留");
        assert_eq!(p.trade.len(), 2);
        assert_eq!((p.trade[0].kind.as_str(), p.trade[0].merchant.as_str(), p.trade[1].item.as_str()), ("buy", "N1", "S2"));
        assert_eq!(p.craft.as_deref(), Some("R1"));
        assert_eq!((p.combat_start[0].name.as_str(), p.combat_start[0].level, p.combat_start[0].count), ("野狼", 1, 2));
    }
}

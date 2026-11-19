//! 剧情纠正检测（反剧情纠正改革）：GM 写完叙述后，程序先找疑点，再交给模型结合因果复核。
//!
//! 程序只找"可能有问题"的地方（关键词不能定罪：人物有理由来找玩家时不是问题），
//! 模型复核确认后才要求重写；绝不通过回滚游戏状态来掩盖叙述错误。

use chronicle_core::events::{self, EventStatus};
use chronicle_core::{campaign, nodes, scripts};
use chronicle_core::rusqlite::Connection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct RailIssue {
    /// negate / revive / item_back / request_again / coincidence / not_happened / ignore_goal
    pub kind: String,
    /// 叙述里的原句。
    #[serde(default)]
    pub quote: String,
    /// 为什么是问题（因果说明）。
    #[serde(default)]
    pub why: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct RailCheckOut {
    #[serde(default)]
    pub issues: Vec<RailIssue>,
}

const DEATH_WORDS: &[&str] = &["死", "尸", "墓", "遗体", "失踪", "下落不明", "生前", "曾经", "回忆", "遗物", "葬", "悼", "传闻", "听说"];
const OBTAIN_WORDS: &[&str] = &["拿到", "得到", "捡起", "找到", "递给你", "交给你", "又一", "另一", "一模一样", "完好无损", "放在你手", "塞给你"];
const HAPPENED_WORDS: &[&str] = &["已经", "发生了", "终于", "刚刚", "之前就"];
const COINCIDENCE_WORDS: &[&str] = &["找上门", "恰好", "正好", "刚好", "碰巧", "偏偏", "迎面撞见", "不期而遇", "突然出现在你面前"];
const REQUEST_WORDS: &[&str] = &["委托", "拜托", "请你", "求你", "帮我", "务必", "必须", "一定要", "还是得", "不如去", "该去", "应该去"];

fn has_any(text: &str, words: &[&str]) -> bool {
    words.iter().any(|w| text.contains(w))
}

/// 找出疑点（给模型复核用的说明）。`input` 是玩家这一回合的话，玩家自己提起的不算。
pub fn suspects(conn: &Connection, campaign_id: &str, input: &str, narration: &str, choices: &[String]) -> Vec<String> {
    let mut out = vec![];
    // 1. 死去 / 失踪的人物像在场一样出现。
    if let Ok(st) = campaign::campaign_state(conn, campaign_id) {
        for (k, v) in &st.entities {
            if let Some(id) = k.strip_suffix("|life_status")
                && matches!(v.as_str(), Some("dead") | Some("missing"))
                && let Ok(Some(n)) = nodes::get_node(conn, id)
                && n.title.chars().count() >= 2
                && narration.contains(&n.title)
                && !has_any(narration, DEATH_WORDS)
                && !n.payload.get("can_revive").and_then(|v| v.as_bool()).unwrap_or(false)
            {
                out.push(format!("revive：「{}」已经{}，叙述里像在场一样出现", n.title, if v.as_str() == Some("dead") { "死亡" } else { "失踪" }));
            }
        }
    }
    // 2. 被摧毁的物品又出现、又被得到。
    for item in events::destroyed_items(conn, campaign_id).unwrap_or_default() {
        let title = scripts::get_item(conn, &item).ok().flatten().map(|i| i.title).or_else(|| nodes::get_node(conn, &item).ok().flatten().map(|n| n.title)).unwrap_or_default();
        // 只要被毁的东西又在叙述里出现（玩家自己没提起它），就交给模型复核：
        // 实测"箱子里整整齐齐码放着好几块备用灯芯石"这种写法不一定带"拿到、找到"之类的词。
        let gone_words = ["扔进", "沉入", "沉进", "毁", "碎", "化成", "没了", "不见", "消失", "再也"];
        if title.chars().count() >= 2 && narration.contains(&title) && !input.contains(&title) && (has_any(narration, OBTAIN_WORDS) || !has_any(narration, &gone_words)) {
            out.push(format!("item_back：「{title}」已经被摧毁，叙述里像是又出现了"));
        }
    }
    // 3. 玩家拒绝 / 放弃了的委托又被提出来。
    if let Ok(closed) = events::closed_quests(conn, campaign_id) {
        for (id, s) in closed.iter().filter(|(_, s)| matches!(s, EventStatus::Bypassed | EventStatus::Invalid)) {
            let title = scripts::get_item(conn, id).ok().flatten().map(|i| i.title).unwrap_or_default();
            if title.chars().count() < 2 || input.contains(&title) {
                continue;
            }
            if narration.contains(&title) && has_any(narration, REQUEST_WORDS) {
                out.push(format!("request_again：委托「{title}」已经{}，叙述里又在要求玩家去做", s.label()));
            }
            if choices.iter().any(|c| c.contains(&title)) {
                out.push(format!("request_again：委托「{title}」已经{}，选项里又出现了", s.label()));
            }
        }
    }
    // 4. 还没发生的事件写成了已经发生。
    for e in events::potential(conn, campaign_id).unwrap_or_default() {
        if e.title.chars().count() >= 3 && narration.contains(&e.title) && has_any(narration, HAPPENED_WORDS) {
            out.push(format!("not_happened：潜在事件「{}」还没发生（{}），叙述里像是已经发生了", e.title, e.status.label()));
        }
    }
    // 5. 巧合：玩家没有去找，人物却刚好出现。
    if has_any(narration, COINCIDENCE_WORDS) && !has_any(input, &["找", "等", "叫", "喊", "约"]) {
        let hit = COINCIDENCE_WORDS.iter().find(|w| narration.contains(**w)).copied().unwrap_or_default();
        out.push(format!("coincidence：叙述里有「{hit}」，需要确认这件事有因果依据（谁、为什么、怎么知道的）"));
    }
    // 6. 玩家有自己的目标时，叙述又转回没开始的主线任务。
    if let Some(goal) = events::player_goal(conn, campaign_id)
        && let Ok(qs) = chronicle_core::quests::quests(conn, campaign_id, true)
    {
        for q in qs.iter().filter(|q| q.quest_type == "main" && q.status.is_empty() && q.title.chars().count() >= 2) {
            if !goal.text.contains(&q.title) && !input.contains(&q.title) && (narration.contains(&q.title) && has_any(narration, REQUEST_WORDS)) {
                out.push(format!("ignore_goal：玩家现在的目标是「{}」，叙述却在催玩家去做主线「{}」", goal.text, q.title));
            }
        }
    }
    out.dedup();
    out
}

/// 选项里去掉重新派发已拒绝 / 已绕过委托的条目。
pub fn filter_choices(conn: &Connection, campaign_id: &str, choices: &mut Vec<String>) {
    let Ok(closed) = events::closed_quests(conn, campaign_id) else { return };
    let titles: Vec<String> = closed
        .iter()
        .filter(|(_, s)| matches!(s, EventStatus::Bypassed | EventStatus::Invalid))
        .filter_map(|(id, _)| scripts::get_item(conn, id).ok().flatten().map(|i| i.title))
        .filter(|t| t.chars().count() >= 2)
        .collect();
    choices.retain(|c| !titles.iter().any(|t| c.contains(t.as_str())));
}

/// 给复核模型的"已确定的事实 / 玩家目标 / 潜在事件"。
pub fn facts_for_check(conn: &Connection, campaign_id: &str) -> (String, String, String) {
    let facts = events::settled_facts(conn, campaign_id).unwrap_or_default();
    let goal = events::player_goal(conn, campaign_id).map(|g| g.text).unwrap_or_else(|| "（玩家没有说明）".into());
    let pot: Vec<String> = events::potential(conn, campaign_id).unwrap_or_default().into_iter().take(10).map(|e| format!("- {}（{}）", e.title, e.status.label())).collect();
    (
        if facts.is_empty() { "（没有）".into() } else { facts.iter().map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n") },
        goal,
        if pot.is_empty() { "（没有）".into() } else { pot.join("\n") },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_lists_cover_examples() {
        assert!(has_any("莫伦恰好找上门来", COINCIDENCE_WORDS));
        assert!(has_any("你又找到了一枚一模一样的钥匙", OBTAIN_WORDS));
        assert!(!has_any("你沿着海岸走了很久", COINCIDENCE_WORDS));
    }
}

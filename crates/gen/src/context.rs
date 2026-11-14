//! 生成前的上下文组装：读取相关世界设定与已有内容，按优先级放入 token 预算。

use chronicle_core::model::*;
use chronicle_core::{World, nodes, scripts};
use chronicle_core::rusqlite::params;

use crate::Result;

/// 粗略估算 token 数：中日韩字符约 1 token/字，其他字符约 4 字符/token。
pub fn estimate_tokens(s: &str) -> usize {
    let mut cjk = 0usize;
    let mut other = 0usize;
    for c in s.chars() {
        if ('\u{2E80}'..='\u{9FFF}').contains(&c) || ('\u{F900}'..='\u{FAFF}').contains(&c) || ('\u{FF00}'..='\u{FFEF}').contains(&c) {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    cjk + other.div_ceil(4)
}

#[derive(Debug, Clone)]
pub struct Section {
    pub title: String,
    pub body: String,
    /// 数字越小越优先；0 表示必须包含。
    pub priority: u8,
}

#[derive(Debug, Clone, Default)]
pub struct Context {
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone)]
pub struct RenderedContext {
    pub text: String,
    pub tokens: usize,
    /// 被省略或截断的部分。
    pub dropped: Vec<String>,
    pub digest: String,
}

fn fnv(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

impl Context {
    pub fn push(&mut self, title: &str, body: String, priority: u8) {
        if !body.trim().is_empty() {
            self.sections.push(Section { title: title.into(), body, priority });
        }
    }

    /// 按优先级装入预算；装不下的部分按行截断，并记录在 `dropped`。
    pub fn render(&self, budget: usize) -> RenderedContext {
        let mut order: Vec<&Section> = self.sections.iter().collect();
        order.sort_by_key(|s| s.priority);
        let mut used = 0usize;
        let mut kept: Vec<(usize, String)> = vec![];
        let mut dropped = vec![];
        for s in order {
            let header = format!("## {}\n", s.title);
            let full = format!("{header}{}\n", s.body.trim_end());
            let cost = estimate_tokens(&full);
            if s.priority == 0 || used + cost <= budget {
                used += cost;
                kept.push((self.sections.iter().position(|x| std::ptr::eq(x, s)).unwrap(), full));
                continue;
            }
            // 尝试按行截断。
            let mut part = header.clone();
            let mut part_cost = estimate_tokens(&header);
            for line in s.body.lines() {
                let c = estimate_tokens(line) + 1;
                if used + part_cost + c > budget {
                    break;
                }
                part.push_str(line);
                part.push('\n');
                part_cost += c;
            }
            if part.len() > header.len() {
                part.push_str("（以下省略）\n");
                used += part_cost;
                kept.push((self.sections.iter().position(|x| std::ptr::eq(x, s)).unwrap(), part));
                dropped.push(format!("{}（部分）", s.title));
            } else {
                dropped.push(s.title.clone());
            }
        }
        kept.sort_by_key(|(i, _)| *i);
        let text: String = kept.into_iter().map(|(_, t)| t).collect::<Vec<_>>().join("\n");
        RenderedContext { digest: fnv(&text), tokens: estimate_tokens(&text), text, dropped }
    }
}

fn line(title: &str, text: &str, max: usize) -> String {
    let t: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        format!("- {title}")
    } else if t.chars().count() > max {
        format!("- {title}：{}…", t.chars().take(max).collect::<String>())
    } else {
        format!("- {title}：{t}")
    }
}

/// 世界核心设定（core canon）。
pub fn core_canon(world: &World, limit: i64) -> Result<String> {
    let mut stmt = world.conn().prepare(
        "SELECT * FROM nodes WHERE canon_level = 'core' AND deleted_at IS NULL ORDER BY path LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], <Node as chronicle_core::entity::Entity>::from_row)?;
    let mut out = vec![];
    for n in rows {
        let n = n?;
        out.push(line(&format!("[{}] {}", n.kind, n.title), if n.summary.is_empty() { &n.player_text } else { &n.summary }, 200));
    }
    Ok(out.join("\n"))
}

/// 已有的世界名称（人物、势力、地点），用于避免重名与引用已有内容。
pub fn world_names(world: &World, kinds: &[NodeKind], per_kind: i64) -> Result<String> {
    let mut out = vec![];
    for k in kinds {
        let list = nodes::nodes_by_kind(world.conn(), *k, 0, per_kind)?;
        if list.is_empty() {
            continue;
        }
        let names: Vec<String> = list
            .iter()
            .map(|n| if n.summary.is_empty() { n.title.clone() } else { format!("{}（{}）", n.title, n.summary.chars().take(30).collect::<String>()) })
            .collect();
        out.push(format!("- {}：{}", k, names.join("、")));
    }
    Ok(out.join("\n"))
}

pub fn script_meta(s: &Script) -> String {
    let mut v = vec![format!("- 名称：{}", s.title)];
    for (k, val) in [("题材", &s.genre), ("基调", &s.tone), ("卖点", &s.premise), ("背景", &s.background), ("核心设定（主持人）", &s.core_setting)] {
        if !val.trim().is_empty() {
            v.push(format!("- {k}：{val}"));
        }
    }
    v.push(format!("- 玩家人数：{}；预计时长：{} 小时", s.player_count, s.duration_hours));
    v.join("\n")
}

/// 剧本大纲：条目树（标题 + 简短说明），含条目 ID，便于模型引用。
pub fn script_outline(world: &World, script_id: &str, with_ids: bool) -> Result<String> {
    let mut out = vec![];
    fn walk(world: &World, sid: &str, parent: Option<&str>, depth: usize, with_ids: bool, out: &mut Vec<String>) -> Result<()> {
        for it in scripts::item_children(world.conn(), sid, parent)? {
            let id = if with_ids { format!(" (id={})", it.id) } else { String::new() };
            let lock = if it.locked { " [已锁定]" } else { "" };
            let mut l = format!("{}- [{}] {}{id}{lock}", "  ".repeat(depth), it.kind, it.title);
            let brief: String = it.player_text.split_whitespace().collect::<Vec<_>>().join(" ");
            if !brief.is_empty() {
                l.push_str(&format!("：{}", brief.chars().take(60).collect::<String>()));
            }
            out.push(l);
            walk(world, sid, Some(&it.id), depth + 1, with_ids, out)?;
        }
        Ok(())
    }
    walk(world, script_id, None, 0, with_ids, &mut out)?;
    Ok(out.join("\n"))
}

pub fn cast_text(world: &World, script_id: &str) -> Result<String> {
    let mut out = vec![];
    for c in scripts::cast_of(world.conn(), script_id)? {
        if let Some(n) = nodes::get_node(world.conn(), &c.node_id)? {
            let mut l = format!("- {}", n.title);
            if !c.role.is_empty() {
                l.push_str(&format!("（{}）", c.role));
            }
            if !c.motive.is_empty() {
                l.push_str(&format!("：动机 {}", c.motive));
            }
            out.push(l);
        }
    }
    Ok(out.join("\n"))
}

pub fn item_detail(it: &ScriptItem) -> String {
    format!(
        "### {} [{}] (id={})\n玩家可见：{}\n主持人：{}",
        it.title,
        it.kind,
        it.id,
        if it.player_text.is_empty() { "（空）" } else { &it.player_text },
        if it.gm_text.is_empty() { "（空）" } else { &it.gm_text }
    )
}

pub fn node_detail(n: &Node) -> String {
    format!(
        "### {} [{}] (id={})\n摘要：{}\n玩家可见：{}\n主持人：{}\n资料：{}",
        n.title, n.kind, n.id, n.summary, n.player_text, n.gm_text, n.payload
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_estimate() {
        assert_eq!(estimate_tokens("你好世界"), 4);
        assert_eq!(estimate_tokens("abcdefgh"), 2);
    }

    #[test]
    fn budget_keeps_required_and_truncates() {
        let mut c = Context::default();
        c.push("必需", "核心".repeat(50), 0);
        c.push("次要", (0..100).map(|i| format!("第{i}行内容")).collect::<Vec<_>>().join("\n"), 3);
        c.push("最次", "不会放入".repeat(100), 9);
        let r = c.render(300);
        assert!(r.text.contains("## 必需"));
        assert!(r.text.contains("（以下省略）"));
        assert!(r.dropped.iter().any(|d| d == "最次"));
        assert!(r.tokens <= 320);
        assert_eq!(r.digest, c.render(300).digest, "同样的输入得到同样的摘要");
    }
}

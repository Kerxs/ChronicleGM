//! 剧透模式（全局，默认关闭）：关闭时，图谱、人物关系、搜索结果、游戏面板、档案馆都不显示主持人信息——
//! 主持人可见的关系、节点的主持人文字与秘密字段、只给主持人看的事件。由程序在返回数据前过滤。

use crate::model::{Node, Relation, Visibility};
use crate::payload::strip_gm_fields;

/// 去掉节点里的主持人信息（主持人文字、秘密等字段）。
pub fn strip_node(n: &mut Node) {
    n.gm_text.clear();
    n.payload = strip_gm_fields(n.kind, &n.payload);
}

/// 关闭剧透模式时：去掉主持人可见的关系和节点里的主持人信息。
pub fn filter_graph(nodes: &mut [Node], relations: &mut Vec<Relation>, spoilers: bool) {
    if spoilers {
        return;
    }
    relations.retain(|r| r.visibility != Visibility::Gm);
    for n in nodes.iter_mut() {
        strip_node(n);
    }
}

/// 只给主持人看的事件类型（AI 方案、被拒绝的修改、导演建议、泄露检查等）。
pub const GM_EVENT_KINDS: &[&str] = &["gm_plan", "effects_rejected", "context_trace", "conflict", "speech_redirected", "manual_edit", "director_beat", "trigger_fired", "quality"];

/// 关闭剧透模式时的事件日志：去掉主持人事件，去掉叙述与台词里的泄露检查结果（里面是秘密原文）。
pub fn filter_events(events: &mut Vec<crate::campaign::EventEntry>, spoilers: bool) {
    if spoilers {
        return;
    }
    events.retain(|e| !gm_only_event(&e.kind));
    for e in events.iter_mut() {
        if let Some(o) = e.payload.as_object_mut() {
            o.remove("leaks");
        }
    }
}

pub fn gm_only_event(kind: &str) -> bool {
    GM_EVENT_KINDS.contains(&kind)
}

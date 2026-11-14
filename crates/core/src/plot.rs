//! 剧情图（有向图）节点与边的读写。

use rusqlite::Connection;

use crate::entity::Entity;
use crate::error::{CoreError, Result};
use crate::model::*;
use crate::scripts::require_script;
use crate::util::new_id;
use crate::world::{Writer, check_rev};

pub fn get_plot_node(conn: &Connection, id: &str) -> Result<Option<PlotNode>> {
    PlotNode::load_active(conn, id)
}

pub fn require_plot_node(conn: &Connection, id: &str) -> Result<PlotNode> {
    get_plot_node(conn, id)?.ok_or_else(|| CoreError::NotFound("剧情节点", id.into()))
}

pub fn get_edge(conn: &Connection, id: &str) -> Result<Option<PlotEdge>> {
    PlotEdge::load_active(conn, id)
}

pub fn plot_nodes_of(conn: &Connection, script_id: &str) -> Result<Vec<PlotNode>> {
    let mut stmt =
        conn.prepare_cached("SELECT * FROM plot_nodes WHERE script_id = ?1 AND deleted_at IS NULL ORDER BY created_at")?;
    let rows = stmt.query_map([script_id], PlotNode::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn edges_of_script(conn: &Connection, script_id: &str) -> Result<Vec<PlotEdge>> {
    let mut stmt =
        conn.prepare_cached("SELECT * FROM plot_edges WHERE script_id = ?1 AND deleted_at IS NULL ORDER BY created_at")?;
    let rows = stmt.query_map([script_id], PlotEdge::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 出边，按优先级（高者优先）排序。
pub fn outgoing(conn: &Connection, node_id: &str) -> Result<Vec<PlotEdge>> {
    let mut stmt = conn.prepare_cached(
        "SELECT * FROM plot_edges WHERE from_id = ?1 AND deleted_at IS NULL ORDER BY priority DESC, created_at",
    )?;
    let rows = stmt.query_map([node_id], PlotEdge::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn incoming(conn: &Connection, node_id: &str) -> Result<Vec<PlotEdge>> {
    let mut stmt =
        conn.prepare_cached("SELECT * FROM plot_edges WHERE to_id = ?1 AND deleted_at IS NULL ORDER BY created_at")?;
    let rows = stmt.query_map([node_id], PlotEdge::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn validate_edge_logic(e: &NewPlotEdge) -> Result<()> {
    if let Some(c) = &e.condition {
        c.validate()?;
    }
    for eff in &e.effects {
        eff.validate()?;
    }
    Ok(())
}

impl Writer<'_> {
    pub fn create_plot_node(&mut self, n: NewPlotNode) -> Result<PlotNode> {
        self.create_plot_node_with_id(new_id(), n)
    }

    pub(crate) fn create_plot_node_with_id(&mut self, id: String, n: NewPlotNode) -> Result<PlotNode> {
        if n.title.trim().is_empty() {
            return Err(CoreError::Invalid("剧情节点标题不能为空".into()));
        }
        require_script(&self.tx, &n.script_id)?;
        if !n.payload.is_object() {
            return Err(CoreError::Invalid("剧情节点 payload 必须是对象".into()));
        }
        if let Some(r) = &n.script_item_ref {
            let it = crate::scripts::require_item(&self.tx, r)?;
            if it.script_id != n.script_id {
                return Err(CoreError::Invalid("引用的剧本条目属于其他剧本".into()));
            }
        }
        if let Some(r) = &n.scene_ref {
            crate::nodes::require_node(&self.tx, r)?;
        }
        let node = PlotNode {
            id,
            script_id: n.script_id,
            kind: n.kind,
            title: n.title.trim().into(),
            player_text: n.player_text,
            gm_text: n.gm_text,
            scene_ref: n.scene_ref,
            script_item_ref: n.script_item_ref,
            status: PlotNodeStatus::Planned,
            payload: n.payload,
            locked: false,
            source: self.author.tag(),
            rev: 1,
            created_at: self.now,
            updated_at: self.now,
            deleted_at: None,
        };
        self.put_new(&node)?;
        Ok(node)
    }

    pub fn update_plot_node(&mut self, id: &str, p: PlotNodePatch, expected_rev: Option<i64>) -> Result<PlotNode> {
        let cur = require_plot_node(&self.tx, id)?;
        check_rev("剧情节点", id, expected_rev, cur.rev)?;
        self.guard(&format!("剧情节点「{}」", cur.title), cur.locked, false, p.is_unlock_only())?;
        if self.author.is_ai() && p.locked.is_some() {
            return Err(CoreError::Protected("AI 不能修改锁定状态".into()));
        }
        let mut n = cur.clone();
        if let Some(v) = p.title {
            if v.trim().is_empty() {
                return Err(CoreError::Invalid("剧情节点标题不能为空".into()));
            }
            n.title = v.trim().into();
        }
        if let Some(v) = p.kind {
            n.kind = v;
        }
        if let Some(v) = p.player_text {
            n.player_text = v;
        }
        if let Some(v) = p.gm_text {
            n.gm_text = v;
        }
        if let Some(v) = p.status {
            n.status = v;
        }
        if let Some(v) = p.payload {
            if !v.is_object() {
                return Err(CoreError::Invalid("剧情节点 payload 必须是对象".into()));
            }
            n.payload = v;
        }
        if let Some(v) = p.locked {
            n.locked = v;
        }
        if n == cur {
            return Ok(cur);
        }
        self.put_update(&cur, n)
    }

    /// 删除剧情节点及与之相连的边。
    pub fn delete_plot_node(&mut self, id: &str, expected_rev: Option<i64>) -> Result<()> {
        let cur = require_plot_node(&self.tx, id)?;
        check_rev("剧情节点", id, expected_rev, cur.rev)?;
        self.guard(&format!("剧情节点「{}」", cur.title), cur.locked, false, false)?;
        let mut edges = outgoing(&self.tx, id)?;
        edges.extend(incoming(&self.tx, id)?);
        for e in edges {
            if PlotEdge::load_active(&self.tx, &e.id)?.is_some() {
                self.guard("剧情边", e.locked, false, false)?;
                let mut d = e.clone();
                d.deleted_at = Some(self.now);
                self.put_update(&e, d)?;
            }
        }
        let mut d = cur.clone();
        d.deleted_at = Some(self.now);
        self.put_update(&cur, d)?;
        Ok(())
    }

    pub fn create_plot_edge(&mut self, e: NewPlotEdge) -> Result<PlotEdge> {
        self.create_plot_edge_with_id(new_id(), e)
    }

    pub(crate) fn create_plot_edge_with_id(&mut self, id: String, e: NewPlotEdge) -> Result<PlotEdge> {
        require_script(&self.tx, &e.script_id)?;
        let from = require_plot_node(&self.tx, &e.from_id)?;
        let to = require_plot_node(&self.tx, &e.to_id)?;
        if from.script_id != e.script_id || to.script_id != e.script_id {
            return Err(CoreError::Invalid("剧情边两端必须属于同一剧本".into()));
        }
        if from.kind == PlotNodeKind::Ending {
            return Err(CoreError::Invalid("结局节点不能有出边".into()));
        }
        validate_edge_logic(&e)?;
        let edge = PlotEdge {
            id,
            script_id: e.script_id,
            from_id: e.from_id,
            to_id: e.to_id,
            label: e.label,
            condition: e.condition,
            effects: e.effects,
            priority: e.priority,
            once: e.once,
            locked: false,
            source: self.author.tag(),
            rev: 1,
            created_at: self.now,
            updated_at: self.now,
            deleted_at: None,
        };
        self.put_new(&edge)?;
        Ok(edge)
    }

    /// 更新边的标签、条件、效果、优先级（两端不可改，改两端请删除重建）。
    pub fn update_plot_edge(&mut self, id: &str, p: PlotEdgePatch, expected_rev: Option<i64>) -> Result<PlotEdge> {
        let PlotEdgePatch { label, condition, effects, priority, once } = p;
        let cur = get_edge(&self.tx, id)?.ok_or_else(|| CoreError::NotFound("剧情边", id.into()))?;
        check_rev("剧情边", id, expected_rev, cur.rev)?;
        self.guard("剧情边", cur.locked, false, false)?;
        let mut e = cur.clone();
        if let Some(v) = label {
            e.label = v;
        }
        if let Some(v) = condition {
            if let Some(c) = &v {
                c.validate()?;
            }
            e.condition = v;
        }
        if let Some(v) = effects {
            for eff in &v {
                eff.validate()?;
            }
            e.effects = v;
        }
        if let Some(v) = priority {
            e.priority = v;
        }
        if let Some(v) = once {
            e.once = v;
        }
        if e == cur {
            return Ok(cur);
        }
        self.put_update(&cur, e)
    }

    pub fn delete_plot_edge(&mut self, id: &str, expected_rev: Option<i64>) -> Result<()> {
        let cur = get_edge(&self.tx, id)?.ok_or_else(|| CoreError::NotFound("剧情边", id.into()))?;
        check_rev("剧情边", id, expected_rev, cur.rev)?;
        self.guard("剧情边", cur.locked, false, false)?;
        let mut d = cur.clone();
        d.deleted_at = Some(self.now);
        self.put_update(&cur, d)?;
        Ok(())
    }
}

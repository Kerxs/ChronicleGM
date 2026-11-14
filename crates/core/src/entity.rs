//! 实体行映射：每种可修订实体实现 [`Entity`]，供通用的修订记录与撤销使用。

use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::Result;
use crate::model::*;
use crate::util::normalize_name;

pub(crate) fn jget<T: DeserializeOwned>(r: &Row, col: &str) -> rusqlite::Result<T> {
    let s: String = r.get(col)?;
    serde_json::from_str(&s).map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(e)))
}

pub(crate) fn jget_opt<T: DeserializeOwned>(r: &Row, col: &str) -> rusqlite::Result<Option<T>> {
    let s: Option<String> = r.get(col)?;
    match s {
        None => Ok(None),
        Some(s) => serde_json::from_str(&s)
            .map(Some)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(e))),
    }
}

pub(crate) fn eget<E>(r: &Row, col: &str, parse: fn(&str) -> Result<E>) -> rusqlite::Result<E> {
    let s: String = r.get(col)?;
    parse(&s).map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(e)))
}

pub(crate) fn js<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("序列化不会失败")
}

pub trait Entity: Serialize + DeserializeOwned + Clone {
    const NAME: &'static str;
    fn id(&self) -> &str;
    fn rev(&self) -> i64;
    fn set_rev(&mut self, rev: i64);
    fn set_updated_at(&mut self, ts: i64);
    fn deleted_at(&self) -> Option<i64>;
    fn from_row(r: &Row) -> rusqlite::Result<Self>;
    fn insert(&self, conn: &Connection) -> Result<()>;
    fn update(&self, conn: &Connection) -> Result<()>;

    fn load(conn: &Connection, id: &str) -> Result<Option<Self>> {
        let sql = format!("SELECT * FROM {} WHERE id = ?1", Self::NAME);
        Ok(conn.query_row(&sql, [id], |r| Self::from_row(r)).optional()?)
    }
    fn load_active(conn: &Connection, id: &str) -> Result<Option<Self>> {
        Ok(Self::load(conn, id)?.filter(|e| e.deleted_at().is_none()))
    }
    fn hard_delete(conn: &Connection, id: &str) -> Result<()> {
        conn.execute(&format!("DELETE FROM {} WHERE id = ?1", Self::NAME), [id])?;
        Ok(())
    }
}

macro_rules! common_impl {
    () => {
        fn id(&self) -> &str {
            &self.id
        }
        fn rev(&self) -> i64 {
            self.rev
        }
        fn set_rev(&mut self, rev: i64) {
            self.rev = rev;
        }
        fn set_updated_at(&mut self, ts: i64) {
            self.updated_at = ts;
        }
        fn deleted_at(&self) -> Option<i64> {
            self.deleted_at
        }
    };
}

// ---------- Node ----------

pub(crate) fn sync_names(conn: &Connection, n: &Node) -> Result<()> {
    sync_node_names(conn, n)
}

fn sync_node_names(conn: &Connection, n: &Node) -> Result<()> {
    conn.execute("DELETE FROM node_names WHERE node_id = ?1", [&n.id])?;
    let mut stmt = conn.prepare_cached("INSERT OR IGNORE INTO node_names(node_id, name_key) VALUES (?1, ?2)")?;
    for name in std::iter::once(&n.title).chain(n.aliases.iter()) {
        let key = normalize_name(name);
        if !key.is_empty() {
            stmt.execute(params![n.id, key])?;
        }
    }
    Ok(())
}

impl Entity for Node {
    const NAME: &'static str = "nodes";
    common_impl!();

    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Node {
            id: r.get("id")?,
            kind: eget(r, "kind", NodeKind::parse)?,
            parent_id: r.get("parent_id")?,
            sort_key: r.get("sort_key")?,
            path: r.get("path")?,
            title: r.get("title")?,
            aliases: jget(r, "aliases")?,
            summary: r.get("summary")?,
            player_text: r.get("player_text")?,
            gm_text: r.get("gm_text")?,
            payload: jget(r, "payload")?,
            tags: jget(r, "tags")?,
            canon_level: eget(r, "canon_level", CanonLevel::parse)?,
            locked: r.get("locked")?,
            status: eget(r, "status", ContentStatus::parse)?,
            source: r.get("source")?,
            rev: r.get("rev")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
            deleted_at: r.get("deleted_at")?,
        })
    }

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT INTO nodes (id, kind, parent_id, sort_key, path, title, aliases, summary, player_text, gm_text,
                payload, tags, canon_level, locked, status, source, rev, created_at, updated_at, deleted_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
            params![
                self.id, self.kind.as_str(), self.parent_id, self.sort_key, self.path, self.title,
                js(&self.aliases), self.summary, self.player_text, self.gm_text, js(&self.payload),
                js(&self.tags), self.canon_level.as_str(), self.locked, self.status.as_str(), self.source,
                self.rev, self.created_at, self.updated_at, self.deleted_at
            ],
        )?;
        sync_node_names(conn, self)
    }

    /// 整行覆盖。若 path 变化，同时改写所有后代的 path 前缀（派生数据，不单独记修订）。
    fn update(&self, conn: &Connection) -> Result<()> {
        let old_path: Option<String> =
            conn.query_row("SELECT path FROM nodes WHERE id = ?1", [&self.id], |r| r.get(0)).optional()?;
        conn.execute(
            "UPDATE nodes SET kind=?2, parent_id=?3, sort_key=?4, path=?5, title=?6, aliases=?7, summary=?8,
                player_text=?9, gm_text=?10, payload=?11, tags=?12, canon_level=?13, locked=?14, status=?15,
                source=?16, rev=?17, created_at=?18, updated_at=?19, deleted_at=?20
             WHERE id=?1",
            params![
                self.id, self.kind.as_str(), self.parent_id, self.sort_key, self.path, self.title,
                js(&self.aliases), self.summary, self.player_text, self.gm_text, js(&self.payload),
                js(&self.tags), self.canon_level.as_str(), self.locked, self.status.as_str(), self.source,
                self.rev, self.created_at, self.updated_at, self.deleted_at
            ],
        )?;
        if let Some(old) = old_path.filter(|p| *p != self.path) {
            let upper = format!("{old}\u{7f}");
            conn.execute(
                "UPDATE nodes SET path = ?1 || substr(path, ?2) WHERE path > ?3 AND path < ?4",
                params![self.path, old.chars().count() as i64 + 1, old, upper],
            )?;
        }
        sync_node_names(conn, self)
    }
}

// ---------- Relation ----------

impl Entity for Relation {
    const NAME: &'static str = "relations";
    common_impl!();

    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Relation {
            id: r.get("id")?,
            src_id: r.get("src_id")?,
            dst_id: r.get("dst_id")?,
            rel_type: r.get("rel_type")?,
            directed: r.get("directed")?,
            label: r.get("label")?,
            attrs: jget(r, "attrs")?,
            visibility: eget(r, "visibility", Visibility::parse)?,
            valid_from: r.get("valid_from")?,
            valid_to: r.get("valid_to")?,
            source: r.get("source")?,
            rev: r.get("rev")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
            deleted_at: r.get("deleted_at")?,
        })
    }

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT INTO relations (id, src_id, dst_id, rel_type, directed, label, attrs, visibility, valid_from,
                valid_to, source, rev, created_at, updated_at, deleted_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            params![
                self.id, self.src_id, self.dst_id, self.rel_type, self.directed, self.label, js(&self.attrs),
                self.visibility.as_str(), self.valid_from, self.valid_to, self.source, self.rev,
                self.created_at, self.updated_at, self.deleted_at
            ],
        )?;
        Ok(())
    }

    fn update(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "UPDATE relations SET src_id=?2, dst_id=?3, rel_type=?4, directed=?5, label=?6, attrs=?7, visibility=?8,
                valid_from=?9, valid_to=?10, source=?11, rev=?12, created_at=?13, updated_at=?14, deleted_at=?15
             WHERE id=?1",
            params![
                self.id, self.src_id, self.dst_id, self.rel_type, self.directed, self.label, js(&self.attrs),
                self.visibility.as_str(), self.valid_from, self.valid_to, self.source, self.rev,
                self.created_at, self.updated_at, self.deleted_at
            ],
        )?;
        Ok(())
    }
}

// ---------- Script ----------

impl Entity for Script {
    const NAME: &'static str = "scripts";
    common_impl!();

    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Script {
            id: r.get("id")?,
            title: r.get("title")?,
            genre: r.get("genre")?,
            premise: r.get("premise")?,
            background: r.get("background")?,
            core_setting: r.get("core_setting")?,
            player_count: r.get("player_count")?,
            duration_hours: r.get("duration_hours")?,
            tone: r.get("tone")?,
            scale_profile: jget(r, "scale_profile")?,
            status: eget(r, "status", ContentStatus::parse)?,
            source: r.get("source")?,
            rev: r.get("rev")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
            deleted_at: r.get("deleted_at")?,
        })
    }

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT INTO scripts (id, title, genre, premise, background, core_setting, player_count, duration_hours,
                tone, scale_profile, status, source, rev, created_at, updated_at, deleted_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            params![
                self.id, self.title, self.genre, self.premise, self.background, self.core_setting,
                self.player_count, self.duration_hours, self.tone, js(&self.scale_profile),
                self.status.as_str(), self.source, self.rev, self.created_at, self.updated_at, self.deleted_at
            ],
        )?;
        Ok(())
    }

    fn update(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "UPDATE scripts SET title=?2, genre=?3, premise=?4, background=?5, core_setting=?6, player_count=?7,
                duration_hours=?8, tone=?9, scale_profile=?10, status=?11, source=?12, rev=?13, created_at=?14,
                updated_at=?15, deleted_at=?16
             WHERE id=?1",
            params![
                self.id, self.title, self.genre, self.premise, self.background, self.core_setting,
                self.player_count, self.duration_hours, self.tone, js(&self.scale_profile),
                self.status.as_str(), self.source, self.rev, self.created_at, self.updated_at, self.deleted_at
            ],
        )?;
        Ok(())
    }
}

// ---------- ScriptItem ----------

impl Entity for ScriptItem {
    const NAME: &'static str = "script_items";
    common_impl!();

    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(ScriptItem {
            id: r.get("id")?,
            script_id: r.get("script_id")?,
            kind: eget(r, "kind", ScriptItemKind::parse)?,
            parent_id: r.get("parent_id")?,
            sort_key: r.get("sort_key")?,
            title: r.get("title")?,
            player_text: r.get("player_text")?,
            gm_text: r.get("gm_text")?,
            payload: jget(r, "payload")?,
            locked: r.get("locked")?,
            status: eget(r, "status", ContentStatus::parse)?,
            source: r.get("source")?,
            rev: r.get("rev")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
            deleted_at: r.get("deleted_at")?,
        })
    }

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT INTO script_items (id, script_id, kind, parent_id, sort_key, title, player_text, gm_text, payload,
                locked, status, source, rev, created_at, updated_at, deleted_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            params![
                self.id, self.script_id, self.kind.as_str(), self.parent_id, self.sort_key, self.title,
                self.player_text, self.gm_text, js(&self.payload), self.locked, self.status.as_str(),
                self.source, self.rev, self.created_at, self.updated_at, self.deleted_at
            ],
        )?;
        Ok(())
    }

    fn update(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "UPDATE script_items SET script_id=?2, kind=?3, parent_id=?4, sort_key=?5, title=?6, player_text=?7,
                gm_text=?8, payload=?9, locked=?10, status=?11, source=?12, rev=?13, created_at=?14,
                updated_at=?15, deleted_at=?16
             WHERE id=?1",
            params![
                self.id, self.script_id, self.kind.as_str(), self.parent_id, self.sort_key, self.title,
                self.player_text, self.gm_text, js(&self.payload), self.locked, self.status.as_str(),
                self.source, self.rev, self.created_at, self.updated_at, self.deleted_at
            ],
        )?;
        Ok(())
    }
}

// ---------- CastMember ----------

impl Entity for CastMember {
    const NAME: &'static str = "script_cast";
    common_impl!();

    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(CastMember {
            id: r.get("id")?,
            script_id: r.get("script_id")?,
            node_id: r.get("node_id")?,
            role: r.get("role")?,
            motive: r.get("motive")?,
            secret: r.get("secret")?,
            relation_to_players: r.get("relation_to_players")?,
            payload: jget(r, "payload")?,
            locked: r.get("locked")?,
            source: r.get("source")?,
            rev: r.get("rev")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
            deleted_at: r.get("deleted_at")?,
        })
    }

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT INTO script_cast (id, script_id, node_id, role, motive, secret, relation_to_players, payload,
                locked, source, rev, created_at, updated_at, deleted_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![
                self.id, self.script_id, self.node_id, self.role, self.motive, self.secret,
                self.relation_to_players, js(&self.payload), self.locked, self.source, self.rev,
                self.created_at, self.updated_at, self.deleted_at
            ],
        )?;
        Ok(())
    }

    fn update(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "UPDATE script_cast SET script_id=?2, node_id=?3, role=?4, motive=?5, secret=?6, relation_to_players=?7,
                payload=?8, locked=?9, source=?10, rev=?11, created_at=?12, updated_at=?13, deleted_at=?14
             WHERE id=?1",
            params![
                self.id, self.script_id, self.node_id, self.role, self.motive, self.secret,
                self.relation_to_players, js(&self.payload), self.locked, self.source, self.rev,
                self.created_at, self.updated_at, self.deleted_at
            ],
        )?;
        Ok(())
    }
}

// ---------- PlotNode ----------

impl Entity for PlotNode {
    const NAME: &'static str = "plot_nodes";
    common_impl!();

    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(PlotNode {
            id: r.get("id")?,
            script_id: r.get("script_id")?,
            kind: eget(r, "kind", PlotNodeKind::parse)?,
            title: r.get("title")?,
            player_text: r.get("player_text")?,
            gm_text: r.get("gm_text")?,
            scene_ref: r.get("scene_ref")?,
            script_item_ref: r.get("script_item_ref")?,
            status: eget(r, "status", PlotNodeStatus::parse)?,
            payload: jget(r, "payload")?,
            locked: r.get("locked")?,
            source: r.get("source")?,
            rev: r.get("rev")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
            deleted_at: r.get("deleted_at")?,
        })
    }

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT INTO plot_nodes (id, script_id, kind, title, player_text, gm_text, scene_ref, script_item_ref,
                status, payload, locked, source, rev, created_at, updated_at, deleted_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            params![
                self.id, self.script_id, self.kind.as_str(), self.title, self.player_text, self.gm_text,
                self.scene_ref, self.script_item_ref, self.status.as_str(), js(&self.payload), self.locked,
                self.source, self.rev, self.created_at, self.updated_at, self.deleted_at
            ],
        )?;
        Ok(())
    }

    fn update(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "UPDATE plot_nodes SET script_id=?2, kind=?3, title=?4, player_text=?5, gm_text=?6, scene_ref=?7,
                script_item_ref=?8, status=?9, payload=?10, locked=?11, source=?12, rev=?13, created_at=?14,
                updated_at=?15, deleted_at=?16
             WHERE id=?1",
            params![
                self.id, self.script_id, self.kind.as_str(), self.title, self.player_text, self.gm_text,
                self.scene_ref, self.script_item_ref, self.status.as_str(), js(&self.payload), self.locked,
                self.source, self.rev, self.created_at, self.updated_at, self.deleted_at
            ],
        )?;
        Ok(())
    }
}

// ---------- PlotEdge ----------

impl Entity for PlotEdge {
    const NAME: &'static str = "plot_edges";
    common_impl!();

    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(PlotEdge {
            id: r.get("id")?,
            script_id: r.get("script_id")?,
            from_id: r.get("from_id")?,
            to_id: r.get("to_id")?,
            label: r.get("label")?,
            condition: jget_opt(r, "condition")?,
            effects: jget(r, "effects")?,
            priority: r.get("priority")?,
            once: r.get("once")?,
            locked: r.get("locked")?,
            source: r.get("source")?,
            rev: r.get("rev")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
            deleted_at: r.get("deleted_at")?,
        })
    }

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT INTO plot_edges (id, script_id, from_id, to_id, label, condition, effects, priority, once,
                locked, source, rev, created_at, updated_at, deleted_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            params![
                self.id, self.script_id, self.from_id, self.to_id, self.label,
                self.condition.as_ref().map(js), js(&self.effects), self.priority, self.once, self.locked,
                self.source, self.rev, self.created_at, self.updated_at, self.deleted_at
            ],
        )?;
        Ok(())
    }

    fn update(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "UPDATE plot_edges SET script_id=?2, from_id=?3, to_id=?4, label=?5, condition=?6, effects=?7,
                priority=?8, once=?9, locked=?10, source=?11, rev=?12, created_at=?13, updated_at=?14,
                deleted_at=?15
             WHERE id=?1",
            params![
                self.id, self.script_id, self.from_id, self.to_id, self.label,
                self.condition.as_ref().map(js), js(&self.effects), self.priority, self.once, self.locked,
                self.source, self.rev, self.created_at, self.updated_at, self.deleted_at
            ],
        )?;
        Ok(())
    }
}

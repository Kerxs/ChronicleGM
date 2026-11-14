//! 剧本、剧本条目、剧本角色的读写。

use rusqlite::{Connection, params};

use crate::entity::Entity;
use crate::error::{CoreError, Result};
use crate::model::*;
use crate::nodes::{key_for_position, require_node};
use crate::payload::validate_script_item_payload;
use crate::util::new_id;
use crate::world::{Writer, check_rev};

fn check_text(t: &str, what: &str) -> Result<()> {
    if t.trim().is_empty() {
        return Err(CoreError::Invalid(format!("{what}不能为空")));
    }
    Ok(())
}

// ===================== 读取 =====================

pub fn get_script(conn: &Connection, id: &str) -> Result<Option<Script>> {
    Script::load_active(conn, id)
}

pub fn require_script(conn: &Connection, id: &str) -> Result<Script> {
    get_script(conn, id)?.ok_or_else(|| CoreError::NotFound("剧本", id.into()))
}

pub fn list_scripts(conn: &Connection) -> Result<Vec<Script>> {
    let mut stmt = conn.prepare("SELECT * FROM scripts WHERE deleted_at IS NULL ORDER BY created_at")?;
    let rows = stmt.query_map([], Script::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn get_item(conn: &Connection, id: &str) -> Result<Option<ScriptItem>> {
    ScriptItem::load_active(conn, id)
}

pub fn require_item(conn: &Connection, id: &str) -> Result<ScriptItem> {
    get_item(conn, id)?.ok_or_else(|| CoreError::NotFound("剧本条目", id.into()))
}

pub fn item_children(conn: &Connection, script_id: &str, parent: Option<&str>) -> Result<Vec<ScriptItem>> {
    let mut stmt = conn.prepare_cached(
        "SELECT * FROM script_items WHERE script_id = ?1 AND parent_id IS ?2 AND deleted_at IS NULL ORDER BY sort_key",
    )?;
    let rows = stmt.query_map(params![script_id, parent], ScriptItem::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn items_by_kind(conn: &Connection, script_id: &str, kind: ScriptItemKind) -> Result<Vec<ScriptItem>> {
    let mut stmt = conn.prepare_cached(
        "SELECT * FROM script_items WHERE script_id = ?1 AND kind = ?2 AND deleted_at IS NULL ORDER BY sort_key",
    )?;
    let rows = stmt.query_map(params![script_id, kind.as_str()], ScriptItem::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 条目及其全部后代（递归 CTE）。
pub fn item_subtree(conn: &Connection, id: &str) -> Result<Vec<ScriptItem>> {
    let mut stmt = conn.prepare_cached(
        "WITH RECURSIVE t(id) AS (
            SELECT id FROM script_items WHERE id = ?1
            UNION ALL SELECT s.id FROM script_items s JOIN t ON s.parent_id = t.id WHERE s.deleted_at IS NULL
         ) SELECT s.* FROM script_items s JOIN t ON s.id = t.id WHERE s.deleted_at IS NULL",
    )?;
    let rows = stmt.query_map([id], ScriptItem::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn cast_of(conn: &Connection, script_id: &str) -> Result<Vec<CastMember>> {
    let mut stmt = conn.prepare_cached(
        "SELECT * FROM script_cast WHERE script_id = ?1 AND deleted_at IS NULL ORDER BY created_at",
    )?;
    let rows = stmt.query_map([script_id], CastMember::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn item_sibling_keys(conn: &Connection, script_id: &str, parent: Option<&str>) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, sort_key FROM script_items WHERE script_id = ?1 AND parent_id IS ?2 AND deleted_at IS NULL
         ORDER BY sort_key",
    )?;
    let rows = stmt.query_map(params![script_id, parent], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ===================== 写入 =====================

impl Writer<'_> {
    pub fn create_script(&mut self, s: NewScript) -> Result<Script> {
        self.create_script_with_id(new_id(), s)
    }

    pub(crate) fn create_script_with_id(&mut self, id: String, s: NewScript) -> Result<Script> {
        check_text(&s.title, "剧本名称")?;
        if !(1..=50).contains(&s.player_count) {
            return Err(CoreError::Invalid("玩家人数需在 1–50".into()));
        }
        if !(s.duration_hours > 0.0 && s.duration_hours <= 500.0) {
            return Err(CoreError::Invalid("游戏时长需在 0–500 小时".into()));
        }
        let profile = scale_profile(s.player_count, s.duration_hours);
        let script = Script {
            id,
            title: s.title.trim().into(),
            genre: s.genre,
            premise: s.premise,
            background: s.background,
            core_setting: s.core_setting,
            player_count: s.player_count,
            duration_hours: s.duration_hours,
            tone: s.tone,
            scale_profile: serde_json::to_value(profile)?,
            status: ContentStatus::Draft,
            source: self.author.tag(),
            rev: 1,
            created_at: self.now,
            updated_at: self.now,
            deleted_at: None,
        };
        self.put_new(&script)?;
        Ok(script)
    }

    pub fn update_script(&mut self, id: &str, p: ScriptPatch, expected_rev: Option<i64>) -> Result<Script> {
        let cur = require_script(&self.tx, id)?;
        check_rev("剧本", id, expected_rev, cur.rev)?;
        let mut s = cur.clone();
        if let Some(v) = p.title {
            check_text(&v, "剧本名称")?;
            s.title = v.trim().into();
        }
        if let Some(v) = p.genre {
            s.genre = v;
        }
        if let Some(v) = p.premise {
            s.premise = v;
        }
        if let Some(v) = p.background {
            s.background = v;
        }
        if let Some(v) = p.core_setting {
            s.core_setting = v;
        }
        if let Some(v) = p.tone {
            s.tone = v;
        }
        if let Some(v) = p.status {
            s.status = v;
        }
        if p.player_count.is_some() || p.duration_hours.is_some() {
            s.player_count = p.player_count.unwrap_or(s.player_count);
            s.duration_hours = p.duration_hours.unwrap_or(s.duration_hours);
            if !(1..=50).contains(&s.player_count) || !(s.duration_hours > 0.0 && s.duration_hours <= 500.0) {
                return Err(CoreError::Invalid("玩家人数或时长超出范围".into()));
            }
            s.scale_profile = serde_json::to_value(scale_profile(s.player_count, s.duration_hours))?;
        }
        if s == cur {
            return Ok(cur);
        }
        self.put_update(&cur, s)
    }

    pub fn delete_script(&mut self, id: &str, expected_rev: Option<i64>) -> Result<()> {
        let cur = require_script(&self.tx, id)?;
        check_rev("剧本", id, expected_rev, cur.rev)?;
        if self.author.is_ai() {
            return Err(CoreError::Protected("AI 不能删除整个剧本".into()));
        }
        let mut d = cur.clone();
        d.deleted_at = Some(self.now);
        self.put_update(&cur, d)?;
        Ok(())
    }

    pub fn create_script_item(&mut self, it: NewScriptItem) -> Result<ScriptItem> {
        self.create_script_item_with_id(new_id(), it)
    }

    pub(crate) fn create_script_item_with_id(&mut self, id: String, it: NewScriptItem) -> Result<ScriptItem> {
        check_text(&it.title, "条目标题")?;
        require_script(&self.tx, &it.script_id)?;
        validate_script_item_payload(it.kind, &it.payload)?;
        if self.author.is_ai() && it.locked {
            return Err(CoreError::Protected("AI 不能创建锁定内容".into()));
        }
        if let Some(p) = &it.parent_id {
            let parent = require_item(&self.tx, p)?;
            if parent.script_id != it.script_id {
                return Err(CoreError::Invalid("父条目属于其他剧本".into()));
            }
        }
        let siblings = item_sibling_keys(&self.tx, &it.script_id, it.parent_id.as_deref())?;
        let item = ScriptItem {
            id,
            script_id: it.script_id,
            kind: it.kind,
            parent_id: it.parent_id,
            sort_key: key_for_position(&siblings, &it.position)?,
            title: it.title.trim().into(),
            player_text: it.player_text,
            gm_text: it.gm_text,
            payload: it.payload,
            locked: it.locked,
            status: ContentStatus::Draft,
            source: self.author.tag(),
            rev: 1,
            created_at: self.now,
            updated_at: self.now,
            deleted_at: None,
        };
        self.put_new(&item)?;
        Ok(item)
    }

    pub fn update_script_item(&mut self, id: &str, p: ScriptItemPatch, expected_rev: Option<i64>) -> Result<ScriptItem> {
        let cur = require_item(&self.tx, id)?;
        check_rev("剧本条目", id, expected_rev, cur.rev)?;
        self.guard(&format!("条目「{}」", cur.title), cur.locked, false, p.is_unlock_only())?;
        if self.author.is_ai() && p.locked.is_some() {
            return Err(CoreError::Protected("AI 不能修改锁定状态".into()));
        }
        let mut s = cur.clone();
        if let Some(v) = p.title {
            check_text(&v, "条目标题")?;
            s.title = v.trim().into();
        }
        if let Some(v) = p.player_text {
            s.player_text = v;
        }
        if let Some(v) = p.gm_text {
            s.gm_text = v;
        }
        if let Some(v) = p.payload {
            validate_script_item_payload(s.kind, &v)?;
            s.payload = v;
        }
        if let Some(v) = p.locked {
            s.locked = v;
        }
        if let Some(v) = p.status {
            s.status = v;
        }
        if s == cur {
            return Ok(cur);
        }
        self.put_update(&cur, s)
    }

    pub fn delete_script_item(&mut self, id: &str, expected_rev: Option<i64>) -> Result<usize> {
        let cur = require_item(&self.tx, id)?;
        check_rev("剧本条目", id, expected_rev, cur.rev)?;
        let items = item_subtree(&self.tx, id)?;
        for it in &items {
            self.guard(&format!("条目「{}」", it.title), it.locked, false, false)?;
        }
        for it in &items {
            let mut d = it.clone();
            d.deleted_at = Some(self.now);
            self.put_update(it, d)?;
        }
        Ok(items.len())
    }

    pub fn add_cast(&mut self, c: NewCastMember) -> Result<CastMember> {
        self.add_cast_with_id(new_id(), c)
    }

    pub(crate) fn add_cast_with_id(&mut self, id: String, c: NewCastMember) -> Result<CastMember> {
        require_script(&self.tx, &c.script_id)?;
        let node = require_node(&self.tx, &c.node_id)?;
        if node.kind != NodeKind::Character {
            return Err(CoreError::Invalid(format!("「{}」不是人物节点", node.title)));
        }
        if !c.payload.is_object() {
            return Err(CoreError::Invalid("角色 payload 必须是对象".into()));
        }
        let m = CastMember {
            id,
            script_id: c.script_id,
            node_id: c.node_id,
            role: c.role,
            motive: c.motive,
            secret: c.secret,
            relation_to_players: c.relation_to_players,
            payload: c.payload,
            locked: false,
            source: self.author.tag(),
            rev: 1,
            created_at: self.now,
            updated_at: self.now,
            deleted_at: None,
        };
        self.put_new(&m)?;
        Ok(m)
    }

    pub fn remove_cast(&mut self, id: &str) -> Result<()> {
        let cur = CastMember::load_active(&self.tx, id)?.ok_or_else(|| CoreError::NotFound("剧本角色", id.into()))?;
        self.guard("剧本角色", cur.locked, false, false)?;
        let mut d = cur.clone();
        d.deleted_at = Some(self.now);
        self.put_update(&cur, d)?;
        Ok(())
    }
}

/// 剧本的全部条目（未删除）。
pub fn all_items(conn: &Connection, script_id: &str) -> Result<Vec<ScriptItem>> {
    let mut stmt = conn.prepare_cached("SELECT * FROM script_items WHERE script_id = ?1 AND deleted_at IS NULL ORDER BY sort_key")?;
    let rows = stmt.query_map([script_id], ScriptItem::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

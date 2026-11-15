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
            game_profile: if s.game_profile.as_object().is_some_and(|o| !o.is_empty()) {
                let gp: crate::profile::GameProfile = serde_json::from_value(s.game_profile.clone()).map_err(|e| CoreError::Invalid(format!("玩法配置格式不对：{e}")))?;
                gp.validate()?;
                serde_json::to_value(&gp)?
            } else {
                serde_json::Value::Object(Default::default())
            },
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
        if let Some(v) = p.game_profile {
            let gp: crate::profile::GameProfile = serde_json::from_value(v.clone()).map_err(|e| CoreError::Invalid(format!("玩法配置格式不对：{e}")))?;
            gp.validate()?;
            s.game_profile = serde_json::to_value(&gp)?;
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

// ===================== 剧本修改后需要复查的世界树节点 =====================

/// 和一个剧本条目有关的世界树节点：条目文字和附加资料里提到名字（或别名、编号）的人物、地点、势力、物品。
/// 剧本改了之后，界面提示这些节点可能需要复查（只提示，不修改世界树）。
pub fn related_nodes(conn: &Connection, item_id: &str) -> Result<Vec<crate::model::Node>> {
    let it = require_item(conn, item_id)?;
    let text = format!("{} {} {} {}", it.title, it.player_text, it.gm_text, it.payload);
    let mut stmt = conn.prepare(
        "SELECT * FROM nodes WHERE deleted_at IS NULL AND kind IN ('character','faction','region','nation','city','site','item','event','organization') LIMIT 3000",
    )?;
    let nodes: Vec<crate::model::Node> = stmt.query_map([], crate::model::Node::from_row)?.collect::<rusqlite::Result<_>>()?;
    let mut out = vec![];
    for n in nodes {
        let names: Vec<String> = std::iter::once(n.title.clone()).chain(n.aliases.iter().cloned()).filter(|s| s.chars().count() >= 2).collect();
        if text.contains(&n.id) || names.iter().any(|s| text.contains(s.as_str())) {
            out.push(n);
            if out.len() >= 20 {
                break;
            }
        }
    }
    Ok(out)
}

// ===================== 剧本检查 =====================

/// 剧本检查的一条结果。只给问题和建议，不修改剧本。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ScriptFinding {
    /// error（缺了就没法玩）/ warning（建议补上）/ info（提示）
    pub severity: String,
    pub message: String,
    #[serde(default)]
    pub suggestion: String,
    /// 相关的剧本条目或世界树节点。
    #[serde(default)]
    pub item_id: Option<String>,
    #[serde(default)]
    pub node_id: Option<String>,
}

fn finding(severity: &str, message: impl Into<String>, suggestion: impl Into<String>) -> ScriptFinding {
    ScriptFinding { severity: severity.into(), message: message.into(), suggestion: suggestion.into(), item_id: None, node_id: None }
}

/// 程序检查：完整性（开场、章节、主线、结局、可选角色与身世）、引用、玩法配置与内容是否相符、
/// 剧本修改后可能需要复查的世界树节点。
pub fn check_script(conn: &Connection, script_id: &str) -> Result<Vec<ScriptFinding>> {
    let s = require_script(conn, script_id)?;
    let items = all_items(conn, script_id)?;
    let count = |k: ScriptItemKind| items.iter().filter(|i| i.kind == k).count();
    let mut out = vec![];
    if count(ScriptItemKind::Opening) == 0 {
        out.push(finding("error", "没有开场", "添加一个「开场」条目：交代环境、玩家身份与初始处境，开始游戏时 GM 会按它讲述开场"));
    }
    let chapters = count(ScriptItemKind::Chapter);
    if chapters == 0 {
        out.push(finding("error", "没有章节", "用「生成剧本大纲」或手动添加章节"));
    }
    let quests: Vec<&ScriptItem> = items.iter().filter(|i| i.kind == ScriptItemKind::Quest).collect();
    if !quests.iter().any(|q| q.payload.get("quest_type").and_then(|v| v.as_str()).unwrap_or("main") == "main") {
        out.push(finding("warning", "没有主线任务", "添加一个主线任务，让玩家知道大方向"));
    }
    if count(ScriptItemKind::Ending) == 0 {
        out.push(finding("warning", "没有结局", "至少写一个结局与达成条件"));
    }
    let pcs: Vec<&ScriptItem> = items.iter().filter(|i| i.kind == ScriptItemKind::PlayerCharacter).collect();
    if pcs.is_empty() {
        out.push(finding("warning", "没有可选角色", "开始游戏时可以让 AI 按剧本生成角色；也可以在剧本里添加「可选角色」并写上身世"));
    }
    for pc in &pcs {
        let b = pc.payload.get("backstory");
        let empty = b.is_none_or(|b| ["origin", "upbringing", "occupation"].iter().all(|k| b.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().is_empty()));
        if empty {
            let mut f = finding("warning", format!("可选角色「{}」没有身世", pc.title), "补上出身、成长经历、职业，以及与主线的关联");
            f.item_id = Some(pc.id.clone());
            out.push(f);
        }
    }
    for c in cast_of(conn, script_id)? {
        match crate::nodes::get_node(conn, &c.node_id)? {
            Some(n) if n.deleted_at.is_none() => {
                // 剧本修改后，关联的世界树人物可能需要复查。
                if s.updated_at > n.updated_at + 1000 && s.rev > 1 {
                    let mut f = finding("info", format!("剧本修改过，人物「{}」的世界树资料可能需要复查", n.title), "打开世界树核对身份、动机与秘密是否与剧本一致");
                    f.node_id = Some(n.id.clone());
                    out.push(f);
                }
            }
            _ => out.push(finding("error", "剧本人物引用的世界树节点不存在或已删除", "在剧本人物列表里移除，或从回收站恢复该人物")),
        }
    }
    let mut missing_places: Vec<String> = items
        .iter()
        .filter_map(|i| i.payload.get("location_name").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    missing_places.sort();
    missing_places.dedup();
    if !missing_places.is_empty() {
        out.push(finding("info", format!("这些地点还不在世界树里：{}", missing_places.join("、")), "用「根据剧本生成世界树」补上地点，或在世界树里手动添加"));
    }
    let gp = crate::profile::for_script(&s);
    let clues = count(ScriptItemKind::Clue);
    let (_, per_max) = gp.clues_per_chapter();
    if chapters > 0 && clues > per_max as usize * chapters + 2 {
        out.push(finding(
            "info",
            format!("线索有 {clues} 条，比玩法配置建议的多（调查比重 {}）", gp.weights.investigation),
            "这不是以探案为主的剧本时，可以删掉一些线索，或在「玩法配置」里调高调查比重",
        ));
    }
    if gp.source == "inferred" {
        out.push(finding("info", "玩法配置是按题材推断的", "打开「玩法配置」确认玩法比重、属性与检定规则"));
    }
    Ok(out)
}

//! 变更集：AI（或导入工具）提出的修改先保存为待审变更集，经校验、
//! 用户逐条接受/编辑/拒绝后，才在一个事务中写入世界。
//!
//! - 新建的实体可以用临时 ID（`tmp:` 开头）在同一变更集的后续操作中引用。
//! - 校验采用"试运行"：在事务中逐条执行并回滚，因此与真实写入使用同一套规则
//!   （引用存在、payload 类型、锁定/核心设定保护、版本冲突），另加重复与矛盾检查。
//! - 写入失败或校验不通过时不修改任何数据。

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::entity::{Entity, js};
use crate::error::{CoreError, Result};
use crate::model::*;
use crate::nodes;
use crate::util::{new_id, now_ms};
use crate::world::{World, Writer};

pub const TEMP_PREFIX: &str = "tmp:";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    CreateNode {
        #[serde(default)]
        temp_id: Option<String>,
        node: NewNode,
    },
    UpdateNode {
        id: String,
        #[serde(default)]
        base_rev: Option<i64>,
        patch: NodePatch,
    },
    DeleteNode {
        id: String,
        #[serde(default)]
        base_rev: Option<i64>,
    },
    CreateRelation {
        #[serde(default)]
        temp_id: Option<String>,
        relation: NewRelation,
    },
    DeleteRelation {
        id: String,
        #[serde(default)]
        base_rev: Option<i64>,
    },
    CreateScript {
        #[serde(default)]
        temp_id: Option<String>,
        script: NewScript,
    },
    UpdateScript {
        id: String,
        #[serde(default)]
        base_rev: Option<i64>,
        patch: ScriptPatch,
    },
    CreateScriptItem {
        #[serde(default)]
        temp_id: Option<String>,
        item: NewScriptItem,
    },
    UpdateScriptItem {
        id: String,
        #[serde(default)]
        base_rev: Option<i64>,
        patch: ScriptItemPatch,
    },
    DeleteScriptItem {
        id: String,
        #[serde(default)]
        base_rev: Option<i64>,
    },
    AddCast {
        #[serde(default)]
        temp_id: Option<String>,
        cast: NewCastMember,
    },
    CreatePlotNode {
        #[serde(default)]
        temp_id: Option<String>,
        node: NewPlotNode,
    },
    UpdatePlotNode {
        id: String,
        #[serde(default)]
        base_rev: Option<i64>,
        patch: PlotNodePatch,
    },
    DeletePlotNode {
        id: String,
        #[serde(default)]
        base_rev: Option<i64>,
    },
    CreatePlotEdge {
        #[serde(default)]
        temp_id: Option<String>,
        edge: NewPlotEdge,
    },
    DeletePlotEdge {
        id: String,
        #[serde(default)]
        base_rev: Option<i64>,
    },
}

impl Op {
    pub fn is_create(&self) -> bool {
        matches!(
            self,
            Op::CreateNode { .. }
                | Op::CreateRelation { .. }
                | Op::CreateScript { .. }
                | Op::CreateScriptItem { .. }
                | Op::AddCast { .. }
                | Op::CreatePlotNode { .. }
                | Op::CreatePlotEdge { .. }
        )
    }

    pub fn temp_id(&self) -> Option<&str> {
        match self {
            Op::CreateNode { temp_id, .. }
            | Op::CreateRelation { temp_id, .. }
            | Op::CreateScript { temp_id, .. }
            | Op::CreateScriptItem { temp_id, .. }
            | Op::AddCast { temp_id, .. }
            | Op::CreatePlotNode { temp_id, .. }
            | Op::CreatePlotEdge { temp_id, .. } => temp_id.as_deref(),
            _ => None,
        }
    }

    /// 修改/删除操作的目标 ID。
    pub fn target_id(&self) -> Option<&str> {
        match self {
            Op::UpdateNode { id, .. }
            | Op::DeleteNode { id, .. }
            | Op::DeleteRelation { id, .. }
            | Op::UpdateScript { id, .. }
            | Op::UpdateScriptItem { id, .. }
            | Op::DeleteScriptItem { id, .. }
            | Op::UpdatePlotNode { id, .. }
            | Op::DeletePlotNode { id, .. }
            | Op::DeletePlotEdge { id, .. } => Some(id),
            _ => None,
        }
    }

    fn base_rev_missing(&self) -> bool {
        match self {
            Op::UpdateNode { base_rev, .. }
            | Op::DeleteNode { base_rev, .. }
            | Op::DeleteRelation { base_rev, .. }
            | Op::UpdateScript { base_rev, .. }
            | Op::UpdateScriptItem { base_rev, .. }
            | Op::DeleteScriptItem { base_rev, .. }
            | Op::UpdatePlotNode { base_rev, .. }
            | Op::DeletePlotNode { base_rev, .. }
            | Op::DeletePlotEdge { base_rev, .. } => base_rev.is_none(),
            _ => false,
        }
    }
}

/// 变更集模式。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Mode {
    Normal,
    /// 继续扩展：只允许新建，不覆盖已有内容。
    Append,
    /// 只重新生成选中条目：只允许修改列出的 ID。
    Targeted { targets: Vec<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Issue {
    pub op: usize,
    pub severity: Severity,
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub related: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ValidationReport {
    pub ok: bool,
    pub issues: Vec<Issue>,
    pub checked_at: i64,
}

impl ValidationReport {
    pub fn errors(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.severity == Severity::Error)
    }
    pub fn summary(&self) -> String {
        self.errors().map(|i| format!("#{} {}", i.op, i.message)).collect::<Vec<_>>().join("；")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ChangesetStatus {
    Pending,
    Applied,
    Partial,
    Rejected,
}

impl ChangesetStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Applied => "applied",
            Self::Partial => "partial",
            Self::Rejected => "rejected",
        }
    }
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "pending" => Self::Pending,
            "applied" => Self::Applied,
            "partial" => Self::Partial,
            "rejected" => Self::Rejected,
            o => return Err(CoreError::Invalid(format!("变更集状态无效: {o}"))),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Changeset {
    pub id: String,
    pub job_id: Option<String>,
    pub scope: String,
    pub mode: Mode,
    pub author: Author,
    pub title: String,
    pub ops: Vec<Op>,
    pub validation: ValidationReport,
    pub status: ChangesetStatus,
    pub accepted_ops: Option<Vec<usize>>,
    pub commit_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NewChangeset {
    #[serde(default)]
    pub job_id: Option<String>,
    pub scope: String,
    pub mode: Mode,
    pub author: Author,
    #[serde(default)]
    pub title: String,
    pub ops: Vec<Op>,
}

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Changeset> {
    use crate::entity::{jget, jget_opt};
    let author: String = r.get("author")?;
    Ok(Changeset {
        id: r.get("id")?,
        job_id: r.get("job_id")?,
        scope: r.get("scope")?,
        mode: jget(r, "mode")?,
        author: Author::from_tag(&author),
        title: r.get("title")?,
        ops: jget(r, "ops")?,
        validation: jget(r, "validation_report")?,
        status: crate::entity::eget(r, "status", ChangesetStatus::parse)?,
        accepted_ops: jget_opt(r, "accepted_ops")?,
        commit_id: r.get("commit_id")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

pub fn get_changeset(conn: &Connection, id: &str) -> Result<Option<Changeset>> {
    Ok(conn.query_row("SELECT * FROM changesets WHERE id = ?1", [id], from_row).optional()?)
}

pub fn list_changesets(conn: &Connection, status: Option<ChangesetStatus>) -> Result<Vec<Changeset>> {
    let mut stmt =
        conn.prepare("SELECT * FROM changesets WHERE (?1 IS NULL OR status = ?1) ORDER BY created_at DESC")?;
    let rows = stmt.query_map([status.map(|s| s.as_str())], from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ===================== 临时 ID 解析 =====================

fn replace_ids(v: &mut Value, ids: &HashMap<String, String>) {
    match v {
        Value::String(s) => {
            if let Some(real) = ids.get(s.as_str()) {
                *s = real.clone();
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| replace_ids(x, ids)),
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                if k != "temp_id" {
                    replace_ids(x, ids);
                }
            }
        }
        _ => {}
    }
}

fn mentions(v: &Value, set: &HashSet<String>) -> Option<String> {
    match v {
        Value::String(s) if set.contains(s) => Some(s.clone()),
        Value::Array(a) => a.iter().find_map(|x| mentions(x, set)),
        Value::Object(o) => o.iter().filter(|(k, _)| *k != "temp_id").find_map(|(_, x)| mentions(x, set)),
        _ => None,
    }
}

fn resolve(op: &Op, ids: &HashMap<String, String>, unavailable: &HashSet<String>) -> Result<Op> {
    let mut v = serde_json::to_value(op)?;
    if let Some(t) = mentions(&v, unavailable) {
        return Err(CoreError::Invalid(format!("依赖的临时实体 {t} 未创建（所在操作失败或未被接受）")));
    }
    replace_ids(&mut v, ids);
    Ok(serde_json::from_value(v)?)
}

/// 执行单个已解析的操作，返回新建实体的 ID。
fn exec(w: &mut Writer, op: Op, real_id: Option<String>) -> Result<Option<String>> {
    let id = || real_id.clone().unwrap_or_else(new_id);
    Ok(match op {
        Op::CreateNode { node, .. } => Some(w.create_node_with_id(id(), node)?.id),
        Op::UpdateNode { id, base_rev, patch } => {
            w.update_node(&id, patch, base_rev)?;
            None
        }
        Op::DeleteNode { id, base_rev } => {
            w.delete_node(&id, base_rev)?;
            None
        }
        Op::CreateRelation { relation, .. } => Some(w.create_relation_with_id(id(), relation)?.id),
        Op::DeleteRelation { id, base_rev } => {
            w.delete_relation(&id, base_rev)?;
            None
        }
        Op::CreateScript { script, .. } => Some(w.create_script_with_id(id(), script)?.id),
        Op::UpdateScript { id, base_rev, patch } => {
            w.update_script(&id, patch, base_rev)?;
            None
        }
        Op::CreateScriptItem { item, .. } => Some(w.create_script_item_with_id(id(), item)?.id),
        Op::UpdateScriptItem { id, base_rev, patch } => {
            w.update_script_item(&id, patch, base_rev)?;
            None
        }
        Op::DeleteScriptItem { id, base_rev } => {
            w.delete_script_item(&id, base_rev)?;
            None
        }
        Op::AddCast { cast, .. } => Some(w.add_cast_with_id(id(), cast)?.id),
        Op::CreatePlotNode { node, .. } => Some(w.create_plot_node_with_id(id(), node)?.id),
        Op::UpdatePlotNode { id, base_rev, patch } => {
            w.update_plot_node(&id, patch, base_rev)?;
            None
        }
        Op::DeletePlotNode { id, base_rev } => {
            w.delete_plot_node(&id, base_rev)?;
            None
        }
        Op::CreatePlotEdge { edge, .. } => Some(w.create_plot_edge_with_id(id(), edge)?.id),
        Op::DeletePlotEdge { id, base_rev } => {
            w.delete_plot_edge(&id, base_rev)?;
            None
        }
    })
}

// ===================== 校验 =====================

/// 互相矛盾的关系类型（同一时间段内不应同时存在）。
pub const OPPOSING_RELATIONS: &[(&str, &str)] = &[("ally", "enemy"), ("alliance", "war"), ("friend", "enemy")];

fn opposing(t: &str) -> Vec<&'static str> {
    OPPOSING_RELATIONS
        .iter()
        .filter_map(|(a, b)| if *a == t { Some(*b) } else if *b == t { Some(*a) } else { None })
        .collect()
}

fn overlaps(a: (Option<i64>, Option<i64>), b: (Option<i64>, Option<i64>)) -> bool {
    let (a0, a1) = (a.0.unwrap_or(i64::MIN), a.1.unwrap_or(i64::MAX));
    let (b0, b1) = (b.0.unwrap_or(i64::MIN), b.1.unwrap_or(i64::MAX));
    a0 <= b1 && b0 <= a1
}

/// 在当前事务状态上对单个（已解析）操作做启发式检查，产生警告。
fn heuristics(conn: &Connection, idx: usize, op: &Op, issues: &mut Vec<Issue>) -> Result<()> {
    let warn = |code: &str, message: String, related: Vec<String>| Issue {
        op: idx,
        severity: Severity::Warning,
        code: code.into(),
        message,
        related,
    };
    match op {
        Op::CreateNode { node, .. } => {
            let mut seen = HashSet::new();
            for name in std::iter::once(&node.title).chain(node.aliases.iter()) {
                for dup in nodes::find_by_name(conn, name, Some(node.kind))? {
                    if seen.insert(dup.id.clone()) {
                        issues.push(warn(
                            "possible_duplicate",
                            format!("已存在同名{}「{}」", node.kind, dup.title),
                            vec![dup.id],
                        ));
                    }
                }
            }
        }
        Op::CreateRelation { relation: r, .. } => {
            let opp = opposing(&r.rel_type);
            for existing in nodes::relations_between(conn, &r.src_id, &r.dst_id)? {
                if opp.contains(&existing.rel_type.as_str())
                    && overlaps((r.valid_from, r.valid_to), (existing.valid_from, existing.valid_to))
                {
                    issues.push(warn(
                        "relation_conflict",
                        format!("与已有关系「{}」在同一时间段矛盾", existing.rel_type),
                        vec![existing.id.clone()],
                    ));
                }
                if existing.rel_type == r.rel_type
                    && existing.src_id == r.src_id
                    && overlaps((r.valid_from, r.valid_to), (existing.valid_from, existing.valid_to))
                {
                    issues.push(warn("duplicate_relation", "已存在相同的关系".into(), vec![existing.id]));
                }
            }
            if r.rel_type == "participated_in"
                && let (Some(who), Some(event)) = (nodes::get_node(conn, &r.src_id)?, nodes::get_node(conn, &r.dst_id)?)
                && who.kind == NodeKind::Character
            {
                let dead = who.payload.get("life_status").and_then(Value::as_str) == Some("dead");
                let death = who.payload.get("death_time").and_then(Value::as_i64);
                let when = event.payload.get("world_time").and_then(Value::as_i64);
                if dead && matches!((death, when), (Some(d), Some(t)) if t > d) {
                    issues.push(warn(
                        "timeline_conflict",
                        format!("「{}」在事件「{}」发生前已经死亡", who.title, event.title),
                        vec![who.id, event.id],
                    ));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn mode_issue(mode: &Mode, idx: usize, op: &Op) -> Option<Issue> {
    let err = |code: &str, message: String| Issue { op: idx, severity: Severity::Error, code: code.into(), message, related: vec![] };
    match mode {
        Mode::Normal => None,
        Mode::Append if !op.is_create() => Some(err("append_only", "扩展模式只允许新建内容，不能修改或删除已有内容".into())),
        Mode::Targeted { targets } => match op.target_id() {
            Some(t) if targets.iter().any(|x| x == t) && !matches!(op, Op::DeleteNode { .. } | Op::DeleteScriptItem { .. } | Op::DeletePlotNode { .. }) => None,
            _ => Some(err("out_of_target", "只能修改选中的条目".into())),
        },
        _ => None,
    }
}

/// 试运行校验：在事务中逐条执行后回滚，返回每条操作的错误与警告。
pub fn validate(world: &mut World, mode: &Mode, author: &Author, ops: &[Op]) -> Result<ValidationReport> {
    let mut issues = Vec::new();
    // 结构检查：临时 ID 必须带前缀且不重复。
    let mut declared = HashSet::new();
    for (i, op) in ops.iter().enumerate() {
        if let Some(t) = op.temp_id()
            && (!t.starts_with(TEMP_PREFIX) || !declared.insert(t.to_string())) {
                issues.push(Issue {
                    op: i,
                    severity: Severity::Error,
                    code: "bad_temp_id".into(),
                    message: format!("临时 ID「{t}」必须以 {TEMP_PREFIX} 开头且不能重复"),
                    related: vec![],
                });
            }
    }

    let author = author.clone();
    let tx = world.conn.transaction()?;
    let mut w = Writer { tx, commit_id: String::new(), author, revisions: 0, now: now_ms() };
    // 试运行使用一个临时 commit 行承接修订记录，事务结束时随之回滚。
    w.tx.execute(
        "INSERT INTO commits (id, author, message, created_at) VALUES ('__dry_run__', ?1, 'dry-run', 0)",
        [w.author.tag()],
    )?;
    w.commit_id = "__dry_run__".into();

    let mut ids: HashMap<String, String> = HashMap::new();
    let mut failed: HashSet<String> = HashSet::new();
    for (i, op) in ops.iter().enumerate() {
        let fail = |failed: &mut HashSet<String>| {
            if let Some(t) = op.temp_id() {
                failed.insert(t.to_string());
            }
        };
        if let Some(issue) = mode_issue(mode, i, op) {
            issues.push(issue);
            fail(&mut failed);
            continue;
        }
        if w.author.is_ai() && op.base_rev_missing() {
            issues.push(Issue {
                op: i,
                severity: Severity::Warning,
                code: "no_base_rev".into(),
                message: "修改未指定基准版本，无法检测期间是否被他人修改".into(),
                related: vec![],
            });
        }
        let resolved = match resolve(op, &ids, &failed) {
            Ok(r) => r,
            Err(e) => {
                issues.push(Issue { op: i, severity: Severity::Error, code: "dependency".into(), message: e.to_string(), related: vec![] });
                fail(&mut failed);
                continue;
            }
        };
        heuristics(&w.tx, i, &resolved, &mut issues)?;
        w.tx.execute_batch("SAVEPOINT op")?;
        match exec(&mut w, resolved, None) {
            Ok(created) => {
                w.tx.execute_batch("RELEASE op")?;
                if let (Some(t), Some(real)) = (op.temp_id(), created) {
                    ids.insert(t.to_string(), real);
                }
            }
            Err(e) => {
                w.tx.execute_batch("ROLLBACK TO op; RELEASE op")?;
                let code = match &e {
                    CoreError::Protected(_) => "protected",
                    CoreError::RevConflict { .. } => "rev_conflict",
                    CoreError::NotFound(..) => "missing_reference",
                    _ => "invalid",
                };
                issues.push(Issue { op: i, severity: Severity::Error, code: code.into(), message: e.to_string(), related: vec![] });
                fail(&mut failed);
            }
        }
    }
    drop(w); // 事务未提交，自动回滚
    let ok = !issues.iter().any(|i| i.severity == Severity::Error);
    Ok(ValidationReport { ok, issues, checked_at: now_ms() })
}

// ===================== 提交流程 =====================

/// 生成器附加的说明（如规模不符），code 以 `gen.` 开头，重新校验时保留。
pub const GEN_NOTE_PREFIX: &str = "gen.";

fn merge_notes(mut report: ValidationReport, notes: &[Issue]) -> ValidationReport {
    report.issues.extend(notes.iter().cloned());
    report.ok = !report.issues.iter().any(|i| i.severity == Severity::Error);
    report
}

/// 保存一个待审变更集（附带校验报告）。不修改世界内容。
pub fn propose(world: &mut World, c: NewChangeset) -> Result<Changeset> {
    propose_with_notes(world, c, vec![])
}

/// 同 [`propose`]，并附加生成器的说明（code 会自动加上 `gen.` 前缀）。
pub fn propose_with_notes(world: &mut World, c: NewChangeset, notes: Vec<Issue>) -> Result<Changeset> {
    let notes: Vec<Issue> = notes
        .into_iter()
        .map(|mut i| {
            if !i.code.starts_with(GEN_NOTE_PREFIX) {
                i.code = format!("{GEN_NOTE_PREFIX}{}", i.code);
            }
            i
        })
        .collect();
    let report = merge_notes(validate(world, &c.mode, &c.author, &c.ops)?, &notes);
    let now = now_ms();
    let cs = Changeset {
        id: new_id(),
        job_id: c.job_id,
        scope: c.scope,
        mode: c.mode,
        author: c.author,
        title: c.title,
        ops: c.ops,
        validation: report,
        status: ChangesetStatus::Pending,
        accepted_ops: None,
        commit_id: None,
        created_at: now,
        updated_at: now,
    };
    world.conn.execute(
        "INSERT INTO changesets (id, job_id, scope, mode, author, title, ops, validation_report, status, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            cs.id, cs.job_id, cs.scope, js(&cs.mode), cs.author.tag(), cs.title, js(&cs.ops),
            js(&cs.validation), cs.status.as_str(), cs.created_at, cs.updated_at
        ],
    )?;
    Ok(cs)
}

fn require_pending(world: &World, id: &str) -> Result<Changeset> {
    let cs = get_changeset(&world.conn, id)?.ok_or_else(|| CoreError::NotFound("变更集", id.into()))?;
    if cs.status != ChangesetStatus::Pending {
        return Err(CoreError::Invalid(format!("变更集已处理（{:?}）", cs.status)));
    }
    Ok(cs)
}

/// 审阅时手动修改某条操作，修改后重新校验。
pub fn revise_op(world: &mut World, id: &str, index: usize, op: Op) -> Result<Changeset> {
    let mut cs = require_pending(world, id)?;
    if index >= cs.ops.len() {
        return Err(CoreError::Invalid(format!("操作序号 {index} 超出范围")));
    }
    cs.ops[index] = op;
    let notes: Vec<Issue> =
        cs.validation.issues.iter().filter(|i| i.code.starts_with(GEN_NOTE_PREFIX)).cloned().collect();
    cs.validation = merge_notes(validate(world, &cs.mode, &cs.author, &cs.ops)?, &notes);
    cs.updated_at = now_ms();
    world.conn.execute(
        "UPDATE changesets SET ops = ?2, validation_report = ?3, updated_at = ?4 WHERE id = ?1",
        params![cs.id, js(&cs.ops), js(&cs.validation), cs.updated_at],
    )?;
    Ok(cs)
}

pub fn reject(world: &mut World, id: &str) -> Result<()> {
    require_pending(world, id)?;
    world.conn.execute(
        "UPDATE changesets SET status = 'rejected', updated_at = ?2 WHERE id = ?1",
        params![id, now_ms()],
    )?;
    Ok(())
}

/// 用户确认后写入：`accepted = None` 表示全部接受。
/// 所选操作会重新校验（世界可能在提出后已变化）；任何一条失败则全部不写入。
pub fn apply(world: &mut World, id: &str, accepted: Option<Vec<usize>>, reviewer: &Author) -> Result<Changeset> {
    let mut cs = require_pending(world, id)?;
    let mut picked: Vec<usize> = accepted.clone().unwrap_or_else(|| (0..cs.ops.len()).collect());
    picked.sort_unstable();
    picked.dedup();
    if picked.iter().any(|i| *i >= cs.ops.len()) {
        return Err(CoreError::Invalid("接受的操作序号超出范围".into()));
    }
    if picked.is_empty() {
        return Err(CoreError::Invalid("没有选中任何操作；如要放弃请使用拒绝".into()));
    }
    let subset: Vec<Op> = picked.iter().map(|i| cs.ops[*i].clone()).collect();
    let report = validate(world, &cs.mode, &cs.author, &subset)?;
    if !report.ok {
        // 把序号映射回原变更集。
        let msg = report.errors().map(|e| format!("#{} {}", picked[e.op], e.message)).collect::<Vec<_>>().join("；");
        return Err(CoreError::Validation(msg));
    }

    let message = format!("应用变更集「{}」（审阅：{}）", cs.title, reviewer.tag());
    let author = cs.author.clone();
    let (_, commit) = world.write_with(&author, &message, Some(&cs.id), None, |w| {
        let mut ids = HashMap::new();
        let none = HashSet::new();
        for op in &subset {
            let resolved = resolve(op, &ids, &none)?;
            if let (Some(t), Some(real)) = (op.temp_id(), exec(w, resolved, None)?) {
                ids.insert(t.to_string(), real);
            }
        }
        Ok(())
    })?;

    cs.status = if picked.len() == cs.ops.len() { ChangesetStatus::Applied } else { ChangesetStatus::Partial };
    cs.accepted_ops = Some(picked);
    cs.commit_id = commit;
    cs.updated_at = now_ms();
    world.conn.execute(
        "UPDATE changesets SET status = ?2, accepted_ops = ?3, commit_id = ?4, updated_at = ?5 WHERE id = ?1",
        params![cs.id, cs.status.as_str(), js(&cs.accepted_ops), cs.commit_id, cs.updated_at],
    )?;
    Ok(cs)
}

/// 某实体是否由已批准的变更集产生（供 Spawn 效果校验）。
pub fn changeset_is_applied(conn: &Connection, id: &str) -> Result<bool> {
    let st: Option<String> =
        conn.query_row("SELECT status FROM changesets WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    Ok(matches!(st.as_deref(), Some("applied") | Some("partial")))
}

/// 供测试与上层使用：读取实体当前版本号。
pub fn current_rev<E: Entity>(conn: &Connection, id: &str) -> Result<Option<i64>> {
    Ok(E::load(conn, id)?.map(|e| e.rev()))
}

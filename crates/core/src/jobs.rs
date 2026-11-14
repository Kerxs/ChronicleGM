//! AI 生成任务记录。任务失败或重试只改这张表，不影响世界内容；
//! 成功的任务产出一个待审变更集（changesets.job_id 指向任务）。

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::entity::js;
use crate::error::{CoreError, Result};
use crate::util::{new_id, now_ms};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            o => return Err(CoreError::Invalid(format!("任务状态无效: {o}"))),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GenJob {
    pub id: String,
    pub kind: String,
    pub target: String,
    pub params: Value,
    pub context_digest: String,
    pub provider: String,
    pub model: String,
    pub template_version: String,
    pub status: JobStatus,
    pub attempts: i64,
    pub raw_output: Option<String>,
    pub error: Option<String>,
    pub note: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// 该任务产出的变更集（如有）。
    pub changeset_id: Option<String>,
}

fn row(r: &rusqlite::Row) -> rusqlite::Result<GenJob> {
    Ok(GenJob {
        id: r.get("id")?,
        kind: r.get("kind")?,
        target: r.get("target")?,
        params: crate::entity::jget(r, "params")?,
        context_digest: r.get("context_digest")?,
        provider: r.get("provider")?,
        model: r.get("model")?,
        template_version: r.get("template_version")?,
        status: crate::entity::eget(r, "status", JobStatus::parse)?,
        attempts: r.get("attempts")?,
        raw_output: r.get("raw_output")?,
        error: r.get("error")?,
        note: r.get("note")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
        changeset_id: r.get("changeset_id")?,
    })
}

const SELECT: &str = "SELECT j.*, (SELECT c.id FROM changesets c WHERE c.job_id = j.id ORDER BY c.created_at DESC LIMIT 1)
    AS changeset_id FROM gen_jobs j";

pub fn create_job(conn: &Connection, kind: &str, target: &str, params: &Value) -> Result<GenJob> {
    let id = new_id();
    let now = now_ms();
    conn.execute(
        "INSERT INTO gen_jobs (id, kind, target, params, status, created_at, updated_at) VALUES (?1,?2,?3,?4,'queued',?5,?5)",
        params![id, kind, target, js(params), now],
    )?;
    get_job(conn, &id)
}

pub fn get_job(conn: &Connection, id: &str) -> Result<GenJob> {
    conn.query_row(&format!("{SELECT} WHERE j.id = ?1"), [id], row)
        .optional()?
        .ok_or_else(|| CoreError::NotFound("生成任务", id.into()))
}

pub fn list_jobs(conn: &Connection, status: Option<JobStatus>, limit: i64) -> Result<Vec<GenJob>> {
    let mut stmt =
        conn.prepare(&format!("{SELECT} WHERE (?1 IS NULL OR j.status = ?1) ORDER BY j.created_at DESC LIMIT ?2"))?;
    let rows = stmt.query_map(params![status.map(|s| s.as_str()), limit], row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 开始执行：记录本次使用的模型与上下文摘要，尝试次数 +1。
pub fn mark_running(conn: &Connection, id: &str, provider: &str, model: &str, digest: &str, template: &str) -> Result<()> {
    let n = conn.execute(
        "UPDATE gen_jobs SET status = 'running', attempts = attempts + 1, provider = ?2, model = ?3,
            context_digest = ?4, template_version = ?5, error = NULL, note = '', updated_at = ?6
         WHERE id = ?1 AND status IN ('queued', 'failed', 'cancelled')",
        params![id, provider, model, digest, template, now_ms()],
    )?;
    if n == 0 {
        return Err(CoreError::Invalid("任务正在运行或已成功，不能重复执行".into()));
    }
    Ok(())
}

pub fn set_note(conn: &Connection, id: &str, note: &str) -> Result<()> {
    conn.execute("UPDATE gen_jobs SET note = ?2, updated_at = ?3 WHERE id = ?1", params![id, note, now_ms()])?;
    Ok(())
}

pub fn finish(conn: &Connection, id: &str, status: JobStatus, raw: Option<&str>, error: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE gen_jobs SET status = ?2, raw_output = COALESCE(?3, raw_output), error = ?4, updated_at = ?5 WHERE id = ?1",
        params![id, status.as_str(), raw, error, now_ms()],
    )?;
    Ok(())
}

/// 程序重启时调用：上次中断时仍在运行的任务标记为失败，可重试。
pub fn recover_interrupted(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE gen_jobs SET status = 'failed', error = '程序在任务运行中退出，任务已中断，可重试', updated_at = ?1
         WHERE status = 'running'",
        [now_ms()],
    )?)
}

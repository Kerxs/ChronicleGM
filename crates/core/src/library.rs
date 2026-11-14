//! 资料库：应用数据目录、世界索引与设置。

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::db::{self, LIBRARY_MIGRATIONS};
use crate::error::{CoreError, Result};
use crate::util::{new_id, now_ms};
use crate::world::{WORLD_FILE_EXT, World};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct WorldInfo {
    pub id: String,
    pub name: String,
    pub file_name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_opened_at: Option<i64>,
}

/// 一次模型请求的元数据（不含提示词与输出内容）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct LlmRequestRecord {
    pub provider_id: String,
    pub model: String,
    pub task: String,
    pub job_id: Option<String>,
    pub ok: bool,
    pub attempts: i64,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub latency_ms: Option<i64>,
    /// 已脱敏的错误信息。
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ProviderStats {
    pub provider_id: String,
    pub requests: i64,
    pub failures: i64,
    pub avg_latency_ms: f64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
}

pub struct Library {
    conn: Connection,
    data_dir: PathBuf,
}

fn info_row(r: &rusqlite::Row) -> rusqlite::Result<WorldInfo> {
    Ok(WorldInfo {
        id: r.get("id")?,
        name: r.get("name")?,
        file_name: r.get("file_name")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
        last_opened_at: r.get("last_opened_at")?,
    })
}

impl Library {
    pub fn open(data_dir: &Path) -> Result<Library> {
        for sub in ["worlds", "backups", "logs"] {
            std::fs::create_dir_all(data_dir.join(sub))?;
        }
        let mut conn = db::open_connection(&data_dir.join("library.db"))?;
        db::migrate(&mut conn, LIBRARY_MIGRATIONS, &data_dir.join("backups"), "library")?;
        Ok(Library { conn, data_dir: data_dir.to_path_buf() })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
    pub fn worlds_dir(&self) -> PathBuf {
        self.data_dir.join("worlds")
    }
    pub fn backups_dir(&self) -> PathBuf {
        self.data_dir.join("backups")
    }

    pub fn list_worlds(&self) -> Result<Vec<WorldInfo>> {
        let mut stmt =
            self.conn.prepare("SELECT * FROM worlds ORDER BY COALESCE(last_opened_at, created_at) DESC")?;
        let rows = stmt.query_map([], info_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn get_world(&self, id: &str) -> Result<WorldInfo> {
        self.conn
            .query_row("SELECT * FROM worlds WHERE id = ?1", [id], info_row)
            .optional()?
            .ok_or_else(|| CoreError::NotFound("世界", id.into()))
    }

    pub fn world_path(&self, info: &WorldInfo) -> PathBuf {
        self.worlds_dir().join(&info.file_name)
    }

    pub fn create_world(&self, name: &str) -> Result<(WorldInfo, World)> {
        if name.trim().is_empty() {
            return Err(CoreError::Invalid("世界名称不能为空".into()));
        }
        let id = new_id();
        let file_name = format!("{id}.{WORLD_FILE_EXT}");
        let path = self.worlds_dir().join(&file_name);
        let world = World::create(&path, &self.backups_dir(), name.trim())?;
        let now = now_ms();
        let info = WorldInfo {
            id,
            name: name.trim().into(),
            file_name,
            created_at: now,
            updated_at: now,
            last_opened_at: Some(now),
        };
        self.conn.execute(
            "INSERT INTO worlds (id, name, file_name, created_at, updated_at, last_opened_at) VALUES (?1,?2,?3,?4,?5,?6)",
            params![info.id, info.name, info.file_name, info.created_at, info.updated_at, info.last_opened_at],
        )?;
        Ok((info, world))
    }

    pub fn open_world(&self, id: &str) -> Result<World> {
        let info = self.get_world(id)?;
        let path = self.world_path(&info);
        if !path.exists() {
            return Err(CoreError::NotFound("世界文件", path.display().to_string()));
        }
        let (world, _) = World::open(&path, &self.backups_dir())?;
        self.conn.execute("UPDATE worlds SET last_opened_at = ?2 WHERE id = ?1", params![id, now_ms()])?;
        Ok(world)
    }

    pub fn rename_world(&self, id: &str, name: &str) -> Result<()> {
        if name.trim().is_empty() {
            return Err(CoreError::Invalid("世界名称不能为空".into()));
        }
        let world = self.open_world(id)?;
        world.set_meta("name", name.trim())?;
        self.conn.execute(
            "UPDATE worlds SET name = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, name.trim(), now_ms()],
        )?;
        Ok(())
    }

    /// 从导入包新建一个世界（ID 保持不变，与现有世界互不影响）。
    pub fn import_world(&self, bundle: &Path, name: Option<&str>) -> Result<(WorldInfo, World, crate::bundle::ImportReport)> {
        let (manifest, data) = crate::bundle::read_bundle(bundle)?;
        let display = name.map(str::to_string).unwrap_or_else(|| manifest.world_name.clone());
        let (info, mut world) = self.create_world(&display)?;
        let report = crate::bundle::import_data_with_images(&mut world, &manifest, data, crate::bundle::ConflictStrategy::Skip, &crate::model::Author::System).and_then(|(mut r, images)| {
            r.images = crate::bundle::import_images(&world, bundle, &images, crate::bundle::ConflictStrategy::Skip)?;
            Ok(r)
        });
        match report {
            Ok(r) => Ok((info, world, r)),
            Err(e) => {
                // 导入失败：撤销登记，新建的空世界文件移入备份目录（不删除）。
                let path = self.world_path(&info);
                drop(world);
                self.conn.execute("DELETE FROM worlds WHERE id = ?1", [&info.id])?;
                let _ = std::fs::rename(&path, self.backups_dir().join(format!("failed-import-{}", info.file_name)));
                Err(e)
            }
        }
    }

    // ---------- 模型服务配置 ----------

    /// 保存（新增或覆盖）一个模型服务配置。配置是 JSON，Key 不在其中。
    pub fn save_provider(&self, id: &str, config: &serde_json::Value) -> Result<()> {
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO providers (id, config, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(id) DO UPDATE SET config = excluded.config, updated_at = excluded.updated_at",
            params![id, config.to_string(), now],
        )?;
        Ok(())
    }

    pub fn providers(&self) -> Result<Vec<serde_json::Value>> {
        let mut stmt = self.conn.prepare("SELECT config FROM providers ORDER BY created_at")?;
        let rows = stmt.query_map([], |r| crate::entity::jget(r, "config"))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn provider(&self, id: &str) -> Result<Option<serde_json::Value>> {
        Ok(self
            .conn
            .query_row("SELECT config FROM providers WHERE id = ?1", [id], |r| crate::entity::jget(r, "config"))
            .optional()?)
    }

    /// 删除配置（相关任务路由一并移除）。不涉及任何模型文件。
    pub fn delete_provider(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM providers WHERE id = ?1", [id])?;
        Ok(())
    }

    /// 托管模式的模型程序配置（JSON）。
    pub fn managed_models(&self) -> Result<Vec<serde_json::Value>> {
        let mut stmt = self.conn.prepare("SELECT config FROM managed_models ORDER BY created_at")?;
        let rows = stmt.query_map([], |r| crate::entity::jget(r, "config"))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn save_managed_model(&self, id: &str, config: &serde_json::Value) -> Result<()> {
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO managed_models (id, config, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(id) DO UPDATE SET config = excluded.config, updated_at = excluded.updated_at",
            params![id, config.to_string(), now],
        )?;
        Ok(())
    }

    pub fn delete_managed_model(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM managed_models WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn set_route(&self, task: &str, provider_id: Option<&str>) -> Result<()> {
        match provider_id {
            Some(p) => {
                if self.provider(p)?.is_none() {
                    return Err(CoreError::NotFound("模型服务", p.into()));
                }
                self.conn.execute(
                    "INSERT INTO task_routes (task, provider_id) VALUES (?1, ?2)
                     ON CONFLICT(task) DO UPDATE SET provider_id = excluded.provider_id",
                    params![task, p],
                )?;
            }
            None => {
                self.conn.execute("DELETE FROM task_routes WHERE task = ?1", [task])?;
            }
        }
        Ok(())
    }

    pub fn routes(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare("SELECT task, provider_id FROM task_routes ORDER BY task")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 某任务使用的模型服务：先查该任务的路由，再回落到 `default` 路由。
    pub fn route_for(&self, task: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT provider_id FROM task_routes WHERE task = ?1 UNION ALL
                 SELECT provider_id FROM task_routes WHERE task = 'default' LIMIT 1",
                [task],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// 只看这个任务自己的路由，不回退到默认（语音不能用对话模型）。
    pub fn route_exact(&self, task: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT provider_id FROM task_routes WHERE task = ?1", [task], |r| r.get(0)).optional()?)
    }

    pub fn log_request(&self, rec: &LlmRequestRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO llm_requests (provider_id, model, task, job_id, ok, attempts, prompt_tokens, completion_tokens,
                latency_ms, error, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                rec.provider_id, rec.model, rec.task, rec.job_id, rec.ok, rec.attempts, rec.prompt_tokens,
                rec.completion_tokens, rec.latency_ms, rec.error, now_ms()
            ],
        )?;
        Ok(())
    }

    pub fn request_stats(&self) -> Result<Vec<ProviderStats>> {
        let mut stmt = self.conn.prepare(
            "SELECT provider_id, COUNT(*), SUM(ok = 0), AVG(latency_ms), SUM(COALESCE(prompt_tokens,0)),
                SUM(COALESCE(completion_tokens,0)) FROM llm_requests GROUP BY provider_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ProviderStats {
                provider_id: r.get(0)?,
                requests: r.get(1)?,
                failures: r.get(2)?,
                avg_latency_ms: r.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                prompt_tokens: r.get(4)?,
                completion_tokens: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

//! 世界树节点的图片：保存、列出、取数据、说明文字、设为封面、删除。
//!
//! 图片以二进制存在世界文件里（`node_images` 表）。界面在上传前先缩放（长边不超过 2048 像素）；
//! 这里再做格式与大小的校验，并核对文件头确实是声明的图片格式。

use base64::Engine as _;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::util::{new_id, now_ms};

/// 单张图片的上限（缩放后的大小）。
pub const MAX_IMAGE_BYTES: usize = 12 * 1024 * 1024;
/// 每个节点最多的图片数。
pub const MAX_IMAGES_PER_NODE: i64 = 40;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NodeImage {
    pub id: String,
    pub node_id: String,
    pub mime: String,
    pub width: i64,
    pub height: i64,
    pub caption: String,
    /// 排序：最小的是封面。
    pub sort: i64,
    pub size_bytes: i64,
    pub created_at: i64,
}

fn row(r: &rusqlite::Row) -> rusqlite::Result<NodeImage> {
    Ok(NodeImage {
        id: r.get("id")?,
        node_id: r.get("node_id")?,
        mime: r.get("mime")?,
        width: r.get("width")?,
        height: r.get("height")?,
        caption: r.get("caption")?,
        sort: r.get("sort")?,
        size_bytes: r.get("size_bytes")?,
        created_at: r.get("created_at")?,
    })
}

const COLS: &str = "id, node_id, mime, width, height, caption, sort, length(bytes) AS size_bytes, created_at";

/// 按文件头判断图片格式（不信任调用方声明的类型）。
pub fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else {
        None
    }
}

/// 给节点添加一张图片。`width`/`height` 由界面在缩放时测得（只用于显示）。
pub fn add_image(conn: &Connection, node_id: &str, bytes: &[u8], width: i64, height: i64, caption: &str) -> Result<NodeImage> {
    if bytes.is_empty() {
        return Err(CoreError::Invalid("图片是空的".into()));
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(CoreError::Invalid(format!("图片太大（{:.1} MB），上限 {} MB", bytes.len() as f64 / 1048576.0, MAX_IMAGE_BYTES >> 20)));
    }
    let mime = sniff_mime(bytes).ok_or_else(|| CoreError::Invalid("不支持的图片格式（支持 PNG、JPEG、WebP、GIF）".into()))?;
    let alive: Option<i64> = conn.query_row("SELECT 1 FROM nodes WHERE id = ?1 AND deleted_at IS NULL", [node_id], |r| r.get(0)).optional()?;
    if alive.is_none() {
        return Err(CoreError::NotFound("节点", node_id.into()));
    }
    let (count, max_sort): (i64, Option<i64>) = conn.query_row("SELECT COUNT(*), MAX(sort) FROM node_images WHERE node_id = ?1", [node_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if count >= MAX_IMAGES_PER_NODE {
        return Err(CoreError::Invalid(format!("每个节点最多 {MAX_IMAGES_PER_NODE} 张图片")));
    }
    let id = new_id();
    conn.execute(
        "INSERT INTO node_images (id, node_id, mime, width, height, bytes, caption, sort, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![id, node_id, mime, width.max(0), height.max(0), bytes, caption.trim().chars().take(500).collect::<String>(), max_sort.map(|s| s + 1).unwrap_or(0), now_ms()],
    )?;
    get_image(conn, &id)
}

pub fn get_image(conn: &Connection, id: &str) -> Result<NodeImage> {
    conn.query_row(&format!("SELECT {COLS} FROM node_images WHERE id = ?1"), [id], row)
        .optional()?
        .ok_or_else(|| CoreError::NotFound("图片", id.into()))
}

pub fn list_images(conn: &Connection, node_id: &str) -> Result<Vec<NodeImage>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM node_images WHERE node_id = ?1 ORDER BY sort, created_at"))?;
    let rows = stmt.query_map([node_id], row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 图片的原始数据与格式。
pub fn image_data(conn: &Connection, id: &str) -> Result<(String, Vec<u8>)> {
    conn.query_row("SELECT mime, bytes FROM node_images WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .ok_or_else(|| CoreError::NotFound("图片", id.into()))
}

/// `data:` URL（界面显示与发给视觉模型用）。
pub fn data_url(conn: &Connection, id: &str) -> Result<String> {
    let (mime, bytes) = image_data(conn, id)?;
    Ok(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

pub fn set_caption(conn: &Connection, id: &str, caption: &str) -> Result<()> {
    let n = conn.execute("UPDATE node_images SET caption = ?2 WHERE id = ?1", params![id, caption.trim().chars().take(500).collect::<String>()])?;
    if n == 0 {
        return Err(CoreError::NotFound("图片", id.into()));
    }
    Ok(())
}

/// 设为封面（排到最前）。
pub fn set_cover(conn: &Connection, id: &str) -> Result<()> {
    let img = get_image(conn, id)?;
    let min: i64 = conn.query_row("SELECT COALESCE(MIN(sort), 0) FROM node_images WHERE node_id = ?1", [&img.node_id], |r| r.get(0))?;
    conn.execute("UPDATE node_images SET sort = ?2 WHERE id = ?1", params![id, min - 1])?;
    Ok(())
}

pub fn delete_image(conn: &Connection, id: &str) -> Result<()> {
    let n = conn.execute("DELETE FROM node_images WHERE id = ?1", [id])?;
    if n == 0 {
        return Err(CoreError::NotFound("图片", id.into()));
    }
    Ok(())
}

/// 一批节点的封面图片 ID（节点 → 封面），树和图谱显示缩略图用。
pub fn covers(conn: &Connection, node_ids: &[String]) -> Result<Vec<(String, String)>> {
    let mut out = vec![];
    let mut stmt = conn.prepare("SELECT id FROM node_images WHERE node_id = ?1 ORDER BY sort, created_at LIMIT 1")?;
    for n in node_ids {
        if let Some(id) = stmt.query_row([n], |r| r.get::<_, String>(0)).optional()? {
            out.push((n.clone(), id));
        }
    }
    Ok(out)
}

/// 缩略图上限：长边 256 像素的 JPEG 一般 10–30 KB。
pub const MAX_THUMB_BYTES: usize = 256 * 1024;

/// 保存缩略图（界面生成）。只接受 JPEG / PNG / WebP。
pub fn set_thumb(conn: &Connection, id: &str, bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() || bytes.len() > MAX_THUMB_BYTES {
        return Err(CoreError::Invalid(format!("缩略图大小不对（{} 字节，上限 {} KB）", bytes.len(), MAX_THUMB_BYTES >> 10)));
    }
    if !matches!(sniff_mime(bytes), Some("image/jpeg" | "image/png" | "image/webp")) {
        return Err(CoreError::Invalid("缩略图只支持 JPEG、PNG、WebP".into()));
    }
    let n = conn.execute("UPDATE node_images SET thumb = ?2 WHERE id = ?1", params![id, bytes])?;
    if n == 0 {
        return Err(CoreError::NotFound("图片", id.into()));
    }
    Ok(())
}

fn thumb_url(b: Option<Vec<u8>>) -> Option<String> {
    b.and_then(|b| sniff_mime(&b).map(|m| format!("data:{m};base64,{}", base64::engine::general_purpose::STANDARD.encode(&b))))
}

/// 一张图片的缩略图；`thumb` 为空表示还没生成（界面会用原图生成并保存）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ImageThumb {
    pub node_id: String,
    pub image_id: String,
    pub thumb: Option<String>,
    pub caption: String,
    /// 这个节点共有几张图片（只在封面查询里有意义）。
    pub count: i64,
}

/// 一批节点的封面缩略图（树、图谱、游戏页用）。没有图片的节点不出现在结果里。
pub fn cover_thumbs(conn: &Connection, node_ids: &[String]) -> Result<Vec<ImageThumb>> {
    let mut out = vec![];
    let mut stmt = conn.prepare(
        "SELECT id, thumb, caption, (SELECT COUNT(*) FROM node_images x WHERE x.node_id = ?1) FROM node_images WHERE node_id = ?1 ORDER BY sort, created_at LIMIT 1",
    )?;
    for n in node_ids {
        if let Some((image_id, thumb, caption, count)) =
            stmt.query_row([n], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<Vec<u8>>>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?))).optional()?
        {
            out.push(ImageThumb { node_id: n.clone(), image_id, thumb: thumb_url(thumb), caption, count });
        }
    }
    Ok(out)
}

/// 一个节点全部图片的缩略图（按封面在前的顺序）。
pub fn node_thumbs(conn: &Connection, node_id: &str) -> Result<Vec<ImageThumb>> {
    let mut stmt = conn.prepare("SELECT id, thumb, caption FROM node_images WHERE node_id = ?1 ORDER BY sort, created_at")?;
    let rows = stmt.query_map([node_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<Vec<u8>>>(1)?, r.get::<_, String>(2)?)))?;
    let rows: Vec<_> = rows.collect::<rusqlite::Result<_>>()?;
    let count = rows.len() as i64;
    Ok(rows.into_iter().map(|(image_id, thumb, caption)| ImageThumb { node_id: node_id.into(), image_id, thumb: thumb_url(thumb), caption, count }).collect())
}

/// 有图片的节点（未删除），游戏页按名称匹配叙述用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct IllustratedNode {
    pub node_id: String,
    pub title: String,
    pub aliases: Vec<String>,
    pub kind: String,
    pub count: i64,
}

pub fn illustrated_nodes(conn: &Connection) -> Result<Vec<IllustratedNode>> {
    let mut stmt = conn.prepare(
        "SELECT n.id, n.title, n.aliases, n.kind, COUNT(i.id) FROM nodes n JOIN node_images i ON i.node_id = n.id
         WHERE n.deleted_at IS NULL GROUP BY n.id ORDER BY n.title",
    )?;
    let rows = stmt.query_map([], |r| {
        let aliases: String = r.get(2)?;
        Ok(IllustratedNode {
            node_id: r.get(0)?,
            title: r.get(1)?,
            aliases: serde_json::from_str(&aliases).unwrap_or_default(),
            kind: r.get(3)?,
            count: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 导出包里每张图片的元数据（图片本体另存为 `images/<id>.<扩展名>`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ImageEntry {
    pub id: String,
    pub node_id: String,
    pub mime: String,
    pub width: i64,
    pub height: i64,
    pub caption: String,
    pub sort: i64,
    pub created_at: i64,
    /// 压缩包内的文件名。
    pub file: String,
}

pub fn ext_for(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "bin",
    }
}

/// 列出（未删除节点的）全部图片元数据。
pub fn all_entries(conn: &Connection, include_deleted: bool) -> Result<Vec<ImageEntry>> {
    let sql = format!(
        "SELECT i.id, i.node_id, i.mime, i.width, i.height, i.caption, i.sort, i.created_at FROM node_images i JOIN nodes n ON n.id = i.node_id {} ORDER BY i.node_id, i.sort",
        if include_deleted { "" } else { "WHERE n.deleted_at IS NULL" }
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| {
        let id: String = r.get(0)?;
        let mime: String = r.get(2)?;
        Ok(ImageEntry { file: format!("images/{id}.{}", ext_for(&mime)), id, node_id: r.get(1)?, mime, width: r.get(3)?, height: r.get(4)?, caption: r.get(5)?, sort: r.get(6)?, created_at: r.get(7)? })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 导入一张图片（导入包用）：节点不存在时跳过；`keep_id` 时保留原 ID（同 ID 已存在则按 `overwrite` 跳过或替换）。
pub fn import_entry(conn: &Connection, e: &ImageEntry, bytes: &[u8], keep_id: bool, overwrite: bool) -> Result<bool> {
    let node: Option<i64> = conn.query_row("SELECT 1 FROM nodes WHERE id = ?1", [&e.node_id], |r| r.get(0)).optional()?;
    if node.is_none() || bytes.len() > MAX_IMAGE_BYTES {
        return Ok(false);
    }
    let Some(mime) = sniff_mime(bytes) else { return Ok(false) };
    let id = if keep_id { e.id.clone() } else { new_id() };
    if keep_id {
        let exists: Option<i64> = conn.query_row("SELECT 1 FROM node_images WHERE id = ?1", [&id], |r| r.get(0)).optional()?;
        if exists.is_some() {
            if !overwrite {
                return Ok(false);
            }
            conn.execute("DELETE FROM node_images WHERE id = ?1", [&id])?;
        }
    }
    conn.execute(
        "INSERT INTO node_images (id, node_id, mime, width, height, bytes, caption, sort, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![id, e.node_id, mime, e.width, e.height, bytes, e.caption, e.sort, e.created_at],
    )?;
    Ok(true)
}

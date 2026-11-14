//! 世界导入导出：带版本号的 zip 包（manifest.json + world.json）。
//!
//! 导入策略：
//! - `Skip`：ID 已存在则跳过；
//! - `Overwrite`：覆盖已存在的同 ID 内容（锁定内容除外），可撤销；
//! - `Copy`：所有 ID 重新生成，作为副本导入，不与已有内容冲突。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::entity::Entity;
use crate::error::{CoreError, Result};
use crate::model::*;
use crate::payload::{validate_node_payload, validate_script_item_payload};
use crate::util::{is_valid_sort_key, new_id, now_ms};
use crate::world::{World, Writer};

pub const BUNDLE_FORMAT: &str = "chronicle-gm-world";
pub const BUNDLE_FORMAT_VERSION: u32 = 1;
const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct BundleManifest {
    pub format: String,
    pub format_version: u32,
    pub app_version: String,
    pub exported_at: i64,
    pub world_id: String,
    pub world_name: String,
    pub counts: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct BundleData {
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub relations: Vec<Relation>,
    #[serde(default)]
    pub scripts: Vec<Script>,
    #[serde(default)]
    pub script_items: Vec<ScriptItem>,
    #[serde(default)]
    pub script_cast: Vec<CastMember>,
    #[serde(default)]
    pub plot_nodes: Vec<PlotNode>,
    #[serde(default)]
    pub plot_edges: Vec<PlotEdge>,
    /// 节点图片的元数据（图片本体是压缩包里的 `images/` 文件）。旧版导入包没有这一项。
    #[serde(default)]
    pub images: Vec<crate::images::ImageEntry>,
}

fn load_all<E: Entity>(world: &World, include_deleted: bool) -> Result<Vec<E>> {
    let sql = format!(
        "SELECT * FROM {} {} ORDER BY rowid",
        E::NAME,
        if include_deleted { "" } else { "WHERE deleted_at IS NULL" }
    );
    let mut stmt = world.conn().prepare(&sql)?;
    let rows = stmt.query_map([], |r| E::from_row(r))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 导出整个世界内容（不含战役进度）。目标文件已存在时报错。
pub fn export_bundle(world: &World, dest: &Path, include_deleted: bool) -> Result<BundleManifest> {
    if dest.exists() {
        return Err(CoreError::Invalid(format!("目标文件已存在: {}", dest.display())));
    }
    let mut data = BundleData {
        nodes: load_all(world, include_deleted)?,
        relations: load_all(world, include_deleted)?,
        scripts: load_all(world, include_deleted)?,
        script_items: load_all(world, include_deleted)?,
        script_cast: load_all(world, include_deleted)?,
        plot_nodes: load_all(world, include_deleted)?,
        plot_edges: load_all(world, include_deleted)?,
        images: crate::images::all_entries(world.conn(), include_deleted)?,
    };
    // 父节点在前，便于导入。
    data.nodes.sort_by(|a, b| a.path.matches('/').count().cmp(&b.path.matches('/').count()).then(a.path.cmp(&b.path)));
    let counts = BTreeMap::from([
        ("nodes".to_string(), data.nodes.len()),
        ("relations".to_string(), data.relations.len()),
        ("scripts".to_string(), data.scripts.len()),
        ("script_items".to_string(), data.script_items.len()),
        ("script_cast".to_string(), data.script_cast.len()),
        ("plot_nodes".to_string(), data.plot_nodes.len()),
        ("plot_edges".to_string(), data.plot_edges.len()),
        ("images".to_string(), data.images.len()),
    ]);
    let manifest = BundleManifest {
        format: BUNDLE_FORMAT.into(),
        format_version: BUNDLE_FORMAT_VERSION,
        app_version: env!("CARGO_PKG_VERSION").into(),
        exported_at: now_ms(),
        world_id: world.meta("world_id")?.unwrap_or_default(),
        world_name: world.name()?,
        counts,
    };

    // 先写临时文件，成功后再改名，避免留下半个文件。
    let tmp = dest.with_extension("partial");
    if tmp.exists() {
        return Err(CoreError::Invalid(format!("临时文件已存在: {}", tmp.display())));
    }
    let res = (|| -> Result<()> {
        let file = std::fs::File::create(&tmp)?;
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("manifest.json", opts)?;
        zip.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
        zip.start_file("world.json", opts)?;
        serde_json::to_writer(&mut zip, &data)?;
        // 图片本体（已是压缩格式，存储时不再压缩）。
        let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for e in &data.images {
            let (_, bytes) = crate::images::image_data(world.conn(), &e.id)?;
            zip.start_file(&e.file, stored)?;
            zip.write_all(&bytes)?;
        }
        zip.finish()?;
        std::fs::rename(&tmp, dest)?;
        Ok(())
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res.map(|_| manifest)
}

fn read_entry(zip: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Result<Value> {
    let entry = zip.by_name(name).map_err(|_| CoreError::Invalid(format!("导入包缺少 {name}")))?;
    if entry.size() > MAX_ENTRY_BYTES {
        return Err(CoreError::Invalid(format!("{name} 过大")));
    }
    let mut buf = Vec::new();
    entry.take(MAX_ENTRY_BYTES + 1).read_to_end(&mut buf)?;
    Ok(serde_json::from_slice(&buf)?)
}

/// 读取导入包，必要时逐级升级旧格式。
pub fn read_bundle(path: &Path) -> Result<(BundleManifest, BundleData)> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let manifest = read_entry(&mut zip, "manifest.json")?;
    let data = read_entry(&mut zip, "world.json")?;
    upgrade_bundle(manifest, data)
}

/// 格式升级入口。新版本格式在这里追加 `vN -> vN+1` 的转换。
pub fn upgrade_bundle(manifest: Value, data: Value) -> Result<(BundleManifest, BundleData)> {
    let format = manifest.get("format").and_then(Value::as_str).unwrap_or_default();
    if format != BUNDLE_FORMAT {
        return Err(CoreError::Invalid(format!("不是 Chronicle GM 世界包（format = {format:?}）")));
    }
    let version = manifest.get("format_version").and_then(Value::as_u64).unwrap_or(0) as u32;
    if version == 0 {
        return Err(CoreError::Invalid("导入包缺少格式版本".into()));
    }
    if version > BUNDLE_FORMAT_VERSION {
        return Err(CoreError::Invalid(format!(
            "导入包格式 v{version} 比程序支持的 v{BUNDLE_FORMAT_VERSION} 新，请升级程序"
        )));
    }
    // 当前只有 v1。
    Ok((serde_json::from_value(manifest)?, serde_json::from_value(data)?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ConflictStrategy {
    Skip,
    Overwrite,
    Copy,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ImportReport {
    pub inserted: usize,
    pub overwritten: usize,
    pub skipped: usize,
    /// 因锁定而未覆盖的数量。
    pub protected: usize,
    pub commit_id: Option<String>,
    /// 导入的节点图片数。
    #[serde(default)]
    pub images: usize,
}

fn remap_value(v: &mut Value, map: &HashMap<String, String>) {
    match v {
        Value::String(s) => {
            if let Some(n) = map.get(s.as_str()) {
                *s = n.clone();
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| remap_value(x, map)),
        Value::Object(o) => o.values_mut().for_each(|x| remap_value(x, map)),
        _ => {}
    }
}

/// 副本导入：所有实体 ID 换新（JSON 中所有等于旧 ID 的字符串一并替换，ULID 不会与普通文本冲突）。
fn remap_all(data: &BundleData) -> Result<BundleData> {
    let mut map = HashMap::new();
    let ids = data
        .nodes
        .iter()
        .map(|e| &e.id)
        .chain(data.relations.iter().map(|e| &e.id))
        .chain(data.scripts.iter().map(|e| &e.id))
        .chain(data.script_items.iter().map(|e| &e.id))
        .chain(data.script_cast.iter().map(|e| &e.id))
        .chain(data.plot_nodes.iter().map(|e| &e.id))
        .chain(data.plot_edges.iter().map(|e| &e.id));
    for id in ids {
        map.insert(id.clone(), new_id());
    }
    let mut v = serde_json::to_value(data)?;
    remap_value(&mut v, &map);
    Ok(serde_json::from_value(v)?)
}

fn validate_entities(data: &BundleData) -> Result<()> {
    for n in &data.nodes {
        validate_node_payload(n.kind, &n.payload)?;
        if !is_valid_sort_key(&n.sort_key) {
            return Err(CoreError::Invalid(format!("节点 {} 的排序键无效", n.id)));
        }
    }
    for it in &data.script_items {
        validate_script_item_payload(it.kind, &it.payload)?;
        if !is_valid_sort_key(&it.sort_key) {
            return Err(CoreError::Invalid(format!("剧本条目 {} 的排序键无效", it.id)));
        }
    }
    for e in &data.plot_edges {
        if let Some(c) = &e.condition {
            c.validate()?;
        }
        for eff in &e.effects {
            eff.validate()?;
        }
    }
    Ok(())
}

/// 节点按"父节点先于子节点"排序（不依赖导出时的 path）。
fn order_nodes(nodes: &[Node]) -> Result<Vec<Node>> {
    let by_id: HashMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut out = Vec::with_capacity(nodes.len());
    let mut done: HashSet<&str> = HashSet::new();
    for n in nodes {
        let mut chain = vec![];
        let mut cur = Some(n);
        let mut guard = 0;
        while let Some(c) = cur {
            if done.contains(c.id.as_str()) {
                break;
            }
            chain.push(c);
            guard += 1;
            if guard > nodes.len() {
                return Err(CoreError::Invalid("导入包中的节点父子关系存在循环".into()));
            }
            cur = c.parent_id.as_deref().and_then(|p| by_id.get(p).copied());
        }
        for c in chain.into_iter().rev() {
            if done.insert(c.id.as_str()) {
                out.push(c.clone());
            }
        }
    }
    Ok(out)
}

trait Importable: Entity {
    fn locked(&self) -> bool;
    fn prepare(&mut self, _w: &Writer) -> Result<()> {
        Ok(())
    }
}

impl Importable for Node {
    fn locked(&self) -> bool {
        self.locked
    }
    /// 重新计算路径（以世界中实际的父节点为准）。
    fn prepare(&mut self, w: &Writer) -> Result<()> {
        let parent_path = match &self.parent_id {
            Some(p) => Node::load(w.conn(), p)?
                .ok_or_else(|| CoreError::Invalid(format!("节点 {} 的父节点 {p} 不存在", self.id)))?
                .path,
            None => "/".into(),
        };
        self.path = format!("{parent_path}{}/", self.id);
        Ok(())
    }
}
macro_rules! importable {
    ($($t:ty),*) => { $(impl Importable for $t { fn locked(&self) -> bool { self.locked } })* };
}
importable!(ScriptItem, CastMember, PlotNode, PlotEdge);
impl Importable for Relation {
    fn locked(&self) -> bool {
        false
    }
}
impl Importable for Script {
    fn locked(&self) -> bool {
        false
    }
}

fn import_rows<E: Importable>(w: &mut Writer, rows: Vec<E>, strategy: ConflictStrategy, rep: &mut ImportReport) -> Result<()> {
    for mut row in rows {
        row.prepare(w)?;
        match E::load(w.conn(), row.id())? {
            None => {
                row.set_rev(1);
                w.put_new(&row)?;
                rep.inserted += 1;
            }
            Some(existing) => match strategy {
                ConflictStrategy::Skip | ConflictStrategy::Copy => rep.skipped += 1,
                ConflictStrategy::Overwrite => {
                    if existing.locked() {
                        rep.protected += 1;
                    } else {
                        w.put_update(&existing, row)?;
                        rep.overwritten += 1;
                    }
                }
            },
        }
    }
    Ok(())
}

/// 把导入包合并进现有世界。整个导入在一个事务中，失败不留下任何修改，成功后可撤销。
pub fn import_bundle(world: &mut World, path: &Path, strategy: ConflictStrategy, author: &Author) -> Result<ImportReport> {
    let (manifest, data) = read_bundle(path)?;
    let (mut rep, images) = import_data_with_images(world, &manifest, data, strategy, author)?;
    rep.images = import_images(world, path, &images, strategy)?;
    Ok(rep)
}

/// 导入包里的节点图片。`images` 是导入内容实际使用的元数据（复制模式下 `node_id` 已换成新节点）。
pub fn import_images(world: &World, path: &Path, images: &[crate::images::ImageEntry], strategy: ConflictStrategy) -> Result<usize> {
    if images.is_empty() {
        return Ok(0);
    }
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut n = 0;
    let tx = world.conn().unchecked_transaction()?;
    for e in images {
        let Ok(mut entry) = zip.by_name(&e.file) else { continue };
        if entry.size() > crate::images::MAX_IMAGE_BYTES as u64 {
            continue;
        }
        let mut buf = Vec::new();
        (&mut entry).take(crate::images::MAX_IMAGE_BYTES as u64 + 1).read_to_end(&mut buf)?;
        if crate::images::import_entry(&tx, e, &buf, strategy != ConflictStrategy::Copy, strategy == ConflictStrategy::Overwrite)? {
            n += 1;
        }
    }
    tx.commit()?;
    Ok(n)
}

pub fn import_data(
    world: &mut World,
    manifest: &BundleManifest,
    data: BundleData,
    strategy: ConflictStrategy,
    author: &Author,
) -> Result<ImportReport> {
    import_data_with_images(world, manifest, data, strategy, author).map(|(r, _)| r)
}

/// 同 [`import_data`]，并返回实际使用的图片元数据（复制模式下已重新编号），供 [`import_images`] 使用。
pub fn import_data_with_images(
    world: &mut World,
    manifest: &BundleManifest,
    data: BundleData,
    strategy: ConflictStrategy,
    author: &Author,
) -> Result<(ImportReport, Vec<crate::images::ImageEntry>)> {
    validate_entities(&data)?;
    let data = if strategy == ConflictStrategy::Copy { remap_all(&data)? } else { data };
    let images = data.images.clone();
    let nodes = order_nodes(&data.nodes)?;
    let msg = format!("导入「{}」（{:?}）", manifest.world_name, strategy);
    let (mut rep, commit) = world.write(author, &msg, |w| {
        let mut rep = ImportReport::default();
        import_rows(w, nodes, strategy, &mut rep)?;
        import_rows(w, data.relations, strategy, &mut rep)?;
        import_rows(w, data.scripts, strategy, &mut rep)?;
        // 剧本条目也需要父条目先导入。
        let mut items = data.script_items;
        items.sort_by_key(|it| it.parent_id.is_some());
        let mut ordered = Vec::with_capacity(items.len());
        let mut placed: HashSet<String> = HashSet::new();
        let mut remaining = items;
        while !remaining.is_empty() {
            let before = remaining.len();
            let (ready, rest): (Vec<_>, Vec<_>) = remaining.into_iter().partition(|it| {
                it.parent_id.as_ref().is_none_or(|p| placed.contains(p) || ScriptItem::load(w.conn(), p).ok().flatten().is_some())
            });
            for it in &ready {
                placed.insert(it.id.clone());
            }
            ordered.extend(ready);
            remaining = rest;
            if remaining.len() == before {
                return Err(CoreError::Invalid("导入包中的剧本条目父子关系无效".into()));
            }
        }
        import_rows(w, ordered, strategy, &mut rep)?;
        import_rows(w, data.script_cast, strategy, &mut rep)?;
        import_rows(w, data.plot_nodes, strategy, &mut rep)?;
        import_rows(w, data.plot_edges, strategy, &mut rep)?;
        Ok(rep)
    })?;
    rep.commit_id = commit;
    Ok((rep, images))
}

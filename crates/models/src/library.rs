//! 本地模型库：记录有哪些模型文件、它们的信息、关联的 mmproj、默认模型。
//!
//! - 添加已有 GGUF 文件（不复制，记录原位置）；下载完成的模型自动加入。
//! - 删除记录（文件保留）或删除文件（界面上必须二次确认；正在使用的不能删；只删模型目录里的文件）。
//! - 修改模型保存位置：把模型目录里的文件移到新位置（外部添加的文件不动）。
//! - 完整性检查：文件还在、大小没变、GGUF 头能读。
//! - 存在 `library.json`（原子写入）。

use std::path::{Path, PathBuf};

use chronicle_llm::gguf::{GgufInfo, read_gguf};
use serde::{Deserialize, Serialize};

use crate::LogFn;
use crate::download::Task;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size: u64,
    /// hf / modelscope / local / url
    pub source: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub quant: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub context_length: Option<u64>,
    #[serde(default)]
    pub size_label: Option<String>,
    #[serde(default)]
    pub mmproj: Option<String>,
    pub added_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LibraryData {
    pub models_dir: String,
    pub default: Option<String>,
    pub models: Vec<ModelEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Integrity {
    Ok,
    Missing,
    SizeChanged { expected: u64, actual: u64 },
    Corrupt { error: String },
}

pub struct Library {
    path: PathBuf,
    pub data: LibraryData,
    log: LogFn,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn norm(p: &Path) -> String {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).display().to_string().trim_start_matches(r"\\?\").to_string()
}

impl Library {
    /// `root`：`数据目录/llama`；`default_models_dir`：第一次使用时的模型目录。
    pub fn open(root: &Path, default_models_dir: &Path, log: LogFn) -> Library {
        let path = root.join("library.json");
        let mut data: LibraryData = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        if data.models_dir.is_empty() {
            data.models_dir = default_models_dir.display().to_string();
        }
        Library { path, data, log }
    }

    pub fn models_dir(&self) -> PathBuf {
        PathBuf::from(&self.data.models_dir)
    }

    pub fn save(&self) -> Result<(), String> {
        if let Some(p) = self.path.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.data).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(tmp, &self.path).map_err(|e| e.to_string())
    }

    pub fn get(&self, id: &str) -> Option<&ModelEntry> {
        self.data.models.iter().find(|m| m.id == id)
    }

    pub fn default_model(&self) -> Option<&ModelEntry> {
        self.data.default.as_deref().and_then(|d| self.get(d)).or_else(|| self.data.models.first())
    }

    fn entry_from(&self, path: &Path, info: &GgufInfo, source: &str, repo: &str) -> ModelEntry {
        let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        ModelEntry {
            id: format!("m{}-{}", now_ms(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)),
            name: info.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| file.trim_end_matches(".gguf").to_string()),
            path: norm(path),
            size: info.file_size,
            source: source.into(),
            repo: repo.into(),
            quant: crate::repo::quant_of(&file).or_else(|| info.quantization.clone()),
            arch: info.architecture.clone(),
            context_length: info.context_length,
            size_label: info.size_label.clone(),
            mmproj: None,
            added_at: now_ms(),
        }
    }

    /// 添加已有的 GGUF 文件（不复制）。同一个文件只记一次。
    pub fn add_file(&mut self, path: &Path, mmproj: Option<&Path>) -> Result<ModelEntry, String> {
        let info = read_gguf(path).map_err(|e| format!("不是有效的 GGUF 文件：{e}"))?;
        if info.is_projector {
            return Err("这是多模态投影文件（mmproj），请在某个模型的详情里关联它，而不是单独添加".into());
        }
        if let Some((i, n)) = info.split
            && i != 1
        {
            return Err(format!("这是分片模型的第 {i}/{n} 片，请选第 1 片"));
        }
        let key = norm(path);
        if let Some(m) = self.data.models.iter().find(|m| m.path == key) {
            return Ok(m.clone());
        }
        let mut e = self.entry_from(path, &info, "local", "");
        if let Some(p) = mmproj {
            e.mmproj = Some(check_mmproj(p)?);
        }
        (self.log)(&format!("模型库添加：{}", e.path));
        self.data.models.push(e.clone());
        if self.data.default.is_none() {
            self.data.default = Some(e.id.clone());
        }
        self.save()?;
        Ok(e)
    }

    /// 下载任务完成：主模型（第一个非 mmproj、分片取第 1 片）加入模型库，mmproj 自动关联。
    pub fn add_from_task(&mut self, t: &Task) -> Result<Option<ModelEntry>, String> {
        let outs: Vec<(&crate::download::TaskFile, &String)> = t.files.iter().zip(t.outputs.iter()).collect();
        let main = outs.iter().filter(|(f, _)| !f.mmproj).find(|(f, _)| crate::repo::split_of(&f.path).is_none_or(|(i, _)| i == 1)).map(|(_, p)| PathBuf::from(p));
        let proj = outs.iter().find(|(f, _)| f.mmproj).map(|(_, p)| PathBuf::from(p));
        let Some(main) = main else {
            // 只下了 mmproj：没有可以单独加入的模型。
            return Ok(None);
        };
        let key = norm(&main);
        if let Some(i) = self.data.models.iter().position(|m| m.path == key) {
            if let Some(p) = &proj {
                self.data.models[i].mmproj = Some(norm(p));
                self.save()?;
            }
            return Ok(Some(self.data.models[i].clone()));
        }
        let info = read_gguf(&main).map_err(|e| format!("不是有效的 GGUF 文件：{e}"))?;
        let mut e = self.entry_from(&main, &info, &t.source, &t.repo);
        e.mmproj = proj.as_deref().map(norm);
        // 分片模型：大小按全部分片算。
        e.size = outs.iter().filter(|(f, _)| !f.mmproj).map(|(f, _)| f.size).sum::<u64>().max(e.size);
        (self.log)(&format!("模型库添加（下载完成）：{} / {}", t.repo, e.path));
        self.data.models.push(e.clone());
        if self.data.default.is_none() {
            self.data.default = Some(e.id.clone());
        }
        self.save()?;
        Ok(Some(e))
    }

    pub fn set_default(&mut self, id: &str) -> Result<(), String> {
        self.get(id).ok_or("模型不存在")?;
        self.data.default = Some(id.into());
        self.save()
    }

    pub fn link_mmproj(&mut self, id: &str, mmproj: Option<&Path>) -> Result<(), String> {
        let v = match mmproj {
            Some(p) => Some(check_mmproj(p)?),
            None => None,
        };
        let m = self.data.models.iter_mut().find(|m| m.id == id).ok_or("模型不存在")?;
        m.mmproj = v;
        self.save()
    }

    pub fn info(&self, id: &str) -> Result<GgufInfo, String> {
        let m = self.get(id).ok_or("模型不存在")?;
        read_gguf(Path::new(&m.path)).map_err(|e| e.to_string())
    }

    pub fn check(&self, id: &str) -> Integrity {
        let Some(m) = self.get(id) else { return Integrity::Missing };
        let p = Path::new(&m.path);
        let Ok(meta) = std::fs::metadata(p) else { return Integrity::Missing };
        let split = crate::repo::split_of(&m.path).is_some();
        if !split && m.size > 0 && meta.len() != m.size {
            return Integrity::SizeChanged { expected: m.size, actual: meta.len() };
        }
        match read_gguf(p) {
            Ok(_) => Integrity::Ok,
            Err(e) => Integrity::Corrupt { error: e.to_string() },
        }
    }

    /// 删除记录；`delete_files` 时同时删除文件（只删模型目录里的；界面必须先二次确认）。
    /// `in_use`：正在被推理服务使用的模型路径。
    pub fn remove(&mut self, id: &str, delete_files: bool, in_use: &[String]) -> Result<Vec<String>, String> {
        let m = self.get(id).cloned().ok_or("模型不存在")?;
        if in_use.iter().any(|p| norm(Path::new(p)) == m.path) {
            return Err("这个模型正在使用，先停止推理服务再删除".into());
        }
        let mut deleted = vec![];
        if delete_files {
            let root = norm(&self.models_dir());
            let inside = |p: &str| norm(Path::new(p)).starts_with(&root);
            let mut targets = vec![m.path.clone()];
            // 分片：其余分片一起删。
            if let (Some(_), Some(dir)) = (crate::repo::split_of(&m.path), Path::new(&m.path).parent()) {
                let base = m.path.rsplit_once("-of-").map(|x| x.0.rsplit_once('-').map(|y| y.0.to_string()).unwrap_or_default()).unwrap_or_default();
                if let Ok(rd) = std::fs::read_dir(dir) {
                    for e in rd.flatten() {
                        let p = norm(&e.path());
                        if p != m.path && p.starts_with(&base) && crate::repo::split_of(&p).is_some() {
                            targets.push(p);
                        }
                    }
                }
            }
            // mmproj 没有别的模型在用才删。
            if let Some(pj) = &m.mmproj
                && !self.data.models.iter().any(|x| x.id != m.id && x.mmproj.as_deref() == Some(pj))
            {
                targets.push(pj.clone());
            }
            for t in targets {
                if !inside(&t) {
                    (self.log)(&format!("模型文件不在模型目录里，只删记录不删文件：{t}"));
                    continue;
                }
                match std::fs::remove_file(&t) {
                    Ok(()) => deleted.push(t),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(format!("删除 {t} 失败：{e}")),
                }
            }
        }
        self.data.models.retain(|x| x.id != id);
        if self.data.default.as_deref() == Some(id) {
            self.data.default = self.data.models.first().map(|x| x.id.clone());
        }
        (self.log)(&format!("模型库移除：{}（删除文件 {} 个）", m.path, deleted.len()));
        self.save()?;
        Ok(deleted)
    }

    /// 修改模型保存位置：模型目录里的文件移到新目录（保持相对路径）；外部添加的文件不动。
    pub fn move_dir(&mut self, new_dir: &Path) -> Result<usize, String> {
        let old = norm(&self.models_dir());
        std::fs::create_dir_all(new_dir).map_err(|e| format!("建新目录失败：{e}"))?;
        let new_norm = norm(new_dir);
        if new_norm == old {
            return Ok(0);
        }
        if new_norm.starts_with(&format!("{old}{}", std::path::MAIN_SEPARATOR)) {
            return Err("新位置不能在旧模型目录里面".into());
        }
        let mut moved = 0;
        let mut files: Vec<String> = vec![];
        for m in &self.data.models {
            files.push(m.path.clone());
            if let Some(p) = &m.mmproj {
                files.push(p.clone());
            }
        }
        files.sort();
        files.dedup();
        let mut map = std::collections::HashMap::new();
        for f in files {
            let Some(rel) = Path::new(&f).strip_prefix(&old).ok().map(Path::to_path_buf) else { continue };
            let to = new_dir.join(&rel);
            if let Some(p) = to.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            if std::fs::rename(&f, &to).is_err() {
                std::fs::copy(&f, &to).map_err(|e| format!("移动 {f} 失败：{e}"))?;
                std::fs::remove_file(&f).map_err(|e| format!("删除旧文件 {f} 失败：{e}"))?;
            }
            map.insert(f, norm(&to));
            moved += 1;
        }
        for m in self.data.models.iter_mut() {
            if let Some(n) = map.get(&m.path) {
                m.path = n.clone();
            }
            if let Some(p) = m.mmproj.clone()
                && let Some(n) = map.get(&p)
            {
                m.mmproj = Some(n.clone());
            }
        }
        self.data.models_dir = new_norm;
        (self.log)(&format!("模型保存位置改为 {}，移动了 {moved} 个文件", self.data.models_dir));
        self.save()?;
        Ok(moved)
    }
}

fn check_mmproj(p: &Path) -> Result<String, String> {
    let i = read_gguf(p).map_err(|e| format!("mmproj 不是有效的 GGUF 文件：{e}"))?;
    if !i.is_projector && !crate::repo::is_mmproj(&p.to_string_lossy()) {
        return Err("这个文件不是多模态投影文件（mmproj）".into());
    }
    Ok(norm(p))
}

//! 模型仓库适配器：每个仓库一个独立实现，只用它们公开、有文档的接口。
//! 仓库返回的说明文字（模型卡）只当纯文本显示，不会作为程序指令执行。

pub mod hf;
pub mod modelscope;

use serde::{Deserialize, Serialize};

use crate::net::NetError;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelSummary {
    /// hf / modelscope
    pub source: String,
    /// 仓库 ID：`作者/名字`。
    pub id: String,
    pub author: String,
    pub downloads: u64,
    pub likes: u64,
    /// ISO 时间。
    pub updated: String,
    pub license: Option<String>,
    pub tags: Vec<String>,
    /// 需要授权（私有或需要同意条款）。
    pub gated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RepoFile {
    pub path: String,
    pub size: u64,
    pub sha256: Option<String>,
    /// 从文件名识别的量化类型（Q4_K_M、Q8_0、IQ4_XS、F16……）。
    pub quant: Option<String>,
    /// 多模态投影文件（mmproj）。
    pub mmproj: bool,
    /// 分片：（第几片，共几片）。
    pub split: Option<(u32, u32)>,
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelDetail {
    pub summary: ModelSummary,
    /// 只列 GGUF 文件。
    pub files: Vec<RepoFile>,
    /// 模型卡开头（纯文本，最多 2000 字，只用于显示）。
    pub card: String,
}

impl ModelDetail {
    /// 量化版本筛选。
    pub fn with_quant(&self, q: &str) -> Vec<&RepoFile> {
        self.files.iter().filter(|f| f.quant.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(q))).collect()
    }
    /// 所有出现过的量化类型（去重，按名字排）。
    pub fn quants(&self) -> Vec<String> {
        let mut v: Vec<String> = self.files.iter().filter(|f| !f.mmproj).filter_map(|f| f.quant.clone()).collect();
        v.sort();
        v.dedup();
        v
    }
    /// 选中一个文件时实际要下载的全部文件：分片模型的所有分片。
    pub fn expand_selection(&self, path: &str) -> Vec<RepoFile> {
        let Some(f) = self.files.iter().find(|f| f.path == path) else { return vec![] };
        match (f.split, split_base(&f.path)) {
            (Some(_), Some(base)) => {
                let mut v: Vec<RepoFile> = self.files.iter().filter(|x| split_base(&x.path).as_deref() == Some(base.as_str())).cloned().collect();
                v.sort_by_key(|x| x.split.map(|s| s.0).unwrap_or(0));
                v
            }
            _ => vec![f.clone()],
        }
    }
    /// 推荐配套的 mmproj（同目录优先，F16 优先）。
    pub fn suggested_mmproj(&self, model_path: &str) -> Option<&RepoFile> {
        let dir = model_path.rsplit_once('/').map(|x| x.0).unwrap_or("");
        let mut c: Vec<&RepoFile> = self.files.iter().filter(|f| f.mmproj).collect();
        c.sort_by_key(|f| (f.path.rsplit_once('/').map(|x| x.0).unwrap_or("") != dir, !f.path.to_ascii_lowercase().contains("f16"), f.size));
        c.into_iter().next()
    }
}

/// 一个模型仓库。
pub trait RepoAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    /// 搜索（关键词或仓库 ID），只返回含 GGUF 的仓库。
    fn search(&self, query: &str, limit: usize, token: Option<&str>) -> Result<Vec<ModelSummary>, NetError>;
    /// 仓库详情与 GGUF 文件列表。
    fn details(&self, id: &str, token: Option<&str>) -> Result<ModelDetail, NetError>;
}

/// 从文件名识别量化类型。
pub fn quant_of(name: &str) -> Option<String> {
    let stem = name.rsplit('/').next().unwrap_or(name);
    let stem = stem.strip_suffix(".gguf").unwrap_or(stem);
    let up = stem.to_ascii_uppercase();
    let tokens: Vec<&str> = up.split(['.', '-', '_']).collect();
    // 先认多段的（Q4_K_M、IQ4_XS、Q8_0……）：在原串里找。
    const MULTI: &[&str] = &[
        "IQ1_S", "IQ1_M", "IQ2_XXS", "IQ2_XS", "IQ2_S", "IQ2_M", "IQ3_XXS", "IQ3_XS", "IQ3_S", "IQ3_M", "IQ4_XS", "IQ4_NL", "Q2_K_S", "Q2_K_L", "Q2_K", "Q3_K_S", "Q3_K_M", "Q3_K_L", "Q3_K_XL", "Q3_K", "Q4_K_S", "Q4_K_M", "Q4_K_L", "Q4_K_XL", "Q4_K", "Q4_0_4_4",
        "Q4_0", "Q4_1", "Q5_K_S", "Q5_K_M", "Q5_K_L", "Q5_K_XL", "Q5_K", "Q5_0", "Q5_1", "Q6_K_L", "Q6_K_XL", "Q6_K", "Q8_K_XL", "Q8_0", "TQ1_0", "TQ2_0", "MXFP4_MOE", "MXFP4",
    ];
    let mut best: Option<&str> = None;
    for q in MULTI {
        if let Some(i) = up.find(q) {
            let before_ok = i == 0 || !up.as_bytes()[i - 1].is_ascii_alphanumeric();
            let after = up.as_bytes().get(i + q.len());
            let after_ok = after.is_none_or(|c| !c.is_ascii_alphanumeric());
            if before_ok && after_ok && best.is_none_or(|b| q.len() > b.len()) {
                best = Some(q);
            }
        }
    }
    if let Some(b) = best {
        return Some(b.to_string());
    }
    tokens.iter().rev().find(|t| matches!(**t, "F16" | "BF16" | "F32" | "FP16" | "FP8")).map(|t| t.to_string())
}

/// `xxx-00001-of-00003.gguf` → （1, 3）。
pub fn split_of(name: &str) -> Option<(u32, u32)> {
    let stem = name.strip_suffix(".gguf")?;
    let (head, total) = stem.rsplit_once("-of-")?;
    let (_, idx) = head.rsplit_once('-')?;
    Some((idx.parse().ok()?, total.parse().ok()?))
}

fn split_base(name: &str) -> Option<String> {
    split_of(name)?;
    let stem = name.strip_suffix(".gguf")?;
    let (head, _) = stem.rsplit_once("-of-")?;
    Some(head.rsplit_once('-')?.0.to_string())
}

pub fn is_mmproj(name: &str) -> bool {
    let n = name.rsplit('/').next().unwrap_or(name).to_ascii_lowercase();
    n.contains("mmproj") || n.contains("projector")
}

/// 模型卡转纯文本：去掉 YAML 头、HTML 标签，截到 2000 字。只用于显示。
pub fn card_text(md: &str) -> String {
    let mut s = md;
    if let Some(rest) = s.strip_prefix("---")
        && let Some(end) = rest.find("\n---")
    {
        s = &rest[end + 4..];
    }
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().chars().take(2000).collect()
}

/// 把仓库文件构造成 `RepoFile`（只要 GGUF）。
pub fn gguf_file(path: &str, size: u64, sha256: Option<String>, url: String) -> Option<RepoFile> {
    path.to_ascii_lowercase().ends_with(".gguf").then(|| RepoFile { path: path.to_string(), size, sha256, quant: quant_of(path), mmproj: is_mmproj(path), split: split_of(path), url })
}

/// URL 里的路径段编码（中文、空格、#、? 等）。
pub fn enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => o.push(b as char),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

/// 查询参数编码（`/` 也编码）。
pub fn enc_q(s: &str) -> String {
    enc(s).replace('/', "%2F")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_quant_split_and_mmproj() {
        assert_eq!(quant_of("Rocinante-X-12B-v1-Heretic-Uncensored.Q4_K_M.gguf").as_deref(), Some("Q4_K_M"));
        assert_eq!(quant_of("Qwen3-8B-IQ4_XS.gguf").as_deref(), Some("IQ4_XS"));
        assert_eq!(quant_of("model-q8_0.gguf").as_deref(), Some("Q8_0"));
        assert_eq!(quant_of("gemma-3-4b-it-BF16.gguf").as_deref(), Some("BF16"));
        assert_eq!(quant_of("mmproj-model-f16.gguf").as_deref(), Some("F16"));
        assert_eq!(quant_of("readme.gguf"), None);
        assert_eq!(split_of("big-Q4_K_M-00002-of-00003.gguf"), Some((2, 3)));
        assert_eq!(split_of("small.gguf"), None);
        assert!(is_mmproj("sub/mmproj-F16.gguf"));
        assert_eq!(card_text("---\nlicense: mit\n---\n# 标题 <img src=x onerror=alert(1)>\n正文"), "# 标题 \n正文");
        assert_eq!(enc("a b/中.gguf"), "a%20b/%E4%B8%AD.gguf");
    }
}

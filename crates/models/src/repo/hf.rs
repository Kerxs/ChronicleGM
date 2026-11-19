//! Hugging Face 适配器（公开 Hub API）：
//! - 搜索：`GET /api/models?search=…&filter=gguf&full=true&sort=downloads&limit=…`
//! - 详情：`GET /api/models/{id}?blobs=true`（文件大小与 LFS 的 SHA256）
//! - 模型卡：`GET /{id}/raw/main/README.md`
//! - 下载：`GET /{id}/resolve/main/{文件}`（私有 / 需授权的仓库带 `Authorization: Bearer <令牌>`）

use serde_json::Value;

use super::{ModelDetail, ModelSummary, RepoAdapter, enc, enc_q, gguf_file};
use crate::net::{self, NetError};

pub struct HuggingFace {
    pub base: String,
}

impl Default for HuggingFace {
    fn default() -> Self {
        HuggingFace { base: std::env::var("CHRONICLE_HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".into()) }
    }
}

fn summary(v: &Value) -> ModelSummary {
    let tags: Vec<String> = v["tags"].as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let id = v["id"].as_str().or(v["modelId"].as_str()).unwrap_or_default().to_string();
    ModelSummary {
        source: "hf".into(),
        author: v["author"].as_str().map(str::to_string).unwrap_or_else(|| id.split('/').next().unwrap_or_default().to_string()),
        id,
        downloads: v["downloads"].as_u64().unwrap_or(0),
        likes: v["likes"].as_u64().unwrap_or(0),
        updated: v["lastModified"].as_str().unwrap_or_default().into(),
        license: v["cardData"]["license"].as_str().map(str::to_string).or_else(|| tags.iter().find_map(|t| t.strip_prefix("license:").map(str::to_string))),
        gated: v["gated"].as_bool().unwrap_or(false) || v["gated"].is_string() || v["private"].as_bool().unwrap_or(false),
        tags,
    }
}

impl RepoAdapter for HuggingFace {
    fn id(&self) -> &'static str {
        "hf"
    }
    fn name(&self) -> &'static str {
        "Hugging Face"
    }

    fn search(&self, query: &str, limit: usize, token: Option<&str>) -> Result<Vec<ModelSummary>, NetError> {
        let q = query.trim();
        // 直接给了仓库 ID：先按 ID 查，查到就只返回这一个。
        if q.contains('/')
            && !q.contains(' ')
            && let Ok(d) = self.details(q, token)
        {
            return Ok(vec![d.summary]);
        }
        let url = format!("{}/api/models?search={}&filter=gguf&full=true&sort=downloads&direction=-1&limit={}", self.base, enc_q(q), limit.clamp(1, 100));
        let v = net::get_json(&url, token)?;
        Ok(v.as_array().map(|a| a.iter().map(summary).filter(|s| !s.id.is_empty()).collect()).unwrap_or_default())
    }

    fn details(&self, id: &str, token: Option<&str>) -> Result<ModelDetail, NetError> {
        let id = id.trim().trim_matches('/');
        let v = net::get_json(&format!("{}/api/models/{}?blobs=true", self.base, enc(id)), token)?;
        let s = summary(&v);
        let files = v["siblings"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|f| {
                        let path = f["rfilename"].as_str()?;
                        let size = f["lfs"]["size"].as_u64().or(f["size"].as_u64()).unwrap_or(0);
                        let sha = f["lfs"]["sha256"].as_str().map(str::to_string);
                        gguf_file(path, size, sha, format!("{}/{}/resolve/main/{}", self.base, enc(id), enc(path)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // 模型卡只取开头当纯文本显示；取不到不影响。
        let card = readme(&format!("{}/{}/raw/main/README.md", self.base, enc(id)), token).map(|t| super::card_text(&t)).unwrap_or_default();
        Ok(ModelDetail { summary: s, files, card })
    }
}

fn readme(url: &str, token: Option<&str>) -> Option<String> {
    let mut r = net::agent().get(url);
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        r = r.header("Authorization", &format!("Bearer {t}"));
    }
    r.call().ok()?.body_mut().with_config().limit(256 * 1024).read_to_string().ok()
}

//! 魔搭社区（ModelScope）适配器，只用官方 Python SDK（`modelscope.hub.api.HubApi`）里用到的接口：
//! - 模型详情：`GET /api/v1/models/{作者}/{名字}`
//! - 文件列表：`GET /api/v1/models/{作者}/{名字}/repo/files?Revision=master&Recursive=true`
//! - 按作者列模型：`PUT /api/v1/models/`，请求体 `{"Path": 作者, "PageNumber", "PageSize"}`
//! - 下载：`GET /api/v1/models/{作者}/{名字}/repo?Revision=master&FilePath={文件}`
//! - 私有仓库：`POST /api/v1/login`（`{"AccessToken": 令牌}`）换会话 Cookie。
//!
//! 官方 SDK 没有公开的"关键词搜索"接口，所以这里只支持按仓库 ID（`作者/名字`）或作者搜索；
//! 网页搜索用的是未公开接口，按任务要求不使用。

use serde_json::{Value, json};

use super::{ModelDetail, ModelSummary, RepoAdapter, enc, enc_q, gguf_file};
use crate::net::{self, NetError};

pub struct ModelScope {
    pub base: String,
}

impl Default for ModelScope {
    fn default() -> Self {
        ModelScope { base: std::env::var("CHRONICLE_MODELSCOPE_ENDPOINT").unwrap_or_else(|_| "https://www.modelscope.cn".into()) }
    }
}

/// 魔搭的回复格式：`{"Code": 200, "Data": …, "Message": …}`。
fn data(v: Value) -> Result<Value, NetError> {
    let code = v["Code"].as_i64().unwrap_or(200);
    if code != 200 && code != 0 {
        let msg = v["Message"].as_str().unwrap_or("").to_string();
        return Err(NetError::Http(if code == 10010205 || msg.contains("permission") { 403 } else { 404 }, msg));
    }
    Ok(v["Data"].clone())
}

fn ts(v: &Value) -> String {
    match v.as_i64() {
        Some(secs) => {
            // Unix 秒 → 粗略的 ISO 日期（只用于显示和排序）。
            let days = secs / 86400;
            let (y, m, d) = civil(days);
            format!("{y:04}-{m:02}-{d:02}")
        }
        None => v.as_str().unwrap_or_default().to_string(),
    }
}

/// 天数（1970-01-01 起）→ 年月日。
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn summary(d: &Value, id_hint: &str) -> ModelSummary {
    let owner = d["Path"].as_str().unwrap_or_default();
    let name = d["Name"].as_str().unwrap_or_default();
    let id = if owner.is_empty() || name.is_empty() { id_hint.to_string() } else { format!("{owner}/{name}") };
    let tags: Vec<String> = d["Tags"].as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_string)).collect()).unwrap_or_default();
    ModelSummary {
        source: "modelscope".into(),
        author: id.split('/').next().unwrap_or_default().into(),
        id,
        downloads: d["Downloads"].as_u64().unwrap_or(0),
        likes: d["Stars"].as_u64().unwrap_or(0),
        updated: ts(&d["LastUpdatedTime"]),
        license: d["License"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
        gated: d["IsPrivate"].as_bool().unwrap_or(false) || d["Visibility"].as_i64() == Some(1),
        tags,
    }
}

impl ModelScope {
    /// 用访问令牌登录，换取会话 Cookie（私有仓库用）。令牌和 Cookie 都不写日志。
    pub fn login(&self, token: &str) -> Result<String, NetError> {
        let r = net::agent().post(&format!("{}/api/v1/login", self.base)).header("Content-Type", "application/json").send(json!({ "AccessToken": token }).to_string());
        let resp = match r {
            Ok(x) => x,
            Err(ureq::Error::StatusCode(c)) => return Err(NetError::Http(c, "登录失败".into())),
            Err(e) => return Err(NetError::Io(format!("网络错误：{e}"))),
        };
        let cookies: Vec<String> = resp.headers().get_all("set-cookie").iter().filter_map(|v| v.to_str().ok()).filter_map(|v| v.split(';').next().map(str::to_string)).collect();
        if cookies.is_empty() {
            return Err(NetError::Http(401, "访问令牌无效".into()));
        }
        Ok(cookies.join("; "))
    }

    fn cookie(&self, token: Option<&str>) -> Result<Option<String>, NetError> {
        match token.filter(|t| !t.is_empty()) {
            Some(t) => self.login(t).map(Some),
            None => Ok(None),
        }
    }

    pub fn file_url(&self, id: &str, path: &str) -> String {
        format!("{}/api/v1/models/{}/repo?Revision=master&FilePath={}", self.base, enc(id), enc_q(path))
    }

    fn details_with(&self, id: &str, cookie: Option<&str>) -> Result<ModelDetail, NetError> {
        let id = id.trim().trim_matches('/');
        let d = data(net::get_json_with(&format!("{}/api/v1/models/{}", self.base, enc(id)), None, cookie)?)?;
        let s = summary(&d, id);
        let fl = data(net::get_json_with(&format!("{}/api/v1/models/{}/repo/files?Revision=master&Recursive=true", self.base, enc(id)), None, cookie)?)?;
        let files = fl["Files"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|f| f["Type"].as_str().unwrap_or("blob") == "blob")
                    .filter_map(|f| {
                        let path = f["Path"].as_str().or(f["Name"].as_str())?;
                        gguf_file(path, f["Size"].as_u64().unwrap_or(0), f["Sha256"].as_str().filter(|s| !s.is_empty()).map(str::to_string), self.file_url(id, path))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let card = d["ReadMeContent"].as_str().or(d["Description"].as_str()).map(super::card_text).unwrap_or_default();
        Ok(ModelDetail { summary: s, files, card })
    }
}

impl RepoAdapter for ModelScope {
    fn id(&self) -> &'static str {
        "modelscope"
    }
    fn name(&self) -> &'static str {
        "魔搭社区"
    }

    fn search(&self, query: &str, limit: usize, token: Option<&str>) -> Result<Vec<ModelSummary>, NetError> {
        let q = query.trim().trim_matches('/');
        let cookie = self.cookie(token)?;
        if q.contains('/') {
            return Ok(vec![self.details_with(q, cookie.as_deref())?.summary]);
        }
        if q.is_empty() || q.contains(' ') {
            return Err(NetError::Http(400, "魔搭只支持按仓库 ID（作者/名字）或作者名搜索".into()));
        }
        // 按作者列出模型，再只留名字里带 GGUF 的（文件列表要逐个查，太慢）。
        let mut r = net::agent().put(&format!("{}/api/v1/models/", self.base)).header("Content-Type", "application/json");
        if let Some(c) = cookie.as_deref() {
            r = r.header("Cookie", c);
        }
        let v: Value = match r.send(json!({ "Path": q, "PageNumber": 1, "PageSize": limit.clamp(1, 100) }).to_string()) {
            Ok(mut resp) => serde_json::from_str(&resp.body_mut().read_to_string().map_err(|e| NetError::Io(e.to_string()))?).map_err(|e| NetError::Io(e.to_string()))?,
            Err(ureq::Error::StatusCode(c)) => return Err(NetError::Http(c, String::new())),
            Err(e) => return Err(NetError::Io(format!("网络错误：{e}"))),
        };
        let d = data(v)?;
        Ok(d["Models"].as_array().map(|a| a.iter().map(|m| summary(m, "")).filter(|s| s.id.to_ascii_lowercase().contains("gguf") || s.tags.iter().any(|t| t.eq_ignore_ascii_case("gguf"))).collect()).unwrap_or_default())
    }

    fn details(&self, id: &str, token: Option<&str>) -> Result<ModelDetail, NetError> {
        let cookie = self.cookie(token)?;
        self.details_with(id, cookie.as_deref())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn unix_seconds_to_date() {
        assert_eq!(super::ts(&serde_json::json!(1_700_000_000)), "2023-11-14");
    }
}

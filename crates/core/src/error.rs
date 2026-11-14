use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("数据库错误: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("文件错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON 错误: {0}")]
    Json(#[from] serde_json::Error),
    #[error("压缩包错误: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("规则错误: {0}")]
    Rules(#[from] chronicle_rules::RulesError),
    #[error("找不到 {0}: {1}")]
    NotFound(&'static str, String),
    #[error("{entity} {id} 已被修改（期望版本 {expected}，当前版本 {found}）")]
    RevConflict { entity: &'static str, id: String, expected: i64, found: i64 },
    #[error("数据无效: {0}")]
    Invalid(String),
    #[error("受保护的内容: {0}")]
    Protected(String),
    #[error("迁移失败: {0}")]
    Migration(String),
    #[error("变更集校验未通过: {0}")]
    Validation(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;

impl Serialize for CoreError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

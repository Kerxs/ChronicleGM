//! Chronicle GM 生成管线：上下文组装 → 提示词 → 模型调用 → 解析与修正 → 待审变更集。
//!
//! AI 只产生提议；所有写入都经过 core 的变更集校验和用户确认。

pub mod context;
pub mod outputs;
pub mod play;
pub mod parse;
pub mod plot_tasks;
pub mod runner;
pub mod tasks;
pub mod templates;
pub mod world_tasks;

pub use runner::{GenOptions, MAX_REFERENCE_IMAGES, RequestLog, call_model, finalize, prepare, prepare_retry, prepare_with_images, run};
pub use tasks::Task;
pub use templates::Templates;

#[derive(Debug, thiserror::Error)]
pub enum GenError {
    #[error("{0}")]
    Core(#[from] chronicle_core::CoreError),
    #[error("{0}")]
    Llm(#[from] chronicle_llm::LlmError),
    #[error("输入有误：{0}")]
    Input(String),
    #[error("模型输出无法使用：{0}")]
    Output(String),
    #[error("模板错误：{0}")]
    Template(String),
    #[error("已取消")]
    Cancelled,
    #[error("JSON 错误：{0}")]
    Json(#[from] serde_json::Error),
    #[error("数据库错误：{0}")]
    Db(#[from] chronicle_core::rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, GenError>;

impl serde::Serialize for GenError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

//! Chronicle GM 的 AI 主持人：回合循环、NPC 代理、战役记忆、秘密泄露检查。
//!
//! 原则：AI 只提议；骰子、规则判定与状态修改由程序完成；核心设定不会被 AI 修改；
//! NPC 只知道自己应该知道的事。

pub mod ask;
pub mod compress;
pub mod context;
pub mod director;
pub mod leak;
pub mod npcday;
pub mod outputs;
pub mod pcgen;
pub mod questgen;
pub mod review;
pub mod speech;
pub mod turn;
pub mod validate;

pub use ask::{GmOptions, WebSearch};
pub use turn::{Engine, OPENING, TurnResult, ensure_welcome, opening_prompt, run_turn, run_turn_with_stages, summarize};

#[derive(Debug, thiserror::Error)]
pub enum GmError {
    #[error("{0}")]
    Core(#[from] chronicle_core::CoreError),
    #[error("{0}")]
    Gen(#[from] chronicle_gen::GenError),
    #[error("{0}")]
    Llm(#[from] chronicle_llm::LlmError),
    #[error("输入有误：{0}")]
    Input(String),
    #[error("模型输出无法使用：{0}")]
    Output(String),
    #[error("已取消")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, GmError>;

//! Chronicle GM 内置模型管理（llama.cpp 运行环境、模型仓库、下载、模型库、自动参数）。
//!
//! 各部分相互独立：
//! - [`hw`] 硬件检测；
//! - [`net`] 带断点续传和校验的单文件下载（运行环境和下载管理器共用）；
//! - [`runtime_mgr`] llama.cpp 运行环境：从官方 Releases 选构建、安装、版本切换、回滚、损坏检测；
//! - [`repo`] 模型仓库适配器（Hugging Face、魔搭）；
//! - [`download`] 下载任务管理器；
//! - [`library`] 本地模型库；
//! - [`tune`] llama-server 自动参数与兼容性检查。
//!
//! 所有函数都是阻塞的，由调用方放到后台线程执行；进度通过回调报告。

pub mod download;
pub mod hw;
pub mod library;
pub mod net;
pub mod repo;
pub mod runtime_mgr;
pub mod tune;

/// 日志回调（写应用日志；不得包含令牌）。
pub type LogFn = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

pub fn no_log() -> LogFn {
    std::sync::Arc::new(|_: &str| {})
}

/// 默认模型（用户指定）：Hugging Face 上的 Rocinante-X-12B Q4_K_M。
pub const DEFAULT_MODEL_REPO: &str = "mradermacher/Rocinante-X-12B-v1-Heretic-Uncensored-GGUF";
pub const DEFAULT_MODEL_FILE: &str = "Rocinante-X-12B-v1-Heretic-Uncensored.Q4_K_M.gguf";

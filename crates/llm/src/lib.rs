//! Chronicle GM 模型接口层：OpenAI 兼容客户端、自动识别、能力实测、GGUF 识别、日志脱敏。
//!
//! 模型由用户自行准备；本 crate 只连接、识别与测试，不下载、不安装、不启动任何程序。

pub mod client;
pub mod discovery;
pub mod gguf;
pub mod image;
pub mod redact;
pub mod secrets;
pub mod tts;
pub mod types;

pub use client::{CancelToken, LlmClient, OpenAiClient, RetryPolicy};
pub use types::*;

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("服务返回 HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("无法连接模型服务: {0}")]
    Transport(String),
    #[error("请求超时")]
    Timeout,
    #[error("已取消")]
    Cancelled,
    #[error("模型返回的数据无法使用: {0}")]
    BadResponse(String),
    #[error("配置错误: {0}")]
    Config(String),
    #[error("文件错误: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, LlmError>;

/// 去掉思考型模型输出中的 `<think>…</think>` 段落（含未闭合的开头段落）。
pub fn strip_thinking(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        match rest.find("<think>") {
            Some(i) => {
                out.push_str(&rest[..i]);
                match rest[i..].find("</think>") {
                    Some(j) => rest = &rest[i + j + "</think>".len()..],
                    None => return out,
                }
            }
            None => {
                // 有些模板只输出结束标记。
                if let Some(j) = rest.find("</think>")
                    && out.is_empty()
                {
                    rest = &rest[j + "</think>".len()..];
                    continue;
                }
                out.push_str(rest);
                return out;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::strip_thinking;

    #[test]
    fn strips_think_blocks() {
        assert_eq!(strip_thinking("<think>嗯……</think>\n{\"a\":1}"), "\n{\"a\":1}");
        assert_eq!(strip_thinking("推理过程</think>答案"), "答案");
        assert_eq!(strip_thinking("<think>没有结束"), "");
        assert_eq!(strip_thinking("普通文本"), "普通文本");
    }
}

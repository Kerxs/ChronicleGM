//! 从模型输出中提取 JSON：去掉思考段落、代码块标记，找到第一个完整的 JSON 对象。

use chronicle_llm::strip_thinking;
use serde::de::DeserializeOwned;

/// 提取第一个括号平衡的 JSON 对象文本。
pub fn extract_json(raw: &str) -> Option<String> {
    let s = strip_thinking(raw);
    let bytes: Vec<char> = s.chars().collect();
    let start = bytes.iter().position(|c| *c == '{')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (i, c) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if esc {
                esc = false;
            } else if *c == '\\' {
                esc = true;
            } else if *c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(bytes[start..=i].iter().collect());
                }
            }
            _ => {}
        }
    }
    None
}

/// 解析为目标结构；失败时返回给模型看的错误说明。
pub fn parse_output<T: DeserializeOwned>(raw: &str) -> Result<(T, serde_json::Value), String> {
    let text = extract_json(raw).ok_or_else(|| "没有找到 JSON 对象（输出可能被截断或包含其他内容）".to_string())?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("JSON 语法错误：{e}"))?;
    let typed = serde_json::from_value::<T>(value.clone()).map_err(|e| format!("JSON 结构不符合要求：{e}"))?;
    Ok((typed, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_from_noise() {
        let raw = "<think>先想想 {不是json}</think>好的，结果如下：\n```json\n{\"a\": \"含有 } 和 { 的字符串\", \"b\": {\"c\": 1}}\n```\n以上。";
        assert_eq!(extract_json(raw).unwrap(), "{\"a\": \"含有 } 和 { 的字符串\", \"b\": {\"c\": 1}}");
        assert!(extract_json("{\"a\": 1").is_none(), "被截断的输出");
        assert!(extract_json("没有 JSON").is_none());
    }

    #[derive(serde::Deserialize, Debug)]
    struct T {
        #[allow(dead_code)]
        x: i32,
    }

    #[test]
    fn typed_errors() {
        assert!(parse_output::<T>("{\"x\": 1}").is_ok());
        assert!(parse_output::<T>("{\"x\": \"a\"}").unwrap_err().contains("结构不符合"));
        assert!(parse_output::<T>("{\"x\": 1,}").unwrap_err().contains("语法错误"));
    }
}

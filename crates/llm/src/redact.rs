//! 日志脱敏：任何写入日志或错误信息的文本都先经过这里。

/// 把已知密钥与常见密钥形态替换为 `***`。
pub fn redact(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_string();
    for s in secrets {
        if s.len() >= 4 {
            out = out.replace(s, "***");
        }
    }
    redact_patterns(&out)
}

/// 处理 `Bearer xxx`、`sk-xxx`、`api_key=xxx` 等形态。
fn redact_patterns(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let is_tok = |c: char| c.is_ascii_alphanumeric() || "-_.~+/=".contains(c);
    while i < chars.len() {
        let rest: String = chars[i..chars.len().min(i + 12)].iter().collect();
        let lower = rest.to_ascii_lowercase();
        let prefix_len = if lower.starts_with("bearer ") {
            7
        } else if lower.starts_with("sk-") || lower.starts_with("sk_") {
            3
        } else if lower.starts_with("api_key=") || lower.starts_with("api-key:") {
            8
        } else if lower.starts_with("x-api-key: ") {
            11
        } else {
            0
        };
        let boundary = i == 0 || !is_tok(chars[i - 1]);
        if prefix_len > 0 && boundary {
            let mut j = i + prefix_len;
            while j < chars.len() && chars[j] == ' ' {
                j += 1;
            }
            let start = j;
            while j < chars.len() && is_tok(chars[j]) {
                j += 1;
            }
            if j - start >= 8 {
                out.extend(&chars[i..i + prefix_len]);
                out.push_str("***");
                i = j;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_known_and_patterns() {
        let s = "Authorization: Bearer abcdef1234567890 key=MYSECRETKEY sk-proj-AAAABBBBCCCC api_key=zzzzzzzzzz";
        let r = redact(s, &["MYSECRETKEY"]);
        assert!(!r.contains("abcdef1234567890"));
        assert!(!r.contains("MYSECRETKEY"));
        assert!(!r.contains("AAAABBBBCCCC"));
        assert!(!r.contains("zzzzzzzzzz"));
        assert!(r.contains("Bearer ***"));
    }

    #[test]
    fn keeps_ordinary_text() {
        let s = "task-runner: skip 3 items; 中文内容不受影响";
        assert_eq!(redact(s, &[]), s);
    }
}

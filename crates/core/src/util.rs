//! ID、时间与分数索引排序键。

use std::time::{SystemTime, UNIX_EPOCH};

pub fn new_id() -> String {
    ulid::Ulid::generate().to_string()
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// base62，按 ASCII 升序排列，因此字符串字典序 == 数值序。
const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn digit_index(c: u8) -> usize {
    DIGITS.iter().position(|d| *d == c).expect("排序键包含非法字符")
}

pub fn is_valid_sort_key(k: &str) -> bool {
    !k.is_empty() && !k.ends_with('0') && k.bytes().all(|b| DIGITS.contains(&b))
}

/// 生成严格位于 `a` 与 `b` 之间的排序键（分数索引）。`None` 表示无界。
/// 插入任意位置只需改一行，不必重排兄弟节点。
pub fn key_between(a: Option<&str>, b: Option<&str>) -> String {
    if let (Some(a), Some(b)) = (a, b) {
        assert!(a < b, "key_between 要求 a < b: {a} >= {b}");
    }
    midpoint(a.unwrap_or("").as_bytes(), b.map(str::as_bytes))
}

fn midpoint(a: &[u8], b: Option<&[u8]>) -> String {
    if let Some(b) = b {
        // 去掉公共前缀（a 不足处按 '0' 补齐）。
        let mut n = 0;
        while n < b.len() && a.get(n).copied().unwrap_or(b'0') == b[n] {
            n += 1;
        }
        if n > 0 {
            let rest_a = if n <= a.len() { &a[n..] } else { &[][..] };
            let prefix = std::str::from_utf8(&b[..n]).unwrap().to_string();
            return prefix + &midpoint(rest_a, Some(&b[n..]));
        }
    }
    let da = a.first().map(|c| digit_index(*c)).unwrap_or(0);
    let db = b.map(|b| digit_index(b[0])).unwrap_or(DIGITS.len());
    if db - da > 1 {
        let mid = (da + db).div_ceil(2);
        (DIGITS[mid] as char).to_string()
    } else if let Some(b) = b.filter(|b| b.len() > 1) {
        (b[0] as char).to_string()
    } else {
        let rest = if a.is_empty() { &[][..] } else { &a[1..] };
        format!("{}{}", DIGITS[da] as char, midpoint(rest, None))
    }
}

/// 规范化名称，用于重复检测：去空白与常见标点，统一小写与全角字符。
pub fn normalize_name(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            _ => c,
        })
        .filter(|c| !c.is_whitespace() && !"·•・-_'\"「」『』《》()（）[]【】.,，。".contains(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// 估算 token 数（没有服务端分词时用）：汉字等宽字符按 1 个，其余按 4 个字符 1 个。
pub fn estimate_tokens(text: &str) -> usize {
    let (mut wide, mut other) = (0usize, 0usize);
    for c in text.chars() {
        if (c as u32) >= 0x2E80 {
            wide += 1;
        } else {
            other += 1;
        }
    }
    wide + other.div_ceil(4)
}

/// 编辑距离（按字符，最多算到 limit+1 就停）。
pub fn edit_distance(a: &str, b: &str, limit: usize) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > limit {
        return limit + 1;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        let mut best = cur[0];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            best = best.min(cur[j]);
        }
        if best > limit {
            return limit + 1;
        }
        prev = cur;
    }
    prev[b.len()]
}

/// 两个名字（已规范化）是否"近似"：长度 3 以上差一个字，或长度 6 以上差两个字。
pub fn names_similar(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let n = a.chars().count().min(b.chars().count());
    let limit = if n >= 6 { 2 } else if n >= 3 { 1 } else { 0 };
    limit > 0 && edit_distance(a, b, limit) <= limit
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn basic_keys() {
        let a = key_between(None, None);
        let b = key_between(Some(&a), None);
        let c = key_between(None, Some(&a));
        let m = key_between(Some(&a), Some(&b));
        assert!(c < a && a < m && m < b);
        assert!(is_valid_sort_key(&a) && is_valid_sort_key(&m));
    }

    #[test]
    fn many_inserts_at_front_and_between() {
        let mut keys = vec![key_between(None, None)];
        for _ in 0..200 {
            let k = key_between(None, Some(&keys[0]));
            keys.insert(0, k);
        }
        for _ in 0..200 {
            let k = key_between(Some(&keys[0]), Some(&keys[1]));
            keys.insert(1, k);
        }
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert!(keys.iter().all(|k| is_valid_sort_key(k)));
    }

    #[test]
    fn normalize() {
        assert_eq!(normalize_name(" 艾 莉娅·星歌 "), normalize_name("艾莉娅星歌"));
        assert_eq!(normalize_name("ＡＢＣ"), "abc");
    }

    proptest! {
        #[test]
        fn random_inserts_stay_ordered(ops in prop::collection::vec(any::<prop::sample::Index>(), 1..150)) {
            let mut keys: Vec<String> = Vec::new();
            for idx in ops {
                let pos = idx.index(keys.len() + 1);
                let a = if pos == 0 { None } else { Some(keys[pos - 1].as_str()) };
                let b = keys.get(pos).map(String::as_str);
                let k = key_between(a, b);
                prop_assert!(is_valid_sort_key(&k));
                if let Some(a) = a { prop_assert!(a < k.as_str()); }
                if let Some(b) = b { prop_assert!(k.as_str() < b); }
                keys.insert(pos, k);
            }
        }
    }
}

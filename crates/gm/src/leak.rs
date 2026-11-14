//! 秘密泄露检查（尽力而为）：玩家可见的文字中，是否原样出现了主持人信息里的片段。
//! 只能发现逐字照搬，无法发现改写后的泄露，因此只提示，不拦截。

/// 把隐藏文字拆成可比对的片段（按标点切分，至少 `min` 个字符）。
pub fn fragments(hidden: &[String], min: usize) -> Vec<String> {
    let mut out = vec![];
    for h in hidden {
        for part in h.split(|c: char| "。！？；，、\n.!?;,：:\"“”「」（）()".contains(c)) {
            let p: String = part.chars().filter(|c| !c.is_whitespace()).collect();
            if p.chars().count() >= min && !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// 返回在 `text` 中原样出现的隐藏片段；也检查长片段中的连续子串（滑动窗口）。
pub fn find_leaks(text: &str, hidden: &[String]) -> Vec<String> {
    const WINDOW: usize = 10;
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let mut found = vec![];
    for frag in fragments(hidden, 6) {
        let chars: Vec<char> = frag.chars().collect();
        let hit = if chars.len() <= WINDOW {
            compact.contains(&frag)
        } else {
            (0..=chars.len() - WINDOW).any(|i| compact.contains(&chars[i..i + WINDOW].iter().collect::<String>()))
        };
        if hit {
            found.push(frag);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_verbatim_secret() {
        let hidden = vec!["村长其实参与过上一次封印，并且害死了自己的兄弟。".to_string()];
        assert_eq!(find_leaks("老人叹气：我参与过上一次封印……", &hidden).len(), 0, "短于窗口的部分照搬不算");
        assert_eq!(find_leaks("你听说村长其实参与过上一次封印。", &hidden).len(), 1);
        assert_eq!(find_leaks("村长看起来很紧张。", &hidden).len(), 0);
        assert!(fragments(&hidden, 6).iter().all(|f| f.chars().count() >= 6));
    }
}

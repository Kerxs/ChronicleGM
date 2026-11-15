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

/// 换了说法的泄露（第 35 项，启发式）：叙述和某条秘密共享的双字组合占秘密的大部分时，提示"可能换了说法说出了秘密"。
/// 只用于提醒主持人，不拦截、不修改叙述。
pub fn paraphrase_leaks(text: &str, hidden: &[String]) -> Vec<String> {
    let bigrams = |s: &str| -> std::collections::HashSet<(char, char)> {
        let c: Vec<char> = s.chars().filter(|c| !c.is_whitespace() && !"。！？；，、：:\"“”「」（）()…—".contains(*c)).collect();
        c.windows(2).map(|w| (w[0], w[1])).collect()
    };
    let t = bigrams(text);
    let mut out = vec![];
    for h in hidden {
        for part in h.split(|c: char| "。！？；\n".contains(c)) {
            let b = bigrams(part);
            if b.len() < 8 {
                continue;
            }
            let shared = b.iter().filter(|x| t.contains(x)).count();
            if shared * 100 / b.len() >= 55 && find_leaks(text, &[part.to_string()]).is_empty() {
                out.push(part.trim().to_string());
            }
        }
    }
    out
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

    #[test]
    fn detects_paraphrased_secret() {
        let hidden = vec!["村长参与过上一次封印仪式，亲手把弟弟留在了遗迹里".to_string()];
        assert_eq!(paraphrase_leaks("你忽然明白：上一次封印仪式村长也参与了，是他亲手把弟弟留在遗迹里的。", &hidden).len(), 1);
        assert!(paraphrase_leaks("村长低头不语，望着北边的山谷。", &hidden).is_empty());
    }
}

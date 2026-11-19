//! 检测 GM 叙述里"替 NPC 说话"的直接引语。
//!
//! 真实试玩中模型常把在场 NPC 的台词直接写进叙述（"村长叹了口气说：'……'"），
//! 这样 NPC 的回答绕过了 NPC 代理的知识隔离。程序找出这类引语：从叙述中删掉该句，
//! 改由对应 NPC 亲自回答。只处理能明确归属到某个在场 NPC 的直接引语。

/// 引号对（中文弯引号、直角引号、英文直引号、全角直引号）。
const QUOTES: [(char, char); 6] = [('“', '”'), ('「', '」'), ('"', '"'), ('＂', '＂'), ('‘', '’'), ('\'', '\'')];

/// 能作为 `open` 的右引号的字符：模型常混用弯引号与直引号（如 “……" 或 "……”）。
fn closes(open: char, c: char) -> bool {
    match open {
        // 实测模型会混用：“……’、‘……”。
        '“' | '"' | '＂' => matches!(c, '”' | '"' | '＂' | '’'),
        '「' => c == '」',
        // 实测模型也用弯单引号 ‘……’ 和英文单引号 '……' 写台词。
        '‘' | '\'' => matches!(c, '’' | '\'' | '”'),
        _ => false,
    }
}
const BOUNDARY: [char; 5] = ['。', '！', '？', '\n', '…'];
const SPEECH_CUES: [&str; 12] = ["说", "道", "问", "答", "喊", "叫", "开口", "低声", "嘟囔", "补充", "：", ":"];

/// 一个 NPC 可能被称呼的名字：全名，以及名字里每两个相邻字（"村长老格"→"村长""长老""老格"）。
/// 实测模型会把名字颠倒着叫（"格长老"），只用首尾两字匹配不到。
fn aliases(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut out = vec![name.to_string()];
    if chars.len() >= 3 {
        out.extend(chars.windows(2).map(|w| w.iter().collect::<String>()));
    }
    out.retain(|a| a.chars().count() >= 2);
    out
}

/// 文本中最后被提到的 NPC。
fn last_mentioned(text: &str, alias_list: &[Vec<String>]) -> Option<usize> {
    alias_list
        .iter()
        .enumerate()
        .filter_map(|(i, al)| al.iter().filter_map(|a| text.rfind(a.as_str())).max().map(|p| (i, p)))
        .max_by_key(|(_, p)| *p)
        .map(|(i, _)| i)
}

pub struct Extracted {
    /// 删掉代说台词后的叙述。
    pub narration: String,
    /// 被代说的 NPC（在 `names` 中的下标，按出现顺序去重）。
    pub speakers: Vec<usize>,
}

/// `names`：在场 NPC 的名字。
pub fn extract_ventriloquism(narration: &str, names: &[&str]) -> Extracted {
    // 剧本格式的台词：一行以"人物名：……"开头（实测："苏桦：（不耐烦地瞥了一眼）喂，船匠……"）。
    let mut speakers: Vec<usize> = vec![];
    let mut kept: Vec<&str> = vec![];
    for line in narration.lines() {
        let t = line.trim_start();
        let who = names.iter().position(|n| {
            !n.is_empty() && t.strip_prefix(*n).is_some_and(|r| r.starts_with('：') || r.starts_with(':'))
        });
        match who {
            Some(idx) => {
                if !speakers.contains(&idx) {
                    speakers.push(idx);
                }
            }
            None => kept.push(line),
        }
    }
    let rest = kept.join("\n");
    let mut q = extract_quotes(&rest, names);
    for s in std::mem::take(&mut q.speakers) {
        if !speakers.contains(&s) {
            speakers.push(s);
        }
    }
    Extracted { narration: q.narration, speakers }
}

/// 引号里的台词（见 [`extract_ventriloquism`]）。
fn extract_quotes(narration: &str, names: &[&str]) -> Extracted {
    let chars: Vec<char> = narration.chars().collect();
    let alias_list: Vec<Vec<String>> = names.iter().map(|n| aliases(n)).collect();
    let mut remove: Vec<(usize, usize)> = vec![];
    let mut speakers = vec![];
    let mut i = 0;
    while i < chars.len() {
        let open = chars[i];
        if !QUOTES.iter().any(|(o, _)| *o == open) {
            i += 1;
            continue;
        }
        let Some(len) = chars[i + 1..].iter().position(|c| closes(open, *c)) else { break };
        let (q_start, q_end) = (i, i + 1 + len); // q_end 指向右引号
        // 引号前：回到上一个句子边界（或上一个引号）为止。
        let mut s = q_start;
        while s > 0 && !BOUNDARY.contains(&chars[s - 1]) && !QUOTES.iter().any(|(_, c)| *c == chars[s - 1]) {
            s -= 1;
        }
        // 引号后：到下一个句子边界（含）为止，最多 15 个字。
        let mut e = q_end + 1;
        while e < chars.len() && e - q_end <= 15 && !BOUNDARY.contains(&chars[e]) && !QUOTES.iter().any(|(o, _)| *o == chars[e]) {
            e += 1;
        }
        // 15 个字内没有句子边界：只切到引号为止，不从句子中间切断（实测留下了"翼翼地检查着……"这样的半句）。
        let at_boundary = e >= chars.len() || BOUNDARY.contains(&chars[e]);
        let after_end = if e < chars.len() && BOUNDARY.contains(&chars[e]) { e + 1 } else if at_boundary { e } else { q_end + 1 };
        let before: String = chars[s..q_start].iter().collect();
        let after: String = chars[q_end + 1..e].iter().collect();
        let owner_of = |t: &str| alias_list.iter().position(|al| al.iter().any(|a| t.contains(a.as_str())));
        let cued = |t: &str| SPEECH_CUES.iter().any(|c| t.contains(c));
        // 引语自身以句号等结尾时，句子在引号处结束，后面的文字属于下一句。
        let closed = matches!(chars[q_end - 1], '。' | '！' | '？' | '…');
        let hit = if let (Some(idx), true) = (owner_of(&before), cued(&before)) {
            // 说明在前："村长说：'……'"。
            Some((idx, if closed { q_end + 1 } else { after_end }))
        } else if let (Some(idx), true) = (owner_of(&after), cued(&after)) {
            // 说明在后："'……'老格低声说。"
            Some((idx, after_end))
        } else if !before.contains('你') && (before.trim().is_empty() || before.contains('他') || before.contains('她')) {
            // 没有点名的引语（"老人抬起头。「……」他顿了顿"）：归给前文最近提到的在场 NPC。
            // 前一句是玩家在说话（"你问道"）时不归属。
            let prev: String = chars[s.saturating_sub(120)..s].iter().collect();
            let last_sentence = prev.trim_end_matches(BOUNDARY).rsplit(BOUNDARY).next().unwrap_or("");
            let player_speaking = ["你说", "你问", "你答", "你道", "你开口", "你喊"].iter().any(|c| last_sentence.contains(c));
            if player_speaking { None } else { last_mentioned(&prev, &alias_list).map(|idx| (idx, if closed { q_end + 1 } else { after_end })) }
        } else {
            None
        };
        if let Some((idx, end)) = hit {
            remove.push((s, end));
            if !speakers.contains(&idx) {
                speakers.push(idx);
            }
            i = end.max(q_end + 1);
        } else {
            i = q_end + 1;
        }
    }
    if remove.is_empty() {
        return Extracted { narration: narration.to_string(), speakers };
    }
    let mut out = String::new();
    let mut last = 0;
    for (s, e) in remove {
        if s >= last {
            out.extend(&chars[last..s]);
            last = e;
        }
    }
    out.extend(&chars[last.min(chars.len())..]);
    let cleaned = out.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
    Extracted { narration: drop_orphan_quote_tails(&cleaned), speakers }
}

/// 删掉孤立的半截引语：一句话以右引号结束、前面却没有左引号（实测："……缝隙。潮洞里的那位……不喜欢火光。’"）。
fn drop_orphan_quote_tails(text: &str) -> String {
    const END: [char; 4] = ['。', '！', '？', '\n'];
    let mut out: Vec<char> = vec![];
    let mut open = 0i32;
    // 每一句的开头位置（最后一个是当前这句）。
    let mut starts: Vec<usize> = vec![0];
    for c in text.chars() {
        match c {
            '“' | '‘' | '「' => open += 1,
            '”' | '’' | '」' if open == 0 => {
                // 孤立的右引号：连同它所属的那一句一起删掉（引号紧跟在句号后面时，属于刚结束的那一句）。
                let ended = out.last().is_some_and(|x| END.contains(x));
                if ended && starts.len() >= 2 {
                    starts.pop();
                }
                let s = *starts.last().unwrap_or(&0);
                out.truncate(s);
                continue;
            }
            '”' | '’' | '」' => open -= 1,
            _ => {}
        }
        out.push(c);
        if END.contains(&c) && open == 0 {
            starts.push(out.len());
        }
    }
    let s: String = out.into_iter().collect();
    s.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_quoted_npc_line_and_reports_speaker() {
        let n = "酒馆里很安静。村长老格叹了口气，说道：“遗迹的事，说来话长。”他看向窗外。";
        let r = extract_ventriloquism(n, &["铁匠阿岩", "村长老格"]);
        assert_eq!(r.speakers, vec![1]);
        assert_eq!(r.narration, "酒馆里很安静。他看向窗外。");
    }

    #[test]
    fn matches_short_name_and_trailing_attribution() {
        let n = "「最近夜里总有低语。」老格压低声音说。炉火噼啪作响。";
        let r = extract_ventriloquism(n, &["村长老格"]);
        assert_eq!(r.speakers, vec![0]);
        assert_eq!(r.narration, "炉火噼啪作响。");
    }

    #[test]
    fn keeps_quotes_that_are_not_npc_speech() {
        // 石板上的文字、玩家的话、没有说话提示的引号都保留。
        let n = "石板上刻着“潮汐终将归来”。你说：“我答应了。”";
        let r = extract_ventriloquism(n, &["村长老格"]);
        assert!(r.speakers.is_empty());
        assert_eq!(r.narration, n);
    }

    #[test]
    fn unattributed_quotes_go_to_last_mentioned_npc() {
        // 真实试玩第 2 回合的形状：台词没有点名，靠上文与"他"指代。
        let n = "你点头答应，村长老格紧绷的肩膀明显松了下来。「多谢，冒险者。这事……拜托你了。」他从怀里摸出一张地图推到你面前。「只是一条山路，不好走。」他顿了顿。傍晚的凉风已起。";
        let r = extract_ventriloquism(n, &["铁匠阿岩", "村长老格"]);
        assert_eq!(r.speakers, vec![1]);
        assert_eq!(r.narration, "你点头答应，村长老格紧绷的肩膀明显松了下来。他从怀里摸出一张地图推到你面前。他顿了顿。傍晚的凉风已起。");
    }

    #[test]
    fn player_quote_after_npc_mention_is_kept() {
        let n = "村长老格看着你。你开口问道。「遗迹在哪里？」";
        let r = extract_ventriloquism(n, &["村长老格"]);
        assert!(r.speakers.is_empty(), "{:?}", r.narration);
    }

    #[test]
    fn ascii_quotes_from_real_turn() {
        // 真实试玩第 4 回合的原文（英文直引号）。
        let n = "你环顾四周，目光在人群里搜寻，试图捕捉铁匠阿岩的身影。他此刻正站在遗迹入口旁的碎石堆边，裤脚还沾着未散的铁锈色粉末，手里的粗布抹布一下下擦着一把并不脏的锤子。看见你的目光落过去，他动作一顿，抹布停在了半空，喉咙动了动，眼神有些慌乱地往别处飘。\"是你啊……怎么，这荒郊野外的，有什么事吗？\"他把锤子往腰间别了别，掌心似乎下意识地按了按那把工具，语气里带着一丝不易察觉的紧绷。";
        let r = extract_ventriloquism(n, &["村长老格", "铁匠阿岩"]);
        assert_eq!(r.speakers, vec![1]);
        assert!(!r.narration.contains("是你啊"), "{}", r.narration);
    }

    #[test]
    fn single_quotes_from_real_turn() {
        let n = "镇长苏桦猛地放下手中的海图，眼神里闪过一丝惊慌，声音压得极低：‘大约一个小时前！当时我正忙着核对出航船只的清单。’";
        let e = extract_ventriloquism(n, &["苏桦"]);
        assert_eq!(e.speakers, vec![0]);
        assert!(!e.narration.contains("大约一个小时前"), "{}", e.narration);
        let n2 = "莫伦的声音从灯塔脚下传来：'别在那儿站着发呆！'你握紧了铜锤。";
        let e2 = extract_ventriloquism(n2, &["莫伦"]);
        assert_eq!(e2.speakers, vec![0]);
        assert!(!e2.narration.contains("发呆"));
    }

    #[test]
    fn script_style_lines_are_npc_speech() {
        let n = "片刻之后，苏桦的声音从旁边传来。\n苏桦：（不耐烦地瞥了一眼）喂，船匠，别在这儿装睡。\n阿澜也凑了过来。\n阿澜：灯塔熄了也是常事。";
        let e = extract_ventriloquism(n, &["苏桦", "阿澜"]);
        assert_eq!(e.speakers, vec![0, 1]);
        assert!(!e.narration.contains("装睡") && !e.narration.contains("常事"), "{}", e.narration);
        assert!(e.narration.contains("凑了过来"));
    }

    #[test]
    fn cut_never_splits_a_sentence_and_orphan_tails_are_dropped() {
        let n = "莫伦在一旁来回踱步，时不时瞥你一眼：“别磨蹭了！”他手里拿着一块木板，小心翼翼地检查着上面的裂缝和每一个钉眼。";
        let e = extract_ventriloquism(n, &["莫伦"]);
        assert_eq!(e.speakers, vec![0]);
        assert!(!e.narration.starts_with("翼翼") && !e.narration.contains("磨蹭"), "{}", e.narration);
        let o = drop_orphan_quote_tails("你躲在货箱后，耳朵紧贴着木板缝隙。潮洞里的那位……不喜欢火光。’");
        assert_eq!(o, "你躲在货箱后，耳朵紧贴着木板缝隙。");
    }

    #[test]
    fn mixed_quote_characters() {
        let n = "阿岩愣了一下，嘟囔道：“我什么都没拿\"。炉火噼啪作响。";
        let r = extract_ventriloquism(n, &["铁匠阿岩"]);
        assert_eq!(r.speakers, vec![0]);
        assert_eq!(r.narration, "炉火噼啪作响。");
    }

    #[test]
    fn reordered_name_from_real_turn() {
        // 真实试玩第 2 回合：模型把"村长老格"叫成"格长老"，台词用英文直引号、没有点名。
        let n = "你答应下来，格长老浑浊的目光里透出几分感激。他从怀中摸出一张旧羊皮纸，推到你面前。\"从村北头出去，半天功夫就能到遗迹入口。\"他顿了顿，声音里带着一丝颤抖，\"路上小心些总没错。\"你把羊皮纸折好收进怀里，起身走出酒馆。";
        let r = extract_ventriloquism(n, &["村长老格", "铁匠阿岩"]);
        assert_eq!(r.speakers, vec![0]);
        assert!(!r.narration.contains("村北头") && !r.narration.contains("小心些"), "{}", r.narration);
        assert!(r.narration.contains("收进怀里"), "{}", r.narration);
    }

    #[test]
    fn multiple_speakers_in_order() {
        let n = "阿岩嘟囔：“别看我。”村长老格说：“去吧。”";
        let r = extract_ventriloquism(n, &["村长老格", "铁匠阿岩"]);
        assert_eq!(r.speakers, vec![1, 0]);
        assert_eq!(r.narration, "");
    }
}

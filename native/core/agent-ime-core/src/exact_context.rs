// Lexical collocations with a specific left context and an exact reading.
// Keep broad cues such as "新鲜" out: both 新鲜食物 and 新鲜事物 are ordinary.
const COLLOCATIONS: &[(&str, &str, &str)] = &[
    ("shiwu", "失误", "低级"),
    ("shiwu", "事务", "日常行政"),
    ("shiwu", "十五", "农历正月"),
    ("zhiding", "置顶", "将该公告"),
    ("zhiding", "置顶", "把本帖"),
    ("gaifa", "该法", "本案根据"),
    ("gaifa", "该法", "新规根据"),
    ("gaifa", "该法", "依照"),
    ("gaifa", "改法", "这种"),
    ("zhibiao", "指标", "关键绩效"),
    ("zhibiao", "治标", "这个偏方只能"),
    ("zhibiao", "治标", "头痛医头只能"),
    ("zhibiao", "制表", "在电脑上自动"),
    ("shishi", "史实", "史书记录的"),
    ("shishi", "史实", "古籍记载的"),
    ("shishi", "史实", "经史料证实的"),
    ("shishi", "失实", "报道内容明显"),
    ("shishi", "失实", "数据严重"),
    ("shishi", "失事", "客机途中"),
    ("shitu", "仕途", "断送了自己的"),
    ("shitu", "视图", "切换到列表"),
    ("shitu", "视图", "切换到三维"),
    ("zhishi", "芝士", "马苏里拉"),
    ("zhishi", "芝士", "帕玛森"),
    ("zhishi", "芝士", "切达"),
    ("xingshi", "姓氏", "族谱上的"),
    ("xingshi", "姓氏", "家谱记载的"),
    ("xingshi", "行驶", "车辆应减速"),
];

struct ContextCue {
    reading: &'static str,
    word: &'static str,
    preceding_phrases: &'static [&'static str],
    endings: &'static [&'static str],
}

// Some collocations need both a grammatical ending and an earlier topic in
// the same sentence. A bare "事先" or "只能" would be too broad on its own.
const CUES: &[ContextCue] = &[
    ContextCue {
        reading: "zhiding", word: "指定",
        preceding_phrases: &["保存到", "上传到", "复制到", "移动到", "寄到", "转存到"],
        endings: &["事先", "预先", "所"],
    },
    ContextCue {
        reading: "zhibiao", word: "治标",
        preceding_phrases: &["根除病因", "暂时缓解", "表面症状", "治本"],
        endings: &["只能", "仅能", "只会"],
    },
    ContextCue {
        reading: "shishi", word: "史实",
        preceding_phrases: &["史料", "史书", "文献", "古籍", "地方志"],
        endings: &["记载的", "证实的", "可查的", "佐证的"],
    },
    ContextCue {
        reading: "shishi", word: "失实",
        preceding_phrases: &["报道", "新闻", "通报", "关键数字"],
        endings: &["明显", "严重"],
    },
    ContextCue {
        reading: "shishi", word: "失事",
        preceding_phrases: &["黑匣子", "坠机", "沉船", "搜救"],
        endings: &["飞机", "客机", "货轮", "船只"],
    },
    ContextCue {
        reading: "zhishi", word: "芝士",
        preceding_phrases: &["披萨", "比萨", "焗饭"],
        endings: &["拉丝的", "刨碎的", "烤化的"],
    },
    ContextCue {
        reading: "zhishi", word: "制式",
        preceding_phrases: &["电视", "广播", "基站"],
        endings: &["视频信号", "传输信号", "无线通信"],
    },
];

pub(crate) fn exact_collocation(raw: &str, preceding: &str) -> Option<&'static str> {
    if let Some(word) = preferred_word(COLLOCATIONS, raw, preceding) {
        return Some(word);
    }
    let matching_endings = CUES.iter().filter(|cue| cue.reading == raw)
        .flat_map(|cue| cue.endings.iter().map(move |ending| (cue, *ending)))
        .filter_map(|(cue, ending)| preceding.strip_suffix(ending).map(|before| (cue, before)));
    let mut matched = None;
    for (cue, before) in matching_endings {
        let clause = before.rsplit(['。', '！', '？', '；', '.', '!', '?', ';', '\n'])
            .next().unwrap_or_default();
        if !cue.preceding_phrases.iter().any(|phrase| clause.contains(phrase)) {
            continue;
        }
        if matched.is_some_and(|word| word != cue.word) {
            return None;
        }
        matched = Some(cue.word);
    }
    matched
}

fn preferred_word<'a>(
    rules: &'a [(&'a str, &'a str, &'a str)],
    raw: &str,
    preceding: &str,
) -> Option<&'a str> {
    let mut best: Option<&'a str> = None;
    let mut best_len = 0usize;
    let mut ambiguous = false;
    for &(key, word, suffix) in rules {
        if raw != key || suffix.is_empty() || !preceding.ends_with(suffix) {
            continue;
        }
        let len = suffix.len();
        if len > best_len {
            best_len = len;
            best = Some(word);
            ambiguous = false;
        } else if len == best_len && best != Some(word) {
            ambiguous = true;
        }
    }
    if ambiguous { None } else { best }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_input_and_immediately_adjacent_text_match() {
        assert_eq!(exact_collocation("shiwu", "犯了个低级"), Some("失误"));
        assert_eq!(exact_collocation("shiw", "犯了个低级"), None);
        assert_eq!(exact_collocation("shiwu", "并不低级。"), None);
        assert_eq!(exact_collocation("shiwu", "接受新鲜"), None);
        assert_eq!(exact_collocation("shiwu", ""), None);
        assert_eq!(exact_collocation("gaifa", "处理这类案件应依照"), Some("该法"));
        assert_eq!(exact_collocation("gaifa", "你先做这种"), None);
        assert_eq!(exact_collocation("gaifa", "按流程根据"), None);
    }

    #[test]
    fn equal_longest_conflicts_abstain_until_more_specific_evidence() {
        let rules = [
            ("shiwu", "事物", "新鲜"),
            ("shiwu", "食物", "新鲜"),
            ("shiwu", "事物", "新鲜"),
            ("shiwu", "食物", "冰箱里的新鲜"),
            ("shiwu", "事物", "新鲜"),
        ];
        assert_eq!(preferred_word(&rules, "shiwu", "新鲜"), None);
        assert_eq!(preferred_word(&rules, "shiwu", "冰箱里的新鲜"), Some("食物"));
    }

    #[test]
    fn compound_clues_need_both_the_topic_and_the_ending_in_a_single_sentence() {
        assert_eq!(exact_collocation("zhiding", "请复制到财务预先"), Some("指定"));
        assert_eq!(exact_collocation("zhiding", "财务预先"), None);
        assert_eq!(exact_collocation("zhiding", "刚复制到硬盘。主任事先"), None);
        assert_eq!(exact_collocation("zhibiao", "这一做法是暂时缓解，通常只会"), Some("治标"));
        assert_eq!(exact_collocation("zhibiao", "说到治本。任务阶段只能"), None);
        assert_eq!(exact_collocation("zhishi", "焗饭上要铺碎屑，选一种能拉丝的"), Some("芝士"));
    }
}

use super::{learning, Index};
use crate::state::Candidate;

#[cfg(test)]
thread_local! {
    pub(crate) static INNER_TIMING: std::cell::RefCell<[u128; 3]> = const { std::cell::RefCell::new([0; 3]) };
}

#[cfg(test)]
struct InnerTimer(usize, std::time::Instant);
#[cfg(test)]
impl Drop for InnerTimer {
    fn drop(&mut self) {
        INNER_TIMING.with(|timing| timing.borrow_mut()[self.0] += self.1.elapsed().as_nanos());
    }
}

#[derive(Clone, Debug)]
struct GeneratedCandidate {
    candidate: Candidate,
    consumed: usize,
}

/// Spelling sequences tried per segmentation when fuzzy groups are enabled.

struct GeneratedCandidates {
    candidates: Vec<GeneratedCandidate>,
    has_standalone_full: bool,
}

struct PrefixCandidates {
    raw: String,
    candidates: std::sync::Arc<Vec<GeneratedCandidate>>,
    has_standalone_full: bool,
    inherited_order: bool,
}

#[cfg(test)]
fn generate(raw: &str, entries: &[learning::Entry], tick: u64) -> Vec<GeneratedCandidate> {
    generate_with_order(raw, entries, tick, true)
}

#[cfg(test)]
fn generate_with_order(raw: &str, entries: &[learning::Entry], tick: u64, inherited_order: bool) -> Vec<GeneratedCandidate> {
    generate_candidates(raw, entries, tick, inherited_order, false)
}

#[cfg(test)]
fn generate_candidates(raw: &str, entries: &[learning::Entry], tick: u64, inherited_order: bool, whole: bool) -> Vec<GeneratedCandidate> {
    generate_candidates_cached(raw, entries, tick, inherited_order, whole, None, &[]).candidates
}

fn generate_candidates_cached(raw: &str, entries: &[learning::Entry], tick: u64, inherited_order: bool, whole: bool,
    lattice: Option<(&mut Option<crate::mixer::WholeLattice>, u64)>, prefixes: &[PrefixCandidates]) -> GeneratedCandidates {
    #[cfg(test)]
    let _timer = InnerTimer(0, std::time::Instant::now());
    use std::collections::HashSet;
    if raw.is_empty() || raw.len() > 120 || raw.starts_with('\'')
        || !raw.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'\'') {
        return GeneratedCandidates { candidates: Vec::new(), has_standalone_full: false };
    }
    let index = super::current();
    let personal = super::learning_index();
    let dictionaries: Vec<_> = std::iter::once(index.as_ref()).chain(personal.as_deref()).collect();
    // Like the whole-word source, inherited Rime scores rank matching words
    // ahead of static weights. A reading match keeps homographs independent.
    let mut learned = std::collections::HashMap::new();
    if inherited_order {
        for entry in entries.iter().filter(|entry| !entry.is_learned() && entry.text.chars().count() >= 2) {
            let score = entry.score(tick);
            learned.entry((entry.text.as_str(), entry.reading.as_str()))
                .and_modify(|kept: &mut f64| *kept = kept.max(score)).or_insert(score);
        }
    }
    let learned_score = |text: &str, reading: &str| learned.get(&(text, reading)).copied();
    let compare_words = |a: &(usize, u32, &str, &str), b: &(usize, u32, &str, &str)|
        b.0.cmp(&a.0)
            .then_with(|| learned_score(b.2, b.3).partial_cmp(&learned_score(a.2, a.3))
                .unwrap_or(std::cmp::Ordering::Equal))
            .then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)).then(a.3.cmp(&b.3));
    let mut words = Vec::new();
    for dictionary in &dictionaries {
        dictionary.visit_words_for_prefixes(raw, |end, text, reading, weight| {
            if reading.split_whitespace().all(|syllable| !matches!(syllable, "m" | "n" | "ng" | "hm" | "hng"))
                && text.chars().all(|ch| ch == '〇' || ('\u{3400}'..='\u{9fff}').contains(&ch))
                && exact_spelling(&raw[..end], reading) {
                words.push((end, weight, text, reading));
            }
        });
    }
    // The complete-reading trie retains rare words; initials deliberately use the
    // existing correction projection, including the pinned learning dictionary.
    let mut boundaries = HashSet::new();
    let mut spellings = Vec::new();
    visit_spellings(raw, |end, tokens| spellings.push((end, tokens.to_vec())));
    let abbreviated = !spellings.iter().any(|(end, tokens)| *end == raw.len()
        && tokens.iter().all(|token| full_syllable(token)));
    let lookup_spelling = |end, tokens: &[&str], words: &mut Vec<_>| {
        let key: String = tokens.iter().filter_map(|token| token.chars().next()).collect();
        for dictionary in &dictionaries {
            let compact: String = tokens.iter().map(|token| canonical_syllable(token)).collect();
            for (text, reading, weight) in dictionary.all_words_for_compact_reading(&compact) {
                if text.chars().all(|ch| ch == '〇' || ('\u{3400}'..='\u{9fff}').contains(&ch))
                    && reading.split_whitespace().count() == tokens.len()
                    && reading.split_whitespace().zip(tokens).all(|(syllable, token)|
                        canonical_syllable(token) == canonical_syllable(syllable)) {
                    words.push((end, weight, text, reading));
                }
            }
            for (text, reading, weight) in dictionary.words_for_initials(&key).filter(|_| abbreviated) {
                if reading.split_whitespace().zip(tokens).all(|(syllable, token)|
                    syllable_matches(token, syllable)) {
                    words.push((end, weight, text, reading));
                }
            }
            if abbreviated && tokens.iter().any(|token| initial(token)) {
                visit_complete_initials(dictionary, tokens, |text, reading, weight| {
                    words.push((end, weight, text, reading));
                });
            }
        }
    };
    // Fuzzy groups read each syllable under its alternative spellings too. The
    // input consumed is unchanged, so every variant keeps the same end. This is
    // lookup_spelling over every variant sequence, with each condition checked
    // per position so dead trie prefixes and initials keys prune the product.
    let fuzzy_mask = super::fuzzy::mask();
    let lookup = |end, tokens: &[&str], words: &mut Vec<_>| {
        if fuzzy_mask == 0 { return lookup_spelling(end, tokens, words); }
        let variants: Vec<Vec<String>> = tokens.iter()
            .map(|token| super::fuzzy::token_variants(token, fuzzy_mask)).collect();
        let canonical: Vec<Vec<String>> = variants.iter()
            .map(|part| part.iter().map(|spelling| canonical_syllable(spelling).into_owned()).collect()).collect();
        // Each initials key is one first letter per position.
        let mut keys = vec![String::new()];
        for part in &variants {
            let mut letters: Vec<char> = part.iter().filter_map(|spelling| spelling.chars().next()).collect();
            letters.dedup();
            keys = keys.iter().flat_map(|key| letters.iter().map(move |&letter| format!("{key}{letter}"))).collect();
        }
        let position_matches = |position: usize, letter: char, syllable: &str|
            variants[position].iter().any(|spelling| spelling.starts_with(letter) && syllable_matches(spelling, syllable));
        let complete_initials = abbreviated && tokens.iter().any(|token| initial(token));
        for dictionary in &dictionaries {
            dictionary.visit_compact_paths(&canonical, |path, text, reading, weight| {
                if text.chars().all(|ch| ch == '〇' || ('\u{3400}'..='\u{9fff}').contains(&ch))
                    && reading.split_whitespace().count() == tokens.len()
                    && reading.split_whitespace().zip(path).enumerate().all(|(position, (syllable, &choice))|
                        canonical[position][choice] == canonical_syllable(syllable)) {
                    words.push((end, weight, text, reading));
                }
            });
            if !abbreviated { continue; }
            for key in &keys {
                for (text, reading, weight) in dictionary.words_for_initials(key) {
                    if reading.split_whitespace().zip(key.chars()).enumerate()
                        .all(|(position, (syllable, letter))| position_matches(position, letter, syllable)) {
                        words.push((end, weight, text, reading));
                    }
                }
                if !complete_initials { continue; }
                let first = key.chars().next().unwrap();
                let ids = dictionary.complete_initials().get(key).map_or(&[][..], Vec::as_slice);
                for &id in ids {
                    let word = &dictionary.entries[id as usize];
                    let reading = dictionary.readings.get(word.reading);
                    let text = dictionary.texts.get(word.text);
                    let mut syllables = reading.split_whitespace();
                    let leads = syllables.next().is_some_and(|syllable| variants[0].iter().any(|spelling|
                        spelling.starts_with(first) && reading.starts_with(canonical_syllable(spelling).as_ref())
                            && syllable_matches(spelling, syllable)));
                    if leads && reading.split_whitespace().count() == tokens.len()
                        && text.chars().all(|ch| ch == '〇' || ('\u{3400}'..='\u{9fff}').contains(&ch))
                        && syllables.zip(key.chars().skip(1)).enumerate()
                            .all(|(position, (syllable, letter))| position_matches(position + 1, letter, syllable)) {
                        words.push((end, word.weight, text, reading));
                    }
                }
            }
        }
    };
    for (end, tokens) in &spellings {
        boundaries.insert(*end);
        lookup(*end, tokens, &mut words);
    }
    words.retain(|word| (word.0 == raw.len() || boundaries.contains(&word.0))
        && word.3.split_whitespace().all(|syllable| !matches!(syllable, "m" | "n" | "ng" | "hm" | "hng")));
    words.sort_by(compare_words);
    let mut seen = HashSet::new();
    // Duplicate dictionary paths need no owned text merely to discard it.
    words.retain(|word| !seen.contains(word.2) && seen.insert(word.2.to_string()));
    let mut weights: std::collections::HashMap<_, _> = words.iter().map(|word| (word.2, word.1)).collect();
    let mut full = words.iter().take_while(|word| word.0 == raw.len()).count();
    let mut result: Vec<_> = words.into_iter().map(|(consumed, _, text, reading_key)| GeneratedCandidate {
        candidate: Candidate { text: text.to_string(), reading_key: reading_key.to_string(), ..Candidate::default() }, consumed,
    }).collect();
    #[cfg(test)]
    let start = std::time::Instant::now();
    let generated = crate::mixer::decoder_whole_candidates_cached(raw, entries, tick, inherited_order, lattice);
    #[cfg(test)]
    INNER_TIMING.with(|timing| timing.borrow_mut()[2] += start.elapsed().as_nanos());
    for candidate in generated.iter().filter(|candidate| candidate.comment == "整串 · 顺手·知序") {
        if seen.insert(candidate.text.clone()) {
            let mut candidate = candidate.clone();
            if let Some(entry) = entries.iter().find(|entry| entry.text == candidate.text
                && entry.reading.replace(' ', "") == raw.replace('\'', "")) {
                candidate.reading_key = entry.reading.clone();
            }
            if !whole { candidate.comment.clear(); }
            result.insert(full, GeneratedCandidate { candidate, consumed: raw.len() });
            full += 1;
        }
    }
    let learned_order: std::collections::HashMap<_, _> = generated.iter().enumerate()
        .filter(|(_, candidate)| candidate.comment == "整串 · 顺手·知序")
        .map(|(rank, candidate)| (candidate.text.as_str(), rank)).collect();
    result[..full].sort_by_key(|item| learned_order.get(item.candidate.text.as_str()).copied().unwrap_or(usize::MAX));
    // The standalone Rime contract has one composed sentence. Product sessions
    // retain block 3's remaining alternatives after the native prefix words.
    if full == 0 {
        // The lattice already orders sentences by score. Mixer's "备选" tag
        // marks another syllable count, not a lower score for this decoder.
        let sentence = generated.iter().find(|candidate|
            matches!(candidate.comment.as_str(), "整句 · 顺手·知序" | "备选 · 顺手·知序")).cloned().or_else(|| {
            // The correction lattice accepts arbitrary syllable prefixes. Native
            // composition accepts full syllables and the schema's initials only.
            // Complete a lexical prefix through the same constrained dictionary.
            let first = result.first()?;
            if first.consumed == 0 || first.consumed >= raw.len() { return None; }
            let rest = raw[first.consumed..].trim_start_matches('\'');
            let next = generate_first_full_cached(rest, entries, tick, inherited_order, prefixes)?;
            Some(Candidate { text: format!("{}{}", first.candidate.text, next.candidate.text),
                reading_key: format!("{} {}", first.candidate.reading_key, next.candidate.reading_key),
                ..Candidate::default() })
        });
        if let Some(mut candidate) = sentence {
            if seen.insert(candidate.text.clone()) {
                if !whole { candidate.comment.clear(); }
                result.insert(0, GeneratedCandidate { candidate, consumed: raw.len() });
            }
        }
    }
    if let Some(end) = boundaries.iter().copied().max().filter(|&end| end < raw.len() && full == 0) {
        if let Some(candidate) = generate_first_full_cached(&raw[..end], entries, tick, inherited_order, prefixes) {
            if seen.insert(candidate.candidate.text.clone()) { result.insert(0, candidate); }
        }
    }
    // An exact ASCII dictionary entry is useful even when there is no Chinese
    // parse. Rime's invalid-spelling fallback itself has no selectable raw echo.
    for dictionary in &dictionaries {
        for (text, reading, _) in dictionary.all_words_for_compact_reading(raw) {
            if text.is_ascii() && seen.insert(text.to_string()) {
                let ascii = GeneratedCandidate { candidate: Candidate {
                    text: text.into(), reading_key: reading.into(), ..Candidate::default()
                }, consumed: raw.len() };
                if full == 0 { result.insert(0, ascii); } else { result.push(ascii); }
            }
        }
    }
    if abbreviated {
        // Native deep pages also retain nonpreferred dictionary spellings such
        // as c+e+s in cesh. Sentence generation above still uses the shortest
        // abbreviation path; partial words share the same frequency ordering.
        let preferred: HashSet<_> = spellings.iter()
            .map(|(end, tokens)| (*end, tokens.as_slice())).collect();
        let mut tail = Vec::new();
        visit_spellings_with(raw, false, |end, tokens| {
            // Allowed words from these exact paths are already in seen.
            if !preferred.contains(&(end, tokens)) { lookup(end, tokens, &mut tail); }
        });
        tail.sort_by(compare_words);
        for (consumed, weight, text, reading_key) in tail {
            if reading_key.split_whitespace().all(|syllable| !matches!(syllable, "m" | "n" | "ng" | "hm" | "hng"))
                && !seen.contains(text) && seen.insert(text.to_string()) {
                weights.insert(text, weight);
                result.push(GeneratedCandidate { candidate: Candidate {
                    text: text.to_string(), reading_key: reading_key.to_string(), ..Candidate::default()
                }, consumed });
            }
        }
    }
    // Every whole match precedes partial matches, including alternate initials.
    result.sort_by(|a, b| b.consumed.cmp(&a.consumed).then_with(|| {
        if a.consumed == raw.len() { std::cmp::Ordering::Equal }
        else {
            learned_score(&b.candidate.text, &b.candidate.reading_key)
                .partial_cmp(&learned_score(&a.candidate.text, &a.candidate.reading_key))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| weights.get(b.candidate.text.as_str()).cmp(&weights.get(a.candidate.text.as_str())))
        }
    }));
    let has_standalone_full = result.first().is_some_and(|item| item.consumed == raw.len());
    if whole {
        let mut sentence = false;
        for mut candidate in generated {
            if candidate.comment == "整串 · 顺手·知序" { continue; }
            let first = candidate.comment == "整句 · 顺手·知序" && !sentence;
            sentence |= first;
            if !seen.insert(candidate.text.clone()) { continue; }
            if first {
                let at = result.iter().take_while(|item| item.consumed == raw.len()).count();
                result.insert(at, GeneratedCandidate { candidate, consumed: raw.len() });
            } else {
                candidate.comment = "备选 · 顺手·知序".into();
                result.push(GeneratedCandidate { candidate, consumed: raw.len() });
            }
        }
    }
    GeneratedCandidates { candidates: result, has_standalone_full }
}

fn generate_first_full_cached(raw: &str, entries: &[learning::Entry], tick: u64, inherited_order: bool,
    prefixes: &[PrefixCandidates]) -> Option<GeneratedCandidate> {
    // Only refresh supplies entries after its dictionary, learning and bonus guards.
    if let Some(cached) = prefixes.iter().find(|cached| cached.raw == raw
        && cached.has_standalone_full && cached.inherited_order == inherited_order) {
        if let Some(first) = cached.candidates.first().filter(|first| first.consumed == raw.len()) {
            let mut first = first.clone();
            first.candidate.comment.clear();
            return Some(first);
        }
    }
    generate_candidates_cached(raw, entries, tick, inherited_order, false, None, prefixes).candidates
        .into_iter().find(|item| item.consumed == raw.len())
}

pub(super) fn canonical_syllable(token: &str) -> std::borrow::Cow<'_, str> {
    if !token.contains("uen") && !token.contains("uei") && !token.contains("iou")
        && !matches!(token, "nve" | "lve")
        && !(token.starts_with(['j', 'q', 'x', 'y']) && token.as_bytes().get(1) == Some(&b'v')) {
        return std::borrow::Cow::Borrowed(token);
    }
    let mut spelling = token.to_string();
    if spelling.starts_with(['j', 'q', 'x', 'y']) && spelling.as_bytes().get(1) == Some(&b'v') {
        spelling.replace_range(1..2, "u");
    }
    if matches!(spelling.as_str(), "nve" | "lve") { spelling.replace_range(1..2, "u"); }
    spelling = spelling.replace("uen", "un").replace("uei", "ui").replace("iou", "iu");
    std::borrow::Cow::Owned(spelling)
}

pub(super) fn initial(token: &str) -> bool {
    matches!(token, "zh" | "ch" | "sh")
        || (token.len() == 1 && b"aeobpmfdtnlgkhjqxrzcsyw".contains(&token.as_bytes()[0]))
}

pub(super) fn full_syllable(token: &str) -> bool {
    let canonical = canonical_syllable(token);
    let spelling = match canonical.as_ref() { "nue" => "nve", "lue" => "lve", other => other };
    crate::mixer::pinyin_syllables().contains(spelling)
        && !matches!(spelling, "m" | "n" | "ng" | "hm" | "hng")
}

fn visit_complete_initials<'a>(dictionary: &'a Index, tokens: &[&str],
    mut visitor: impl FnMut(&'a str, &'a str, u32)) {
    #[cfg(test)]
    let _timer = InnerTimer(1, std::time::Instant::now());
    let first = canonical_syllable(tokens[0]);
    let key: String = tokens.iter().filter_map(|token| token.chars().next()).collect();
    let ids = dictionary.complete_initials().get(&key).map_or(&[][..], Vec::as_slice);
    for &id in ids {
        let word = &dictionary.entries[id as usize];
        let reading = dictionary.readings.get(word.reading);
        let text = dictionary.texts.get(word.text);
        if reading.starts_with(first.as_ref()) && reading.split_whitespace().count() == tokens.len()
            && text.chars().all(|ch| ch == '〇' || ('\u{3400}'..='\u{9fff}').contains(&ch))
            && reading.split_whitespace().zip(tokens).all(|(syllable, token)| syllable_matches(token, syllable)) {
            visitor(text, reading, word.weight);
        }
    }
}

fn syllable_matches(token: &str, syllable: &str) -> bool {
    token == syllable || (initial(token) && syllable.starts_with(token))
        || canonical_syllable(token) == canonical_syllable(syllable)
}

/// Native candidates carry complete syllables; a generated raw echo is not a
/// pronunciation. Follow only this reading through the schema's spelling edges.
pub(crate) fn candidate_coverage(raw: &str, candidate: &Candidate) -> Option<usize> {
    if raw.is_empty() || raw.len() > 120 || raw.starts_with('\'')
        || !raw.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'\'') {
        return None;
    }
    let syllables: Vec<_> = candidate.reading_key.split_whitespace().collect();
    if syllables.is_empty() || syllables.len() != candidate.text.chars().count()
        || !syllables.iter().all(|syllable| full_syllable(syllable))
        || !candidate.text.chars().all(|ch| ch == '〇' || ('\u{3400}'..='\u{9fff}').contains(&ch)) {
        return None;
    }
    let mut positions = vec![false; raw.len() + 1];
    positions[0] = true;
    let mut covered = None;
    for (index, syllable) in syllables.iter().enumerate() {
        let mut next_positions = vec![false; raw.len() + 1];
        for start in 0..raw.len() {
            if !positions[start] { continue; }
            for end in start + 1..=raw.len().min(start + 6) {
                let token = &raw[start..end];
                if token.contains('\'') { break; }
                if (!initial(token) && !full_syllable(token)) || !syllable_matches(token, syllable) { continue; }
                let mut next = end;
                while raw.as_bytes().get(next) == Some(&b'\'') { next += 1; }
                next_positions[next] = true;
                if index + 1 == syllables.len() {
                    let end = if next == raw.len() { next } else { end };
                    covered = Some(covered.map_or(end, |previous: usize| previous.max(end)));
                }
            }
        }
        positions = next_positions;
    }
    covered
}

fn exact_spelling(raw: &str, reading: &str) -> bool {
    let mut offset = 0;
    for syllable in reading.split_whitespace() {
        if !raw[offset..].starts_with(syllable) { return false; }
        offset += syllable.len();
        while raw.as_bytes().get(offset) == Some(&b'\'') { offset += 1; }
    }
    offset == raw.len()
}

fn visit_spellings<'a>(raw: &'a str, mut visitor: impl FnMut(usize, &[&'a str])) {
    visit_spellings_with(raw, true, &mut visitor);
}

fn visit_spellings_with<'a>(raw: &'a str, preferred: bool, mut visitor: impl FnMut(usize, &[&'a str])) {
    let mut edges = vec![Vec::new(); raw.len() + 1];
    let mut reachable = vec![false; raw.len() + 1];
    reachable[0] = true;
    for start in 0..raw.len() {
        if !reachable[start] { continue; }
        for end in start + 1..=raw.len().min(start + 6) {
            let token = &raw[start..end];
            if token.contains('\'') { break; }
            let full = full_syllable(token);
            if !initial(token) && !full { continue; }
            let mut next = end;
            while raw.as_bytes().get(next) == Some(&b'\'') { next += 1; }
            edges[start].push((end, next));
            reachable[next] = true;
        }
    }
    let last = reachable.iter().rposition(|reachable| *reachable).unwrap_or(0);
    let mut abbreviations = vec![usize::MAX; raw.len() + 1];
    abbreviations[last] = 0;
    for start in (0..last).rev() {
        abbreviations[start] = edges[start].iter().filter(|&&(_, next)| abbreviations[next] != usize::MAX)
            .map(|&(end, next)| abbreviations[next] + usize::from(!full_syllable(&raw[start..end])))
            .min().unwrap_or(usize::MAX);
    }
    // Prefer complete syllables to splitting their interior into initials, even
    // when the last syllable is abbreviated (deng lu c, not d en g lu c).
    for (start, options) in edges.iter_mut().enumerate() {
        options.retain(|&(end, next)| abbreviations[next] != usize::MAX
            && (!preferred || abbreviations[start] == abbreviations[next] + usize::from(!full_syllable(&raw[start..end]))));
    }
    let reaches_last: Vec<_> = abbreviations.iter().map(|&cost| cost != usize::MAX).collect();
    fn walk<'a>(raw: &'a str, start: usize, preferred: bool, edges: &[Vec<(usize, usize)>], reaches_last: &[bool], tokens: &mut Vec<&'a str>,
        visitor: &mut impl FnMut(usize, &[&'a str])) {
        // The schema composes phrases of at most eight syllables. Longer exact
        // dictionary words are independently retained by the full-reading trie.
        if start == raw.len() || tokens.len() == 8 { return; }
        for &(end, next) in &edges[start] {
            if !reaches_last[next] { continue; }
            let token = &raw[start..end];
            tokens.push(token);
            visitor(end, tokens);
            if next == raw.len() && end < next { visitor(next, tokens); }
            walk(raw, next, preferred, edges, reaches_last, tokens, visitor);
            tokens.pop();
            // The schema accepts both sh and s+h (likewise zh/ch). Preserve
            // dictionary words and the first-initial prefix without reopening
            // arbitrary full-syllable interiors such as d+en+g in deng.
            if preferred && matches!(token, "zh" | "ch" | "sh") {
                tokens.push(&raw[start..start + 1]);
                visitor(start + 1, tokens);
                tokens.push(&raw[start + 1..end]);
                visitor(end, tokens);
                if next == raw.len() && end < next { visitor(next, tokens); }
                walk(raw, next, preferred, edges, reaches_last, tokens, visitor);
                tokens.pop();
                tokens.pop();
            }
        }
    }
    walk(raw, 0, preferred, &edges, &reaches_last, &mut Vec::new(), &mut visitor);
}

#[cfg(test)]
mod candidate_tests {
    use super::*;
    use super::super::loader::{DictionaryData, Entry};
    use std::sync::Arc;

    fn fixture() -> Arc<Index> {
        Arc::new(Index::from_data(DictionaryData { entries: [
            ("你", "ni", 9000), ("泥", "ni", 1000), ("好", "hao", 8000),
            ("哈", "ha", 1000), ("你好", "ni hao", 5000),
            ("世界", "shi jie", 6000), ("世", "shi", 1000), ("界", "jie", 1000),
            ("中", "zhong", 9000), ("国", "guo", 8000), ("人", "ren", 7000),
            ("中国", "zhong guo", 6000), ("中国人", "zhong guo ren", 5000),
            ("中间", "zhong jian", 4000), ("中级", "zhong ji", 3000),
            ("女", "nv", 2000), ("略", "lue", 2000), ("局", "ju", 2000),
            ("鋝", "lue", 20), ("佛", "fo", 2000), ("仏", "fo", 20), ("坲", "fo", 10),
            ("发", "fa", 5000),
            ("hello", "hello", 2000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "pinyin_simp".into(),
        }).collect() }))
    }

    #[test]
    fn y6_syllable_fast_paths_match_canonical_predicate_exhaustively() {
        let syllables = crate::mixer::pinyin_syllables();
        let mut spellings: Vec<String> = syllables.iter().map(|value| value.to_string()).collect();
        for syllable in syllables {
            spellings.push(syllable.replace("un", "uen").replace("ui", "uei").replace("iu", "iou"));
            if syllable.starts_with(['j', 'q', 'x', 'y']) && syllable.as_bytes().get(1) == Some(&b'u') {
                let mut alias = syllable.to_string();
                alias.replace_range(1..2, "v");
                spellings.push(alias);
            }
        }
        spellings.extend(["uen", "uei", "iou", "nve", "lve", "nue", "lue", "jv", "qv", "xv", "yv",
            "zh", "ch", "sh", "", "'", "中文"].into_iter().map(str::to_string));
        spellings.extend("aeobpmfdtnlgkhjqxrzcsyw".chars().map(|letter| letter.to_string()));
        spellings.sort();
        spellings.dedup();
        for token in &spellings {
            for syllable in &spellings {
                let (token, syllable) = (token.as_str(), syllable.as_str());
                let expected = canonical_syllable(token) == canonical_syllable(syllable)
                    || (initial(token) && syllable.starts_with(token));
                assert_eq!(syllable_matches(token, syllable), expected, "token={token:?} syllable={syllable:?}");
            }
        }
    }

    #[test]
    fn y4_complete_initial_index_matches_original_range_scan_before_and_after_prewarm() {
        let index = fixture();
        let cases = [vec!["zh", "g"], vec!["z", "g", "r"], vec!["ni", "h"], vec!["l"], vec!["n"], vec!["j"], vec!["f"]];
        for warm in [false, true] {
            if warm { index.warm_whole(); }
            for tokens in &cases {
                let first = canonical_syllable(tokens[0]);
                let expected: Vec<_> = index.by_reading.iter().filter_map(|&id| {
                    let word = &index.entries[id as usize];
                    let reading = index.readings.get(word.reading);
                    let text = index.texts.get(word.text);
                    (reading.starts_with(first.as_ref()) && reading.split_whitespace().count() == tokens.len()
                        && text.chars().all(|ch| ch == '〇' || ('\u{3400}'..='\u{9fff}').contains(&ch))
                        && reading.split_whitespace().zip(tokens).all(|(syllable, token)| syllable_matches(token, syllable)))
                        .then_some((text, reading, word.weight))
                }).collect();
                let mut actual = Vec::new();
                visit_complete_initials(&index, tokens, |text, reading, weight| actual.push((text, reading, weight)));
                assert_eq!(actual, expected, "{tokens:?} warm={warm}");
            }
        }
    }

    #[test]
    fn whole_word_sentence_and_longest_prefix_groups() {
        super::super::with_index(fixture(), || {
            let whole = generate("zhongguoren", &[], 0);
            assert_eq!(whole[0].candidate.text, "中国人");
            assert_eq!(whole[1].candidate.text, "中国");
            assert_eq!(whole[2].candidate.text, "中");
            assert_eq!(whole.iter().map(|item| item.consumed).collect::<Vec<_>>(), vec![11, 8, 5]);
            let sentence = generate("nihaoshijie", &[], 0);
            assert_eq!(sentence[0].candidate.text, "你好世界");
            assert_eq!(sentence[1].candidate.text, "你好");
        });
    }

    #[test]
    fn f1_fuzzy_groups_read_alternative_spellings_and_mask_zero_is_unchanged() {
        super::super::with_index(fixture(), || {
            let texts = |raw: &str| generate(raw, &[], 0).into_iter()
                .map(|item| (item.candidate.text, item.consumed)).collect::<Vec<_>>();
            let exact = texts("zhongguoren");
            {
                let _fuzzy = super::super::fuzzy::pin_mask(0);
                assert_eq!(texts("zhongguoren"), exact, "mask 0 is the exact path");
                assert!(!texts("zongguo").iter().any(|(text, _)| text == "中国"));
            }
            let _fuzzy = super::super::fuzzy::pin_mask(1 << 0);
            assert!(texts("zongguo").iter().any(|(text, consumed)| text == "中国" && *consumed == 7));
            assert!(texts("zongg").iter().any(|(text, _)| text == "中国"), "abbreviated tail");
            assert_eq!(texts("zhongguoren")[0], exact[0], "exact spelling keeps its whole word first");
            drop(_fuzzy);
            let _fuzzy = super::super::fuzzy::pin_mask(1 << 3);
            assert!(texts("li").iter().any(|(text, _)| text == "你"), "n/l");
            assert!(texts("lihao").iter().any(|(text, _)| text == "你好"));
        });
    }

    #[test]
    fn f1_decoder_fuzzy_mask_reaches_generation_and_resets_the_cache() {
        super::super::with_index(fixture(), || {
            let mut decoder = ZhixuDecoder::from_ready_dictionary(fixture());
            decoder.input("zongguo").unwrap();
            assert!(!decoder.candidates.iter().any(|item| item.candidate.text == "中国"));
            decoder.clear();
            decoder.configure_fuzzy(1 << 0).unwrap();
            decoder.input("zongguo").unwrap();
            assert!(decoder.candidates.iter().any(|item| item.candidate.text == "中国"));
            decoder.clear();
            assert_eq!(decoder.configure_fuzzy(1 << 8), Err(-12));
            decoder.configure_fuzzy(0).unwrap();
            decoder.input("zongguo").unwrap();
            assert!(!decoder.candidates.iter().any(|item| item.candidate.text == "中国"), "cache does not leak across masks");
        });
    }

    #[test]
    fn fuzzy_snapshot_hands_the_mixer_the_whole_pool_and_takes_pool_indexes() {
        // The mixer pages fuzzy candidates itself; one page here left every fuzzy reading
        // on a single last page (block 5 card 3: Swift fuzzy paging tests).
        let index = Arc::new(Index::from_data(DictionaryData { entries: [
            ("知", "zhi", 9000), ("之", "zhi", 8000), ("只", "zhi", 7000), ("值", "zhi", 6000),
            ("字", "zi", 5000), ("自", "zi", 4000), ("子", "zi", 3000), ("资", "zi", 2000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "pinyin_simp".into(),
        }).collect() }));
        super::super::with_index(index.clone(), || {
            let mut decoder = ZhixuDecoder::from_ready_dictionary(index.clone());
            decoder.configure_fuzzy(1 << 0).unwrap();
            decoder.input("zi").unwrap();
            let pool = decoder.snapshot();
            assert!(pool.candidates.len() > PAGE_SIZE, "pool {:?}", pool.candidates.iter().map(|c| &c.text).collect::<Vec<_>>());
            assert_eq!(pool.candidates.len(), decoder.candidates.len().min(FUZZY_CANDIDATE_POOL));
            assert!(pool.candidates.iter().enumerate().all(|(index, c)| c.source_index == Some(index)));
            let last = pool.candidates.len() - 1;
            let text = pool.candidates[last].text.clone();
            let committed = decoder.select_candidate(last, 0, LearningIntent::DeliberateSelection).unwrap();
            assert!(committed.ends_with(&text) || decoder.confirmed_text().ends_with(&text), "{committed} / {text}");
        });
    }

    #[test]
    fn first_syllable_singles_and_incomplete_tail() {
        super::super::with_index(fixture(), || {
            let singles = generate("ni", &[], 0);
            assert_eq!(singles.iter().map(|item| item.candidate.text.as_str()).collect::<Vec<_>>(), vec!["你", "泥"]);
            for raw in ["zhongj", "zhong'j"] {
                let candidates = generate(raw, &[], 0);
                assert_eq!(candidates[0].candidate.text, "中间");
                assert_eq!(candidates[1].candidate.text, "中级");
                assert_eq!(candidates[0].consumed, raw.len());
                assert!(candidates.iter().any(|item| item.candidate.text == "中" && item.consumed == 5));
            }
        });
    }

    #[test]
    fn initials_mixed_spelling_and_explicit_boundaries() {
        super::super::with_index(fixture(), || {
            for raw in ["zgr", "zhgr", "zhonggr", "zg'ren"] {
                assert_eq!(generate(raw, &[], 0)[0].candidate.text, "中国人", "{raw}");
            }
            assert_eq!(generate("ni'h", &[], 0)[0].candidate.text, "你好");
            assert_eq!(generate("ni''hao", &[], 0)[0].candidate.text, "你好");
            assert_eq!(generate("nhr", &[], 0)[0].candidate.text, "你好人");
            assert!(generate("n'ihao", &[], 0).iter().all(|item| item.candidate.text != "你好"));
        });
    }

    #[test]
    fn invalid_reading_has_no_echo_and_english_uses_the_dictionary() {
        super::super::with_index(fixture(), || {
            assert_eq!(generate("hello", &[], 0)[0].candidate.text, "hello");
            let echo = generate("vqqv", &[], 0);
            assert!(echo.is_empty());
            assert!(generate("i", &[], 0).is_empty());
            assert!(generate("'ni", &[], 0).is_empty());
        });
    }

    #[test]
    fn learning_overlay_uses_existing_pinned_index() {
        super::super::with_index(fixture(), || {
            let personal = Arc::new(Index::from_data(DictionaryData { entries: vec![Entry {
                text: "拟好".into(), reading: "ni hao".into(), weight: 800,
                layer: "agent_zhixu_learning".into(),
            }] }));
            let _learning = super::super::pin_learning(Some(personal));
            assert!(generate("nihao", &[], 0).iter().any(|item| item.candidate.text == "拟好"));
        });
    }

    #[test]
    fn complete_syllables_suppress_initial_prefixes_and_keep_rare_aliases() {
        super::super::with_index(fixture(), || {
            let fo = generate("fo", &[], 0);
            assert_eq!(fo.iter().map(|item| item.candidate.text.as_str()).collect::<Vec<_>>(), vec!["佛", "仏", "坲"]);
            assert!(generate("lve", &[], 0).iter().any(|item| item.candidate.text == "鋝"));
            assert!(generate("l", &[], 0).iter().any(|item| item.candidate.text == "鋝"));
            assert!(generate("nihaoi", &[], 0).iter().any(|item| item.candidate.text == "你好" && item.consumed == 5));
            assert!(generate("nihaoshijiev", &[], 0).iter().any(|item| item.candidate.text == "你好世界" && item.consumed == 11));
        });
    }

    #[test]
    fn y3c_abbreviated_candidates_restore_inherited_learning_order() {
        let index = Arc::new(Index::from_data(DictionaryData { entries: [
            ("生产", "sheng chan", 9000), ("生成", "sheng cheng", 1000),
            ("生", "sheng", 9000), ("声", "sheng", 1000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "public-test".into(),
        }).collect() }));
        super::super::with_index(index, || {
            let mut store = learning::LearningStore::default();
            store.import_rime_export("#@/tick\t1000\nsheng cheng \t生成\tc=7 d=8 t=999\nsheng \t声\tc=70 d=80 t=999\n", 100).unwrap();
            for raw in ["shengc", "sc", "shengch", "sheng'c"] {
                assert_eq!(generate(raw, &[], 0)[0].candidate.text, "生产", "{raw} without learning");
                let learned = generate(raw, store.entries(), store.rime_tick);
                assert_eq!(learned[0].candidate.text, "生成", "{raw} with migrated learning");
                assert_eq!(learned[0].candidate.reading_key, "sheng cheng");
                assert_eq!(learned[0].consumed, raw.len());
            }
            for raw in ["sheng", "sh"] {
                let candidates = generate(raw, store.entries(), store.rime_tick);
                let position = |text| candidates.iter().position(|item| item.candidate.text == text).unwrap();
                assert!(position("生") < position("声"), "single-character migration stays unpromoted: {raw}");
            }
        });
    }

    #[test]
    fn y3c_inherited_abbreviations_use_dynamic_scores_and_exact_readings() {
        let index = Arc::new(Index::from_data(DictionaryData { entries: [
            ("生产", "sheng chan", 9000), ("生成", "sheng cheng", 1000),
            ("生财", "sheng cai", 8000), ("常常", "chang chang", 9000), ("重传", "chong chuan", 7000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "public-test".into(),
        }).collect() }));
        super::super::with_index(index, || {
            let mut store = learning::LearningStore::default();
            store.import_rime_export("#@/tick\t1000\nsheng chan \t生产\tc=37 d=12 t=1\nsheng cheng \t生成\tc=7 d=8 t=999\nzhong chuan \t重传\tc=70 d=80 t=999\n", 100).unwrap();
            let position = |candidates: &[GeneratedCandidate], text: &str| candidates.iter()
                .position(|item| item.candidate.text == text).unwrap();
            for raw in ["shengc", "sc", "shengcaiy"] {
                let candidates = generate(raw, store.entries(), store.rime_tick);
                assert!(position(&candidates, "生成") < position(&candidates, "生产"), "{raw}: dynamic score wins over lifetime count");
                if raw == "shengcaiy" {
                    assert_eq!(candidates[0].candidate.text, "生财", "whole coverage stays ahead of learned first pieces");
                    assert_eq!(candidates[position(&candidates, "生成")].consumed, 6);
                }
            }
            let candidates = generate("cc", store.entries(), store.rime_tick);
            assert_eq!(candidates[0].candidate.text, "常常", "another pronunciation cannot promote 重传");
            let candidates = generate_with_order("shengc", store.entries(), store.rime_tick, false);
            assert_eq!(candidates[0].candidate.text, "生产", "bounded promotion belongs to the outer mixer");
        });
    }

    #[test]
    fn y3c_new_abbreviation_learning_never_uses_unbounded_inherited_scores() {
        let index = Arc::new(Index::from_data(DictionaryData { entries: [
            ("生产", "sheng chan", 9000), ("生成", "sheng cheng", 1000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "public-test".into(),
        }).collect() }));
        super::super::with_index(index, || {
            let mut store = learning::LearningStore::default();
            store.import_rime_export("#@/tick\t1000\nsheng cheng \t生成\tc=7 d=8 t=999\n", 100).unwrap();
            store.record("sheng cheng", "生成", 101);
            // The legacy raw gate does not yet recognize abbreviated readings.
            // This candidate-source change must not bypass bounded promotion.
            assert!(store.use_rime_order("shengc"));
            assert!(store.entries()[0].is_learned());
            assert_eq!(generate("shengc", store.entries(), store.rime_tick)[0].candidate.text, "生产");
        });
    }

    #[test]
    fn y3c_abbreviated_first_pieces_keep_exact_consumption() {
        let cases = [
            ("shangjai", "上架", "shang jia", 6), ("yigng", "义工", "yi gong", 3),
            ("shoukkuan", "收款", "shou kuan", 5), ("kuaije", "快捷", "kuai jie", 5),
            ("ganje", "感觉", "gan jue", 4), ("leiss", "类似", "lei si", 4),
            ("wozenmy", "我怎么", "wo zen me", 6), ("read", "热爱", "re ai", 3),
        ];
        let index = Arc::new(Index::from_data(DictionaryData { entries: cases.iter()
            .map(|&(_, text, reading, _)| Entry {
                text: text.into(), reading: reading.into(), weight: 5000, layer: "public-test".into(),
            }).collect() }));
        super::super::with_index(index, || {
            for (raw, text, reading, consumed) in cases {
                let candidates = generate(raw, &[], 0);
                let found = candidates.iter().find(|item| item.candidate.text == text)
                    .unwrap_or_else(|| panic!("missing first piece {text} for {raw}: {candidates:?}"));
                assert_eq!(found.consumed, consumed, "{raw}: {found:?}");
                assert_eq!(found.candidate.reading_key, reading);
            }
        });
    }

    #[test]
    fn y3e_candidate_coverage_uses_native_syllables_and_initials() {
        for (raw, text, reading, consumed) in [
            ("leiss", "类似", "lei si", 4), ("read", "热爱", "re ai", 3),
            ("wozenmy", "我怎么", "wo zen me", 6), ("ganje", "感觉", "gan jue", 4),
            ("renkd", "认可", "ren ke", 4), ("houmm", "后妈", "hou ma", 4),
            ("yigng", "义工", "yi gong", 3), ("app", "阿婆", "a po", 2),
            ("shangjai", "上架", "shang jia", 6), ("zgr", "中国人", "zhong guo ren", 3),
            ("shengc", "生成", "sheng cheng", 6), ("henkyi", "很可以", "hen ke yi", 6),
            ("clo", "丑陋", "chou lou", 2), ("sheng'c", "生成", "sheng cheng", 7),
            ("sheng'c'", "生成", "sheng cheng", 8), ("jv", "局", "ju", 2),
        ] {
            let candidate = Candidate { text: text.into(), reading_key: reading.into(), ..Candidate::default() };
            assert_eq!(candidate_coverage(raw, &candidate), Some(consumed), "{raw}: {candidate:?}");
        }
        for (raw, text, reading) in [
            ("leiss", "类似是", "leiss"), ("nihao", "你好", "nihao"),
            ("clo", "丑陋", "c l o"), ("clo", "丑陋", "chou l"),
            ("shi", "是时", "shi"), ("a", "", "a"), ("ng", "嗯", "ng"),
            ("", "是", "shi"), ("'shi", "是", "shi"), ("你好", "你好", "ni hao"),
        ] {
            let candidate = Candidate { text: text.into(), reading_key: reading.into(), ..Candidate::default() };
            assert_eq!(candidate_coverage(raw, &candidate), None, "{raw}: {candidate:?}");
        }
    }

    #[test]
    fn y3e_first_piece_structure_keeps_native_sentence_and_exact_prefix_coverage() {
        for (raw, text, reading, consumed, tail, tail_reading) in [
            ("leiss", "类似", "lei si", 4, "是", "shi"),
            ("read", "热爱", "re ai", 3, "的", "de"),
            ("wozenmy", "我怎么", "wo zen me", 6, "有", "you"),
            ("ganje", "感觉", "gan jue", 4, "鹅", "e"),
            ("renkd", "认可", "ren ke", 4, "的", "de"),
            ("houmm", "后妈", "hou ma", 4, "没", "mei"),
            ("yigng", "义工", "yi gong", 3, "那个", "na ge"),
            ("app", "阿婆", "a po", 2, "怕", "pa"),
            ("shangjai", "上架", "shang jia", 6, "爱", "ai"),
        ] {
            let index = Arc::new(Index::from_data(DictionaryData { entries: [
                (text, reading, 5000), (tail, tail_reading, 1000),
            ].into_iter().map(|(text, reading, weight)| Entry {
                text: text.into(), reading: reading.into(), weight, layer: "public-test".into(),
            }).collect() }));
            super::super::with_index(index, || {
                for whole in [false, true] {
                    let candidates = generate_candidates(raw, &[], 0, true, whole);
                    let prefix = candidates.iter().position(|item| item.candidate.text == text)
                        .unwrap_or_else(|| panic!("missing {text}: {raw}: {candidates:?}"));
                    // The public Rime probe also puts a composed sentence before
                    // the lexical prefix. Product ordering belongs to Mixer.
                    assert_eq!(candidates[0].consumed, raw.len(), "{raw}: {candidates:?}");
                    assert!(prefix > 0, "{raw}: {candidates:?}");
                    assert_eq!(candidates[prefix].consumed, consumed, "{raw}");
                    assert_eq!(candidates[prefix].candidate.reading_key, reading);
                }
            });
        }
    }

    #[test]
    fn y3e_complete_abbreviations_and_full_syllable_tails_keep_whole_first() {
        let index = Arc::new(Index::from_data(DictionaryData { entries: [
            ("中国人", "zhong guo ren", 5000), ("中国", "zhong guo", 6000),
            ("生成", "sheng cheng", 5000), ("生", "sheng", 6000),
            ("很可以", "hen ke yi", 5000), ("很可", "hen ke", 6000),
            ("类似", "lei si", 5000), ("是", "shi", 1000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "public-test".into(),
        }).collect() }));
        super::super::with_index(index, || {
            for (raw, expected) in [("zgr", "中国人"), ("shengc", "生成"),
                ("henkyi", "很可以"), ("leisishi", "类似是")] {
                for whole in [false, true] {
                    let candidates = generate_candidates(raw, &[], 0, true, whole);
                    assert_eq!(candidates[0].candidate.text, expected, "{raw}");
                    assert_eq!(candidates[0].consumed, raw.len(), "{raw}");
                }
            }
        });
    }

    #[test]
    fn whole_candidates_reuse_existing_learning_order_and_entries() {
        super::super::with_index(fixture(), || {
            let entries = vec![learning::Entry { reading: "ni hao".into(), text: "拟好".into(),
                count: 10, last_ms: 0, rime: None }];
            let candidates = generate("nihao", &entries, 0);
            assert_eq!(candidates[0].candidate.text, "拟好");
            assert_eq!(candidates[0].candidate.reading_key, "ni hao");
        });
    }

    #[test]
    fn complete_dictionary_syllables_keep_their_native_page() {
        let index = Arc::new(Index::from_data(DictionaryData { entries: [
            ("扽", "den", 30), ("扥", "den", 20), ("的", "de", 9000),
            ("得", "de", 8000), ("地", "de", 7000), ("你", "ni", 6000),
            ("嗯", "n", 5000), ("hello", "hello", 1000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "pinyin_simp".into(),
        }).collect() }));
        super::super::with_index(index, || {
            assert!(full_syllable("den"));
            assert!(!full_syllable("n"));
            assert!(!full_syllable("hello"));
            let candidates = generate("den", &[], 0);
            assert_eq!(candidates.iter().map(|item| item.candidate.text.as_str()).collect::<Vec<_>>(), vec!["扽", "扥"]);
        });
    }

    #[test]
    fn digraph_initials_keep_two_syllable_words_and_shorter_prefixes() {
        let index = Arc::new(Index::from_data(DictionaryData { entries: [
            ("是", "shi", 9000), ("说", "shuo", 8000), ("上", "shang", 7000),
            ("时", "shi", 6000), ("时候", "shi hou", 5000),
            ("测试", "ce shi", 9000), ("侧身", "ce shen", 8000),
            ("厕所", "ce suo", 7000), ("测算", "ce suan", 6000), ("测速", "ce su", 5000),
            ("测", "ce", 10000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "pinyin_simp".into(),
        }).collect() }));
        super::super::with_index(index, || {
            assert!(generate("sh", &[], 0).iter().any(|item| item.candidate.text == "时候" && item.consumed == 2));
            let candidates = generate("cesh", &[], 0);
            assert_eq!(candidates.iter().take(5).map(|item| (item.candidate.text.as_str(), item.consumed)).collect::<Vec<_>>(),
                vec![("测试", 4), ("侧身", 4), ("厕所", 3), ("测算", 3), ("测速", 3)]);
        });
    }

    #[test]
    fn complete_vowel_syllables_do_not_expand_as_initials() {
        let index = Arc::new(Index::from_data(DictionaryData { entries: [
            ("系", "xi", 9000), ("啊", "a", 8000), ("西安", "xi an", 10000), ("喜爱", "xi ai", 9000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "pinyin_simp".into(),
        }).collect() }));
        super::super::with_index(index, || {
            let candidates = generate("xi'a", &[], 0);
            assert_eq!(candidates[0].candidate.text, "系啊");
            assert!(candidates.iter().all(|item| !matches!(item.candidate.text.as_str(), "西安" | "喜爱")));
        });
    }

    #[test]
    fn initial_lookup_keeps_rare_dictionary_syllables_outside_the_static_table() {
        let index = Arc::new(Index::from_data(DictionaryData { entries: [
            ("发", "fa", 5000), ("覅", "fiao", 6),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "pinyin_simp".into(),
        }).collect() }));
        super::super::with_index(index, || {
            let candidates = generate("f", &[], 0);
            assert!(candidates.iter().any(|item| item.candidate.text == "覅"
                && item.candidate.reading_key == "fiao" && item.consumed == 1));
        });
    }

    #[test]
    fn incomplete_input_retains_initial_prefixes_after_the_primary_groups() {
        let index = Arc::new(Index::from_data(DictionaryData { entries: [
            ("授课", "shou ke", 10000), ("收", "shou", 9000), ("是", "shi", 8000),
            ("说", "shuo", 7000), ("三", "san", 6000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "pinyin_simp".into(),
        }).collect() }));
        super::super::with_index(index, || {
            let candidates = generate("shouk", &[], 0);
            assert_eq!(candidates.iter().map(|item| (item.candidate.text.as_str(), item.consumed)).collect::<Vec<_>>(),
                vec![("授课", 5), ("收", 4), ("是", 2), ("说", 2), ("三", 1)]);
        });
    }

    #[test]
    fn deep_pages_keep_nonpreferred_spellings_in_coverage_groups() {
        let index = Arc::new(Index::from_data(DictionaryData { entries: [
            ("测试", "ce shi", 9000), ("测", "ce", 8000), ("从", "cong", 7000),
            ("嫦娥", "chang e", 6000), ("查尔斯", "cha er si", 5000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "pinyin_simp".into(),
        }).collect() }));
        super::super::with_index(index, || {
            let candidates = generate("cesh", &[], 0);
            assert_eq!(candidates[0].candidate.text, "测试");
            assert_eq!(candidates[1].candidate.text, "查尔斯");
            assert!(candidates.windows(2).all(|pair| pair[0].consumed >= pair[1].consumed));
            for (text, consumed) in [("查尔斯", 3), ("嫦娥", 2), ("从", 1)] {
                assert!(candidates.iter().any(|item| item.candidate.text == text && item.consumed == consumed));
            }
        });
    }
}

use crate::state::{BoundaryPurpose, CompositionBoundary, Decoder, DecoderSnapshot, LearningIntent};
use super::learning_session::LearningSession;

const PAGE_SIZE: usize = 5;
const PREFIX_CANDIDATE_LIMIT: usize = 512;
/// With fuzzy pinyin on, the mixer pages a whole candidate pool itself and hands
/// back indexes into that pool (the contract librime's fuzzy session kept).
const FUZZY_CANDIDATE_POOL: usize = 256;
const PREFIX_CANDIDATE_BYTES: usize = 16 * 1024 * 1024;

fn candidate_cache_payload(prefix: &String, candidates: &Vec<GeneratedCandidate>) -> usize {
    prefix.capacity() + std::mem::size_of::<Vec<GeneratedCandidate>>() + 2 * std::mem::size_of::<usize>()
        + candidates.capacity() * std::mem::size_of::<GeneratedCandidate>()
        + candidates.iter().map(|item| item.candidate.text.capacity() + item.candidate.comment.capacity()
            + item.candidate.reading_key.capacity()).sum::<usize>()
}

#[derive(Clone)]
struct ConfirmedSegment {
    text: String,
    end: usize,
    candidate: Candidate,
}

/// Native segmented composition with a stable whole-input editing menu.
/// Construction never opens a runtime directory; learning uses the existing session.
pub struct ZhixuDecoder {
    raw: String,
    preedit: String,
    caret: usize,
    cursor_utf8: usize,
    caret_focused: bool,
    letter_caret: bool,
    caret_stops: Vec<usize>,
    confirmed: Vec<ConfirmedSegment>,
    undo_confirmation: bool,
    candidates: std::sync::Arc<Vec<GeneratedCandidate>>,
    // Undoing a confirmed segment restores its spelling boundary. Menu
    // navigation must not replace that boundary with the highlighted word.
    restored_preedit: Option<GeneratedCandidate>,
    selected: usize,
    learning: LearningSession,
    whole: bool,
    dictionary: Option<std::sync::Arc<Index>>,
    lattice: std::cell::RefCell<Option<crate::mixer::WholeLattice>>,
    prefix_candidates: Vec<PrefixCandidates>,
    prefix_candidate_bytes: usize,
    prefix_generation: Option<(u64, u64, u64, usize)>,
    prefix_bonuses: Vec<i64>,
    fuzzy_mask: u32,
}

impl Default for ZhixuDecoder {
    fn default() -> Self { Self::new() }
}

impl ZhixuDecoder {
    pub fn new() -> Self {
        Self { raw: String::new(), preedit: String::new(), caret: 0, cursor_utf8: 0,
            caret_focused: false, letter_caret: false, caret_stops: Vec::new(), confirmed: Vec::new(), undo_confirmation: false, candidates: std::sync::Arc::default(),
            restored_preedit: None, selected: 0, learning: LearningSession::default(), whole: false, dictionary: None, lattice: std::cell::RefCell::new(None), prefix_candidates: Vec::new(), prefix_candidate_bytes: 0, prefix_generation: None, prefix_bonuses: Vec::new(), fuzzy_mask: 0 }
    }

    pub(crate) fn from_ready_dictionary(dictionary: std::sync::Arc<Index>) -> Self {
        Self { dictionary: Some(dictionary), whole: true, ..Self::new() }
    }

    pub(crate) fn attach_learning(&mut self, directory: &std::path::Path) {
        self.learning.attach(directory);
    }

    fn confirmed_end(&self) -> usize {
        self.confirmed.last().map_or(0, |segment| segment.end)
    }

    fn confirmed_text(&self) -> String {
        self.confirmed.iter().map(|segment| segment.text.as_str()).collect()
    }

    fn generate(&self, raw: &str) -> Vec<GeneratedCandidate> {
        let dictionary = self.dictionary.clone().unwrap_or_else(super::current);
        let _dictionary = super::pin_snapshot(Some(dictionary));
        let _learning = super::pin_learning(self.learning.whole_overlay().or_else(super::learning_index));
        let _fuzzy = super::fuzzy::pin_mask(self.fuzzy_mask);
        generate_candidates_cached(raw, &self.learning.entries(), self.learning.rime_tick(),
            self.learning.use_rime_order(raw), self.whole,
            Some((&mut self.lattice.borrow_mut(), self.learning.revision())), &[]).candidates
    }

    fn refresh(&mut self) {
        let dictionary = self.dictionary.clone().unwrap_or_else(super::current);
        let personal = self.learning.whole_overlay().or_else(super::learning_index);
        let generation = (dictionary.generation, personal.as_ref().map_or(0, |index| index.generation),
            self.learning.revision(), self.confirmed_end());
        let now = crate::personal_picks::now_ms();
        let bonuses: Vec<_> = self.learning.entries().iter().map(|entry|
            crate::mixer::whole_learning_bonus(entry, self.learning.rime_tick(), now)).collect();
        if self.prefix_generation != Some(generation) || self.prefix_bonuses != bonuses {
            self.prefix_candidates.clear();
            self.prefix_candidate_bytes = 0;
            self.prefix_generation = Some(generation);
            self.prefix_bonuses = bonuses;
        }
        let raw = &self.raw[self.confirmed_end()..];
        // Retain a bounded LRU across edits and composition boundaries. Short
        // prefixes recur often, but remain valid only under the generation above.
        self.candidates = if let Some(position) = self.prefix_candidates.iter().position(|cached| cached.raw == raw) {
            let cached = self.prefix_candidates.remove(position);
            let candidates = cached.candidates.clone();
            self.prefix_candidates.push(cached);
            candidates
        } else {
            let _dictionary = super::pin_snapshot(Some(dictionary));
            let _personal = super::pin_learning(personal);
            let _fuzzy = super::fuzzy::pin_mask(self.fuzzy_mask);
            let entries = self.learning.entries();
            let tick = self.learning.rime_tick();
            let inherited_order = self.learning.use_rime_order(raw);
            let generated = generate_candidates_cached(raw, &entries, tick,
                inherited_order, self.whole, Some((&mut self.lattice.borrow_mut(), self.learning.revision())),
                &self.prefix_candidates);
            let has_standalone_full = generated.has_standalone_full;
            let candidates = std::sync::Arc::new(generated.candidates);
            if !raw.is_empty() && raw.len() <= 120 {
                let prefix = raw.to_string();
                let bytes = candidate_cache_payload(&prefix, &candidates);
                if self.prefix_candidates.capacity() == 0 {
                    self.prefix_candidates.reserve_exact(PREFIX_CANDIDATE_LIMIT);
                }
                let maximum = PREFIX_CANDIDATE_BYTES - self.prefix_candidates.capacity()
                    * std::mem::size_of::<PrefixCandidates>();
                // Count the actual slot allocation as well as candidate storage.
                if bytes <= maximum {
                    while self.prefix_candidates.len() >= PREFIX_CANDIDATE_LIMIT
                        || self.prefix_candidate_bytes + bytes > maximum {
                        let oldest = self.prefix_candidates.iter().position(|cached| cached.raw.len() > 4).unwrap_or(0);
                        let cached = self.prefix_candidates.remove(oldest);
                        self.prefix_candidate_bytes -= candidate_cache_payload(&cached.raw, &cached.candidates);
                    }
                    self.prefix_candidate_bytes += bytes;
                    self.prefix_candidates.push(PrefixCandidates { raw: prefix, candidates: candidates.clone(),
                        has_standalone_full, inherited_order });
                }
            }
            candidates
        };
        self.restored_preedit = None;
        self.selected = 0;
        self.caret_focused = false;
        self.letter_caret = self.confirmed.is_empty() && self.caret > 0 && self.caret < self.raw.len()
            && !full_syllable(&self.raw[..self.caret])
            && !format_preedit(&self.raw[..self.caret]).contains([' ', '\'']);
        self.caret_stops.clear();
        self.refresh_preedit();
    }

    fn refresh_preedit(&mut self) {
        let start = self.confirmed_end();
        let raw = &self.raw[start..];
        let caret = self.caret.saturating_sub(start).min(raw.len());
        let confirmed = self.confirmed_text();
        let selected = if self.caret_focused || caret == 0 { self.candidates.first() }
            else if !self.confirmed.is_empty() { self.candidates.get(self.selected) }
            else { self.restored_preedit.as_ref().or_else(|| self.candidates.first()) };
        let (rendered, cursor) = if caret > 0 && caret < raw.len() {
            let focused = self.generate(&raw[..caret]);
            let candidate = selected.filter(|item| !self.caret_focused && item.consumed <= caret)
                .or_else(|| focused.first());
            let end = candidate.map_or(caret, |item| item.consumed.min(caret));
            let mut rendered = self.active_preedit(&raw[..end], candidate);
            let cursor = rendered.len() + caret - end;
            rendered.push_str(&raw[end..]);
            (rendered, cursor)
        } else {
            let whole = if self.confirmed.is_empty() { self.restored_preedit.is_none() } else { self.selected == 0 };
            let end = if self.caret_focused || caret == 0 || whole { raw.len() }
                else { selected.map_or(raw.len(), |item| item.consumed) };
            let mut rendered = self.active_preedit(&raw[..end], selected);
            rendered.push_str(&raw[end..]);
            let cursor = if caret == 0 { 0 } else { rendered.len() };
            (rendered, cursor)
        };
        self.cursor_utf8 = confirmed.len() + cursor;
        self.preedit = confirmed + &rendered;
    }

    fn active_preedit(&self, raw: &str, candidate: Option<&GeneratedCandidate>) -> String {
        if !self.confirmed.is_empty() && !self.caret_focused && self.selected != 0 {
            // A shorter suffix candidate does not repartition a complete
            // syllable into initials (shun must not become shu+n).
            format_preedit(raw)
        } else {
            candidate_preedit(raw, candidate)
        }
    }

    fn refresh_navigation_preedit(&mut self) {
        // The legacy whole-input menu leaves the decoder's spelling untouched.
        // Once a prefix is confirmed, Rime navigates the remaining segment.
        if self.confirmed.is_empty() { return; }
        self.caret_focused = false;
        self.caret_stops.clear();
        self.refresh_preedit();
    }

    fn move_caret(&mut self, left: bool) {
        let start = self.confirmed_end();
        let raw = &self.raw[start..];
        let caret = self.caret.saturating_sub(start).min(raw.len());
        let focused = if !self.caret_focused && caret > 0 && caret < raw.len() {
            self.generate(&raw[..caret])
        } else { Vec::new() };
        let selected = if self.caret_focused || caret == 0 { self.candidates.first() }
            else if !self.confirmed.is_empty() { self.candidates.get(self.selected) }
            else { self.restored_preedit.as_ref().or_else(|| self.candidates.first()) };
        let candidate = if !focused.is_empty() && selected.is_none_or(|item| item.consumed > caret) {
            focused.first()
        } else { selected };
        let end = candidate.map_or(raw.len(), |item| item.consumed);
        let rendered = self.active_preedit(&raw[..end], candidate);
        let mut stops = if self.caret_focused && caret > 0 && caret < raw.len() {
            self.caret_stops.clone()
        } else {
            let mut stops = vec![0];
            let mut offset = 0;
            for ch in rendered.chars() {
                if ch == ' ' { stops.push(offset); }
                else {
                    offset += 1;
                    if ch == '\'' { stops.push(offset); }
                }
            }
            stops.push(end);
            stops.push(raw.len());
            stops
        };
        stops.sort_unstable();
        stops.dedup();
        let next = if self.letter_caret {
            if left { caret.saturating_sub(1) } else { (caret + 1).min(raw.len()) }
        } else if stops.len() <= 2 && !self.confirmed.is_empty() {
            // A confirmed prefix makes the final syllable a segment, rather
            // than a standalone spelling edited one letter at a time.
            if left { 0 } else { raw.len() }
        } else if stops.len() <= 2 {
            if left { caret.saturating_sub(1) } else { (caret + 1).min(raw.len()) }
        } else if left {
            stops.iter().rev().copied().find(|&at| at < caret).unwrap_or(stops[stops.len() - 2])
        } else {
            stops.iter().copied().find(|&at| at > caret).unwrap_or(stops[1])
        };
        self.caret = start + next;
        self.caret_stops = stops;
        self.caret_focused = true;
        if next == 0 || next == raw.len() { self.letter_caret = false; }
        self.undo_confirmation = false;
        self.refresh_preedit();
    }

    fn preview(&self) -> String {
        let mut text = self.confirmed_text();
        let mut rest = &self.raw[self.confirmed_end()..];
        let mut next = self.candidates.get(self.selected).cloned();
        while !rest.is_empty() {
            let Some(item) = next.filter(|item| item.consumed > 0 && item.consumed <= rest.len()) else {
                text.push_str(rest);
                break;
            };
            text.push_str(&item.candidate.text);
            rest = rest[item.consumed..].trim_start_matches('\'');
            next = self.generate(rest).into_iter().next();
        }
        text
    }

    fn snapshot_range(&self, start: usize, maximum: usize, global: bool) -> DecoderSnapshot {
        let end = start.saturating_add(maximum).min(self.candidates.len());
        DecoderSnapshot {
            raw_input: self.raw.clone(), preedit: self.preedit.clone(), cursor_utf8: self.cursor_utf8,
            candidates: self.candidates.iter().enumerate().skip(start).take(maximum).map(|(index, item)| {
                let mut candidate = item.candidate.clone();
                candidate.source_index = global.then_some(index);
                candidate
            }).collect(),
            secondary_candidates: Vec::new(),
            highlighted_index: if global { 0 } else { self.selected % PAGE_SIZE },
            page_no: if global { 0 } else { start / PAGE_SIZE },
            is_last_page: !self.candidates.is_empty() && end == self.candidates.len(),
        }
    }

    fn select_global(&mut self, index: usize, intent: LearningIntent) -> Result<String, i32> {
        let Some(item) = self.candidates.get(index).cloned() else { return Err(-104); };
        if self.caret < self.raw.len() {
            // Session's Sogou editing contract commits exactly the displayed
            // choice; native editing must not leave its unchosen suffix behind.
            let text = self.confirmed_text() + &item.candidate.text;
            self.clear_after_selection();
            return Ok(text);
        }
        if intent == LearningIntent::RequiredSegmentCommit {
            let text = self.preview();
            self.clear_after_selection();
            return Ok(text);
        }
        let start = self.confirmed_end();
        let mut end = start + item.consumed;
        while self.raw.as_bytes().get(end) == Some(&b'\'') { end += 1; }
        if end == self.raw.len() {
            let text = self.confirmed_text() + &item.candidate.text;
            self.clear_after_selection();
            return Ok(text);
        }
        self.confirmed.push(ConfirmedSegment { text: item.candidate.text.clone(), end,
            candidate: item.candidate });
        self.caret = self.raw.len();
        self.undo_confirmation = true;
        self.refresh();
        Ok(String::new())
    }

    #[cfg(test)]
    pub(crate) fn fork_unlearned_composition(&self) -> Self {
        let learning = LearningSession::default();
        // This probe fork is valid only for the public unlearned fixture. The
        // decoder has no runtime attach API; never clone a learning session.
        assert_eq!(self.learning.mode, super::learning::LearningMode::Rime);
        assert!(self.learning.entries().is_empty());
        assert!(self.learning.choices().is_empty());
        assert!(self.learning.overlay().is_none());
        assert!(self.learning.whole_overlay().is_none());
        assert_eq!(self.learning.rime_tick(), learning.rime_tick());
        Self { raw: self.raw.clone(), preedit: self.preedit.clone(), caret: self.caret,
            cursor_utf8: self.cursor_utf8, caret_focused: self.caret_focused, letter_caret: self.letter_caret,
            caret_stops: self.caret_stops.clone(), confirmed: self.confirmed.clone(),
            undo_confirmation: self.undo_confirmation, candidates: self.candidates.clone(),
            restored_preedit: self.restored_preedit.clone(), selected: self.selected, learning, whole: self.whole, dictionary: self.dictionary.clone(),
            lattice: std::cell::RefCell::new(None), prefix_candidates: Vec::new(), prefix_candidate_bytes: 0, prefix_generation: None, prefix_bonuses: Vec::new(), fuzzy_mask: 0 }
    }

    #[cfg(test)]
    pub(crate) fn candidate_covered_letters(&self, index: usize) -> Option<usize> {
        let start = self.selected / PAGE_SIZE * PAGE_SIZE;
        if index >= PAGE_SIZE { return None; }
        let item = self.candidates.get(start + index)?;
        let consumed = if self.caret < self.raw.len() { self.raw.len() - self.confirmed_end() } else { item.consumed };
        Some(self.raw.get(self.confirmed_end()..self.confirmed_end() + consumed)?.bytes().filter(u8::is_ascii_alphabetic).count())
    }
}

impl Decoder for ZhixuDecoder {
    fn dictionary_snapshot(&self) -> Option<std::sync::Arc<Index>> { self.dictionary.clone() }

    fn native_editing(&self) -> bool { true }

    fn composition_boundary(&self, _purpose: BoundaryPurpose) -> Option<CompositionBoundary> {
        Some(CompositionBoundary { confirmed: self.confirmed_text(),
            unconfirmed_raw: self.raw[self.confirmed_end()..].to_string() })
    }

    fn configure_learning(&mut self, mode: super::learning::LearningMode) -> Result<(), i32> {
        self.prefix_candidates.clear();
        self.prefix_candidate_bytes = 0;
        self.learning.set_mode(mode, &super::current());
        self.refresh();
        Ok(())
    }

    fn configure_fuzzy(&mut self, mask: u32) -> Result<(), i32> {
        if mask & !super::fuzzy::FUZZY_MASK_ALL != 0 { return Err(-12); }
        if self.fuzzy_mask == mask { return Ok(()); }
        self.prefix_candidates.clear();
        self.prefix_candidate_bytes = 0;
        self.fuzzy_mask = mask;
        self.refresh();
        Ok(())
    }

    fn configure_zhixu_whole(&mut self, on: bool) -> Result<(), i32> {
        if self.is_composing() { return Err(-10); }
        if on { self.reload_dictionaries()?; }
        self.prefix_candidates.clear();
        self.prefix_candidate_bytes = 0;
        self.whole = on;
        self.refresh();
        Ok(())
    }

    fn reload_dictionaries(&mut self) -> Result<(), i32> {
        #[cfg(test)]
        if let Some(dictionary) = super::PINNED.with(|slot| slot.borrow().clone()).flatten() {
            self.dictionary = Some(dictionary);
            self.refresh();
            return Ok(());
        }
        let dictionary = super::loader::deployed_snapshot().map_err(|_| -110)?;
        self.dictionary = dictionary;
        self.refresh();
        Ok(())
    }

    fn snapshot(&self) -> DecoderSnapshot {
        if self.fuzzy_mask != 0 { return self.snapshot_range(0, FUZZY_CANDIDATE_POOL, true); }
        self.snapshot_range(self.selected / PAGE_SIZE * PAGE_SIZE, PAGE_SIZE, false)
    }

    fn snapshot_all_candidates(&self) -> Option<DecoderSnapshot> {
        (!self.candidates.is_empty()).then(|| self.snapshot_range(0, usize::MAX, true))
    }

    fn snapshot_candidate_prefix(&self, maximum: usize) -> Option<DecoderSnapshot> {
        Some(self.snapshot_range(0, maximum, true))
    }

    fn whole_prefix_has_raw_echo(&self) -> bool {
        self.candidates.is_empty() || self.candidates.iter().any(|item|
            item.candidate.text.is_ascii() && item.candidate.text.eq_ignore_ascii_case(&self.raw))
    }

    fn is_composing(&self) -> bool { !self.raw.is_empty() }

    fn input(&mut self, text: &str) -> Result<String, i32> {
        for byte in text.bytes() {
            if !byte.is_ascii() { return Err(-103); }
            if byte.is_ascii_lowercase() || byte == b'\'' {
                self.raw.insert(self.caret, byte as char);
                self.caret += 1;
                self.undo_confirmation = false;
                self.refresh();
            }
        }
        Ok(String::new())
    }

    fn backspace(&mut self) -> Result<String, i32> {
        if !self.confirmed.is_empty() && self.caret <= self.confirmed_end() && self.caret < self.raw.len() {
            self.confirmed.pop();
            if self.caret > 0 {
                self.caret -= 1;
                self.raw.remove(self.caret);
            }
            self.undo_confirmation = false;
            self.refresh();
        } else if !self.confirmed.is_empty() && (self.undo_confirmation || self.confirmed_end() == self.raw.len()) {
            let segment = self.confirmed.pop().unwrap();
            self.caret = self.raw.len();
            self.refresh();
            if let Some(index) = self.candidates.iter().position(|item|
                item.candidate.text == segment.candidate.text && item.candidate.reading_key == segment.candidate.reading_key) {
                self.selected = index;
                self.restored_preedit = self.candidates.get(index).cloned();
                self.refresh_preedit();
            }
            self.undo_confirmation = !self.confirmed.is_empty();
        } else if self.caret > self.confirmed_end() {
            self.caret -= 1;
            self.raw.remove(self.caret);
            // Erasing the last suffix letter reopens the preceding segment in
            // the same key, so composition never strands converted-only text.
            if self.caret == self.confirmed_end() && !self.confirmed.is_empty() {
                self.confirmed.pop();
            }
            self.undo_confirmation = false;
            self.refresh();
        }
        Ok(String::new())
    }

    fn move_left(&mut self) -> Result<String, i32> {
        self.move_caret(true);
        Ok(String::new())
    }

    fn move_right(&mut self) -> Result<String, i32> {
        self.move_caret(false);
        Ok(String::new())
    }

    fn page_up(&mut self) -> Result<String, i32> {
        if self.caret == self.confirmed_end() && self.caret < self.raw.len() { return Ok(String::new()); }
        if self.selected >= PAGE_SIZE { self.selected -= PAGE_SIZE; } else { self.selected = 0; }
        self.refresh_navigation_preedit();
        Ok(String::new())
    }

    fn page_down(&mut self) -> Result<String, i32> {
        if self.caret == self.confirmed_end() && self.caret < self.raw.len() { return Ok(String::new()); }
        let next_page = (self.selected / PAGE_SIZE + 1) * PAGE_SIZE;
        if next_page < self.candidates.len() {
            self.selected = (self.selected + PAGE_SIZE).min(self.candidates.len() - 1);
        }
        self.refresh_navigation_preedit();
        Ok(String::new())
    }

    fn select_previous(&mut self) -> Result<String, i32> {
        if self.caret == self.confirmed_end() && self.caret < self.raw.len() { return Ok(String::new()); }
        self.selected = self.selected.saturating_sub(1);
        self.refresh_navigation_preedit();
        Ok(String::new())
    }

    fn select_next(&mut self) -> Result<String, i32> {
        if self.caret == self.confirmed_end() && self.caret < self.raw.len() { return Ok(String::new()); }
        if self.selected + 1 < self.candidates.len() { self.selected += 1; }
        self.refresh_navigation_preedit();
        Ok(String::new())
    }

    fn select_candidate(&mut self, index: usize, _delivery_id: u64, intent: LearningIntent) -> Result<String, i32> {
        let global = if self.fuzzy_mask != 0 { index }
            else if index >= PAGE_SIZE { return Err(-104); }
            else { self.selected / PAGE_SIZE * PAGE_SIZE + index };
        let Some(item) = self.candidates.get(global) else { return Err(-104); };
        let intent = if intent == LearningIntent::NoLearn && index == 0
            && (!self.confirmed.is_empty() || item.consumed < self.raw.len() - self.confirmed_end()) {
            LearningIntent::RequiredSegmentCommit
        } else { intent };
        self.select_global(global, intent)
    }

    fn select_candidate_global(&mut self, index: usize, _delivery_id: u64, intent: LearningIntent) -> Result<String, i32> {
        self.select_global(index, intent)
    }

    fn peek_following_pages(&mut self, count: usize) -> Vec<Vec<Candidate>> {
        let next = (self.selected / PAGE_SIZE + 1) * PAGE_SIZE;
        self.candidates[next.min(self.candidates.len())..].chunks(PAGE_SIZE).take(count)
            .map(|page| page.iter().map(|item| item.candidate.clone()).collect()).collect()
    }

    fn delete_candidate(&mut self, index: usize) -> Result<(), i32> {
        if self.fuzzy_mask != 0 { return self.delete_candidate_global(index); }
        if index >= PAGE_SIZE { return Err(-105); }
        self.delete_candidate_global(self.selected / PAGE_SIZE * PAGE_SIZE + index)
    }

    fn delete_candidate_global(&mut self, index: usize) -> Result<(), i32> {
        let candidate = self.candidates.get(index).ok_or(-105)?.candidate.clone();
        self.learning.forget(&candidate.reading_key, &candidate.text)?;
        self.refresh();
        Ok(())
    }

    fn commit_raw(&mut self) -> String {
        let raw = self.raw.clone();
        self.clear();
        raw
    }

    fn clear(&mut self) {
        self.learning.clear_segments();
        self.clear_after_selection();
    }

    fn clear_after_selection(&mut self) {
        let mut removed = 0;
        self.prefix_candidates.retain(|cached| {
            if cached.raw.len() <= 4 { return true; }
            removed += candidate_cache_payload(&cached.raw, &cached.candidates);
            false
        });
        self.prefix_candidate_bytes -= removed;
        *self.lattice.borrow_mut() = None;
        self.raw.clear();
        self.preedit.clear();
        self.caret = 0;
        self.cursor_utf8 = 0;
        self.caret_focused = false;
        self.letter_caret = false;
        self.caret_stops.clear();
        self.confirmed.clear();
        self.undo_confirmation = false;
        self.candidates = std::sync::Arc::default();
        self.restored_preedit = None;
        self.selected = 0;
    }
}

fn display_syllable(raw: &str) -> String {
    let mut text = raw.replace("nv", "nü").replace("lv", "lü")
        .replace("nue", "nüe").replace("lue", "lüe");
    for initial in ['j', 'q', 'x', 'y'] {
        text = text.replace(&format!("{initial}v"), &format!("{initial}u"));
    }
    text
}

fn spelling_token(token: &str) -> bool { full_syllable(token) || initial(token) }

fn format_preedit(raw: &str) -> String {
    if raw.len() > 120 { return display_syllable(raw); }
    let mut result = String::new();
    let mut parsed = true;
    for (part, segment) in raw.split('\'').enumerate() {
        if part > 0 { result.push('\''); }
        if !parsed || (part == 0 && segment.is_empty()) {
            result.push_str(segment);
            parsed = false;
            continue;
        }
        let mut paths: Vec<Option<Vec<&str>>> = vec![None; segment.len() + 1];
        paths[0] = Some(Vec::new());
        for end in 1..=segment.len() {
            for start in end.saturating_sub(6)..end {
                let token = &segment[start..end];
                if !spelling_token(token) { continue; }
                let Some(prefix) = paths[start].as_ref() else { continue; };
                let mut path = prefix.clone();
                path.push(token);
                if paths[end].as_ref().is_none_or(|old| path.len() < old.len()
                    || path.len() == old.len() && path.iter().map(|s| s.len()).cmp(old.iter().map(|s| s.len())).is_gt()) {
                    paths[end] = Some(path);
                }
            }
        }
        let end = (0..=segment.len()).rev().find(|&end| paths[end].is_some()).unwrap();
        result.push_str(&paths[end].as_ref().unwrap().iter().map(|token| display_syllable(token)).collect::<Vec<_>>().join(" "));
        result.push_str(&segment[end..]);
        parsed = end == segment.len();
    }
    result
}

fn candidate_preedit(raw: &str, candidate: Option<&GeneratedCandidate>) -> String {
    fn align(raw: &str, start: usize, syllables: &[&str], boundaries: &mut Vec<usize>,
        visit: &mut impl FnMut(usize, &[usize])) {
        if syllables.is_empty() { visit(start, boundaries); return; }
        for end in (start + 1..=raw.len().min(start + 6)).rev() {
            let token = &raw[start..end];
            if !syllable_matches(token, syllables[0]) { continue; }
            let next = end + raw[end..].bytes().take_while(|byte| *byte == b'\'').count();
            boundaries.push(end);
            align(raw, next, &syllables[1..], boundaries, visit);
            boundaries.pop();
        }
    }
    fn compose(raw: &str, text: &str, raw_at: usize, text_at: usize, dictionaries: &[&Index],
        failed: &mut std::collections::HashSet<(usize, usize)>) -> Option<Vec<usize>> {
        if text_at == text.len() { return (raw_at == raw.len()).then(Vec::new); }
        if raw_at == raw.len() || failed.contains(&(raw_at, text_at)) { return None; }
        let mut ends: Vec<_> = text[text_at..].char_indices().map(|(at, ch)| text_at + at + ch.len_utf8()).collect();
        ends.reverse();
        for end in ends {
            for dictionary in dictionaries {
                for reading in dictionary.all_readings_for_text(&text[text_at..end]) {
                    let syllables: Vec<_> = reading.split_whitespace().collect();
                    let mut matches = Vec::new();
                    align(raw, raw_at, &syllables, &mut Vec::new(), &mut |next, boundaries|
                        matches.push((next, boundaries.to_vec())));
                    for (next, mut boundaries) in matches {
                        if let Some(tail) = compose(raw, text, next, end, dictionaries, failed) {
                            boundaries.extend(tail);
                            return Some(boundaries);
                        }
                    }
                }
            }
        }
        failed.insert((raw_at, text_at));
        None
    }
    let Some(candidate) = candidate.filter(|item| item.consumed == raw.len()
        && !item.candidate.text.is_ascii() && raw.len() <= 120) else { return format_preedit(raw); };
    let reading = &candidate.candidate.reading_key;
    let syllables: Vec<_> = reading.split_whitespace().collect();
    let mut boundaries = None;
    if syllables.len() == candidate.candidate.text.chars().count() {
        align(raw, 0, &syllables, &mut Vec::new(), &mut |end, path| {
            if end == raw.len() && boundaries.is_none() { boundaries = Some(path.to_vec()); }
        });
    }
    let boundaries = boundaries.or_else(|| {
        let index = super::current();
        let personal = super::learning_index();
        let dictionaries: Vec<_> = std::iter::once(index.as_ref()).chain(personal.as_deref()).collect();
        compose(raw, &candidate.candidate.text, 0, 0, &dictionaries, &mut std::collections::HashSet::new())
    });
    let Some(boundaries) = boundaries else { return format_preedit(raw); };
    let mut rendered = String::new();
    for (at, byte) in raw.bytes().enumerate() {
        if at > 0 && boundaries.contains(&at) && byte != b'\'' && raw.as_bytes()[at - 1] != b'\'' {
            rendered.push(' ');
        }
        rendered.push(byte as char);
    }
    display_syllable(&rendered)
}

#[cfg(test)]
mod state_tests {
    use super::*;
    use crate::zhixu::{Index, loader::{DictionaryData, Entry}};
    use std::sync::Arc;

    #[test]
    fn y6_recursive_cache_rejects_whole_only_full_candidates() {
        let dictionary = Arc::new(Index::from_data(DictionaryData { entries: [
            ("你好", "ni ha", 10000), ("你", "ni", 10000), ("泥", "ni", 1000), ("好", "hao", 10000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "public-test".into(),
        }).collect() }));
        super::super::with_index(dictionary, || {
            let generated = generate_candidates_cached("nihao", &[], 0, true, true, None, &[]);
            assert!(!generated.has_standalone_full);
            assert!(generated.candidates.iter().any(|item| item.consumed == 5 && item.candidate.text == "泥好"));
            let cached = [PrefixCandidates { raw: "nihao".into(), candidates: Arc::new(generated.candidates),
                has_standalone_full: generated.has_standalone_full, inherited_order: true }];
            assert!(generate_with_order("nihao", &[], 0, true).into_iter().all(|item| item.consumed != 5));
            assert!(generate_first_full_cached("nihao", &[], 0, true, &cached).is_none());
        });
    }

    #[test]
    fn y6_recursive_cache_order_mismatch_uses_cold_generation() {
        fixture(|| {
            let expected = generate_with_order("nihao", &[], 0, true).into_iter().find(|item| item.consumed == 5).unwrap();
            let cached = [PrefixCandidates { raw: "nihao".into(), candidates: Arc::new(vec![GeneratedCandidate {
                candidate: Candidate { text: "不可复用".into(), reading_key: "ni hao".into(), ..Candidate::default() }, consumed: 5,
            }]), has_standalone_full: true, inherited_order: false }];
            let actual = generate_first_full_cached("nihao", &[], 0, true, &cached).unwrap();
            assert_eq!(actual.candidate, expected.candidate);
            assert_eq!(actual.consumed, expected.consumed);
        });
    }

    #[test]
    fn y6_recursive_cache_clears_only_the_returned_clone_comment() {
        let dictionary = Arc::new(Index::from_data(DictionaryData { entries: [
            ("你", "ni", 10000), ("好", "hao", 10000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "public-test".into(),
        }).collect() }));
        super::super::with_index(dictionary, || {
            let generated = generate_candidates_cached("nihao", &[], 0, true, true, None, &[]);
            assert!(generated.has_standalone_full);
            let comment = generated.candidates[0].candidate.comment.clone();
            assert!(!comment.is_empty());
            let cached = [PrefixCandidates { raw: "nihao".into(), candidates: Arc::new(generated.candidates),
                has_standalone_full: generated.has_standalone_full, inherited_order: true }];
            let actual = generate_first_full_cached("nihao", &[], 0, true, &cached).unwrap();
            let expected = generate_with_order("nihao", &[], 0, true).into_iter().find(|item| item.consumed == 5).unwrap();
            assert!(actual.candidate.comment.is_empty());
            assert_eq!(actual.candidate, expected.candidate);
            assert_eq!(actual.consumed, expected.consumed);
            assert_eq!(cached[0].candidates[0].candidate.comment, comment);
        });
    }

    #[test]
    fn y6_recursive_cache_matches_cold_generation_with_learning_entries() {
        fixture(|| {
            let entries = [learning::Entry { text: "拟好".into(), reading: "ni hao".into(), count: 20,
                last_ms: crate::personal_picks::now_ms(), rime: None }];
            for inherited_order in [false, true] {
                let generated = generate_candidates_cached("nihao", &entries, 0, inherited_order, true, None, &[]);
                assert!(generated.has_standalone_full);
                let cached = [PrefixCandidates { raw: "nihao".into(), candidates: Arc::new(generated.candidates),
                    has_standalone_full: generated.has_standalone_full, inherited_order }];
                let actual = generate_first_full_cached("nihao", &entries, 0, inherited_order, &cached).unwrap();
                let expected = generate_with_order("nihao", &entries, 0, inherited_order).into_iter()
                    .find(|item| item.consumed == 5).unwrap();
                assert_eq!(actual.candidate, expected.candidate);
                assert_eq!(actual.consumed, expected.consumed);
            }
        });
    }

    #[derive(Clone, Copy, Debug)]
    enum Y6Key { Text(char), Backspace, Enter, Escape }

    fn y6_apply(decoder: &mut ZhixuDecoder, key: Y6Key) -> String {
        match key {
            Y6Key::Text(ch) => decoder.input(&ch.to_string()).unwrap(),
            Y6Key::Backspace => decoder.backspace().unwrap(),
            Y6Key::Enter => decoder.commit_raw(),
            Y6Key::Escape => { decoder.clear(); String::new() },
        }
    }

    fn y6_compare_step(cached: &mut ZhixuDecoder, rebuilt: &mut ZhixuDecoder, key: Y6Key) {
        rebuilt.prefix_candidates.clear();
        rebuilt.prefix_candidate_bytes = 0;
        *rebuilt.lattice.borrow_mut() = None;
        assert_eq!(y6_apply(cached, key), y6_apply(rebuilt, key), "commit {key:?}");
        assert_eq!(format!("{:?}", cached.snapshot()), format!("{:?}", rebuilt.snapshot()), "page {key:?}");
        assert_eq!(format!("{:?}", cached.snapshot_all_candidates()),
            format!("{:?}", rebuilt.snapshot_all_candidates()), "all {key:?}");
        for decoder in [&*cached, &*rebuilt] {
            assert_eq!(y6_cache_usage(decoder).2, decoder.prefix_candidate_bytes
                + decoder.prefix_candidates.capacity() * std::mem::size_of::<PrefixCandidates>());
        }
        let raw = &cached.raw[cached.confirmed_end()..];
        if !raw.is_empty() {
            let dictionary = cached.dictionary.clone().unwrap_or_else(super::super::current);
            let _dictionary = super::super::pin_snapshot(Some(dictionary));
            let _learning = super::super::pin_learning(cached.learning.whole_overlay()
                .or_else(super::super::learning_index));
            // This reference passes no lattice and cannot mutate either decoder's cache.
            let expected = generate_candidates(raw, &cached.learning.entries(), cached.learning.rime_tick(),
                cached.learning.use_rime_order(raw), cached.whole);
            assert_eq!(format!("{:?}", cached.candidates), format!("{expected:?}"), "cold {key:?} {raw}");
        }
    }

    #[test]
    fn y6_two_sessions_match_cold_rebuild_after_boundaries_learning_and_reload() {
        fixture(|| {
            let directory = std::env::temp_dir().join(format!("zhixu-y6-parity-{}", std::process::id()));
            std::fs::create_dir(&directory).unwrap();
            let now = crate::personal_picks::now_ms();
            let mut store = learning::LearningStore::default();
            store.import_rime_export("拟好\tni hao\t20\n", now).unwrap();
            store.save(&directory).unwrap();
            let create = || {
                let mut decoder = ZhixuDecoder::new();
                decoder.whole = true;
                decoder.attach_learning(&directory);
                decoder.configure_learning(learning::LearningMode::Zhixu).unwrap();
                decoder
            };
            let mut cached = [create(), create()];
            let mut rebuilt = [create(), create()];
            let mut peer = LearningSession::default();
            peer.attach(&directory);
            peer.set_mode(learning::LearningMode::Zhixu, &super::super::current());
            let original = super::super::current();
            let replacement = Arc::new(Index::from_data(DictionaryData { entries: [
                ("您好", "ni hao", 9000), ("您", "ni", 8000), ("好", "hao", 7000),
                ("视界", "shi jie", 6000), ("世", "shi", 5000), ("界", "jie", 4000),
            ].into_iter().map(|(text, reading, weight)| Entry {
                text: text.into(), reading: reading.into(), weight, layer: "public-test".into(),
            }).collect() }));
            for phase in 0..6 {
                match phase {
                    1 => peer.remember_for_test("ni hao", "你号", now),
                    2 => peer.forget("ni hao", "拟好").unwrap(),
                    3 => peer.remember_for_test("ni hao", "你号", now + 1),
                    5 => peer.forget("ni hao", "你号").unwrap(),
                    _ => {},
                }
                let dictionary = if phase >= 4 { replacement.clone() } else { original.clone() };
                super::super::with_index(dictionary, || {
                    if phase == 4 {
                        for decoder in cached.iter_mut().chain(&mut rebuilt) {
                            decoder.reload_dictionaries().unwrap();
                        }
                    }
                    for sequence in 0..16 {
                        let slot = sequence % 2;
                        let raw = ["nihao", "ni", "nh", "nihaoshijie", "nih", "n", "nihao", "shijie"][sequence % 8];
                        for ch in raw.chars() {
                            y6_compare_step(&mut cached[slot], &mut rebuilt[slot], Y6Key::Text(ch));
                        }
                        y6_compare_step(&mut cached[slot], &mut rebuilt[slot], Y6Key::Backspace);
                        y6_compare_step(&mut cached[slot], &mut rebuilt[slot], Y6Key::Text(raw.chars().last().unwrap()));
                        let boundary = if sequence % 5 == 0 { Y6Key::Enter } else { Y6Key::Escape };
                        y6_compare_step(&mut cached[slot], &mut rebuilt[slot], boundary);
                        if sequence % 4 == 0 {
                            y6_compare_step(&mut cached[1 - slot], &mut rebuilt[1 - slot], Y6Key::Escape);
                        }
                    }
                });
            }
            drop(peer);
            drop(cached);
            drop(rebuilt);
            std::fs::remove_dir_all(directory).unwrap();
        });
    }

    // Owned allocation capacities only; allocator headers and resident pages are separate.
    fn y6_cache_usage(decoder: &ZhixuDecoder) -> (usize, usize, usize) {
        let mut bytes = decoder.prefix_candidates.capacity()
            * std::mem::size_of::<PrefixCandidates>();
        let mut candidates = 0;
        let mut seen = std::collections::HashSet::new();
        for cached in &decoder.prefix_candidates {
            let entries = &cached.candidates;
            bytes += cached.raw.capacity();
            if !seen.insert(Arc::as_ptr(entries)) { continue; }
            bytes += std::mem::size_of::<Vec<GeneratedCandidate>>() + 2 * std::mem::size_of::<usize>();
            bytes += entries.capacity() * std::mem::size_of::<GeneratedCandidate>();
            candidates += entries.len();
            for entry in entries.iter() {
                bytes += entry.candidate.text.capacity() + entry.candidate.comment.capacity()
                    + entry.candidate.reading_key.capacity();
            }
        }
        (decoder.prefix_candidates.len(), candidates, bytes)
    }

    #[test]
    fn y6_candidate_cache_has_byte_budget() {
        let entries = (0..40_000).map(|offset| Entry {
            text: format!("{}{}", char::from_u32(0x4e00 + offset / 200).unwrap(),
                char::from_u32(0x4e00 + offset % 200).unwrap()),
            reading: "ni hao".into(), weight: 9000, layer: "public-test".into(),
        }).collect();
        let dictionary = Arc::new(Index::from_data(DictionaryData { entries }));
        dictionary.warm_whole();
        super::super::with_index(dictionary, || {
            let mut decoder = ZhixuDecoder::new();
            decoder.whole = true;
            for raw in ["nh", "nih", "nhao", "niha"] {
                decoder.clear();
                for ch in raw.chars() {
                    decoder.input(&ch.to_string()).unwrap();
                    let usage = y6_cache_usage(&decoder);
                    assert_eq!(usage.2, decoder.prefix_candidate_bytes + decoder.prefix_candidates.capacity()
                        * std::mem::size_of::<PrefixCandidates>());
                    assert!(usage.2 <= 16 * 1024 * 1024, "raw={} cache={usage:?}", decoder.raw);
                    let expected = generate_candidates(&decoder.raw, &[], 0, false, true);
                    assert_eq!(decoder.candidates.len(), expected.len());
                    for (actual, expected) in decoder.candidates.iter().zip(&expected) {
                        assert_eq!(actual.candidate, expected.candidate);
                        assert_eq!(actual.consumed, expected.consumed);
                    }
                }
            }
        });
    }

    #[test]
    fn y6_cache_accounting_counts_shared_candidate_storage_once() {
        let entries = Arc::new(vec![GeneratedCandidate { candidate: Candidate {
            text: "你好".into(), comment: "整串 · 顺手·知序".into(), reading_key: "ni hao".into(),
            ..Candidate::default()
        }, consumed: 5 }]);
        let mut decoder = ZhixuDecoder::new();
        decoder.prefix_candidates.push(PrefixCandidates { raw: "nihao".into(), candidates: entries.clone(),
            has_standalone_full: true, inherited_order: true });
        decoder.prefix_candidates.reserve(1);
        let first = y6_cache_usage(&decoder);
        decoder.prefix_candidates.push(PrefixCandidates { raw: "nh".into(), candidates: entries,
            has_standalone_full: true, inherited_order: true });
        let second = y6_cache_usage(&decoder);
        assert_eq!(second.0, 2);
        assert_eq!(first.1, 1);
        assert_eq!(second.1, 1, "shared candidate vector must not be double-counted");
        assert_eq!(second.2 - first.2, decoder.prefix_candidates[1].raw.capacity());
        decoder.prefix_candidates.clear();
        assert_eq!(y6_cache_usage(&decoder).2, decoder.prefix_candidates.capacity()
            * std::mem::size_of::<PrefixCandidates>());
    }

    #[test]
    #[ignore]
    fn y6_full_dictionary_candidate_cache_memory_profile() {
        let shared = std::path::PathBuf::from(std::env::var("ZHIXU_Z8_SHARED").unwrap());
        let user = std::env::temp_dir().join(format!("zhixu-y6-memory-{}", std::process::id()));
        std::fs::create_dir(&user).unwrap();
        super::super::loader::attach(&shared, &user);
        let dictionary = super::super::loader::deployed_snapshot().unwrap().unwrap();
        dictionary.warm_whole();
        println!("Y6_MEMORY_DICTIONARY entries={}", dictionary.entry_count());
        let corpus: serde_json::Value = serde_json::from_str(include_str!("../../../../../quality/corpus/base-500.json")).unwrap();
        let cases = corpus["cases"].as_array().unwrap();
        super::super::with_index(dictionary.clone(), || {
            let mut decoders = [ZhixuDecoder::from_ready_dictionary(dictionary.clone()),
                ZhixuDecoder::from_ready_dictionary(dictionary.clone())];
            let mut peaks = [(0, 0, 0); 2];
            let mut combined_peak = 0;
            for sequence in 0..2000 {
                let slot = sequence % 2;
                let raw = cases[sequence % cases.len()]["query"].as_str().unwrap();
                let events = raw.chars().map(Y6Key::Text)
                    .chain([Y6Key::Backspace, Y6Key::Text(raw.chars().last().unwrap()),
                        if sequence % 5 == 0 { Y6Key::Enter } else { Y6Key::Escape }]);
                for key in events {
                    y6_apply(&mut decoders[slot], key);
                    let usage = y6_cache_usage(&decoders[slot]);
                    assert!(usage.0 <= 512);
                    assert!(usage.2 <= 16 * 1024 * 1024, "slot={slot} cache={usage:?}");
                    if usage.2 > peaks[slot].2 { peaks[slot] = usage; }
                    combined_peak = combined_peak.max(usage.2 + y6_cache_usage(&decoders[1 - slot]).2);
                }
                if sequence % 500 == 499 {
                    println!("Y6_MEMORY sequence={} slots={:?} combined_peak_bytes={combined_peak}",
                        sequence + 1, decoders.iter().map(y6_cache_usage).collect::<Vec<_>>());
                }
            }
            println!("Y6_MEMORY_PEAK slots={peaks:?} combined_bytes={combined_peak}");
        });
        std::fs::remove_dir_all(user).unwrap();
    }

    #[test]
    fn y5c_reuses_candidates_across_raw_commit_escape_and_retype() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.whole = true;
            decoder.input("nihao").unwrap();
            let expected = format!("{:?}", decoder.snapshot_all_candidates());
            assert_eq!(decoder.candidates.as_ptr(), decoder.prefix_candidates.last().unwrap().candidates.as_ptr(),
                "cache and current candidates share immutable storage");
            decoder.backspace().unwrap();
            INNER_TIMING.with(|timing| *timing.borrow_mut() = [0; 3]);
            decoder.input("o").unwrap();
            assert_eq!(INNER_TIMING.with(|timing| timing.borrow()[0]), 0, "retyped suffix reuses candidates");
            assert_eq!(format!("{:?}", decoder.snapshot_all_candidates()), expected);
            decoder.clear();
            decoder.input("ni").unwrap();
            let expected = format!("{:?}", decoder.snapshot_all_candidates());
            for commit in [false, true] {
                if commit { assert_eq!(decoder.commit_raw(), "ni"); } else { decoder.clear(); }
                INNER_TIMING.with(|timing| *timing.borrow_mut() = [0; 3]);
                decoder.input("ni").unwrap();
                assert_eq!(INNER_TIMING.with(|timing| timing.borrow()[0]), 0, "composition boundary preserves valid short candidates");
                assert_eq!(format!("{:?}", decoder.snapshot_all_candidates()), expected);
            }
        });
    }

    #[test]
    fn y5c_cached_candidates_match_rebuild_after_shared_learning_changes() {
        fixture(|| {
            let directory = std::env::temp_dir().join(format!("zhixu-y5c-cache-{}", std::process::id()));
            std::fs::create_dir_all(&directory).unwrap();
            let now = crate::personal_picks::now_ms();
            let mut store = learning::LearningStore::default();
            store.import_rime_export("拟好\tni hao\t20\n", now).unwrap();
            store.save(&directory).unwrap();
            let mut decoder = ZhixuDecoder::new();
            decoder.whole = true;
            decoder.attach_learning(&directory);
            decoder.configure_learning(learning::LearningMode::Zhixu).unwrap();
            let mut peer = LearningSession::default();
            peer.attach(&directory);
            peer.set_mode(learning::LearningMode::Zhixu, &super::super::current());
            for change in 0..4 {
                match change {
                    1 => peer.remember_for_test("ni hao", "你号", now),
                    2 => peer.forget("ni hao", "拟好").unwrap(),
                    3 => peer.remember_for_test("ni hao", "你号", now + 1),
                    _ => {},
                }
                for raw in ["ni", "nihao", "nh", "nihao", "ni"] {
                    decoder.clear();
                    decoder.input(raw).unwrap();
                    let expected = decoder.generate(raw);
                    assert_eq!(format!("{:?}", decoder.candidates), format!("{expected:?}"), "change={change} raw={raw}");
                }
            }
            drop(peer);
            drop(decoder);
            std::fs::remove_dir_all(directory).unwrap();
        });
    }

    #[test]
    fn y5c_candidate_cache_is_bounded_across_unrelated_compositions() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            for a in b'a'..=b'z' {
                for b in b'a'..=b'z' {
                    let raw = format!("{}{}", a as char, b as char);
                    decoder.clear();
                    decoder.input(&raw).unwrap();
                    assert!(decoder.prefix_candidates.len() <= 512);
                    assert_eq!(format!("{:?}", decoder.candidates), format!("{:?}", decoder.generate(&raw)));
                }
            }
        });
    }

    #[test]
    fn y4_learning_revision_invalidates_cached_prefixes() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.configure_learning(learning::LearningMode::Zhixu).unwrap();
            decoder.input("nihao").unwrap();
            decoder.learning.remember_for_test("ni hao", "拟好", crate::personal_picks::now_ms());
            INNER_TIMING.with(|timing| *timing.borrow_mut() = [0; 3]);
            decoder.backspace().unwrap();
            assert!(INNER_TIMING.with(|timing| timing.borrow()[0]) > 0);
            let expected = decoder.generate(&decoder.raw);
            assert_eq!(format!("{:?}", decoder.candidates), format!("{expected:?}"));
            assert!(decoder.prefix_candidates.iter().all(|cached| decoder.raw.starts_with(&cached.raw)));
        });
    }

    #[test]
    fn y4_cold_and_prewarmed_sequences_match_and_dictionary_changes_invalidate_prefixes() {
        fixture(|| {
            let index = super::super::current();
            let replay = || {
                let mut decoder = ZhixuDecoder::new();
                decoder.whole = true;
                let mut states = Vec::new();
                for letter in "nihaoshijie".chars() {
                    decoder.input(&letter.to_string()).unwrap();
                    let expected = generate_candidates(&decoder.raw, &[], 0, true, true);
                    assert_eq!(format!("{:?}", decoder.candidates), format!("{expected:?}"));
                    states.push(format!("{:?}", decoder.snapshot_all_candidates()));
                }
                for _ in 0..5 {
                    decoder.backspace().unwrap();
                    states.push(format!("{:?}", decoder.snapshot_all_candidates()));
                }
                states
            };
            let cold = replay();
            index.warm_whole();
            assert_eq!(cold, replay());
            let mut decoder = ZhixuDecoder::new();
            decoder.input("nihao").unwrap();
            let next = Arc::new(Index::from_data(DictionaryData { entries: vec![Entry {
                text: "拟好".into(), reading: "ni hao".into(), weight: 9000, layer: "public-test".into(),
            }] }));
            super::super::with_index(next, || {
                decoder.backspace().unwrap();
                let expected = generate_candidates(&decoder.raw, &[], 0, true, false);
                assert_eq!(format!("{:?}", decoder.candidates), format!("{expected:?}"));
            });
            decoder.clear();
            assert!(decoder.prefix_candidates.iter().all(|cached| cached.raw.len() <= 4));
            assert!(decoder.lattice.borrow().is_none());
        });
    }

    #[test]
    fn y4_prefix_cache_matches_fresh_generation_and_avoids_backspace_rebuild() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            for letter in "nihaoshijie".chars() {
                decoder.input(&letter.to_string()).unwrap();
                let expected = generate_candidates(&decoder.raw, &[], 0, true, false);
                assert_eq!(format!("{:?}", decoder.candidates), format!("{expected:?}"));
            }
            for _ in 0..5 {
                INNER_TIMING.with(|timing| *timing.borrow_mut() = [0; 3]);
                decoder.backspace().unwrap();
                assert_eq!(INNER_TIMING.with(|timing| timing.borrow()[0]), 0, "backspace must restore cached candidates");
                let expected = generate_candidates(&decoder.raw, &[], 0, true, false);
                assert_eq!(format!("{:?}", decoder.candidates), format!("{expected:?}"));
            }
            for letter in "zhongguo".chars() {
                decoder.input(&letter.to_string()).unwrap();
                let expected = generate_candidates(&decoder.raw, &[], 0, true, false);
                assert_eq!(format!("{:?}", decoder.candidates), format!("{expected:?}"));
            }
            decoder.caret = 2;
            decoder.input("a").unwrap();
            let expected = generate_candidates(&decoder.raw, &[], 0, true, false);
            assert_eq!(format!("{:?}", decoder.candidates), format!("{expected:?}"));
            decoder.clear();
        });
    }

    #[test]
    fn y3c_imported_only_abbreviation_uses_the_learning_overlay() {
        let dictionary = Arc::new(Index::from_data(DictionaryData { entries: vec![Entry {
            text: "生产".into(), reading: "sheng chan".into(), weight: 9000, layer: "public-test".into(),
        }] }));
        crate::zhixu::with_index(dictionary, || {
            let directory = std::env::temp_dir().join(format!("zhixu-y3c-imported-only-{}", std::process::id()));
            std::fs::create_dir_all(&directory).unwrap();
            let now = crate::personal_picks::now_ms();
            let mut store = learning::LearningStore::default();
            store.import_rime_export("#@/tick\t1000\nsheng cheng \t生成\tc=7 d=8 t=999\n", now).unwrap();
            store.save(&directory).unwrap();
            let mut decoder = ZhixuDecoder::new();
            decoder.attach_learning(&directory);
            decoder.configure_learning(learning::LearningMode::Zhixu).unwrap();
            decoder.input("shengc").unwrap();
            let first = &decoder.snapshot().candidates[0];
            assert_eq!(first.text, "生成");
            assert_eq!(first.reading_key, "sheng cheng");
            assert_eq!(decoder.select_candidate(0, 1, LearningIntent::NoLearn), Ok("生成".into()));
            drop(decoder);
            std::fs::remove_dir_all(directory).unwrap();
        });
    }

    #[test]
    fn y3c_first_piece_selection_retains_the_remaining_suffix() {
        let cases = [
            ("shangjai", "上架", "shang jia", "ai"), ("yigng", "义工", "yi gong", "ng"),
            ("shoukkuan", "收款", "shou kuan", "kuan"), ("kuaije", "快捷", "kuai jie", "e"),
            ("ganje", "感觉", "gan jue", "e"), ("leiss", "类似", "lei si", "s"),
            ("wozenmy", "我怎么", "wo zen me", "y"), ("read", "热爱", "re ai", "d"),
        ];
        let suffixes = [("爱", "ai"), ("那个", "na ge"), ("款", "kuan"), ("鹅", "e"),
            ("是", "shi"), ("有", "you"), ("的", "de")];
        let index = Arc::new(Index::from_data(DictionaryData { entries: cases.iter()
            .map(|&(_, text, reading, _)| Entry {
                text: text.into(), reading: reading.into(), weight: 5000, layer: "public-test".into(),
            }).chain(suffixes.iter().map(|&(text, reading)| Entry {
                text: text.into(), reading: reading.into(), weight: 1000, layer: "public-test".into(),
            })).collect() }));
        crate::zhixu::with_index(index, || {
            for (raw, text, _, remaining) in cases {
                let mut decoder = ZhixuDecoder::new();
                decoder.input(raw).unwrap();
                let candidates = decoder.snapshot_all_candidates().unwrap().candidates;
                let at = candidates.iter().position(|candidate| candidate.text == text).unwrap();
                assert_eq!(decoder.select_candidate_global(at, 1, LearningIntent::DeliberateSelection), Ok(String::new()));
                assert_eq!(decoder.raw, raw);
                assert_eq!(decoder.confirmed_text(), text);
                assert_eq!(&decoder.raw[decoder.confirmed_end()..], remaining);
                let suffix = match remaining { "ai" => "爱", "ng" => "那个", "kuan" => "款", "e" => "鹅",
                    "s" => "是", "y" => "有", "d" => "的", _ => unreachable!() };
                let at = decoder.snapshot_all_candidates().unwrap().candidates.iter()
                    .position(|candidate| candidate.text == suffix).unwrap();
                assert_eq!(decoder.select_candidate_global(at, 2, LearningIntent::DeliberateSelection),
                    Ok(format!("{text}{suffix}")));
                assert!(!decoder.is_composing());
            }
        });
    }

    #[test]
    fn deleted_learning_restores_dictionary_order_and_survives_restart() {
        let dictionary = Arc::new(Index::from_data(DictionaryData { entries: [
            ("你好", 10000), ("拟好", 9000),
        ].into_iter().map(|(text, weight)| Entry {
            text: text.into(), reading: "ni hao".into(), weight, layer: "public-test".into(),
        }).collect() }));
        crate::zhixu::with_index(dictionary, || {
            for global in [false, true] {
                let directory = std::env::temp_dir().join(format!("zhixu-y3b-decoder-delete-{}-{global}", std::process::id()));
                std::fs::create_dir_all(&directory).unwrap();
                let now = crate::personal_picks::now_ms();
                let mut store = learning::LearningStore::default();
                store.import_rime_export("拟好\tni hao\t20\n", now).unwrap();
                store.save(&directory).unwrap();
                let build = || {
                    let mut decoder = ZhixuDecoder::new();
                    decoder.attach_learning(&directory);
                    decoder.configure_learning(learning::LearningMode::Zhixu).unwrap();
                    decoder.input("nihao").unwrap();
                    decoder
                };
                let mut decoder = build();
                assert_eq!(decoder.snapshot().candidates[0].text, "拟好");
                assert_eq!(if global { decoder.delete_candidate_global(0) } else { decoder.delete_candidate(0) }, Ok(()));
                assert_eq!(decoder.snapshot().candidates.iter().map(|item| item.text.as_str()).collect::<Vec<_>>(), ["你好", "拟好"]);
                assert!(learning::LearningStore::load(&directory, now).entries().is_empty());
                drop(decoder);
                let mut restarted = build();
                assert_eq!(restarted.snapshot().candidates.iter().map(|item| item.text.as_str()).collect::<Vec<_>>(), ["你好", "拟好"]);
                let before = restarted.snapshot();
                assert_eq!(restarted.delete_candidate(5), Err(-105));
                assert_eq!(restarted.delete_candidate_global(999), Err(-105));
                assert_eq!(restarted.snapshot().candidates, before.candidates);
                drop(restarted);
                std::fs::remove_dir_all(directory).unwrap();
            }
        });
    }

    fn fixture<T>(f: impl FnOnce() -> T) -> T {
        let entries = [
            ("你好", "ni hao", 1000), ("你", "ni", 1000), ("泥", "ni", 900),
            ("拟", "ni", 800), ("尼", "ni", 700), ("逆", "ni", 600),
            ("腻", "ni", 500), ("妮", "ni", 400), ("倪", "ni", 300),
            ("好", "hao", 1000), ("哈", "ha", 1000), ("世界", "shi jie", 1000),
            ("下", "xia", 1000), ("男", "nan", 1000), ("干", "gan", 1000), ("旅", "lv", 1000),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "public-test".into(),
        }).collect();
        crate::zhixu::with_index(Arc::new(Index::from_data(DictionaryData { entries })), f)
    }

    #[test]
    fn preedit_spaces_umlaut_and_explicit_delimiters() {
        for (raw, expected) in [("nihao", "ni hao"), ("nvhao", "nü hao"), ("lve", "lüe"),
            ("nue", "nüe"), ("jvhao", "ju hao"), ("zhongj", "zhong j"),
            ("zho", "zh o"), ("zhon", "zh o n"), ("shouko", "shou k o"),
            ("zg", "z g"), ("xi'an", "xi'an"), ("xi'", "xi'")] {
            assert_eq!(format_preedit(raw), expected, "{raw}");
        }
    }

    #[test]
    fn incomplete_spelling_prefers_full_syllables_and_erases_degenerate_readings() {
        let mut paths = Vec::new();
        visit_spellings("dengluc", |end, tokens| {
            if end == 7 { paths.push(tokens.iter().map(|token| token.to_string()).collect::<Vec<_>>()); }
        });
        assert_eq!(paths, vec![vec!["deng", "lu", "c"]]);
        crate::zhixu::with_index(Arc::new(Index::from_data(DictionaryData { entries: vec![
            Entry { text: "嗯".into(), reading: "n".into(), weight: 50000, layer: "public-test".into() },
            Entry { text: "你".into(), reading: "ni".into(), weight: 1000, layer: "public-test".into() },
        ] })), || {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("n").unwrap();
            assert_eq!(decoder.snapshot().candidates.iter().map(|item| item.text.as_str()).collect::<Vec<_>>(), vec!["你"]);
        });
    }

    #[test]
    fn top_sentence_reading_controls_ambiguous_preedit() {
        fixture(|| {
            for (raw, text, expected) in [("xianan", "下男", "xia nan"), ("nangan", "男干", "nan gan")] {
                let candidate = GeneratedCandidate { candidate: Candidate {
                    text: text.into(), reading_key: raw.into(), ..Candidate::default()
                }, consumed: raw.len() };
                assert_eq!(candidate_preedit(raw, Some(&candidate)), expected);
            }
        });
    }

    #[test]
    fn sentence_keeps_block_three_score_order_across_syllable_counts() {
        let entries = [("表弟", "biao di", 3000), ("表", "biao", 4000),
            ("弟", "di", 3000), ("啊", "a", 2000), ("嗲", "dia", 1)]
            .into_iter().map(|(text, reading, weight)| Entry {
                text: text.into(), reading: reading.into(), weight, layer: "public-test".into(),
            }).collect();
        crate::zhixu::with_index(Arc::new(Index::from_data(DictionaryData { entries })), || {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("biaodia").unwrap();
            assert_eq!(decoder.snapshot().candidates[0].text, "表弟啊");
            assert_eq!(decoder.snapshot().preedit, "biao di a");
        });
    }

    #[test]
    fn alternate_initial_words_stay_before_partial_candidates() {
        let entries = [("马上", "ma shang", 10000), ("马", "ma", 5000), ("马鞍山", "ma an shan", 1)]
            .into_iter().map(|(text, reading, weight)| Entry {
                text: text.into(), reading: reading.into(), weight, layer: "public-test".into(),
            }).collect();
        crate::zhixu::with_index(Arc::new(Index::from_data(DictionaryData { entries })), || {
            let candidates = generate("mash", &[], 0);
            assert_eq!(candidates.iter().map(|item| item.candidate.text.as_str()).collect::<Vec<_>>(),
                vec!["马上", "马鞍山", "马"]);
        });
    }

    #[test]
    fn ideographic_zero_is_a_chinese_dictionary_candidate() {
        let entries = vec![Entry { text: "〇".into(), reading: "ling".into(), weight: 248,
            layer: "public-test".into() }];
        crate::zhixu::with_index(Arc::new(Index::from_data(DictionaryData { entries })), || {
            for raw in ["l", "ling"] {
                assert!(generate(raw, &[], 0).iter().any(|item| item.candidate.text == "〇"));
            }
        });
    }

    #[test]
    fn alternate_partial_words_share_the_frequency_order() {
        let entries = [("开始", "kai shi", 5000), ("开", "kai", 8000), ("凯", "kai", 100),
            ("可爱", "ke ai", 1000), ("下", "xia", 2000)]
            .into_iter().map(|(text, reading, weight)| Entry {
                text: text.into(), reading: reading.into(), weight, layer: "public-test".into(),
            }).collect();
        crate::zhixu::with_index(Arc::new(Index::from_data(DictionaryData { entries })), || {
            let candidates = generate("kaishix", &[], 0);
            let words: Vec<_> = candidates.iter().filter(|item| item.consumed == 3)
                .map(|item| item.candidate.text.as_str()).collect();
            assert_eq!(words, vec!["开", "可爱", "凯"]);
        });
    }

    #[test]
    fn decoder_retains_existing_learning_snapshot_and_bounds_long_preedit() {
        fixture(|| {
            let personal = Arc::new(Index::from_data(DictionaryData { entries: vec![Entry {
                text: "拟好".into(), reading: "ni hao".into(), weight: 800,
                layer: "agent_zhixu_learning".into(),
            }] }));
            let _learning = crate::zhixu::pin_learning(Some(personal));
            let mut decoder = ZhixuDecoder::new();
            decoder.input("nihao").unwrap();
            assert!(decoder.snapshot_all_candidates().unwrap().candidates.iter().any(|item| item.text == "拟好"));
            let raw = "n".repeat(4096);
            assert_eq!(format_preedit(&raw), raw);
        });
    }

    #[test]
    fn type_backspace_commit_and_clear_contract() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            assert!(!decoder.snapshot().is_last_page);
            assert!(decoder.snapshot_all_candidates().is_none());
            assert!(!decoder.snapshot_candidate_prefix(0).unwrap().is_last_page);
            assert_eq!(decoder.backspace(), Ok(String::new()));
            decoder.input("nihao").unwrap();
            assert_eq!(decoder.snapshot().preedit, "ni hao");
            assert_eq!(decoder.select_candidate(999, 1, LearningIntent::NoLearn), Err(-104));
            assert_eq!(decoder.snapshot().raw_input, "nihao");
            assert_eq!(decoder.select_candidate(0, 1, LearningIntent::NoLearn), Ok("你好".into()));
            assert!(!decoder.is_composing());
            decoder.input("nihao").unwrap();
            decoder.backspace().unwrap();
            assert_eq!(decoder.commit_raw(), "niha");
            assert!(!decoder.snapshot().is_last_page);
            assert_eq!(decoder.input("ni好"), Err(-103));
            assert_eq!(decoder.snapshot().raw_input, "ni");
            decoder.clear();
            assert!(!decoder.is_composing());
        });
    }

    #[test]
    fn paging_keeps_column_and_queries_do_not_move_live_page() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("ni").unwrap();
            decoder.select_next().unwrap();
            decoder.select_next().unwrap();
            decoder.page_down().unwrap();
            assert_eq!(decoder.snapshot().page_no, 1);
            assert_eq!(decoder.snapshot().highlighted_index, 2);
            let all = decoder.snapshot_all_candidates().unwrap();
            let prefix = decoder.snapshot_candidate_prefix(6).unwrap();
            assert_eq!(prefix.candidates, all.candidates[..6]);
            assert!(!prefix.is_last_page);
            assert_eq!(all.candidates[5].source_index, Some(5));
            assert_eq!(decoder.snapshot().page_no, 1);
            assert_eq!(decoder.snapshot().highlighted_index, 2);
            decoder.page_up().unwrap();
            assert_eq!(decoder.snapshot().highlighted_index, 2);
            decoder.select_next().unwrap();
            decoder.select_next().unwrap();
            decoder.page_down().unwrap();
            assert_eq!(decoder.snapshot().highlighted_index, 2);
            decoder.page_up().unwrap();
            assert_eq!(decoder.snapshot().highlighted_index, 2);
            decoder.select_previous_linear().unwrap();
            decoder.select_previous_linear().unwrap();
            decoder.select_previous_linear().unwrap();
            assert_eq!(decoder.snapshot().highlighted_index, 0);
        });
    }

    fn select_text(decoder: &mut ZhixuDecoder, text: &str) -> String {
        let index = decoder.snapshot_all_candidates().unwrap().candidates.iter()
            .position(|item| item.text == text).unwrap();
        decoder.select_candidate_global(index, 1, LearningIntent::DeliberateSelection).unwrap()
    }

    #[test]
    fn partial_selection_keeps_full_raw_and_commits_confirmed_prefix() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("nihaoshijie").unwrap();
            assert_eq!(select_text(&mut decoder, "你好"), "");
            assert_eq!(decoder.snapshot().raw_input, "nihaoshijie");
            assert_eq!(decoder.snapshot().preedit, "你好shi jie");
            assert_eq!(decoder.snapshot().cursor_utf8, 13);
            assert_eq!(decoder.select_candidate(0, 2, LearningIntent::NoLearn), Ok("你好世界".into()));
            assert!(!decoder.is_composing());
        });
    }

    #[test]
    fn multiple_confirmed_segments_are_not_lost_on_whole_selection() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("nihaonihaoshijie").unwrap();
            assert_eq!(select_text(&mut decoder, "你好"), "");
            assert_eq!(select_text(&mut decoder, "你好"), "");
            assert_eq!(decoder.snapshot().preedit, "你好你好shi jie");
            assert_eq!(decoder.select_candidate(0, 2, LearningIntent::NoLearn), Ok("你好你好世界".into()));
        });
    }

    #[test]
    fn backspace_undoes_last_confirmation_before_deleting_raw() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("nihaoshijie").unwrap();
            select_text(&mut decoder, "你好");
            decoder.backspace().unwrap();
            assert_eq!(decoder.snapshot().raw_input, "nihaoshijie");
            assert_eq!(decoder.snapshot().preedit, "ni haoshijie");
            assert_eq!(decoder.snapshot().highlighted_index, 1);
            decoder.backspace().unwrap();
            assert_eq!(decoder.snapshot().raw_input, "nihaoshiji");
            assert_eq!(decoder.snapshot().preedit, "ni hao shi ji");
        });
    }

    #[test]
    fn consecutive_backspaces_unwind_multiple_confirmed_segments() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("nihaonihaoshijie").unwrap();
            select_text(&mut decoder, "你好");
            select_text(&mut decoder, "你好");
            decoder.backspace().unwrap();
            assert_eq!(decoder.composition_boundary(BoundaryPurpose::Editing).unwrap().confirmed, "你好");
            decoder.backspace().unwrap();
            assert_eq!(decoder.snapshot().raw_input, "nihaonihaoshijie");
            assert!(decoder.composition_boundary(BoundaryPurpose::Editing).unwrap().confirmed.is_empty());
            decoder.backspace().unwrap();
            assert_eq!(decoder.snapshot().raw_input, "nihaonihaoshiji");
        });
    }

    #[test]
    fn typing_then_erasing_suffix_restores_last_confirmed_pinyin() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("nihaoshijie").unwrap();
            select_text(&mut decoder, "你好");
            decoder.input("a").unwrap();
            for _ in 0..7 { decoder.backspace().unwrap(); }
            assert_eq!(decoder.snapshot().raw_input, "nihao");
            assert_eq!(decoder.snapshot().preedit, "ni hao");
            decoder.backspace().unwrap();
            assert_eq!(decoder.snapshot().raw_input, "niha");
        });
    }

    #[test]
    fn required_preview_and_space_on_first_partial_commit_whole_composition() {
        fixture(|| {
            for intent in [LearningIntent::RequiredSegmentCommit, LearningIntent::NoLearn] {
                let mut decoder = ZhixuDecoder::new();
                decoder.input("nihaoi").unwrap();
                assert_eq!(decoder.candidates[0].consumed, 5);
                assert_eq!(decoder.select_candidate(0, 1, intent), Ok("你好i".into()));
                assert!(!decoder.is_composing());
            }
        });
    }

    #[test]
    fn required_preview_uses_highlighted_preview_not_requested_candidate() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("ni").unwrap();
            decoder.select_next().unwrap();
            assert_eq!(decoder.select_candidate(0, 1, LearningIntent::RequiredSegmentCommit), Ok("泥".into()));
        });
    }

    #[test]
    fn unlearned_probe_fork_matches_full_replay_without_mutating_source() {
        fn replay(stage: usize) -> ZhixuDecoder {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("nihaonihaoshijie").unwrap();
            if stage >= 1 { select_text(&mut decoder, "你好"); }
            if stage >= 2 { select_text(&mut decoder, "你好"); }
            if stage >= 3 { decoder.backspace().unwrap(); }
            if stage >= 4 { decoder.move_left().unwrap(); }
            if stage >= 5 { decoder.backspace().unwrap(); }
            decoder
        }
        fixture(|| {
            for stage in 0..=5 {
                let source = replay(stage);
                let before = format!("{:?}", source.snapshot());
                for index in 0..source.snapshot().candidates.len() {
                    let mut fork = source.fork_unlearned_composition();
                    let mut independent = replay(stage);
                    assert_eq!(format!("{:?}", fork.snapshot()), before);
                    assert_eq!(fork.select_candidate(index, 42, LearningIntent::DeliberateSelection),
                        independent.select_candidate(index, 42, LearningIntent::DeliberateSelection));
                    assert_eq!(format!("{:?}", fork.snapshot()), format!("{:?}", independent.snapshot()));
                    assert_eq!(fork.composition_boundary(BoundaryPurpose::Editing),
                        independent.composition_boundary(BoundaryPurpose::Editing));
                    assert_eq!(format!("{:?}", source.snapshot()), before);
                }
            }
        });
    }

    #[test]
    fn first_page_pageup_resets_highlight_and_boundary_navigation_is_idle() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("ni").unwrap();
            decoder.select_next().unwrap();
            decoder.page_up().unwrap();
            assert_eq!(decoder.snapshot().highlighted_index, 0);
            decoder.clear();
            decoder.input("nihaoshijie").unwrap();
            select_text(&mut decoder, "你好");
            decoder.move_left().unwrap();
            decoder.move_left().unwrap();
            let before = decoder.snapshot();
            decoder.select_next().unwrap();
            decoder.page_down().unwrap();
            decoder.page_up().unwrap();
            decoder.select_previous().unwrap();
            let after = decoder.snapshot();
            assert_eq!((after.preedit, after.cursor_utf8, after.page_no, after.highlighted_index),
                (before.preedit, before.cursor_utf8, before.page_no, before.highlighted_index));
        });
    }

    #[test]
    fn restoring_a_complete_first_syllable_returns_to_segment_steps() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("zhongguo").unwrap();
            for _ in 0..3 { decoder.move_left().unwrap(); }
            decoder.backspace().unwrap();
            decoder.input("g").unwrap();
            assert_eq!(decoder.snapshot().raw_input, "zhongguo");
            decoder.move_right().unwrap();
            assert_eq!(decoder.snapshot().cursor_utf8, 9);
            decoder.move_right().unwrap();
            assert_eq!(decoder.snapshot().cursor_utf8, 5);
        });
    }

    #[test]
    fn boundary_edit_in_first_syllable_continues_by_raw_letter() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("nihao").unwrap();
            select_text(&mut decoder, "你");
            decoder.move_left().unwrap();
            decoder.backspace().unwrap();
            assert_eq!(decoder.snapshot().raw_input, "nhao");
            decoder.move_right().unwrap();
            assert_eq!(decoder.caret, 2);
        });
    }

    #[test]
    fn navigation_inside_a_prefix_keeps_its_tail_as_one_caret_step() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("nihaoshijie").unwrap();
            decoder.select_next().unwrap();
            decoder.move_left().unwrap();
            decoder.select_next().unwrap();
            decoder.select_previous().unwrap();
            decoder.page_up().unwrap();
            decoder.move_right().unwrap();
            assert_eq!(decoder.snapshot().cursor_utf8, decoder.snapshot().preedit.len());
        });
    }

    #[test]
    fn y3b_paging_and_highlight_preserve_preedit_and_cursor() {
        fixture(|| {
            for restored_prefix in [false, true] {
                let mut decoder = ZhixuDecoder::new();
                decoder.input("nihaoshijie").unwrap();
                if restored_prefix {
                    select_text(&mut decoder, "你好");
                    decoder.backspace().unwrap();
                }
                let before = decoder.snapshot();
                let navigation: [fn(&mut ZhixuDecoder) -> Result<String, i32>; 4] = [
                    ZhixuDecoder::page_down, ZhixuDecoder::select_next,
                    ZhixuDecoder::select_previous, ZhixuDecoder::page_up,
                ];
                for action in navigation {
                    action(&mut decoder).unwrap();
                    let after = decoder.snapshot();
                    assert_eq!((&after.preedit, after.cursor_utf8),
                        (&before.preedit, before.cursor_utf8), "restored_prefix={restored_prefix}");
                }
            }
        });
    }

    #[test]
    fn y3b_confirmed_suffix_navigation_keeps_primary_syllable_spelling() {
        let entries = [
            ("做", "zuo", 1000), ("顺利", "shun li", 1000), ("顺路", "shun lu", 900),
            ("顺", "shun", 800), ("数年", "shu nian", 700), ("枢纽", "shu niu", 600),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "public-test".into(),
        }).collect();
        crate::zhixu::with_index(Arc::new(Index::from_data(DictionaryData { entries })), || {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("zuoshunl").unwrap();
            select_text(&mut decoder, "做");
            assert_eq!(decoder.snapshot().preedit, "做shun l");
            let target = decoder.candidates.iter().position(|item| item.candidate.text == "枢纽").unwrap();
            for _ in 0..target { decoder.select_next().unwrap(); }
            assert_eq!(decoder.snapshot().preedit, "做shunl");
            assert_eq!(decoder.snapshot().cursor_utf8, 8);
        });
    }

    #[test]
    fn y3b_restored_prefix_caret_ignores_paged_highlight() {
        let entries = [
            ("今天", "jin tian", 1000), ("天", "tian", 1000), ("气", "qi", 1000),
            ("需要", "xu yao", 1000), ("修", "xiu", 1000), ("改", "gai", 1000),
            ("祝你", "zhu ni", 1000), ("一", "yi", 1000), ("路", "lu", 1000), ("顺", "shun", 1000),
            ("今", "jin", 900), ("仅", "jin", 800), ("紧", "jin", 700), ("近", "jin", 600),
            ("须", "xu", 900), ("徐", "xu", 800), ("许", "xu", 700), ("续", "xu", 600),
            ("朱", "zhu", 900), ("住", "zhu", 800), ("主", "zhu", 700), ("助", "zhu", 600),
        ].into_iter().map(|(text, reading, weight)| Entry {
            text: text.into(), reading: reading.into(), weight, layer: "public-test".into(),
        }).collect();
        crate::zhixu::with_index(Arc::new(Index::from_data(DictionaryData { entries })), || {
            for (raw, prefix, expected_preedit, expected_cursor) in [
                ("jintiantianqi", "今天", "jin tiantianqi", 8),
                ("xuyaoxiugai", "需要", "xu yaoxiugai", 6),
                ("zhuniyilushun", "祝你", "zhu niyilushun", 6),
            ] {
                let mut decoder = ZhixuDecoder::new();
                decoder.input(raw).unwrap();
                select_text(&mut decoder, prefix);
                decoder.backspace().unwrap();
                decoder.page_down().unwrap();
                decoder.move_left().unwrap();
                assert_eq!((decoder.snapshot().preedit, decoder.snapshot().cursor_utf8),
                    (expected_preedit.into(), expected_cursor), "{raw}: left");
                decoder.select_next().unwrap();
                decoder.select_previous().unwrap();
                decoder.page_up().unwrap();
                assert_eq!((decoder.snapshot().preedit, decoder.snapshot().cursor_utf8),
                    (expected_preedit.into(), expected_cursor), "{raw}: navigation");
            }
        });
    }

    #[test]
    fn caret_ignores_highlighted_prefix_and_restores_whole_tail_spelling() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("nihaoshijie").unwrap();
            decoder.select_next().unwrap();
            assert_eq!(decoder.snapshot().candidates[1].text, "你好");
            decoder.move_left().unwrap();
            assert_eq!(decoder.snapshot().cursor_utf8, 9);
            assert_eq!(decoder.snapshot().preedit, "ni haoshijie");
            decoder.move_right().unwrap();
            assert_eq!(decoder.snapshot().preedit, "ni hao shi jie");
            assert_eq!(decoder.snapshot().cursor_utf8, 14);
        });
    }

    #[test]
    fn backspace_at_confirmed_boundary_reopens_and_erases_previous_letter() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("nihaoshijie").unwrap();
            select_text(&mut decoder, "你好");
            decoder.move_left().unwrap();
            decoder.move_left().unwrap();
            decoder.backspace().unwrap();
            assert_eq!(decoder.snapshot().raw_input, "nihashijie");
            assert_eq!(decoder.snapshot().preedit, "ni hashijie");
            assert_eq!(decoder.snapshot().cursor_utf8, 5);
            assert!(decoder.composition_boundary(BoundaryPurpose::Editing).unwrap().confirmed.is_empty());
        });
    }

    #[test]
    fn restored_raw_tail_keeps_ascii_v_and_caret_span_spacing() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("nihaolv").unwrap();
            select_text(&mut decoder, "你好");
            decoder.backspace().unwrap();
            assert_eq!(decoder.snapshot().preedit, "ni haolv");
            decoder.clear();
            decoder.input("nihaoshijie").unwrap();
            decoder.move_left().unwrap();
            decoder.select_next().unwrap();
            assert_eq!(decoder.snapshot().preedit, "ni haoshijie");
            assert_eq!(decoder.snapshot().cursor_utf8, 9);
        });
    }

    #[test]
    fn confirmed_segment_makes_single_remaining_syllable_a_caret_step() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("nihaoni").unwrap();
            select_text(&mut decoder, "你好");
            let whole = decoder.snapshot().candidates;
            for _ in 0..3 {
                decoder.move_left().unwrap();
                assert_eq!(decoder.snapshot().preedit, "你好ni");
                assert_eq!(decoder.snapshot().cursor_utf8, 6);
                assert_eq!(decoder.snapshot().candidates, whole);
            }
            for _ in 0..3 {
                decoder.move_right().unwrap();
                assert_eq!(decoder.snapshot().cursor_utf8, 8);
            }
        });
    }

    #[test]
    fn caret_keeps_whole_menu_and_rime_syllable_steps() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("nihao").unwrap();
            let candidates = decoder.snapshot().candidates;
            for (left, expected_cursor, expected_preedit) in [
                (true, 2, "nihao"), (true, 0, "ni hao"),
                (false, 2, "nihao"), (false, 6, "ni hao"),
            ] {
                if left { decoder.move_left().unwrap(); } else { decoder.move_right().unwrap(); }
                let snapshot = decoder.snapshot();
                assert_eq!(snapshot.cursor_utf8, expected_cursor);
                assert_eq!(snapshot.preedit, expected_preedit);
                assert_eq!(snapshot.candidates, candidates);
            }
            decoder.move_left().unwrap();
            assert_eq!(decoder.select_candidate(0, 1, LearningIntent::NoLearn), Ok("你好".into()));
        });
    }

    #[test]
    fn editing_partial_pick_finishes_exact_displayed_candidate() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("nihaoshijie").unwrap();
            select_text(&mut decoder, "你好");
            decoder.move_left().unwrap();
            let displayed = decoder.snapshot().candidates[0].text.clone();
            assert_eq!(decoder.select_candidate(0, 2, LearningIntent::NoLearn), Ok(format!("你好{displayed}")));
            assert!(!decoder.is_composing());
            decoder.input("nihao").unwrap();
            decoder.move_left().unwrap();
            assert_eq!(select_text(&mut decoder, "你"), "你");
            assert!(!decoder.is_composing());
        });
    }

    #[test]
    fn editing_candidate_coverage_matches_cleared_raw() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("nihao").unwrap();
            decoder.move_left().unwrap();
            let index = decoder.snapshot().candidates.iter().position(|candidate| candidate.text == "你").unwrap();
            assert_eq!(decoder.candidate_covered_letters(index), Some(5));
            assert_eq!(decoder.select_candidate(index, 1, LearningIntent::DeliberateSelection), Ok("你".into()));
            assert!(decoder.snapshot().raw_input.is_empty());
        });
    }

    #[test]
    fn exact_boundary_is_independent_of_preedit_and_tracks_reopened_suffix() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("nihaoshijie").unwrap();
            select_text(&mut decoder, "你好");
            for purpose in [BoundaryPurpose::Editing, BoundaryPurpose::Mixing] {
                let boundary = decoder.composition_boundary(purpose).unwrap();
                assert_eq!(boundary.confirmed, "你好");
                assert_eq!(boundary.unconfirmed_raw, "shijie");
            }
            decoder.input("a").unwrap();
            for _ in 0..7 { decoder.backspace().unwrap(); }
            let boundary = decoder.composition_boundary(BoundaryPurpose::Mixing).unwrap();
            assert!(boundary.confirmed.is_empty());
            assert_eq!(boundary.unconfirmed_raw, "nihao");
            decoder.clear();
            assert!(decoder.composition_boundary(BoundaryPurpose::Editing).unwrap().confirmed.is_empty());
        });
    }

    #[test]
    fn caret_inserts_and_deletes_at_raw_position() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            decoder.input("ni").unwrap();
            decoder.move_left().unwrap();
            decoder.input("h").unwrap();
            assert_eq!(decoder.snapshot().raw_input, "nhi");
            decoder.backspace().unwrap();
            assert_eq!(decoder.snapshot().raw_input, "ni");
            decoder.move_right().unwrap();
            assert_eq!(decoder.snapshot().cursor_utf8, 2);
        });
    }

    #[test]
    fn caret_umlaut_and_separator_offsets_are_utf8_boundaries() {
        fixture(|| {
            for (raw, left_cursor) in [("nvhao", 3), ("ni'hao", 3)] {
                let mut decoder = ZhixuDecoder::new();
                decoder.input(raw).unwrap();
                decoder.move_left().unwrap();
                let snapshot = decoder.snapshot();
                assert_eq!(snapshot.cursor_utf8, left_cursor, "{raw}");
                assert!(snapshot.preedit.is_char_boundary(snapshot.cursor_utf8));
            }
        });
    }

    #[test]
    fn clear_and_commit_raw_handle_confirmed_segments() {
        fixture(|| {
            let mut decoder = ZhixuDecoder::new();
            for clear_after in [false, true] {
                decoder.input("nihaoshijie").unwrap();
                select_text(&mut decoder, "你好");
                assert!(decoder.snapshot().preedit.starts_with("你好"));
                if clear_after { decoder.clear_after_selection(); } else { decoder.clear(); }
                assert!(decoder.snapshot().preedit.is_empty());
                assert!(decoder.snapshot().raw_input.is_empty());
            }
            decoder.input("nihaoshijie").unwrap();
            select_text(&mut decoder, "你好");
            assert_eq!(decoder.commit_raw(), "nihaoshijie");
            decoder.input("nihao").unwrap();
            assert_eq!(decoder.select_candidate(0, 2, LearningIntent::NoLearn), Ok("你好".into()));
        });
    }
}

//! Compact syllable trie shared by the lattice and typo decoders.

#[derive(Clone, Copy, Debug, Default)]
struct CompactReadingNode {
    letters: u32,
    children_start: u32,
    words_start: u32,
    words_len: u32,
}

/// Complete readings use one letter walk, with terminal ranges in the original
/// homophone order. The correction trie's bounded projection stays independent.
pub(crate) struct CompactReadingTrie {
    nodes: Vec<CompactReadingNode>,
    children: Vec<u32>,
}

impl CompactReadingTrie {
    pub(crate) fn new<'a>(readings: impl Iterator<Item = (&'a str, u32, u32)>) -> Self {
        let mut nodes = vec![CompactReadingNode::default()];
        let mut edges = Vec::new();
        let mut path = vec![0usize];
        let mut previous = "";
        // Sorted keys append every parent's children in letter order.
        for (reading, words_start, words_len) in readings {
            if reading.is_empty() || !reading.bytes().all(|letter| letter.is_ascii_lowercase()) {
                continue;
            }
            let shared = reading.bytes().zip(previous.bytes())
                .take_while(|(left, right)| left == right).count();
            path.truncate(shared + 1);
            for letter in reading.bytes().skip(shared) {
                let parent = *path.last().unwrap();
                nodes[parent].letters |= 1 << (letter - b'a');
                let child = nodes.len();
                nodes.push(CompactReadingNode::default());
                edges.push((parent as u32, child as u32));
                path.push(child);
            }
            let node = &mut nodes[*path.last().unwrap()];
            node.words_start = words_start;
            node.words_len = words_len;
            previous = reading;
        }
        let mut offset = 0;
        for node in &mut nodes {
            node.children_start = offset;
            offset += node.letters.count_ones();
        }
        let mut children = vec![0; edges.len()];
        for (parent, child) in edges {
            let node = &mut nodes[parent as usize];
            children[node.children_start as usize] = child;
            node.children_start += 1;
        }
        for node in &mut nodes {
            node.children_start -= node.letters.count_ones();
        }
        nodes.shrink_to_fit();
        Self { nodes, children }
    }

    pub(crate) fn visit_prefixes(&self, reading: &str, mut visitor: impl FnMut(usize, std::ops::Range<usize>)) {
        let mut node = &self.nodes[0];
        for (offset, letter) in reading.bytes().enumerate() {
            if letter == b'\'' { continue; }
            if !letter.is_ascii_lowercase() { break; }
            let bit = 1 << (letter - b'a');
            if node.letters & bit == 0 { break; }
            let child = node.children_start + (node.letters & (bit - 1)).count_ones();
            node = &self.nodes[self.children[child as usize] as usize];
            if node.words_len != 0 {
                let start = node.words_start as usize;
                visitor(offset + 1, start..start + node.words_len as usize);
            }
        }
    }

    /// The node reached from `node` by `letters`, if the trie has that path.
    pub(crate) fn walk(&self, mut node: usize, letters: &str) -> Option<usize> {
        for letter in letters.bytes() { node = self.child(node, letter)?; }
        Some(node)
    }

    /// Word range of the reading that ends exactly at `node` (empty if none).
    pub(crate) fn terminal(&self, node: usize) -> std::ops::Range<usize> {
        let node = &self.nodes[node];
        let start = node.words_start as usize;
        start..start + node.words_len as usize
    }

    fn child(&self, node: usize, letter: u8) -> Option<usize> {
        if !letter.is_ascii_lowercase() { return None; }
        let node = &self.nodes[node];
        let bit = 1 << (letter - b'a');
        if node.letters & bit == 0 { return None; }
        let child = node.children_start + (node.letters & (bit - 1)).count_ones();
        Some(self.children[child as usize] as usize)
    }

    /// Like `visit_prefixes`, but also walks the spellings fuzzy groups allow:
    /// z/c/s with or without h, n for l, f for h, and an/en/in with or without g.
    /// The walk is deliberately permissive; callers verify each match against
    /// the word's own syllables. Each (end, terminal) is visited once.
    pub(crate) fn visit_prefixes_fuzzy(&self, reading: &str, mask: u32,
        mut visitor: impl FnMut(usize, std::ops::Range<usize>)) {
        let bytes = reading.as_bytes();
        let at = |position: usize| bytes.get(position).copied().unwrap_or(0);
        let mut seen = std::collections::HashSet::new();
        let mut emitted = std::collections::HashSet::new();
        let mut stack = vec![(0usize, 0usize)];
        while let Some((node, position)) = stack.pop() {
            if !seen.insert((node, position)) { continue; }
            if node != 0 && self.nodes[node].words_len != 0 && emitted.insert((position, node)) {
                let start = self.nodes[node].words_start as usize;
                visitor(position, start..start + self.nodes[node].words_len as usize);
            }
            let letter = at(position);
            // an/en/in typed after the vowel: the dictionary may continue with g
            // (also at the end of the input), and a typed g may be absent from it.
            let final_bit = |vowel: u8| match vowel { b'a' => 1 << 5, b'e' => 1 << 6, b'i' => 1 << 7, _ => 0 };
            if position >= 2 && at(position - 1) == b'n' && mask & final_bit(at(position - 2)) != 0 {
                if let Some(child) = self.child(node, b'g') { stack.push((child, position)); }
                if letter == b'g' { stack.push((node, position + 1)); }
            }
            if letter == b'\'' { stack.push((node, position + 1)); continue; }
            if !letter.is_ascii_lowercase() { continue; }
            if let Some(child) = self.child(node, letter) { stack.push((child, position + 1)); }
            let initial_bit = match letter { b'z' => 1 << 0, b'c' => 1 << 1, b's' => 1 << 2, _ => 0 };
            if mask & initial_bit != 0 {
                if at(position + 1) == b'h' {
                    // zh typed, z in the dictionary.
                    if let Some(child) = self.child(node, letter) { stack.push((child, position + 2)); }
                } else if let Some(child) = self.child(node, letter).and_then(|child| self.child(child, b'h')) {
                    stack.push((child, position + 1));
                }
            }
            let swapped = match letter {
                b'n' if mask & (1 << 3) != 0 => b'l', b'l' if mask & (1 << 3) != 0 => b'n',
                b'f' if mask & (1 << 4) != 0 => b'h', b'h' if mask & (1 << 4) != 0 => b'f',
                _ => 0,
            };
            if let Some(child) = (swapped != 0).then(|| self.child(node, swapped)).flatten() {
                stack.push((child, position + 1));
            }
        }
    }

    pub(crate) fn estimated_bytes(&self) -> usize {
        self.nodes.capacity() * std::mem::size_of::<CompactReadingNode>()
            + self.children.capacity() * std::mem::size_of::<u32>()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HeavyTerminal {
    pub(crate) text: u32,
    pub(crate) lexical_score: i64,
    pub(crate) full_letter_count: u8,
}

#[derive(Clone, Copy, Debug, Default)]
struct HeavyTrieNode {
    letters: u32,
    children_start: u32,
    boundary: u32,
    abbreviation_start: u32,
    abbreviation_len: u32,
    words_start: u32,
    exact_len: u32,
    prefix_len: u32,
    terminal: u32,
}

const PAGE_LEN: usize = 4096;

/// Fixed-size pages avoid reallocating a large node buffer while the old allocation is live.
#[derive(Clone, Debug)]
struct Pool<T> {
    pages: Vec<Vec<T>>,
    len: usize,
}

impl<T> Pool<T> {
    fn new() -> Self {
        Self {
            pages: Vec::new(),
            len: 0,
        }
    }

    fn push(&mut self, value: T) {
        if self.len % PAGE_LEN == 0 {
            self.pages.push(Vec::with_capacity(PAGE_LEN));
        }
        self.pages.last_mut().unwrap().push(value);
        self.len += 1;
    }

    fn len(&self) -> usize {
        self.len
    }

    fn get(&self, index: usize) -> Option<&T> {
        (index < self.len).then(|| &self[index])
    }

    fn estimated_bytes(&self) -> usize {
        self.pages.capacity() * std::mem::size_of::<Vec<T>>()
            + self
                .pages
                .iter()
                .map(|page| page.capacity() * std::mem::size_of::<T>())
                .sum::<usize>()
    }
}

impl<T> std::ops::Index<usize> for Pool<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        &self.pages[index / PAGE_LEN][index % PAGE_LEN]
    }
}

impl<T> std::ops::IndexMut<usize> for Pool<T> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        &mut self.pages[index / PAGE_LEN][index % PAGE_LEN]
    }
}

#[derive(Clone, Copy, Debug)]
struct BuildEdge {
    child: u32,
    next: u32,
    letter: u8,
}

#[derive(Clone, Copy, Debug)]
struct Link {
    value: u32,
    next: u32,
}

#[derive(Clone, Debug)]
struct Builder {
    nodes: Pool<HeavyTrieNode>,
    children: Pool<BuildEdge>,
    abbreviations: Pool<Link>,
    words: Pool<Link>,
    terminals: Vec<HeavyTerminal>,
}

#[derive(Clone, Debug)]
pub(crate) struct HeavyTrie {
    nodes: Pool<HeavyTrieNode>,
    children: Vec<u32>,
    abbreviation_boundaries: Vec<u32>,
    terminals: Vec<HeavyTerminal>,
    words: Vec<HeavyTerminal>,
    builder: Option<Builder>,
}

impl Default for HeavyTrie {
    fn default() -> Self {
        Self::new()
    }
}

impl HeavyTrie {
    pub(crate) fn new() -> Self {
        let mut nodes = Pool::new();
        nodes.push(HeavyTrieNode::default());
        Self {
            nodes: Pool::new(),
            children: Vec::new(),
            abbreviation_boundaries: Vec::new(),
            terminals: Vec::new(),
            words: Vec::new(),
            builder: Some(Builder {
                nodes,
                children: Pool::new(),
                abbreviations: Pool::new(),
                words: Pool::new(),
                terminals: Vec::new(),
            }),
        }
    }

    pub(crate) fn insert(&mut self, text: u32, reading: &str, weight: u32) {
        self.builder
            .as_mut()
            .expect("cannot insert into a frozen trie")
            .insert(text, reading, weight);
    }

    /// Reuse node pages in place. During construction offsets hold linked-list heads;
    /// prefix_len holds the prefix head, and exact_len already holds the exact count.
    pub(crate) fn finish(&mut self) {
        let Some(Builder {
            mut nodes,
            children,
            abbreviations,
            words,
            mut terminals,
        }) = self.builder.take()
        else {
            return;
        };
        self.children = Vec::with_capacity(children.len());
        self.abbreviation_boundaries = Vec::with_capacity(abbreviations.len());
        self.words = Vec::with_capacity(words.len());
        for id in 0..nodes.len() {
            let node = nodes[id];
            let mut next = node.children_start;
            let mut slots = [0u32; 26];
            while next != 0 {
                let edge = children[next as usize - 1];
                slots[(edge.letter - b'a') as usize] = edge.child;
                next = edge.next;
            }
            nodes[id].children_start = self.children.len() as u32;
            self.children
                .extend(slots.into_iter().filter(|slot| *slot != 0));

            next = node.abbreviation_start;
            let start = self.abbreviation_boundaries.len();
            while next != 0 {
                let link = abbreviations[next as usize - 1];
                self.abbreviation_boundaries.push(link.value);
                next = link.next;
            }
            self.abbreviation_boundaries[start..].reverse();
            nodes[id].abbreviation_start = start as u32;

            nodes[id].words_start = self.words.len() as u32;
            for head in [node.words_start, node.prefix_len] {
                next = head;
                while next != 0 {
                    let link = words[next as usize - 1];
                    self.words.push(terminals[link.value as usize]);
                    next = link.next;
                }
            }
            nodes[id].prefix_len = self.words.len() as u32 - nodes[id].words_start - node.exact_len;
        }
        self.nodes = nodes;
        drop(children);
        drop(abbreviations);
        drop(words);
        terminals.shrink_to_fit();
        self.terminals = terminals;
    }

    pub(crate) fn letter_child(&self, node: usize, letter: u8) -> Option<usize> {
        let offset = letter.checked_sub(b'a')?;
        if offset >= 26 {
            return None;
        }
        let node = self.nodes.get(node)?;
        let bit = 1u32 << offset;
        if node.letters & bit == 0 {
            return None;
        }
        let rank = (node.letters & (bit - 1)).count_ones() as usize;
        Some(self.children[node.children_start as usize + rank] as usize - 1)
    }

    pub(crate) fn boundary_children(&self, node: usize) -> impl Iterator<Item = usize> + '_ {
        let node = &self.nodes[node];
        let start = node.abbreviation_start as usize;
        (node.boundary != 0)
            .then(|| node.boundary as usize - 1)
            .into_iter()
            .chain(
                self.abbreviation_boundaries[start..start + node.abbreviation_len as usize]
                    .iter()
                    .map(|slot| *slot as usize - 1),
            )
    }

    pub(crate) fn terminal(&self, node: usize) -> Option<HeavyTerminal> {
        let slot = self.nodes[node].terminal;
        (slot != 0).then(|| self.terminals[slot as usize - 1])
    }

    pub(crate) fn terminals(&self, node: usize) -> &[HeavyTerminal] {
        let node = &self.nodes[node];
        let start = node.words_start as usize;
        let len = if node.exact_len != 0 {
            node.exact_len
        } else {
            node.prefix_len
        };
        &self.words[start..start + len as usize]
    }

    pub(crate) fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub(crate) fn memory_breakdown(&self) -> [(&'static str, usize); 5] {
        use std::mem::size_of;
        [
            ("nodes", self.nodes.estimated_bytes()),
            ("children", self.children.capacity() * size_of::<u32>()),
            (
                "abbreviations",
                self.abbreviation_boundaries.capacity() * size_of::<u32>(),
            ),
            (
                "terminals",
                self.terminals.capacity() * size_of::<HeavyTerminal>(),
            ),
            (
                "homophones",
                self.words.capacity() * size_of::<HeavyTerminal>(),
            ),
        ]
    }

    pub(crate) fn estimated_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self
                .memory_breakdown()
                .iter()
                .map(|(_, bytes)| bytes)
                .sum::<usize>()
    }
}

impl Builder {
    fn ensure_letter(&mut self, node: usize, letter: u8) -> usize {
        let bit = 1u32 << (letter - b'a');
        let head = self.nodes[node].children_start;
        if self.nodes[node].letters & bit != 0 {
            let mut next = head;
            while next != 0 {
                let edge = self.children[next as usize - 1];
                if edge.letter == letter {
                    return edge.child as usize - 1;
                }
                next = edge.next;
            }
            unreachable!("letter bitmap must match its child list");
        }
        let child = self.nodes.len();
        self.nodes.push(HeavyTrieNode::default());
        self.children.push(BuildEdge {
            child: child as u32 + 1,
            next: head,
            letter,
        });
        self.nodes[node].letters |= bit;
        self.nodes[node].children_start = self.children.len() as u32;
        child
    }

    fn ensure_boundary(&mut self, node: usize) -> usize {
        let current = self.nodes[node].boundary;
        if current != 0 {
            return current as usize - 1;
        }
        let child = self.nodes.len();
        self.nodes.push(HeavyTrieNode::default());
        self.nodes[node].boundary = child as u32 + 1;
        child
    }

    fn abbreviation(&mut self, node: usize, boundary: u32) {
        let head = self.nodes[node].abbreviation_start;
        let mut next = head;
        while next != 0 {
            let link = self.abbreviations[next as usize - 1];
            if link.value == boundary {
                return;
            }
            next = link.next;
        }
        self.abbreviations.push(Link {
            value: boundary,
            next: head,
        });
        self.nodes[node].abbreviation_start = self.abbreviations.len() as u32;
        self.nodes[node].abbreviation_len += 1;
    }

    fn better(left: HeavyTerminal, right: HeavyTerminal) -> bool {
        left.lexical_score > right.lexical_score
            || left.lexical_score == right.lexical_score && left.text < right.text
    }

    fn prefer_terminal(slot: &mut u32, candidate: u32, terminals: &[HeavyTerminal]) {
        if *slot == 0 || Self::better(terminals[candidate as usize], terminals[*slot as usize - 1])
        {
            *slot = candidate + 1;
        }
    }

    /// Preserve first-seen duplicate text. A full list reuses its displaced tail slot.
    fn keep_top(
        head: &mut u32,
        candidate: u32,
        words: &mut Pool<Link>,
        terminals: &[HeavyTerminal],
    ) -> bool {
        let value = terminals[candidate as usize];
        let (mut next, mut previous, mut penultimate, mut count) = (*head, 0, 0, 0);
        let mut insertion = None;
        while next != 0 {
            let link = words[next as usize - 1];
            let kept = terminals[link.value as usize];
            if kept.text == value.text {
                return false;
            }
            if insertion.is_none() && Self::better(value, kept) {
                insertion = Some((previous, next, count));
            }
            penultimate = previous;
            previous = next;
            next = link.next;
            count += 1;
        }
        let (before, mut after, position) = insertion.unwrap_or((previous, 0, count));
        let keep = homophones();
        if position >= keep {
            return false;
        }
        let grew = count < keep;
        let slot = if grew {
            words.push(Link {
                value: candidate,
                next: 0,
            });
            words.len() as u32
        } else {
            if penultimate == 0 {
                *head = 0;
            } else {
                words[penultimate as usize - 1].next = 0;
            }
            if after == previous {
                after = 0;
            }
            previous
        };
        words[slot as usize - 1] = Link {
            value: candidate,
            next: after,
        };
        if before == 0 {
            *head = slot;
        } else {
            words[before as usize - 1].next = slot;
        }
        grew
    }

    fn insert(&mut self, text: u32, reading: &str, weight: u32) {
        let syllables = reading.split_whitespace().collect::<Vec<_>>();
        let candidate = self.terminals.len() as u32;
        self.terminals.push(HeavyTerminal {
            text,
            lexical_score: ((weight.max(1) as f64).powf(0.2) * 1_000.0).round() as i64,
            full_letter_count: reading.bytes().filter(u8::is_ascii_lowercase).count() as u8,
        });
        let mut node = 0usize;
        for (syllable_index, syllable) in syllables.iter().enumerate() {
            let mut prefix_nodes = Vec::with_capacity(syllable.len());
            for (letter_index, letter) in syllable.bytes().enumerate() {
                node = self.ensure_letter(node, letter);
                prefix_nodes.push(node);
                if syllable_index + 1 == syllables.len() && letter_index + 1 < syllable.len() {
                    let node = &mut self.nodes[node];
                    if node.exact_len == 0 {
                        Self::prefer_terminal(&mut node.terminal, candidate, &self.terminals);
                    }
                    Self::keep_top(
                        &mut node.prefix_len,
                        candidate,
                        &mut self.words,
                        &self.terminals,
                    );
                }
            }
            if syllable_index + 1 < syllables.len() {
                let boundary = self.ensure_boundary(node);
                let boundary_slot = boundary as u32 + 1;
                for prefix_node in prefix_nodes
                    .into_iter()
                    .take(syllable.len().saturating_sub(1))
                {
                    self.abbreviation(prefix_node, boundary_slot);
                }
                node = boundary;
            }
        }
        let node = &mut self.nodes[node];
        if node.exact_len == 0 {
            node.terminal = candidate + 1;
        } else {
            Self::prefer_terminal(&mut node.terminal, candidate, &self.terminals);
        }
        if Self::keep_top(
            &mut node.words_start,
            candidate,
            &mut self.words,
            &self.terminals,
        ) {
            node.exact_len += 1;
        }
    }
}

fn homophones() -> usize {
    static KEEP: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *KEEP.get_or_init(|| {
        std::env::var("ZHIZI_LAT_HOMO")
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|value| *value > 0)
            .unwrap_or(6)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_trie_node_fits_compact_budget() {
        assert!(
            std::mem::size_of::<HeavyTrieNode>() <= 36,
            "frozen node reserves {} bytes",
            std::mem::size_of::<HeavyTrieNode>()
        );
    }

    #[test]
    fn compact_trie_preserves_nodes_edges_and_duplicate_prefix_terminals() {
        let mut compact = HeavyTrie::new();
        let mut reference = ReferenceTrie::new();
        let mut insert = |text, reading, weight| {
            compact.insert(text, reading, weight);
            reference.insert(text, reading, weight);
        };
        for text in (0..14).rev() {
            insert(text, "ni hao", 1_000 + (text % 3) * 100);
        }
        for (text, reading, weight) in [
            (20, "shi", 10),
            (20, "shan", 8_000),
            (21, "shao", 900),
            (22, "sha", 10),
            (23, "shang", 900),
            (24, "sha", 100),
            (25, "shi jie", 50),
            (26, "shi jing", 40),
            (27, "si jie", 30),
            (28, "shi", 10),
            (29, "zhao hua", 20),
            (30, "zhuan hua", 20),
            (31, "zhong hua", 20),
            (32, "ni hao shi jie", 60),
            (33, "n", 0),
            (0, "ni hao", 9_000),
        ] {
            insert(text, reading, weight);
        }
        let readings = [
            "ni hao", "ni he", "sha", "shan", "shang", "shi", "shi jie", "si jie",
        ];
        let mut state = 7u32;
        for _ in 0..500 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            insert(
                state % 31,
                readings[(state >> 8) as usize % readings.len()],
                state % 10_000,
            );
        }
        compact.finish();
        compact.finish();
        assert!(compact.builder.is_none());
        assert_eq!(compact.node_count(), reference.nodes.len());
        for (id, expected) in reference.nodes.iter().enumerate() {
            for letter in b'`'..=b'{' {
                assert_eq!(
                    compact.letter_child(id, letter),
                    reference.letter_child(id, letter),
                    "letter {letter} of node {id}"
                );
            }
            assert_eq!(
                compact.boundary_children(id).collect::<Vec<_>>(),
                reference.boundary_children(id).collect::<Vec<_>>(),
                "boundaries of node {id}"
            );
            assert_eq!(
                compact.terminal(id),
                expected.exact.or(expected.final_prefix),
                "best terminal of node {id}"
            );
            let node = &compact.nodes[id];
            let start = node.words_start as usize;
            let end = start + node.exact_len as usize;
            assert_eq!(
                &compact.words[start..end],
                &expected.exact_all,
                "exact list of node {id}"
            );
            assert_eq!(
                &compact.words[end..end + node.prefix_len as usize],
                &expected.prefix_all,
                "prefix list of node {id}"
            );
            assert_eq!(
                compact.terminals(id),
                if expected.exact.is_some() {
                    &expected.exact_all
                } else {
                    &expected.prefix_all
                }
            );
        }
        assert_eq!(compact.letter_child(usize::MAX, b'a'), None);
        assert_eq!(compact.letter_child(0, 0), None);
        assert_eq!(compact.letter_child(0, 255), None);
    }

    #[test]
    fn fallback_trie_stays_within_twenty_four_mib() {
        let entries: Vec<_> = include_str!("../../resources/heavy_garble_words.tsv")
            .lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| {
                let mut columns = line.split('\t');
                Some((
                    columns.next()?,
                    columns.next()?,
                    columns.next()?.parse::<u32>().ok()?,
                ))
            })
            .collect();
        let mut texts: Vec<_> = entries.iter().map(|(text, _, _)| *text).collect();
        texts.sort_unstable();
        texts.dedup();
        let mut trie = HeavyTrie::new();
        for (text, reading, weight) in entries {
            trie.insert(texts.binary_search(&text).unwrap() as u32, reading, weight);
        }
        let builder = trie.builder.as_ref().unwrap();
        let construction = builder.nodes.estimated_bytes()
            + builder.children.estimated_bytes()
            + builder.abbreviations.estimated_bytes()
            + builder.words.estimated_bytes()
            + builder.terminals.capacity() * std::mem::size_of::<HeavyTerminal>();
        let freezing = construction
            + (builder.children.len() + builder.abbreviations.len()) * std::mem::size_of::<u32>()
            + builder.words.len() * std::mem::size_of::<HeavyTerminal>();
        assert!(
            freezing <= 40 * 1024 * 1024,
            "trie construction reserves {freezing} bytes"
        );
        trie.finish();
        assert!(
            trie.estimated_bytes() <= 24 * 1024 * 1024,
            "fallback trie uses {} bytes: {:?}",
            trie.estimated_bytes(),
            trie.memory_breakdown()
        );
        assert!(trie.node_count() > 250_000);
    }

    // The previous representation is deliberately independent of the compact builder.
    #[derive(Clone, Debug)]
    pub(crate) struct ReferenceNode {
        /// Child indices plus one; zero is the absent sentinel so the node stays
        /// compact and lookups never allocate or hash on the keystroke path.
        pub(crate) letters: [u32; 26],
        pub(crate) boundary: u32,
        pub(crate) abbreviation_boundaries: Vec<u32>,
        pub(crate) exact: Option<HeavyTerminal>,
        pub(crate) final_prefix: Option<HeavyTerminal>,
        /// The lattice decoder's homophones: the best few words per reading (and per
        /// typed-as-prefix reading), best first. `exact` and `final_prefix` stay the single
        /// best, so the heavy-garble layer and the whole-reading decoder do not change.
        pub(crate) exact_all: Vec<HeavyTerminal>,
        pub(crate) prefix_all: Vec<HeavyTerminal>,
    }

    impl Default for ReferenceNode {
        fn default() -> Self {
            Self {
                letters: [0; 26],
                boundary: 0,
                abbreviation_boundaries: Vec::new(),
                exact: None,
                exact_all: Vec::new(),
                prefix_all: Vec::new(),
                final_prefix: None,
            }
        }
    }

    #[derive(Clone, Debug, Default)]
    pub(crate) struct ReferenceTrie {
        pub(crate) nodes: Vec<ReferenceNode>,
    }

    impl ReferenceTrie {
        pub(crate) fn new() -> Self {
            Self {
                nodes: vec![ReferenceNode::default()],
            }
        }

        pub(crate) fn letter_child(&self, node: usize, letter: u8) -> Option<usize> {
            let slot = *self
                .nodes
                .get(node)?
                .letters
                .get(usize::from(letter.checked_sub(b'a')?))?;
            (slot != 0).then(|| slot as usize - 1)
        }

        pub(crate) fn boundary_children(&self, node: usize) -> impl Iterator<Item = usize> + '_ {
            let trie_node = &self.nodes[node];
            (trie_node.boundary != 0)
                .then(|| trie_node.boundary as usize - 1)
                .into_iter()
                .chain(
                    trie_node
                        .abbreviation_boundaries
                        .iter()
                        .map(|slot| *slot as usize - 1),
                )
        }

        pub(crate) fn ensure_letter(&mut self, node: usize, letter: u8) -> usize {
            let offset = usize::from(letter - b'a');
            let current = self.nodes[node].letters[offset];
            if current != 0 {
                return current as usize - 1;
            }
            let child = self.nodes.len();
            self.nodes.push(ReferenceNode::default());
            self.nodes[node].letters[offset] = child as u32 + 1;
            child
        }

        pub(crate) fn ensure_boundary(&mut self, node: usize) -> usize {
            let current = self.nodes[node].boundary;
            if current != 0 {
                return current as usize - 1;
            }
            let child = self.nodes.len();
            self.nodes.push(ReferenceNode::default());
            self.nodes[node].boundary = child as u32 + 1;
            child
        }

        pub(crate) fn prefer_terminal(slot: &mut Option<HeavyTerminal>, candidate: HeavyTerminal) {
            let replace = slot.is_none_or(|current| {
                candidate.lexical_score > current.lexical_score
                    || candidate.lexical_score == current.lexical_score
                        && candidate.text < current.text
            });
            if replace {
                *slot = Some(candidate);
            }
        }

        /// Keep the best `LATTICE_HOMOPHONES` words of a reading, best first, one per text.
        pub(crate) fn keep_top(list: &mut Vec<HeavyTerminal>, candidate: HeavyTerminal) {
            if list.iter().any(|kept| kept.text == candidate.text) {
                return;
            }
            let at = list
                .iter()
                .position(|kept| {
                    candidate.lexical_score > kept.lexical_score
                        || candidate.lexical_score == kept.lexical_score
                            && candidate.text < kept.text
                })
                .unwrap_or(list.len());
            let keep = homophones();
            if at < keep {
                list.insert(at, candidate);
                list.truncate(keep);
            }
        }

        pub(crate) fn insert(&mut self, text: u32, reading: &str, weight: u32) {
            let syllables = reading.split_whitespace().collect::<Vec<_>>();
            let terminal = HeavyTerminal {
                text,
                // This is the probe's dictionary-weight term, scaled to an integer
                // once at index construction so ranking is deterministic and the
                // keystroke path performs no floating-point work.
                lexical_score: ((weight.max(1) as f64).powf(0.2) * 1_000.0).round() as i64,
                full_letter_count: reading.bytes().filter(u8::is_ascii_lowercase).count() as u8,
            };
            let mut node = 0usize;
            for (syllable_index, syllable) in syllables.iter().enumerate() {
                let mut prefix_nodes = Vec::with_capacity(syllable.len());
                for (letter_index, letter) in syllable.bytes().enumerate() {
                    node = self.ensure_letter(node, letter);
                    prefix_nodes.push(node);
                    if syllable_index + 1 == syllables.len() && letter_index + 1 < syllable.len() {
                        Self::prefer_terminal(&mut self.nodes[node].final_prefix, terminal);
                        Self::keep_top(&mut self.nodes[node].prefix_all, terminal);
                    }
                }
                if syllable_index + 1 < syllables.len() {
                    let boundary = self.ensure_boundary(node);
                    let boundary_slot = boundary as u32 + 1;
                    // Every non-empty prefix may explicitly end the current
                    // syllable. The pointer rejoins the canonical suffix trie, so
                    // abbreviations do not duplicate suffix nodes exponentially.
                    for prefix_node in prefix_nodes
                        .into_iter()
                        .take(syllable.len().saturating_sub(1))
                    {
                        let targets = &mut self.nodes[prefix_node].abbreviation_boundaries;
                        if !targets.contains(&boundary_slot) {
                            targets.push(boundary_slot);
                        }
                    }
                    node = boundary;
                }
            }
            Self::prefer_terminal(&mut self.nodes[node].exact, terminal);
            Self::keep_top(&mut self.nodes[node].exact_all, terminal);
        }
    }
}

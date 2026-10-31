// Never persist a candidate's raw-input echo as its pronunciation. Read the
// dictionary, then align complete syllables (or their typed prefixes) to keys.
fn learning_readings(text: &str) -> Vec<String> {
    let index = zhixu::current();
    let mut readings: Vec<_> = index.all_readings_for_text(text).map(str::to_string).collect();
    if let Some(known) = dictionary_readings(text) {
        readings.extend(known.iter().map(|(_, reading)| reading.clone()));
    }
    if let Some(index) = zhixu::learning_index() {
        readings.extend(index.all_readings_for_text(text).map(str::to_string));
    }
    readings.retain(|reading| crate::zhixu::learning::normalize_reading(reading).is_some()
        && reading.split_whitespace().count() == text.chars().count());
    readings.sort();
    readings.dedup();
    readings
}

fn learning_covers(raw: &str, reading: &str) -> bool {
    let raw = raw.replace([' ', '\''], "");
    raw == reading.replace(' ', "") || syllable_prefix_abbreviation(&raw, reading)
}

fn learning_partial(raw: &str, text: &str) -> bool {
    let raw = raw.replace([' ', '\''], "");
    let readings = learning_readings(text);
    !readings.iter().any(|reading| learning_covers(&raw, reading))
        && readings.iter().any(|reading| {
            let compact = reading.replace(' ', "");
            compact.len() < raw.len() && raw.starts_with(&compact)
        })
}

fn learning_reading_ends(raw: &str, start: usize, reading: &str) -> Vec<(usize, usize)> {
    let mut positions = vec![(start, 0)];
    for syllable in reading.split_whitespace() {
        let mut next = Vec::new();
        for (position, cost) in positions {
            for len in 1..=syllable.len().min(raw.len().saturating_sub(position)) {
                if syllable.starts_with(&raw[position..position + len]) { next.push((position + len, cost + syllable.len() - len)); }
            }
        }
        next.sort_unstable();
        next.dedup();
        positions = next;
    }
    positions
}

fn resolve_learning_reading(raw: &str, text: &str) -> Option<(String, usize)> {
    let raw = raw.replace([' ', '\''], "");
    if raw.is_empty() || raw.len() > 128 || !raw.bytes().all(|b| b.is_ascii_lowercase())
        || text.is_empty() || text.chars().count() > 64 { return None; }
    let boundaries: Vec<_> = text.char_indices().map(|(i, _)| i).chain(std::iter::once(text.len())).collect();
    let mut states: Vec<Vec<(usize, String, usize)>> = vec![Vec::new(); boundaries.len()];
    states[0].push((0, String::new(), 0));
    for start in 0..boundaries.len() - 1 {
        if states[start].is_empty() { continue; }
        for end in (start + 1..boundaries.len()).rev() {
            let piece = &text[boundaries[start]..boundaries[end]];
            let readings = learning_readings(piece);
            for (position, prefix, cost) in states[start].clone() {
                for reading in &readings {
                    for (next, added) in learning_reading_ends(&raw, position, reading) {
                        let full = if prefix.is_empty() { reading.clone() } else { format!("{prefix} {reading}") };
                        if states[end].len() >= 256 { return None; }
                        if !states[end].iter().any(|state| state == &(next, full.clone(), cost + added)) { states[end].push((next, full, cost + added)); }
                    }
                }
            }
        }
    }
    let last = states.last()?;
    let consumed = last.iter().map(|(n, _, _)| *n).max()?;
    let cost = last.iter().filter(|(n, _, _)| *n == consumed).map(|(_, _, cost)| *cost).min()?;
    let mut readings = last.iter().filter(|(n, _, c)| *n == consumed && *c == cost).map(|(_, reading, _)| reading);
    let reading = readings.next()?;
    // A polyphonic/ambiguous match is skipped rather than teaching a guessed pronunciation.
    if readings.any(|other| other != reading) { return None; }
    Some((reading.clone(), consumed))
}

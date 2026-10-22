use crate::replay_gate::{CompositionOutcome, RecordedQualityEvent};
use crate::storage::ConfidentProfileTarget;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

const MAXIMUM_TARGETS: usize = 20;
const MINIMUM_CONFIDENT_FRICTION: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProfileIntrospectionReport {
    source_kind: &'static str,
    read_only: bool,
    total_events: usize,
    friction_events: usize,
    raw_commit_events: usize,
    homophone_split_count: usize,
    deep_page_events: usize,
    abandoned_events: usize,
    redo_events: usize,
    latest_recorded_at_ms: Option<i64>,
    top_n_targets: Vec<TopNTarget>,
    whole_string_targets: Vec<WholeStringTarget>,
    homophone_splits: Vec<HomophoneSplit>,
    #[serde(skip)]
    pub(crate) confident_targets: Vec<ConfidentProfileTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TopNTarget {
    pinyin: String,
    selected_text: String,
    friction_count: usize,
    total_count: usize,
    deepest_rank: usize,
    latest_recorded_at_ms: i64,
    confident: bool,
    runner_up_friction_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct WholeStringTarget {
    pinyin: String,
    raw_commit_count: usize,
    latest_recorded_at_ms: i64,
    chinese_candidate_preview: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct HomophoneSplit {
    pinyin: String,
    friction_count: usize,
    latest_recorded_at_ms: i64,
    choices: Vec<HomophoneChoice>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct HomophoneChoice {
    text: String,
    friction_count: usize,
}

#[derive(Default)]
struct TopNAccumulator {
    friction_count: usize,
    total_count: usize,
    deepest_rank: usize,
    latest_recorded_at_ms: i64,
}

#[derive(Default)]
struct RawCommitAccumulator {
    count: usize,
    latest_recorded_at_ms: i64,
    chinese_candidate_preview: Vec<String>,
}

pub(crate) fn mine_profile_introspection(
    events: &[RecordedQualityEvent],
) -> ProfileIntrospectionReport {
    let mut pair_counts = HashMap::<(String, String), TopNAccumulator>::new();
    let mut raw_commits = HashMap::<String, RawCommitAccumulator>::new();
    let mut friction_by_reading = HashMap::<String, HashMap<String, usize>>::new();
    let mut friction_latest = HashMap::<String, i64>::new();
    let mut friction_events = 0usize;
    let mut raw_commit_events = 0usize;
    let deep_page_events = events.iter().filter(|event| event.max_page >= 3).count();
    let abandoned_events = events
        .iter()
        .filter(|event| event.outcome == CompositionOutcome::Abandoned)
        .count();
    let redo_events = events
        .iter()
        .filter(|event| event.redo_after_commit)
        .count();

    for event in events {
        if event.outcome == CompositionOutcome::Abandoned {
            continue;
        }
        let pair = (event.pinyin.clone(), event.selected_text.clone());
        let pair_count = pair_counts.entry(pair).or_default();
        pair_count.total_count += 1;
        pair_count.latest_recorded_at_ms =
            pair_count.latest_recorded_at_ms.max(event.recorded_at_ms);

        if event.selected_index > 0 {
            friction_events += 1;
            pair_count.friction_count += 1;
            pair_count.deepest_rank = pair_count.deepest_rank.max(event.selected_index + 1);
            *friction_by_reading
                .entry(event.pinyin.clone())
                .or_default()
                .entry(event.selected_text.clone())
                .or_default() += 1;
            friction_latest
                .entry(event.pinyin.clone())
                .and_modify(|latest| *latest = (*latest).max(event.recorded_at_ms))
                .or_insert(event.recorded_at_ms);
        }

        if event.selected_text == event.pinyin {
            raw_commit_events += 1;
            let raw = raw_commits.entry(event.pinyin.clone()).or_default();
            raw.count += 1;
            if event.recorded_at_ms >= raw.latest_recorded_at_ms {
                raw.latest_recorded_at_ms = event.recorded_at_ms;
                raw.chinese_candidate_preview = event
                    .candidates
                    .iter()
                    .filter(|candidate| candidate.chars().any(is_han))
                    .take(3)
                    .cloned()
                    .collect();
            }
        }
    }

    let mut top_n_targets = pair_counts
        .into_iter()
        .filter(|(_, count)| count.friction_count > 0)
        .map(|((pinyin, selected_text), count)| TopNTarget {
            pinyin,
            selected_text,
            friction_count: count.friction_count,
            total_count: count.total_count,
            deepest_rank: count.deepest_rank,
            latest_recorded_at_ms: count.latest_recorded_at_ms,
            confident: false,
            runner_up_friction_count: 0,
        })
        .collect::<Vec<_>>();
    let mut indexes_by_pinyin = BTreeMap::<String, Vec<usize>>::new();
    for (index, target) in top_n_targets.iter().enumerate() {
        indexes_by_pinyin
            .entry(target.pinyin.clone())
            .or_default()
            .push(index);
    }
    let mut confident_targets = Vec::new();
    for indexes in indexes_by_pinyin.values_mut() {
        indexes.sort_by(|left, right| {
            top_n_targets[*right]
                .friction_count
                .cmp(&top_n_targets[*left].friction_count)
                .then_with(|| {
                    top_n_targets[*left]
                        .selected_text
                        .cmp(&top_n_targets[*right].selected_text)
                })
        });
        let leader_index = indexes[0];
        let runner_up = indexes
            .get(1)
            .map(|index| top_n_targets[*index].friction_count)
            .unwrap_or(0);
        let leader = &mut top_n_targets[leader_index];
        leader.runner_up_friction_count = runner_up;
        leader.confident = leader.friction_count >= MINIMUM_CONFIDENT_FRICTION
            && leader.friction_count >= runner_up.saturating_mul(2);
        if leader.confident {
            confident_targets.push(ConfidentProfileTarget {
                pinyin: leader.pinyin.clone(),
                text: leader.selected_text.clone(),
                friction_count: leader.friction_count,
                runner_up_friction_count: runner_up,
                latest_friction_at_ms: leader.latest_recorded_at_ms,
            });
        }
        for index in indexes.iter().skip(1) {
            top_n_targets[*index].runner_up_friction_count =
                top_n_targets[leader_index].friction_count;
        }
    }
    confident_targets.sort_by(|left, right| {
        left.pinyin
            .cmp(&right.pinyin)
            .then_with(|| left.text.cmp(&right.text))
    });
    top_n_targets.sort_by(|left, right| {
        right
            .friction_count
            .cmp(&left.friction_count)
            .then_with(|| right.latest_recorded_at_ms.cmp(&left.latest_recorded_at_ms))
            .then_with(|| left.pinyin.cmp(&right.pinyin))
            .then_with(|| left.selected_text.cmp(&right.selected_text))
    });
    top_n_targets.truncate(MAXIMUM_TARGETS);

    let mut whole_string_targets = raw_commits
        .into_iter()
        .map(|(pinyin, count)| WholeStringTarget {
            pinyin,
            raw_commit_count: count.count,
            latest_recorded_at_ms: count.latest_recorded_at_ms,
            chinese_candidate_preview: count.chinese_candidate_preview,
        })
        .collect::<Vec<_>>();
    whole_string_targets.sort_by(|left, right| {
        right
            .raw_commit_count
            .cmp(&left.raw_commit_count)
            .then_with(|| right.latest_recorded_at_ms.cmp(&left.latest_recorded_at_ms))
            .then_with(|| left.pinyin.cmp(&right.pinyin))
    });
    whole_string_targets.truncate(MAXIMUM_TARGETS);

    let mut homophone_splits = friction_by_reading
        .into_iter()
        .filter(|(_, choices)| choices.len() > 1)
        .map(|(pinyin, choices)| {
            let mut choices = choices
                .into_iter()
                .map(|(text, friction_count)| HomophoneChoice {
                    text,
                    friction_count,
                })
                .collect::<Vec<_>>();
            choices.sort_by(|left, right| {
                right
                    .friction_count
                    .cmp(&left.friction_count)
                    .then_with(|| left.text.cmp(&right.text))
            });
            HomophoneSplit {
                friction_count: choices.iter().map(|choice| choice.friction_count).sum(),
                latest_recorded_at_ms: friction_latest.get(&pinyin).copied().unwrap_or_default(),
                pinyin,
                choices,
            }
        })
        .collect::<Vec<_>>();
    homophone_splits.sort_by(|left, right| {
        right
            .friction_count
            .cmp(&left.friction_count)
            .then_with(|| right.latest_recorded_at_ms.cmp(&left.latest_recorded_at_ms))
            .then_with(|| left.pinyin.cmp(&right.pinyin))
    });
    let homophone_split_count = homophone_splits.len();
    homophone_splits.truncate(MAXIMUM_TARGETS);

    ProfileIntrospectionReport {
        source_kind: "quality-events-jsonl-v1",
        read_only: true,
        total_events: events.len(),
        friction_events,
        raw_commit_events,
        homophone_split_count,
        deep_page_events,
        abandoned_events,
        redo_events,
        latest_recorded_at_ms: events.iter().map(|event| event.recorded_at_ms).max(),
        top_n_targets,
        whole_string_targets,
        homophone_splits,
        confident_targets,
    }
}

pub(crate) fn mine_profile_introspection_value(
    events: &[RecordedQualityEvent],
) -> Result<serde_json::Value, String> {
    serde_json::to_value(mine_profile_introspection(events))
        .map_err(|error| format!("无法编码档案自省报告：{error}"))
}

fn is_han(character: char) -> bool {
    matches!(character as u32, 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0x20000..=0x2fa1f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(
        id: &str,
        pinyin: &str,
        candidates: &[&str],
        selected_index: usize,
        recorded_at_ms: i64,
    ) -> RecordedQualityEvent {
        RecordedQualityEvent {
            id: id.into(),
            recorded_at_ms,
            pinyin: pinyin.into(),
            candidates: candidates.iter().map(|value| (*value).into()).collect(),
            candidate_comments: Vec::new(),
            selected_index,
            selected_text: candidates[selected_index].into(),
            selected_candidate_comment: None,
            app_hash: "0".repeat(64),
            preceding_text: String::new(),
            fuzzy_mask: 0,
            typo_correction_enabled: false,
            active_project: None,
            active_packs: Vec::new(),
            secondary_enabled: true,
            max_page: 0,
            outcome: CompositionOutcome::CommittedCandidate,
            duration_ms: 10,
            backspaces: 0,
            redo_after_commit: false,
            candidate_count: candidates.len(),
            context_model: None,
        }
    }

    #[test]
    fn context_model_metadata_does_not_change_introspection() {
        let legacy = event("1", "ba", &["吧", "把"], 1, 10);
        let mut object = serde_json::to_value(&legacy).unwrap();
        object["contextModel"] = serde_json::json!({
            "enabled": true, "outcome": "reordered", "baseOrder": [1, 0], "groupSize": 2
        });
        let with_model = serde_json::from_value(object).unwrap();
        assert_eq!(mine_profile_introspection_value(&[legacy]).unwrap(),
                   mine_profile_introspection_value(&[with_model]).unwrap());
    }

    #[test]
    fn mines_friction_raw_commits_targets_and_homophone_splits() {
        let events = vec![
            event("1", "ba", &["吧", "把"], 1, 10),
            event("2", "ba", &["吧", "把"], 1, 20),
            event("3", "ba", &["把", "吧"], 1, 30),
            event("4", "dans", &["但是", "但是呢"], 0, 40),
            event("5", "danswojide", &["danswojide", "但是我记得"], 0, 50),
        ];

        let report = mine_profile_introspection(&events);

        assert!(report.read_only);
        assert_eq!(report.total_events, 5);
        assert_eq!(report.friction_events, 3);
        assert_eq!(report.raw_commit_events, 1);
        assert_eq!(report.homophone_split_count, 1);
        assert_eq!(report.latest_recorded_at_ms, Some(50));
        assert_eq!(report.top_n_targets[0].pinyin, "ba");
        assert_eq!(report.top_n_targets[0].selected_text, "把");
        assert_eq!(report.top_n_targets[0].friction_count, 2);
        assert_eq!(report.top_n_targets[0].total_count, 2);
        assert_eq!(report.top_n_targets[0].deepest_rank, 2);
        assert!(report.top_n_targets[0].confident);
        assert_eq!(report.top_n_targets[0].runner_up_friction_count, 1);
        assert_eq!(report.confident_targets.len(), 1);
        assert_eq!(report.confident_targets[0].text, "把");
        assert_eq!(report.whole_string_targets[0].pinyin, "danswojide");
        assert_eq!(
            report.whole_string_targets[0].chinese_candidate_preview,
            ["但是我记得"]
        );
        assert_eq!(report.homophone_splits.len(), 1);
        assert_eq!(report.homophone_splits[0].pinyin, "ba");
        assert_eq!(
            report.homophone_splits[0]
                .choices
                .iter()
                .map(|choice| (choice.text.as_str(), choice.friction_count))
                .collect::<Vec<_>>(),
            [("把", 2), ("吧", 1)]
        );
    }

    #[test]
    fn reports_deep_pages_abandonment_and_immediate_redo_without_learning_abandonment() {
        let mut deep = event("deep", "ni", &["你"], 0, 10);
        deep.max_page = 3;
        deep.redo_after_commit = true;
        let mut abandoned = event("abandoned", "wo", &["我"], 0, 20);
        abandoned.outcome = CompositionOutcome::Abandoned;
        abandoned.selected_text.clear();
        let report = mine_profile_introspection(&[deep, abandoned]);
        assert_eq!(report.deep_page_events, 1);
        assert_eq!(report.abandoned_events, 1);
        assert_eq!(report.redo_events, 1);
        assert_eq!(report.total_events, 2);
        assert!(report.top_n_targets.is_empty());
    }

    #[test]
    fn ignores_reflex_top_one_counts_as_preference_signals() {
        let events = vec![
            event("1", "zai", &["在", "再"], 0, 10),
            event("2", "zai", &["在", "再"], 0, 20),
            event("3", "zai", &["在", "再"], 0, 30),
            event("4", "zai", &["在", "再"], 1, 40),
            event("5", "zai", &["在", "再"], 1, 50),
        ];

        let report = mine_profile_introspection(&events);

        assert_eq!(report.friction_events, 2);
        assert_eq!(report.top_n_targets.len(), 1);
        assert_eq!(report.top_n_targets[0].selected_text, "再");
        assert_eq!(report.top_n_targets[0].friction_count, 2);
        assert!(report.homophone_splits.is_empty());
        assert_eq!(report.confident_targets.len(), 1);
        assert_eq!(report.confident_targets[0].text, "再");
    }

    #[test]
    fn split_without_two_times_lead_stays_visible_but_is_not_applicable() {
        let events = vec![
            event("1", "ma", &["吗", "嘛"], 1, 10),
            event("2", "ma", &["吗", "嘛"], 1, 20),
            event("3", "ma", &["嘛", "吗"], 1, 30),
            event("4", "ma", &["嘛", "吗"], 1, 40),
        ];

        let report = mine_profile_introspection(&events);

        assert_eq!(report.top_n_targets.len(), 2);
        assert!(report.top_n_targets.iter().all(|target| !target.confident));
        assert!(report.confident_targets.is_empty());
    }

    #[test]
    fn one_off_friction_stays_visible_but_is_not_confident() {
        let report = mine_profile_introspection(&[event("1", "zai", &["在", "再"], 1, 10)]);
        assert_eq!(report.top_n_targets.len(), 1);
        assert_eq!(report.top_n_targets[0].friction_count, 1);
        assert!(!report.top_n_targets[0].confident);
        assert!(report.confident_targets.is_empty());
    }

    #[test]
    #[ignore = "reads the owner-authorized live Quality directory supplied by AIM_W7_QUALITY_ROOT"]
    fn live_owner_layer_zero_targets_are_identical_after_preceding_text_truncation() {
        let root = std::env::var("AIM_W7_QUALITY_ROOT")
            .expect("AIM_W7_QUALITY_ROOT must name the owner Quality directory");
        let mut events =
            crate::replay_gate::read_quality_events(std::path::Path::new(&root)).unwrap();
        let live_event_count = events.len();
        assert!(live_event_count >= 9_007);
        events.truncate(9_007);
        let before = mine_profile_introspection(&events);
        let mut truncated = events.clone();
        for event in &mut truncated {
            let mut suffix = Vec::new();
            let mut units = 0;
            for character in event.preceding_text.chars().rev() {
                let next = character.len_utf16();
                if units + next > 8 {
                    break;
                }
                suffix.push(character);
                units += next;
            }
            suffix.reverse();
            event.preceding_text = suffix.into_iter().collect();
        }
        assert!(truncated
            .iter()
            .all(|event| event.preceding_text.encode_utf16().count() <= 8));
        let after = mine_profile_introspection(&truncated);
        assert_eq!(before.confident_targets, after.confident_targets);
        assert_eq!(after.confident_targets.len(), 38);
        let ba = after
            .confident_targets
            .iter()
            .find(|target| target.pinyin == "ba" && target.text == "把")
            .expect("ba -> 把 must remain confident");
        assert_eq!((ba.friction_count, ba.runner_up_friction_count), (11, 4));
        println!(
            "W7-LAYER0 snapshot_events={} live_events={} max_preceding_utf16=8 confident_before={} confident_after={} ba=11:4",
            events.len(), live_event_count,
            before.confident_targets.len(),
            after.confident_targets.len()
        );
    }
}

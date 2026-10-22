use serde::Deserialize;
use std::collections::{HashMap, HashSet};

const MAXIMUM_BYTES: usize = 1024 * 1024;
const MAXIMUM_RULES: usize = 1_000;
const MAXIMUM_SENSES_PER_RULE: usize = 64;
const MAXIMUM_TRIGGERS_PER_SENSE: usize = 128;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ContextScore {
    pub(crate) matched_bytes: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ContextModel {
    triggers_by_reading: HashMap<String, HashMap<String, Vec<String>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DisambiguationTable {
    rules: Vec<DisambiguationRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DisambiguationRule {
    pinyin: String,
    senses: Vec<DisambiguationSense>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DisambiguationSense {
    word: String,
    triggers: Vec<String>,
}

impl ContextModel {
    pub fn from_json(bytes: &[u8]) -> Result<Self, i32> {
        if bytes.is_empty() || bytes.len() > MAXIMUM_BYTES {
            return Err(-211);
        }
        let table: DisambiguationTable = serde_json::from_slice(bytes).map_err(|_| -212)?;
        if table.rules.is_empty() || table.rules.len() > MAXIMUM_RULES {
            return Err(-212);
        }

        let mut triggers_by_reading = HashMap::new();
        for rule in table.rules {
            if rule.pinyin.is_empty()
                || !rule.pinyin.bytes().all(|byte| byte.is_ascii_lowercase())
                || rule.senses.is_empty()
                || rule.senses.len() > MAXIMUM_SENSES_PER_RULE
                || triggers_by_reading.contains_key(&rule.pinyin)
            {
                return Err(-212);
            }
            let mut senses = HashMap::new();
            for sense in rule.senses {
                if !valid_text(&sense.word)
                    || sense.triggers.is_empty()
                    || sense.triggers.len() > MAXIMUM_TRIGGERS_PER_SENSE
                    || senses.contains_key(&sense.word)
                {
                    return Err(-212);
                }
                let mut unique = HashSet::new();
                for trigger in &sense.triggers {
                    if !valid_text(trigger) || !unique.insert(trigger.as_str()) {
                        return Err(-212);
                    }
                }
                senses.insert(sense.word, sense.triggers);
            }
            triggers_by_reading.insert(rule.pinyin, senses);
        }
        Ok(Self {
            triggers_by_reading,
        })
    }

    pub(crate) fn score(
        &self,
        reading: &str,
        preceding_text: &str,
        candidate: &str,
    ) -> Option<ContextScore> {
        let triggers = self
            .triggers_by_reading
            .get(reading)
            .and_then(|senses| senses.get(candidate))?;
        triggers.iter().fold(None, |best, trigger| {
            let matched = preceding_text.ends_with(trigger).then_some(ContextScore {
                matched_bytes: trigger.len(),
            });
            best.max(matched)
        })
    }
}

fn valid_text(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = r#"{
      "rules": [
        {"pinyin":"ta","senses":[
          {"word":"他","triggers":["同事","朋友"]},
          {"word":"它","triggers":["设备","这个设备"]}
        ]}
      ]
    }"#;

    #[test]
    fn parses_table_and_prefers_the_longest_matching_suffix() {
        let model = ContextModel::from_json(TABLE.as_bytes()).unwrap();
        assert_eq!(
            model.score("ta", "请检查这个设备", "它"),
            Some(ContextScore {
                matched_bytes: "这个设备".len(),
            })
        );
        assert_eq!(model.score("ta", "设备需要重启", "它"), None);
        assert_eq!(model.score("ta", "设备", "他"), None);
        assert_eq!(model.score("other", "这个设备", "它"), None);
    }

    #[test]
    fn rejects_malformed_duplicate_or_noncanonical_tables() {
        assert_eq!(ContextModel::from_json(b"").unwrap_err(), -211);
        assert_eq!(ContextModel::from_json(b"{}").unwrap_err(), -212);
        assert_eq!(
            ContextModel::from_json(
                r#"{"rules":[{"pinyin":"Ta","senses":[{"word":"它","triggers":["设备"]}]}]}"#
                    .as_bytes()
            )
            .unwrap_err(),
            -212
        );
        assert_eq!(
            ContextModel::from_json(
                r#"{"rules":[{"pinyin":"ta","senses":[{"word":"它","triggers":["设备","设备"]}]}]}"#
                    .as_bytes()
            )
            .unwrap_err(),
            -212
        );
    }
}

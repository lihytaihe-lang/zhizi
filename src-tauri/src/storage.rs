#[cfg(not(test))]
use crate::keychain::system_provider_keychain;
use crate::keychain::{
    provider_keychain_reference, validate_provider_keychain_reference, ProviderKeychain,
};
use crate::skin_catalog::is_candidate_skin_id;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(test)]
use std::path::Path;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

static IMPORT_BATCH_COUNTER: AtomicU64 = AtomicU64::new(1);

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/001_initial.sql")),
    (2, include_str!("../migrations/002_personal_lexicon.sql")),
    (
        3,
        include_str!("../migrations/003_chunk2_lexicon_management.sql"),
    ),
    (4, include_str!("../migrations/004_project_profiles.sql")),
    (
        5,
        include_str!("../migrations/005_dynamic_hotword_sources.sql"),
    ),
    (6, include_str!("../migrations/006_prediction_runtime.sql")),
    (7, include_str!("../migrations/007_local_knowledge.sql")),
    (8, include_str!("../migrations/008_daily_maintenance.sql")),
    (
        9,
        include_str!("../migrations/009_personal_profile_auto_update.sql"),
    ),
    (
        10,
        include_str!("../migrations/010_owner_declared_disambiguation.sql"),
    ),
    (
        11,
        include_str!("../migrations/011_hotword_review_and_candidate_theme.sql"),
    ),
    (
        12,
        include_str!("../migrations/012_knowledge_retrieval.sql"),
    ),
    (
        13,
        include_str!("../migrations/013_candidate_skin_catalog.sql"),
    ),
    (
        14,
        include_str!("../migrations/014_context_model_ranking.sql"),
    ),
];
const INITIALIZED_KEY: &str = "storageInitialized";
const SELECTED_MODE_KEY: &str = "selectedModeId";
const PROVIDER_SETTINGS_KEY: &str = "providerSettings";
const SHORTCUT_SETTINGS_KEY: &str = "shortcutSettings";
#[cfg(any(feature = "personal-seed", test))]
const PERSONAL_PROFILE_V1_SEEDED_KEY: &str = "personalProfileV1Seeded";
#[cfg(feature = "personal-seed")]
const PERSONAL_PROFILE_V1: &str = include_str!("../../quality/mined/personal-profile-v1.json");
#[cfg(feature = "personal-seed")]
const PERSONAL_PROFILE_V1_SEED: &str =
    include_str!("../../quality/mined/personal-profile-v1-seed.json");
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_millis(250);
pub(crate) const PERSONAL_PROFILE_AUTO_WEIGHT: i64 = 520;
pub(crate) const PERSONAL_PROFILE_DECAY_AFTER_MS: i64 = 30 * 24 * 60 * 60 * 1000;

#[cfg(feature = "personal-seed")]
#[derive(Debug, Deserialize)]
struct PersonalProfileDocument {
    friction_boost: Vec<FrictionBoost>,
    disambiguation_by_friction: Vec<FrictionDisambiguation>,
    owner_declared_disambiguation: Vec<OwnerDeclaredDisambiguation>,
}

#[cfg(feature = "personal-seed")]
#[derive(Debug, Deserialize)]
struct FrictionBoost {
    pinyin: String,
    text: String,
    friction: u64,
}

#[cfg(feature = "personal-seed")]
#[derive(Debug, Deserialize)]
struct FrictionDisambiguation {
    pinyin: String,
    prefer: String,
    friction: HashMap<String, u64>,
    confident: bool,
}

#[cfg(feature = "personal-seed")]
#[derive(Debug, Deserialize)]
struct OwnerDeclaredDisambiguation {
    pinyin: String,
    candidates: Vec<String>,
    confident: bool,
    source: String,
}

#[cfg(feature = "personal-seed")]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersonalProfileSeed {
    kind: String,
    format_version: u32,
    source: String,
    source_sha256: String,
    baseline_artifact_checksum: String,
    baseline_top1: String,
    authoring_method: String,
    still_red_readings: Vec<String>,
}

pub struct AppStorage {
    connection: Mutex<Connection>,
    provider_keychain: Arc<dyn ProviderKeychain>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfidentProfileTarget {
    pub pinyin: String,
    pub text: String,
    pub friction_count: usize,
    pub runner_up_friction_count: usize,
    pub latest_friction_at_ms: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersonalProfileAutoUpdateItem {
    pub id: String,
    pub entry_id: String,
    pub pinyin: String,
    pub text: String,
    pub friction_count: usize,
    pub runner_up_friction_count: usize,
    pub latest_friction_at_ms: i64,
    pub applied_at_ms: i64,
    pub last_refreshed_at_ms: i64,
    pub status: String,
    pub status_changed_at_ms: i64,
    pub prediction_eligible: bool,
    pub decay_after_ms: i64,
    pub evidence_kind: String,
    pub owner_declared_candidates: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OwnerDeclaredEvidence {
    pinyin: String,
    candidates: Vec<String>,
    prefer: String,
    confident: bool,
    source: String,
}

#[derive(Debug, Clone)]
enum ProfileUpdateEvidence {
    Friction,
    OwnerDeclared(OwnerDeclaredEvidence),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersonalProfileAutoUpdateReport {
    pub applied_count: usize,
    pub refreshed_count: usize,
    pub decayed_count: usize,
    pub blacklisted_count: usize,
    pub skipped_revoked_count: usize,
    pub items: Vec<PersonalProfileAutoUpdateItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_base64_fixture(encoded: &str) -> Vec<u8> {
        fn sextet(byte: u8) -> u8 {
            match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => panic!("invalid base64 fixture byte: {byte}"),
            }
        }

        let bytes = encoded
            .bytes()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect::<Vec<_>>();
        assert_eq!(bytes.len() % 4, 0, "base64 fixture quartet mismatch");

        let mut decoded = Vec::with_capacity(bytes.len() / 4 * 3);
        for quartet in bytes.chunks_exact(4) {
            let first = sextet(quartet[0]);
            let second = sextet(quartet[1]);
            decoded.push((first << 2) | (second >> 4));

            if quartet[2] != b'=' {
                let third = sextet(quartet[2]);
                decoded.push((second << 4) | (third >> 2));

                if quartet[3] != b'=' {
                    let fourth = sextet(quartet[3]);
                    decoded.push((third << 6) | fourth);
                }
            } else {
                assert_eq!(quartet[3], b'=', "invalid base64 fixture padding");
            }
        }
        decoded
    }

    fn test_database_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!(
                "agent-input-layer-storage-tests-{}",
                std::process::id()
            ))
            .join(format!(
                "{name}-{}.sqlite3",
                std::thread::current().name().unwrap_or("test")
            ))
    }

    fn remove_database_files(path: &Path) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path.with_extension("sqlite3-shm"));
        let _ = fs::remove_file(path.with_extension("sqlite3-wal"));
    }

    fn fallback_state() -> AppState {
        AppState {
            history: vec![HistoryItem {
                id: "history-1".into(),
                raw_input: "做一个登录页".into(),
                mode_id: "codex-execution".into(),
                target_tool: None,
                output: "任务指令".into(),
                favorite: true,
                created_at: "2026-05-13T00:00:00.000Z".into(),
            }],
            user_rules: vec![UserRule {
                id: "rule-1".into(),
                text: "先做 MVP".into(),
                enabled: true,
            }],
            selected_mode_id: "codex-execution".into(),
            provider_settings: ProviderSettings {
                enabled: false,
                base_url: String::new(),
                model: String::new(),
                api_key: String::new(),
                provider_kind: default_provider_kind(),
                anthropic_base_url: default_anthropic_base_url(),
                anthropic_model: String::new(),
                anthropic_api_key: String::new(),
                ollama_base_url: default_ollama_base_url(),
                ollama_model: String::new(),
            },
            shortcut_settings: ShortcutSettings {
                shortcut: "alt+space".into(),
                mcp_enabled: false,
            },
        }
    }

    fn lexicon_input(id: &str, text: &str, pinyin: &str) -> LexiconEntryInput {
        LexiconEntryInput {
            id: id.into(),
            text: text.into(),
            pinyin: pinyin.into(),
            aliases: vec![format!("{text}-alias")],
            professional_packs: vec!["software-engineering".into()],
            project_domains: vec!["agent-input-method".into()],
            sources: vec![LexiconSource {
                source_kind: "manual".into(),
                source: "owner-entry".into(),
                license: "private-user-data".into(),
                confidence_millis: 1000,
            }],
            base_weight: 120,
            fixed: true,
            disabled: false,
            demoted: false,
            updated_at_ms: 1_752_624_000_000,
        }
    }

    #[test]
    fn context_model_toggle_is_default_off_persistent_and_legacy_safe() {
        let path = test_database_path("context-model-toggle");
        remove_database_files(&path);
        let storage = AppStorage::open(path.clone()).unwrap();
        assert!(
            !storage
                .project_runtime_configuration()
                .unwrap()
                .context_model_ranking_enabled
        );
        assert!(
            storage
                .set_context_model_ranking_enabled(true)
                .unwrap()
                .context_model_ranking_enabled
        );
        drop(storage);
        let storage = AppStorage::open(path.clone()).unwrap();
        let mut legacy =
            serde_json::to_value(storage.project_runtime_configuration().unwrap()).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("contextModelRankingEnabled");
        assert!(
            !serde_json::from_value::<ProjectRuntimeConfiguration>(legacy)
                .unwrap()
                .context_model_ranking_enabled
        );
        assert!(
            storage
                .project_runtime_configuration()
                .unwrap()
                .context_model_ranking_enabled
        );
        assert!(
            !storage
                .set_context_model_ranking_enabled(false)
                .unwrap()
                .context_model_ranking_enabled
        );
        drop(storage);
        remove_database_files(&path);
    }

    #[test]
    fn seeds_once_then_loads_saved_state() {
        let path = test_database_path("seed-once");
        remove_database_files(&path);

        let storage = AppStorage::open(path.clone()).expect("open test database");
        #[cfg(unix)]
        {
            assert_eq!(
                fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let fallback = fallback_state();
        let seeded = storage
            .load_app_state(fallback.clone())
            .expect("seed fallback state");

        assert_eq!(seeded.history.len(), 1);
        assert_eq!(seeded.user_rules.len(), 1);
        assert_eq!(seeded.selected_mode_id, "codex-execution");

        let saved = AppState {
            history: Vec::new(),
            user_rules: Vec::new(),
            selected_mode_id: "bug-fix".into(),
            provider_settings: ProviderSettings {
                enabled: true,
                base_url: "https://example.com/v1".into(),
                model: "test-model".into(),
                api_key: "local-test-key".into(),
                provider_kind: default_provider_kind(),
                anthropic_base_url: default_anthropic_base_url(),
                anthropic_model: String::new(),
                anthropic_api_key: String::new(),
                ollama_base_url: default_ollama_base_url(),
                ollama_model: String::new(),
            },
            shortcut_settings: ShortcutSettings {
                shortcut: "control+space".into(),
                mcp_enabled: false,
            },
        };

        storage
            .save_app_state(&saved)
            .expect("save explicit empty state");
        let reloaded = storage
            .load_app_state(fallback)
            .expect("load saved state after initialization");

        assert!(reloaded.history.is_empty());
        assert!(reloaded.user_rules.is_empty());
        assert_eq!(reloaded.selected_mode_id, "bug-fix");
        assert_eq!(reloaded.provider_settings.model, "test-model");
        assert_eq!(reloaded.shortcut_settings.shortcut, "control+space");

        drop(storage);
        remove_database_files(&path);
    }

    #[cfg(not(feature = "personal-seed"))]
    #[test]
    fn public_build_does_not_seed_or_mark_profile_complete() {
        let root = test_database_path("public-no-seed").with_extension("dir");
        let path = root.join("agent-input-layer.sqlite3");
        let storage = AppStorage::open(path.clone()).unwrap();
        assert!(storage.list_lexicon_entries().unwrap().is_empty());
        assert_eq!(
            get_setting::<bool>(
                &storage.lock_connection().unwrap(),
                PERSONAL_PROFILE_V1_SEEDED_KEY,
            )
            .unwrap(),
            None
        );
        drop(storage);
        remove_database_files(&path);
        fs::remove_dir(root).unwrap();
    }

    #[cfg(feature = "personal-seed")]
    #[test]
    fn personal_profile_seed_uses_only_red_confident_friction_and_runs_once() {
        let path = test_database_path("personal-profile-v1");
        remove_database_files(&path);
        let storage = AppStorage::open(path.clone()).unwrap();

        assert_eq!(
            storage.seed_personal_profile_v1(1_752_624_000_000).unwrap(),
            46
        );
        assert_eq!(
            storage.seed_personal_profile_v1(1_752_624_000_001).unwrap(),
            0
        );
        let entries = storage.list_lexicon_entries().unwrap();
        assert_eq!(entries.len(), 46);
        assert!(entries.iter().all(|entry| {
            entry.base_weight == 520
                && !entry.fixed
                && !entry.disabled
                && entry.sources.len() == 1
                && entry.sources[0].source_kind == "personal-friction"
                && entry.sources[0].source.ends_with("#friction")
        }));
        let pairs = entries
            .iter()
            .map(|entry| (entry.pinyin.as_str(), entry.text.as_str()))
            .collect::<HashSet<_>>();
        assert!(pairs.contains(&("ba", "把")));
        assert!(pairs.contains(&("zai", "再")));
        assert!(pairs.contains(&("ta", "它")));
        assert!(pairs.contains(&("shi", "事")));
        assert!(!entries.iter().any(|entry| {
            matches!(
                entry.pinyin.as_str(),
                "ma" | "you" | "da" | "na" | "gao" | "yici"
            )
        }));
        assert!(!entries.iter().any(|entry| entry.pinyin == "zhge"));
        assert!(!entries
            .iter()
            .any(|entry| matches!(entry.pinyin.as_str(), "zhidao" | "zenmkn")));

        remove_database_files(&path);
    }

    #[test]
    fn profile_auto_update_is_visible_decayable_reversible_and_vetoed() {
        let path = test_database_path("profile-auto-update");
        remove_database_files(&path);
        let storage = AppStorage::open(path.clone()).unwrap();
        let now = 1_800_000_000_000_i64;
        let knowing = ConfidentProfileTarget {
            pinyin: "wozenmzhidao".into(),
            text: "我怎么知道".into(),
            friction_count: 4,
            runner_up_friction_count: 2,
            latest_friction_at_ms: now,
        };

        let applied = storage
            .apply_personal_profile_auto_update(&[knowing.clone()], &HashSet::new(), now)
            .unwrap();
        assert_eq!(applied.applied_count, 1);
        let entry = storage.list_lexicon_entries().unwrap().pop().unwrap();
        assert_eq!(
            (entry.pinyin.as_str(), entry.text.as_str()),
            ("wozenmzhidao", "我怎么知道")
        );
        assert_eq!(entry.base_weight, PERSONAL_PROFILE_AUTO_WEIGHT);
        assert!(entry.sources.iter().any(|source| {
            source.source_kind == "personal-friction-auto"
                && source.source.contains("friction=4;runnerUp=2")
        }));
        assert_eq!(applied.items[0].status, "active");
        assert!(applied.items[0].prediction_eligible);

        let refreshed = storage
            .apply_personal_profile_auto_update(&[knowing.clone()], &HashSet::new(), now + 1)
            .unwrap();
        assert_eq!(refreshed.refreshed_count, 1);
        let id = refreshed.items[0].id.clone();
        storage
            .revoke_personal_profile_auto_update(&id, now + 2)
            .unwrap();
        assert!(storage.list_lexicon_entries().unwrap().is_empty());
        let rejected = storage
            .apply_personal_profile_auto_update(&[knowing], &HashSet::new(), now + 3)
            .unwrap();
        assert_eq!(rejected.skipped_revoked_count, 1);
        assert!(storage.list_lexicon_entries().unwrap().is_empty());

        let decay_target = ConfidentProfileTarget {
            pinyin: "xiangmu".into(),
            text: "项目".into(),
            friction_count: 2,
            runner_up_friction_count: 1,
            latest_friction_at_ms: now + 10,
        };
        storage
            .apply_personal_profile_auto_update(&[decay_target.clone()], &HashSet::new(), now + 10)
            .unwrap();
        let decayed = storage
            .apply_personal_profile_auto_update(
                &[decay_target],
                &HashSet::new(),
                now + 10 + PERSONAL_PROFILE_DECAY_AFTER_MS,
            )
            .unwrap();
        assert_eq!(decayed.decayed_count, 1);
        assert!(decayed.items.iter().any(|item| item.status == "decayed"));
        assert!(storage.list_lexicon_entries().unwrap().is_empty());

        let blocked_target = ConfidentProfileTarget {
            pinyin: "zhge".into(),
            text: "栅格".into(),
            friction_count: 8,
            runner_up_friction_count: 1,
            latest_friction_at_ms: now + 20,
        };
        storage
            .apply_personal_profile_auto_update(
                &[blocked_target.clone()],
                &HashSet::new(),
                now + 20,
            )
            .unwrap();
        let blacklist = HashSet::from([("zhge".to_string(), "栅格".to_string())]);
        let blocked = storage
            .apply_personal_profile_auto_update(&[blocked_target], &blacklist, now + 21)
            .unwrap();
        assert_eq!(blocked.blacklisted_count, 1);
        assert!(blocked
            .items
            .iter()
            .any(|item| item.status == "blacklisted"));
        assert!(storage.list_lexicon_entries().unwrap().is_empty());

        drop(storage);
        remove_database_files(&path);
    }

    #[test]
    fn owner_declared_disambiguation_uses_auto_update_audit_and_revocation_veto() {
        let path = test_database_path("owner-declared-disambiguation");
        remove_database_files(&path);
        let storage = AppStorage::open(path.clone()).unwrap();
        let now = 1_800_000_100_000_i64;
        let candidates = vec!["知道".to_string(), "指导".to_string()];

        let declared = storage
            .declare_owner_disambiguation("zhidao", "知道", &candidates, &HashSet::new(), now)
            .unwrap();
        assert_eq!(declared.applied_count, 1);
        let item = declared
            .items
            .iter()
            .find(|item| item.pinyin == "zhidao")
            .unwrap();
        assert_eq!(item.friction_count, 0, "owner 明示不得伪造后翻次数");
        assert_eq!(item.runner_up_friction_count, 0);
        assert_eq!(item.evidence_kind, "owner_declared_disambiguation");
        assert_eq!(
            item.owner_declared_candidates,
            vec!["指导".to_string(), "知道".to_string()]
        );
        let entry = storage
            .list_lexicon_entries()
            .unwrap()
            .into_iter()
            .find(|entry| entry.pinyin == "zhidao" && entry.text == "知道")
            .unwrap();
        assert!(entry.sources.iter().any(|source| {
            source.source_kind == "owner-declared-disambiguation"
                && source.source == "owner_declared_disambiguation#manager-panel"
        }));

        let id = item.id.clone();
        storage
            .revoke_personal_profile_auto_update(&id, now + 1)
            .unwrap();
        assert!(!storage
            .list_lexicon_entries()
            .unwrap()
            .iter()
            .any(|entry| entry.pinyin == "zhidao" && entry.text == "知道"));

        let later_friction = ConfidentProfileTarget {
            pinyin: "zhidao".into(),
            text: "知道".into(),
            friction_count: 4,
            runner_up_friction_count: 1,
            latest_friction_at_ms: now + 2,
        };
        let rerun = storage
            .apply_personal_profile_auto_update(&[later_friction], &HashSet::new(), now + 2)
            .unwrap();
        assert_eq!(rerun.skipped_revoked_count, 1);
        assert!(!storage
            .list_lexicon_entries()
            .unwrap()
            .iter()
            .any(|entry| entry.pinyin == "zhidao" && entry.text == "知道"));

        drop(storage);
        remove_database_files(&path);
    }

    #[test]
    fn history_is_bounded_and_explicit_empty_values_survive_restart() {
        let path = test_database_path("bounded-history");
        remove_database_files(&path);
        let provider_keychain: Arc<dyn ProviderKeychain> =
            Arc::new(crate::keychain::MemoryProviderKeychain::default());
        let storage =
            AppStorage::open_with_provider_keychain(path.clone(), provider_keychain.clone())
                .expect("open test database");
        let mut state = fallback_state();
        state.history = (0..55)
            .map(|index| HistoryItem {
                id: format!("history-{index}"),
                raw_input: format!("raw-{index}"),
                mode_id: "codex-execution".into(),
                target_tool: Some("Codex".into()),
                output: format!("output-{index}"),
                favorite: index == 0,
                created_at: format!("2026-07-12T00:00:{index:02}.000Z"),
            })
            .collect();
        state.provider_settings.api_key = "test-local-key".into();

        storage
            .save_app_state(&state)
            .expect("save bounded history");
        drop(storage);

        let direct = Connection::open(&path).expect("inspect provider settings directly");
        let provider_row: String = direct
            .query_row(
                "SELECT value FROM settings WHERE key='providerSettings'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        println!("SQLITE-REVERSE saved-providerSettings={provider_row}");
        assert!(!provider_row.contains("test-local-key"));
        assert!(!provider_row.contains("\"apiKey\""));
        drop(direct);

        let reopened =
            AppStorage::open_with_provider_keychain(path.clone(), provider_keychain.clone())
                .expect("reopen test database");
        let loaded = reopened
            .load_app_state(fallback_state())
            .expect("load persisted state");
        assert_eq!(loaded.history.len(), 50);
        assert_eq!(loaded.history[0].id, "history-0");
        assert!(loaded.history[0].favorite);
        assert_eq!(loaded.provider_settings.api_key, "test-local-key");

        drop(reopened);
        remove_database_files(&path);
    }

    #[test]
    fn legacy_provider_migration_and_keychain_failures_are_readable_and_redacted() {
        let path = test_database_path("provider-keychain-failure");
        remove_database_files(&path);
        let bootstrap_keychain: Arc<dyn ProviderKeychain> =
            Arc::new(crate::keychain::MemoryProviderKeychain::default());
        let bootstrap =
            AppStorage::open_with_provider_keychain(path.clone(), bootstrap_keychain).unwrap();
        bootstrap.load_app_state(fallback_state()).unwrap();
        drop(bootstrap);

        let legacy_secret = "synthetic-legacy-secret-never-log";
        let mut legacy = fallback_state().provider_settings;
        legacy.enabled = true;
        legacy.base_url = "https://provider.invalid/v1".into();
        legacy.model = "legacy-model".into();
        legacy.api_key = legacy_secret.into();
        let legacy_json = serde_json::to_string(&legacy).unwrap();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE settings SET value=?1 WHERE key='providerSettings'",
                params![legacy_json],
            )
            .unwrap();
        drop(connection);

        let unavailable: Arc<dyn ProviderKeychain> = Arc::new(
            crate::keychain::MemoryProviderKeychain::unavailable(legacy_secret),
        );
        let unavailable_storage =
            AppStorage::open_with_provider_keychain(path.clone(), unavailable).unwrap();
        let error = unavailable_storage
            .load_app_state(fallback_state())
            .expect_err("unavailable keychain must block legacy plaintext migration");
        assert!(error.contains("无法把 openai-compatible Provider 密钥迁移到 macOS 钥匙串"));
        assert!(!error.contains(legacy_secret));
        drop(unavailable_storage);

        let direct = Connection::open(&path).unwrap();
        let still_legacy: String = direct
            .query_row(
                "SELECT value FROM settings WHERE key='providerSettings'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(still_legacy.contains(legacy_secret));
        drop(direct);

        let migrated_keychain: Arc<dyn ProviderKeychain> =
            Arc::new(crate::keychain::MemoryProviderKeychain::default());
        let migrated =
            AppStorage::open_with_provider_keychain(path.clone(), migrated_keychain.clone())
                .expect("available keychain migrates legacy secret");
        let loaded = migrated.load_app_state(fallback_state()).unwrap();
        assert_eq!(loaded.provider_settings.api_key, legacy_secret);
        drop(migrated);

        let missing_keychain: Arc<dyn ProviderKeychain> =
            Arc::new(crate::keychain::MemoryProviderKeychain::default());
        let reopened =
            AppStorage::open_with_provider_keychain(path.clone(), missing_keychain).unwrap();
        let missing_error = reopened.load_app_state(fallback_state()).unwrap_err();
        assert_eq!(missing_error, "钥匙串中未找到 Provider 密钥，请重新填写。");
        assert!(!missing_error.contains(legacy_secret));
        drop(reopened);

        let inaccessible: Arc<dyn ProviderKeychain> = Arc::new(
            crate::keychain::MemoryProviderKeychain::unavailable(legacy_secret),
        );
        let reopened = AppStorage::open_with_provider_keychain(path.clone(), inaccessible).unwrap();
        let inaccessible_error = reopened.load_app_state(fallback_state()).unwrap_err();
        assert_eq!(
            inaccessible_error,
            "无法读取 macOS 钥匙串中的 Provider 密钥，请解锁钥匙串后重试。"
        );
        assert!(!inaccessible_error.contains(legacy_secret));
        drop(reopened);

        remove_database_files(&path);
    }

    #[test]
    fn v11_ownership_migration_preserves_enabled_source_links() {
        let path = test_database_path("v11-source-link-upgrade");
        remove_database_files(&path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE schema_migrations (
                   version INTEGER PRIMARY KEY,
                   applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                 );",
            )
            .unwrap();
        for (version, sql) in MIGRATIONS.iter().filter(|(version, _)| *version <= 10) {
            let transaction = connection.transaction().unwrap();
            transaction.execute_batch(sql).unwrap();
            transaction
                .execute(
                    "INSERT INTO schema_migrations(version) VALUES (?1)",
                    params![version],
                )
                .unwrap();
            transaction.commit().unwrap();
        }
        connection
            .execute(
                "INSERT INTO dynamic_sources
                 (id,display_name,source_type,refresh_interval_minutes,authorization,terms_url,
                  license,cache_policy,ownership,distribution,disable_policy,endpoint,enabled,
                  opt_in,created_at_ms,updated_at_ms)
                 VALUES ('kept','Kept','local_text',60,'self','self://terms','self-authored',
                         'local','user_local','local','pause','',1,1,1,1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO hotword_items
                 (id,text,pinyin,confidence_millis,freshness_millis,domain,review_required,state,
                  created_at_ms,updated_at_ms)
                 VALUES ('kept-item','保留词','bao liu ci',600,1000,'local',0,'staged',1,1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO hotword_item_sources
                 (item_id,source_id,source_record_id,fetched_at_ms)
                 VALUES ('kept-item','kept','line-1',1)",
                [],
            )
            .unwrap();
        drop(connection);

        let storage = AppStorage::open(path.clone()).unwrap();
        assert_eq!(storage.list_dynamic_sources().unwrap().len(), 1);
        assert_eq!(
            storage.list_hotword_items().unwrap()[0].source_ids,
            ["kept"]
        );
        let connection = storage.lock_connection().unwrap();
        let foreign_key_errors: i64 = connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(foreign_key_errors, 0);
        drop(connection);
        drop(storage);
        remove_database_files(&path);
    }

    #[test]
    fn v12_knowledge_fts_migration_backfills_existing_chunks_and_policies() {
        let path = test_database_path("v12-knowledge-fts-backfill");
        remove_database_files(&path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE schema_migrations (
                   version INTEGER PRIMARY KEY,
                   applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                 );",
            )
            .unwrap();
        for (version, sql) in MIGRATIONS.iter().filter(|(version, _)| *version <= 11) {
            let transaction = connection.transaction().unwrap();
            transaction.execute_batch(sql).unwrap();
            transaction
                .execute(
                    "INSERT INTO schema_migrations(version) VALUES (?1)",
                    params![version],
                )
                .unwrap();
            transaction.commit().unwrap();
        }
        connection
            .execute(
                "INSERT INTO knowledge_sources
                 (id,name,source_type,authorized_scope,authorization,status,last_error,
                  created_at_ms,updated_at_ms)
                 VALUES ('legacy-source','Legacy source','local_path','/tmp/legacy',
                         'legacy owner authorization','ready',NULL,1,1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO knowledge_documents
                 (id,source_id,relative_path,format,content_hash,byte_count,updated_at_ms)
                 VALUES ('legacy-document','legacy-source','legacy.md','md','hash',32,1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO knowledge_chunks
                 (id,document_id,ordinal,content_hash,content)
                 VALUES ('legacy-chunk','legacy-document',0,'chunk-hash',
                         'LegacyNeedle | legacy needle')",
                [],
            )
            .unwrap();
        drop(connection);

        let storage = AppStorage::open(path.clone()).unwrap();
        let source = storage.list_knowledge_sources().unwrap().remove(0);
        assert_eq!(source.project_id, "legacy-source");
        assert_eq!(source.retrieval_status, "approved");
        assert!(source.sensitive_enabled);
        let hits = storage
            .search_knowledge("LegacyNeedle", "legacy-source", 5)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].chunk_id, "legacy-chunk");
        let connection = storage.lock_connection().unwrap();
        assert_eq!(
            connection
                .query_row::<i64, _, _>("SELECT COUNT(*) FROM knowledge_chunks_fts", [], |row| {
                    row.get(0)
                })
                .unwrap(),
            1
        );
        drop(connection);
        drop(storage);
        remove_database_files(&path);
    }

    #[test]
    fn v13_candidate_skin_migration_preserves_midnight_fixture_and_removes_sql_check() {
        let path = test_database_path("v13-candidate-skin-midnight");
        remove_database_files(&path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE schema_migrations (
                   version INTEGER PRIMARY KEY,
                   applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                 );",
            )
            .unwrap();
        for (version, sql) in MIGRATIONS.iter().filter(|(version, _)| *version <= 12) {
            let transaction = connection.transaction().unwrap();
            transaction.execute_batch(sql).unwrap();
            transaction
                .execute(
                    "INSERT INTO schema_migrations(version) VALUES (?1)",
                    params![version],
                )
                .unwrap();
            transaction.commit().unwrap();
        }
        connection
            .execute(
                "UPDATE project_runtime_settings
                 SET secondary_layer_enabled=0,prediction_enabled=1,candidate_theme='midnight'
                 WHERE singleton=1",
                [],
            )
            .unwrap();
        drop(connection);

        let storage = AppStorage::open(path.clone()).unwrap();
        let runtime = storage.project_runtime_configuration().unwrap();
        assert_eq!(runtime.candidate_theme, "midnight");
        assert!(!runtime.secondary_layer_enabled);
        assert!(runtime.prediction_enabled);
        assert!(storage.set_candidate_theme("not-installed").is_err());
        drop(storage);

        let connection = Connection::open(&path).unwrap();
        let table_sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type='table' AND name='project_runtime_settings'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!table_sql.contains("candidate_theme IN"));
        assert_eq!(
            connection
                .query_row::<String, _, _>(
                    "SELECT candidate_theme FROM project_runtime_settings WHERE singleton=1",
                    [],
                    |row| row.get(0),
                )
                .unwrap(),
            "midnight"
        );
        assert_eq!(
            connection
                .query_row::<i64, _, _>(
                    "SELECT COUNT(*) FROM project_runtime_settings WHERE singleton=1",
                    [],
                    |row| row.get(0),
                )
                .unwrap(),
            1
        );
        drop(connection);
        remove_database_files(&path);
    }

    #[test]
    fn opens_real_v010_database_fixture_and_preserves_all_state() {
        const V010_DATABASE: &str =
            include_str!("../fixtures/v0.1.0/agent-input-layer.sqlite3.b64");
        const V010_DATABASE_BYTES: usize = 45_056;

        let path = test_database_path("v010-real-upgrade");
        remove_database_files(&path);
        fs::create_dir_all(path.parent().expect("upgrade fixture parent"))
            .expect("create upgrade fixture parent");

        let decoded = decode_base64_fixture(V010_DATABASE);
        assert_eq!(decoded.len(), V010_DATABASE_BYTES);
        assert_eq!(&decoded[..16], b"SQLite format 3\0");
        fs::write(&path, decoded).expect("materialize v0.1.0 SQLite fixture");

        let before = Connection::open(&path).expect("inspect v0.1.0 database before migration");
        let history_count_before: i64 = before
            .query_row("SELECT COUNT(*) FROM history", [], |row| row.get(0))
            .expect("count v0.1.0 history before migration");
        assert_eq!(history_count_before, 2);
        drop(before);

        let provider_keychain: Arc<dyn ProviderKeychain> =
            Arc::new(crate::keychain::MemoryProviderKeychain::default());
        let storage =
            AppStorage::open_with_provider_keychain(path.clone(), provider_keychain.clone())
                .expect("open v0.1.0 database in v0.4.0");
        let loaded = storage
            .load_app_state(fallback_state())
            .expect("load v0.1.0 state in v0.4.0");

        assert_eq!(loaded.history.len(), 2);
        assert_eq!(loaded.history[0].id, "v010-history-favorite");
        assert_eq!(loaded.history[0].target_tool.as_deref(), Some("Codex"));
        assert!(loaded.history[0].favorite);
        assert_eq!(loaded.history[1].mode_id, "bug-fix");
        assert_eq!(loaded.user_rules.len(), 2);
        assert!(loaded.user_rules[0].enabled);
        assert!(!loaded.user_rules[1].enabled);
        assert_eq!(loaded.selected_mode_id, "bug-fix");
        assert!(loaded.provider_settings.enabled);
        assert_eq!(
            loaded.provider_settings.base_url,
            "https://provider.invalid/v1"
        );
        assert_eq!(loaded.provider_settings.model, "synthetic-v010-model");
        assert_eq!(
            loaded.provider_settings.api_key,
            "synthetic-v010-key-never-contacted"
        );
        assert_eq!(loaded.shortcut_settings.shortcut, "control+space");

        let mut updated = loaded;
        updated.selected_mode_id = "product-planning".into();
        updated.shortcut_settings.shortcut = "alt+space".into();
        storage
            .save_app_state(&updated)
            .expect("save v0.4.0 update over v0.1.0 database");
        drop(storage);

        let reopened =
            AppStorage::open_with_provider_keychain(path.clone(), provider_keychain.clone())
                .expect("restart upgraded database");
        let restarted = reopened
            .load_app_state(fallback_state())
            .expect("reload upgraded database");
        assert_eq!(restarted.history.len(), 2);
        assert_eq!(restarted.user_rules.len(), 2);
        assert_eq!(restarted.selected_mode_id, "product-planning");
        assert_eq!(restarted.shortcut_settings.shortcut, "alt+space");
        assert_eq!(
            restarted.provider_settings.api_key,
            "synthetic-v010-key-never-contacted"
        );
        drop(reopened);

        let connection = Connection::open(&path).expect("inspect upgraded database");
        let provider_row: String = connection
            .query_row(
                "SELECT value FROM settings WHERE key='providerSettings'",
                [],
                |row| row.get(0),
            )
            .expect("read migrated provider settings");
        println!("SQLITE-REVERSE providerSettings={provider_row}");
        assert!(!provider_row.contains("synthetic-v010-key-never-contacted"));
        assert!(!provider_row.contains("\"apiKey\""));
        assert!(provider_row
            .contains("keychain://com.agentinputlayer.desktop/provider/openai-compatible"));
        for database_file in [
            path.clone(),
            path.with_extension("sqlite3-wal"),
            path.with_extension("sqlite3-shm"),
        ] {
            if database_file.exists() {
                let bytes = fs::read(&database_file).unwrap();
                assert!(
                    !bytes
                        .windows(b"synthetic-v010-key-never-contacted".len())
                        .any(|window| window == b"synthetic-v010-key-never-contacted"),
                    "legacy Provider key remained in {}",
                    database_file.display()
                );
            }
        }
        let migration_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("read migration inventory");
        assert_eq!(migration_count, MIGRATIONS.len() as i64);
        let history_count_after: i64 = connection
            .query_row("SELECT COUNT(*) FROM history", [], |row| row.get(0))
            .expect("count v0.1.0 history after migration");
        assert_eq!(history_count_after, history_count_before);
        drop(connection);

        let reopened_again =
            AppStorage::open_with_provider_keychain(path.clone(), provider_keychain)
                .expect("reopen migrated database again");
        drop(reopened_again);
        let connection = Connection::open(&path).expect("inspect idempotent migration state");
        let migration_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("read idempotent migration inventory");
        assert_eq!(migration_count, MIGRATIONS.len() as i64);
        assert_eq!(
            connection
                .query_row::<i64, _, _>("SELECT COUNT(*) FROM history", [], |row| row.get(0))
                .unwrap(),
            history_count_before
        );
        drop(connection);
        remove_database_files(&path);
    }

    #[test]
    fn lexicon_crud_update_and_delete_are_transactional() {
        let path = test_database_path("lexicon-crud");
        remove_database_files(&path);
        let storage = AppStorage::open(path.clone()).expect("open lexicon database");

        let created = storage
            .create_lexicon_entry(lexicon_input(
                "entry-1",
                "快照编译器",
                "kuai zhao bian yi qi",
            ))
            .expect("create lexicon entry");
        assert_eq!(created.aliases, vec!["快照编译器-alias"]);
        assert_eq!(created.professional_packs, vec!["software-engineering"]);
        assert_eq!(created.project_domains, vec!["agent-input-method"]);
        assert!(created.fixed);

        let mut updated = lexicon_input("entry-1", "快照编译器", "kuai zhao bian yi qi");
        updated.aliases = vec!["快照器".into(), "snapshot compiler".into()];
        updated.professional_packs = vec!["input-method".into()];
        updated.project_domains = vec!["v04".into()];
        updated.sources = vec![LexiconSource {
            source_kind: "import".into(),
            source: "synthetic-test.tsv".into(),
            license: "CC0-1.0".into(),
            confidence_millis: 875,
        }];
        updated.base_weight = 88;
        updated.fixed = false;
        updated.disabled = true;
        updated.demoted = true;
        updated.updated_at_ms += 1;
        let saved = storage
            .update_lexicon_entry(updated.clone())
            .expect("update lexicon entry");
        assert_eq!(saved.aliases, vec!["snapshot compiler", "快照器"]);
        assert_eq!(saved.sources, updated.sources);
        assert!(saved.disabled);
        assert!(saved.demoted);
        assert_eq!(storage.list_lexicon_entries().unwrap(), vec![saved]);

        assert!(storage.delete_lexicon_entry("entry-1").unwrap());
        assert!(storage.get_lexicon_entry("entry-1").unwrap().is_none());
        assert!(!storage.delete_lexicon_entry("entry-1").unwrap());

        let connection = Connection::open(&path).unwrap();
        for table in [
            "lexicon_entries",
            "lexicon_aliases",
            "professional_pack_entries",
            "lexicon_entry_domains",
            "lexicon_sources",
        ] {
            let query = format!("SELECT COUNT(*) FROM {table}");
            let count: i64 = connection.query_row(&query, [], |row| row.get(0)).unwrap();
            assert_eq!(count, 0, "delete left rows in {table}");
        }
        drop(connection);
        drop(storage);
        remove_database_files(&path);
    }

    #[test]
    fn project_profiles_switch_domains_packs_context_and_preserve_global_mode() {
        let path = test_database_path("project-profiles");
        remove_database_files(&path);
        let storage = AppStorage::open(path.clone()).unwrap();
        assert_eq!(
            storage.project_runtime_configuration().unwrap(),
            ProjectRuntimeConfiguration {
                active_profile_id: None,
                active_project_domain: None,
                active_professional_packs: Vec::new(),
                agent_context: String::new(),
                secondary_layer_enabled: true,
                prediction_enabled: false,
                context_model_ranking_enabled: false,
                candidate_theme: "system".into(),
            }
        );
        let profile = storage
            .save_project_profile(ProjectProfileInput {
                id: "aim".into(),
                name: "Agent Input Method".into(),
                project_domain: "agent-input-method".into(),
                professional_packs: vec!["rust".into(), "input-method".into()],
                agent_context: "只修改当前输入法项目".into(),
                updated_at_ms: 10,
            })
            .unwrap();
        assert!(!profile.active);
        let active = storage.activate_project_profile(Some("aim")).unwrap();
        assert_eq!(
            active.active_project_domain.as_deref(),
            Some("agent-input-method")
        );
        assert_eq!(
            active.active_professional_packs,
            vec!["input-method", "rust"]
        );
        assert_eq!(active.agent_context, "只修改当前输入法项目");
        assert!(
            !storage
                .set_secondary_layer_enabled(false)
                .unwrap()
                .secondary_layer_enabled
        );
        assert!(
            !storage
                .project_runtime_configuration()
                .unwrap()
                .prediction_enabled
        );
        assert!(
            storage
                .set_prediction_enabled(true)
                .unwrap()
                .prediction_enabled
        );
        for candidate_theme in ["system", "xuanmo", "shuitun", "youzi", "luoying"] {
            assert_eq!(
                storage
                    .set_candidate_theme(candidate_theme)
                    .unwrap()
                    .candidate_theme,
                candidate_theme
            );
        }
        assert_eq!(
            storage
                .set_candidate_theme_from_catalog("owner-red", true)
                .unwrap()
                .candidate_theme,
            "owner-red"
        );
        assert!(storage
            .set_candidate_theme_from_catalog("owner-red", false)
            .is_err());
        storage.set_candidate_theme("luoying").unwrap();
        assert!(storage.set_candidate_theme("unknown").is_err());
        let global = storage.activate_project_profile(None).unwrap();
        assert!(global.active_profile_id.is_none());
        assert!(global.active_professional_packs.is_empty());
        assert_eq!(storage.list_project_profiles().unwrap().len(), 1);
        drop(storage);
        let restarted = AppStorage::open(path.clone()).unwrap();
        assert_eq!(
            restarted
                .project_runtime_configuration()
                .unwrap()
                .candidate_theme,
            "luoying"
        );
        assert!(restarted.delete_project_profile("aim").unwrap());
        drop(restarted);
        remove_database_files(&path);
    }

    #[test]
    fn import_preview_duplicate_rejection_undo_and_export_restore_are_transactional() {
        let source_path = test_database_path("lexicon-import-source");
        let restore_path = test_database_path("lexicon-import-restore");
        remove_database_files(&source_path);
        remove_database_files(&restore_path);
        let source = AppStorage::open(source_path.clone()).unwrap();
        source
            .create_lexicon_entry(lexicon_input("export-1", "可恢复", "ke hui fu"))
            .unwrap();
        let exported = source.export_lexicon().unwrap();

        let restore = AppStorage::open(restore_path.clone()).unwrap();
        assert!(restore
            .preview_lexicon_import("not-json")
            .unwrap_err()
            .contains("JSON"));
        assert!(restore
            .preview_lexicon_import("\u{fffd}\u{fffd}\u{fffd}")
            .unwrap_err()
            .contains("UTF-8"));
        assert!(restore
            .preview_lexicon_import("{\"schema_version\":1,\"entries\":[{}]}")
            .is_err());
        let preview = restore.preview_lexicon_import(&exported).unwrap();
        assert_eq!(preview.entry_count, 1);
        assert_eq!(preview.texts, ["可恢复"]);
        let imported = restore.import_lexicon(&exported, 1_000).unwrap();
        assert_eq!(imported.imported_count, 1);
        assert_eq!(restore.list_lexicon_entries().unwrap().len(), 1);
        assert!(restore
            .preview_lexicon_import(&exported)
            .unwrap_err()
            .contains("重复导入"));
        assert!(restore.import_lexicon(&exported, 1_001).is_err());
        assert_eq!(restore.list_lexicon_entries().unwrap().len(), 1);
        assert_eq!(
            restore
                .undo_lexicon_import(&imported.batch_id, 2_000)
                .unwrap(),
            1
        );
        assert!(restore.list_lexicon_entries().unwrap().is_empty());
        assert!(restore
            .undo_lexicon_import(&imported.batch_id, 2_001)
            .is_err());

        drop(source);
        drop(restore);
        remove_database_files(&source_path);
        remove_database_files(&restore_path);
    }

    #[test]
    fn lexicon_conflicts_are_rejected_without_overwriting() {
        let path = test_database_path("lexicon-conflict");
        remove_database_files(&path);
        let storage = AppStorage::open(path.clone()).unwrap();
        let original = lexicon_input("entry-1", "原子加载", "yuan zi jia zai");
        storage.create_lexicon_entry(original.clone()).unwrap();

        let duplicate = lexicon_input("entry-2", "原子加载", "yuan zi jia zai");
        let error = storage
            .create_lexicon_entry(duplicate)
            .expect_err("duplicate text and pinyin must conflict");
        assert!(error.contains("冲突"), "unexpected conflict error: {error}");
        assert_eq!(storage.list_lexicon_entries().unwrap().len(), 1);
        assert_eq!(
            storage.get_lexicon_entry("entry-1").unwrap().unwrap().text,
            original.text
        );

        drop(storage);
        remove_database_files(&path);
    }

    #[test]
    fn lexicon_source_and_license_are_required_at_api_and_database_layers() {
        let path = test_database_path("lexicon-required-provenance");
        remove_database_files(&path);
        let storage = AppStorage::open(path.clone()).unwrap();

        let mut missing_sources = lexicon_input("entry-api-1", "来源", "lai yuan");
        missing_sources.sources.clear();
        assert!(storage.create_lexicon_entry(missing_sources).is_err());

        for (id, source, license) in [
            ("entry-api-2", "   ", "private-user-data"),
            ("entry-api-3", "owner-entry", "\n\t"),
        ] {
            let mut input = lexicon_input(id, "许可证", "xu ke zheng");
            input.sources[0].source = source.into();
            input.sources[0].license = license.into();
            assert!(storage.create_lexicon_entry(input).is_err());
        }

        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO lexicon_entries
             (id, text, pinyin, base_weight, fixed, disabled, created_at_ms, updated_at_ms)
             VALUES ('direct', '直接写入', 'zhi jie xie ru', 0, 0, 0, 1, 1)",
                [],
            )
            .unwrap();
        for (source, license) in [("", "private"), ("manual", "   ")] {
            assert!(connection
                .execute(
                    "INSERT INTO lexicon_sources
                 (entry_id, source_kind, source, license, confidence_millis)
                 VALUES ('direct', 'manual', ?1, ?2, 1000)",
                    params![source, license],
                )
                .is_err());
        }

        drop(connection);
        drop(storage);
        remove_database_files(&path);
    }

    #[test]
    fn sqlite_lock_is_bounded_and_recovers_without_data_loss() {
        let path = test_database_path("locked-recovery");
        remove_database_files(&path);

        let storage = AppStorage::open(path.clone()).expect("open test database");
        let original = fallback_state();
        storage
            .save_app_state(&original)
            .expect("seed state before taking external lock");

        let blocker = Connection::open(&path).expect("open independent lock connection");
        blocker
            .execute_batch("BEGIN IMMEDIATE;")
            .expect("hold SQLite writer lock");

        let mut replacement = fallback_state();
        replacement.selected_mode_id = "bug-fix".into();
        let started = std::time::Instant::now();
        let error = storage
            .save_app_state(&replacement)
            .expect_err("locked database must reject the write");
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(2),
            "SQLite lock handling exceeded the bounded timeout: {elapsed:?}"
        );
        assert!(
            error.contains("locked") || error.contains("busy"),
            "unexpected SQLite lock error: {error}"
        );

        blocker
            .execute_batch("ROLLBACK;")
            .expect("release SQLite writer lock");
        drop(blocker);

        let unchanged = storage
            .load_app_state(fallback_state())
            .expect("load original state after rejected transaction");
        assert_eq!(unchanged.selected_mode_id, original.selected_mode_id);

        storage
            .save_app_state(&replacement)
            .expect("save succeeds after lock release");
        let recovered = storage
            .load_app_state(fallback_state())
            .expect("load recovered state");
        assert_eq!(recovered.selected_mode_id, "bug-fix");

        drop(storage);
        remove_database_files(&path);
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub history: Vec<HistoryItem>,
    pub user_rules: Vec<UserRule>,
    pub selected_mode_id: String,
    pub provider_settings: ProviderSettings,
    pub shortcut_settings: ShortcutSettings,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub id: String,
    pub raw_input: String,
    pub mode_id: String,
    pub target_tool: Option<String>,
    pub output: String,
    pub favorite: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserRule {
    pub id: String,
    pub text: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettings {
    pub enabled: bool,
    #[serde(default = "default_provider_kind")]
    pub provider_kind: String,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    #[serde(default = "default_anthropic_base_url")]
    pub anthropic_base_url: String,
    #[serde(default)]
    pub anthropic_model: String,
    #[serde(default)]
    pub anthropic_api_key: String,
    #[serde(default = "default_ollama_base_url")]
    pub ollama_base_url: String,
    #[serde(default)]
    pub ollama_model: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedProviderSettings {
    credential_storage_version: u32,
    enabled: bool,
    provider_kind: String,
    base_url: String,
    model: String,
    api_key_ref: Option<String>,
    anthropic_base_url: String,
    anthropic_model: String,
    anthropic_api_key_ref: Option<String>,
    ollama_base_url: String,
    ollama_model: String,
}

impl PersistedProviderSettings {
    fn from_runtime(
        settings: &ProviderSettings,
        api_key_ref: Option<String>,
        anthropic_api_key_ref: Option<String>,
    ) -> Self {
        Self {
            credential_storage_version: 1,
            enabled: settings.enabled,
            provider_kind: settings.provider_kind.clone(),
            base_url: settings.base_url.clone(),
            model: settings.model.clone(),
            api_key_ref,
            anthropic_base_url: settings.anthropic_base_url.clone(),
            anthropic_model: settings.anthropic_model.clone(),
            anthropic_api_key_ref,
            ollama_base_url: settings.ollama_base_url.clone(),
            ollama_model: settings.ollama_model.clone(),
        }
    }
}

fn default_provider_kind() -> String {
    "openai-compatible".into()
}
fn default_anthropic_base_url() -> String {
    "https://api.anthropic.com".into()
}
fn default_ollama_base_url() -> String {
    "http://127.0.0.1:11434".into()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutSettings {
    pub shortcut: String,
    #[serde(default)]
    pub mcp_enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LexiconSource {
    pub source_kind: String,
    pub source: String,
    pub license: String,
    pub confidence_millis: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LexiconEntryInput {
    pub id: String,
    pub text: String,
    pub pinyin: String,
    pub aliases: Vec<String>,
    pub professional_packs: Vec<String>,
    pub project_domains: Vec<String>,
    pub sources: Vec<LexiconSource>,
    pub base_weight: i64,
    pub fixed: bool,
    pub disabled: bool,
    pub demoted: bool,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LexiconEntry {
    pub id: String,
    pub text: String,
    pub pinyin: String,
    pub aliases: Vec<String>,
    pub professional_packs: Vec<String>,
    pub project_domains: Vec<String>,
    pub sources: Vec<LexiconSource>,
    pub base_weight: i64,
    pub fixed: bool,
    pub disabled: bool,
    pub demoted: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LexiconExportDocument {
    pub format_version: u32,
    pub entries: Vec<LexiconEntryInput>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LexiconImportPreview {
    pub entry_count: usize,
    pub texts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LexiconImportResult {
    pub batch_id: String,
    pub imported_count: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectProfileInput {
    pub id: String,
    pub name: String,
    pub project_domain: String,
    pub professional_packs: Vec<String>,
    pub agent_context: String,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectProfile {
    pub id: String,
    pub name: String,
    pub project_domain: String,
    pub professional_packs: Vec<String>,
    pub agent_context: String,
    pub active: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRuntimeConfiguration {
    pub active_profile_id: Option<String>,
    pub active_project_domain: Option<String>,
    pub active_professional_packs: Vec<String>,
    pub agent_context: String,
    pub secondary_layer_enabled: bool,
    pub prediction_enabled: bool,
    #[serde(default)]
    pub context_model_ranking_enabled: bool,
    #[serde(default = "default_candidate_theme")]
    pub candidate_theme: String,
}

fn default_candidate_theme() -> String {
    "system".into()
}

impl AppStorage {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        #[cfg(test)]
        let provider_keychain: Arc<dyn ProviderKeychain> =
            Arc::new(crate::keychain::MemoryProviderKeychain::default());
        #[cfg(not(test))]
        let provider_keychain = system_provider_keychain();
        Self::open_with_provider_keychain(path, provider_keychain)
    }

    #[cfg(test)]
    pub(crate) fn open_with_provider_keychain(
        path: PathBuf,
        provider_keychain: Arc<dyn ProviderKeychain>,
    ) -> Result<Self, String> {
        Self::open_inner(path, provider_keychain)
    }

    #[cfg(not(test))]
    fn open_with_provider_keychain(
        path: PathBuf,
        provider_keychain: Arc<dyn ProviderKeychain>,
    ) -> Result<Self, String> {
        Self::open_inner(path, provider_keychain)
    }

    fn open_inner(
        path: PathBuf,
        provider_keychain: Arc<dyn ProviderKeychain>,
    ) -> Result<Self, String> {
        #[cfg(feature = "personal-seed")]
        let should_seed_personal_profile = path
            .file_name()
            .is_some_and(|name| name == "agent-input-layer.sqlite3");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("无法创建存储目录：{error}"))?;
            #[cfg(unix)]
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .map_err(|error| format!("无法设置存储目录权限：{error}"))?;
        }

        let connection =
            Connection::open(&path).map_err(|error| format!("无法打开 SQLite 数据库：{error}"))?;
        connection
            .busy_timeout(SQLITE_BUSY_TIMEOUT)
            .map_err(|error| format!("无法设置 SQLite 锁等待上限：{error}"))?;
        let storage = Self {
            connection: Mutex::new(connection),
            provider_keychain,
        };

        storage.migrate()?;
        #[cfg(feature = "personal-seed")]
        if should_seed_personal_profile {
            storage.seed_personal_profile_v1(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| "系统时间早于 Unix epoch，个人档案种子已阻止。".to_string())?
                    .as_millis()
                    .try_into()
                    .map_err(|_| "系统时间超出个人档案范围。".to_string())?,
            )?;
        }
        #[cfg(unix)]
        {
            let wal = path.with_extension("sqlite3-wal");
            let shm = path.with_extension("sqlite3-shm");
            for candidate in [&path, &wal, &shm] {
                if candidate.exists() {
                    fs::set_permissions(candidate, fs::Permissions::from_mode(0o600))
                        .map_err(|error| format!("无法设置 SQLite 权限：{error}"))?;
                }
            }
        }
        Ok(storage)
    }

    fn migrate_legacy_provider_credentials(&self) -> Result<(), String> {
        let raw = {
            let connection = self.lock_connection()?;
            get_setting_text(&connection, PROVIDER_SETTINGS_KEY)?
        };
        let Some(raw) = raw else {
            return Ok(());
        };
        let document: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|error| format!("无法解析设置 {PROVIDER_SETTINGS_KEY}：{error}"))?;
        if document.get("credentialStorageVersion").is_some() {
            serde_json::from_value::<PersistedProviderSettings>(document)
                .map_err(|error| format!("无法解析 Provider 密钥引用：{error}"))?;
            return Ok(());
        }

        let legacy: ProviderSettings = serde_json::from_value(document)
            .map_err(|error| format!("无法解析旧版 Provider 设置：{error}"))?;
        let api_key_ref = self.migrate_legacy_password("openai-compatible", &legacy.api_key)?;
        let anthropic_api_key_ref =
            self.migrate_legacy_password("anthropic", &legacy.anthropic_api_key)?;
        let persisted =
            PersistedProviderSettings::from_runtime(&legacy, api_key_ref, anthropic_api_key_ref);
        let mut connection = self.lock_connection()?;
        connection
            .execute_batch("PRAGMA secure_delete = ON;")
            .map_err(|error| format!("无法启用 Provider 明文安全抹除：{error}"))?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始 Provider 密钥抹除事务：{error}"))?;
        set_setting(&transaction, PROVIDER_SETTINGS_KEY, &persisted)?;
        transaction
            .commit()
            .map_err(|error| format!("无法提交 Provider 密钥抹除事务：{error}"))?;
        connection
            .execute_batch(
                "PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);",
            )
            .map_err(|error| format!("无法完成 Provider 明文物理抹除：{error}"))
    }

    fn migrate_legacy_password(
        &self,
        provider_kind: &str,
        password: &str,
    ) -> Result<Option<String>, String> {
        if password.is_empty() {
            return Ok(None);
        }
        self.provider_keychain
            .set_password(provider_kind, password)
            .map_err(|_| {
                format!(
                    "无法把 {provider_kind} Provider 密钥迁移到 macOS 钥匙串；数据库明文未被使用，请解锁钥匙串后重试。"
                )
            })?;
        Ok(Some(provider_keychain_reference(provider_kind)))
    }

    fn load_provider_settings(
        &self,
        fallback: ProviderSettings,
    ) -> Result<ProviderSettings, String> {
        let persisted = {
            let connection = self.lock_connection()?;
            get_setting::<PersistedProviderSettings>(&connection, PROVIDER_SETTINGS_KEY)?
        };
        let Some(persisted) = persisted else {
            return Ok(fallback);
        };
        if persisted.credential_storage_version != 1 {
            return Err("Provider 密钥引用版本不受支持。".into());
        }
        let api_key =
            self.load_referenced_password("openai-compatible", persisted.api_key_ref.as_deref())?;
        let anthropic_api_key =
            self.load_referenced_password("anthropic", persisted.anthropic_api_key_ref.as_deref())?;
        Ok(ProviderSettings {
            enabled: persisted.enabled,
            provider_kind: persisted.provider_kind,
            base_url: persisted.base_url,
            model: persisted.model,
            api_key,
            anthropic_base_url: persisted.anthropic_base_url,
            anthropic_model: persisted.anthropic_model,
            anthropic_api_key,
            ollama_base_url: persisted.ollama_base_url,
            ollama_model: persisted.ollama_model,
        })
    }

    fn load_referenced_password(
        &self,
        provider_kind: &str,
        reference: Option<&str>,
    ) -> Result<String, String> {
        let Some(reference) = reference else {
            return Ok(String::new());
        };
        validate_provider_keychain_reference(provider_kind, reference)?;
        self.provider_keychain
            .get_password(provider_kind)
            .map_err(|_| {
                "无法读取 macOS 钥匙串中的 Provider 密钥，请解锁钥匙串后重试。".to_string()
            })?
            .filter(|password| !password.is_empty())
            .ok_or_else(|| "钥匙串中未找到 Provider 密钥，请重新填写。".to_string())
    }

    fn persist_provider_settings(
        &self,
        settings: &ProviderSettings,
    ) -> Result<PersistedProviderSettings, String> {
        let current = {
            let connection = self.lock_connection()?;
            get_setting::<PersistedProviderSettings>(&connection, PROVIDER_SETTINGS_KEY)?
        };
        let api_key_ref = self.persist_password(
            "openai-compatible",
            &settings.api_key,
            current
                .as_ref()
                .and_then(|item| item.api_key_ref.as_deref()),
        )?;
        let anthropic_api_key_ref = self.persist_password(
            "anthropic",
            &settings.anthropic_api_key,
            current
                .as_ref()
                .and_then(|item| item.anthropic_api_key_ref.as_deref()),
        )?;
        Ok(PersistedProviderSettings::from_runtime(
            settings,
            api_key_ref,
            anthropic_api_key_ref,
        ))
    }

    fn persist_password(
        &self,
        provider_kind: &str,
        password: &str,
        current_reference: Option<&str>,
    ) -> Result<Option<String>, String> {
        if password.is_empty() {
            if let Some(reference) = current_reference {
                validate_provider_keychain_reference(provider_kind, reference)?;
                self.provider_keychain
                    .delete_password(provider_kind)
                    .map_err(|_| {
                        "无法删除 macOS 钥匙串中的 Provider 密钥，请解锁钥匙串后重试。".to_string()
                    })?;
            }
            return Ok(None);
        }
        self.provider_keychain
            .set_password(provider_kind, password)
            .map_err(|_| {
                "无法写入 macOS 钥匙串中的 Provider 密钥，请解锁钥匙串后重试。".to_string()
            })?;
        Ok(Some(provider_keychain_reference(provider_kind)))
    }

    pub fn provider_api_key(&self, provider_kind: &str) -> Result<String, String> {
        if provider_kind == "ollama" {
            return Ok(String::new());
        }
        self.migrate_legacy_provider_credentials()?;
        let persisted = {
            let connection = self.lock_connection()?;
            get_setting::<PersistedProviderSettings>(&connection, PROVIDER_SETTINGS_KEY)?
        }
        .ok_or_else(|| "请先填写完整 Provider 配置。".to_string())?;
        let reference = match provider_kind {
            "openai-compatible" => persisted.api_key_ref.as_deref(),
            "anthropic" => persisted.anthropic_api_key_ref.as_deref(),
            _ => return Err("不支持的 Provider 类型。".into()),
        };
        self.load_referenced_password(provider_kind, reference)
    }

    /// Seeds the verifier-frozen owner profile exactly once through the
    /// existing SQLite -> immutable snapshot -> Mixer personal-word channel.
    /// The parser deliberately has no field for raw selection totals: only
    /// friction and confident friction disambiguation can influence entries.
    #[cfg(feature = "personal-seed")]
    fn seed_personal_profile_v1(&self, now_ms: i64) -> Result<usize, String> {
        if now_ms < 0 {
            return Err("个人档案种子时间不能为负数。".into());
        }
        let profile: PersonalProfileDocument = serde_json::from_str(PERSONAL_PROFILE_V1)
            .map_err(|error| format!("个人档案 v1 无法解析：{error}"))?;
        let seed: PersonalProfileSeed = serde_json::from_str(PERSONAL_PROFILE_V1_SEED)
            .map_err(|error| format!("个人档案 v1 红项清单无法解析：{error}"))?;
        if seed.kind != "personal-profile-friction-seed"
            || seed.format_version != 1
            || seed.source != "quality/mined/personal-profile-v1.json"
            || seed.source_sha256
                != "63bbd50ba5a7570bd659bec603502837952db1dabfd3f81a368bf7ca80b35203"
            || seed.baseline_artifact_checksum.len() != 64
            || seed.baseline_top1 != "41/87"
            || seed.authoring_method.trim().is_empty()
        {
            return Err("个人档案 v1 红项清单元数据不匹配。".into());
        }
        let declared = profile
            .owner_declared_disambiguation
            .iter()
            .map(|item| {
                (
                    item.pinyin.as_str(),
                    item.candidates
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                    item.confident,
                    item.source.as_str(),
                )
            })
            .collect::<Vec<_>>();
        if declared
            != [
                (
                    "zhidao",
                    vec!["知道", "指导"],
                    false,
                    "owner-explicit-2026-07-22",
                ),
                (
                    "zenmkn",
                    vec!["怎么看", "怎么可能"],
                    false,
                    "owner-explicit-2026-07-22",
                ),
            ]
        {
            return Err("个人档案 v1 owner 同音歧义登记不匹配。".into());
        }
        let red = seed.still_red_readings.into_iter().collect::<HashSet<_>>();
        if red.len() != 46 {
            return Err("个人档案 v1 红项清单必须恰好包含 46 个唯一读音。".into());
        }

        let disambiguation = profile
            .disambiguation_by_friction
            .into_iter()
            .map(|item| (item.pinyin.clone(), item))
            .collect::<HashMap<_, _>>();
        let mut targets = HashMap::<String, (String, u64)>::new();
        for item in profile.friction_boost {
            if !disambiguation.contains_key(&item.pinyin) {
                targets.insert(item.pinyin, (item.text, item.friction));
            }
        }
        for item in disambiguation.into_values().filter(|item| item.confident) {
            let friction = item.friction.get(&item.prefer).copied().unwrap_or(0);
            if friction == 0 {
                return Err(format!(
                    "个人档案 confident 项缺少 friction：{}",
                    item.pinyin
                ));
            }
            targets.insert(item.pinyin, (item.prefer, friction));
        }
        if targets.len() != 87 || !red.iter().all(|reading| targets.contains_key(reading)) {
            return Err("个人档案 v1 friction 派生集合与红项清单不一致。".into());
        }

        let mut connection = self.lock_connection()?;
        if get_setting::<bool>(&connection, PERSONAL_PROFILE_V1_SEEDED_KEY)?.unwrap_or(false) {
            return Ok(0);
        }
        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始个人档案种子事务：{error}"))?;
        let mut inserted = 0;
        let mut ordered = red.into_iter().collect::<Vec<_>>();
        ordered.sort();
        for reading in ordered {
            let (text, friction) = targets
                .remove(&reading)
                .ok_or_else(|| "个人档案 v1 红项在派生集合中缺失。".to_string())?;
            let input = LexiconEntryInput {
                id: format!("personal-profile-v1:{reading}"),
                text,
                pinyin: reading,
                aliases: Vec::new(),
                professional_packs: Vec::new(),
                project_domains: Vec::new(),
                sources: vec![LexiconSource {
                    source_kind: "personal-friction".into(),
                    source: "quality/mined/personal-profile-v1.json#friction".into(),
                    license: "private-user-data".into(),
                    confidence_millis: 1000,
                }],
                base_weight: 520,
                fixed: false,
                disabled: false,
                demoted: false,
                updated_at_ms: now_ms.saturating_add(friction.min(999) as i64),
            };
            let input = validate_lexicon_input(input)?;
            let conflict = transaction
                .query_row(
                    "SELECT 1 FROM lexicon_entries WHERE id = ?1 OR (text = ?2 AND pinyin = ?3)",
                    params![&input.id, &input.text, &input.pinyin],
                    |_| Ok(()),
                )
                .optional()
                .map_err(|error| format!("无法检查个人档案种子冲突：{error}"))?
                .is_some();
            if !conflict {
                insert_lexicon_entry_in_transaction(&transaction, &input)?;
                inserted += 1;
            }
        }
        set_setting(&transaction, PERSONAL_PROFILE_V1_SEEDED_KEY, &true)?;
        transaction
            .commit()
            .map_err(|error| format!("无法提交个人档案种子事务：{error}"))?;
        Ok(inserted)
    }

    pub(crate) fn apply_personal_profile_auto_update(
        &self,
        targets: &[ConfidentProfileTarget],
        blacklist: &HashSet<(String, String)>,
        now_ms: i64,
    ) -> Result<PersonalProfileAutoUpdateReport, String> {
        let targets = targets
            .iter()
            .cloned()
            .map(|target| (target, ProfileUpdateEvidence::Friction))
            .collect::<Vec<_>>();
        self.apply_personal_profile_updates(&targets, blacklist, now_ms)
    }

    pub(crate) fn declare_owner_disambiguation(
        &self,
        pinyin: &str,
        preferred_text: &str,
        candidates: &[String],
        blacklist: &HashSet<(String, String)>,
        now_ms: i64,
    ) -> Result<PersonalProfileAutoUpdateReport, String> {
        let pinyin = required_trimmed("owner 指定读音", pinyin.to_ascii_lowercase())?;
        if !pinyin
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'\'')
        {
            return Err("owner 指定读音只能包含小写拼音字母与分隔符。".into());
        }
        let preferred_text = required_trimmed("owner 指定词", preferred_text.to_string())?;
        let mut candidates = candidates
            .iter()
            .map(|candidate| required_trimmed("owner 候选词", candidate.clone()))
            .collect::<Result<Vec<_>, _>>()?;
        candidates.sort();
        candidates.dedup();
        if candidates.len() < 2 || !candidates.contains(&preferred_text) {
            return Err("owner 指定必须从同一读音的至少两个候选中选择。".into());
        }
        let evidence = OwnerDeclaredEvidence {
            pinyin: pinyin.clone(),
            candidates,
            prefer: preferred_text.clone(),
            confident: true,
            source: "owner-explicit-manager-panel".into(),
        };
        let target = ConfidentProfileTarget {
            pinyin,
            text: preferred_text,
            friction_count: 0,
            runner_up_friction_count: 0,
            latest_friction_at_ms: now_ms,
        };
        self.apply_personal_profile_updates(
            &[(target, ProfileUpdateEvidence::OwnerDeclared(evidence))],
            blacklist,
            now_ms,
        )
    }

    fn apply_personal_profile_updates(
        &self,
        targets: &[(ConfidentProfileTarget, ProfileUpdateEvidence)],
        blacklist: &HashSet<(String, String)>,
        now_ms: i64,
    ) -> Result<PersonalProfileAutoUpdateReport, String> {
        if now_ms < 0 {
            return Err("档案自更新时间不能为负数。".into());
        }
        let mut ordered = targets.to_vec();
        ordered.sort_by(|left, right| {
            left.0
                .pinyin
                .cmp(&right.0.pinyin)
                .then_with(|| left.0.text.cmp(&right.0.text))
        });
        if ordered.iter().any(|(target, evidence)| {
            target.pinyin.is_empty()
                || target.text.trim().is_empty()
                || target.latest_friction_at_ms < 0
                || match evidence {
                    ProfileUpdateEvidence::Friction => {
                        target.friction_count == 0
                            || target.friction_count
                                < target.runner_up_friction_count.saturating_mul(2)
                    }
                    ProfileUpdateEvidence::OwnerDeclared(owner) => {
                        target.friction_count != 0
                            || target.runner_up_friction_count != 0
                            || !owner.confident
                            || owner.pinyin != target.pinyin
                            || owner.prefer != target.text
                            || !owner.candidates.contains(&target.text)
                    }
                }
        }) || ordered
            .windows(2)
            .any(|pair| pair[0].0.pinyin == pair[1].0.pinyin && pair[0].0.text == pair[1].0.text)
        {
            return Err("档案自更新只接受唯一的高置信结论或 owner 明示选择。".into());
        }

        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始档案自更新事务：{error}"))?;
        let mut report = PersonalProfileAutoUpdateReport {
            applied_count: 0,
            refreshed_count: 0,
            decayed_count: 0,
            blacklisted_count: 0,
            skipped_revoked_count: 0,
            items: Vec::new(),
        };

        let active = load_auto_update_rows(&transaction, Some("active"))?;
        for row in active {
            let pair = (row.pinyin.clone(), row.text.clone());
            let next_status = if blacklist.contains(&pair) {
                report.blacklisted_count += 1;
                Some("blacklisted")
            } else if now_ms.saturating_sub(row.latest_friction_at_ms)
                >= PERSONAL_PROFILE_DECAY_AFTER_MS
            {
                report.decayed_count += 1;
                Some("decayed")
            } else {
                None
            };
            if let Some(status) = next_status {
                restore_auto_entry(&transaction, &row)?;
                transaction
                    .execute(
                        "UPDATE personal_profile_auto_updates
                         SET status=?2,status_changed_at_ms=?3,last_refreshed_at_ms=?3
                         WHERE id=?1",
                        params![&row.id, status, now_ms],
                    )
                    .map_err(|error| format!("无法记录档案自更新回落：{error}"))?;
            }
        }

        for (target, evidence) in ordered {
            let pair = (target.pinyin.clone(), target.text.clone());
            if blacklist.contains(&pair)
                || now_ms.saturating_sub(target.latest_friction_at_ms)
                    >= PERSONAL_PROFILE_DECAY_AFTER_MS
            {
                continue;
            }
            let previous =
                load_auto_update_row_by_pair(&transaction, &target.pinyin, &target.text)?;
            if previous.as_ref().is_some_and(|row| row.status == "revoked") {
                report.skipped_revoked_count += 1;
                continue;
            }

            let (baseline, entry_id, applied_at_ms, was_active) = if let Some(row) = previous {
                if row.status == "active" {
                    let current = current_auto_entry_input(&transaction, &row.entry_id)?
                        .ok_or_else(|| {
                            "自动应用中的个人词已消失，档案自更新失败关闭。".to_string()
                        })?;
                    let expected: LexiconEntryInput = serde_json::from_str(&row.applied_entry_json)
                        .map_err(|_| "档案自更新审计快照损坏。".to_string())?;
                    if current != expected {
                        return Err("自动应用中的个人词被外部修改，档案自更新失败关闭。".into());
                    }
                    (
                        decode_optional_entry(&row.baseline_entry_json)?,
                        row.entry_id,
                        row.applied_at_ms,
                        true,
                    )
                } else {
                    let baseline = decode_optional_entry(&row.baseline_entry_json)?;
                    ensure_restored_baseline(&transaction, &row.entry_id, baseline.as_ref())?;
                    (baseline, row.entry_id, now_ms, false)
                }
            } else {
                let baseline =
                    load_lexicon_entry_by_pair(&transaction, &target.pinyin, &target.text)?
                        .map(lexicon_entry_input);
                let entry_id = baseline
                    .as_ref()
                    .map(|entry| entry.id.clone())
                    .unwrap_or_else(|| {
                        format!("personal-profile-auto:{}:{}", target.pinyin, target.text)
                    });
                (baseline, entry_id, now_ms, false)
            };

            let applied =
                auto_applied_entry(&entry_id, baseline.as_ref(), &target, &evidence, now_ms)?;
            write_lexicon_entry(&transaction, &applied)?;
            let baseline_json = baseline
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|error| format!("无法编码档案自更新基线：{error}"))?;
            let applied_json = serde_json::to_string(&applied)
                .map_err(|error| format!("无法编码档案自更新结果：{error}"))?;
            let id = format!("profile-auto:{}:{}", target.pinyin, target.text);
            let (evidence_kind, evidence_json) = match evidence {
                ProfileUpdateEvidence::Friction => ("friction", None),
                ProfileUpdateEvidence::OwnerDeclared(owner) => (
                    "owner_declared_disambiguation",
                    Some(
                        serde_json::to_string(&owner)
                            .map_err(|error| format!("无法编码 owner 指定依据：{error}"))?,
                    ),
                ),
            };
            transaction
                .execute(
                    "INSERT INTO personal_profile_auto_updates
                     (id,entry_id,pinyin,text,friction_count,runner_up_friction_count,
                      latest_friction_at_ms,applied_at_ms,last_refreshed_at_ms,status,
                      status_changed_at_ms,baseline_entry_json,applied_entry_json,prediction_eligible,
                      evidence_kind,evidence_json)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'active',?9,?10,?11,1,?12,?13)
                     ON CONFLICT(pinyin,text) DO UPDATE SET
                      entry_id=excluded.entry_id,friction_count=excluded.friction_count,
                      runner_up_friction_count=excluded.runner_up_friction_count,
                      latest_friction_at_ms=excluded.latest_friction_at_ms,
                      applied_at_ms=excluded.applied_at_ms,last_refreshed_at_ms=excluded.last_refreshed_at_ms,
                      status='active',status_changed_at_ms=excluded.status_changed_at_ms,
                      baseline_entry_json=excluded.baseline_entry_json,
                      applied_entry_json=excluded.applied_entry_json,prediction_eligible=1,
                      evidence_kind=excluded.evidence_kind,evidence_json=excluded.evidence_json",
                    params![
                        &id,
                        &entry_id,
                        &target.pinyin,
                        &target.text,
                        target.friction_count as i64,
                        target.runner_up_friction_count as i64,
                        target.latest_friction_at_ms,
                        applied_at_ms,
                        now_ms,
                        baseline_json,
                        applied_json,
                        evidence_kind,
                        evidence_json,
                    ],
                )
                .map_err(|error| format!("无法记录档案自更新依据：{error}"))?;
            if was_active {
                report.refreshed_count += 1;
            } else {
                report.applied_count += 1;
            }
        }

        transaction
            .commit()
            .map_err(|error| format!("无法提交档案自更新：{error}"))?;
        drop(connection);
        report.items = self.list_personal_profile_auto_updates()?;
        Ok(report)
    }

    pub fn list_personal_profile_auto_updates(
        &self,
    ) -> Result<Vec<PersonalProfileAutoUpdateItem>, String> {
        let connection = self.lock_connection()?;
        Ok(load_auto_update_rows(&connection, None)?
            .into_iter()
            .map(auto_update_item)
            .collect())
    }

    pub fn revoke_personal_profile_auto_update(
        &self,
        id: &str,
        now_ms: i64,
    ) -> Result<PersonalProfileAutoUpdateItem, String> {
        require_nonempty("档案自更新 ID", id)?;
        if now_ms < 0 {
            return Err("档案自更新撤销时间不能为负数。".into());
        }
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始档案自更新撤销：{error}"))?;
        let row = load_auto_update_row_by_id(&transaction, id)?
            .ok_or_else(|| "要撤销的档案自更新不存在。".to_string())?;
        if row.status == "revoked" {
            return Err("该档案自更新已经撤销。".into());
        }
        if row.status == "active" {
            restore_auto_entry(&transaction, &row)?;
        }
        transaction
            .execute(
                "UPDATE personal_profile_auto_updates
                 SET status='revoked',status_changed_at_ms=?2,last_refreshed_at_ms=?2
                 WHERE id=?1",
                params![id, now_ms],
            )
            .map_err(|error| format!("无法记录档案自更新撤销：{error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("无法提交档案自更新撤销：{error}"))?;
        drop(connection);
        self.list_personal_profile_auto_updates()?
            .into_iter()
            .find(|item| item.id == id)
            .ok_or_else(|| "档案自更新撤销后审计记录不可见。".to_string())
    }

    pub fn load_app_state(&self, fallback: AppState) -> Result<AppState, String> {
        self.migrate_legacy_provider_credentials()?;
        let initialized = {
            let connection = self.lock_connection()?;
            get_setting::<bool>(&connection, INITIALIZED_KEY)?.unwrap_or(false)
        };

        if !initialized {
            self.save_app_state(&fallback)?;
            return Ok(fallback);
        }

        let fallback_provider_settings = fallback.provider_settings.clone();
        let (selected_mode_id, shortcut_settings, history, user_rules) = {
            let connection = self.lock_connection()?;
            (
                get_setting::<String>(&connection, SELECTED_MODE_KEY)?
                    .unwrap_or(fallback.selected_mode_id),
                get_setting::<ShortcutSettings>(&connection, SHORTCUT_SETTINGS_KEY)?
                    .unwrap_or(fallback.shortcut_settings),
                load_history(&connection)?,
                load_user_rules(&connection)?,
            )
        };
        let provider_settings = self.load_provider_settings(fallback_provider_settings)?;

        Ok(AppState {
            history,
            user_rules,
            selected_mode_id,
            provider_settings,
            shortcut_settings,
        })
    }

    pub fn save_app_state(&self, state: &AppState) -> Result<(), String> {
        self.migrate_legacy_provider_credentials()?;
        let persisted_provider_settings =
            self.persist_provider_settings(&state.provider_settings)?;
        let mut connection = self.lock_connection()?;
        save_app_state_in_transaction(&mut connection, state, &persisted_provider_settings)
    }

    pub fn create_lexicon_entry(&self, input: LexiconEntryInput) -> Result<LexiconEntry, String> {
        let input = validate_lexicon_input(input)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始个人词新增事务：{error}"))?;
        let result = transaction.execute(
            "INSERT INTO lexicon_entries
             (id, text, pinyin, base_weight, fixed, disabled, demoted, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![
                &input.id,
                &input.text,
                &input.pinyin,
                input.base_weight,
                bool_i64(input.fixed),
                bool_i64(input.disabled),
                bool_i64(input.demoted),
                input.updated_at_ms,
            ],
        );
        if let Err(error) = result {
            return Err(map_lexicon_write_error("新增个人词", error));
        }
        replace_lexicon_relations(&transaction, &input)?;
        transaction
            .commit()
            .map_err(|error| format!("无法提交个人词新增事务：{error}"))?;
        drop(connection);
        self.get_lexicon_entry(&input.id)?
            .ok_or_else(|| "个人词新增后不可见。".to_string())
    }

    pub fn update_lexicon_entry(&self, input: LexiconEntryInput) -> Result<LexiconEntry, String> {
        let input = validate_lexicon_input(input)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始个人词更新事务：{error}"))?;
        let changed = transaction
            .execute(
                "UPDATE lexicon_entries SET
               text = ?2, pinyin = ?3, base_weight = ?4, fixed = ?5,
               disabled = ?6, demoted = ?7, updated_at_ms = ?8
             WHERE id = ?1",
                params![
                    &input.id,
                    &input.text,
                    &input.pinyin,
                    input.base_weight,
                    bool_i64(input.fixed),
                    bool_i64(input.disabled),
                    bool_i64(input.demoted),
                    input.updated_at_ms,
                ],
            )
            .map_err(|error| map_lexicon_write_error("更新个人词", error))?;
        if changed == 0 {
            return Err("要更新的个人词不存在。".into());
        }
        clear_lexicon_relations(&transaction, &input.id)?;
        replace_lexicon_relations(&transaction, &input)?;
        transaction
            .commit()
            .map_err(|error| format!("无法提交个人词更新事务：{error}"))?;
        drop(connection);
        self.get_lexicon_entry(&input.id)?
            .ok_or_else(|| "个人词更新后不可见。".to_string())
    }

    pub fn get_lexicon_entry(&self, id: &str) -> Result<Option<LexiconEntry>, String> {
        let connection = self.lock_connection()?;
        load_lexicon_entry(&connection, id)
    }

    pub fn list_lexicon_entries(&self) -> Result<Vec<LexiconEntry>, String> {
        let connection = self.lock_connection()?;
        let mut statement = connection
            .prepare("SELECT id FROM lexicon_entries ORDER BY text, pinyin, id")
            .map_err(|error| format!("无法准备个人词列表：{error}"))?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("无法读取个人词列表：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法解析个人词列表：{error}"))?;
        ids.into_iter()
            .map(|id| {
                load_lexicon_entry(&connection, &id)?
                    .ok_or_else(|| "个人词列表在读取期间发生变化。".to_string())
            })
            .collect()
    }

    pub fn delete_lexicon_entry(&self, id: &str) -> Result<bool, String> {
        require_nonempty("个人词 ID", id)?;
        let connection = self.lock_connection()?;
        connection
            .execute("DELETE FROM lexicon_entries WHERE id = ?1", params![id])
            .map(|count| count > 0)
            .map_err(|error| format!("无法删除个人词：{error}"))
    }

    pub fn delete_lexicon_entries(&self, ids: Vec<String>) -> Result<usize, String> {
        if ids.is_empty() {
            return Ok(0);
        }
        let mut unique = HashSet::new();
        for id in &ids {
            require_nonempty("个人词 ID", id)?;
            if !unique.insert(id.clone()) {
                return Err("批量删除包含重复 ID。".into());
            }
        }
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始批量删除事务：{error}"))?;
        let mut deleted = 0;
        for id in &ids {
            deleted += transaction
                .execute("DELETE FROM lexicon_entries WHERE id = ?1", params![id])
                .map_err(|error| format!("无法批量删除个人词：{error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("无法提交批量删除：{error}"))?;
        Ok(deleted)
    }

    pub fn preview_lexicon_import(&self, document: &str) -> Result<LexiconImportPreview, String> {
        let entries = parse_import_document(document)?;
        self.reject_import_conflicts(&entries)?;
        Ok(LexiconImportPreview {
            entry_count: entries.len(),
            texts: entries
                .iter()
                .take(20)
                .map(|entry| entry.text.clone())
                .collect(),
        })
    }

    pub fn import_lexicon(
        &self,
        document: &str,
        now_ms: i64,
    ) -> Result<LexiconImportResult, String> {
        if now_ms < 0 {
            return Err("导入时间无效。".into());
        }
        let entries = parse_import_document(document)?;
        self.reject_import_conflicts(&entries)?;
        let batch_id = format!(
            "import-{now_ms}-{}-{}",
            std::process::id(),
            IMPORT_BATCH_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始词库导入事务：{error}"))?;
        transaction
            .execute(
                "INSERT INTO lexicon_import_batches (id, created_at_ms) VALUES (?1, ?2)",
                params![&batch_id, now_ms],
            )
            .map_err(|error| format!("无法创建词库导入批次：{error}"))?;
        for entry in &entries {
            insert_lexicon_entry_in_transaction(&transaction, entry)?;
            transaction
                .execute(
                    "INSERT INTO lexicon_import_batch_entries (batch_id, entry_id) VALUES (?1, ?2)",
                    params![&batch_id, &entry.id],
                )
                .map_err(|error| format!("无法记录词库导入批次：{error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("无法提交词库导入：{error}"))?;
        Ok(LexiconImportResult {
            batch_id,
            imported_count: entries.len(),
        })
    }

    pub fn undo_lexicon_import(&self, batch_id: &str, now_ms: i64) -> Result<usize, String> {
        require_nonempty("导入批次 ID", batch_id)?;
        if now_ms < 0 {
            return Err("撤销时间无效。".into());
        }
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始撤销导入事务：{error}"))?;
        let active = transaction
            .query_row(
                "SELECT 1 FROM lexicon_import_batches WHERE id = ?1 AND undone_at_ms IS NULL",
                params![batch_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| format!("无法读取导入批次：{error}"))?
            .is_some();
        if !active {
            return Err("导入批次不存在或已撤销。".into());
        }
        let ids = {
            let mut statement = transaction.prepare(
                "SELECT entry_id FROM lexicon_import_batch_entries WHERE batch_id = ?1 ORDER BY entry_id"
            ).map_err(|error| format!("无法准备撤销清单：{error}"))?;
            let collected = statement
                .query_map(params![batch_id], |row| row.get::<_, String>(0))
                .map_err(|error| format!("无法读取撤销清单：{error}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("无法解析撤销清单：{error}"))?;
            collected
        };
        for id in &ids {
            transaction
                .execute("DELETE FROM lexicon_entries WHERE id = ?1", params![id])
                .map_err(|error| format!("无法撤销导入词条：{error}"))?;
        }
        transaction
            .execute(
                "UPDATE lexicon_import_batches SET undone_at_ms = ?2 WHERE id = ?1",
                params![batch_id, now_ms],
            )
            .map_err(|error| format!("无法完成导入撤销：{error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("无法提交导入撤销：{error}"))?;
        Ok(ids.len())
    }

    pub fn export_lexicon(&self) -> Result<String, String> {
        let entries = self
            .list_lexicon_entries()?
            .into_iter()
            .map(|entry| LexiconEntryInput {
                id: entry.id,
                text: entry.text,
                pinyin: entry.pinyin,
                aliases: entry.aliases,
                professional_packs: entry.professional_packs,
                project_domains: entry.project_domains,
                sources: entry.sources,
                base_weight: entry.base_weight,
                fixed: entry.fixed,
                disabled: entry.disabled,
                demoted: entry.demoted,
                updated_at_ms: entry.updated_at_ms,
            })
            .collect();
        serde_json::to_string_pretty(&LexiconExportDocument {
            format_version: 1,
            entries,
        })
        .map_err(|error| format!("无法导出个人词：{error}"))
    }

    pub fn clear_learned_lexicon_entries(&self) -> Result<usize, String> {
        let connection = self.lock_connection()?;
        connection
            .execute(
                "DELETE FROM lexicon_entries
             WHERE fixed = 0 AND EXISTS (
               SELECT 1 FROM lexicon_sources s
               WHERE s.entry_id = lexicon_entries.id AND s.source_kind = 'learned'
             )",
                [],
            )
            .map_err(|error| format!("无法清理误学词：{error}"))
    }

    pub fn save_project_profile(
        &self,
        input: ProjectProfileInput,
    ) -> Result<ProjectProfile, String> {
        let input = validate_project_profile_input(input)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始项目 Profile 保存事务：{error}"))?;
        transaction
            .execute(
                "INSERT INTO project_profiles
                 (id, name, project_domain, agent_context, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                   name = excluded.name,
                   project_domain = excluded.project_domain,
                   agent_context = excluded.agent_context,
                   updated_at_ms = excluded.updated_at_ms",
                params![
                    &input.id,
                    &input.name,
                    &input.project_domain,
                    &input.agent_context,
                    input.updated_at_ms,
                ],
            )
            .map_err(|error| map_lexicon_write_error("保存项目 Profile", error))?;
        transaction
            .execute(
                "DELETE FROM project_profile_packs WHERE profile_id = ?1",
                params![&input.id],
            )
            .map_err(|error| format!("无法更新 Profile 词包：{error}"))?;
        for pack in &input.professional_packs {
            transaction
                .execute(
                    "INSERT INTO professional_packs (id, name) VALUES (?1, ?1)
                     ON CONFLICT(id) DO NOTHING",
                    params![pack],
                )
                .map_err(|error| format!("无法保存 Profile 词包：{error}"))?;
            transaction
                .execute(
                    "INSERT INTO project_profile_packs (profile_id, pack_id) VALUES (?1, ?2)",
                    params![&input.id, pack],
                )
                .map_err(|error| format!("无法关联 Profile 词包：{error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("无法提交项目 Profile：{error}"))?;
        drop(connection);
        self.get_project_profile(&input.id)?
            .ok_or_else(|| "项目 Profile 保存后不可见。".to_string())
    }

    pub fn list_project_profiles(&self) -> Result<Vec<ProjectProfile>, String> {
        let connection = self.lock_connection()?;
        let active_id: Option<String> = connection
            .query_row(
                "SELECT active_profile_id FROM project_runtime_settings WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| format!("无法读取激活项目：{error}"))?;
        let mut statement = connection
            .prepare("SELECT id FROM project_profiles ORDER BY name, id")
            .map_err(|error| format!("无法准备项目 Profile 列表：{error}"))?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("无法读取项目 Profile 列表：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法解析项目 Profile 列表：{error}"))?;
        ids.into_iter()
            .map(|id| load_project_profile(&connection, &id, active_id.as_deref()))
            .collect()
    }

    pub fn delete_project_profile(&self, id: &str) -> Result<bool, String> {
        require_nonempty("项目 Profile ID", id)?;
        let connection = self.lock_connection()?;
        connection
            .execute("DELETE FROM project_profiles WHERE id = ?1", params![id])
            .map(|count| count > 0)
            .map_err(|error| format!("无法删除项目 Profile：{error}"))
    }

    pub fn activate_project_profile(
        &self,
        id: Option<&str>,
    ) -> Result<ProjectRuntimeConfiguration, String> {
        let connection = self.lock_connection()?;
        if let Some(id) = id {
            require_nonempty("项目 Profile ID", id)?;
            let exists = connection
                .query_row(
                    "SELECT 1 FROM project_profiles WHERE id = ?1",
                    params![id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(|error| format!("无法检查项目 Profile：{error}"))?
                .is_some();
            if !exists {
                return Err("要激活的项目 Profile 不存在。".into());
            }
        }
        connection
            .execute(
                "UPDATE project_runtime_settings SET active_profile_id = ?1 WHERE singleton = 1",
                params![id],
            )
            .map_err(|error| format!("无法切换项目 Profile：{error}"))?;
        drop(connection);
        self.project_runtime_configuration()
    }

    pub fn set_secondary_layer_enabled(
        &self,
        enabled: bool,
    ) -> Result<ProjectRuntimeConfiguration, String> {
        let connection = self.lock_connection()?;
        connection
            .execute(
                "UPDATE project_runtime_settings SET secondary_layer_enabled = ?1 WHERE singleton = 1",
                params![bool_i64(enabled)],
            )
            .map_err(|error| format!("无法更新第二层候选开关：{error}"))?;
        drop(connection);
        self.project_runtime_configuration()
    }

    pub fn set_context_model_ranking_enabled(
        &self,
        enabled: bool,
    ) -> Result<ProjectRuntimeConfiguration, String> {
        let connection = self.lock_connection()?;
        connection.execute(
            "UPDATE project_runtime_settings SET context_model_ranking_enabled = ?1 WHERE singleton = 1",
            params![bool_i64(enabled)],
        ).map_err(|error| format!("无法更新上下文模型排序：{error}"))?;
        drop(connection);
        self.project_runtime_configuration()
    }

    pub fn set_prediction_enabled(
        &self,
        enabled: bool,
    ) -> Result<ProjectRuntimeConfiguration, String> {
        let connection = self.lock_connection()?;
        connection
            .execute(
                "UPDATE project_runtime_settings SET prediction_enabled = ?1 WHERE singleton = 1",
                params![bool_i64(enabled)],
            )
            .map_err(|error| format!("无法更新本地关联预测开关：{error}"))?;
        drop(connection);
        self.project_runtime_configuration()
    }

    pub fn set_candidate_theme(
        &self,
        candidate_theme: &str,
    ) -> Result<ProjectRuntimeConfiguration, String> {
        self.set_candidate_theme_from_catalog(
            candidate_theme,
            is_candidate_skin_id(candidate_theme),
        )
    }

    pub fn set_candidate_theme_from_catalog(
        &self,
        candidate_theme: &str,
        available: bool,
    ) -> Result<ProjectRuntimeConfiguration, String> {
        if !available {
            return Err("候选皮肤无效。".into());
        }
        let connection = self.lock_connection()?;
        connection
            .execute(
                "UPDATE project_runtime_settings SET candidate_theme = ?1 WHERE singleton = 1",
                params![candidate_theme],
            )
            .map_err(|error| format!("无法更新候选皮肤：{error}"))?;
        drop(connection);
        self.project_runtime_configuration()
    }

    pub fn project_runtime_configuration(&self) -> Result<ProjectRuntimeConfiguration, String> {
        let connection = self.lock_connection()?;
        let (active_profile_id, secondary_layer_enabled, prediction_enabled, candidate_theme, context_model_ranking_enabled): (
            Option<String>,
            bool,
            bool,
            String,
            bool,
        ) = connection
            .query_row(
                "SELECT active_profile_id, secondary_layer_enabled, prediction_enabled, candidate_theme, context_model_ranking_enabled
                 FROM project_runtime_settings WHERE singleton = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get::<_, i64>(1)? != 0,
                        row.get::<_, i64>(2)? != 0,
                        row.get(3)?,
                        row.get::<_, i64>(4)? != 0,
                    ))
                },
            )
            .map_err(|error| format!("无法读取项目运行时配置：{error}"))?;
        let Some(id) = active_profile_id else {
            return Ok(ProjectRuntimeConfiguration {
                active_profile_id: None,
                active_project_domain: None,
                active_professional_packs: Vec::new(),
                agent_context: String::new(),
                secondary_layer_enabled,
                prediction_enabled,
                context_model_ranking_enabled,
                candidate_theme,
            });
        };
        let profile = load_project_profile(&connection, &id, Some(&id))?;
        Ok(ProjectRuntimeConfiguration {
            active_profile_id: Some(profile.id),
            active_project_domain: Some(profile.project_domain),
            active_professional_packs: profile.professional_packs,
            agent_context: profile.agent_context,
            secondary_layer_enabled,
            prediction_enabled,
            context_model_ranking_enabled,
            candidate_theme,
        })
    }

    fn get_project_profile(&self, id: &str) -> Result<Option<ProjectProfile>, String> {
        let connection = self.lock_connection()?;
        let exists = connection
            .query_row(
                "SELECT 1 FROM project_profiles WHERE id = ?1",
                params![id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| format!("无法读取项目 Profile：{error}"))?
            .is_some();
        if !exists {
            return Ok(None);
        }
        let active_id: Option<String> = connection
            .query_row(
                "SELECT active_profile_id FROM project_runtime_settings WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| format!("无法读取激活项目：{error}"))?;
        load_project_profile(&connection, id, active_id.as_deref()).map(Some)
    }

    fn reject_import_conflicts(&self, entries: &[LexiconEntryInput]) -> Result<(), String> {
        let connection = self.lock_connection()?;
        for entry in entries {
            let conflict = connection
                .query_row(
                    "SELECT 1 FROM lexicon_entries WHERE id = ?1 OR (text = ?2 AND pinyin = ?3)",
                    params![&entry.id, &entry.text, &entry.pinyin],
                    |_| Ok(()),
                )
                .optional()
                .map_err(|error| format!("无法检查重复词条：{error}"))?
                .is_some();
            if conflict {
                return Err(format!("重复导入被拒绝：{} / {}", entry.text, entry.pinyin));
            }
        }
        Ok(())
    }

    fn migrate(&self) -> Result<(), String> {
        let mut connection = self.lock_connection()?;

        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 PRAGMA journal_mode = WAL;
                 CREATE TABLE IF NOT EXISTS schema_migrations (
                   version INTEGER PRIMARY KEY,
                   applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                 );",
            )
            .map_err(|error| format!("无法初始化迁移表：{error}"))?;

        for (version, sql) in MIGRATIONS {
            let applied = connection
                .query_row(
                    "SELECT 1 FROM schema_migrations WHERE version = ?1",
                    params![version],
                    |_| Ok(()),
                )
                .optional()
                .map_err(|error| format!("无法读取迁移状态：{error}"))?
                .is_some();

            if applied {
                continue;
            }

            let transaction = connection
                .transaction()
                .map_err(|error| format!("无法开始迁移：{error}"))?;
            transaction
                .execute_batch(sql)
                .map_err(|error| format!("无法执行迁移 {version}：{error}"))?;
            transaction
                .execute(
                    "INSERT INTO schema_migrations (version) VALUES (?1)",
                    params![version],
                )
                .map_err(|error| format!("无法记录迁移 {version}：{error}"))?;
            transaction
                .commit()
                .map_err(|error| format!("无法提交迁移 {version}：{error}"))?;
        }

        Ok(())
    }

    pub(crate) fn lock_connection(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.connection
            .lock()
            .map_err(|_| "SQLite 连接锁已损坏。".to_string())
    }
}

fn save_app_state_in_transaction(
    connection: &mut Connection,
    state: &AppState,
    provider_settings: &PersistedProviderSettings,
) -> Result<(), String> {
    let transaction = connection
        .transaction()
        .map_err(|error| format!("无法开始保存事务：{error}"))?;

    save_history(&transaction, &state.history)?;
    save_user_rules(&transaction, &state.user_rules)?;
    set_setting(&transaction, SELECTED_MODE_KEY, &state.selected_mode_id)?;
    set_setting(&transaction, PROVIDER_SETTINGS_KEY, provider_settings)?;
    set_setting(
        &transaction,
        SHORTCUT_SETTINGS_KEY,
        &state.shortcut_settings,
    )?;
    set_setting(&transaction, INITIALIZED_KEY, &true)?;

    transaction
        .commit()
        .map_err(|error| format!("无法提交保存事务：{error}"))
}

fn load_history(connection: &Connection) -> Result<Vec<HistoryItem>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, raw_input, mode_id, target_tool, output, favorite, created_at
             FROM history
             ORDER BY position ASC, created_at DESC
             LIMIT 50",
        )
        .map_err(|error| format!("无法读取历史记录：{error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(HistoryItem {
                id: row.get(0)?,
                raw_input: row.get(1)?,
                mode_id: row.get(2)?,
                target_tool: row.get(3)?,
                output: row.get(4)?,
                favorite: row.get::<_, i64>(5)? != 0,
                created_at: row.get(6)?,
            })
        })
        .map_err(|error| format!("无法读取历史记录：{error}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法解析历史记录：{error}"))
}

fn save_history(transaction: &Transaction<'_>, history: &[HistoryItem]) -> Result<(), String> {
    transaction
        .execute("DELETE FROM history", [])
        .map_err(|error| format!("无法清空历史记录：{error}"))?;

    {
        let mut statement = transaction
            .prepare(
                "INSERT INTO history (
                   id, raw_input, mode_id, target_tool, output, favorite, created_at, position
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .map_err(|error| format!("无法准备历史记录保存：{error}"))?;

        for (position, item) in history.iter().take(50).enumerate() {
            statement
                .execute(params![
                    &item.id,
                    &item.raw_input,
                    &item.mode_id,
                    item.target_tool.as_deref(),
                    &item.output,
                    if item.favorite { 1_i64 } else { 0_i64 },
                    &item.created_at,
                    position as i64
                ])
                .map_err(|error| format!("无法保存历史记录：{error}"))?;
        }
    }

    Ok(())
}

fn load_user_rules(connection: &Connection) -> Result<Vec<UserRule>, String> {
    let mut statement = connection
        .prepare("SELECT id, text, enabled FROM user_rules ORDER BY position ASC")
        .map_err(|error| format!("无法读取长期规则：{error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(UserRule {
                id: row.get(0)?,
                text: row.get(1)?,
                enabled: row.get::<_, i64>(2)? != 0,
            })
        })
        .map_err(|error| format!("无法读取长期规则：{error}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法解析长期规则：{error}"))
}

fn save_user_rules(transaction: &Transaction<'_>, rules: &[UserRule]) -> Result<(), String> {
    transaction
        .execute("DELETE FROM user_rules", [])
        .map_err(|error| format!("无法清空长期规则：{error}"))?;

    {
        let mut statement = transaction
            .prepare(
                "INSERT INTO user_rules (id, text, enabled, position)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(|error| format!("无法准备长期规则保存：{error}"))?;

        for (position, rule) in rules.iter().enumerate() {
            statement
                .execute(params![
                    &rule.id,
                    &rule.text,
                    if rule.enabled { 1_i64 } else { 0_i64 },
                    position as i64
                ])
                .map_err(|error| format!("无法保存长期规则：{error}"))?;
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct PersonalProfileAutoUpdateRow {
    id: String,
    entry_id: String,
    pinyin: String,
    text: String,
    friction_count: i64,
    runner_up_friction_count: i64,
    latest_friction_at_ms: i64,
    applied_at_ms: i64,
    last_refreshed_at_ms: i64,
    status: String,
    status_changed_at_ms: i64,
    baseline_entry_json: Option<String>,
    applied_entry_json: String,
    prediction_eligible: bool,
    evidence_kind: String,
    evidence_json: Option<String>,
}

fn load_auto_update_rows(
    connection: &Connection,
    status: Option<&str>,
) -> Result<Vec<PersonalProfileAutoUpdateRow>, String> {
    let sql = if status.is_some() {
        "SELECT id,entry_id,pinyin,text,friction_count,runner_up_friction_count,
                latest_friction_at_ms,applied_at_ms,last_refreshed_at_ms,status,
                status_changed_at_ms,baseline_entry_json,applied_entry_json,prediction_eligible,
                evidence_kind,evidence_json
         FROM personal_profile_auto_updates WHERE status=?1 ORDER BY pinyin,text,id"
    } else {
        "SELECT id,entry_id,pinyin,text,friction_count,runner_up_friction_count,
                latest_friction_at_ms,applied_at_ms,last_refreshed_at_ms,status,
                status_changed_at_ms,baseline_entry_json,applied_entry_json,prediction_eligible,
                evidence_kind,evidence_json
         FROM personal_profile_auto_updates ORDER BY pinyin,text,id"
    };
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| format!("无法准备档案自更新列表：{error}"))?;
    let decode = |row: &rusqlite::Row<'_>| {
        Ok(PersonalProfileAutoUpdateRow {
            id: row.get(0)?,
            entry_id: row.get(1)?,
            pinyin: row.get(2)?,
            text: row.get(3)?,
            friction_count: row.get(4)?,
            runner_up_friction_count: row.get(5)?,
            latest_friction_at_ms: row.get(6)?,
            applied_at_ms: row.get(7)?,
            last_refreshed_at_ms: row.get(8)?,
            status: row.get(9)?,
            status_changed_at_ms: row.get(10)?,
            baseline_entry_json: row.get(11)?,
            applied_entry_json: row.get(12)?,
            prediction_eligible: row.get::<_, i64>(13)? != 0,
            evidence_kind: row.get(14)?,
            evidence_json: row.get(15)?,
        })
    };
    let rows = if let Some(status) = status {
        statement.query_map(params![status], decode)
    } else {
        statement.query_map([], decode)
    }
    .map_err(|error| format!("无法读取档案自更新列表：{error}"))?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|error| format!("无法解析档案自更新列表：{error}"))?;
    Ok(rows)
}

fn load_auto_update_row_by_pair(
    connection: &Connection,
    pinyin: &str,
    text: &str,
) -> Result<Option<PersonalProfileAutoUpdateRow>, String> {
    Ok(load_auto_update_rows(connection, None)?
        .into_iter()
        .find(|row| row.pinyin == pinyin && row.text == text))
}

fn load_auto_update_row_by_id(
    connection: &Connection,
    id: &str,
) -> Result<Option<PersonalProfileAutoUpdateRow>, String> {
    Ok(load_auto_update_rows(connection, None)?
        .into_iter()
        .find(|row| row.id == id))
}

fn auto_update_item(row: PersonalProfileAutoUpdateRow) -> PersonalProfileAutoUpdateItem {
    let owner_declared_candidates = if row.evidence_kind == "owner_declared_disambiguation" {
        row.evidence_json
            .as_deref()
            .and_then(|value| serde_json::from_str::<OwnerDeclaredEvidence>(value).ok())
            .map(|evidence| evidence.candidates)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    PersonalProfileAutoUpdateItem {
        id: row.id,
        entry_id: row.entry_id,
        pinyin: row.pinyin,
        text: row.text,
        friction_count: row.friction_count as usize,
        runner_up_friction_count: row.runner_up_friction_count as usize,
        latest_friction_at_ms: row.latest_friction_at_ms,
        applied_at_ms: row.applied_at_ms,
        last_refreshed_at_ms: row.last_refreshed_at_ms,
        status: row.status,
        status_changed_at_ms: row.status_changed_at_ms,
        prediction_eligible: row.prediction_eligible,
        decay_after_ms: PERSONAL_PROFILE_DECAY_AFTER_MS,
        evidence_kind: row.evidence_kind,
        owner_declared_candidates,
    }
}

fn lexicon_entry_input(entry: LexiconEntry) -> LexiconEntryInput {
    LexiconEntryInput {
        id: entry.id,
        text: entry.text,
        pinyin: entry.pinyin,
        aliases: entry.aliases,
        professional_packs: entry.professional_packs,
        project_domains: entry.project_domains,
        sources: entry.sources,
        base_weight: entry.base_weight,
        fixed: entry.fixed,
        disabled: entry.disabled,
        demoted: entry.demoted,
        updated_at_ms: entry.updated_at_ms,
    }
}

fn current_auto_entry_input(
    connection: &Connection,
    id: &str,
) -> Result<Option<LexiconEntryInput>, String> {
    load_lexicon_entry(connection, id).map(|entry| entry.map(lexicon_entry_input))
}

fn load_lexicon_entry_by_pair(
    connection: &Connection,
    pinyin: &str,
    text: &str,
) -> Result<Option<LexiconEntry>, String> {
    let id = connection
        .query_row(
            "SELECT id FROM lexicon_entries WHERE pinyin=?1 AND text=?2",
            params![pinyin, text],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("无法检查档案自更新个人词：{error}"))?;
    id.map(|id| load_lexicon_entry(connection, &id))
        .transpose()
        .map(Option::flatten)
}

fn decode_optional_entry(value: &Option<String>) -> Result<Option<LexiconEntryInput>, String> {
    value
        .as_ref()
        .map(|value| {
            serde_json::from_str(value).map_err(|_| "档案自更新基线快照损坏。".to_string())
        })
        .transpose()
}

fn ensure_restored_baseline(
    connection: &Connection,
    entry_id: &str,
    baseline: Option<&LexiconEntryInput>,
) -> Result<(), String> {
    let current = current_auto_entry_input(connection, entry_id)?;
    if current.as_ref() != baseline {
        return Err("已回落的个人词基线被外部修改，档案自更新失败关闭。".into());
    }
    Ok(())
}

fn auto_applied_entry(
    entry_id: &str,
    baseline: Option<&LexiconEntryInput>,
    target: &ConfidentProfileTarget,
    evidence: &ProfileUpdateEvidence,
    now_ms: i64,
) -> Result<LexiconEntryInput, String> {
    let mut input = baseline.cloned().unwrap_or_else(|| LexiconEntryInput {
        id: entry_id.to_string(),
        text: target.text.clone(),
        pinyin: target.pinyin.clone(),
        aliases: Vec::new(),
        professional_packs: Vec::new(),
        project_domains: Vec::new(),
        sources: Vec::new(),
        base_weight: PERSONAL_PROFILE_AUTO_WEIGHT,
        fixed: false,
        disabled: false,
        demoted: false,
        updated_at_ms: now_ms,
    });
    input.sources.retain(|source| {
        !matches!(
            source.source_kind.as_str(),
            "personal-friction-auto" | "owner-declared-disambiguation"
        )
    });
    let (source_kind, source) = match evidence {
        ProfileUpdateEvidence::Friction => (
            "personal-friction-auto",
            format!(
                "Quality/events.jsonl#friction={};runnerUp={};latest={}",
                target.friction_count,
                target.runner_up_friction_count,
                target.latest_friction_at_ms
            ),
        ),
        ProfileUpdateEvidence::OwnerDeclared(_) => (
            "owner-declared-disambiguation",
            "owner_declared_disambiguation#manager-panel".into(),
        ),
    };
    input.sources.push(LexiconSource {
        source_kind: source_kind.into(),
        source,
        license: "private-user-data".into(),
        confidence_millis: 1000,
    });
    input.base_weight = input.base_weight.max(PERSONAL_PROFILE_AUTO_WEIGHT);
    input.updated_at_ms = now_ms.saturating_add(target.friction_count.min(999) as i64);
    validate_lexicon_input(input)
}

fn write_lexicon_entry(
    transaction: &Transaction<'_>,
    input: &LexiconEntryInput,
) -> Result<(), String> {
    if current_auto_entry_input(transaction, &input.id)?.is_some() {
        transaction
            .execute(
                "UPDATE lexicon_entries SET text=?2,pinyin=?3,base_weight=?4,fixed=?5,
                 disabled=?6,demoted=?7,updated_at_ms=?8 WHERE id=?1",
                params![
                    &input.id,
                    &input.text,
                    &input.pinyin,
                    input.base_weight,
                    bool_i64(input.fixed),
                    bool_i64(input.disabled),
                    bool_i64(input.demoted),
                    input.updated_at_ms,
                ],
            )
            .map_err(|error| map_lexicon_write_error("应用档案自更新", error))?;
        clear_lexicon_relations(transaction, &input.id)?;
        replace_lexicon_relations(transaction, input)
    } else {
        insert_lexicon_entry_in_transaction(transaction, input)
    }
}

fn restore_auto_entry(
    transaction: &Transaction<'_>,
    row: &PersonalProfileAutoUpdateRow,
) -> Result<(), String> {
    let expected: LexiconEntryInput = serde_json::from_str(&row.applied_entry_json)
        .map_err(|_| "档案自更新结果快照损坏。".to_string())?;
    let current = current_auto_entry_input(transaction, &row.entry_id)?
        .ok_or_else(|| "待回落的自动个人词已消失，失败关闭。".to_string())?;
    if current != expected {
        return Err("待回落的自动个人词被外部修改，失败关闭。".into());
    }
    if let Some(baseline) = decode_optional_entry(&row.baseline_entry_json)? {
        write_lexicon_entry(transaction, &baseline)
    } else {
        transaction
            .execute(
                "DELETE FROM lexicon_entries WHERE id=?1",
                params![&row.entry_id],
            )
            .map_err(|error| format!("无法回落自动个人词：{error}"))?;
        Ok(())
    }
}

fn parse_import_document(document: &str) -> Result<Vec<LexiconEntryInput>, String> {
    if document.len() > 8 * 1024 * 1024 {
        return Err("导入文件超过 8 MiB 上限。".into());
    }
    // Browser File.text() replaces malformed UTF-8 bytes with U+FFFD. Reject
    // that lossy decode marker before JSON parsing so a damaged import cannot
    // silently become authoritative lexicon data.
    if document.contains('\u{fffd}') {
        return Err("导入文件包含无效 UTF-8 编码。".into());
    }
    let parsed: LexiconExportDocument = serde_json::from_str(document)
        .map_err(|_| "导入文件不是有效的 UTF-8 JSON 词库。".to_string())?;
    if parsed.format_version != 1 {
        return Err("不支持这个词库导入版本。".into());
    }
    if parsed.entries.is_empty() {
        return Err("导入文件不含词条。".into());
    }
    if parsed.entries.len() > 10_000 {
        return Err("单次导入最多 10000 条。".into());
    }
    let entries = parsed
        .entries
        .into_iter()
        .map(validate_lexicon_input)
        .collect::<Result<Vec<_>, _>>()?;
    let mut ids = HashSet::new();
    let mut keys = HashSet::new();
    for entry in &entries {
        if !ids.insert(entry.id.clone()) || !keys.insert((entry.text.clone(), entry.pinyin.clone()))
        {
            return Err("导入文件内部包含重复 ID 或 text+pinyin。".into());
        }
    }
    Ok(entries)
}

pub(crate) fn insert_lexicon_entry_in_transaction(
    transaction: &Transaction<'_>,
    input: &LexiconEntryInput,
) -> Result<(), String> {
    transaction
        .execute(
            "INSERT INTO lexicon_entries
         (id, text, pinyin, base_weight, fixed, disabled, demoted, created_at_ms, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![
                &input.id,
                &input.text,
                &input.pinyin,
                input.base_weight,
                bool_i64(input.fixed),
                bool_i64(input.disabled),
                bool_i64(input.demoted),
                input.updated_at_ms,
            ],
        )
        .map_err(|error| map_lexicon_write_error("导入个人词", error))?;
    replace_lexicon_relations(transaction, input)
}

pub(crate) fn validate_lexicon_input(
    mut input: LexiconEntryInput,
) -> Result<LexiconEntryInput, String> {
    input.id = required_trimmed("个人词 ID", input.id)?;
    input.text = required_trimmed("词条文本", input.text)?;
    input.pinyin = required_trimmed("拼音", input.pinyin)?;
    if input.updated_at_ms < 0 {
        return Err("个人词更新时间不能为负数。".into());
    }
    if input.sources.is_empty() {
        return Err("个人词至少需要一个来源，source 不可为空。".into());
    }

    input.aliases = validate_named_list("别名", input.aliases)?;
    input.professional_packs = validate_named_list("专业词包", input.professional_packs)?;
    input.project_domains = validate_named_list("项目域", input.project_domains)?;

    let mut source_keys = HashSet::new();
    for source in &mut input.sources {
        source.source_kind = required_trimmed("来源类型", std::mem::take(&mut source.source_kind))?;
        source.source = required_trimmed("source", std::mem::take(&mut source.source))?;
        source.license = required_trimmed("license", std::mem::take(&mut source.license))?;
        if !(0..=1000).contains(&source.confidence_millis) {
            return Err("来源置信度必须在 0 到 1000 之间。".into());
        }
        let key = (
            source.source_kind.clone(),
            source.source.clone(),
            source.license.clone(),
        );
        if !source_keys.insert(key) {
            return Err("个人词来源存在重复项。".into());
        }
    }
    input.sources.sort_by(|left, right| {
        (&left.source_kind, &left.source, &left.license).cmp(&(
            &right.source_kind,
            &right.source,
            &right.license,
        ))
    });
    Ok(input)
}

fn validate_project_profile_input(
    mut input: ProjectProfileInput,
) -> Result<ProjectProfileInput, String> {
    input.id = required_trimmed("项目 Profile ID", input.id)?;
    input.name = required_trimmed("项目名称", input.name)?;
    input.project_domain = required_trimmed("项目域", input.project_domain)?;
    input.professional_packs = validate_named_list("专业词包", input.professional_packs)?;
    input.agent_context = input.agent_context.trim().to_string();
    if input.agent_context.len() > 32 * 1024 {
        return Err("Agent 上下文超过 32 KiB 上限。".into());
    }
    if input.updated_at_ms < 0 {
        return Err("项目 Profile 更新时间不能为负数。".into());
    }
    Ok(input)
}

fn validate_named_list(label: &str, values: Vec<String>) -> Result<Vec<String>, String> {
    let mut normalized = Vec::with_capacity(values.len());
    let mut seen = HashSet::new();
    for value in values {
        let value = required_trimmed(label, value)?;
        if !seen.insert(value.clone()) {
            return Err(format!("{label}存在重复项。"));
        }
        normalized.push(value);
    }
    normalized.sort();
    Ok(normalized)
}

fn required_trimmed(label: &str, value: String) -> Result<String, String> {
    let trimmed = value.trim();
    require_nonempty(label, trimmed)?;
    Ok(trimmed.to_string())
}

fn require_nonempty(label: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{label}不可为空。"))
    } else {
        Ok(())
    }
}

fn bool_i64(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

fn map_lexicon_write_error(action: &str, error: rusqlite::Error) -> String {
    if matches!(
        &error,
        rusqlite::Error::SqliteFailure(details, _)
            if details.code == rusqlite::ErrorCode::ConstraintViolation
    ) {
        format!("{action}发生冲突或违反数据约束：{error}")
    } else {
        format!("无法{action}：{error}")
    }
}

fn clear_lexicon_relations(transaction: &Transaction<'_>, entry_id: &str) -> Result<(), String> {
    for table in [
        "lexicon_aliases",
        "professional_pack_entries",
        "lexicon_entry_domains",
        "lexicon_sources",
    ] {
        transaction
            .execute(
                &format!("DELETE FROM {table} WHERE entry_id = ?1"),
                params![entry_id],
            )
            .map_err(|error| format!("无法更新个人词关联数据：{error}"))?;
    }
    Ok(())
}

fn replace_lexicon_relations(
    transaction: &Transaction<'_>,
    input: &LexiconEntryInput,
) -> Result<(), String> {
    for alias in &input.aliases {
        transaction
            .execute(
                "INSERT INTO lexicon_aliases (entry_id, alias) VALUES (?1, ?2)",
                params![&input.id, alias],
            )
            .map_err(|error| map_lexicon_write_error("保存个人词别名", error))?;
    }
    for pack in &input.professional_packs {
        transaction
            .execute(
                "INSERT INTO professional_packs (id, name) VALUES (?1, ?1)
                 ON CONFLICT(id) DO NOTHING",
                params![pack],
            )
            .map_err(|error| map_lexicon_write_error("保存专业词包", error))?;
        transaction
            .execute(
                "INSERT INTO professional_pack_entries (pack_id, entry_id) VALUES (?1, ?2)",
                params![pack, &input.id],
            )
            .map_err(|error| map_lexicon_write_error("关联专业词包", error))?;
    }
    for domain in &input.project_domains {
        transaction
            .execute(
                "INSERT INTO project_domains (id, name) VALUES (?1, ?1)
                 ON CONFLICT(id) DO NOTHING",
                params![domain],
            )
            .map_err(|error| map_lexicon_write_error("保存项目域", error))?;
        transaction
            .execute(
                "INSERT INTO lexicon_entry_domains (entry_id, domain_id) VALUES (?1, ?2)",
                params![&input.id, domain],
            )
            .map_err(|error| map_lexicon_write_error("关联项目域", error))?;
    }
    for source in &input.sources {
        transaction
            .execute(
                "INSERT INTO lexicon_sources
                 (entry_id, source_kind, source, license, confidence_millis)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    &input.id,
                    &source.source_kind,
                    &source.source,
                    &source.license,
                    source.confidence_millis,
                ],
            )
            .map_err(|error| map_lexicon_write_error("保存个人词来源", error))?;
    }
    Ok(())
}

fn load_lexicon_entry(connection: &Connection, id: &str) -> Result<Option<LexiconEntry>, String> {
    let base = connection
        .query_row(
            "SELECT id, text, pinyin, base_weight, fixed, disabled, demoted, created_at_ms, updated_at_ms
             FROM lexicon_entries WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)? != 0,
                    row.get::<_, i64>(5)? != 0,
                    row.get::<_, i64>(6)? != 0,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("无法读取个人词：{error}"))?;
    let Some((
        id,
        text,
        pinyin,
        base_weight,
        fixed,
        disabled,
        demoted,
        created_at_ms,
        updated_at_ms,
    )) = base
    else {
        return Ok(None);
    };

    let aliases = load_string_column(
        connection,
        "SELECT alias FROM lexicon_aliases WHERE entry_id = ?1 ORDER BY alias",
        &id,
    )?;
    let professional_packs = load_string_column(
        connection,
        "SELECT p.name FROM professional_packs p
         JOIN professional_pack_entries pe ON pe.pack_id = p.id
         WHERE pe.entry_id = ?1 ORDER BY p.name",
        &id,
    )?;
    let project_domains = load_string_column(
        connection,
        "SELECT d.name FROM project_domains d
         JOIN lexicon_entry_domains ed ON ed.domain_id = d.id
         WHERE ed.entry_id = ?1 ORDER BY d.name",
        &id,
    )?;
    let mut statement = connection
        .prepare(
            "SELECT source_kind, source, license, confidence_millis
             FROM lexicon_sources WHERE entry_id = ?1
             ORDER BY source_kind, source, license",
        )
        .map_err(|error| format!("无法准备个人词来源读取：{error}"))?;
    let sources = statement
        .query_map(params![&id], |row| {
            Ok(LexiconSource {
                source_kind: row.get(0)?,
                source: row.get(1)?,
                license: row.get(2)?,
                confidence_millis: row.get(3)?,
            })
        })
        .map_err(|error| format!("无法读取个人词来源：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法解析个人词来源：{error}"))?;

    Ok(Some(LexiconEntry {
        id,
        text,
        pinyin,
        aliases,
        professional_packs,
        project_domains,
        sources,
        base_weight,
        fixed,
        disabled,
        demoted,
        created_at_ms,
        updated_at_ms,
    }))
}

fn load_string_column(
    connection: &Connection,
    query: &str,
    id: &str,
) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(query)
        .map_err(|error| format!("无法准备个人词关联读取：{error}"))?;
    let values = statement
        .query_map(params![id], |row| row.get::<_, String>(0))
        .map_err(|error| format!("无法读取个人词关联：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法解析个人词关联：{error}"))?;
    Ok(values)
}

fn load_project_profile(
    connection: &Connection,
    id: &str,
    active_id: Option<&str>,
) -> Result<ProjectProfile, String> {
    let (id, name, project_domain, agent_context, created_at_ms, updated_at_ms): (
        String,
        String,
        String,
        String,
        i64,
        i64,
    ) = connection
        .query_row(
            "SELECT id, name, project_domain, agent_context, created_at_ms, updated_at_ms
             FROM project_profiles WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .map_err(|error| format!("无法读取项目 Profile：{error}"))?;
    let professional_packs = load_string_column(
        connection,
        "SELECT p.name FROM professional_packs p
         JOIN project_profile_packs pp ON pp.pack_id = p.id
         WHERE pp.profile_id = ?1 ORDER BY p.name",
        &id,
    )?;
    Ok(ProjectProfile {
        active: active_id == Some(id.as_str()),
        id,
        name,
        project_domain,
        professional_packs,
        agent_context,
        created_at_ms,
        updated_at_ms,
    })
}

fn get_setting<T: DeserializeOwned>(
    connection: &Connection,
    key: &str,
) -> Result<Option<T>, String> {
    get_setting_text(connection, key)?
        .map(|text| {
            serde_json::from_str::<T>(&text).map_err(|error| format!("无法解析设置 {key}：{error}"))
        })
        .transpose()
}

fn get_setting_text(connection: &Connection, key: &str) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("无法读取设置 {key}：{error}"))
}

fn set_setting<T: Serialize>(
    transaction: &Transaction<'_>,
    key: &str,
    value: &T,
) -> Result<(), String> {
    let text =
        serde_json::to_string(value).map_err(|error| format!("无法序列化设置 {key}：{error}"))?;

    transaction
        .execute(
            "INSERT INTO settings (key, value, updated_at)
             VALUES (?1, ?2, CURRENT_TIMESTAMP)
             ON CONFLICT(key) DO UPDATE SET
               value = excluded.value,
               updated_at = CURRENT_TIMESTAMP",
            params![key, text],
        )
        .map(|_| ())
        .map_err(|error| format!("无法保存设置 {key}：{error}"))
}

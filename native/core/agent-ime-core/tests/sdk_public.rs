use agent_ime_core::{
    sdk::{Engine, Error, KeyEvent, KeyKind, Security},
    zhixu::learning,
};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
struct Fixture {
    root: PathBuf,
    shared: PathBuf,
    user: PathBuf,
}
impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "zhixu-sdk-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            label
        ));
        let shared = root.join("dictionary");
        let user = root.join("profile");
        fs::create_dir_all(&shared).unwrap();
        fs::create_dir(&user).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [&root, &shared, &user] {
                fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        fs::write(shared.join("agent_pinyin.dict.yaml"), "---\nname: agent_pinyin\nversion: test\nsort: by_weight\n...\n你好\tni hao\t1000\n拟好\tni hao\t10\n中国\tzhong guo\t1000\n").unwrap();
        Self { root, shared, user }
    }
    fn engine(&self) -> Engine {
        Engine::open(&self.shared, &self.user, "org.zhixu.sdk.test").unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn key(kind: KeyKind, text: &str, id: u64) -> KeyEvent<'_> {
    KeyEvent {
        kind,
        text,
        modifiers: 0,
        security: Security::NonSecure,
        delivery_id: id,
    }
}
fn type_text(engine: &mut Engine, text: &str) -> agent_ime_core::sdk::Outcome {
    for (i, c) in text.chars().enumerate() {
        engine
            .process(key(KeyKind::Text, &c.to_string(), i as u64 + 1))
            .unwrap();
    }
    engine.snapshot().unwrap()
}
#[test]
fn real_pinyin_commit_and_owned_page_survive_native_output_release() {
    let _guard = serial();
    let fixture = Fixture::new("commit");
    let mut engine = fixture.engine();
    let page = type_text(&mut engine, "nihao");
    assert_eq!(page.candidates[0].text, "你好");
    assert_eq!(page.candidates[0].reading_key, "ni hao");
    let committed = engine.process(key(KeyKind::Space, "", 10)).unwrap();
    assert_eq!(committed.commit_text, page.candidates[0].text);
    assert!(engine.snapshot().unwrap().marked_text.is_empty());
    drop(engine);
    assert_eq!(page.candidates[0].text, "你好");
    assert!(!page.marked_text.is_empty());
}
#[test]
fn unknown_security_never_becomes_ordinary_input_and_reload_preserves_composition() {
    let _guard = serial();
    let fixture = Fixture::new("security");
    let mut engine = fixture.engine();
    assert_eq!(Security::default(), Security::Unknown);
    let out = engine
        .process(KeyEvent {
            security: Security::Unknown,
            ..key(KeyKind::Text, "n", 1)
        })
        .unwrap();
    assert!(!out.consumed && out.secure_degraded && out.marked_text.is_empty());
    let page = type_text(&mut engine, "nihao");
    assert_eq!(engine.reload_dictionaries(), Err(Error::Api(-10)));
    let mut after = engine.snapshot().unwrap();
    let mut before = page;
    // Per-read timing varies; every page/state field must remain identical.
    after.core_elapsed_ns = 0;
    before.core_elapsed_ns = 0;
    assert_eq!(after, before);
    engine.process(key(KeyKind::Escape, "", 20)).unwrap();
    engine.reload_dictionaries().unwrap();
}
#[test]
fn deployment_lease_allows_same_profile_and_releases_only_after_last_engine() {
    let _guard = serial();
    let a = Fixture::new("lease-a");
    let b = Fixture::new("lease-b");
    let first = a.engine();
    let second = a.engine();
    assert!(matches!(
        Engine::open(&b.shared, &b.user, "b"),
        Err(Error::DeploymentConflict)
    ));
    drop(first);
    assert!(matches!(
        Engine::open(&b.shared, &b.user, "b"),
        Err(Error::DeploymentConflict)
    ));
    drop(second);
    let mut other = b.engine();
    assert_eq!(type_text(&mut other, "zhongguo").candidates[0].text, "中国");
}
#[test]
fn invalid_create_does_not_claim_a_deployment_or_create_user_data() {
    let _guard = serial();
    let bad = Fixture::new("invalid");
    fs::remove_file(bad.shared.join("agent_pinyin.dict.yaml")).unwrap();
    assert!(matches!(
        Engine::open(&bad.shared, &bad.user, "bad"),
        Err(Error::CreateFailed)
    ));
    let good = Fixture::new("recovery");
    let _engine = good.engine();
    assert!(Engine::open(std::path::Path::new("relative"), &good.user, "bad").is_err());
}
#[test]
fn corrected_blacklist_key_maps_to_real_core_behavior_and_version_is_available() {
    let _guard = serial();
    let fixture = Fixture::new("abi");
    let mut engine = fixture.engine();
    assert_eq!(agent_ime_core::aim_core_abi_version(), 6);
    // SAFETY: this API returns one immutable, static NUL-terminated string.
    assert_eq!(
        unsafe { std::ffi::CStr::from_ptr(agent_ime_core::aim_core_version()) }
            .to_str()
            .unwrap(),
        env!("CARGO_PKG_VERSION")
    );
    let page = type_text(&mut engine, "nihao");
    let target = page.candidates[0].text.clone();
    let out = engine
        .process(KeyEvent {
            kind: KeyKind::BlacklistCandidate,
            modifiers: 3,
            ..key(KeyKind::Text, "", 99)
        })
        .unwrap();
    assert_eq!(out.status, 0);
    assert!(!out.candidates.iter().any(|c| c.text == target));
    engine.process(key(KeyKind::Escape, "", 100)).unwrap();
    assert!(
        !type_text(&mut engine, "nihao")
            .candidates
            .iter()
            .any(|c| c.text == target)
    );
}
#[test]
fn bounded_learning_backup_merge_and_rejected_import_keep_existing_data() {
    let _guard = serial();
    let fixture = Fixture::new("learning");
    let mut store = learning::LearningStore::load(&fixture.user, 100);
    store.record("ni hao", "你好", 100);
    store.save(&fixture.user).unwrap();
    let backup = learning::management_export(&fixture.user).unwrap();
    let before = fs::read(fixture.user.join(learning::FILE_NAME)).unwrap();
    let preview = learning::management_preview(&fixture.user, &backup).unwrap();
    assert_eq!(preview.entries, 1);
    learning::management_import(&fixture.user, &backup).unwrap();
    learning::management_import(&fixture.user, &backup).unwrap();
    assert_eq!(
        learning::management_list(&fixture.user).unwrap().items[0].entries[0].count,
        1
    );
    assert!(learning::management_import(&fixture.user, "{invalid").is_err());
    assert!(learning::management_import(&fixture.user, &" ".repeat(16 * 1024 * 1024 + 1)).is_err());
    assert_eq!(
        before,
        fs::read(fixture.user.join(learning::FILE_NAME)).unwrap()
    );
}

#[test]
fn external_scorer_receives_valid_tokens_obeys_zero_budget_and_keeps_page() {
    let _guard = serial();
    let fixture = Fixture::new("scoring");
    fs::write(fixture.shared.join("agent_pinyin.dict.yaml"), "---\nname: agent_pinyin\nsort: by_weight\n...\n我\two\t100\n们\tmen\t100\n想\txiang\t100\n吃\tchi\t100\n我们\two men\t100\n").unwrap();
    let vocabulary = fixture.root.join("itos.json");
    fs::write(
        &vocabulary,
        serde_json::to_vec(&["<pad>", "<bos>", "<unk>", "<chat>", "我", "们", "想", "吃"]).unwrap(),
    )
    .unwrap();
    let mut engine = fixture.engine();
    let mut before = type_text(&mut engine, "wmxc");
    #[derive(Default)]
    struct Calls {
        count: usize,
    }
    unsafe extern "C" fn uniform(
        user: *mut std::ffi::c_void,
        tokens: *const i32,
        rows: usize,
        cols: usize,
        output: *mut f32,
        vocab: usize,
    ) -> i32 {
        if user.is_null()
            || tokens.is_null()
            || output.is_null()
            || rows == 0
            || cols != 40
            || vocab != 8
        {
            return -1;
        }
        // SAFETY: the core supplies initialized borrowed input/output arrays of
        // these declared dimensions; state is exclusive for this synchronous call.
        unsafe {
            let ids = std::slice::from_raw_parts(tokens, rows * cols);
            if ids.iter().any(|id| *id < 0 || *id as usize >= vocab) {
                return -1;
            }
            (*(user as *mut Calls)).count += 1;
            std::slice::from_raw_parts_mut(output, rows * vocab).fill(-(vocab as f32).ln());
        }
        0
    }
    let path = vocabulary.to_str().unwrap();
    let mut calls = Calls::default();
    let mut output = [0u8; 16_384];
    let mut length = output.len();
    // SAFETY: engine stays live/exclusive; vocabulary path is borrowed UTF-8;
    // callback state and output remain initialized and alive throughout.
    unsafe {
        let session = engine.as_raw();
        assert_eq!(
            agent_ime_core::aim_session_set_zhiju_vocab(session, path.as_ptr(), path.len()),
            0
        );
        assert_eq!(
            agent_ime_core::aim_session_zhiju_search(
                session,
                Some(uniform),
                (&mut calls as *mut Calls).cast(),
                8,
                0,
                output.as_mut_ptr(),
                &mut length
            ),
            0
        );
        assert_eq!(calls.count, 0);
        assert_eq!(length, 0);
        length = output.len();
        assert_eq!(
            agent_ime_core::aim_session_zhiju_search(
                session,
                Some(uniform),
                (&mut calls as *mut Calls).cast(),
                8,
                40,
                output.as_mut_ptr(),
                &mut length
            ),
            0
        );
    }
    assert!(calls.count > 0 && length > 0);
    let completed = std::str::from_utf8(&output[..length]).unwrap();
    assert!(completed.lines().all(|row| row.split('\t').count() == 3));
    let mut after = engine.snapshot().unwrap();
    before.core_elapsed_ns = 0;
    after.core_elapsed_ns = 0;
    assert_eq!(
        after, before,
        "search callback cannot alter the displayed page"
    );
}

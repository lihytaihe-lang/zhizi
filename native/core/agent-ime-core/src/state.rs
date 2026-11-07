pub const MOD_SHIFT: u32 = 1 << 0;
/// The expanded panel shows the current page plus at most three more rows.
pub const MAX_PEEK_PAGES: usize = 3;
pub const MOD_CONTROL: u32 = 1 << 1;
pub const MOD_OPTION: u32 = 1 << 2;
pub const MOD_COMMAND: u32 = 1 << 3;
pub const MOD_CAPS_LOCK: u32 = 1 << 4;
const CANDIDATE_PAGE_SIZE: usize = 5;
const SECONDARY_SELECTION_DIGITS: [char; 5] = ['6', '7', '8', '9', '0'];
use crate::blacklist::Store;
use crate::context_model::ContextModel;
use crate::mixer::{MixerDecoder, MixerLexicon};
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};

fn app_modes() -> &'static Mutex<HashMap<String, InputMode>> {
    static MODES: OnceLock<Mutex<HashMap<String, InputMode>>> = OnceLock::new();
    MODES.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum InputMode {
    Chinese = 0,
    DirectEnglish = 1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecurityState {
    NonSecure,
    Secure,
    Unknown,
}

impl SecurityState {
    pub fn from_u32(value: u32) -> Self {
        match value {
            0 => Self::NonSecure,
            1 => Self::Secure,
            _ => Self::Unknown,
        }
    }

    fn degraded(self) -> bool {
        self != Self::NonSecure
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyKind {
    Text,
    Backspace,
    Left,
    Right,
    Space,
    Enter,
    Escape,
    PageUp,
    PageDown,
    Up,
    Down,
    ShiftTap,
    CapsLock,
    FocusLost,
    Deactivate,
    Sleep,
    Resume,
    DeleteCandidate,
    BlacklistCandidate,
    Unsupported,
    LinearNext,
    LinearPrevious,
}

impl KeyKind {
    pub fn from_u32(value: u32) -> Option<Self> {
        Some(match value {
            0 => Self::Text,
            1 => Self::Backspace,
            2 => Self::Left,
            3 => Self::Right,
            4 => Self::Space,
            5 => Self::Enter,
            6 => Self::Escape,
            7 => Self::PageUp,
            8 => Self::PageDown,
            9 => Self::Up,
            10 => Self::Down,
            11 => Self::ShiftTap,
            12 => Self::CapsLock,
            13 => Self::FocusLost,
            14 => Self::Deactivate,
            15 => Self::Sleep,
            16 => Self::Resume,
            17 => Self::DeleteCandidate,
            18 => Self::BlacklistCandidate,
            20 => Self::LinearNext,
            21 => Self::LinearPrevious,
            _ => Self::Unsupported,
        })
    }
}

#[derive(Clone, Debug)]
pub struct KeyEvent {
    pub kind: KeyKind,
    pub text: String,
    pub modifiers: u32,
    pub security: SecurityState,
    /// Stable identifier derived by the host from the physical NSEvent.
    /// Zero means that the host could not prove stable delivery metadata.
    pub delivery_id: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Candidate {
    pub text: String,
    pub comment: String,
    /// Decoder reading key used for deterministic text+reading de-duplication.
    pub reading_key: String,
    /// Stable source-list index when a decoder exposes more than one native
    /// page for Mixer ranking. This value never crosses the public ABI.
    pub source_index: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct DecoderSnapshot {
    pub raw_input: String,
    pub preedit: String,
    pub cursor_utf8: usize,
    pub candidates: Vec<Candidate>,
    pub secondary_candidates: Vec<Candidate>,
    pub highlighted_index: usize,
    pub page_no: usize,
    pub is_last_page: bool,
}

// The shim exposes no confirmed-segment boundary. Match whole trailing pinyin
// tokens against a suffix of the raw input instead of classifying confirmed text
// as non-ASCII. Rime inserts syllable spaces and displays raw v as ü. Never split
// a mismatching token to salvage a shorter suffix: that boundary is uncertain.
fn editing_confirmed_prefix(snapshot: &DecoderSnapshot) -> Option<&str> {
    fn spelling(text: &str) -> String {
        text.chars().filter_map(|ch| match ch {
            ' ' | '\'' => None,
            'ü' | 'Ü' => Some('v'),
            ch => Some(ch),
        }).collect()
    }
    let raw = spelling(&snapshot.raw_input);
    let preedit = snapshot.preedit.as_str();
    let mut end = preedit.len();
    let mut boundary = None;
    loop {
        let start = preedit[..end].trim_end_matches(|ch: char|
            ch.is_ascii_lowercase() || matches!(ch, 'ü' | 'Ü' | '\'')).len();
        if start == end { break; }
        let suffix = spelling(&preedit[start..]);
        if suffix.is_empty() || !raw.ends_with(&suffix) { break; }
        boundary = Some(start);
        end = preedit[..start].trim_end_matches(' ').len();
        if end == start { break; }
    }
    // A match that stopped at a token that did not match (jv is shown as ju, so "ju hao" against raw "jvhao" matches only "hao") leaves
    // pinyin-looking text before the boundary: that text is unconfirmed, so the boundary is uncertain and nothing is guessed.
    boundary.and_then(|at| {
        let before = preedit[..at].trim_end_matches(' ');
        if before.chars().next_back().is_some_and(|ch| ch.is_ascii_lowercase() || matches!(ch, 'ü' | 'Ü' | '\'')) {
            None
        } else {
            Some(&preedit[..at])
        }
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LearningIntent {
    /// Commit without authorizing any persistent UserDB write.
    NoLearn,
    /// The owner deliberately selected a non-default candidate.
    DeliberateSelection,
    /// Commit the complete multi-segment preview without authorizing learning.
    RequiredSegmentCommit,
}

/// Legacy consumers accepted different preedit spellings. Native decoders answer
/// every purpose exactly from their own composition state; only editing recovery
/// still reconstructs a boundary from the preedit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundaryPurpose {
    /// An empty confirmed prefix is valid; None means recovery is uncertain.
    Editing,
    /// Mixer's own composition work on the letters left after a picked segment.
    Mixing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompositionBoundary {
    pub confirmed: String,
    pub unconfirmed_raw: String,
}

pub(crate) fn default_composition_boundary(snapshot: &DecoderSnapshot) -> Option<CompositionBoundary> {
    let confirmed = editing_confirmed_prefix(snapshot)?;
    let spelling = |text: &str| text.chars().filter_map(|ch| match ch {
        ' ' | '\'' => None,
        'ü' | 'Ü' => Some('v'),
        ch => Some(ch),
    }).collect::<String>();
    let suffix = spelling(&snapshot.preedit[confirmed.len()..]);
    let start = snapshot.raw_input.char_indices().map(|(at, _)| at).find(|&at|
        spelling(&snapshot.raw_input[at..]) == suffix)?;
    Some(CompositionBoundary {
        confirmed: confirmed.to_owned(), unconfirmed_raw: snapshot.raw_input[start..].to_owned(),
    })
}

pub trait Decoder: Send {
    /// A native decoder's deployed words must also drive Mixer's reading checks,
    /// ranking and learning. Legacy decoders use the background dictionary.
    fn dictionary_snapshot(&self) -> Option<Arc<crate::zhixu::Index>> { None }
    /// Native editing does not need Session's Rime menu/selection restoration.
    fn native_editing(&self) -> bool { false }
    fn composition_boundary(&self, purpose: BoundaryPurpose) -> Option<CompositionBoundary> {
        // Only editing recovery reconstructs a boundary from the preedit; a
        // mixing consumer needs the decoder's own exact answer.
        (purpose == BoundaryPurpose::Editing)
            .then(|| default_composition_boundary(&self.snapshot()))
            .flatten()
    }
    fn configure_learning(&mut self, _mode: crate::zhixu::learning::LearningMode) -> Result<(), i32> { Ok(()) }
    fn configure_zhixu_whole(&mut self, _on: bool) -> Result<(), i32> { Ok(()) }
    fn learning_key(&mut self, _event: &KeyEvent, _now: u64) {}
    fn learning_commit(&mut self, _text: &str, _delivery_id: u64, _now: u64) {}
    fn withdraw_learning(&mut self, _raw: &str, _text: &str) -> bool { false }
    fn snapshot(&self) -> DecoderSnapshot;
    /// Returns the complete stable candidate sequence without changing the
    /// native page. Decoders that cannot expose it keep the native pager.
    fn snapshot_all_candidates(&self) -> Option<DecoderSnapshot> {
        None
    }
    /// Returns a stable global prefix without moving the native pager.
    /// `is_last_page` is true only when the prefix contains the complete sequence.
    fn snapshot_candidate_prefix(&self, maximum: usize) -> Option<DecoderSnapshot> {
        let mut snapshot = self.snapshot_all_candidates()?;
        snapshot.is_last_page &= snapshot.candidates.len() <= maximum;
        snapshot.candidates.truncate(maximum);
        for (index, candidate) in snapshot.candidates.iter_mut().enumerate() {
            candidate.source_index = Some(candidate.source_index.unwrap_or(index));
        }
        snapshot.page_no = 0;
        snapshot.highlighted_index = 0;
        Some(snapshot)
    }
    /// A raw ASCII echo anywhere in the native sequence disables whole generation.
    /// Unknown decoders conservatively require the complete snapshot.
    fn whole_prefix_has_raw_echo(&self) -> bool { true }
    /// Cheap composition probe. The default derives it from `snapshot()`;
    /// decoders whose `snapshot()` runs a full ranking pipeline should
    /// override this so keystroke guards do not pay for ranking.
    fn is_composing(&self) -> bool {
        !self.snapshot().raw_input.is_empty()
    }
    fn input(&mut self, text: &str) -> Result<String, i32>;
    fn backspace(&mut self) -> Result<String, i32>;
    fn move_left(&mut self) -> Result<String, i32>;
    fn move_right(&mut self) -> Result<String, i32>;
    fn page_up(&mut self) -> Result<String, i32>;
    fn page_down(&mut self) -> Result<String, i32>;
    fn select_previous(&mut self) -> Result<String, i32>;
    fn select_next(&mut self) -> Result<String, i32>;
    /// Linear candidate navigation for hosts that cross row boundaries.
    fn select_next_linear(&mut self) -> Result<String, i32> {
        self.select_next()
    }
    fn select_previous_linear(&mut self) -> Result<String, i32> {
        self.select_previous()
    }
    /// Commits a candidate through the decoder's at-most-once learning
    /// boundary. `delivery_id` is only the stable host event identity;
    /// `learning_intent` independently authorizes (or forbids) persistence.
    fn select_candidate(
        &mut self,
        index: usize,
        delivery_id: u64,
        learning_intent: LearningIntent,
    ) -> Result<String, i32>;
    fn select_candidate_global(
        &mut self,
        _index: usize,
        _delivery_id: u64,
        _learning_intent: LearningIntent,
    ) -> Result<String, i32> {
        Err(-10)
    }
    fn select_secondary_candidate(
        &mut self,
        _index: usize,
        _delivery_id: u64,
    ) -> Result<String, i32> {
        Err(-10)
    }
    fn delete_candidate(&mut self, index: usize) -> Result<(), i32>;
    fn delete_candidate_global(&mut self, _index: usize) -> Result<(), i32> {
        Err(-10)
    }
    fn blacklist_candidate(&mut self, _index: usize) -> Result<(), i32> {
        Err(-10)
    }
    fn commit_raw(&mut self) -> String;
    fn clear(&mut self);
    /// End any remaining segment without undoing the choice just learned.
    fn clear_after_selection(&mut self) {
        self.clear();
    }
    fn configure_fuzzy(&mut self, _mask: u32) -> Result<(), i32> {
        Ok(())
    }
    /// Deploys again so dictionaries added to the user directory (downloaded packs) are compiled into the running session.
    fn reload_dictionaries(&mut self) -> Result<(), i32> {
        Ok(())
    }
    fn apply_lexicon(&mut self, _lexicon: Arc<MixerLexicon>) {}
    fn apply_context_model(&mut self, _model: Arc<ContextModel>) {}
    fn context_group_mask(&self) -> u32 {
        0
    }
    fn context_complete_mask(&self) -> u32 {
        0
    }
    fn reorder_context_candidates(&mut self, _order: &[usize]) -> Result<(), i32> {
        Err(-10)
    }
    /// The candidates of up to `count` pages after the current one, exactly as
    /// paging would show them, leaving page, highlight and model order as they were.
    fn peek_following_pages(&mut self, _count: usize) -> Vec<Vec<Candidate>> {
        Vec::new()
    }
    fn corrections_changed(&mut self) {}

    /// C3: whole-reading compositions the context model may lead with.
    fn composition_alternatives(&self) -> Vec<String> {
        Vec::new()
    }
    /// Normalized typing cost (cost / edit_penalty) for the first and each alternative.
    fn composition_edits(&self) -> Vec<f64> {
        Vec::new()
    }
    /// The typed letters are not whole pinyin, so the first is a guess as well.
    fn composition_guessed(&self) -> bool {
        false
    }
    fn lead_with_composition(&mut self, _text: &str) -> Result<(), i32> {
        Err(-10)
    }
    /// Shows one of the current alternatives in the last slot of the first page.
    fn offer_composition(&mut self, _text: &str) -> Result<(), i32> {
        Err(-10)
    }
    fn configure_preceding_text(&mut self, _preceding_text: Option<String>) {}
    fn configure_typo(&mut self, _enabled: bool) {}
    fn configure_code_ascii_bias(&mut self, _enabled: bool) {}
    fn configure_candidate_context(
        &mut self,
        _active_project: Option<String>,
        _active_packs: Vec<String>,
        _secondary_enabled: bool,
        _prediction_enabled: bool,
    ) {
    }
}

#[cfg(test)]
pub struct MockDecoder {
    raw: String,
    cursor: usize,
    highlighted: usize,
    page_no: usize,
    secondary_enabled: bool,
}

#[cfg(test)]
impl Default for MockDecoder {
    fn default() -> Self {
        Self {
            raw: String::new(),
            cursor: 0,
            highlighted: 0,
            page_no: 0,
            secondary_enabled: true,
        }
    }
}

#[cfg(test)]
impl MockDecoder {
    fn all_candidates(&self) -> Vec<Candidate> {
        let words: &[&str] = match self.raw.as_str() {
            "ni" => &["你", "呢", "泥"],
            "nihao" => &["你好", "拟好", "你号"],
            "shuming" => &["书名", "署名"],
            "sha" => &["SHA", "啥", "杀", "沙", "傻"],
            "zhongguo" => &["中国", "中过"],
            "ceshi" => &["测试", "侧视"],
            "" | "nocandidate" | "utf" | "utf8" => &[],
            _ => &[
                "候选一",
                "候选二",
                "候选三",
                "候选四",
                "候选五",
                "候选六",
                "候选七",
                "候选八",
                "候选九",
                "候选十",
            ],
        };
        words
            .iter()
            .map(|word| Candidate {
                text: (*word).into(),
                comment: String::new(),
                reading_key: self.raw.clone(),
                source_index: None,
            })
            .collect()
    }

    fn page_candidates(&self) -> Vec<Candidate> {
        self.all_candidates()
            .into_iter()
            .skip(self.page_no * CANDIDATE_PAGE_SIZE)
            .take(CANDIDATE_PAGE_SIZE)
            .collect()
    }

    fn secondary_candidates(&self) -> Vec<Candidate> {
        if !self.secondary_enabled || self.raw != "niha" {
            return Vec::new();
        }
        ["第二一", "第二二", "第二三", "第二四", "第二五"]
            .into_iter()
            .map(|word| Candidate {
                text: word.into(),
                comment: "纠错 · nihao".into(),
                reading_key: "nihao".into(),
                source_index: None,
            })
            .collect()
    }

    fn finish(&mut self, text: String) -> String {
        self.clear();
        text
    }
}

#[cfg(test)]
impl Decoder for MockDecoder {
    fn snapshot(&self) -> DecoderSnapshot {
        let total = self.all_candidates().len();
        DecoderSnapshot {
            raw_input: self.raw.clone(),
            preedit: self.raw.clone(),
            cursor_utf8: self.cursor,
            candidates: self.page_candidates(),
            secondary_candidates: self.secondary_candidates(),
            highlighted_index: self.highlighted,
            page_no: self.page_no,
            is_last_page: (self.page_no + 1) * CANDIDATE_PAGE_SIZE >= total,
        }
    }

    fn input(&mut self, text: &str) -> Result<String, i32> {
        self.raw.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.highlighted = 0;
        self.page_no = 0;
        Ok(String::new())
    }

    fn backspace(&mut self) -> Result<String, i32> {
        if self.cursor > 0 {
            let previous = self.raw[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(index, _)| index)
                .unwrap_or(0);
            self.raw.replace_range(previous..self.cursor, "");
            self.cursor = previous;
        }
        Ok(String::new())
    }

    fn move_left(&mut self) -> Result<String, i32> {
        self.cursor = self.raw[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0);
        Ok(String::new())
    }

    fn move_right(&mut self) -> Result<String, i32> {
        if self.cursor < self.raw.len() {
            let width = self.raw[self.cursor..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(0);
            self.cursor += width;
        }
        Ok(String::new())
    }

    fn page_up(&mut self) -> Result<String, i32> {
        self.page_no = self.page_no.saturating_sub(1);
        self.highlighted = 0;
        Ok(String::new())
    }

    fn page_down(&mut self) -> Result<String, i32> {
        if (self.page_no + 1) * CANDIDATE_PAGE_SIZE < self.all_candidates().len() {
            self.page_no += 1;
            self.highlighted = 0;
        }
        Ok(String::new())
    }

    fn select_previous(&mut self) -> Result<String, i32> {
        self.highlighted = self.highlighted.saturating_sub(1);
        Ok(String::new())
    }

    fn select_next(&mut self) -> Result<String, i32> {
        let count = self.page_candidates().len();
        if self.highlighted + 1 < count {
            self.highlighted += 1;
        }
        Ok(String::new())
    }

    fn select_candidate(
        &mut self,
        index: usize,
        _delivery_id: u64,
        _learning_intent: LearningIntent,
    ) -> Result<String, i32> {
        let page = self.page_candidates();
        page.get(index)
            .map(|candidate| self.finish(candidate.text.clone()))
            .ok_or(-10)
    }

    fn select_secondary_candidate(
        &mut self,
        index: usize,
        _delivery_id: u64,
    ) -> Result<String, i32> {
        self.secondary_candidates()
            .get(index)
            .map(|candidate| self.finish(candidate.text.clone()))
            .ok_or(-10)
    }

    fn delete_candidate(&mut self, index: usize) -> Result<(), i32> {
        if index < self.page_candidates().len() {
            Ok(())
        } else {
            Err(-10)
        }
    }

    fn commit_raw(&mut self) -> String {
        let raw = self.raw.clone();
        self.finish(raw)
    }

    fn clear(&mut self) {
        self.raw.clear();
        self.cursor = 0;
        self.highlighted = 0;
        self.page_no = 0;
    }

    fn configure_candidate_context(
        &mut self,
        _active_project: Option<String>,
        _active_packs: Vec<String>,
        secondary_enabled: bool,
        _prediction_enabled: bool,
    ) {
        self.secondary_enabled = secondary_enabled;
    }
}

#[derive(Debug)]
pub struct ResultModel {
    pub status: i32,
    pub consumed: bool,
    pub mode: InputMode,
    pub secure_degraded: bool,
    pub marked_text: String,
    pub marked_cursor_utf8: usize,
    pub commit_text: String,
    pub candidates: Vec<Candidate>,
    pub secondary_candidates: Vec<Candidate>,
    pub highlighted_index: usize,
    pub page_no: usize,
    pub is_last_page: bool,
    pub selection_event_id: u64,
}

pub struct Session {
    decoder: Box<dyn Decoder>,
    mode: InputMode,
    app_id: String,
    context_profile: ContextProfile,
    pending_context_profile: Option<ContextProfile>,
    single_quote_open: bool,
    double_quote_open: bool,
    previous_text_kind: PreviousTextKind,
    host_previous_text_kind: Option<PreviousTextKind>,
    host_preceding_text: Option<String>,
    pending_selection_event_id: Cell<u64>,
    secure_degraded: bool,
    /// Inline English started by a Shift+letter: following letters keep
    /// typing English (`LoRA`, `GPT4`) until Space/Enter commits it.
    inline_ascii: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AppClass {
    Code,
    Prose,
    Browser,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PunctuationPolicy {
    Automatic,
    FixedFullwidth,
    FixedAscii,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ContextProfile {
    app_class: AppClass,
    punctuation_policy: PunctuationPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreviousTextKind {
    None,
    Ascii,
    HanOrFullwidth,
    Other,
}

fn default_app_class(app_id: &str) -> AppClass {
    const CODE_APPS: &[&str] = &[
        "com.apple.dt.Xcode",
        "com.apple.Terminal",
        "com.anthropic.claudefordesktop",
        "com.googlecode.iterm2",
        "com.mitchellh.ghostty",
        "com.microsoft.VSCode",
        "com.openai.codex",
        "com.todesktop.230313mzl4w4u92",
    ];
    const PROSE_APPS: &[&str] = &[
        "com.apple.TextEdit",
        "com.apple.iWork.Pages",
        "com.apple.MobileSMS",
        "com.microsoft.Word",
        "com.tencent.xinWeChat",
        "com.tinyspeck.slackmacgap",
        "net.whatsapp.WhatsApp",
        "notion.id",
    ];
    const BROWSER_APPS: &[&str] = &[
        "com.apple.Safari",
        "com.google.Chrome",
        "com.brave.Browser",
        "com.microsoft.edgemac",
        "org.mozilla.firefox",
    ];
    // Windows shells identify the application by its process file name (lower-cased here).
    const WINDOWS_CODE_APPS: &[&str] = &[
        "code.exe", "code - insiders.exe", "cursor.exe", "windsurf.exe", "zed.exe", "devenv.exe", "windowsterminal.exe", "wt.exe",
        "cmd.exe", "powershell.exe", "pwsh.exe", "conhost.exe", "mintty.exe", "alacritty.exe", "wezterm-gui.exe", "claude.exe",
        "codex.exe", "sublime_text.exe", "notepad++.exe", "idea64.exe", "pycharm64.exe", "webstorm64.exe", "clion64.exe",
        "goland64.exe", "rider64.exe", "rustrover64.exe", "phpstorm64.exe", "studio64.exe",
    ];
    const WINDOWS_PROSE_APPS: &[&str] = &["winword.exe", "wechat.exe", "weixin.exe", "wps.exe", "wpsoffice.exe", "qq.exe", "dingtalk.exe", "feishu.exe", "lark.exe", "notion.exe", "slack.exe", "whatsapp.exe"];
    const WINDOWS_BROWSER_APPS: &[&str] = &["chrome.exe", "msedge.exe", "firefox.exe", "brave.exe", "opera.exe", "360se.exe", "360chrome.exe", "qqbrowser.exe", "sogouexplorer.exe"];

    let windows_name = app_id.to_ascii_lowercase();
    if CODE_APPS.contains(&app_id) || app_id.starts_with("com.jetbrains.") || WINDOWS_CODE_APPS.contains(&windows_name.as_str()) {
        AppClass::Code
    } else if PROSE_APPS.contains(&app_id) || WINDOWS_PROSE_APPS.contains(&windows_name.as_str()) {
        AppClass::Prose
    } else if BROWSER_APPS.contains(&app_id) || WINDOWS_BROWSER_APPS.contains(&windows_name.as_str()) {
        AppClass::Browser
    } else {
        AppClass::Unknown
    }
}

fn classify_last_text(text: &str) -> PreviousTextKind {
    let Some(character) = text.chars().last() else {
        return PreviousTextKind::None;
    };
    if character.is_ascii_alphanumeric() {
        return PreviousTextKind::Ascii;
    }
    let code = character as u32;
    if matches!(
        code,
        0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xf900..=0xfaff
            | 0x20000..=0x2fa1f
            | 0x3000..=0x303f
            | 0xff01..=0xff60
            | 0xffe0..=0xffe6
    ) {
        PreviousTextKind::HanOrFullwidth
    } else {
        PreviousTextKind::Other
    }
}

fn contains_han(text: &str) -> bool {
    text.chars().any(|character| {
        matches!(
            character as u32,
            0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x2fa1f
        )
    })
}

fn is_inline_ascii_text(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(c), None) if c.is_ascii_alphanumeric() || "-_+#.'".contains(c)
    )
}

fn page_has_chinese_candidate(snapshot: &DecoderSnapshot) -> bool {
    snapshot
        .candidates
        .iter()
        .chain(snapshot.secondary_candidates.iter())
        .any(|candidate| contains_han(&candidate.text))
}

impl Session {
    pub fn configure_zhixu_whole(&mut self, on: bool) -> Result<(), i32> {
        if self.composing() { return Err(-10); }
        // The knob stays for the shells but no longer changes the inner decoder.
        self.decoder.configure_zhixu_whole(on)
    }

    pub fn configure_learning(&mut self, mode: crate::zhixu::learning::LearningMode) -> Result<(), i32> {
        // Switching a displayed menu would change what its digits select.
        if self.composing() { return Err(-10); }
        self.decoder.configure_learning(mode)
    }

    pub fn withdraw_learning(&mut self, raw: &str, text: &str) -> bool {
        self.decoder.withdraw_learning(raw, text)
    }
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(decoder: Box<dyn Decoder>, app_id: String) -> Self {
        Self::build(MixerDecoder::new(decoder, app_id.clone()), app_id)
    }
    /// The product constructor: the native Zhixu decoder is the session's permanent
    /// inner, over one fully deployed dictionary generation, learning in Zhixu.
    /// The host's own learning/whole calls land on it exactly as before.
    pub fn new_native(
        app_id: String,
        blacklist: Arc<Mutex<Store>>,
        dictionary: Arc<crate::zhixu::Index>,
    ) -> Result<Self, i32> {
        let decoder = crate::mixer::MixerDecoder::new_native(app_id.clone(), blacklist, dictionary)?;
        crate::zhixu::prewarm_whole();
        Ok(Self::build(decoder, app_id))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new_with_blacklist(
        decoder: Box<dyn Decoder>,
        app_id: String,
        blacklist: Arc<Mutex<Store>>,
    ) -> Result<Self, i32> {
        let mut decoder = MixerDecoder::new(decoder, app_id.clone());
        decoder.apply_blacklist(blacklist);
        Ok(Self::build(decoder, app_id))
    }

    fn build(
        mut decoder: MixerDecoder,
        app_id: String,
    ) -> Self {
        let mode = app_modes()
            .lock()
            .ok()
            .and_then(|modes| modes.get(&app_id).copied())
            .unwrap_or(InputMode::Chinese);
        let context_profile = ContextProfile {
            app_class: default_app_class(&app_id),
            punctuation_policy: PunctuationPolicy::Automatic,
        };
        decoder.configure_code_ascii_bias(context_profile.app_class == AppClass::Code);
        Self {
            decoder: Box::new(decoder),
            mode,
            app_id,
            context_profile,
            pending_context_profile: None,
            single_quote_open: false,
            double_quote_open: false,
            previous_text_kind: PreviousTextKind::None,
            host_previous_text_kind: None,
            host_preceding_text: None,
            pending_selection_event_id: Cell::new(0),
            secure_degraded: false,
            inline_ascii: String::new(),
        }
    }

    pub fn configure_context_profile(
        &mut self,
        app_class_override: u32,
        punctuation_policy: u32,
    ) -> Result<(), i32> {
        let app_class = match app_class_override {
            0 => default_app_class(&self.app_id),
            1 => AppClass::Code,
            2 => AppClass::Prose,
            3 => AppClass::Browser,
            4 => AppClass::Unknown,
            _ => return Err(-13),
        };
        let punctuation_policy = match punctuation_policy {
            0 => PunctuationPolicy::Automatic,
            1 => PunctuationPolicy::FixedFullwidth,
            2 => PunctuationPolicy::FixedAscii,
            _ => return Err(-14),
        };
        let profile = ContextProfile {
            app_class,
            punctuation_policy,
        };
        if self.composing() {
            self.pending_context_profile = Some(profile);
        } else {
            self.context_profile = profile;
            self.decoder
                .configure_code_ascii_bias(profile.app_class == AppClass::Code);
        }
        Ok(())
    }

    pub fn set_host_preceding_text(&mut self, text: &str) {
        // Context is frozen at the composition boundary. A host that retries
        // this call while composing must not change the context underneath a
        // candidate list that is already visible.
        if self.composing() {
            return;
        }
        self.host_previous_text_kind = Some(classify_last_text(text));
        self.host_preceding_text = Some(text.to_owned());
        self.decoder.configure_preceding_text(Some(text.to_owned()));
    }

    pub fn apply_lexicon(&mut self, lexicon: Arc<MixerLexicon>) {
        self.decoder.apply_lexicon(lexicon);
    }

    pub fn context_group_mask(&self) -> u32 {
        self.decoder.context_group_mask()
    }

    pub fn context_complete_mask(&self) -> u32 {
        self.decoder.context_complete_mask()
    }

    pub fn set_correction_settings(&mut self, settings: crate::ranking::CorrectionSettings) {
        crate::ranking::set_correction_settings(settings);
        self.decoder.corrections_changed();
    }

    pub fn composition_alternatives(&self) -> Vec<String> {
        if !self.inline_ascii.is_empty() {
            return Vec::new();
        }
        self.decoder.composition_alternatives()
    }

    /// The texts of the page exactly as the digit keys resolve against it (the single source of truth for what
    /// a shell may draw): shells that reorder or insert candidates for display must match this.
    pub fn page_texts(&self) -> Vec<String> {
        if !self.inline_ascii.is_empty() {
            return Vec::new();
        }
        self.decoder.snapshot().candidates.into_iter().map(|candidate| candidate.text).collect()
    }

    /// Reads the complete current page without consuming a selection event or finishing composition.
    pub fn current_result(&self) -> ResultModel {
        self.result_from_snapshot(self.decoder.snapshot(), false, String::new(), 0)
    }

    pub fn composition_edits(&self) -> Vec<f64> {
        if !self.inline_ascii.is_empty() {
            return Vec::new();
        }
        self.decoder.composition_edits()
    }

    pub fn composition_guessed(&self) -> bool {
        if !self.inline_ascii.is_empty() {
            return false;
        }
        self.decoder.composition_guessed()
    }

    pub fn lead_with_composition(&mut self, text: &str) -> Result<(), i32> {
        self.decoder.lead_with_composition(text)
    }

    pub fn offer_composition(&mut self, text: &str) -> Result<(), i32> {
        self.decoder.offer_composition(text)
    }

    /// Expanded panel (`+`): the following pages, read without moving the page.
    pub fn peek_following_pages(&mut self, count: usize) -> Vec<Vec<Candidate>> {
        if !self.composing() || !self.inline_ascii.is_empty() {
            return Vec::new();
        }
        self.decoder.peek_following_pages(count.min(MAX_PEEK_PAGES))
    }

    pub fn reorder_context_candidates(&mut self, order: &[usize]) -> Result<(), i32> {
        self.decoder.reorder_context_candidates(order)
    }

    pub fn apply_context_model(&mut self, model: Arc<ContextModel>) {
        self.decoder.apply_context_model(model);
    }

    pub fn configure_typo(&mut self, enabled: bool) {
        self.decoder.clear();
        self.decoder.configure_typo(enabled);
    }

    pub fn configure_candidate_context(
        &mut self,
        active_project: Option<String>,
        active_packs: Vec<String>,
        secondary_enabled: bool,
        prediction_enabled: bool,
    ) {
        self.decoder.clear();
        self.decoder.configure_candidate_context(
            active_project,
            active_packs,
            secondary_enabled,
            prediction_enabled,
        );
    }

    pub fn select_secondary_candidate(
        &mut self,
        index: usize,
        delivery_id: u64,
        security: SecurityState,
    ) -> ResultModel {
        if security.degraded() {
            return self.finish(false, String::new(), 0, security);
        }
        let result = self.decoder.select_secondary_candidate(index, delivery_id);
        if result.is_ok() {
            self.mark_selection_event(delivery_id);
        }
        self.apply_decoder_result(result, security)
    }

    pub fn configure_fuzzy(&mut self, mask: u32) -> Result<(), i32> {
        if mask & !0xff != 0 {
            return Err(-12);
        }
        // A native session's live menu reads the mask itself and must not change.
        if self.decoder.native_editing() && self.composing() {
            return Err(-10);
        }
        self.decoder.clear();
        self.decoder.configure_fuzzy(mask)
    }

    /// Compiles dictionaries that appeared in the user directory since the session started. Any composition is dropped.
    pub fn reload_dictionaries(&mut self) -> Result<(), i32> {
        self.decoder.clear();
        self.decoder.reload_dictionaries()
    }

    pub fn set_mode(&mut self, mode: InputMode) {
        self.decoder.clear();
        self.mode = mode;
        if let Ok(mut modes) = app_modes().lock() {
            modes.insert(self.app_id.clone(), mode);
        }
    }

    fn composing(&self) -> bool {
        self.decoder.is_composing()
    }

    fn finish(
        &mut self,
        consumed: bool,
        commit_text: String,
        status: i32,
        security: SecurityState,
    ) -> ResultModel {
        if status == 0 && !security.degraded() && !commit_text.is_empty() {
            self.decoder.learning_commit(&commit_text, self.pending_selection_event_id.get(), crate::personal_picks::now_ms());
        }
        if !security.degraded() && !commit_text.is_empty() {
            self.previous_text_kind = classify_last_text(&commit_text);
        }
        let snapshot = self.decoder.snapshot();
        let composition_ended = snapshot.raw_input.is_empty();
        self.secure_degraded = security.degraded();
        let result = self.result_from_snapshot(snapshot, consumed, commit_text, status);
        self.pending_selection_event_id.set(0);
        if composition_ended {
            self.host_preceding_text = None;
            self.decoder.configure_preceding_text(None);
            if let Some(profile) = self.pending_context_profile.take() {
                self.context_profile = profile;
                self.decoder
                    .configure_code_ascii_bias(profile.app_class == AppClass::Code);
            }
        }
        result
    }

    fn result_from_snapshot(
        &self,
        snapshot: DecoderSnapshot,
        consumed: bool,
        commit_text: String,
        status: i32,
    ) -> ResultModel {
        let mut result = ResultModel {
            status,
            consumed,
            mode: self.mode,
            secure_degraded: self.secure_degraded,
            marked_text: snapshot.preedit,
            marked_cursor_utf8: snapshot.cursor_utf8,
            commit_text,
            candidates: snapshot.candidates,
            secondary_candidates: snapshot.secondary_candidates,
            highlighted_index: snapshot.highlighted_index,
            page_no: snapshot.page_no,
            is_last_page: snapshot.is_last_page,
            selection_event_id: self.pending_selection_event_id.get(),
        };
        if !self.inline_ascii.is_empty() {
            result.marked_text = self.inline_ascii.clone();
            result.marked_cursor_utf8 = self.inline_ascii.len();
            result.candidates.clear();
            result.secondary_candidates.clear();
            result.highlighted_index = 0;
            result.page_no = 0;
            result.is_last_page = true;
        }
        result
    }

    fn mark_selection_event(&self, delivery_id: u64) {
        // Fail closed when no stable host delivery identity is available.
        // Diagnostics must never invent a process-local fallback that changes
        // when the same physical event is replayed after a crash.
        self.pending_selection_event_id.set(delivery_id);
    }

    fn apply_decoder_result(
        &mut self,
        result: Result<String, i32>,
        security: SecurityState,
    ) -> ResultModel {
        match result {
            Ok(commit) => self.finish(true, commit, 0, security),
            Err(status) => self.finish(true, String::new(), status, security),
        }
    }

    fn current_or_raw(&mut self, delivery_id: u64) -> String {
        let snapshot = self.decoder.snapshot();
        if snapshot.candidates.is_empty() {
            self.decoder.commit_raw()
        } else {
            let learning_intent = if snapshot.page_no > 0 || snapshot.highlighted_index > 0 {
                LearningIntent::DeliberateSelection
            } else {
                LearningIntent::NoLearn
            };
            match self.decoder.select_candidate(
                snapshot.highlighted_index,
                delivery_id,
                learning_intent,
            ) {
                Ok(commit) => {
                    self.mark_selection_event(delivery_id);
                    commit
                }
                Err(_) => self.decoder.commit_raw(),
            }
        }
    }

    fn should_use_fullwidth(&self, immediate_context: PreviousTextKind) -> bool {
        match self.context_profile.punctuation_policy {
            PunctuationPolicy::FixedFullwidth => true,
            PunctuationPolicy::FixedAscii => false,
            PunctuationPolicy::Automatic => match immediate_context {
                PreviousTextKind::HanOrFullwidth => true,
                PreviousTextKind::Ascii => false,
                PreviousTextKind::None => self.context_profile.app_class != AppClass::Code,
                PreviousTextKind::Other => true,
            },
        }
    }

    fn mapped_punctuation(
        &mut self,
        text: &str,
        immediate_context: PreviousTextKind,
    ) -> Option<&'static str> {
        if !self.should_use_fullwidth(immediate_context) {
            return None;
        }
        match text {
            "," => Some("，"),
            "." => Some("。"),
            "?" => Some("？"),
            "!" => Some("！"),
            ":" => Some("："),
            ";" => Some("；"),
            "(" => Some("（"),
            ")" => Some("）"),
            "[" => Some("【"),
            "]" => Some("】"),
            "<" => Some("《"),
            ">" => Some("》"),
            "\\" => Some("、"),
            "^" => Some("……"),
            "_" => Some("——"),
            // `@` stays `@` (it was mapped to `·` by mistake: owner and a Windows friend could not type it in Chinese mode, 2026-10-16);
            // the middle dot sits on the backtick key and the wave dash on the tilde key, as in the other Chinese input methods.
            "`" => Some("·"),
            "~" => Some("～"),
            "$" => Some("￥"),
            "'" => {
                self.single_quote_open = !self.single_quote_open;
                Some(if self.single_quote_open { "‘" } else { "’" })
            }
            "\"" => {
                self.double_quote_open = !self.double_quote_open;
                Some(if self.double_quote_open { "“" } else { "”" })
            }
            _ => None,
        }
    }

    pub fn process(&mut self, event: KeyEvent) -> ResultModel {
        self.decoder.learning_key(&event, crate::personal_picks::now_ms());
        let host_previous_text_kind = self.host_previous_text_kind.take();
        let security = event.security;
        if security.degraded() {
            self.inline_ascii.clear();
            self.host_preceding_text = None;
            self.host_previous_text_kind = None;
            self.decoder.configure_preceding_text(None);
            self.decoder.clear();
            return self.finish(false, String::new(), 0, security);
        }
        let system_modifier = event.modifiers & (MOD_COMMAND | MOD_OPTION | MOD_CONTROL) != 0;
        let caps_lock = event.modifiers & MOD_CAPS_LOCK != 0;

        let action_modifiers =
            event.modifiers & (MOD_SHIFT | MOD_CONTROL | MOD_OPTION | MOD_COMMAND);
        if event.kind == KeyKind::DeleteCandidate
            && action_modifiers == MOD_CONTROL
            && self.composing()
        {
            let index = self.decoder.snapshot().highlighted_index;
            let status = self.decoder.delete_candidate(index).err().unwrap_or(0);
            return self.finish(true, String::new(), status, security);
        }
        if event.kind == KeyKind::BlacklistCandidate
            && action_modifiers == (MOD_SHIFT | MOD_CONTROL)
            && self.composing()
        {
            let index = self.decoder.snapshot().highlighted_index;
            let status = self.decoder.blacklist_candidate(index).err().unwrap_or(0);
            return self.finish(true, String::new(), status, security);
        }

        if system_modifier {
            return self.finish(false, String::new(), 0, security);
        }

        if !self.inline_ascii.is_empty() {
            return self.process_inline_ascii(event, security);
        }

        if matches!(event.kind, KeyKind::FocusLost | KeyKind::Deactivate) {
            let commit = if self.composing() {
                self.current_or_raw(event.delivery_id)
            } else {
                String::new()
            };
            return self.finish(false, commit, 0, security);
        }

        if event.kind == KeyKind::Sleep {
            self.decoder.clear();
            return self.finish(false, String::new(), 0, security);
        }

        if event.kind == KeyKind::Resume {
            self.decoder.clear();
            return self.finish(false, String::new(), 0, security);
        }

        if event.kind == KeyKind::ShiftTap {
            let commit = if self.composing() {
                self.current_or_raw(event.delivery_id)
            } else {
                String::new()
            };
            self.mode = match self.mode {
                InputMode::Chinese => InputMode::DirectEnglish,
                InputMode::DirectEnglish => InputMode::Chinese,
            };
            if let Ok(mut modes) = app_modes().lock() {
                modes.insert(self.app_id.clone(), self.mode);
            }
            return self.finish(true, commit, 0, security);
        }

        if caps_lock || self.mode == InputMode::DirectEnglish {
            let commit = if self.composing() {
                self.current_or_raw(event.delivery_id)
            } else {
                String::new()
            };
            let result = self.finish(false, commit, 0, security);
            if event.kind == KeyKind::Text {
                self.previous_text_kind = classify_last_text(&event.text);
            }
            return result;
        }

        match event.kind {
            KeyKind::Text => self.process_text(
                &event.text,
                event.modifiers,
                security,
                event.delivery_id,
                host_previous_text_kind,
            ),
            KeyKind::Backspace if self.composing() => {
                let result = self.decoder.backspace();
                self.apply_decoder_result(result, security)
            }
            KeyKind::Left if self.composing() => {
                let result = self.decoder.move_left();
                self.apply_decoder_result(result, security)
            }
            KeyKind::Right if self.composing() => {
                let result = self.decoder.move_right();
                self.apply_decoder_result(result, security)
            }
            KeyKind::PageUp if self.composing() => {
                let result = self.decoder.page_up();
                self.apply_decoder_result(result, security)
            }
            KeyKind::PageDown if self.composing() => {
                let result = self.decoder.page_down();
                self.apply_decoder_result(result, security)
            }
            KeyKind::Up if self.composing() => {
                let result = self.decoder.select_previous();
                self.apply_decoder_result(result, security)
            }
            KeyKind::Down if self.composing() => {
                let result = self.decoder.select_next();
                self.apply_decoder_result(result, security)
            }
            KeyKind::LinearPrevious if self.composing() => {
                let result = self.decoder.select_previous_linear();
                self.apply_decoder_result(result, security)
            }
            KeyKind::LinearNext if self.composing() => {
                let result = self.decoder.select_next_linear();
                self.apply_decoder_result(result, security)
            }
            KeyKind::Space if self.composing() => {
                let snapshot = self.decoder.snapshot();
                let commit = if snapshot.candidates.is_empty() {
                    format!("{} ", self.decoder.commit_raw())
                } else {
                    self.current_or_raw(event.delivery_id)
                };
                self.finish(true, commit, 0, security)
            }
            KeyKind::Enter if self.composing() => {
                let raw = self.decoder.commit_raw();
                self.finish(true, raw, 0, security)
            }
            KeyKind::Escape if self.composing() => {
                self.decoder.clear();
                self.finish(true, String::new(), 0, security)
            }
            KeyKind::Unsupported if self.composing() => {
                let raw = self.current_or_raw(event.delivery_id);
                self.finish(false, raw, 0, security)
            }
            _ => self.finish(false, String::new(), 0, security),
        }
    }

    /// Keys while inline English is open: letters, digits and word joiners
    /// extend it; Backspace edits it; Space/Enter/Shift commit it alone (no
    /// mode toggle); Escape
    /// drops it; any other key commits it and then acts as usual.
    fn process_inline_ascii(&mut self, event: KeyEvent, security: SecurityState) -> ResultModel {
        match event.kind {
            KeyKind::Text if is_inline_ascii_text(&event.text) => {
                self.inline_ascii.push_str(&event.text);
                self.finish(true, String::new(), 0, security)
            }
            KeyKind::Backspace => {
                self.inline_ascii.pop();
                self.finish(true, String::new(), 0, security)
            }
            KeyKind::Space | KeyKind::Enter | KeyKind::ShiftTap => {
                let commit = std::mem::take(&mut self.inline_ascii);
                self.finish(true, commit, 0, security)
            }
            KeyKind::Escape | KeyKind::Sleep | KeyKind::Resume => {
                self.inline_ascii.clear();
                self.finish(true, String::new(), 0, security)
            }
            _ => {
                let flushed = std::mem::take(&mut self.inline_ascii);
                self.previous_text_kind = classify_last_text(&flushed);
                let mut result = self.process(event);
                result.commit_text.insert_str(0, &flushed);
                result
            }
        }
    }

    fn process_text(
        &mut self,
        text: &str,
        modifiers: u32,
        security: SecurityState,
        delivery_id: u64,
        host_previous_text_kind: Option<PreviousTextKind>,
    ) -> ResultModel {
        if text.is_empty() {
            return self.finish(false, String::new(), 0, security);
        }
        let mut chars = text.chars();
        let first = chars.next().unwrap_or_default();
        let single_character = chars.next().is_none();

        if single_character && first.is_ascii_alphabetic() {
            if modifiers & MOD_SHIFT != 0 {
                let commit = if self.composing() {
                    self.current_or_raw(delivery_id)
                } else {
                    String::new()
                };
                self.inline_ascii.push(first);
                return self.finish(true, commit, 0, security);
            }
            let result = self.decoder.input(text);
            return self.apply_decoder_result(result, security);
        }

        if text == "'" && self.composing() {
            let result = self.decoder.input(text);
            return self.apply_decoder_result(result, security);
        }

        if single_character && first.is_ascii_digit() && self.composing() {
            let index = first.to_digit(10).unwrap_or(0) as usize;
            let snapshot = self.decoder.snapshot();
            // The visible page is the sole owner check for digits. ASCII
            // conveniences may extend only when neither numbered layer has a
            // Chinese candidate; repository vocabulary is deliberately not
            // consulted here because it says nothing about the current page.
            if !page_has_chinese_candidate(&snapshot) {
                let result = self.decoder.input(text);
                return self.apply_decoder_result(result, security);
            }
            if (1..=CANDIDATE_PAGE_SIZE).contains(&index) && index <= snapshot.candidates.len() {
                let learning_intent = if snapshot.page_no > 0 || index > 1 {
                    LearningIntent::DeliberateSelection
                } else {
                    LearningIntent::NoLearn
                };
                let result = self
                    .decoder
                    .select_candidate(index - 1, delivery_id, learning_intent);
                if result.is_ok() {
                    self.mark_selection_event(delivery_id);
                }
                return self.apply_decoder_result(result, security);
            }
            if let Some(secondary_index) = SECONDARY_SELECTION_DIGITS
                .iter()
                .position(|digit| *digit == first)
                .filter(|secondary_index| *secondary_index < snapshot.secondary_candidates.len())
            {
                let result = self
                    .decoder
                    .select_secondary_candidate(secondary_index, delivery_id);
                if result.is_ok() {
                    self.mark_selection_event(delivery_id);
                }
                return self.apply_decoder_result(result, security);
            }
            // A numbered slot that is absent from this Chinese page remains
            // owned by candidate selection. Consume it without mutating or
            // committing the composition; otherwise an empty slot would still
            // leak through as `raw + digit` and recreate the same ambiguity.
            return self.finish(true, String::new(), 0, security);
        }

        let was_composing = self.composing();
        let mut prefix = if was_composing {
            self.current_or_raw(delivery_id)
        } else {
            String::new()
        };
        // A first candidate that covers only part of the input is picked and the rest stays in the
        // composition; punctuation must not land in front of that picked text, so take the rest too.
        let mut rounds = 0;
        while was_composing && self.composing() && rounds < 8 {
            prefix.push_str(&self.current_or_raw(delivery_id));
            rounds += 1;
        }
        let immediate_context = if prefix.is_empty() {
            host_previous_text_kind.unwrap_or(self.previous_text_kind)
        } else {
            classify_last_text(&prefix)
        };
        if let Some(mapped) = self.mapped_punctuation(text, immediate_context) {
            return self.finish(true, format!("{prefix}{mapped}"), 0, security);
        }

        if was_composing {
            let result = self.finish(false, prefix, 0, security);
            self.previous_text_kind = classify_last_text(text);
            return result;
        }
        let result = self.finish(false, String::new(), 0, security);
        self.previous_text_kind = classify_last_text(text);
        result
    }
}

#[cfg(test)]
mod tests {
    include!("zhixu/learning_state_tests.rs");
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    static NEXT_APP: AtomicUsize = AtomicUsize::new(1);
    thread_local! { static ZHIXU_KEY_TEST: Cell<bool> = const { Cell::new(false) }; }

    #[test]
    fn whole_native_prefix_preserves_sequence_last_flag_and_pager() {
        struct CompleteDecoder(DecoderSnapshot);
        impl Decoder for CompleteDecoder {
            fn snapshot(&self) -> DecoderSnapshot { self.0.clone() }
            fn snapshot_all_candidates(&self) -> Option<DecoderSnapshot> { Some(self.0.clone()) }
            fn input(&mut self, _: &str) -> Result<String, i32> { unreachable!() }
            fn backspace(&mut self) -> Result<String, i32> { unreachable!() }
            fn move_left(&mut self) -> Result<String, i32> { unreachable!() }
            fn move_right(&mut self) -> Result<String, i32> { unreachable!() }
            fn page_up(&mut self) -> Result<String, i32> { unreachable!() }
            fn page_down(&mut self) -> Result<String, i32> { unreachable!() }
            fn select_previous(&mut self) -> Result<String, i32> { unreachable!() }
            fn select_next(&mut self) -> Result<String, i32> { unreachable!() }
            fn select_candidate(&mut self, _: usize, _: u64, _: LearningIntent) -> Result<String, i32> { unreachable!() }
            fn delete_candidate(&mut self, _: usize) -> Result<(), i32> { unreachable!() }
            fn commit_raw(&mut self) -> String { unreachable!() }
            fn clear(&mut self) { unreachable!() }
        }
        let decoder = CompleteDecoder(DecoderSnapshot {
            raw_input: "abc".into(), preedit: "a bc".into(), cursor_utf8: 4,
            candidates: (0..13).map(|index| Candidate {
                text: format!("候选{index}"), comment: format!("注释{index}"),
                reading_key: "abc".into(), source_index: Some(index),
            }).collect(),
            page_no: 2, highlighted_index: 1, is_last_page: true,
            ..DecoderSnapshot::default()
        });
        for maximum in [0, 1, 5, 10, 13, 14, usize::MAX] {
            let prefix = decoder.snapshot_candidate_prefix(maximum).unwrap();
            assert_eq!(prefix.candidates, decoder.0.candidates[..maximum.min(13)]);
            assert_eq!(prefix.is_last_page, maximum >= 13);
            assert_eq!((prefix.page_no, prefix.highlighted_index), (0, 0));
            assert_eq!((&prefix.raw_input, &prefix.preedit, prefix.cursor_utf8),
                (&decoder.0.raw_input, &decoder.0.preedit, decoder.0.cursor_utf8));
        }
        assert_eq!((decoder.snapshot().page_no, decoder.snapshot().highlighted_index), (2, 1));
        assert!(MockDecoder::default().snapshot_candidate_prefix(10).is_none());
        assert!(decoder.whole_prefix_has_raw_echo(), "unknown decoders require full enumeration");
    }

    fn event(kind: KeyKind, text: &str) -> KeyEvent {
        KeyEvent {
            kind,
            text: text.into(),
            modifiers: 0,
            security: SecurityState::NonSecure,
            delivery_id: 0,
        }
    }

    fn session() -> Session {
        let decoder: Box<dyn Decoder> = if ZHIXU_KEY_TEST.get() {
            Box::new(crate::zhixu::decoder::ZhixuDecoder::new())
        } else {
            Box::new(MockDecoder::default())
        };
        Session::new(decoder, format!("test.app.{}", NEXT_APP.fetch_add(1, Ordering::Relaxed)))
    }

    fn type_text(session: &mut Session, text: &str) -> ResultModel {
        let mut result = session.finish(false, String::new(), 0, SecurityState::NonSecure);
        for character in text.chars() {
            result = session.process(event(KeyKind::Text, &character.to_string()));
        }
        result
    }

    fn type_visible_text(session: &mut Session, text: &str) -> String {
        let mut visible = String::new();
        for character in text.chars() {
            let input = character.to_string();
            let result = session.process(event(KeyKind::Text, &input));
            visible.push_str(&result.commit_text);
            if !result.consumed {
                visible.push(character);
            }
        }
        visible
    }

    fn assert_menu_matches(result: &ResultModel, snapshot: &DecoderSnapshot) {
        assert_eq!(result.candidates, snapshot.candidates);
        assert_eq!(result.page_no, snapshot.page_no);
        assert_eq!(result.highlighted_index, snapshot.highlighted_index);
        assert_eq!(result.is_last_page, snapshot.is_last_page);
    }

    fn assert_same_page(actual: &ResultModel, expected: &ResultModel) {
        assert_eq!(actual.marked_text, expected.marked_text);
        assert_eq!(actual.marked_cursor_utf8, expected.marked_cursor_utf8);
        assert_eq!(actual.candidates, expected.candidates);
        assert_eq!(actual.secondary_candidates, expected.secondary_candidates);
        assert_eq!(actual.highlighted_index, expected.highlighted_index);
        assert_eq!(actual.page_no, expected.page_no);
        assert_eq!(actual.is_last_page, expected.is_last_page);
        assert_eq!(actual.mode, expected.mode);
        assert_eq!(actual.secure_degraded, expected.secure_degraded);
    }

    fn assert_current_result(actual: &ResultModel, expected: &ResultModel) {
        assert_same_page(actual, expected);
        assert_eq!(actual.status, 0);
        assert!(!actual.consumed);
        assert!(actual.commit_text.is_empty());
        assert_eq!(actual.selection_event_id, expected.selection_event_id);
    }

    #[test]
    fn current_result_matches_process_and_is_idempotent() {
        let mut value = session();
        value.decoder = Box::new(MockDecoder::default());
        type_text(&mut value, "niha");
        value.process(event(KeyKind::Down, ""));
        let expected = value.process(event(KeyKind::Left, ""));
        assert!(!expected.candidates.is_empty());
        assert_eq!(expected.secondary_candidates.len(), 5);
        assert!(!expected.secondary_candidates[0].comment.is_empty());
        assert!(!expected.secondary_candidates[0].reading_key.is_empty());
        assert_eq!(expected.highlighted_index, 1);
        assert_eq!(expected.marked_cursor_utf8, 3);
        let first = value.current_result();
        let second = value.current_result();
        assert_current_result(&first, &expected);
        assert_current_result(&second, &first);
    }

    #[test]
    fn current_result_matches_process_after_paging() {
        let mut value = session();
        let first = type_text(&mut value, "abc");
        assert!(!first.is_last_page);
        assert_current_result(&value.current_result(), &first);
        for page_no in 1..=2 {
            let page = value.process(event(KeyKind::PageDown, ""));
            assert_eq!(page.page_no, page_no);
            assert_ne!(page.candidates, first.candidates);
            let highlighted = value.process(event(KeyKind::Down, ""));
            assert_current_result(&value.current_result(), &highlighted);
            assert_current_result(&value.current_result(), &highlighted);
            if highlighted.is_last_page { break; }
        }
    }

    #[test]
    fn current_result_does_not_change_following_key_or_host_order() {
        for (kind, text) in [(KeyKind::Text, "h"), (KeyKind::Left, ""),
            (KeyKind::PageDown, ""), (KeyKind::Space, "")] {
            let mut baseline = session();
            let mut value = session();
            for session in [&mut baseline, &mut value] {
                type_text(session, "ni");
                session.reorder_context_candidates(&[1, 0, 2]).unwrap();
                session.process(event(KeyKind::Down, ""));
            }
            let before = value.page_texts();
            assert_eq!(before[0], "呢");
            value.current_result();
            value.current_result();
            assert_eq!(value.page_texts(), before);
            let mut key = event(kind, text);
            key.delivery_id = 941;
            let expected = baseline.process(key.clone());
            let actual = value.process(key);
            assert_same_page(&actual, &expected);
            assert_eq!(actual.status, expected.status);
            assert_eq!(actual.consumed, expected.consumed);
            assert_eq!(actual.commit_text, expected.commit_text);
            assert_eq!(actual.selection_event_id, expected.selection_event_id);
        }
    }

    #[test]
    fn current_result_preserves_inline_ascii_mode_and_security() {
        let mut value = session();
        let mut uppercase = event(KeyKind::Text, "A");
        uppercase.modifiers = MOD_SHIFT;
        value.process(uppercase);
        let inline = value.process(event(KeyKind::Text, "b"));
        assert_eq!(inline.marked_text, "Ab");
        assert_current_result(&value.current_result(), &inline);
        assert_current_result(&value.current_result(), &inline);
        assert!(inline.candidates.is_empty());
        assert!(inline.secondary_candidates.is_empty());
        assert_eq!((inline.page_no, inline.highlighted_index, inline.is_last_page), (0, 0, true));

        for security in [SecurityState::Secure, SecurityState::Unknown, SecurityState::NonSecure] {
            let mut key = event(KeyKind::Text, "n");
            key.security = security;
            let expected = value.process(key);
            assert_eq!(expected.secure_degraded, security.degraded());
            assert_current_result(&value.current_result(), &expected);
        }
        value.set_mode(InputMode::DirectEnglish);
        let english = value.process(event(KeyKind::Unsupported, ""));
        assert_eq!(english.mode, InputMode::DirectEnglish);
        assert_current_result(&value.current_result(), &english);
    }

    #[test]
    fn current_result_does_not_consume_selection_or_finish_pending_context() {
        let mut value = session();
        value.set_host_preceding_text("前文");
        for text in ["n", "i"] {
            value.process(event(KeyKind::Text, text));
        }
        value.configure_context_profile(2, 2).unwrap();
        value.decoder.clear();
        value.mark_selection_event(941);
        for _ in 0..2 {
            let current = value.current_result();
            assert_eq!(current.selection_event_id, 941);
            assert_eq!(current.status, 0);
            assert!(!current.consumed);
            assert!(current.commit_text.is_empty());
            assert_eq!(value.pending_selection_event_id.get(), 941);
            assert_eq!(value.host_preceding_text.as_deref(), Some("前文"));
            assert!(value.pending_context_profile.is_some());
        }
        let finished = value.process(event(KeyKind::Unsupported, ""));
        assert_eq!(finished.selection_event_id, 941);
        assert_eq!(value.current_result().selection_event_id, 0);
        assert!(value.host_preceding_text.is_none());
        assert!(value.pending_context_profile.is_none());
    }

    #[test]
    fn linear_defaults_match_selection_and_session_navigation() {
        for (linear, ordinary) in [(KeyKind::LinearNext, KeyKind::Down),
                                   (KeyKind::LinearPrevious, KeyKind::Up)] {
            let mut direct = MockDecoder::default();
            let mut defaulted = MockDecoder::default();
            direct.input("abc").unwrap();
            defaulted.input("abc").unwrap();
            for _ in 0..2 {
                direct.select_next().unwrap();
                defaulted.select_next().unwrap();
            }
            for _ in 0..8 {
                let expected = if ordinary == KeyKind::Down { direct.select_next() }
                    else { direct.select_previous() };
                let actual = if linear == KeyKind::LinearNext { defaulted.select_next_linear() }
                    else { defaulted.select_previous_linear() };
                assert_eq!(actual, expected);
                let actual = defaulted.snapshot();
                let expected = direct.snapshot();
                assert_eq!(actual.candidates, expected.candidates);
                assert_eq!(actual.highlighted_index, expected.highlighted_index);
                assert_eq!(actual.page_no, expected.page_no);
                assert_eq!(actual.is_last_page, expected.is_last_page);
            }
            let mut baseline = session();
            let mut value = session();
            baseline.decoder = Box::new(MockDecoder::default());
            value.decoder = Box::new(MockDecoder::default());
            type_text(&mut baseline, "abc");
            type_text(&mut value, "abc");
            baseline.process(event(KeyKind::Down, ""));
            value.process(event(KeyKind::Down, ""));
            for _ in 0..8 {
                let expected = baseline.process(event(ordinary, ""));
                let actual = value.process(event(linear, ""));
                assert_eq!(actual.consumed, expected.consumed);
                assert_eq!(actual.status, expected.status);
                assert_eq!(actual.commit_text, expected.commit_text);
                assert_eq!(actual.selection_event_id, expected.selection_event_id);
                assert_menu_matches(&actual, &baseline.decoder.snapshot());
            }
        }
    }

    #[test]
    fn session_linear_keys_use_mixer_crossing_while_vertical_keys_stay_clamped() {
        let mut vertical = session();
        let mut linear = session();
        for value in [&mut vertical, &mut linear] {
            type_text(value, "abc");
            value.decoder.configure_fuzzy(1).unwrap();
        }
        let first = linear.decoder.snapshot();
        assert_eq!(first.candidates.len(), 5);
        assert!(!first.is_last_page);
        for _ in 0..5 {
            vertical.process(event(KeyKind::Down, ""));
            let moved = linear.process(event(KeyKind::LinearNext, ""));
            assert!(moved.consumed);
            assert!(moved.commit_text.is_empty());
            assert_eq!(moved.selection_event_id, 0);
        }
        assert_eq!(vertical.decoder.snapshot().page_no, 0);
        assert_eq!(vertical.decoder.snapshot().highlighted_index, 4);
        assert_eq!(linear.decoder.snapshot().page_no, 1);
        assert_eq!(linear.decoder.snapshot().highlighted_index, 0);
        let returned = linear.process(event(KeyKind::LinearPrevious, ""));
        assert_eq!(returned.page_no, 0);
        assert_eq!(returned.highlighted_index, 4);
        assert_eq!(returned.candidates, first.candidates);
        assert_menu_matches(&returned, &linear.decoder.snapshot());
    }

    #[test]
    fn linear_key_numbers_append_without_changing_existing_mapping() {
        assert_eq!(KeyKind::from_u32(9), Some(KeyKind::Up));
        assert_eq!(KeyKind::from_u32(10), Some(KeyKind::Down));
        assert_eq!(KeyKind::from_u32(18), Some(KeyKind::BlacklistCandidate));
        assert_eq!(KeyKind::from_u32(19), Some(KeyKind::Unsupported));
        assert_eq!(KeyKind::from_u32(20), Some(KeyKind::LinearNext));
        assert_eq!(KeyKind::from_u32(21), Some(KeyKind::LinearPrevious));
        assert_eq!(KeyKind::from_u32(22), Some(KeyKind::Unsupported));
    }

    #[test]
    fn inline_ascii_arrows_commit_and_pass_through_and_system_arrows_do_not_move() {
        for kind in [KeyKind::Left, KeyKind::Right, KeyKind::Space] {
            let mut value = session();
            let mut uppercase = event(KeyKind::Text, "A");
            uppercase.modifiers = MOD_SHIFT;
            assert_eq!(value.process(uppercase).marked_text, "A");
            let result = value.process(event(kind, ""));
            assert_eq!(result.commit_text, "A");
            assert_eq!(result.consumed, kind == KeyKind::Space);
            assert!(result.marked_text.is_empty());
        }
        for (kind, modifier) in [(KeyKind::Right, MOD_OPTION), (KeyKind::Right, MOD_CONTROL), (KeyKind::Left, MOD_COMMAND)] {
            let mut value = session();
            let before = type_text(&mut value, "shi");
            let mut key = event(kind, "");
            key.modifiers = modifier;
            let after = value.process(key);
            assert!(!after.consumed);
            assert_eq!(after.marked_cursor_utf8, before.marked_cursor_utf8);
            assert_eq!(after.highlighted_index, before.highlighted_index);
        }
    }

    #[test]
    fn host_context_digits_and_space_select_displayed_order_while_enter_remains_raw() {
        for (kind, text, expected) in [
            (KeyKind::Text, "1", "呢"),
            (KeyKind::Text, "2", "你"),
            (KeyKind::Enter, "", "ni"),
            (KeyKind::Space, "", "呢"),
        ] {
            let mut session = session();
            type_text(&mut session, "ni");
            assert_eq!(session.context_group_mask(), 0b111);
            session.reorder_context_candidates(&[1, 0, 2]).unwrap();
            assert_eq!(session.decoder.snapshot().candidates[0].text, "呢");
            assert_eq!(session.process(event(kind, text)).commit_text, expected);
        }
    }

    #[test]
    fn candidate_page_has_five_slots_and_digits_one_through_five_select_them() {
        assert!(include_str!("../../../resources/Rime/default.yaml").contains("page_size: 5"));

        let mut fifth = session();
        let composition = type_text(&mut fifth, "shi");
        assert_eq!(composition.candidates.len(), 5);
        assert!(!composition.is_last_page);
        assert_eq!(
            fifth.process(event(KeyKind::Text, "5")).commit_text,
            "候选五"
        );

        let mut sixth = session();
        type_text(&mut sixth, "shi");
        let out_of_page = sixth.process(event(KeyKind::Text, "6"));
        assert!(out_of_page.commit_text.is_empty());
        assert!(out_of_page.consumed);
        assert_eq!(out_of_page.marked_text, "shi");
    }

    #[test]
    fn secondary_digits_six_through_zero_select_parallel_candidates() {
        for (digit, expected) in SECONDARY_SELECTION_DIGITS.into_iter().zip([
            "第二一",
            "第二二",
            "第二三",
            "第二四",
            "第二五",
        ]) {
            let mut value = session();
            value.decoder = Box::<MockDecoder>::default();
            let composition = type_text(&mut value, "niha");
            assert_eq!(composition.candidates.len(), 5);
            assert_eq!(composition.secondary_candidates.len(), 5);
            let selected = value.process(event(KeyKind::Text, &digit.to_string()));
            assert_eq!(selected.commit_text, expected, "digit={digit}");
            assert!(selected.consumed, "digit={digit}");
        }
    }

    #[test]
    fn digits_six_to_zero_do_nothing_when_the_second_layer_is_off() {
        let mut value = session();
        value.decoder = Box::<MockDecoder>::default();
        value.configure_candidate_context(None, vec![], true, false);
        let composition = type_text(&mut value, "niha");
        assert_eq!(composition.secondary_candidates.len(), 5);
        let selected = value.process(event(KeyKind::Text, "6"));
        assert_eq!(selected.commit_text, "第二一");
        assert!(selected.consumed);

        value.configure_candidate_context(None, vec![], false, false);
        for digit in SECONDARY_SELECTION_DIGITS {
            let composition = type_text(&mut value, "niha");
            assert!(composition.secondary_candidates.is_empty(), "digit={digit}");
            let result = value.process(event(KeyKind::Text, &digit.to_string()));
            assert!(result.commit_text.is_empty(), "digit={digit}");
            assert!(result.secondary_candidates.is_empty(), "digit={digit}");
            assert!(result.consumed, "digit={digit}");
            assert_eq!(result.marked_text, "niha", "digit={digit}");
            value.process(event(KeyKind::Escape, ""));
        }
    }

    #[test]
    fn decimal_period_stays_ascii_while_chinese_sentence_period_is_full_width() {
        let mut decimal = session();
        assert_eq!(type_visible_text(&mut decimal, "0.1"), "0.1");

        let mut pi = session();
        assert_eq!(type_visible_text(&mut pi, "3.14"), "3.14");

        let mut chinese = session();
        type_text(&mut chinese, "ni");
        assert_eq!(
            chinese.process(event(KeyKind::Text, ".")).commit_text,
            "你。"
        );
    }

    #[test]
    fn context_router_uses_static_app_classes_and_user_overrides() {
        assert_eq!(default_app_class("com.apple.dt.Xcode"), AppClass::Code);
        assert_eq!(
            default_app_class("com.anthropic.claudefordesktop"),
            AppClass::Code
        );
        assert_eq!(default_app_class("com.jetbrains.intellij"), AppClass::Code);
        assert_eq!(default_app_class("com.apple.TextEdit"), AppClass::Prose);
        assert_eq!(default_app_class("com.apple.Safari"), AppClass::Browser);
        assert_eq!(default_app_class("example.unlisted"), AppClass::Unknown);
        // Windows shells pass the process file name, in whatever case the system reports it.
        assert_eq!(default_app_class("Code.exe"), AppClass::Code);
        assert_eq!(default_app_class("WindowsTerminal.exe"), AppClass::Code);
        assert_eq!(default_app_class("WINWORD.EXE"), AppClass::Prose);
        assert_eq!(default_app_class("chrome.exe"), AppClass::Browser);
        assert_eq!(default_app_class("notepad.exe"), AppClass::Unknown);

        let mut code = Session::new(
            Box::new(MockDecoder::default()),
            "com.apple.dt.Xcode".into(),
        );
        assert!(!code.process(event(KeyKind::Text, ",")).consumed);
        assert_eq!(code.configure_context_profile(2, 0), Ok(()));
        assert_eq!(code.process(event(KeyKind::Text, ",")).commit_text, "，");
        assert_eq!(code.configure_context_profile(0, 1), Ok(()));
        assert_eq!(code.process(event(KeyKind::Text, ".")).commit_text, "。");
        assert_eq!(code.configure_context_profile(0, 2), Ok(()));
        assert!(!code.process(event(KeyKind::Text, ".")).consumed);
        assert_eq!(code.configure_context_profile(5, 0), Err(-13));
        assert_eq!(code.configure_context_profile(0, 3), Err(-14));
    }

    #[test]
    fn preceding_script_wins_over_app_class_for_punctuation() {
        let mut code = Session::new(
            Box::new(MockDecoder::default()),
            "com.apple.dt.Xcode".into(),
        );
        code.previous_text_kind = PreviousTextKind::HanOrFullwidth;
        assert_eq!(code.process(event(KeyKind::Text, ",")).commit_text, "，");
        code.previous_text_kind = PreviousTextKind::Ascii;
        assert!(!code.process(event(KeyKind::Text, ".")).consumed);

        let mut prose = Session::new(
            Box::new(MockDecoder::default()),
            "com.apple.TextEdit".into(),
        );
        prose.previous_text_kind = PreviousTextKind::Ascii;
        assert!(!prose.process(event(KeyKind::Text, ".")).consumed);
        prose.previous_text_kind = PreviousTextKind::HanOrFullwidth;
        assert_eq!(prose.process(event(KeyKind::Text, ",")).commit_text, "，");
    }

    #[test]
    fn frozen_context_punctuation_examples_are_stable() {
        let mut decimal = session();
        assert_eq!(type_visible_text(&mut decimal, "0.1"), "0.1");

        let mut ascii = session();
        ascii.previous_text_kind = PreviousTextKind::Ascii;
        assert!(!ascii.process(event(KeyKind::Text, ".")).consumed); // v1.2
        ascii.previous_text_kind = PreviousTextKind::Ascii;
        assert!(!ascii.process(event(KeyKind::Text, "(")).consumed); // fn(x)

        let mut chinese = session();
        type_text(&mut chinese, "nihao");
        assert_eq!(
            chinese.process(event(KeyKind::Text, ",")).commit_text,
            "你好，"
        );

        let mut quotation = session();
        quotation.previous_text_kind = PreviousTextKind::HanOrFullwidth; // 他说
        assert_eq!(
            quotation.process(event(KeyKind::Text, ":")).commit_text,
            "："
        );
        assert_eq!(
            quotation.process(event(KeyKind::Text, "\"")).commit_text,
            "“"
        );
        type_text(&mut quotation, "hao");
        assert!(
            quotation
                .process(event(KeyKind::Text, "\""))
                .commit_text
                .ends_with('”')
        );
    }

    #[test]
    fn context_profile_change_waits_for_composition_boundary() {
        let mut session = Session::new(
            Box::new(MockDecoder::default()),
            "com.apple.TextEdit".into(),
        );
        type_text(&mut session, "ni");
        assert_eq!(session.configure_context_profile(1, 0), Ok(()));
        assert_eq!(session.context_profile.app_class, AppClass::Prose);
        assert_eq!(
            session.process(event(KeyKind::Text, ",")).commit_text,
            "你，"
        );
        assert_eq!(session.context_profile.app_class, AppClass::Code);
        session.previous_text_kind = PreviousTextKind::None;
        assert!(!session.process(event(KeyKind::Text, ",")).consumed);
    }

    #[test]
    fn code_context_biases_generated_ascii_terms_without_displacing_chinese() {
        let mut code = Session::new(Box::new(MockDecoder::default()), "com.openai.codex".into());
        let english = type_text(&mut code, "productinputs");
        assert_eq!(
            english.candidates.first().map(|item| item.text.as_str()),
            Some("ProductInputs")
        );
        code.process(event(KeyKind::Escape, ""));
        let chinese = type_text(&mut code, "nihao");
        assert_eq!(
            chinese.candidates.first().map(|item| item.text.as_str()),
            Some("你好")
        );

        let mut unknown = Session::new(Box::new(MockDecoder::default()), "example.unlisted".into());
        let baseline = type_text(&mut unknown, "productinputs");
        assert_ne!(
            baseline.candidates.first().map(|item| item.text.as_str()),
            Some("ProductInputs")
        );
        unknown.process(event(KeyKind::Escape, ""));
        assert_eq!(unknown.configure_context_profile(4, 0), Ok(()));
        let explicit_unknown = type_text(&mut unknown, "productinputs");
        assert_eq!(explicit_unknown.candidates, baseline.candidates);
    }

    #[test]
    fn code_ascii_bias_override_waits_until_composition_ends() {
        let mut prose = Session::new(
            Box::new(MockDecoder::default()),
            "com.apple.TextEdit".into(),
        );
        let before = type_text(&mut prose, "productinputs");
        assert_ne!(
            before.candidates.first().map(|item| item.text.as_str()),
            Some("ProductInputs")
        );
        assert_eq!(prose.configure_context_profile(1, 0), Ok(()));
        let during = prose.finish(false, String::new(), 0, SecurityState::NonSecure);
        assert_eq!(during.candidates, before.candidates);
        prose.process(event(KeyKind::Escape, ""));
        let after = type_text(&mut prose, "productinputs");
        assert_eq!(
            after.candidates.first().map(|item| item.text.as_str()),
            Some("ProductInputs")
        );
    }

    #[test]
    fn secure_and_unknown_security_states_disable_context_routing() {
        for security in [SecurityState::Secure, SecurityState::Unknown] {
            let mut session = session();
            session.previous_text_kind = PreviousTextKind::HanOrFullwidth;
            let result = session.process(KeyEvent {
                kind: KeyKind::Text,
                text: ",".into(),
                modifiers: 0,
                security,
                delivery_id: 0,
            });
            assert!(!result.consumed);
            assert!(result.commit_text.is_empty());
            assert!(result.secure_degraded);
        }
    }

    #[test]
    fn static_context_lookup_p99_is_below_one_millisecond_and_deterministic() {
        let app_ids = [
            "com.apple.dt.Xcode",
            "com.jetbrains.intellij",
            "com.apple.TextEdit",
            "com.apple.Safari",
            "example.unlisted",
        ];
        let expected = [
            AppClass::Code,
            AppClass::Code,
            AppClass::Prose,
            AppClass::Browser,
            AppClass::Unknown,
        ];
        let mut samples = Vec::with_capacity(10_000);
        for index in 0..10_000 {
            let started = Instant::now();
            assert_eq!(
                default_app_class(app_ids[index % app_ids.len()]),
                expected[index % expected.len()]
            );
            samples.push(started.elapsed().as_nanos());
        }
        samples.sort_unstable();
        let p99 = samples[(samples.len() * 99) / 100];
        eprintln!("context_route_samples=10000 p99_ns={p99} threshold_ns=1000000");
        assert!(p99 < 1_000_000, "static lookup p99 was {p99}ns");
    }

    #[test]
    fn frozen_punctuation_corpus_is_complete_and_routing_is_byte_deterministic() {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../quality/corpus/punctuation-context-v1.json"
        ))
        .unwrap();
        assert_eq!(corpus["version"], 1);
        let cases = corpus["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 29); // 28 + the `@` case added after the owner could not type `@` (2026-10-16)

        let mut baseline = None;
        for _ in 0..100 {
            let mut observed = Vec::new();
            for case in cases {
                let app_id = match case["appClass"].as_str().unwrap() {
                    "code" => "com.apple.dt.Xcode",
                    "prose" => "com.apple.TextEdit",
                    "browser" => "com.apple.Safari",
                    "unknown" => "example.unlisted",
                    other => panic!("unknown punctuation app class: {other}"),
                };
                let mut session = Session::new(Box::new(MockDecoder::default()), app_id.into());
                let policy = match case.get("policy").and_then(|value| value.as_str()) {
                    None => 0,
                    Some("fullwidth") => 1,
                    Some("ascii") => 2,
                    Some(other) => panic!("unknown punctuation policy: {other}"),
                };
                session.configure_context_profile(0, policy).unwrap();
                session.previous_text_kind = classify_last_text(case["previous"].as_str().unwrap());
                if case["id"] == "chinese-close-quote" {
                    session.double_quote_open = true;
                }
                let input = case["input"].as_str().unwrap();
                let result = session.process(event(KeyKind::Text, input));
                let visible = if result.consumed {
                    result.commit_text
                } else {
                    format!("{}{}", result.commit_text, input)
                };
                assert_eq!(
                    visible,
                    case["expected"].as_str().unwrap(),
                    "case={}",
                    case["id"]
                );
                observed.extend_from_slice(visible.as_bytes());
                observed.push(0);
            }
            if let Some(expected) = &baseline {
                assert_eq!(&observed, expected);
            } else {
                baseline = Some(observed);
            }
        }
    }

    #[test]
    fn double_character_chinese_punctuation_commits_atomically_and_backspace_passes_through() {
        for (input, expected) in [("^", "……"), ("_", "——")] {
            let mut value = session();
            value.previous_text_kind = PreviousTextKind::HanOrFullwidth;
            let punctuation = value.process(event(KeyKind::Text, input));
            assert!(punctuation.consumed);
            assert_eq!(punctuation.commit_text, expected);
            assert!(punctuation.marked_text.is_empty());
            let backspace = value.process(event(KeyKind::Backspace, ""));
            assert!(!backspace.consumed);
            assert!(backspace.commit_text.is_empty());
        }
    }

    #[test]
    fn at_sign_is_left_alone_and_the_middle_dot_is_on_the_backtick_key() {
        for (input, expected) in [("`", Some("·")), ("~", Some("～")), ("@", None), ("#", None), ("&", None), ("*", None)] {
            let mut value = session();
            value.previous_text_kind = PreviousTextKind::HanOrFullwidth;
            let result = value.process(event(KeyKind::Text, input));
            match expected {
                Some(text) => {
                    assert!(result.consumed, "{input} is mapped");
                    assert_eq!(result.commit_text, text);
                }
                None => assert_ne!(result.commit_text, "·", "{input} must not turn into a middle dot"),
            }
        }
    }

    #[test]
    fn book_title_marks_map_in_chinese_and_ascii_angle_brackets_pass_in_english() {
        let mut chinese = session();
        let mut title = String::new();
        title.push_str(&chinese.process(event(KeyKind::Text, "<")).commit_text);
        type_text(&mut chinese, "shuming");
        title.push_str(&chinese.process(event(KeyKind::Text, ">")).commit_text);
        assert_eq!(title, "《书名》");

        let mut english = session();
        english.process(event(KeyKind::ShiftTap, ""));
        assert_eq!(type_visible_text(&mut english, "<tag>"), "<tag>");
    }

    #[test]
    fn full_pinyin_space_commits_first_candidate() {
        let mut session = session();
        type_text(&mut session, "nihao");
        let result = session.process(event(KeyKind::Space, ""));
        assert_eq!(result.commit_text, "你好");
        assert!(result.marked_text.is_empty());
    }

    #[test]
    fn enter_raw_and_escape_cancel_are_distinct() {
        let mut session = session();
        type_text(&mut session, "ni");
        assert_eq!(session.process(event(KeyKind::Enter, "")).commit_text, "ni");
        type_text(&mut session, "ni");
        let cancelled = session.process(event(KeyKind::Escape, ""));
        assert!(cancelled.commit_text.is_empty());
        assert!(cancelled.marked_text.is_empty());
    }

    #[test]
    fn empty_digit_slot_on_chinese_page_is_consumed_without_mutating_composition() {
        let mut session = session();
        type_text(&mut session, "ni");
        let result = session.process(event(KeyKind::Text, "9"));
        assert!(result.commit_text.is_empty());
        assert!(result.consumed);
        assert_eq!(result.marked_text, "ni");
    }

    #[test]
    fn no_chinese_page_allows_alphanumeric_digit_extension() {
        let mut session = session();
        let result = type_text(&mut session, "utf8");
        assert_eq!(result.marked_text, "utf8");
        assert!(result.commit_text.is_empty());
    }

    #[test]
    fn explicit_raw_ascii_path_keeps_base64_typeable_without_stealing_digits() {
        let mut session = session();
        let composition = type_text(&mut session, "base");
        assert!(!composition.candidates.is_empty());
        assert!(page_has_chinese_candidate(&session.decoder.snapshot()));
        let selected = session.process(event(KeyKind::Enter, ""));
        assert_eq!(selected.commit_text, "base");
        assert_eq!(
            format!(
                "{}{}",
                selected.commit_text,
                type_visible_text(&mut session, "64")
            ),
            "base64"
        );
    }

    #[test]
    fn chinese_page_owns_digits_even_when_ascii_candidate_is_first() {
        let mut session = session();
        let composition = type_text(&mut session, "sha");
        assert_eq!(composition.candidates[0].text, "SHA");
        assert_eq!(composition.candidates[1].text, "啥");
        let selection = session.process(event(KeyKind::Text, "2"));
        assert_eq!(selection.commit_text, "啥");
        assert!(selection.marked_text.is_empty());
    }

    #[test]
    fn apostrophe_stays_in_composition_but_idle_quote_pairs() {
        let mut session = session();
        type_text(&mut session, "xi");
        let composing = session.process(event(KeyKind::Text, "'"));
        assert_eq!(composing.marked_text, "xi'");
        session.process(event(KeyKind::Escape, ""));
        assert_eq!(session.process(event(KeyKind::Text, "'")).commit_text, "‘");
        assert_eq!(session.process(event(KeyKind::Text, "'")).commit_text, "’");
    }

    #[test]
    fn shift_commits_composition_and_toggles_mode() {
        let mut session = session();
        type_text(&mut session, "ni");
        let toggled = session.process(event(KeyKind::ShiftTap, ""));
        assert_eq!(toggled.commit_text, "你");
        assert_eq!(toggled.mode, InputMode::DirectEnglish);
        assert!(!session.process(event(KeyKind::Text, "a")).consumed);
    }

    #[test]
    fn command_option_control_pass_through_without_ending_composition() {
        for modifier in [MOD_COMMAND, MOD_OPTION, MOD_CONTROL] {
            let mut session = session();
            type_text(&mut session, "ni");
            let result = session.process(KeyEvent {
                kind: KeyKind::Text,
                text: "c".into(),
                modifiers: modifier,
                security: SecurityState::NonSecure,
                delivery_id: 0,
            });
            assert!(!result.consumed);
            assert_eq!(result.marked_text, "ni");
        }
    }

    #[test]
    fn secure_and_unknown_are_marked_fail_closed() {
        for security in [SecurityState::Secure, SecurityState::Unknown] {
            let mut session = session();
            let result = session.process(KeyEvent {
                kind: KeyKind::Text,
                text: "n".into(),
                modifiers: 0,
                security,
                delivery_id: 0,
            });
            assert!(result.secure_degraded);
            assert!(!result.consumed);
            assert!(result.marked_text.is_empty());
            assert!(result.candidates.is_empty());
        }
    }

    #[test]
    fn preceding_text_is_frozen_for_one_composition_and_cleared_for_secure_input() {
        let mut session = session();
        session.set_host_preceding_text("0123456789中文上下文");
        let first = session.process(event(KeyKind::Text, "n"));
        assert_eq!(first.marked_text, "n");
        assert_eq!(
            session.host_preceding_text.as_deref(),
            Some("0123456789中文上下文")
        );

        session.set_host_preceding_text("不得中途替换");
        assert_eq!(
            session.host_preceding_text.as_deref(),
            Some("0123456789中文上下文")
        );

        let degraded = session.process(KeyEvent {
            kind: KeyKind::Text,
            text: "i".into(),
            modifiers: 0,
            security: SecurityState::Secure,
            delivery_id: 0,
        });
        assert!(degraded.secure_degraded);
        assert!(session.host_preceding_text.is_none());
        assert!(session.host_previous_text_kind.is_none());
    }

    #[test]
    fn sessions_do_not_share_composition() {
        let mut first = session();
        let second = session();
        type_text(&mut first, "ni");
        assert_eq!(first.decoder.snapshot().raw_input, "ni");
        assert!(second.decoder.snapshot().raw_input.is_empty());
    }

    #[test]
    fn no_candidate_space_commits_raw_and_ascii_space() {
        let mut session = session();
        type_text(&mut session, "nocandidate");
        assert_eq!(
            session.process(event(KeyKind::Space, "")).commit_text,
            "nocandidate "
        );
    }

    #[test]
    fn digit_selects_candidate_and_paging_reaches_second_page() {
        let mut session = session();
        type_text(&mut session, "abc");
        assert_eq!(
            session.process(event(KeyKind::Text, "2")).commit_text,
            "候选二"
        );
        type_text(&mut session, "abc");
        let page = session.process(event(KeyKind::PageDown, ""));
        assert_eq!(page.page_no, 1);
        assert_eq!(page.candidates.len(), 5);
        // The raw spelling takes a slot of the first page; the candidate it pushed off leads
        // the second page instead of being lost (owner 2026-10-12), then the decoder's page 2.
        assert_eq!(page.candidates[0].text, "候选五");
        assert_eq!(page.candidates[1].text, "候选六");
        assert_eq!(
            session.process(event(KeyKind::Text, "2")).commit_text,
            "候选六"
        );
    }

    #[test]
    fn selection_event_id_is_host_delivery_id_and_emitted_once() {
        let mut first = session();
        type_text(&mut first, "ni");
        let mut physical_selection = event(KeyKind::Text, "1");
        physical_selection.delivery_id = u64::MAX - 58;
        let first_selection = first.process(physical_selection.clone());
        assert_eq!(first_selection.selection_event_id, u64::MAX - 58);
        assert_eq!(
            first.process(event(KeyKind::Escape, "")).selection_event_id,
            0
        );

        let mut second = session();
        type_text(&mut second, "ni");
        let replayed_selection = second.process(physical_selection);
        assert_eq!(
            replayed_selection.selection_event_id,
            first_selection.selection_event_id
        );

        let mut without_host_identity = session();
        type_text(&mut without_host_identity, "ni");
        assert_eq!(
            without_host_identity
                .process(event(KeyKind::Text, "1"))
                .selection_event_id,
            0
        );
    }

    #[test]
    fn editing_keys_modify_only_composition_cursor() {
        let mut session = session();
        type_text(&mut session, "ni");
        session.process(event(KeyKind::Left, ""));
        session.process(event(KeyKind::Text, "h"));
        assert_eq!(session.decoder.snapshot().raw_input, "nhi");
        session.process(event(KeyKind::Backspace, ""));
        assert_eq!(session.decoder.snapshot().raw_input, "ni");
        session.process(event(KeyKind::Right, ""));
        assert_eq!(session.decoder.snapshot().cursor_utf8, 2);
    }

    #[test]
    fn editing_keys_commit_the_composition_and_reach_the_application() {
        for text in ["\t", "\u{F72B}", "\u{F729}", "\u{F728}"] {
            let mut value = session();
            let composition = type_text(&mut value, "ni");
            assert_eq!(composition.marked_text, "ni");
            assert!(value.composing());

            let result = value.process(event(KeyKind::Text, text));
            assert!(!result.consumed, "key={text:?}");
            assert_eq!(result.commit_text, "你", "key={text:?}");
            assert!(result.marked_text.is_empty(), "key={text:?}");
            assert!(!value.composing(), "key={text:?}");

            let idle = value.process(event(KeyKind::Text, text));
            assert!(!idle.consumed, "key={text:?}");
            assert!(idle.commit_text.is_empty(), "key={text:?}");
            assert!(!value.composing(), "key={text:?}");
        }
    }

    #[test]
    fn punctuation_commits_candidate_then_mapped_mark_and_ascii_mode_passes_through() {
        let mut session = session();
        type_text(&mut session, "ni");
        assert_eq!(
            session.process(event(KeyKind::Text, ",")).commit_text,
            "你，"
        );
        session.process(event(KeyKind::ShiftTap, ""));
        assert!(!session.process(event(KeyKind::Text, ",")).consumed);
    }

    #[test]
    fn unsupported_printable_commits_composition_before_host_pass_through() {
        let mut session = session();
        type_text(&mut session, "ni");
        let result = session.process(event(KeyKind::Text, "#"));
        assert_eq!(result.commit_text, "你");
        assert!(!result.consumed);
        assert!(result.marked_text.is_empty());
    }

    #[test]
    fn shifted_letter_and_caps_lock_pass_through_without_persistent_mode_change() {
        let mut session = session();
        let shifted = session.process(KeyEvent {
            kind: KeyKind::Text,
            text: "A".into(),
            modifiers: MOD_SHIFT,
            security: SecurityState::NonSecure,
            delivery_id: 0,
        });
        assert!(shifted.consumed);
        assert_eq!(shifted.marked_text, "A");
        assert_eq!(shifted.mode, InputMode::Chinese);
        session.process(event(KeyKind::Escape, ""));
        let caps = session.process(KeyEvent {
            kind: KeyKind::Text,
            text: "A".into(),
            modifiers: MOD_CAPS_LOCK,
            security: SecurityState::NonSecure,
            delivery_id: 0,
        });
        assert!(!caps.consumed);
        assert_eq!(caps.mode, InputMode::Chinese);
    }

    fn shift_key(text: &str) -> KeyEvent {
        KeyEvent {
            kind: KeyKind::Text,
            text: text.into(),
            modifiers: MOD_SHIFT,
            security: SecurityState::NonSecure,
            delivery_id: 0,
        }
    }

    #[test]
    fn shifted_letter_opens_inline_english_until_space() {
        let mut session = session();
        assert_eq!(session.process(shift_key("L")).marked_text, "L");
        assert_eq!(session.process(event(KeyKind::Text, "o")).marked_text, "Lo");
        session.process(shift_key("R"));
        let typed = session.process(shift_key("A"));
        assert!(typed.consumed && typed.candidates.is_empty());
        assert_eq!(typed.marked_text, "LoRA");
        let committed = session.process(event(KeyKind::Space, ""));
        assert_eq!(committed.commit_text, "LoRA");
        assert!(committed.marked_text.is_empty());
        assert_eq!(committed.mode, InputMode::Chinese);
        // Chinese resumes right after.
        assert!(!session.process(event(KeyKind::Text, "n")).marked_text.is_empty());
    }

    #[test]
    fn inline_english_edits_and_flushes_before_other_keys() {
        let mut session = session();
        type_text(&mut session, "ni");
        let opened = session.process(shift_key("G"));
        assert_eq!(opened.commit_text, "你");
        assert_eq!(opened.marked_text, "G");
        for key in ["P", "T", "5"] {
            session.process(KeyEvent { modifiers: 0, ..shift_key(key) });
        }
        assert_eq!(session.process(event(KeyKind::Backspace, "")).marked_text, "GPT");
        let flushed = session.process(event(KeyKind::Text, ","));
        assert!(flushed.commit_text.starts_with("GPT"));
        assert!(flushed.marked_text.is_empty());
        session.process(shift_key("X"));
        let dropped = session.process(event(KeyKind::Escape, ""));
        assert!(dropped.commit_text.is_empty() && dropped.marked_text.is_empty());
        session.process(shift_key("X"));
        let shift = session.process(event(KeyKind::ShiftTap, ""));
        assert_eq!(shift.commit_text, "X");
        assert_eq!(shift.mode, InputMode::Chinese);
    }

    #[test]
    fn focus_loss_commits_but_sleep_clears_without_duplicate_commit() {
        let mut session = session();
        type_text(&mut session, "ni");
        assert_eq!(
            session.process(event(KeyKind::FocusLost, "")).commit_text,
            "你"
        );
        type_text(&mut session, "ni");
        assert!(
            session
                .process(event(KeyKind::Sleep, ""))
                .commit_text
                .is_empty()
        );
        assert!(
            session
                .process(event(KeyKind::Resume, ""))
                .marked_text
                .is_empty()
        );
    }

    #[test]
    fn control_delete_is_the_only_control_composition_action() {
        let mut session = session();
        type_text(&mut session, "ni");
        let deleted = session.process(KeyEvent {
            kind: KeyKind::DeleteCandidate,
            text: String::new(),
            modifiers: MOD_CONTROL,
            security: SecurityState::NonSecure,
            delivery_id: 0,
        });
        assert!(deleted.consumed);
        assert_eq!(deleted.status, 0);
        assert_eq!(deleted.marked_text, "ni");

        for modifiers in [
            MOD_CONTROL | MOD_COMMAND,
            MOD_CONTROL | MOD_OPTION,
            MOD_CONTROL | MOD_SHIFT,
        ] {
            let combined = session.process(KeyEvent {
                kind: KeyKind::DeleteCandidate,
                text: String::new(),
                modifiers,
                security: SecurityState::NonSecure,
                delivery_id: 0,
            });
            assert!(!combined.consumed);
            assert_eq!(combined.marked_text, "ni");
        }
    }

    #[test]
    fn blacklist_action_is_exact_and_secure_unknown_fail_closed() {
        let mut nonsecure_session = session();
        type_text(&mut nonsecure_session, "ni");
        let action = nonsecure_session.process(KeyEvent {
            kind: KeyKind::BlacklistCandidate,
            text: String::new(),
            modifiers: MOD_SHIFT | MOD_CONTROL,
            security: SecurityState::NonSecure,
            delivery_id: 0,
        });
        assert!(action.consumed);
        assert_eq!(action.status, 0);
        assert!(
            action
                .candidates
                .iter()
                .all(|candidate| candidate.text != "你")
        );

        for security in [SecurityState::Secure, SecurityState::Unknown] {
            let mut session = session();
            type_text(&mut session, "ni");
            let result = session.process(KeyEvent {
                kind: KeyKind::BlacklistCandidate,
                text: String::new(),
                modifiers: MOD_SHIFT | MOD_CONTROL,
                security,
                delivery_id: 0,
            });
            assert!(!result.consumed);
            assert!(result.secure_degraded);
            assert!(result.marked_text.is_empty());
        }
    }

    #[test]
    fn mode_is_remembered_per_app_but_composition_is_not_shared() {
        let app = format!("mode-test-{}", std::process::id());
        let mut first = Session::new(Box::new(MockDecoder::default()), app.clone());
        first.process(event(KeyKind::ShiftTap, ""));
        let second = Session::new(Box::new(MockDecoder::default()), app);
        let other = Session::new(Box::new(MockDecoder::default()), "other-mode-test".into());
        assert_eq!(second.mode, InputMode::DirectEnglish);
        assert_eq!(other.mode, InputMode::Chinese);
        assert!(second.decoder.snapshot().raw_input.is_empty());
    }

    #[test]
    fn explicit_persisted_mode_restore_is_idempotent() {
        let app = format!("restored-mode-{}", NEXT_APP.fetch_add(1, Ordering::Relaxed));
        let mut value = Session::new(Box::new(MockDecoder::default()), app.clone());
        value.set_mode(InputMode::DirectEnglish);
        value.set_mode(InputMode::DirectEnglish);
        assert_eq!(value.mode, InputMode::DirectEnglish);
        assert_eq!(
            Session::new(Box::new(MockDecoder::default()), app).mode,
            InputMode::DirectEnglish
        );
    }

    #[test]
    fn fuzzy_mask_rejects_bits_outside_the_eight_supported_groups() {
        let mut value = session();
        type_text(&mut value, "ni");
        assert_eq!(value.configure_fuzzy(1 << 8), Err(-12));
        assert_eq!(value.decoder.snapshot().raw_input, "ni");
        assert!(value.configure_fuzzy(0xff).is_ok());
        assert!(value.decoder.snapshot().raw_input.is_empty());
    }

    #[test]
    fn product_native_rejects_busy_fuzzy_switches_in_both_directions() {
        let dictionary = crate::zhixu::fallback();
        let mut value = Session::new_native("y3.fuzzy-idle".into(),
            Arc::new(Mutex::new(Store::default())), dictionary).unwrap();
        assert!(value.decoder.native_editing());
        type_text(&mut value, "nihao");
        let before = value.decoder.snapshot();
        assert_eq!(value.configure_fuzzy(1), Err(-10));
        assert_eq!(value.configure_zhixu_whole(false), Err(-10));
        assert_eq!(value.decoder.snapshot().preedit, before.preedit);
        assert!(value.decoder.native_editing());
        value.decoder.clear();
        // F2: Zhixu reads fuzzy spellings itself, so fuzzy input stays native.
        value.configure_fuzzy(1).unwrap();
        assert!(value.decoder.native_editing());
        type_text(&mut value, "nihao");
        assert_eq!(value.configure_fuzzy(0), Err(-10));
        assert_eq!(value.decoder.snapshot().raw_input, "nihao");
        value.decoder.clear();
        value.configure_fuzzy(0).unwrap();
        assert!(value.decoder.native_editing());
        // The retained whole knob changes neither the inner nor fuzzy behavior.
        value.configure_zhixu_whole(false).unwrap();
        type_text(&mut value, "nihao");
        assert!(value.decoder.native_editing());
        assert_eq!(value.configure_fuzzy(1), Err(-10));
        assert_eq!(value.decoder.snapshot().raw_input, "nihao", "rejection must not clear a composing session");
    }

    #[test]
    fn frozen_behavior_golden_is_nonempty_complete_and_passing() {
        use std::collections::HashSet;

        const REQUIRED_IDS: [&str; 29] = [
            "idle-letter",
            "space-candidate",
            "multi-segment-space",
            "space-no-candidate",
            "digit-in-range",
            "digit-out-of-range",
            "page-contract",
            "up-down",
            "left-right",
            "backspace",
            "enter",
            "escape",
            "control-delete",
            "apostrophe",
            "punctuation",
            "idle-quotes",
            "shift-idle",
            "shift-composing",
            "shift-letter",
            "caps-lock",
            "system-modifiers",
            "unsupported-composing",
            "focus-lost",
            "deactivate",
            "sleep-resume",
            "secure",
            "unknown-security",
            "multi-session",
            "per-app-mode",
        ];
        let fixture = include_str!("../../../../quality/golden/behavior-v0.3.tsv");
        let ids = fixture
            .lines()
            .skip(1)
            .filter_map(|line| line.split('\t').next())
            .collect::<Vec<_>>();
        let unique = ids.iter().copied().collect::<HashSet<_>>();
        let required = REQUIRED_IDS.into_iter().collect::<HashSet<_>>();
        assert_eq!(
            ids.len(),
            REQUIRED_IDS.len(),
            "behavior golden row count changed"
        );
        assert_eq!(
            unique.len(),
            ids.len(),
            "behavior golden contains duplicate IDs"
        );
        assert_eq!(unique, required, "behavior golden required ID set changed");
        for id in ids {
            let passed = match id {
                "idle-letter" => !type_text(&mut session(), "ni").candidates.is_empty(),
                "space-candidate" => {
                    let mut value = session();
                    type_text(&mut value, "nihao");
                    value.process(event(KeyKind::Space, "")).commit_text == "你好"
                }
                "multi-segment-space" => {
                    let mut value = session();
                    type_text(&mut value, "nihao");
                    value.process(event(KeyKind::Space, "")).commit_text == "你好"
                }
                "space-no-candidate" => {
                    let mut value = session();
                    type_text(&mut value, "nocandidate");
                    value.process(event(KeyKind::Space, "")).commit_text == "nocandidate "
                }
                "digit-in-range" => {
                    let mut value = session();
                    type_text(&mut value, "ni");
                    value.process(event(KeyKind::Text, "2")).commit_text == "呢"
                }
                "digit-out-of-range" => {
                    let mut value = session();
                    type_text(&mut value, "nocandidate");
                    let result = value.process(event(KeyKind::Text, "9"));
                    result.commit_text.is_empty()
                        && result.consumed
                        && result.marked_text == "nocandidate9"
                }
                "page-contract" => {
                    let mut value = session();
                    type_text(&mut value, "abc");
                    value.process(event(KeyKind::PageDown, "")).page_no == 1
                        && value.process(event(KeyKind::PageUp, "")).page_no == 0
                }
                "up-down" => {
                    let mut value = session();
                    type_text(&mut value, "ni");
                    let down = value.process(event(KeyKind::Down, ""));
                    let up = value.process(event(KeyKind::Up, ""));
                    down.highlighted_index == 1 && up.highlighted_index == 0
                }
                "left-right" => {
                    let mut value = session();
                    type_text(&mut value, "ni");
                    value.process(event(KeyKind::Left, ""));
                    let left = value.decoder.snapshot().cursor_utf8;
                    value.process(event(KeyKind::Right, ""));
                    left == 1 && value.decoder.snapshot().cursor_utf8 == 2
                }
                "backspace" => {
                    let mut value = session();
                    type_text(&mut value, "ni");
                    value.process(event(KeyKind::Backspace, "")).marked_text == "n"
                }
                "enter" => {
                    let mut value = session();
                    type_text(&mut value, "ni");
                    value.process(event(KeyKind::Enter, "")).commit_text == "ni"
                }
                "escape" => {
                    let mut value = session();
                    type_text(&mut value, "ni");
                    let result = value.process(event(KeyKind::Escape, ""));
                    result.commit_text.is_empty() && result.marked_text.is_empty()
                }
                "control-delete" => {
                    let mut value = session();
                    type_text(&mut value, "ni");
                    let result = value.process(KeyEvent {
                        kind: KeyKind::DeleteCandidate,
                        text: String::new(),
                        modifiers: MOD_CONTROL,
                        security: SecurityState::NonSecure,
                        delivery_id: 0,
                    });
                    result.consumed && result.status == 0 && result.marked_text == "ni"
                }
                "apostrophe" => {
                    let mut value = session();
                    type_text(&mut value, "xi");
                    value.process(event(KeyKind::Text, "'")).marked_text == "xi'"
                }
                "punctuation" => {
                    let mut value = session();
                    type_text(&mut value, "ni");
                    value.process(event(KeyKind::Text, ",")).commit_text == "你，"
                }
                "idle-quotes" => {
                    let mut value = session();
                    value.process(event(KeyKind::Text, "'")).commit_text == "‘"
                        && value.process(event(KeyKind::Text, "'")).commit_text == "’"
                        && value.process(event(KeyKind::Text, "\"")).commit_text == "“"
                        && value.process(event(KeyKind::Text, "\"")).commit_text == "”"
                }
                "shift-idle" => {
                    session().process(event(KeyKind::ShiftTap, "")).mode == InputMode::DirectEnglish
                }
                "shift-composing" => {
                    let mut value = session();
                    type_text(&mut value, "ni");
                    let result = value.process(event(KeyKind::ShiftTap, ""));
                    result.commit_text == "你" && result.mode == InputMode::DirectEnglish
                }
                "shift-letter" => {
                    let mut value = session();
                    let result = value.process(KeyEvent {
                        kind: KeyKind::Text,
                        text: "A".into(),
                        modifiers: MOD_SHIFT,
                        security: SecurityState::NonSecure,
                        delivery_id: 0,
                    });
                    result.consumed && result.marked_text == "A" && result.mode == InputMode::Chinese
                }
                "caps-lock" => {
                    let mut value = session();
                    let result = value.process(KeyEvent {
                        kind: KeyKind::Text,
                        text: "A".into(),
                        modifiers: MOD_CAPS_LOCK,
                        security: SecurityState::NonSecure,
                        delivery_id: 0,
                    });
                    !result.consumed && result.mode == InputMode::Chinese
                }
                "system-modifiers" => {
                    [MOD_COMMAND, MOD_OPTION, MOD_CONTROL]
                        .into_iter()
                        .all(|modifier| {
                            let mut value = session();
                            type_text(&mut value, "ni");
                            let result = value.process(KeyEvent {
                                kind: KeyKind::Text,
                                text: "x".into(),
                                modifiers: modifier,
                                security: SecurityState::NonSecure,
                                delivery_id: 0,
                            });
                            !result.consumed && result.marked_text == "ni"
                        })
                }
                "unsupported-composing" => {
                    let mut value = session();
                    type_text(&mut value, "ni");
                    let result = value.process(event(KeyKind::Unsupported, ""));
                    !result.consumed && result.commit_text == "你"
                }
                "focus-lost" | "deactivate" => {
                    let mut value = session();
                    type_text(&mut value, "ni");
                    let kind = if id == "focus-lost" {
                        KeyKind::FocusLost
                    } else {
                        KeyKind::Deactivate
                    };
                    let first = value.process(event(kind, ""));
                    let second = value.process(event(kind, ""));
                    first.commit_text == "你" && second.commit_text.is_empty()
                }
                "sleep-resume" => {
                    let mut value = session();
                    type_text(&mut value, "ni");
                    let sleep = value.process(event(KeyKind::Sleep, ""));
                    let resume = value.process(event(KeyKind::Resume, ""));
                    sleep.commit_text.is_empty() && resume.marked_text.is_empty()
                }
                "secure" | "unknown-security" => {
                    let mut value = session();
                    let security = if id == "secure" {
                        SecurityState::Secure
                    } else {
                        SecurityState::Unknown
                    };
                    let result = value.process(KeyEvent {
                        kind: KeyKind::Text,
                        text: "n".into(),
                        modifiers: 0,
                        security,
                        delivery_id: 0,
                    });
                    result.secure_degraded
                        && !result.consumed
                        && result.marked_text.is_empty()
                        && result.candidates.is_empty()
                }
                "multi-session" => {
                    let mut first = session();
                    let second = session();
                    type_text(&mut first, "ni");
                    first.decoder.snapshot().raw_input == "ni"
                        && second.decoder.snapshot().raw_input.is_empty()
                }
                "per-app-mode" => {
                    let app = format!("golden-app-{}", NEXT_APP.fetch_add(1, Ordering::Relaxed));
                    let mut first = Session::new(Box::new(MockDecoder::default()), app.clone());
                    first.process(event(KeyKind::ShiftTap, ""));
                    Session::new(Box::new(MockDecoder::default()), app).mode
                        == InputMode::DirectEnglish
                }
                other => panic!("unimplemented behavior golden case: {other}"),
            };
            assert!(passed, "behavior golden failed: {id}");
        }
    }

    fn prefix_probe(raw: &str, preedit: &str) -> Option<String> {
        let snapshot = DecoderSnapshot {
            raw_input: raw.into(), preedit: preedit.into(), cursor_utf8: preedit.len(), ..Default::default()
        };
        let old = editing_confirmed_prefix(&snapshot).map(str::to_owned);
        let boundary = default_composition_boundary(&snapshot);
        assert_eq!(boundary.map(|value| value.confirmed), old);
        old
    }

    #[test]
    fn confirmed_prefix_boundary_is_not_guessed_when_a_pinyin_token_does_not_match() {
        // jv is shown as ju: the tail "hao" matches, "ju" does not, so the boundary is uncertain (used to return "ju ", repeating the pinyin).
        assert_eq!(prefix_probe("jvhao", "ju hao"), None);
        assert_eq!(prefix_probe("nvhai", "nu hai"), None);
        // clean cases keep working
        assert_eq!(prefix_probe("nihao", "ni hao").as_deref(), Some(""));
        assert_eq!(prefix_probe("lvhao", "lü hao").as_deref(), Some(""));
        assert_eq!(prefix_probe("nihaoma", "你好ma").as_deref(), Some("你好"));
        assert_eq!(prefix_probe("nihao", "AI ni hao").as_deref(), Some("AI "));
        assert_eq!(prefix_probe("nihao", "你好2 ni hao").as_deref(), Some("你好2 "));
    }

    #[test]
    fn editing_prefix_matches_complete_raw_suffix_tokens_and_umlaut() {
        for (raw, preedit, expected) in [
            ("nihaonihao", "你好AIni hao", Some("你好AI")),
            ("nihaonihao", "你好2026ni hao", Some("你好2026")),
            ("nihaonihao", "你好 AI ni hao", Some("你好 AI ")),
            ("nihaolvse", "你好AIlü se", Some("你好AI")),
            ("nihaolüse", "你好AIlü se", Some("你好AI")),
            ("nihaoxi'an", "你好AIxi an", Some("你好AI")),
            ("nihao", "ni hao", Some("")),
            ("nihao", "你好AIwronghao", None),
            ("nihao", "你好AI", None),
            ("", "你好AI", None),
        ] {
            let snapshot = DecoderSnapshot { raw_input: raw.into(), preedit: preedit.into(), ..Default::default() };
            assert_eq!(editing_confirmed_prefix(&snapshot), expected, "raw={raw:?}, preedit={preedit:?}");
            assert_eq!(default_composition_boundary(&snapshot)
                .map(|value| value.confirmed), expected.map(str::to_owned));
        }
    }

    mod zhixu_keys {
        use super::*;
        use crate::zhixu::loader::{DictionaryData, Entry};
        use crate::zhixu::Index;

        fn fixture(run: impl FnOnce()) {
            let mut entries = [
                ("你", "ni", 9000), ("呢", "ni", 8000), ("拟", "ni", 7000),
                ("你好", "ni hao", 6000), ("好", "hao", 8000),
                ("吗", "ma", 7000), ("嘛", "ma", 6000), ("马", "ma", 5000),
                ("西", "xi", 5000), ("先", "xian", 6000), ("安", "an", 4000),
                ("比如", "bi ru", 7000), ("比", "bi", 5000), ("如", "ru", 5000),
                ("中华", "zhong hua", 7000), ("中", "zhong", 6000), ("华", "hua", 5000),
                ("书名", "shu ming", 7000), ("书", "shu", 5000), ("名", "ming", 5000),
                ("巴塞", "ba se", 6000), ("巴", "ba", 5000), ("塞", "se", 5000),
                ("女", "nv", 5000),
            ].into_iter().map(|(text, reading, weight)| Entry {
                text: text.into(), reading: reading.into(), weight, layer: "pinyin_simp".into(),
            }).collect::<Vec<_>>();
            entries.extend("是时事十使世式市识师诗史石室视试士实始示势食失施适释拾饰氏尸湿蚀誓驶逝矢柿侍狮"
                .chars().enumerate().map(|(at, text)| Entry {
                    text: text.to_string(), reading: "shi".into(), weight: 9000 - at as u32,
                    layer: "pinyin_simp".into(),
                }));
            struct Reset(bool);
            impl Drop for Reset { fn drop(&mut self) { ZHIXU_KEY_TEST.set(self.0); } }
            crate::zhixu::with_index(Arc::new(Index::from_data(DictionaryData { entries })), || {
                let _reset = Reset(ZHIXU_KEY_TEST.replace(true));
                run();
            });
        }

        // Re-run the existing Session assertions with only the inner decoder
        // replaced. The ordinary copies above still exercise the test double.
        macro_rules! existing_key_test {
            ($name:ident) => { #[test] fn $name() { fixture(super::$name); } };
        }
        existing_key_test!(full_pinyin_space_commits_first_candidate);
        existing_key_test!(enter_raw_and_escape_cancel_are_distinct);
        existing_key_test!(empty_digit_slot_on_chinese_page_is_consumed_without_mutating_composition);
        existing_key_test!(apostrophe_stays_in_composition_but_idle_quote_pairs);
        existing_key_test!(shift_commits_composition_and_toggles_mode);
        existing_key_test!(command_option_control_pass_through_without_ending_composition);
        existing_key_test!(secure_and_unknown_are_marked_fail_closed);
        existing_key_test!(preceding_text_is_frozen_for_one_composition_and_cleared_for_secure_input);
        existing_key_test!(sessions_do_not_share_composition);
        existing_key_test!(selection_event_id_is_host_delivery_id_and_emitted_once);
        existing_key_test!(editing_keys_modify_only_composition_cursor);
        existing_key_test!(editing_keys_commit_the_composition_and_reach_the_application);
        existing_key_test!(punctuation_commits_candidate_then_mapped_mark_and_ascii_mode_passes_through);
        existing_key_test!(unsupported_printable_commits_composition_before_host_pass_through);
        existing_key_test!(shifted_letter_and_caps_lock_pass_through_without_persistent_mode_change);
        existing_key_test!(shifted_letter_opens_inline_english_until_space);
        existing_key_test!(inline_english_edits_and_flushes_before_other_keys);
        existing_key_test!(focus_loss_commits_but_sleep_clears_without_duplicate_commit);
        existing_key_test!(double_character_chinese_punctuation_commits_atomically_and_backspace_passes_through);
        existing_key_test!(at_sign_is_left_alone_and_the_middle_dot_is_on_the_backtick_key);
        existing_key_test!(book_title_marks_map_in_chinese_and_ascii_angle_brackets_pass_in_english);
        existing_key_test!(inline_ascii_arrows_commit_and_pass_through_and_system_arrows_do_not_move);
        existing_key_test!(host_context_digits_and_space_select_displayed_order_while_enter_remains_raw);
        existing_key_test!(decimal_period_stays_ascii_while_chinese_sentence_period_is_full_width);
        existing_key_test!(frozen_context_punctuation_examples_are_stable);
        existing_key_test!(secure_and_unknown_security_states_disable_context_routing);
        existing_key_test!(no_chinese_page_allows_alphanumeric_digit_extension);
        existing_key_test!(blacklist_action_is_exact_and_secure_unknown_fail_closed);
        existing_key_test!(explicit_raw_ascii_path_keeps_base64_typeable_without_stealing_digits);

        #[test]
        fn five_primary_digits_select_displayed_slots_and_other_digits_stay_owned() {
            fixture(|| {
                for digit in 1..=5 {
                    let mut value = session();
                    let shown = type_text(&mut value, "shi");
                    assert_eq!(shown.candidates.len(), 5);
                    let selected = value.process(event(KeyKind::Text, &digit.to_string()));
                    assert_eq!(selected.commit_text, shown.candidates[digit - 1].text);
                    assert!(selected.marked_text.is_empty());
                }
                let mut value = session();
                let shown = type_text(&mut value, "shi");
                for digit in SECONDARY_SELECTION_DIGITS {
                    let ignored = value.process(event(KeyKind::Text, &digit.to_string()));
                    assert!(ignored.consumed);
                    assert!(ignored.commit_text.is_empty());
                    assert_eq!(ignored.marked_text, shown.marked_text);
                    assert_eq!(ignored.candidates, shown.candidates);
                }
                value.process(event(KeyKind::PageDown, ""));
                let shown = value.decoder.snapshot();
                let selected = value.process(event(KeyKind::Text, "2"));
                assert_eq!(selected.commit_text, shown.candidates[1].text);
                assert!(selected.marked_text.is_empty());
            });
        }

        #[test]
        fn empty_candidate_page_space_commits_raw_and_ascii_space() {
            fixture(|| {
                let mut value = session();
                let shown = type_text(&mut value, "vqqv");
                assert!(shown.candidates.is_empty());
                assert_eq!(value.process(event(KeyKind::Space, "")).commit_text, "vqqv ");
            });
        }

        #[test]
        fn ordinary_input_snapshots_match_the_live_native_decoder() {
            fixture(|| {
                let mut value = session();
                for character in "nihaoma".chars() {
                    let typed = value.process(event(KeyKind::Text, &character.to_string()));
                    let snapshot = value.decoder.snapshot();
                    assert_eq!(typed.marked_text, snapshot.preedit);
                    assert_eq!(typed.marked_cursor_utf8, snapshot.cursor_utf8);
                    assert_eq!(typed.secondary_candidates, snapshot.secondary_candidates);
                    assert_menu_matches(&typed, &snapshot);
                }
                for kind in [KeyKind::Down, KeyKind::Left, KeyKind::Left, KeyKind::Right, KeyKind::PageDown] {
                    let moved = value.process(event(kind, ""));
                    assert_menu_matches(&moved, &value.decoder.snapshot());
                }
            });
        }

        #[test]
        fn caret_boundaries_keep_whole_menu_without_session_freezing() {
            fixture(|| {
                // SogouKeyModelTests.testCoreCaretBoundariesKeepWholeReadingMenuAndReturnToEnd.
                for (raw, boundaries) in [("nihao", vec![2, 0]), ("biru", vec![2, 0]),
                    ("bi'ru", vec![3, 0]), ("xian", vec![3, 2, 1, 0]), ("zhonghua", vec![5, 0])] {
                    let mut value = session();
                    type_text(&mut value, raw);
                    assert!(value.decoder.native_editing());
                    let whole = value.decoder.snapshot();
                    for &boundary in &boundaries {
                        let moved = value.process(event(KeyKind::Left, ""));
                        assert_eq!(moved.status, 0, "{raw}");
                        assert_eq!(moved.marked_cursor_utf8, boundary, "{raw}");
                        assert_menu_matches(&moved, &whole);
                    }
                    for &boundary in boundaries.iter().rev().skip(1) {
                        let moved = value.process(event(KeyKind::Right, ""));
                        assert_eq!(moved.marked_cursor_utf8, boundary, "{raw}");
                        assert_menu_matches(&moved, &whole);
                    }
                    let ended = value.process(event(KeyKind::Right, ""));
                    assert_eq!(ended.marked_cursor_utf8, ended.marked_text.len());
                    assert_menu_matches(&ended, &whole);
                }
            });
        }

        #[test]
        fn expanded_right_flow_and_peek_follow_four_consecutive_pages() {
            fixture(|| {
                let mut value = session();
                type_text(&mut value, "shi");
                let mut baseline = session();
                type_text(&mut baseline, "shi");
                let mut canonical = vec![baseline.decoder.snapshot().candidates];
                for _ in 0..6 {
                    canonical.push(baseline.process(event(KeyKind::PageDown, "")).candidates);
                }
                // The host's Down-to-expand uses pageDown plus three-page peek;
                // expansion state belongs to Swift, not the Decoder ABI.
                let expanded = value.process(event(KeyKind::PageDown, ""));
                assert_eq!((expanded.page_no, expanded.highlighted_index), (1, 0));
                assert_eq!(value.peek_following_pages(3), canonical[2..5]);
                for position in 6..=26 {
                    let moved = value.process(event(KeyKind::LinearNext, ""));
                    assert_eq!((moved.page_no, moved.highlighted_index), (position / 5, position % 5));
                    assert_eq!(moved.candidates, canonical[position / 5]);
                    assert!(moved.commit_text.is_empty());
                    assert_eq!(moved.selection_event_id, 0);
                }
                for position in (4..26).rev() {
                    let moved = value.process(event(KeyKind::LinearPrevious, ""));
                    assert_eq!((moved.page_no, moved.highlighted_index), (position / 5, position % 5));
                    assert_eq!(moved.candidates, canonical[position / 5]);
                }
            });
        }

        #[test]
        fn paging_preserves_column_and_peek_does_not_change_the_selection() {
            fixture(|| {
                let mut value = session();
                type_text(&mut value, "shi");
                for _ in 0..3 { value.process(event(KeyKind::Down, "")); }
                let first = value.decoder.snapshot();
                let second = value.process(event(KeyKind::PageDown, ""));
                assert_eq!((second.page_no, second.highlighted_index), (1, 3));
                let before = value.decoder.snapshot();
                let peeked = value.peek_following_pages(99);
                assert_eq!(peeked.len(), MAX_PEEK_PAGES);
                assert_menu_matches(&value.finish(true, String::new(), 0, SecurityState::NonSecure), &before);
                let next = value.process(event(KeyKind::PageDown, ""));
                assert_eq!((next.page_no, next.highlighted_index), (2, 3));
                assert_eq!(next.candidates, peeked[0]);
                value.process(event(KeyKind::PageUp, ""));
                let back = value.process(event(KeyKind::PageUp, ""));
                assert_menu_matches(&back, &first);
            });
        }

        #[test]
        fn editing_selection_uses_live_page_highlight_and_reordered_display() {
            fixture(|| {
                for page in [0, 1] {
                    for (kind, text, slot) in [(KeyKind::Space, "", 3), (KeyKind::Text, "2", 1)] {
                        let mut value = session();
                        type_text(&mut value, "shi");
                        if page > 0 { value.process(event(KeyKind::PageDown, "")); }
                        value.reorder_context_candidates(&[1, 0, 2, 3, 4]).unwrap();
                        for _ in 0..3 { value.process(event(KeyKind::Down, "")); }
                        let shown = value.decoder.snapshot();
                        let moved = value.process(event(KeyKind::Left, ""));
                        assert_menu_matches(&moved, &shown);
                        let mut key = event(kind, text);
                        key.delivery_id = 941;
                        let selected = value.process(key);
                        assert_eq!(selected.commit_text, shown.candidates[slot].text);
                        assert_eq!(selected.selection_event_id, 941);
                        assert!(selected.marked_text.is_empty());
                    }
                }
            });
        }

        #[test]
        fn editing_partial_pick_commits_displayed_word_and_confirmed_prefix_once() {
            fixture(|| {
                for (confirmed, space) in [(false, false), (false, true), (true, false), (true, true)] {
                    let mut value = session();
                    type_text(&mut value, if confirmed { "nihaoma" } else { "nihao" });
                    if confirmed {
                        let before = value.decoder.snapshot();
                        let slot = before.candidates.iter().position(|item| item.text == "你好").unwrap();
                        let partial = value.process(event(KeyKind::Text, &(slot + 1).to_string()));
                        assert!(partial.commit_text.is_empty());
                        assert!(partial.marked_text.starts_with("你好"));
                    }
                    let moved = value.process(event(KeyKind::Left, ""));
                    let target = if confirmed { "吗" } else { "你" };
                    let slot = moved.candidates.iter().position(|item| item.text == target).unwrap();
                    let selected = if space {
                        for _ in 0..slot { value.process(event(KeyKind::Down, "")); }
                        value.process(event(KeyKind::Space, ""))
                    } else {
                        value.process(event(KeyKind::Text, &(slot + 1).to_string()))
                    };
                    assert_eq!(selected.commit_text, if confirmed { "你好吗" } else { "你" });
                    assert!(selected.marked_text.is_empty());
                    assert!(!value.composing());
                }
            });
        }
    }

}

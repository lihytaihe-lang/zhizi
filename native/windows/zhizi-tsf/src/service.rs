use super::*;
use std::{
    cell::{Cell, RefCell},
    mem::ManuallyDrop,
    rc::{Rc, Weak},
    sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        mpsc, Mutex,
    },
};
use windows::Win32::{
    System::{Com::*, Registry::*, Variant::*},
    UI::{Input::KeyboardAndMouse::*, TextServices::*, WindowsAndMessaging::*},
};
use zhizi_host::{modifiers as mods, Candidate, Host, HostConfig, Key, View};
/// The key just mapped was the `= +` key without Shift (it reaches the core as PageDown): on the Mac that key also expands the
/// one-row panel to four rows (owner 2026-10-11); a real PageDown key must keep paging.
static EQUALS_KEY: AtomicBool = AtomicBool::new(false);
/// A held Backspace has just emptied the composition: its remaining autorepeats are swallowed so they do not go on to delete the text
/// before the caret (the Mac does the same). A fresh key-down, or any other key, ends it.
static BACKSPACE_REPEAT_GUARD: AtomicBool = AtomicBool::new(false);
fn is_repeat(l: LPARAM) -> bool {
    (l.0 >> 30) & 1 == 1
}
pub static EVER_ACTIVATED: AtomicBool = AtomicBool::new(false);
/// How long the first letter typed while the Host is still loading waits for it (the Mac core is ready at once; on Windows Rime and
/// the model load in the background and the keys used to pass straight through to the application meanwhile).
const LOAD_WAIT: std::time::Duration = std::time::Duration::from_millis(2500);
/// How long the 中/英 badge stays at the caret after Shift switches the mode.
pub const TOAST_FOR: std::time::Duration = std::time::Duration::from_millis(900);
fn data_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("Zhizi")
}
fn app_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_default()
}
// Rime has process-global state. Hosts remain apartment-owned; serialize all Rime entry points.
static CORE_GATE: Mutex<()> = Mutex::new(());
pub const WM_RANK: u32 = WM_APP + 41;
pub struct Grid {
    pub rows: Vec<Vec<Candidate>>,
    pub row: usize,
}
pub struct State {
    pub host: Option<Host>,
    loading: Option<mpsc::Receiver<std::result::Result<Host, String>>>,
    pub view: View,
    pub grid: Option<Grid>,
    pub hwnd: HWND,
    pub caret: RECT,
    pub context: Option<ITfContext>,
    pub composition: Option<ITfComposition>,
    pub client: u32,
    pub shift: bool,
    pub active: bool,
    pub error: bool,
    pub atom: u32,
    layout: Option<(ITfSource, u32)>,
    position_pending: bool,
    pub manager: Option<ITfThreadMgr>,
    /// Shift just switched the mode: the badge to show at the caret (true = English) and since when.
    pub toast: Option<(bool, std::time::Instant)>,
    /// The first letter already waited for a loading Host once; later keys never block.
    waited: bool,
}
impl Drop for State {
    fn drop(&mut self) {
        let _gate = CORE_GATE.lock().unwrap_or_else(|e| e.into_inner());
        // Destroy the Rime session (including a queued background result) under the same lock.
        drop(self.host.take());
        drop(self.loading.take());
    }
}
impl State {
    fn new() -> Self {
        Self {
            host: None,
            loading: None,
            view: View::default(),
            grid: None,
            hwnd: HWND::default(),
            caret: RECT {
                left: 30,
                top: 30,
                right: 31,
                bottom: 50,
            },
            context: None,
            composition: None,
            client: 0,
            shift: false,
            active: false,
            error: false,
            atom: 0,
            layout: None,
            position_pending: false,
            manager: None,
            toast: None,
            waited: false,
        }
    }
    /// The Host finished loading (or failed): take it over, with the person's saved switches and this application's saved mode.
    fn finish_load(&mut self, result: std::result::Result<Host, String>) {
        self.loading = None;
        match result {
            Ok(mut h) => {
                let _gate = CORE_GATE.lock().unwrap_or_else(|e| e.into_inner());
                h.set_typo_correction(zhizi_host::settings::load(&data_dir()).typo_correction);
                let english = zhizi_host::settings::english_mode(&data_dir(), &app_name());
                h.set_english_mode(english);
                drop(_gate);
                self.view.english_mode = english;
                self.host = Some(h);
                publish_mode(self, english);
                diagnostic("Host ready; ONNX model and recording enabled")
            }
            Err(_) => {
                self.error = true;
                diagnostic("Host/model initialization failed")
            }
        }
    }
    /// The first letter typed while the Host is loading waits a moment for it instead of passing through as plain English.
    fn wait_for_host(&mut self) {
        if self.host.is_some() || self.waited {
            return;
        }
        let Some(receiver) = &self.loading else { return };
        self.waited = true;
        if let Ok(result) = receiver.recv_timeout(LOAD_WAIT) {
            self.finish_load(result);
        }
    }
    fn step(&mut self, key: Key, modifiers: u32, secure: bool) -> View {
        let _gate = CORE_GATE.lock().unwrap_or_else(|e| e.into_inner());
        self.host
            .as_mut()
            .map(|h| h.process(key, modifiers, secure))
            .unwrap_or_default()
    }
    fn regrid(&mut self) {
        let _gate = CORE_GATE.lock().unwrap_or_else(|e| e.into_inner());
        let extra = self
            .host
            .as_mut()
            .map(|h| h.peek_following_pages(3))
            .unwrap_or_default();
        let mut rows = vec![self.view.candidates.clone()];
        // Host 0.1.0 returns one tab-delimited page per Candidate. Also accept
        // flattened candidates, so a Host parser correction remains compatible.
        if extra.iter().any(|c| c.text.contains('\t')) {
            rows.extend(extra.iter().map(|page| {
                page.text
                    .split('\t')
                    .filter(|t| !t.is_empty())
                    .map(|t| Candidate {
                        text: t.into(),
                        comment: String::new(),
                    })
                    .collect()
            }));
        } else {
            rows.extend(extra.chunks(5).map(|s| s.to_vec()));
        }
        self.grid = Some(Grid { rows, row: 0 });
    }
    fn process(&mut self, key: Key, modifiers: u32, secure: bool) -> View {
        if secure {
            self.grid = None;
            return self.step(key, modifiers, true);
        }
        if !self.view.preedit.is_empty()
            && key == Key::PageDown
            && self.grid.is_none()
            && EQUALS_KEY.swap(false, AtomicOrdering::Relaxed)
        {
            // `=` without Shift on a one-row panel expands it, as on the Mac; expanded, it pages four rows below.
            self.regrid();
            let mut view = self.view.clone();
            view.consumed = true;
            view.commit.clear();
            return view;
        }
        if !self.view.preedit.is_empty() && key == Key::Text("+".into()) {
            if self.grid.is_some() {
                self.grid = None
            } else {
                self.regrid()
            }
            let mut view = self.view.clone();
            view.consumed = true;
            view.commit.clear();
            return view;
        }
        if let Some(g) = &self.grid {
            let row = g.row;
            let rows = g.rows.len();
            let col = self.view.highlighted;
            let mut next = None;
            match key {
                Key::Escape => {
                    self.grid = None;
                    let mut v = self.view.clone();
                    v.consumed = true;
                    v.commit.clear();
                    return v;
                }
                Key::Left => next = Some(self.step(Key::Up, 0, false)),
                Key::Right => next = Some(self.step(Key::Down, 0, false)),
                Key::Down | Key::Up => {
                    let down = key == Key::Down;
                    if (down && row + 1 < rows) || (!down && row > 0) {
                        let mut v =
                            self.step(if down { Key::PageDown } else { Key::PageUp }, 0, false);
                        for _ in 0..col.min(v.candidates.len().saturating_sub(1)) {
                            v = self.step(Key::Down, 0, false)
                        }
                        self.grid.as_mut().unwrap().row = if down { row + 1 } else { row - 1 };
                        next = Some(v);
                    }
                }
                Key::PageDown | Key::PageUp => {
                    let down = key == Key::PageDown;
                    let steps = if down { rows - row } else { row + 4 };
                    if !down || rows == 4 {
                        for _ in 0..steps {
                            next = Some(self.step(
                                if down { Key::PageDown } else { Key::PageUp },
                                0,
                                false,
                            ))
                        }
                    }
                    if let Some(v) = next.take() {
                        self.view = v;
                        self.regrid();
                        next = Some(self.view.clone())
                    }
                }
                _ => {
                    self.grid = None;
                    return self.step(key, modifiers, false);
                }
            }
            let mut v = next.unwrap_or_else(|| self.view.clone());
            v.consumed = true;
            v.commit.clear();
            if let Some(g) = &mut self.grid {
                if g.row < g.rows.len() {
                    g.rows[g.row] = v.candidates.clone()
                }
            }
            return v;
        }
        self.step(key, modifiers, false)
    }
}
fn diagnostic(message: &str) {
    use std::io::Write;
    let dir =
        std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("Zhizi");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("shell-status.log"))
    {
        let _ = writeln!(f, "pid={} {}", std::process::id(), message);
    }
}
fn cpu_name() -> String {
    unsafe {
        let mut data = [0u16; 256];
        let mut size = (data.len() * 2) as u32;
        let result = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0"),
            w!("ProcessorNameString"),
            RRF_RT_REG_SZ,
            None,
            Some(data.as_mut_ptr() as _),
            Some(&mut size),
        );
        if result.is_ok() {
            String::from_utf16_lossy(&data[..(size as usize / 2).saturating_sub(1)])
                .trim()
                .into()
        } else {
            "unknown CPU".into()
        }
    }
}
fn begin_load(state: &Rc<RefCell<State>>) {
    let (send, recv) = mpsc::channel();
    let hwnd = state.borrow().hwnd.0 as usize;
    state.borrow_mut().loading = Some(recv);
    let root = install_dir();
    let data = data_dir();
    let app = app_name();
    std::thread::spawn(move || {
        let result = (|| -> std::result::Result<Host, String> {
            zhizi_host::ranking::onnx::init_runtime(&root.join("onnxruntime.dll"))?;
            diagnostic("ONNX runtime initialized");
            let threads = (std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(2)
                / 2)
            .clamp(1, 4);
            let scorer = zhizi_host::ranking::onnx::OnnxScorer::load(
                &root.join("Model/zhizi30m_v2.onnx"),
                &root.join("Model/itos.json"),
                threads,
            )?;
            let mut host = {
                let _gate = CORE_GATE.lock().unwrap_or_else(|e| e.into_inner());
                unsafe {
                    LoadLibraryExW(
                        PCWSTR(wide(&root.join("rime.dll").to_string_lossy()).as_ptr()),
                        None,
                        LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
                    )
                    .map_err(|_| "rime DLL load failed")?;
                }
                Host::open(&HostConfig {
                    shared_dir: root.join("Rime"),
                    user_dir: data.join("Rime"),
                    app_id: app,
                })?
            };
            diagnostic("Rime initialized");
            host.enable_model(
                Box::new(scorer),
                Some(Box::new(move || unsafe {
                    let _ = PostMessageW(Some(HWND(hwnd as _)), WM_RANK, WPARAM(0), LPARAM(0));
                })),
            );
            if let Ok(records) = std::fs::read_to_string(data.join("Quality/events.jsonl")) {
                host.set_owner_style(&records)
            }
            if zhizi_host::quality::consent_granted(&data) {
                host.enable_recording(zhizi_host::quality::RecorderConfig {
                    path: data.join("Quality/events.jsonl"),
                    platform: "windows".into(),
                    build: env!("CARGO_PKG_VERSION").into(),
                    device: cpu_name(),
                });
            }
            Ok(host)
        })();
        {
            let _gate = CORE_GATE.lock().unwrap_or_else(|e| e.into_inner());
            // SendError owns Host when the UI has already gone away.
            let _ = send.send(result);
        }
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as _)), WM_RANK, WPARAM(0), LPARAM(0));
        }
    });
}
pub fn poll(state: &Rc<RefCell<State>>) {
    let Ok(mut s) = state.try_borrow_mut() else {
        return;
    };
    if let Some(receiver) = &s.loading {
        if let Ok(result) = receiver.try_recv() {
            s.finish_load(result);
        }
    }
    if !s.active {
        return;
    }
    // the mode badge has been up long enough
    if matches!(s.toast, Some((_, at)) if at.elapsed() >= TOAST_FOR) {
        s.toast = None;
        if s.view.preedit.is_empty() {
            unsafe {
                let _ = ShowWindow(s.hwnd, SW_HIDE);
            }
        }
    }
    let _gate = CORE_GATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(v) = s.host.as_mut().and_then(|h| h.poll_ranking()) {
        if !s.view.preedit.is_empty() {
            if let Some(g) = &mut s.grid {
                g.rows[g.row] = v.candidates.clone()
            }
            s.view = v;
            drop(_gate);
            drop(s);
            panel::show(state);
        }
    }
}

#[implement(
    ITfTextInputProcessorEx,
    ITfKeyEventSink,
    ITfThreadMgrEventSink,
    ITfTextLayoutSink,
    ITfDisplayAttributeProvider
)]
pub struct Service {
    state: Rc<RefCell<State>>,
    manager: RefCell<Option<ITfThreadMgr>>,
    cookie: Cell<u32>,
}
impl Service {
    pub fn new() -> Self {
        OBJECTS.fetch_add(1, Ordering::SeqCst);
        Self {
            state: Rc::new(RefCell::new(State::new())),
            manager: RefCell::new(None),
            cookie: Cell::new(u32::MAX),
        }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        let hwnd = self.state.borrow().hwnd;
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}
impl ITfTextInputProcessor_Impl for Service_Impl {
    fn Activate(&self, manager: Ref<'_, ITfThreadMgr>, client: u32) -> Result<()> {
        self.ActivateEx(manager, client, 0)
    }
    fn Deactivate(&self) -> Result<()> {
        reset(&self.state, Key::Deactivate);
        if let Some(manager) = self.manager.borrow_mut().take() {
            unsafe {
                if let Ok(keys) = manager.cast::<ITfKeystrokeMgr>() {
                    let _ = keys.UnadviseKeyEventSink(self.state.borrow().client);
                }
                if let Ok(source) = manager.cast::<ITfSource>() {
                    let _ = source.UnadviseSink(self.cookie.get());
                }
            }
        }
        let mut s = self.state.borrow_mut();
        s.active = false;
        s.shift = false;
        s.toast = None;
        s.manager = None;
        unsafe {
            let _ = ShowWindow(s.hwnd, SW_HIDE);
        }
        Ok(())
    }
}
impl ITfTextInputProcessorEx_Impl for Service_Impl {
    fn ActivateEx(&self, manager: Ref<'_, ITfThreadMgr>, client: u32, _: u32) -> Result<()> {
        let manager = manager.as_ref().ok_or_else(error)?.clone();
        EVER_ACTIVATED.store(true, Ordering::SeqCst);
        static PANIC_HOOK: std::sync::Once = std::sync::Once::new();
        PANIC_HOOK.call_once(|| {
            std::panic::set_hook(Box::new(|info| {
                if let Some(place) = info.location() {
                    diagnostic(&format!("panic at {}:{}", place.file(), place.line()));
                }
            }))
        });
        self.state.borrow_mut().client = client;
        self.state.borrow_mut().active = true;
        if self.state.borrow().hwnd.0.is_null() {
            panel::create(&self.state)?;
            begin_load(&self.state);
        }
        unsafe {
            let cat: ITfCategoryMgr =
                CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
            self.state.borrow_mut().atom = cat.RegisterGUID(&ATTR_ZHIZI)?;
            let keys: ITfKeystrokeMgr = manager.cast()?;
            let sink: ITfKeyEventSink = self.to_interface();
            keys.AdviseKeyEventSink(client, &sink, true)?;
            let source: ITfSource = manager.cast()?;
            let sink: ITfThreadMgrEventSink = self.to_interface();
            self.cookie
                .set(source.AdviseSink(&ITfThreadMgrEventSink::IID, &sink)?);
        }
        {
            let mut s = self.state.borrow_mut();
            s.manager = Some(manager.clone());
            let english = s.view.english_mode;
            publish_mode(&s, english);
        }
        *self.manager.borrow_mut() = Some(manager);
        Ok(())
    }
}
/// Tells Windows the input mode (中 / A) so the taskbar indicator follows Shift: the keyboard conversion-mode compartment.
fn publish_mode(state: &State, english: bool) {
    let Some(manager) = &state.manager else { return };
    unsafe {
        let Ok(compartments) = manager.cast::<ITfCompartmentMgr>() else { return };
        for (guid, value) in [
            (&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE, 1),
            (&GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION, if english { 0 } else { 1 }),
        ] {
            if let Ok(compartment) = compartments.GetCompartment(guid) {
                let _ = compartment.SetValue(state.client, &VARIANT::from(value));
            }
        }
    }
}
fn modifiers() -> u32 {
    unsafe {
        let mut m = 0;
        for (vk, bit) in [
            (VK_SHIFT, mods::SHIFT),
            (VK_CONTROL, mods::CONTROL),
            (VK_MENU, mods::ALT),
            (VK_LWIN, mods::META),
            (VK_RWIN, mods::META),
        ] {
            if GetKeyState(vk.0 as i32) < 0 {
                m |= bit
            }
        }
        if GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0 {
            m |= mods::CAPS_LOCK
        }
        m
    }
}
/// Ctrl+Backspace while composing removes the highlighted learned candidate, Ctrl+Shift+Backspace blacklists it (the Mac's shortcuts).
fn candidate_edit_chord(m: u32) -> Option<Key> {
    if m & mods::CONTROL == 0 || m & (mods::ALT | mods::META) != 0 {
        return None;
    }
    Some(if m & mods::SHIFT == 0 { Key::DeleteCandidate } else { Key::BlacklistCandidate })
}
fn map_key(vk: u32, lparam: LPARAM) -> Option<Key> {
    EQUALS_KEY.store(false, AtomicOrdering::Relaxed);
    Some(match vk {
        8 => Key::Backspace,
        13 => Key::Enter,
        27 => Key::Escape,
        32 => Key::Space,
        33 => Key::PageUp,
        34 => Key::PageDown,
        37 => Key::Left,
        38 => Key::Up,
        39 => Key::Right,
        40 => Key::Down,
        20 => Key::CapsLock,
        // Tab / End / Home / Delete: with letters being composed the core commits them and hands the key on (the Mac does the same
        // through the function-key characters AppKit gives these keys); with nothing composed the shell never claims them.
        9 => Key::Text("\t".into()),
        35 => Key::Text("\u{F72B}".into()),
        36 => Key::Text("\u{F729}".into()),
        46 => Key::Text("\u{F728}".into()),
        _ => unsafe {
            let mut keyboard = [0u8; 256];
            GetKeyboardState(&mut keyboard).ok()?;
            let mut text = [0u16; 8];
            let n = ToUnicode(
                vk,
                ((lparam.0 >> 16) & 255) as u32,
                Some(&keyboard),
                &mut text,
                4,
            );
            if n <= 0 {
                return None;
            }
            let text = String::from_utf16_lossy(&text[..n as usize]);
            if text.chars().any(|c| c.is_control()) {
                return None;
            }
            if text == "=" {
                EQUALS_KEY.store(true, AtomicOrdering::Relaxed);
                Key::PageDown
            } else if text == "-" {
                Key::PageUp
            } else {
                Key::Text(text)
            }
        },
    })
}
impl ITfKeyEventSink_Impl for Service_Impl {
    fn OnSetFocus(&self, foreground: BOOL) -> Result<()> {
        if !foreground.as_bool() {
            reset(&self.state, Key::FocusLost)
        }
        Ok(())
    }
    fn OnTestKeyDown(&self, _: Ref<'_, ITfContext>, w: WPARAM, l: LPARAM) -> Result<BOOL> {
        let mut s = self.state.borrow_mut();
        if [16, 160, 161].contains(&w.0) {
            diagnostic("Shift test down");
            if modifiers() & (mods::CONTROL | mods::ALT | mods::META) == 0 {
                s.shift = s.host.is_some();
            }
            // TSF must own the modifier down event to route its matching key-up.
            return Ok(s.shift.into());
        }
        s.shift = false;
        if (0x41..=0x5A).contains(&w.0) && modifiers() & (mods::CONTROL | mods::ALT | mods::META) == 0 {
            s.wait_for_host();
        }
        if w.0 == 8 && is_repeat(l) && BACKSPACE_REPEAT_GUARD.load(AtomicOrdering::Relaxed) {
            return Ok(TRUE);
        }
        if !(w.0 == 8 && is_repeat(l)) {
            BACKSPACE_REPEAT_GUARD.store(false, AtomicOrdering::Relaxed);
        }
        if w.0 == 8 && candidate_edit_chord(modifiers()).is_some() && !s.view.preedit.is_empty() && s.host.is_some() {
            return Ok(TRUE);
        }
        if s.host.is_none() || modifiers() & (mods::CONTROL | mods::ALT | mods::META) != 0 {
            return Ok(FALSE);
        }
        // IMM/TSF bridge clients can swallow a key once TestKeyDown claims it,
        // even when OnKeyDown later declines it. Do not claim known pass-through keys.
        if (s.view.english_mode && w.0 != 20)
            || map_key(w.0 as u32, l).is_none()
            || (s.view.preedit.is_empty()
                && [8, 9, 13, 27, 32, 33, 34, 35, 36, 37, 38, 39, 40, 46, 187, 189].contains(&w.0))
        {
            // These edits belong to the application. Avoid retaining stale context.
            if let Some(host) = &mut s.host {
                host.clear_context();
            }
            return Ok(FALSE);
        }
        Ok(TRUE)
    }
    fn OnTestKeyUp(&self, _: Ref<'_, ITfContext>, w: WPARAM, _: LPARAM) -> Result<BOOL> {
        if [16, 160, 161].contains(&w.0) {
            diagnostic("Shift test up");
        }
        Ok((self.state.borrow().shift && [16, 160, 161].contains(&w.0)).into())
    }
    fn OnKeyDown(&self, ctx: Ref<'_, ITfContext>, w: WPARAM, l: LPARAM) -> Result<BOOL> {
        if let (8, Some(edit)) = (w.0, candidate_edit_chord(modifiers())) {
            if !self.state.borrow().view.preedit.is_empty() {
                return execute(&self.state, ctx.as_ref(), edit, modifiers());
            }
        }
        if modifiers() & (mods::CONTROL | mods::ALT | mods::META) != 0 {
            return Ok(FALSE);
        }
        if [16, 160, 161].contains(&w.0) {
            diagnostic("Shift down");
            return Ok(self.state.borrow().shift.into());
        }
        let Some(key) = map_key(w.0 as u32, l) else {
            return Ok(FALSE);
        };
        if w.0 == 8 && is_repeat(l) && BACKSPACE_REPEAT_GUARD.load(AtomicOrdering::Relaxed) {
            return Ok(TRUE);
        }
        let was_composing = !self.state.borrow().view.preedit.is_empty();
        let handled = execute(&self.state, ctx.as_ref(), key, modifiers());
        let emptied = was_composing && w.0 == 8 && matches!(handled, Ok(h) if h.as_bool()) && self.state.borrow().view.preedit.is_empty();
        BACKSPACE_REPEAT_GUARD.store(emptied, AtomicOrdering::Relaxed);
        handled
    }
    fn OnKeyUp(&self, ctx: Ref<'_, ITfContext>, w: WPARAM, _: LPARAM) -> Result<BOOL> {
        let alone = self.state.borrow().shift && [16, 160, 161].contains(&w.0);
        self.state.borrow_mut().shift = false;
        if alone {
            let result = execute(&self.state, ctx.as_ref(), Key::ShiftTap, 0);
            let english = self.state.borrow().view.english_mode;
            diagnostic(if english { "Mode English" } else { "Mode Chinese" });
            {
                let mut s = self.state.borrow_mut();
                s.toast = Some((english, std::time::Instant::now()));
                publish_mode(&s, english);
            }
            let _ = zhizi_host::settings::save_english_mode(&data_dir(), &app_name(), english);
            panel::show(&self.state);
            result
        } else {
            Ok(FALSE)
        }
    }
    fn OnPreservedKey(&self, _: Ref<'_, ITfContext>, _: *const GUID) -> Result<BOOL> {
        Ok(FALSE)
    }
}
impl ITfThreadMgrEventSink_Impl for Service_Impl {
    fn OnInitDocumentMgr(&self, _: Ref<'_, ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }
    fn OnUninitDocumentMgr(&self, _: Ref<'_, ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }
    fn OnSetFocus(&self, _: Ref<'_, ITfDocumentMgr>, _: Ref<'_, ITfDocumentMgr>) -> Result<()> {
        reset(&self.state, Key::FocusLost);
        let app = app_name();
        let switches = zhizi_host::settings::load(&data_dir());
        let _gate = CORE_GATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(h) = self.state.borrow_mut().host.as_mut() {
            h.set_app(&app);
            h.set_typo_correction(switches.typo_correction)
        }
        Ok(())
    }
    fn OnPushContext(&self, _: Ref<'_, ITfContext>) -> Result<()> {
        reset(&self.state, Key::FocusLost);
        Ok(())
    }
    fn OnPopContext(&self, _: Ref<'_, ITfContext>) -> Result<()> {
        reset(&self.state, Key::FocusLost);
        Ok(())
    }
}
impl ITfTextLayoutSink_Impl for Service_Impl {
    fn OnLayoutChange(
        &self,
        _: Ref<'_, ITfContext>,
        _: TfLayoutCode,
        _: Ref<'_, ITfContextView>,
    ) -> Result<()> {
        panel::show(&self.state);
        Ok(())
    }
}
impl ITfDisplayAttributeProvider_Impl for Service_Impl {
    fn EnumDisplayAttributeInfo(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        Ok(display::Attributes(Cell::new(false)).into())
    }
    fn GetDisplayAttributeInfo(&self, guid: *const GUID) -> Result<ITfDisplayAttributeInfo> {
        if guid.is_null() || unsafe { *guid } != ATTR_ZHIZI {
            return Err(E_INVALIDARG.into());
        }
        Ok(display::Attribute.into())
    }
}
pub fn execute(
    state: &Rc<RefCell<State>>,
    ctx: Option<&ITfContext>,
    key: Key,
    mods: u32,
) -> Result<BOOL> {
    let Some(ctx) = ctx else { return Ok(FALSE) };
    if state.borrow().context.as_ref() != Some(ctx) {
        let mut s = state.borrow_mut();
        if let Some((source, cookie)) = s.layout.take() {
            unsafe {
                let _ = source.UnadviseSink(cookie);
            }
        }
        s.context = Some(ctx.clone());
        unsafe {
            if let Ok(source) = ctx.cast::<ITfSource>() {
                let sink: ITfTextLayoutSink = LayoutSink {
                    state: Rc::downgrade(state),
                }
                .into();
                if let Ok(cookie) = source.AdviseSink(&ITfTextLayoutSink::IID, &sink) {
                    s.layout = Some((source, cookie));
                }
            }
        }
    }
    let consumed = Rc::new(Cell::new(false));
    let session: ITfEditSession = Edit {
        state: state.clone(),
        ctx: ctx.clone(),
        key: Some(key),
        mods,
        consumed: consumed.clone(),
    }
    .into();
    let client = state.borrow().client;
    let result = unsafe { ctx.RequestEditSession(client, &session, TF_ES_SYNC | TF_ES_READWRITE) };
    if !matches!(result,Ok(hr) if hr.is_ok()) {
        return Ok(FALSE);
    }
    Ok(consumed.get().into())
}
fn reset(state: &Rc<RefCell<State>>, key: Key) {
    let (ctx, client, comp) = {
        let mut s = state.borrow_mut();
        let english_mode = s.step(key, 0, false).english_mode;
        s.grid = None;
        s.view = View {
            english_mode,
            ..View::default()
        };
        s.shift = false;
        if let Some(h) = s.host.as_mut() {
            h.clear_context()
        }
        if let Some((source, cookie)) = s.layout.take() {
            unsafe {
                let _ = source.UnadviseSink(cookie);
            }
        }
        (s.context.take(), s.client, s.composition.take())
    };
    // A delayed edit owns only the old composition: it cannot reset a newly focused Host.
    if let (Some(ctx), Some(comp)) = (ctx, comp) {
        let session: ITfEditSession = Cleanup { comp }.into();
        unsafe {
            let _ = ctx.RequestEditSession(client, &session, TF_ES_ASYNCDONTCARE | TF_ES_READWRITE);
        }
    }
    panel::hide(state);
}
#[implement(ITfEditSession)]
struct Cleanup {
    comp: ITfComposition,
}
impl ITfEditSession_Impl for Cleanup_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        unsafe {
            let range = self.comp.GetRange()?;
            range.SetText(ec, 0, &[])?;
            self.comp.EndComposition(ec)
        }
    }
}
#[implement(ITfTextLayoutSink)]
struct LayoutSink {
    state: Weak<RefCell<State>>,
}
impl ITfTextLayoutSink_Impl for LayoutSink_Impl {
    fn OnLayoutChange(
        &self,
        ctx: Ref<'_, ITfContext>,
        code: TfLayoutCode,
        _: Ref<'_, ITfContextView>,
    ) -> Result<()> {
        let Some(state) = self.state.upgrade() else {
            return Ok(());
        };
        if code == TF_LC_DESTROY {
            panel::hide(&state);
            return Ok(());
        }
        let Some(ctx) = ctx.as_ref() else {
            return Ok(());
        };
        let Ok(mut s) = state.try_borrow_mut() else {
            return Ok(());
        };
        if s.position_pending || s.view.preedit.is_empty() {
            return Ok(());
        }
        let client = s.client;
        s.position_pending = true;
        drop(s);
        let edit: ITfEditSession = Position {
            state: Rc::downgrade(&state),
            ctx: ctx.clone(),
        }
        .into();
        unsafe {
            if ctx
                .RequestEditSession(client, &edit, TF_ES_ASYNC | TF_ES_READ)
                .is_err()
            {
                state.borrow_mut().position_pending = false;
            }
        }
        Ok(())
    }
}
#[implement(ITfEditSession)]
struct Position {
    state: Weak<RefCell<State>>,
    ctx: ITfContext,
}
impl ITfEditSession_Impl for Position_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        unsafe {
            let Some(state) = self.state.upgrade() else {
                return Ok(());
            };
            state.borrow_mut().position_pending = false;
            if state.borrow().context.as_ref() != Some(&self.ctx) {
                return Ok(());
            }
            let caret = selection(&self.ctx, ec)?;
            let mut rect = RECT::default();
            let mut clipped = FALSE;
            self.ctx
                .GetActiveView()?
                .GetTextExt(ec, &caret, &mut rect, &mut clipped)?;
            state.borrow_mut().caret = rect;
            panel::show(&state);
            Ok(())
        }
    }
}
unsafe fn selection(ctx: &ITfContext, ec: u32) -> Result<ITfRange> {
    let mut selections = [TF_SELECTION::default()];
    let mut n = 0;
    ctx.GetSelection(ec, TF_DEFAULT_SELECTION, &mut selections, &mut n)?;
    if n == 0 {
        return Err(E_FAIL.into());
    }
    ManuallyDrop::take(&mut selections[0].range).ok_or_else(error)
}
/// Up to 32 UTF-16 units of the document right before the start of `range`; None when the application cannot be read.
unsafe fn preceding_text(range: &ITfRange, ec: u32) -> Option<String> {
    const UNITS: i32 = 32;
    let before = range.Clone().ok()?;
    before.Collapse(ec, TF_ANCHOR_START).ok()?;
    let mut shifted = 0;
    before.ShiftStart(ec, -UNITS, &mut shifted, std::ptr::null()).ok()?;
    if shifted == 0 {
        return Some(String::new());
    }
    let mut buffer = [0u16; UNITS as usize + 2];
    let mut fetched = 0u32;
    before.GetText(ec, 0, &mut buffer, &mut fetched).ok()?;
    let text = String::from_utf16_lossy(&buffer[..(fetched as usize).min(buffer.len())]);
    // a surrogate pair cut in two at the far end, and object placeholders, are not text
    Some(text.trim_start_matches('\u{FFFD}').replace('\u{FFFC}', ""))
}
unsafe fn secure(ctx: &ITfContext, ec: u32, range: &ITfRange) -> bool {
    if let Ok(status) = ctx.GetStatus() {
        if status.dwDynamicFlags & TS_SD_READONLY != 0 {
            return true;
        }
    }
    if let Ok(prop) = ctx.GetAppProperty(&GUID_PROP_INPUTSCOPE) {
        if let Ok(value) = prop.GetValue(ec, range) {
            if let Ok(unknown) = IUnknown::try_from(&value) {
                if let Ok(scope) = unknown.cast::<ITfInputScope>() {
                    let mut list = std::ptr::null_mut();
                    let mut count = 0;
                    if scope.GetInputScopes(&mut list, &mut count).is_ok() {
                        let password = !list.is_null()
                            && std::slice::from_raw_parts(list, count as usize)
                                .iter()
                                .any(|s| *s == IS_PASSWORD);
                        CoTaskMemFree(Some(list as _));
                        if password {
                            return true;
                        }
                    }
                }
            }
        }
    }
    // Legacy Edit password controls may omit TSF input scope.
    if let Ok(view) = ctx.GetActiveView() {
        if let Ok(hwnd) = view.GetWnd() {
            let mut class = [0u16; 128];
            let n = GetClassNameW(hwnd, &mut class);
            if String::from_utf16_lossy(&class[..n.max(0) as usize]).eq_ignore_ascii_case("Edit")
                && GetWindowLongW(hwnd, GWL_STYLE) as u32 & ES_PASSWORD as u32 != 0
            {
                return true;
            }
        }
    }
    false
}
#[implement(ITfEditSession)]
struct Edit {
    state: Rc<RefCell<State>>,
    ctx: ITfContext,
    key: Option<Key>,
    mods: u32,
    consumed: Rc<Cell<bool>>,
}
impl ITfEditSession_Impl for Edit_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        unsafe {
            let selected = selection(&self.ctx, ec)?;
            let password = secure(&self.ctx, ec, &selected);
            if !password {
                // What the application really has before the caret (before the composition when one is open) is the context for
                // ranking and for the punctuation direction; applications that cannot be read keep the shell's own record.
                let existing = self.state.borrow().composition.clone();
                let anchor = existing.and_then(|c| c.GetRange().ok());
                if let Some(text) = preceding_text(anchor.as_ref().unwrap_or(&selected), ec) {
                    if let Some(h) = &mut self.state.borrow_mut().host {
                        h.set_context(&text)
                    }
                }
                if self.key == Some(Key::ShiftTap) {
                    // the mode badge appears at the caret
                    if let Ok(view) = self.ctx.GetActiveView() {
                        let mut rect = RECT::default();
                        let mut clipped = FALSE;
                        if view.GetTextExt(ec, &selected, &mut rect, &mut clipped).is_ok() {
                            self.state.borrow_mut().caret = rect;
                        }
                    }
                }
            }
            let view = {
                let mut s = self.state.borrow_mut();
                let key = self.key.clone().unwrap_or(Key::FocusLost);
                let view = s.process(key.clone(), self.mods, password);
                if !view.consumed && !password {
                    if let Key::Text(text) = key {
                        let editing_key = text.chars().any(|c| c.is_control() || ('\u{F700}'..='\u{F8FF}').contains(&c));
                        if let (false, Some(h)) = (editing_key, &mut s.host) {
                            h.note_committed(&text)
                        }
                    }
                }
                s.view = view.clone();
                self.consumed.set(view.consumed);
                view
            };
            if password {
                self.state.borrow_mut().grid = None;
                panel::hide(&self.state);
                return Ok(());
            }
            let existing = self.state.borrow().composition.clone();
            let mut range = if let Some(comp) = &existing {
                comp.GetRange()?
            } else {
                selected
            };
            if !view.commit.is_empty() {
                range.SetText(ec, 0, &view.commit.encode_utf16().collect::<Vec<_>>())?;
                if let Some(comp) = existing {
                    let _ = self
                        .ctx
                        .GetProperty(&GUID_PROP_ATTRIBUTE)?
                        .Clear(ec, &range);
                    self.state.borrow_mut().composition = None;
                    comp.EndComposition(ec)?;
                }
                range.Collapse(ec, TF_ANCHOR_END)?;
                let sel = TF_SELECTION {
                    range: ManuallyDrop::new(Some(range.clone())),
                    style: TF_SELECTIONSTYLE {
                        ase: TF_AE_NONE,
                        fInterimChar: FALSE,
                    },
                };
                self.ctx.SetSelection(ec, std::slice::from_ref(&sel))?;
                drop(ManuallyDrop::into_inner(sel.range));
            }
            if !view.preedit.is_empty() {
                if self.state.borrow().composition.is_none() {
                    let insert: ITfInsertAtSelection = self.ctx.cast()?;
                    range = insert.InsertTextAtSelection(ec, TF_IAS_QUERYONLY, &[])?;
                    let sink: ITfCompositionSink =
                        CompositionSink(Rc::downgrade(&self.state)).into();
                    let composing: ITfContextComposition = self.ctx.cast()?;
                    let comp = composing.StartComposition(ec, &range, &sink)?;
                    self.state.borrow_mut().composition = Some(comp);
                }
                range.SetText(ec, 0, &view.preedit.encode_utf16().collect::<Vec<_>>())?;
                let value = VARIANT::from(self.state.borrow().atom as i32);
                self.ctx
                    .GetProperty(&GUID_PROP_ATTRIBUTE)?
                    .SetValue(ec, &range, &value)?;
                let caret = range.Clone()?;
                caret.Collapse(ec, TF_ANCHOR_START)?;
                let utf16 = view
                    .preedit
                    .get(..view.cursor_utf8)
                    .unwrap_or(&view.preedit)
                    .encode_utf16()
                    .count();
                let mut shifted = 0;
                caret.ShiftStart(ec, utf16 as i32, &mut shifted, std::ptr::null())?;
                caret.Collapse(ec, TF_ANCHOR_START)?;
                let sel = TF_SELECTION {
                    range: ManuallyDrop::new(Some(caret.clone())),
                    style: TF_SELECTIONSTYLE {
                        ase: TF_AE_NONE,
                        fInterimChar: FALSE,
                    },
                };
                self.ctx.SetSelection(ec, std::slice::from_ref(&sel))?;
                drop(ManuallyDrop::into_inner(sel.range));
                if let Ok(view) = self.ctx.GetActiveView() {
                    let mut rect = RECT::default();
                    let mut clipped = FALSE;
                    if view.GetTextExt(ec, &caret, &mut rect, &mut clipped).is_ok() {
                        self.state.borrow_mut().caret = rect;
                    }
                }
            } else if let Some(comp) = self.state.borrow_mut().composition.take() {
                let range = comp.GetRange()?;
                range.SetText(ec, 0, &[])?;
                comp.EndComposition(ec)?;
            }
            panel::show(&self.state);
            Ok(())
        }
    }
}
#[implement(ITfCompositionSink)]
struct CompositionSink(Weak<RefCell<State>>);
impl ITfCompositionSink_Impl for CompositionSink_Impl {
    fn OnCompositionTerminated(&self, _: u32, _: Ref<'_, ITfComposition>) -> Result<()> {
        if let Some(state) = self.0.upgrade() {
            if let Ok(mut s) = state.try_borrow_mut() {
                if s.composition.take().is_some() {
                    s.step(Key::FocusLost, 0, false);
                    s.view = View::default();
                    s.grid = None;
                    unsafe {
                        let _ = ShowWindow(s.hwnd, SW_HIDE);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Exercises the shell's grid/key policy against the real Windows Host/model.
    // This deliberately does not claim TSF text-store or desktop UI coverage.
    #[test]
    fn real_host_shell_policy() {
        let root = std::path::PathBuf::from(
            std::env::var("ZHIZI_TEST_INSTALL").expect("ZHIZI_TEST_INSTALL"),
        );
        let testdir =
            std::path::PathBuf::from(std::env::var("ZHIZI_TEST_DATA").expect("ZHIZI_TEST_DATA"));
        assert!(testdir.starts_with("H:\\zhizi"));
        unsafe {
            LoadLibraryExW(
                PCWSTR(wide(&root.join("rime.dll").to_string_lossy()).as_ptr()),
                None,
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            )
            .unwrap();
        }
        let mut host = Host::open(&HostConfig {
            shared_dir: root.join("Rime"),
            user_dir: testdir.join("Rime"),
            app_id: "zhizi-shell-policy-test".into(),
        })
        .unwrap();
        zhizi_host::ranking::onnx::init_runtime(&root.join("onnxruntime.dll")).unwrap();
        let scorer = zhizi_host::ranking::onnx::OnnxScorer::load(
            &root.join("Model/zhizi30m_v2.onnx"),
            &root.join("Model/itos.json"),
            4,
        )
        .unwrap();
        host.enable_model(Box::new(scorer), None);
        host.enable_recording(zhizi_host::quality::RecorderConfig {
            path: testdir.join("events.jsonl"),
            platform: "windows".into(),
            build: "shell-policy-test".into(),
            device: "synthetic".into(),
        });
        let mut s = State::new();
        s.host = Some(host);
        fn step(s: &mut State, key: Key) -> View {
            let v = s.process(key, 0, false);
            s.view = v.clone();
            v
        }
        fn text(s: &mut State, t: &str) {
            for c in t.chars() {
                step(s, Key::Text(c.to_string()));
            }
            for _ in 0..100 {
                if let Some(v) = s.host.as_mut().unwrap().poll_ranking() {
                    s.view = v;
                }
                if s.host.as_ref().unwrap().model_observation().outcome != "pending" {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        text(&mut s, "nihao");
        assert_eq!(step(&mut s, Key::Space).commit, "你好");
        text(&mut s, "taifuzale");
        assert_eq!(s.view.candidates[0].text, "太复杂了");
        // The model must have run for this group (the sentence now often leads from the dictionary
        // itself, so the "整句" comment is no proof either way).
        let outcome = s.host.as_ref().unwrap().model_observation().outcome;
        assert!(matches!(outcome, "composed" | "unchanged" | "reordered" | "no_group"), "model outcome {outcome}");
        assert_eq!(step(&mut s, Key::Space).commit, "太复杂了");
        text(&mut s, "opencode");
        assert_eq!(s.view.candidates[0].text, "OpenCode"); // official spelling from the AI-names layer
        assert_eq!(step(&mut s, Key::Space).commit, "OpenCode");
        text(&mut s, "ni");
        step(&mut s, Key::Text("+".into()));
        assert_eq!(s.grid.as_ref().unwrap().rows.len(), 4);
        step(&mut s, Key::Down);
        assert_eq!(s.grid.as_ref().unwrap().row, 1);
        step(&mut s, Key::Right);
        assert_eq!(s.view.highlighted, 1);
        let selected = s.view.candidates[1].text.clone();
        assert_eq!(step(&mut s, Key::Space).commit, selected);
        // `=` without Shift expands the one-row panel (as on the Mac); a real PageDown key does not.
        text(&mut s, "ni");
        step(&mut s, Key::PageDown);
        assert!(s.grid.is_none(), "PageDown pages, it does not expand");
        EQUALS_KEY.store(true, AtomicOrdering::Relaxed);
        step(&mut s, Key::PageDown);
        assert_eq!(s.grid.as_ref().unwrap().rows.len(), 4, "= expands the one-row panel");
        step(&mut s, Key::Escape);
        text(&mut s, "ni");
        step(&mut s, Key::Text("+".into()));
        step(&mut s, Key::Escape);
        assert!(s.grid.is_none());
        assert!(!s.view.preedit.is_empty());
        step(&mut s, Key::Escape);
        // Tab / End / Home / Delete with letters composed: the letters are committed and the key goes on to the application.
        for key in ["\t", "\u{F72B}", "\u{F729}", "\u{F728}"] {
            text(&mut s, "ni");
            let v = step(&mut s, Key::Text(key.into()));
            assert!(!v.consumed, "{key:?} must reach the application");
            assert!(!v.commit.is_empty() && v.preedit.is_empty(), "{key:?} commits the composition: {v:?}");
        }
        // Ctrl+Backspace is claimed only while composing; Shift or Alt with it is left alone.
        assert_eq!(candidate_edit_chord(mods::CONTROL), Some(Key::DeleteCandidate));
        assert_eq!(candidate_edit_chord(mods::CONTROL | mods::SHIFT), Some(Key::BlacklistCandidate));
        assert!(candidate_edit_chord(0).is_none() && candidate_edit_chord(mods::CONTROL | mods::ALT).is_none());
        text(&mut s, "ni");
        assert!(s.process(Key::DeleteCandidate, mods::CONTROL, false).consumed);
        step(&mut s, Key::Escape);
        // The parallel second layer is off: 6-0 are not claimed by candidates nobody can see.
        text(&mut s, "niha");
        assert!(s.host.as_ref().is_some());
        let v = step(&mut s, Key::Text("6".into()));
        assert!(v.commit.is_empty(), "digit 6 must not pick a hidden second-layer candidate: {v:?}");
        step(&mut s, Key::Escape);
        step(&mut s, Key::ShiftTap);
        let pass = step(&mut s, Key::Text("x".into()));
        assert!(!pass.consumed);
        assert!(pass.preedit.is_empty());
        step(&mut s, Key::ShiftTap);
        let pass = s.process(Key::Text("secret-test".into()), 0, true);
        assert!(!pass.consumed);
        assert!(pass.preedit.is_empty());
        assert!(pass.commit.is_empty());
        s.host.as_mut().unwrap().set_app("finish-test");
        drop(s);
        let records = std::fs::read_to_string(testdir.join("events.jsonl")).unwrap();
        assert!(records.contains("太复杂了"));
        assert!(!records.contains("secret-test"));
    }
}

//! Owned Rust entry point for the same native engine used by the desktop shells.
//!
//! Prepare explicit dictionary/private profile directories before opening. A
//! process may have multiple sessions for one deployment. Close every session
//! before switching deployments. Raw C clients must observe the same rule and
//! must not change deployment while these owned sessions are alive.
//!
//! Each engine belongs to one thread and cannot be shared across threads:
//! ```compile_fail
//! use agent_ime_core::sdk::Engine;
//! fn needs_send<T: Send>() {}
//! needs_send::<Engine>();
//! ```
//! ```compile_fail
//! use agent_ime_core::sdk::Engine;
//! fn needs_sync<T: Sync>() {}
//! needs_sync::<Engine>();
//! ```

use std::{
    fmt,
    marker::PhantomData,
    path::{Path, PathBuf},
    ptr::NonNull,
    rc::Rc,
    sync::{Mutex, OnceLock},
};
#[path = "sdk_owned.rs"]
mod owned;
pub use owned::{Candidate, Outcome};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum KeyKind {
    Text = 0,
    Backspace = 1,
    Left = 2,
    Right = 3,
    Space = 4,
    Enter = 5,
    Escape = 6,
    PageUp = 7,
    PageDown = 8,
    Up = 9,
    Down = 10,
    ShiftTap = 11,
    CapsLock = 12,
    FocusLost = 13,
    Deactivate = 14,
    Sleep = 15,
    Resume = 16,
    DeleteCandidate = 17,
    BlacklistCandidate = 18,
    Unsupported = 19,
    LinearNext = 20,
    LinearPrevious = 21,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u32)]
pub enum Security {
    NonSecure = 0,
    Secure = 1,
    #[default]
    Unknown = 2,
}

#[derive(Clone, Copy, Debug)]
pub struct KeyEvent<'a> {
    pub kind: KeyKind,
    pub text: &'a str,
    pub modifiers: u32,
    pub security: Security,
    /// Zero is permitted and means an unverified event for persistent learning.
    pub delivery_id: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    Configuration(&'static str),
    DeploymentConflict,
    CreateFailed,
    Api(i32),
    InvalidResult(&'static str),
    DeploymentPoisoned,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(reason) => write!(f, "invalid engine configuration: {reason}"),
            Self::DeploymentConflict => {
                f.write_str("close active engines before changing dictionary/profile deployment")
            }
            Self::CreateFailed => {
                f.write_str("native dictionary or private profile validation failed")
            }
            Self::Api(code) => write!(f, "native API returned {code}"),
            Self::InvalidResult(reason) => write!(f, "invalid native result: {reason}"),
            Self::DeploymentPoisoned => f.write_str("engine deployment guard is unavailable"),
        }
    }
}
impl std::error::Error for Error {}

#[derive(Default)]
struct Deployment {
    paths: Option<(PathBuf, PathBuf)>,
    active: usize,
}
fn deployment() -> &'static Mutex<Deployment> {
    static STATE: OnceLock<Mutex<Deployment>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(Deployment::default()))
}
struct Output(crate::AimResult);
impl Default for Output {
    fn default() -> Self {
        Self(crate::AimResult::default())
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        // SAFETY: this guard exclusively owns the initialized FFI output. It
        // is freed exactly once, even when conversion or an API call fails.
        unsafe { crate::aim_result_free(&mut self.0) }
    }
}
impl Output {
    fn owned(&self, status: i32) -> Result<Outcome, Error> {
        if status != 0 {
            return Err(Error::Api(status));
        }
        // SAFETY: a successful native call initialized every borrowed slice;
        // this guard keeps those allocations alive throughout the copy.
        unsafe { owned::copy_result(&self.0) }.map_err(Error::InvalidResult)
    }
}

pub struct Engine {
    handle: NonNull<crate::AimSession>,
    _thread: PhantomData<Rc<()>>,
}
impl Engine {
    pub fn open(shared: &Path, user: &Path, app_id: &str) -> Result<Self, Error> {
        if !shared.is_absolute() || !user.is_absolute() {
            return Err(Error::Configuration("directories must be absolute"));
        }
        if app_id.is_empty() || app_id.len() > 128 || app_id.chars().any(char::is_control) {
            return Err(Error::Configuration(
                "application identifier must be 1..128 bytes without controls",
            ));
        }
        let metadata = std::fs::symlink_metadata(user)
            .map_err(|_| Error::Configuration("private profile directory is unavailable"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(Error::Configuration(
                "private profile must be an existing directory, not a symlink",
            ));
        }
        let shared = shared
            .canonicalize()
            .map_err(|_| Error::Configuration("dictionary directory is unavailable"))?;
        let user = user
            .canonicalize()
            .map_err(|_| Error::Configuration("private profile directory is unavailable"))?;
        let shared_text = shared
            .to_str()
            .ok_or(Error::Configuration("dictionary path must be UTF-8"))?;
        let user_text = user
            .to_str()
            .ok_or(Error::Configuration("private profile path must be UTF-8"))?;
        let paths = (shared.clone(), user.clone());
        let mut state = deployment().lock().map_err(|_| Error::DeploymentPoisoned)?;
        if state.active != 0 && state.paths.as_ref() != Some(&paths) {
            return Err(Error::DeploymentConflict);
        }
        let next = state
            .active
            .checked_add(1)
            .ok_or(Error::Configuration("too many active engines"))?;
        // SAFETY: all UTF-8 byte buffers live until the call returns. Creation
        // is serialized and active engines use the same deployment. A returned
        // nonnull pointer is transferred to this Engine's exclusive ownership.
        let handle = unsafe {
            crate::aim_session_create(
                shared_text.as_ptr(),
                shared_text.len(),
                user_text.as_ptr(),
                user_text.len(),
                app_id.as_ptr(),
                app_id.len(),
            )
        };
        let handle = NonNull::new(handle).ok_or(Error::CreateFailed)?;
        state.paths = Some(paths);
        state.active = next;
        Ok(Self {
            handle,
            _thread: PhantomData,
        })
    }

    /// The returned outcome retains the model's status. API failures are Err;
    /// model-level rejection can still return a valid page with status != 0.
    pub fn process(&mut self, event: KeyEvent<'_>) -> Result<Outcome, Error> {
        let raw = crate::AimKeyEvent {
            kind: event.kind as u32,
            text: event.text.as_ptr(),
            text_len: event.text.len(),
            modifiers: event.modifiers,
            security_state: event.security as u32,
            delivery_id: event.delivery_id,
        };
        let mut output = Output::default();
        // SAFETY: self exclusively owns a live session and both event and
        // output storage remain valid for this sequential call.
        let status =
            unsafe { crate::aim_session_process(self.handle.as_ptr(), &raw, &mut output.0) };
        output.owned(status)
    }
    pub fn snapshot(&mut self) -> Result<Outcome, Error> {
        let mut output = Output::default();
        // SAFETY: exclusive live session and initialized output storage.
        let status =
            unsafe { crate::aim_session_current_page(self.handle.as_ptr(), &mut output.0) };
        output.owned(status)
    }
    pub fn set_fuzzy(&mut self, mask: u32) -> Result<(), Error> {
        // SAFETY: exclusive live session, scalar mask validated by the core.
        status(unsafe { crate::aim_session_set_fuzzy(self.handle.as_ptr(), mask) })
    }
    /// Reload only at idle; preserve any displayed composition on rejection.
    pub fn reload_dictionaries(&mut self) -> Result<(), Error> {
        if !self.snapshot()?.marked_text.is_empty() {
            return Err(Error::Api(-10));
        }
        // SAFETY: exclusive live session, checked at its idle boundary.
        status(unsafe { crate::aim_session_reload_dictionaries(self.handle.as_ptr()) })
    }
    pub fn withdraw(&mut self, raw: &str, text: &str) -> Result<bool, Error> {
        // SAFETY: exclusive live session and both UTF-8 buffers live until return.
        match unsafe {
            crate::aim_session_withdraw_selection(
                self.handle.as_ptr(),
                raw.as_ptr(),
                raw.len(),
                text.as_ptr(),
                text.len(),
            )
        } {
            0 => Ok(false),
            1 => Ok(true),
            code => Err(Error::Api(code)),
        }
    }
    /// Access advanced C APIs (lexicons, scoring and generation) without
    /// transferring ownership.
    ///
    /// # Safety
    /// Do not destroy the session, retain it past this Engine, use it from
    /// another thread, reenter it, or change global dictionary deployment.
    /// Observe every C API's buffer, callback and security contract.
    pub unsafe fn as_raw(&mut self) -> *mut crate::AimSession {
        self.handle.as_ptr()
    }
}
fn status(code: i32) -> Result<(), Error> {
    if code == 0 {
        Ok(())
    } else {
        Err(Error::Api(code))
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        let guard = deployment().lock();
        // SAFETY: this Engine exclusively owns the live pointer and destroys
        // it once. A poisoned guard must never prevent resource reclamation.
        unsafe { crate::aim_session_destroy(self.handle.as_ptr()) }
        if let Ok(mut state) = guard {
            state.active = state.active.saturating_sub(1);
            if state.active == 0 {
                state.paths = None;
            }
        }
    }
}

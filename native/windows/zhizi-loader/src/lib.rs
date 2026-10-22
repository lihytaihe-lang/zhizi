#![allow(non_snake_case)]
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

type Hresult = i32;
const S_OK: Hresult = 0;
const S_FALSE: Hresult = 1;
const E_POINTER: Hresult = 0x8000_4003_u32 as i32;
const E_FAIL: Hresult = 0x8000_4005_u32 as i32;
const CLASS_E_CLASSNOTAVAILABLE: Hresult = 0x8004_0111_u32 as i32;
/// Dependencies of the implementation DLL are looked up next to it.
const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x8;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn GetModuleFileNameW(module: *mut c_void, buffer: *mut u16, size: u32) -> u32;
    fn SetEnvironmentVariableW(name: *const u16, value: *const u16) -> i32;
}

static MODULE: AtomicUsize = AtomicUsize::new(0);
/// Every implementation this process has loaded, oldest first. They are never unloaded: objects of an older release may still be alive.
static LOADED: Mutex<Vec<(PathBuf, usize)>> = Mutex::new(Vec::new());

type GetClassObject = unsafe extern "system" fn(*const c_void, *const c_void, *mut *mut c_void) -> Hresult;
type Register = unsafe extern "system" fn() -> Hresult;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// The loader's own full path (the registered one).
fn own_path() -> PathBuf {
    let mut buffer = [0u16; 32768];
    let n = unsafe { GetModuleFileNameW(MODULE.load(Ordering::Relaxed) as *mut c_void, buffer.as_mut_ptr(), buffer.len() as u32) };
    PathBuf::from(String::from_utf16_lossy(&buffer[..n as usize]))
}

/// `<install root>\<release folder named in current.txt>\zhizi_tsf.dll`; None when the file is missing or names something odd.
fn wanted() -> Option<PathBuf> {
    let root = own_path().parent()?.to_path_buf();
    let text = std::fs::read_to_string(root.join("current.txt")).ok()?;
    let name = text.trim().trim_start_matches('\u{feff}');
    let plain = !name.is_empty() && name.len() < 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    plain.then(|| root.join(name).join("zhizi_tsf.dll"))
}

/// The module of the implementation to use now: the release `current.txt` names (loaded on first use), else the newest one already loaded.
fn implementation() -> Option<usize> {
    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(path) = wanted() {
        if let Some((_, module)) = loaded.iter().find(|(p, _)| *p == path) {
            return Some(*module);
        }
        let name = wide(&path.to_string_lossy());
        let module = unsafe { LoadLibraryExW(name.as_ptr(), std::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH) };
        if !module.is_null() {
            loaded.push((path, module as usize));
            return Some(module as usize);
        }
    }
    loaded.last().map(|(_, module)| *module)
}

unsafe fn forward(name: &[u8]) -> Option<*mut c_void> {
    let module = implementation()?;
    let address = GetProcAddress(module as *mut c_void, name.as_ptr());
    (!address.is_null()).then_some(address)
}

#[no_mangle]
pub unsafe extern "system" fn DllMain(module: *mut c_void, reason: u32, _: *mut c_void) -> i32 {
    if reason == 1 {
        MODULE.store(module as usize, Ordering::Relaxed);
    }
    1
}

#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(clsid: *const c_void, iid: *const c_void, out: *mut *mut c_void) -> Hresult {
    if clsid.is_null() || iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    *out = std::ptr::null_mut();
    match forward(b"DllGetClassObject\0") {
        Some(address) => std::mem::transmute::<*mut c_void, GetClassObject>(address)(clsid, iid, out),
        None => CLASS_E_CLASSNOTAVAILABLE,
    }
}

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> Hresult {
    // An implementation, once loaded, stays (the model worker and Rime keep callbacks until the process ends).
    let loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if loaded.is_empty() { S_OK } else { S_FALSE }
}

unsafe fn registration(name: &[u8]) -> Hresult {
    // The implementation registers the path it is told to: this loader, not itself.
    let own = wide(&own_path().to_string_lossy());
    SetEnvironmentVariableW(wide("ZHIZI_REGISTER_AS").as_ptr(), own.as_ptr());
    match forward(name) {
        Some(address) => std::mem::transmute::<*mut c_void, Register>(address)(),
        None => E_FAIL,
    }
}

#[no_mangle]
pub unsafe extern "system" fn DllRegisterServer() -> Hresult {
    registration(b"DllRegisterServer\0")
}

#[no_mangle]
pub unsafe extern "system" fn DllUnregisterServer() -> Hresult {
    registration(b"DllUnregisterServer\0")
}

#[allow(dead_code)]
const _: Hresult = S_OK;

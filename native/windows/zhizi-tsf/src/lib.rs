#![allow(non_snake_case)]
mod display;
mod panel;
mod registration;
mod service;
use std::sync::atomic::{AtomicUsize, Ordering};
use windows::{
    core::*,
    Win32::{
        Foundation::*,
        System::{Com::*, LibraryLoader::*},
    },
};

pub const CLSID_ZHIZI: GUID = GUID::from_u128(0x371930f8_3da5_4b10_90a3_88f486c9a251);
pub const PROFILE_ZHIZI: GUID = GUID::from_u128(0x371930f8_3da5_4b10_90a3_88f486c9a252);
pub const ATTR_ZHIZI: GUID = GUID::from_u128(0x371930f8_3da5_4b10_90a3_88f486c9a253);
pub static MODULE: AtomicUsize = AtomicUsize::new(0);
pub static OBJECTS: AtomicUsize = AtomicUsize::new(0);
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
/// Full path of this DLL. Each release installs into its own folder, so an upgrade never has to replace a DLL that
/// running apps still hold (no "restart now?" prompt); the registration must point at the file actually loaded.
pub fn module_path() -> std::path::PathBuf {
    let mut buf = [0u16; 32768];
    let n =
        unsafe { GetModuleFileNameW(Some(HMODULE(MODULE.load(Ordering::Relaxed) as _)), &mut buf) };
    std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n as usize]))
}
pub fn install_dir() -> std::path::PathBuf {
    module_path().parent().unwrap().to_path_buf()
}
pub fn error() -> Error {
    Error::from_hresult(E_FAIL)
}

#[implement(IClassFactory)]
struct Factory;
impl Drop for Factory {
    fn drop(&mut self) {
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}
impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<'_, IUnknown>,
        iid: *const GUID,
        out: *mut *mut std::ffi::c_void,
    ) -> Result<()> {
        if !outer.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        if out.is_null() || iid.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            *out = std::ptr::null_mut();
        }
        let tip: windows::Win32::UI::TextServices::ITfTextInputProcessorEx =
            service::Service::new().into();
        unsafe { tip.query(iid, out).ok() }
    }
    fn LockServer(&self, lock: BOOL) -> Result<()> {
        if lock.as_bool() {
            OBJECTS.fetch_add(1, Ordering::SeqCst);
        } else {
            OBJECTS.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }
}
#[no_mangle]
pub unsafe extern "system" fn DllMain(
    module: HMODULE,
    reason: u32,
    _: *mut std::ffi::c_void,
) -> BOOL {
    if reason == 1 {
        MODULE.store(module.0 as usize, Ordering::Relaxed);
    }
    TRUE
}
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    iid: *const GUID,
    out: *mut *mut std::ffi::c_void,
) -> HRESULT {
    if clsid.is_null() || iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    *out = std::ptr::null_mut();
    if *clsid != CLSID_ZHIZI {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    OBJECTS.fetch_add(1, Ordering::SeqCst);
    let factory: IClassFactory = Factory.into();
    factory.query(iid, out)
}
#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    // The model worker and Rime may retain callbacks until process teardown.
    // Do not unload their code while an activated process remains alive.
    if service::EVER_ACTIVATED.load(Ordering::SeqCst) {
        S_FALSE
    } else if OBJECTS.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}
#[no_mangle]
pub unsafe extern "system" fn DllRegisterServer() -> HRESULT {
    registration::register()
        .map(|_| S_OK)
        .unwrap_or_else(|e| e.code())
}
#[no_mangle]
pub unsafe extern "system" fn DllUnregisterServer() -> HRESULT {
    registration::unregister()
        .map(|_| S_OK)
        .unwrap_or_else(|e| e.code())
}

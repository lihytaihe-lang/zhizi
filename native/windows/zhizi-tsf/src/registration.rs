use super::*;
use windows::Win32::{System::Registry::*, UI::TextServices::*};
const CLASS: &str = "Software\\Classes\\CLSID\\{371930F8-3DA5-4B10-90A3-88F486C9A251}";
unsafe fn reg_string(path: &str, name: &str, value: &str) -> Result<()> {
    let mut key = HKEY::default();
    RegCreateKeyExW(
        HKEY_LOCAL_MACHINE,
        PCWSTR(wide(path).as_ptr()),
        None,
        None,
        REG_OPTION_NON_VOLATILE,
        KEY_WRITE | KEY_WOW64_64KEY,
        None,
        &mut key,
        None,
    )
    .ok()?;
    let data = wide(value);
    let bytes = std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2);
    let result = RegSetValueExW(key, PCWSTR(wide(name).as_ptr()), None, REG_SZ, Some(bytes)).ok();
    let _ = RegCloseKey(key);
    result
}
const CATEGORIES: &[GUID] = &[
    GUID_TFCAT_TIP_KEYBOARD,
    GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
    GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
    GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
];
unsafe fn install_profile(remove: bool) -> Result<()> {
    let dll = LoadLibraryExW(w!("input.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)?;
    let address = GetProcAddress(dll, s!("InstallLayoutOrTip")).ok_or_else(error)?;
    let install: unsafe extern "system" fn(PCWSTR, u32) -> BOOL = std::mem::transmute(address);
    let ok = install(
        w!("0x0804:{371930F8-3DA5-4B10-90A3-88F486C9A251}{371930F8-3DA5-4B10-90A3-88F486C9A252}"),
        if remove { 1 } else { 0 },
    );
    let _ = FreeLibrary(dll);
    if ok.as_bool() {
        Ok(())
    } else {
        Err(error())
    }
}
pub unsafe fn register() -> Result<()> {
    let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    let result = (|| {
        // Registered through the loader, which names itself here; registered directly (development), the implementation names itself.
        let dll = std::env::var("ZHIZI_REGISTER_AS").unwrap_or_else(|_| module_path().to_string_lossy().to_string());
        reg_string(CLASS, "", "知字拼音")?;
        reg_string(&format!("{CLASS}\\InprocServer32"), "", &dll)?;
        reg_string(
            &format!("{CLASS}\\InprocServer32"),
            "ThreadingModel",
            "Apartment",
        )?;
        let profiles: ITfInputProcessorProfiles =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
        profiles.Register(&CLSID_ZHIZI)?;
        profiles.AddLanguageProfile(
            &CLSID_ZHIZI,
            0x0804,
            &PROFILE_ZHIZI,
            &wide("知字拼音")[..4],
            // the icon lives in the implementation's resources, not in the loader
            &module_path().to_string_lossy().encode_utf16().collect::<Vec<_>>(),
            0,
        )?;
        profiles.EnableLanguageProfileByDefault(&CLSID_ZHIZI, 0x0804, &PROFILE_ZHIZI, true)?;
        profiles.EnableLanguageProfile(&CLSID_ZHIZI, 0x0804, &PROFILE_ZHIZI, true)?;
        let cat: ITfCategoryMgr =
            CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
        for id in CATEGORIES {
            cat.RegisterCategory(&CLSID_ZHIZI, id, &CLSID_ZHIZI)?;
        }
        install_profile(false)?;
        Ok(())
    })();
    if init.is_ok() {
        CoUninitialize();
    }
    result
}
pub unsafe fn unregister() -> Result<()> {
    let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    let result = (|| {
        let profiles: ITfInputProcessorProfiles =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
        let _ = install_profile(true);
        let _ = profiles.RemoveLanguageProfile(&CLSID_ZHIZI, 0x0804, &PROFILE_ZHIZI);
        profiles.Unregister(&CLSID_ZHIZI)?;
        let cat: ITfCategoryMgr =
            CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
        for id in CATEGORIES {
            let _ = cat.UnregisterCategory(&CLSID_ZHIZI, id, &CLSID_ZHIZI);
        }
        RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(wide(CLASS).as_ptr())).ok()?;
        Ok(())
    })();
    if init.is_ok() {
        CoUninitialize();
    }
    result
}

use windows::{
    core::*,
    Win32::{System::Com::*, UI::TextServices::*},
};
fn main() -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let clsid = GUID::from_u128(0x371930f8_3da5_4b10_90a3_88f486c9a251);
        let profile = GUID::from_u128(0x371930f8_3da5_4b10_90a3_88f486c9a252);
        let profiles: ITfInputProcessorProfiles =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
        println!(
            "enabled={}",
            profiles
                .IsEnabledLanguageProfile(&clsid, 0x0804, &profile)?
                .as_bool()
        );
        println!(
            "description={}",
            profiles.GetLanguageProfileDescription(&clsid, 0x0804, &profile)?
        );
        let tip: ITfTextInputProcessorEx = CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER)?;
        let provider: ITfDisplayAttributeProvider = tip.cast()?;
        let _ = provider.EnumDisplayAttributeInfo()?;
        println!("COM creation and display attribute provider PASS");
        if std::env::args().any(|arg| arg == "--activate") {
            use windows::Win32::UI::WindowsAndMessaging::*;
            let manager: ITfThreadMgr =
                CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)?;
            let client = manager.Activate()?;
            tip.ActivateEx(&manager, client, 0)?;
            let until = std::time::Instant::now() + std::time::Duration::from_secs(12);
            let mut message = MSG::default();
            while std::time::Instant::now() < until {
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            tip.Deactivate()?;
            manager.Deactivate()?;
            println!("activation/deactivation PASS (not a text-entry UI test)");
        }
        Ok(())
    }
}

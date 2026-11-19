use crate::keychain::ProviderKeychain;
use std::sync::Arc;

pub fn system_vault() -> Arc<dyn ProviderKeychain> {
    #[cfg(target_os = "macos")]
    { crate::keychain::system_provider_keychain() }
    #[cfg(not(target_os = "macos"))]
    { Arc::new(SkinVault) }
}

#[cfg(not(target_os = "macos"))]
struct SkinVault;

#[cfg(target_os = "linux")]
impl SkinVault {
    fn call(args: &[&str], secret: Option<&str>) -> Result<Option<String>, String> {
        use std::io::{Read, Write};
        use std::process::{Command, Stdio};
        let mut child = Command::new("secret-tool").args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .spawn().map_err(|_| "请安装 libsecret-tools 并启用系统密钥环。".to_string())?;
        if let Some(secret) = secret { child.stdin.take().ok_or("系统凭据输入不可用。")?.write_all(secret.as_bytes()).map_err(|_| "无法保存系统凭据。")?; }
        else { drop(child.stdin.take()); }
        let start = std::time::Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if start.elapsed() < std::time::Duration::from_secs(30) => std::thread::sleep(std::time::Duration::from_millis(40)),
                _ => { let _ = child.kill(); let _ = child.wait(); return Err("系统密钥环没有响应。".into()); }
            }
        };
        let mut output = String::new();
        if let Some(stdout) = child.stdout.take() { stdout.take(4097).read_to_string(&mut output).map_err(|_| "无法读取系统凭据。")?; }
        if !status.success() {
            if args.first() == Some(&"lookup") && status.code() == Some(1) && output.is_empty() { return Ok(None); }
            return Err("系统密钥环不可用或已锁定。".into());
        }
        if output.len() > 4096 { return Err("系统凭据格式无效。".into()); }
        Ok(Some(output.trim_end_matches(['\r', '\n']).into()))
    }
}

#[cfg(target_os = "linux")]
impl ProviderKeychain for SkinVault {
    fn set_password(&self, account: &str, password: &str) -> Result<(), String> {
        Self::call(&["store", "--label=知字皮肤账号", "service", "com.agentinputlayer.skin-commerce", "account", account], Some(password)).map(|_| ())
    }
    fn get_password(&self, account: &str) -> Result<Option<String>, String> {
        Self::call(&["lookup", "service", "com.agentinputlayer.skin-commerce", "account", account], None)
    }
    fn delete_password(&self, account: &str) -> Result<(), String> {
        Self::call(&["clear", "service", "com.agentinputlayer.skin-commerce", "account", account], None).map(|_| ())
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use std::ffi::c_void;
    #[repr(C)] struct FileTime { low: u32, high: u32 }
    #[repr(C)] struct Credential {
        flags: u32, kind: u32, target: *mut u16, comment: *mut u16, last_written: FileTime,
        blob_size: u32, blob: *mut u8, persist: u32, attribute_count: u32, attributes: *mut c_void,
        target_alias: *mut u16, user_name: *mut u16,
    }
    #[link(name="Advapi32")]
    extern "system" {
        fn CredWriteW(credential: *const Credential, flags: u32) -> i32;
        fn CredReadW(target: *const u16, kind: u32, flags: u32, credential: *mut *mut Credential) -> i32;
        fn CredDeleteW(target: *const u16, kind: u32, flags: u32) -> i32;
        fn CredFree(buffer: *mut c_void);
    }
    #[link(name="Kernel32")]
    extern "system" { fn GetLastError() -> u32; }
    fn target(account: &str) -> Vec<u16> { format!("Zhizi/SkinCommerce/{account}\0").encode_utf16().collect() }
    impl ProviderKeychain for SkinVault {
        fn set_password(&self, account: &str, password: &str) -> Result<(), String> {
            if password.len() > 4096 { return Err("登录凭据过大。".into()); }
            let mut name = target(account);
            let credential = Credential { flags: 0, kind: 1, target: name.as_mut_ptr(), comment: std::ptr::null_mut(), last_written: FileTime {low:0,high:0},
                blob_size: password.len() as u32, blob: password.as_ptr().cast_mut(), persist: 2, attribute_count: 0, attributes: std::ptr::null_mut(), target_alias: std::ptr::null_mut(), user_name: std::ptr::null_mut() };
            if unsafe { CredWriteW(&credential, 0) } == 0 { Err(format!("无法保存 Windows 登录凭据（系统错误 {}）。", unsafe { GetLastError() })) } else { Ok(()) }
        }
        fn get_password(&self, account: &str) -> Result<Option<String>, String> {
            let name = target(account); let mut pointer = std::ptr::null_mut();
            if unsafe { CredReadW(name.as_ptr(), 1, 0, &mut pointer) } == 0 {
                return if unsafe { GetLastError() } == 1168 { Ok(None) } else { Err("无法读取 Windows 登录凭据。".into()) };
            }
            struct Guard(*mut Credential); impl Drop for Guard { fn drop(&mut self) { unsafe { CredFree(self.0.cast()) } } }
            let guard = Guard(pointer);
            let credential = unsafe { &*guard.0 };
            if credential.blob_size > 4096 || (credential.blob_size != 0 && credential.blob.is_null()) { return Err("Windows 登录凭据格式无效。".into()); }
            if credential.blob_size == 0 { return Ok(None); }
            String::from_utf8(unsafe { std::slice::from_raw_parts(credential.blob, credential.blob_size as usize) }.to_vec())
                .map(Some).map_err(|_| "Windows 登录凭据格式无效。".into())
        }
        fn delete_password(&self, account: &str) -> Result<(), String> {
            if unsafe { CredDeleteW(target(account).as_ptr(), 1, 0) } != 0 || unsafe { GetLastError() } == 1168 { Ok(()) }
            else { Err("无法清除 Windows 登录凭据。".into()) }
        }
    }
}

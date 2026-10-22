use std::sync::Arc;

#[cfg_attr(test, allow(dead_code))]
pub const PROVIDER_KEYCHAIN_SERVICE: &str = "com.agentinputlayer.desktop";
const PROVIDER_KEYCHAIN_REFERENCE_PREFIX: &str = "keychain://com.agentinputlayer.desktop/provider/";

pub trait ProviderKeychain: Send + Sync {
    fn set_password(&self, account: &str, password: &str) -> Result<(), String>;
    fn get_password(&self, account: &str) -> Result<Option<String>, String>;
    fn delete_password(&self, account: &str) -> Result<(), String>;
}

#[cfg_attr(test, allow(dead_code))]
pub fn system_provider_keychain() -> Arc<dyn ProviderKeychain> {
    Arc::new(SystemProviderKeychain)
}

#[cfg(test)]
#[derive(Default)]
pub struct MemoryProviderKeychain {
    passwords: std::sync::Mutex<std::collections::HashMap<String, String>>,
    failure: std::sync::Mutex<Option<String>>,
}

#[cfg(test)]
impl MemoryProviderKeychain {
    pub fn unavailable(message: &str) -> Self {
        Self {
            passwords: std::sync::Mutex::new(std::collections::HashMap::new()),
            failure: std::sync::Mutex::new(Some(message.into())),
        }
    }

    fn failure(&self) -> Result<(), String> {
        if let Some(message) = self
            .failure
            .lock()
            .map_err(|_| "测试钥匙串锁已损坏。".to_string())?
            .clone()
        {
            Err(message)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
impl ProviderKeychain for MemoryProviderKeychain {
    fn set_password(&self, account: &str, password: &str) -> Result<(), String> {
        self.failure()?;
        self.passwords
            .lock()
            .map_err(|_| "测试钥匙串锁已损坏。".to_string())?
            .insert(account.into(), password.into());
        Ok(())
    }

    fn get_password(&self, account: &str) -> Result<Option<String>, String> {
        self.failure()?;
        Ok(self
            .passwords
            .lock()
            .map_err(|_| "测试钥匙串锁已损坏。".to_string())?
            .get(account)
            .cloned())
    }

    fn delete_password(&self, account: &str) -> Result<(), String> {
        self.failure()?;
        self.passwords
            .lock()
            .map_err(|_| "测试钥匙串锁已损坏。".to_string())?
            .remove(account);
        Ok(())
    }
}

pub fn provider_keychain_reference(provider_kind: &str) -> String {
    format!("{PROVIDER_KEYCHAIN_REFERENCE_PREFIX}{provider_kind}")
}

pub fn validate_provider_keychain_reference(
    provider_kind: &str,
    reference: &str,
) -> Result<(), String> {
    if reference == provider_keychain_reference(provider_kind) {
        Ok(())
    } else {
        Err("Provider 密钥引用无效，请重新填写密钥。".into())
    }
}

#[cfg_attr(test, allow(dead_code))]
struct SystemProviderKeychain;

#[cfg(target_os = "macos")]
#[cfg_attr(test, allow(dead_code))]
mod macos {
    use super::{ProviderKeychain, SystemProviderKeychain, PROVIDER_KEYCHAIN_SERVICE};
    use std::{
        ffi::{c_long, c_uchar, c_void},
        ptr, slice,
    };

    type CFIndex = c_long;
    type CFTypeRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFDataRef = *const c_void;
    type CFDictionaryRef = *const c_void;
    type CFAllocatorRef = *const c_void;
    type OSStatus = i32;

    const UTF8_ENCODING: u32 = 0x0800_0100;
    const ERR_SEC_SUCCESS: OSStatus = 0;
    const ERR_SEC_DUPLICATE_ITEM: OSStatus = -25_299;
    const ERR_SEC_ITEM_NOT_FOUND: OSStatus = -25_300;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFTypeDictionaryKeyCallBacks: c_void;
        static kCFTypeDictionaryValueCallBacks: c_void;
        static kCFBooleanTrue: CFTypeRef;

        fn CFStringCreateWithBytes(
            allocator: CFAllocatorRef,
            bytes: *const c_uchar,
            length: CFIndex,
            encoding: u32,
            is_external_representation: bool,
        ) -> CFStringRef;
        fn CFDataCreate(
            allocator: CFAllocatorRef,
            bytes: *const c_uchar,
            length: CFIndex,
        ) -> CFDataRef;
        fn CFDataGetLength(data: CFDataRef) -> CFIndex;
        fn CFDataGetBytePtr(data: CFDataRef) -> *const c_uchar;
        fn CFDictionaryCreate(
            allocator: CFAllocatorRef,
            keys: *const *const c_void,
            values: *const *const c_void,
            count: CFIndex,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> CFDictionaryRef;
        fn CFRelease(value: CFTypeRef);
    }

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        static kSecClass: CFTypeRef;
        static kSecClassGenericPassword: CFTypeRef;
        static kSecAttrService: CFTypeRef;
        static kSecAttrAccount: CFTypeRef;
        static kSecValueData: CFTypeRef;
        static kSecReturnData: CFTypeRef;
        static kSecMatchLimit: CFTypeRef;
        static kSecMatchLimitOne: CFTypeRef;

        fn SecItemAdd(attributes: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;
        fn SecItemCopyMatching(query: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;
        fn SecItemUpdate(query: CFDictionaryRef, attributes_to_update: CFDictionaryRef)
            -> OSStatus;
        fn SecItemDelete(query: CFDictionaryRef) -> OSStatus;
    }

    struct OwnedCf(CFTypeRef);

    impl OwnedCf {
        fn new(value: CFTypeRef, description: &str) -> Result<Self, String> {
            if value.is_null() {
                Err(format!("无法创建钥匙串{description}。"))
            } else {
                Ok(Self(value))
            }
        }

        fn as_ptr(&self) -> CFTypeRef {
            self.0
        }
    }

    impl Drop for OwnedCf {
        fn drop(&mut self) {
            unsafe { CFRelease(self.0) };
        }
    }

    fn cf_string(value: &str) -> Result<OwnedCf, String> {
        let length: CFIndex = value
            .len()
            .try_into()
            .map_err(|_| "钥匙串字段过长。".to_string())?;
        OwnedCf::new(
            unsafe {
                CFStringCreateWithBytes(ptr::null(), value.as_ptr(), length, UTF8_ENCODING, false)
            },
            "文本",
        )
    }

    fn cf_data(value: &[u8]) -> Result<OwnedCf, String> {
        let length: CFIndex = value
            .len()
            .try_into()
            .map_err(|_| "Provider 密钥过长。".to_string())?;
        OwnedCf::new(
            unsafe { CFDataCreate(ptr::null(), value.as_ptr(), length) },
            "数据",
        )
    }

    fn dictionary(keys: &[CFTypeRef], values: &[CFTypeRef]) -> Result<OwnedCf, String> {
        debug_assert_eq!(keys.len(), values.len());
        let count: CFIndex = keys
            .len()
            .try_into()
            .map_err(|_| "钥匙串属性过多。".to_string())?;
        OwnedCf::new(
            unsafe {
                CFDictionaryCreate(
                    ptr::null(),
                    keys.as_ptr(),
                    values.as_ptr(),
                    count,
                    ptr::addr_of!(kCFTypeDictionaryKeyCallBacks),
                    ptr::addr_of!(kCFTypeDictionaryValueCallBacks),
                )
            },
            "属性",
        )
    }

    fn status_error(action: &str, status: OSStatus) -> String {
        format!("无法{action} macOS 钥匙串（状态 {status}），请解锁钥匙串后重试。")
    }

    fn base_query(service: CFTypeRef, account: CFTypeRef) -> Result<OwnedCf, String> {
        dictionary(
            unsafe { &[kSecClass, kSecAttrService, kSecAttrAccount] },
            unsafe { &[kSecClassGenericPassword, service, account] },
        )
    }

    impl ProviderKeychain for SystemProviderKeychain {
        fn set_password(&self, account: &str, password: &str) -> Result<(), String> {
            let service = cf_string(PROVIDER_KEYCHAIN_SERVICE)?;
            let account = cf_string(account)?;
            let password = cf_data(password.as_bytes())?;
            let add = dictionary(
                unsafe { &[kSecClass, kSecAttrService, kSecAttrAccount, kSecValueData] },
                unsafe {
                    &[
                        kSecClassGenericPassword,
                        service.as_ptr(),
                        account.as_ptr(),
                        password.as_ptr(),
                    ]
                },
            )?;
            let status = unsafe { SecItemAdd(add.as_ptr(), ptr::null_mut()) };
            if status == ERR_SEC_SUCCESS {
                return Ok(());
            }
            if status != ERR_SEC_DUPLICATE_ITEM {
                return Err(status_error("写入", status));
            }

            let query = base_query(service.as_ptr(), account.as_ptr())?;
            let update = dictionary(unsafe { &[kSecValueData] }, &[password.as_ptr()])?;
            let status = unsafe { SecItemUpdate(query.as_ptr(), update.as_ptr()) };
            if status == ERR_SEC_SUCCESS {
                Ok(())
            } else {
                Err(status_error("更新", status))
            }
        }

        fn get_password(&self, account: &str) -> Result<Option<String>, String> {
            let service = cf_string(PROVIDER_KEYCHAIN_SERVICE)?;
            let account = cf_string(account)?;
            let query = dictionary(
                unsafe {
                    &[
                        kSecClass,
                        kSecAttrService,
                        kSecAttrAccount,
                        kSecReturnData,
                        kSecMatchLimit,
                    ]
                },
                unsafe {
                    &[
                        kSecClassGenericPassword,
                        service.as_ptr(),
                        account.as_ptr(),
                        kCFBooleanTrue,
                        kSecMatchLimitOne,
                    ]
                },
            )?;
            let mut result: CFTypeRef = ptr::null();
            let status = unsafe { SecItemCopyMatching(query.as_ptr(), &mut result) };
            if status == ERR_SEC_ITEM_NOT_FOUND {
                return Ok(None);
            }
            if status != ERR_SEC_SUCCESS {
                return Err(status_error("读取", status));
            }
            let data = OwnedCf::new(result, "返回数据")?;
            let length = unsafe { CFDataGetLength(data.as_ptr()) };
            if length < 0 {
                return Err("钥匙串返回了无效的 Provider 密钥。".into());
            }
            if length == 0 {
                return Ok(Some(String::new()));
            }
            let bytes = unsafe {
                slice::from_raw_parts(
                    CFDataGetBytePtr(data.as_ptr()),
                    usize::try_from(length).map_err(|_| "钥匙串密钥长度无效。")?,
                )
            };
            String::from_utf8(bytes.to_vec())
                .map(Some)
                .map_err(|_| "钥匙串中的 Provider 密钥不是有效 UTF-8，请重新填写。".into())
        }

        fn delete_password(&self, account: &str) -> Result<(), String> {
            let service = cf_string(PROVIDER_KEYCHAIN_SERVICE)?;
            let account = cf_string(account)?;
            let query = base_query(service.as_ptr(), account.as_ptr())?;
            let status = unsafe { SecItemDelete(query.as_ptr()) };
            if status == ERR_SEC_SUCCESS || status == ERR_SEC_ITEM_NOT_FOUND {
                Ok(())
            } else {
                Err(status_error("删除", status))
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl ProviderKeychain for SystemProviderKeychain {
    fn set_password(&self, _account: &str, _password: &str) -> Result<(), String> {
        Err("Provider 密钥仅支持存入 macOS 钥匙串。".into())
    }

    fn get_password(&self, _account: &str) -> Result<Option<String>, String> {
        Err("Provider 密钥仅支持从 macOS 钥匙串读取。".into())
    }

    fn delete_password(&self, _account: &str) -> Result<(), String> {
        Err("Provider 密钥仅支持从 macOS 钥匙串删除。".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_is_the_bundle_identifier_and_references_bind_provider_kind() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(
            config["identifier"].as_str(),
            Some(PROVIDER_KEYCHAIN_SERVICE)
        );
        for provider_kind in ["openai-compatible", "anthropic"] {
            let reference = provider_keychain_reference(provider_kind);
            assert!(validate_provider_keychain_reference(provider_kind, &reference).is_ok());
            assert!(validate_provider_keychain_reference("other", &reference).is_err());
        }
    }
}

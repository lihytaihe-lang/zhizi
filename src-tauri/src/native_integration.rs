use crate::layer2::AppClass;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionCapture {
    pub status: String,
    pub text: String,
    pub app_class: AppClass,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalTextAccess {
    pub authorized: bool,
    pub target_available: bool,
    pub actions_enabled: bool,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExternalTextAction {
    Replace,
    Insert,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalTextActionResult {
    pub status: String,
    pub message: String,
    pub fallback_to_clipboard: bool,
}

#[derive(Debug, Clone)]
struct CapturedTarget {
    capture: SelectionCapture,
    pid: Option<i32>,
}

pub struct NativeIntegrationState {
    latest: Mutex<CapturedTarget>,
}

impl Default for NativeIntegrationState {
    fn default() -> Self {
        Self {
            latest: Mutex::new(CapturedTarget {
                capture: SelectionCapture {
                    status: "empty".into(),
                    text: String::new(),
                    app_class: AppClass::Unknown,
                    message: "未读取到选区，可手动粘贴。".into(),
                },
                pid: None,
            }),
        }
    }
}

impl NativeIntegrationState {
    pub fn capture_before_panel(&self) {
        let captured = platform::capture_selection();
        if let Ok(mut latest) = self.latest.lock() {
            *latest = captured;
        }
    }

    pub fn latest_capture(&self) -> Result<SelectionCapture, String> {
        self.latest
            .lock()
            .map(|latest| latest.capture.clone())
            .map_err(|_| "选区状态锁已损坏。".into())
    }

    pub fn external_text_access(&self) -> Result<ExternalTextAccess, String> {
        let pid = self
            .latest
            .lock()
            .map_err(|_| "选区状态锁已损坏。".to_string())?
            .pid;
        Ok(platform::external_text_access(pid))
    }

    pub fn perform_text_action(
        &self,
        action: ExternalTextAction,
        text: &str,
    ) -> Result<ExternalTextActionResult, String> {
        if text.trim().is_empty() {
            return Err("没有可写回的结果；请先生成并预览结果。".into());
        }
        let pid = self
            .latest
            .lock()
            .map_err(|_| "选区状态锁已损坏。".to_string())?
            .pid;
        Ok(platform::perform_text_action(pid, action, text))
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;

    #[test]
    fn missing_capture_never_disables_panel_and_write_falls_back_readably() {
        let state = NativeIntegrationState::default();
        let capture = state.latest_capture().unwrap();
        assert_eq!(capture.status, "empty");
        assert_eq!(capture.text, "");
        assert_eq!(capture.app_class, AppClass::Unknown);
        let access = state.external_text_access().unwrap();
        assert!(!access.actions_enabled);
        let result = state
            .perform_text_action(ExternalTextAction::Replace, "结果")
            .unwrap();
        assert!(result.fallback_to_clipboard);
        assert!(result.message.contains("剪贴板"));
    }

    #[test]
    fn insert_preserves_selected_text_while_replace_does_not() {
        assert_eq!(
            platform::selected_text_write_value(ExternalTextAction::Replace, "原文", "结果"),
            "结果"
        );
        assert_eq!(
            platform::selected_text_write_value(ExternalTextAction::Insert, "原文", "结果"),
            "原文结果"
        );
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{
        AppClass, CapturedTarget, ExternalTextAccess, ExternalTextAction, ExternalTextActionResult,
        SelectionCapture,
    };
    use std::{
        ffi::{c_char, c_int, c_long, c_void, CStr},
        mem,
        path::Path,
        process::Command,
        ptr,
    };

    type CFTypeRef = *const c_void;
    type CFStringRef = *const c_void;
    type AXUIElementRef = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    const AX_SUCCESS: c_int = 0;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXUIElementCreateSystemWide() -> AXUIElementRef;
        fn AXUIElementCreateApplication(pid: c_int) -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> c_int;
        fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut c_int) -> c_int;
        fn AXUIElementIsAttributeSettable(
            element: AXUIElementRef,
            attribute: CFStringRef,
            settable: *mut bool,
        ) -> c_int;
        fn AXUIElementSetAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: CFTypeRef,
        ) -> c_int;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            allocator: CFTypeRef,
            value: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFRelease(value: CFTypeRef);
        fn CFGetTypeID(value: CFTypeRef) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFStringGetLength(value: CFStringRef) -> c_long;
        fn CFStringGetMaximumSizeForEncoding(length: c_long, encoding: u32) -> c_long;
        fn CFStringGetCString(
            value: CFStringRef,
            buffer: *mut c_char,
            buffer_size: c_long,
            encoding: u32,
        ) -> bool;
    }

    #[link(name = "proc")]
    extern "C" {
        fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffer_size: u32) -> c_int;
    }

    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    unsafe fn cf_string(value: CFTypeRef) -> Option<String> {
        if value.is_null() || CFGetTypeID(value) != CFStringGetTypeID() {
            return None;
        }
        let length = CFStringGetLength(value);
        let capacity = CFStringGetMaximumSizeForEncoding(length, UTF8).checked_add(1)?;
        let mut bytes = vec![0_i8; usize::try_from(capacity).ok()?];
        if !CFStringGetCString(value, bytes.as_mut_ptr(), capacity, UTF8) {
            return None;
        }
        Some(
            CStr::from_ptr(bytes.as_ptr())
                .to_string_lossy()
                .into_owned(),
        )
    }

    unsafe fn attribute(element: AXUIElementRef, name: CFStringRef) -> Option<CFTypeRef> {
        let mut value: CFTypeRef = ptr::null();
        (AXUIElementCopyAttributeValue(element, name, &mut value) == 0 && !value.is_null())
            .then_some(value)
    }

    unsafe fn named_attribute(element: AXUIElementRef, name: &CStr) -> Option<CFTypeRef> {
        let key = CFStringCreateWithCString(ptr::null(), name.as_ptr(), UTF8);
        if key.is_null() {
            return None;
        }
        let value = attribute(element, key);
        CFRelease(key);
        value
    }

    unsafe fn frontmost_application_pid() -> Option<i32> {
        type SendObject = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
        type SendPid = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int;

        let workspace_class = objc_getClass(c"NSWorkspace".as_ptr());
        if workspace_class.is_null() {
            return None;
        }
        let send_object: SendObject = mem::transmute(objc_msgSend as *const ());
        let send_pid: SendPid = mem::transmute(objc_msgSend as *const ());
        let shared_workspace = send_object(
            workspace_class,
            sel_registerName(c"sharedWorkspace".as_ptr()),
        );
        if shared_workspace.is_null() {
            return None;
        }
        let application = send_object(
            shared_workspace,
            sel_registerName(c"frontmostApplication".as_ptr()),
        );
        if application.is_null() {
            return None;
        }
        let pid = send_pid(application, sel_registerName(c"processIdentifier".as_ptr()));
        (pid > 0).then_some(pid)
    }

    unsafe fn focused_element_for_pid(pid: i32) -> Option<AXUIElementRef> {
        let application = AXUIElementCreateApplication(pid);
        if application.is_null() {
            return None;
        }
        let focused = named_attribute(application, c"AXFocusedUIElement")
            .map(|value| value as AXUIElementRef);
        CFRelease(application);
        focused
    }

    unsafe fn element_is_secure(element: AXUIElementRef) -> bool {
        [c"AXRole", c"AXSubrole"].into_iter().any(|name| {
            named_attribute(element, name).is_some_and(|value| {
                let secure = cf_string(value)
                    .is_some_and(|text| text.to_ascii_lowercase().contains("secure"));
                CFRelease(value);
                secure
            })
        })
    }

    unsafe fn selected_text(element: AXUIElementRef) -> Option<String> {
        named_attribute(element, c"AXSelectedText").and_then(|value| {
            let text = cf_string(value);
            CFRelease(value);
            text
        })
    }

    unsafe fn selected_text_is_settable(element: AXUIElementRef) -> bool {
        let key = CFStringCreateWithCString(ptr::null(), c"AXSelectedText".as_ptr(), UTF8);
        if key.is_null() {
            return false;
        }
        let mut settable = false;
        let status = AXUIElementIsAttributeSettable(element, key, &mut settable);
        CFRelease(key);
        status == AX_SUCCESS && settable
    }

    unsafe fn set_selected_text(element: AXUIElementRef, text: &str) -> c_int {
        let Ok(text) = std::ffi::CString::new(text) else {
            return -25212;
        };
        let key = CFStringCreateWithCString(ptr::null(), c"AXSelectedText".as_ptr(), UTF8);
        let value = CFStringCreateWithCString(ptr::null(), text.as_ptr(), UTF8);
        if key.is_null() || value.is_null() {
            if !key.is_null() {
                CFRelease(key);
            }
            if !value.is_null() {
                CFRelease(value);
            }
            return -25212;
        }
        let status = AXUIElementSetAttributeValue(element, key, value);
        CFRelease(value);
        CFRelease(key);
        status
    }

    pub(super) fn selected_text_write_value(
        action: ExternalTextAction,
        selected: &str,
        result: &str,
    ) -> String {
        match action {
            ExternalTextAction::Replace => result.to_string(),
            ExternalTextAction::Insert => format!("{selected}{result}"),
        }
    }

    fn bundle_id_for_pid(pid: i32) -> Option<String> {
        let mut buffer = vec![0_u8; 4096];
        let count = unsafe { proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
        if count <= 0 {
            return None;
        }
        let executable = String::from_utf8_lossy(&buffer[..count as usize]).into_owned();
        let marker = ".app/Contents/";
        let end = executable.find(marker)? + ".app".len();
        let plist = Path::new(&executable[..end]).join("Contents/Info.plist");
        let output = Command::new("/usr/bin/plutil")
            .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
            .arg(plist)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|id| !id.is_empty())
    }

    fn classify_app(app_id: Option<&str>) -> AppClass {
        const CODE: &[&str] = &[
            "com.apple.dt.Xcode",
            "com.apple.Terminal",
            "com.anthropic.claudefordesktop",
            "com.googlecode.iterm2",
            "com.mitchellh.ghostty",
            "com.microsoft.VSCode",
            "com.openai.codex",
            "com.todesktop.230313mzl4w4u92",
        ];
        const PROSE: &[&str] = &[
            "com.apple.TextEdit",
            "com.apple.iWork.Pages",
            "com.apple.MobileSMS",
            "com.microsoft.Word",
            "com.tencent.xinWeChat",
            "com.tinyspeck.slackmacgap",
            "net.whatsapp.WhatsApp",
            "notion.id",
        ];
        const BROWSER: &[&str] = &[
            "com.apple.Safari",
            "com.google.Chrome",
            "com.brave.Browser",
            "com.microsoft.edgemac",
            "org.mozilla.firefox",
        ];
        let Some(app_id) = app_id else {
            return AppClass::Unknown;
        };
        if CODE.contains(&app_id) || app_id.starts_with("com.jetbrains.") {
            AppClass::Code
        } else if PROSE.contains(&app_id) {
            AppClass::Prose
        } else if BROWSER.contains(&app_id) {
            AppClass::Browser
        } else {
            AppClass::Unknown
        }
    }

    pub(super) fn capture_selection() -> CapturedTarget {
        if !unsafe { AXIsProcessTrusted() } {
            return CapturedTarget {
                capture: SelectionCapture {
                    status: "permission-denied".into(),
                    text: String::new(),
                    app_class: AppClass::Unknown,
                    message: "未授予辅助功能权限；替换和插入不可用，复制仍可用。可在“系统设置 → 隐私与安全性 → 辅助功能”授权。".into(),
                },
                pid: unsafe { frontmost_application_pid() },
            };
        }
        unsafe {
            let system = AXUIElementCreateSystemWide();
            let Some(focused_value) = named_attribute(system, c"AXFocusedUIElement") else {
                if !system.is_null() {
                    CFRelease(system);
                }
                return CapturedTarget {
                    capture: SelectionCapture {
                        status: "empty".into(),
                        text: String::new(),
                        app_class: AppClass::Unknown,
                        message: "未读取到前台选区，可手动粘贴。".into(),
                    },
                    pid: None,
                };
            };
            let focused = focused_value as AXUIElementRef;
            let mut pid = 0;
            let pid_ok = AXUIElementGetPid(focused, &mut pid) == 0;
            let secure = element_is_secure(focused);
            let app_class = if secure {
                AppClass::Unknown
            } else {
                classify_app(pid_ok.then(|| bundle_id_for_pid(pid)).flatten().as_deref())
            };
            let text = if secure { None } else { selected_text(focused) };
            CFRelease(focused_value);
            if !system.is_null() {
                CFRelease(system);
            }
            if secure {
                return CapturedTarget {
                    capture: SelectionCapture {
                        status: "secure".into(),
                        text: String::new(),
                        app_class: AppClass::Unknown,
                        message: "安全输入框不读取选区或场景档案；可手动粘贴非敏感文本。".into(),
                    },
                    pid: None,
                };
            }
            let text = text.unwrap_or_default();
            let status = if text.trim().is_empty() {
                "empty"
            } else {
                "captured"
            };
            let message = if status == "captured" {
                "已从前台 App 预填选区。"
            } else {
                "前台没有选中文字，可手动粘贴。"
            };
            CapturedTarget {
                capture: SelectionCapture {
                    status: status.into(),
                    text,
                    app_class,
                    message: message.into(),
                },
                pid: pid_ok.then_some(pid),
            }
        }
    }

    pub(super) fn external_text_access(pid: Option<i32>) -> ExternalTextAccess {
        if !unsafe { AXIsProcessTrusted() } {
            return ExternalTextAccess {
                authorized: false,
                target_available: pid.is_some(),
                actions_enabled: false,
                status: "permission-denied".into(),
                message: "替换与插入需要辅助功能权限；复制始终可用。请在“系统设置 → 隐私与安全性 → 辅助功能”授权，授权后会自动生效，无需重启。".into(),
            };
        }
        let Some(pid) = pid else {
            return ExternalTextAccess {
                authorized: true,
                target_available: false,
                actions_enabled: false,
                status: "target-missing".into(),
                message: "没有可写回的前台目标；请关闭小面板并从目标 App 重新唤起，复制仍可用。"
                    .into(),
            };
        };
        unsafe {
            let Some(focused) = focused_element_for_pid(pid) else {
                return ExternalTextAccess {
                    authorized: true,
                    target_available: false,
                    actions_enabled: false,
                    status: "target-missing".into(),
                    message: "原前台 App 没有可用的聚焦元素；复制仍可用。".into(),
                };
            };
            let secure = element_is_secure(focused);
            CFRelease(focused);
            if secure {
                ExternalTextAccess {
                    authorized: true,
                    target_available: true,
                    actions_enabled: false,
                    status: "secure-refused".into(),
                    message: "安全输入目标已拒绝：替换与插入不可用，复制仍可用。".into(),
                }
            } else {
                ExternalTextAccess {
                    authorized: true,
                    target_available: true,
                    actions_enabled: true,
                    status: "ready".into(),
                    message: "可替换原选区或在选区后插入；只有点击动作后才会写回。".into(),
                }
            }
        }
    }

    pub(super) fn perform_text_action(
        pid: Option<i32>,
        action: ExternalTextAction,
        result: &str,
    ) -> ExternalTextActionResult {
        let access = external_text_access(pid);
        if !access.actions_enabled {
            return ExternalTextActionResult {
                status: access.status,
                message: format!("{} 结果将退回剪贴板。", access.message),
                fallback_to_clipboard: true,
            };
        }
        let Some(pid) = pid else {
            unreachable!("enabled external-text access requires a captured pid");
        };
        unsafe {
            let Some(focused) = focused_element_for_pid(pid) else {
                return ExternalTextActionResult {
                    status: "target-missing".into(),
                    message: "原前台 App 的聚焦元素已失效；结果将退回剪贴板。".into(),
                    fallback_to_clipboard: true,
                };
            };
            if element_is_secure(focused) {
                CFRelease(focused);
                return ExternalTextActionResult {
                    status: "secure-refused".into(),
                    message: "安全输入目标已拒绝，未写入任何文字；结果将退回剪贴板。".into(),
                    fallback_to_clipboard: true,
                };
            }
            let selected = selected_text(focused);
            let Some(selected) = selected else {
                CFRelease(focused);
                return ExternalTextActionResult {
                    status: "unsupported".into(),
                    message: "目标不支持读取 AXSelectedText，未写入任何文字；结果将退回剪贴板。"
                        .into(),
                    fallback_to_clipboard: true,
                };
            };
            if action == ExternalTextAction::Replace && selected.is_empty() {
                CFRelease(focused);
                return ExternalTextActionResult {
                    status: "selection-missing".into(),
                    message: "前台没有选中文字可替换，未写入任何文字；结果将退回剪贴板。".into(),
                    fallback_to_clipboard: true,
                };
            }
            if !selected_text_is_settable(focused) {
                CFRelease(focused);
                return ExternalTextActionResult {
                    status: "read-only".into(),
                    message: "目标选区只读，不支持 AXSelectedText 写入；结果将退回剪贴板。".into(),
                    fallback_to_clipboard: true,
                };
            }
            let value = selected_text_write_value(action, &selected, result);
            let status = set_selected_text(focused, &value);
            CFRelease(focused);
            if status != AX_SUCCESS {
                return ExternalTextActionResult {
                    status: "write-failed".into(),
                    message: format!(
                        "目标拒绝 AXSelectedText 写入（AXError {status}），未静默失败；结果将退回剪贴板。"
                    ),
                    fallback_to_clipboard: true,
                };
            }
        }
        let (status, message) = match action {
            ExternalTextAction::Replace => ("replaced", "已原地替换前台选区。"),
            ExternalTextAction::Insert => ("inserted", "已在前台选区后插入，原文保持不变。"),
        };
        ExternalTextActionResult {
            status: status.into(),
            message: message.into(),
            fallback_to_clipboard: false,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn app_class_mapping_matches_context_routing_contract() {
            assert_eq!(classify_app(Some("com.apple.dt.Xcode")), AppClass::Code);
            assert_eq!(classify_app(Some("com.apple.TextEdit")), AppClass::Prose);
            assert_eq!(classify_app(Some("com.apple.Safari")), AppClass::Browser);
            assert_eq!(classify_app(Some("private.unknown")), AppClass::Unknown);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::{
        AppClass, CapturedTarget, ExternalTextAccess, ExternalTextAction, ExternalTextActionResult,
        SelectionCapture,
    };

    pub(super) fn selected_text_write_value(
        action: ExternalTextAction,
        selected: &str,
        result: &str,
    ) -> String {
        match action {
            ExternalTextAction::Replace => result.to_string(),
            ExternalTextAction::Insert => format!("{selected}{result}"),
        }
    }

    pub(super) fn capture_selection() -> CapturedTarget {
        CapturedTarget {
            capture: SelectionCapture {
                status: "unsupported".into(),
                text: String::new(),
                app_class: AppClass::Unknown,
                message: "当前平台不支持辅助功能选区读取，可手动粘贴。".into(),
            },
            pid: None,
        }
    }

    pub(super) fn external_text_access(pid: Option<i32>) -> ExternalTextAccess {
        ExternalTextAccess {
            authorized: false,
            target_available: pid.is_some(),
            actions_enabled: false,
            status: "unsupported".into(),
            message: "当前平台不支持 AXSelectedText 写入；复制仍可用。".into(),
        }
    }

    pub(super) fn perform_text_action(
        _: Option<i32>,
        _: ExternalTextAction,
        _: &str,
    ) -> ExternalTextActionResult {
        ExternalTextActionResult {
            status: "unsupported".into(),
            message: "当前平台不支持 AXSelectedText 写入；结果将退回剪贴板。".into(),
            fallback_to_clipboard: true,
        }
    }
}

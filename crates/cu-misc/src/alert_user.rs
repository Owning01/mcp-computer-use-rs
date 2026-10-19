// cu-misc::alert_user — port de computer_alert_user (index.js + nativeAlertUser).
// MessageBoxW en thread separado + auto-cierre por timer (replica Wscript.Shell.Popup con timeout).
use serde_json::{json, Value};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, MessageBoxW, PostMessageW, MB_ICONINFORMATION, MB_OK, WM_CLOSE,
};

pub fn alert_user(args: &Value) -> Value {
    let msg = args
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if msg.is_empty() {
        return json!({"ok": false, "error": "message parameter is required"});
    }
    let sound = !matches!(args.get("sound"), Some(Value::Bool(false)));
    let duration_ms = args
        .get("duration_ms")
        .and_then(|v| v.as_f64())
        .filter(|n| *n > 0.0)
        .unwrap_or(5000.0) as u64;

    if sound {
        use windows::Win32::System::Diagnostics::Debug::MessageBeep;
        use windows::Win32::UI::WindowsAndMessaging::MB_ICONEXCLAMATION;
        let _ = unsafe { MessageBeep(MB_ICONEXCLAMATION) };
    }

    let msg_clone = msg.clone();
    // Auto-close thread
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(duration_ms));
        unsafe {
            let title: Vec<u16> = "Antigravity Assistant\0".encode_utf16().collect();
            if let Ok(hwnd) = FindWindowW(None, windows::core::PCWSTR(title.as_ptr())) {
                if !hwnd.0.is_null() {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
        }
    });
    // Dialog thread (blocking, but separate so tool returns immediately)
    std::thread::spawn(move || {
        unsafe {
            let text: Vec<u16> = msg_clone.encode_utf16().chain(std::iter::once(0)).collect();
            let title: Vec<u16> = "Antigravity Assistant\0".encode_utf16().collect();
            let _ = MessageBoxW(
                None,
                windows::core::PCWSTR(text.as_ptr()),
                windows::core::PCWSTR(title.as_ptr()),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    });

    json!({"ok": true, "message": msg, "indicator": true})
}
// cu-misc::dismiss_overlay — port de nativeDismissOverlay (index.js).
// Implementación segura: sin UIA/inspección de elementos; devuelve no-op (dismissed=false)
// salvo que se proporcione target_window, en cuyo caso envía ESC vía SendInput.
use cu_native::system::active_window;
use serde_json::{json, Value};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_ESCAPE,
};

pub fn dismiss_overlay(args: &Value) -> Value {
    let target_window = args.get("target_window").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let max_attempts = {
        let n = args.get("max_attempts").and_then(|v| v.as_f64()).unwrap_or(3.0);
        if !n.is_finite() { 3i64 } else { (n as i64).clamp(1, 5) }
    };

    if !target_window.is_empty() {
        // focus not implemented; send ESC max_attempts times via SendInput
        for _ in 0..max_attempts {
            send_escape();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    let active = active_window();
    let title = active.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
    json!({
        "ok": true,
        "dismissed": false,
        "clicked_element": Value::Null,
        "active_window": title
    })
}

fn send_escape() {
    unsafe {
        let mut inputs: [INPUT; 2] = [
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_ESCAPE,
                        wScan: 0,
                        dwFlags: Default::default(),
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_ESCAPE,
                        wScan: 0,
                        dwFlags: KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
        ];
        SendInput(&mut inputs, std::mem::size_of::<INPUT>() as i32);
    }
}
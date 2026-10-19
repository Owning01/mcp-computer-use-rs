// PORT NOTE: nativeWindowAction portado. Se omite el intento DesktopBridge.exe (no existe en Rust).
use crate::focus::focus_window;
use serde_json::{json, Value, Map};
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYBD_EVENT_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowTextW, IsWindowVisible, PostMessageW,
    SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, ShowWindowAsync, WM_CLOSE,
};

pub fn window_action(args: &Value) -> Value {
    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if action.is_empty() {
        return json!({"ok": false, "error": "action parameter is required"});
    }
    // confirm handling for close is done in handle_call layer? Origin elicits in handler before calling native.
    // Here we assume confirm already resolved by caller; if action close and confirm != true, origin elicits.
    // We implement direct action.

    let target = args.get("target_window").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if !target.is_empty() {
        let _ = focus_window(&json!({"title": target}));
    }

    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            return json!({"ok": false, "error": "no_foreground_window"});
        }

        match action.as_str() {
            "maximize" => {
                let _ = ShowWindowAsync(fg, SW_MAXIMIZE);
                bump_and_return(json!({"ok": true, "action": "maximize", "hwnd": fg.0 as isize as i64}))
            }
            "minimize" => {
                let _ = ShowWindowAsync(fg, SW_MINIMIZE);
                bump_and_return(json!({"ok": true, "action": "minimize", "hwnd": fg.0 as isize as i64}))
            }
            "restore" => {
                let _ = ShowWindowAsync(fg, SW_RESTORE);
                bump_and_return(json!({"ok": true, "action": "restore", "hwnd": fg.0 as isize as i64}))
            }
            "close" => {
                let _ = PostMessageW(Some(fg), WM_CLOSE, windows::Win32::Foundation::WPARAM(0), windows::Win32::Foundation::LPARAM(0));
                bump_and_return(json!({"ok": true, "action": "close", "hwnd": fg.0 as isize as i64}))
            }
            "snap_left" => {
                // Win+Left
                keybd_event(0x5B, 0, KEYBD_EVENT_FLAGS(0), 0usize); // LWIN down
                keybd_event(0x25, 0, KEYBD_EVENT_FLAGS(0), 0usize); // LEFT down
                std::thread::sleep(std::time::Duration::from_millis(40));
                keybd_event(0x25, 0, KEYBD_EVENT_FLAGS(2), 0usize); // LEFT up
                keybd_event(0x5B, 0, KEYBD_EVENT_FLAGS(2), 0usize); // LWIN up
                bump_and_return(json!({"ok": true, "action": "snap_left", "hwnd": fg.0 as isize as i64}))
            }
            "snap_right" => {
                keybd_event(0x5B, 0, KEYBD_EVENT_FLAGS(0), 0usize);
                keybd_event(0x27, 0, KEYBD_EVENT_FLAGS(0), 0usize);
                std::thread::sleep(std::time::Duration::from_millis(40));
                keybd_event(0x27, 0, KEYBD_EVENT_FLAGS(2), 0usize);
                keybd_event(0x5B, 0, KEYBD_EVENT_FLAGS(2), 0usize);
                bump_and_return(json!({"ok": true, "action": "snap_right", "hwnd": fg.0 as isize as i64}))
            }
            "move_resize" => {
                let x = args.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let y = args.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let w = args.get("width").and_then(|v| v.as_i64()).unwrap_or(800) as i32;
                let h = args.get("height").and_then(|v| v.as_i64()).unwrap_or(600) as i32;

                // DWM invisible border compensation
                let mut outer = RECT::default();
                let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(fg, &mut outer);
                let mut visible = RECT::default();
                let has_dwm = DwmGetWindowAttribute(fg, DWMWA_EXTENDED_FRAME_BOUNDS, &mut visible as *mut _ as *mut core::ffi::c_void, std::mem::size_of::<RECT>() as u32).is_ok();
                let (dx, dy, dw, dh) = if has_dwm {
                    let dx = visible.left - outer.left;
                    let dy = visible.top - outer.top;
                    let dw = (outer.right - outer.left) - (visible.right - visible.left);
                    let dh = (outer.bottom - outer.top) - (visible.bottom - visible.top);
                    (dx, dy, dw, dh)
                } else {
                    (0, 0, 0, 0)
                };

                let target_x = x - dx;
                let target_y = y - dy;
                let target_w = w + dw;
                let target_h = h + dh;

                use windows::Win32::UI::WindowsAndMessaging::SetWindowPos;
                use windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS;
                // 0x0014 = SWP_NOZORDER | SWP_NOACTIVATE
                let flags = SET_WINDOW_POS_FLAGS(0x0014);
                let _ = SetWindowPos(fg, None, target_x, target_y, target_w, target_h, flags);
                let _ = SetWindowPos(fg, None, target_x, target_y, target_w, target_h, flags);

                let mut obj = Map::new();
                obj.insert("ok".into(), json!(true));
                obj.insert("action".into(), json!("move_resize"));
                obj.insert("hwnd".into(), json!(fg.0 as isize as i64));
                obj.insert("x".into(), json!(x));
                obj.insert("y".into(), json!(y));
                obj.insert("width".into(), json!(w));
                obj.insert("height".into(), json!(h));
                obj.insert("dwm_compensated".into(), json!(has_dwm));
                bump_and_return(Value::Object(obj))
            }
            "dismiss_dialogs" => {
                let mut dismissed: Vec<Value> = Vec::new();
                struct Ctx { dismissed: *mut Vec<Value> }
                unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
                    unsafe {
                        if !IsWindowVisible(hwnd).as_bool() {
                            return windows::core::BOOL(1);
                        }
                        let mut cls_buf = [0u16; 256];
                        let cn = GetClassNameW(hwnd, &mut cls_buf);
                        if cn <= 0 {
                            return windows::core::BOOL(1);
                        }
                        let cls = String::from_utf16_lossy(&cls_buf[..cn as usize]);
                        let is_dialog = cls == "#32770"
                            || cls.to_lowercase().contains("taskdialog")
                            || cls.to_lowercase().contains("dialog");
                        if is_dialog {
                            let mut title_buf = [0u16; 256];
                            let tn = GetWindowTextW(hwnd, &mut title_buf);
                            let title = if tn > 0 {
                                String::from_utf16_lossy(&title_buf[..tn as usize])
                            } else {
                                String::new()
                            };
                            let _ = PostMessageW(Some(hwnd), WM_CLOSE, windows::Win32::Foundation::WPARAM(0), windows::Win32::Foundation::LPARAM(0));
                            let ctx = &mut *(lparam.0 as *mut Ctx);
                            (*ctx.dismissed).push(json!({"hwnd": hwnd.0 as isize as i64, "title": title, "class": cls}));
                        }
                        windows::core::BOOL(1)
                    }
                }
                let mut ctx = Ctx { dismissed: &mut dismissed };
                let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut Ctx as isize));
                let count = dismissed.len();
                let mut obj = Map::new();
                obj.insert("ok".into(), json!(true));
                obj.insert("action".into(), json!("dismiss_dialogs"));
                obj.insert("dismissed".into(), json!(dismissed));
                obj.insert("count".into(), json!(count));
                bump_and_return(Value::Object(obj))
            }
            _ => {
                // default: ok true action hwnd (como origen para acciones desconocidas)
                bump_and_return(json!({"ok": true, "action": action, "hwnd": fg.0 as isize as i64}))
            }
        }
    }
}

fn bump_and_return(v: Value) -> Value {
    let epoch = cu_core::epoch::bump();
    // insert epoch into object if possible
    match v {
        Value::Object(mut m) => {
            m.insert("epoch".into(), json!(epoch));
            Value::Object(m)
        }
        other => other,
    }
}

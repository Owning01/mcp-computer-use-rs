// cu-input — input nativo (SendInput/keybd_event/mouse_event). Reglas: migration/RULEBOOK.md.
// Contrato del puente: handles(name) + handle_call(name, args) -> cu_core::out::ToolOut.
use base64::Engine;
use cu_core::out::ToolOut;
use serde_json::{json, Value};
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::{HWND, LPARAM, POINT};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::System::Threading::AttachThreadInput;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentThreadId() -> u32;
}

// ---------------------------------------------------------------------------
// Helpers de semántica JS (int/clamp).
// ---------------------------------------------------------------------------
fn int_arg(v: Option<&Value>, min: i64, max: i64, def: i64) -> i64 {
    let n = match v {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        Some(Value::Bool(b)) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    };
    let n = match n {
        Some(f) if f.is_finite() => f.trunc() as i64,
        _ => def,
    };
    n.clamp(min, max)
}

fn get_vk(k: &str) -> u8 {
    let kl = k.trim().to_lowercase();
    match kl.as_str() {
        "enter" | "return" => 0x0D,
        "tab" => 0x09,
        "escape" | "esc" => 0x1B,
        "backspace" | "bs" => 0x08,
        "delete" | "del" => 0x2E,
        "space" => 0x20,
        "up" | "arrowup" => 0x26,
        "down" | "arrowdown" => 0x28,
        "left" | "arrowleft" => 0x25,
        "right" | "arrowright" => 0x27,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "f1" => 0x70,
        "f2" => 0x71,
        "f3" => 0x72,
        "f4" => 0x73,
        "f5" => 0x74,
        "f6" => 0x75,
        "f7" => 0x76,
        "f8" => 0x77,
        "f9" => 0x78,
        "f10" => 0x79,
        "f11" => 0x7A,
        "f12" => 0x7B,
        "ctrl" | "control" => 0x11,
        "shift" => 0x10,
        "alt" => 0x12,
        "meta" | "win" | "super" | "cmd" => 0x5B,
        _ => {
            if kl.len() == 1 {
                let c = kl.chars().next().unwrap();
                if c.is_ascii_lowercase() {
                    return 0x41 + (c as u8 - b'a');
                }
                if c.is_ascii_digit() {
                    return 0x30 + (c as u8 - b'0');
                }
            }
            0
        }
    }
}

fn parse_hotkey(combo: &str) -> (String, Vec<String>) {
    let parts: Vec<String> = combo
        .to_lowercase()
        .split('+')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let mut mods = Vec::new();
    let mut key = String::new();
    for p in parts {
        if p == "ctrl" || p == "control" {
            mods.push("ctrl".to_string());
        } else if p == "shift" {
            mods.push("shift".to_string());
        } else if p == "alt" {
            mods.push("alt".to_string());
        } else if p == "win" || p == "meta" || p == "super" || p == "cmd" {
            mods.push("meta".to_string());
        } else {
            key = p;
        }
    }
    (key, mods)
}

fn native_key_action(key: &str, action: &str, mods: &[String]) {
    let mod_vks: Vec<u8> = mods
        .iter()
        .map(|m| get_vk(m))
        .filter(|&v| v != 0)
        .collect();
    let main_vk = get_vk(key);
    unsafe {
        for &m in &mod_vks {
            keybd_event(m, 0, KEYBD_EVENT_FLAGS(0), 0);
        }
        match action {
            "down" => {
                if main_vk != 0 {
                    keybd_event(main_vk, 0, KEYBD_EVENT_FLAGS(0), 0);
                }
            }
            "up" => {
                if main_vk != 0 {
                    keybd_event(main_vk, 0, KEYEVENTF_KEYUP, 0);
                }
                let mut rev = mod_vks.clone();
                rev.reverse();
                for &m in &rev {
                    keybd_event(m, 0, KEYEVENTF_KEYUP, 0);
                }
            }
            _ => {
                if main_vk != 0 {
                    keybd_event(main_vk, 0, KEYBD_EVENT_FLAGS(0), 0);
                    thread::sleep(Duration::from_millis(20));
                    keybd_event(main_vk, 0, KEYEVENTF_KEYUP, 0);
                }
                let mut rev = mod_vks.clone();
                rev.reverse();
                for &m in &rev {
                    keybd_event(m, 0, KEYEVENTF_KEYUP, 0);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Foco de ventana (versión simplificada del helper del origen).
// ---------------------------------------------------------------------------
unsafe fn find_window_by_title(needle: &str) -> HWND {
    let needle_l = needle.to_lowercase();
    struct Ctx {
        needle: String,
        found: HWND,
    }
    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        unsafe {
            let ctx = &mut *(lparam.0 as *mut Ctx);
            if !IsWindowVisible(hwnd).as_bool() {
                return windows::core::BOOL(1);
            }
            let mut buf = [0u16; 512];
            let len = GetWindowTextW(hwnd, &mut buf);
            if len > 0 {
                let title = String::from_utf16_lossy(&buf[..len as usize]).to_lowercase();
                if !ctx.needle.is_empty() && title.contains(&ctx.needle) {
                    ctx.found = hwnd;
                    return windows::core::BOOL(0);
                }
            }
            windows::core::BOOL(1)
        }
    }
    let mut ctx = Ctx {
        needle: needle_l,
        found: HWND(std::ptr::null_mut()),
    };
    unsafe { let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut _ as isize)); }
    ctx.found
}

unsafe fn focus_window(target: &Value) {
    // Resolver hwnd.
    let mut hwnd_val: i64 = 0;
    if let Some(n) = target.as_i64() {
        hwnd_val = n;
    } else if let Some(s) = target.as_str() {
        let t = s.trim();
        if let Ok(n) = t.parse::<i64>() {
            hwnd_val = n;
        }
    }
    let mut target_hwnd = if hwnd_val > 0 {
        HWND(hwnd_val as *mut _)
    } else {
        HWND(std::ptr::null_mut())
    };
    if target_hwnd.0.is_null() {
        if let Some(s) = target.as_str() {
            target_hwnd = unsafe { find_window_by_title(s) };
        }
    }
    if target_hwnd.0.is_null() {
        return;
    }
    // Restaurar si está minimizada.
    unsafe {
        if IsIconic(target_hwnd).as_bool() {
            let _ = ShowWindow(target_hwnd, SW_RESTORE);
            thread::sleep(Duration::from_millis(40));
        }
    }
    let mut pid: u32 = 0;
    let fg = unsafe { GetForegroundWindow() };
    let fg_thread = if !fg.0.is_null() {
        unsafe { GetWindowThreadProcessId(fg, Some(&mut pid)) }
    } else {
        0
    };
    let target_thread = unsafe { GetWindowThreadProcessId(target_hwnd, Some(&mut pid)) };
    let cur_thread = unsafe { GetCurrentThreadId() };
    unsafe {
        if fg_thread != 0 && cur_thread != fg_thread {
            let _ = AttachThreadInput(cur_thread, fg_thread, true);
        }
        if target_thread != 0 && cur_thread != target_thread {
            let _ = AttachThreadInput(cur_thread, target_thread, true);
        }
        let _ = ShowWindow(target_hwnd, SW_RESTORE);
        let _ = BringWindowToTop(target_hwnd);
        let _ = SetForegroundWindow(target_hwnd);
        if fg_thread != 0 && cur_thread != fg_thread {
            let _ = AttachThreadInput(cur_thread, fg_thread, false);
        }
        if target_thread != 0 && cur_thread != target_thread {
            let _ = AttachThreadInput(cur_thread, target_thread, false);
        }
    }
}

// ---------------------------------------------------------------------------
// handlers
// ---------------------------------------------------------------------------
pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "computer_hotkey"
            | "computer_key"
            | "computer_move"
            | "computer_copy"
            | "computer_paste"
            | "computer_bulk_input"
            | "computer_drag_scroll"
            | "computer_click"
            | "computer_double_click"
            | "computer_human_click"
            | "computer_drag"
            | "computer_scroll"
            | "computer_type"
    )
}

pub fn handle_call(name: &str, args: &Value) -> ToolOut {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match name {
        "computer_hotkey" => handle_hotkey(args),
        "computer_key" => handle_key(args),
        "computer_move" => handle_move(args),
        "computer_copy" => handle_copy(args),
        "computer_paste" => handle_paste(args),
        "computer_bulk_input" => handle_bulk_input(args),
        "computer_drag_scroll" => handle_drag_scroll(args),
        "computer_click" => handle_click(args, false),
        "computer_double_click" => handle_click(args, true),
        "computer_human_click" => handle_human_click(args),
        "computer_drag" => handle_drag(args),
        "computer_scroll" => handle_scroll(args),
        "computer_type" => handle_type(args),
        _ => ToolOut::Json(json!({"ok": false, "error": "not_implemented"}), true),
    }));
    match result {
        Ok(out) => out,
        Err(_) => ToolOut::Json(json!({"ok": false, "error": "panic"}), true),
    }
}

fn handle_hotkey(args: &Value) -> ToolOut {
    let combo = args
        .get("combo")
        .map(cu_core::js::js_string_of)
        .unwrap_or_default()
        .trim()
        .to_string();
    if combo.is_empty() {
        return ToolOut::Json(json!({"ok": false, "error": "combo parameter is required (e.g. 'ctrl+c', 'win+r')"}), true);
    }
    let (key, mods) = parse_hotkey(&combo);
    if key.is_empty() && mods.is_empty() {
        return ToolOut::Json(json!({"ok": false, "error": format!("Invalid hotkey combo: {}", combo)}), true);
    }
    let mods_ref: Vec<String> = mods.clone();
    native_key_action(&key, "tap", &mods_ref);
    let epoch = cu_core::epoch::bump();
    ToolOut::Json(
        json!({"ok": true, "hotkey": combo, "key": key, "mods": mods, "epoch": epoch}),
        false,
    )
}

fn handle_key(args: &Value) -> ToolOut {
    let key = args
        .get("key")
        .map(cu_core::js::js_string_of)
        .unwrap_or_default();
    let action = args
        .get("action")
        .map(cu_core::js::js_string_of)
        .unwrap_or_else(|| "tap".to_string());
    let action = if action.is_empty() { "tap".to_string() } else { action };
    let mods: Vec<String> = match args.get("mods") {
        Some(Value::Array(a)) => a.iter().map(cu_core::js::js_string_of).collect(),
        Some(v) => vec![cu_core::js::js_string_of(v)],
        None => Vec::new(),
    };
    native_key_action(&key, &action, &mods);
    let epoch = cu_core::epoch::bump();
    ToolOut::Json(json!({"ok": true, "action": "key", "epoch": epoch}), false)
}

fn handle_move(args: &Value) -> ToolOut {
    let x = int_arg(args.get("x"), -100000, 100000, 0) as i32;
    let y = int_arg(args.get("y"), -100000, 100000, 0) as i32;
    unsafe {
        let _ = SetCursorPos(x, y);
    }
    let epoch = cu_core::epoch::bump();
    ToolOut::Json(json!({"ok": true, "action": "move", "epoch": epoch}), false)
}

fn handle_copy(args: &Value) -> ToolOut {
    if let Some(tw) = args.get("target_window") {
        if !tw.is_null() {
            unsafe { focus_window(tw) };
        }
    }
    native_key_action("c", "tap", &["ctrl".to_string()]);
    thread::sleep(Duration::from_millis(60));
    let text = cu_native::system::clipboard_get().unwrap_or_default().trim().to_string();
    ToolOut::Json(json!({"ok": true, "text": text}), false)
}

fn handle_paste(args: &Value) -> ToolOut {
    if let Some(tw) = args.get("target_window") {
        if !tw.is_null() {
            // app_blocked check omitido (no disponible en Rust); foco directo.
            unsafe { focus_window(tw) };
        }
    }
    let mut text = args
        .get("text")
        .map(cu_core::js::js_string_of)
        .unwrap_or_default();
    let is_secret = args.get("secret_b64").is_some();
    if let Some(b64) = args.get("secret_b64").and_then(Value::as_str) {
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
            text = String::from_utf8_lossy(&bytes).to_string();
        }
    }
    let press_enter = args
        .get("press_enter")
        .map(|v| cu_core::js::js_truthy(v))
        .unwrap_or(false);

    // nativeSafePaste
    let prev = cu_native::system::clipboard_get().unwrap_or_default();
    let _ = cu_native::system::clipboard_set(&text);
    native_key_action("v", "tap", &["ctrl".to_string()]);
    if press_enter {
        thread::sleep(Duration::from_millis(40));
        unsafe {
            keybd_event(0x0D, 0, KEYBD_EVENT_FLAGS(0), 0);
            thread::sleep(Duration::from_millis(20));
            keybd_event(0x0D, 0, KEYEVENTF_KEYUP, 0);
        }
    }
    thread::sleep(Duration::from_millis(60));
    // restaurar clipboard previo
    if !prev.is_empty() {
        let _ = cu_native::system::clipboard_set(&prev);
    } else {
        let _ = cu_native::system::clipboard_set("");
    }
    let _ = is_secret;
    let epoch = cu_core::epoch::current();
    let payload = json!({
        "ok": true,
        "action": "paste",
        "epoch": epoch,
        "clipboard_restored": true
    });
    ToolOut::Json(payload, false)
}

fn handle_bulk_input(args: &Value) -> ToolOut {
    let rows = args.get("rows").cloned().unwrap_or(Value::Array(Vec::new()));
    let rows_arr = match rows {
        Value::Array(a) => a,
        _ => Vec::new(),
    };
    if rows_arr.is_empty() {
        return ToolOut::Json(json!({"ok": false, "error": "rows parameter must be a non-empty 2D array"}), true);
    }
    if let Some(tw) = args.get("target_window") {
        if !tw.is_null() {
            unsafe { focus_window(tw) };
        }
    }
    let target_x = int_arg(args.get("target_x"), -100000, 100000, 0) as i32;
    let target_y = int_arg(args.get("target_y"), -100000, 100000, 0) as i32;
    if target_x != 0 || target_y != 0 {
        unsafe {
            let _ = SetCursorPos(target_x, target_y);
            thread::sleep(Duration::from_millis(20));
            mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
            thread::sleep(Duration::from_millis(40));
            mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
            thread::sleep(Duration::from_millis(80));
        }
    }

    // Construir TSV.
    let mut tsv_lines: Vec<String> = Vec::new();
    let mut total_cells: i64 = 0;
    for row in &rows_arr {
        let cells: Vec<String> = match row {
            Value::Array(a) => a.iter().map(cu_core::js::js_string_of).collect(),
            _ => vec![cu_core::js::js_string_of(row)],
        };
        total_cells += cells.len() as i64;
        tsv_lines.push(cells.join("\t"));
    }
    let tsv_text = tsv_lines.join("\n");

    let _ = cu_native::system::clipboard_set(&tsv_text);
    thread::sleep(Duration::from_millis(40));
    native_key_action("v", "tap", &["ctrl".to_string()]);

    let row_count = rows_arr.len() as i64;
    let epoch = cu_core::epoch::bump();
    ToolOut::Json(
        json!({
            "ok": true,
            "rows_injected": row_count,
            "total_cells": total_cells,
            "method": "clipboard_tsv_grid",
            "epoch": epoch
        }),
        false,
    )
}

fn handle_drag_scroll(args: &Value) -> ToolOut {
    // Parse payload like origin handle.
    let mode = args
        .get("mode")
        .map(cu_core::js::js_string_of)
        .unwrap_or_else(|| "rubber_band".to_string())
        .to_lowercase();
    let mode = if mode.is_empty() { "rubber_band".to_string() } else { mode };

    let from_x = if args.get("from_x").is_none() { 0 } else { int_arg(args.get("from_x"), -1_000_000, 1_000_000, 0) as i32 };
    let from_y = if args.get("from_y").is_none() { 0 } else { int_arg(args.get("from_y"), -1_000_000, 1_000_000, 0) as i32 };
    let to_x = if args.get("to_x").is_none() { 0 } else { int_arg(args.get("to_x"), -1_000_000, 1_000_000, 0) as i32 };
    let to_y = if args.get("to_y").is_none() { 0 } else { int_arg(args.get("to_y"), -1_000_000, 1_000_000, 0) as i32 };

    let button = args
        .get("button")
        .map(cu_core::js::js_string_of)
        .unwrap_or_else(|| "left".to_string())
        .to_lowercase();
    let button = if button.is_empty() { "left".to_string() } else { button };

    let modifiers: Vec<String> = match args.get("modifiers") {
        Some(Value::Array(a)) => a.iter().map(cu_core::js::js_string_of).collect(),
        Some(v) => vec![cu_core::js::js_string_of(v)],
        None => Vec::new(),
    };

    let mut steps = args.get("steps").map(|v| int_arg(Some(v), 1, 100000, 14)).unwrap_or(14) as i32;
    if steps <= 0 { steps = 14; }
    let mut delay_ms = args.get("delay_ms").map(|v| int_arg(Some(v), 1, 100000, 15)).unwrap_or(15) as i32;
    if delay_ms <= 0 { delay_ms = 15; }

    let hold_default = if mode == "drag_scroll" { 800 } else { 0 };
    let hold_duration_ms = args
        .get("hold_duration_ms")
        .map(|v| int_arg(Some(v), 0, 1_000_000, hold_default))
        .unwrap_or(hold_default as i64) as i32;

    let scroll_pulses = args.get("scroll_pulses").map(|v| int_arg(Some(v), 0, 100000, 0)).unwrap_or(0) as i32;
    let scroll_delta = args.get("scroll_delta").map(|v| int_arg(Some(v), -100000, 100000, -120)).unwrap_or(-120) as i32;
    let release = args.get("release").map(|v| cu_core::js::js_truthy(v)).unwrap_or(true);

    // Ejecutar gesto.
    let mut modifiers_applied: Vec<String> = Vec::new();
    let mut pressed_keys: Vec<u8> = Vec::new();
    let mut pulses_sent: i64 = 0;

    let (down_flag, up_flag) = if button == "right" {
        (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
    } else if button == "middle" {
        (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP)
    } else {
        (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
    };

    unsafe {
        // press modifiers
        for m in &modifiers {
            let vk = match m.to_lowercase().as_str() {
                "ctrl" | "control" => 0x11u8,
                "shift" => 0x10u8,
                "alt" => 0x12u8,
                _ => 0u8,
            };
            if vk != 0 && !pressed_keys.contains(&vk) {
                keybd_event(vk, 0, KEYBD_EVENT_FLAGS(0), 0);
                pressed_keys.push(vk);
                modifiers_applied.push(m.clone());
            }
        }
        if !pressed_keys.is_empty() {
            thread::sleep(Duration::from_millis(30));
        }

        let _ = SetCursorPos(from_x, from_y);
        mouse_event(down_flag, 0, 0, 0, 0);
        thread::sleep(Duration::from_millis(40));

        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            let smooth = t * t * (3.0 - 2.0 * t);
            let cur_x = (from_x as f64 + (to_x - from_x) as f64 * smooth).round() as i32;
            let cur_y = (from_y as f64 + (to_y - from_y) as f64 * smooth).round() as i32;
            let _ = SetCursorPos(cur_x, cur_y);
            thread::sleep(Duration::from_millis(delay_ms as u64));
        }
        let _ = SetCursorPos(to_x, to_y);

        if (mode == "drag_scroll" || mode == "scroll") && hold_duration_ms > 0 {
            let total_hold = hold_duration_ms;
            let interval_ms = 60i32;
            let iterations = (total_hold / interval_ms).max(1);
            let pulse_step = if scroll_pulses > 0 {
                (iterations / scroll_pulses).max(1)
            } else {
                0
            };
            for i in 0..iterations {
                let jitter = if i % 2 == 0 { 1 } else { -1 };
                let _ = SetCursorPos(to_x + jitter, to_y);
                thread::sleep(Duration::from_millis((interval_ms / 2) as u64));
                let _ = SetCursorPos(to_x, to_y);
                thread::sleep(Duration::from_millis((interval_ms / 2) as u64));
                if scroll_pulses > 0 && pulse_step > 0 && (i % pulse_step == 0) && pulses_sent < scroll_pulses as i64 {
                    let delta = if scroll_delta != 0 { scroll_delta } else { -120 };
                    mouse_event(MOUSEEVENTF_WHEEL, 0, 0, delta, 0);
                    pulses_sent += 1;
                }
            }
        }

        if release {
            thread::sleep(Duration::from_millis(30));
            mouse_event(up_flag, 0, 0, 0, 0);
        }

        // release modifiers
        for vk in &pressed_keys {
            keybd_event(*vk, 0, KEYEVENTF_KEYUP, 0);
        }
    }

    let mut from_obj = serde_json::Map::new();
    from_obj.insert("x".to_string(), json!(from_x));
    from_obj.insert("y".to_string(), json!(from_y));
    let mut to_obj = serde_json::Map::new();
    to_obj.insert("x".to_string(), json!(to_x));
    to_obj.insert("y".to_string(), json!(to_y));

    let mut res = serde_json::Map::new();
    res.insert("ok".to_string(), json!(true));
    res.insert("mode".to_string(), json!(mode));
    res.insert("from".to_string(), Value::Object(from_obj));
    res.insert("to".to_string(), Value::Object(to_obj));
    res.insert("modifiers_applied".to_string(), json!(modifiers_applied));
    res.insert("steps".to_string(), json!(steps));
    res.insert("pulses_sent".to_string(), json!(pulses_sent));

    ToolOut::Json(Value::Object(res), false)
}

fn arg_f64(args: &Value, key: &str) -> Option<f64> {
    match args.get(key) {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn has_arg(args: &Value, key: &str) -> bool {
    args.get(key).map(|v| !v.is_null()).unwrap_or(false)
}

pub fn accepts(name: &str, args: &Value) -> bool {
    match name {
        "computer_hotkey" => args.get("combo").map(|v| !cu_core::js::js_string_of(v).trim().is_empty()).unwrap_or(false),
        "computer_key" => true,
        "computer_move" => true,
        "computer_copy" => true,
        "computer_paste" => true,
        "computer_bulk_input" => {
            let rows_ok = match args.get("rows") {
                Some(Value::Array(a)) => !a.is_empty(),
                _ => false,
            };
            if !rows_ok { return false; }
            // target_text sin coords resueltas => proxy (no lo resolvemos nativo).
            if has_arg(args, "target_text")
                && !(arg_f64(args, "target_x").is_some() && arg_f64(args, "target_y").is_some())
            {
                return false;
            }
            true
        }
        "computer_drag_scroll" => true,
        "computer_click" | "computer_double_click" => {
            if arg_f64(args, "x").is_none() || arg_f64(args, "y").is_none() { return false; }
            // subsistemas externos no soportados
            if has_arg(args, "id") || has_arg(args, "ref") || has_arg(args, "name")
                || has_arg(args, "automation_id") || has_arg(args, "target_text")
            {
                return false;
            }
            // headless no soportado
            if let Some(b) = args.get("behavior").and_then(Value::as_str) {
                if b == "headless" { return false; }
                if !["auto", "foreground", "humanized"].contains(&b) { return false; }
            }
            true
        }
        "computer_human_click" => {
            if arg_f64(args, "x").is_none() || arg_f64(args, "y").is_none() { return false; }
            if has_arg(args, "id") { return false; }
            true
        }
        "computer_drag" => {
            if arg_f64(args, "fromX").is_none() || arg_f64(args, "fromY").is_none()
                || arg_f64(args, "toX").is_none() || arg_f64(args, "toY").is_none()
            {
                return false;
            }
            if let Some(b) = args.get("behavior").and_then(Value::as_str) {
                if b == "headless" { return false; }
                if !["auto", "foreground", "humanized"].contains(&b) { return false; }
            }
            true
        }
        "computer_scroll" => {
            if let Some(b) = args.get("behavior").and_then(Value::as_str) {
                if b == "headless" { return false; }
                if !["auto", "foreground", "humanized"].contains(&b) { return false; }
            }
            true
        }
        "computer_type" => {
            let text_ok = args.get("text").map(|v| !cu_core::js::js_string_of(v).is_empty()).unwrap_or(false)
                || args.get("secret_b64").and_then(Value::as_str).map(|s| !s.is_empty()).unwrap_or(false);
            if !text_ok { return false; }
            if has_arg(args, "id") || has_arg(args, "ref") || has_arg(args, "name")
                || has_arg(args, "automation_id")
            {
                return false;
            }
            if let Some(b) = args.get("behavior").and_then(Value::as_str) {
                if b == "headless" { return false; }
                if !["auto", "foreground", "humanized"].contains(&b) { return false; }
            }
            true
        }
        "computer_fill_form" | "computer_scroll_to_text" => false,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// RNG simple (no depende de crate externo).
// ---------------------------------------------------------------------------
struct SimpleRng { state: u64 }
impl SimpleRng {
    fn new(seed: u64) -> Self { Self { state: if seed == 0 { 1 } else { seed } } }
    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1);
        self.state
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo { return lo; }
        lo + (self.next() % (hi - lo + 1) as u64) as i32
    }
}

fn button_flags(button: &str) -> (MOUSE_EVENT_FLAGS, MOUSE_EVENT_FLAGS) {
    if button == "right" { (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP) }
    else if button == "middle" { (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP) }
    else { (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP) }
}

fn send_unicode_char(ch: char, keyup: bool) {
    unsafe {
        let mut flags = KEYEVENTF_UNICODE;
        if keyup { flags = flags | KEYEVENTF_KEYUP; }
        let inp = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: ch as u16,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let _ = SendInput(&[inp], std::mem::size_of::<INPUT>() as i32);
    }
}

fn native_human_click(x: i32, y: i32, button: &str, double: bool) -> Value {
    let mut pt = POINT::default();
    unsafe { let _ = GetCursorPos(&mut pt); }
    let start_x = pt.x;
    let start_y = pt.y;
    let dist = ((x - start_x) as f64).hypot((y - start_y) as f64);
    let steps = (dist / 35.0).max(10.0).min(35.0) as i32;
    let steps = steps.max(10);
    let mut rng = SimpleRng::new((start_x as u64).wrapping_mul(31).wrapping_add(start_y as u64));
    let dev = (dist * 0.18).min(100.0) as i32;
    let dev = dev.max(0);
    let ctrl1x = start_x + ((x - start_x) as f64 * 0.3) as i32 + rng.range(-dev, dev);
    let ctrl1y = start_y + ((y - start_y) as f64 * 0.3) as i32 + rng.range(-dev, dev);
    let ctrl2x = start_x + ((x - start_x) as f64 * 0.7) as i32 + rng.range(-dev, dev);
    let ctrl2y = start_y + ((y - start_y) as f64 * 0.7) as i32 + rng.range(-dev, dev);

    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let u = 1.0 - t;
        let tt = t * t;
        let uu = u * u;
        let uuu = uu * u;
        let ttt = tt * t;
        let bx = (uuu * start_x as f64 + 3.0 * uu * t * ctrl1x as f64
            + 3.0 * u * tt * ctrl2x as f64 + ttt * x as f64) as i32;
        let by = (uuu * start_y as f64 + 3.0 * uu * t * ctrl1y as f64
            + 3.0 * u * tt * ctrl2y as f64 + ttt * y as f64) as i32;
        unsafe { let _ = SetCursorPos(bx, by); }
        thread::sleep(Duration::from_millis(rng.range(5, 14) as u64));
    }
    unsafe { let _ = SetCursorPos(x, y); }
    thread::sleep(Duration::from_millis(rng.range(30, 60) as u64));

    let (down, up) = button_flags(button);
    unsafe {
        mouse_event(down, 0, 0, 0, 0);
    }
    thread::sleep(Duration::from_millis(rng.range(65, 120) as u64));
    unsafe {
        mouse_event(up, 0, 0, 0, 0);
    }
    if double {
        thread::sleep(Duration::from_millis(rng.range(80, 130) as u64));
        unsafe { mouse_event(down, 0, 0, 0, 0); }
        thread::sleep(Duration::from_millis(rng.range(50, 95) as u64));
        unsafe { mouse_event(up, 0, 0, 0, 0); }
    }
    json!({"ok": true, "x": x, "y": y})
}

fn native_human_drag(args: &Value) {
    let from_x = int_arg(args.get("fromX"), -100000, 100000, 0) as i32;
    let from_y = int_arg(args.get("fromY"), -100000, 100000, 0) as i32;
    let to_x = int_arg(args.get("toX"), -100000, 100000, 0) as i32;
    let to_y = int_arg(args.get("toY"), -100000, 100000, 0) as i32;
    let button = args.get("button").map(cu_core::js::js_string_of).unwrap_or_else(|| "left".to_string()).to_lowercase();
    let button = if button.is_empty() { "left".to_string() } else { button };
    let (down, up) = button_flags(&button);
    let mut rng = SimpleRng::new((from_x as u64).wrapping_add(from_y as u64));
    unsafe {
        let _ = SetCursorPos(from_x, from_y);
    }
    thread::sleep(Duration::from_millis(rng.range(40, 90) as u64));
    unsafe { mouse_event(down, 0, 0, 0, 0); }
    thread::sleep(Duration::from_millis(rng.range(50, 110) as u64));
    let dist = ((to_x - from_x) as f64).hypot((to_y - from_y) as f64);
    let steps = (dist / 22.0).max(10.0).min(28.0) as i32;
    let steps = steps.max(10);
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let smooth = t * t * (3.0 - 2.0 * t);
        let jx = if i < steps { rng.range(-3, 3) } else { 0 };
        let jy = if i < steps { rng.range(-3, 3) } else { 0 };
        let cur_x = (from_x as f64 + (to_x - from_x) as f64 * smooth) as i32 + jx;
        let cur_y = (from_y as f64 + (to_y - from_y) as f64 * smooth) as i32 + jy;
        unsafe { let _ = SetCursorPos(cur_x, cur_y); }
        thread::sleep(Duration::from_millis(rng.range(8, 22) as u64));
    }
    unsafe { let _ = SetCursorPos(to_x, to_y); }
    thread::sleep(Duration::from_millis(rng.range(50, 110) as u64));
    unsafe { mouse_event(up, 0, 0, 0, 0); }
}

fn native_human_scroll(args: &Value) {
    let dy = int_arg(args.get("dy"), -100000, 100000, 0);
    let dx = int_arg(args.get("dx"), -100000, 100000, 0);
    let x = int_arg(args.get("x"), -100000, 100000, 0) as i32;
    let y = int_arg(args.get("y"), -100000, 100000, 0) as i32;
    let total = dy.abs().max(dx.abs()).max(1);
    let steps = (total / 120).max(1).min(8) as usize;
    let mut rest_y = dy;
    let mut rest_x = dx;
    for i in (1..=steps).rev() {
        let step_y = rest_y / i as i64;
        let step_x = rest_x / i as i64;
        rest_y -= step_y;
        rest_x -= step_x;
        if step_y != 0 || step_x != 0 {
            unsafe {
                let _ = SetCursorPos(x, y);
                mouse_event(MOUSEEVENTF_WHEEL, 0, 0, step_y as i32, 0);
                if step_x != 0 {
                    mouse_event(MOUSEEVENTF_HWHEEL, 0, 0, step_x as i32, 0);
                }
            }
        }
        if i > 1 {
            thread::sleep(Duration::from_millis(35 + (SimpleRng::new(i as u64).next() % 45)));
        }
    }
}

fn native_human_type(text: &str, is_secret: bool) {
    let _ = is_secret;
    let mut rng = SimpleRng::new(text.len() as u64 + 1);
    for ch in text.chars() {
        let escaped = ch.to_string();
        // Send unicode char.
        send_unicode_char(ch, false);
        thread::sleep(Duration::from_millis(10));
        send_unicode_char(ch, true);
        let mut delay = rng.range(45, 110) as u64;
        if rng.range(0, 9) == 0 {
            delay += rng.range(80, 180) as u64;
        }
        thread::sleep(Duration::from_millis(delay));
        let _ = escaped;
    }
}

fn handle_click(args: &Value, is_double: bool) -> ToolOut {
    if let Some(tw) = args.get("target_window") {
        if !tw.is_null() { unsafe { focus_window(tw) }; }
    }
    let x = int_arg(args.get("x"), -100000, 100000, 0) as i32;
    let y = int_arg(args.get("y"), -100000, 100000, 0) as i32;
    // si no vino x/y explícito (acepts ya lo garantiza), error.
    if args.get("x").is_none() || args.get("y").is_none() {
        return ToolOut::Json(json!({"ok": false, "error": "x,y or id or ref is required"}), true);
    }
    let button = args.get("button").map(cu_core::js::js_string_of).unwrap_or_else(|| "left".to_string()).to_lowercase();
    let button = if button.is_empty() { "left".to_string() } else { button };
    let humanize = args.get("human_like").map(|v| cu_core::js::js_truthy(v)).unwrap_or(false)
        || args.get("behavior").and_then(Value::as_str) == Some("humanized");

    if humanize {
        let res = native_human_click(x, y, &button, is_double);
        let epoch = cu_core::epoch::bump();
        let action = if is_double { "double_click" } else { "click" };
        return ToolOut::Json(json!({"ok": true, "action": action, "x": x, "y": y, "behavior": "humanized", "epoch": epoch, "human_click": res}), false);
    }

    unsafe {
        let _ = SetCursorPos(x, y);
        let (down, up) = button_flags(&button);
        if is_double {
            mouse_event(down, 0, 0, 0, 0);
            thread::sleep(Duration::from_millis(20));
            mouse_event(up, 0, 0, 0, 0);
            thread::sleep(Duration::from_millis(40));
            mouse_event(down, 0, 0, 0, 0);
            thread::sleep(Duration::from_millis(20));
            mouse_event(up, 0, 0, 0, 0);
        } else {
            mouse_event(down, 0, 0, 0, 0);
            thread::sleep(Duration::from_millis(20));
            mouse_event(up, 0, 0, 0, 0);
        }
    }
    let epoch = cu_core::epoch::bump();
    let action = if is_double { "double_click" } else { "click" };
    let payload = json!({"ok": true, "action": action, "x": x, "y": y, "behavior": "foreground", "epoch": epoch});
    ToolOut::Json(payload, false)
}

fn handle_human_click(args: &Value) -> ToolOut {
    if let Some(tw) = args.get("target_window") {
        if !tw.is_null() { unsafe { focus_window(tw) }; }
    }
    let x = int_arg(args.get("x"), -100000, 100000, 0) as i32;
    let y = int_arg(args.get("y"), -100000, 100000, 0) as i32;
    if args.get("x").is_none() || args.get("y").is_none() {
        return ToolOut::Json(json!({"ok": false, "error": "x,y or id is required"}), true);
    }
    let button = args.get("button").map(cu_core::js::js_string_of).unwrap_or_else(|| "left".to_string());
    let button = if button.is_empty() { "left".to_string() } else { button };
    let double = args.get("double_click").map(|v| cu_core::js::js_truthy(v)).unwrap_or(false);
    let res = native_human_click(x, y, &button, double);
    let epoch = cu_core::epoch::bump();
    ToolOut::Json(json!({"ok": true, "human_click": res, "epoch": epoch}), false)
}

fn handle_drag(args: &Value) -> ToolOut {
    if let Some(tw) = args.get("target_window") {
        if !tw.is_null() { unsafe { focus_window(tw) }; }
    }
    let from_x = int_arg(args.get("fromX"), -100000, 100000, 0) as i32;
    let from_y = int_arg(args.get("fromY"), -100000, 100000, 0) as i32;
    let to_x = int_arg(args.get("toX"), -100000, 100000, 0) as i32;
    let to_y = int_arg(args.get("toY"), -100000, 100000, 0) as i32;
    let button = args.get("button").map(cu_core::js::js_string_of).unwrap_or_else(|| "left".to_string()).to_lowercase();
    let button = if button.is_empty() { "left".to_string() } else { button };
    let humanize = args.get("human_like").map(|v| cu_core::js::js_truthy(v)).unwrap_or(false)
        || args.get("behavior").and_then(Value::as_str) == Some("humanized");

    if humanize {
        native_human_drag(args);
    } else {
        // nativeMouseAction drag (8 steps smooth)
        let (down, up) = button_flags(&button);
        unsafe {
            let _ = SetCursorPos(from_x, from_y);
            mouse_event(down, 0, 0, 0, 0);
            thread::sleep(Duration::from_millis(60));
            let steps = 8i32;
            for i in 1..=steps {
                let t = i as f64 / steps as f64;
                let smooth = t * t * (3.0 - 2.0 * t);
                let cur_x = (from_x as f64 + (to_x - from_x) as f64 * smooth) as i32;
                let cur_y = (from_y as f64 + (to_y - from_y) as f64 * smooth) as i32;
                let _ = SetCursorPos(cur_x, cur_y);
                thread::sleep(Duration::from_millis(15));
            }
            let _ = SetCursorPos(to_x, to_y);
            thread::sleep(Duration::from_millis(40));
            mouse_event(up, 0, 0, 0, 0);
        }
    }
    let epoch = cu_core::epoch::bump();
    let behavior = if humanize { "humanized" } else { "foreground" };
    ToolOut::Json(json!({"ok": true, "action": "drag", "behavior": behavior, "epoch": epoch}), false)
}

fn handle_scroll(args: &Value) -> ToolOut {
    if let Some(tw) = args.get("target_window") {
        if !tw.is_null() { unsafe { focus_window(tw) }; }
    }
    let x = int_arg(args.get("x"), -100000, 100000, 0) as i32;
    let y = int_arg(args.get("y"), -100000, 100000, 0) as i32;
    let dy = int_arg(args.get("dy"), -100000, 100000, 0);
    let dx = int_arg(args.get("dx"), -100000, 100000, 0);
    let humanize = args.get("human_like").map(|v| cu_core::js::js_truthy(v)).unwrap_or(false)
        || args.get("behavior").and_then(Value::as_str) == Some("humanized");

    if humanize {
        native_human_scroll(args);
    } else {
        unsafe {
            if x != 0 || y != 0 { let _ = SetCursorPos(x, y); }
            mouse_event(MOUSEEVENTF_WHEEL, 0, 0, dy as i32, 0);
            if dx != 0 {
                mouse_event(MOUSEEVENTF_HWHEEL, 0, 0, dx as i32, 0);
            }
        }
    }
    let epoch = cu_core::epoch::bump();
    let behavior = if humanize { "humanized" } else { "foreground" };
    ToolOut::Json(json!({"ok": true, "action": "scroll", "behavior": behavior, "epoch": epoch}), false)
}

fn handle_type(args: &Value) -> ToolOut {
    if let Some(tw) = args.get("target_window") {
        if !tw.is_null() { unsafe { focus_window(tw) }; }
    }
    let mut plain_text = args.get("text").map(cu_core::js::js_string_of).unwrap_or_default();
    let is_secret = args.get("is_secret").map(|v| cu_core::js::js_truthy(v)).unwrap_or(false)
        || args.get("secret_b64").is_some();
    if let Some(b64) = args.get("secret_b64").and_then(Value::as_str) {
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
            plain_text = String::from_utf8_lossy(&bytes).to_string();
        }
    }
    let clear_first = args.get("clear_first").map(|v| cu_core::js::js_truthy(v)).unwrap_or(false);
    if clear_first {
        native_key_action("a", "tap", &["ctrl".to_string()]);
        native_key_action("Backspace", "tap", &[]);
    }
    let jitter = args.get("human_jitter").map(|v| cu_core::js::js_truthy(v)).unwrap_or(false)
        || args.get("human_like").map(|v| cu_core::js::js_truthy(v)).unwrap_or(false)
        || args.get("behavior").and_then(Value::as_str) == Some("humanized");

    if jitter {
        native_human_type(&plain_text, is_secret);
    } else {
        for ch in plain_text.chars() {
            send_unicode_char(ch, false);
            thread::sleep(Duration::from_millis(2));
            send_unicode_char(ch, true);
        }
    }
    let press_enter = args.get("press_enter").map(|v| cu_core::js::js_truthy(v)).unwrap_or(false);
    if press_enter {
        native_key_action("Enter", "tap", &[]);
    }
    let epoch = cu_core::epoch::bump();
    let behavior = if jitter { "humanized" } else { "foreground" };
    ToolOut::Json(json!({"ok": true, "action": "type", "secret": is_secret, "behavior": behavior, "human_jitter": jitter, "epoch": epoch}), false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_hotkey_basic() {
        let (key, mods) = parse_hotkey("Ctrl+Shift+C");
        assert_eq!(key, "c");
        assert_eq!(mods, vec!["ctrl".to_string(), "shift".to_string()]);
    }

    #[test]
    fn parse_hotkey_win() {
        let (key, mods) = parse_hotkey("win+r");
        assert_eq!(key, "r");
        assert_eq!(mods, vec!["meta".to_string()]);
    }

    #[test]
    fn int_clamp_works() {
        assert_eq!(int_arg(Some(&json!(5.9)), 0, 10, 0), 5);
        assert_eq!(int_arg(Some(&json!("abc")), 0, 10, 3), 3);
        assert_eq!(int_arg(Some(&json!(100)), 0, 10, 0), 10);
        assert_eq!(int_arg(Some(&json!(-5)), 0, 10, 0), 0);
    }

    #[test]
    fn get_vk_letters() {
        assert_eq!(get_vk("a"), 0x41);
        assert_eq!(get_vk("Z"), 0x5A);
        assert_eq!(get_vk("Enter"), 0x0D);
        assert_eq!(get_vk("F5"), 0x74);
    }

    #[test]
    fn accepts_subsets() {
        use serde_json::json;
        assert!(accepts("computer_click", &json!({"x": 10, "y": 20})));
        assert!(!accepts("computer_click", &json!({"x": 10, "y": 20, "id": 5})));
        assert!(!accepts("computer_click", &json!({"x": 10, "y": 20, "behavior": "headless"})));
        assert!(accepts("computer_type", &json!({"text": "hi"})));
        assert!(!accepts("computer_type", &json!({"text": "hi", "id": 3})));
        assert!(!accepts("computer_bulk_input", &json!({"rows": [[1]], "target_text": "x"})));
        assert!(accepts("computer_bulk_input", &json!({"rows": [[1]], "target_x": 5, "target_y": 6, "target_text": "x"})));
        assert!(!accepts("computer_fill_form", &json!({})));
        assert!(!accepts("computer_scroll_to_text", &json!({"text":"x"})));
    }
}

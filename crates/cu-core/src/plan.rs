// cu-core::plan — `planLock` del origen (src/handlers/lock.js:288).
//
// Decide el scope del lock multi-agente para cada tool:
//   - readonly / no-lock → sin lock
//   - background con hwnd → "hwnd:<n>", con proceso → "app:<proc>", si no → "desktop"
//   - foreground → "desktop"
//   - tool desconocida → "desktop" (fallback conservador: se asume input real)
use serde_json::Value;

pub struct LockPlan {
    pub scope: String,
    pub tool: String,
}

const NO_LOCK_TOOLS: [&str; 4] = [
    "computer_lock",
    "computer_incidents",
    "computer_macro_record",
    "computer_macro_delete",
];

const READONLY_TOOLS: [&str; 21] = [
    "computer_screenshot",
    "computer_zoom",
    "computer_screen_info",
    "computer_map_ui",
    "computer_snapshot",
    "computer_state_fingerprint",
    "computer_wait_stable",
    "computer_verify",
    "computer_macro_list",
    "computer_process_tree",
    "computer_ocr",
    "computer_list_windows",
    "computer_find_element",
    "computer_inspect_ui",
    "computer_active_window",
    "computer_wait_for",
    "computer_wait_download",
    "computer_find_template",
    "computer_read_selection",
    "computer_check_health",
    "computer_calibrate_dpi",
];

const BACKGROUND_TOOLS: [&str; 7] = [
    "computer_background_action",
    "computer_ui_action",
    "computer_window_action",
    "computer_toast_action",
    "computer_system_tray",
    "computer_monitor_action",
    "computer_audio_action",
];

const FOREGROUND_TOOLS: [&str; 27] = [
    "computer_click",
    "computer_double_click",
    "computer_click_text",
    "computer_human_click",
    "computer_type",
    "computer_key",
    "computer_hotkey",
    "computer_scroll",
    "computer_scroll_to_text",
    "computer_move",
    "computer_drag",
    "computer_drag_scroll",
    "computer_copy",
    "computer_paste",
    "computer_clipboard",
    "computer_fill_form",
    "computer_bulk_input",
    "computer_focus_window",
    "computer_handle_file_dialog",
    "computer_upload_file",
    "computer_login",
    "computer_pass_barrier",
    "computer_context_menu",
    "computer_alert_user",
    "computer_macro_replay",
    "shell_exec",
    "shell_background",
];

/// hwnd de los args (hwnd | window | handle), como string.
fn hwnd_from_args(args: &Value) -> Option<String> {
    for key in ["hwnd", "window", "handle"] {
        if let Some(n) = args.get(key).and_then(Value::as_i64) {
            if n > 0 {
                return Some(n.to_string());
            }
        }
        if let Some(n) = args.get(key).and_then(Value::as_f64) {
            if n.is_finite() && n > 0.0 {
                return Some((n.trunc() as i64).to_string());
            }
        }
    }
    None
}

/// nombre de proceso/app de los args (process | process_name | exe | app), en minúsculas.
fn process_from_args(args: &Value) -> Option<String> {
    for key in ["process", "process_name", "exe", "app"] {
        if let Some(v) = args.get(key) {
            let value = match v {
                Value::String(s) => s.trim().to_string(),
                other => crate::js::js_string_of(other).trim().to_string(),
            };
            if !value.is_empty() {
                return Some(value.to_lowercase());
            }
        }
    }
    None
}

/// planLock(name, args) -> Option<LockPlan>. None = no lockear.
pub fn plan_lock(name: &str, args: &Value) -> Option<LockPlan> {
    if NO_LOCK_TOOLS.contains(&name) || READONLY_TOOLS.contains(&name) {
        return None;
    }
    let desktop = || Some(LockPlan {
        scope: "desktop".to_string(),
        tool: name.to_string(),
    });
    if name == "computer_detect_block" {
        return if args.get("solve") == Some(&Value::Bool(true)) {
            desktop()
        } else {
            None
        };
    }
    if name == "computer_batch" {
        let empty = serde_json::Value::Array(Vec::new());
        let steps = args
            .get("actions")
            .filter(|v| v.is_array())
            .or_else(|| args.get("steps").filter(|v| v.is_array()))
            .unwrap_or(&empty);
        let steps = steps.as_array().cloned().unwrap_or_default();
        if steps.is_empty() {
            return desktop();
        }
        let mut first_hwnd: Option<String> = None;
        for step in &steps {
            let Some(hwnd) = hwnd_from_args(step) else {
                return desktop();
            };
            if first_hwnd.is_none() {
                first_hwnd = Some(hwnd);
            }
        }
        return Some(LockPlan {
            scope: format!("hwnd:{}", first_hwnd.unwrap_or_default()),
            tool: name.to_string(),
        });
    }
    if BACKGROUND_TOOLS.contains(&name) {
        if let Some(hwnd) = hwnd_from_args(args) {
            return Some(LockPlan {
                scope: format!("hwnd:{hwnd}"),
                tool: name.to_string(),
            });
        }
        if let Some(proc) = process_from_args(args) {
            return Some(LockPlan {
                scope: format!("app:{proc}"),
                tool: name.to_string(),
            });
        }
        return desktop();
    }
    if FOREGROUND_TOOLS.contains(&name) {
        return desktop();
    }
    // Tool desconocida o nueva: se asume input real.
    desktop()
}

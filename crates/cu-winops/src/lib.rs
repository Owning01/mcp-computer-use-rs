// cu-winops — operaciones de ventana y monitor (focus, window actions, wait_for, monitor, wait, login).
// Contrato del puente: handles(name) + accepts(name,args) + handle_call(name, args) -> cu_core::out::ToolOut.
mod computer_login;
mod computer_wait;
pub mod focus;
pub mod helpers;
pub mod monitor;
pub mod wait_for;
mod window_action;

use cu_core::out::ToolOut;
use serde_json::{json, Value};

pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "computer_focus_window"
            | "computer_window_action"
            | "computer_wait_for"
            | "computer_monitor_action"
            | "computer_wait"
            | "computer_login"
    )
}

fn flag_truthy(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Some(Value::String(s)) => {
            let l = s.trim().to_lowercase();
            l == "true" || l == "1" || l == "yes"
        }
        Some(Value::Null) | None => false,
        Some(_) => true,
    }
}

pub fn accepts(name: &str, args: &Value) -> bool {
    match name {
        "computer_focus_window" | "computer_window_action" | "computer_wait_for"
        | "computer_monitor_action" | "computer_wait" => true,
        "computer_login" => {
            if flag_truthy(args.get("list_profiles")) {
                return false; // puente nativo
            }
            let account = args.get("account").and_then(|v| v.as_str()).unwrap_or("").trim();
            if !account.is_empty() {
                return false; // CDP no replicado
            }
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("").trim();
            if url.is_empty() {
                return false; // origin lanza error; proxiar
            }
            true
        }
        _ => false,
    }
}

pub fn handle_call(name: &str, args: &Value) -> ToolOut {
    match name {
        "computer_focus_window" => {
            let v = focus::focus_window(args);
            ToolOut::Json(v, false)
        }
        "computer_window_action" => {
            let v = window_action::window_action(args);
            ToolOut::Json(v, false)
        }
        "computer_wait_for" => wait_for::wait_for(args),
        "computer_monitor_action" => {
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("list").trim();
            if action == "screenshot_monitor" {
                monitor::monitor_screenshot(args)
            } else {
                let v = monitor::monitor_action(args);
                ToolOut::Json(v, false)
            }
        }
        "computer_wait" => computer_wait::computer_wait(args),
        "computer_login" => computer_login::login(args),
        _ => ToolOut::Json(json!({"ok": false, "error": "not_implemented"}), true),
    }
}
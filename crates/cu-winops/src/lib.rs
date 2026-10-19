// cu-winops — operaciones de ventana y monitor (focus, window actions, wait_for, monitor).
// Contrato del puente: handles(name) + handle_call(name, args) -> cu_core::out::ToolOut.
pub mod focus;
pub mod helpers;
pub mod monitor;
pub mod wait_for;
pub mod window_action;

use cu_core::out::ToolOut;
use serde_json::{json, Value};

pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "computer_focus_window"
            | "computer_window_action"
            | "computer_wait_for"
            | "computer_monitor_action"
    )
}

// accepts: subconjunto de args servible nativo (default true; el puente consulta esto).
pub fn accepts(_name: &str, _args: &Value) -> bool {
    true
}

pub fn handle_call(name: &str, args: &Value) -> ToolOut {
    match name {
        "computer_focus_window" => {
            let v = focus::focus_window(args);
            ToolOut::Json(v, false)
        }
        "computer_window_action" => {
            // elicitation de confirm para close no implementada (PORT NOTE): se procede
            // como cliente sin elicitation, replicando el ramo elicited === null del origen.
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("").trim();
            if action == "close" && args.get("confirm").and_then(|v| v.as_bool()) != Some(true) {
                // No tenemos acceso al cliente para elicit; procedemos (comportamiento clásico).
            }
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
        _ => ToolOut::Json(json!({"ok": false, "error": "not_implemented"}), true),
    }
}
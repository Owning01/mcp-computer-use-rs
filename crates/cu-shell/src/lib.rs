// cu-shell �?" HUD/notificaciones del shell (indicator, toast, tray, context menu). Reglas: migration/RULEBOOK.md.
// Contrato del puente: handles(name) + handle_call(name, args) -> cu_core::out::ToolOut.
mod indicator;

use cu_core::out::ToolOut;
use serde_json::{json, Value};

pub fn handles(name: &str) -> bool {
    matches!(name, "computer_indicator")
}

pub fn accepts(_name: &str, _args: &Value) -> bool {
    true
}

pub fn handle_call(name: &str, args: &Value) -> ToolOut {
    match name {
        "computer_indicator" => {
            let (v, is_err) = indicator::handle_tool(args);
            ToolOut::Json(v, is_err)
        }
        _ => ToolOut::Json(json!({"ok": false, "error": "not_implemented"}), true),
    }
}

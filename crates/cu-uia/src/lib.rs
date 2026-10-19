// cu-uia — capa UIA nativa (F6). Reglas: migration/RULEBOOK.md.
// Contrato del puente: handles(name) + handle_call(name, args) -> cu_core::out::ToolOut.
// Stub: sin tools nativos todavía (lo completa la fase F6).
use cu_core::out::ToolOut;
use serde_json::{json, Value};

pub fn handles(_name: &str) -> bool {
    false
}

pub fn handle_call(_name: &str, _args: &Value) -> ToolOut {
    ToolOut::Json(json!({"ok": false, "error": "not_implemented"}), true)
}

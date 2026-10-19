// cu-ocr — computer_ocr nativo con la cascada del origen: PaddleOCR (worker Python
// persistente, supervisor Rust) -> fallback Windows.Media.Ocr nativo (WinRT). Ver migration/RULEBOOK.md.
// Contrato del puente: handles(name) + accepts(name, args) + handle_call(name, args) -> cu_core::out::ToolOut.
// Stub: sin implementar todavía (lo completa la fase F6; mientras tanto proxea al hijo).
use cu_core::out::ToolOut;
use serde_json::{json, Value};

pub fn handles(_name: &str) -> bool {
    false
}

pub fn accepts(_name: &str, _args: &Value) -> bool {
    true
}

pub fn handle_call(_name: &str, _args: &Value) -> ToolOut {
    ToolOut::Json(json!({"ok": false, "error": "not_implemented"}), true)
}

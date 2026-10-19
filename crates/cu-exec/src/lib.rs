// cu-exec — computer_batch y computer_macro_replay nativos (ejecución con dispatch interno).
// Reglas: migration/RULEBOOK.md.
//
// A diferencia de los otros clusters, estos tools necesitan EJECUTAR sub-llamadas: el
// puente les inyecta un Executor que rutea a los clusters nativos o al hijo Node.
// Contrato: handles(name) + accepts(name, args) + handle_call(name, args, exec) -> ToolOut.
use cu_core::out::ToolOut;
use serde_json::{json, Value};

/// Ejecutor de tools inyectado por el puente.
pub trait Executor {
    /// Ejecuta un tool y devuelve (payload JSON, isError).
    fn call(&self, tool: &str, args: &Value) -> Result<(Value, bool), String>;
    /// Elementos UIA de una ventana (para el healing de macros); [] si no hay soporte.
    fn collect_elements(&self, _target: &Value) -> Vec<Value> {
        Vec::new()
    }
    /// Ventana activa {hwnd, title,...} o None.
    fn active_window(&self) -> Option<Value> {
        None
    }
}

pub fn handles(name: &str) -> bool {
    matches!(name, "computer_batch" | "computer_macro_replay")
}

pub fn accepts(_name: &str, _args: &Value) -> bool {
    true
}

pub fn handle_call(_name: &str, _args: &Value, _exec: &dyn Executor) -> ToolOut {
    ToolOut::Json(json!({"ok": false, "error": "not_implemented"}), true)
}

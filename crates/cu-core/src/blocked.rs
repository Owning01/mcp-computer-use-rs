// cu-core::blocked — bloqueo de interacción con gestores de contraseñas.
// Puerto de src/utils/error_recovery.js (BLOCKED_APP_FRAGMENTS + formatErrorResponse).
use serde_json::{json, Value};

pub const BLOCKED_APP_FRAGMENTS: [&str; 7] = [
    "1password",
    "bitwarden",
    "dashlane",
    "lastpass",
    "nordpass",
    "proton pass",
    "keepass",
];

/// true si el target (título de ventana o proceso) coincide con un gestor bloqueado.
pub fn is_app_blocked(target: &str) -> bool {
    if target.is_empty() {
        return false;
    }
    let lower = target.to_lowercase();
    BLOCKED_APP_FRAGMENTS
        .iter()
        .any(|fragment| lower.contains(fragment))
}

/// Payload de error del origen cuando la app está bloqueada.
pub fn app_blocked_payload(target: &str) -> Value {
    json!({
        "ok": false,
        "error": "app_blocked",
        "message": format!(
            "Interacción con el gestor de contraseñas \"{target}\" bloqueada por seguridad."
        )
    })
}

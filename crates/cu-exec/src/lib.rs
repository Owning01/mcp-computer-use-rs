// cu-exec — computer_batch y computer_macro_replay nativos (ejecución con dispatch interno).
// Reglas: migration/RULEBOOK.md.
//
// A diferencia de los otros clusters, estos tools necesitan EJECUTAR sub-llamadas: el
// puente les inyecta un Executor que rutea a los clusters nativos o al hijo Node.
// Contrato: handles(name) + accepts(name, args) + handle_call(name, args, exec) -> ToolOut.
mod batch;
mod replay;
#[cfg(test)]
mod testkit;

use cu_core::js::js_truthy;
use cu_core::out::ToolOut;
use serde_json::{Value, json};
use std::time::Duration;

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

pub fn handle_call(name: &str, args: &Value, exec: &dyn Executor) -> ToolOut {
    match name {
        "computer_batch" => batch::run(args, exec),
        "computer_macro_replay" => replay::run(args, exec),
        _ => ToolOut::Json(
            json!({"ok": false, "error": "not_handled", "name": name}),
            true,
        ),
    }
}

// Valor truthy de una clave ausente = false (como `args.x` en JS).
fn flag(args: &Value, key: &str) -> bool {
    args.get(key).map(js_truthy).unwrap_or(false)
}

// `args.x !== false`: solo el booleano false desactiva el flag; ausente = true.
fn not_false(args: &Value, key: &str) -> bool {
    !matches!(args.get(key), Some(Value::Bool(false)))
}

// Número JSON: enteros salen sin ".0" (JS imprime `1`, no `1.0`).
fn num_value(f: f64) -> Value {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.0e15 {
        json!(f as i64)
    } else {
        json!(f)
    }
}

// Math.round de JS: el empate va hacia +infinito (-1.5 -> -1).
fn round_js(f: f64) -> f64 {
    (f + 0.5).floor()
}

// Espera local (pausa entre acciones de input); no pasa por el Executor.
fn pause(ms: u64) {
    if ms > 0 {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

// ISO-8601 UTC con milisegundos (= Date.prototype.toISOString).
// PORT NOTE: cu_core::incidents::iso_now es pub(crate); copia local hasta exportarla desde cu-core.
fn iso_now() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_days(days);
    let h = rem / 3_600_000;
    let mi = rem / 60_000 % 60;
    let s = rem / 1000 % 60;
    let milli = rem % 1000;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{milli:03}Z")
}

// Días desde 1970-01-01 -> (año, mes, día). Algoritmo de Howard Hinnant.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn num_value_keeps_integers_without_fraction() {
        assert_eq!(num_value(1.0).to_string(), "1");
        assert_eq!(num_value(0.833).to_string(), "0.833");
    }

    #[test]
    fn round_js_breaks_ties_upward() {
        assert_eq!(round_js(-1.5), -1.0);
        assert_eq!(round_js(2.5), 3.0);
    }

    #[test]
    fn iso_now_has_js_shape() {
        let s = iso_now();
        assert_eq!(s.len(), 24);
        assert!(s.ends_with('Z'));
        assert_eq!(&s[10..11], "T");
    }
}

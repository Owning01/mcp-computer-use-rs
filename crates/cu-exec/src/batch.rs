// computer_batch — port de src/index.js (handler "computer_batch" + nativeBatchAction).
// PORT NOTES:
// - (a) La ruta del daemon `/shell/computer/batch` NO se porta: siempre se usa el fallback
//   nativo, igual que el origen cuando la API no responde. Cada acción se traduce a su sub-tool
//   nativa y se llama vía Executor::call. La respuesta usa el shape del fallback nativo
//   ({ok, native, executed, epoch, screenshot}).
// - (b) El appendRunLog/appendError del origen no se replica: la bitácora de corrida vive en
//   el harness, no en el puente.
// - sleep/delay y delay_ms entre acciones corren localmente (thread::sleep), como el fallback.
// - Corte en el primer error de sub-tool (el origen no tenía flag stop_on_failure propio).
// - Tipo desconocido y hotkey sin combo cuentan como ejecutados sin hacer nada, como el
//   PowerShell del fallback nativo (no son error).
// - El epoch se incrementa una sola vez, antes de la captura. Lo gestiona el puente.
use crate::{Executor, flag, pause};
use cu_core::js::{js_num_to_f64, js_string_of, js_truthy};
use cu_core::out::ToolOut;
use serde_json::{Map, Value, json};

const SLEEP_DEFAULT_MS: i64 = 100;

// Paso ya traducido: sub-tool nativa, pausa local, o nada (cuenta como ejecutado).
enum Step {
    Tool { name: &'static str, args: Value },
    Sleep(i64),
    Noop,
}

pub fn run(args: &Value, exec: &dyn Executor) -> ToolOut {
    if let Some(expected) = args.get("expected_epoch") {
        let current = cu_core::epoch::current();
        let expected_n = if expected.is_null() {
            Some(0.0)
        } else {
            js_num_to_f64(expected)
        };
        if expected_n != Some(current as f64) {
            return ToolOut::Raw(
                format!(
                    "STALE_EPOCH: expected {}, current UI epoch is {current}. Capture new screenshot.",
                    js_string_of(expected)
                ),
                true,
            );
        }
    }

    let delay_ms = match args.get("delay_ms").filter(|v| js_truthy(v)) {
        Some(v) => js_num_to_f64(v).unwrap_or(0.0),
        None => 0.0,
    };
    let actions: Vec<Value> = args
        .get("actions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut executed: u64 = 0;
    for action in &actions {
        let outcome = match plan_action(action) {
            Ok(Step::Noop) => Ok(()),
            Ok(Step::Sleep(ms)) => {
                pause(ms.max(0) as u64);
                Ok(())
            }
            Ok(Step::Tool { name, args }) => match exec.call(name, &args) {
                Ok((_, false)) => Ok(()),
                Ok((payload, true)) => Err(error_text(&payload)),
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        };
        if let Err(e) = outcome {
            return failure(&e, executed);
        }
        executed += 1;
        if delay_ms > 0.0 {
            pause(delay_ms as u64);
        }
    }

    // Único incremento del epoch: antes de la captura (el puente gestiona el contador).
    let mut screenshot = Value::Null;
    if flag(args, "screenshot") {
        cu_core::epoch::bump();
        let shot_args = match args.get("screenshot") {
            Some(v @ Value::Object(_)) => v.clone(),
            _ => json!({}),
        };
        if let Ok((payload, false)) = exec.call("computer_screenshot", &shot_args) {
            screenshot = payload;
        }
    }

    ToolOut::Json(
        json!({
            "ok": true,
            "native": true,
            "executed": executed,
            "epoch": cu_core::epoch::current(),
            "screenshot": screenshot
        }),
        false,
    )
}

// Texto de error del origen: `res?.error || "unknown error"`.
fn failure(error: &str, executed: u64) -> ToolOut {
    let shown = if error.is_empty() {
        "unknown error"
    } else {
        error
    };
    ToolOut::Raw(
        format!("Batch action failed: {shown} (executed: {executed})"),
        true,
    )
}

// Mensaje de una sub-tool con isError: `message`, luego `error`, luego vacío.
fn error_text(payload: &Value) -> String {
    match payload {
        Value::String(s) => s.clone(),
        Value::Object(_) => ["message", "error"]
            .iter()
            .filter_map(|k| payload.get(*k))
            .find(|v| js_truthy(v))
            .map(js_string_of)
            .unwrap_or_default(),
        _ => String::new(),
    }
}

// Traduce una acción del lote (normalizeAction + mapeo a sub-tool nativa).
fn plan_action(action: &Value) -> Result<Step, String> {
    let raw_type = action.get("type").map(js_string_of).unwrap_or_default();
    let kind = match raw_type.as_str() {
        "double_click" | "dblclick" | "double" => "dblclick",
        other => other,
    };
    match kind {
        "click" => tool("computer_click", pick(action, &["x", "y", "button"])),
        "dblclick" => tool("computer_double_click", pick(action, &["x", "y", "button"])),
        "move" => tool("computer_move", pick(action, &["x", "y"])),
        "drag" => tool(
            "computer_drag",
            pick(action, &["fromX", "fromY", "toX", "toY", "button"]),
        ),
        "scroll" => tool("computer_scroll", pick(action, &["x", "y", "dx", "dy"])),
        "key" => tool("computer_key", pick(action, &["key", "mods", "action"])),
        "hotkey" => match action.get("combo").filter(|c| js_truthy(c)) {
            Some(combo) => {
                let (key, mods) = parse_hotkey(&js_string_of(combo));
                tool(
                    "computer_key",
                    json!({"key": key, "mods": mods, "action": "tap"}),
                )
            }
            // Sin combo: el fallback nativo lo cuenta como ejecutado sin hacer nada.
            None => Ok(Step::Noop),
        },
        "type" => tool(
            "computer_type",
            pick(action, &["text", "secret_b64", "is_secret"]),
        ),
        "sleep" | "delay" => Ok(Step::Sleep(sleep_ms_of(action))),
        // Tipo desconocido: el fallback nativo no tiene rama; cuenta como ejecutado.
        _ => Ok(Step::Noop),
    }
}

fn tool(name: &'static str, args: Value) -> Result<Step, String> {
    Ok(Step::Tool { name, args })
}

// Copia solo las claves presentes (ausente = undefined en JS, no se envía).
fn pick(action: &Value, keys: &[&str]) -> Value {
    let mut out = Map::new();
    for key in keys {
        if let Some(v) = action.get(*key) {
            out.insert((*key).to_string(), v.clone());
        }
    }
    Value::Object(out)
}

// Espejo de parseHotkey(): minúsculas, mods ctrl/shift/alt/meta, resto = tecla.
fn parse_hotkey(combo: &str) -> (String, Vec<&'static str>) {
    let mut mods: Vec<&'static str> = Vec::new();
    let mut key = String::new();
    for part in combo.to_lowercase().split('+') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        match p {
            "ctrl" | "control" => mods.push("ctrl"),
            "shift" => mods.push("shift"),
            "alt" => mods.push("alt"),
            "win" | "meta" | "super" | "cmd" => mods.push("meta"),
            other => key = other.to_string(),
        }
    }
    (key, mods)
}

// Duración de un paso sleep/delay: `ms`, luego `delay_ms`, luego 100 (como el origen).
fn sleep_ms_of(action: &Value) -> i64 {
    let picked = ["ms", "delay_ms"]
        .iter()
        .filter_map(|k| action.get(*k))
        .find(|v| js_truthy(v));
    match picked {
        Some(v) => js_num_to_f64(v).map(|f| f.trunc() as i64).unwrap_or(0),
        None => SLEEP_DEFAULT_MS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::FakeExec;

    fn json_ok(out: ToolOut) -> Value {
        match out {
            ToolOut::Json(v, false) => v,
            _ => panic!("se esperaba Json sin error"),
        }
    }

    #[test]
    fn two_steps_ok_maps_actions_to_subtools() {
        let exec = FakeExec::default();
        let args = json!({
            "actions": [
                {"type": "click", "x": 1, "y": 2},
                {"type": "hotkey", "combo": "Ctrl+Shift+S"}
            ]
        });
        let v = json_ok(run(&args, &exec));
        assert_eq!(v["executed"], json!(2));
        assert_eq!(v["native"], json!(true));
        assert_eq!(v["screenshot"], Value::Null);
        let calls = exec.calls.borrow();
        assert_eq!(calls[0].0, "computer_click");
        assert_eq!(calls[0].1, json!({"x": 1, "y": 2}));
        assert_eq!(calls[1].0, "computer_key");
        assert_eq!(
            calls[1].1,
            json!({"key": "s", "mods": ["ctrl", "shift"], "action": "tap"})
        );
    }

    #[test]
    fn failing_step_stops_batch_and_reports_executed() {
        let exec = FakeExec::default();
        exec.script(
            "computer_key",
            vec![Ok((json!({"ok": false, "message": "boom"}), true))],
        );
        let args = json!({
            "actions": [
                {"type": "click", "x": 1, "y": 2},
                {"type": "key", "key": "enter"},
                {"type": "click", "x": 3, "y": 4}
            ]
        });
        match run(&args, &exec) {
            ToolOut::Raw(text, true) => {
                assert_eq!(text, "Batch action failed: boom (executed: 1)");
            }
            _ => panic!("se esperaba error crudo"),
        }
        assert_eq!(exec.calls.borrow().len(), 2);
    }

    #[test]
    fn stale_epoch_is_rejected_before_running() {
        let exec = FakeExec::default();
        let expected = cu_core::epoch::current() + 1000;
        let args =
            json!({"actions": [{"type": "click", "x": 1, "y": 2}], "expected_epoch": expected});
        match run(&args, &exec) {
            ToolOut::Raw(text, true) => assert!(text.starts_with("STALE_EPOCH: expected")),
            _ => panic!("se esperaba STALE_EPOCH"),
        }
        assert!(exec.calls.borrow().is_empty());
    }

    #[test]
    fn unknown_type_and_hotkey_without_combo_count_as_executed_noop() {
        let exec = FakeExec::default();
        let args = json!({
            "actions": [
                {"type": "wiggle"},
                {"type": "hotkey"},
                {"type": "click", "x": 1, "y": 2}
            ]
        });
        let v = json_ok(run(&args, &exec));
        assert_eq!(v["executed"], json!(3));
        let calls = exec.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "computer_click");
    }

    #[test]
    fn screenshot_returns_full_payload_and_bumps_epoch_once() {
        let exec = FakeExec::default();
        let full = json!({
            "width": 1920, "height": 1080, "etag": "abc",
            "format": "png", "image": "AAAA", "extra": 7
        });
        exec.script("computer_screenshot", vec![Ok((full.clone(), false))]);
        let before = cu_core::epoch::current();
        let args = json!({"actions": [], "screenshot": {"width": 800}});
        let v = json_ok(run(&args, &exec));
        assert_eq!(v["screenshot"], full);
        assert_eq!(v["epoch"], json!(before + 1));
        let calls = exec.calls.borrow();
        assert_eq!(calls[0].0, "computer_screenshot");
        assert_eq!(calls[0].1, json!({"width": 800}));
    }
}

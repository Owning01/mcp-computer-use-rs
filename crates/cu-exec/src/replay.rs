// computer_macro_replay — port de src/handlers/macros.js (replayMacro, bestMatch, applyMatch)
// y del handler "computer_macro_replay" de src/index.js.
// PORT NOTES:
// - El origen pasa dispatch/collectElements/activeWindow como callbacks; aquí son Executor.
// - MacroStore abstrae load/save para testear sin tocar el directorio real de macros.
// - normalize_text duplica la versión privada de cu-core (tabla de diacríticos Latin-1 básica).
//   Pendiente: exportarla desde cu-core para paridad exacta de healing.
// - Sin progreso (notifications/progress): el Executor no tiene canal de progreso.
// - Pausas entre acciones de input con thread::sleep (base 250 ms / speed), como el origen.
use crate::{Executor, flag, iso_now, not_false, num_value, pause, round_js};
use cu_core::js::{js_num_to_f64, js_string_of, js_truthy};
use cu_core::macros::{load_macro_pub, save_macro_pub, slugify};
use cu_core::out::ToolOut;
use serde_json::{Map, Value, json};
use std::time::Instant;

const MATCH_THRESHOLD: f64 = 0.6;
const BASE_DELAY_MS: f64 = 250.0;
const SECRET_MARKER: &str = "__secret__";
const HUMAN_CLICK: &str = "computer_human_click";

// Herramientas que inyectan input y por tanto esperan la pausa base (INPUT_TOOLS del origen).
const INPUT_TOOLS: &[&str] = &[
    "computer_click",
    "computer_double_click",
    "computer_human_click",
    "computer_move",
    "computer_drag",
    "computer_scroll",
    "computer_scroll_to_text",
    "computer_key",
    "computer_hotkey",
    "computer_type",
    "computer_click_text",
    "computer_paste",
    "computer_fill_form",
    "computer_batch",
    "computer_bulk_input",
    "computer_ui_action",
    "computer_window_action",
    "computer_clipboard",
    "computer_upload_file",
    "computer_handle_file_dialog",
    "computer_login",
    "computer_alert_user",
    "computer_monitor_action",
    "computer_system_tray",
    "computer_toast_action",
    "computer_context_menu",
    "computer_drag_scroll",
    "computer_audio_action",
    "shell_exec",
    "shell_background",
];

// Almacén de macros. La implementación real usa cu_core::macros (directorio por defecto).
pub trait MacroStore {
    fn load(&self, name: &str) -> Option<Value>;
    fn save(&self, macro_v: &Value) -> Result<(), String>;
}

struct CoreStore;

impl MacroStore for CoreStore {
    fn load(&self, name: &str) -> Option<Value> {
        load_macro_pub(name)
    }
    fn save(&self, macro_v: &Value) -> Result<(), String> {
        save_macro_pub(macro_v).map(|_| ())
    }
}

// Paso del plan: args ya con overrides por índice aplicados.
struct PlanStep {
    i: usize,
    tool: String,
    args: Value,
    element: Option<Value>,
    secret_required: bool,
}

pub fn run(args: &Value, exec: &dyn Executor) -> ToolOut {
    run_with(args, exec, &CoreStore)
}

pub fn run_with(args: &Value, exec: &dyn Executor, store: &dyn MacroStore) -> ToolOut {
    let name = args
        .get("name")
        .filter(|v| js_truthy(v))
        .map(js_string_of)
        .unwrap_or_default();
    let name = name.trim().to_string();
    if name.is_empty() {
        return ToolOut::Json(
            json!({"ok": false, "error": "execution_failed", "message": "name parameter is required"}),
            true,
        );
    }

    let started = Instant::now();
    let dry_run = flag(args, "dry_run");
    let stop_on_failure = not_false(args, "stop_on_failure");
    let heal = not_false(args, "heal");
    // Number(speed) || 1; el replay usa 1 si el valor no es > 0.
    let speed = match args.get("speed").and_then(js_num_to_f64) {
        Some(s) if s > 0.0 => s,
        _ => 1.0,
    };
    let overrides: Map<String, Value> = args
        .get("overrides")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let Some(mut macro_v) = store.load(&name) else {
        return ToolOut::Json(
            json!({
                "ok": false,
                "name": slugify(&name),
                "error": "macro_not_found",
                "executed": 0,
                "healed": 0,
                "skipped": 0,
                "failed": 0,
                "steps": [],
                "elapsed_ms": elapsed(started)
            }),
            true,
        );
    };

    let macro_name = macro_name_of(&macro_v, &name);
    let plan = build_plan(&macro_v, &overrides);

    if dry_run {
        let mut skipped = 0u64;
        let steps: Vec<Value> = plan
            .iter()
            .map(|p| {
                let mut entry = entry(p.i, &p.tool, "dry");
                entry.insert("args".to_string(), p.args.clone());
                if p.secret_required {
                    skipped += 1;
                    entry.insert("detail".to_string(), json!("secret_required"));
                }
                Value::Object(entry)
            })
            .collect();
        return ToolOut::Json(
            json!({
                "ok": true,
                "name": macro_name,
                "dry_run": true,
                "executed": 0,
                "healed": 0,
                "skipped": skipped,
                "failed": 0,
                "steps": steps,
                "elapsed_ms": elapsed(started)
            }),
            false,
        );
    }

    let mut executed: u64 = 0;
    let mut healed: u64 = 0;
    let mut skipped: u64 = 0;
    let mut failed: u64 = 0;
    let mut healed_any = false;
    let mut steps_out: Vec<Value> = Vec::new();

    for p in &plan {
        if p.tool.starts_with("computer_macro_") {
            steps_out.push(with_detail(
                entry(p.i, &p.tool, "failed"),
                "macro_tool_not_replayable",
            ));
            failed += 1;
            if stop_on_failure {
                break;
            }
            continue;
        }

        if p.secret_required {
            steps_out.push(with_detail(
                entry(p.i, &p.tool, "skipped"),
                "secret_required",
            ));
            skipped += 1;
            continue;
        }

        if INPUT_TOOLS.contains(&p.tool.as_str()) {
            pause(round_js(BASE_DELAY_MS / speed).max(0.0) as u64);
        }

        let payload = strip_internal(&p.args);
        let mut failure: Option<String> = None;
        let mut is_err = false;
        match exec.call(&p.tool, &payload) {
            Ok((_, err)) => is_err = err,
            Err(e) => failure = Some(e),
        }
        executed += 1;

        if failure.is_none() && !is_err {
            steps_out.push(Value::Object(entry(p.i, &p.tool, "ok")));
            continue;
        }

        let heal_info = if heal && p.element.is_some() {
            attempt_heal(p, &payload, exec)
        } else {
            None
        };
        if let Some((m, new_args)) = &heal_info {
            if matches!(exec.call(&p.tool, new_args), Ok((_, false))) {
                healed_any = true;
                healed += 1;
                update_stored_step(&mut macro_v, p.i, &p.tool, m);
                let detail = if m.get("ref").map(js_truthy).unwrap_or(false) {
                    "healed_ref"
                } else {
                    "healed_coords"
                };
                steps_out.push(with_detail(entry(p.i, &p.tool, "healed"), detail));
                continue;
            }
        }

        let detail = match failure {
            Some(f) if !f.is_empty() => f,
            _ if heal && p.element.is_some() => {
                if heal_info.is_none() {
                    "no_match".to_string()
                } else {
                    "heal_retry_failed".to_string()
                }
            }
            _ => "isError".to_string(),
        };
        steps_out.push(with_detail(entry(p.i, &p.tool, "failed"), &detail));
        failed += 1;
        if stop_on_failure {
            break;
        }
    }

    let ok = failed == 0;
    let elapsed_ms = elapsed(started);
    persist(
        &mut macro_v,
        store,
        ok,
        executed,
        healed,
        skipped,
        failed,
        healed_any,
    );

    ToolOut::Json(
        json!({
            "ok": ok,
            "name": macro_name,
            "executed": executed,
            "healed": healed,
            "skipped": skipped,
            "failed": failed,
            "steps": steps_out,
            "elapsed_ms": elapsed_ms
        }),
        !ok,
    )
}

fn elapsed(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

fn macro_name_of(macro_v: &Value, fallback: &str) -> String {
    macro_v
        .get("name")
        .filter(|v| js_truthy(v))
        .map(js_string_of)
        .unwrap_or_else(|| slugify(fallback))
}

fn entry(i: usize, tool: &str, status: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("i".to_string(), json!(i));
    m.insert("tool".to_string(), json!(tool));
    m.insert("status".to_string(), json!(status));
    m
}

fn with_detail(mut m: Map<String, Value>, detail: &str) -> Value {
    m.insert("detail".to_string(), json!(detail));
    Value::Object(m)
}

// Plan: args = args guardados + override del índice (merge superficial), como buildPlanStep.
fn build_plan(macro_v: &Value, overrides: &Map<String, Value>) -> Vec<PlanStep> {
    let steps = macro_v
        .get("steps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let tool = step
                .get("tool")
                .filter(|v| js_truthy(v))
                .map(js_string_of)
                .unwrap_or_default();
            let mut args = match step.get("args") {
                Some(Value::Object(o)) => o.clone(),
                _ => Map::new(),
            };
            let element = args.get("__element").filter(|v| v.is_object()).cloned();
            if let Some(Value::Object(ov)) = overrides.get(i.to_string().as_str()) {
                for (k, v) in ov {
                    args.insert(k.clone(), v.clone());
                }
            }
            let args = Value::Object(args);
            let secret_required = has_secret_markers(&args, 0);
            PlanStep {
                i,
                tool,
                args,
                element,
                secret_required,
            }
        })
        .collect()
}

// Marcador { __secret__: "<tipo>" } pendiente en cualquier profundidad (hasSecretMarkers).
fn has_secret_markers(value: &Value, depth: usize) -> bool {
    if depth > 20 {
        return false;
    }
    match value {
        Value::Array(items) => items.iter().any(|v| has_secret_markers(v, depth + 1)),
        Value::Object(map) => {
            if let Some(Value::String(s)) = map.get(SECRET_MARKER) {
                if !s.is_empty() {
                    return true;
                }
            }
            map.values().any(|v| has_secret_markers(v, depth + 1))
        }
        _ => false,
    }
}

// Copia de args sin __element (stripInternal).
fn strip_internal(args: &Value) -> Value {
    let mut out = match args {
        Value::Object(o) => o.clone(),
        _ => Map::new(),
    };
    out.remove("__element");
    Value::Object(out)
}

// Healing: snapshot UIA + bestMatch contra el __element del paso. Devuelve (match, args nuevos).
fn attempt_heal(p: &PlanStep, payload: &Value, exec: &dyn Executor) -> Option<(Value, Value)> {
    let element = p.element.as_ref()?;
    let mut target = Map::new();
    if let Some(hwnd) = payload.get("hwnd") {
        target.insert("hwnd".to_string(), hwnd.clone());
    }
    let target_window = payload
        .get("target_window")
        .filter(|v| js_truthy(v))
        .cloned()
        .unwrap_or_else(|| json!(""));
    target.insert("target_window".to_string(), target_window);

    let has_hwnd = target.get("hwnd").map(js_truthy).unwrap_or(false);
    let has_window = target.get("target_window").map(js_truthy).unwrap_or(false);
    if !has_hwnd && !has_window {
        if let Some(active) = exec.active_window() {
            if let Some(h) = active.get("hwnd").filter(|v| js_truthy(v)) {
                target.insert("hwnd".to_string(), h.clone());
            }
        }
    }

    let elements = exec.collect_elements(&Value::Object(target));
    let m = best_match(&elements, element)?;
    let new_args = apply_match(payload, &m, &p.tool);
    Some((m, new_args))
}

// Reescribe el paso guardado con el match curado y marca healed (persistencia del healing).
fn update_stored_step(macro_v: &mut Value, i: usize, tool: &str, m: &Value) {
    let Some(stored) = macro_v
        .get_mut("steps")
        .and_then(Value::as_array_mut)
        .and_then(|a| a.get_mut(i))
    else {
        return;
    };
    let base = match stored.get("args") {
        Some(v @ Value::Object(_)) => v.clone(),
        _ => json!({}),
    };
    let new_args = apply_match(&base, m, tool);
    if let Some(obj) = stored.as_object_mut() {
        obj.insert("args".to_string(), new_args);
        obj.insert("healed".to_string(), json!(true));
        obj.insert("healed_at".to_string(), json!(iso_now()));
    }
}

// applyMatch: ref si el tool lo admite; si no, coordenadas del centro del rectángulo.
fn apply_match(args: &Value, m: &Value, tool: &str) -> Value {
    let mut out = match args {
        Value::Object(o) => o.clone(),
        _ => Map::new(),
    };
    for key in ["id", "ref", "x", "y"] {
        out.remove(key);
    }
    if tool != HUMAN_CLICK {
        if let Some(r) = m.get("ref").filter(|v| js_truthy(v)) {
            out.insert("ref".to_string(), r.clone());
            return Value::Object(out);
        }
    }
    let [x, y, w, h] = match m.get("bounds") {
        Some(Value::Array(a)) => [0, 1, 2, 3].map(|k| num_or_zero(a.get(k))),
        _ => [m.get("x"), m.get("y"), m.get("width"), m.get("height")].map(num_or_zero),
    };
    out.insert("x".to_string(), num_value(round_js(x + w / 2.0)));
    out.insert("y".to_string(), num_value(round_js(y + h / 2.0)));
    Value::Object(out)
}

// Number(v) || 0.
fn num_or_zero(v: Option<&Value>) -> f64 {
    v.and_then(js_num_to_f64)
        .filter(|f| !f.is_nan())
        .unwrap_or(0.0)
}

// Texto de un campo: `campo || ""` y luego String().
fn text_of(v: Option<&Value>) -> String {
    v.filter(|v| js_truthy(v))
        .map(js_string_of)
        .unwrap_or_default()
}

// `a || b || ""` sobre dos claves de un objeto.
fn first_text(v: &Value, a: &str, b: &str) -> String {
    let first = text_of(v.get(a));
    if !first.is_empty() {
        first
    } else {
        text_of(v.get(b))
    }
}

// bestMatch: control_type compatible si ambos lo traen; similitud >= 0.6; automation_id igual => >= 0.9.
fn best_match(elements: &[Value], target: &Value) -> Option<Value> {
    let name = normalize_text(&text_of(target.get("name")));
    let id = normalize_text(&text_of(target.get("automation_id")));
    let ty = normalize_text(&first_text(target, "type", "control_type"));
    if name.is_empty() && id.is_empty() {
        return None;
    }
    let mut best: Option<&Value> = None;
    let mut best_score = 0.0f64;
    for el in elements {
        if !el.is_object() {
            continue;
        }
        let el_type = normalize_text(&first_text(el, "type", "control_type"));
        if !ty.is_empty() && !el_type.is_empty() && ty != el_type {
            continue;
        }
        let el_name = normalize_text(&text_of(el.get("name")));
        let el_id = normalize_text(&text_of(el.get("automation_id")));
        let mut score = similarity(&name, &el_name);
        if !id.is_empty() && !el_id.is_empty() && id == el_id {
            score = score.max(0.9);
        }
        if score > best_score {
            best_score = score;
            best = Some(el);
        }
    }
    let best = best?;
    if best_score < MATCH_THRESHOLD {
        return None;
    }
    let mut out = best.as_object()?.clone();
    let rounded: f64 = format!("{best_score:.3}").parse().unwrap_or(best_score);
    out.insert("match_score".to_string(), num_value(rounded));
    Some(Value::Object(out))
}

// normalizeText: minúsculas, sin diacríticos, espacios colapsados y recortados.
fn normalize_text(value: &str) -> String {
    let mut out = String::new();
    let mut in_ws = false;
    for ch in value.to_lowercase().chars() {
        let mapped = strip_accent(ch);
        if mapped.is_whitespace() {
            if !in_ws && !out.is_empty() {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(mapped);
            in_ws = false;
        }
    }
    out.trim_end().to_string()
}

fn strip_accent(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        other => other,
    }
}

// Similitud Levenshtein normalizada (0..1); ambos vacíos = 1, uno vacío = 0.
fn similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let s: Vec<char> = a.chars().collect();
    let t: Vec<char> = b.chars().collect();
    let max_len = s.len().max(t.len()) as f64;
    1.0 - levenshtein(&s, &t) as f64 / max_len
}

fn levenshtein(s: &[char], t: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=t.len()).collect();
    for (i, sc) in s.iter().enumerate() {
        let mut row = vec![i + 1];
        for (j, tc) in t.iter().enumerate() {
            let cost = usize::from(sc != tc);
            let v = (prev[j + 1] + 1).min(row[j] + 1).min(prev[j] + cost);
            row.push(v);
        }
        prev = row;
    }
    prev[t.len()]
}

// Persistencia del replay: last_replay siempre; healed_count/updated_at si hubo healing.
#[allow(clippy::too_many_arguments)]
fn persist(
    macro_v: &mut Value,
    store: &dyn MacroStore,
    ok: bool,
    executed: u64,
    healed: u64,
    skipped: u64,
    failed: u64,
    healed_any: bool,
) {
    let previous_healed = macro_v
        .get("healed_count")
        .and_then(js_num_to_f64)
        .filter(|f| !f.is_nan())
        .unwrap_or(0.0);
    let Some(obj) = macro_v.as_object_mut() else {
        return;
    };
    obj.insert(
        "last_replay".to_string(),
        json!({
            "at": iso_now(),
            "ok": ok,
            "executed": executed,
            "healed": healed,
            "skipped": skipped,
            "failed": failed
        }),
    );
    if healed_any {
        obj.insert(
            "healed_count".to_string(),
            num_value(previous_healed + healed as f64),
        );
        obj.insert("updated_at".to_string(), json!(iso_now()));
    }
    let _ = store.save(macro_v);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::FakeExec;
    use std::cell::RefCell;

    // Almacén en memoria: guarda lo último que se intenta persistir.
    struct MemStore {
        macro_v: Option<Value>,
        saved: RefCell<Vec<Value>>,
    }

    impl MacroStore for MemStore {
        fn load(&self, _name: &str) -> Option<Value> {
            self.macro_v.clone()
        }
        fn save(&self, macro_v: &Value) -> Result<(), String> {
            self.saved.borrow_mut().push(macro_v.clone());
            Ok(())
        }
    }

    fn store(steps: Value) -> MemStore {
        MemStore {
            macro_v: Some(json!({
                "schema": 1,
                "name": "demo",
                "created_at": "2026-10-01T00:00:00.000Z",
                "updated_at": "2026-10-01T00:00:00.000Z",
                "step_count": steps.as_array().map(|a| a.len()).unwrap_or(0),
                "steps": steps
            })),
            saved: RefCell::new(Vec::new()),
        }
    }

    fn json_of(out: ToolOut) -> (Value, bool) {
        match out {
            ToolOut::Json(v, e) => (v, e),
            _ => panic!("se esperaba Json"),
        }
    }

    #[test]
    fn missing_macro_reports_not_found() {
        let exec = FakeExec::default();
        let st = MemStore {
            macro_v: None,
            saved: RefCell::new(Vec::new()),
        };
        let (v, is_err) = json_of(run_with(&json!({"name": "Nada"}), &exec, &st));
        assert!(is_err);
        assert_eq!(v["error"], json!("macro_not_found"));
        assert_eq!(v["name"], json!("nada"));
    }

    #[test]
    fn dry_run_plans_without_executing() {
        let exec = FakeExec::default();
        let st = store(json!([
            {"tool": "computer_click", "args": {"x": 1, "y": 2}, "at": 1},
            {"tool": "computer_type", "args": {"secret_b64": {"__secret__": "secret_b64"}}, "at": 2}
        ]));
        let (v, is_err) = json_of(run_with(
            &json!({"name": "demo", "dry_run": true}),
            &exec,
            &st,
        ));
        assert!(!is_err);
        assert_eq!(v["dry_run"], json!(true));
        assert_eq!(v["executed"], json!(0));
        assert_eq!(v["skipped"], json!(1));
        assert_eq!(v["steps"][0]["status"], json!("dry"));
        assert_eq!(v["steps"][1]["detail"], json!("secret_required"));
        assert!(exec.calls.borrow().is_empty());
        assert!(st.saved.borrow().is_empty());
    }

    #[test]
    fn single_step_replays_and_persists_last_replay() {
        let exec = FakeExec::default();
        let st = store(json!([
            {"tool": "computer_click", "args": {"x": 5, "y": 6}, "at": 1}
        ]));
        let (v, is_err) = json_of(run_with(
            &json!({"name": "demo", "speed": 1000}),
            &exec,
            &st,
        ));
        assert!(!is_err);
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["executed"], json!(1));
        assert_eq!(v["steps"][0]["status"], json!("ok"));
        assert_eq!(exec.calls.borrow()[0].1, json!({"x": 5, "y": 6}));
        let saved = st.saved.borrow();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0]["last_replay"]["executed"], json!(1));
    }

    #[test]
    fn healing_with_match_uses_ref_and_persists_healed_step() {
        let exec = FakeExec::default();
        exec.script("computer_click", vec![Ok((json!({"ok": false}), true))]);
        *exec.elements.borrow_mut() = vec![json!({
            "name": "Guardar", "type": "Button", "ref": "e7",
            "bounds": [10, 20, 100, 40]
        })];
        let st = store(json!([
            {"tool": "computer_click", "args": {"x": 1, "y": 1,
                "__element": {"name": "Guardar", "type": "Button"}}, "at": 1}
        ]));
        let (v, is_err) = json_of(run_with(
            &json!({"name": "demo", "speed": 1000}),
            &exec,
            &st,
        ));
        assert!(!is_err);
        assert_eq!(v["healed"], json!(1));
        assert_eq!(v["steps"][0]["status"], json!("healed"));
        assert_eq!(v["steps"][0]["detail"], json!("healed_ref"));
        let calls = exec.calls.borrow();
        assert_eq!(calls[1].1, json!({"ref": "e7"}));
        let saved = st.saved.borrow();
        assert_eq!(saved[0]["healed_count"], json!(1));
        assert_eq!(
            saved[0]["steps"][0]["args"],
            json!({"__element": {"name": "Guardar", "type": "Button"}, "ref": "e7"})
        );
    }

    #[test]
    fn healing_without_match_fails_with_no_match() {
        let exec = FakeExec::default();
        exec.script("computer_click", vec![Ok((json!({"ok": false}), true))]);
        let st = store(json!([
            {"tool": "computer_click", "args": {"x": 1, "y": 1,
                "__element": {"name": "Guardar", "type": "Button"}}, "at": 1}
        ]));
        let (v, is_err) = json_of(run_with(
            &json!({"name": "demo", "speed": 1000}),
            &exec,
            &st,
        ));
        assert!(is_err);
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["failed"], json!(1));
        assert_eq!(v["steps"][0]["detail"], json!("no_match"));
        assert_eq!(exec.calls.borrow().len(), 1);
    }

    #[test]
    fn apply_match_uses_bounds_center_without_ref() {
        let m = json!({"bounds": [10, 20, 100, 40]});
        let out = apply_match(&json!({"x": 1, "y": 2, "text": "a"}), &m, "computer_click");
        assert_eq!(out, json!({"text": "a", "x": 60, "y": 40}));
    }

    #[test]
    fn best_match_respects_threshold() {
        let els = vec![json!({"name": "Cancelar", "type": "Button"})];
        assert!(best_match(&els, &json!({"name": "Guardar", "type": "Button"})).is_none());
        let hit = best_match(&els, &json!({"name": "cancelar", "type": "Button"}));
        assert!(hit.is_some());
    }
}

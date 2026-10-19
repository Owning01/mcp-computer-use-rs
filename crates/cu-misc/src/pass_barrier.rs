// cu-misc::pass_barrier — port de computer_pass_barrier (barrier.js) en modo seguro (sin dispatch).
// Implementa el flujo detect-only + handoff humano (ask_user) sin clicks/teclado.
use cu_native::system::active_window;
use serde_json::{json, Value};

const STRATEGIES: &[&str] = &["auto", "checkbox", "wait", "reload", "real_chrome", "ask_user"];
fn clamp_int(v: &Value, key: &str, min: i64, max: i64, def: i64) -> i64 {
    let n = v.get(key).and_then(|x| x.as_f64()).unwrap_or(def as f64);
    if !n.is_finite() { return def; }
    (n as i64).clamp(min, max)
}

fn build_strategy(t: &str) -> &'static str {
    match t {
        "turnstile" | "recaptcha" | "hcaptcha" => "checkbox",
        "cloudflare_challenge" => "wait",
        "google_unsafe" => "real_chrome",
        "waf" => "reload",
        "none" => "none",
        _ => "ask_user",
    }
}

pub fn pass_barrier(args: &Value) -> Value {
    let strategy_arg_raw = args.get("strategy").and_then(|v| v.as_str()).unwrap_or("auto");
    let strategy_arg = if STRATEGIES.contains(&strategy_arg_raw) { strategy_arg_raw } else { "auto" };
    let max_attempts = clamp_int(args, "max_attempts", 1, 10, 3);
    let timeout_ms = clamp_int(args, "timeout_ms", 1000, 300000, 45000);
    let backoff_ms = clamp_int(args, "backoff_ms", 0, 120000, 20000);

    let mut target = serde_json::Map::new();
    if let Some(h) = args.get("hwnd").and_then(|v| v.as_i64()) { target.insert("hwnd".into(), json!(h)); }
    if let Some(tw) = args.get("target_window").and_then(|v| v.as_str()) { target.insert("target_window".into(), json!(tw)); }
    let target_val = Value::Object(target);

    let active = active_window();
    let before_fp = active.get("title").and_then(|v| v.as_str()).map(|s| s.to_string());

    // detect
    let detection = crate::detect_block::detect_block(&target_val);
    let blocked = detection.get("blocked").and_then(|v| v.as_bool()).unwrap_or(false);
    let det_type_raw = detection.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let det_title = detection.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();

    let base_type = if !blocked { "none".to_string() } else {
        let classified = if det_type_raw.is_empty() { "unknown".to_string() } else { det_type_raw.clone() };
        if classified == "none" { "unknown".to_string() } else { classified }
    };

    let base = json!({
        "ok": true,
        "blocked": blocked,
        "type": base_type.clone(),
        "strategy_used": "none",
        "attempts": 0,
        "resolved": !blocked,
        "handled_by_user": false,
        "evidence": {
            "title": det_title.clone(),
            "before_fingerprint": before_fp,
            "after_fingerprint": before_fp
        }
    });

    if !blocked {
        return base;
    }

    let mut strategy = if strategy_arg == "auto" { build_strategy(&base_type) } else { strategy_arg };
    // checkbox requires checkbox_location; detect_block (title-only) has none -> ask_user
    if strategy == "checkbox" { strategy = "ask_user"; }

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
    let mut resolved = false;
    let mut attempts = 0i64;
    let mut handled_by_user = false;
    let mut detail: Option<String> = None;

    if strategy == "ask_user" {
        let message = format!("Bloqueo {} detectado: resolvé la verificación manualmente y el agente continuará.", base_type);
        crate::alert_user::alert_user(&json!({"message": message, "sound": true}));
        attempts = 0;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            let det = crate::detect_block::detect_block(&target_val);
            let still_blocked = det.get("blocked").and_then(|v| v.as_bool()).unwrap_or(false);
            if !still_blocked { resolved = true; break; }
        }
        handled_by_user = true;
        detail = Some(if resolved { "user_resolved".to_string() } else { "handoff_timeout".to_string() });
    } else {
        // Other strategies: fallback to ask_user handoff without side effects.
        let message = format!("Bloqueo {} detectado: resolvé manualmente.", base_type);
        crate::alert_user::alert_user(&json!({"message": message, "sound": true}));
        attempts = 1;
        handled_by_user = true;
        detail = Some("handoff".to_string());
    }

    let after_fp = active.get("title").and_then(|v| v.as_str()).map(|s| s.to_string());

    let mut out = json!({
        "ok": true,
        "blocked": blocked,
        "type": base_type,
        "strategy_used": strategy,
        "attempts": attempts,
        "resolved": resolved,
        "handled_by_user": handled_by_user,
        "evidence": {
            "title": det_title,
            "before_fingerprint": before_fp,
            "after_fingerprint": after_fp
        }
    });
    if let Some(d) = detail { out["detail"] = json!(d); }
    let _ = (max_attempts, backoff_ms, deadline);
    out
}
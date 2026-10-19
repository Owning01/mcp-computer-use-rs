// cu-core::js — helpers de semántica JavaScript usados por los ports.
// PORT NOTE: js_string_of cubre los casos reales (escalares/arrays); objetos -> "[object Object]".

pub fn js_truthy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(true),
        serde_json::Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

pub fn js_string_of(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Array(a) => a
            .iter()
            .map(|x| match x {
                serde_json::Value::Null => String::new(),
                other => js_string_of(other),
            })
            .collect::<Vec<_>>()
            .join(","),
        serde_json::Value::Object(_) => "[object Object]".to_string(),
    }
}

pub fn js_num_to_f64(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        serde_json::Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

// Truthy de flags de tool: ausente/null -> false; bool/number/string/objeto -> semántica JS.
pub fn flag_truthy(v: Option<&serde_json::Value>) -> bool {
    match v {
        None => false,
        Some(sv) => js_truthy(sv),
    }
}

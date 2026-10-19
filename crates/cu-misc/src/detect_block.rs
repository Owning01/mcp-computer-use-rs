// cu-misc::detect_block — port de nativeDetectBlock (index.js), detección por título de ventana.
// (El escaneo UIA del origen se omite: depende de cu-uia; el título cubre el caso anti-bot habitual.)
use cu_native::system::active_window;
use serde_json::{json, Value};

pub fn detect_block(args: &Value) -> Value {
    let active = active_window();
    let title_raw = active.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let title = title_raw.to_lowercase();

    let patterns: &[(&str, &str, &str)] = &[
        ("just a moment", "cloudflare_turnstile", "Cloudflare Turnstile / DDoS Protection"),
        ("attention required! | cloudflare", "cloudflare_waf", "Cloudflare WAF / Ray ID"),
        ("security challenge", "captcha_challenge", "Security Challenge"),
        ("pardon our interruption", "datadome_challenge", "DataDome / PerimeterX"),
        ("access denied", "access_denied", "Access Denied (403)"),
        ("unusual traffic", "google_unusual_traffic", "Google Unusual Traffic"),
    ];
    for (pat, typ, name) in patterns {
        if title.contains(pat) {
            return json!({
                "blocked": true,
                "type": typ,
                "detected_in": "window_title",
                "title": title_raw,
                "message": format!("Anti-bot protection detected: {}", name),
                "recommendations": [
                    "Use computer_human_click on the verification checkbox with natural mouse trajectory",
                    "If using automated browser, switch to real user Chrome via computer_login with authentic cookies/fingerprints"
                ]
            });
        }
    }
    json!({"blocked": false, "message": "No active bot gates or challenges detected"})
}
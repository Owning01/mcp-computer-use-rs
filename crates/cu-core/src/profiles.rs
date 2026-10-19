// cu-core::profiles — port de src/utils/browser_profiles.js (computer_login list_profiles).
// Lee los "Local State" de Chrome y Edge desde %LOCALAPPDATA%. Sin Win32.
use crate::js::js_truthy;
use serde_json::{json, Value};
use std::path::PathBuf;

// Devuelve los perfiles en orden: Chrome primero, luego Edge; dentro de cada uno,
// el orden de claves del info_cache (preserve_order).
pub fn get_browser_profiles() -> Vec<Value> {
    let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let mut results: Vec<Value> = Vec::new();
    let sources: [(&str, [&str; 3]); 2] = [
        ("chrome", ["Google", "Chrome", "User Data"]),
        ("edge", ["Microsoft", "Edge", "User Data"]),
    ];
    for (browser, dir_parts) in sources {
        let path = PathBuf::from(&local_app_data)
            .join(dir_parts[0])
            .join(dir_parts[1])
            .join(dir_parts[2])
            .join("Local State");
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(data) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let Some(cache) = data
            .get("profile")
            .and_then(|p| p.get("info_cache"))
            .and_then(|c| c.as_object())
        else {
            continue;
        };
        for (folder, info) in cache {
            let info_obj = info.as_object();
            let get = |k: &str| info_obj.and_then(|o| o.get(k));
            let name = get("name")
                .filter(|v| js_truthy(v))
                .cloned()
                .or_else(|| get("shortcut_name").filter(|v| js_truthy(v)).cloned())
                .unwrap_or_else(|| json!(folder));
            let email = get("user_name")
                .filter(|v| js_truthy(v))
                .cloned()
                .unwrap_or_else(|| json!(""));
            let gaia_name = get("gaia_name")
                .filter(|v| js_truthy(v))
                .cloned()
                .unwrap_or_else(|| json!(""));
            results.push(json!({
                "browser": browser,
                "folder": folder,
                "name": name,
                "email": email,
                "gaia_name": gaia_name,
                "is_default": folder == "Default"
            }));
        }
    }
    results
}

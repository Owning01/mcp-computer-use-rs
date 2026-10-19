// PORT NOTE: computer_login rama launch (no list_profiles, no account/CDP).
// list_profiles lo sirve el puente nativo; account (CDP) se excluye via accepts().
use crate::focus::focus_window;
use cu_core::out::ToolOut;
use serde_json::json;
use std::fs;
use std::process::Command;

#[derive(Clone)]
struct Profile {
    browser: String,
    folder: String,
    name: String,
    email: String,
    gaia_name: String,
}

fn get_browser_profiles() -> Vec<Profile> {
    let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let mut results = Vec::new();
    // Chrome
    let chrome_state = format!("{}\\Google\\Chrome\\User Data\\Local State", local_app_data);
    if let Ok(data) = fs::read_to_string(&chrome_state) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
            if let Some(cache) = v.pointer("/profile/info_cache").and_then(|x| x.as_object()) {
                for (folder, info) in cache {
                    results.push(Profile {
                        browser: "chrome".to_string(),
                        folder: folder.clone(),
                        name: info.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        email: info.get("user_name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        gaia_name: info.get("gaia_name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    });
                }
            }
        }
    }
    // Edge
    let edge_state = format!("{}\\Microsoft\\Edge\\User Data\\Local State", local_app_data);
    if let Ok(data) = fs::read_to_string(&edge_state) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
            if let Some(cache) = v.pointer("/profile/info_cache").and_then(|x| x.as_object()) {
                for (folder, info) in cache {
                    results.push(Profile {
                        browser: "edge".to_string(),
                        folder: folder.clone(),
                        name: info.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        email: info.get("user_name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        gaia_name: info.get("gaia_name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    });
                }
            }
        }
    }
    results
}

fn resolve_browser_profile(query: &str) -> Option<Profile> {
    if query.is_empty() {
        return None;
    }
    let profiles: Vec<Profile> = get_browser_profiles().into_iter().filter(|p| p.browser == "chrome").collect();
    let q = query.trim().to_lowercase();
    // exact folder
    if let Some(p) = profiles.iter().find(|p| p.folder.to_lowercase() == q) {
        return Some(p.clone());
    }
    // email
    if let Some(p) = profiles.iter().find(|p| p.email.to_lowercase() == q || p.email.to_lowercase().contains(&q)) {
        return Some(p.clone());
    }
    // name / gaia_name
    if let Some(p) = profiles.iter().find(|p| p.name.to_lowercase() == q || p.gaia_name.to_lowercase().contains(&q)) {
        return Some(p.clone());
    }
    None
}

pub fn login(args: &serde_json::Value) -> ToolOut {
    let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if url.is_empty() {
        return ToolOut::Json(json!({"ok": false, "error": "url parameter is required (unless list_profiles is true)"}), true);
    }
    let profile_query = args.get("profile").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let resolved = if profile_query.is_empty() {
        None
    } else {
        resolve_browser_profile(&profile_query)
    };
    let profile_folder = if let Some(r) = &resolved {
        r.folder.clone()
    } else if profile_query.is_empty() {
        String::new()
    } else {
        profile_query.clone()
    };

    let remote_debugging = args.get("remote_debugging").and_then(|v| v.as_bool()).unwrap_or(false);

    // Chrome paths
    let chrome_paths = [
        "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
        "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
    ];
    let chrome_exe = chrome_paths.iter().find(|p| std::path::Path::new(p).exists()).map(|s| s.to_string());

    if let Some(chrome) = chrome_exe {
        let mut launch_args: Vec<String> = Vec::new();
        if !profile_folder.is_empty() {
            launch_args.push(format!("--profile-directory={}", profile_folder));
        }
        if remote_debugging {
            launch_args.push("--remote-debugging-port=9222".to_string());
        }
        launch_args.push(url.clone());

        let _ = Command::new(&chrome).args(&launch_args).spawn();

        // focus Chrome
        let _ = focus_window(&json!({"title": "Chrome"}));

        // account handling excluded via accepts(); skip here.

        let resolved_user = resolved.as_ref().map(|r| {
            if !r.email.is_empty() { r.email.clone() } else { r.name.clone() }
        });

        return ToolOut::Json(json!({
            "ok": true,
            "opened": "chrome",
            "profile": if profile_folder.is_empty() { "default" } else { &profile_folder },
            "resolved_user": resolved_user,
            "url": url,
            "focused": true,
            "remote_debugging": remote_debugging
        }), false);
    }

    // default system browser
    let _ = Command::new("cmd").args(["/C", "start", "", &url]).spawn();
    ToolOut::Json(json!({"ok": true, "opened": "default_browser", "url": url}), false)
}
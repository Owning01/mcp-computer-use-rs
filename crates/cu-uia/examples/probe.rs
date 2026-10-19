use cu_core::out::ToolOut;
use cu_uia;
use serde_json::json;

fn main() {
    for (name, args) in [
        ("computer_find_element", json!({"name":"Close","max_results":5})),
        ("computer_map_ui", json!({})),
        ("computer_find_template", json!({"template_b64":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==","min_confidence":0.5})),
    ] {
        let out = cu_uia::handle_call(name, &args);
        match out {
            ToolOut::Json(v, err) => println!("{} err={} {}", name, err, v.to_string().chars().take(300).collect::<String>()),
            ToolOut::JsonStructured(v, err) => println!("{} err={} {}", name, err, v.to_string().chars().take(300).collect::<String>()),
            _ => println!("{} other", name),
        }
    }
}
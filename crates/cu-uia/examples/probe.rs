use cu_core::out::ToolOut;
use cu_uia;
use serde_json::json;

fn main() {
    for name in ["computer_state_fingerprint", "computer_snapshot", "computer_verify", "computer_inspect_ui"] {
        let args = match name {
            "computer_snapshot" => json!({"max_results": 5}),
            "computer_verify" => json!({"expect": {"element": {"name": "__cu_smoke_never_exists__", "exists": false}}, "timeout_ms": 800, "interval_ms": 100}),
            "computer_inspect_ui" => json!({"max_results": 3}),
            _ => json!({}),
        };
        let out = cu_uia::handle_call(name, &args);
        match out {
            ToolOut::Json(v, err) => println!("{} err={} {}", name, err, v),
            ToolOut::Raw(s, err) => println!("{} err={} {}", name, err, s),
            _ => println!("{} other", name),
        }
    }
}
// cu-uia — capa UIA nativa (F6). Reglas: migration/RULEBOOK.md.
// Contrato del puente: handles(name) + handle_call(name, args) -> cu_core::out::ToolOut.
use cu_core::out::ToolOut;
use serde_json::{Map, Value, json};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTreeWalker,
    UIA_ExpandCollapsePatternId, UIA_InvokePatternId, UIA_SelectionItemPatternId,
    UIA_TogglePatternId, UIA_ValuePatternId, IUIAutomationValuePattern,
    IUIAutomationSelectionItemPattern,
};

// ---------------- helpers ----------------

fn as_str(v: &Value) -> Option<&str> { v.as_str() }
fn as_i64(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64() { return Some(n); }
    if let Some(n) = v.as_f64() { return Some(n as i64); }
    if let Some(s) = v.as_str() { return s.trim().parse::<i64>().ok(); }
    None
}
fn as_bool(v: &Value) -> Option<bool> { v.as_bool() }

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn bstr_to_string(b: windows::core::BSTR) -> String {
    b.to_string()
}

fn control_type_name(id: i32) -> String {
    // mapeo UIA_CONTROLTYPE_ID -> nombre corto (como .NET ProgrammaticName sin "ControlType.")
    let pairs: &[(i32, &str)] = &[
        (50032, "Window"), (50040, "AppBar"), (50000, "Button"), (50001, "Calendar"), (50002, "CheckBox"),
        (50003, "ComboBox"), (50025, "Custom"), (50028, "DataGrid"), (50029, "DataItem"),
        (50030, "Document"), (50004, "Edit"), (50026, "Group"), (50034, "Header"),
        (50035, "HeaderItem"), (50005, "Hyperlink"), (50006, "Image"), (50008, "List"),
        (50007, "ListItem"), (50010, "MenuBar"), (50009, "Menu"), (50011, "MenuItem"),
        (50033, "Pane"), (50012, "ProgressBar"), (50013, "RadioButton"), (50014, "ScrollBar"),
        (50038, "Separator"), (50015, "Slider"), (50016, "Spinner"), (50031, "SplitButton"),
        (50017, "StatusBar"), (50018, "Tab"), (50019, "TabItem"), (50036, "Table"),
        (50020, "Text"), (50027, "Thumb"), (50037, "TitleBar"), (50021, "ToolBar"),
        (50022, "ToolTip"), (50023, "Tree"), (50024, "TreeItem"),
    ];
    for (k, v) in pairs { if *k == id { return v.to_string(); } }
    format!("Unknown({})", id)
}

// ---------------- UIA automation ----------------

fn init_com() {
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
}

fn create_automation() -> Option<IUIAutomation> {
    init_com();
    unsafe {
        unsafe { CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok() }
    }
}

fn root_element(auto: &IUIAutomation, hwnd: i64, target_window: &str) -> Option<IUIAutomationElement> {
    unsafe {
        if hwnd > 0 {
            return auto.ElementFromHandle(HWND(hwnd as *mut _)).ok();
        }
        if !target_window.is_empty() {
            // buscar entre hijos top-level
            if let Ok(root) = auto.GetRootElement() {
                if let Ok(walker) = auto.ControlViewWalker() {
                    let mut child = walker.GetFirstChildElement(&root).ok();
                    let needle = target_window.to_lowercase();
                    let aliases: &[(&str,&[&str])] = &[
                        ("notepad", &["bloc de notas","notepad"]),
                        ("calculator", &["calculadora","calc","calculator"]),
                        ("calc", &["calculadora","calculator","calc"]),
                        ("paint", &["paint","mspaint"]),
                        ("terminal", &["powershell","cmd","terminal","símbolo del sistema"]),
                        ("explorer", &["explorador de archivos","explorer"]),
                        ("settings", &["configuración","ajustes","settings"]),
                    ];
                    let mut terms = vec![target_window.to_string()];
                    for (k, vals) in aliases {
                        if needle.contains(k) {
                            for v in *vals { if !terms.iter().any(|t| t.eq_ignore_ascii_case(v)) { terms.push(v.to_string()); } }
                        }
                    }
                    while let Some(el) = child {
                        let name = el.CurrentName().map(bstr_to_string).unwrap_or_default();
                        let aid = el.CurrentAutomationId().map(bstr_to_string).unwrap_or_default();
                        let pid = el.CurrentProcessId().unwrap_or(0);
                        let pname = process_name(pid as u32);
                        for t in &terms {
                            let tl = t.to_lowercase();
                            if (!name.is_empty() && name.to_lowercase().contains(&tl))
                                || (!aid.is_empty() && aid.to_lowercase().contains(&tl))
                                || (!pname.is_empty() && pname.to_lowercase().contains(&tl)) {
                                return Some(el);
                            }
                        }
                        child = walker.GetNextSiblingElement(&el).ok();
                    }
                }
            }
            // fallback: procesos
            // (omitido por simplicidad; usar foreground si nada)
        }
        // foreground: preferir la ventana foreground real (hwnd no nulo)
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
            let fg = GetForegroundWindow();
            if !fg.0.is_null() {
                if let Ok(el) = auto.ElementFromHandle(fg) { return Some(el); }
            }
        }
        auto.GetFocusedElement().ok()
    }
}

fn process_name(pid: u32) -> String {
    // via windows toolhelp
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    unsafe {
        let snap = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) { Ok(h) => h, Err(_) => return String::new() };
        let mut pe = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut pe).is_ok() {
            loop {
                if pe.th32ProcessID == pid {
                    let name = String::from_utf16_lossy(&pe.szExeFile);
                    let name = name.trim_end_matches('\0').to_string();
                    let _ = windows::Win32::Foundation::CloseHandle(snap);
                    return name;
                }
                if Process32NextW(snap, &mut pe).is_err() { break; }
            }
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
    }
    String::new()
}

// ---------------- collect ----------------

#[derive(Debug, Clone)]
pub struct CollectResult {
    pub ok: bool,
    pub error: String,
    pub window_title: String,
    pub pid: i64,
    pub hwnd: i64,
    pub truncated: bool,
    pub elements: Vec<Map<String, Value>>,
}

fn test_sensitive(c_name: &str, aid: &str, class_name: &str) -> bool {
    let parts = format!("{} {} {}", c_name, aid, class_name);
    let low = parts.to_lowercase();
    // regex-ish keywords
    for kw in ["password", "passcode", "secret", "pin", "one-time code", "verification code", "otp"] {
        if low.contains(kw) { return true; }
    }
    // pin\b : check word boundary
    if low.split(|c: char| !c.is_alphanumeric()).any(|w| w == "pin") { return true; }
    false
}

fn collect_elements(
    hwnd: i64,
    target_window: &str,
    max_depth: i64,
    max_results: i64,
    include_patterns: bool,
    interactive_only: bool,
) -> CollectResult {
    let max_depth = max_depth.clamp(1, 30);
    let max_results = max_results.clamp(1, 1000);
    let auto = match create_automation() { Some(a) => a, None => {
        return CollectResult { ok:false, error:"uia_init_failed".into(), window_title:String::new(), pid:0, hwnd, truncated:false, elements:vec![] };
    }};
    let root = match root_element(&auto, hwnd, target_window) {
        Some(r) => r,
        None => return CollectResult { ok:false, error:"window_not_found".into(), window_title:String::new(), pid:0, hwnd, truncated:false, elements:vec![] },
    };

    // walker
    let walker = match unsafe { auto.ControlViewWalker() } { Ok(w) => w, Err(_) => {
        return CollectResult { ok:false, error:"walker_failed".into(), window_title:String::new(), pid:0, hwnd, truncated:false, elements:vec![] };
    }};

    // raw items
    let mut items: Vec<Map<String, Value>> = Vec::new();
    let mut truncated = false;

    // recursive closure via function
    fn visit(
        walker: &IUIAutomationTreeWalker,
        el: &IUIAutomationElement,
        depth: i64,
        parent_idx: i64,
        max_depth: i64,
        max_results: usize,
        include_patterns: bool,
        items: &mut Vec<Map<String, Value>>,
        truncated: &mut bool,
    ) {
        if items.len() >= max_results { *truncated = true; return; }
        let idx = items.len() as i64;
        // build node
        let mut node: Map<String, Value> = Map::new();
        let c_type_id = unsafe { el.CurrentControlType().map(|v| v.0).unwrap_or(0) };
        let type_short = control_type_name(c_type_id);
        let name = unsafe { el.CurrentName().map(bstr_to_string).unwrap_or_default() };
        let aid = unsafe { el.CurrentAutomationId().map(bstr_to_string).unwrap_or_default() };
        let class_name = unsafe { el.CurrentClassName().map(bstr_to_string).unwrap_or_default() };
        let rect = unsafe { el.CurrentBoundingRectangle().unwrap_or(RECT { left:0, top:0, right:0, bottom:0 }) };
        let enabled = unsafe { el.CurrentIsEnabled().map(|b| b.as_bool()).unwrap_or(true) };
        let offscreen = unsafe { el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false) };
        let native_hwnd = unsafe { el.CurrentNativeWindowHandle().map(|h| h.0 as i64).unwrap_or(0) };
        let is_sensitive = type_short == "Edit" && test_sensitive(&name, &aid, &class_name);

        node.insert("index".into(), json!(idx));
        node.insert("parent".into(), json!(parent_idx));
        node.insert("depth".into(), json!(depth));
        node.insert("control_type".into(), json!(type_short));
        node.insert("x".into(), json!(rect.left));
        node.insert("y".into(), json!(rect.top));
        node.insert("width".into(), json!(rect.right - rect.left));
        node.insert("height".into(), json!(rect.bottom - rect.top));
        node.insert("enabled".into(), json!(enabled));
        if !name.is_empty() {
            if is_sensitive { node.insert("name".into(), json!("[redacted]")); }
            else { node.insert("name".into(), json!(name)); }
        }
        if !aid.is_empty() { node.insert("automation_id".into(), json!(aid)); }
        if !class_name.is_empty() { node.insert("class_name".into(), json!(class_name)); }
        if offscreen { node.insert("offscreen".into(), json!(true)); }
        if is_sensitive { node.insert("is_password".into(), json!(true)); }
        if native_hwnd > 0 { node.insert("hwnd".into(), json!(native_hwnd)); }

        if include_patterns {
            let mut pats: Vec<&str> = Vec::new();
            unsafe {
                if el.GetCurrentPattern(UIA_InvokePatternId).is_ok() { pats.push("invoke"); }
                if el.GetCurrentPattern(UIA_ValuePatternId).is_ok() { pats.push("value"); }
                if el.GetCurrentPattern(UIA_TogglePatternId).is_ok() { pats.push("toggle"); }
                if el.GetCurrentPattern(UIA_ExpandCollapsePatternId).is_ok() { pats.push("expand"); }
                if el.GetCurrentPattern(UIA_SelectionItemPatternId).is_ok() { pats.push("select"); }
            }
            if !pats.is_empty() { node.insert("patterns".into(), json!(pats)); }
        }

        // value + focused for Edit/ComboBox/Document
        if type_short == "Edit" || type_short == "ComboBox" || type_short == "Document" {
            if is_sensitive {
                node.insert("value".into(), json!("[redacted]"));
            } else {
                unsafe {
                    if let Ok(vp) = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
                        if let Ok(v) = vp.CurrentValue() {
                            let val = bstr_to_string(v);
                            if !val.is_empty() {
                                let val = if val.len() > 500 { val.chars().take(500).collect::<String>() } else { val };
                                node.insert("value".into(), json!(val));
                            }
                        }
                    }
                }
            }
            // focused: .NET c.IsFocused; IUIAutomation element has CurrentHasKeyboardFocus
            unsafe {
                if let Ok(b) = el.CurrentHasKeyboardFocus() {
                    if b.as_bool() { node.insert("focused".into(), json!(true)); }
                }
            }
        }

        items.push(node);

        // subtree suppression
        if (type_short == "Button" || type_short == "CheckBox" || type_short == "RadioButton" || type_short == "Hyperlink")
            && !name.is_empty() {
            return;
        }
        if depth >= max_depth { return; }

        // tab compression
        if type_short == "Tab" || type_short == "ToolBar" || type_short == "Custom" {
            // collect child tabs
            let mut tabs: Vec<IUIAutomationElement> = Vec::new();
            unsafe {
                let mut ch = walker.GetFirstChildElement(el).ok();
                while let Some(c) = ch {
                    let ct = c.CurrentControlType().map(|v| v.0).unwrap_or(0);
                    if control_type_name(ct) == "TabItem" { tabs.push(c.clone()); }
                    ch = walker.GetNextSiblingElement(&c).ok();
                }
            }
            if tabs.len() >= 10 {
                // active tab
                let mut active: Option<IUIAutomationElement> = None;
                for t in &tabs {
                    unsafe {
                        if let Ok(sp) = t.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId) {
                            if let Ok(sel) = sp.CurrentIsSelected() {
                                if sel.as_bool() { active = Some(t.clone()); break; }
                            }
                        }
                    }
                }
                if active.is_none() && !tabs.is_empty() { active = Some(tabs[0].clone()); }
                if let Some(at) = active {
                    visit(walker, &at, depth + 1, idx, max_depth, max_results, include_patterns, items, truncated);
                    // omitted summary
                    if items.len() < max_results {
                        let omitted = tabs.len() - 1;
                        let mut onode: Map<String, Value> = Map::new();
                        onode.insert("index".into(), json!(items.len()));
                        onode.insert("parent".into(), json!(idx));
                        onode.insert("depth".into(), json!(depth + 1));
                        onode.insert("control_type".into(), json!("TabItem"));
                        onode.insert("name".into(), json!(format!("... ({} inactive browser tabs omitted)", omitted)));
                        onode.insert("is_summary".into(), json!(true));
                        onode.insert("enabled".into(), json!(true));
                        onode.insert("x".into(), json!(0));
                        onode.insert("y".into(), json!(0));
                        onode.insert("width".into(), json!(0));
                        onode.insert("height".into(), json!(0));
                        items.push(onode);
                    }
                }
                return;
            }
        }

        // children
        unsafe {
            let mut child = walker.GetFirstChildElement(el).ok();
            while let Some(ch) = child {
                if items.len() >= max_results { *truncated = true; break; }
                visit(walker, &ch, depth + 1, idx, max_depth, max_results, include_patterns, items, truncated);
                child = walker.GetNextSiblingElement(&ch).ok();
            }
        }
    }

    // Need import IUIAutomationSelectionItemPattern? We used fully qualified maybe. Let's ensure imported. We'll add import.
    unsafe {
        visit(&walker, &root, 0, -1, max_depth, max_results as usize, include_patterns, &mut items, &mut truncated);
    }

    // window info
    let mut window_title = String::new();
    let mut proc_id: i64 = 0;
    let mut root_hwnd: i64 = hwnd;
    unsafe {
        if let Ok(rc) = root.CurrentName() { window_title = bstr_to_string(rc); }
        if let Ok(p) = root.CurrentProcessId() { proc_id = p as i64; }
        if let Ok(h) = root.CurrentNativeWindowHandle() { if h.0 as i64 != 0 { root_hwnd = h.0 as i64; } }
    }
    if root_hwnd == 0 {
        // hwnd param
        root_hwnd = hwnd;
    }

    // index element tree
    let indexed = index_element_tree(items);

    // filter interactive if requested
    let filtered = if interactive_only {
        indexed.into_iter().filter(|el| {
            let ct = el.get("control_type").and_then(as_str).unwrap_or("");
            let off = el.get("offscreen").and_then(as_bool).unwrap_or(false);
            let w = el.get("width").and_then(as_i64).unwrap_or(0);
            let h = el.get("height").and_then(as_i64).unwrap_or(0);
            const INTER: &[&str] = &["Button","Edit","MenuItem","CheckBox","RadioButton","TabItem","ComboBox","Hyperlink","Document","ListItem","TreeItem"];
            INTER.contains(&ct) && !off && w > 5 && h > 5
        }).collect()
    } else {
        indexed
    };

    CollectResult {
        ok: true,
        error: String::new(),
        window_title,
        pid: proc_id,
        hwnd: root_hwnd,
        truncated,
        elements: filtered,
    }
}

// Need import SelectionItemPattern interface. We'll add to imports.
// ---------------- index element tree ----------------

fn identity_segment(el: &Map<String, Value>) -> String {
    let ct = el.get("control_type").or(el.get("type")).and_then(as_str).unwrap_or("");
    let id = el.get("automation_id").and_then(as_str).unwrap_or("");
    let name = el.get("name").and_then(as_str).unwrap_or("");
    if ct.is_empty() && id.is_empty() && name.is_empty() {
        let idx = el.get("index").or(el.get("i")).or(el.get("sibling_index")).and_then(as_i64).unwrap_or(0);
        return format!("#{}", idx);
    }
    let sibling = el.get("sibling_index").and_then(as_i64).unwrap_or(0);
    format!("{}:{}:{}#{}", if ct.is_empty() {"?"} else {ct}, id, name, sibling)
}

fn compute_ref(el: &Map<String, Value>) -> String {
    let ancestors = el.get("ancestors").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let mut parts: Vec<String> = Vec::new();
    for a in &ancestors {
        if let Some(s) = a.as_str() { parts.push(s.to_string()); }
        else if let Some(obj) = a.as_object() { parts.push(identity_segment(obj)); }
    }
    parts.push(identity_segment(el));
    parts.join("/")
}

fn index_element_tree(elements: Vec<Map<String, Value>>) -> Vec<Map<String, Value>> {
    let nodes = elements;
    let n = nodes.len();
    let mut counters: HashMap<String, usize> = HashMap::new();
    let mut stored: Vec<Map<String, Value>> = Vec::with_capacity(n);
    let mut list: Vec<Map<String, Value>> = Vec::with_capacity(n);
    for i in 0..n {
        let el = nodes[i].clone();
        let parent = el.get("parent").and_then(as_i64).unwrap_or(-1);
        let parent_idx = if parent >= 0 && (parent as usize) < i { parent } else { -1 };
        let parent_node = if parent_idx >= 0 { Some(&stored[parent_idx as usize]) } else { None };
        let ct = el.get("control_type").and_then(as_str).unwrap_or("").to_string();
        let aid = el.get("automation_id").and_then(as_str).unwrap_or("").to_string();
        let name = el.get("name").and_then(as_str).unwrap_or("").to_string();
        let key = format!("{}|{}|{}", ct, aid, name);
        let counter_key = format!("{}|{}", parent_idx, key);
        let sibling_index = *counters.get(&counter_key).unwrap_or(&0);
        counters.insert(counter_key, sibling_index + 1);

        let mut node = el.clone();
        node.insert("parent".into(), json!(parent_idx));
        node.insert("sibling_index".into(), json!(sibling_index));

        let ancestors_val = if let Some(pn) = parent_node {
            let mut anc = pn.get("ancestors").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            anc.push(json!(identity_segment(pn)));
            anc
        } else {
            Vec::new()
        };
        node.insert("ancestors".into(), Value::Array(ancestors_val));

        let r = compute_ref(&node);
        node.insert("ref".into(), json!(r));

        stored.push(node.clone());
        list.push(node);
    }
    list
}

// ---------------- fingerprint ----------------

fn bounds_of_str(el: &Map<String, Value>) -> String {
    if let Some(arr) = el.get("bounds").and_then(|v| v.as_array()) {
        return arr.iter().map(|x| x.to_string().trim_matches('"').to_string()).collect::<Vec<_>>().join(",");
    }
    let x = el.get("x").and_then(as_i64).unwrap_or(0);
    let y = el.get("y").and_then(as_i64).unwrap_or(0);
    let w = el.get("width").or(el.get("w")).and_then(as_i64).unwrap_or(0);
    let h = el.get("height").or(el.get("h")).and_then(as_i64).unwrap_or(0);
    format!("{},{},{},{}", x, y, w, h)
}

fn sha1_hex(data: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(data);
    let res = h.finalize();
    res.iter().map(|b| format!("{:02x}", b)).collect()
}

fn fingerprint_of(elements: &[Map<String, Value>], window_title: &str, pid: i64, hwnd: i64) -> String {
    let mut rows: Vec<String> = elements.iter().map(|el| {
        let r = el.get("ref").or(el.get("i")).map(|v| {
            match v {
                Value::String(s) => s.clone(),
                other => other.to_string().trim_matches('"').to_string(),
            }
        }).unwrap_or_default();
        let name = el.get("name").and_then(as_str).unwrap_or("");
        let enabled = if el.get("enabled").and_then(as_bool) == Some(false) || el.get("disabled").and_then(as_bool) == Some(true) { "0" } else { "1" };
        let b = bounds_of_str(el);
        format!("{}|{}|{}|{}", r, name, b, enabled)
    }).collect();
    rows.sort();
    let payload = format!("t:{}\np:{}\nh:{}\n{}", window_title, pid, hwnd, rows.join("\n"));
    sha1_hex(payload.as_bytes())
}

// ---------------- snapshot state ----------------

#[derive(Default)]
struct SnapshotState {
    hwnd: i64,
    title: String,
    generation: i64,
    map: HashMap<String, Map<String, Value>>,
    order: Vec<String>,
}
static SNAPSHOT_STATE: OnceLock<Mutex<SnapshotState>> = OnceLock::new();
fn snapshot_state() -> &'static Mutex<SnapshotState> {
    SNAPSHOT_STATE.get_or_init(|| Mutex::new(SnapshotState::default()))
}

static LAST_FP: OnceLock<Mutex<Option<String>>> = OnceLock::new();
fn last_fp() -> &'static Mutex<Option<String>> {
    LAST_FP.get_or_init(|| Mutex::new(None))
}

// ---------------- to_public_element ----------------

fn to_public_element(el: &Map<String, Value>, i: usize, include_patterns: bool) -> Map<String, Value> {
    let mut pubm = Map::new();
    pubm.insert("ref".into(), el.get("ref").cloned().unwrap_or(Value::Null));
    pubm.insert("i".into(), json!(i));
    let ct = el.get("control_type").and_then(as_str).unwrap_or("");
    pubm.insert("type".into(), json!(ct));
    let x = el.get("x").and_then(as_i64).unwrap_or(0);
    let y = el.get("y").and_then(as_i64).unwrap_or(0);
    let w = el.get("width").and_then(as_i64).unwrap_or(0);
    let h = el.get("height").and_then(as_i64).unwrap_or(0);
    pubm.insert("bounds".into(), json!([x, y, w, h]));
    let enabled = el.get("enabled").and_then(as_bool).unwrap_or(true);
    pubm.insert("enabled".into(), json!(enabled));
    if let Some(name) = el.get("name").and_then(as_str) {
        if !name.is_empty() { pubm.insert("name".into(), json!(name)); }
    }
    if let Some(aid) = el.get("automation_id").and_then(as_str) {
        if !aid.is_empty() { pubm.insert("automation_id".into(), json!(aid)); }
    }
    if include_patterns {
        if let Some(pats) = el.get("patterns").and_then(|v| v.as_array()) {
            if !pats.is_empty() { pubm.insert("patterns".into(), Value::Array(pats.clone())); }
        }
    }
    pubm
}

// ---------------- handlers ----------------

fn handle_inspect_ui(args: &Value) -> ToolOut {
    let hwnd = args.get("hwnd").and_then(as_i64).unwrap_or(0);
    let title = args.get("title").or(args.get("target_window")).and_then(as_str).unwrap_or("").to_string();
    let name_need = args.get("name").and_then(as_str).unwrap_or("").to_string();
    let id_need = args.get("automation_id").and_then(as_str).unwrap_or("").to_string();
    let type_need = args.get("control_type").and_then(as_str).unwrap_or("").to_string();
    let max_depth = args.get("max_depth").and_then(as_i64).unwrap_or(12);
    let max_results = args.get("max_results").and_then(as_i64).unwrap_or(200);

    let collected = collect_elements(hwnd, &title, max_depth, max_results, true, false);
    if !collected.ok {
        return ToolOut::Json(json!({"error":"window_not_found","elements":[]}), true);
    }
    // filter by name/id/type
    let mut items: Vec<Map<String, Value>> = Vec::new();
    for el in &collected.elements {
        let c_name = el.get("name").and_then(as_str).unwrap_or("");
        let c_aid = el.get("automation_id").and_then(as_str).unwrap_or("");
        let c_type = el.get("control_type").and_then(as_str).unwrap_or("");
        let type_short = c_type;
        // contains-text for name
        if !name_need.is_empty() {
            if !c_name.to_lowercase().contains(&name_need.to_lowercase()) { continue; }
        }
        if !id_need.is_empty() && c_aid != id_need { continue; }
        if !type_need.is_empty() {
            if type_need != type_short && type_need != c_type { continue; }
        }
        // build node like origin
        let mut node: Map<String, Value> = Map::new();
        node.insert("control_type".into(), json!(type_short));
        // bounding rect fields
        let rect_left = el.get("x").and_then(as_i64).unwrap_or(0);
        let rect_top = el.get("y").and_then(as_i64).unwrap_or(0);
        let rect_w = el.get("width").and_then(as_i64).unwrap_or(0);
        let rect_h = el.get("height").and_then(as_i64).unwrap_or(0);
        node.insert("x".into(), json!(rect_left));
        node.insert("y".into(), json!(rect_top));
        node.insert("width".into(), json!(rect_w));
        node.insert("height".into(), json!(rect_h));
        node.insert("depth".into(), el.get("depth").cloned().unwrap_or(json!(0)));
        if !c_name.is_empty() { node.insert("name".into(), json!(c_name)); }
        if !c_aid.is_empty() { node.insert("automation_id".into(), json!(c_aid)); }
        if let Some(cn) = el.get("class_name").and_then(as_str) {
            if !cn.is_empty() { node.insert("class_name".into(), json!(cn)); }
        }
        if el.get("hwnd").is_some() { node.insert("hwnd".into(), el.get("hwnd").cloned().unwrap()); }
        if el.get("disabled").and_then(as_bool) == Some(true) { } 
        if el.get("enabled").and_then(as_bool) == Some(false) { node.insert("disabled".into(), json!(true)); }
        if el.get("offscreen").and_then(as_bool) == Some(true) { node.insert("offscreen".into(), json!(true)); }
        if let Some(pats) = el.get("patterns") {
            if pats.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
                node.insert("patterns".into(), pats.clone());
            }
        }
        items.push(node);
        if items.len() >= max_results as usize { break; }
    }

    let payload = json!({
        "found": !items.is_empty(),
        "root_hwnd": collected.hwnd,
        "elements": items
    });
    ToolOut::Json(payload, false)
}

fn handle_snapshot(args: &Value) -> ToolOut {
    let hwnd = args.get("hwnd").and_then(as_i64).unwrap_or(0);
    let target_window = args.get("target_window").and_then(as_str).unwrap_or("").to_string();
    let max_depth = args.get("max_depth").and_then(as_i64).unwrap_or(12);
    let max_results = args.get("max_results").and_then(as_i64).unwrap_or(200);
    let include_patterns = args.get("include_patterns").and_then(as_bool).unwrap_or(false);
    let mode_req = args.get("mode").and_then(as_str).unwrap_or("auto").to_string();
    let requested = if ["auto","full","diff"].contains(&mode_req.as_str()) { mode_req } else { "auto".to_string() };
    let generation_arg = args.get("generation");

    let collected = collect_elements(hwnd, &target_window, max_depth, max_results, include_patterns, true);
    if !collected.ok {
        // return error response like origin
        let h = collected.hwnd;
        let st = snapshot_state().lock().unwrap();
        let generation = st.generation;
        let payload = json!({
            "ok": false,
            "mode": "full",
            "generation": generation,
            "hwnd": h,
            "window_title": "",
            "error": collected.error,
            "counts": { "total": 0, "new": 0, "changed": 0, "removed": 0 },
            "elements": []
        });
        return ToolOut::Json(payload, false);
    }

    let hwnd = collected.hwnd;
    let current: Vec<Map<String, Value>> = collected.elements.iter().enumerate().map(|(idx, el)| {
        to_public_element(el, idx + 1, include_patterns)
    }).collect();

    let mut st = snapshot_state().lock().unwrap();
    let same_window = st.hwnd == hwnd && hwnd != 0;
    let generation_matches = match generation_arg {
        None => true,
        Some(v) => as_i64(v).map(|g| g == st.generation).unwrap_or(false),
    };
    let can_diff = !st.map.is_empty() && same_window && generation_matches;
    let mode = if requested == "full" || !can_diff { "full" } else { "diff" };

    st.generation += 1;
    st.hwnd = hwnd;
    st.title = collected.window_title.clone();
    st.order = current.iter().filter_map(|e| e.get("ref").and_then(as_str).map(|s| s.to_string())).collect();

    let mut response = if mode == "full" {
        json!({
            "ok": true,
            "mode": mode,
            "generation": st.generation,
            "hwnd": hwnd,
            "window_title": collected.window_title,
            "counts": { "total": current.len(), "new": current.len(), "changed": 0, "removed": 0 },
            "elements": current
        })
    } else {
        // diff
        let prev = st.map.clone();
        // diff elements
        let mut added = Vec::new();
        let mut changed = Vec::new();
        let mut removed = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for el in &current {
            let refstr = el.get("ref").and_then(as_str).unwrap_or("").to_string();
            if refstr.is_empty() { continue; }
            seen.insert(refstr.clone());
            if let Some(before) = prev.get(&refstr) {
                // changed fields
                let mut fields = Vec::new();
                let bn = before.get("name").and_then(as_str).unwrap_or("");
                let an = el.get("name").and_then(as_str).unwrap_or("");
                if bn != an { fields.push("name"); }
                let bb = before.get("bounds");
                let ab = el.get("bounds");
                if bb != ab { fields.push("bounds"); }
                let be = before.get("enabled").and_then(as_bool).unwrap_or(true);
                let ae = el.get("enabled").and_then(as_bool).unwrap_or(true);
                if be != ae { fields.push("enabled"); }
                if !fields.is_empty() {
                    let mut c = el.clone();
                    c.insert("change".into(), json!("changed"));
                    c.insert("changes".into(), json!(fields));
                    changed.push(Value::Object(c));
                }
            } else {
                let mut a = el.clone();
                a.insert("change".into(), json!("new"));
                added.push(Value::Object(a));
            }
        }
        for key in prev.keys() {
            if !seen.contains(key) { removed.push(Value::String(key.clone())); }
        }
        let mut elements = Vec::new();
        for a in &added { elements.push(a.clone()); }
        for c in &changed { elements.push(c.clone()); }
        let mut resp = json!({
            "ok": true,
            "mode": mode,
            "generation": st.generation,
            "hwnd": hwnd,
            "window_title": collected.window_title,
            "counts": { "total": current.len(), "new": added.len(), "changed": changed.len(), "removed": removed.len() },
            "elements": elements
        });
        if !removed.is_empty() {
            resp["removed"] = json!(removed);
        }
        resp
    };

    if collected.truncated {
        response["truncated"] = json!(true);
    }

    // store map
    let newmap: HashMap<String, Map<String, Value>> = current.iter().filter_map(|e| {
        e.get("ref").and_then(as_str).map(|r| (r.to_string(), e.clone()))
    }).collect();
    st.map = newmap;

    ToolOut::JsonStructured(response, false)
}

fn handle_state_fingerprint(args: &Value) -> ToolOut {
    let hwnd = args.get("hwnd").and_then(as_i64).unwrap_or(0);
    let target_window = args.get("target_window").and_then(as_str).unwrap_or("").to_string();
    let max_results = args.get("max_results").and_then(as_i64).unwrap_or(200);
    let collected = collect_elements(hwnd, &target_window, 30, max_results, false, false);
    if !collected.ok {
        return ToolOut::Json(json!({"ok": false, "error": collected.error}), true);
    }
    let fp = fingerprint_of(&collected.elements, &collected.window_title, collected.pid, collected.hwnd);
    *last_fp().lock().unwrap() = Some(fp.clone());
    let payload = json!({
        "ok": true,
        "fingerprint": fp,
        "window_title": collected.window_title,
        "element_count": collected.elements.len(),
        "ts": now_ms()
    });
    ToolOut::JsonStructured(payload, false)
}

fn handle_verify(args: &Value) -> ToolOut {
    let expect = args.get("expect").cloned().unwrap_or(json!({}));
    if !expect.is_object() || expect.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        return ToolOut::Json(json!({"ok": false, "error": "expect with at least one check is required"}), true);
    }
    let timeout_ms = args.get("timeout_ms").and_then(as_i64).unwrap_or(3000).clamp(200, 30000);
    let interval_ms = args.get("interval_ms").and_then(as_i64).unwrap_or(300).clamp(100, 5000);
    let hwnd = args.get("hwnd").and_then(as_i64).unwrap_or(0);
    let target_window = args.get("target_window").and_then(as_str).unwrap_or("").to_string();
    let max_results = args.get("max_results").and_then(as_i64).unwrap_or(200);

    let started = now_ms();
    let mut initial_fp: Option<String> = None;
    let mut current_fp: Option<String> = None;
    let mut window_title = String::new();
    let mut checks = json!([]);
    let mut collected_once = false;

    while now_ms() - started < timeout_ms {
        let collected = collect_elements(hwnd, &target_window, 30, max_results, false, false);
        if collected.ok {
            collected_once = true;
            window_title = collected.window_title.clone();
            let fp = fingerprint_of(&collected.elements, &window_title, collected.pid, collected.hwnd);
            if initial_fp.is_none() { initial_fp = Some(fp.clone()); }
            current_fp = Some(fp.clone());
            let prev_fp = last_fp().lock().unwrap().clone();
            let eval = evaluate_expectation(&expect, &collected.elements, &window_title, &prev_fp, &fp);
            checks = eval.checks;
            if eval.all_pass {
                *last_fp().lock().unwrap() = Some(fp.clone());
                let payload = json!({
                    "ok": true,
                    "status": "confirmed",
                    "fingerprint": fp,
                    "elapsed_ms": now_ms() - started,
                    "window_title": window_title,
                    "checks": checks
                });
                return ToolOut::JsonStructured(payload, false);
            }
        }
        // sleep
        std::thread::sleep(std::time::Duration::from_millis(interval_ms as u64));
    }
    let status = if !collected_once {
        "timeout"
    } else if current_fp != initial_fp {
        "unexpected"
    } else {
        "no_change"
    };
    if let Some(fp) = &current_fp {
        *last_fp().lock().unwrap() = Some(fp.clone());
    }
    let payload = json!({
        "ok": true,
        "status": status,
        "fingerprint": current_fp,
        "elapsed_ms": now_ms() - started,
        "window_title": window_title,
        "checks": checks
    });
    ToolOut::JsonStructured(payload, false)
}

fn evaluate_expectation(expect: &Value, elements: &[Map<String, Value>], window_title: &str, prev_fp: &Option<String>, current_fp: &str) -> EvalResult {
    let mut checks = Vec::new();
    let list = elements;
    let exp = expect.as_object().cloned().unwrap_or_default();

    if let Some(needle) = exp.get("window_title_contains").and_then(as_str) {
        let title = window_title;
        let pass = title.to_lowercase().contains(&needle.to_lowercase());
        checks.push(json!({"check":"window_title_contains","pass":pass,"detail":title}));
    }

    if let Some(q) = exp.get("element").and_then(|v| v.as_object()) {
        let matches: Vec<&Map<String, Value>> = list.iter().filter(|el| {
            if let Some(nm) = q.get("name").and_then(as_str) {
                let elname = el.get("name").and_then(as_str).unwrap_or("");
                if !elname.to_lowercase().contains(&nm.to_lowercase()) { return false; }
            }
            if let Some(aid) = q.get("automation_id").and_then(as_str) {
                let ela = el.get("automation_id").and_then(as_str).unwrap_or("");
                if ela != aid { return false; }
            }
            if let Some(ct) = q.get("control_type").and_then(as_str) {
                let elct = el.get("control_type").or(el.get("type")).and_then(as_str).unwrap_or("");
                if elct.to_lowercase() != ct.to_lowercase() { return false; }
            }
            true
        }).collect();
        let found = !matches.is_empty();
        let want_exists = q.get("exists").and_then(as_bool).unwrap_or(true);
        let pass = if want_exists { found } else { !found };
        checks.push(json!({"check":"element","pass":pass,"detail":{"exists":found,"matches":matches.len(),"wanted":want_exists}}));
    }

    if let Some(needle) = exp.get("text_visible").and_then(as_str) {
        let mut matches = 0;
        for el in list {
            let nm = el.get("name").and_then(as_str).unwrap_or("");
            if nm.to_lowercase().contains(&needle.to_lowercase()) { matches += 1; }
        }
        checks.push(json!({"check":"text_visible","pass": matches > 0, "detail":{"matches":matches,"source":"uia_only"}}));
    }

    if let Some(target) = exp.get("fingerprint_equals").and_then(as_str) {
        let pass = current_fp == target;
        checks.push(json!({"check":"fingerprint_equals","pass":pass,"detail":current_fp}));
    }

    if exp.get("fingerprint_changed_from").is_some() {
        let from = if let Some(s) = exp.get("fingerprint_changed_from").and_then(as_str) {
            s.to_string()
        } else {
            prev_fp.clone().unwrap_or_default()
        };
        let pass = !from.is_empty() && current_fp != from;
        checks.push(json!({"check":"fingerprint_changed_from","pass":pass,"detail":{"from": if from.is_empty() { Value::Null } else { json!(from) }, "to": current_fp}}));
    }

    let all_pass = !checks.is_empty() && checks.iter().all(|c| c.get("pass").and_then(as_bool).unwrap_or(false));
    EvalResult { checks: Value::Array(checks), all_pass }
}

struct EvalResult { checks: Value, all_pass: bool }

// ---------------- find_element / map_ui ----------------

fn foreground_hwnd() -> i64 {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        let fg = GetForegroundWindow();
        fg.0 as i64
    }
}

fn handle_find_element(args: &Value) -> ToolOut {
    let mut a = args.clone();
    if a.get("max_results").is_none() {
        a["max_results"] = json!(20);
    }
    handle_inspect_ui(&a)
}

fn handle_map_ui(args: &Value) -> ToolOut {
    let hwnd = args.get("hwnd").and_then(as_i64).unwrap_or(0);
    let hwnd = if hwnd > 0 { hwnd } else { foreground_hwnd() };
    let target_window = args.get("target_window").and_then(as_str).unwrap_or("").to_string();
    let collected = collect_elements(hwnd, &target_window, 12, 150, false, false);
    let mut elements = Vec::new();
    let mut id = 1i64;
    if collected.ok {
        let interactive_types = ["Button","Edit","MenuItem","CheckBox","RadioButton","TabItem","ComboBox","Hyperlink","Document","ListItem","TreeItem"];
        for el in &collected.elements {
            let ct = el.get("control_type").and_then(as_str).unwrap_or("");
            let off = el.get("offscreen").and_then(as_bool).unwrap_or(false);
            let w = el.get("width").and_then(as_i64).unwrap_or(0);
            let h = el.get("height").and_then(as_i64).unwrap_or(0);
            if off || w <= 5 || h <= 5 { continue; }
            if !interactive_types.contains(&ct) { continue; }
            let name = el.get("name").and_then(as_str).unwrap_or("(unnamed)").to_string();
            let name = if name.is_empty() { "(unnamed)".to_string() } else { name };
            let item = json!({
                "id": id,
                "type": ct,
                "name": name,
                "x": el.get("x").cloned().unwrap_or(json!(0)),
                "y": el.get("y").cloned().unwrap_or(json!(0)),
                "width": el.get("width").cloned().unwrap_or(json!(0)),
                "height": el.get("height").cloned().unwrap_or(json!(0))
            });
            elements.push(item);
            id += 1;
        }
    }
    ToolOut::Json(json!({"count": elements.len(), "elements": elements}), false)
}

// ---------------- find_template ----------------

fn capture_region_rgba(hwnd: i64) -> (i64, i64, image::RgbaImage) {
    unsafe {
        use windows::Win32::Foundation::RECT as R;
        use windows::Win32::Graphics::Gdi::*;
        use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, GetWindowRect};
        let target = if hwnd > 0 { windows::Win32::Foundation::HWND(hwnd as *mut _) } else { GetForegroundWindow() };
        let mut rect = R::default();
        let mut ox = 0i64;
        let mut oy = 0i64;
        let mut w = 0i64;
        let mut h = 0i64;
        if GetWindowRect(target, &mut rect).is_ok() {
            w = (rect.right - rect.left) as i64;
            h = (rect.bottom - rect.top) as i64;
            if w > 20 && h > 20 {
                ox = rect.left as i64;
                oy = rect.top as i64;
            } else {
                w = 0; h = 0;
            }
        }
        if w == 0 || h == 0 {
            w = GetSystemMetrics(SM_CXSCREEN) as i64;
            h = GetSystemMetrics(SM_CYSCREEN) as i64;
            ox = 0; oy = 0;
        }
        if w <= 0 || h <= 0 {
            w = 1920; h = 1080;
        }
        let screen_dc = GetDC(None);
        let mem_dc = CreateCompatibleDC(Some(screen_dc));
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let hbmp = CreateDIBSection(Some(mem_dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
        let old = SelectObject(mem_dc, hbmp.into());
        let _ = BitBlt(mem_dc, 0, 0, w as i32, h as i32, Some(screen_dc), ox as i32, oy as i32, SRCCOPY);
        let mut data = vec![0u8; (w * h * 4) as usize];
        if !bits.is_null() {
            std::ptr::copy_nonoverlapping(bits as *const u8, data.as_mut_ptr(), data.len());
        }
        let _ = SelectObject(mem_dc, old);
        let _ = DeleteObject(hbmp.into());
        let _ = DeleteDC(mem_dc);
        ReleaseDC(None, screen_dc);
        let img = image::RgbaImage::from_raw(w as u32, h as u32, data).unwrap_or_else(|| image::RgbaImage::new(1,1));
        (ox, oy, img)
    }
}

fn handle_find_template(args: &Value) -> ToolOut {
    let tmpl_b64 = args.get("template_b64").and_then(as_str).unwrap_or("").trim().to_string();
    if tmpl_b64.is_empty() {
        return ToolOut::Json(json!({"ok": false, "error": "template_b64 parameter is required"}), true);
    }
    let min_conf = args.get("min_confidence").and_then(|v| v.as_f64()).unwrap_or(0.75).clamp(0.0, 1.0);
    let hwnd = args.get("hwnd").and_then(as_i64).unwrap_or(0);

    use base64::Engine;
    let bytes = match base64::engine::general_purpose::STANDARD.decode(&tmpl_b64) {
        Ok(b) => b,
        Err(_) => return ToolOut::Json(json!({"ok": false, "error": "invalid base64"}), true),
    };
    let tmpl = match image::load_from_memory(&bytes) {
        Ok(d) => d.to_rgba8(),
        Err(_) => return ToolOut::Json(json!({"ok": false, "error": "invalid image"}), true),
    };
    let (ox, oy, screen) = capture_region_rgba(hwnd);

    let sw = screen.width() as usize;
    let sh = screen.height() as usize;
    let tw = tmpl.width() as usize;
    let th = tmpl.height() as usize;

    if tw > sw || th > sh || tw < 3 || th < 3 {
        let payload = json!({
            "ok": true, "found": false, "x": 0, "y": 0, "center_x": 0, "center_y": 0,
            "width": tmpl.width(), "height": tmpl.height(),
            "confidence": 0.0
        });
        return ToolOut::Json(payload, false);
    }

    let step: usize = if sw > 1920 { 2 } else { 1 };
    let mut best_x: i64 = -1;
    let mut best_y: i64 = -1;
    let mut min_diff: i64 = i64::MAX;

    for y in (0..=(sh - th)).step_by(step) {
        for x in (0..=(sw - tw)).step_by(step) {
            let mut diff: i64 = 0;
            for ty in (0..th).step_by(2) {
                for tx in (0..tw).step_by(2) {
                    let sp = screen.get_pixel((x + tx) as u32, (y + ty) as u32);
                    let tp = tmpl.get_pixel(tx as u32, ty as u32);
                    diff += (sp[0] as i64 - tp[0] as i64).abs();
                    diff += (sp[1] as i64 - tp[1] as i64).abs();
                    diff += (sp[2] as i64 - tp[2] as i64).abs();
                }
            }
            if diff < min_diff {
                min_diff = diff;
                best_x = x as i64;
                best_y = y as i64;
                if min_diff == 0 { break; }
            }
        }
        if min_diff == 0 { break; }
    }

    let sample_count = ((tw as f64 / 2.0).ceil() * (th as f64 / 2.0).ceil()) as i64;
    let max_diff = sample_count * 255 * 3;
    let conf = if max_diff == 0 { 1.0 } else { (1.0 - (min_diff as f64 / max_diff as f64)).max(0.0) };

    let found = best_x >= 0 && conf >= min_conf;
    let (fx, fy) = if found { (best_x + ox, best_y + oy) } else { (0i64, 0i64) };
    let (cx, cy) = if found { (best_x + ox + (tw as i64 / 2), best_y + oy + (th as i64 / 2)) } else { (0i64, 0i64) };
    let conf_pct = (conf * 100.0).round() as i32;
    let confidence = conf_pct as f64 / 100.0;
    let payload = json!({
        "ok": true,
        "found": found,
        "x": fx,
        "y": fy,
        "center_x": cx,
        "center_y": cy,
        "width": tmpl.width(),
        "height": tmpl.height(),
        "confidence": confidence
    });
    ToolOut::Json(payload, false)
}

// ---------------- lib entry ----------------

pub fn handles(name: &str) -> bool {
    matches!(name,
        "computer_inspect_ui" | "computer_snapshot" | "computer_state_fingerprint" | "computer_verify" |
        "computer_find_element" | "computer_map_ui" | "computer_find_template"
    )
}

// accepts: subconjunto de args servible nativo (default true; el puente consulta esto).
pub fn accepts(name: &str, args: &Value) -> bool {
    match name {
        "computer_find_element" => {
            // rama harness remoto si no hay criterios locales
            let has = args.get("title").is_some() || args.get("hwnd").is_some()
                || args.get("automation_id").is_some() || args.get("control_type").is_some()
                || args.get("name").is_some();
            has
        }
        "computer_wait_stable" | "computer_read_selection" | "computer_observe" => false,
        "computer_find_template" => args.get("template_b64").and_then(|v| v.as_str()).map(|s| !s.trim().is_empty()).unwrap_or(false),
        _ => true,
    }
}

pub fn handle_call(name: &str, args: &Value) -> ToolOut {
    match name {
        "computer_inspect_ui" => handle_inspect_ui(args),
        "computer_snapshot" => handle_snapshot(args),
        "computer_state_fingerprint" => handle_state_fingerprint(args),
        "computer_verify" => handle_verify(args),
        "computer_find_element" => handle_find_element(args),
        "computer_map_ui" => handle_map_ui(args),
        "computer_find_template" => handle_find_template(args),
        _ => ToolOut::Json(json!({"ok": false, "error": "not_implemented"}), true),
    }
}

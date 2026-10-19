// cu-uia — capa UIA nativa (F6). Reglas: migration/RULEBOOK.md.
// Contrato del puente: handles(name) + handle_call(name, args) -> cu_core::out::ToolOut.
use cu_core::out::ToolOut;
use serde_json::{Map, Value, json};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationSelectionItemPattern,
    IUIAutomationTreeWalker, IUIAutomationValuePattern, UIA_ExpandCollapsePatternId,
    UIA_InvokePatternId, UIA_SelectionItemPatternId, UIA_TogglePatternId, UIA_ValuePatternId,
};

// ---------------- helpers ----------------

fn as_str(v: &Value) -> Option<&str> {
    v.as_str()
}
fn as_i64(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        return Some(n);
    }
    if let Some(n) = v.as_f64() {
        return Some(n as i64);
    }
    if let Some(s) = v.as_str() {
        return s.trim().parse::<i64>().ok();
    }
    None
}
fn as_bool(v: &Value) -> Option<bool> {
    v.as_bool()
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn bstr_to_string(b: windows::core::BSTR) -> String {
    b.to_string()
}

fn control_type_name(id: i32) -> String {
    // mapeo UIA_CONTROLTYPE_ID -> nombre corto (como .NET ProgrammaticName sin "ControlType.")
    let pairs: &[(i32, &str)] = &[
        (50032, "Window"),
        (50040, "AppBar"),
        (50000, "Button"),
        (50001, "Calendar"),
        (50002, "CheckBox"),
        (50003, "ComboBox"),
        (50025, "Custom"),
        (50028, "DataGrid"),
        (50029, "DataItem"),
        (50030, "Document"),
        (50004, "Edit"),
        (50026, "Group"),
        (50034, "Header"),
        (50035, "HeaderItem"),
        (50005, "Hyperlink"),
        (50006, "Image"),
        (50008, "List"),
        (50007, "ListItem"),
        (50010, "MenuBar"),
        (50009, "Menu"),
        (50011, "MenuItem"),
        (50033, "Pane"),
        (50012, "ProgressBar"),
        (50013, "RadioButton"),
        (50014, "ScrollBar"),
        (50038, "Separator"),
        (50015, "Slider"),
        (50016, "Spinner"),
        (50031, "SplitButton"),
        (50017, "StatusBar"),
        (50018, "Tab"),
        (50019, "TabItem"),
        (50036, "Table"),
        (50020, "Text"),
        (50027, "Thumb"),
        (50037, "TitleBar"),
        (50021, "ToolBar"),
        (50022, "ToolTip"),
        (50023, "Tree"),
        (50024, "TreeItem"),
    ];
    for (k, v) in pairs {
        if *k == id {
            return v.to_string();
        }
    }
    format!("Unknown({})", id)
}

// ---------------- UIA automation ----------------

fn init_com() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

fn create_automation() -> Option<IUIAutomation> {
    init_com();
    unsafe {
        unsafe {
            CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()
        }
    }
}

fn root_element(
    auto: &IUIAutomation,
    hwnd: i64,
    target_window: &str,
) -> Option<IUIAutomationElement> {
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
                    let aliases: &[(&str, &[&str])] = &[
                        ("notepad", &["bloc de notas", "notepad"]),
                        ("calculator", &["calculadora", "calc", "calculator"]),
                        ("calc", &["calculadora", "calculator", "calc"]),
                        ("paint", &["paint", "mspaint"]),
                        (
                            "terminal",
                            &["powershell", "cmd", "terminal", "símbolo del sistema"],
                        ),
                        ("explorer", &["explorador de archivos", "explorer"]),
                        ("settings", &["configuración", "ajustes", "settings"]),
                    ];
                    let mut terms = vec![target_window.to_string()];
                    for (k, vals) in aliases {
                        if needle.contains(k) {
                            for v in *vals {
                                if !terms.iter().any(|t| t.eq_ignore_ascii_case(v)) {
                                    terms.push(v.to_string());
                                }
                            }
                        }
                    }
                    while let Some(el) = child {
                        let name = el.CurrentName().map(bstr_to_string).unwrap_or_default();
                        let aid = el
                            .CurrentAutomationId()
                            .map(bstr_to_string)
                            .unwrap_or_default();
                        let pid = el.CurrentProcessId().unwrap_or(0);
                        let pname = process_name(pid as u32);
                        for t in &terms {
                            let tl = t.to_lowercase();
                            if (!name.is_empty() && name.to_lowercase().contains(&tl))
                                || (!aid.is_empty() && aid.to_lowercase().contains(&tl))
                                || (!pname.is_empty() && pname.to_lowercase().contains(&tl))
                            {
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
                if let Ok(el) = auto.ElementFromHandle(fg) {
                    return Some(el);
                }
            }
        }
        auto.GetFocusedElement().ok()
    }
}

fn process_name(pid: u32) -> String {
    // via windows toolhelp
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    unsafe {
        let snap = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(_) => return String::new(),
        };
        let mut pe = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut pe).is_ok() {
            loop {
                if pe.th32ProcessID == pid {
                    let name = String::from_utf16_lossy(&pe.szExeFile);
                    let name = name.trim_end_matches('\0').to_string();
                    let _ = windows::Win32::Foundation::CloseHandle(snap);
                    return name;
                }
                if Process32NextW(snap, &mut pe).is_err() {
                    break;
                }
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
    for kw in [
        "password",
        "passcode",
        "secret",
        "pin",
        "one-time code",
        "verification code",
        "otp",
    ] {
        if low.contains(kw) {
            return true;
        }
    }
    // pin\b : check word boundary
    if low
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| w == "pin")
    {
        return true;
    }
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
    let auto = match create_automation() {
        Some(a) => a,
        None => {
            return CollectResult {
                ok: false,
                error: "uia_init_failed".into(),
                window_title: String::new(),
                pid: 0,
                hwnd,
                truncated: false,
                elements: vec![],
            };
        }
    };
    let root = match root_element(&auto, hwnd, target_window) {
        Some(r) => r,
        None => {
            return CollectResult {
                ok: false,
                error: "window_not_found".into(),
                window_title: String::new(),
                pid: 0,
                hwnd,
                truncated: false,
                elements: vec![],
            };
        }
    };

    // walker
    let walker = match unsafe { auto.ControlViewWalker() } {
        Ok(w) => w,
        Err(_) => {
            return CollectResult {
                ok: false,
                error: "walker_failed".into(),
                window_title: String::new(),
                pid: 0,
                hwnd,
                truncated: false,
                elements: vec![],
            };
        }
    };

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
        if items.len() >= max_results {
            *truncated = true;
            return;
        }
        let idx = items.len() as i64;
        // build node
        let mut node: Map<String, Value> = Map::new();
        let c_type_id = unsafe { el.CurrentControlType().map(|v| v.0).unwrap_or(0) };
        let type_short = control_type_name(c_type_id);
        let name = unsafe { el.CurrentName().map(bstr_to_string).unwrap_or_default() };
        let aid = unsafe {
            el.CurrentAutomationId()
                .map(bstr_to_string)
                .unwrap_or_default()
        };
        let class_name = unsafe {
            el.CurrentClassName()
                .map(bstr_to_string)
                .unwrap_or_default()
        };
        let rect = unsafe {
            el.CurrentBoundingRectangle().unwrap_or(RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            })
        };
        let enabled = unsafe { el.CurrentIsEnabled().map(|b| b.as_bool()).unwrap_or(true) };
        let offscreen = unsafe {
            el.CurrentIsOffscreen()
                .map(|b| b.as_bool())
                .unwrap_or(false)
        };
        let native_hwnd = unsafe {
            el.CurrentNativeWindowHandle()
                .map(|h| h.0 as i64)
                .unwrap_or(0)
        };
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
            if is_sensitive {
                node.insert("name".into(), json!("[redacted]"));
            } else {
                node.insert("name".into(), json!(name));
            }
        }
        if !aid.is_empty() {
            node.insert("automation_id".into(), json!(aid));
        }
        if !class_name.is_empty() {
            node.insert("class_name".into(), json!(class_name));
        }
        if offscreen {
            node.insert("offscreen".into(), json!(true));
        }
        if is_sensitive {
            node.insert("is_password".into(), json!(true));
        }
        if native_hwnd > 0 {
            node.insert("hwnd".into(), json!(native_hwnd));
        }

        if include_patterns {
            let mut pats: Vec<&str> = Vec::new();
            unsafe {
                if el.GetCurrentPattern(UIA_InvokePatternId).is_ok() {
                    pats.push("invoke");
                }
                if el.GetCurrentPattern(UIA_ValuePatternId).is_ok() {
                    pats.push("value");
                }
                if el.GetCurrentPattern(UIA_TogglePatternId).is_ok() {
                    pats.push("toggle");
                }
                if el.GetCurrentPattern(UIA_ExpandCollapsePatternId).is_ok() {
                    pats.push("expand");
                }
                if el.GetCurrentPattern(UIA_SelectionItemPatternId).is_ok() {
                    pats.push("select");
                }
            }
            if !pats.is_empty() {
                node.insert("patterns".into(), json!(pats));
            }
        }

        // value + focused for Edit/ComboBox/Document
        if type_short == "Edit" || type_short == "ComboBox" || type_short == "Document" {
            if is_sensitive {
                node.insert("value".into(), json!("[redacted]"));
            } else {
                unsafe {
                    if let Ok(vp) =
                        el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                    {
                        if let Ok(v) = vp.CurrentValue() {
                            let val = bstr_to_string(v);
                            if !val.is_empty() {
                                let val = if val.len() > 500 {
                                    val.chars().take(500).collect::<String>()
                                } else {
                                    val
                                };
                                node.insert("value".into(), json!(val));
                            }
                        }
                    }
                }
            }
            // focused: .NET c.IsFocused; IUIAutomation element has CurrentHasKeyboardFocus
            unsafe {
                if let Ok(b) = el.CurrentHasKeyboardFocus() {
                    if b.as_bool() {
                        node.insert("focused".into(), json!(true));
                    }
                }
            }
        }

        items.push(node);

        // subtree suppression
        if (type_short == "Button"
            || type_short == "CheckBox"
            || type_short == "RadioButton"
            || type_short == "Hyperlink")
            && !name.is_empty()
        {
            return;
        }
        if depth >= max_depth {
            return;
        }

        // tab compression
        if type_short == "Tab" || type_short == "ToolBar" || type_short == "Custom" {
            // collect child tabs
            let mut tabs: Vec<IUIAutomationElement> = Vec::new();
            unsafe {
                let mut ch = walker.GetFirstChildElement(el).ok();
                while let Some(c) = ch {
                    let ct = c.CurrentControlType().map(|v| v.0).unwrap_or(0);
                    if control_type_name(ct) == "TabItem" {
                        tabs.push(c.clone());
                    }
                    ch = walker.GetNextSiblingElement(&c).ok();
                }
            }
            if tabs.len() >= 10 {
                // active tab
                let mut active: Option<IUIAutomationElement> = None;
                for t in &tabs {
                    unsafe {
                        if let Ok(sp) = t.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                            UIA_SelectionItemPatternId,
                        ) {
                            if let Ok(sel) = sp.CurrentIsSelected() {
                                if sel.as_bool() {
                                    active = Some(t.clone());
                                    break;
                                }
                            }
                        }
                    }
                }
                if active.is_none() && !tabs.is_empty() {
                    active = Some(tabs[0].clone());
                }
                if let Some(at) = active {
                    visit(
                        walker,
                        &at,
                        depth + 1,
                        idx,
                        max_depth,
                        max_results,
                        include_patterns,
                        items,
                        truncated,
                    );
                    // omitted summary
                    if items.len() < max_results {
                        let omitted = tabs.len() - 1;
                        let mut onode: Map<String, Value> = Map::new();
                        onode.insert("index".into(), json!(items.len()));
                        onode.insert("parent".into(), json!(idx));
                        onode.insert("depth".into(), json!(depth + 1));
                        onode.insert("control_type".into(), json!("TabItem"));
                        onode.insert(
                            "name".into(),
                            json!(format!("... ({} inactive browser tabs omitted)", omitted)),
                        );
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
                if items.len() >= max_results {
                    *truncated = true;
                    break;
                }
                visit(
                    walker,
                    &ch,
                    depth + 1,
                    idx,
                    max_depth,
                    max_results,
                    include_patterns,
                    items,
                    truncated,
                );
                child = walker.GetNextSiblingElement(&ch).ok();
            }
        }
    }

    // Need import IUIAutomationSelectionItemPattern? We used fully qualified maybe. Let's ensure imported. We'll add import.
    unsafe {
        visit(
            &walker,
            &root,
            0,
            -1,
            max_depth,
            max_results as usize,
            include_patterns,
            &mut items,
            &mut truncated,
        );
    }

    // window info
    let mut window_title = String::new();
    let mut proc_id: i64 = 0;
    let mut root_hwnd: i64 = hwnd;
    unsafe {
        if let Ok(rc) = root.CurrentName() {
            window_title = bstr_to_string(rc);
        }
        if let Ok(p) = root.CurrentProcessId() {
            proc_id = p as i64;
        }
        if let Ok(h) = root.CurrentNativeWindowHandle() {
            if h.0 as i64 != 0 {
                root_hwnd = h.0 as i64;
            }
        }
    }
    if root_hwnd == 0 {
        // hwnd param
        root_hwnd = hwnd;
    }

    // index element tree
    let indexed = index_element_tree(items);

    // filter interactive if requested
    let filtered = if interactive_only {
        indexed
            .into_iter()
            .filter(|el| {
                let ct = el.get("control_type").and_then(as_str).unwrap_or("");
                let off = el.get("offscreen").and_then(as_bool).unwrap_or(false);
                let w = el.get("width").and_then(as_i64).unwrap_or(0);
                let h = el.get("height").and_then(as_i64).unwrap_or(0);
                const INTER: &[&str] = &[
                    "Button",
                    "Edit",
                    "MenuItem",
                    "CheckBox",
                    "RadioButton",
                    "TabItem",
                    "ComboBox",
                    "Hyperlink",
                    "Document",
                    "ListItem",
                    "TreeItem",
                ];
                INTER.contains(&ct) && !off && w > 5 && h > 5
            })
            .collect()
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
    let ct = el
        .get("control_type")
        .or(el.get("type"))
        .and_then(as_str)
        .unwrap_or("");
    let id = el.get("automation_id").and_then(as_str).unwrap_or("");
    let name = el.get("name").and_then(as_str).unwrap_or("");
    if ct.is_empty() && id.is_empty() && name.is_empty() {
        let idx = el
            .get("index")
            .or(el.get("i"))
            .or(el.get("sibling_index"))
            .and_then(as_i64)
            .unwrap_or(0);
        return format!("#{}", idx);
    }
    let sibling = el.get("sibling_index").and_then(as_i64).unwrap_or(0);
    format!(
        "{}:{}:{}#{}",
        if ct.is_empty() { "?" } else { ct },
        id,
        name,
        sibling
    )
}

fn compute_ref(el: &Map<String, Value>) -> String {
    let ancestors = el
        .get("ancestors")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut parts: Vec<String> = Vec::new();
    for a in &ancestors {
        if let Some(s) = a.as_str() {
            parts.push(s.to_string());
        } else if let Some(obj) = a.as_object() {
            parts.push(identity_segment(obj));
        }
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
        let parent_idx = if parent >= 0 && (parent as usize) < i {
            parent
        } else {
            -1
        };
        let parent_node = if parent_idx >= 0 {
            Some(&stored[parent_idx as usize])
        } else {
            None
        };
        let ct = el
            .get("control_type")
            .and_then(as_str)
            .unwrap_or("")
            .to_string();
        let aid = el
            .get("automation_id")
            .and_then(as_str)
            .unwrap_or("")
            .to_string();
        let name = el.get("name").and_then(as_str).unwrap_or("").to_string();
        let key = format!("{}|{}|{}", ct, aid, name);
        let counter_key = format!("{}|{}", parent_idx, key);
        let sibling_index = *counters.get(&counter_key).unwrap_or(&0);
        counters.insert(counter_key, sibling_index + 1);

        let mut node = el.clone();
        node.insert("parent".into(), json!(parent_idx));
        node.insert("sibling_index".into(), json!(sibling_index));

        let ancestors_val = if let Some(pn) = parent_node {
            let mut anc = pn
                .get("ancestors")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
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
        return arr
            .iter()
            .map(|x| x.to_string().trim_matches('"').to_string())
            .collect::<Vec<_>>()
            .join(",");
    }
    let x = el.get("x").and_then(as_i64).unwrap_or(0);
    let y = el.get("y").and_then(as_i64).unwrap_or(0);
    let w = el
        .get("width")
        .or(el.get("w"))
        .and_then(as_i64)
        .unwrap_or(0);
    let h = el
        .get("height")
        .or(el.get("h"))
        .and_then(as_i64)
        .unwrap_or(0);
    format!("{},{},{},{}", x, y, w, h)
}

fn sha1_hex(data: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(data);
    let res = h.finalize();
    res.iter().map(|b| format!("{:02x}", b)).collect()
}

fn fingerprint_of(
    elements: &[Map<String, Value>],
    window_title: &str,
    pid: i64,
    hwnd: i64,
) -> String {
    let mut rows: Vec<String> = elements
        .iter()
        .map(|el| {
            let r = el
                .get("ref")
                .or(el.get("i"))
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string().trim_matches('"').to_string(),
                })
                .unwrap_or_default();
            let name = el.get("name").and_then(as_str).unwrap_or("");
            let enabled = if el.get("enabled").and_then(as_bool) == Some(false)
                || el.get("disabled").and_then(as_bool) == Some(true)
            {
                "0"
            } else {
                "1"
            };
            let b = bounds_of_str(el);
            format!("{}|{}|{}|{}", r, name, b, enabled)
        })
        .collect();
    rows.sort();
    let payload = format!(
        "t:{}\np:{}\nh:{}\n{}",
        window_title,
        pid,
        hwnd,
        rows.join("\n")
    );
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

fn lock_snapshot_state() -> std::sync::MutexGuard<'static, SnapshotState> {
    match snapshot_state().lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    }
}

fn lock_last_fp() -> std::sync::MutexGuard<'static, Option<String>> {
    match last_fp().lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    }
}

// ---------------- to_public_element ----------------

fn to_public_element(
    el: &Map<String, Value>,
    i: usize,
    include_patterns: bool,
) -> Map<String, Value> {
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
        if !name.is_empty() {
            pubm.insert("name".into(), json!(name));
        }
    }
    if let Some(aid) = el.get("automation_id").and_then(as_str) {
        if !aid.is_empty() {
            pubm.insert("automation_id".into(), json!(aid));
        }
    }
    if include_patterns {
        if let Some(pats) = el.get("patterns").and_then(|v| v.as_array()) {
            if !pats.is_empty() {
                pubm.insert("patterns".into(), Value::Array(pats.clone()));
            }
        }
    }
    pubm
}

// ---------------- handlers ----------------

fn handle_inspect_ui(args: &Value) -> ToolOut {
    let hwnd = args.get("hwnd").and_then(as_i64).unwrap_or(0);
    let title = args
        .get("title")
        .or(args.get("target_window"))
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    let name_need = args.get("name").and_then(as_str).unwrap_or("").to_string();
    let id_need = args
        .get("automation_id")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    let type_need = args
        .get("control_type")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
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
            if !c_name.to_lowercase().contains(&name_need.to_lowercase()) {
                continue;
            }
        }
        if !id_need.is_empty() && c_aid != id_need {
            continue;
        }
        if !type_need.is_empty() {
            if type_need != type_short && type_need != c_type {
                continue;
            }
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
        if !c_name.is_empty() {
            node.insert("name".into(), json!(c_name));
        }
        if !c_aid.is_empty() {
            node.insert("automation_id".into(), json!(c_aid));
        }
        if let Some(cn) = el.get("class_name").and_then(as_str) {
            if !cn.is_empty() {
                node.insert("class_name".into(), json!(cn));
            }
        }
        if el.get("hwnd").is_some() {
            node.insert(
                "hwnd".into(),
                el.get("hwnd").cloned().unwrap_or(Value::Null),
            );
        }
        if el.get("disabled").and_then(as_bool) == Some(true) {}
        if el.get("enabled").and_then(as_bool) == Some(false) {
            node.insert("disabled".into(), json!(true));
        }
        if el.get("offscreen").and_then(as_bool) == Some(true) {
            node.insert("offscreen".into(), json!(true));
        }
        if let Some(pats) = el.get("patterns") {
            if pats.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
                node.insert("patterns".into(), pats.clone());
            }
        }
        items.push(node);
        if items.len() >= max_results as usize {
            break;
        }
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
    let target_window = args
        .get("target_window")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    let max_depth = args.get("max_depth").and_then(as_i64).unwrap_or(12);
    let max_results = args.get("max_results").and_then(as_i64).unwrap_or(200);
    let include_patterns = args
        .get("include_patterns")
        .and_then(as_bool)
        .unwrap_or(false);
    let mode_req = args
        .get("mode")
        .and_then(as_str)
        .unwrap_or("auto")
        .to_string();
    let requested = if ["auto", "full", "diff"].contains(&mode_req.as_str()) {
        mode_req
    } else {
        "auto".to_string()
    };
    let generation_arg = args.get("generation");

    let collected = collect_elements(
        hwnd,
        &target_window,
        max_depth,
        max_results,
        include_patterns,
        true,
    );
    if !collected.ok {
        // return error response like origin
        let h = collected.hwnd;
        let st = lock_snapshot_state();
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
    let current: Vec<Map<String, Value>> = collected
        .elements
        .iter()
        .enumerate()
        .map(|(idx, el)| to_public_element(el, idx + 1, include_patterns))
        .collect();

    let mut st = lock_snapshot_state();
    let same_window = st.hwnd == hwnd && hwnd != 0;
    let generation_matches = match generation_arg {
        None => true,
        Some(v) => as_i64(v).map(|g| g == st.generation).unwrap_or(false),
    };
    let can_diff = !st.map.is_empty() && same_window && generation_matches;
    let mode = if requested == "full" || !can_diff {
        "full"
    } else {
        "diff"
    };

    st.generation += 1;
    st.hwnd = hwnd;
    st.title = collected.window_title.clone();
    st.order = current
        .iter()
        .filter_map(|e| e.get("ref").and_then(as_str).map(|s| s.to_string()))
        .collect();

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
            if refstr.is_empty() {
                continue;
            }
            seen.insert(refstr.clone());
            if let Some(before) = prev.get(&refstr) {
                // changed fields
                let mut fields = Vec::new();
                let bn = before.get("name").and_then(as_str).unwrap_or("");
                let an = el.get("name").and_then(as_str).unwrap_or("");
                if bn != an {
                    fields.push("name");
                }
                let bb = before.get("bounds");
                let ab = el.get("bounds");
                if bb != ab {
                    fields.push("bounds");
                }
                let be = before.get("enabled").and_then(as_bool).unwrap_or(true);
                let ae = el.get("enabled").and_then(as_bool).unwrap_or(true);
                if be != ae {
                    fields.push("enabled");
                }
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
            if !seen.contains(key) {
                removed.push(Value::String(key.clone()));
            }
        }
        let mut elements = Vec::new();
        for a in &added {
            elements.push(a.clone());
        }
        for c in &changed {
            elements.push(c.clone());
        }
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
    let newmap: HashMap<String, Map<String, Value>> = current
        .iter()
        .filter_map(|e| {
            e.get("ref")
                .and_then(as_str)
                .map(|r| (r.to_string(), e.clone()))
        })
        .collect();
    st.map = newmap;

    ToolOut::JsonStructured(response, false)
}

fn handle_state_fingerprint(args: &Value) -> ToolOut {
    let hwnd = args.get("hwnd").and_then(as_i64).unwrap_or(0);
    let target_window = args
        .get("target_window")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    let max_results = args.get("max_results").and_then(as_i64).unwrap_or(200);
    let collected = collect_elements(hwnd, &target_window, 30, max_results, false, false);
    if !collected.ok {
        return ToolOut::Json(json!({"ok": false, "error": collected.error}), true);
    }
    let fp = fingerprint_of(
        &collected.elements,
        &collected.window_title,
        collected.pid,
        collected.hwnd,
    );
    *lock_last_fp() = Some(fp.clone());
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
        return ToolOut::Json(
            json!({"ok": false, "error": "expect with at least one check is required"}),
            true,
        );
    }
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(as_i64)
        .unwrap_or(3000)
        .clamp(200, 30000);
    let interval_ms = args
        .get("interval_ms")
        .and_then(as_i64)
        .unwrap_or(300)
        .clamp(100, 5000);
    let hwnd = args.get("hwnd").and_then(as_i64).unwrap_or(0);
    let target_window = args
        .get("target_window")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
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
            let fp = fingerprint_of(
                &collected.elements,
                &window_title,
                collected.pid,
                collected.hwnd,
            );
            if initial_fp.is_none() {
                initial_fp = Some(fp.clone());
            }
            current_fp = Some(fp.clone());
            let prev_fp = lock_last_fp().clone();
            let eval =
                evaluate_expectation(&expect, &collected.elements, &window_title, &prev_fp, &fp);
            checks = eval.checks;
            if eval.all_pass {
                *lock_last_fp() = Some(fp.clone());
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
        *lock_last_fp() = Some(fp.clone());
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

fn evaluate_expectation(
    expect: &Value,
    elements: &[Map<String, Value>],
    window_title: &str,
    prev_fp: &Option<String>,
    current_fp: &str,
) -> EvalResult {
    let mut checks = Vec::new();
    let list = elements;
    let exp = expect.as_object().cloned().unwrap_or_default();

    if let Some(needle) = exp.get("window_title_contains").and_then(as_str) {
        let title = window_title;
        let pass = title.to_lowercase().contains(&needle.to_lowercase());
        checks.push(json!({"check":"window_title_contains","pass":pass,"detail":title}));
    }

    if let Some(q) = exp.get("element").and_then(|v| v.as_object()) {
        let matches: Vec<&Map<String, Value>> = list
            .iter()
            .filter(|el| {
                if let Some(nm) = q.get("name").and_then(as_str) {
                    let elname = el.get("name").and_then(as_str).unwrap_or("");
                    if !elname.to_lowercase().contains(&nm.to_lowercase()) {
                        return false;
                    }
                }
                if let Some(aid) = q.get("automation_id").and_then(as_str) {
                    let ela = el.get("automation_id").and_then(as_str).unwrap_or("");
                    if ela != aid {
                        return false;
                    }
                }
                if let Some(ct) = q.get("control_type").and_then(as_str) {
                    let elct = el
                        .get("control_type")
                        .or(el.get("type"))
                        .and_then(as_str)
                        .unwrap_or("");
                    if elct.to_lowercase() != ct.to_lowercase() {
                        return false;
                    }
                }
                true
            })
            .collect();
        let found = !matches.is_empty();
        let want_exists = q.get("exists").and_then(as_bool).unwrap_or(true);
        let pass = if want_exists { found } else { !found };
        checks.push(json!({"check":"element","pass":pass,"detail":{"exists":found,"matches":matches.len(),"wanted":want_exists}}));
    }

    if let Some(needle) = exp.get("text_visible").and_then(as_str) {
        let mut matches = 0;
        for el in list {
            let nm = el.get("name").and_then(as_str).unwrap_or("");
            if nm.to_lowercase().contains(&needle.to_lowercase()) {
                matches += 1;
            }
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

    let all_pass = !checks.is_empty()
        && checks
            .iter()
            .all(|c| c.get("pass").and_then(as_bool).unwrap_or(false));
    EvalResult {
        checks: Value::Array(checks),
        all_pass,
    }
}

struct EvalResult {
    checks: Value,
    all_pass: bool,
}

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
    let target_window = args
        .get("target_window")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    let collected = collect_elements(hwnd, &target_window, 12, 150, false, false);
    let mut elements = Vec::new();
    let mut id = 1i64;
    if collected.ok {
        let interactive_types = [
            "Button",
            "Edit",
            "MenuItem",
            "CheckBox",
            "RadioButton",
            "TabItem",
            "ComboBox",
            "Hyperlink",
            "Document",
            "ListItem",
            "TreeItem",
        ];
        for el in &collected.elements {
            let ct = el.get("control_type").and_then(as_str).unwrap_or("");
            let off = el.get("offscreen").and_then(as_bool).unwrap_or(false);
            let w = el.get("width").and_then(as_i64).unwrap_or(0);
            let h = el.get("height").and_then(as_i64).unwrap_or(0);
            if off || w <= 5 || h <= 5 {
                continue;
            }
            if !interactive_types.contains(&ct) {
                continue;
            }
            let name = el
                .get("name")
                .and_then(as_str)
                .unwrap_or("(unnamed)")
                .to_string();
            let name = if name.is_empty() {
                "(unnamed)".to_string()
            } else {
                name
            };
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
    ToolOut::Json(
        json!({"count": elements.len(), "elements": elements}),
        false,
    )
}

// ---------------- find_template ----------------

fn capture_region_rgba(hwnd: i64) -> Result<(i64, i64, image::RgbaImage), String> {
    unsafe {
        use windows::Win32::Foundation::RECT as R;
        use windows::Win32::Graphics::Gdi::*;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetSystemMetrics, GetWindowRect, SM_CXSCREEN, SM_CYSCREEN,
        };
        let target = if hwnd > 0 {
            windows::Win32::Foundation::HWND(hwnd as *mut _)
        } else {
            GetForegroundWindow()
        };
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
                w = 0;
                h = 0;
            }
        }
        if w == 0 || h == 0 {
            w = GetSystemMetrics(SM_CXSCREEN) as i64;
            h = GetSystemMetrics(SM_CYSCREEN) as i64;
            ox = 0;
            oy = 0;
        }
        if w <= 0 || h <= 0 {
            return Err("invalid_capture_dimensions".into());
        }

        let screen_dc = GetDC(None);
        if screen_dc.0.is_null() {
            return Err("get_dc_failed".into());
        }
        let mem_dc = CreateCompatibleDC(Some(screen_dc));
        if mem_dc.0.is_null() {
            ReleaseDC(None, screen_dc);
            return Err("create_compatible_dc_failed".into());
        }

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
        let hbmp = match CreateDIBSection(Some(mem_dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bmp) => bmp,
            Err(_) => {
                let _ = DeleteDC(mem_dc);
                ReleaseDC(None, screen_dc);
                return Err("create_dib_section_failed".into());
            }
        };
        let old = SelectObject(mem_dc, hbmp.into());
        if old.0.is_null() {
            let _ = DeleteObject(hbmp.into());
            let _ = DeleteDC(mem_dc);
            ReleaseDC(None, screen_dc);
            return Err("select_object_failed".into());
        }

        let blt_ok = BitBlt(
            mem_dc,
            0,
            0,
            w as i32,
            h as i32,
            Some(screen_dc),
            ox as i32,
            oy as i32,
            SRCCOPY,
        );

        let mut data = Vec::new();
        if blt_ok.is_ok() && !bits.is_null() {
            let len = (w as usize)
                .checked_mul(h as usize)
                .and_then(|v| v.checked_mul(4))
                .unwrap_or(0);
            if len > 0 {
                data = vec![0u8; len];
                std::ptr::copy_nonoverlapping(bits as *const u8, data.as_mut_ptr(), len);
            }
        }

        let _ = SelectObject(mem_dc, old);
        let _ = DeleteObject(hbmp.into());
        let _ = DeleteDC(mem_dc);
        ReleaseDC(None, screen_dc);

        if blt_ok.is_err() {
            return Err("bitblt_failed".into());
        }
        if data.is_empty() {
            return Err("capture_data_empty".into());
        }
        let img = image::RgbaImage::from_raw(w as u32, h as u32, data)
            .unwrap_or_else(|| image::RgbaImage::new(1, 1));
        Ok((ox, oy, img))
    }
}

fn handle_find_template(args: &Value) -> ToolOut {
    let tmpl_b64 = args
        .get("template_b64")
        .and_then(as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if tmpl_b64.is_empty() {
        return ToolOut::Json(
            json!({"ok": false, "error": "template_b64 parameter is required"}),
            true,
        );
    }
    let min_conf = args
        .get("min_confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.75)
        .clamp(0.0, 1.0);
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
    let (ox, oy, screen) = match capture_region_rgba(hwnd) {
        Ok(v) => v,
        Err(e) => return ToolOut::Json(json!({"ok": false, "error": e, "message": e}), true),
    };

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
                if min_diff == 0 {
                    break;
                }
            }
        }
        if min_diff == 0 {
            break;
        }
    }

    let sample_count = ((tw as f64 / 2.0).ceil() * (th as f64 / 2.0).ceil()) as i64;
    let max_diff = sample_count * 255 * 3;
    let conf = if max_diff == 0 {
        1.0
    } else {
        (1.0 - (min_diff as f64 / max_diff as f64)).max(0.0)
    };

    let found = best_x >= 0 && conf >= min_conf;
    let (fx, fy) = if found {
        (best_x + ox, best_y + oy)
    } else {
        (0i64, 0i64)
    };
    let (cx, cy) = if found {
        (best_x + ox + (tw as i64 / 2), best_y + oy + (th as i64 / 2))
    } else {
        (0i64, 0i64)
    };
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

// ---------------- ui_action ----------------

fn handle_ui_action(args: &Value) -> ToolOut {
    // ref no soportado nativamente (estado del proceso Node)
    if args.get("ref").is_some() {
        return ToolOut::Json(
            json!({"ok": false, "error": "ref_not_supported_native"}),
            true,
        );
    }
    let action = args
        .get("action")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    let supported = [
        "focus",
        "invoke",
        "toggle",
        "expand",
        "collapse",
        "select",
        "set_value",
    ];
    if !supported.contains(&action.as_str()) {
        return ToolOut::Json(json!({"ok": false, "error": "unsupported_ui_action"}), true);
    }
    let hwnd = args.get("hwnd").and_then(as_i64).unwrap_or(0);
    let hwnd = if hwnd > 0 { hwnd } else { foreground_hwnd() };
    let target_window = args
        .get("target_window")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    let name_need = args.get("name").and_then(as_str).unwrap_or("").to_string();
    let id_need = args
        .get("automation_id")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    let type_need = args
        .get("control_type")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();

    let auto = match create_automation() {
        Some(a) => a,
        None => {
            return ToolOut::Json(json!({"ok": false, "error": "uia_init_failed"}), true);
        }
    };
    let root = match root_element(&auto, hwnd, &target_window) {
        Some(r) => r,
        None => {
            return ToolOut::Json(json!({"ok": false, "error": "window_not_found"}), true);
        }
    };
    let walker = match unsafe { auto.ControlViewWalker() } {
        Ok(w) => w,
        Err(_) => {
            return ToolOut::Json(json!({"ok": false, "error": "walker_failed"}), true);
        }
    };

    // find first matching element (DFS)
    fn find_el(
        walker: &IUIAutomationTreeWalker,
        el: &IUIAutomationElement,
        name_need: &str,
        id_need: &str,
        type_need: &str,
    ) -> Option<IUIAutomationElement> {
        unsafe {
            let c = el;
            let c_name = c.CurrentName().map(bstr_to_string).unwrap_or_default();
            let c_aid = c
                .CurrentAutomationId()
                .map(bstr_to_string)
                .unwrap_or_default();
            let c_type = control_type_name(c.CurrentControlType().map(|v| v.0).unwrap_or(0));
            let name_ok =
                name_need.is_empty() || c_name.to_lowercase().contains(&name_need.to_lowercase());
            let id_ok = id_need.is_empty() || c_aid == id_need;
            let type_ok = type_need.is_empty()
                || c_type == type_need
                || c_type.to_lowercase() == type_need.to_lowercase();
            if name_ok && id_ok && type_ok && (!name_need.is_empty() || !id_need.is_empty()) {
                return Some(el.clone());
            }
            let mut child = walker.GetFirstChildElement(el).ok();
            while let Some(ch) = child {
                if let Some(found) = find_el(walker, &ch, name_need, id_need, type_need) {
                    return Some(found);
                }
                child = walker.GetNextSiblingElement(&ch).ok();
            }
            None
        }
    }

    let matched = find_el(&walker, &root, &name_need, &id_need, &type_need);
    let matched = match matched {
        Some(m) => m,
        None => {
            return ToolOut::Json(json!({"ok": false, "error": "element_not_found"}), true);
        }
    };

    let c = matched.clone();
    let name = unsafe { c.CurrentName().map(bstr_to_string).unwrap_or_default() };
    let ct = unsafe { control_type_name(c.CurrentControlType().map(|v| v.0).unwrap_or(0)) };
    let rect = unsafe {
        c.CurrentBoundingRectangle()
            .unwrap_or(windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            })
    };

    let perform = |act: &str| -> Result<(), String> {
        unsafe {
            match act {
                "focus" => c.SetFocus().map_err(|e| e.to_string()),
                "invoke" => {
                    let ip = c.GetCurrentPatternAs::<windows::Win32::UI::Accessibility::IUIAutomationInvokePattern>(UIA_InvokePatternId).map_err(|e| e.to_string())?;
                    ip.Invoke().map_err(|e| e.to_string())
                }
                "toggle" => {
                    let tp = c.GetCurrentPatternAs::<windows::Win32::UI::Accessibility::IUIAutomationTogglePattern>(UIA_TogglePatternId).map_err(|e| e.to_string())?;
                    tp.Toggle().map_err(|e| e.to_string())
                }
                "expand" => {
                    let ep = c.GetCurrentPatternAs::<windows::Win32::UI::Accessibility::IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId).map_err(|e| e.to_string())?;
                    ep.Expand().map_err(|e| e.to_string())
                }
                "collapse" => {
                    let ep = c.GetCurrentPatternAs::<windows::Win32::UI::Accessibility::IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId).map_err(|e| e.to_string())?;
                    ep.Collapse().map_err(|e| e.to_string())
                }
                "select" => {
                    let sp = c.GetCurrentPatternAs::<windows::Win32::UI::Accessibility::IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId).map_err(|e| e.to_string())?;
                    sp.Select().map_err(|e| e.to_string())
                }
                "set_value" => {
                    let vp = c
                        .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                        .map_err(|e| e.to_string())?;
                    let bstr =
                        windows::core::BSTR::from(args.get("value").and_then(as_str).unwrap_or(""));
                    vp.SetValue(&bstr).map_err(|e| e.to_string())
                }
                _ => Err("unsupported_ui_action".into()),
            }
        }
    };
    let res = perform(&action);

    if let Err(e) = res {
        let err_code = if e.contains("pattern") { e } else { e };
        return ToolOut::Json(json!({"ok": false, "error": err_code}), true);
    }

    let epoch = cu_core::epoch::bump();
    let mut payload = json!({
        "ok": true,
        "action": action,
        "name": name,
        "control_type": ct,
        "x": (rect.left + (rect.right-rect.left)/2),
        "y": (rect.top + (rect.bottom-rect.top)/2),
        "epoch": epoch
    });

    // snapshot_after
    if args
        .get("snapshot_after")
        .and_then(as_bool)
        .unwrap_or(false)
    {
        let snap = collect_elements(hwnd, &target_window, 12, 60, false, true);
        if snap.ok {
            let ec = snap.elements.len();
            payload["fresh_snapshot"] = json!({
                "window_title": snap.window_title,
                "hwnd": snap.hwnd,
                "elements": snap.elements.into_iter().take(30).collect::<Vec<_>>(),
                "element_count": ec
            });
        }
    }

    ToolOut::Json(payload, false)
}

// ---------------- read_selection ----------------

fn handle_read_selection(args: &Value) -> ToolOut {
    let select_all = args.get("select_all").and_then(as_bool).unwrap_or(false);
    let target_window = args
        .get("target_window")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    if !target_window.is_empty() {
        // focus window best-effort omitted (no NativeWin.FocusWindow binding here)
    }
    // UIA TextPattern on focused element
    let auto = match create_automation() {
        Some(a) => a,
        None => {
            return ToolOut::Json(json!({"ok": false, "error": "uia_init_failed"}), true);
        }
    };
    let el = match unsafe { auto.GetFocusedElement() } {
        Ok(e) => e,
        Err(_) => {
            return ToolOut::Json(json!({"ok": false, "error": "no_focused_element"}), true);
        }
    };
    unsafe {
        use windows::Win32::UI::Accessibility::IUIAutomationTextPattern;
        if let Ok(tp) = el.GetCurrentPatternAs::<IUIAutomationTextPattern>(
            windows::Win32::UI::Accessibility::UIA_TextPatternId,
        ) {
            if let Ok(range) = tp.DocumentRange() {
                if let Ok(bstr) = range.GetText(-1) {
                    let text = bstr_to_string(bstr);
                    let payload = json!({"ok": true, "text": text, "length": text.len()});
                    return ToolOut::Json(payload, false);
                }
            }
        }
    }
    // fallback: no text
    ToolOut::Json(json!({"ok": true, "text": "", "length": 0}), false)
}

// ---------------- observe ----------------

fn active_window_info() -> (String, String, i64) {
    let hwnd = foreground_hwnd();
    let auto = match create_automation() {
        Some(a) => a,
        None => return ("(Sin ventana activa)".into(), "unknown".into(), hwnd),
    };
    let el =
        match unsafe { auto.ElementFromHandle(windows::Win32::Foundation::HWND(hwnd as *mut _)) } {
            Ok(e) => e,
            Err(_) => return ("(Sin ventana activa)".into(), "unknown".into(), hwnd),
        };
    let title = unsafe { el.CurrentName().map(bstr_to_string).unwrap_or_default() };
    let pid = unsafe { el.CurrentProcessId().unwrap_or(0) as u32 };
    let pname = process_name(pid);
    let title = if title.is_empty() {
        "(Sin ventana activa)".into()
    } else {
        title
    };
    (title, pname, hwnd)
}

fn handle_observe(args: &Value) -> ToolOut {
    let (win_title, proc_name, hwnd) = active_window_info();
    // map ui
    let mapped = handle_map_ui(&json!({"hwnd": hwnd, "max_results": 200}));
    let mut elements = Vec::new();
    if let ToolOut::Json(v, _) = &mapped {
        if let Some(arr) = v.get("elements").and_then(|x| x.as_array()) {
            elements = arr.clone();
        }
    }
    let limit = args
        .get("limit")
        .and_then(as_i64)
        .unwrap_or(25)
        .clamp(5, 100) as usize;
    let format = args.get("format").and_then(as_str).unwrap_or("table");
    let is_browser = false;
    let count = elements.len();

    if format == "json" {
        let compact: Vec<Value> = elements.iter().take(limit).map(|el| {
            json!({
                "id": el.get("id").cloned().unwrap_or(json!(null)),
                "type": el.get("type").cloned().unwrap_or(json!("Control")),
                "name": el.get("name").cloned().unwrap_or(json!("")).as_str().unwrap_or("").chars().take(60).collect::<String>()
            })
        }).collect();
        let payload = json!({
            "ok": true,
            "window": {"title": win_title, "process": proc_name, "hwnd": hwnd, "is_browser": is_browser},
            "source": "native_uia",
            "count": count,
            "elements": compact,
            "screenshot": null
        });
        return ToolOut::JsonStructured(payload, false);
    }

    // table
    let slice = &elements[..elements.len().min(limit)];
    let mut table = String::from("| ID | Tipo | Control / Texto |\n|---|---|---|\n");
    for el in slice {
        let id = el
            .get("id")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".into());
        let typ = el.get("type").and_then(as_str).unwrap_or("Item");
        let raw = el.get("name").and_then(as_str).unwrap_or("");
        let clean = raw
            .replace(['\r', '\n', '\t', '|'], " ")
            .chars()
            .take(45)
            .collect::<String>();
        let clean = if clean.is_empty() {
            "(sin texto)".to_string()
        } else {
            clean
        };
        table.push_str(&format!("| {} | {} | {} |\n", id, typ, clean));
    }
    if count > limit {
        table.push_str(&format!(
            "*(+{} controles adicionales omitidos para ahorrar tokens)*\n",
            count - limit
        ));
    }
    let text_summary = format!(
        "### 🖥️ Ventana: {} [{}]\n*Modo: {} | Controles: {}*\n{}\n",
        win_title,
        proc_name,
        if is_browser {
            "🌐 Navegador (CDP DOM)"
        } else {
            "🪟 App Nativa (UIA)"
        },
        count,
        table
    );
    let payload = json!({
        "ok": true,
        "summary": text_summary,
        "window": {"title": win_title, "process": proc_name, "hwnd": hwnd, "is_browser": is_browser},
        "count": count,
        "shot": null
    });
    ToolOut::Json(payload, false)
}

// ---------------- context_menu ----------------

fn vk_from_key(k: &str) -> u8 {
    match k.trim().to_lowercase().as_str() {
        "down" => 0x28,
        "up" => 0x26,
        "left" => 0x25,
        "right" => 0x27,
        "enter" | "return" => 0x0D,
        "escape" | "esc" => 0x1B,
        "tab" => 0x09,
        "space" => 0x20,
        s if s.len() == 1
            && s.chars()
                .next()
                .map(|c| c.is_ascii_lowercase())
                .unwrap_or(false) =>
        {
            (0x41 + (s.chars().next().unwrap_or('a') as u8 - b'a')) as u8
        }
        s if s.len() == 1
            && s.chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false) =>
        {
            (0x30 + (s.chars().next().unwrap_or('0') as u8 - b'0')) as u8
        }
        _ => 0,
    }
}

fn handle_context_menu(args: &Value) -> ToolOut {
    let x = args.get("x").and_then(as_i64).unwrap_or(0) as i32;
    let y = args.get("y").and_then(as_i64).unwrap_or(0) as i32;
    let item_text = args
        .get("item_text")
        .or(args.get("select_text"))
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    let keys: Vec<String> = args
        .get("keys")
        .or(args.get("key_sequence"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let delay_ms = args.get("delay_ms").and_then(as_i64).unwrap_or(150).max(0) as u64;

    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SetCursorPos(x, y);
        windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
            windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_RIGHTDOWN,
            0,
            0,
            0,
            0,
        );
        windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
            windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_RIGHTUP,
            0,
            0,
            0,
            0,
        );
    }
    std::thread::sleep(std::time::Duration::from_millis(delay_ms));

    if !item_text.is_empty() {
        // find menu item via UIA root children (menus/popups)
        let auto = match create_automation() {
            Some(a) => a,
            None => {
                return ToolOut::Json(json!({"ok": false, "error": "uia_init_failed"}), true);
            }
        };
        unsafe {
            use windows::Win32::UI::Accessibility::{
                IUIAutomation, UIA_MenuControlTypeId, UIA_WindowControlTypeId,
            };
            let root = match auto.GetRootElement() {
                Ok(r) => r,
                Err(_) => {
                    // dismiss
                    windows::Win32::UI::Input::KeyboardAndMouse::keybd_event(
                        0x1B,
                        0,
                        windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(2),
                        0,
                    );
                    return ToolOut::Json(json!({"ok": false, "error": "root_failed"}), true);
                }
            };
            // find menu item by name contains
            fn find_menu_item(
                walker: &IUIAutomationTreeWalker,
                el: &windows::Win32::UI::Accessibility::IUIAutomationElement,
                needle: &str,
            ) -> Option<windows::Win32::UI::Accessibility::IUIAutomationElement> {
                unsafe {
                    // check if control type menu/window and search descendants
                    let mut child = walker.GetFirstChildElement(el).ok();
                    // first search direct children menus
                    // We'll do DFS for MenuItem
                    fn dfs(
                        walker: &IUIAutomationTreeWalker,
                        el: &windows::Win32::UI::Accessibility::IUIAutomationElement,
                        needle: &str,
                    ) -> Option<windows::Win32::UI::Accessibility::IUIAutomationElement>
                    {
                        unsafe {
                            let ct = el.CurrentControlType().map(|v| v.0).unwrap_or(0);
                            if ct == 50011 {
                                // MenuItem
                                let name = el.CurrentName().map(bstr_to_string).unwrap_or_default();
                                if !name.is_empty()
                                    && name.to_lowercase().contains(&needle.to_lowercase())
                                {
                                    return Some(el.clone());
                                }
                            }
                            let mut c = walker.GetFirstChildElement(el).ok();
                            while let Some(ch) = c {
                                if let Some(f) = dfs(walker, &ch, needle) {
                                    return Some(f);
                                }
                                c = walker.GetNextSiblingElement(&ch).ok();
                            }
                            None
                        }
                    }
                    // search in root's top-level menus
                    let mut c = walker.GetFirstChildElement(el).ok();
                    while let Some(ch) = c {
                        if let Some(f) = dfs(walker, &ch, needle) {
                            return Some(f);
                        }
                        c = walker.GetNextSiblingElement(&ch).ok();
                    }
                    None
                }
            }
            let walker = match auto.ControlViewWalker() {
                Ok(w) => w,
                Err(_) => {
                    return ToolOut::Json(json!({"ok": false, "error": "walker_failed"}), true);
                }
            };
            if let Some(matched) = find_menu_item(&walker, &root, &item_text) {
                let rect = matched.CurrentBoundingRectangle().unwrap_or(
                    windows::Win32::Foundation::RECT {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    },
                );
                let cx = rect.left + (rect.right - rect.left) / 2;
                let cy = rect.top + (rect.bottom - rect.top) / 2;
                windows::Win32::UI::WindowsAndMessaging::SetCursorPos(cx, cy);
                std::thread::sleep(std::time::Duration::from_millis(40));
                windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
                    windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_LEFTDOWN,
                    0,
                    0,
                    0,
                    0,
                );
                windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
                    windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_LEFTUP,
                    0,
                    0,
                    0,
                    0,
                );
                let selected = matched
                    .CurrentName()
                    .map(bstr_to_string)
                    .unwrap_or_default();
                let payload = json!({
                    "ok": true,
                    "method": "uia_text_match",
                    "selected": selected,
                    "coords": {"x": cx, "y": cy}
                });
                return ToolOut::Json(payload, false);
            }
        }
        // not found -> dismiss
        unsafe {
            use windows::Win32::UI::Input::KeyboardAndMouse::keybd_event;
            windows::Win32::UI::Input::KeyboardAndMouse::keybd_event(
                0x1B,
                0,
                windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0),
                0,
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
            windows::Win32::UI::Input::KeyboardAndMouse::keybd_event(
                0x1B,
                0,
                windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(2),
                0,
            );
        }
        return ToolOut::Json(
            json!({"ok": false, "error": "Item not found and no key sequence provided; menu dismissed with Escape"}),
            true,
        );
    }

    // key sequence
    if !keys.is_empty() {
        for k in &keys {
            let vk = vk_from_key(k);
            if vk != 0 {
                unsafe {
                    use windows::Win32::UI::Input::KeyboardAndMouse::keybd_event;
                    windows::Win32::UI::Input::KeyboardAndMouse::keybd_event(
                        vk,
                        0,
                        windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0),
                        0,
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
                unsafe {
                    use windows::Win32::UI::Input::KeyboardAndMouse::keybd_event;
                    windows::Win32::UI::Input::KeyboardAndMouse::keybd_event(
                        vk,
                        0,
                        windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(2),
                        0,
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(35));
            }
        }
        let payload = json!({"ok": true, "method": "key_sequence", "keys_sent": keys.len()});
        return ToolOut::Json(payload, false);
    }

    // dismiss
    unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::keybd_event;
        windows::Win32::UI::Input::KeyboardAndMouse::keybd_event(
            0x1B,
            0,
            windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0),
            0,
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        windows::Win32::UI::Input::KeyboardAndMouse::keybd_event(
            0x1B,
            0,
            windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(2),
            0,
        );
    }
    ToolOut::Json(
        json!({"ok": false, "error": "Item not found and no key sequence provided; menu dismissed with Escape"}),
        true,
    )
}

// ---------------- toast_action ----------------

fn handle_toast_action(args: &Value) -> ToolOut {
    let action = args
        .get("action")
        .and_then(as_str)
        .unwrap_or("list")
        .to_lowercase();
    let button_text = args
        .get("button_text")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();

    if action == "click_button" {
        if button_text.is_empty() {
            return ToolOut::Json(
                json!({"ok": false, "error": "button_text is required for click_button action"}),
                true,
            );
        }
        // UIA find button by name contains? origin exact NameProperty equals btnText. We'll do contains? Origin exact. Use exact first, then contains fallback.
        let auto = match create_automation() {
            Some(a) => a,
            None => {
                return ToolOut::Json(json!({"ok": false, "error": "uia_init_failed"}), true);
            }
        };
        unsafe {
            let root = match auto.GetRootElement() {
                Ok(r) => r,
                Err(_) => {
                    return ToolOut::Json(json!({"ok": false, "error": "root_failed"}), true);
                }
            };
            // DFS for button with name == button_text
            fn find_btn(
                walker: &windows::Win32::UI::Accessibility::IUIAutomationTreeWalker,
                el: &windows::Win32::UI::Accessibility::IUIAutomationElement,
                needle: &str,
            ) -> Option<windows::Win32::UI::Accessibility::IUIAutomationElement> {
                unsafe {
                    let ct = el.CurrentControlType().map(|v| v.0).unwrap_or(0);
                    if ct == 50000 {
                        // Button
                        let name = el.CurrentName().map(bstr_to_string).unwrap_or_default();
                        if name == needle {
                            return Some(el.clone());
                        }
                    }
                    let mut c = walker.GetFirstChildElement(el).ok();
                    while let Some(ch) = c {
                        if let Some(f) = find_btn(walker, &ch, needle) {
                            return Some(f);
                        }
                        c = walker.GetNextSiblingElement(&ch).ok();
                    }
                    None
                }
            }
            let walker = match auto.ControlViewWalker() {
                Ok(w) => w,
                Err(_) => {
                    return ToolOut::Json(json!({"ok": false, "error": "walker_failed"}), true);
                }
            };
            if let Some(btn) = find_btn(&walker, &root, &button_text) {
                // try invoke pattern
                if let Ok(ip) = btn.GetCurrentPatternAs::<windows::Win32::UI::Accessibility::IUIAutomationInvokePattern>(windows::Win32::UI::Accessibility::UIA_InvokePatternId) {
                    let _ = ip.Invoke();
                    return ToolOut::Json(json!({"ok": true, "action": "click_button", "clicked": button_text}), false);
                }
                // fallback click
                let rect =
                    btn.CurrentBoundingRectangle()
                        .unwrap_or(windows::Win32::Foundation::RECT {
                            left: 0,
                            top: 0,
                            right: 0,
                            bottom: 0,
                        });
                let cx = rect.left + (rect.right - rect.left) / 2;
                let cy = rect.top + (rect.bottom - rect.top) / 2;
                windows::Win32::UI::WindowsAndMessaging::SetCursorPos(cx, cy);
                std::thread::sleep(std::time::Duration::from_millis(20));
                windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
                    windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_LEFTDOWN,
                    0,
                    0,
                    0,
                    0,
                );
                windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
                    windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_LEFTUP,
                    0,
                    0,
                    0,
                    0,
                );
                return ToolOut::Json(
                    json!({"ok": true, "action": "click_button", "clicked": button_text}),
                    false,
                );
            } else {
                return ToolOut::Json(
                    json!({"ok": false, "error": format!("Toast action button not found: {}", button_text)}),
                    true,
                );
            }
        }
    }

    // list/dismiss/clear_all not supported natively (WinRT) -> proxy
    ToolOut::Json(json!({"ok": false, "error": "not_supported_native"}), true)
}

// ---------------- system_tray ----------------

fn handle_system_tray(args: &Value) -> ToolOut {
    // Simplified: list visible icons only (no overflow) and actions by mouse.
    let action = args
        .get("action")
        .and_then(as_str)
        .unwrap_or("list")
        .to_lowercase();
    let target = args
        .get("target")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    let index = args.get("index").and_then(as_i64).unwrap_or(-1);
    let include_overflow = args
        .get("include_overflow")
        .and_then(as_bool)
        .unwrap_or(true);

    // Find Shell_TrayWnd
    let tray_hwnd = unsafe {
        use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
        FindWindowW(windows::core::w!("Shell_TrayWnd"), None).unwrap_or_default()
    };
    if tray_hwnd.0.is_null() {
        return ToolOut::Json(
            json!({"ok": false, "error": "Shell_TrayWnd window not found"}),
            true,
        );
    }
    let auto = match create_automation() {
        Some(a) => a,
        None => {
            return ToolOut::Json(json!({"ok": false, "error": "uia_init_failed"}), true);
        }
    };
    let tray_elem = match unsafe { auto.ElementFromHandle(tray_hwnd) } {
        Ok(e) => e,
        Err(_) => {
            return ToolOut::Json(json!({"ok": false, "error": "tray_element_failed"}), true);
        }
    };

    // collect visible icons: Button with AutomationId "NotifyItemIcon"
    let mut items: Vec<serde_json::Map<String, Value>> = Vec::new();
    let mut idx = 0i64;
    unsafe {
        let walker = match auto.ControlViewWalker() {
            Ok(w) => w,
            Err(_) => {
                return ToolOut::Json(json!({"ok": false, "error": "walker_failed"}), true);
            }
        };
        // DFS collect
        fn collect(
            walker: &windows::Win32::UI::Accessibility::IUIAutomationTreeWalker,
            el: &windows::Win32::UI::Accessibility::IUIAutomationElement,
            items: &mut Vec<serde_json::Map<String, Value>>,
            idx: &mut i64,
            is_overflow: bool,
        ) {
            unsafe {
                let ct = el.CurrentControlType().map(|v| v.0).unwrap_or(0);
                let aid = el
                    .CurrentAutomationId()
                    .map(bstr_to_string)
                    .unwrap_or_default();
                if ct == 50000 && aid == "NotifyItemIcon" {
                    let r =
                        el.CurrentBoundingRectangle()
                            .unwrap_or(windows::Win32::Foundation::RECT {
                                left: 0,
                                top: 0,
                                right: 0,
                                bottom: 0,
                            });
                    if r.right - r.left > 0 && r.bottom - r.top > 0 {
                        let name = el.CurrentName().map(bstr_to_string).unwrap_or_default();
                        let mut info = serde_json::Map::new();
                        info.insert("Index".into(), json!(*idx));
                        info.insert("Id".into(), json!(format!("tray_vis_{}", *idx)));
                        info.insert("Name".into(), json!(name.clone()));
                        info.insert("Tooltip".into(), json!(name.clone()));
                        info.insert("Area".into(), json!("visible"));
                        info.insert("IsOverflow".into(), json!(false));
                        info.insert("X".into(), json!(r.left + (r.right - r.left) / 2));
                        info.insert("Y".into(), json!(r.top + (r.bottom - r.top) / 2));
                        info.insert("Width".into(), json!(r.right - r.left));
                        info.insert("Height".into(), json!(r.bottom - r.top));
                        items.push(info);
                        *idx += 1;
                    }
                }
                let mut c = walker.GetFirstChildElement(el).ok();
                while let Some(ch) = c {
                    collect(walker, &ch, items, idx, is_overflow);
                    c = walker.GetNextSiblingElement(&ch).ok();
                }
            }
        }
        collect(&walker, &tray_elem, &mut items, &mut idx, false);
    }

    // normalize items like origin
    let mut norm_items: Vec<Value> = Vec::new();
    for it in &items {
        let name = it.get("Name").and_then(as_str).unwrap_or("").to_string();
        let process = resolve_process_name(&name);
        let pid = 0i64; // not resolved
        let x = it.get("X").and_then(as_i64).unwrap_or(0);
        let y = it.get("Y").and_then(as_i64).unwrap_or(0);
        let w = it.get("Width").and_then(as_i64).unwrap_or(0);
        let h = it.get("Height").and_then(as_i64).unwrap_or(0);
        let idxv = it.get("Index").and_then(as_i64).unwrap_or(0);
        let id = it.get("Id").and_then(as_str).unwrap_or("").to_string();
        let area = it
            .get("Area")
            .and_then(as_str)
            .unwrap_or("visible")
            .to_string();
        let is_overflow = it.get("IsOverflow").and_then(as_bool).unwrap_or(false);
        norm_items.push(json!({
            "id": id,
            "name": name,
            "process": process,
            "tooltip": name,
            "area": area,
            "coords": {"x": x, "y": y, "w": w, "h": h},
            "index": idxv,
            "is_overflow": is_overflow
        }));
    }

    let visible_count = norm_items.len();
    let mut overflow_count = 0usize;

    // overflow not implemented (complex) -> if include_overflow, note? We'll skip to avoid invasiveness.
    // action list
    if action == "list" {
        let payload = json!({
            "ok": true,
            "action": "list",
            "count": norm_items.len(),
            "visible_count": visible_count,
            "overflow_count": overflow_count,
            "items": norm_items
        });
        return ToolOut::Json(payload, false);
    }

    // find target
    let mut target_item: Option<Value> = None;
    if index >= 0 && (index as usize) < norm_items.len() {
        target_item = Some(norm_items[index as usize].clone());
    } else if !target.is_empty() {
        // find by name/process/tooltip/id
        for it in &norm_items {
            let name = it.get("name").and_then(as_str).unwrap_or("");
            let process = it.get("process").and_then(as_str).unwrap_or("");
            let id = it.get("id").and_then(as_str).unwrap_or("");
            if name.to_lowercase().contains(&target.to_lowercase())
                || process.to_lowercase().contains(&target.to_lowercase())
                || id.to_lowercase().contains(&target.to_lowercase())
            {
                target_item = Some(it.clone());
                break;
            }
        }
    }
    let target = match target_item {
        Some(t) => t,
        None => {
            return ToolOut::Json(
                json!({"ok": false, "error": format!("Target tray icon not found: {}", target)}),
                true,
            );
        }
    };

    let tx = target
        .get("coords")
        .and_then(|c| c.get("x"))
        .and_then(as_i64)
        .unwrap_or(0) as i32;
    let ty = target
        .get("coords")
        .and_then(|c| c.get("y"))
        .and_then(as_i64)
        .unwrap_or(0) as i32;

    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SetCursorPos(tx, ty);
        std::thread::sleep(std::time::Duration::from_millis(50));
        match action.as_str() {
            "click" => {
                windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
                    windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_LEFTDOWN,
                    0,
                    0,
                    0,
                    0,
                );
                windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
                    windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_LEFTUP,
                    0,
                    0,
                    0,
                    0,
                );
            }
            "right_click" => {
                windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
                    windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_RIGHTDOWN,
                    0,
                    0,
                    0,
                    0,
                );
                windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
                    windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_RIGHTUP,
                    0,
                    0,
                    0,
                    0,
                );
            }
            "double_click" => {
                windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
                    windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_LEFTDOWN,
                    0,
                    0,
                    0,
                    0,
                );
                windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
                    windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_LEFTUP,
                    0,
                    0,
                    0,
                    0,
                );
                std::thread::sleep(std::time::Duration::from_millis(60));
                windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
                    windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_LEFTDOWN,
                    0,
                    0,
                    0,
                    0,
                );
                windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
                    windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_LEFTUP,
                    0,
                    0,
                    0,
                    0,
                );
            }
            _ => {}
        }
    }

    let epoch = cu_core::epoch::bump();
    let tname = target
        .get("name")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    let tprocess = target
        .get("process")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    let tidx = target.get("index").and_then(as_i64).unwrap_or(0);
    let tid = target.get("id").and_then(as_str).unwrap_or("").to_string();
    let tisov = target.get("is_overflow").and_then(as_bool).unwrap_or(false);
    let tw = target
        .get("coords")
        .and_then(|c| c.get("w"))
        .and_then(as_i64)
        .unwrap_or(0);
    let th = target
        .get("coords")
        .and_then(|c| c.get("h"))
        .and_then(as_i64)
        .unwrap_or(0);

    let payload = json!({
        "ok": true,
        "action": action,
        "count": norm_items.len(),
        "visible_count": visible_count,
        "overflow_count": overflow_count,
        "items": norm_items,
        "target": {
            "index": tidx,
            "id": tid,
            "name": tname,
            "process": tprocess,
            "pid": 0,
            "x": tx,
            "y": ty,
            "is_overflow": tisov
        },
        "epoch": epoch
    });
    ToolOut::Json(payload, false)
}

fn resolve_process_name(name: &str) -> String {
    if name.is_empty() {
        return String::new();
    }
    let lower = name.to_lowercase();
    // iterate processes
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    unsafe {
        let snap = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(_) => return String::new(),
        };
        let mut pe = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut pe).is_ok() {
            loop {
                let pname = String::from_utf16_lossy(&pe.szExeFile)
                    .trim_end_matches('\0')
                    .to_string();
                let pl = pname.to_lowercase();
                if pl.len() > 2 && (lower.contains(&pl) || pl.contains(&lower)) {
                    let _ = windows::Win32::Foundation::CloseHandle(snap);
                    return format!("{}.exe", pname.trim_end_matches(".exe"));
                }
                if Process32NextW(snap, &mut pe).is_err() {
                    break;
                }
            }
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
    }
    String::new()
}

// ---------------- background_action ----------------

fn is_browser_window(class_name: &str, process_name: &str) -> bool {
    let cls = class_name.to_lowercase();
    let proc = process_name
        .to_lowercase()
        .replace(".exe", "")
        .trim()
        .to_string();
    if cls.contains("chrome_widgetwin") {
        return true;
    }
    if cls.contains("mozillawindowclass") {
        return true;
    }
    let browsers = [
        "chrome",
        "msedge",
        "firefox",
        "brave",
        "opera",
        "vivaldi",
        "iexplore",
        "msedgewebview2",
        "electron",
    ];
    browsers.contains(&proc.as_str())
}

fn handle_background_action(args: &Value) -> ToolOut {
    let action = args
        .get("action")
        .and_then(as_str)
        .unwrap_or("")
        .to_lowercase();
    let supported = [
        "invoke",
        "toggle",
        "select",
        "expand",
        "collapse",
        "set_value",
        "focus",
    ];
    if !supported.contains(&action.as_str()) {
        return ToolOut::Json(
            json!({"ok": false, "error": "invalid_action", "message": format!("action debe ser una de: {}", supported.join(", "))}),
            true,
        );
    }
    let hwnd = args.get("hwnd").and_then(as_i64).unwrap_or(0);
    let target_window = args
        .get("target_window")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    let win_hwnd = if hwnd > 0 { hwnd } else { foreground_hwnd() };

    // window validity
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::IsWindow;
        if IsWindow(Some(windows::Win32::Foundation::HWND(win_hwnd as *mut _))).as_bool() == false {
            return ToolOut::Json(
                json!({"ok": false, "error": "window_not_found", "message": "no se pudo resolver la ventana objetivo"}),
                true,
            );
        }
    }

    // window info
    let auto = match create_automation() {
        Some(a) => a,
        None => {
            return ToolOut::Json(
                json!({"ok": false, "error": "uia_init_failed", "message": "no se pudo resolver la ventana objetivo"}),
                true,
            );
        }
    };
    let root = match root_element(&auto, win_hwnd, &target_window) {
        Some(r) => r,
        None => {
            return ToolOut::Json(
                json!({"ok": false, "error": "window_not_found", "message": "no se pudo resolver la ventana objetivo"}),
                true,
            );
        }
    };
    let class_name = unsafe {
        root.CurrentClassName()
            .map(bstr_to_string)
            .unwrap_or_default()
    };
    let proc_name = {
        let pid = unsafe { root.CurrentProcessId().unwrap_or(0) as u32 };
        process_name(pid)
    };
    if is_browser_window(&class_name, &proc_name) {
        return ToolOut::Json(
            json!({"ok": false, "error": "background_blocked_browser", "message": "contenido web no soporta modo background/headless"}),
            true,
        );
    }

    // resolve element by name/automation_id/control_type
    let name_need = args.get("name").and_then(as_str).unwrap_or("").to_string();
    let id_need = args
        .get("automation_id")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    let type_need = args
        .get("control_type")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    if name_need.is_empty() && id_need.is_empty() && type_need.is_empty() {
        return ToolOut::Json(
            json!({"ok": false, "error": "element_not_found", "message": "no se resolvió el elemento: pasá name/automation_id"}),
            true,
        );
    }
    let walker = match unsafe { auto.ControlViewWalker() } {
        Ok(w) => w,
        Err(_) => {
            return ToolOut::Json(
                json!({"ok": false, "error": "walker_failed", "message": "no se pudo resolver la ventana objetivo"}),
                true,
            );
        }
    };
    // find
    fn find(
        walker: &IUIAutomationTreeWalker,
        el: &IUIAutomationElement,
        name_need: &str,
        id_need: &str,
        type_need: &str,
    ) -> Option<IUIAutomationElement> {
        unsafe {
            let c_name = el.CurrentName().map(bstr_to_string).unwrap_or_default();
            let c_aid = el
                .CurrentAutomationId()
                .map(bstr_to_string)
                .unwrap_or_default();
            let c_type = control_type_name(el.CurrentControlType().map(|v| v.0).unwrap_or(0));
            let name_ok =
                name_need.is_empty() || c_name.to_lowercase().contains(&name_need.to_lowercase());
            let id_ok = id_need.is_empty() || c_aid == id_need;
            let type_ok = type_need.is_empty() || c_type == type_need;
            if name_ok && id_ok && type_ok && (!name_need.is_empty() || !id_need.is_empty()) {
                return Some(el.clone());
            }
            let mut ch = walker.GetFirstChildElement(el).ok();
            while let Some(c) = ch {
                if let Some(f) = find(walker, &c, name_need, id_need, type_need) {
                    return Some(f);
                }
                ch = walker.GetNextSiblingElement(&c).ok();
            }
            None
        }
    }
    let matched = match find(&walker, &root, &name_need, &id_need, &type_need) {
        Some(m) => m,
        None => {
            return ToolOut::Json(
                json!({"ok": false, "error": "element_not_found", "message": "no se resolvió el elemento"}),
                true,
            );
        }
    };

    // patterns available
    let mut avail: Vec<&str> = Vec::new();
    unsafe {
        if matched.GetCurrentPattern(UIA_InvokePatternId).is_ok() {
            avail.push("invoke");
        }
        if matched.GetCurrentPattern(UIA_TogglePatternId).is_ok() {
            avail.push("toggle");
        }
        if matched
            .GetCurrentPattern(UIA_SelectionItemPatternId)
            .is_ok()
        {
            avail.push("select");
        }
        if matched
            .GetCurrentPattern(UIA_ExpandCollapsePatternId)
            .is_ok()
        {
            avail.push("expand");
        }
        // collapse via expandcollapse too
        if matched.GetCurrentPattern(UIA_ValuePatternId).is_ok() {
            avail.push("value");
        }
    }
    // collapse not in patterns list detection? treat expand as collapse-capable
    // pick
    let priority: &[&str] = match action.as_str() {
        "invoke" => &["invoke", "toggle", "select", "expand", "value"],
        "toggle" => &["toggle", "invoke", "select"],
        "select" => &["select", "invoke", "toggle"],
        "expand" => &["expand", "invoke", "toggle"],
        "collapse" => &["expand"],
        "set_value" => &["value"],
        "focus" => &["focus"],
        _ => &[],
    };
    let mut chosen: Option<&str> = None;
    for p in priority {
        if avail.contains(p) {
            chosen = Some(*p);
            break;
        }
    }
    // focus special
    if action == "focus" {
        chosen = Some("focus");
    }

    let perform = |pat: &str| -> Result<(), String> {
        unsafe {
            match (action.as_str(), pat) {
                ("invoke"|"toggle"|"select"|"expand", "invoke") => matched.GetCurrentPatternAs::<windows::Win32::UI::Accessibility::IUIAutomationInvokePattern>(UIA_InvokePatternId).and_then(|ip| ip.Invoke()).map_err(|e| e.to_string()),
                ("toggle", "toggle") | (_, "toggle") => matched.GetCurrentPatternAs::<windows::Win32::UI::Accessibility::IUIAutomationTogglePattern>(UIA_TogglePatternId).and_then(|tp| tp.Toggle()).map_err(|e| e.to_string()),
                ("select", "select") | (_, "select") => matched.GetCurrentPatternAs::<windows::Win32::UI::Accessibility::IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId).and_then(|sp| sp.Select()).map_err(|e| e.to_string()),
                ("expand", "expand") | ("collapse", "expand") => {
                    let ep = matched.GetCurrentPatternAs::<windows::Win32::UI::Accessibility::IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId).map_err(|e| e.to_string())?;
                    if action == "collapse" { ep.Collapse().map_err(|e| e.to_string()) } else { ep.Expand().map_err(|e| e.to_string()) }
                }
                ("set_value", "value") => {
                    let vp = matched.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId).map_err(|e| e.to_string())?;
                    let bstr = windows::core::BSTR::from(args.get("value").and_then(as_str).unwrap_or(""));
                    vp.SetValue(&bstr).map_err(|e| e.to_string())
                }
                ("focus", "focus") => matched.SetFocus().map_err(|e| e.to_string()),
                _ => Err("pattern_failed".into()),
            }
        }
    };
    let res = if let Some(pat) = chosen {
        perform(pat)
    } else {
        Err("background_unsupported".into())
    };

    if let Err(e) = res {
        return ToolOut::Json(
            json!({"ok": false, "error": "background_unsupported", "message": e, "attempts": [{"pattern": chosen.unwrap_or(""), "error": e}]}),
            true,
        );
    }

    let epoch = cu_core::epoch::bump();
    let c_name = unsafe {
        matched
            .CurrentName()
            .map(bstr_to_string)
            .unwrap_or_default()
    };
    let c_type =
        unsafe { control_type_name(matched.CurrentControlType().map(|v| v.0).unwrap_or(0)) };
    let rect = unsafe {
        matched
            .CurrentBoundingRectangle()
            .unwrap_or(windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            })
    };
    let payload = json!({
        "ok": true,
        "method_used": chosen.unwrap_or("focus"),
        "element": {"name": c_name, "type": c_type, "automation_id": unsafe { matched.CurrentAutomationId().map(bstr_to_string).unwrap_or_default() }},
        "window": {"hwnd": win_hwnd, "title": unsafe { root.CurrentName().map(bstr_to_string).unwrap_or_default() }},
        "epoch": epoch
    });
    ToolOut::Json(payload, false)
}

// ---------------- click_text ----------------

fn handle_click_text(args: &Value) -> ToolOut {
    let search_needle = args
        .get("text")
        .and_then(as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if search_needle.is_empty() {
        return ToolOut::Json(
            json!({"ok": false, "error": "text parameter is required"}),
            true,
        );
    }
    let is_exact = args.get("exact").and_then(as_bool).unwrap_or(false);
    let is_case_sensitive = args
        .get("case_sensitive")
        .and_then(as_bool)
        .unwrap_or(false);
    let occurrence = args.get("occurrence").and_then(as_i64).unwrap_or(1);
    let offset_x = args.get("offset_x").and_then(as_i64).unwrap_or(0);
    let offset_y = args.get("offset_y").and_then(as_i64).unwrap_or(0);
    let hwnd = args.get("hwnd").and_then(as_i64).unwrap_or(0);
    let target_window = args
        .get("target_window")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    let hwnd = if hwnd > 0 { hwnd } else { foreground_hwnd() };

    // inspect ui
    let ins = handle_inspect_ui(
        &json!({"hwnd": hwnd, "target_window": target_window, "max_results": 50, "name": search_needle}),
    );
    let mut elements = Vec::new();
    if let ToolOut::Json(v, _) = &ins {
        if let Some(arr) = v.get("elements").and_then(|x| x.as_array()) {
            elements = arr.clone();
        }
    }
    // filter
    let mut filtered: Vec<Value> = Vec::new();
    for el in &elements {
        let el_name = el
            .get("name")
            .and_then(as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let match_ok = if is_exact {
            if is_case_sensitive {
                el_name == search_needle
            } else {
                el_name.to_lowercase() == search_needle.to_lowercase()
            }
        } else {
            if is_case_sensitive {
                el_name.contains(&search_needle)
            } else {
                el_name
                    .to_lowercase()
                    .contains(&search_needle.to_lowercase())
            }
        };
        if match_ok {
            let offscreen = el.get("offscreen").and_then(as_bool).unwrap_or(false);
            let w = el.get("width").and_then(as_i64).unwrap_or(0);
            let h = el.get("height").and_then(as_i64).unwrap_or(0);
            if !offscreen && w > 0 && h > 0 {
                filtered.push(el.clone());
            }
        }
    }
    if filtered.is_empty() {
        return ToolOut::Json(
            json!({"ok": false, "found": false, "text": search_needle, "message": "Text not found on screen via UIA or OCR"}),
            true,
        );
    }
    let idx = if occurrence > 0 {
        (occurrence - 1) as usize
    } else {
        (filtered.len() as i64 + occurrence) as usize
    };
    let idx = idx.min(filtered.len().saturating_sub(1));
    let target_el = &filtered[idx];
    let mx = target_el.get("x").and_then(as_i64).unwrap_or(0)
        + target_el.get("width").and_then(as_i64).unwrap_or(0) / 2
        + offset_x;
    let my = target_el.get("y").and_then(as_i64).unwrap_or(0)
        + target_el.get("height").and_then(as_i64).unwrap_or(0) / 2
        + offset_y;
    let matched_text = target_el
        .get("name")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();

    // click
    unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, mouse_event,
        };
        use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;
        SetCursorPos(mx as i32, my as i32);
        std::thread::sleep(std::time::Duration::from_millis(20));
        mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
        mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
    }
    let epoch = cu_core::epoch::bump();
    let payload = json!({
        "ok": true,
        "found": true,
        "source": "uia",
        "matched_text": matched_text,
        "click": {"x": mx, "y": my, "button": "left", "double": false, "offset_x": offset_x, "offset_y": offset_y, "human_like": false},
        "typed": false,
        "modal_appeared": false,
        "epoch": epoch
    });
    ToolOut::Json(payload, false)
}

// ---------------- wait_stable (sync) ----------------

fn handle_wait_stable(args: &Value) -> ToolOut {
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(as_i64)
        .unwrap_or(10000)
        .clamp(100, 60000);
    let interval_ms = args
        .get("interval_ms")
        .and_then(as_i64)
        .unwrap_or(300)
        .clamp(100, 5000);
    let stable_samples = args
        .get("stable_samples")
        .and_then(as_i64)
        .unwrap_or(2)
        .max(1);
    let max_results = args.get("max_results").and_then(as_i64).unwrap_or(150);
    let hwnd = args.get("hwnd").and_then(as_i64).unwrap_or(0);
    let target_window = args
        .get("target_window")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();

    let started = now_ms();
    let mut last_fp: Option<String> = None;
    let mut last_change_at = started;
    let mut stable_count = 0i64;
    let mut samples = 0i64;
    let mut window_title = String::new();
    let mut current_fp: Option<String> = None;

    while now_ms() - started < timeout_ms {
        let collected = collect_elements(hwnd, &target_window, 30, max_results, false, false);
        if collected.ok {
            let fp = fingerprint_of(
                &collected.elements,
                &collected.window_title,
                collected.pid,
                collected.hwnd,
            );
            samples += 1;
            if last_fp.as_ref() == Some(&fp) {
                stable_count += 1;
            } else {
                stable_count = 1;
                last_change_at = now_ms();
            }
            last_fp = Some(fp.clone());
            current_fp = Some(fp.clone());
            window_title = collected.window_title.clone();
            if stable_count >= stable_samples {
                *lock_last_fp() = Some(fp.clone());
                let payload = json!({
                    "ok": true,
                    "stable": true,
                    "elapsed_ms": now_ms() - started,
                    "samples": samples,
                    "fingerprint": fp,
                    "window_title": window_title
                });
                return ToolOut::JsonStructured(payload, false);
            }
        }
        if now_ms() - started >= timeout_ms {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(interval_ms as u64));
    }
    if let Some(fp) = &current_fp {
        *lock_last_fp() = Some(fp.clone());
    }
    let payload = json!({
        "ok": true,
        "stable": false,
        "elapsed_ms": now_ms() - started,
        "last_change_ago_ms": now_ms() - last_change_at,
        "fingerprint": current_fp,
        "samples": samples,
        "window_title": window_title
    });
    ToolOut::JsonStructured(payload, false)
}

// ---------------- fill_form ----------------

fn set_clipboard_text(text: &str) -> bool {
    use windows::Win32::Foundation::*;
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let mut bytes: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        bytes.extend_from_slice(&0u16.to_le_bytes());
        let h = match GlobalAlloc(GMEM_MOVEABLE, bytes.len()) {
            Ok(v) => v,
            Err(_) => {
                let _ = CloseClipboard();
                return false;
            }
        };
        let ptr = GlobalLock(h);
        if ptr.is_null() {
            let _ = GlobalFree(Some(h));
            let _ = CloseClipboard();
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
        let _ = GlobalUnlock(h);
        let _ = SetClipboardData(13, Some(windows::Win32::Foundation::HANDLE(h.0)));
        let _ = CloseClipboard();
        true
    }
}

fn handle_fill_form(args: &Value) -> ToolOut {
    let target_window = args
        .get("target_window")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    let fields = args
        .get("fields")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let submit_button = args
        .get("submit_button")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();

    let act = active_window_info();
    let mut target_hwnd = act.2;
    if target_hwnd == 0 {
        target_hwnd = foreground_hwnd();
    }

    let mut results = Vec::new();
    let synonyms: &[(&str, &[&str])] = &[
        (
            "email",
            &[
                "email",
                "e-mail",
                "correo",
                "correo electrónico",
                "usuario",
                "user",
                "username",
            ],
        ),
        (
            "correo",
            &[
                "correo",
                "correo electrónico",
                "email",
                "e-mail",
                "usuario",
                "user",
            ],
        ),
        (
            "usuario",
            &[
                "usuario",
                "user",
                "username",
                "email",
                "correo",
                "identificación",
                "cuenta",
            ],
        ),
        (
            "password",
            &["password", "contraseña", "clave", "pass", "pin", "código"],
        ),
        (
            "contraseña",
            &["contraseña", "password", "clave", "pass", "código"],
        ),
        ("clave", &["clave", "contraseña", "password"]),
        ("buscar", &["buscar", "search", "búsqueda", "explore"]),
        ("search", &["search", "buscar", "find", "query"]),
    ];

    for f in &fields {
        let fobj = f.as_object().cloned().unwrap_or_default();
        let label = fobj.get("label").and_then(as_str).unwrap_or("").to_string();
        let value = if fobj.get("secret_b64").is_some() {
            // decode base64
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(fobj.get("secret_b64").and_then(as_str).unwrap_or(""))
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_default()
        } else {
            fobj.get("value").and_then(as_str).unwrap_or("").to_string()
        };
        let clear_first = fobj.get("clear_first").and_then(as_bool).unwrap_or(true);
        let offset_x = fobj.get("offset_x").and_then(as_i64).unwrap_or(80);
        let offset_y = fobj.get("offset_y").and_then(as_i64).unwrap_or(0);

        let mut queries = vec![label.clone()];
        let lower = label.to_lowercase();
        for (key, syns) in synonyms {
            if lower.contains(key) {
                for s in *syns {
                    if !queries.contains(&s.to_string()) {
                        queries.push(s.to_string());
                    }
                }
            }
        }

        let mut matched: Option<Value> = None;
        for q in &queries {
            let ins =
                handle_inspect_ui(&json!({"name": q, "hwnd": target_hwnd, "max_results": 15}));
            if let ToolOut::Json(v, _) = &ins {
                if let Some(arr) = v.get("elements").and_then(|x| x.as_array()) {
                    if let Some(el) = arr.iter().find(|e| {
                        !e.get("offscreen").and_then(as_bool).unwrap_or(false)
                            && e.get("width").and_then(as_i64).unwrap_or(0) > 0
                            && e.get("height").and_then(as_i64).unwrap_or(0) > 0
                    }) {
                        matched = Some(el.clone());
                        break;
                    }
                }
            }
        }

        if let Some(el) = matched {
            let is_edit = el.get("control_type").and_then(as_str).unwrap_or("") == "Edit";
            let mx = if is_edit {
                el.get("x").and_then(as_i64).unwrap_or(0)
                    + el.get("width").and_then(as_i64).unwrap_or(0) / 2
                    + offset_x
            } else {
                el.get("x").and_then(as_i64).unwrap_or(0) + offset_x
            };
            // origin: if not edit, offset_x default 80 means click to right of label
            let mx = if is_edit {
                mx
            } else {
                el.get("x").and_then(as_i64).unwrap_or(0)
                    + (if fobj.get("offset_x").is_some() {
                        offset_x
                    } else {
                        el.get("width").and_then(as_i64).unwrap_or(0) + 50
                    })
            };
            let my = el.get("y").and_then(as_i64).unwrap_or(0)
                + el.get("height").and_then(as_i64).unwrap_or(0) / 2
                + offset_y;

            // click
            unsafe {
                use windows::Win32::UI::Input::KeyboardAndMouse::{
                    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, mouse_event,
                };
                use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;
                let _ = SetCursorPos(mx as i32, my as i32);
                std::thread::sleep(std::time::Duration::from_millis(20));
                mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
                mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
            }
            std::thread::sleep(std::time::Duration::from_millis(60));

            if clear_first {
                unsafe {
                    use windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS;
                    use windows::Win32::UI::Input::KeyboardAndMouse::keybd_event;
                    keybd_event(0x11, 0, KEYBD_EVENT_FLAGS(0), 0); // ctrl down
                    keybd_event(0x41, 0, KEYBD_EVENT_FLAGS(0), 0); // a
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    keybd_event(0x41, 0, KEYBD_EVENT_FLAGS(2), 0);
                    keybd_event(0x11, 0, KEYBD_EVENT_FLAGS(2), 0);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    keybd_event(0x08, 0, KEYBD_EVENT_FLAGS(0), 0); // backspace
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    keybd_event(0x08, 0, KEYBD_EVENT_FLAGS(2), 0);
                }
            }

            if !value.is_empty() {
                let _ = set_clipboard_text(&value);
                std::thread::sleep(std::time::Duration::from_millis(30));
                unsafe {
                    use windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS;
                    use windows::Win32::UI::Input::KeyboardAndMouse::keybd_event;
                    keybd_event(0x11, 0, KEYBD_EVENT_FLAGS(0), 0); // ctrl
                    keybd_event(0x56, 0, KEYBD_EVENT_FLAGS(0), 0); // v
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    keybd_event(0x56, 0, KEYBD_EVENT_FLAGS(2), 0);
                    keybd_event(0x11, 0, KEYBD_EVENT_FLAGS(2), 0);
                }
            }

            let id = el
                .get("automation_id")
                .and_then(as_str)
                .unwrap_or("")
                .to_string();
            results.push(json!({"status": "filled", "method": "uia", "label": label, "id": id}));
        } else {
            results.push(json!({"status": "not_found", "label": label}));
        }
    }

    let mut submitted = false;
    if !submit_button.is_empty() {
        let ins = handle_inspect_ui(
            &json!({"name": submit_button, "hwnd": target_hwnd, "max_results": 5}),
        );
        if let ToolOut::Json(v, _) = &ins {
            if let Some(arr) = v.get("elements").and_then(|x| x.as_array()) {
                if let Some(el) = arr.first() {
                    let cx = el.get("x").and_then(as_i64).unwrap_or(0)
                        + el.get("width").and_then(as_i64).unwrap_or(0) / 2;
                    let cy = el.get("y").and_then(as_i64).unwrap_or(0)
                        + el.get("height").and_then(as_i64).unwrap_or(0) / 2;
                    unsafe {
                        use windows::Win32::UI::Input::KeyboardAndMouse::{
                            MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, mouse_event,
                        };
                        use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;
                        let _ = SetCursorPos(cx as i32, cy as i32);
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
                        mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
                    }
                    submitted = true;
                }
            }
        }
    }

    let epoch = cu_core::epoch::bump();
    ToolOut::Json(
        json!({"ok": true, "results": results, "submitted": submitted, "epoch": epoch}),
        false,
    )
}

// ---------------- scroll_to_text ----------------

fn handle_scroll_to_text(args: &Value) -> ToolOut {
    let needle = args
        .get("text")
        .and_then(as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if needle.is_empty() {
        return ToolOut::Json(
            json!({"ok": false, "error": "text parameter is required"}),
            true,
        );
    }
    let max_scrolls = args
        .get("max_scrolls")
        .and_then(as_i64)
        .unwrap_or(5)
        .clamp(1, 15);
    let scroll_amount = args.get("scroll_amount").and_then(as_i64).unwrap_or(-400);
    let target_window = args
        .get("target_window")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    let hwnd = args.get("hwnd").and_then(as_i64).unwrap_or(0);
    let hwnd = if hwnd > 0 { hwnd } else { foreground_hwnd() };

    for i in 0..=max_scrolls {
        let ins = handle_inspect_ui(&json!({"name": needle, "hwnd": hwnd, "max_results": 10}));
        if let ToolOut::Json(v, _) = &ins {
            if let Some(arr) = v.get("elements").and_then(|x| x.as_array()) {
                if let Some(el) = arr.iter().find(|e| {
                    !e.get("offscreen").and_then(as_bool).unwrap_or(false)
                        && e.get("width").and_then(as_i64).unwrap_or(0) > 0
                }) {
                    let epoch = cu_core::epoch::bump();
                    let mut res = el.clone();
                    res["text"] = json!(el.get("name").cloned().unwrap_or(json!("")));
                    let payload = json!({
                        "ok": true,
                        "found": true,
                        "scrolls_performed": i,
                        "element": res,
                        "source": "uia",
                        "epoch": epoch
                    });
                    return ToolOut::Json(payload, false);
                }
            }
        }

        if i < max_scrolls {
            let act = active_window_info();
            let cx = act.2; // hwnd not coords; use foreground window rect? fallback 0
            // We don't have window rect here; scroll at center of screen
            unsafe {
                use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_WHEEL;
                use windows::Win32::UI::Input::KeyboardAndMouse::mouse_event;
                // move cursor to center of primary screen
                use windows::Win32::UI::WindowsAndMessaging::{
                    GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, SetCursorPos,
                };
                let sw = GetSystemMetrics(SM_CXSCREEN);
                let sh = GetSystemMetrics(SM_CYSCREEN);
                let _ = SetCursorPos(sw / 2, sh / 2);
                mouse_event(MOUSEEVENTF_WHEEL, 0, 0, scroll_amount as i32, 0);
            }
            std::thread::sleep(std::time::Duration::from_millis(350));
        }
    }

    let epoch = cu_core::epoch::bump();
    ToolOut::Json(
        json!({"ok": false, "found": false, "scrolls_performed": max_scrolls, "message": format!("Text \"{}\" not found after {} scrolls.", needle, max_scrolls), "epoch": epoch}),
        false,
    )
}

// ---------------- computer_act ----------------
// PORT NOTE: implementación parcial de handleAct del origen. Solo resolución de
// target por name/automation_id/control_type vía UIA; acciones click/double_click/
// right_click/focus/set_value/clear/toggle/expand/collapse/select/press/scroll/wait/type.
// No soporta ref/id (estado del proceso Node), coordenadas, CDP ni OCR: el puente
// proxea esos casos al hijo Node sin regresión.

fn click_at(x: i32, y: i32) {
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(x, y);
        std::thread::sleep(std::time::Duration::from_millis(20));
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, mouse_event,
        };
        mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
        mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
    }
}

fn double_click_at(x: i32, y: i32) {
    click_at(x, y);
    std::thread::sleep(std::time::Duration::from_millis(60));
    click_at(x, y);
}

fn right_click_at(x: i32, y: i32) {
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(x, y);
        std::thread::sleep(std::time::Duration::from_millis(20));
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, mouse_event,
        };
        mouse_event(MOUSEEVENTF_RIGHTDOWN, 0, 0, 0, 0);
        mouse_event(MOUSEEVENTF_RIGHTUP, 0, 0, 0, 0);
    }
}

fn scroll_at(x: i32, y: i32, dy: i32) {
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(x, y);
        std::thread::sleep(std::time::Duration::from_millis(20));
        use windows::Win32::UI::Input::KeyboardAndMouse::{MOUSEEVENTF_WHEEL, mouse_event};
        mouse_event(MOUSEEVENTF_WHEEL, 0, 0, dy, 0);
    }
}

fn send_key_tap(vk: u8) {
    unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::{KEYBD_EVENT_FLAGS, keybd_event};
        keybd_event(vk, 0, KEYBD_EVENT_FLAGS(0), 0);
        std::thread::sleep(std::time::Duration::from_millis(25));
        keybd_event(vk, 0, KEYBD_EVENT_FLAGS(2), 0);
    }
}

fn send_hotkey(mods: &[u8], vk: u8) {
    unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::{KEYBD_EVENT_FLAGS, keybd_event};
        for m in mods {
            keybd_event(*m, 0, KEYBD_EVENT_FLAGS(0), 0);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        keybd_event(vk, 0, KEYBD_EVENT_FLAGS(0), 0);
        std::thread::sleep(std::time::Duration::from_millis(25));
        keybd_event(vk, 0, KEYBD_EVENT_FLAGS(2), 0);
        for m in mods.iter().rev() {
            keybd_event(*m, 0, KEYBD_EVENT_FLAGS(2), 0);
        }
    }
}

fn clear_edit() {
    send_hotkey(&[0x11], 0x41); // Ctrl+A
    std::thread::sleep(std::time::Duration::from_millis(30));
    send_key_tap(0x08); // Backspace
}

fn type_via_clipboard(text: &str) {
    let _ = set_clipboard_text(text);
    std::thread::sleep(std::time::Duration::from_millis(30));
    unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::{KEYBD_EVENT_FLAGS, keybd_event};
        keybd_event(0x11, 0, KEYBD_EVENT_FLAGS(0), 0); // Ctrl
        keybd_event(0x56, 0, KEYBD_EVENT_FLAGS(0), 0); // V
        std::thread::sleep(std::time::Duration::from_millis(20));
        keybd_event(0x56, 0, KEYBD_EVENT_FLAGS(2), 0);
        keybd_event(0x11, 0, KEYBD_EVENT_FLAGS(2), 0);
    }
}

fn vk_from_key_extended(k: &str) -> u8 {
    match k.trim().to_lowercase().as_str() {
        "enter" | "return" => 0x0D,
        "tab" => 0x09,
        "escape" | "esc" => 0x1B,
        "backspace" | "bs" => 0x08,
        "delete" | "del" => 0x2E,
        "space" => 0x20,
        "up" | "arrowup" => 0x26,
        "down" | "arrowdown" => 0x28,
        "left" | "arrowleft" => 0x25,
        "right" | "arrowright" => 0x27,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "f1" => 0x70,
        "f2" => 0x71,
        "f3" => 0x72,
        "f4" => 0x73,
        "f5" => 0x74,
        "f6" => 0x75,
        "f7" => 0x76,
        "f8" => 0x77,
        "f9" => 0x78,
        "f10" => 0x79,
        "f11" => 0x7A,
        "f12" => 0x7B,
        "ctrl" | "control" => 0x11,
        "shift" => 0x10,
        "alt" => 0x12,
        "win" | "meta" | "super" | "cmd" => 0x5B,
        s if s.len() == 1
            && s.chars()
                .next()
                .map(|c| c.is_ascii_lowercase())
                .unwrap_or(false) =>
        {
            0x41 + (s.chars().next().unwrap_or('a') as u8 - b'a')
        }
        s if s.len() == 1
            && s.chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false) =>
        {
            0x30 + (s.chars().next().unwrap_or('0') as u8 - b'0')
        }
        _ => 0,
    }
}

fn find_element_dfs(
    walker: &IUIAutomationTreeWalker,
    el: &IUIAutomationElement,
    name_need: &str,
    id_need: &str,
    type_need: &str,
) -> Option<IUIAutomationElement> {
    unsafe {
        let c_name = el.CurrentName().map(bstr_to_string).unwrap_or_default();
        let c_aid = el
            .CurrentAutomationId()
            .map(bstr_to_string)
            .unwrap_or_default();
        let c_type = control_type_name(el.CurrentControlType().map(|v| v.0).unwrap_or(0));
        let name_ok =
            name_need.is_empty() || c_name.to_lowercase().contains(&name_need.to_lowercase());
        let id_ok = id_need.is_empty() || c_aid == id_need;
        let type_ok = type_need.is_empty() || c_type.eq_ignore_ascii_case(type_need);
        if name_ok
            && id_ok
            && type_ok
            && (!name_need.is_empty() || !id_need.is_empty() || !type_need.is_empty())
        {
            return Some(el.clone());
        }
        let mut ch = walker.GetFirstChildElement(el).ok();
        while let Some(c) = ch {
            if let Some(found) = find_element_dfs(walker, &c, name_need, id_need, type_need) {
                return Some(found);
            }
            ch = walker.GetNextSiblingElement(&c).ok();
        }
        None
    }
}

fn handle_act(args: &Value) -> ToolOut {
    // Casos que el origen resuelve con estado Node, CDP u OCR: el puente los proxea.
    if args.get("ref").is_some() || args.get("id").is_some() {
        return ToolOut::Json(
            json!({"ok": false, "error": "ref_or_id_not_supported_native"}),
            true,
        );
    }
    if args.get("target").map(|t| t.is_object()).unwrap_or(false) {
        return ToolOut::Json(
            json!({"ok": false, "error": "coordinates_target_not_supported_native"}),
            true,
        );
    }

    let action = args
        .get("action")
        .and_then(as_str)
        .unwrap_or("click")
        .to_string();
    const SUPPORTED: &[&str] = &[
        "click",
        "double_click",
        "right_click",
        "focus",
        "set_value",
        "clear",
        "toggle",
        "expand",
        "collapse",
        "select",
        "press",
        "scroll",
        "wait",
        "type",
    ];
    if !SUPPORTED.contains(&action.as_str()) {
        return ToolOut::Json(json!({"ok": false, "error": "unsupported_action"}), true);
    }

    // El modo humanized requiere humanClick/typeText del origen; no disponible nativo.
    if args.get("behavior").and_then(as_str) == Some("humanized") {
        return ToolOut::Json(
            json!({"ok": false, "error": "humanized_not_supported_native"}),
            true,
        );
    }

    let name_need = args.get("name").and_then(as_str).unwrap_or("").to_string();
    let id_need = args
        .get("automation_id")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();
    let type_need = args
        .get("control_type")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();

    let needs_element = !matches!(action.as_str(), "wait" | "press");
    if needs_element && name_need.is_empty() && id_need.is_empty() && type_need.is_empty() {
        return ToolOut::Json(
            json!({"ok": false, "error": "missing_target_criteria"}),
            true,
        );
    }

    if action == "type" && args.get("text").is_none() && args.get("secret_b64").is_none() {
        return ToolOut::Json(json!({"ok": false, "error": "type_requires_text"}), true);
    }
    if action == "set_value" && args.get("value").is_none() {
        return ToolOut::Json(
            json!({"ok": false, "error": "set_value_requires_value"}),
            true,
        );
    }
    if action == "press" && args.get("key").is_none() {
        return ToolOut::Json(json!({"ok": false, "error": "press_requires_key"}), true);
    }

    let hwnd = args.get("hwnd").and_then(as_i64).unwrap_or(0);
    let target_window = args
        .get("target_window")
        .and_then(as_str)
        .unwrap_or("")
        .to_string();

    // PORT NOTE: target_window enfoca la ventana en el origen; aquí no hay binding
    // de NativeWin.FocusWindow, así que el foco previo queda como best-effort.

    let auto = match create_automation() {
        Some(a) => a,
        None => return ToolOut::Json(json!({"ok": false, "error": "uia_init_failed"}), true),
    };

    let mut matched_el: Option<IUIAutomationElement> = None;
    let mut matched_name = String::new();
    let mut cx: i32 = 0;
    let mut cy: i32 = 0;

    if needs_element {
        let root = match root_element(&auto, hwnd, &target_window) {
            Some(r) => r,
            None => return ToolOut::Json(json!({"ok": false, "error": "window_not_found"}), true),
        };
        let walker = match unsafe { auto.ControlViewWalker() } {
            Ok(w) => w,
            Err(_) => return ToolOut::Json(json!({"ok": false, "error": "walker_failed"}), true),
        };

        if let Some(el) = find_element_dfs(&walker, &root, &name_need, &id_need, &type_need) {
            unsafe {
                matched_name = el.CurrentName().map(bstr_to_string).unwrap_or_default();
                let rect = el.CurrentBoundingRectangle().unwrap_or(RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                });
                cx = rect.left + (rect.right - rect.left) / 2;
                cy = rect.top + (rect.bottom - rect.top) / 2;
            }
            matched_el = Some(el);
        } else {
            return ToolOut::Json(json!({"ok": false, "error": "element_not_found"}), true);
        }
    }

    if let Some(ox) = args.get("offset_x").and_then(as_i64) {
        cx += ox as i32;
    }
    if let Some(oy) = args.get("offset_y").and_then(as_i64) {
        cy += oy as i32;
    }

    let mut typed = false;
    let mut key_pressed: Option<String> = None;
    let target_name = if matched_name.is_empty() {
        if action == "wait" || action == "press" {
            "current_focus".to_string()
        } else {
            format!("[{}, {}]", cx, cy)
        }
    } else {
        matched_name.clone()
    };

    match action.as_str() {
        "click" => click_at(cx, cy),
        "double_click" => double_click_at(cx, cy),
        "right_click" => right_click_at(cx, cy),
        "focus" => {
            if let Some(el) = &matched_el {
                unsafe {
                    let _ = el.SetFocus();
                }
            }
        }
        "set_value" => {
            let value = args.get("value").and_then(as_str).unwrap_or("");
            if let Some(el) = &matched_el {
                unsafe {
                    if let Ok(vp) =
                        el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                    {
                        let bstr = windows::core::BSTR::from(value);
                        let _ = vp.SetValue(&bstr);
                    }
                }
            }
            typed = !value.is_empty();
        }
        "clear" => {
            if let Some(el) = &matched_el {
                unsafe {
                    if let Ok(vp) =
                        el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                    {
                        let bstr = windows::core::BSTR::from("");
                        let _ = vp.SetValue(&bstr);
                    } else {
                        let _ = el.SetFocus();
                        clear_edit();
                    }
                }
            }
        }
        "toggle" => {
            if let Some(el) = &matched_el {
                unsafe {
                    if let Ok(tp) = el.GetCurrentPatternAs::<windows::Win32::UI::Accessibility::IUIAutomationTogglePattern>(UIA_TogglePatternId) {
                        let _ = tp.Toggle();
                    }
                }
            }
        }
        "expand" => {
            if let Some(el) = &matched_el {
                unsafe {
                    if let Ok(ep) = el.GetCurrentPatternAs::<windows::Win32::UI::Accessibility::IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId) {
                        let _ = ep.Expand();
                    }
                }
            }
        }
        "collapse" => {
            if let Some(el) = &matched_el {
                unsafe {
                    if let Ok(ep) = el.GetCurrentPatternAs::<windows::Win32::UI::Accessibility::IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId) {
                        let _ = ep.Collapse();
                    }
                }
            }
        }
        "select" => {
            if let Some(el) = &matched_el {
                unsafe {
                    if let Ok(sp) = el.GetCurrentPatternAs::<windows::Win32::UI::Accessibility::IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId) {
                        let _ = sp.Select();
                    }
                }
            }
        }
        "press" => {
            if let Some(key) = args.get("key").and_then(as_str) {
                let combo = key.trim();
                if combo.contains('+') {
                    let parts: Vec<&str> = combo
                        .split('+')
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .collect();
                    let mut mods: Vec<u8> = Vec::new();
                    let mut main_key = String::new();
                    for p in parts {
                        match p.to_lowercase().as_str() {
                            "ctrl" | "control" => mods.push(0x11),
                            "shift" => mods.push(0x10),
                            "alt" => mods.push(0x12),
                            "win" | "meta" | "super" | "cmd" => mods.push(0x5B),
                            _ => main_key = p.to_string(),
                        }
                    }
                    let vk = vk_from_key_extended(&main_key);
                    if vk != 0 {
                        send_hotkey(&mods, vk);
                        key_pressed = Some(key.to_string());
                    }
                } else {
                    let vk = vk_from_key_extended(combo);
                    if vk != 0 {
                        send_key_tap(vk);
                        key_pressed = Some(key.to_string());
                    }
                }
            }
        }
        "scroll" => {
            let dy = args.get("scroll_amount").and_then(as_i64).unwrap_or(-400) as i32;
            scroll_at(cx, cy, dy);
        }
        "wait" => {
            let ms = args
                .get("wait_ms")
                .and_then(as_i64)
                .unwrap_or(0)
                .clamp(0, 30000);
            if ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(ms as u64));
            }
        }
        "type" => {
            let text = if args.get("secret_b64").is_some() {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD
                    .decode(args.get("secret_b64").and_then(as_str).unwrap_or(""))
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
                    .unwrap_or_default()
            } else {
                args.get("text").and_then(as_str).unwrap_or("").to_string()
            };
            if !text.is_empty() {
                if let Some(el) = &matched_el {
                    unsafe {
                        let _ = el.SetFocus();
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(40));
                if args.get("clear_first").and_then(as_bool).unwrap_or(true) {
                    clear_edit();
                    std::thread::sleep(std::time::Duration::from_millis(30));
                }
                type_via_clipboard(&text);
                typed = true;
            }
        }
        _ => {}
    }

    // wait_ms post-acción (compatible con origen)
    if action != "wait" {
        if let Some(ms) = args.get("wait_ms").and_then(as_i64) {
            let ms = ms.clamp(0, 30000);
            if ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(ms as u64));
            }
        }
    }

    // press_enter post-acción
    if args.get("press_enter").and_then(as_bool).unwrap_or(false) {
        std::thread::sleep(std::time::Duration::from_millis(40));
        send_key_tap(0x0D);
        key_pressed = Some("Enter".to_string());
    }

    // observe_after
    let observation_after = if args.get("observe_after").and_then(as_bool).unwrap_or(false) {
        let obs = handle_observe(&json!({"limit": 15, "format": "table"}));
        match obs {
            ToolOut::Json(v, _) => v.get("summary").cloned(),
            ToolOut::JsonStructured(v, _) => v.get("summary").cloned(),
            _ => None,
        }
    } else {
        None
    };

    let epoch = cu_core::epoch::bump();
    let mut payload = json!({
        "ok": true,
        "action": action,
        "target": target_name,
        "method_used": if matched_name.is_empty() { "direct" } else { "native_uia_lookup" },
        "typed": typed,
        "key_pressed": key_pressed,
        "observation_after": observation_after
    });
    payload["epoch"] = json!(epoch);
    ToolOut::Json(payload, false)
}

// ---------------- lib entry ----------------

pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "computer_inspect_ui"
            | "computer_snapshot"
            | "computer_state_fingerprint"
            | "computer_verify"
            | "computer_find_element"
            | "computer_map_ui"
            | "computer_find_template"
            | "computer_ui_action"
            | "computer_read_selection"
            | "computer_observe"
            | "computer_system_tray"
            | "computer_context_menu"
            | "computer_toast_action"
            | "computer_background_action"
            | "computer_click_text"
            | "computer_wait_stable"
            | "computer_fill_form"
            | "computer_scroll_to_text"
            | "computer_act"
    )
}

// accepts: subconjunto de args servible nativo (default true; el puente consulta esto).
pub fn accepts(name: &str, args: &Value) -> bool {
    match name {
        "computer_find_element" => {
            let has = args.get("title").is_some()
                || args.get("hwnd").is_some()
                || args.get("automation_id").is_some()
                || args.get("control_type").is_some()
                || args.get("name").is_some();
            has
        }
        "computer_read_selection" => true,
        "computer_observe" => !args.get("screenshot").and_then(as_bool).unwrap_or(false),
        "computer_ui_action" => args.get("ref").is_none(),
        "computer_system_tray" => {
            // PORT NOTE: el overflow de bandeja no se enumera nativamente.
            // Solo aceptamos llamadas que no requieran overflow; el resto va al hijo Node.
            !args
                .get("include_overflow")
                .and_then(as_bool)
                .unwrap_or(true)
        }
        "computer_context_menu" => args.get("x").is_some() && args.get("y").is_some(),
        "computer_toast_action" => {
            args.get("action").and_then(as_str).unwrap_or("list") == "click_button"
        }
        "computer_background_action" => true,
        "computer_click_text" => args
            .get("text")
            .and_then(as_str)
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false),
        "computer_wait_stable" => {
            // PORT NOTE: solo el camino síncrono es nativo; async/task sigue en el hijo Node.
            !args.get("async").and_then(as_bool).unwrap_or(false) && args.get("task").is_none()
        }
        "computer_fill_form" => args
            .get("fields")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "computer_scroll_to_text" => args
            .get("text")
            .and_then(as_str)
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false),
        "computer_find_template" => args
            .get("template_b64")
            .and_then(|v| v.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false),
        "computer_act" => {
            // Solo subconjunto nativo: target por name/automation_id/control_type y
            // acciones click/double_click/right_click/focus/set_value/clear/toggle/
            // expand/collapse/select/press/scroll/wait/type.
            if args.get("ref").is_some() || args.get("id").is_some() {
                return false;
            }
            if args.get("target").map(|t| t.is_object()).unwrap_or(false) {
                return false;
            }
            let action = args.get("action").and_then(as_str).unwrap_or("click");
            const SUPPORTED: &[&str] = &[
                "click",
                "double_click",
                "right_click",
                "focus",
                "set_value",
                "clear",
                "toggle",
                "expand",
                "collapse",
                "select",
                "press",
                "scroll",
                "wait",
                "type",
            ];
            if !SUPPORTED.contains(&action) {
                return false;
            }
            // humanized requiere primitivas del origen no portadas.
            if args.get("behavior").and_then(as_str) == Some("humanized") {
                return false;
            }
            // type/set_value/press requieren sus argumentos.
            if action == "type" && args.get("text").is_none() && args.get("secret_b64").is_none() {
                return false;
            }
            if action == "set_value" && args.get("value").is_none() {
                return false;
            }
            if action == "press" && args.get("key").is_none() {
                return false;
            }
            // wait y press no necesitan elemento; el resto sí.
            let needs_criteria = !matches!(action, "wait" | "press");
            if needs_criteria {
                let has_criteria = args.get("name").is_some()
                    || args.get("automation_id").is_some()
                    || args.get("control_type").is_some();
                if !has_criteria {
                    return false;
                }
            }
            true
        }
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
        "computer_ui_action" => handle_ui_action(args),
        "computer_read_selection" => handle_read_selection(args),
        "computer_observe" => handle_observe(args),
        "computer_system_tray" => handle_system_tray(args),
        "computer_context_menu" => handle_context_menu(args),
        "computer_toast_action" => handle_toast_action(args),
        "computer_background_action" => handle_background_action(args),
        "computer_click_text" => handle_click_text(args),
        "computer_wait_stable" => handle_wait_stable(args),
        "computer_fill_form" => handle_fill_form(args),
        "computer_scroll_to_text" => handle_scroll_to_text(args),
        "computer_act" => handle_act(args),
        _ => ToolOut::Json(json!({"ok": false, "error": "not_implemented"}), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_act_click_with_name() {
        let args = json!({"action": "click", "name": "Aceptar"});
        assert!(accepts("computer_act", &args));
    }

    #[test]
    fn accepts_act_with_automation_id() {
        let args = json!({"action": "set_value", "automation_id": "txtUser", "value": "x"});
        assert!(accepts("computer_act", &args));
    }

    #[test]
    fn accepts_act_with_control_type() {
        let args = json!({"action": "toggle", "control_type": "CheckBox"});
        assert!(accepts("computer_act", &args));
    }

    #[test]
    fn accepts_act_wait_no_criteria() {
        let args = json!({"action": "wait", "wait_ms": 100});
        assert!(accepts("computer_act", &args));
    }

    #[test]
    fn accepts_act_press_no_criteria() {
        let args = json!({"action": "press", "key": "enter"});
        assert!(accepts("computer_act", &args));
    }

    #[test]
    fn accepts_act_false_for_ref() {
        let args = json!({"action": "click", "ref": "Button:btnOk:::"});
        assert!(!accepts("computer_act", &args));
    }

    #[test]
    fn accepts_act_false_for_id() {
        let args = json!({"action": "click", "id": 3});
        assert!(!accepts("computer_act", &args));
    }

    #[test]
    fn accepts_act_false_for_coordinates_target() {
        let args = json!({"action": "click", "target": {"x": 100, "y": 200}});
        assert!(!accepts("computer_act", &args));
    }

    #[test]
    fn accepts_act_false_for_humanized() {
        let args = json!({"action": "click", "name": "Aceptar", "behavior": "humanized"});
        assert!(!accepts("computer_act", &args));
    }

    #[test]
    fn accepts_act_false_for_unsupported_action() {
        let args = json!({"action": "drag", "name": "x"});
        assert!(!accepts("computer_act", &args));
    }

    #[test]
    fn accepts_act_type_requires_text() {
        let args = json!({"action": "type", "name": "x"});
        assert!(!accepts("computer_act", &args));
        let args2 = json!({"action": "type", "name": "x", "text": "hola"});
        assert!(accepts("computer_act", &args2));
    }

    // handle_act se prueba con hwnd inválido para no inyectar input real.
    #[test]
    fn handle_act_rejects_ref() {
        let args = json!({"action": "click", "name": "x", "ref": "y"});
        let out = handle_act(&args);
        match out {
            ToolOut::Json(v, is_err) => {
                assert!(is_err);
                assert_eq!(v["ok"], false);
                assert_eq!(v["error"], "ref_or_id_not_supported_native");
            }
            _ => panic!("expected Json"),
        }
    }

    #[test]
    fn handle_act_missing_criteria() {
        let args = json!({"action": "click"});
        let out = handle_act(&args);
        match out {
            ToolOut::Json(v, is_err) => {
                assert!(is_err);
                assert_eq!(v["ok"], false);
                assert_eq!(v["error"], "missing_target_criteria");
            }
            _ => panic!("expected Json"),
        }
    }

    #[test]
    fn handle_act_wait_safe() {
        let args = json!({"action": "wait", "wait_ms": 0});
        let out = handle_act(&args);
        match out {
            ToolOut::Json(v, is_err) => {
                assert!(!is_err);
                assert_eq!(v["ok"], true);
                assert_eq!(v["action"], "wait");
                assert!(v["epoch"].is_number());
            }
            _ => panic!("expected Json"),
        }
    }

    #[test]
    fn handle_act_invalid_hwnd_returns_window_not_found() {
        // hwnd 1 es inválido como ventana UIA; no inyecta input.
        let args = json!({"action": "click", "name": "NoExiste", "hwnd": 1});
        let out = handle_act(&args);
        match out {
            ToolOut::Json(v, is_err) => {
                assert!(is_err);
                assert_eq!(v["ok"], false);
                assert_eq!(v["error"], "window_not_found");
            }
            _ => panic!("expected Json"),
        }
    }

    #[test]
    fn response_shape_order() {
        let args = json!({"action": "wait", "wait_ms": 0});
        let out = handle_act(&args);
        match out {
            ToolOut::Json(v, _) => {
                let keys: Vec<_> = v.as_object().unwrap().keys().collect();
                assert_eq!(
                    keys,
                    vec![
                        "ok",
                        "action",
                        "target",
                        "method_used",
                        "typed",
                        "key_pressed",
                        "observation_after",
                        "epoch"
                    ]
                );
            }
            _ => panic!("expected Json"),
        }
    }

    #[test]
    fn accepts_system_tray_overflow_default_false() {
        // include_overflow default es true en el origen; nosotros no lo soportamos nativo.
        let args = json!({"action": "list"});
        assert!(!accepts("computer_system_tray", &args));
    }

    #[test]
    fn accepts_system_tray_no_overflow_true() {
        let args = json!({"action": "list", "include_overflow": false});
        assert!(accepts("computer_system_tray", &args));
    }

    #[test]
    fn accepts_wait_stable_sync_true() {
        let args = json!({"timeout_ms": 1000});
        assert!(accepts("computer_wait_stable", &args));
    }

    #[test]
    fn accepts_wait_stable_async_false() {
        let args = json!({"async": true});
        assert!(!accepts("computer_wait_stable", &args));
    }

    #[test]
    fn accepts_wait_stable_task_false() {
        let args = json!({"task": "t1"});
        assert!(!accepts("computer_wait_stable", &args));
    }
}

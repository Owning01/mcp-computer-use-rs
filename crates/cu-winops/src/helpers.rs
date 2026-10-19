// PORT NOTE: helpers compartidos del cluster winops (aliases, bloqueo de apps, nombres de proceso).
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::ProcessStatus::{EnumProcesses, GetModuleBaseNameW};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};

pub fn is_app_blocked(target: &str) -> bool {
    if target.is_empty() {
        return false;
    }
    let lower = target.to_lowercase();
    const FRAGMENTS: &[&str] = &[
        "1password",
        "bitwarden",
        "dashlane",
        "lastpass",
        "nordpass",
        "proton pass",
        "keepass",
    ];
    FRAGMENTS.iter().any(|f| lower.contains(f))
}

pub fn alias_terms(needle: &str) -> Vec<String> {
    // Replica el mapa de aliases de focusWindowHelper / wait_for.
    let mut terms = vec![needle.to_string()];
    let lower = needle.trim().to_lowercase();
    let aliases: &[(&str, &[&str])] = &[
        ("notepad", &["bloc de notas", "notepad"]),
        ("bloc de notas", &["notepad", "bloc de notas"]),
        ("calculator", &["calculadora", "calc", "calculator"]),
        ("calculadora", &["calculator", "calc", "calculadora"]),
        ("calc", &["calculadora", "calculator", "calc"]),
        ("paint", &["paint", "mspaint"]),
        ("terminal", &["powershell", "cmd", "terminal", "símbolo del sistema"]),
        ("explorer", &["explorador de archivos", "explorer"]),
        ("settings", &["configuración", "ajustes", "settings"]),
        ("task manager", &["administrador de tareas", "task manager"]),
    ];
    for (k, arr) in aliases {
        if lower.contains(k) {
            for a in *arr {
                if !terms.iter().any(|t| t.eq_ignore_ascii_case(a)) {
                    terms.push((*a).to_string());
                }
            }
        }
    }
    terms
}

pub fn contains_ci(hay: &str, needle: &str) -> bool {
    hay.to_lowercase().contains(&needle.to_lowercase())
}

pub fn process_name(pid: u32) -> String {
    unsafe {
        let h = match OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) {
            Ok(h) => h,
            Err(_) => return String::new(),
        };
        let mut buf = [0u16; 260];
        let n = GetModuleBaseNameW(h, None, &mut buf);
        let _ = CloseHandle(h);
        if n == 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

pub fn is_process_name_blocked_or_match(pid: u32, terms: &[String]) -> bool {
    let name = process_name(pid);
    let lower = name.to_lowercase();
    terms.iter().any(|t| lower.contains(&t.to_lowercase()))
}

#[allow(dead_code)]
pub fn enum_pids() -> Vec<u32> {
    unsafe {
        let mut ids = vec![0u32; 4096];
        let mut needed = 0u32;
        if EnumProcesses(ids.as_mut_ptr(), (ids.len() * 4) as u32, &mut needed).is_err() {
            return Vec::new();
        }
        let count = (needed as usize) / std::mem::size_of::<u32>();
        ids.truncate(count);
        ids
    }
}
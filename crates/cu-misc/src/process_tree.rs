// cu-misc::process_tree — port de nativeProcessTree (index.js).
// Enumera procesos con CreateToolhelp32Snapshot/Process32First/Next + QueryFullProcessImageNameW.
// PORT NOTE: el origen usa Get-CimInstance Win32_Process (WMI); Toolhelp32 no expone CommandLine
// ni WorkingSet; CommandLine se intenta leer del PEB (best-effort, "" si no se puede).
use serde_json::{json, Value};
use std::collections::HashMap;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};
use windows::Wdk::System::Threading::{NtQueryInformationProcess, PROCESSINFOCLASS};
use windows::Win32::System::Threading::PROCESS_BASIC_INFORMATION;

#[derive(Clone)]
struct ProcInfo {
    pid: i64,
    parent_pid: i64,
    name: String,
    exe_path: String,
    command_line: String,
}

fn enum_processes(include_cmd: bool) -> Vec<ProcInfo> {
    let mut out: Vec<ProcInfo> = Vec::new();
    unsafe {
        let snap = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(_) => return out,
        };
        let mut pe = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut pe).is_ok() {
            loop {
                let pid = pe.th32ProcessID as i64;
                let parent = pe.th32ParentProcessID as i64;
                let name = String::from_utf16_lossy(
                    &pe.szExeFile[..pe.szExeFile.iter().position(|&c| c == 0).unwrap_or(pe.szExeFile.len())],
                );
                let (exe_path, command_line) = query_details(pid, include_cmd);
                out.push(ProcInfo {
                    pid,
                    parent_pid: parent,
                    name,
                    exe_path,
                    command_line,
                });
                if Process32NextW(snap, &mut pe).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

fn query_details(pid: i64, include_cmd: bool) -> (String, String) {
    if pid <= 0 {
        return (String::new(), String::new());
    }
    unsafe {
        let handle = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, false, pid as u32) {
            Ok(h) => h,
            Err(_) => {
                // try without VM_READ
                match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid as u32) {
                    Ok(h) => h,
                    Err(_) => return (String::new(), String::new()),
                }
            }
        };
        // exe path
        let mut buf = vec![0u16; 32768];
        let mut size = buf.len() as u32;
        let pw = windows::core::PWSTR(buf.as_mut_ptr());
        let exe = if QueryFullProcessImageNameW(handle, windows::Win32::System::Threading::PROCESS_NAME_WIN32, pw, &mut size).is_ok() {
            String::from_utf16_lossy(&buf[..size as usize])
        } else {
            String::new()
        };
        // command line
        let cmd = if include_cmd {
            read_command_line(handle).unwrap_or_default()
        } else {
            String::new()
        };
        let _ = CloseHandle(handle);
        (exe, cmd)
    }
}

unsafe fn read_command_line(handle: HANDLE) -> Option<String> {
    // PROCESS_BASIC_INFORMATION
    let mut pbi = PROCESS_BASIC_INFORMATION::default();
    let mut ret: u32 = 0;
    let status = unsafe {
        NtQueryInformationProcess(
            handle,
            PROCESSINFOCLASS(0), // ProcessBasicInformation
            &mut pbi as *mut _ as *mut _,
            std::mem::size_of::<PROCESS_BASIC_INFORMATION>() as u32,
            &mut ret,
        )
    };
    if status.0 != 0 {
        return None;
    }
    let peb_addr = pbi.PebBaseAddress;
    if peb_addr.is_null() {
        return None;
    }
    // read PEB (first 0x30 bytes enough for ProcessParameters at 0x20)
    let mut peb_buf = [0u8; 0x30];
    let mut read = 0usize;
    if unsafe { ReadProcessMemory(handle, peb_addr as *const _, peb_buf.as_mut_ptr() as *mut _, peb_buf.len(), Some(&mut read)) }.is_err() {
        return None;
    }
    let params_ptr = usize::from_ne_bytes(peb_buf[0x20..0x28].try_into().ok()?);
    if params_ptr == 0 {
        return None;
    }
    // read RTL_USER_PROCESS_PARAMETERS header up to CommandLine (offset 0x70 + 16)
    let mut params_buf = vec![0u8; 0x80];
    let mut read2 = 0usize;
    if unsafe { ReadProcessMemory(handle, params_ptr as *const _, params_buf.as_mut_ptr() as *mut _, params_buf.len(), Some(&mut read2)) }.is_err() {
        return None;
    }
    // CommandLine UNICODE_STRING at offset 0x70: Length (u16), MaxLength (u16), padding, Buffer (usize)
    let len = u16::from_ne_bytes(params_buf[0x70..0x72].try_into().ok()?) as usize;
    let buf_ptr = usize::from_ne_bytes(params_buf[0x78..0x80].try_into().ok()?);
    if buf_ptr == 0 || len == 0 {
        return Some(String::new());
    }
    let mut str_buf = vec![0u8; len];
    let mut read3 = 0usize;
    if unsafe { ReadProcessMemory(handle, buf_ptr as *const _, str_buf.as_mut_ptr() as *mut _, str_buf.len(), Some(&mut read3)) }.is_err() {
        return None;
    }
    let u16s: Vec<u16> = str_buf
        .chunks_exact(2)
        .map(|c| u16::from_ne_bytes([c[0], c[1]]))
        .collect();
    Some(String::from_utf16_lossy(&u16s))
}

fn make_node(
    p: &ProcInfo,
    depth: i32,
    max_depth: i32,
    include_cmd: bool,
    by_parent: &HashMap<i64, Vec<usize>>,
    all: &[ProcInfo],
    idx_map: &HashMap<i64, usize>,
) -> Value {
    let mut o = serde_json::Map::new();
    o.insert("pid".into(), json!(p.pid));
    o.insert("parent_pid".into(), json!(p.parent_pid));
    o.insert("name".into(), json!(p.name));
    o.insert("depth".into(), json!(depth));
    if include_cmd {
        o.insert("command_line".into(), json!(p.command_line));
    }
    if !p.exe_path.is_empty() {
        o.insert("executable_path".into(), json!(p.exe_path));
    }
    // children
    let mut children: Vec<Value> = Vec::new();
    if depth < max_depth {
        if let Some(kids) = by_parent.get(&p.pid) {
            for &ki in kids {
                children.push(make_node(&all[ki], depth + 1, max_depth, include_cmd, by_parent, all, idx_map));
            }
        }
    }
    o.insert("children".into(), json!(children));
    Value::Object(o)
}

pub fn process_tree(args: &Value) -> Value {
    let max_depth = args.get("max_depth").and_then(|v| v.as_i64()).filter(|&d| d > 0).unwrap_or(8) as i32;
    let include_cmd = args.get("include_command_line").and_then(|v| v.as_bool()).unwrap_or(false);
    let target_pid = args.get("pid").and_then(|v| v.as_i64()).unwrap_or(0);

    let all = enum_processes(include_cmd);
    // index by pid
    let mut idx_map: HashMap<i64, usize> = HashMap::new();
    for (i, p) in all.iter().enumerate() {
        idx_map.insert(p.pid, i);
    }
    // by parent
    let mut by_parent: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, p) in all.iter().enumerate() {
        by_parent.entry(p.parent_pid).or_default().push(i);
    }

    let roots: Vec<Value> = if target_pid > 0 && idx_map.contains_key(&target_pid) {
        vec![make_node(&all[idx_map[&target_pid]], 0, max_depth, include_cmd, &by_parent, &all, &idx_map)]
    } else {
        // roots = processes whose parent not in byId
        let mut r = Vec::new();
        for (i, p) in all.iter().enumerate() {
            if !idx_map.contains_key(&p.parent_pid) {
                r.push(make_node(&all[i], 0, max_depth, include_cmd, &by_parent, &all, &idx_map));
            }
        }
        r
    };

    json!({
        "roots": roots,
        "process_count": all.len() as i64
    })
}
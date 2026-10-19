// Probe de cu-wgc: lanza notepad, captura su ventana, imprime dims + hash FNV.
// Uso: cargo run --release -p cu-wgc --example probe

use std::process::{Child, Command};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, IsWindowVisible};
use windows::core::BOOL;

use cu_wgc::capture;

static mut FOUND_HWND: isize = 0;

unsafe extern "system" fn enum_cb(hwnd: HWND, _: LPARAM) -> BOOL {
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return BOOL(1);
    }
    let mut buf = [0u16; 256];
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
    let title = String::from_utf16_lossy(&buf[..len as usize]);
    let lower = title.to_lowercase();
    unsafe {
        if FOUND_HWND == 0
            && (lower.contains("notepad")
                || lower.contains("bloc de notas")
                || lower.contains("untitled")
                || lower.contains("sin título"))
        {
            FOUND_HWND = hwnd.0 as isize;
            eprintln!(
                "probe: ventana encontrada: hwnd={} titulo={:?}",
                hwnd.0 as isize, title
            );
        }
    }
    BOOL(1)
}

fn find_notepad_window() -> Option<isize> {
    unsafe {
        FOUND_HWND = 0;
        let _ = EnumWindows(Some(enum_cb), LPARAM(0));
        if FOUND_HWND != 0 {
            Some(FOUND_HWND)
        } else {
            None
        }
    }
}

fn main() {
    let mut child: Child = match Command::new("notepad.exe").spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("probe: no se pudo lanzar notepad: {e}");
            return;
        }
    };
    thread::sleep(Duration::from_millis(2000));

    let hwnd = find_notepad_window();
    let result = match hwnd {
        Some(h) => capture(h as i64, 3000),
        None => Err("wgc_invalid_window".to_string()),
    };

    match result {
        Ok(frame) => {
            let expected = (frame.width as usize) * (frame.height as usize) * 4;
            println!(
                "probe: Ok width={} height={} buffer={} (expected {})",
                frame.width,
                frame.height,
                frame.bgra.len(),
                expected
            );
            println!("probe: fnv64={:#018x}", fnv1a64(&frame.bgra));
            if frame.width == 0 || frame.height == 0 || frame.bgra.len() != expected {
                eprintln!("probe: FAIL (dimensiones o buffer invalidos)");
            } else {
                println!("probe: PASS");
            }
        }
        Err(e) => {
            println!("probe: error={e}");
            if e.starts_with("wgc_") {
                println!("probe: error es wgc_* esperado (no panic)");
            } else {
                eprintln!("probe: error NO tiene prefijo wgc_");
            }
        }
    }

    let _ = child.kill();
    let _ = child.wait();
}

fn fnv1a64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

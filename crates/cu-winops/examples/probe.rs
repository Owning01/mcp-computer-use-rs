// Probe de solo lectura para cu-winops (no muta ventanas).
use cu_native::list_windows;
use cu_winops::monitor;

fn main() {
    println!("=== windows ===");
    for w in list_windows(None) {
        println!("hwnd={} title={:?} pid={}", w.hwnd, w.title, w.pid);
    }
    println!("=== monitors ===");
    let mons = monitor::list_monitors();
    for m in &mons {
        println!(
            "idx={} device={} primary={} bounds=({},{} {}x{}) work=({},{} {}x{}) bpp={}",
            m.index, m.device_name, m.primary,
            m.bounds_x, m.bounds_y, m.bounds_w, m.bounds_h,
            m.work_x, m.work_y, m.work_w, m.work_h, m.bits_per_pixel
        );
    }
    println!("monitors count={}", mons.len());
}
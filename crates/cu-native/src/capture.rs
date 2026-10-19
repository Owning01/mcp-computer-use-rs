// cu-native::capture — port de computer_screenshot (F5, repo-migrate).
// Contrato: src/handlers/capture.js + nativeScreenshot/nativePrintWindowCapture/
// finalizeCapturedPng de index.js. Hoy: gdi + printwindow + WGC real (cu-wgc, in-process
// con Windows.Graphics.Capture) y el chain auto: printwindow -> wgc -> gdi.
// Pendiente: cursor, annotate_marks, etag-diff server-side. Reglas: migration/RULEBOOK.md.
use image::imageops::{resize, FilterType};
use image::{ExtendedColorType, ImageEncoder, RgbaImage};
use serde_json::{Map, Value, json};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC, GetMonitorInfoW, HGDIOBJ,
    HMONITOR, MONITORINFO, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, MONITORINFOF_PRIMARY};

// PrintWindow (user32) vive en el namespace Xps del crate windows; binding local minimo.
#[link(name = "user32")]
unsafe extern "system" {
    fn PrintWindow(hwnd: HWND, hdcblt: windows::Win32::Graphics::Gdi::HDC, nflags: u32) -> BOOL;
}

const COLOR_BUCKET: f64 = 8.0;
const UNIFORM_RATIO_DEFAULT: f64 = 0.95;
const DARK_THRESHOLD_DEFAULT: f64 = 40.0;

pub struct ShotArgs {
    pub width: i64,
    pub format: String,
    pub quality: i64,
    pub hwnd: i64,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    pub screen: i64,
    pub capture: String,
    pub interpolation: String,
    pub wgc_timeout_ms: i64,
}

pub struct ShotOk {
    pub meta: Map<String, Value>,
    pub bytes: Vec<u8>,
    pub is_jpeg: bool,
}

pub enum ShotOut {
    Ok(ShotOk),
    Err { error: String, message: String, capture_method: String },
}

fn err(message: impl Into<String>, capture_method: &str) -> ShotOut {
    ShotOut::Err {
        error: "screenshot_failed".into(),
        message: message.into(),
        capture_method: capture_method.into(),
    }
}

// true si >= uniform_ratio de las muestras caen en el mismo bucket Y ese color es oscuro
// (port de isNearUniformFrame; buckets de 8 sobre rgb crudo).
fn is_near_uniform(samples: &[[u8; 3]]) -> bool {
    if samples.is_empty() {
        return false;
    }
    let mut counts: std::collections::HashMap<(u8, u8, u8), usize> = std::collections::HashMap::new();
    for s in samples {
        let key = (
            (s[0] as f64 / COLOR_BUCKET).floor() as u8,
            (s[1] as f64 / COLOR_BUCKET).floor() as u8,
            (s[2] as f64 / COLOR_BUCKET).floor() as u8,
        );
        *counts.entry(key).or_insert(0) += 1;
    }
    let valid = samples.len();
    let (&(br, bg, bb), &best) = counts.iter().max_by_key(|(_, c)| **c).unwrap();
    if (best as f64) / (valid as f64) < UNIFORM_RATIO_DEFAULT {
        return false;
    }
    let center = COLOR_BUCKET / 2.0;
    let luminance = (br as f64 * COLOR_BUCKET + center + bg as f64 * COLOR_BUCKET + center + bb as f64 * COLOR_BUCKET + center) / 3.0;
    luminance <= DARK_THRESHOLD_DEFAULT
}

fn sample_grid(img: &RgbaImage) -> Vec<[u8; 3]> {
    let w = img.width() as i32;
    let h = img.height() as i32;
    let step_x = (w / 20).max(1);
    let step_y = (h / 20).max(1);
    let mut out = Vec::with_capacity(400);
    for iy in 0..20 {
        for ix in 0..20 {
            let px = (ix * step_x).min(w - 1).max(0) as u32;
            let py = (iy * step_y).min(h - 1).max(0) as u32;
            let p = img.get_pixel(px, py);
            out.push([p[0], p[1], p[2]]);
        }
    }
    out
}

fn dib_capture<F>(w: i32, h: i32, draw: F) -> Option<RgbaImage>
where
    F: FnOnce(windows::Win32::Graphics::Gdi::HDC) -> bool,
{
    if w <= 0 || h <= 0 {
        return None;
    }
    unsafe {
        let screen_dc = GetDC(None);
        let mem_dc = CreateCompatibleDC(Some(screen_dc));
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let hbmp = match CreateDIBSection(Some(screen_dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
        {
            Ok(b) => b,
            Err(_) => {
                let _ = DeleteDC(mem_dc);
                ReleaseDC(None, screen_dc);
                return None;
            }
        };
        let old = SelectObject(mem_dc, HGDIOBJ(hbmp.0));
        let ok = draw(mem_dc);
        let img = if ok && !bits.is_null() {
            let src = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
            let mut img = RgbaImage::new(w as u32, h as u32);
            for (i, px) in img.pixels_mut().enumerate() {
                let o = i * 4;
                // BGRA -> RGBA
                *px = image::Rgba([src[o + 2], src[o + 1], src[o], 255]);
            }
            Some(img)
        } else {
            None
        };
        SelectObject(mem_dc, old);
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
        let _ = DeleteDC(mem_dc);
        ReleaseDC(None, screen_dc);
        img
    }
}

fn gdi_region(x: i32, y: i32, w: i32, h: i32) -> Option<RgbaImage> {
    unsafe {
        let screen_dc = GetDC(None);
        let img = dib_capture(w, h, move |mem_dc| {
            BitBlt(mem_dc, 0, 0, w, h, Some(screen_dc), x, y, SRCCOPY).is_ok()
        });
        ReleaseDC(None, screen_dc);
        img
    }
}

struct PwCapture {
    img: RgbaImage,
    flag: u32,
    rect: RECT,
}

fn printwindow_capture(hwnd_raw: i64) -> Result<PwCapture, String> {
    let hwnd = HWND(hwnd_raw as isize as *mut core::ffi::c_void);
    let mut rect = RECT::default();
    unsafe {
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return Err("window_not_found".into());
        }
    }
    let w = (rect.right - rect.left).max(1);
    let h = (rect.bottom - rect.top).max(1);
    for flag in [2u32, 0u32] {
        let img = dib_capture(w, h, |mem_dc| unsafe {
            PrintWindow(hwnd, mem_dc, flag).as_bool()
        });
        if let Some(img) = img {
            return Ok(PwCapture { img, flag, rect });
        }
    }
    Err("printwindow_failed".into())
}

fn monitors() -> (Vec<RECT>, Option<RECT>) {
    let mut rects: Vec<RECT> = Vec::new();
    unsafe extern "system" fn cb(hmon: HMONITOR, _hdc: windows::Win32::Graphics::Gdi::HDC, _rc: *mut RECT, lparam: LPARAM) -> BOOL {
        unsafe {
            let state = &mut *(lparam.0 as *mut (Vec<RECT>, Option<RECT>));
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(hmon, &mut mi).as_bool() {
                state.0.push(mi.rcMonitor);
                if mi.dwFlags & MONITORINFOF_PRIMARY != 0 {
                    state.1 = Some(mi.rcMonitor);
                }
            }
            BOOL(1)
        }
    }
    let mut state = (Vec::<RECT>::new(), None::<RECT>);
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut state as *mut _ as isize));
    }
    rects.append(&mut state.0);
    (rects, state.1)
}

// union anclada en cero, calcada del script PS (vLeft/vTop arrancan en 0).
fn virtual_screen(rects: &[RECT]) -> (i32, i32, i32, i32) {
    let (mut vl, mut vt, mut vr, mut vb) = (0i32, 0i32, 0i32, 0i32);
    for r in rects {
        if r.left < vl {
            vl = r.left;
        }
        if r.top < vt {
            vt = r.top;
        }
        if r.right > vr {
            vr = r.right;
        }
        if r.bottom > vb {
            vb = r.bottom;
        }
    }
    (vl, vt, vr - vl, vb - vt)
}

fn round4(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}

// WGC real (cu-wgc in-process): frame BGRA -> RGBA -> PNG (el origen transcodifica WGC
// siempre a PNG) + meta WGC. PORT NOTE: sin `samples` del helper C# (solo diagnóstico);
// non_black_percent propio sobre el frame.
fn wgc_shot(
    hwnd: i64,
    target_width: i64,
    quality: u8,
    interp: FilterType,
    timeout_ms: i64,
    vs: (i32, i32, i32, i32),
) -> Result<ShotOk, (String, String)> {
    let frame = cu_wgc::capture(hwnd, timeout_ms.clamp(500, 60000) as u64)
        .map_err(|e| (e.clone(), format!("WGC no pudo capturar la ventana: {e}")))?;
    let cu_wgc::Frame {
        bgra,
        width,
        height,
    } = frame;
    let mut rgba = bgra;
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let Some(img) = RgbaImage::from_raw(width, height, rgba) else {
        return Err(("wgc_bad_output".into(), "frame WGC con buffer inesperado".into()));
    };
    let ow = img.width();
    let oh = img.height();
    let non_black = non_black_percent(&img);
    let Some((bytes, _is_jpeg_out, w, h, sf, _)) =
        finalize(&img, target_width, false, quality, interp)
    else {
        return Err(("wgc_convert_failed".into(), "no se pudo codificar el frame WGC".into()));
    };
    // WGC captura el content area (DWM extended frame bounds), no GetWindowRect.
    let (sx, sy) = window_content_rect(hwnd);
    let mut m = base_meta(w, h, ow, oh, sf, "png", sx, sy, vs);
    m.insert("capture_method".into(), json!("wgc"));
    m.insert("capture_used".into(), json!("wgc"));
    m.insert("wgc_non_black_percent".into(), json!(non_black));
    m.insert("wgc_native_width".into(), json!(ow));
    m.insert("wgc_native_height".into(), json!(oh));
    Ok(ShotOk {
        meta: m,
        bytes,
        is_jpeg: false,
    })
}

fn non_black_percent(img: &RgbaImage) -> f64 {
    let total = (img.width() as u64) * (img.height() as u64);
    if total == 0 {
        return 0.0;
    }
    let mut nb = 0u64;
    for px in img.pixels() {
        let [r, g, b, _a] = px.0;
        if r != 0 || g != 0 || b != 0 {
            nb += 1;
        }
    }
    ((nb as f64) * 10000.0 / (total as f64)).round() / 100.0
}

fn window_content_rect(hwnd: i64) -> (i32, i32) {
    use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    let hw = HWND(hwnd as isize as *mut core::ffi::c_void);
    let mut r = RECT::default();
    let res = unsafe {
        DwmGetWindowAttribute(
            hw,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut RECT as *mut core::ffi::c_void,
            std::mem::size_of::<RECT>() as u32,
        )
    };
    if res.is_ok() {
        (r.left, r.top)
    } else {
        (0, 0)
    }
}

fn interpolation(value: &str) -> FilterType {
    // map del origen: nearest -> NearestNeighbor; resto (incl. default) -> HighQualityBicubic.
    // PORT NOTE: image crate no tiene "HighQualityBicubic"; CatmullRom es la familia bicubica.
    match value.to_lowercase().as_str() {
        "nearest" => FilterType::Nearest,
        _ => FilterType::CatmullRom,
    }
}

// finalize del origen: PNG con downscale adaptativo si supera 900KB, luego (si jpeg) transcodifica.
fn finalize(
    img: &RgbaImage,
    target_width: i64,
    is_jpeg: bool,
    quality: u8,
    interp: FilterType,
) -> Option<(Vec<u8>, bool, u32, u32, f64, u32)> {
    let orig_w = img.width();
    let orig_h = img.height();
    let mut current: RgbaImage = if target_width > 0 && target_width != orig_w as i64 {
        let scale = target_width as f64 / orig_w as f64;
        let target_h = (orig_h as f64 * scale) as u32;
        resize(img, target_width as u32, target_h.max(1), interp)
    } else {
        img.clone()
    };

    // PNG staging + downscale adaptativo (patron Orca; aplica tambien en modo jpeg).
    let mut png_bytes = encode_png(&current)?;
    let max_bytes = 900_000usize;
    if png_bytes.len() > max_bytes {
        let mut cur_scale = 0.85f64;
        while cur_scale >= 0.25 {
            let rw = ((current.width() as f64 * cur_scale).round() as i64).max(1) as u32;
            let rh = ((current.height() as f64 * cur_scale).round() as i64).max(1) as u32;
            let res = resize(&current, rw, rh, FilterType::Triangle);
            let res_bytes = encode_png(&res)?;
            if res_bytes.len() <= max_bytes || cur_scale <= 0.3 {
                png_bytes = res_bytes;
                current = res;
                break;
            }
            cur_scale *= 0.85;
        }
    }

    let (bytes, is_jpeg_out) = if is_jpeg {
        (encode_jpeg(&current, quality)?, true)
    } else {
        (png_bytes, false)
    };
    let sf = round4(current.width() as f64 / orig_w.max(1) as f64);
    Some((bytes, is_jpeg_out, current.width(), current.height(), sf, 0))
}

fn encode_png(img: &RgbaImage) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buf)
        .write_image(img.as_raw(), img.width(), img.height(), ExtendedColorType::Rgba8)
        .ok()?;
    Some(buf)
}

fn encode_jpeg(img: &RgbaImage, quality: u8) -> Option<Vec<u8>> {
    // El encoder JPEG de image no acepta Rgba8: se convierte a RGB8 (mismo resultado visual
    // que el Save() de System.Drawing, que ignora alfa).
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).into_rgb8();
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality)
        .encode(rgb.as_raw(), rgb.width(), rgb.height(), ExtendedColorType::Rgb8)
        .ok()?;
    Some(buf)
}

fn base_meta(width: u32, height: u32, ow: u32, oh: u32, sf: f64, format: &str, screen_x: i32, screen_y: i32, vs: (i32, i32, i32, i32)) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("width".into(), json!(width));
    m.insert("height".into(), json!(height));
    m.insert("original_width".into(), json!(ow));
    m.insert("original_height".into(), json!(oh));
    m.insert("scale_factor".into(), json!(sf));
    m.insert("format".into(), json!(format));
    m.insert("screen_x".into(), json!(screen_x));
    m.insert("screen_y".into(), json!(screen_y));
    m.insert(
        "virtual_screen".into(),
        json!({ "left": vs.0, "top": vs.1, "width": vs.2, "height": vs.3 }),
    );
    m
}

pub fn screenshot(a: &ShotArgs) -> ShotOut {
    // DPI: replica el PS del origen (proceso unaware -> coords virtualizadas). Contexto
    // explicito por llamada: los threads del pool de spawn_blocking se reusan.
    unsafe {
        use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_UNAWARE, SetThreadDpiAwarenessContext};
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_UNAWARE);
    }
    let quality = a.quality.clamp(10, 95) as u8;
    let target_width = a.width.max(0);
    let is_jpeg = !a.format.eq_ignore_ascii_case("png");
    let hwnd = a.hwnd;
    let mode = match a.capture.to_lowercase().as_str() {
        "gdi" => "gdi",
        "printwindow" => "printwindow",
        "wgc" => "wgc",
        _ => "auto",
    };

    let (meta_rects, primary) = monitors();
    let vs = virtual_screen(&meta_rects);
    let interp = interpolation(&a.interpolation);

    if mode == "wgc" {
        if hwnd <= 0 {
            return ShotOut::Err {
                error: "no_hwnd".into(),
                message: "capture:'wgc' requiere el parámetro hwnd de una ventana visible.".into(),
                capture_method: "wgc".into(),
            };
        }
        return match wgc_shot(hwnd, target_width, quality, interp, a.wgc_timeout_ms, vs) {
            Ok(ok) => ShotOut::Ok(ok),
            Err((code, msg)) => ShotOut::Err {
                error: code,
                message: msg,
                capture_method: "wgc".into(),
            },
        };
    }

    let mut pw_fallback: Option<String> = None;
    let mut pw: Option<PwCapture> = None;
    if mode == "printwindow" || (mode == "auto" && hwnd > 0) {
        match printwindow_capture(hwnd) {
            Ok(p) => pw = Some(p),
            Err(e) => pw_fallback = Some(e),
        }
    } else if mode == "printwindow" {
        pw_fallback = Some("no_hwnd".into());
    }

    if let Some(p) = pw {
        let samples = sample_grid(&p.img);
        if mode == "printwindow" || !is_near_uniform(&samples) {
            let ow = p.img.width();
            let oh = p.img.height();
            let Some((bytes, is_jpeg_out, w, h, sf, _)) = finalize(&p.img, target_width, is_jpeg, quality, interp) else {
                return err("encode_failed", "printwindow");
            };
            let mut m = base_meta(w, h, ow, oh, sf, if is_jpeg_out { "jpeg" } else { "png" }, p.rect.left, p.rect.top, vs);
            m.insert("capture_method".into(), json!("printwindow"));
            m.insert("capture_used".into(), json!("printwindow"));
            m.insert("printwindow_flag".into(), json!(p.flag));
            return ShotOut::Ok(ShotOk { meta: m, bytes, is_jpeg: is_jpeg_out });
        }
        pw_fallback = Some("near_uniform_frame".into());
    }

    let mut wgc_fallback: Option<String> = None;
    if mode == "auto" && hwnd > 0 {
        // PrintWindow dio frame negro/uniforme (o fallo): se intenta WGC real antes de caer a GDI.
        match wgc_shot(hwnd, target_width, quality, interp, a.wgc_timeout_ms, vs) {
            Ok(mut ok) => {
                ok.meta
                    .insert("capture_fallback_from".into(), json!("printwindow"));
                ok.meta
                    .insert("capture_fallback_reason".into(), json!("near_uniform_frame"));
                return ShotOut::Ok(ok);
            }
            Err((code, _msg)) => wgc_fallback = Some(code),
        }
    }

    // GDI: rect de ventana (si falla, cae a region/monitor como el origen), region explicita o monitor.
    let mut crop: Option<(i32, i32, i32, i32)> = None;
    if hwnd > 0 {
        let mut r = RECT::default();
        let hw = HWND(hwnd as isize as *mut core::ffi::c_void);
        unsafe {
            if GetWindowRect(hw, &mut r).is_ok() {
                crop = Some((r.left, r.top, (r.right - r.left).max(10), (r.bottom - r.top).max(10)));
            }
        }
    }
    let (cx, cy, cw, ch) = if let Some(c) = crop {
        c
    } else if a.w != 0 && a.h != 0 {
        (a.x as i32, a.y as i32, a.w as i32, a.h as i32)
    } else {
        let idx = a.screen.max(0) as usize;
        let rect = if idx < meta_rects.len() {
            Some(meta_rects[idx])
        } else {
            primary.or_else(|| meta_rects.first().copied())
        };
        match rect {
            Some(r) => (r.left, r.top, r.right - r.left, r.bottom - r.top),
            None => return err("no_monitor", "gdi"),
        }
    };

    let Some(img) = gdi_region(cx, cy, cw, ch) else {
        return err("gdi_capture_failed", "gdi");
    };
    let ow = img.width();
    let oh = img.height();
    let Some((bytes, is_jpeg_out, w, h, sf, _)) = finalize(&img, target_width, is_jpeg, quality, interp) else {
        return err("encode_failed", "gdi");
    };
    let mut m = base_meta(w, h, ow, oh, sf, if is_jpeg_out { "jpeg" } else { "png" }, cx, cy, vs);
    m.insert("capture_method".into(), json!("gdi"));
    m.insert("capture_used".into(), json!("gdi"));
    if let Some(reason) = pw_fallback {
        m.insert("capture_fallback_from".into(), json!("printwindow"));
        m.insert("capture_fallback_reason".into(), json!(reason));
    }
    if let Some(w) = wgc_fallback {
        let combined = match m.get("capture_fallback_reason").and_then(Value::as_str) {
            Some(prev) => format!("{prev}; {w}"),
            None => w,
        };
        m.insert("capture_fallback_reason".into(), json!(combined));
        if !m.contains_key("capture_fallback_from") {
            m.insert("capture_fallback_from".into(), json!("wgc"));
        }
    }
    ShotOut::Ok(ShotOk { meta: m, bytes, is_jpeg: is_jpeg_out })
}

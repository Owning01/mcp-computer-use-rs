// computer_ocr: PaddleOCR persistente y fallback Windows.Media.Ocr del origen.
// PORT NOTE: el handler original usa windows_media_ocr como backend WinRT y completa
// device con windows_native al normalizar la respuesta.
use cu_core::out::ToolOut;
use serde_json::{Map, Value, json};
use std::env;
use std::future::{Future, IntoFuture};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Mutex, OnceLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
use windows::core::HSTRING;

const OCR_TIMEOUT: Duration = Duration::from_secs(30);

pub fn handles(name: &str) -> bool {
    name == "computer_ocr"
}

pub fn accepts(_name: &str, _args: &Value) -> bool {
    true
}

pub fn handle_call(name: &str, args: &Value) -> ToolOut {
    if !handles(name) {
        return ToolOut::Json(json!({"ok": false, "error": "not_implemented"}), true);
    }
    match computer_ocr(args) {
        Ok(payload) => ToolOut::Json(payload, false),
        Err(payload) => ToolOut::Json(payload, true),
    }
}

fn computer_ocr(args: &Value) -> Result<Value, Value> {
    let mut region = Region {
        x: number_i64(args.get("x")),
        y: number_i64(args.get("y")),
        w: number_i64(args.get("w")),
        h: number_i64(args.get("h")),
    };
    let hwnd = number_i64(args.get("hwnd"));
    if js_truthy(args.get("hwnd")) {
        region = window_rect(hwnd).map_err(|error| json!({"error": error}))?;
    }

    let capture_args = cu_native::capture::ShotArgs {
        x: region.x,
        y: region.y,
        w: region.w,
        h: region.h,
        width: 0,
        format: "png".into(),
        quality: 75,
        hwnd: 0,
        screen: 0,
        capture: "auto".into(),
        interpolation: String::new(),
        wgc_timeout_ms: 3000,
    };
    let screenshot = match cu_native::capture::screenshot(&capture_args) {
        cu_native::capture::ShotOut::Ok(ok) => ok,
        cu_native::capture::ShotOut::Err { error, message, .. } => {
            return Err(json!({"ok": false, "error": error, "message": message}));
        }
    };
    let image = encode_base64(&screenshot.bytes);
    let language = js_string_or(args.get("language"), "en");
    let device = js_string_or(
        args.get("device"),
        &env_or("COMPUTER_USE_OCR_DEVICE", "auto"),
    );
    let min_confidence = args
        .get("min_confidence")
        .filter(|v| !v.is_null())
        .and_then(Value::as_f64)
        .unwrap_or(0.35);
    let max_results = args
        .get("max_results")
        .filter(|value| js_truthy(Some(value)))
        .cloned()
        .unwrap_or_else(|| json!(200));
    let native_maximum = max_results
        .as_f64()
        .map(|value| value.round_ties_even() as i64)
        .filter(|value| *value > 0)
        .unwrap_or(200);
    let query = js_string_or(args.get("query"), "");
    let group_lines = js_truthy(args.get("group_lines"));

    let paddle = paddle_request(
        &image,
        &language,
        &device,
        min_confidence,
        &max_results,
        &query,
        group_lines,
    );
    let result = match paddle {
        Ok(value) if value.get("error").is_none() => value,
        Ok(_) | Err(_) => {
            match windows_media_ocr(
                &screenshot.bytes,
                &language,
                &query,
                group_lines,
                native_maximum,
            ) {
                Ok(value) => value,
                Err(error) => json!({"error": error}),
            }
        }
    };

    if result.get("error").is_some() {
        return Err(result);
    }
    let ox = if region.x == 0 { 0 } else { region.x };
    let oy = if region.y == 0 { 0 } else { region.y };
    let items = result
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut item| {
            if let Some(fields) = item.as_object_mut() {
                offset_field(fields, "x", ox);
                offset_field(fields, "y", oy);
            }
            item
        })
        .collect::<Vec<_>>();
    let backend = result.get("backend").cloned().unwrap_or(Value::Null);
    let result_device = result
        .get("device")
        .filter(|value| !value.is_null() && value.as_str().is_none_or(|s| !s.is_empty()))
        .cloned()
        .unwrap_or_else(|| json!("windows_native"));
    let width = screenshot.meta.get("width").cloned().unwrap_or(Value::Null);
    let height = screenshot
        .meta
        .get("height")
        .cloned()
        .unwrap_or(Value::Null);
    Ok(json!({
        "ok": true,
        "backend": backend,
        "device": result_device,
        "width": width,
        "height": height,
        "items": items
    }))
}

#[derive(Clone, Copy)]
struct Region {
    x: i64,
    y: i64,
    w: i64,
    h: i64,
}

fn window_rect(hwnd: i64) -> Result<Region, String> {
    let handle = HWND(hwnd as isize as *mut core::ffi::c_void);
    let mut rect = RECT::default();
    // SAFETY: Win32 recibe el HWND que envió el llamador y un RECT válido para escritura.
    unsafe { GetWindowRect(handle, &mut rect) }.map_err(|_| "window_not_found".to_owned())?;
    Ok(Region {
        x: i64::from(rect.left),
        y: i64::from(rect.top),
        w: i64::from(rect.right - rect.left),
        h: i64::from(rect.bottom - rect.top),
    })
}

fn offset_field(fields: &mut Map<String, Value>, key: &str, offset: i64) {
    let value = fields
        .get(key)
        .and_then(Value::as_i64)
        .map(|value| value.saturating_add(offset))
        .map_or(Value::Null, |value| json!(value));
    fields.insert(key.to_owned(), value);
}

fn number_i64(value: Option<&Value>) -> i64 {
    value
        .and_then(Value::as_i64)
        .or_else(|| value.and_then(Value::as_f64).map(|number| number as i64))
        .unwrap_or(0)
}

fn js_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        _ => true,
    }
}

fn js_string_or(value: Option<&Value>, default: &str) -> String {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .unwrap_or(default)
        .to_owned()
}

fn env_or(name: &str, default: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().saturating_add(2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        encoded.push(TABLE[(first >> 2) as usize] as char);
        encoded.push(TABLE[(((first & 0b11) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(TABLE[(((second & 0b1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(TABLE[(third & 0b11_1111) as usize] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<Value>,
    job: Option<isize>,
}

impl Worker {
    fn spawn() -> Result<Self, String> {
        let python = env_or("COMPUTER_USE_OCR_PYTHON", "python");
        let script = env::var_os("COMPUTER_USE_OCR_WORKER")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("python")
                    .join("ocr_worker.py")
            });
        let mut command = Command::new(python);
        command
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let job = attach_kill_on_close_job(&child);
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ocr_worker_stdin_unavailable".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "ocr_worker_stdout_unavailable".to_owned())?;
        if let Some(stderr) = child.stderr.take() {
            thread::spawn(move || {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            });
        }
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if let Ok(message) = serde_json::from_str::<Value>(line.trim()) {
                            if sender.send(message).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            lines,
            job,
        })
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.close_job();
    }

    fn close_job(&mut self) {
        if let Some(job) = self.job.take() {
            // SAFETY: attach_kill_on_close_job devolvió un handle de job válido.
            let _ = unsafe {
                windows::Win32::Foundation::CloseHandle(windows::Win32::Foundation::HANDLE(
                    job as *mut core::ffi::c_void,
                ))
            };
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.close_job();
    }
}

fn attach_kill_on_close_job(child: &Child) -> Option<isize> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows::core::PCWSTR;

    // SAFETY: se crea un job anónimo y se configuran sus límites antes de asignar el worker.
    let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }.ok()?;
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: limits permanece válido durante la llamada y su tamaño coincide con el tipo.
    let configured = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    }
    .is_ok();
    let process = HANDLE(child.as_raw_handle() as *mut core::ffi::c_void);
    // SAFETY: Child mantiene abierto el handle del proceso mientras se realiza la asignación.
    let assigned = configured && unsafe { AssignProcessToJobObject(job, process) }.is_ok();
    if assigned {
        Some(job.0 as isize)
    } else {
        // SAFETY: el handle recién creado se cierra si no pudo asignarse el worker.
        let _ = unsafe { CloseHandle(job) };
        None
    }
}

struct Supervisor {
    worker: Option<Worker>,
    next_id: u64,
}

impl Supervisor {
    fn request(&mut self, mut payload: Map<String, Value>) -> Result<Value, String> {
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| "ocr_request_id_exhausted".to_owned())?;
        let id = self.next_id;
        let mut request = Map::new();
        request.insert("id".into(), json!(id));
        request.append(&mut payload);
        let request = Value::Object(request);
        let deadline = Instant::now() + OCR_TIMEOUT;

        loop {
            if self.worker.is_none() {
                self.worker = Some(Worker::spawn()?);
            }
            let worker = self
                .worker
                .as_mut()
                .ok_or_else(|| "ocr_worker_unavailable".to_owned())?;
            match worker.child.try_wait() {
                Ok(Some(_)) => {
                    self.worker = None;
                    continue;
                }
                Err(error) => {
                    if let Some(mut worker) = self.worker.take() {
                        worker.stop();
                    }
                    return Err(error.to_string());
                }
                Ok(None) => {}
            }
            let encoded = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
            if let Err(error) = worker
                .stdin
                .write_all(&encoded)
                .and_then(|()| worker.stdin.write_all(b"\n"))
                .and_then(|()| worker.stdin.flush())
            {
                if let Some(mut worker) = self.worker.take() {
                    worker.stop();
                }
                return Err(error.to_string());
            }

            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err("ocr_timeout".into());
                }
                match worker.lines.recv_timeout(remaining) {
                    Ok(message) if message.get("id").and_then(Value::as_u64) == Some(id) => {
                        return Ok(message);
                    }
                    Ok(_) => {}
                    Err(mpsc::RecvTimeoutError::Timeout) => return Err("ocr_timeout".into()),
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        if let Some(mut worker) = self.worker.take() {
                            worker.stop();
                        }
                        return Err("ocr_worker_exit".into());
                    }
                }
            }
        }
    }
}

fn supervisor() -> &'static Mutex<Supervisor> {
    static SUPERVISOR: OnceLock<Mutex<Supervisor>> = OnceLock::new();
    SUPERVISOR.get_or_init(|| {
        Mutex::new(Supervisor {
            worker: None,
            next_id: 0,
        })
    })
}

fn paddle_request(
    image: &str,
    language: &str,
    device: &str,
    min_confidence: f64,
    max_results: &Value,
    query: &str,
    group_lines: bool,
) -> Result<Value, String> {
    let mut payload = Map::new();
    payload.insert("image".into(), json!(image));
    payload.insert("language".into(), json!(language));
    payload.insert("device".into(), json!(device));
    payload.insert("min_confidence".into(), json!(min_confidence));
    payload.insert("max_results".into(), max_results.clone());
    payload.insert("query".into(), json!(query));
    payload.insert("group_lines".into(), json!(group_lines));
    let mut guard = supervisor()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.request(payload)
}

fn windows_media_ocr(
    bytes: &[u8],
    language: &str,
    query: &str,
    group_lines: bool,
    max_results: i64,
) -> Result<Value, String> {
    let bytes = bytes.to_vec();
    let language = language.to_owned();
    let query = query.to_owned();
    thread::Builder::new()
        .name("cu-ocr-winrt".into())
        .spawn(move || {
            windows_media_ocr_on_thread(&bytes, &language, &query, group_lines, max_results)
        })
        .map_err(|error| error.to_string())?
        .join()
        .map_err(|_| "windows_ocr_thread_failed".to_owned())?
}

fn windows_media_ocr_on_thread(
    bytes: &[u8],
    language: &str,
    query: &str,
    group_lines: bool,
    max_results: i64,
) -> Result<Value, String> {
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::BitmapDecoder;
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};

    let _apartment = WinRtApartment::initialize()?;
    let stream = InMemoryRandomAccessStream::new().map_err(win_error)?;
    let output = stream.GetOutputStreamAt(0).map_err(win_error)?;
    let writer = DataWriter::CreateDataWriter(&output).map_err(win_error)?;
    writer.WriteBytes(bytes).map_err(win_error)?;
    wait_winrt(writer.StoreAsync().map_err(win_error)?)?;
    let _ = writer.DetachStream().map_err(win_error)?;
    stream.Seek(0).map_err(win_error)?;
    let decoder = wait_winrt(BitmapDecoder::CreateAsync(&stream).map_err(win_error)?)?;
    let bitmap = wait_winrt(decoder.GetSoftwareBitmapAsync().map_err(win_error)?)?;

    let mut engine = None;
    if !language.is_empty() {
        let tag = HSTRING::from(language);
        if let Ok(lang) = Language::CreateLanguage(&tag) {
            engine = OcrEngine::TryCreateFromLanguage(&lang).ok();
        }
    }
    if engine.is_none() {
        engine = OcrEngine::TryCreateFromUserProfileLanguages().ok();
    }
    if engine.is_none() {
        if let Ok(languages) = OcrEngine::AvailableRecognizerLanguages() {
            if languages.Size().unwrap_or(0) > 0 {
                if let Ok(language) = languages.GetAt(0) {
                    engine = OcrEngine::TryCreateFromLanguage(&language).ok();
                }
            }
        }
    }
    let Some(engine) = engine else {
        return Err("no_windows_ocr_engine_available".into());
    };
    let result = wait_winrt(engine.RecognizeAsync(&bitmap).map_err(win_error)?)?;
    let lines = result.Lines().map_err(win_error)?;
    let needle = query.trim().to_lowercase();
    let limit = if max_results > 0 {
        max_results as usize
    } else {
        200
    };
    let mut items = Vec::new();

    for line_index in 0..lines.Size().map_err(win_error)? {
        let line = lines.GetAt(line_index).map_err(win_error)?;
        let text = clean_control_text(&line.Text().map_err(win_error)?.to_string());
        if text.trim().is_empty() || !matches_query(&text, &needle) {
            continue;
        }
        let words = line.Words().map_err(win_error)?;
        if group_lines {
            let mut min_x = 999_999.0f32;
            let mut min_y = 999_999.0f32;
            let mut max_r = 0.0f32;
            let mut max_b = 0.0f32;
            for word_index in 0..words.Size().map_err(win_error)? {
                let rect = words
                    .GetAt(word_index)
                    .map_err(win_error)?
                    .BoundingRect()
                    .map_err(win_error)?;
                min_x = min_x.min(rect.X);
                min_y = min_y.min(rect.Y);
                max_r = max_r.max(rect.X + rect.Width);
                max_b = max_b.max(rect.Y + rect.Height);
            }
            if min_x < 999_999.0 {
                items.push(native_item(
                    &text,
                    min_x,
                    min_y,
                    max_r - min_x,
                    max_b - min_y,
                ));
            }
        } else {
            for word_index in 0..words.Size().map_err(win_error)? {
                let word = words.GetAt(word_index).map_err(win_error)?;
                let word_text = clean_control_text(&word.Text().map_err(win_error)?.to_string());
                if word_text.trim().is_empty() || !matches_query(&word_text, &needle) {
                    continue;
                }
                let rect = word.BoundingRect().map_err(win_error)?;
                items.push(native_item(
                    &word_text,
                    rect.X,
                    rect.Y,
                    rect.Width,
                    rect.Height,
                ));
                if items.len() >= limit {
                    break;
                }
            }
        }
        if items.len() >= limit {
            break;
        }
    }
    Ok(json!({
        "ok": true,
        "backend": "windows_media_ocr",
        "items": items
    }))
}

fn native_item(text: &str, x: f32, y: f32, w: f32, h: f32) -> Value {
    // El cast [int] del PowerShell usa redondeo al entero más cercano, con empate par.
    // ORDEN de claves: el origen arma el item como hashtable de PowerShell 5.1
    // (`@{ text=..; confidence=0.95; x=..; y=..; w=..; h=.. }`) y ConvertTo-Json lo
    // serializa en el orden de enumeración del hashtable: confidence, text, y, h, x, w.
    json!({
        "confidence": 0.95,
        "text": text,
        "y": y.round_ties_even() as i32,
        "h": h.round_ties_even() as i32,
        "x": x.round_ties_even() as i32,
        "w": w.round_ties_even() as i32
    })
}

fn clean_control_text(text: &str) -> String {
    text.chars()
        .filter(|ch| !(*ch <= '\u{001f}' || *ch == '\u{007f}'))
        .collect()
}

fn matches_query(text: &str, needle: &str) -> bool {
    needle.is_empty() || text.to_lowercase().contains(needle)
}

fn win_error(error: windows::core::Error) -> String {
    error.to_string()
}

struct WinRtApartment;

impl WinRtApartment {
    fn initialize() -> Result<Self, String> {
        // SAFETY: esta función corre en un hilo dedicado que no inicializa COM por otra vía.
        unsafe {
            windows::Win32::System::WinRT::RoInitialize(
                windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
            )
        }
        .map_err(win_error)?;
        Ok(Self)
    }
}

impl Drop for WinRtApartment {
    fn drop(&mut self) {
        // SAFETY: initialize() tuvo éxito en este mismo hilo.
        unsafe { windows::Win32::System::WinRT::RoUninitialize() };
    }
}

struct ThreadWake(thread::Thread);

impl std::task::Wake for ThreadWake {
    fn wake(self: std::sync::Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &std::sync::Arc<Self>) {
        self.0.unpark();
    }
}

fn wait_winrt<T, O>(operation: O) -> Result<T, String>
where
    T: windows::core::RuntimeType,
    O: IntoFuture<Output = windows::core::Result<T>>,
    O::IntoFuture: Future<Output = windows::core::Result<T>>,
{
    let future = operation.into_future();
    block_on_timeout(future, Duration::from_secs(12))
        .ok_or_else(|| "windows_ocr_timeout".to_owned())?
        .map_err(win_error)
}

fn block_on_timeout<F: Future>(future: F, timeout: Duration) -> Option<F::Output> {
    let waker = std::task::Waker::from(std::sync::Arc::new(ThreadWake(thread::current())));
    let mut context = std::task::Context::from_waker(&waker);
    let mut future = Box::pin(future);
    let deadline = Instant::now() + timeout;
    loop {
        if let std::task::Poll::Ready(result) = future.as_mut().poll(&mut context) {
            return Some(result);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        thread::park_timeout(remaining);
    }
}

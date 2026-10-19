// cu-misc::audio_action — port de computer_audio_action (audio_action.js), Core Audio COM.
use serde_json::{json, Value};
use windows::core::Interface;
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use windows::Win32::Media::Audio::{
    IAudioSessionControl2, IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, AudioSessionStateActive,
    EDataFlow, ERole, MMDeviceEnumerator, eMultimedia, eRender,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED};

pub fn audio_action(args: &Value) -> Value {
    // normalize action like handle()
    let mut action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("status")
        .to_lowercase();
    if action == "set_mute" {
        action = if args.get("mute").and_then(|v| v.as_bool()).unwrap_or(false) {
            "mute".to_string()
        } else {
            "unmute".to_string()
        };
    }
    let new_volume = args.get("volume").and_then(|v| v.as_f64()).unwrap_or(0.0) as i32;

    // Run COM on a dedicated thread (MTA)
    let handle = std::thread::spawn(move || execute_audio(&action, new_volume));
    match handle.join() {
        Ok(v) => v,
        Err(_) => json!({"ok": false, "error": "thread panicked"}),
    }
}

fn execute_audio(action: &str, new_volume: i32) -> Value {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        // hr may be S_OK or S_FALSE (already init); proceed
        let _ = hr;
        let result = run_audio(action, new_volume);
        CoUninitialize();
        result
    }
}

unsafe fn run_audio(action: &str, new_volume: i32) -> Value {
    unsafe {
        // create enumerator
        let enumerator: IMMDeviceEnumerator =
            match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
                Ok(e) => e,
                Err(e) => return json!({"ok": false, "error": format!("cocreate_failed: {e}")}),
            };
        let dev: IMMDevice = match enumerator.GetDefaultAudioEndpoint(EDataFlow(eRender.0), ERole(eMultimedia.0)) {
            Ok(d) => d,
            Err(_) => return json!({"ok": false, "error": "no_default_audio_device"}),
        };

        // activate endpoint volume
        let ep_vol: IAudioEndpointVolume = match dev.Activate(CLSCTX_ALL, None) {
            Ok(v) => v,
            Err(e) => return json!({"ok": false, "error": format!("activate_volume_failed: {e}")}),
        };

        let empty = windows::core::GUID::zeroed();
        if action == "set_volume" {
            let scalar = (new_volume.clamp(0, 100) as f32) / 100.0;
            let _ = ep_vol.SetMasterVolumeLevelScalar(scalar, &empty);
        } else if action == "mute" {
            let _ = ep_vol.SetMute(true, &empty);
        } else if action == "unmute" {
            let _ = ep_vol.SetMute(false, &empty);
        } else if action == "toggle_mute" {
            if let Ok(cur) = ep_vol.GetMute() {
                let _ = ep_vol.SetMute(!cur.as_bool(), &empty);
            }
        }

        // read state
        let master_vol = ep_vol.GetMasterVolumeLevelScalar().unwrap_or(0.0);
        let is_muted = ep_vol.GetMute().map(|b| b.as_bool()).unwrap_or(false);

        // meter
        let mut peak: f32 = 0.0;
        if let Ok(meter) = dev.Activate::<IAudioMeterInformation>(CLSCTX_ALL, None) {
            if let Ok(p) = meter.GetPeakValue() {
                peak = p;
            }
        }

        // sessions
        let mut sessions: Vec<Value> = Vec::new();
        if let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) {
            if let Ok(senum) = mgr.GetSessionEnumerator() {
                if let Ok(count) = senum.GetCount() {
                    for i in 0..count {
                        if let Ok(sctrl) = senum.GetSession(i) {
                            let state = sctrl.GetState().ok();
                            let active = state.map(|s| s == AudioSessionStateActive).unwrap_or(false);
                            if !active {
                                continue;
                            }
                            // try cast to IAudioSessionControl2
                            let mut pid: u32 = 0;
                            // IAudioSessionControl2 is a different interface; need QueryInterface
                            // Use cast via .cast() if available
                            if let Ok(ctrl2) = sctrl.cast::<IAudioSessionControl2>() {
                                if let Ok(p) = ctrl2.GetProcessId() {
                                    pid = p;
                                }
                            }
                            let proc_name = if pid > 0 {
                                process_name_by_pid(pid).unwrap_or_default()
                            } else {
                                String::new()
                            };
                            sessions.push(json!({
                                "pid": pid,
                                "process": proc_name,
                                "active": true
                            }));
                        }
                    }
                }
            }
        }

        let volume = (master_vol * 100.0).round() as i32;
        let volume_scalar = (master_vol as f64 * 10000.0).round() / 10000.0;
        let peak_r = (peak as f64 * 10000.0).round() / 10000.0;
        let is_playing = peak > 0.001;

        json!({
            "ok": true,
            "action": action,
            "volume": volume,
            "volume_scalar": volume_scalar,
            "muted": is_muted,
            "peak": peak_r,
            "is_playing_audio": is_playing,
            "active_sessions": sessions
        })
    }
}

fn process_name_by_pid(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 32768];
        let mut size = buf.len() as u32;
        let pw = windows::core::PWSTR(buf.as_mut_ptr());
        let name = if QueryFullProcessImageNameW(h, windows::Win32::System::Threading::PROCESS_NAME_WIN32, pw, &mut size).is_ok() {
            let full = String::from_utf16_lossy(&buf[..size as usize]);
            std::path::Path::new(&full)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let _ = CloseHandle(h);
        if name.is_empty() { None } else { Some(name) }
    }
}
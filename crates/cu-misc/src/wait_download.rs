// cu-misc::wait_download — port de nativeWaitDownload (index.js).
use serde_json::{json, Value};
use std::fs;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub fn wait_download(args: &Value) -> Value {
    let timeout_seconds = args.get("timeout_seconds").and_then(|v| v.as_f64()).unwrap_or(30.0);
    let timeout_seconds = if timeout_seconds > 0.0 { timeout_seconds as i64 } else { 30 };
    let download_dir = args.get("download_dir").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let expected_ext = args.get("expected_ext").and_then(|v| v.as_str()).unwrap_or("").to_string();

    let dir = if download_dir.is_empty() {
        let up = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\Default".to_string());
        format!("{}\\Downloads", up)
    } else {
        download_dir
    };

    if !std::path::Path::new(&dir).exists() {
        return json!({"ok": false, "error": "download_dir_not_found", "dir": dir});
    }

    let start_time = SystemTime::now() - Duration::from_secs(5);
    let sw_start = Instant::now();
    let timeout = Duration::from_secs(timeout_seconds as u64);

    while sw_start.elapsed() < timeout {
        // active downloads
        let mut active = 0usize;
        let mut completed: Vec<fs::DirEntry> = Vec::new();
        if let Ok(entries) = fs::read_dir(&dir) {
            for e in entries.flatten() {
                if !e.path().is_file() { continue; }
                let meta = match e.metadata() { Ok(m) => m, Err(_) => continue };
                let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
                if mtime < start_time { continue; }
                let name = e.file_name().to_string_lossy().to_string();
                let ext = e.path().extension().map(|x| format!(".{}", x.to_string_lossy())).unwrap_or_default();
                let is_active = ext == ".crdownload" || ext == ".tmp" || ext == ".part" || ext == ".download" || name.contains(".tmp");
                if is_active {
                    active += 1;
                } else {
                    completed.push(e);
                }
            }
        }
        // sort completed by mtime desc
        completed.sort_by(|a, b| {
            let ma = a.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH);
            let mb = b.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH);
            mb.cmp(&ma)
        });

        if !completed.is_empty() && active == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    // Re-evaluate found file cleanly
    let found_path = select_found_file(&dir, start_time, &expected_ext, timeout);

    let elapsed = sw_start.elapsed().as_secs_f64();
    if let Some(path) = found_path {
        std::thread::sleep(Duration::from_millis(150));
        let meta = fs::metadata(&path).ok();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let name = std::path::Path::new(&path).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let ext = std::path::Path::new(&path).extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default();
        let created_at = meta.and_then(|m| m.created().ok()).map(|t| {
            let dur = t.duration_since(UNIX_EPOCH).unwrap_or_default();
            // RFC3339-ish UTC
            format!("{}", dur.as_secs())
        }).unwrap_or_default();
        let sha256 = compute_sha256(&path).unwrap_or_default();
        json!({
            "ok": true,
            "filename": name,
            "path": path,
            "size_bytes": size,
            "extension": ext,
            "created_at": created_at,
            "sha256": sha256,
            "elapsed_seconds": (elapsed * 100.0).round() / 100.0
        })
    } else {
        json!({
            "ok": false,
            "timeout": true,
            "elapsed_seconds": (elapsed * 100.0).round() / 100.0,
            "message": format!("No completed download found in {} within {} seconds", dir, timeout_seconds)
        })
    }
}

fn select_found_file(dir: &str, start_time: SystemTime, expected_ext: &str, _timeout: Duration) -> Option<String> {
    // Re-run a single scan for the most recent completed file matching criteria.
    let mut completed: Vec<(String, SystemTime)> = Vec::new();
    let mut active = 0usize;
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            if !e.path().is_file() { continue; }
            let meta = match e.metadata() { Ok(m) => m, Err(_) => continue };
            let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
            if mtime < start_time { continue; }
            let name = e.file_name().to_string_lossy().to_string();
            let ext = e.path().extension().map(|x| format!(".{}", x.to_string_lossy())).unwrap_or_default();
            let is_active = ext == ".crdownload" || ext == ".tmp" || ext == ".part" || ext == ".download" || name.contains(".tmp");
            if is_active { active += 1; continue; }
            completed.push((e.path().to_string_lossy().to_string(), mtime));
        }
    }
    if completed.is_empty() || active > 0 { return None; }
    completed.sort_by(|a, b| b.1.cmp(&a.1));
    if expected_ext.is_empty() {
        return Some(completed[0].0.clone());
    }
    for (p, _) in &completed {
        let ext = std::path::Path::new(p).extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default();
        if ext == expected_ext || ext == format!(".{}", expected_ext) {
            return Some(p.clone());
        }
    }
    None
}

// SHA-256 manual (no dep)
fn compute_sha256(path: &str) -> Option<String> {
    let data = fs::read(path).ok()?;
    Some(sha256_hex(&data))
}

fn sha256_hex(data: &[u8]) -> String {
    // standard SHA-256 implementation
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let k: [u32; 64] = [
        0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
        0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
        0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
        0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
        0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
        0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
        0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
        0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2,
    ];

    // padding
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    msg.push(0x80);
    while (msg.len() % 64) != 56 { msg.push(0); }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i*4], chunk[i*4+1], chunk[i*4+2], chunk[i*4+3]]);
        }
        for i in 16..64 {
            let s0 = w[i-15].rotate_right(7) ^ w[i-15].rotate_right(18) ^ (w[i-15] >> 3);
            let s1 = w[i-2].rotate_right(17) ^ w[i-2].rotate_right(19) ^ (w[i-2] >> 10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }
        let [mut a,mut b,mut c,mut d,mut e,mut f,mut g,mut hh] = state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(k[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g; g = f; f = e; e = d.wrapping_add(temp1); d = c; c = b; b = a; a = temp1.wrapping_add(temp2);
        }
        state[0] = state[0].wrapping_add(a); state[1] = state[1].wrapping_add(b); state[2] = state[2].wrapping_add(c); state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e); state[5] = state[5].wrapping_add(f); state[6] = state[6].wrapping_add(g); state[7] = state[7].wrapping_add(hh);
    }
    state.iter().map(|x| format!("{:08x}", x)).collect()
}
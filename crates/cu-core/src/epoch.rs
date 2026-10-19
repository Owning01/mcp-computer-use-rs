// cu-core::epoch — contador global de epoch (espejo de currentEpoch del origen).
// Lo comparten main.rs (screenshot/zoom/clipboard) y los clusters nativos (B).
use std::sync::atomic::{AtomicU64, Ordering};

static EPOCH: AtomicU64 = AtomicU64::new(0);

pub fn bump() -> u64 {
    EPOCH.fetch_add(1, Ordering::SeqCst) + 1
}

pub fn current() -> u64 {
    EPOCH.load(Ordering::SeqCst)
}

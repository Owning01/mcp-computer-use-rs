// cu-core::epoch — contador global de epoch (espejo de `currentEpoch` del origen).
//
// El origen arranca en 1 (`let currentEpoch = 1`) e incrementa al mutar la UI.
// El puente mantiene el MISMO contador: los handlers nativos lo incrementan y las
// respuestas proxeadas del hijo lo adoptan (así hay un único contador lógico).
use std::sync::atomic::{AtomicU64, Ordering};

static EPOCH: AtomicU64 = AtomicU64::new(1);

pub fn bump() -> u64 {
    EPOCH.fetch_add(1, Ordering::SeqCst) + 1
}

pub fn current() -> u64 {
    EPOCH.load(Ordering::SeqCst)
}

/// Adopta el epoch reportado por el hijo (respuestas proxeadas). Monótono: un epoch
/// menor (el hijo no ve los bumps locales del puente) no hace retroceder el contador.
pub fn adopt(value: u64) {
    if value > current() {
        set(value);
    }
}

/// Adopta el epoch reportado por el hijo (respuestas proxeadas).
pub fn set(value: u64) {
    EPOCH.store(value, Ordering::SeqCst);
}

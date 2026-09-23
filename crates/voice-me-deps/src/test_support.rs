//! Test-only helpers shared by the detection and provisioning tests.
//!
//! The environment is process-global, so every test that touches it holds
//! the one lock below and puts back what it found — a stray variable left
//! behind would silently change what every later test in this process
//! sees.

use std::sync::{Mutex, MutexGuard, OnceLock};

/// Holds the lock and restores every variable it touched.
///
/// One type rather than one per variable because `std::sync::Mutex` is
/// not reentrant: a test that wanted both `ORT_DYLIB_PATH` and
/// `VOICE_ME_MODEL_CACHE` would deadlock against itself.
pub struct EnvGuard {
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _guard: MutexGuard<'static, ()>,
}

impl EnvGuard {
    pub fn new() -> Self {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let guard = LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self {
            previous: Vec::new(),
            _guard: guard,
        }
    }

    fn remember(&mut self, key: &'static str) {
        if !self.previous.iter().any(|(seen, _)| *seen == key) {
            self.previous.push((key, std::env::var_os(key)));
        }
    }

    /// [`Self::set`] on a guard already held — for a test that has to
    /// change one more variable after its fixture took the lock.
    pub fn set_mut(&mut self, key: &'static str, value: impl AsRef<std::ffi::OsStr>) {
        self.remember(key);
        unsafe { std::env::set_var(key, value) };
    }

    pub fn set(mut self, key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        self.remember(key);
        // Every test that touches the environment holds this same lock,
        // and `Drop` puts back what was there.
        unsafe { std::env::set_var(key, value) };
        self
    }

    pub fn unset(mut self, key: &'static str) -> Self {
        self.remember(key);
        unsafe { std::env::remove_var(key) };
        self
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}

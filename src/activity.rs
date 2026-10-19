//! Cross-thread user-activity tracker for idle gating.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Default)]
pub struct ActivityTracker {
    pub last_physical_ms: AtomicU64,
    pub injecting: AtomicBool,
    pub abort_inject: AtomicBool,
}

impl ActivityTracker {
    pub fn new() -> Arc<Self> {
        let t = Arc::new(Self::default());
        t.touch();
        t
    }

    pub fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    pub fn touch(&self) {
        self.last_physical_ms.store(Self::now_ms(), Ordering::SeqCst);
    }

    pub fn on_physical_input(&self) {
        self.touch();
        if self.injecting.load(Ordering::SeqCst) {
            self.abort_inject.store(true, Ordering::SeqCst);
        }
    }

    /// Absolute ms timestamp of last input (hooks + optional GetLastInputInfo).
    pub fn last_ms(&self) -> u64 {
        let hook = self.last_physical_ms.load(Ordering::SeqCst);
        #[cfg(windows)]
        {
            if let Some(sys) = windows_last_input_unix_ms() {
                return hook.max(sys);
            }
        }
        hook
    }

    pub fn begin_inject(&self) {
        self.abort_inject.store(false, Ordering::SeqCst);
        self.injecting.store(true, Ordering::SeqCst);
    }

    pub fn end_inject(&self) {
        self.injecting.store(false, Ordering::SeqCst);
    }

    pub fn should_abort(&self) -> bool {
        self.abort_inject.load(Ordering::SeqCst)
    }
}

#[cfg(windows)]
fn windows_last_input_unix_ms() -> Option<u64> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    use windows::Win32::System::SystemInformation::GetTickCount;
    unsafe {
        let mut info = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        if GetLastInputInfo(&mut info) == windows_core::BOOL(0) {
            return None;
        }
        let tick = GetTickCount();
        let ago = tick.wrapping_sub(info.dwTime) as u64;
        Some(ActivityTracker::now_ms().saturating_sub(ago))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_and_abort() {
        let a = ActivityTracker::new();
        let t0 = a.last_ms();
        a.begin_inject();
        assert!(!a.should_abort());
        a.on_physical_input();
        assert!(a.should_abort());
        assert!(a.last_ms() >= t0);
        a.end_inject();
    }
}

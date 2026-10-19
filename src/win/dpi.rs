use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
};
use std::ptr;

/// Per-Monitor-Aware V2 (-4). Falls back to -3 then SetProcessDPIAware.
pub fn set_per_monitor_v2() -> Result<(), String> {
    let v2 = DPI_AWARENESS_CONTEXT(-4isize as _);
    let v1 = DPI_AWARENESS_CONTEXT(-3isize as _);
    unsafe {
        let prev = SetThreadDpiAwarenessContext(v2);
        if !prev.0.is_null() {
            let _ = SetProcessDpiAwarenessContext(v2);
            return Ok(());
        }
        let prev = SetThreadDpiAwarenessContext(v1);
        if !prev.0.is_null() {
            let _ = SetProcessDpiAwarenessContext(v1);
            return Ok(());
        }
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwareness(
            windows::Win32::UI::HiDpi::PROCESS_PER_MONITOR_DPI_AWARE,
        );
    }
    let _ = ptr::null_mut::<()>();
    Ok(())
}

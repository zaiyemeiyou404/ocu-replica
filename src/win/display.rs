//! Enumerate monitors (EnumDisplayMonitors + GetMonitorInfoW + GetDpiForMonitor).

use crate::geometry::Rect;
use crate::monitor::DisplayInfo;
use std::sync::Mutex;
use windows::Win32::Foundation::{LPARAM, RECT};
use windows_core::BOOL;
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

static COLLECT: Mutex<Vec<DisplayInfo>> = Mutex::new(Vec::new());

unsafe extern "system" fn enum_proc(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _lprc: *mut RECT,
    _data: LPARAM,
) -> BOOL {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // GetMonitorInfoW accepts MONITORINFO*; MONITORINFOEXW lays out with cbSize of the EX form.
    if !GetMonitorInfoW(hmonitor, &mut info as *mut _ as *mut MONITORINFO).as_bool() {
        return BOOL(1);
    }
    let rc = info.monitorInfo.rcMonitor;
    let wa = info.monitorInfo.rcWork;
    let primary = (info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0;
    let device = {
        let end = info
            .szDevice
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.szDevice.len());
        String::from_utf16_lossy(&info.szDevice[..end])
    };
    let mut dpix = 96u32;
    let mut dpiy = 96u32;
    let _ = GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpix, &mut dpiy);
    let dpi = dpix.max(1);
    let scale = dpi as f64 / 96.0;
    if let Ok(mut g) = COLLECT.lock() {
        let index = g.len() as u32;
        g.push(DisplayInfo {
            index,
            device_name: device,
            friendly_name: None, // cheap path skips EDID; device name is enough
            rect: Rect::new(rc.left, rc.top, rc.right, rc.bottom),
            work_area: Rect::new(wa.left, wa.top, wa.right, wa.bottom),
            dpi,
            scale,
            primary,
        });
    }
    BOOL(1)
}

pub fn list_displays() -> Result<Vec<DisplayInfo>, String> {
    {
        let mut g = COLLECT.lock().map_err(|_| "display collect lock poisoned")?;
        g.clear();
    }
    unsafe {
        if !EnumDisplayMonitors(None, None, Some(enum_proc), LPARAM(0)).as_bool() {
            return Err("EnumDisplayMonitors failed".into());
        }
    }
    let g = COLLECT.lock().map_err(|_| "display collect lock poisoned")?;
    if g.is_empty() {
        return Err("no monitors found".into());
    }
    Ok(g.clone())
}

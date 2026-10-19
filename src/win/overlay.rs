//! Cursor / border overlay windows with WDA_EXCLUDEFROMCAPTURE.
//! Verified: SetWindowDisplayAffinity(0x11), class names conceptually similar
//! to the original (we use our own class names; no trademarked UI strings).

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, RegisterClassW, SetLayeredWindowAttributes, SetWindowPos,
    ShowWindow, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, LWA_ALPHA, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SW_HIDE, SW_SHOWNOACTIVATE, WM_DESTROY, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::Win32::Graphics::Gdi::{GetStockObject, WHITE_BRUSH, HBRUSH};
use std::sync::{Arc, Mutex};

const WDA_EXCLUDEFROMCAPTURE: u32 = 0x11;

struct OverlayState {
    cursor: HWND,
    border: HWND,
    visible: bool,
}

pub struct OverlayHandle {
    inner: Arc<Mutex<OverlayState>>,
}

impl OverlayHandle {
    /// No-op overlay used when the interactive desktop rejects layered windows.
    pub fn disabled() -> Self {
        Self {
            inner: Arc::new(Mutex::new(OverlayState {
                cursor: HWND::default(),
                border: HWND::default(),
                visible: false,
            })),
        }
    }

    pub fn spawn() -> Result<Self, String> {
        let cursor = create_overlay_window("OcuReplicaCursorOverlay", 24, 24)?;
        let border = create_overlay_window("OcuReplicaDisplayOverlay", 1, 1)?;
        // WDA_EXCLUDEFROMCAPTURE needs Win10 2004+; soft-fail on Wine / older builds.
        let _ = set_exclude_from_capture(cursor);
        let _ = set_exclude_from_capture(border);
        Ok(Self {
            inner: Arc::new(Mutex::new(OverlayState { cursor, border, visible: false })),
        })
    }

    pub fn set_visible(&self, visible: bool) {
        if let Ok(mut g) = self.inner.lock() {
            g.visible = visible;
            if g.cursor.0.is_null() {
                return;
            }
            unsafe {
                let _ = ShowWindow(g.cursor, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
                let _ = ShowWindow(g.border, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
        }
    }

    pub fn move_to(&self, x: i32, y: i32) {
        if let Ok(g) = self.inner.lock() {
            if g.cursor.0.is_null() {
                return;
            }
            unsafe {
                let _ = SetWindowPos(
                    g.cursor,
                    Some(windows::Win32::UI::WindowsAndMessaging::HWND_TOPMOST),
                    x - 4,
                    y - 4,
                    24,
                    24,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
        }
    }

    pub fn raise_for(&self, _hwnd: HWND) {
        if let Ok(g) = self.inner.lock() {
            if g.visible {
                unsafe {
                    let _ = SetWindowPos(
                        g.border,
                        Some(windows::Win32::UI::WindowsAndMessaging::HWND_TOPMOST),
                        0, 0, 0, 0,
                        SWP_NOACTIVATE,
                    );
                }
            }
        }
    }
}

fn create_overlay_window(class: &str, w: i32, h: i32) -> Result<HWND, String> {
    use std::os::windows::ffi::OsStrExt;
    let class_w: Vec<u16> = std::ffi::OsStr::new(class).encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(overlay_proc),
            hInstance: windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                .map_err(|e| format!("GetModuleHandleW: {e}"))?
                .into(),
            hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
            lpszClassName: PCWSTR(class_w.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            PCWSTR(class_w.as_ptr()),
            PCWSTR(class_w.as_ptr()),
            WS_POPUP,
            CW_USEDEFAULT, CW_USEDEFAULT, w, h,
            None, None,
            windows::Win32::System::LibraryLoader::GetModuleHandleW(None).ok().map(|h| h.into()),
            None,
        )
        .map_err(|e| format!("CreateWindowExW failed: {e}"))?;
        let _ = SetLayeredWindowAttributes(hwnd, windows::Win32::Foundation::COLORREF(0), 160, LWA_ALPHA);
        Ok(hwnd)
    }
}

fn set_exclude_from_capture(hwnd: HWND) -> Result<(), String> {
    // SetWindowDisplayAffinity is in Win32_UI_WindowsAndMessaging on recent windows crate.
    type FnSet = unsafe extern "system" fn(HWND, u32) -> i32;
    unsafe {
        let mod_user = windows::Win32::System::LibraryLoader::GetModuleHandleW(
            windows::core::PCWSTR(widestring("user32.dll").as_ptr()),
        )
        .map_err(|e| e.to_string())?;
        let proc = windows::Win32::System::LibraryLoader::GetProcAddress(
            mod_user,
            windows::core::PCSTR(b"SetWindowDisplayAffinity\0".as_ptr()),
        );
        if let Some(f) = proc {
            let f: FnSet = std::mem::transmute(f);
            if f(hwnd, WDA_EXCLUDEFROMCAPTURE) == 0 {
                return Err("SetWindowDisplayAffinity failed".into());
            }
        }
    }
    Ok(())
}

fn widestring(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_DESTROY {
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, w, l)
}

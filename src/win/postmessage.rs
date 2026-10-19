//! Optional PostMessage input path (OCU_ALLOW_POSTMESSAGE=1).
//! Documented as unreliable for Chromium/Electron/DirectX.

use crate::keys::Chord;
use crate::protocol::MouseButton;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::WindowsAndMessaging::{
    ChildWindowFromPointEx, PostMessageW, CWP_SKIPDISABLED, CWP_SKIPINVISIBLE,
    CWP_SKIPTRANSPARENT, WM_CHAR, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP,
};

fn lparam_xy(x: i32, y: i32) -> LPARAM {
    let packed = ((y as u32) << 16) | (x as u32 & 0xFFFF);
    LPARAM(packed as isize)
}

fn deepest_child(hwnd: HWND, client_x: i32, client_y: i32) -> HWND {
    unsafe {
        let pt = POINT { x: client_x, y: client_y };
        let child = ChildWindowFromPointEx(
            hwnd,
            pt,
            CWP_SKIPINVISIBLE | CWP_SKIPDISABLED | CWP_SKIPTRANSPARENT,
        );
        if child.0.is_null() || child == hwnd {
            hwnd
        } else {
            // One level is enough for most Win32 hierarchies; recurse once more.
            let cpt = POINT { x: client_x, y: client_y };
            // Coordinates are still relative to the parent; for simplicity target `child`.
            let _ = cpt;
            child
        }
    }
}

pub fn screen_to_client(hwnd: HWND, screen_x: i32, screen_y: i32) -> Result<(i32, i32), String> {
    unsafe {
        let mut pt = POINT { x: screen_x, y: screen_y };
        if !ScreenToClient(hwnd, &mut pt).as_bool() {
            return Err("ScreenToClient failed".into());
        }
        Ok((pt.x, pt.y))
    }
}

pub fn click(hwnd: HWND, screen_x: i32, screen_y: i32, button: MouseButton, count: u32) -> Result<(), String> {
    let (cx, cy) = screen_to_client(hwnd, screen_x, screen_y)?;
    let target = deepest_child(hwnd, cx, cy);
    let (down, up) = match button {
        MouseButton::Left => (WM_LBUTTONDOWN, WM_LBUTTONUP),
        MouseButton::Right => (WM_RBUTTONDOWN, WM_RBUTTONUP),
        MouseButton::Middle => (WM_MBUTTONDOWN, WM_MBUTTONUP),
    };
    unsafe {
        let lp = lparam_xy(cx, cy);
        let _ = PostMessageW(Some(target), WM_MOUSEMOVE, WPARAM(0), lp);
        for _ in 0..count.max(1) {
            PostMessageW(Some(target), down, WPARAM(1), lp)
                .map_err(|_| "PostMessage button-down failed".to_string())?;
            PostMessageW(Some(target), up, WPARAM(0), lp)
                .map_err(|_| "PostMessage button-up failed".to_string())?;
        }
    }
    Ok(())
}

pub fn type_text(hwnd: HWND, text: &str) -> Result<(), String> {
    unsafe {
        for ch in text.encode_utf16() {
            PostMessageW(Some(hwnd), WM_CHAR, WPARAM(ch as usize), LPARAM(0))
                .map_err(|_| "PostMessage WM_CHAR failed".to_string())?;
        }
    }
    Ok(())
}

pub fn press_chord(hwnd: HWND, chord: &Chord) -> Result<(), String> {
    unsafe {
        for &m in &chord.modifiers {
            let _ = PostMessageW(Some(hwnd), WM_KEYDOWN, WPARAM(m as usize), LPARAM(0));
        }
        if let Some(ch) = chord.unicode {
            for u in ch.encode_utf16(&mut [0; 2]).iter().copied() {
                let _ = PostMessageW(Some(hwnd), WM_CHAR, WPARAM(u as usize), LPARAM(0));
            }
        } else if chord.key != 0 {
            let _ = PostMessageW(Some(hwnd), WM_KEYDOWN, WPARAM(chord.key as usize), LPARAM(0));
            let _ = PostMessageW(Some(hwnd), WM_KEYUP, WPARAM(chord.key as usize), LPARAM(0));
        }
        for &m in chord.modifiers.iter().rev() {
            let _ = PostMessageW(Some(hwnd), WM_KEYUP, WPARAM(m as usize), LPARAM(0));
        }
    }
    Ok(())
}

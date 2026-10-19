//! Bounded, target-specific messages for known standard native controls.
//! Unlike UIA provider actions, these routes do not request foreground focus.
//! This is deliberately narrower than generic PostMessage-to-top-level input.
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetWindowLongW, SendMessageTimeoutW, GWL_STYLE,
    SMTO_ABORTIFHUNG, SMTO_BLOCK,
};

fn class_name(hwnd: HWND) -> String {
    let mut text = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut text) };
    String::from_utf16_lossy(&text[..len.max(0) as usize]).to_ascii_lowercase()
}

pub fn is_edit(hwnd: HWND) -> bool {
    let class = class_name(hwnd);
    class == "edit" || class.starts_with("windowsforms10.edit.") || class == "richeditd2dpt"
}

pub fn is_button(hwnd: HWND) -> bool {
    let class = class_name(hwnd);
    class == "button" || class.starts_with("windowsforms10.button.")
}

fn send(hwnd: HWND, message: u32, wparam: usize, lparam: isize) -> Result<usize, String> {
    let mut result = 0usize;
    let delivered = unsafe {
        SendMessageTimeoutW(hwnd, message, WPARAM(wparam), LPARAM(lparam),
            SMTO_ABORTIFHUNG | SMTO_BLOCK, 500, Some(&mut result))
    };
    if delivered.0 == 0 {
        return Err("native control message failed or timed out; outcome unknown, refresh before retrying".into());
    }
    Ok(result)
}

pub fn set_text(hwnd: HWND, text: &str, replace_selection: bool) -> Result<(), String> {
    if !is_edit(hwnd) { return Err("unsupported native edit class".into()); }
    // ES_READONLY / ES_PASSWORD. Do not turn UIA's checks into a message bypass.
    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
    if style & (0x0800 | 0x0020) != 0 { return Err("native edit is read-only or a password field".into()); }
    if text.contains('\0') { return Err("native edit text cannot contain NUL".into()); }
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let pointer = PCWSTR(wide.as_ptr());
    let result = send(hwnd, if replace_selection { 0x00C2 } else { 0x000C },
        usize::from(replace_selection), pointer.0 as isize)?;
    if !replace_selection && result == 0 { return Err("native edit rejected WM_SETTEXT".into()); }
    Ok(())
}

pub fn click(hwnd: HWND) -> Result<(), String> {
    if !is_button(hwnd) { return Err("unsupported native button class".into()); }
    send(hwnd, 0x00F5, 0, 0)?;
    Ok(())
}

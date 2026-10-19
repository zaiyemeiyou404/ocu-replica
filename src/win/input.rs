use crate::geometry::{self, Rect};
use crate::keys::Chord;
use crate::protocol::MouseButton;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_WHEEL, MOUSEEVENTF_HWHEEL, MOUSEINPUT, KEYBDINPUT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId,
    IsIconic, SetCursorPos, ShowWindow, SwitchToThisWindow, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_RESTORE,
};
use windows::Win32::System::Threading::AttachThreadInput;

pub fn virtual_desktop() -> Rect {
    unsafe {
        Rect::new(
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN) + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

pub fn activate(hwnd: HWND) -> Result<(), String> {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let fg = GetForegroundWindow();
        let fg_tid = GetWindowThreadProcessId(fg, None);
        let tg_tid = GetWindowThreadProcessId(hwnd, None);
        if fg_tid != 0 && tg_tid != 0 && fg_tid != tg_tid {
            let _ = AttachThreadInput(fg_tid, tg_tid, true);
            let _ = windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd);
            let _ = BringWindowToTop(hwnd);
            SwitchToThisWindow(hwnd, true);
            let _ = AttachThreadInput(fg_tid, tg_tid, false);
        } else {
            let _ = windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd);
            let _ = BringWindowToTop(hwnd);
            SwitchToThisWindow(hwnd, true);
        }
    }
    Ok(())
}

fn send(inputs: &[INPUT]) -> Result<(), String> {
    if inputs.is_empty() {
        return Ok(());
    }
    let n = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if n as usize != inputs.len() {
        return Err(format!("SendInput delivered {n}/{} events", inputs.len()));
    }
    Ok(())
}

fn mouse_move_abs(x: i32, y: i32) -> Result<(), String> {
    let desk = virtual_desktop();
    let (ax, ay) = geometry::absolute_mouse(x, y, &desk);
    unsafe {
        let _ = SetCursorPos(x, y);
    }
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: ax as i32,
                dy: ay as i32,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    send(&[input])
}

fn button_flags(button: MouseButton, down: bool) -> MOUSE_EVENT_FLAGS {
    match (button, down) {
        (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
        (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
        (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
        (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
        (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
        (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
    }
}

pub fn click_at(x: i32, y: i32, button: MouseButton, count: u32) -> Result<(), String> {
    if button == MouseButton::Right && count >= 2 {
        return Err("right double click is not supported".into());
    }
    mouse_move_abs(x, y)?;
    let mut inputs = Vec::new();
    for _ in 0..count.max(1) {
        inputs.push(INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0, dy: 0, mouseData: 0,
                    dwFlags: button_flags(button, true),
                    time: 0, dwExtraInfo: 0,
                },
            },
        });
        inputs.push(INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0, dy: 0, mouseData: 0,
                    dwFlags: button_flags(button, false),
                    time: 0, dwExtraInfo: 0,
                },
            },
        });
    }
    send(&inputs)
}

pub fn drag(from: (i32, i32), to: (i32, i32)) -> Result<(), String> {
    mouse_move_abs(from.0, from.1)?;
    let down = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0, dy: 0, mouseData: 0,
                dwFlags: MOUSEEVENTF_LEFTDOWN, time: 0, dwExtraInfo: 0,
            },
        },
    };
    send(&[down])?;
    mouse_move_abs(to.0, to.1)?;
    let up = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0, dy: 0, mouseData: 0,
                dwFlags: MOUSEEVENTF_LEFTUP, time: 0, dwExtraInfo: 0,
            },
        },
    };
    send(&[up])
}

pub fn scroll_at(x: i32, y: i32, dx: i32, dy: i32) -> Result<(), String> {
    mouse_move_abs(x, y)?;
    let mut inputs = Vec::new();
    if dy != 0 {
        // WHEEL_DELTA = 120; positive dy = scroll up in our API? Original: negative scrollY = up.
        // We treat positive scrollY as down (matching the type docs), so invert for MOUSEEVENTF_WHEEL.
        let data = (-dy).saturating_mul(120) as u32;
        inputs.push(INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0, dy: 0, mouseData: data,
                    dwFlags: MOUSEEVENTF_WHEEL, time: 0, dwExtraInfo: 0,
                },
            },
        });
    }
    if dx != 0 {
        let data = dx.saturating_mul(120) as u32;
        inputs.push(INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0, dy: 0, mouseData: data,
                    dwFlags: MOUSEEVENTF_HWHEEL, time: 0, dwExtraInfo: 0,
                },
            },
        });
    }
    send(&inputs)
}

fn key_event(vk: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn unicode_event(ch: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0),
                wScan: ch,
                dwFlags: if up { KEYEVENTF_UNICODE | KEYEVENTF_KEYUP } else { KEYEVENTF_UNICODE },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

pub fn type_text(text: &str) -> Result<(), String> {
    let mut inputs = Vec::with_capacity(text.len() * 2);
    for ch in text.encode_utf16() {
        inputs.push(unicode_event(ch, false));
        inputs.push(unicode_event(ch, true));
    }
    send(&inputs)
}

pub fn press_chord(chord: &Chord) -> Result<(), String> {
    let mut inputs = Vec::new();
    for &m in &chord.modifiers {
        inputs.push(key_event(m, KEYBD_EVENT_FLAGS(0)));
    }
    if let Some(ch) = chord.unicode {
        for u in ch.encode_utf16(&mut [0; 2]).iter().copied() {
            inputs.push(unicode_event(u, false));
            inputs.push(unicode_event(u, true));
        }
    } else if chord.key != 0 {
        inputs.push(key_event(chord.key, KEYBD_EVENT_FLAGS(0)));
        inputs.push(key_event(chord.key, KEYEVENTF_KEYUP));
    }
    for &m in chord.modifiers.iter().rev() {
        inputs.push(key_event(m, KEYEVENTF_KEYUP));
    }
    send(&inputs)
}


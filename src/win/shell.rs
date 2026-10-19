use crate::engine::{AppInfo, WindowInfo};
use crate::geometry::Rect;
use std::collections::HashMap;
use windows::core::PWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, MAX_PATH};
use windows_core::BOOL;
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowLongW, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, GWL_EXSTYLE, GWL_STYLE, WS_EX_TOOLWINDOW, WS_CHILD,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::core::PCWSTR;

#[derive(Default)]
struct EnumState {
    windows: Vec<WindowInfo>,
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let state = &mut *(lparam.0 as *mut EnumState);
    if let Ok(info) = inspect(hwnd) {
        state.windows.push(info);
    }
    BOOL(1)
}

fn inspect(hwnd: HWND) -> Result<WindowInfo, String> {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return Err("invisible".into());
        }
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        if style & (WS_CHILD.0 as u32) != 0 {
            return Err("child".into());
        }
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex & (WS_EX_TOOLWINDOW.0 as u32) != 0 {
            return Err("toolwindow".into());
        }
        let mut title_buf = [0u16; 512];
        let tlen = GetWindowTextW(hwnd, &mut title_buf);
        let title = if tlen > 0 {
            Some(String::from_utf16_lossy(&title_buf[..tlen as usize]))
        } else {
            None
        };
        // Skip untitled host windows that aren't useful.
        let mut class_buf = [0u16; 256];
        let clen = GetClassNameW(hwnd, &mut class_buf);
        let class = String::from_utf16_lossy(&class_buf[..clen as usize]);
        for skip in ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd", "DV2ControlHost"] {
            if class == skip {
                return Err("system".into());
            }
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return Err("no pid".into());
        }
        let process_path = process_image(pid).ok();
        let app = process_path
            .as_deref()
            .map(app_id_from_path)
            .unwrap_or_else(|| format!("pid:{pid}"));
        let bounds = window_rect(hwnd)?;
        if bounds.is_empty() && !IsIconic(hwnd).as_bool() {
            return Err("empty bounds".into());
        }
        Ok(WindowInfo {
            id: hwnd.0 as usize as u64,
            app,
            title,
            process_path,
            pid,
            bounds,
        })
    }
}

fn process_image(pid: u32) -> Result<String, String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
            .map_err(|e| format!("OpenProcess: {e}"))?;
        let mut buf = [0u16; MAX_PATH as usize];
        let mut size = buf.len() as u32;
        QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size)
            .map_err(|e| format!("QueryFullProcessImageNameW: {e}"))?;
        let _ = windows::Win32::Foundation::CloseHandle(h);
        Ok(String::from_utf16_lossy(&buf[..size as usize]))
    }
}

fn app_id_from_path(path: &str) -> String {
    let name = path.replace('/', "\\");
    name.rsplit('\\').next().unwrap_or(path).to_string()
}

pub fn window_rect(hwnd: HWND) -> Result<Rect, String> {
    unsafe {
        let mut r = windows::Win32::Foundation::RECT::default();
        // Prefer DWM extended frame bounds (matches original).
        let hr = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut _ as *mut _,
            std::mem::size_of::<windows::Win32::Foundation::RECT>() as u32,
        );
        if hr.is_err() {
            windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut r)
                .map_err(|e| format!("GetWindowRect failed: {e}"))?;
        }
        Ok(Rect::new(r.left, r.top, r.right, r.bottom))
    }
}

pub fn list_windows() -> Result<Vec<WindowInfo>, String> {
    let mut state = EnumState::default();
    unsafe {
        EnumWindows(Some(enum_proc), LPARAM(&mut state as *mut _ as isize))
            .map_err(|e| format!("EnumWindows failed: {e}"))?;
    }
    Ok(state.windows)
}

pub fn get_window(id: u64, app: Option<&str>) -> Result<WindowInfo, String> {
    let hwnd = HWND(id as usize as *mut _);
    let info = inspect(hwnd).map_err(|_| {
        let current = list_windows().unwrap_or_default();
        let listing: Vec<String> = current.iter().map(|w| format!("{}:{:?}", w.id, w.title)).collect();
        format!("window id {id} was not found. Current windows: {}", listing.join(", "))
    })?;
    if let Some(a) = app {
        if !info.app.eq_ignore_ascii_case(a)
            && !info.process_path.as_deref().map(|p| p.eq_ignore_ascii_case(a)).unwrap_or(false)
        {
            return Err(format!(
                "window id {id} no longer belongs to {a}; current owner is {}",
                info.app
            ));
        }
    }
    Ok(info)
}

pub fn hwnd_from_id(id: u64) -> Result<HWND, String> {
    let hwnd = HWND(id as usize as *mut _);
    // Validate by re-inspecting.
    let _ = inspect(hwnd).map_err(|e| format!("window id {id} is not usable ({e})"))?;
    Ok(hwnd)
}

pub fn list_apps() -> Result<Vec<AppInfo>, String> {
    let wins = list_windows()?;
    let mut by_app: HashMap<String, AppInfo> = HashMap::new();
    for w in wins {
        let entry = by_app.entry(w.app.clone()).or_insert_with(|| AppInfo {
            id: w.app.clone(),
            display_name: w.title.clone(),
            windows: Vec::new(),
            last_used_date: None,
            use_count: None,
            is_running: Some(true),
            process_path: w.process_path.clone(),
        });
        entry.windows.push(w);
    }
    // Also surface Start-Menu / WindowsApps aliases when cheap. Full catalog
    // enumeration (shell:AppsFolder) is inferred optional; we expose running apps
    // plus a few well-known system apps via App Paths registry if needed later.
    Ok(by_app.into_values().collect())
}

pub fn launch_app(app: &str) -> Result<(), String> {
    // Absolute .exe path or shell:AppsFolder\Aumid.
    let target = if app.to_ascii_lowercase().ends_with(".exe") || app.contains('\\') || app.contains('/') {
        app.to_string()
    } else if app.contains('!') {
        format!("shell:AppsFolder\\{app}")
    } else {
        // Try as bare executable name via ShellExecuteW ("open").
        app.to_string()
    };
    let wide: Vec<u16> = {
        use std::os::windows::ffi::OsStrExt;
        std::ffi::OsStr::new(&target).encode_wide().chain(std::iter::once(0)).collect()
    };
    let op: Vec<u16> = {
        use std::os::windows::ffi::OsStrExt;
        std::ffi::OsStr::new("open").encode_wide().chain(std::iter::once(0)).collect()
    };
    let rc = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(op.as_ptr()),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        )
    };
    if (rc.0 as isize) <= 32 {
        return Err(format!("Windows failed to launch app (ShellExecuteW returned {})", rc.0 as isize));
    }
    Ok(())
}

/// Move a top-level window to a target rect (keep Z-order).
pub fn move_window_to(hwnd: HWND, dest: Rect) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};
    unsafe {
        SetWindowPos(
            hwnd,
            None,
            dest.left,
            dest.top,
            dest.width(),
            dest.height(),
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
        .map_err(|e| format!("SetWindowPos: {e}"))?;
    }
    Ok(())
}

/// After launch, wait up to `timeout_ms` for a new top-level window whose process
/// path/name matches `app`, then move it into `work_area` (size preserved).
pub fn wait_and_place_launched(app: &str, work_area: Rect, timeout_ms: u64) -> Result<u64, String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let needle = app.to_ascii_lowercase();
    let before: std::collections::HashSet<u64> = list_windows()
        .unwrap_or_default()
        .into_iter()
        .map(|w| w.id)
        .collect();
    loop {
        let wins = list_windows().unwrap_or_default();
        for w in &wins {
            if before.contains(&w.id) {
                continue;
            }
            let path = w.process_path.as_deref().unwrap_or("").to_ascii_lowercase();
            let name = w.app.to_ascii_lowercase();
            if path.contains(&needle) || name.contains(&needle) || needle.contains(&name) {
                let hwnd = hwnd_from_id(w.id)?;
                let cur = window_rect(hwnd)?;
                let placed = crate::monitor::place_window_in_work_area(&work_area, cur.width(), cur.height());
                move_window_to(hwnd, placed)?;
                return Ok(w.id);
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for launched window of '{app}' to appear for monitor placement"
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

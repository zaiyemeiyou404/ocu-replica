//! Windows backend (cfg(windows) only).
//!
//! Clean-room reimplementation of the behaviours documented in
//! ANALYSIS_windows_zh.md. No code was copied from the original binary.

mod dpi;
mod input;
mod capture;
mod uia;
mod overlay;
mod interrupt;
mod shell;
mod audio;
mod signature;
mod approval;
mod postmessage;
mod display;

use crate::activity::ActivityTracker;
use crate::engine::{AccessSnapshot, AppInfo, AudioResult, Backend, CaptureResult, WindowInfo};
use crate::geometry::Rect;
use crate::keys::Chord;
use crate::mode::{idle_ms_from_env, idle_timeout_ms_from_env, postmessage_enabled, wait_until_idle, InputMode};
use crate::policy::{PersistScope, RiskLevel, SignatureInfo};
use crate::protocol::{Direction, MouseButton, SecondaryAction};
use crate::cdp;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow, SetCursorPos, SetForegroundWindow};

pub struct WinBackend {
    pub interrupt: Arc<Mutex<bool>>,
    pub activity: Arc<ActivityTracker>,
    ax: uia::AxState,
    audio: audio::AudioState,
    overlay: overlay::OverlayHandle,
    _hooks: interrupt::HookGuard,
    saved_cursor: Option<(i32, i32)>,
    saved_fg: Option<HWND>,
    last_backend: String,
    last_capture_backend: String,
    input_mode_label: String,
    scope_rect: Option<Rect>,
}

impl WinBackend {
    pub fn start(interrupt_flag: Arc<Mutex<bool>>, activity: Arc<ActivityTracker>) -> Result<Self, String> {
        dpi::set_per_monitor_v2()?;
        let hooks = interrupt::install(interrupt_flag.clone(), activity.clone())?;
        let overlay = match overlay::OverlayHandle::spawn() {
            Ok(o) => o,
            Err(e) => {
                eprintln!("ocu-replica: overlay unavailable ({e}); continuing without it");
                overlay::OverlayHandle::disabled()
            }
        };
        Ok(Self {
            interrupt: interrupt_flag,
            activity,
            ax: uia::AxState::new()?,
            audio: audio::AudioState::default(),
            overlay,
            _hooks: hooks,
            saved_cursor: None,
            saved_fg: None,
            last_backend: String::new(),
            last_capture_backend: String::new(),
            input_mode_label: "auto".into(),
            scope_rect: None,
        })
    }

    fn hwnd(id: u64) -> Result<HWND, String> {
        shell::hwnd_from_id(id)
    }

    fn clamp_xy(&self, x: i32, y: i32) -> (i32, i32) {
        match self.scope_rect {
            Some(r) => crate::monitor::clamp_point_to_monitor(x, y, &r),
            None => (x, y),
        }
    }

    fn note(&mut self, backend: &str) {
        self.last_backend = backend.into();
    }

    fn save_ctx(&mut self) {
        unsafe {
            let mut pt = POINT::default();
            if GetCursorPos(&mut pt).is_ok() {
                self.saved_cursor = Some((pt.x, pt.y));
            }
            let fg = GetForegroundWindow();
            if !fg.0.is_null() {
                self.saved_fg = Some(fg);
            }
        }
    }

    fn restore_ctx(&mut self, aborted: bool) {
        unsafe {
            if aborted {
                if let Some((x, y)) = self.saved_cursor.take() {
                    let _ = SetCursorPos(x, y);
                }
                if let Some(fg) = self.saved_fg.take() {
                    let _ = SetForegroundWindow(fg);
                }
            } else {
                self.saved_cursor = None;
                self.saved_fg = None;
            }
        }
    }

    fn wait_idle_if_needed(&self, mode: InputMode) -> Result<(), String> {
        if mode != InputMode::Auto {
            return Ok(());
        }
        let act = self.activity.clone();
        wait_until_idle(idle_ms_from_env(), idle_timeout_ms_from_env(), || {
            (ActivityTracker::now_ms(), act.last_ms())
        })
    }

    fn inject_click(&mut self, sx: i32, sy: i32, button: MouseButton, count: u32) -> Result<(), String> {
        let (sx, sy) = self.clamp_xy(sx, sy);
        self.save_ctx();
        self.activity.begin_inject();
        self.overlay.move_to(sx, sy);
        let r = input::click_at(sx, sy, button, count);
        let abort = self.activity.should_abort();
        self.activity.end_inject();
        self.restore_ctx(abort || r.is_err());
        if abort {
            return Err("user_busy: physical input arrived during SendInput".into());
        }
        r
    }

    fn inject_drag(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<(), String> {
        let from = self.clamp_xy(from.0, from.1);
        let to = self.clamp_xy(to.0, to.1);
        self.save_ctx();
        self.activity.begin_inject();
        self.overlay.move_to(from.0, from.1);
        let r = input::drag(from, to);
        let abort = self.activity.should_abort();
        self.activity.end_inject();
        self.restore_ctx(abort || r.is_err());
        if abort {
            return Err("user_busy: physical input arrived during SendInput".into());
        }
        r
    }

    fn inject_scroll(&mut self, sx: i32, sy: i32, dx: i32, dy: i32) -> Result<(), String> {
        let (sx, sy) = self.clamp_xy(sx, sy);
        self.save_ctx();
        self.activity.begin_inject();
        self.overlay.move_to(sx, sy);
        let r = input::scroll_at(sx, sy, dx, dy);
        let abort = self.activity.should_abort();
        self.activity.end_inject();
        self.restore_ctx(abort || r.is_err());
        if abort {
            return Err("user_busy: physical input arrived during SendInput".into());
        }
        r
    }

    fn inject_type(&mut self, text: &str) -> Result<(), String> {
        self.save_ctx();
        self.activity.begin_inject();
        let r = input::type_text(text);
        let abort = self.activity.should_abort();
        self.activity.end_inject();
        self.restore_ctx(abort || r.is_err());
        if abort {
            return Err("user_busy: physical input arrived during SendInput".into());
        }
        r
    }

    fn inject_key(&mut self, chord: &Chord) -> Result<(), String> {
        self.save_ctx();
        self.activity.begin_inject();
        let r = input::press_chord(chord);
        let abort = self.activity.should_abort();
        self.activity.end_inject();
        self.restore_ctx(abort || r.is_err());
        if abort {
            return Err("user_busy: physical input arrived during SendInput".into());
        }
        r
    }
}

unsafe impl Send for WinBackend {}
unsafe impl Send for uia::AxState {}
unsafe impl Send for overlay::OverlayHandle {}
unsafe impl Send for interrupt::HookGuard {}

impl Backend for WinBackend {
    fn list_windows(&mut self) -> Result<Vec<WindowInfo>, String> {
        shell::list_windows()
    }
    fn list_apps(&mut self) -> Result<Vec<AppInfo>, String> {
        shell::list_apps()
    }
    fn launch_app(&mut self, app: &str) -> Result<(), String> {
        shell::launch_app(app)
    }
    fn get_window(&mut self, id: u64, app: Option<&str>) -> Result<WindowInfo, String> {
        shell::get_window(id, app)
    }
    fn activate_window(&mut self, id: u64) -> Result<(), String> {
        let hwnd = Self::hwnd(id)?;
        input::activate(hwnd)?;
        self.overlay.raise_for(hwnd);
        Ok(())
    }
    fn window_bounds(&mut self, id: u64) -> Result<Rect, String> {
        let hwnd = Self::hwnd(id)?;
        shell::window_rect(hwnd)
    }
    fn capture_window(&mut self, id: u64, max_edge: u32) -> Result<CaptureResult, String> {
        let hwnd = Self::hwnd(id)?;
        capture::capture_window(hwnd, max_edge)
    }
    fn capture_for_mode(&mut self, id: u64, max_edge: u32, background: bool) -> Result<(CaptureResult, String), String> {
        let hwnd = Self::hwnd(id)?;
        let (cap, tag) = capture::capture_for_mode(hwnd, max_edge, background)?;
        self.last_capture_backend = tag.into();
        Ok((cap, tag.into()))
    }
    fn accessibility_tree(&mut self, id: u64) -> Result<AccessSnapshot, String> {
        let hwnd = Self::hwnd(id)?;
        self.ax.snapshot(hwnd)
    }
    fn element_bounds(&mut self, id: u64, element_index: u64, revision: u64) -> Result<Rect, String> {
        self.ax.element_bounds(id, element_index, revision)
    }
    fn set_value(&mut self, id: u64, element_index: u64, revision: u64, value: &str) -> Result<(), String> {
        self.ax.set_value(id, element_index, revision, value)?;
        self.note("uia");
        Ok(())
    }
    fn scroll_element(&mut self, id: u64, element_index: u64, revision: u64, dir: Direction, pages: f64) -> Result<(), String> {
        self.ax.scroll_element(id, element_index, revision, dir, pages)?;
        self.note("uia");
        Ok(())
    }
    fn secondary_action(&mut self, id: u64, element_index: u64, revision: u64, action: SecondaryAction) -> Result<(), String> {
        self.ax.secondary_action(id, element_index, revision, action)?;
        self.note("uia");
        Ok(())
    }
    fn click_at(&mut self, screen_x: i32, screen_y: i32, button: MouseButton, count: u32) -> Result<(), String> {
        self.overlay.move_to(screen_x, screen_y);
        input::click_at(screen_x, screen_y, button, count)?;
        self.note("sendinput");
        Ok(())
    }
    fn drag(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<(), String> {
        self.overlay.move_to(from.0, from.1);
        input::drag(from, to)?;
        self.note("sendinput");
        Ok(())
    }
    fn scroll_at(&mut self, screen_x: i32, screen_y: i32, dx: i32, dy: i32) -> Result<(), String> {
        self.overlay.move_to(screen_x, screen_y);
        input::scroll_at(screen_x, screen_y, dx, dy)?;
        self.note("sendinput");
        Ok(())
    }
    fn type_text(&mut self, text: &str) -> Result<(), String> {
        input::type_text(text)?;
        self.note("sendinput");
        Ok(())
    }
    fn press_key(&mut self, chord: &Chord) -> Result<(), String> {
        input::press_chord(chord)?;
        self.note("sendinput");
        Ok(())
    }
    fn start_audio(&mut self, max_ms: u64) -> Result<(), String> {
        if std::env::var("SKY_ENABLE_AUDIO").ok().as_deref() != Some("1") {
            return Err("computer audio recording is not enabled (set SKY_ENABLE_AUDIO=1)".into());
        }
        self.audio.start(max_ms)
    }
    fn stop_audio(&mut self) -> Result<AudioResult, String> {
        self.audio.stop()
    }
    fn diagnostic(&mut self, id: Option<u64>) -> Result<Value, String> {
        Ok(json!({
            "cacheDiagnostics": {
                "accessibilitySnapshotCount": self.ax.snapshot_count(),
                "accessibilityRevision": self.ax.current_revision(id),
                "captureCachedSessionCount": 0,
                "lastCaptureInvalidationReason": Value::Null,
                "lastCapturePath": capture::last_path(),
                "lastCapturePathDetail": capture::last_path_detail(),
                "inputMode": self.input_mode_label,
                "lastActionBackend": self.last_backend,
                "lastCaptureBackend": self.last_capture_backend,
                "postMessageEnabled": postmessage_enabled(),
                "cdpPorts": cdp::discovery_ports(),
                "escHookInstalled": true,
            }
        }))
    }
    fn show_overlay(&mut self, visible: bool) {
        self.overlay.set_visible(visible);
    }
    fn ask_approval(&mut self, subject: &str, display: &str, risk: RiskLevel, allow_persistent: bool) -> Result<PersistScope, String> {
        approval::ask(subject, display, risk, allow_persistent)
    }
    fn signature_of(&self, path: &std::path::Path) -> SignatureInfo {
        signature::check(path)
    }

    fn set_input_mode_label(&mut self, mode: &str) {
        self.input_mode_label = mode.into();
    }
    fn last_action_backend(&self) -> String {
        self.last_backend.clone()
    }

    fn click_element_uia(&mut self, id: u64, element_index: u64, revision: u64) -> Result<(), String> {
        self.ax.click_element_background(id, element_index, revision)?;
        self.note("uia");
        Ok(())
    }
    fn append_value_uia(&mut self, id: u64, element_index: u64, revision: u64, text: &str) -> Result<(), String> {
        self.ax.append_value(id, element_index, revision, text)?;
        self.note("uia");
        Ok(())
    }
    fn click_point_uia(&mut self, id: u64, screen_x: i32, screen_y: i32) -> Result<(), String> {
        self.ax.click_point_background(id, screen_x, screen_y)?;
        self.note("uia");
        Ok(())
    }
    fn postmessage_click(&mut self, id: u64, sx: i32, sy: i32, button: MouseButton, count: u32) -> Result<(), String> {
        if !postmessage_enabled() {
            return Err("PostMessage path disabled (set OCU_ALLOW_POSTMESSAGE=1)".into());
        }
        let hwnd = Self::hwnd(id)?;
        postmessage::click(hwnd, sx, sy, button, count)?;
        self.note("postmessage");
        Ok(())
    }
    fn postmessage_type(&mut self, id: u64, text: &str) -> Result<(), String> {
        if !postmessage_enabled() {
            return Err("PostMessage path disabled (set OCU_ALLOW_POSTMESSAGE=1)".into());
        }
        let hwnd = Self::hwnd(id)?;
        postmessage::type_text(hwnd, text)?;
        self.note("postmessage");
        Ok(())
    }
    fn postmessage_key(&mut self, id: u64, chord: &Chord) -> Result<(), String> {
        if !postmessage_enabled() {
            return Err("PostMessage path disabled (set OCU_ALLOW_POSTMESSAGE=1)".into());
        }
        let hwnd = Self::hwnd(id)?;
        postmessage::press_chord(hwnd, chord)?;
        self.note("postmessage");
        Ok(())
    }

    fn action_click_element(
        &mut self,
        mode: InputMode,
        id: u64,
        element_index: u64,
        revision: u64,
        button: MouseButton,
        count: u32,
    ) -> Result<String, String> {
        // UIA first (background / auto).
        if mode != InputMode::Foreground {
            match self.click_element_uia(id, element_index, revision) {
                Ok(()) => return Ok("uia".into()),
                Err(e) if mode == InputMode::Background => {
                    if postmessage_enabled() {
                        let rect = self.element_bounds(id, element_index, revision)?;
                        let (sx, sy) = rect.center();
                        self.postmessage_click(id, sx, sy, button, count)?;
                        return Ok("postmessage".into());
                    }
                    return Err(format!("background click_element failed: {e}"));
                }
                Err(_) => {}
            }
        }
        if mode == InputMode::Background {
            return Err("background mode cannot fall back to SendInput".into());
        }
        self.wait_idle_if_needed(mode)?;
        let rect = self.element_bounds(id, element_index, revision)?;
        let (sx, sy) = rect.center();
        self.activate_window(id)?;
        self.inject_click(sx, sy, button, count)?;
        self.note("sendinput");
        Ok("sendinput".into())
    }

    fn action_click(
        &mut self,
        mode: InputMode,
        id: u64,
        sx: i32,
        sy: i32,
        button: MouseButton,
        count: u32,
    ) -> Result<String, String> {
        if mode != InputMode::Foreground {
            match self.click_point_uia(id, sx, sy) {
                Ok(()) => return Ok("uia".into()),
                Err(e) if mode == InputMode::Background => {
                    if postmessage_enabled() {
                        self.postmessage_click(id, sx, sy, button, count)?;
                        return Ok("postmessage".into());
                    }
                    return Err(format!("background click failed: {e}"));
                }
                Err(_) => {}
            }
        }
        if mode == InputMode::Background {
            return Err("background mode cannot fall back to SendInput".into());
        }
        self.wait_idle_if_needed(mode)?;
        self.activate_window(id)?;
        self.inject_click(sx, sy, button, count)?;
        self.note("sendinput");
        Ok("sendinput".into())
    }

    fn action_type_text(
        &mut self,
        mode: InputMode,
        id: u64,
        text: &str,
        focused: Option<(u64, u64)>,
    ) -> Result<String, String> {
        if mode != InputMode::Foreground {
            // The engine does not yet thread a focused element index through.
            // Fall back to the first edit/document cached by the last snapshot
            // so background type_text can use ValuePattern without SendInput.
            let target = focused.or_else(|| self.ax.first_editable(id));
            if let Some((ei, rev)) = target {
                match self.append_value_uia(id, ei, rev, text) {
                    Ok(()) => return Ok("uia".into()),
                    Err(e) if mode == InputMode::Background => {
                        if postmessage_enabled() {
                            self.postmessage_type(id, text)?;
                            return Ok("postmessage".into());
                        }
                        return Err(format!("background type_text failed: {e}"));
                    }
                    Err(_) => {}
                }
            } else if mode == InputMode::Background {
                if postmessage_enabled() {
                    self.postmessage_type(id, text)?;
                    return Ok("postmessage".into());
                }
                return Err("background type_text needs a focused ValuePattern element or OCU_ALLOW_POSTMESSAGE=1".into());
            }
        }
        if mode == InputMode::Background {
            return Err("background mode cannot fall back to SendInput".into());
        }
        self.wait_idle_if_needed(mode)?;
        self.activate_window(id)?;
        self.inject_type(text)?;
        self.note("sendinput");
        Ok("sendinput".into())
    }

    fn action_press_key(&mut self, mode: InputMode, id: u64, chord: &Chord) -> Result<String, String> {
        if mode == InputMode::Background {
            if postmessage_enabled() {
                self.postmessage_key(id, chord)?;
                return Ok("postmessage".into());
            }
            return Err("background press_key requires OCU_ALLOW_POSTMESSAGE=1 (no UIA key path)".into());
        }
        if mode == InputMode::Auto && postmessage_enabled() {
            if self.postmessage_key(id, chord).is_ok() {
                return Ok("postmessage".into());
            }
        }
        self.wait_idle_if_needed(mode)?;
        self.activate_window(id)?;
        self.inject_key(chord)?;
        self.note("sendinput");
        Ok("sendinput".into())
    }

    fn action_drag(
        &mut self,
        mode: InputMode,
        id: u64,
        from: (i32, i32),
        to: (i32, i32),
    ) -> Result<String, String> {
        if mode == InputMode::Background {
            return Err("background mode does not support drag (needs SendInput)".into());
        }
        self.wait_idle_if_needed(mode)?;
        self.activate_window(id)?;
        self.inject_drag(from, to)?;
        self.note("sendinput");
        Ok("sendinput".into())
    }

    fn action_scroll(
        &mut self,
        mode: InputMode,
        id: u64,
        sx: i32,
        sy: i32,
        dx: i32,
        dy: i32,
    ) -> Result<String, String> {
        if mode == InputMode::Background {
            return Err("background mode does not support coordinate scroll (use scroll_element)".into());
        }
        self.wait_idle_if_needed(mode)?;
        self.activate_window(id)?;
        self.inject_scroll(sx, sy, dx, dy)?;
        self.note("sendinput");
        Ok("sendinput".into())
    }

    fn browser_list_tabs(&mut self) -> Result<Value, String> {
        let targets = cdp::discover_targets();
        let arr: Vec<Value> = targets
            .into_iter()
            .map(|t| {
                json!({
                    "id": t.id,
                    "title": t.title,
                    "url": t.url,
                    "type": t.typ,
                    "port": t.port,
                    "wsUrl": t.ws_url,
                })
            })
            .collect();
        self.note("cdp");
        Ok(json!(arr))
    }

    fn browser_call(&mut self, tab_id: &str, method: &str, params: Value) -> Result<Value, String> {
        let targets = cdp::discover_targets();
        let t = targets
            .into_iter()
            .find(|t| t.id == tab_id)
            .ok_or_else(|| format!("cdp tab id '{tab_id}' not found; call browser_list_tabs"))?;
        let mut session = cdp::CdpSession::connect(&t.ws_url)?;
        let result = session.call(method, params)?;
        self.note("cdp");
        Ok(result)
    }

    fn browser_screenshot(&mut self, tab_id: &str) -> Result<Value, String> {
        let r = self.browser_call(tab_id, "Page.captureScreenshot", cdp::screenshot_params())?;
        let data = r.get("data").and_then(Value::as_str).unwrap_or("");
        Ok(json!({
            "url": format!("data:image/png;base64,{data}"),
            "backend": "cdp",
        }))
    }

    fn browser_get_state(&mut self, tab_id: &str) -> Result<Value, String> {
        let r = self.browser_call(tab_id, "Accessibility.getFullAXTree", json!({}))?;
        let (tree, nodes) = cdp::format_ax_tree(&r);
        Ok(json!({
            "tree": tree,
            "element_count": nodes.len(),
            "backend": "cdp",
        }))
    }

    fn browser_click(&mut self, tab_id: &str, x: f64, y: f64, button: &str, click_count: u32) -> Result<Value, String> {
        let _ = self.browser_call(tab_id, "Input.dispatchMouseEvent", cdp::click_params(x, y, button, click_count))?;
        let _ = self.browser_call(tab_id, "Input.dispatchMouseEvent", cdp::click_release_params(x, y, button, click_count))?;
        Ok(json!({"backend": "cdp"}))
    }

    fn browser_type(&mut self, tab_id: &str, text: &str) -> Result<Value, String> {
        let _ = self.browser_call(tab_id, "Input.insertText", cdp::insert_text_params(text))?;
        Ok(json!({"backend": "cdp"}))
    }

    fn browser_press_key(&mut self, tab_id: &str, key: &str) -> Result<Value, String> {
        let _ = self.browser_call(tab_id, "Input.dispatchKeyEvent", cdp::key_params(key, "keyDown"))?;
        let _ = self.browser_call(tab_id, "Input.dispatchKeyEvent", cdp::key_params(key, "keyUp"))?;
        Ok(json!({"backend": "cdp"}))
    }

    fn browser_scroll(&mut self, tab_id: &str, x: f64, y: f64, dx: f64, dy: f64) -> Result<Value, String> {
        let _ = self.browser_call(tab_id, "Input.dispatchMouseEvent", cdp::scroll_params(x, y, dx, dy))?;
        Ok(json!({"backend": "cdp"}))
    }

    fn browser_navigate(&mut self, tab_id: &str, url: &str) -> Result<Value, String> {
        let _ = self.browser_call(tab_id, "Page.navigate", cdp::navigate_params(url))?;
        Ok(json!({"backend": "cdp"}))
    }

    fn list_displays(&mut self) -> Result<Vec<crate::monitor::DisplayInfo>, String> {
        display::list_displays()
    }

    fn move_window_to_rect(&mut self, id: u64, dest: Rect) -> Result<(), String> {
        let hwnd = Self::hwnd(id)?;
        shell::move_window_to(hwnd, dest)?;
        self.note("sendinput");
        Ok(())
    }

    fn launch_app_on_monitor(&mut self, app: &str, work: Rect) -> Result<(), String> {
        shell::launch_app(app)?;
        match shell::wait_and_place_launched(app, work, 15_000) {
            Ok(_) => Ok(()),
            Err(e) => {
                // Launch succeeded but placement timed out — still Ok with warning path.
                Err(e)
            }
        }
    }

    fn capture_monitor(&mut self, monitor: &Rect, max_edge: u32) -> Result<CaptureResult, String> {
        let c = capture::capture_monitor(monitor, max_edge)?;
        self.last_capture_backend = "wgc".into();
        Ok(c)
    }

    fn clamp_inject_point(&mut self, x: i32, y: i32) -> (i32, i32) {
        self.clamp_xy(x, y)
    }

    fn set_scope_rect(&mut self, r: Option<Rect>) {
        self.scope_rect = r;
    }
}

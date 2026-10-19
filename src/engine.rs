//! Call dispatcher: protocol -> policy -> platform backend.

use crate::activity::ActivityTracker;
use crate::geometry::{self, CaptureGeometry, Rect};
use crate::mode::{self, InputMode};
use crate::monitor::{self, DisplayInfo, MonitorSelector};
use crate::keys;
use crate::policy::{self, Decision, PersistScope, PolicyEngine, RiskLevel};
use crate::protocol::{self, ApprovalRequest, Call, MouseButton, TurnKey, WindowRef};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Abstract OS backend. The Windows implementation lives under `win/`; the
/// Linux stub exists so unit tests and `cargo test` work on the build host.
pub trait Backend: Send {
    fn list_windows(&mut self) -> Result<Vec<WindowInfo>, String>;
    fn list_apps(&mut self) -> Result<Vec<AppInfo>, String>;
    fn launch_app(&mut self, app: &str) -> Result<(), String>;
    fn get_window(&mut self, id: u64, app: Option<&str>) -> Result<WindowInfo, String>;
    fn activate_window(&mut self, id: u64) -> Result<(), String>;
    fn window_bounds(&mut self, id: u64) -> Result<Rect, String>;
    fn capture_window(&mut self, id: u64, max_edge: u32) -> Result<CaptureResult, String>;
    fn accessibility_tree(&mut self, id: u64) -> Result<AccessSnapshot, String>;
    fn element_bounds(&mut self, id: u64, element_index: u64, revision: u64) -> Result<Rect, String>;
    fn set_value(&mut self, id: u64, element_index: u64, revision: u64, value: &str) -> Result<(), String>;
    fn scroll_element(&mut self, id: u64, element_index: u64, revision: u64, dir: protocol::Direction, pages: f64) -> Result<(), String>;
    fn secondary_action(&mut self, id: u64, element_index: u64, revision: u64, action: protocol::SecondaryAction) -> Result<(), String>;
    fn click_at(&mut self, screen_x: i32, screen_y: i32, button: MouseButton, count: u32) -> Result<(), String>;
    fn drag(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<(), String>;
    fn scroll_at(&mut self, screen_x: i32, screen_y: i32, dx: i32, dy: i32) -> Result<(), String>;
    fn type_text(&mut self, text: &str) -> Result<(), String>;
    fn press_key(&mut self, chord: &keys::Chord) -> Result<(), String>;
    fn start_audio(&mut self, max_ms: u64) -> Result<(), String>;
    fn stop_audio(&mut self) -> Result<AudioResult, String>;
    fn diagnostic(&mut self, id: Option<u64>) -> Result<Value, String>;
    fn show_overlay(&mut self, _visible: bool) {}
    fn ask_approval(&mut self, subject: &str, display: &str, risk: RiskLevel, allow_persistent: bool) -> Result<PersistScope, String>;
    fn signature_of(&self, path: &std::path::Path) -> policy::SignatureInfo {
        policy::null_signature_checker(path)
    }

    fn capture_for_mode(&mut self, id: u64, max_edge: u32, background: bool) -> Result<(CaptureResult, String), String> {
        let _ = background;
        Ok((self.capture_window(id, max_edge)?, "foreground".into()))
    }
    fn set_input_mode_label(&mut self, _mode: &str) {}
    fn last_action_backend(&self) -> String { String::new() }
    fn click_element_uia(&mut self, _id: u64, _ei: u64, _rev: u64) -> Result<(), String> {
        Err("UIA click not available".into())
    }
    fn append_value_uia(&mut self, _id: u64, _ei: u64, _rev: u64, _text: &str) -> Result<(), String> {
        Err("UIA type not available".into())
    }
    fn click_point_uia(&mut self, _id: u64, _x: i32, _y: i32) -> Result<(), String> {
        Err("UIA point click not available".into())
    }
    fn postmessage_click(&mut self, _id: u64, _x: i32, _y: i32, _b: MouseButton, _c: u32) -> Result<(), String> {
        Err("PostMessage not available".into())
    }
    fn postmessage_type(&mut self, _id: u64, _t: &str) -> Result<(), String> {
        Err("PostMessage not available".into())
    }
    fn postmessage_key(&mut self, _id: u64, _c: &keys::Chord) -> Result<(), String> {
        Err("PostMessage not available".into())
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
        let _ = (mode, revision);
        let rect = self.element_bounds(id, element_index, revision)?;
        let (sx, sy) = rect.center();
        self.activate_window(id)?;
        self.click_at(sx, sy, button, count)?;
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
        let _ = mode;
        self.activate_window(id)?;
        self.click_at(sx, sy, button, count)?;
        Ok("sendinput".into())
    }
    fn action_type_text(
        &mut self,
        mode: InputMode,
        id: u64,
        text: &str,
        _focused: Option<(u64, u64)>,
    ) -> Result<String, String> {
        let _ = mode;
        self.activate_window(id)?;
        self.type_text(text)?;
        Ok("sendinput".into())
    }
    fn action_press_key(&mut self, mode: InputMode, id: u64, chord: &keys::Chord) -> Result<String, String> {
        let _ = mode;
        self.activate_window(id)?;
        self.press_key(chord)?;
        Ok("sendinput".into())
    }
    fn action_drag(
        &mut self,
        mode: InputMode,
        id: u64,
        from: (i32, i32),
        to: (i32, i32),
    ) -> Result<String, String> {
        let _ = mode;
        self.activate_window(id)?;
        self.drag(from, to)?;
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
        let _ = mode;
        self.activate_window(id)?;
        self.scroll_at(sx, sy, dx, dy)?;
        Ok("sendinput".into())
    }
    fn browser_list_tabs(&mut self) -> Result<Value, String> {
        Err("CDP browser tools require a Windows host with a debug Chromium".into())
    }
    fn browser_screenshot(&mut self, _tab_id: &str) -> Result<Value, String> {
        Err("CDP not available".into())
    }
    fn browser_get_state(&mut self, _tab_id: &str) -> Result<Value, String> {
        Err("CDP not available".into())
    }
    fn browser_click(&mut self, _tab_id: &str, _x: f64, _y: f64, _button: &str, _click_count: u32) -> Result<Value, String> {
        Err("CDP not available".into())
    }
    fn browser_type(&mut self, _tab_id: &str, _text: &str) -> Result<Value, String> {
        Err("CDP not available".into())
    }
    fn browser_press_key(&mut self, _tab_id: &str, _key: &str) -> Result<Value, String> {
        Err("CDP not available".into())
    }
    fn browser_scroll(&mut self, _tab_id: &str, _x: f64, _y: f64, _dx: f64, _dy: f64) -> Result<Value, String> {
        Err("CDP not available".into())
    }
    fn browser_navigate(&mut self, _tab_id: &str, _url: &str) -> Result<Value, String> {
        Err("CDP not available".into())
    }
    fn browser_call(&mut self, _tab_id: &str, _method: &str, _params: Value) -> Result<Value, String> {
        Err("CDP not available".into())
    }
    fn list_displays(&mut self) -> Result<Vec<DisplayInfo>, String> {
        // Linux stub: one virtual display.
        Ok(vec![DisplayInfo {
            index: 0,
            device_name: r"\\.\DISPLAY1".into(),
            friendly_name: Some("Stub Display".into()),
            rect: Rect::new(0, 0, 1920, 1080),
            work_area: Rect::new(0, 0, 1920, 1040),
            dpi: 96,
            scale: 1.0,
            primary: true,
        }])
    }
    fn move_window_to_rect(&mut self, _id: u64, _dest: Rect) -> Result<(), String> {
        Ok(())
    }
    fn launch_app_on_monitor(&mut self, app: &str, _work: Rect) -> Result<(), String> {
        self.launch_app(app)
    }
    fn capture_monitor(&mut self, _monitor: &Rect, max_edge: u32) -> Result<CaptureResult, String> {
        // Stub: reuse first window capture shape.
        self.capture_window(1, max_edge)
    }
    fn clamp_inject_point(&mut self, x: i32, y: i32) -> (i32, i32) {
        (x, y)
    }
    fn set_scope_rect(&mut self, _r: Option<Rect>) {}
}

#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub id: u64,
    pub app: String,
    pub title: Option<String>,
    pub process_path: Option<String>,
    pub pid: u32,
    pub bounds: Rect,
}

#[derive(Debug, Clone)]
pub struct AppInfo {
    pub id: String,
    pub display_name: Option<String>,
    pub windows: Vec<WindowInfo>,
    pub last_used_date: Option<String>,
    pub use_count: Option<u64>,
    pub is_running: Option<bool>,
    pub process_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CaptureResult {
    pub png_data_url: String,
    pub geometry: CaptureGeometry,
    pub screenshot_id: String,
}

#[derive(Debug, Clone)]
pub struct AccessSnapshot {
    pub tree: String,
    pub focused_element: Option<String>,
    pub selected_text: Option<String>,
    pub selected_elements: Option<Vec<String>>,
    pub document_text: Option<String>,
    pub revision: u64,
}

#[derive(Debug, Clone)]
pub struct AudioResult {
    pub filepath: String,
}

struct ShotCache {
    window_id: u64,
    geometry: CaptureGeometry,
    app: String,
}

struct AxCache {
    window_id: u64,
    revision: u64,
}

pub struct Engine {
    pub home: PathBuf,
    pub policy: PolicyEngine,
    pub interrupts: policy::InterruptState,
    pub backend: Box<dyn Backend>,
    shots: HashMap<String, ShotCache>,
    ax: Option<AxCache>,
    current_turn: Option<TurnKey>,
    shot_seq: u64,
    max_edge: u32,
    closed: bool,
    /// When true (stdio-native), Esc causes process exit 130.
    pub exit_on_esc: bool,
    pub exit_code: Option<i32>,
    pub input_mode: InputMode,
    pub activity: Arc<ActivityTracker>,
    /// Mode resolved for the current request (params override).
    request_mode: InputMode,
    /// Active single-monitor scope (from OCU_MONITOR or scope_monitor).
    pub scope: Option<MonitorSelector>,
    /// Cached displays for the session (refreshed by list_displays / scope_monitor).
    displays: Vec<DisplayInfo>,
}

impl Engine {
    pub fn new(backend: Box<dyn Backend>, exit_on_esc: bool) -> Self {
        Self::with_activity(backend, exit_on_esc, ActivityTracker::new())
    }

    pub fn with_activity(backend: Box<dyn Backend>, exit_on_esc: bool, activity: Arc<ActivityTracker>) -> Self {
        let home = policy::resolve_home();
        let input_mode = mode::default_mode_from_env();
        Self {
            policy: PolicyEngine::new(&home),
            home,
            interrupts: policy::InterruptState::default(),
            backend,
            shots: HashMap::new(),
            ax: None,
            current_turn: None,
            shot_seq: 0,
            max_edge: std::env::var("OCU_REPLICA_MAX_EDGE").ok().and_then(|s| s.parse().ok()).unwrap_or(1600),
            closed: false,
            exit_on_esc,
            exit_code: None,
            input_mode,
            activity,
            request_mode: input_mode,
            scope: monitor::default_selector_from_env(),
            displays: Vec::new(),
        }
    }

    pub fn handle(&mut self, method: &str, params: &Value, meta: &Map<String, Value>) -> EngineOutcome {
        if self.closed {
            return EngineOutcome::Err("helper is closed".into());
        }
        self.request_mode = match mode::resolve_mode(params, self.input_mode) {
            Ok(m) => m,
            Err(e) => return EngineOutcome::Err(e),
        };
        self.backend.set_input_mode_label(self.request_mode.as_str());
        // Esc latch / marker.
        if let Some(tk) = protocol::turn_key(meta) {
            if policy::interrupt_marker_exists(&self.home, &tk.session, &tk.turn)
                || self.interrupts.is_stopped(&tk.session, &tk.turn)
            {
                return EngineOutcome::Err(ESC_MSG.into());
            }
            self.current_turn = Some(tk);
        }
        if self.interrupts.latched {
            if let Some(tk) = self.current_turn.clone() {
                let _ = policy::write_interrupt_marker(&self.home, &tk.session, &tk.turn);
                self.interrupts.mark(&tk.session, &tk.turn);
            }
            if self.exit_on_esc {
                self.exit_code = Some(130);
            }
            return EngineOutcome::Err(ESC_MSG.into());
        }

        let call = match protocol::parse_call(method, params) {
            Ok(c) => c,
            Err(e) => return EngineOutcome::Err(e),
        };

        // Pre-approved app from meta short-circuits the dialog for one call.
        if let Some(app) = protocol::approved_app(meta) {
            self.policy.store.grant(&app, PersistScope::Session);
        }

        match self.dispatch(call) {
            Ok(v) => EngineOutcome::Ok(v),
            Err(EngineError::Approval(a)) => EngineOutcome::Approval(a),
            Err(EngineError::Msg(m)) => EngineOutcome::Err(m),
            Err(EngineError::Close) => {
                self.closed = true;
                EngineOutcome::Ok(Value::Null)
            }
        }
    }

    /// Called by the Esc hook thread.
    pub fn on_physical_esc(&mut self) {
        self.interrupts.latched = true;
        if let Some(tk) = self.current_turn.clone() {
            let _ = policy::write_interrupt_marker(&self.home, &tk.session, &tk.turn);
            self.interrupts.mark(&tk.session, &tk.turn);
        }
        if self.exit_on_esc {
            self.exit_code = Some(130);
        }
    }

    fn dispatch(&mut self, call: Call) -> Result<Value, EngineError> {
        match call {
            Call::Close => return Err(EngineError::Close),
            Call::EndTurn => {
                if let Some(tk) = self.current_turn.take() {
                    self.interrupts.clear_turn(&tk.session, &tk.turn);
                }
                self.interrupts.latched = false;
                self.backend.show_overlay(false);
                return Ok(Value::Null);
            }
            Call::ListApps => {
                let mut apps = self.backend.list_apps().map_err(EngineError::Msg)?;
                if let Some(mon) = self.scoped_rect()? {
                    for a in &mut apps {
                        a.windows.retain(|w| monitor::window_mostly_on_monitor(&w.bounds, &mon));
                    }
                    apps.retain(|a| a.is_running == Some(true) && !a.windows.is_empty() || a.is_running != Some(true));
                }
                return Ok(Value::Array(apps.into_iter().map(app_to_json).collect()));
            }
            Call::ListWindows => {
                let mut wins = self.backend.list_windows().map_err(EngineError::Msg)?;
                if let Some(mon) = self.scoped_rect()? {
                    wins.retain(|w| monitor::window_mostly_on_monitor(&w.bounds, &mon));
                }
                return Ok(Value::Array(wins.into_iter().map(window_to_json).collect()));
            }
            Call::GetWindow { id, app } => {
                let w = self.backend.get_window(id, app.as_deref()).map_err(EngineError::Msg)?;
                self.ensure_window_in_scope(&w)?;
                return Ok(window_to_json(w));
            }
            Call::DiagnosticState { window } => {
                let id = window.map(|w| w.id);
                let mut v = self.backend.diagnostic(id).map_err(EngineError::Msg)?;
                if let Some(obj) = v.as_object_mut() {
                    if let Some(cd) = obj.get_mut("cacheDiagnostics").and_then(|c| c.as_object_mut()) {
                        cd.insert("inputMode".into(), json!(self.request_mode.as_str()));
                        cd.insert("backend".into(), json!(self.backend.last_action_backend()));
                        if let Some(sel) = &self.scope {
                            cd.insert("scopeMonitor".into(), json!(format!("{sel:?}")));
                        }
                    }
                }
                return Ok(v);
            }
            Call::StartAudioRecording { max_duration_ms } => {
                self.ensure_approved("computer-audio", "computer audio", None, true)?;
                self.backend.start_audio(max_duration_ms).map_err(EngineError::Msg)?;
                return Ok(Value::Null);
            }
            Call::StopAudioRecording => {
                let a = self.backend.stop_audio().map_err(EngineError::Msg)?;
                return Ok(json!({"filepath": a.filepath}));
            }
            Call::LaunchApp { app } => {
                self.ensure_approved(&app, &app, Some(&app), false)?;
                if let Some(work) = self.scoped_work_area()? {
                    self.backend.launch_app_on_monitor(&app, work).map_err(EngineError::Msg)?;
                } else {
                    self.backend.launch_app(&app).map_err(EngineError::Msg)?;
                }
                return Ok(Value::Null);
            }
            Call::ListDisplays => {
                self.refresh_displays()?;
                return Ok(Value::Array(self.displays.iter().map(|d| d.to_json()).collect()));
            }
            Call::ScopeMonitor { selector } => {
                self.refresh_displays()?;
                match selector {
                    None => {
                        self.scope = None;
                        self.backend.set_scope_rect(None);
                        return Ok(json!({"scope": Value::Null, "message": "monitor scope cleared"}));
                    }
                    Some(s) => {
                        let sel = monitor::parse_monitor_selector(&s).map_err(EngineError::Msg)?;
                        let d = monitor::resolve_display(&self.displays, &sel).map_err(EngineError::Msg)?;
                        let out = d.to_json();
                        self.scope = Some(sel);
                        self.sync_scope_to_backend();
                        return Ok(json!({"scope": out}));
                    }
                }
            }
            Call::BrowserListTabs => {
                return self.backend.browser_list_tabs().map_err(EngineError::Msg);
            }
            Call::BrowserScreenshot { tab_id } => {
                return self.backend.browser_screenshot(&tab_id).map_err(EngineError::Msg);
            }
            Call::BrowserGetState { tab_id } => {
                return self.backend.browser_get_state(&tab_id).map_err(EngineError::Msg);
            }
            Call::BrowserClick { tab_id, x, y, button, click_count } => {
                return self.backend.browser_click(&tab_id, x, y, &button, click_count).map_err(EngineError::Msg);
            }
            Call::BrowserType { tab_id, text } => {
                return self.backend.browser_type(&tab_id, &text).map_err(EngineError::Msg);
            }
            Call::BrowserPressKey { tab_id, key } => {
                return self.backend.browser_press_key(&tab_id, &key).map_err(EngineError::Msg);
            }
            Call::BrowserScroll { tab_id, x, y, scroll_x, scroll_y } => {
                return self.backend.browser_scroll(&tab_id, x, y, scroll_x, scroll_y).map_err(EngineError::Msg);
            }
            Call::BrowserNavigate { tab_id, url } => {
                // Blocklist/approvals: treat navigate URL host as subject when possible.
                self.ensure_approved("browser-cdp", &url, None, false)?;
                return self.backend.browser_navigate(&tab_id, &url).map_err(EngineError::Msg);
            }
            other => {
                let window = other.window().cloned().ok_or_else(|| EngineError::Msg("internal: missing window".into()))?;
                self.ensure_window_allowed(&window)?;
                let mode = self.request_mode;
                match other {
                    Call::ActivateWindow { window } => {
                        if mode == InputMode::Background {
                            return Err(EngineError::Msg(
                                "activate_window is a foreground action; refuse in background mode".into(),
                            ));
                        }
                        self.backend.activate_window(window.id).map_err(EngineError::Msg)?;
                        Ok(json!({"backend": "sendinput"}))
                    }
                    Call::GetWindowState { window, include_screenshot, include_text } => {
                        self.get_window_state(&window, include_screenshot, include_text)
                    }
                    Call::Click { window, x, y, screenshot_id, click_count, button } => {
                        let (sx, sy) = self.resolve_point(&window, x, y, screenshot_id.as_deref())?;
                        self.ensure_point_in_scope(sx, sy)?;
                        let backend = self
                            .backend
                            .action_click(mode, window.id, sx, sy, button, click_count)
                            .map_err(EngineError::Msg)?;
                        Ok(json!({"backend": backend}))
                    }
                    Call::ClickElement { window, element_index, click_count, button } => {
                        let rev = self.ax_revision(window.id)?;
                        let rect = self.backend.element_bounds(window.id, element_index, rev).map_err(EngineError::Msg)?;
                        let (sx, sy) = rect.center();
                        self.ensure_point_in_scope(sx, sy)?;
                        self.verify_live_bounds(&window)?;
                        let backend = self
                            .backend
                            .action_click_element(mode, window.id, element_index, rev, button, click_count)
                            .map_err(EngineError::Msg)?;
                        Ok(json!({"backend": backend}))
                    }
                    Call::Drag { window, from, to, screenshot_id } => {
                        let a = self.resolve_point(&window, from.0, from.1, screenshot_id.as_deref())?;
                        let b = self.resolve_point(&window, to.0, to.1, screenshot_id.as_deref())?;
                        self.ensure_point_in_scope(a.0, a.1)?;
                        self.ensure_point_in_scope(b.0, b.1)?;
                        let backend = self.backend.action_drag(mode, window.id, a, b).map_err(EngineError::Msg)?;
                        Ok(json!({"backend": backend}))
                    }
                    Call::Scroll { window, x, y, screenshot_id, scroll_x, scroll_y } => {
                        let (sx, sy) = self.resolve_point(&window, x, y, screenshot_id.as_deref())?;
                        self.ensure_point_in_scope(sx, sy)?;
                        let backend = self
                            .backend
                            .action_scroll(mode, window.id, sx, sy, scroll_x.round() as i32, scroll_y.round() as i32)
                            .map_err(EngineError::Msg)?;
                        Ok(json!({"backend": backend}))
                    }
                    Call::ScrollElement { window, element_index, direction, pages } => {
                        let rev = self.ax_revision(window.id)?;
                        self.backend.scroll_element(window.id, element_index, rev, direction, pages).map_err(EngineError::Msg)?;
                        Ok(json!({"backend": "uia"}))
                    }
                    Call::TypeText { window, text } => {
                        let focused = self.ax.as_ref().and_then(|ax| {
                            if ax.window_id == window.id {
                                // Best-effort: no focused index cached; None → ValuePattern/PostMessage/SendInput.
                                None
                            } else {
                                None
                            }
                        });
                        let backend = self
                            .backend
                            .action_type_text(mode, window.id, &text, focused)
                            .map_err(EngineError::Msg)?;
                        Ok(json!({"backend": backend}))
                    }
                    Call::PressKey { window, key } => {
                        let chord = keys::parse_chord(&key).map_err(EngineError::Msg)?;
                        let backend = self.backend.action_press_key(mode, window.id, &chord).map_err(EngineError::Msg)?;
                        Ok(json!({"backend": backend}))
                    }
                    Call::SetValue { window, element_index, value } => {
                        let rev = self.ax_revision(window.id)?;
                        self.backend.set_value(window.id, element_index, rev, &value).map_err(EngineError::Msg)?;
                        Ok(json!({"backend": "uia"}))
                    }
                    Call::PerformSecondaryAction { window, element_index, action } => {
                        let rev = self.ax_revision(window.id)?;
                        self.backend.secondary_action(window.id, element_index, rev, action).map_err(EngineError::Msg)?;
                        Ok(json!({"backend": "uia"}))
                    }
                    Call::MoveWindowToScope { window } => {
                        let work = self.scoped_work_area()?.ok_or_else(|| {
                            EngineError::Msg("out_of_scope: no monitor scope set (call scope_monitor or set OCU_MONITOR)".into())
                        })?;
                        let bounds = self.backend.window_bounds(window.id).map_err(EngineError::Msg)?;
                        let dest = monitor::place_window_in_work_area(&work, bounds.width(), bounds.height());
                        self.backend.move_window_to_rect(window.id, dest).map_err(EngineError::Msg)?;
                        Ok(json!({"backend": "sendinput", "rect": {
                            "left": dest.left, "top": dest.top, "right": dest.right, "bottom": dest.bottom
                        }}))
                    }
                    _ => Err(EngineError::Msg("internal: unhandled call".into())),
                }
            }
        }
    }

    fn ensure_window_allowed(&mut self, window: &WindowRef) -> Result<(), EngineError> {
        let info = self.backend.get_window(window.id, Some(&window.app)).map_err(EngineError::Msg)?;
        self.ensure_window_in_scope(&info)?;
        // Ownership check.
        if !info.app.eq_ignore_ascii_case(&window.app) && !window.app.is_empty() {
            // Allow process-path subjects that share a basename.
            let same = info.process_path.as_deref().map(|p| p.eq_ignore_ascii_case(&window.app)).unwrap_or(false);
            if !same {
                return Err(EngineError::Msg(format!(
                    "window id {} no longer belongs to {}; current owner is {}",
                    window.id, window.app, info.app
                )));
            }
        }
        let subject = info.process_path.clone().unwrap_or_else(|| info.app.clone());
        let display = info.title.clone().unwrap_or_else(|| info.app.clone());
        self.ensure_approved(&subject, &display, info.process_path.as_deref(), false)
    }

    fn ensure_approved(&mut self, subject: &str, display: &str, path: Option<&str>, is_audio: bool) -> Result<(), EngineError> {
        match self.policy.evaluate(subject, display, path, is_audio) {
            Decision::Allowed => Ok(()),
            Decision::Denied { reason } => Err(EngineError::Msg(reason)),
            Decision::NeedsApproval { subject, display_name, risk, allow_persistent } => {
                // Try an interactive dialog first; if the backend cannot show one
                // (headless / MCP without elicitation bridge), surface approvalRequest.
                match self.backend.ask_approval(&subject, &display_name, risk, allow_persistent) {
                    Ok(scope) => {
                        self.policy.grant(&subject, scope);
                        Ok(())
                    }
                    Err(_) => Err(EngineError::Approval(ApprovalRequest {
                        app: subject,
                        display_name,
                        risk_level: match risk {
                            RiskLevel::High => "high".into(),
                            RiskLevel::Low => "low".into(),
                        },
                        allow_persistent_approval: allow_persistent,
                    })),
                }
            }
        }
    }

    fn get_window_state(&mut self, window: &WindowRef, include_screenshot: bool, include_text: bool) -> Result<Value, EngineError> {
        let mode = self.request_mode;
        if mode == InputMode::Foreground {
            self.backend.activate_window(window.id).ok();
        }
        let mut screenshots = Vec::new();
        let mut capture_backend = String::new();
        if include_screenshot {
            let background = mode != InputMode::Foreground;
            let (cap, tag) = self
                .backend
                .capture_for_mode(window.id, self.max_edge, background)
                .map_err(EngineError::Msg)?;
            capture_backend = tag;
            self.shot_seq += 1;
            let id = format!("screenshot-{}", self.shot_seq - 1);
            self.shots.insert(id.clone(), ShotCache {
                window_id: window.id,
                geometry: cap.geometry,
                app: window.app.clone(),
            });
            screenshots.push(json!({
                "id": id,
                "zIndex": 0,
                "url": cap.png_data_url,
                "originX": cap.geometry.window.left,
                "originY": cap.geometry.window.top,
                "width": cap.geometry.image_w,
                "height": cap.geometry.image_h,
            }));
        }
        let accessibility = if include_text {
            let ax = self.backend.accessibility_tree(window.id).map_err(EngineError::Msg)?;
            self.ax = Some(AxCache { window_id: window.id, revision: ax.revision });
            Some(json!({
                "tree": ax.tree,
                "focused_element": ax.focused_element,
                "selected_text": ax.selected_text,
                "selected_elements": ax.selected_elements,
                "document_text": ax.document_text,
            }))
        } else {
            None
        };
        let info = self.backend.get_window(window.id, Some(&window.app)).map_err(EngineError::Msg)?;
        let out = json!({
            "window": window_to_json(info),
            "screenshots": screenshots,
            "accessibility": accessibility,
            "backend": if capture_backend.is_empty() { Value::Null } else { json!(capture_backend) },
            "input_mode": mode.as_str(),
        });
        Ok(out)
    }

    fn refresh_displays(&mut self) -> Result<(), EngineError> {
        self.displays = self.backend.list_displays().map_err(EngineError::Msg)?;
        self.sync_scope_to_backend();
        Ok(())
    }

    fn sync_scope_to_backend(&mut self) {
        let rect = self.scope.as_ref().and_then(|sel| {
            monitor::resolve_display(&self.displays, sel).ok().map(|d| d.rect)
        });
        self.backend.set_scope_rect(rect);
    }

    fn scoped_rect(&mut self) -> Result<Option<Rect>, EngineError> {
        let Some(sel) = self.scope.clone() else { return Ok(None) };
        if self.displays.is_empty() {
            self.refresh_displays()?;
        }
        let d = monitor::resolve_display(&self.displays, &sel).map_err(EngineError::Msg)?;
        Ok(Some(d.rect))
    }

    fn scoped_work_area(&mut self) -> Result<Option<Rect>, EngineError> {
        let Some(sel) = self.scope.clone() else { return Ok(None) };
        if self.displays.is_empty() {
            self.refresh_displays()?;
        }
        let d = monitor::resolve_display(&self.displays, &sel).map_err(EngineError::Msg)?;
        Ok(Some(d.work_area))
    }

    fn ensure_window_in_scope(&mut self, w: &WindowInfo) -> Result<(), EngineError> {
        if let Some(mon) = self.scoped_rect()? {
            if !monitor::window_mostly_on_monitor(&w.bounds, &mon) {
                return Err(EngineError::Msg(monitor::out_of_scope_msg(&format!(
                    "window {} is not mostly on the scoped monitor",
                    w.id
                ))));
            }
        }
        Ok(())
    }

    fn ensure_point_in_scope(&mut self, x: i32, y: i32) -> Result<(), EngineError> {
        if let Some(mon) = self.scoped_rect()? {
            if !monitor::point_in_monitor(x, y, &mon) {
                return Err(EngineError::Msg(monitor::out_of_scope_msg(&format!(
                    "coordinate ({x},{y}) is outside the scoped monitor"
                ))));
            }
        }
        Ok(())
    }

    fn resolve_point(&mut self, window: &WindowRef, x: f64, y: f64, screenshot_id: Option<&str>) -> Result<(i32, i32), EngineError> {
        // Prefer the named screenshot; fall back to the most recent for this window.
        let geo = if let Some(id) = screenshot_id {
            let shot = self.shots.get(id).ok_or_else(|| EngineError::Msg(format!("unknown screenshotId {id}")))?;
            if shot.window_id != window.id {
                return Err(EngineError::Msg("screenshotId belongs to another window".into()));
            }
            shot.geometry
        } else {
            let mut best: Option<&ShotCache> = None;
            let mut best_id: Option<&str> = None;
            for (id, s) in &self.shots {
                if s.window_id == window.id {
                    if best_id.map(|b| id.as_str() > b).unwrap_or(true) {
                        best_id = Some(id.as_str());
                        best = Some(s);
                    }
                }
            }
            best.map(|s| s.geometry).ok_or_else(|| EngineError::Msg("call get_window_state before using this window".into()))?
        };
        let (sx, sy) = geometry::screenshot_to_screen(&geo, x, y).map_err(EngineError::Msg)?;
        self.verify_live_bounds(window)?;
        // Also confirm the live window still matches the capture geometry.
        let live = self.backend.window_bounds(window.id).map_err(EngineError::Msg)?;
        if !geometry::bounds_match(&live, &geo.window, geometry::BOUNDS_SLACK_PX) {
            return Err(EngineError::Msg(
                "window bounds changed before coordinate input; call get_window_state".into(),
            ));
        }
        Ok((sx, sy))
    }

    fn verify_live_bounds(&mut self, window: &WindowRef) -> Result<(), EngineError> {
        let _ = self.backend.window_bounds(window.id).map_err(|e| {
            EngineError::Msg(format!("window bounds unavailable for coordinate input: {e}"))
        })?;
        Ok(())
    }

    fn ax_revision(&self, window_id: u64) -> Result<u64, EngineError> {
        match &self.ax {
            Some(ax) if ax.window_id == window_id => Ok(ax.revision),
            _ => Err(EngineError::Msg("call get_window_state (with include_text) before using element_index".into())),
        }
    }
}

#[derive(Debug)]
pub enum EngineOutcome {
    Ok(Value),
    Err(String),
    Approval(ApprovalRequest),
}

enum EngineError {
    Msg(String),
    Approval(ApprovalRequest),
    Close,
}

pub const ESC_MSG: &str = "Computer Use was stopped by the user with the physical Escape key. Stop your work, do not call further Computer Use tools in this turn, and send a final message noting that the user stopped Computer Use.";

fn window_to_json(w: WindowInfo) -> Value {
    let mut m = json!({"app": w.app, "id": w.id});
    if let Some(t) = w.title {
        m["title"] = json!(t);
    }
    m
}
fn app_to_json(a: AppInfo) -> Value {
    let mut m = json!({"id": a.id, "windows": a.windows.into_iter().map(window_to_json).collect::<Vec<_>>()});
    if let Some(v) = a.display_name { m["displayName"] = json!(v); }
    if let Some(v) = a.last_used_date { m["lastUsedDate"] = json!(v); }
    if let Some(v) = a.use_count { m["useCount"] = json!(v); }
    if let Some(v) = a.is_running { m["isRunning"] = json!(v); }
    m
}

/// Shared interrupt latch accessible from the hook thread.
pub type SharedEngine = Arc<Mutex<Engine>>;

/// Linux / test stub backend.
pub struct NullBackend {
    pub windows: Vec<WindowInfo>,
}
impl Default for NullBackend {
    fn default() -> Self {
        Self {
            windows: vec![WindowInfo {
                id: 1,
                app: "stub".into(),
                title: Some("Stub Window".into()),
                process_path: Some("/usr/bin/stub".into()),
                pid: 1,
                bounds: Rect::new(0, 0, 800, 600),
            }],
        }
    }
}
impl Backend for NullBackend {
    fn list_windows(&mut self) -> Result<Vec<WindowInfo>, String> { Ok(self.windows.clone()) }
    fn list_apps(&mut self) -> Result<Vec<AppInfo>, String> {
        Ok(vec![AppInfo {
            id: "stub".into(), display_name: Some("Stub".into()),
            windows: self.windows.clone(), last_used_date: None, use_count: None,
            is_running: Some(true), process_path: Some("/usr/bin/stub".into()),
        }])
    }
    fn launch_app(&mut self, _app: &str) -> Result<(), String> { Err("launch_app is only available on Windows".into()) }
    fn get_window(&mut self, id: u64, _app: Option<&str>) -> Result<WindowInfo, String> {
        self.windows.iter().find(|w| w.id == id).cloned().ok_or_else(|| format!("window id {id} was not found"))
    }
    fn activate_window(&mut self, _id: u64) -> Result<(), String> { Ok(()) }
    fn window_bounds(&mut self, id: u64) -> Result<Rect, String> { Ok(self.get_window(id, None)?.bounds) }
    fn capture_window(&mut self, id: u64, max_edge: u32) -> Result<CaptureResult, String> {
        let w = self.get_window(id, None)?;
        let (iw, ih, scale) = geometry::fit_size(w.bounds.width() as u32, w.bounds.height() as u32, max_edge);
        // 1x1 blue PNG.
        let png = crate::image::encode_bgra_png(1, 1, &[255, 0, 0, 255])?;
        Ok(CaptureResult {
            png_data_url: crate::image::png_data_url(&png),
            geometry: CaptureGeometry { window: w.bounds, scale, image_w: iw.max(1), image_h: ih.max(1) },
            screenshot_id: format!("stub-{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()),
        })
    }
    fn accessibility_tree(&mut self, id: u64) -> Result<AccessSnapshot, String> {
        let _ = self.get_window(id, None)?;
        Ok(AccessSnapshot {
            tree: "0 Window: \"Stub Window\", App: stub\n  1 button \"OK\"".into(),
            focused_element: Some("1 button \"OK\"".into()),
            selected_text: None, selected_elements: None, document_text: None, revision: 1,
        })
    }
    fn element_bounds(&mut self, id: u64, _element_index: u64, _revision: u64) -> Result<Rect, String> {
        let b = self.window_bounds(id)?;
        Ok(Rect::new(b.left + 10, b.top + 10, b.left + 60, b.top + 40))
    }
    fn set_value(&mut self, _id: u64, _i: u64, _r: u64, _v: &str) -> Result<(), String> { Ok(()) }
    fn scroll_element(&mut self, _id: u64, _i: u64, _r: u64, _d: protocol::Direction, _p: f64) -> Result<(), String> { Ok(()) }
    fn secondary_action(&mut self, _id: u64, _i: u64, _r: u64, _a: protocol::SecondaryAction) -> Result<(), String> { Ok(()) }
    fn click_at(&mut self, _x: i32, _y: i32, _b: MouseButton, _c: u32) -> Result<(), String> { Ok(()) }
    fn drag(&mut self, _a: (i32, i32), _b: (i32, i32)) -> Result<(), String> { Ok(()) }
    fn scroll_at(&mut self, _x: i32, _y: i32, _dx: i32, _dy: i32) -> Result<(), String> { Ok(()) }
    fn type_text(&mut self, _t: &str) -> Result<(), String> { Ok(()) }
    fn press_key(&mut self, _c: &keys::Chord) -> Result<(), String> { Ok(()) }
    fn start_audio(&mut self, _ms: u64) -> Result<(), String> {
        Err("audio recording is not enabled (set SKY_ENABLE_AUDIO=1 on a real Windows host)".into())
    }
    fn stop_audio(&mut self) -> Result<AudioResult, String> {
        Err("computer audio recording is not active".into())
    }
    fn diagnostic(&mut self, _id: Option<u64>) -> Result<Value, String> {
        Ok(json!({
            "cacheDiagnostics": {
                "accessibilitySnapshotCount": 0,
                "captureCachedSessionCount": 0,
                "lastCaptureInvalidationReason": Value::Null,
                "lastCapturePath": "stub",
                "lastCapturePathDetail": "NullBackend (non-Windows or WGC init failed)"
            }
        }))
    }
    fn ask_approval(&mut self, _s: &str, _d: &str, _r: RiskLevel, _p: bool) -> Result<PersistScope, String> {
        Err("no interactive approval UI".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn eng() -> Engine {
        // Force a temp home so tests don't touch the real one.
        let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let home = std::env::temp_dir().join(format!("ocu-eng-{n}"));
        std::env::set_var("OCU_REPLICA_HOME", &home);
        let mut e = Engine::new(Box::new(NullBackend::default()), false);
        // Pre-approve stub so most calls skip the dialog.
        e.policy.store.grant("/usr/bin/stub", PersistScope::Always);
        e.policy.store.grant("stub", PersistScope::Always);
        e
    }

    #[test]
    fn list_and_state() {
        let mut e = eng();
        let out = e.handle("list_windows", &json!({}), &Map::new());
        match out {
            EngineOutcome::Ok(v) => assert!(v.as_array().unwrap().len() >= 1),
            other => panic!("{other:?}"),
        }
        let out = e.handle("get_window_state", &json!({"window":{"app":"stub","id":1},"include_text":true}), &Map::new());
        match out {
            EngineOutcome::Ok(v) => {
                assert!(v["screenshots"].as_array().unwrap().len() >= 1);
                assert!(v["accessibility"]["tree"].as_str().unwrap().contains("Stub"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn esc_blocks_turn() {
        let mut e = eng();
        let mut meta = Map::new();
        meta.insert("x-codex-turn-metadata".into(), json!({"session_id":"s","turn_id":"t"}));
        e.on_physical_esc();
        match e.handle("list_apps", &json!({}), &meta) {
            EngineOutcome::Err(m) => assert!(m.contains("Escape")),
            other => panic!("{other:?}"),
        }
        // end_turn clears it for MCP mode.
        let _ = e.handle("end_turn", &json!({}), &meta);
        // Marker file still exists, so same turn stays blocked.
        match e.handle("list_apps", &json!({}), &meta) {
            EngineOutcome::Err(m) => assert!(m.contains("Escape")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bounds_change_rejects_click() {
        let mut e = eng();
        let _ = e.handle("get_window_state", &json!({"window":{"app":"stub","id":1}}), &Map::new());
        // Simulate a stale capture by replacing geometry with a mismatched rect.
        let sid = e.shots.keys().next().unwrap().clone();
        if let Some(shot) = e.shots.get_mut(&sid) {
            shot.geometry.window = Rect::new(0, 0, 10, 10);
        }
        match e.handle("click", &json!({"window":{"app":"stub","id":1},"x":1,"y":1,"screenshotId": sid}), &Map::new()) {
            EngineOutcome::Err(m) => assert!(m.contains("bounds changed")),
            other => panic!("{other:?}"),
        }
    }
}

//! Wire protocol shared by both front-ends.
//!
//! Native mode ("--stdio-native"): one JSON object per line.
//!   request : {"id": <int>, "method": <string>, "params": {...}, "meta": {...}?}
//!   success : {"id": <int>, "ok": true,  "result": <value>}
//!   failure : {"id": <int>, "ok": false, "error": <string>}
//!   approval: {"id": <int>, "ok": false, "approvalRequest": {app, displayName, riskLevel, allowPersistentApproval}}
//! Field names were recovered from the serde field table of the original helper
//! and the JS transport that talks to it (see README_zh.md for the verified/inferred table).

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// The 20 methods, in the order the original's dispatcher lists them.
pub const METHODS: [&str; 20] = [
    "list_apps",
    "launch_app",
    "list_windows",
    "get_window",
    "activate_window",
    "get_window_state",
    "click",
    "click_element",
    "drag",
    "scroll",
    "scroll_element",
    "type_text",
    "press_key",
    "set_value",
    "perform_secondary_action",
    "start_audio_recording",
    "stop_audio_recording",
    "diagnostic_state",
    "end_turn",
    "close",
];

/// Extra tools beyond the original 20 (background CDP + single-monitor scope).
pub const EXTRA_METHODS: [&str; 11] = [
    "browser_list_tabs",
    "browser_screenshot",
    "browser_get_state",
    "browser_click",
    "browser_type",
    "browser_press_key",
    "browser_scroll",
    "browser_navigate",
    "list_displays",
    "scope_monitor",
    "move_window_to_scope",
];

pub const META_BUDGET_MS: &str = "x-oai-cua-request-budget-ms";
pub const META_TURN: &str = "x-codex-turn-metadata";
pub const META_APPROVED_APP: &str = "x-oai-cua-approved-app";

#[derive(Debug, Clone, PartialEq)]
pub struct NativeRequest {
    pub id: Value,
    pub method: String,
    pub params: Value,
    pub meta: Map<String, Value>,
}

/// Parse one native line. Returns Err(message) on malformed input; `id` is
/// echoed back when it could be recovered so the caller can still reply.
pub fn parse_native_line(line: &str) -> Result<NativeRequest, (Option<Value>, String)> {
    let v: Value = serde_json::from_str(line).map_err(|e| (None, format!("malformed request JSON: {e}")))?;
    let obj = v.as_object().ok_or((None, "request must be a JSON object".to_string()))?;
    let id = obj.get("id").cloned();
    let id_ok = matches!(&id, Some(Value::Number(_)));
    if !id_ok {
        return Err((id, "request id must be a number".into()));
    }
    let id = id.unwrap();
    let method = match obj.get("method").and_then(Value::as_str) {
        Some(m) if !m.trim().is_empty() => m.to_string(),
        _ => return Err((Some(id), "request method must be a non-empty string".into())),
    };
    let params = match obj.get("params") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(p @ Value::Object(_)) => p.clone(),
        Some(_) => return Err((Some(id), "request params must be an object".into())),
    };
    let meta = match obj.get("meta") {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    Ok(NativeRequest { id, method, params, meta })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub app: String,
    pub display_name: String,
    pub risk_level: String,
    pub allow_persistent_approval: bool,
}

pub fn native_ok(id: &Value, result: Value) -> Value {
    json!({"id": id, "ok": true, "result": result})
}
pub fn native_err(id: &Value, msg: &str) -> Value {
    json!({"id": id, "ok": false, "error": msg})
}
pub fn native_approval(id: &Value, req: &ApprovalRequest) -> Value {
    json!({"id": id, "ok": false, "approvalRequest": req})
}

/// Turn scope recovered from request metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TurnKey {
    pub session: String,
    pub turn: String,
}

fn nonempty(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

/// Flatten meta the same way the JS caller does: `x-codex-turn-metadata` may be
/// an object or a JSON string; its keys are merged on top of the outer meta.
pub fn flatten_meta(meta: &Map<String, Value>) -> Map<String, Value> {
    let mut out = meta.clone();
    if let Some(inner) = out.remove(META_TURN) {
        let inner = match inner {
            Value::String(s) => serde_json::from_str::<Value>(&s).unwrap_or(Value::Null),
            other => other,
        };
        if let Value::Object(m) = inner {
            for (k, v) in m {
                out.insert(k, v);
            }
        }
    }
    out
}

/// Session aliases and turn aliases accepted (verified from the original's string table).
pub fn turn_key(meta: &Map<String, Value>) -> Option<TurnKey> {
    let m = flatten_meta(meta);
    let session = ["session_id", "sessionId", "thread-id", "conversation_id", "conversationId"]
        .iter()
        .find_map(|k| nonempty(m.get(*k)))?;
    let turn = ["turn_id", "turn-id", "turnId"].iter().find_map(|k| nonempty(m.get(*k)))?;
    Some(TurnKey { session, turn })
}

pub fn approved_app(meta: &Map<String, Value>) -> Option<String> {
    nonempty(flatten_meta(meta).get(META_APPROVED_APP))
}

pub fn budget_ms(meta: &Map<String, Value>) -> Option<u64> {
    let m = flatten_meta(meta);
    match m.get(META_BUDGET_MS)? {
        Value::Number(n) => n.as_f64().filter(|f| f.is_finite() && *f >= 0.0).map(|f| f as u64),
        Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    }
}

/// Path component sanitizer, same rule as the JS caller: anything outside
/// [A-Za-z0-9._-] becomes '_'.
pub fn sanitize_component(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' { c } else { '_' })
        .collect()
}

// ---------------------------------------------------------------- params

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowRef {
    pub app: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

fn obj<'a>(p: &'a Value) -> Result<&'a Map<String, Value>, String> {
    p.as_object().ok_or_else(|| "params must be an object".to_string())
}

pub fn int_ge0(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                Some(u)
            } else {
                n.as_f64().filter(|f| f.fract() == 0.0 && *f >= 0.0).map(|f| f as u64)
            }
        }
        Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    }
}

pub fn parse_window(p: &Value) -> Result<WindowRef, String> {
    let w = obj(p)?.get("window").ok_or("window is required")?;
    let w = w.as_object().ok_or("window must be an object")?;
    let app = nonempty(w.get("app")).ok_or("window.app must be a non-empty string")?;
    let id = w.get("id").and_then(int_ge0).ok_or("window.id must be an integer >= 0")?;
    Ok(WindowRef { app, id, title: nonempty(w.get("title")) })
}

fn finite(m: &Map<String, Value>, k: &str) -> Result<f64, String> {
    m.get(k)
        .and_then(Value::as_f64)
        .filter(|f| f.is_finite())
        .ok_or_else(|| format!("{k} must be a finite number"))
}
fn opt_finite(m: &Map<String, Value>, k: &str) -> Result<Option<f64>, String> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_f64().filter(|f| f.is_finite()).map(Some).ok_or_else(|| format!("{k} must be a finite number")),
    }
}
fn opt_str(m: &Map<String, Value>, k: &str) -> Option<String> {
    m.get(k).and_then(Value::as_str).map(String::from)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}
pub fn parse_button(v: Option<&Value>) -> Result<MouseButton, String> {
    match v.and_then(Value::as_str).map(|s| s.trim().to_ascii_lowercase()) {
        None => Ok(MouseButton::Left),
        Some(s) => match s.as_str() {
            "left" | "l" => Ok(MouseButton::Left),
            "right" | "r" => Ok(MouseButton::Right),
            "middle" | "m" => Ok(MouseButton::Middle),
            other => Err(format!("mouse_button '{other}' is not one of left/right/middle")),
        },
    }
}
fn click_count(m: &Map<String, Value>) -> Result<u32, String> {
    match m.get("click_count") {
        None | Some(Value::Null) => Ok(1),
        Some(v) => {
            let f = v.as_f64().filter(|f| f.is_finite()).ok_or("click_count must be a finite number")?;
            let n = f.round();
            if n < 1.0 {
                return Err("click_count has to be at least 1".into());
            }
            Ok(n.min(10.0) as u32)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    ListApps,
    LaunchApp { app: String },
    ListWindows,
    GetWindow { id: u64, app: Option<String> },
    ActivateWindow { window: WindowRef },
    GetWindowState { window: WindowRef, include_screenshot: bool, include_text: bool },
    Click { window: WindowRef, x: f64, y: f64, screenshot_id: Option<String>, click_count: u32, button: MouseButton },
    ClickElement { window: WindowRef, element_index: u64, click_count: u32, button: MouseButton },
    Drag { window: WindowRef, from: (f64, f64), to: (f64, f64), screenshot_id: Option<String> },
    Scroll { window: WindowRef, x: f64, y: f64, screenshot_id: Option<String>, scroll_x: f64, scroll_y: f64 },
    ScrollElement { window: WindowRef, element_index: u64, direction: Direction, pages: f64 },
    TypeText { window: WindowRef, text: String, element_index: Option<u64> },
    PressKey { window: WindowRef, key: String },
    SetValue { window: WindowRef, element_index: u64, value: String },
    PerformSecondaryAction { window: WindowRef, element_index: u64, action: SecondaryAction },
    StartAudioRecording { max_duration_ms: u64 },
    StopAudioRecording,
    DiagnosticState { window: Option<WindowRef> },
    EndTurn,
    Close,
    // --- extras (background CDP + monitor scope) ---
    BrowserListTabs,
    BrowserScreenshot { tab_id: String },
    BrowserGetState { tab_id: String },
    BrowserClick { tab_id: String, x: f64, y: f64, button: String, click_count: u32 },
    BrowserType { tab_id: String, text: String },
    BrowserPressKey { tab_id: String, key: String },
    BrowserScroll { tab_id: String, x: f64, y: f64, scroll_x: f64, scroll_y: f64 },
    BrowserNavigate { tab_id: String, url: String },
    ListDisplays,
    ScopeMonitor { selector: Option<String> },
    MoveWindowToScope { window: WindowRef },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}
pub fn parse_direction(s: &str) -> Result<Direction, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "up" | "u" => Ok(Direction::Up),
        "down" | "d" => Ok(Direction::Down),
        "left" | "l" => Ok(Direction::Left),
        "right" | "r" => Ok(Direction::Right),
        other => Err(format!("direction '{other}' must be up, down, left or right")),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondaryAction {
    Raise,
    Scroll(Direction),
    Expand,
    Collapse,
}
impl SecondaryAction {
    pub fn label(&self) -> &'static str {
        match self {
            SecondaryAction::Raise => "Raise",
            SecondaryAction::Scroll(Direction::Up) => "Scroll Up",
            SecondaryAction::Scroll(Direction::Down) => "Scroll Down",
            SecondaryAction::Scroll(Direction::Left) => "Scroll Left",
            SecondaryAction::Scroll(Direction::Right) => "Scroll Right",
            SecondaryAction::Expand => "Expand",
            SecondaryAction::Collapse => "Collapse",
        }
    }
}
pub fn parse_secondary_action(s: &str) -> Result<SecondaryAction, String> {
    let norm: String = s.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase();
    Ok(match norm.as_str() {
        "raise" => SecondaryAction::Raise,
        "scroll up" => SecondaryAction::Scroll(Direction::Up),
        "scroll down" => SecondaryAction::Scroll(Direction::Down),
        "scroll left" => SecondaryAction::Scroll(Direction::Left),
        "scroll right" => SecondaryAction::Scroll(Direction::Right),
        "expand" => SecondaryAction::Expand,
        "collapse" => SecondaryAction::Collapse,
        _ => {
            return Err(format!(
                "secondary action '{s}' is not supported (use Raise, Scroll Up/Down/Left/Right, Expand or Collapse)"
            ))
        }
    })
}

pub const AUDIO_DEFAULT_MS: u64 = 60_000;
pub const AUDIO_MIN_MS: u64 = 1;
pub const AUDIO_MAX_MS: u64 = 300_000;

fn element_index(m: &Map<String, Value>) -> Result<u64, String> {
    m.get("element_index").and_then(int_ge0).ok_or_else(|| "element_index must be an integer >= 0".to_string())
}

pub fn parse_call(method: &str, params: &Value) -> Result<Call, String> {
    let empty = Map::new();
    let m = params.as_object().unwrap_or(&empty);
    Ok(match method {
        "list_apps" => Call::ListApps,
        "list_windows" => Call::ListWindows,
        "launch_app" => Call::LaunchApp { app: nonempty(m.get("app")).ok_or("app must be a non-empty string")? },
        "get_window" => Call::GetWindow {
            id: m.get("id").and_then(int_ge0).ok_or("id must be an integer >= 0")?,
            app: nonempty(m.get("app")),
        },
        "activate_window" => Call::ActivateWindow { window: parse_window(params)? },
        "get_window_state" => {
            let include_screenshot = m.get("include_screenshot").and_then(Value::as_bool).unwrap_or(true);
            let include_text = m.get("include_text").and_then(Value::as_bool).unwrap_or(false);
            if !include_screenshot && !include_text {
                return Err("get_window_state needs include_screenshot and/or include_text set to true".into());
            }
            Call::GetWindowState { window: parse_window(params)?, include_screenshot, include_text }
        }
        "click" => {
            // The JS caller routes element clicks to click_element, but accept
            // element_index here too for MCP clients that only know `click`.
            let window = parse_window(params)?;
            if let Some(idx) = m.get("element_index").filter(|v| !v.is_null()) {
                let element_index = int_ge0(idx).ok_or("element_index must be an integer >= 0")?;
                return Ok(Call::ClickElement {
                    window,
                    element_index,
                    click_count: click_count(m)?,
                    button: parse_button(m.get("mouse_button"))?,
                });
            }
            let (x, y) = match (opt_finite(m, "x")?, opt_finite(m, "y")?) {
                (Some(x), Some(y)) => (x, y),
                _ => return Err("click needs element_index or both x and y".into()),
            };
            Call::Click {
                window,
                x,
                y,
                screenshot_id: opt_str(m, "screenshotId"),
                click_count: click_count(m)?,
                button: parse_button(m.get("mouse_button"))?,
            }
        }
        "click_element" => Call::ClickElement {
            window: parse_window(params)?,
            element_index: element_index(m)?,
            click_count: click_count(m)?,
            button: parse_button(m.get("mouse_button"))?,
        },
        "drag" => Call::Drag {
            window: parse_window(params)?,
            from: (finite(m, "from_x")?, finite(m, "from_y")?),
            to: (finite(m, "to_x")?, finite(m, "to_y")?),
            screenshot_id: opt_str(m, "screenshotId"),
        },
        "scroll" => Call::Scroll {
            window: parse_window(params)?,
            x: finite(m, "x")?,
            y: finite(m, "y")?,
            screenshot_id: opt_str(m, "screenshotId"),
            scroll_x: opt_finite(m, "scrollX")?.unwrap_or(0.0),
            scroll_y: opt_finite(m, "scrollY")?.unwrap_or(0.0),
        },
        "scroll_element" => {
            let pages = opt_finite(m, "pages")?.unwrap_or(1.0);
            if pages <= 0.0 {
                return Err("pages has to be a finite number greater than 0".into());
            }
            Call::ScrollElement {
                window: parse_window(params)?,
                element_index: element_index(m)?,
                direction: parse_direction(m.get("direction").and_then(Value::as_str).ok_or("direction is required")?)?,
                pages,
            }
        }
        "type_text" => Call::TypeText {
            window: parse_window(params)?,
            text: m.get("text").and_then(Value::as_str).ok_or("text must be a string")?.to_string(),
            element_index: match m.get("element_index") {
                None => None,
                Some(v) => Some(int_ge0(v).ok_or("element_index must be a nonnegative integer")?),
            },
        },
        "press_key" => Call::PressKey {
            window: parse_window(params)?,
            key: nonempty(m.get("key")).ok_or("key must be a non-empty string")?,
        },
        "set_value" => Call::SetValue {
            window: parse_window(params)?,
            element_index: element_index(m)?,
            value: m.get("value").and_then(Value::as_str).ok_or("value must be a string")?.to_string(),
        },
        "perform_secondary_action" => Call::PerformSecondaryAction {
            window: parse_window(params)?,
            element_index: element_index(m)?,
            action: parse_secondary_action(m.get("action").and_then(Value::as_str).ok_or("action is required")?)?,
        },
        "start_audio_recording" => {
            let ms = match m.get("max_duration_ms") {
                None | Some(Value::Null) => AUDIO_DEFAULT_MS,
                Some(v) => {
                    let f = v.as_f64().filter(|f| f.is_finite()).ok_or("max_duration_ms must be a number")?;
                    let ms = f.round();
                    if ms < AUDIO_MIN_MS as f64 || ms > AUDIO_MAX_MS as f64 {
                        return Err(format!("max_duration_ms has to be between {AUDIO_MIN_MS} and {AUDIO_MAX_MS}"));
                    }
                    ms as u64
                }
            };
            Call::StartAudioRecording { max_duration_ms: ms }
        }
        "stop_audio_recording" => Call::StopAudioRecording,
        "diagnostic_state" => Call::DiagnosticState {
            window: if m.contains_key("window") { Some(parse_window(params)?) } else { None },
        },
        "end_turn" => Call::EndTurn,
        "close" => Call::Close,
        "browser_list_tabs" => Call::BrowserListTabs,
        "browser_screenshot" => Call::BrowserScreenshot {
            tab_id: nonempty(m.get("tab_id").or_else(|| m.get("tabId"))).ok_or("tab_id must be a non-empty string")?,
        },
        "browser_get_state" => Call::BrowserGetState {
            tab_id: nonempty(m.get("tab_id").or_else(|| m.get("tabId"))).ok_or("tab_id must be a non-empty string")?,
        },
        "browser_click" => Call::BrowserClick {
            tab_id: nonempty(m.get("tab_id").or_else(|| m.get("tabId"))).ok_or("tab_id must be a non-empty string")?,
            x: finite(m, "x")?,
            y: finite(m, "y")?,
            button: m.get("mouse_button").or_else(|| m.get("button")).and_then(Value::as_str).unwrap_or("left").into(),
            click_count: click_count(m)?,
        },
        "browser_type" => Call::BrowserType {
            tab_id: nonempty(m.get("tab_id").or_else(|| m.get("tabId"))).ok_or("tab_id must be a non-empty string")?,
            text: m.get("text").and_then(Value::as_str).ok_or("text must be a string")?.into(),
        },
        "browser_press_key" => Call::BrowserPressKey {
            tab_id: nonempty(m.get("tab_id").or_else(|| m.get("tabId"))).ok_or("tab_id must be a non-empty string")?,
            key: nonempty(m.get("key")).ok_or("key must be a non-empty string")?,
        },
        "browser_scroll" => Call::BrowserScroll {
            tab_id: nonempty(m.get("tab_id").or_else(|| m.get("tabId"))).ok_or("tab_id must be a non-empty string")?,
            x: finite(m, "x")?,
            y: finite(m, "y")?,
            scroll_x: opt_finite(m, "scrollX")?.unwrap_or(0.0),
            scroll_y: opt_finite(m, "scrollY")?.unwrap_or(0.0),
        },
        "browser_navigate" => Call::BrowserNavigate {
            tab_id: nonempty(m.get("tab_id").or_else(|| m.get("tabId"))).ok_or("tab_id must be a non-empty string")?,
            url: nonempty(m.get("url")).ok_or("url must be a non-empty string")?,
        },
        "list_displays" => Call::ListDisplays,
        "scope_monitor" => Call::ScopeMonitor {
            selector: nonempty(m.get("monitor").or_else(|| m.get("selector"))),
        },
        "move_window_to_scope" => Call::MoveWindowToScope { window: parse_window(params)? },
        other => return Err(format!("unknown method: {other}")),
    })
}

impl Call {
    pub fn window(&self) -> Option<&WindowRef> {
        match self {
            Call::ActivateWindow { window }
            | Call::GetWindowState { window, .. }
            | Call::Click { window, .. }
            | Call::ClickElement { window, .. }
            | Call::Drag { window, .. }
            | Call::Scroll { window, .. }
            | Call::ScrollElement { window, .. }
            | Call::TypeText { window, .. }
            | Call::PressKey { window, .. }
            | Call::SetValue { window, .. }
            | Call::PerformSecondaryAction { window, .. }
            | Call::MoveWindowToScope { window } => Some(window),
            _ => None,
        }
    }
    /// Methods that inject real input into the target.
    pub fn is_input(&self) -> bool {
        matches!(
            self,
            Call::Click { .. }
                | Call::ClickElement { .. }
                | Call::Drag { .. }
                | Call::Scroll { .. }
                | Call::TypeText { .. }
                | Call::PressKey { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_native_request() {
        let r = parse_native_line(r#"{"id":3,"method":"click","params":{"window":{"app":"a","id":5},"x":1,"y":2},"meta":{"x-oai-cua-request-budget-ms":9000}}"#).unwrap();
        assert_eq!(r.id, json!(3));
        assert_eq!(r.method, "click");
        assert_eq!(budget_ms(&r.meta), Some(9000));
    }

    #[test]
    fn rejects_bad_requests() {
        assert!(parse_native_line("nope").is_err());
        let e = parse_native_line(r#"{"id":"x","method":"a"}"#).unwrap_err();
        assert_eq!(e.0, Some(json!("x")));
        let e = parse_native_line(r#"{"id":1,"method":""}"#).unwrap_err();
        assert_eq!(e.0, Some(json!(1)));
        assert!(parse_native_line(r#"{"id":1,"method":"m","params":[]}"#).is_err());
    }

    #[test]
    fn missing_params_default_to_object() {
        let r = parse_native_line(r#"{"id":1,"method":"list_apps"}"#).unwrap();
        assert_eq!(r.params, json!({}));
    }

    #[test]
    fn turn_metadata_flattening() {
        let meta = json!({"x-codex-turn-metadata": "{\"session_id\":\"s 1\",\"turn_id\":\"t/2\"}", "x-oai-cua-approved-app": " notepad "});
        let m = meta.as_object().unwrap();
        let k = turn_key(m).unwrap();
        assert_eq!(k.session, "s 1");
        assert_eq!(k.turn, "t/2");
        assert_eq!(approved_app(m).as_deref(), Some("notepad"));
        let meta2 = json!({"x-codex-turn-metadata": {"conversationId":"c","turnId":"t"}});
        assert_eq!(turn_key(meta2.as_object().unwrap()).unwrap().session, "c");
        assert!(turn_key(json!({"session_id":"s"}).as_object().unwrap()).is_none());
    }

    #[test]
    fn sanitizes_components() {
        assert_eq!(sanitize_component("a b/c:d.e-f_g"), "a_b_c_d.e-f_g");
    }

    #[test]
    fn responses_have_expected_shape() {
        let id = json!(7);
        assert_eq!(native_ok(&id, json!(null)), json!({"id":7,"ok":true,"result":null}));
        assert_eq!(native_err(&id, "x")["ok"], json!(false));
        let a = ApprovalRequest { app: "a".into(), display_name: "A".into(), risk_level: "low".into(), allow_persistent_approval: true };
        let v = native_approval(&id, &a);
        assert_eq!(v["approvalRequest"]["displayName"], json!("A"));
        assert_eq!(v["approvalRequest"]["allowPersistentApproval"], json!(true));
    }

    #[test]
    fn click_routes() {
        let c = parse_call("click", &json!({"window":{"app":"a","id":1},"x":10.4,"y":5})).unwrap();
        assert!(matches!(c, Call::Click { click_count: 1, button: MouseButton::Left, .. }));
        let c = parse_call("click", &json!({"window":{"app":"a","id":"1"},"element_index":3,"mouse_button":"r"})).unwrap();
        assert!(matches!(c, Call::ClickElement { element_index: 3, button: MouseButton::Right, .. }));
        assert!(parse_call("click", &json!({"window":{"app":"a","id":1}})).is_err());
        assert!(parse_call("click", &json!({"window":{"app":"","id":1},"x":1,"y":1})).is_err());
        assert!(parse_call("click", &json!({"window":{"app":"a","id":-1},"x":1,"y":1})).is_err());
        assert!(parse_call("click", &json!({"window":{"app":"a","id":1},"x":1,"y":1,"click_count":0})).is_err());
        assert!(parse_call("click", &json!({"window":{"app":"a","id":1},"x":1,"y":1,"mouse_button":"x"})).is_err());
    }

    #[test]
    fn get_window_state_defaults() {
        let c = parse_call("get_window_state", &json!({"window":{"app":"a","id":1}})).unwrap();
        assert!(matches!(c, Call::GetWindowState { include_screenshot: true, include_text: false, .. }));
        assert!(parse_call("get_window_state", &json!({"window":{"app":"a","id":1},"include_screenshot":false})).is_err());
    }

    #[test]
    fn type_text_explicit_target_is_optional_and_validated() {
        let valid = json!({"window":{"app":"test.exe","id":1},"text":"hello","element_index":3});
        assert!(matches!(parse_call("type_text", &valid).unwrap(), Call::TypeText { element_index: Some(3), .. }));
        let legacy = json!({"window":{"app":"test.exe","id":1},"text":"hello"});
        assert!(matches!(parse_call("type_text", &legacy).unwrap(), Call::TypeText { element_index: None, .. }));
        let native_string = json!({"window":{"app":"test.exe","id":1},"text":"hello","element_index":"3"});
        assert!(matches!(parse_call("type_text", &native_string).unwrap(), Call::TypeText { element_index: Some(3), .. }));
        for invalid in [json!(-1), json!(1.5), json!("invalid"), Value::Null] {
            let params = json!({"window":{"app":"test.exe","id":1},"text":"hello","element_index":invalid});
            assert!(parse_call("type_text", &params).is_err());
        }
    }

    #[test]
    fn scroll_element_and_secondary() {
        let c = parse_call("scroll_element", &json!({"window":{"app":"a","id":1},"element_index":2,"direction":"D","pages":2})).unwrap();
        assert!(matches!(c, Call::ScrollElement { direction: Direction::Down, .. }));
        assert!(parse_call("scroll_element", &json!({"window":{"app":"a","id":1},"element_index":2,"direction":"down","pages":0})).is_err());
        assert_eq!(parse_secondary_action("  scroll   LEFT ").unwrap(), SecondaryAction::Scroll(Direction::Left));
        assert!(parse_secondary_action("toggle").is_err());
    }

    #[test]
    fn audio_bounds() {
        assert!(matches!(parse_call("start_audio_recording", &json!({})).unwrap(), Call::StartAudioRecording { max_duration_ms: 60000 }));
        assert!(parse_call("start_audio_recording", &json!({"max_duration_ms": 300001})).is_err());
    }

    #[test]
    fn all_methods_parse_or_validate() {
        for m in METHODS {
            // must not be "unknown method"
            if let Err(e) = parse_call(m, &json!({})) {
                assert!(!e.starts_with("unknown method"), "{m}: {e}");
            }
        }
        assert!(parse_call("bogus", &json!({})).unwrap_err().starts_with("unknown method"));
    }
}

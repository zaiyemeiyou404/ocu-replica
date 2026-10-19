//! Input mode selection and idle-gating helpers (pure / unit-tested on Linux).

use serde_json::Value;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// Current behaviour: activate window + SendInput.
    Foreground,
    /// Never touch the real cursor or focus; fail if impossible.
    Background,
    /// Prefer background; fall back to foreground only when the user is idle.
    Auto,
}

impl InputMode {
    pub fn as_str(self) -> &'static str {
        match self {
            InputMode::Foreground => "foreground",
            InputMode::Background => "background",
            InputMode::Auto => "auto",
        }
    }
}

pub fn parse_mode_str(s: &str) -> Result<InputMode, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "foreground" | "fg" => Ok(InputMode::Foreground),
        "background" | "bg" => Ok(InputMode::Background),
        "auto" => Ok(InputMode::Auto),
        other => Err(format!("input_mode '{other}' must be foreground, background, or auto")),
    }
}

/// Default from `OCU_INPUT_MODE`, else `auto` (new default for background-capable builds).
pub fn default_mode_from_env() -> InputMode {
    std::env::var("OCU_INPUT_MODE")
        .ok()
        .and_then(|s| parse_mode_str(&s).ok())
        .unwrap_or(InputMode::Auto)
}

/// Per-call override from params.input_mode, else the engine default.
pub fn resolve_mode(params: &Value, engine_default: InputMode) -> Result<InputMode, String> {
    match params.get("input_mode").or_else(|| params.get("inputMode")) {
        None | Some(Value::Null) => Ok(engine_default),
        Some(Value::String(s)) => parse_mode_str(s),
        Some(_) => Err("input_mode must be a string".into()),
    }
}

pub fn idle_ms_from_env() -> u64 {
    std::env::var("OCU_IDLE_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3_000)
}

pub fn idle_timeout_ms_from_env() -> u64 {
    std::env::var("OCU_IDLE_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30_000)
}

pub fn postmessage_enabled() -> bool {
    matches!(std::env::var("OCU_ALLOW_POSTMESSAGE").ok().as_deref(), Some("1") | Some("true") | Some("yes"))
}

/// Pure idle check used by unit tests and the wait loop.
/// `now_ms` / `last_input_ms` are monotonic-ish milliseconds since an epoch.
pub fn is_idle(now_ms: u64, last_input_ms: u64, idle_ms: u64) -> bool {
    now_ms.saturating_sub(last_input_ms) >= idle_ms
}

/// Decide whether a SendInput fallback is allowed under `mode`.
/// Returns Ok(()) if we may inject, Err("user_busy") if auto timed out waiting.
pub fn decide_sendinput_allowed(
    mode: InputMode,
    now_ms: u64,
    last_input_ms: u64,
    idle_ms: u64,
    waited_ms: u64,
    timeout_ms: u64,
) -> Result<(), String> {
    match mode {
        InputMode::Background => Err("background mode cannot use SendInput".into()),
        InputMode::Foreground => Ok(()),
        InputMode::Auto => {
            if is_idle(now_ms, last_input_ms, idle_ms) {
                Ok(())
            } else if waited_ms >= timeout_ms {
                Err("user_busy".into())
            } else {
                Err("waiting_for_idle".into())
            }
        }
    }
}

/// Spin until idle or timeout. `poll` returns (now_ms, last_input_ms).
pub fn wait_until_idle<F>(idle_ms: u64, timeout_ms: u64, mut poll: F) -> Result<(), String>
where
    F: FnMut() -> (u64, u64),
{
    let start = Instant::now();
    loop {
        let (now_ms, last) = poll();
        let waited = start.elapsed().as_millis() as u64;
        match decide_sendinput_allowed(InputMode::Auto, now_ms, last, idle_ms, waited, timeout_ms) {
            Ok(()) => return Ok(()),
            Err(e) if e == "waiting_for_idle" => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionBackend {
    Uia,
    PostMessage,
    Cdp,
    SendInput,
    WgcWindow,
    PrintWindow,
    WgcMonitor,
    BitBlt,
}

impl ActionBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            ActionBackend::Uia => "uia",
            ActionBackend::PostMessage => "postmessage",
            ActionBackend::Cdp => "cdp",
            ActionBackend::SendInput => "sendinput",
            ActionBackend::WgcWindow => "wgc_window",
            ActionBackend::PrintWindow => "printwindow",
            ActionBackend::WgcMonitor => "wgc",
            ActionBackend::BitBlt => "bitblt",
        }
    }
}

pub fn wrap_backend(backend: ActionBackend) -> Value {
    serde_json::json!({"backend": backend.as_str()})
}

pub fn merge_backend(mut result: Value, backend: ActionBackend) -> Value {
    match result {
        Value::Null => wrap_backend(backend),
        Value::Object(ref mut m) => {
            m.insert("backend".into(), Value::String(backend.as_str().into()));
            result
        }
        other => serde_json::json!({"result": other, "backend": backend.as_str()}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_default() {
        assert_eq!(parse_mode_str("AUTO").unwrap(), InputMode::Auto);
        assert_eq!(parse_mode_str("bg").unwrap(), InputMode::Background);
        assert!(parse_mode_str("nope").is_err());
        let params = serde_json::json!({"input_mode": "background"});
        assert_eq!(resolve_mode(&params, InputMode::Auto).unwrap(), InputMode::Background);
        assert_eq!(resolve_mode(&serde_json::json!({}), InputMode::Foreground).unwrap(), InputMode::Foreground);
    }

    #[test]
    fn idle_gating() {
        assert!(is_idle(10_000, 6_000, 3_000));
        assert!(!is_idle(10_000, 8_000, 3_000));
        assert!(decide_sendinput_allowed(InputMode::Foreground, 0, 0, 3000, 0, 30000).is_ok());
        assert!(decide_sendinput_allowed(InputMode::Background, 0, 0, 3000, 0, 30000).is_err());
        assert_eq!(
            decide_sendinput_allowed(InputMode::Auto, 5000, 4000, 3000, 1000, 30000).unwrap_err(),
            "waiting_for_idle"
        );
        assert_eq!(
            decide_sendinput_allowed(InputMode::Auto, 5000, 4000, 3000, 30_000, 30_000).unwrap_err(),
            "user_busy"
        );
        assert!(decide_sendinput_allowed(InputMode::Auto, 10_000, 5_000, 3000, 0, 30000).is_ok());
    }

    #[test]
    fn wait_until_idle_succeeds() {
        let mut n = 0u64;
        wait_until_idle(100, 2000, || {
            n += 1;
            // Become idle after a few polls.
            if n < 3 { (n * 10, n * 10) } else { (10_000, 0) }
        })
        .unwrap();
    }

    #[test]
    fn merge_backend_shapes() {
        assert_eq!(wrap_backend(ActionBackend::Uia)["backend"], "uia");
        let v = merge_backend(serde_json::json!({"filepath":"x"}), ActionBackend::Cdp);
        assert_eq!(v["backend"], "cdp");
        assert_eq!(v["filepath"], "x");
    }
}

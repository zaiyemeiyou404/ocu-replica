//! Per-app approval + optional allow/deny rules.
//!
//! Verified behaviour of the original:
//! - First use of an app returns an `approvalRequest` (native protocol) /
//!   elicitation (MCP) with `riskLevel` and `allowPersistentApproval`.
//! - Persist choices are `session` or `always` (when allowed).
//! - Computer-audio can only be granted for the current session.
//! - Enterprise allow/deny rules match AUMID or absolute exe path and may
//!   require a trusted Authenticode signer (WinVerifyTrust).
//!
//! We intentionally do NOT call any remote site-status backend. A pluggable
//! `UrlPolicy` trait is provided and left disabled by default.

use crate::blocklist;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PersistScope {
    Session,
    Always,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ApprovalStore {
    /// Apps granted for the rest of this process.
    #[serde(default)]
    pub session: HashSet<String>,
    /// Apps granted forever (persisted to disk).
    #[serde(default)]
    pub always: HashSet<String>,
}

impl ApprovalStore {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, path)
    }
    pub fn is_approved(&self, subject: &str) -> bool {
        self.session.contains(subject) || self.always.contains(subject)
    }
    pub fn grant(&mut self, subject: &str, scope: PersistScope) {
        match scope {
            PersistScope::Session => {
                self.session.insert(subject.to_string());
            }
            PersistScope::Always => {
                self.always.insert(subject.to_string());
                self.session.insert(subject.to_string());
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AppRules {
    #[serde(default)]
    pub aumids: RuleSet,
    #[serde(default)]
    pub exes: RuleSet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RuleSet {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    /// Needs a first-use approval dialog.
    NeedsApproval {
        subject: String,
        display_name: String,
        risk: RiskLevel,
        allow_persistent: bool,
    },
    Denied { reason: String },
}

#[derive(Debug, Clone)]
pub struct PolicyEngine {
    pub store: ApprovalStore,
    pub store_path: PathBuf,
    pub rules: AppRules,
    /// When true, `always` persist is offered (except for audio).
    pub allow_persistent_approval: bool,
    /// Default access when no rule matches: ask (true) or allow silently (false).
    pub require_first_use_approval: bool,
    /// Tokens from `OCU_PREAPPROVE` / `OCU_PREAPPROVE_FILE`. Empty unless the
    /// operator opted in. Matching subjects skip the MessageBox but are not
    /// written to the persistent approval store.
    pub preapprove: Vec<String>,
}

impl PolicyEngine {
    pub fn new(home: &Path) -> Self {
        let store_path = home.join("cache").join("computer-use").join("approvals.json");
        Self {
            store: ApprovalStore::load(&store_path),
            store_path,
            rules: AppRules::default(),
            allow_persistent_approval: true,
            require_first_use_approval: true,
            preapprove: preapprove_tokens_from_env(),
        }
    }

    /// Evaluate an app before it is targeted. `subject` is the stable key used
    /// for approvals (AUMID or absolute exe path, lowercased).
    pub fn evaluate(
        &self,
        subject: &str,
        display_name: &str,
        process_path: Option<&str>,
        is_audio: bool,
    ) -> Decision {
        // 1. Hard-coded product policy.
        if let Some(cat) = blocklist::blocked(process_path.unwrap_or(subject), Some(display_name)) {
            return Decision::Denied {
                reason: format!(
                    "product policy blocks this app ({subject}, category={cat:?})"
                ),
            };
        }
        // 2. Explicit deny rules.
        if self.rules.matches_deny(subject, process_path) {
            return Decision::Denied {
                reason: format!("policy denies this app ({subject})"),
            };
        }
        // 3. Explicit allow rules short-circuit the first-use dialog.
        if self.rules.matches_allow(subject, process_path) {
            return Decision::Allowed;
        }
        // 4. Prior approval.
        if self.store.is_approved(subject) {
            return Decision::Allowed;
        }
        // 4b. Explicit unattended pre-approval (OCU_PREAPPROVE). Not persisted.
        if preapprove_matches(&self.preapprove, subject, process_path) {
            return Decision::Allowed;
        }
        // 5. First-use approval (or silent allow if disabled).
        if !self.require_first_use_approval {
            return Decision::Allowed;
        }
        Decision::NeedsApproval {
            subject: subject.to_string(),
            display_name: display_name.to_string(),
            risk: if is_audio { RiskLevel::High } else { RiskLevel::Low },
            allow_persistent: self.allow_persistent_approval && !is_audio,
        }
    }

    pub fn grant(&mut self, subject: &str, scope: PersistScope) {
        // Audio is forced to session-only regardless of the requested scope.
        let scope = if subject == "computer-audio" { PersistScope::Session } else { scope };
        self.store.grant(subject, scope);
        if matches!(scope, PersistScope::Always) {
            let _ = self.store.save(&self.store_path);
        }
    }
}

impl AppRules {
    fn matches_deny(&self, subject: &str, path: Option<&str>) -> bool {
        let s = subject.to_ascii_lowercase();
        if self.aumids.deny.iter().any(|d| d.eq_ignore_ascii_case(&s)) {
            return true;
        }
        if let Some(p) = path {
            let p = p.to_ascii_lowercase();
            return self.exes.deny.iter().any(|d| p == d.to_ascii_lowercase());
        }
        false
    }
    fn matches_allow(&self, subject: &str, path: Option<&str>) -> bool {
        let s = subject.to_ascii_lowercase();
        if self.aumids.allow.iter().any(|d| d.eq_ignore_ascii_case(&s)) {
            return true;
        }
        if let Some(p) = path {
            let p = p.to_ascii_lowercase();
            return self.exes.allow.iter().any(|d| p == d.to_ascii_lowercase());
        }
        false
    }
}

/// Pluggable URL policy. The original talks to chatgpt.com/.../site_status; we
/// leave a no-op implementation that always allows. Hosts can swap this in.
pub trait UrlPolicy: Send + Sync {
    fn check(&self, _url: &str) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Default)]
pub struct AllowAllUrlPolicy;
impl UrlPolicy for AllowAllUrlPolicy {}

/// Persist an interrupt marker for (session, turn). Path layout matches the
/// original: `<home>/cache/computer-use/interrupts/<session>/<turn>`.
pub fn interrupt_marker_path(home: &Path, session: &str, turn: &str) -> PathBuf {
    home.join("cache")
        .join("computer-use")
        .join("interrupts")
        .join(crate::protocol::sanitize_component(session))
        .join(crate::protocol::sanitize_component(turn))
}

pub fn write_interrupt_marker(home: &Path, session: &str, turn: &str) -> std::io::Result<()> {
    let path = interrupt_marker_path(home, session, turn);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, b"")
}

pub fn interrupt_marker_exists(home: &Path, session: &str, turn: &str) -> bool {
    interrupt_marker_path(home, session, turn).exists()
}

/// In-memory per-turn interrupt flag (MCP mode keeps running after Esc).
#[derive(Default)]
pub struct InterruptState {
    /// Turns that were stopped by Esc this process.
    pub stopped: HashSet<(String, String)>,
    /// Physical Esc observed since last cleared.
    pub latched: bool,
}

impl InterruptState {
    pub fn mark(&mut self, session: &str, turn: &str) {
        self.stopped.insert((session.to_string(), turn.to_string()));
        self.latched = true;
    }
    pub fn is_stopped(&self, session: &str, turn: &str) -> bool {
        self.stopped.contains(&(session.to_string(), turn.to_string()))
    }
    pub fn clear_turn(&mut self, session: &str, turn: &str) {
        self.stopped.remove(&(session.to_string(), turn.to_string()));
        self.latched = false;
    }
}

/// Resolve the helper's home directory: `$CODEX_HOME`, else `$USERPROFILE/.codex`
/// (Windows) / `$HOME/.ocu-replica` (elsewhere). We also accept `OCU_REPLICA_HOME`.
pub fn resolve_home() -> PathBuf {
    if let Ok(v) = std::env::var("OCU_REPLICA_HOME") {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Ok(v) = std::env::var("CODEX_HOME") {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Ok(v) = std::env::var("USERPROFILE") {
        if !v.trim().is_empty() {
            return PathBuf::from(v).join(".codex");
        }
    }
    dirs_fallback().join(".ocu-replica")
}

/// Split `OCU_PREAPPROVE`: comma or semicolon separated. Quotes are stripped.
/// Whitespace inside a token is preserved so a full path can contain spaces.
pub fn split_preapprove_list(s: &str) -> Vec<String> {
    s.split([',', ';'])
        .map(str::trim)
        .map(|t| t.trim_matches('"').trim_matches('\'').trim())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Load pre-approval tokens. Unset env / missing file yields an empty list,
/// which leaves the MessageBox path unchanged.
pub fn preapprove_tokens_from_env() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(v) = std::env::var("OCU_PREAPPROVE") {
        out.extend(split_preapprove_list(&v));
    }
    if let Ok(path) = std::env::var("OCU_PREAPPROVE_FILE") {
        let path = path.trim();
        if !path.is_empty() {
            match std::fs::read_to_string(path) {
                Ok(text) => {
                    for line in text.lines() {
                        let line = line.trim();
                        if line.is_empty() || line.starts_with('#') {
                            continue;
                        }
                        out.extend(split_preapprove_list(line));
                    }
                }
                Err(e) => {
                    eprintln!("ocu-replica: OCU_PREAPPROVE_FILE '{path}' not readable ({e})");
                }
            }
        }
    }
    out
}

fn path_basename(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// `notepad.exe` matches a basename; `C:\Windows\System32\notepad.exe` matches
/// the full subject or process path only. No wildcards.
pub fn preapprove_matches(tokens: &[String], subject: &str, path: Option<&str>) -> bool {
    for token in tokens {
        if token.eq_ignore_ascii_case(subject) {
            return true;
        }
        if path.is_some_and(|p| p.eq_ignore_ascii_case(token)) {
            return true;
        }
        let bare = !token.contains('\\') && !token.contains('/');
        if bare {
            if path_basename(subject).eq_ignore_ascii_case(token) {
                return true;
            }
            if path.is_some_and(|p| path_basename(p).eq_ignore_ascii_case(token)) {
                return true;
            }
        }
    }
    false
}

fn dirs_fallback() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Signature check stub used by allow/deny rules. On non-Windows this always
/// reports "unsigned". The real WinVerifyTrust path lives under `win/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureInfo {
    pub trusted: bool,
    pub subject: Option<String>,
}

pub type SignatureChecker = fn(&Path) -> SignatureInfo;

pub fn null_signature_checker(_path: &Path) -> SignatureInfo {
    SignatureInfo { trusted: false, subject: None }
}

/// Optional rule that an allow-listed exe must also present a trusted signer
/// whose subject contains `required_subject` (case-insensitive substring).
pub fn allow_exe_with_signature(
    path: &Path,
    required_subject: Option<&str>,
    checker: SignatureChecker,
) -> Result<(), String> {
    let info = checker(path);
    if !info.trusted {
        return Err(format!("executable signature is not trusted for {}", path.display()));
    }
    if let Some(req) = required_subject {
        let subj = info.subject.unwrap_or_default();
        if !subj.to_ascii_lowercase().contains(&req.to_ascii_lowercase()) {
            return Err(format!(
                "executable signer subject does not match required identity for {}",
                path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_home() -> PathBuf {
        let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("ocu-replica-test-{n}"));
        let _ = std::fs::create_dir_all(&p);
        p
    }

    #[test]
    fn first_use_then_persist() {
        let home = tmp_home();
        let mut eng = PolicyEngine::new(&home);
        let d = eng.evaluate("C:\\Windows\\notepad.exe", "Notepad", Some("C:\\Windows\\notepad.exe"), false);
        assert!(matches!(d, Decision::NeedsApproval { allow_persistent: true, .. }));
        eng.grant("C:\\Windows\\notepad.exe", PersistScope::Always);
        assert!(matches!(
            eng.evaluate("C:\\Windows\\notepad.exe", "Notepad", Some("C:\\Windows\\notepad.exe"), false),
            Decision::Allowed
        ));
        // Reloaded store still has it.
        let eng2 = PolicyEngine::new(&home);
        assert!(eng2.store.is_approved("C:\\Windows\\notepad.exe"));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn audio_forced_session() {
        let home = tmp_home();
        let mut eng = PolicyEngine::new(&home);
        let d = eng.evaluate("computer-audio", "computer audio", None, true);
        match d {
            Decision::NeedsApproval { risk: RiskLevel::High, allow_persistent: false, .. } => {}
            other => panic!("unexpected {other:?}"),
        }
        eng.grant("computer-audio", PersistScope::Always); // forced down to session
        assert!(eng.store.session.contains("computer-audio"));
        assert!(!eng.store.always.contains("computer-audio"));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn blocklist_short_circuits() {
        let home = tmp_home();
        let eng = PolicyEngine::new(&home);
        let d = eng.evaluate("cmd", "Command Prompt", Some("cmd.exe"), false);
        assert!(matches!(d, Decision::Denied { .. }));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn interrupt_marker_roundtrip() {
        let home = tmp_home();
        assert!(!interrupt_marker_exists(&home, "s/1", "t:2"));
        write_interrupt_marker(&home, "s/1", "t:2").unwrap();
        assert!(interrupt_marker_exists(&home, "s/1", "t:2"));
        // Sanitized components.
        assert!(home.join("cache/computer-use/interrupts/s_1/t_2").exists());
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn preapprove_basename_skips_dialog_without_persisting() {
        let home = tmp_home();
        let mut eng = PolicyEngine::new(&home);
        assert!(eng.preapprove.is_empty() || std::env::var_os("OCU_PREAPPROVE").is_some());
        eng.preapprove = split_preapprove_list("notepad.exe, mspaint.exe");
        let path = r"C:\Windows\System32\notepad.exe";
        let d = eng.evaluate(path, "Notepad", Some(path), false);
        assert_eq!(d, Decision::Allowed);
        // Not written to the store: a fresh engine without the list still asks.
        assert!(!eng.store.is_approved(path));
        let eng2 = PolicyEngine::new(&home);
        if eng2.preapprove.is_empty() {
            assert!(matches!(
                eng2.evaluate(path, "Notepad", Some(path), false),
                Decision::NeedsApproval { .. }
            ));
        }
        // A different app is still gated. Audio is not implied by notepad.exe.
        assert!(matches!(
            eng.evaluate("computer-audio", "computer audio", None, true),
            Decision::NeedsApproval { .. }
        ));
        assert!(!preapprove_matches(&eng.preapprove, r"C:\Windows\System32\cmd.exe", Some(r"C:\Windows\System32\cmd.exe")));
        // Full path token does not match a different directory's same basename.
        let full = split_preapprove_list(r"C:\Approved\notepad.exe");
        assert!(!preapprove_matches(&full, r"C:\Windows\System32\notepad.exe", Some(r"C:\Windows\System32\notepad.exe")));
        assert!(preapprove_matches(&full, r"C:\Approved\notepad.exe", Some(r"C:\Approved\notepad.exe")));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn allow_deny_rules() {
        let home = tmp_home();
        let mut eng = PolicyEngine::new(&home);
        eng.rules.exes.deny.push("c:\\blocked\\app.exe".into());
        let d = eng.evaluate("blocked", "Blocked", Some("C:\\blocked\\app.exe"), false);
        assert!(matches!(d, Decision::Denied { .. }));
        eng.rules.exes.allow.push("c:\\ok\\app.exe".into());
        let d = eng.evaluate("ok", "Ok", Some("C:\\ok\\app.exe"), false);
        assert_eq!(d, Decision::Allowed);
        let _ = std::fs::remove_dir_all(home);
    }
}

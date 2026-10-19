use crate::policy::{PersistScope, RiskLevel};
use windows::core::PCWSTR;
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_ICONQUESTION, MB_YESNO, MB_YESNOCANCEL};

/// Native MessageBox first-use approval.
/// Yes = session, Yes-to-all style (IDYES with allow_persistent -> Always via Cancel=Always?);
/// We map: Yes = Session, Yes+Shift isn't available, so:
///   MB_YESNOCANCEL: Yes=Session, No=Deny, Cancel=Always (when allowed).
pub fn ask(subject: &str, display: &str, risk: RiskLevel, allow_persistent: bool) -> Result<PersistScope, String> {
    let title = widestring("ocu-replica — allow app?");
    let risk_s = match risk { RiskLevel::High => "HIGH", RiskLevel::Low => "low" };
    let body = if allow_persistent {
        format!(
            "Allow ocu-replica to control \"{display}\"?\n\nSubject: {subject}\nRisk: {risk_s}\n\nYes = this session only\nCancel = always allow\nNo = deny"
        )
    } else {
        format!(
            "Allow ocu-replica to control \"{display}\" for this session?\n\nSubject: {subject}\nRisk: {risk_s}\n\nYes = this session\nNo = deny"
        )
    };
    let body_w = widestring(&body);
    let flags = if allow_persistent { MB_YESNOCANCEL | MB_ICONQUESTION } else { MB_YESNO | MB_ICONQUESTION };
    let rc = unsafe { MessageBoxW(None, PCWSTR(body_w.as_ptr()), PCWSTR(title.as_ptr()), flags) };
    match rc {
        IDYES => Ok(PersistScope::Session),
        r if allow_persistent && r.0 == 2 /* IDCANCEL */ => Ok(PersistScope::Always),
        _ => Err(format!("user denied control of {display}")),
    }
}

fn widestring(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

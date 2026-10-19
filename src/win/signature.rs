use crate::policy::SignatureInfo;
use std::path::Path;

pub fn check(path: &Path) -> SignatureInfo {
    match verify(path) {
        Ok(s) => s,
        Err(_) => SignatureInfo { trusted: false, subject: None },
    }
}

fn verify(path: &Path) -> Result<SignatureInfo, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{GUID, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{HANDLE, HWND};
    use windows::Win32::Security::WinTrust::{
        WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
        WINTRUST_DATA_PROVIDER_FLAGS, WINTRUST_DATA_UICHOICE, WINTRUST_DATA_UICONTEXT,
        WINTRUST_DATA_UNION_CHOICE, WINTRUST_FILE_INFO, WTD_CHOICE_FILE, WTD_REVOKE_NONE,
        WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        let mut file_info = WINTRUST_FILE_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: PCWSTR(wide.as_ptr()),
            hFile: HANDLE::default(),
            pgKnownSubject: std::ptr::null_mut(),
        };
        let mut data = WINTRUST_DATA {
            cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
            pPolicyCallbackData: std::ptr::null_mut(),
            pSIPClientData: std::ptr::null_mut(),
            dwUIChoice: WINTRUST_DATA_UICHOICE(WTD_UI_NONE.0),
            fdwRevocationChecks: WTD_REVOKE_NONE,
            dwUnionChoice: WINTRUST_DATA_UNION_CHOICE(WTD_CHOICE_FILE.0),
            Anonymous: WINTRUST_DATA_0 { pFile: &mut file_info },
            dwStateAction: WTD_STATEACTION_VERIFY,
            hWVTStateData: HANDLE::default(),
            pwszURLReference: PWSTR::null(),
            dwProvFlags: WINTRUST_DATA_PROVIDER_FLAGS(0),
            dwUIContext: WINTRUST_DATA_UICONTEXT(0),
            pSignatureSettings: std::ptr::null_mut(),
        };
        let mut action: GUID = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let status = WinVerifyTrust(HWND::default(), &mut action, &mut data as *mut _ as *mut _);
        Ok(SignatureInfo {
            trusted: status == 0,
            subject: None,
        })
    }
}

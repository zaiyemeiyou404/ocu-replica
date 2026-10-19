//! Built-in product-policy blocklist.
//!
//! The original's `policy/managed.rs` ships a hard-coded denylist of terminals,
//! password managers, AV/EDR UIs and system-shell hosts. The names below were
//! recovered from its string table; matching is case-insensitive against the
//! process name (basename of the .exe without extension) and, when available,
//! the AUMID / display name. A hit yields "product policy blocks this app".

/// Categories used only for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Terminal,
    PasswordManager,
    Antivirus,
    SystemShell,
}

const TERMINALS: &[&str] = &[
    "cmd", "cmder", "conemu", "conemu64", "conemu64c", "conemuc", "conhost",
    "powershell", "powershell_ise", "pwsh", "wt", "windowsterminal", "windows terminal",
    "wsl", "bash", "git-bash", "git bash", "putty", "puttytel",
    "alacritty", "wezterm", "wezterm-gui", "hyper", "kitty", "mintty",
    "mobaxterm", "openconsole", "tabby", "terminal", "terminus", "termius",
    "ttermpro", "warp", "fluentterminal", "fluent terminal", "tera term",
];

const PASSWORD_MANAGERS: &[&str] = &[
    "1password", "1password-browsersupport", "bitwarden", "dashlane", "enpass",
    "enpass password manager", "icloud passwords", "icloudpasswords", "identities",
    "keepass", "keepass password safe", "keepassxc", "keeper password manager",
    "keeperpasswordmanager", "lastpass", "nordpass", "proton pass", "protonpass", "roboform",
];

const ANTIVIRUS: &[&str] = &[
    "a2start", "ad-aware antivirus", "adaware", "avg antivirus", "avg antivirus free",
    "avg internet security", "avgui", "avgnt", "avast antivirus", "avast free antivirus",
    "avast one", "avast premium security", "avastui", "avktray", "avira", "avira antivirus",
    "avira free antivirus", "avira.oe.systray", "avira.systray", "avp", "avpui", "bdagent",
    "bitdefender", "bitdefender agent", "bitdefender antivirus free", "bitdefender security center",
    "bitdefender total security", "ciscistray", "clamwin", "clamwin antivirus", "comodo antivirus",
    "comodo firewall", "comodo internet security", "crowdstrike falcon sensor",
    "crowdstrike windows sensor", "csfalconcontainer", "csfalconservice", "dr.web",
    "dr.web security space", "dwservice", "egui", "emsisoft anti-malware", "emsisoft security center",
    "eset", "eset gui", "eset internet security", "eset main gui", "eset nod32 antivirus",
    "eset security", "f-secure", "fs_ui_32", "g data antivirus", "g data security software",
    "g data securitycenter", "g data total protection", "gdsck", "kaspersky", "kaspersky anti-virus",
    "kaspersky free", "kaspersky internet security", "kaspersky security cloud",
    "kaspersky total security", "malwarebytes", "malwarebytes anti-malware", "mbam", "mbamgui",
    "mcafee", "mcafee livesafe", "mcafee security", "mcafee security scan plus",
    "mcafee total protection", "mcafee ui container", "mcuicnt", "microsoft defender",
    "microsoft defender antivirus", "norton 360", "norton antivirus", "norton security",
    "nortonsecurity", "panda dome", "panda security", "pccntmon", "psuaconsole", "seccenter",
    "sechealthui", "securityhealthsystray", "sophos home", "sophos ui", "trend micro",
    "trend micro internet security", "trend micro maximum security", "totalav",
    "totalav ultimate antivirus", "totalav ultimate antivirus user interface", "uiwinmgr",
    "uistub", "webroot secureanywhere", "windows security", "wrsazatray", "zlclient",
    "zonealarm", "zonealarm security",
];

const SYSTEM_SHELL: &[&str] = &[
    "applicationframehost", "backgroundtaskhost", "click to do", "clicktodoapp", "ctfmon",
    "inputapp", "lockapp", "runtimebroker", "searchapp", "searchhost", "shellexperiencehost",
    "sihost", "startmenuexperiencehost", "tabtip", "textinputhost", "codex",
    "codex (command prompt)",
];

fn normalize(s: &str) -> String {
    s.trim().to_ascii_lowercase().replace(['_', '-'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn process_stem(path_or_name: &str) -> String {
    let name = path_or_name.replace('\\', "/");
    let base = name.rsplit('/').next().unwrap_or(&name);
    // Prefer stripping only a trailing .exe/.com/.bat; keep other dots.
    let stem = if let Some((s, ext)) = base.rsplit_once('.') {
        match ext.to_ascii_lowercase().as_str() {
            "exe" | "com" | "bat" | "cmd" | "scr" => s,
            _ => base,
        }
    } else {
        base
    };
    normalize(stem)
}

fn list_contains(list: &[&str], candidate: &str) -> bool {
    let c = normalize(candidate);
    list.iter().any(|e| normalize(e) == c)
}

/// Match an app against the hard-coded denylist.
/// `process_path_or_name` is the executable path or basename; `aumid_or_display`
/// is an optional AUMID / product name.
pub fn blocked(process_path_or_name: &str, aumid_or_display: Option<&str>) -> Option<Category> {
    let stem = process_stem(process_path_or_name);
    let display = aumid_or_display.map(normalize);
    let candidates: Vec<&str> = std::iter::once(stem.as_str())
        .chain(display.as_deref())
        .collect();

    for c in &candidates {
        if list_contains(TERMINALS, c) {
            return Some(Category::Terminal);
        }
        if list_contains(PASSWORD_MANAGERS, c) {
            return Some(Category::PasswordManager);
        }
        if list_contains(ANTIVIRUS, c) {
            return Some(Category::Antivirus);
        }
        if list_contains(SYSTEM_SHELL, c) {
            return Some(Category::SystemShell);
        }
    }
    None
}

pub fn is_blocked(process_path_or_name: &str, aumid_or_display: Option<&str>) -> bool {
    blocked(process_path_or_name, aumid_or_display).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_terminals() {
        assert_eq!(blocked("C:\\Windows\\System32\\cmd.exe", None), Some(Category::Terminal));
        assert_eq!(blocked("pwsh", None), Some(Category::Terminal));
        assert_eq!(blocked("WindowsTerminal.exe", None), Some(Category::Terminal));
        assert_eq!(blocked("wt.exe", None), Some(Category::Terminal));
        assert_eq!(blocked("wsl.exe", None), Some(Category::Terminal));
        assert_eq!(blocked("bash", None), Some(Category::Terminal));
        assert_eq!(blocked("Git Bash", Some("Git Bash")), Some(Category::Terminal));
    }

    #[test]
    fn blocks_password_managers() {
        assert_eq!(blocked("1Password.exe", None), Some(Category::PasswordManager));
        assert_eq!(blocked("KeePassXC.exe", None), Some(Category::PasswordManager));
        assert_eq!(blocked("Bitwarden.exe", None), Some(Category::PasswordManager));
    }

    #[test]
    fn blocks_antivirus_and_shell() {
        assert_eq!(blocked("MsMpEng.exe", None), None); // engine itself is not on the UI list
        assert_eq!(blocked("SecurityHealthSystray.exe", None), Some(Category::Antivirus));
        assert_eq!(blocked("LockApp.exe", None), Some(Category::SystemShell));
        assert_eq!(blocked("StartMenuExperienceHost.exe", None), Some(Category::SystemShell));
        assert_eq!(blocked("SearchHost.exe", None), Some(Category::SystemShell));
    }

    #[test]
    fn allows_ordinary_apps() {
        assert!(blocked("notepad.exe", None).is_none());
        assert!(blocked("C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe", None).is_none());
        assert!(blocked("Code.exe", Some("Visual Studio Code")).is_none());
    }
}

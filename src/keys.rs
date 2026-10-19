//! X Window System keysym-style key chord parsing.
//!
//! The chord grammar and alias table were recovered from the original helper's
//! string table (`CONTROL CTRL_L CONTROL_L ... F20` and the `+`-separated chord
//! described in the window2 PressKey docs). Virtual-key assignments below are our
//! own mapping to Win32 VKs.

/// A resolved chord: modifier VKs held down (in order) around a main VK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    pub modifiers: Vec<u16>,
    pub key: u16,
    /// For printable keys with no dedicated VK, the char to send via unicode.
    pub unicode: Option<char>,
}

// Win32 virtual-key codes we reference (subset; kept local so the pure module
// builds on Linux for tests).
pub const VK_CONTROL: u16 = 0x11;
pub const VK_LCONTROL: u16 = 0xA2;
pub const VK_RCONTROL: u16 = 0xA3;
pub const VK_SHIFT: u16 = 0x10;
pub const VK_LSHIFT: u16 = 0xA0;
pub const VK_RSHIFT: u16 = 0xA1;
pub const VK_MENU: u16 = 0x12; // Alt
pub const VK_LMENU: u16 = 0xA4;
pub const VK_RMENU: u16 = 0xA5;
pub const VK_LWIN: u16 = 0x5B;
pub const VK_RWIN: u16 = 0x5C;
pub const VK_RETURN: u16 = 0x0D;
pub const VK_TAB: u16 = 0x09;
pub const VK_SPACE: u16 = 0x20;
pub const VK_ESCAPE: u16 = 0x1B;
pub const VK_BACK: u16 = 0x08;
pub const VK_DELETE: u16 = 0x2E;
pub const VK_INSERT: u16 = 0x2D;
pub const VK_HOME: u16 = 0x24;
pub const VK_END: u16 = 0x23;
pub const VK_PRIOR: u16 = 0x21;
pub const VK_NEXT: u16 = 0x22;
pub const VK_UP: u16 = 0x26;
pub const VK_DOWN: u16 = 0x28;
pub const VK_LEFT: u16 = 0x25;
pub const VK_RIGHT: u16 = 0x27;
pub const VK_CLEAR: u16 = 0x0C;
pub const VK_APPS: u16 = 0x5D;
pub const VK_HELP: u16 = 0x2F;
pub const VK_SELECT: u16 = 0x29;
pub const VK_PRINT: u16 = 0x2A;
pub const VK_SNAPSHOT: u16 = 0x2C;
pub const VK_EXECUTE: u16 = 0x2B;
pub const VK_CANCEL: u16 = 0x03;
pub const VK_PAUSE: u16 = 0x13;
pub const VK_SCROLL: u16 = 0x91;
pub const VK_NUMLOCK: u16 = 0x90;
pub const VK_CAPITAL: u16 = 0x14;

fn modifier_vk(name: &str) -> Option<u16> {
    Some(match name {
        "CONTROL" | "CTRL" => VK_CONTROL,
        "CTRL_L" | "CONTROL_L" => VK_LCONTROL,
        "CTRL_R" | "CONTROL_R" => VK_RCONTROL,
        "SHIFT" => VK_SHIFT,
        "SHIFT_L" => VK_LSHIFT,
        "SHIFT_R" => VK_RSHIFT,
        "ALT" | "OPTION" => VK_MENU,
        "ALT_L" | "OPTION_L" => VK_LMENU,
        "ALT_R" | "OPTION_R" => VK_RMENU,
        "WIN" | "WINDOWS" | "META" | "COMMAND" | "CMD" | "SUPER" | "OS" | "WIN_L" | "WINDOWS_L"
        | "META_L" | "COMMAND_L" | "CMD_L" | "SUPER_L" | "OS_L" => VK_LWIN,
        "WIN_R" | "WINDOWS_R" | "META_R" | "COMMAND_R" | "CMD_R" | "SUPER_R" | "OS_R" => VK_RWIN,
        _ => return None,
    })
}

/// Resolve a single, non-modifier token to a VK (and optional unicode char).
fn named_key(name: &str) -> Option<(u16, Option<char>)> {
    let vk = match name {
        "ENTER" | "RETURN" | "LINEFEED" => VK_RETURN,
        "KP_ENTER" | "NUMPAD_ENTER" | "NUMPADENTER" | "NUM_ENTER" | "NUMENTER" => VK_RETURN,
        "TAB" => VK_TAB,
        "CLEAR" => VK_CLEAR,
        "SPACE" => VK_SPACE,
        "ESC" | "ESCAPE" => VK_ESCAPE,
        "BACKSPACE" => VK_BACK,
        "DELETE" | "DEL" | "KP_DELETE" | "NUMPAD_DELETE" | "NUMPADDELETE" | "NUM_DELETE" | "NUMDELETE" => VK_DELETE,
        "INSERT" => VK_INSERT,
        "HOME" | "BEGIN" => VK_HOME,
        "END" => VK_END,
        "PRIOR" | "PAGEUP" | "PAGE_UP" => VK_PRIOR,
        "NEXT" | "PAGEDOWN" | "PAGE_DOWN" => VK_NEXT,
        "UP" => VK_UP,
        "DOWN" => VK_DOWN,
        "LEFT" => VK_LEFT,
        "RIGHT" => VK_RIGHT,
        "MENU" => VK_APPS,
        "HELP" => VK_HELP,
        "SELECT" => VK_SELECT,
        "PRINT" => VK_PRINT,
        "SYS_REQ" => VK_SNAPSHOT,
        "EXECUTE" => VK_EXECUTE,
        "CANCEL" | "BREAK" => VK_CANCEL,
        "PAUSE" => VK_PAUSE,
        "SCROLL_LOCK" => VK_SCROLL,
        "NUM_LOCK" => VK_NUMLOCK,
        "CAPS_LOCK" | "SHIFT_LOCK" => VK_CAPITAL,
        "MODE_SWITCH" | "SCRIPT_SWITCH" => return Some((VK_MENU, None)),
        _ => return named_key_extended(name),
    };
    Some((vk, None))
}

fn named_key_extended(name: &str) -> Option<(u16, Option<char>)> {
    // Function keys F1..F20 -> VK_F1 (0x70) ..
    if let Some(num) = name.strip_prefix('F') {
        if let Ok(n) = num.parse::<u16>() {
            if (1..=20).contains(&n) {
                return Some((0x6F + n, None));
            }
        }
    }
    // Numpad digits.
    for (aliases, vk) in NUMPAD_DIGITS {
        if aliases.contains(&name) {
            return Some((*vk, None));
        }
    }
    // Numpad operators map to characters we emit via unicode.
    let punct = match name {
        "KP_MULTIPLY" | "KP_STAR" | "NUMPAD_MULTIPLY" | "NUMPADMULTIPLY" | "NUMPAD_STAR" | "NUMPADSTAR"
        | "NUM_MULTIPLY" | "NUMMULTIPLY" => '*',
        "KP_ADD" | "KP_PLUS" | "NUMPAD_ADD" | "NUMPADADD" | "NUMPAD_PLUS" | "NUMPADPLUS" | "NUM_ADD" | "NUMADD" => '+',
        "KP_SUBTRACT" | "KP_MINUS" | "NUMPAD_SUBTRACT" | "NUMPADSUBTRACT" | "NUMPAD_MINUS" | "NUMPADMINUS"
        | "NUM_SUBTRACT" | "NUMSUBTRACT" => '-',
        "KP_DECIMAL" | "KP_PERIOD" | "KP_DOT" | "NUMPAD_DECIMAL" | "NUMPADDECIMAL" | "NUMPAD_PERIOD" | "NUMPADPERIOD"
        | "NUMPAD_DOT" | "NUMPADDOT" => '.',
        "KP_DIVIDE" | "KP_SLASH" | "NUMPAD_DIVIDE" | "NUMPADDIVIDE" | "NUMPAD_SLASH" | "NUMPADSLASH" | "NUM_DIVIDE"
        | "NUMDIVIDE" => '/',
        "KP_EQUAL" | "NUMPAD_EQUAL" | "NUMPADEQUAL" | "NUM_EQUAL" | "NUMEQUAL" => '=',
        "PLUS" => '+',
        "EQUAL" | "EQUALS" => '=',
        "MINUS" | "HYPHEN" => '-',
        "UNDERSCORE" => '_',
        "COMMA" => ',',
        "LESS" => '<',
        "PERIOD" | "DOT" | "FULL_STOP" => '.',
        "GREATER" => '>',
        "SLASH" | "FORWARD_SLASH" => '/',
        "QUESTION" => '?',
        "SEMICOLON" => ';',
        "COLON" => ':',
        "APOSTROPHE" | "QUOTE" | "SINGLE_QUOTE" => '\'',
        "DOUBLE_QUOTE" => '"',
        "GRAVE" | "BACKTICK" => '`',
        "TILDE" => '~',
        "BRACKETLEFT" | "BRACKET_LEFT" | "LEFTBRACKET" | "LEFT_BRACKET" => '[',
        "BRACELEFT" | "BRACE_LEFT" | "LEFTBRACE" | "LEFT_BRACE" => '{',
        "BRACKETRIGHT" | "BRACKET_RIGHT" | "RIGHTBRACKET" | "RIGHT_BRACKET" => ']',
        "BRACERIGHT" | "BRACE_RIGHT" | "RIGHTBRACE" | "RIGHT_BRACE" => '}',
        "BACKSLASH" | "BACK_SLASH" => '\\',
        "BAR" | "PIPE" => '|',
        _ => return None,
    };
    Some((0, Some(punct)))
}

const NUMPAD_DIGITS: &[(&[&str], u16)] = &[
    (&["KP_0", "KP0", "NUMPAD_0", "NUMPAD0", "NUM_0", "NUM0"], 0x60),
    (&["KP_1", "KP1", "NUMPAD_1", "NUMPAD1", "NUM_1", "NUM1"], 0x61),
    (&["KP_2", "KP2", "NUMPAD_2", "NUMPAD2", "NUM_2", "NUM2"], 0x62),
    (&["KP_3", "KP3", "NUMPAD_3", "NUMPAD3", "NUM_3", "NUM3"], 0x63),
    (&["KP_4", "KP4", "NUMPAD_4", "NUMPAD4", "NUM_4", "NUM4"], 0x64),
    (&["KP_5", "KP5", "NUMPAD_5", "NUMPAD5", "NUM_5", "NUM5"], 0x65),
    (&["KP_6", "KP6", "NUMPAD_6", "NUMPAD6", "NUM_6", "NUM6"], 0x66),
    (&["KP_7", "KP7", "NUMPAD_7", "NUMPAD7", "NUM_7", "NUM7"], 0x67),
    (&["KP_8", "KP8", "NUMPAD_8", "NUMPAD8", "NUM_8", "NUM8"], 0x68),
    (&["KP_9", "KP9", "NUMPAD_9", "NUMPAD9", "NUM_9", "NUM9"], 0x69),
];

/// Resolve one token (already trimmed, non-empty) into either a modifier or a key.
enum Token {
    Modifier(u16),
    Key(u16, Option<char>),
}

fn resolve_token(raw: &str) -> Result<Token, String> {
    let upper = raw.to_ascii_uppercase();
    if let Some(vk) = modifier_vk(&upper) {
        return Ok(Token::Modifier(vk));
    }
    // Single printable character token (letters/digits/punctuation): send as unicode,
    // preserving case (lowercase 'a' vs uppercase 'A').
    let mut chars = raw.chars();
    if let (Some(c), None) = (chars.next(), chars.clone().next()) {
        if c.is_ascii_graphic() {
            // Letters/digits can use their VK; punctuation goes through unicode.
            if c.is_ascii_alphanumeric() {
                let vk = c.to_ascii_uppercase() as u16;
                return Ok(Token::Key(vk, if c.is_ascii_lowercase() { None } else { Some(c) }));
            }
            return Ok(Token::Key(0, Some(c)));
        }
    }
    if let Some((vk, uni)) = named_key(&upper) {
        return Ok(Token::Key(vk, uni));
    }
    Err(format!("key '{raw}' is not recognized"))
}

/// Parse a `+`-separated chord. Whitespace around `+` is ignored.
pub fn parse_chord(input: &str) -> Result<Chord, String> {
    let tokens: Vec<&str> = input.split('+').map(str::trim).filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() {
        return Err("key is required".into());
    }
    let mut modifiers = Vec::new();
    let mut main: Option<(u16, Option<char>)> = None;
    for t in tokens {
        match resolve_token(t)? {
            Token::Modifier(vk) => modifiers.push(vk),
            Token::Key(vk, uni) => {
                if main.is_some() {
                    return Err("a chord may contain only one non-modifier key".into());
                }
                main = Some((vk, uni));
            }
        }
    }
    let (key, unicode) = main.ok_or("a chord needs exactly one non-modifier key")?;
    Ok(Chord { modifiers, key, unicode })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_letter() {
        let c = parse_chord("a").unwrap();
        assert_eq!(c.key, b'A' as u16);
        assert!(c.modifiers.is_empty());
        assert_eq!(c.unicode, None);
    }

    #[test]
    fn uppercase_letter_keeps_char() {
        let c = parse_chord("A").unwrap();
        assert_eq!(c.unicode, Some('A'));
    }

    #[test]
    fn ctrl_a() {
        let c = parse_chord("Control_L+a").unwrap();
        assert_eq!(c.modifiers, vec![VK_LCONTROL]);
        assert_eq!(c.key, b'A' as u16);
    }

    #[test]
    fn multi_modifier_with_alias() {
        let c = parse_chord("Control_L + Shift_L + period").unwrap();
        assert_eq!(c.modifiers, vec![VK_LCONTROL, VK_LSHIFT]);
        assert_eq!(c.unicode, Some('.'));
    }

    #[test]
    fn mac_aliases_map() {
        assert_eq!(parse_chord("Command+c").unwrap().modifiers, vec![VK_LWIN]);
        assert_eq!(parse_chord("Ctrl+c").unwrap().modifiers, vec![VK_CONTROL]);
    }

    #[test]
    fn named_keys() {
        assert_eq!(parse_chord("Return").unwrap().key, VK_RETURN);
        assert_eq!(parse_chord("space").unwrap().key, VK_SPACE);
        assert_eq!(parse_chord("KP_0").unwrap().key, 0x60);
        assert_eq!(parse_chord("Page_Up").unwrap().key, VK_PRIOR);
        assert_eq!(parse_chord("F5").unwrap().key, 0x74);
        assert_eq!(parse_chord("Numpad_0").unwrap().key, 0x60);
    }

    #[test]
    fn errors() {
        assert!(parse_chord("").is_err());
        assert!(parse_chord("  + ").is_err());
        assert!(parse_chord("a+b").is_err());
        assert!(parse_chord("Nonsense_Key").is_err());
    }
}

/// Convert an xdotool-style key name to terminal escape sequence bytes.
pub fn key_to_bytes(key: &str) -> Vec<u8> {
    // Handle modifier+key combos: ctrl+x, alt+x, shift+x, ctrl+shift+x
    if let Some(pos) = key.rfind('+') {
        let (modifiers_str, rest) = key.split_at(pos);
        let base = &rest[1..]; // skip the '+'

        let modifiers: Vec<&str> = modifiers_str.split('+').collect();

        // Check all parts are known modifiers
        let all_modifiers = modifiers.iter().all(|m| {
            matches!(
                m.to_lowercase().as_str(),
                "ctrl" | "control" | "alt" | "meta" | "shift" | "super"
            )
        });

        if all_modifiers {
            let has_ctrl = modifiers.iter().any(|m| {
                matches!(m.to_lowercase().as_str(), "ctrl" | "control")
            });
            let has_alt = modifiers.iter().any(|m| {
                matches!(m.to_lowercase().as_str(), "alt" | "meta")
            });

            // ctrl+letter → control character
            if has_ctrl && base.len() == 1 {
                let ch = base.chars().next().unwrap();
                if ch.is_ascii_alphabetic() {
                    let ctrl_byte = (ch.to_ascii_lowercase() as u8) - b'a' + 1;
                    if has_alt {
                        return vec![0x1b, ctrl_byte];
                    }
                    return vec![ctrl_byte];
                }
            }

            // alt+key → ESC prefix + key bytes
            if has_alt && !has_ctrl {
                let base_bytes = key_to_bytes(base);
                let mut result = vec![0x1b];
                result.extend_from_slice(&base_bytes);
                return result;
            }

            // For special keys with modifiers, use CSI modifier encoding
            // modifier param: shift=2, alt=3, shift+alt=4, ctrl=5, shift+ctrl=6, alt+ctrl=7, shift+alt+ctrl=8
            let has_shift = modifiers.iter().any(|m| {
                matches!(m.to_lowercase().as_str(), "shift" | "super")
            });
            let modifier_param = 1
                + if has_shift { 1 } else { 0 }
                + if has_alt { 2 } else { 0 }
                + if has_ctrl { 4 } else { 0 };

            // Apply modifier encoding to arrow/nav keys
            if let Some(bytes) = modified_special_key(base, modifier_param) {
                return bytes;
            }
        }
    }

    // Simple key names
    match key {
        "Return" | "KP_Enter" | "Enter" => vec![b'\r'],
        "BackSpace" => vec![0x7f],
        "Tab" => vec![b'\t'],
        "ISO_Left_Tab" => vec![0x1b, b'[', b'Z'], // shift-tab
        "Escape" => vec![0x1b],
        "space" | "Space" => vec![b' '],
        "Delete" | "KP_Delete" => vec![0x1b, b'[', b'3', b'~'],
        "Insert" => vec![0x1b, b'[', b'2', b'~'],
        "Home" => vec![0x1b, b'[', b'H'],
        "End" => vec![0x1b, b'[', b'F'],
        "Up" => vec![0x1b, b'[', b'A'],
        "Down" => vec![0x1b, b'[', b'B'],
        "Right" => vec![0x1b, b'[', b'C'],
        "Left" => vec![0x1b, b'[', b'D'],
        "Page_Up" | "Prior" => vec![0x1b, b'[', b'5', b'~'],
        "Page_Down" | "Next" => vec![0x1b, b'[', b'6', b'~'],
        "F1" => vec![0x1b, b'O', b'P'],
        "F2" => vec![0x1b, b'O', b'Q'],
        "F3" => vec![0x1b, b'O', b'R'],
        "F4" => vec![0x1b, b'O', b'S'],
        "F5" => vec![0x1b, b'[', b'1', b'5', b'~'],
        "F6" => vec![0x1b, b'[', b'1', b'7', b'~'],
        "F7" => vec![0x1b, b'[', b'1', b'8', b'~'],
        "F8" => vec![0x1b, b'[', b'1', b'9', b'~'],
        "F9" => vec![0x1b, b'[', b'2', b'0', b'~'],
        "F10" => vec![0x1b, b'[', b'2', b'1', b'~'],
        "F11" => vec![0x1b, b'[', b'2', b'3', b'~'],
        "F12" => vec![0x1b, b'[', b'2', b'4', b'~'],
        // Single character: return as-is
        other if other.len() == 1 => other.as_bytes().to_vec(),
        // Unknown: return as literal bytes
        other => other.as_bytes().to_vec(),
    }
}

/// Generate modified special key sequence using CSI u or xterm-style encoding.
/// E.g., ctrl+Up → \x1b[1;5A, shift+Home → \x1b[1;2H
fn modified_special_key(base: &str, modifier_param: u8) -> Option<Vec<u8>> {
    let mod_str = modifier_param.to_string();
    let mod_bytes = mod_str.as_bytes();

    match base {
        "Up" => Some([&[0x1b, b'[', b'1', b';'], mod_bytes, &[b'A']].concat()),
        "Down" => Some([&[0x1b, b'[', b'1', b';'], mod_bytes, &[b'B']].concat()),
        "Right" => Some([&[0x1b, b'[', b'1', b';'], mod_bytes, &[b'C']].concat()),
        "Left" => Some([&[0x1b, b'[', b'1', b';'], mod_bytes, &[b'D']].concat()),
        "Home" => Some([&[0x1b, b'[', b'1', b';'], mod_bytes, &[b'H']].concat()),
        "End" => Some([&[0x1b, b'[', b'1', b';'], mod_bytes, &[b'F']].concat()),
        "Insert" => Some([&[0x1b, b'[', b'2', b';'], mod_bytes, &[b'~']].concat()),
        "Delete" | "KP_Delete" => Some([&[0x1b, b'[', b'3', b';'], mod_bytes, &[b'~']].concat()),
        "Page_Up" | "Prior" => Some([&[0x1b, b'[', b'5', b';'], mod_bytes, &[b'~']].concat()),
        "Page_Down" | "Next" => Some([&[0x1b, b'[', b'6', b';'], mod_bytes, &[b'~']].concat()),
        "F1" => Some([&[0x1b, b'[', b'1', b';'], mod_bytes, &[b'P']].concat()),
        "F2" => Some([&[0x1b, b'[', b'1', b';'], mod_bytes, &[b'Q']].concat()),
        "F3" => Some([&[0x1b, b'[', b'1', b';'], mod_bytes, &[b'R']].concat()),
        "F4" => Some([&[0x1b, b'[', b'1', b';'], mod_bytes, &[b'S']].concat()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_keys() {
        assert_eq!(key_to_bytes("Return"), vec![b'\r']);
        assert_eq!(key_to_bytes("Enter"), vec![b'\r']);
        assert_eq!(key_to_bytes("Tab"), vec![b'\t']);
        assert_eq!(key_to_bytes("Escape"), vec![0x1b]);
        assert_eq!(key_to_bytes("space"), vec![b' ']);
        assert_eq!(key_to_bytes("BackSpace"), vec![0x7f]);
    }

    #[test]
    fn arrow_keys() {
        assert_eq!(key_to_bytes("Up"), vec![0x1b, b'[', b'A']);
        assert_eq!(key_to_bytes("Down"), vec![0x1b, b'[', b'B']);
        assert_eq!(key_to_bytes("Right"), vec![0x1b, b'[', b'C']);
        assert_eq!(key_to_bytes("Left"), vec![0x1b, b'[', b'D']);
    }

    #[test]
    fn nav_keys() {
        assert_eq!(key_to_bytes("Home"), vec![0x1b, b'[', b'H']);
        assert_eq!(key_to_bytes("End"), vec![0x1b, b'[', b'F']);
        assert_eq!(key_to_bytes("Delete"), vec![0x1b, b'[', b'3', b'~']);
        assert_eq!(key_to_bytes("Insert"), vec![0x1b, b'[', b'2', b'~']);
        assert_eq!(key_to_bytes("Page_Up"), vec![0x1b, b'[', b'5', b'~']);
        assert_eq!(key_to_bytes("Page_Down"), vec![0x1b, b'[', b'6', b'~']);
    }

    #[test]
    fn function_keys() {
        assert_eq!(key_to_bytes("F1"), vec![0x1b, b'O', b'P']);
        assert_eq!(key_to_bytes("F4"), vec![0x1b, b'O', b'S']);
        assert_eq!(key_to_bytes("F5"), vec![0x1b, b'[', b'1', b'5', b'~']);
        assert_eq!(key_to_bytes("F12"), vec![0x1b, b'[', b'2', b'4', b'~']);
    }

    #[test]
    fn ctrl_letter() {
        assert_eq!(key_to_bytes("ctrl+c"), vec![0x03]); // ETX
        assert_eq!(key_to_bytes("ctrl+a"), vec![0x01]); // SOH
        assert_eq!(key_to_bytes("ctrl+z"), vec![0x1a]); // SUB
        assert_eq!(key_to_bytes("ctrl+d"), vec![0x04]); // EOT
        assert_eq!(key_to_bytes("ctrl+l"), vec![0x0c]); // FF
    }

    #[test]
    fn alt_key() {
        assert_eq!(key_to_bytes("alt+a"), vec![0x1b, b'a']);
        assert_eq!(key_to_bytes("alt+x"), vec![0x1b, b'x']);
    }

    #[test]
    fn ctrl_alt() {
        assert_eq!(key_to_bytes("ctrl+alt+c"), vec![0x1b, 0x03]);
    }

    #[test]
    fn modified_arrows() {
        // ctrl+Up → \x1b[1;5A
        assert_eq!(key_to_bytes("ctrl+Up"), vec![0x1b, b'[', b'1', b';', b'5', b'A']);
        // shift+Right → \x1b[1;2C
        assert_eq!(key_to_bytes("shift+Right"), vec![0x1b, b'[', b'1', b';', b'2', b'C']);
    }

    #[test]
    fn single_char() {
        assert_eq!(key_to_bytes("a"), vec![b'a']);
        assert_eq!(key_to_bytes("Z"), vec![b'Z']);
    }
}

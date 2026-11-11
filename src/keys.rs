//! Translates keystrokes into the sequences sent to the shell.
//!
//! Only "special" keys are translated here (arrows, Ctrl+letter, F1…).
//! Ordinary text goes through the system input handler, which is what
//! makes accents, dead keys and the IME work.

use alacritty_terminal::term::TermMode;
use gpui::Keystroke;

pub fn to_bytes(keystroke: &Keystroke, mode: TermMode, option_as_meta: bool) -> Option<Vec<u8>> {
    let mods = &keystroke.modifiers;
    let key = keystroke.key.as_str();

    // macOS-style editing shortcuts, translated to their readline equivalents.
    if mods.platform {
        if mods.control || mods.alt {
            return None;
        }
        return match key {
            "left" => Some(vec![0x01]),      // start of line
            "right" => Some(vec![0x05]),     // end of line
            "backspace" => Some(vec![0x15]), // clear the line
            _ => None,
        };
    }

    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    // xterm parameter: 1 + Shift(1) + Alt(2) + Ctrl(4).
    let modifier = 1 + mods.shift as u8 + 2 * mods.alt as u8 + 4 * mods.control as u8;
    let plain = modifier == 1;

    // Keys of the form `CSI 1 ; mod X` (or `SS3 X` in application mode).
    let cursor_key = |letter: char| -> Vec<u8> {
        if plain {
            let prefix = if app_cursor { "\x1bO" } else { "\x1b[" };
            format!("{prefix}{letter}").into_bytes()
        } else {
            format!("\x1b[1;{modifier}{letter}").into_bytes()
        }
    };
    // Keys of the form `CSI n ; mod ~`.
    let tilde_key = |number: u8| -> Vec<u8> {
        if plain {
            format!("\x1b[{number}~").into_bytes()
        } else {
            format!("\x1b[{number};{modifier}~").into_bytes()
        }
    };

    let bytes = match key {
        "enter" if mods.alt => b"\x1b\r".to_vec(),
        "enter" => b"\r".to_vec(),
        "tab" if mods.shift => b"\x1b[Z".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" => b"\x1b".to_vec(),
        "backspace" if mods.alt => b"\x1b\x7f".to_vec(),
        "backspace" if mods.control => b"\x08".to_vec(),
        "backspace" => b"\x7f".to_vec(),

        // Option+arrow: move word by word, as everywhere on macOS.
        "left" if mods.alt && !mods.control && !mods.shift => b"\x1bb".to_vec(),
        "right" if mods.alt && !mods.control && !mods.shift => b"\x1bf".to_vec(),
        "up" => cursor_key('A'),
        "down" => cursor_key('B'),
        "right" => cursor_key('C'),
        "left" => cursor_key('D'),
        "home" => cursor_key('H'),
        "end" => cursor_key('F'),

        "insert" => tilde_key(2),
        "delete" => tilde_key(3),
        "pageup" => tilde_key(5),
        "pagedown" => tilde_key(6),

        "f1" => function_key('P', plain, modifier),
        "f2" => function_key('Q', plain, modifier),
        "f3" => function_key('R', plain, modifier),
        "f4" => function_key('S', plain, modifier),
        "f5" => tilde_key(15),
        "f6" => tilde_key(17),
        "f7" => tilde_key(18),
        "f8" => tilde_key(19),
        "f9" => tilde_key(20),
        "f10" => tilde_key(21),
        "f11" => tilde_key(23),
        "f12" => tilde_key(24),

        _ => return printable_with_modifiers(keystroke, option_as_meta),
    };
    Some(bytes)
}

fn function_key(letter: char, plain: bool, modifier: u8) -> Vec<u8> {
    if plain {
        format!("\x1bO{letter}").into_bytes()
    } else {
        format!("\x1b[1;{modifier}{letter}").into_bytes()
    }
}

/// Ctrl+key and, with `option_as_meta`, Option+key.
fn printable_with_modifiers(keystroke: &Keystroke, option_as_meta: bool) -> Option<Vec<u8>> {
    let mods = &keystroke.modifiers;
    let key = if keystroke.key == "space" { " " } else { keystroke.key.as_str() };
    let mut chars = key.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return None;
    };

    if mods.control {
        let code = match ch.to_ascii_lowercase() {
            c @ 'a'..='z' => c as u8 - b'a' + 1,
            ' ' | '@' | '2' => 0x00,
            '[' | '3' => 0x1b,
            '\\' | '4' => 0x1c,
            ']' | '5' => 0x1d,
            '^' | '6' => 0x1e,
            '_' | '-' | '/' | '7' => 0x1f,
            '?' | '8' => 0x7f,
            _ => return None,
        };
        return Some(if mods.alt { vec![0x1b, code] } else { vec![code] });
    }

    if mods.alt && option_as_meta {
        let ch = if mods.shift { ch.to_ascii_uppercase() } else { ch };
        let mut bytes = vec![0x1b];
        bytes.extend(ch.to_string().as_bytes());
        return Some(bytes);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(source: &str, mode: TermMode, option_as_meta: bool) -> Option<Vec<u8>> {
        to_bytes(&Keystroke::parse(source).unwrap(), mode, option_as_meta)
    }

    #[test]
    fn control_letters() {
        assert_eq!(bytes("ctrl-c", TermMode::NONE, false), Some(vec![0x03]));
        assert_eq!(bytes("ctrl-space", TermMode::NONE, false), Some(vec![0x00]));
    }

    #[test]
    fn arrows_follow_application_cursor_mode() {
        assert_eq!(bytes("up", TermMode::NONE, false), Some(b"\x1b[A".to_vec()));
        assert_eq!(bytes("up", TermMode::APP_CURSOR, false), Some(b"\x1bOA".to_vec()));
        assert_eq!(bytes("shift-up", TermMode::NONE, false), Some(b"\x1b[1;2A".to_vec()));
        assert_eq!(bytes("alt-left", TermMode::NONE, false), Some(b"\x1bb".to_vec()));
    }

    #[test]
    fn plain_text_is_left_to_the_input_handler() {
        assert_eq!(bytes("a", TermMode::NONE, false), None);
        assert_eq!(bytes("alt-l", TermMode::NONE, false), None);
        assert_eq!(bytes("alt-l", TermMode::NONE, true), Some(b"\x1bl".to_vec()));
    }

    #[test]
    fn mac_editing_shortcuts() {
        assert_eq!(bytes("cmd-left", TermMode::NONE, false), Some(vec![0x01]));
        assert_eq!(bytes("cmd-backspace", TermMode::NONE, false), Some(vec![0x15]));
        assert_eq!(bytes("cmd-t", TermMode::NONE, false), None);
    }
}

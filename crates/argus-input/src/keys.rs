//! Key combinations: `"cmd+shift+s"`, `"Return"`, `"cmd++"`.
//!
//! Key names follow the xdotool / computer-use keysyms agents already use
//! (`Return`, `Page_Down`, `KP_Add`), plus common aliases. Virtual key codes
//! are those of the ANSI US layout.

use crate::{Error, Result};

/// Held modifier keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    /// Command.
    pub cmd: bool,
    /// Shift.
    pub shift: bool,
    /// Option.
    pub alt: bool,
    /// Control.
    pub ctrl: bool,
    /// Function.
    pub function: bool,
}

impl Modifiers {
    /// Adds the modifier called `name`; `false` if `name` is not one.
    pub fn add(&mut self, name: &str) -> bool {
        match name.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "super" | "meta" | "win" => self.cmd = true,
            "shift" => self.shift = true,
            "alt" | "option" | "opt" => self.alt = true,
            "ctrl" | "control" => self.ctrl = true,
            "fn" => self.function = true,
            _ => return false,
        }
        true
    }

    /// The canonical names of the held modifiers.
    pub fn names(&self) -> Vec<&'static str> {
        [
            (self.cmd, "cmd"),
            (self.shift, "shift"),
            (self.alt, "alt"),
            (self.ctrl, "ctrl"),
            (self.function, "fn"),
        ]
        .into_iter()
        .filter_map(|(held, name)| held.then_some(name))
        .collect()
    }
}

/// One key with its modifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combo {
    /// Canonical key name (`return`, `s`, `=`).
    pub key: String,
    /// Virtual key code.
    pub code: u16,
    /// Held modifiers.
    pub modifiers: Modifiers,
}

impl Combo {
    /// Canonical form, e.g. `cmd+shift+s`.
    pub fn canonical(&self) -> String {
        let mut parts = self.modifiers.names();
        parts.push(&self.key);
        parts.join("+")
    }
}

const CODES: &[(&str, u16)] = &[
    ("a", 0),
    ("s", 1),
    ("d", 2),
    ("f", 3),
    ("h", 4),
    ("g", 5),
    ("z", 6),
    ("x", 7),
    ("c", 8),
    ("v", 9),
    ("b", 11),
    ("q", 12),
    ("w", 13),
    ("e", 14),
    ("r", 15),
    ("y", 16),
    ("t", 17),
    ("1", 18),
    ("2", 19),
    ("3", 20),
    ("4", 21),
    ("6", 22),
    ("5", 23),
    ("=", 24),
    ("9", 25),
    ("7", 26),
    ("-", 27),
    ("8", 28),
    ("0", 29),
    ("]", 30),
    ("o", 31),
    ("u", 32),
    ("[", 33),
    ("i", 34),
    ("p", 35),
    ("return", 36),
    ("l", 37),
    ("j", 38),
    ("'", 39),
    ("k", 40),
    (";", 41),
    ("\\", 42),
    (",", 43),
    ("/", 44),
    ("n", 45),
    ("m", 46),
    (".", 47),
    ("tab", 48),
    ("space", 49),
    ("`", 50),
    ("backspace", 51),
    ("escape", 53),
    ("kp_decimal", 65),
    ("kp_multiply", 67),
    ("kp_add", 69),
    ("kp_divide", 75),
    ("kp_enter", 76),
    ("kp_subtract", 78),
    ("kp_equal", 81),
    ("f5", 96),
    ("f6", 97),
    ("f7", 98),
    ("f3", 99),
    ("f8", 100),
    ("f9", 101),
    ("f11", 103),
    ("f10", 109),
    ("f12", 111),
    ("home", 115),
    ("page_up", 116),
    ("forward_delete", 117),
    ("f4", 118),
    ("end", 119),
    ("f2", 120),
    ("page_down", 121),
    ("f1", 122),
    ("left", 123),
    ("right", 124),
    ("down", 125),
    ("up", 126),
];

/// Other names of keys in [`CODES`].
const ALIASES: &[(&str, &str)] = &[
    ("enter", "return"),
    ("esc", "escape"),
    ("delete", "backspace"),
    ("del", "forward_delete"),
    ("forwarddelete", "forward_delete"),
    ("pageup", "page_up"),
    ("prior", "page_up"),
    ("pagedown", "page_down"),
    ("next", "page_down"),
    ("arrowleft", "left"),
    ("arrowright", "right"),
    ("arrowup", "up"),
    ("arrowdown", "down"),
    ("equal", "="),
    ("minus", "-"),
    ("comma", ","),
    ("period", "."),
    ("slash", "/"),
    ("backslash", "\\"),
    ("semicolon", ";"),
    ("apostrophe", "'"),
    ("grave", "`"),
    ("bracketleft", "["),
    ("bracketright", "]"),
];

/// Shifted characters: the unshifted key plus Shift.
const SHIFTED: &[(&str, &str)] = &[
    ("+", "="),
    ("plus", "="),
    ("*", "8"),
    ("asterisk", "8"),
    ("_", "-"),
    ("underscore", "-"),
    ("?", "/"),
    ("question", "/"),
    (":", ";"),
    ("colon", ";"),
];

/// Parses a combination such as `cmd+shift+s`, `Return` or `cmd++`.
pub fn parse_combo(text: &str) -> Result<Combo> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::BadKeys("no key given".to_owned()));
    }
    let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
    // `cmd++` → ["cmd", "", ""]: the key is `+`.
    if text.ends_with("++") || text == "+" {
        parts.retain(|part| !part.is_empty());
        parts.push("+");
    } else if parts.iter().any(|part| part.is_empty()) {
        return Err(Error::BadKeys(format!("cannot read `{text}`")));
    }
    let mut modifiers = Modifiers::default();
    let mut keys = Vec::new();
    for part in parts {
        if !modifiers.add(part) {
            keys.push(part);
        }
    }
    let [key] = keys.as_slice() else {
        return Err(Error::BadKeys(format!(
            "`{text}` must contain exactly one key besides modifiers (cmd, shift, alt, ctrl)"
        )));
    };
    let mut name = key.to_ascii_lowercase();
    if let Some((_, base)) = SHIFTED.iter().find(|(shifted, _)| *shifted == name) {
        name = (*base).to_owned();
        modifiers.shift = true;
    }
    if let Some((_, canonical)) = ALIASES.iter().find(|(alias, _)| *alias == name) {
        name = (*canonical).to_owned();
    }
    let code = CODES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, code)| *code)
        .ok_or_else(|| Error::BadKeys(format!("unknown key `{key}`")))?;
    Ok(Combo { key: name, code, modifiers })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_are_parsed_with_aliases() {
        let save = parse_combo("Cmd+Shift+S").unwrap();
        assert_eq!(save.canonical(), "cmd+shift+s");
        assert_eq!(save.code, 1);
        assert_eq!(parse_combo("Return").unwrap().code, 36);
        assert_eq!(parse_combo("enter").unwrap().key, "return");
        assert_eq!(parse_combo("Page_Down").unwrap().code, 121);
        assert_eq!(parse_combo("option+ArrowLeft").unwrap().canonical(), "alt+left");
    }

    #[test]
    fn shifted_characters_add_shift() {
        let zoom = parse_combo("cmd++").unwrap();
        assert_eq!(zoom.canonical(), "cmd+shift+=");
        assert_eq!(parse_combo("+").unwrap().canonical(), "shift+=");
        assert_eq!(parse_combo("*").unwrap().code, 28);
    }

    #[test]
    fn malformed_combos_are_refused() {
        assert!(parse_combo("").is_err());
        assert!(parse_combo("cmd+shift").is_err(), "no key");
        assert!(parse_combo("a+b").is_err(), "two keys");
        assert!(parse_combo("cmd+hyper").is_err(), "unknown key");
        assert!(parse_combo("cmd+").is_err());
    }
}

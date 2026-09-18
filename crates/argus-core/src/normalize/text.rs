//! Text cleanup.
//!
//! Screen text is untrusted input. Cleanup makes text comparable across
//! sources (Unicode NFC, collapsed whitespace) and removes characters that are
//! invisible or can disguise content (control characters, zero-width
//! characters, bidirectional overrides).

use unicode_normalization::UnicodeNormalization;

/// Maximum length of names and descriptions, in characters.
pub(crate) const MAX_LABEL_CHARS: usize = 1_000;

/// Maximum length of values, in characters.
pub(crate) const MAX_VALUE_CHARS: usize = 10_000;

/// Marker appended to truncated text.
const ELLIPSIS: char = '…';

/// Cleans a single-line label (name, description).
///
/// Whitespace runs, including line breaks, collapse into one space; the
/// result is trimmed. Empty labels become `None`.
pub(crate) fn clean_label(raw: Option<&str>) -> Option<String> {
    let mut label = String::new();
    let mut pending_space = false;
    for c in raw?.nfc().filter(|c| !is_stripped(*c)) {
        if c.is_whitespace() || c.is_control() {
            pending_space = !label.is_empty();
        } else {
            if pending_space {
                label.push(' ');
                pending_space = false;
            }
            label.push(c);
        }
    }
    (!label.is_empty()).then(|| truncate(label, MAX_LABEL_CHARS))
}

/// Cleans a value (text field contents, ...).
///
/// Line structure is preserved (`\r\n` and `\r` become `\n`); an empty value
/// stays empty, because "empty" is meaningful for a text field.
pub(crate) fn clean_value(raw: Option<&str>) -> Option<String> {
    let raw = raw?.replace("\r\n", "\n").replace('\r', "\n");
    let value: String = raw
        .nfc()
        .filter(|c| !is_stripped(*c))
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    Some(truncate(value, MAX_VALUE_CHARS))
}

/// Invisible or deceptive characters removed from all text.
fn is_stripped(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'                   // soft hyphen
        | '\u{200B}'                 // zero-width space
        | '\u{2060}'                 // word joiner
        | '\u{FEFF}'                 // byte order mark
        | '\u{FFFC}'                 // object replacement (attachments)
        | '\u{202A}'..='\u{202E}'    // bidirectional embeddings and overrides
        | '\u{2066}'..='\u{2069}'    // bidirectional isolates
    )
}

fn truncate(text: String, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((cut, _)) => {
            let mut truncated = text[..cut].to_owned();
            truncated.pop();
            truncated.push(ELLIPSIS);
            truncated
        }
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_collapse_whitespace_and_trim() {
        assert_eq!(clean_label(Some("  Save\n\tas…  ")).as_deref(), Some("Save as…"));
        assert_eq!(clean_label(Some("a\u{00A0}\u{00A0}b")).as_deref(), Some("a b"));
    }

    #[test]
    fn empty_labels_are_absent() {
        assert_eq!(clean_label(Some("   \n ")), None);
        assert_eq!(clean_label(Some("\u{200B}")), None);
        assert_eq!(clean_label(None), None);
    }

    #[test]
    fn text_is_nfc_normalized() {
        // "é" decomposed (as in macOS file names) and composed must match.
        assert_eq!(clean_label(Some("Re\u{0301}sume\u{0301}")), clean_label(Some("Résumé")));
        assert_eq!(clean_value(Some("e\u{0301}")).as_deref(), Some("é"));
    }

    #[test]
    fn deceptive_characters_are_removed() {
        // A right-to-left override could make "exe.txt" render as "txt.exe".
        assert_eq!(clean_label(Some("report\u{202E}txt.exe")).as_deref(), Some("reporttxt.exe"));
        assert_eq!(clean_label(Some("pay\u{200B}pal")).as_deref(), Some("paypal"));
        assert_eq!(clean_label(Some("\u{FEFF}Title\u{0007}")).as_deref(), Some("Title"));
    }

    #[test]
    fn emoji_sequences_survive() {
        let family = "👩\u{200D}👩\u{200D}👧";
        assert_eq!(clean_label(Some(family)).as_deref(), Some(family));
    }

    #[test]
    fn values_keep_lines_and_emptiness() {
        assert_eq!(clean_value(Some("a\r\nb\rc\td")).as_deref(), Some("a\nb\nc\td"));
        assert_eq!(clean_value(Some("")).as_deref(), Some(""));
        assert_eq!(clean_value(Some("x\u{0000}y")).as_deref(), Some("xy"));
    }

    #[test]
    fn long_text_is_truncated_with_marker() {
        let label = clean_label(Some(&"a".repeat(MAX_LABEL_CHARS + 50))).unwrap();
        assert_eq!(label.chars().count(), MAX_LABEL_CHARS);
        assert!(label.ends_with(ELLIPSIS));

        let exact = "b".repeat(MAX_LABEL_CHARS);
        assert_eq!(clean_label(Some(&exact)).unwrap(), exact);
    }
}

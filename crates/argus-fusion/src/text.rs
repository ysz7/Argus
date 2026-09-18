//! Tolerant comparison of recognized text with text reported by other
//! sources.

/// Whether two texts plausibly say the same thing, allowing for OCR errors,
/// case, punctuation and truncation (one text contained in the other).
pub(crate) fn similar(a: &str, b: &str) -> bool {
    let (a_folded, b_folded) = (fold(a), fold(b));
    if a_folded.is_empty() || b_folded.is_empty() {
        // Symbols only ("+", ","): compare as written.
        return a_folded.is_empty() && b_folded.is_empty() && a.trim() == b.trim();
    }
    if a_folded == b_folded {
        return true;
    }
    let (short, long) = if a_folded.len() <= b_folded.len() {
        (&a_folded, &b_folded)
    } else {
        (&b_folded, &a_folded)
    };
    if short.len() >= 3 && long.windows(short.len()).any(|window| window == short.as_slice()) {
        return true;
    }
    // At most one edit in five characters.
    const MAX_COMPARED: usize = 256;
    long.len() <= MAX_COMPARED && edit_distance(short, long) * 5 <= long.len()
}

/// Lowercase letters and digits only.
fn fold(text: &str) -> Vec<char> {
    text.chars().flat_map(char::to_lowercase).filter(|c| c.is_alphanumeric()).collect()
}

/// Levenshtein distance.
fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerates_recognition_noise() {
        assert!(similar("Save", "save"));
        assert!(similar("Save As…", "Save As..."));
        assert!(similar("Smart links", "Smart Iinks"), "one OCR error");
        assert!(similar("Show ruler", "Show ruler when opening documents"), "truncation");
        assert!(similar("+", " + "));
        assert!(!similar(",", "Point"));
        assert!(!similar("Save", "Cancel"));
        assert!(!similar("1", "7"), "short texts must match exactly");
        assert!(!similar("", "Save"));
    }

    #[test]
    fn measures_edits() {
        let chars = |text: &str| text.chars().collect::<Vec<_>>();
        assert_eq!(edit_distance(&chars("kitten"), &chars("sitting")), 3);
        assert_eq!(edit_distance(&chars(""), &chars("abc")), 3);
    }
}

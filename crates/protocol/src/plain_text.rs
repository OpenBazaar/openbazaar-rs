//! Refuse public text a client cannot lay out safely.
//!
//! A record is replicated to every node and shown in other people's
//! interfaces, so its text must not carry control characters or bidirectional
//! overrides that reorder the text around it. Unlike the checks in
//! `public_text`, this one runs when a record is verified as well as when it
//! is signed, so a node drops a hostile record instead of passing it on.
//! Zero-width joiners and variation selectors pass: emoji are built from them.

/// True when `text` is free of control and direction-override characters.
/// `multiline` lets a description keep tabs and line breaks.
pub fn is_plain(text: &str, multiline: bool) -> bool {
    !text.chars().any(|ch| match ch {
        '\t' | '\n' | '\r' => !multiline,
        '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' => true,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => true,
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::is_plain;

    #[test]
    fn ordinary_text_and_emoji_pass() {
        assert!(is_plain("Wool blankets", false));
        assert!(is_plain("Café 日本茶 👨‍👩‍👧 ❤️", false));
        assert!(is_plain("Line one.\nLine two.\tTabbed.", true));
    }

    #[test]
    fn controls_and_direction_overrides_are_refused() {
        for text in ["abc\u{202e}exe", "a\u{2066}b", "nul\u{0}", "bell\u{7}", "csi\u{9b}", "sep\u{2028}", "rlm\u{200f}"] {
            assert!(!is_plain(text, true), "{text:?}");
        }
        assert!(!is_plain("Books\nBUY NOW", false));
    }
}

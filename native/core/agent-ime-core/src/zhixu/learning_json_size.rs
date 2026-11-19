pub(super) fn json_string_bytes(value: &str) -> usize {
    let mut total: usize = 2;
    for ch in value.chars() {
        let encoded = match ch {
            '"' | '\\' => 2,
            '\u{0008}' | '\u{000C}' | '\u{000A}' | '\u{000D}' | '\u{0009}' => 2,
            c if (c as u32) < 0x20 => 6,
            c => c.len_utf8(),
        };
        total = total.saturating_add(encoded);
    }
    total
}

#[cfg(test)]
mod tests {
    use super::json_string_bytes;

    #[test]
    fn empty_and_plain_ascii() {
        assert_eq!(json_string_bytes(""), 2);
        assert_eq!(json_string_bytes("hello"), 7);
        assert_eq!(json_string_bytes("abc 123!"), 10);
    }

    #[test]
    fn quote_and_backslash_are_two_byte_escapes() {
        assert_eq!(json_string_bytes("\""), 4);
        assert_eq!(json_string_bytes("\\"), 4);
        assert_eq!(json_string_bytes("a\"b\\c"), 9);
    }

    #[test]
    fn five_short_control_escapes() {
        assert_eq!(json_string_bytes("\u{0008}"), 4);
        assert_eq!(json_string_bytes("\u{000C}"), 4);
        assert_eq!(json_string_bytes("\u{000A}"), 4);
        assert_eq!(json_string_bytes("\u{000D}"), 4);
        assert_eq!(json_string_bytes("\u{0009}"), 4);
        assert_eq!(json_string_bytes("\u{0009}\u{000A}"), 6);
    }

    #[test]
    fn other_c0_controls_are_six_byte_unicode_escapes() {
        assert_eq!(json_string_bytes("\u{0000}"), 8);
        assert_eq!(json_string_bytes("\u{0001}"), 8);
        assert_eq!(json_string_bytes("\u{001F}"), 8);
        assert_eq!(json_string_bytes("\u{0007}\u{001B}"), 14);
    }

    #[test]
    fn cjk_preserves_utf8_length() {
        assert_eq!(json_string_bytes("中"), 5);
        assert_eq!(json_string_bytes("中文"), 8);
    }

    #[test]
    fn emoji_preserves_utf8_length() {
        assert_eq!(json_string_bytes("😀"), 6);
        assert_eq!(json_string_bytes("a😀b"), 8);
    }

    #[test]
    fn forward_slash_is_not_escaped() {
        assert_eq!(json_string_bytes("a/b"), 5);
    }

    #[test]
    fn mixed_unicode() {
        assert_eq!(json_string_bytes("a\n中😀\t\""), 16);
        assert_eq!(json_string_bytes("\u{0000}中/\\😀"), 18);
    }
}

//! Small allocation-free text predicates shared across the hot parse paths.

/// ASCII case-insensitive substring test — the allocation-free equivalent of
/// `haystack_lower.contains(needle_lower)` when both are ASCII.
///
/// Callers pass an already-upper/lower-cased `needle` and get the same boolean
/// `haystack.to_ascii_uppercase().contains(NEEDLE)` would, without the
/// throwaway per-call `String`. Used by the search name-match classifier and by
/// `ContactService::parse_vcard`'s per-line `TYPE=` routing
/// (benches/ROUND20.md §A3).
pub fn ascii_ci_contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_uppercase_contains() {
        // Parity with the `to_ascii_uppercase().contains(NEEDLE)` shape it
        // replaced, across mixed case and the empty/oversize edge cases.
        let cases: &[(&str, &str)] = &[
            ("EMAIL;TYPE=home:a@b.com", "TYPE=HOME"),
            ("EMAIL;type=Work:a@b.com", "TYPE=WORK"),
            ("TEL;TYPE=CELL:+1", "TYPE=CELL"),
            ("TEL;TYPE=voice:+1", "TYPE=CELL"),
            ("ADR;TYPE=Home:;;x", "TYPE=WORK"),
            ("", "TYPE=HOME"),
            ("short", "a-very-long-needle"),
        ];
        for (hay, needle) in cases {
            let reference = hay.to_ascii_uppercase().contains(needle);
            assert_eq!(
                ascii_ci_contains(hay.as_bytes(), needle.as_bytes()),
                reference,
                "mismatch for haystack={hay:?} needle={needle:?}"
            );
        }
    }

    #[test]
    fn empty_needle_is_true() {
        assert!(ascii_ci_contains(b"anything", b""));
    }
}

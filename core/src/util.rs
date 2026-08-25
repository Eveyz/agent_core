//! Shared utility functions used across the crate.

/// Find the largest valid UTF-8 character boundary at or before `idx`.
/// Prevents panics when slicing strings at arbitrary byte offsets.
pub fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Longest prefix of `s` whose byte length is `<= max_bytes` and which is
/// still valid UTF-8. Safe to call with any cap, including 0 and values
/// that land inside a multi-byte character.
pub fn utf8_prefix(s: &str, max_bytes: usize) -> &str {
    &s[..floor_char_boundary(s, s.len().min(max_bytes))]
}

/// Expand a tilde (`~`) path to an absolute path using the user's home
/// directory. Falls back to the original path if HOME/USERPROFILE is
/// unavailable.
pub fn expand_tilde(path: &str) -> String {
    if path.starts_with('~') {
        if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
            if path == "~" {
                return home;
            }
            if path.starts_with("~/") {
                return format!("{}/{}", home, &path[2..]);
            }
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2-byte, 3-byte, and 4-byte code points used to probe mid-character caps.
    const MULTIBYTE: &[char] = &['é', 'ß', '金', '你', '😀', '🧬'];

    #[test]
    fn floor_char_boundary_always_lands_on_a_char() {
        let samples = ["", "abc", "é", "金", "😀", "a金b😀céß"];
        for s in samples {
            for idx in 0..=s.len() + 8 {
                let end = floor_char_boundary(s, idx);
                assert!(
                    s.is_char_boundary(end),
                    "s={s:?} idx={idx} end={end} is not a char boundary"
                );
                assert!(end <= s.len());
                if idx >= s.len() {
                    assert_eq!(end, s.len());
                } else {
                    assert!(end <= idx);
                }
            }
        }
    }

    #[test]
    fn floor_char_boundary_backs_up_inside_every_multibyte_char() {
        for &ch in MULTIBYTE {
            let s = ch.to_string();
            assert!(s.len() > 1, "{ch:?} should be multibyte");
            for interior in 1..s.len() {
                assert!(!s.is_char_boundary(interior));
                assert_eq!(floor_char_boundary(&s, interior), 0);
            }
            assert_eq!(floor_char_boundary(&s, s.len()), s.len());
        }
    }

    #[test]
    fn utf8_prefix_never_splits_a_char_at_any_cap() {
        let samples = ["", "a", "abc", "é", "金", "😀", "hello 金 world 😀 ß"];
        for s in samples {
            for max in 0..=s.len() + 8 {
                let prefix = utf8_prefix(s, max);
                assert!(s.starts_with(prefix), "s={s:?} max={max}");
                assert!(
                    prefix.len() <= max.min(s.len()),
                    "s={s:?} max={max} prefix_len={}",
                    prefix.len()
                );
                assert!(s.is_char_boundary(prefix.len()));
            }
        }
    }

    #[test]
    fn utf8_prefix_drops_a_char_that_would_cross_the_cap() {
        for &ch in MULTIBYTE {
            let s = format!("x{ch}y");
            // Cap landing on the second byte of `ch` (index 2 if ch starts at 1).
            let ch_start = 1;
            let interior = ch_start + 1;
            assert!(!s.is_char_boundary(interior));
            let prefix = utf8_prefix(&s, interior);
            assert_eq!(prefix, "x");
        }
    }
}

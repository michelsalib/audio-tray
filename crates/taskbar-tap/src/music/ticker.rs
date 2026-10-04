//! Scrolling long titles: a character window over a wrapped string, advanced per sweep and
//! written with `put_Text` (no rebuild, no `Storyboard`, no text measurement needed).

/// Marks the seam where the text wraps around to its start.
const SEPARATOR: &str = "   •   ";

/// The visible slice of `text`, `width` characters wide, starting `offset` characters in. Text
/// that fits is returned trimmed, without the separator. Counts `char`s, not bytes.
pub fn window(text: &str, width: usize, offset: usize) -> String {
    let trimmed = text.trim();
    let count = trimmed.chars().count();
    if count <= width {
        return trimmed.to_string();
    }

    // The wrapped sequence is text + separator, repeated; the window can straddle the seam.
    let wrapped: Vec<char> = trimmed.chars().chain(SEPARATOR.chars()).collect();
    let period = wrapped.len();
    let start = offset % period;
    (0..width)
        .map(|i| wrapped[(start + i) % period])
        .collect()
}

/// Whether `text` needs scrolling at all.
pub fn scrolls(text: &str, width: usize) -> bool {
    text.trim().chars().count() > width
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_untouched_and_gains_no_separator() {
        assert_eq!(window("Narctis", 15, 0), "Narctis");
        assert_eq!(window("Narctis", 15, 7), "Narctis");
        assert!(!scrolls("Narctis", 15));
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        assert_eq!(window("  Narctis  ", 15, 0), "Narctis");
    }

    #[test]
    fn long_text_yields_a_window_of_exactly_the_requested_width() {
        let artist = "Lifeformed et Janice Kwan";
        assert!(scrolls(artist, 18));
        for offset in 0..40 {
            assert_eq!(window(artist, 18, offset).chars().count(), 18, "offset {offset}");
        }
    }

    #[test]
    fn the_window_advances_by_one_character_per_offset() {
        let text = "abcdefghijklmnop";
        assert_eq!(window(text, 5, 0), "abcde");
        assert_eq!(window(text, 5, 1), "bcdef");
        assert_eq!(window(text, 5, 2), "cdefg");
    }

    #[test]
    fn it_wraps_around_through_the_separator_and_repeats() {
        let text = "abcdefghij";
        let period = text.chars().count() + SEPARATOR.chars().count();
        // A full period returns to the start, so the scroll is seamless rather than jumping.
        assert_eq!(window(text, 5, 0), window(text, 5, period));
        assert_eq!(window(text, 5, 3), window(text, 5, period + 3));
    }

    #[test]
    fn multibyte_characters_are_never_split() {
        // Byte slicing would panic here; char windowing must not.
        let text = "Björk — Jóga með hljómsveit";
        assert!(scrolls(text, 10));
        for offset in 0..30 {
            assert_eq!(window(text, 10, offset).chars().count(), 10);
        }
    }

    #[test]
    fn an_empty_string_stays_empty() {
        assert_eq!(window("", 15, 0), "");
        assert!(!scrolls("", 15));
    }

    #[test]
    fn text_exactly_at_the_limit_does_not_scroll() {
        let text = "abcdefghijklmno"; // 15
        assert!(!scrolls(text, 15));
        assert_eq!(window(text, 15, 5), text);
    }
}

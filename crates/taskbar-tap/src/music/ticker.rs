//! Scrolling long titles horizontally.
//!
//! Two constraints shape this, and together they rule out the obvious approaches:
//!
//! * **The strip must not be rebuilt to animate.** Re-running `XamlReader.Load` builds new
//!   elements, which drops the click handlers and breaks the buttons (measured — press and
//!   release land on different objects). So scrolling is a `put_Text` on an existing
//!   `TextBlock`, never a new subtree.
//! * **No XAML storyboard.** A `Storyboard` would be the idiomatic answer, but UWP XAML has
//!   no declarative event triggers to start one, so it would have to be found and `Begin()`d
//!   through yet another hand-rolled interface. Advancing a character per tick on the sweep
//!   that already exists is far less machinery for the same effect.
//!
//! The scroll is a **character window over a wrapped string** — the classic ticker — rather
//! than a pixel offset, because a pixel offset needs a transform to animate and text
//! measurement to know when to stop.

/// What separates the end of the text from its start as it wraps around.
///
/// Without it a wrapping title reads as one run-on word; the bullet makes the seam obvious
/// and gives the eye a rest point.
const SEPARATOR: &str = "   •   ";

/// The visible slice of `text`, `width` characters wide, starting `offset` characters in.
///
/// Text that already fits is returned untouched — importantly *without* the separator, so a
/// short title never grows a stray bullet.
///
/// Counts `char`s, not bytes: song titles are full of accents and CJK, and slicing a `String`
/// by byte index would panic mid-character.
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

//! What the strip should show right now, as published by audio-tray.
//!
//! The TAP runs inside `explorer.exe` and cannot call into the app, so the now-playing
//! state arrives as a small file that audio-tray rewrites on every change. A file rather
//! than `WM_COPYDATA` for two reasons: the cover art has to reach XAML as an `Image`
//! source, and the only way to give XAML a bitmap it did not create is a path — so there
//! is a file in play regardless, and one mechanism is better than two.
//!
//! Written atomically by the app (temp file then rename), so a half-written state is never
//! read. Both sides must agree on [`STATE_FILE`] and on the key names.

/// Where the app publishes the state. **Must match `music::publish::STATE_FILE` in audio-tray.**
pub const STATE_FILE: &str = "audio-tray-music.txt";

pub fn state_path() -> std::path::PathBuf {
    std::env::temp_dir().join(STATE_FILE)
}

/// Playback state, reduced to what the strip draws.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Playback {
    #[default]
    Stopped,
    Playing,
    Paused,
}

impl Playback {
    fn parse(text: &str) -> Self {
        match text {
            "playing" => Self::Playing,
            "paused" => Self::Paused,
            _ => Self::Stopped,
        }
    }

    /// Segoe Fluent codepoint for the play/pause button: showing what the *click* will do,
    /// which is the convention every media control follows — a pause glyph while playing.
    pub fn toggle_glyph(self) -> &'static str {
        match self {
            Self::Playing => "\u{E769}", // pause
            _ => "\u{E768}",             // play
        }
    }
}

/// The strip's contents.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Strip {
    pub title: String,
    pub artist: String,
    pub playback: Playback,
    /// Absolute path to a cover image, if there is one.
    ///
    /// The app writes a **new filename per cover**, because `BitmapImage` caches by URI:
    /// rewriting the same path leaves the previous cover on screen.
    pub cover: Option<String>,
}

impl Strip {
    /// Whether there is anything worth drawing.
    pub fn has_track(&self) -> bool {
        !self.title.trim().is_empty()
    }

    /// The title to draw, substituting an idle label when nothing is playing.
    ///
    /// **The strip never stands down to the weather while audio-tray is running.** A gap in the
    /// feed is routine — YouTube Music reports no track for a moment while it buffers the next
    /// one — and handing the slot back for that produced a visible flip through the weather UI
    /// on every song change. Showing an idle label instead also removes any need for a
    /// keep-the-last-song timeout, which would have been a guess about how long a gap lasts.
    pub fn display_title(&self) -> &str {
        if self.has_track() {
            self.title.trim()
        } else {
            "Nothing playing"
        }
    }

    /// The artist to draw; blank while idle, so the idle state reads as one line.
    pub fn display_artist(&self) -> &str {
        if self.has_track() {
            self.artist.trim()
        } else {
            "YouTube Music"
        }
    }

    /// Read the published state, or `None` if the app has not written one yet.
    pub fn read() -> Option<Self> {
        let text = std::fs::read_to_string(state_path()).ok()?;
        Some(Self::parse(&text))
    }

    pub fn parse(text: &str) -> Self {
        let mut strip = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key {
                "title" => strip.title = value.to_string(),
                "artist" => strip.artist = value.to_string(),
                "status" => strip.playback = Playback::parse(value),
                // An empty value means "no cover", which is a normal state — plenty of
                // sessions publish no artwork at all.
                "cover" => {
                    strip.cover = (!value.trim().is_empty()).then(|| value.to_string());
                }
                _ => {}
            }
        }
        strip
    }
}

/// Escape text for inclusion in XAML markup.
///
/// Not optional: track and artist names really do contain `&` and quotes, and an
/// unescaped one fails the whole `XamlReader.Load` — which shows up as the strip
/// silently vanishing for one song and coming back for the next.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_state() {
        let strip = Strip::parse(
            "title=Love and Hold Nothing Back\nartist=Yvette Young\nstatus=playing\ncover=C:\\t\\c1.png\n",
        );
        assert_eq!(strip.title, "Love and Hold Nothing Back");
        assert_eq!(strip.artist, "Yvette Young");
        assert_eq!(strip.playback, Playback::Playing);
        assert_eq!(strip.cover.as_deref(), Some("C:\\t\\c1.png"));
        assert!(strip.has_track());
    }

    #[test]
    fn an_empty_cover_means_none() {
        let strip = Strip::parse("title=x\ncover=\n");
        assert_eq!(strip.cover, None);
    }

    #[test]
    fn unknown_keys_are_ignored_so_the_format_can_grow() {
        let strip = Strip::parse("title=x\nalbum=y\nfuture=z\n");
        assert_eq!(strip.title, "x");
    }

    #[test]
    fn a_title_with_an_equals_sign_survives() {
        // `split_once` keeps everything after the first `=`, which matters for real titles.
        assert_eq!(Strip::parse("title=a=b").title, "a=b");
    }

    #[test]
    fn no_title_means_nothing_to_draw() {
        assert!(!Strip::parse("artist=someone").has_track());
    }

    #[test]
    fn markup_special_characters_are_escaped() {
        assert_eq!(escape("Sturm & Drang"), "Sturm &amp; Drang");
        assert_eq!(escape("<tag>"), "&lt;tag&gt;");
        assert_eq!(escape("say \"hi\""), "say &quot;hi&quot;");
    }

    #[test]
    fn the_toggle_glyph_shows_what_the_click_will_do() {
        assert_eq!(Playback::Playing.toggle_glyph(), "\u{E769}"); // pause
        assert_eq!(Playback::Paused.toggle_glyph(), "\u{E768}"); // play
    }
}

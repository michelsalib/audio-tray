//! What the strip should show right now, as published by audio-tray.
//!
//! The state arrives as a `key=value` file audio-tray rewrites atomically (temp + rename) on
//! every change; a file because the cover art must reach XAML as a path anyway. Both sides must
//! agree on [`STATE_FILE`] and the key names.

pub use tap_proto::MUSIC_STATE_FILE as STATE_FILE;

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
}

/// The strip's contents.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Strip {
    pub title: String,
    pub artist: String,
    pub playback: Playback,
    /// Absolute path to a cover image. A new filename per cover: `BitmapImage` caches by URI.
    pub cover: Option<String>,
}

impl Strip {
    /// Whether there is anything worth drawing.
    pub fn has_track(&self) -> bool {
        !self.title.trim().is_empty()
    }

    /// The title to draw, or an idle label when there is no track (gaps between songs are routine,
    /// so the tile stays up rather than handing the button back).
    pub fn display_title(&self) -> &str {
        if self.has_track() {
            self.title.trim()
        } else {
            "Nothing playing"
        }
    }

    /// The artist to draw, or the player's name while idle.
    pub fn display_artist(&self) -> &str {
        if self.has_track() {
            self.artist.trim()
        } else {
            "YouTube Music"
        }
    }

    /// Read the published state, or `None` if the app has not written one yet. Cached on mtime
    /// and length so an unchanged file costs only a stat on the shell's UI thread.
    pub fn read() -> Option<Self> {
        use std::sync::Mutex;
        static CACHED: Mutex<Option<(u64, u64, Strip)>> = Mutex::new(None);

        let path = state_path();
        let stamp = std::fs::metadata(&path).ok().and_then(|meta| {
            let modified = meta
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?;
            Some((modified.as_nanos() as u64, meta.len()))
        });

        let mut cache = crate::lock(&CACHED);
        if let (Some((mtime, len)), Some((known_mtime, known_len, strip))) = (stamp, cache.as_ref())
        {
            if mtime == *known_mtime && len == *known_len {
                return Some(strip.clone());
            }
        }

        let text = std::fs::read_to_string(&path).ok()?;
        let strip = Self::parse(&text);
        *cache = stamp.map(|(mtime, len)| (mtime, len, strip.clone()));
        Some(strip)
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
                // Empty means no cover (common).
                "cover" => {
                    strip.cover = (!value.trim().is_empty()).then(|| value.to_string());
                }
                _ => {}
            }
        }
        strip
    }
}

/// Escape text for XAML markup; an unescaped `&` or quote fails the whole `XamlReader.Load`.
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
}

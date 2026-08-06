//! Publishing what the strip should show.
//!
//! The TAP lives in `explorer.exe` and cannot call into this process, so the now-playing
//! state is handed over as a small file it re-reads. A file rather than a message because
//! the cover art has to reach XAML as an image *source*, and the only way to give XAML a
//! bitmap it did not create is a path — so a file is in play regardless, and one mechanism
//! beats two.
//!
//! Written atomically (temp file, then rename over the target) so the TAP never reads a
//! half-written state.

use std::io::Write;

use anyhow::{Context, Result};

use crate::music::feed::{PlaybackStatus, State};

/// Where the state is published. **Must match `state::STATE_FILE` in the TAP.**
const STATE_FILE: &str = "audio-tray-music.txt";

/// Cover files are written as `audio-tray-cover-<n>.png`.
///
/// A new name per cover, not one reused name: XAML's `BitmapImage` caches by URI, so
/// rewriting the same path leaves the *previous* cover on screen — the track changes and
/// the art does not.
const COVER_PREFIX: &str = "audio-tray-cover-";

pub struct Publisher {
    /// The last state written, so an unchanged snapshot costs no disk write. The TAP
    /// rebuilds its XAML subtree whenever this file changes, and rebuilding destroys the
    /// elements under the cursor — so writing needlessly would break clicking.
    last: Option<String>,
    /// Bumped per cover to defeat `BitmapImage`'s URI cache.
    cover_generation: u64,
    /// The cover file currently referenced, deleted once a newer one replaces it.
    current_cover: Option<std::path::PathBuf>,
    /// Content fingerprint of the cover on disk, so identical artwork is not rewritten.
    ///
    /// Compared by content rather than by track, because consecutive tracks from one album
    /// carry the same art — and a rewrite would change the URI, forcing the TAP to rebuild
    /// the strip and dropping the click handlers for no reason.
    current_cover_fingerprint: Option<(usize, u64)>,
}

impl Publisher {
    pub fn new() -> Self {
        Self {
            last: None,
            cover_generation: 0,
            current_cover: None,
            current_cover_fingerprint: None,
        }
    }

    /// Write `state` out, if it differs from what was last written.
    ///
    /// Returns whether anything was written.
    pub fn publish(&mut self, state: &State) -> Result<bool> {
        let (title, artist, status, cover) = match state {
            State::Absent => (String::new(), String::new(), "stopped", None),
            State::Track(snapshot) => (
                sanitise(&snapshot.title),
                sanitise(&snapshot.artist),
                match snapshot.status {
                    PlaybackStatus::Playing => "playing",
                    PlaybackStatus::Paused => "paused",
                    _ => "stopped",
                },
                snapshot.cover.as_deref(),
            ),
        };

        // The cover is compared by *content*, because a track change can reuse artwork
        // (same album) and rewriting it would force a needless rebuild.
        let cover_changed = cover.map(fingerprint) != self.current_cover_fingerprint;
        let cover_path = if cover_changed {
            self.write_cover(cover)?
        } else {
            self.current_cover
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
        };
        if cover_changed {
            self.current_cover_fingerprint = cover.map(fingerprint);
        }

        let body = format!(
            "title={title}\nartist={artist}\nstatus={status}\ncover={}\n",
            cover_path.as_deref().unwrap_or("")
        );
        if self.last.as_deref() == Some(body.as_str()) {
            return Ok(false);
        }

        write_atomically(&state_path(), body.as_bytes()).context("publishing the strip state")?;
        self.last = Some(body);
        Ok(true)
    }

    /// Remove the published state, so the TAP hands the Widgets button back.
    pub fn clear(&mut self) {
        let _ = std::fs::remove_file(state_path());
        if let Some(cover) = self.current_cover.take() {
            let _ = std::fs::remove_file(cover);
        }
        self.last = None;
        self.current_cover_fingerprint = None;
    }

    /// Write new cover bytes to a fresh filename and drop the previous one.
    fn write_cover(&mut self, cover: Option<&[u8]>) -> Result<Option<String>> {
        let previous = self.current_cover.take();
        let path = match cover {
            Some(bytes) => {
                self.cover_generation += 1;
                let path = std::env::temp_dir()
                    .join(format!("{COVER_PREFIX}{}.png", self.cover_generation));
                write_atomically(&path, bytes).context("writing the cover art")?;
                let display = path.to_string_lossy().into_owned();
                self.current_cover = Some(path);
                Some(display)
            }
            None => None,
        };
        // Only after the new one is in place: the TAP may still be rendering the old path.
        if let Some(previous) = previous {
            let _ = std::fs::remove_file(previous);
        }
        Ok(path)
    }
}

fn state_path() -> std::path::PathBuf {
    std::env::temp_dir().join(STATE_FILE)
}

/// Cheap content fingerprint — length plus a sum, which is ample to notice a different
/// cover and costs nothing next to hashing 25 KB four times a second.
fn fingerprint(bytes: &[u8]) -> (usize, u64) {
    (bytes.len(), bytes.iter().map(|b| u64::from(*b)).sum())
}

/// Strip anything that would corrupt the line-based format.
///
/// Newlines are the only real hazard — a title containing one would be read as a second
/// key — and they do not occur in practice, but the format must not depend on that.
fn sanitise(text: &str) -> String {
    text.replace(['\r', '\n'], " ").trim().to_string()
}

/// Write via a temp file and rename, so a reader never sees a partial file.
fn write_atomically(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension("tmp");
    {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.flush()?;
    }
    // `rename` over an existing file fails on Windows; `fs::rename` maps to MoveFileEx
    // with replace semantics in std, so this is safe.
    std::fs::rename(&temp, path)?;
    Ok(())
}

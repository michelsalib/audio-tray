//! Publishing what the strip should show, as a small file the TAP re-reads (the cover must reach
//! XAML as a path anyway, so one file mechanism serves both). Written atomically so the TAP never
//! reads a half-written state; key names are shared with the TAP's `music::state`.

use std::io::Write;

use anyhow::{Context, Result};

use crate::music::feed::{PlaybackStatus, State};

use tap_proto::MUSIC_STATE_FILE as STATE_FILE;

/// Cover files are `audio-tray-cover-<pid>-<n>.png`: a fresh name per cover because `BitmapImage`
/// caches by URI, and the pid because Explorer outlives us and keeps the previous run's names cached.
const COVER_PREFIX: &str = "audio-tray-cover-";

pub struct Publisher {
    /// The last state written, so an unchanged snapshot costs no write (or TAP re-read).
    last: Option<String>,
    /// Bumped per cover to defeat `BitmapImage`'s URI cache.
    cover_generation: u64,
    /// The cover file currently referenced, deleted once a newer one replaces it.
    current_cover: Option<std::path::PathBuf>,
    /// Content fingerprint of the cover on disk: same-album tracks share art, and a rewrite would
    /// change the URI and make the TAP rebuild the cover for nothing.
    current_cover_fingerprint: Option<(usize, u64)>,
}

impl Publisher {
    pub fn new() -> Self {
        sweep_orphaned_covers();
        Self {
            last: None,
            cover_generation: 0,
            current_cover: None,
            current_cover_fingerprint: None,
        }
    }

    /// Write `state` out if it differs from what was last written; returns whether it wrote.
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

    /// Remove the published state and current cover, so the TAP has nothing new to draw.
    pub fn clear(&mut self) {
        let _ = std::fs::remove_file(state_path());
        if let Some(cover) = self.current_cover.take() {
            let _ = std::fs::remove_file(cover);
        }
        self.last = None;
        self.current_cover_fingerprint = None;
    }

    /// Write new cover bytes to a fresh filename and drop the previous one. On failure the previous
    /// cover stays current and the next poll retries.
    fn write_cover(&mut self, cover: Option<&[u8]>) -> Result<Option<String>> {
        let next = match cover {
            Some(bytes) => {
                self.cover_generation += 1;
                let path = std::env::temp_dir().join(format!(
                    "{COVER_PREFIX}{}-{}.png",
                    std::process::id(),
                    self.cover_generation
                ));
                write_atomically(&path, bytes).context("writing the cover art")?;
                Some(path)
            }
            None => None,
        };
        let display = next.as_ref().map(|path| path.to_string_lossy().into_owned());
        // Only after the new one is in place: the TAP may still be rendering the old path.
        if let Some(previous) = std::mem::replace(&mut self.current_cover, next) {
            let _ = std::fs::remove_file(previous);
        }
        Ok(display)
    }
}

fn state_path() -> std::path::PathBuf {
    std::env::temp_dir().join(STATE_FILE)
}

/// Delete cover files left in `%TEMP%` by killed runs (clean runs delete their own). Files with our
/// own pid are left alone.
fn sweep_orphaned_covers() {
    let ours = format!("{COVER_PREFIX}{}-", std::process::id());
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(COVER_PREFIX) && !name.starts_with(&ours) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Cheap content fingerprint (length plus byte sum): ample to notice a different cover.
fn fingerprint(bytes: &[u8]) -> (usize, u64) {
    (bytes.len(), bytes.iter().map(|b| u64::from(*b)).sum())
}

/// Strip newlines, which would corrupt the line-based format.
fn sanitise(text: &str) -> String {
    text.replace(['\r', '\n'], " ").trim().to_string()
}

/// Write via a temp file and rename, so a reader never sees a partial file.
fn write_atomically(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension("tmp");
    let written = std::fs::File::create(&temp).and_then(|mut file| {
        file.write_all(bytes)?;
        file.flush()
    });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(e.into());
    }
    // std's `fs::rename` uses MoveFileEx with replace semantics, so this overwrites on Windows.
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })?;
    Ok(())
}

/// Remove the published state file, from any thread — the fallback when the feed thread cannot
/// run its own [`Publisher::clear`]. The cover file is left for the next start's sweep.
pub fn remove_published() {
    let _ = std::fs::remove_file(state_path());
}

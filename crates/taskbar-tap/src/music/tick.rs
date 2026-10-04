//! Advancing the scrolling title/artist text, and the [`Segment`] transport codes.
//!
//! Scrolling is `put_Text` on the existing `TextBlock`s, never a rebuild of the tile.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::xamlom::IXamlDiagnostics;

use super::{layout, state, ticker};

/// Sweeps per one-character advance: about a character a second at the 250 ms sweep.
const SWEEPS_PER_CHARACTER: u32 = 4;

static SWEEP: AtomicU32 = AtomicU32::new(0);

/// Start the scroll over from the first character — for a new title or a freshly placed strip.
pub fn restart() {
    SWEEP.store(0, Ordering::SeqCst);
}

/// Move the ticker on one step, if anything needs it.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn scroll(diagnostics: &IXamlDiagnostics, strip: &state::Strip) {
    let step = SWEEP.fetch_add(1, Ordering::SeqCst) / SWEEPS_PER_CHARACTER;
    let l = layout::layout();
    for (name, full, width) in [
        ("MusicTileTitle", strip.display_title(), l.title_chars),
        ("MusicTileArtist", strip.display_artist(), l.artist_chars),
    ] {
        // Text that fits was written by the markup; leave it alone.
        if !ticker::scrolls(full, width) {
            continue;
        }
        let text = ticker::window(full, width, step as usize);
        for node in crate::tree::find_by_name(name) {
            crate::decorate::set_text(diagnostics, node, &text);
        }
    }
}

/// Which transport control was hit (wired by [`super::thumbbar`]). The strip body is deliberately
/// absent: its clicks stay the shell's (activate, minimise, drag).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Segment {
    Previous,
    PlayPause,
    Next,
}

impl Segment {
    pub fn label(self) -> &'static str {
        match self {
            Self::Previous => "previous",
            Self::PlayPause => "play/pause",
            Self::Next => "next",
        }
    }

    /// Wire code for the message posted to audio-tray (`tap_proto::ACTION_*`).
    pub fn code(self) -> usize {
        match self {
            Self::Previous => tap_proto::ACTION_MUSIC_PREVIOUS,
            Self::PlayPause => tap_proto::ACTION_MUSIC_PLAY_PAUSE,
            Self::Next => tap_proto::ACTION_MUSIC_NEXT,
        }
    }
}

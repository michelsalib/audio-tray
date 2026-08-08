//! Advancing the scrolling title, and wiring the transport glyphs.
//!
//! **Both are property writes on existing elements, never a rebuild.** Rebuilding the strip to show
//! the next window of a long title would replace every element in it — including the ones the click
//! handlers are attached to — so the buttons would go dead a quarter of a second after the strip
//! appeared. `put_Text` on the same `TextBlock` leaves the tree, and the handlers, alone.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::xamlom::IXamlDiagnostics;

use super::{layout, state, ticker};

/// Sweeps between one-character advances of the scroll.
///
/// The sweep is 250 ms at its fastest, so four of them is a character a second: fast enough to read a
/// long title in a reasonable time, slow enough not to be a flicker in the corner of the eye.
const SWEEPS_PER_CHARACTER: u32 = 4;

static SWEEP: AtomicU32 = AtomicU32::new(0);

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
        // Text that fits is written once, by the markup, and then left alone: re-setting an unchanged
        // property four times a second is pointless work on the shell's UI thread.
        if !ticker::scrolls(full, width) {
            continue;
        }
        let text = ticker::window(full, width, step as usize);
        for node in crate::tree::find_by_name(name) {
            crate::decorate::set_text(diagnostics, node, &text);
        }
    }
}

// The strip's own transport glyphs, and the code that wired them, are gone: the controls live on the
// hover preview's thumbnail toolbar now, which is the shell's to draw and `super::thumbbar`'s to
// wire. [`Segment`] stays because both halves still speak in it.

/// Which transport control was hit.
///
/// The strip *body* is deliberately not in here. On an app's own button the shell's own click already
/// means "bring this app forward or minimise it", and its press is where drag-to-reorder begins — so
/// leaving the body alone is what makes the tile behave like every other taskbar icon.
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

    /// Wire code for the message posted to audio-tray. **Must match `taskbar::Action` there.**
    pub fn code(self) -> usize {
        match self {
            Self::Previous => 10,
            Self::PlayPause => 11,
            Self::Next => 12,
        }
    }
}

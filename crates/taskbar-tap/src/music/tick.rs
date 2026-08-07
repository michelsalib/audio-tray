//! Advancing the scrolling title, and wiring the transport glyphs.
//!
//! **Both are property writes on existing elements, never a rebuild.** Rebuilding the strip to show
//! the next window of a long title would replace every element in it — including the ones the click
//! handlers are attached to — so the buttons would go dead a quarter of a second after the strip
//! appeared. `put_Text` on the same `TextBlock` leaves the tree, and the handlers, alone.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use crate::log::logf;
use crate::xamlom::{InstanceHandle, IXamlDiagnostics};

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

/// Segments we have already attached handlers to.
///
/// Handlers are never detached — the TAP lives as long as the Explorer process it is pinned in — so
/// the only thing to avoid is attaching a *second* handler to the same element, which would act on
/// every click twice.
static WIRED: Mutex<Vec<InstanceHandle>> = Mutex::new(Vec::new());

/// Attach a click handler to whichever transport glyphs XAML has announced back to us.
///
/// Called on a later sweep than the placement, always: our elements are announced *after* `put_Child`
/// returns, so on the placing sweep they are not in the recorded tree yet.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn wire(diagnostics: &IXamlDiagnostics) {
    for segment in [Segment::Previous, Segment::PlayPause, Segment::Next] {
        for node in crate::tree::find_by_name(segment.element_name()) {
            let fresh = {
                let mut wired = match WIRED.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                if wired.contains(&node) {
                    false
                } else {
                    wired.push(node);
                    true
                }
            };
            if fresh && crate::interact::attach_music(diagnostics, segment, node) {
                logf!("music: {} wired on 0x{node:x}", segment.label());
            }
        }
    }
}

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
    /// The `x:Name` in the markup, and how the element is found again once XAML announces it.
    pub fn element_name(self) -> &'static str {
        match self {
            Self::Previous => "MusicTilePrevious",
            Self::PlayPause => "MusicTilePlayPause",
            Self::Next => "MusicTileNext",
        }
    }

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

//! Making the shell's own thumbnail-toolbar buttons do something.
//!
//! audio-tray puts three buttons under the player's hover preview with `ITaskbarList3::
//! ThumbBarAddButtons` (see `music::thumbbar` on the app side). The shell draws them — themed, DPI
//! correct, in the right place — and that is the whole of what that API gives us, because the click
//! it raises is a `WM_COMMAND`/`THBN_CLICKED` **sent to the window the buttons were registered
//! against**. That window is YouTube Music's, owned by Chromium, which has never heard of them.
//! Measured exactly as predicted: the buttons drew and did nothing.
//!
//! This is the other half. The buttons the shell draws are ordinary XAML elements in Explorer's tree
//! — where this DLL already lives:
//!
//! ```text
//! Microsoft.UI.Xaml.Controls.ItemsRepeater#ThumbBarRepeater
//!   Taskbar.ThumbBarButton#ThumbBarButton      <- one per button, in the order they were added
//! ```
//!
//! So the click is taken the same way every other control in this TAP takes one: a `Tapped` handler
//! attached from the visual-tree callback, posting a wire code to audio-tray. The shell draws, we
//! listen, and neither half has to know about the other.
//!
//! **Why not just draw the buttons ourselves too.** Replacing the flyout's content was built and
//! abandoned — see the note in [`super`] — and this arrangement avoids every problem that had: there
//! is nothing of ours in the flyout to take back out when the pointer moves to another app, because
//! the shell owns and rebuilds the whole thing.

use std::sync::Mutex;

use crate::lock;
use crate::log::logf;
use crate::xamlom::{InstanceHandle, IXamlDiagnostics};

use super::{tick::Segment, tile};

/// The shell's type for one thumbnail-toolbar button.
///
/// Visible to the crate because the visual-tree callback watches for it too: a button being
/// *announced* is what asks for the wiring below to happen now rather than on the next sweep — see
/// `crate::wire_transport`.
pub(crate) const BUTTON_TYPE: &str = "Taskbar.ThumbBarButton";

/// Buttons we have already attached a handler to.
///
/// Handlers are never detached — this DLL outlives every flyout — so the only thing to avoid is
/// attaching twice to the same element, which would send two commands per click.
///
/// **Pruned to what the tree still holds**, because the shell builds three fresh buttons on every
/// hover and this would otherwise be a list that only grows for the life of the Explorer process,
/// scanned linearly four times a second. Dropping a handle that XAML has already removed cannot
/// cause a double-attach: the element it named is gone, so it can never be offered again.
static WIRED: Mutex<Vec<InstanceHandle>> = Mutex::new(Vec::new());

/// Attach transport handlers to the thumbnail-toolbar buttons of *our* preview.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn wire(diagnostics: &IXamlDiagnostics, host: &tile::Host) {
    let buttons = crate::tree::find_by_type(BUTTON_TYPE);
    if buttons.is_empty() {
        return;
    }

    // **Only our app's preview.** A thumbnail toolbar is a public API and another media player could
    // have one; wiring its buttons would make somebody else's pause button skip our track. The check
    // is the same substring match on the flyout's accessible name that finds the taskbar button, and
    // it fails closed.
    if !flyout_is_ours(diagnostics, host) {
        return;
    }

    {
        // Cheap: `buttons` is three handles on a normal hover, and this runs only when a preview is
        // open at all.
        let mut wired = lock(&WIRED);
        wired.retain(|handle| buttons.contains(handle));
    }

    for &button in &buttons {
        let Some(segment) = segment_of(diagnostics, button) else {
            continue;
        };
        let fresh = {
            let mut wired = lock(&WIRED);
            if wired.contains(&button) {
                false
            } else {
                wired.push(button);
                true
            }
        };
        if fresh && crate::interact::attach_music(diagnostics, segment, button) {
            logf!("music: thumb-bar {} wired on 0x{button:x}", segment.label());
        }
    }
}

/// Which transport control a button is, from its own accessible name.
///
/// **Position cannot be used, and that is what "play/pause stops working after one press" was.**
/// Changing the play glyph to a pause glyph means `ThumbBarUpdateButtons`, and the shell answers that
/// by rebuilding the button — a new XAML element, with the handler still attached to the old one.
/// The rebuilt button is announced *after* the other two, so indexing a sequence-ordered list put
/// previous and next at 0 and 1 and the live play/pause off the end at 3. One press, and the only
/// button whose glyph changes is the only one that goes dead.
///
/// The names come from `szTip`, which audio-tray sets on the buttons it adds — so this is a contract
/// between the two halves of this feature, like the 10/11/12 wire codes, and not something the shell
/// localises out from under us.
///
/// # Safety
/// XAML UI thread only.
unsafe fn segment_of(diagnostics: &IXamlDiagnostics, button: InstanceHandle) -> Option<Segment> {
    let name = crate::decorate::automation_name(diagnostics, button)?.to_lowercase();
    let segment = if name.contains("previous") {
        Segment::Previous
    } else if name.contains("play") || name.contains("pause") {
        Segment::PlayPause
    } else if name.contains("next") {
        Segment::Next
    } else {
        // Somebody else's thumbnail toolbar, or a name we did not expect. Logged once so the latter
        // is diagnosable rather than a button that silently never works.
        if !name.trim().is_empty() && !UNKNOWN_LOGGED.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            logf!("music: thumb-bar button named {name:?} matches no transport control");
        }
        return None;
    };
    Some(segment)
}

static UNKNOWN_LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the open hover preview is our app's.
///
/// The flyout carries the window's title, so this is the same substring match used for the taskbar
/// button — the shell's names are localised and a suffix cannot be predicted.
///
/// # Safety
/// XAML UI thread only.
unsafe fn flyout_is_ours(diagnostics: &IXamlDiagnostics, host: &tile::Host) -> bool {
    let Some(content) = crate::tree::find_by_name("HoverFlyoutContent").into_iter().next() else {
        return false;
    };
    let wanted = host.name.to_lowercase();

    let mut level = vec![content];
    for _ in 0..4 {
        let mut next = Vec::new();
        for handle in level {
            if crate::decorate::automation_name(diagnostics, handle)
                .is_some_and(|name| name.to_lowercase().contains(&wanted))
            {
                return true;
            }
            if crate::decorate::text_of(diagnostics, handle)
                .is_some_and(|text| text.to_lowercase().contains(&wanted))
            {
                return true;
            }
            next.extend(crate::tree::children_of(handle));
        }
        if next.is_empty() {
            break;
        }
        level = next;
    }
    false
}

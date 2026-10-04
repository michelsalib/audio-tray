//! Making the shell's thumbnail-toolbar buttons do something.
//!
//! audio-tray adds three buttons under the player's hover preview (`ThumbBarAddButtons`), but their
//! `THBN_CLICKED` goes to the player's own window, which ignores it. The buttons are XAML elements
//! in Explorer's tree (`ItemsRepeater#ThumbBarRepeater` → `Taskbar.ThumbBarButton`), so we attach
//! handlers that post a wire code to audio-tray. Wiring runs from `crate::wire_transport` (posted
//! via `lifecycle::nudge_transport` / `WM_TAP_WIRE_TRANSPORT`, with the sweep as fallback), never
//! inside the visual-tree callback.

use std::sync::Mutex;

use crate::lock;
use crate::log::logf;
use crate::xamlom::{InstanceHandle, IXamlDiagnostics};

use super::{tick::Segment, tile};

/// The shell's type for one thumbnail-toolbar button. The visual-tree callback watches for it to
/// request wiring (`crate::wire_transport`).
pub(crate) const BUTTON_TYPE: &str = "Taskbar.ThumbBarButton";

/// Buttons already wired, so none gets two handlers (two commands per click). Handlers are never
/// detached; the list is pruned to what the tree still holds.
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

    // Only our app's preview (other players have thumbnail toolbars too); fails closed.
    if !flyout_is_ours(diagnostics, host) {
        return;
    }

    {
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

/// Which transport control a button is, from its accessible name, never its position (the shell
/// rebuilds a button on `ThumbBarUpdateButtons`, reordering them). The names are audio-tray's
/// `szTip` values: a contract between the two halves.
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
        // Unexpected name: logged once so it is diagnosable.
        if !name.trim().is_empty() && !UNKNOWN_LOGGED.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            logf!("music: thumb-bar button named {name:?} matches no transport control");
        }
        return None;
    };
    Some(segment)
}

static UNKNOWN_LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the open hover preview is our app's: a substring match on the window title it carries.
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

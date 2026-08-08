//! The music tile: YouTube Music's own taskbar button, drawn as a now-playing strip.
//!
//! Runs inside the same TAP as audio-tray's own strip, because it has to — XAML Diagnostics takes one
//! consumer per endpoint, so a second process drawing into the taskbar cannot coexist with this one.
//!
//! ```text
//! state     what the app published, re-read from a file
//! ticker    the scrolling window over a title too long to fit
//! layout    the geometry and the XAML, from one number: how wide the strip is
//! tile      the button itself: Border.Child, the widening chain, the shell's own indicators
//! ```
//!
//! [`sweep`] is the whole of the entry point, called from the TAP's existing sweep. It is deliberately
//! quiet when the feature is off, when the app has published nothing, or when the button is not there
//! — a user with no YouTube Music sees no difference at all.

pub mod layout;
pub mod state;
pub mod thumbbar;
pub mod tick;
pub mod ticker;
pub mod tile;

use std::sync::Mutex;

use crate::log::logf;
use crate::xamlom::{InstanceHandle, IXamlDiagnostics};

/// The `Border#BackgroundElement` we are drawing in, and the strip we last drew.
///
/// Kept so a sweep can tell "nothing changed" from "the shell rebuilt the button": the strip is only
/// rebuilt when the *content* changed, because rebuilding replaces every element in it — including
/// the ones the click handlers are attached to, which is how a track change could silently break the
/// transport buttons.
static PLACED: Mutex<Option<Placed>> = Mutex::new(None);

struct Placed {
    border: InstanceHandle,
    shown: state::Strip,
}

/// Where the strip is, if it is up. Used by the app-facing side to answer "is the tile live".
pub fn placed_border() -> Option<InstanceHandle> {
    let guard = match PLACED.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.as_ref().map(|placed| placed.border)
}

// **The hover preview is deliberately left alone.** Replacing its content with a now-playing card
// was built and abandoned, and the two measurements that killed it are worth keeping:
//
// * **There is one `ContentPresenter#HoverFlyoutContent`, shared by every taskbar button** — not one
//   per flyout, as its animation suggests. The shell shows a different app by *updating* the
//   `TaskItemThumbnailList` inside it, so replacing the content leaves nothing for it to update and
//   the card appears on every app's preview.
// * **Handing it back on the next sweep does not rescue it.** Ownership can only be re-checked when
//   the timer next runs, so a foreign preview shows our card until it does, and our own shows the
//   shell's thumbnail until it does — a visible flip-flop either way, with no event to hang the work
//   on instead (`OnVisualTreeChange` may not mutate XAML; it wedges the shell).
//
// The transport controls live on the shell's own thumbnail toolbar instead — `ITaskbarList3::
// ThumbBarAddButtons`, driven from audio-tray in `music::thumbbar`, which needs no XAML at all.

/// One pass: find the button, put the strip in it, and keep it there.
///
/// # Safety
/// XAML UI thread only, and only with the event stream quiet — the caller's own gating already
/// guarantees both.
pub unsafe fn sweep(diagnostics: &IXamlDiagnostics) {
    let Some(host) = tile::host() else {
        return;
    };
    // Nothing published yet means audio-tray is running without the music half — or has only just
    // started. Either way there is nothing to draw, and drawing an empty strip over somebody's button
    // would be worse than leaving it alone.
    let Some(strip) = state::Strip::read() else {
        return;
    };

    // The transport buttons under the hover preview. Drawn by the shell at audio-tray's request and
    // wired here, because the click the shell raises goes to the player's window rather than to us.
    // Cheap and quiet when no preview is open: one lookup by type that finds nothing.
    thumbbar::wire(diagnostics, &host);

    let Some(button) = find_button(diagnostics, &host) else {
        return;
    };
    let Some(border) = find_background_element(diagnostics, button) else {
        logf!("music: {} has no Border#BackgroundElement", host.name);
        return;
    };

    let changed = {
        let guard = match PLACED.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        match guard.as_ref() {
            Some(placed) => placed.border != border || placed.shown != strip,
            None => true,
        }
    };

    if changed && tile::set_child(diagnostics, border, &layout::now_playing_markup(&strip)) {
        let mut guard = match PLACED.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = Some(Placed {
            border,
            shown: strip.clone(),
        });
        logf!(
            "music: strip placed on 0x{border:x} — {:?} / {:?} [{:?}]",
            strip.title,
            strip.artist,
            strip.playback
        );
    }

    if placed_border() != Some(border) {
        return;
    }

    // Everything below is re-applied every sweep on purpose: the shell re-asserts the button's own
    // width and rebuilds its indicators, so a single application is undone within a second. Each of
    // these is a no-op when the value is already ours.
    tile::hide_app_icon(diagnostics, button);
    tile::widen(diagnostics, border, &host);
    tile::place_button_state(diagnostics, button);
    tick::scroll(diagnostics, &strip);
}

/// Hand the button back: our content out, the shell's own widths and indicators restored.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn revert(diagnostics: &IXamlDiagnostics) {
    let placed = {
        let mut guard = match PLACED.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.take()
    };
    // Order matters: the sizes and margins go back *before* the content comes out, so the button is
    // never briefly its own size with our strip still in it.
    tile::restore(diagnostics);
    if let Some(placed) = placed {
        let cleared = tile::clear_child(diagnostics, placed.border);
        logf!("music: cleared the strip on 0x{:x} -> {cleared}", placed.border);
    }
    // **The progress bar, which is audio-tray's to set and ours to clean up after.** The one case
    // that needs this is a *killed* audio-tray: it runs no teardown, so the bar it put on the
    // player's button would stay frozen mid-track until Explorer restarts. This revert is already
    // the thing that runs on owner death (`lifecycle::watch_owner`), and from in here the shell's own
    // `ITaskbarList3` is a local call on an STA.
    clear_progress_bar();
}

/// Take the taskbar progress bar off the player's window.
///
/// Finds the window the way audio-tray does — a visible top-level window whose title carries the
/// host's name — because the pid in the init data is audio-tray's, not the player's, and the TAP has
/// no other handle on it.
///
/// Failures are silent: no player window is the ordinary case (the user closed it), and this runs on
/// a teardown path where there is nobody left to tell anyway.
fn clear_progress_bar() {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList, TBPF_NOPROGRESS};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, IsWindowVisible};
    use windows_core::BOOL;

    let Some(host) = tile::host() else {
        return;
    };

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = unsafe { &mut *(lparam.0 as *mut (String, Option<HWND>)) };
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return BOOL(1);
        }
        let mut title = [0u16; 512];
        let len = unsafe { GetWindowTextW(hwnd, &mut title) };
        if len > 0 {
            let title = String::from_utf16_lossy(&title[..len as usize]).to_lowercase();
            if title.contains(&search.0) {
                search.1 = Some(hwnd);
                return BOOL(0);
            }
        }
        BOOL(1)
    }

    let mut search = (host.name.to_lowercase(), None);
    let _ = unsafe {
        EnumWindows(
            Some(visit),
            LPARAM(&mut search as *mut (String, Option<HWND>) as isize),
        )
    };
    let Some(hwnd) = search.1 else {
        return;
    };
    unsafe {
        let Ok(taskbar) = CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_ALL)
        else {
            return;
        };
        if taskbar.HrInit().is_err() {
            return;
        }
        let cleared = taskbar.SetProgressState(hwnd, TBPF_NOPROGRESS).is_ok();
        logf!("music: cleared the progress bar on {:?} -> {cleared}", hwnd.0);
    }
}

/// The app's taskbar button, matched on its accessible name.
///
/// A **substring** match, because the shell's name carries a localised suffix — `"YouTube Music
/// épinglé"` on this machine. A miss logs every button it saw, which is the only way to discover those
/// names: they are not documented anywhere and they change with the display language.
///
/// # Safety
/// XAML UI thread only.
unsafe fn find_button(diagnostics: &IXamlDiagnostics, host: &tile::Host) -> Option<InstanceHandle> {
    let wanted = host.name.to_lowercase();
    let buttons = crate::tree::find_by_type(tile::Host::TYPE);
    let mut seen = Vec::new();
    for button in &buttons {
        let Some(name) = crate::decorate::automation_name(diagnostics, *button) else {
            continue;
        };
        if name.to_lowercase().contains(&wanted) {
            return Some(*button);
        }
        seen.push(name);
    }
    // Once, not every sweep: this runs four times a second, and the answer does not change until the
    // user pins something.
    if !seen.is_empty() && !MISS_LOGGED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        logf!("music: no button matching {:?} — saw {seen:?}", host.name);
    }
    None
}

static MISS_LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The `Border#BackgroundElement` inside the button's panel.
///
/// By name through the recorded tree rather than by walking `VisualTreeHelper`: the tree is already
/// recorded, the name is stable across Windows builds, and every level skipped is work not done on the
/// shell's UI thread four times a second.
///
/// **The first `Border` in the panel is not it.** A `TaskListButton` panel holds an unnamed `Border`
/// before the named one, and putting the strip in that one draws nothing — it sits behind the
/// background rather than in it.
fn find_background_element(
    _diagnostics: &IXamlDiagnostics,
    button: InstanceHandle,
) -> Option<InstanceHandle> {
    for panel in crate::tree::children_of(button) {
        for child in crate::tree::children_of(panel) {
            if crate::tree::name_of(child).as_deref() == Some("BackgroundElement") {
                return Some(child);
            }
        }
    }
    None
}

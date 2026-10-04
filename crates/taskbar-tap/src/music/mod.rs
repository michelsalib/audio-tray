//! The music tile: a player's own taskbar button, drawn as a now-playing strip.
//!
//! Lives in the same TAP as the audio strip because XAML Diagnostics takes one consumer per endpoint.
//!
//! ```text
//! state     what the app published, re-read from a file
//! ticker    the scrolling window over a title too long to fit; tick drives it
//! layout    the geometry and the XAML, from one number: how wide the strip is
//! tile      the button itself: Border.Child, the widening chain, the shell's own indicators
//! thumbbar  wiring the transport buttons on the shell's thumbnail toolbar
//! ```
//!
//! [`sweep`] is the entry point, called from the TAP's sweep; it does nothing when the feature is off,
//! nothing is published, or the button is absent. The hover preview is deliberately left alone (see
//! FINDINGS.md, "Taking over the hover flyout — built, and abandoned").

pub mod layout;
pub mod state;
pub mod thumbbar;
pub mod tick;
pub mod ticker;
pub mod tile;

use std::sync::Mutex;

use crate::log::logf;
use crate::xamlom::{InstanceHandle, IXamlDiagnostics};

/// Every `Border#BackgroundElement` we draw in and the strip it shows, so a sweep can tell "nothing
/// changed" from "the shell rebuilt the button". One per taskbar (one per display); keyed on the button.
static PLACED: Mutex<Vec<Placed>> = Mutex::new(Vec::new());

struct Placed {
    button: InstanceHandle,
    border: InstanceHandle,
    shown: state::Strip,
}

/// What the strip in `button`'s `border` is showing, if we put one there.
fn shown_on(button: InstanceHandle, border: InstanceHandle) -> Option<state::Strip> {
    crate::lock(&PLACED)
        .iter()
        .find(|placed| placed.button == button && placed.border == border)
        .map(|placed| placed.shown.clone())
}

/// Which `Border` we last drew `button`'s strip into.
fn border_of(button: InstanceHandle) -> Option<InstanceHandle> {
    crate::lock(&PLACED)
        .iter()
        .find(|placed| placed.button == button)
        .map(|placed| placed.border)
}

/// Note that `button`'s strip is now in `border`, showing `strip`, dropping any earlier record of
/// that button.
fn record(button: InstanceHandle, border: InstanceHandle, strip: &state::Strip) {
    let mut placed = crate::lock(&PLACED);
    placed.retain(|placed| placed.button != button);
    placed.push(Placed {
        button,
        border,
        shown: strip.clone(),
    });
}

/// One pass: find the buttons, put a strip in each, and keep them there.
///
/// # Safety
/// XAML UI thread only, with the event stream quiet (the caller's gating guarantees both).
pub unsafe fn sweep(diagnostics: &IXamlDiagnostics) {
    let Some(host) = tile::host() else {
        return;
    };
    // Nothing published: leave the button alone rather than draw an empty strip.
    let Some(strip) = state::Strip::read() else {
        return;
    };

    // The thumbnail-toolbar buttons are wired by `crate::wire_transport`, not here (this runs behind
    // the mutation gate, too late for the first press).

    // Drop records whose button XAML has removed.
    crate::lock(&PLACED).retain(|placed| crate::tree::type_of(placed.button).is_some());
    tile::prune_originals();

    // Collected first: content writes find elements by name, so they reach every strip at once and
    // are made once below, not per button.
    let mut drawn: Vec<(InstanceHandle, InstanceHandle)> = Vec::new();
    let mut stale: Vec<state::Strip> = Vec::new();

    for button in find_buttons(diagnostics, &host) {
        if !crate::live() {
            return;
        }
        let Some(border) = find_background_element(button) else {
            logf!("music: {} has no Border#BackgroundElement", host.name);
            continue;
        };

        // A track change is an update, never a rebuild: a rebuild re-lays out the button and makes
        // the shell's indicators visibly jump (FINDINGS.md, "The progress line jumped…").
        match shown_on(button, border) {
            Some(shown) if shown == strip => drawn.push((button, border)),
            // Same button, different track: patch what differs and leave the tree alone.
            Some(shown) => {
                if !stale.contains(&shown) {
                    stale.push(shown);
                }
                drawn.push((button, border));
            }
            // A button we have not drawn into — first sweep, or the shell rebuilt it under us.
            None => {
                // Log which cause: no record, or a record against a different `BackgroundElement`.
                if let Some(previous) = border_of(button) {
                    logf!("music: border moved 0x{previous:x} -> 0x{border:x}; rebuilding");
                }
                if tile::set_child(diagnostics, border, &layout::now_playing_markup(&strip)) {
                    logf!(
                        "music: strip placed on 0x{border:x} — {:?} / {:?} [{:?}]",
                        strip.title,
                        strip.artist,
                        strip.playback
                    );
                    record(button, border, &strip);
                    tick::restart();
                    drawn.push((button, border));
                }
            }
        }
    }

    // Once per distinct stale value; each write lands on every strip.
    for shown in stale {
        if !tile::update_in_place(diagnostics, &shown, &strip) {
            continue;
        }
        let mut placed = crate::lock(&PLACED);
        for record in placed.iter_mut().filter(|record| record.shown == shown) {
            record.shown = strip.clone();
        }
    }

    if drawn.is_empty() {
        return;
    }

    // Re-applied every sweep (the shell undoes them); each is a no-op when already ours.
    for (button, border) in drawn {
        if !crate::live() {
            return;
        }
        tile::hide_app_icon(diagnostics, button);
        tile::widen(diagnostics, border, &host);
        tile::place_button_state(diagnostics, button);
    }
    // One call covers every strip (writes go by name).
    tick::scroll(diagnostics, &strip);
}

/// Whether `handle` sits (within three levels) under a button we have drawn into. Tree-only, so it
/// is safe from the visual-tree callback.
pub fn is_tracked_part(handle: InstanceHandle) -> bool {
    let placed: Vec<InstanceHandle> = crate::lock(&PLACED).iter().map(|placed| placed.button).collect();
    if placed.is_empty() {
        return false;
    }
    let mut at = handle;
    for _ in 0..3 {
        let Some(parent) = crate::tree::parent_of(at) else {
            return false;
        };
        if placed.contains(&parent) {
            return true;
        }
        at = parent;
    }
    false
}

/// Re-pin the shell's indicators on every button we draw into — the event-driven half of what the
/// sweep re-applies. Property writes only.
///
/// # Safety
/// XAML UI thread only, outside the visual-tree callback, with the stream briefly quiet.
pub unsafe fn repin(diagnostics: &IXamlDiagnostics) {
    let buttons: Vec<InstanceHandle> = crate::lock(&PLACED).iter().map(|placed| placed.button).collect();
    for button in buttons {
        if crate::tree::type_of(button).is_some() {
            tile::place_button_state(diagnostics, button);
        }
    }
}

/// Hand the button back: our content out, the shell's own widths and indicators restored.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn revert(diagnostics: &IXamlDiagnostics) {
    let placed = std::mem::take(&mut *crate::lock(&PLACED));
    // Sizes and margins go back before the content comes out.
    tile::restore(diagnostics);
    for placed in placed {
        let cleared = tile::clear_child(diagnostics, placed.border);
        logf!("music: cleared the strip on 0x{:x} -> {cleared}", placed.border);
    }
    // audio-tray sets the progress bar, but a killed audio-tray cannot clear it; this revert runs on
    // owner death (`lifecycle::watch_owner`), so clear it here.
    clear_progress_bar();
}

/// Take the taskbar progress bar off the player's window: the first visible top-level window whose
/// title contains the host's name (as audio-tray finds it). Silent on failure (teardown path).
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
        // A panic must not unwind into user32: treat it as "stop".
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { visit_inner(hwnd, lparam) })).unwrap_or(BOOL(0))
    }

    unsafe fn visit_inner(hwnd: HWND, lparam: LPARAM) -> BOOL {
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

/// The app's taskbar button on **every** taskbar (one per display), matched as a substring of its
/// localised accessible name; a miss logs the names seen.
///
/// The newest per repeater, since the recorded tree keeps unannounced-removed elements. Never
/// cached: the repeater recycles buttons, so a handle can belong to another app next sweep.
///
/// # Safety
/// XAML UI thread only.
unsafe fn find_buttons(diagnostics: &IXamlDiagnostics, host: &tile::Host) -> Vec<InstanceHandle> {
    let wanted = host.name.to_lowercase();
    let mut taskbars: Vec<(InstanceHandle, Vec<InstanceHandle>)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for button in crate::tree::find_by_type(tile::Host::TYPE) {
        let Some(name) = crate::decorate::automation_name(diagnostics, button) else {
            continue;
        };
        if !name.to_lowercase().contains(&wanted) {
            // Deduplicated: each app appears once per taskbar.
            if !seen.contains(&name) {
                seen.push(name);
            }
            continue;
        }
        let taskbar = taskbar_of(button);
        match taskbars.iter_mut().find(|(known, _)| *known == taskbar) {
            Some((_, buttons)) => buttons.push(button),
            None => taskbars.push((taskbar, vec![button])),
        }
    }

    let buttons: Vec<InstanceHandle> = taskbars
        .into_iter()
        .filter_map(|(_, buttons)| crate::tree::newest(buttons))
        .collect();

    if buttons.is_empty() {
        // Logged once, not every sweep.
        if !seen.is_empty() && !MISS_LOGGED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            logf!("music: no button matching {:?} — saw {seen:?}", host.name);
        }
        return buttons;
    }
    // Log the taskbar count whenever it changes (displays plugged/unplugged).
    if FOUND.swap(buttons.len(), std::sync::atomic::Ordering::SeqCst) != buttons.len() {
        logf!(
            "music: {:?} has a button on {} taskbar(s)",
            host.name,
            buttons.len()
        );
    }
    buttons
}

static MISS_LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The button count last logged, so it is logged once per change.
static FOUND: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Which taskbar a button belongs to: the handle of its `ItemsRepeater` (one per display). Bounded;
/// without a repeater it falls back to the last recorded ancestor.
fn taskbar_of(button: InstanceHandle) -> InstanceHandle {
    let mut handle = button;
    // Same depth as `tile::widen`.
    for _ in 0..6 {
        let Some(parent) = crate::tree::parent_of(handle) else {
            break;
        };
        let Some(type_name) = crate::tree::type_of(parent) else {
            break;
        };
        if type_name == tile::REPEATER_TYPE {
            return parent;
        }
        handle = parent;
    }
    handle
}

/// The `Border#BackgroundElement` inside the button's panel, by name from the recorded tree (the
/// unnamed `Border` before it draws behind the background).
///
/// The **newest** match is load-bearing: stale duplicates linger in the recorded tree, and alternating
/// between them rebuilt the strip every sweep.
fn find_background_element(button: InstanceHandle) -> Option<InstanceHandle> {
    let candidates = crate::tree::children_of(button)
        .into_iter()
        .flat_map(crate::tree::children_of)
        .filter(|child| crate::tree::name_of(*child).as_deref() == Some("BackgroundElement"));
    crate::tree::newest(candidates)
}

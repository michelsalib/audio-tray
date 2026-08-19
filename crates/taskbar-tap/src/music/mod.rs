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

/// Every `Border#BackgroundElement` we are drawing in, and the strip each one shows.
///
/// Kept so a sweep can tell "nothing changed" from "the shell rebuilt the button": the strip is only
/// rebuilt when the *content* changed, because rebuilding replaces every element in it — including
/// the ones the click handlers are attached to, which is how a track change could silently break the
/// transport buttons.
///
/// **A list, because there is one button per taskbar.** With the taskbar shown on all displays the
/// shell builds the app's button once per display, and one record meant every display but one kept
/// the plain app icon. Keyed on the *button*, so a button the shell rebuilt replaces its own record
/// rather than leaving one behind per rebuild.
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

/// One pass: find the buttons, put a strip in each, and keep them there.
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

    // The transport buttons under the hover preview are **not** wired from here, though they were:
    // this runs behind the sweep's mutation gate, and a button is only rebuilt at the moment someone
    // is pointing at it. `crate::wire_transport` does it ahead of that gate, and off the
    // announcement, so the first press lands on a live button.

    // A record whose button XAML has removed — a display unplugged, the app closed. Dropping it
    // cannot lose a strip: the element it named is gone, so there is nothing left to hand back.
    crate::lock(&PLACED).retain(|placed| crate::tree::type_of(placed.button).is_some());

    // Where a strip now is, and what any out-of-date one is still showing. Both are collected before
    // a single character is written, because the content writes reach **every** strip at once — they
    // find their elements by name, and every taskbar's strip names them the same — so they are made
    // once, below, rather than once per button.
    let mut drawn: Vec<(InstanceHandle, InstanceHandle)> = Vec::new();
    let mut stale: Vec<state::Strip> = Vec::new();

    for button in find_buttons(diagnostics, &host) {
        let Some(border) = find_background_element(button) else {
            logf!("music: {} has no Border#BackgroundElement", host.name);
            continue;
        };

        // **A track change is an update, not a rebuild, and that distinction is visible.** Replacing
        // the `Border`'s child changes the strip's identity and momentarily its measured size, which
        // makes the shell re-run the button's layout and re-assert its own `RunningIndicator` and
        // `ProgressIndicator` from the template. The user sees the progress line snap back to the
        // shell's centred default and then step through our margin and width writes as they land — a
        // "centre, left, full width" jump on every song. Keeping the strip's elements in place keeps
        // the button's layout still, and the indicators with it.
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
                // Says *why* a rebuild is happening. A rebuild per track change is the defect this
                // branch's neighbours exist to avoid, and the two causes — no record at all, versus a
                // record against a different `BackgroundElement` — need opposite fixes.
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
                    drawn.push((button, border));
                }
            }
        }
    }

    // Once per *distinct* stale value, not once per button: what to write is decided by what differs
    // from what is up, and the write itself lands on every strip naming those elements.
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

    // Everything below is re-applied every sweep on purpose: the shell re-asserts the button's own
    // width and rebuilds its indicators, so a single application is undone within a second. Each of
    // these is a no-op when the value is already ours.
    for (button, border) in drawn {
        tile::hide_app_icon(diagnostics, button);
        tile::widen(diagnostics, border, &host);
        tile::place_button_state(diagnostics, button);
    }
    // One call for every strip on screen, for the same reason the content writes are: the ticker
    // writes its window to every `TextBlock` of that name, wherever it is.
    tick::scroll(diagnostics, &strip);
}

/// Hand the button back: our content out, the shell's own widths and indicators restored.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn revert(diagnostics: &IXamlDiagnostics) {
    let placed = std::mem::take(&mut *crate::lock(&PLACED));
    // Order matters: the sizes and margins go back *before* the content comes out, so the button is
    // never briefly its own size with our strip still in it.
    tile::restore(diagnostics);
    for placed in placed {
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

/// The app's taskbar button on **every** taskbar, matched on its accessible name.
///
/// A **substring** match, because the shell's name carries a localised suffix — `"YouTube Music
/// épinglé"` on this machine. A miss logs every button it saw, which is the only way to discover those
/// names: they are not documented anywhere and they change with the display language.
///
/// **One per taskbar, and that is what puts the strip on a second display.** With the taskbar shown
/// on all displays the shell builds the app's button once per display — same type, same accessible
/// name, each in a repeater of its own — and taking the first match left every display but one
/// showing the plain app icon.
///
/// **The newest per taskbar, for the reason [`find_background_element`] takes the newest `Border`:**
/// the recorded tree keeps elements whose removal XAML never announced, so one taskbar can offer
/// several buttons that differ only in age. Grouping by the repeater they sit in drops the dead ones
/// without collapsing several displays into one.
///
/// Every candidate's name is read rather than stopping at the first match, and none of them is
/// cached: the answer is a set now, and the repeater **recycles** buttons — a handle that says
/// "YouTube Music" on one sweep can be another app's button on the next, and a remembered verdict
/// would put our strip on that app.
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
            // Deduplicated, because every app is now seen once per taskbar and the point of this list
            // is the *names* the shell uses.
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
        // Once, not every sweep: this runs four times a second, and the answer does not change until
        // the user pins something.
        if !seen.is_empty() && !MISS_LOGGED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            logf!("music: no button matching {:?} — saw {seen:?}", host.name);
        }
        return buttons;
    }
    // How many taskbars carry the button, on the way in and whenever it changes. A display plugged in
    // or unplugged is the whole of what moves it, and it is the first thing a "the strip is missing on
    // my second screen" report needs.
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

/// How many buttons the last log line reported, so it is written once per change rather than once per
/// sweep.
static FOUND: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Which taskbar a button belongs to, as the handle of the `ItemsRepeater` laying it out.
///
/// Each display's taskbar has a repeater of its own, so the repeater **is** the identity of "this
/// display's copy of the button" — and unlike a screen position it costs no XAML call to read.
///
/// Bounded, and it stops at the last *recorded* ancestor: a tree with no repeater above the button
/// degrades to grouping by that ancestor rather than to an unbounded climb to the root.
fn taskbar_of(button: InstanceHandle) -> InstanceHandle {
    let mut handle = button;
    // The same depth `tile::widen` walks, and for the same reason: that is where the repeater is.
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

/// The `Border#BackgroundElement` inside the button's panel.
///
/// By name through the recorded tree rather than by walking `VisualTreeHelper`: the tree is already
/// recorded, the name is stable across Windows builds, and every level skipped is work not done on the
/// shell's UI thread four times a second.
///
/// **The first `Border` in the panel is not it.** A `TaskListButton` panel holds an unnamed `Border`
/// before the named one, and putting the strip in that one draws nothing — it sits behind the
/// background rather than in it.
///
/// **The newest match, and that is load-bearing.** The recorded tree keeps elements whose removal
/// XAML never announced, so a button the shell has rebuilt can offer two `BackgroundElement`s that
/// are identical by name and parent. Taking the first meant the answer alternated between sweeps —
/// and since the placement record is keyed on this handle, every alternation looked like "a button we
/// have not drawn into" and rebuilt the whole strip. That is what made the progress line jump on
/// every track change: the rebuild, not the track.
fn find_background_element(button: InstanceHandle) -> Option<InstanceHandle> {
    let candidates = crate::tree::children_of(button)
        .into_iter()
        .flat_map(crate::tree::children_of)
        .filter(|child| crate::tree::name_of(*child).as_deref() == Some("BackgroundElement"));
    crate::tree::newest(candidates)
}

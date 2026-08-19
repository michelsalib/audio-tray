//! Drawing the strip into an app's **own** taskbar button.
//!
//! This is the half audio-tray's TAP had no counterpart for. Its own strip lives in the notification
//! area, where `ContentPresenter.Content` takes arbitrary XAML and the slot sizes itself to whatever
//! you put in it. A taskbar button gives you neither:
//!
//! ```text
//! notification area   NotifyIconView → ContentPresenter.Content   slot sizes itself
//! taskbar button      TaskListButton → Border#BackgroundElement   fixed at 44 epx, must be widened
//! ```
//!
//! So three things here are new: `Border.Child` (nothing at that end of the taskbar is a
//! `ContentControl`), a **widening chain** (the Border honours a width its parent then clips, so every
//! ancestor up to the repeater has to be asked too), and placing the shell's own `RunningIndicator`
//! and `ProgressIndicator`, which are centred in a button that is now five times its normal width.
//!
//! **Whose button, and why that is the whole point.** Matching an app's existing button by name makes
//! the strip that app's taskbar presence, so the shell keeps doing what it already does well:
//! launching adds no second icon, minimising goes there, dragging reorders it, and a right-click gives
//! its jump list. The click on the strip *body* is left to the shell for the same reason — only the
//! three transport glyphs are ours.

use std::sync::Mutex;

use windows::Win32::Foundation::S_OK;
use windows_core::{IInspectable, Interface, HSTRING};

use crate::decorate::object_from_handle;
use crate::log::logf;
use crate::winrt::{
    IBorder, IFrameworkElement, IXamlReaderStatics, Thickness, HORIZONTAL_ALIGNMENT_LEFT,
    XAML_READER,
};
use crate::xamlom::{InstanceHandle, IXamlDiagnostics};

use super::layout;

/// The taskbar button the strip is drawn into.
///
/// One host, unlike media-tray's three: the Widgets entry point needs `TaskbarDa` enabled and clips
/// 71 epx of what it is asked for, and a `NotifyIconView` of our own is where audio-tray's *audio*
/// strip already lives. An app's own button is the one that earns its place — see the module docs.
pub struct Host {
    /// Matched as a **substring** of `AutomationProperties.Name`, because the shell's name carries a
    /// localised suffix: `"YouTube Music épinglé"`, `"Visual Studio Code - 1 fenêtre …"`.
    pub name: String,
}

/// The type XAML reports for the repeater that lays the taskbar's buttons out.
///
/// The level [`widen`] stops below — and, because every display's taskbar has one of its own, the
/// identity of a taskbar in [`super::find_buttons`].
pub const REPEATER_TYPE: &str = "Microsoft.UI.Xaml.Controls.ItemsRepeater";

impl Host {
    pub const TYPE: &'static str = "Taskbar.TaskListButton";

    /// How much wider than the strip the *button* is asked for, in epx.
    ///
    /// **Not slack — measured, and it buys one specific thing.** The plate has to be narrower than the
    /// button, or its rounded right corner lands on the boundary and gets shaved square; a flat
    /// `240/240/240` does exactly that. A task button's natural geometry is `button 44 → panel 44 →
    /// Border 40`, so 4 epx is the inset the shell itself uses, and 4 is what works on screen.
    ///
    /// Every epx here is paid twice, which is why it is worth being exact: the button owns the hit
    /// area and the tooltip, so overhang is hover that fires before the strip is reached — and it is
    /// taskbar width spent on nothing.
    pub const SLOT_OVERHEAD: f64 = 4.0;

    fn ask(&self, content: u32) -> f64 {
        f64::from(content) + Self::SLOT_OVERHEAD
    }
}

/// Which button to decorate, from `tile=<app name>` in the initialization data.
///
/// `None` disables the tile and nothing else: the feed still runs and the progress bar still appears,
/// because that one is the shell's own and needs no strip.
static HOST: Mutex<Option<String>> = Mutex::new(None);

pub fn set_host(name: Option<String>) {
    *crate::lock(&HOST) = name.filter(|name| !name.trim().is_empty());
}

pub fn host() -> Option<Host> {
    crate::lock(&HOST).clone().map(|name| Host { name })
}

/// The shell's own visuals inside the button that have to go.
///
/// **`RunningIndicator` and `ProgressIndicator` are deliberately absent.** The first is the only cue
/// that the app is open — the strip cannot supply it, since "closed" and "open, nothing playing" both
/// read as `Nothing playing` — and the second is where the track position goes, the same bar MPC-HC
/// draws. Both are later siblings than our host `Border`, so they paint *over* the strip's bottom edge
/// rather than being hidden by it. They are moved instead, by [`place_button_state`].
const HIDE: &[&str] = &["Icon", "DefaultIcon", "OverlayIcon"];

/// Everything we have written to, with what it held first.
///
/// **First write wins, and that is load-bearing.** A second recording would capture *our* value as the
/// original, and the revert would then put our value back — which for a `Width` on the shell's own
/// Border means leaving somebody's taskbar button 240 epx wide until Explorer restarts.
static ORIGINALS: Mutex<Vec<(InstanceHandle, Original)>> = Mutex::new(Vec::new());

#[derive(Clone, Copy)]
struct Original {
    width: f64,
    min_width: f64,
    alignment: i32,
    margin: Thickness,
}

/// # Safety
/// XAML UI thread only.
unsafe fn remember(diagnostics: &IXamlDiagnostics, handle: InstanceHandle) {
    let mut originals = crate::lock(&ORIGINALS);
    if originals.iter().any(|(known, _)| *known == handle) {
        return;
    }
    let Some(object) = object_from_handle(diagnostics, handle) else {
        return;
    };
    let Ok(framework) = object.cast::<IFrameworkElement>() else {
        return;
    };
    // `NaN` for a width that was never set, which is what `put_Width` itself treats as unset. Writing
    // `0.0` there would leave the element permanently zero-width — on screen, indistinguishable from
    // still being hidden.
    let (mut width, mut min_width) = (f64::NAN, f64::NAN);
    let mut alignment = HORIZONTAL_ALIGNMENT_LEFT;
    let mut margin = Thickness::default();
    if framework.get_Width(&mut width) != S_OK {
        width = f64::NAN;
    }
    if framework.get_MinWidth(&mut min_width) != S_OK {
        min_width = f64::NAN;
    }
    let _ = framework.get_HorizontalAlignment(&mut alignment);
    let _ = framework.get_Margin(&mut margin);
    originals.push((
        handle,
        Original {
            width,
            min_width,
            alignment,
            margin,
        },
    ));
}

/// Put every element we touched back as we found it.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn restore(diagnostics: &IXamlDiagnostics) {
    let originals = std::mem::take(&mut *crate::lock(&ORIGINALS));
    for (handle, original) in originals {
        let Some(object) = object_from_handle(diagnostics, handle) else {
            continue;
        };
        let Ok(framework) = object.cast::<IFrameworkElement>() else {
            continue;
        };
        // Position before size: putting the margin and alignment back first means the element is
        // never briefly our size *and* its own position, which is a visible jump.
        let margin = framework.put_Margin(original.margin) == S_OK;
        let aligned = framework.put_HorizontalAlignment(original.alignment) == S_OK;
        let width = framework.put_Width(original.width) == S_OK;
        let min = framework.put_MinWidth(original.min_width) == S_OK;
        logf!(
            "music: restored 0x{handle:x} — margin {margin}, align {aligned}, width {width}, min {min}"
        );
    }
}

/// Hang our markup inside a `Border` via its `Child` property.
///
/// # Safety
/// XAML UI thread only, and only with the visual-tree event stream quiet — a `put_*` against a
/// taskbar element while `AdviseVisualTreeChange` is streaming does not return, and takes the whole
/// taskbar with it.
pub unsafe fn set_child(
    diagnostics: &IXamlDiagnostics,
    handle: InstanceHandle,
    markup: &str,
) -> bool {
    let Some(target) = object_from_handle(diagnostics, handle) else {
        return false;
    };
    let Ok(border) = target.cast::<IBorder>() else {
        logf!("music: 0x{handle:x} is not an IBorder");
        return false;
    };
    let Some(child) = load_markup(markup) else {
        return false;
    };
    let hr = border.put_Child(child.as_raw());
    if hr != S_OK {
        logf!("music: put_Child on 0x{handle:x} failed: 0x{:08x}", hr.0);
        return false;
    }
    true
}

/// Take our content out, handing the shell's own emptiness back.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn clear_child(diagnostics: &IXamlDiagnostics, handle: InstanceHandle) -> bool {
    let Some(target) = object_from_handle(diagnostics, handle) else {
        return false;
    };
    let Ok(border) = target.cast::<IBorder>() else {
        return false;
    };
    border.put_Child(core::ptr::null_mut()) == S_OK
}

/// Build a live element from markup.
///
/// `XamlReader.Load`, because `IVisualTreeService::CreateInstance` is `E_NOTIMPL` inside Explorer —
/// the same reason audio-tray's own strip is built this way.
///
/// # Safety
/// XAML UI thread only.
unsafe fn load_markup(markup: &str) -> Option<IInspectable> {
    let reader: IXamlReaderStatics =
        match windows::Win32::System::WinRT::RoGetActivationFactory(&HSTRING::from(XAML_READER)) {
            Ok(factory) => factory,
            Err(err) => {
                logf!("music: XamlReader factory failed ({err})");
                return None;
            }
        };
    let markup = HSTRING::from(markup);
    // `HSTRING` is repr(transparent) over the handle; `as_ptr` would hand over the UTF-16 buffer
    // instead, which the callee would misread as a handle.
    let handle = core::mem::transmute_copy::<HSTRING, *mut core::ffi::c_void>(&markup);
    let mut created: *mut core::ffi::c_void = core::ptr::null_mut();
    let hr = reader.Load(handle, &mut created);
    if hr != S_OK || created.is_null() {
        logf!("music: XamlReader.Load rejected the strip markup: 0x{:08x}", hr.0);
        return None;
    }
    Some(core::mem::transmute::<*mut core::ffi::c_void, IInspectable>(created))
}

/// Widen the host `Border` **and every ancestor up to the `ItemsRepeater`**.
///
/// **One level is not enough, and that is the trap that hid the strip for a long time.** The Border
/// alone reports the width it was given and still draws 44 epx, because its parent
/// `TaskListButtonPanel` sits at an explicit `Width` the shell set — so the Border honours our width
/// and its parent clips it. The walk therefore continues upward and stops *below* the repeater, the
/// one level with no width property of its own. The repeater was never a wall: with the slot widened
/// the centred cluster moves right by exactly half the growth, so its layout is allocating the space
/// rather than refusing it.
///
/// Re-applied every sweep, because **the shell puts its own `Width=44` back** — measured, repeatedly.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn widen(diagnostics: &IXamlDiagnostics, border: InstanceHandle, host: &Host) {
    let content = layout::layout().strip;
    // The Border gets the *content* width and is pinned left; its ancestors get the ask. That gap is
    // what keeps the plate's rounded right corner clear of the boundary instead of shaved square.
    set_width(diagnostics, border, f64::from(content));
    pin_left(diagnostics, border);

    let ask = host.ask(content);
    let mut handle = border;
    // Bounded rather than a `while`: a tree that somehow has no repeater above us must not turn into
    // an unbounded walk to the root, widening everything on the way.
    for _ in 0..6 {
        let Some(parent) = crate::tree::parent_of(handle) else {
            return;
        };
        if crate::tree::type_of(parent).as_deref() == Some(REPEATER_TYPE) {
            return;
        }
        set_width(diagnostics, parent, ask);
        handle = parent;
    }
}

/// Set `Width` and `MinWidth`, remembering what was there first.
///
/// Both, because both are written: a `MinWidth` of ours left behind would pin the shell's own element
/// at our size for as long as Explorer lives.
///
/// # Safety
/// XAML UI thread only.
unsafe fn set_width(diagnostics: &IXamlDiagnostics, handle: InstanceHandle, width: f64) {
    let Some(object) = object_from_handle(diagnostics, handle) else {
        return;
    };
    let Ok(framework) = object.cast::<IFrameworkElement>() else {
        return;
    };
    // Already ours: writing the same value four times a second buys nothing, and skipping is also the
    // measurement — reaching here on a later sweep means the shell put its own width back.
    let mut live = f64::NAN;
    if framework.get_Width(&mut live) == S_OK && (live - width).abs() < 0.5 {
        return;
    }
    remember(diagnostics, handle);
    let _ = framework.put_Width(width);
    let _ = framework.put_MinWidth(width);
}

/// Pin an element to the left of its slot.
///
/// Needed because of a XAML rule that bites exactly once: an element left at `Stretch` and then given
/// an explicit `Width` is **centred**, not left-aligned. That slid the strip 40 epx right — half of
/// `ask − content` — and pushed the `next` glyph past the clip.
///
/// # Safety
/// XAML UI thread only.
unsafe fn pin_left(diagnostics: &IXamlDiagnostics, handle: InstanceHandle) {
    let Some(object) = object_from_handle(diagnostics, handle) else {
        return;
    };
    let Ok(framework) = object.cast::<IFrameworkElement>() else {
        return;
    };
    let mut alignment = 0i32;
    if framework.get_HorizontalAlignment(&mut alignment) == S_OK
        && alignment == HORIZONTAL_ALIGNMENT_LEFT
    {
        return;
    }
    remember(diagnostics, handle);
    let _ = framework.put_HorizontalAlignment(HORIZONTAL_ALIGNMENT_LEFT);
}

/// # Safety
/// XAML UI thread only.
unsafe fn set_margin(diagnostics: &IXamlDiagnostics, handle: InstanceHandle, margin: Thickness) {
    let Some(object) = object_from_handle(diagnostics, handle) else {
        return;
    };
    let Ok(framework) = object.cast::<IFrameworkElement>() else {
        return;
    };
    let mut live = Thickness::default();
    if framework.get_Margin(&mut live) == S_OK && (live.left - margin.left).abs() < 0.5 {
        return;
    }
    remember(diagnostics, handle);
    let _ = framework.put_Margin(margin);
}

/// Bring a strip already on screen in line with a new track, without replacing it.
///
/// Returns whether the strip is now showing `next` — `false` means the elements could not be found,
/// which is the caller's cue to fall back to a full placement.
///
/// Only what differs is touched. Title and artist are `put_Text` on the `TextBlock`s the markup
/// named; the cover is the one part that has to be reparsed, because pointing an `Image` somewhere
/// new needs a fresh `BitmapImage` — see [`layout::cover_markup`]. Even that is contained to the
/// `Border` around it, so the strip's own size never changes and the shell has no reason to re-lay
/// out the button.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn update_in_place(
    diagnostics: &IXamlDiagnostics,
    shown: &super::state::Strip,
    next: &super::state::Strip,
) -> bool {
    let l = layout::layout();
    let mut ok = true;
    let mut wrote_anything = false;

    for (name, was, now) in [
        ("MusicTileTitle", shown.display_title(), next.display_title()),
        (
            "MusicTileArtist",
            shown.display_artist(),
            next.display_artist(),
        ),
    ] {
        if was == now {
            continue;
        }
        // The ticker owns this text from here on, so write the window it would show at step 0
        // rather than the whole string — otherwise a long title appears unscrolled for a tick.
        let text = super::ticker::window(now, character_budget(name, &l), 0);
        let mut wrote = false;
        for node in crate::tree::find_by_name(name) {
            wrote |= crate::decorate::set_text(diagnostics, node, &text);
        }
        ok &= wrote;
        wrote_anything |= wrote;
    }

    if shown.cover != next.cover {
        let markup = layout::cover_markup(next, l.cover, l.gap);
        let mut wrote = false;
        for node in crate::tree::find_by_name(layout::COVER_HOST) {
            wrote |= set_child(diagnostics, node, &markup);
        }
        ok &= wrote;
        wrote_anything |= wrote;
    }

    // Only when something actually moved. A play/pause toggle changes `Strip` without changing
    // anything this function draws — the transport glyphs left for the hover preview — so logging
    // every call would be a line a second saying nothing happened.
    if wrote_anything {
        logf!(
            "music: strip updated in place — {:?} / {:?}",
            next.title,
            next.artist
        );
    }
    ok
}

fn character_budget(name: &str, l: &layout::Layout) -> usize {
    if name == "MusicTileTitle" {
        l.title_chars
    } else {
        l.artist_chars
    }
}

/// Move the shell's running indicator and progress bar under the strip's app icon.
///
/// Both are centred in the *button* by the template, which is right at 44 epx and wrong once it is
/// the width of a strip. Pinning left and setting a margin fixes each, but they want opposite
/// things: the running pill is about the *app*, so it goes under the icon, and the progress bar is
/// about the *track*, so it spans the whole plate.
///
/// The pill's own width is **read** rather than assumed, because the shell grows it when the window is
/// in the foreground — a hard-coded margin would be off-centre in one of the two states.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn place_button_state(diagnostics: &IXamlDiagnostics, button: InstanceHandle) {
    for panel in crate::tree::children_of(button) {
        for child in crate::tree::children_of(panel) {
            let name = crate::tree::name_of(child).unwrap_or_default();
            let fixed_width = match name.as_str() {
                "RunningIndicator" => None,
                // The full plate, not the icon — see [`layout::strip_width`].
                //
                // **The shell's own bar, deliberately.** Drawing our own line inside the strip was
                // tried, to escape the shell re-asserting this element from the button's template;
                // it works and it is wrong. Windows merges the progress bar *into* the running
                // indicator — one underline that fills — and a separate line of ours alongside the
                // shell's pill reads as two controls saying different things about the same app.
                "ProgressIndicator" => Some(layout::strip_width()),
                _ => continue,
            };
            let left = match fixed_width {
                // Flush with the plate's own left edge, so the bar and the strip start together.
                Some(_) => 0.0,
                None => {
                    let width = crate::decorate::actual_width(diagnostics, child).unwrap_or(0.0);
                    (layout::icon_centre() - width / 2.0).max(0.0)
                }
            };
            // **Width first, then position.** An STA pumps messages while an outgoing COM call is in
            // flight, so a frame can be rendered *between* these writes — and the order decides what
            // that frame looks like. Sizing first means the intermediate is a full-width bar still
            // centred, which slides into place; the other order shows a stub at the left edge that
            // then grows, which is the more obviously wrong-looking of the two.
            if let Some(width) = fixed_width {
                set_width(diagnostics, child, width);
            }
            pin_left(diagnostics, child);
            set_margin(
                diagnostics,
                child,
                Thickness {
                    left,
                    ..Default::default()
                },
            );
        }
    }
}

/// Collapse the app's own bitmap, which would otherwise paint across the strip.
///
/// Named rather than positional, so a future Windows build reordering the template's children can
/// never make us collapse our own strip. See [`HIDE`] for what is deliberately left alone.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn hide_app_icon(diagnostics: &IXamlDiagnostics, button: InstanceHandle) {
    for panel in crate::tree::children_of(button) {
        for child in crate::tree::children_of(panel) {
            let name = crate::tree::name_of(child).unwrap_or_default();
            if HIDE.contains(&name.as_str()) {
                crate::decorate::collapse(diagnostics, child);
            }
        }
    }
}

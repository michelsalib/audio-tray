//! Drawing the tile into an app's **own** taskbar button (`TaskListButton`).
//!
//! Content goes in `Border#BackgroundElement.Child`; the Border and every ancestor up to the
//! `ItemsRepeater` must be widened (re-applied each sweep); the shell's `RunningIndicator` and
//! `ProgressIndicator` are moved under the strip. Every write is recorded first so [`restore`] can
//! undo it. Clicks, drag, jump list etc. stay the shell's; the transport buttons live on the
//! thumbnail toolbar (`super::thumbbar`). See FINDINGS.md, "The music tile".

use std::sync::Mutex;

use windows::Win32::Foundation::S_OK;
use windows_core::{IInspectable, Interface};

use crate::decorate::{element, object_from_handle};
use crate::log::logf;
use crate::winrt::{
    IBorder, IFrameworkElement, Thickness, HORIZONTAL_ALIGNMENT_LEFT,
};
use crate::xamlom::{InstanceHandle, IXamlDiagnostics};

use super::layout;

/// The app's taskbar button the tile is drawn into.
pub struct Host {
    /// Matched as a **substring** of `AutomationProperties.Name`, which carries a localised suffix
    /// (`"YouTube Music épinglé"`).
    pub name: String,
}

/// The repeater laying out a taskbar's buttons: where [`widen`] stops, and one per display's
/// taskbar in [`super::find_buttons`].
pub const REPEATER_TYPE: &str = "Microsoft.UI.Xaml.Controls.ItemsRepeater";

impl Host {
    pub const TYPE: &'static str = "Taskbar.TaskListButton";

    /// How much wider than the strip the button's ancestors are asked to be: the shell's own 4-epx
    /// inset, so the plate's rounded corner is not shaved. Measured; more is wasted hover area.
    pub const SLOT_OVERHEAD: f64 = 4.0;

    fn ask(&self, content: u32) -> f64 {
        f64::from(content) + Self::SLOT_OVERHEAD
    }
}

/// Which button to decorate, from `tile=<app name>` in the init data; `None` disables the tile.
static HOST: Mutex<Option<String>> = Mutex::new(None);

pub fn set_host(name: Option<String>) {
    *crate::lock(&HOST) = name.filter(|name| !name.trim().is_empty());
}

pub fn host() -> Option<Host> {
    crate::lock(&HOST).clone().map(|name| Host { name })
}

/// The shell's visuals inside the button that are collapsed. `RunningIndicator` and
/// `ProgressIndicator` are deliberately kept and moved instead ([`place_button_state`]).
const HIDE: &[&str] = &["Icon", "DefaultIcon", "OverlayIcon"];

/// Everything we have written to, with what it held first. First record wins (load-bearing):
/// a later one would capture our own value and the revert would keep it.
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
    let Some(framework) = element::<IFrameworkElement>(diagnostics, handle) else {
        return;
    };
    // `NaN` = never set ("Auto"); restoring `0.0` instead would leave it zero-width.
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
        let Some(framework) = element::<IFrameworkElement>(diagnostics, handle) else {
            continue;
        };
        // Position before size, to avoid a visible intermediate frame.
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
/// XAML UI thread only, never while the visual-tree event stream is delivering (a `put_*` there
/// hangs the taskbar).
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
    let Some(border) = element::<IBorder>(diagnostics, handle) else {
        return false;
    };
    border.put_Child(core::ptr::null_mut()) == S_OK
}

unsafe fn load_markup(markup: &str) -> Option<IInspectable> {
    crate::decorate::load_xaml(markup)
}

/// Widen the host `Border` **and every ancestor below the `ItemsRepeater`** (each parent clips to
/// its own width). Re-applied every sweep: the shell puts `Width=44` back.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn widen(diagnostics: &IXamlDiagnostics, border: InstanceHandle, host: &Host) {
    let content = layout::layout().strip;
    // The Border gets the content width, pinned left; its ancestors get the ask (corner clearance).
    set_width(diagnostics, border, f64::from(content));
    pin_left(diagnostics, border);

    let ask = host.ask(content);
    let mut handle = border;
    // Bounded, so a tree with no repeater cannot widen everything up to the root.
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

/// Set `Width` and `MinWidth`, remembering both first.
///
/// # Safety
/// XAML UI thread only.
unsafe fn set_width(diagnostics: &IXamlDiagnostics, handle: InstanceHandle, width: f64) {
    let Some(framework) = element::<IFrameworkElement>(diagnostics, handle) else {
        return;
    };
    // Skip if already ours (this runs every sweep).
    let mut live = f64::NAN;
    if framework.get_Width(&mut live) == S_OK && (live - width).abs() < 0.5 {
        return;
    }
    remember(diagnostics, handle);
    let _ = framework.put_Width(width);
    let _ = framework.put_MinWidth(width);
}

/// Pin an element to the left of its slot (a `Stretch` element given an explicit `Width` is centred).
///
/// # Safety
/// XAML UI thread only.
unsafe fn pin_left(diagnostics: &IXamlDiagnostics, handle: InstanceHandle) {
    let Some(framework) = element::<IFrameworkElement>(diagnostics, handle) else {
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

/// Set the left margin, keeping the template's other three sides.
///
/// # Safety
/// XAML UI thread only.
unsafe fn set_margin_left(diagnostics: &IXamlDiagnostics, handle: InstanceHandle, left: f64) {
    let Some(framework) = element::<IFrameworkElement>(diagnostics, handle) else {
        return;
    };
    let mut live = Thickness::default();
    if framework.get_Margin(&mut live) != S_OK {
        return;
    }
    if (live.left - left).abs() < 0.5 {
        return;
    }
    remember(diagnostics, handle);
    let _ = framework.put_Margin(Thickness { left, ..live });
}

/// Forget originals of elements the shell has since destroyed (every indicator rebuild leaves one).
pub fn prune_originals() {
    crate::lock(&ORIGINALS).retain(|(handle, _)| crate::tree::type_of(*handle).is_some());
}

/// Bring a strip already on screen in line with a new track, without replacing it. Returns
/// whether it now shows `next`; `false` means fall back to a full placement.
///
/// Only what differs is touched: `put_Text` for title/artist, and a rebuild of just the cover's
/// `Border` (see [`layout::cover_markup`]), so the strip's size never changes.
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
        // Write the ticker's step-0 window, not the whole string.
        let text = super::ticker::window(now, character_budget(name, &l), 0);
        let mut wrote = false;
        for node in crate::tree::find_by_name(name) {
            wrote |= crate::decorate::set_text(diagnostics, node, &text);
        }
        ok &= wrote;
        wrote_anything |= wrote;
        // New text scrolls from the start.
        super::tick::restart();
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

    // Log only real writes: a play/pause change alters `Strip` but nothing drawn here.
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

/// Re-place the shell's indicators, which the template centres in the (now wide) button: the
/// running pill under the icon (its width is read, since it grows in the foreground), the
/// progress bar across the whole plate.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn place_button_state(diagnostics: &IXamlDiagnostics, button: InstanceHandle) {
    for panel in crate::tree::children_of(button) {
        for child in crate::tree::children_of(panel) {
            let name = crate::tree::name_of(child).unwrap_or_default();
            let fixed_width = match name.as_str() {
                "RunningIndicator" => None,
                // The shell's own bar, deliberately (it merges with the running indicator).
                "ProgressIndicator" => Some(layout::strip_width()),
                _ => continue,
            };
            let left = match fixed_width {
                // Flush with the plate's left edge.
                Some(_) => 0.0,
                None => {
                    let width = crate::decorate::actual_width(diagnostics, child).unwrap_or(0.0);
                    (layout::icon_centre() - width / 2.0).max(0.0)
                }
            };
            // Width first, then position: a frame can render between these writes, and this order
            // looks least wrong.
            if let Some(width) = fixed_width {
                set_width(diagnostics, child, width);
            }
            pin_left(diagnostics, child);
            set_margin_left(diagnostics, child, left);
        }
    }
}

/// Collapse the app's own icon, matched by name (never by position) so we cannot hit our strip.
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

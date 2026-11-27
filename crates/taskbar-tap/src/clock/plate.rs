//! Two buttons: the clock and the bell lit apart.
//!
//! The shell draws one plate (`Border#BackgroundBorder`) behind the whole button, clock and bell
//! together. While the bell is the notifications' button, that plate is hidden and each icon's grid
//! becomes a plate of its own (a grid's background and corners are property values): inset as the
//! shell's, its corners, a gap between them. Lit under the pointer, and while its card is open.
//! While any of audio-tray's panels is open, the tooltips around are quiet ([`quiet_tooltips`]).

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::log::logf;
use crate::part::{SavedGrid, Unsplittable, ICON_GRID, PLATE, PLATE_GAP};
use crate::tree::{descendants, named};
use crate::winrt::Thickness;
use crate::{xaml, xamlom};

/// The part's name, in the log and for what it shares with the battery ([`crate::part`]).
const WHO: &str = "clock";

/// A button split in two, and what was there before (put back by [`unsplit`]).
struct Split {
    section: u64,
    plate: u64,
    /// Each icon's grid, made a plate, and what it held before.
    grids: Vec<SavedGrid>,
    clock_grid: u64,
    bell_grid: u64,
    /// The bell's grid's minimum width before it was split, and its width then.
    bell_min: f64,
    bell_width: f64,
}

static SPLIT: Mutex<Vec<Split>> = Mutex::new(Vec::new());
/// Sections whose button could not be split (missing parts): left whole, not retried.
static UNSPLITTABLE: Unsplittable = Unsplittable::new(WHO, "the button");

/// The icon grid lit as under the pointer.
static HOVERED: Mutex<Option<u64>> = Mutex::new(None);
/// The part (icon, and whether it is the bell) the pointer is on, or was last on while still in
/// the button: the strips beside and between the parts belong to the nearest one.
static PART: Mutex<Option<(u64, bool)>> = Mutex::new(None);
/// The pointer is in the clock's button (anywhere in it).
static SECTION_OVER: AtomicBool = AtomicBool::new(false);
/// Which card audio-tray has open (`tap_proto::OPEN_*`).
static OPEN: AtomicUsize = AtomicUsize::new(tap_proto::OPEN_NONE);
thread_local! {
    /// Tooltips hidden while a panel is open (the XAML thread's objects).
    static TOOLTIPS: std::cell::RefCell<Vec<Hidden>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A tooltip hidden while a panel is open: its owner, the tooltip, and whether it was faded out
/// (else taken off the owner).
struct Hidden {
    owner: u64,
    tip: windows_core::IInspectable,
    faded: bool,
}

/// Whether any button is split.
pub(super) fn any_split() -> bool {
    !crate::lock(&SPLIT).is_empty()
}

/// Whether `section`'s button is split, or was left whole (reported once).
pub(super) fn split_or_left_whole(section: u64) -> bool {
    UNSPLITTABLE.contains(section) || crate::lock(&SPLIT).iter().any(|s| s.section == section)
}

/// The width the shell laid the bell's grid `outer` out at, and its minimum width, once split.
pub(super) fn bell_base(outer: u64) -> Option<(f64, f64)> {
    crate::lock(&SPLIT).iter().find(|s| s.bell_grid == outer).map(|s| (s.bell_width, s.bell_min))
}

/// Split `section`'s button in two (once).
///
/// # Safety
/// XAML UI thread only.
pub(super) unsafe fn split(diagnostics: &xamlom::IXamlDiagnostics, section: u64) {
    use crate::winrt::IFrameworkElement;
    if crate::lock(&SPLIT).iter().any(|s| s.section == section) || UNSPLITTABLE.contains(section) {
        return;
    }
    let (Some(plate), Some(clock), Some(bell)) = (named(section, PLATE), super::clock_of(section), super::bell_of(section)) else {
        return UNSPLITTABLE.note(section, "the plate, the clock or the bell missing");
    };
    let (Some(clock_grid), Some(bell_grid)) = (named(clock, ICON_GRID), named(bell, ICON_GRID)) else {
        return UNSPLITTABLE.note(section, "an icon grid missing");
    };
    // The shell's plate: its inset, its corners and its edge, for ours.
    let (margin, radius, edge) = crate::part::plate_shape(diagnostics, plate);
    let sizes = |h: u64| xaml::actual_size(diagnostics, h).unwrap_or_default();
    logf!(
        "clock: splitting the button: plate {:?} margin {margin:?} radius {}, clock grid {:?}, bell grid {:?}",
        sizes(plate),
        radius.top_left,
        sizes(clock_grid),
        sizes(bell_grid)
    );
    // Both grids found (and their looks saved) before either is written: half a split could not
    // be put back.
    let (Some(clock_plate), Some(bell_plate)) = (crate::part::to_plate(diagnostics, clock_grid), crate::part::to_plate(diagnostics, bell_grid)) else {
        return UNSPLITTABLE.note(section, "an icon grid's interfaces missing");
    };
    let grids = vec![
        crate::part::make_plate(clock_plate, Thickness { right: PLATE_GAP / 2.0, ..margin }, radius, edge),
        crate::part::make_plate(bell_plate, Thickness { left: PLATE_GAP / 2.0, ..margin }, radius, edge),
    ];
    // The bell's width as the shell lays it out, widened while it wears a badge
    // ([`super::bell::show_bell_badge`]).
    let mut bell_min = 0.0;
    if let Some(framework) = xaml::element::<IFrameworkElement>(diagnostics, bell_grid) {
        let _ = framework.get_MinWidth(&mut bell_min);
    }
    let bell_width = xaml::actual_width(diagnostics, bell_grid).unwrap_or(0.0).max(bell_min);
    // Filled and faded as the shell's plate is.
    if let Some(fades) = crate::part::dress_like_plate(diagnostics, plate, &[clock_grid, bell_grid]) {
        logf!("clock: the shell's plate fades {fades}");
    }
    crate::part::watch_plate(diagnostics, plate, "clock: the plate callback", relight_from_plate);
    xaml::set_opacity(diagnostics, plate, 0.0);
    crate::lock(&SPLIT).push(Split { section, plate, grids, clock_grid, bell_grid, bell_min, bell_width });
    light(diagnostics);
}

/// The shell's plate changed its look: the clock's and the bell's plates follow.
fn relight_from_plate() {
    if let Some(diagnostics) = crate::diagnostics() {
        unsafe { light(&diagnostics) };
    }
}

/// Put the button back whole, its tooltips with it (a revert).
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn unsplit(diagnostics: &xamlom::IXamlDiagnostics) {
    use crate::winrt::IFrameworkElement;
    OPEN.store(tap_proto::OPEN_NONE, Ordering::SeqCst);
    quiet_tooltips(diagnostics, false, &[]);
    UNSPLITTABLE.clear();
    for split in std::mem::take(&mut *crate::lock(&SPLIT)) {
        xaml::set_opacity(diagnostics, split.plate, 1.0);
        if let Some(framework) = xaml::element::<IFrameworkElement>(diagnostics, split.bell_grid) {
            let _ = framework.put_MinWidth(split.bell_min);
        }
        let glyph = super::bell_of(split.section).and_then(super::bell::glyph_of);
        if let Some(framework) = glyph.and_then(|g| xaml::element::<IFrameworkElement>(diagnostics, g)) {
            let _ = framework.put_Margin(Thickness::default());
        }
        for saved in &split.grids {
            crate::part::restore_grid(diagnostics, saved);
        }
    }
    *crate::lock(&HOVERED) = None;
    *crate::lock(&PART) = None;
}

/// The pointer entered or left an icon (the clock's or the bell's): its plate follows.
pub fn part_hover(icon: u64, bell: bool, over: bool) {
    let mut part = crate::lock(&PART);
    if over {
        // Another part was lit (the pointer crossed the strip between them): it is no longer.
        if let Some((_, was_bell)) = part.replace((icon, bell)) {
            if was_bell && !bell {
                crate::ipc::send_bell_hover(false);
            }
        }
        if bell {
            crate::ipc::send_bell_hover(true);
        }
    } else if !SECTION_OVER.load(Ordering::SeqCst) && part.is_some_and(|(i, _)| i == icon) {
        *part = None;
        if bell {
            crate::ipc::send_bell_hover(false);
        }
    }
    // Left for the strip beside it, still in the button: it stays lit, as the shell's one plate
    // did over the whole button. It goes when the pointer leaves the button ([`section_hover`]).
    drop(part);
    relight();
}

/// The pointer entering or leaving the clock's button as a whole (both parts and the strips
/// around and between them). Entered on a strip, not on a part (from Show desktop, from above):
/// the bell lights, the nearest part there is on the strip at its right.
pub fn section_hover(section: u64, over: bool) {
    SECTION_OVER.store(over, Ordering::SeqCst);
    let mut part = crate::lock(&PART);
    if over {
        if part.is_none() {
            if let Some(bell) = super::bell_of(section) {
                *part = Some((bell, true));
                crate::ipc::send_bell_hover(true);
            }
        }
    } else if let Some((_, bell)) = part.take() {
        if bell {
            crate::ipc::send_bell_hover(false);
        }
    }
    drop(part);
    relight();
}

/// The part to light, from [`PART`], lit.
fn relight() {
    let grid = crate::lock(&PART).and_then(|(icon, _)| named(icon, ICON_GRID));
    *crate::lock(&HOVERED) = grid;
    if let Some(diagnostics) = crate::diagnostics() {
        unsafe { light(&diagnostics) };
    }
}

/// audio-tray opened a panel, swapped it, or closed it (`tap_proto::OPEN_*`): the calendar's or the
/// notifications' button stays lit while its card is open. (The tooltips are quieted by
/// [`crate::panel_changed`].)
pub fn set_panel(open: usize) {
    OPEN.store(open, Ordering::SeqCst);
    if let Some(diagnostics) = crate::diagnostics() {
        unsafe { light(&diagnostics) };
    }
}

/// Which of audio-tray's panels is open (`tap_proto::OPEN_*`).
pub fn open_panel() -> usize {
    OPEN.load(Ordering::SeqCst)
}

/// Each split button's plates, lit as they should be now: the icon under the pointer wears exactly
/// what the shell's own plate wears at that moment (so it matches every other taskbar button, in
/// any theme), and the open card's icon the look last seen under the pointer.
///
/// # Safety
/// XAML UI thread only.
unsafe fn light(diagnostics: &xamlom::IXamlDiagnostics) {
    let hovered = *crate::lock(&HOVERED);
    let open = OPEN.load(Ordering::SeqCst);
    for split in crate::lock(&SPLIT).iter() {
        let now = crate::part::plate_look(diagnostics, split.plate);
        if hovered.is_some() {
            if let Some(look) = &now {
                crate::part::remember_look(WHO, look);
            }
        }
        for (grid, part) in [(split.clock_grid, tap_proto::OPEN_CALENDAR), (split.bell_grid, tap_proto::OPEN_NOTIFICATIONS)] {
            let look = if hovered == Some(grid) {
                now.clone()
            } else if open == part {
                crate::part::open_look(WHO)
            } else {
                None
            };
            crate::part::wear(diagnostics, grid, look.as_ref());
        }
    }
}

/// Hide the tooltips of the clock's button, its icons and what lies between, of our strip's icon and
/// of the elements under `others` (`quiet`), or show again those hidden: no tooltip of ours shows
/// over an open panel.
///
/// A tooltip that is an element is faded out (opacity 0) and stays attached: taking it off its
/// owner while the pointer was on it (the click that opened the audio panel) left that owner's
/// tooltip broken after it was put back. Only one that is not an element (a bare value) is taken
/// off and put back.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn quiet_tooltips(diagnostics: &xamlom::IXamlDiagnostics, quiet: bool, others: &[u64]) {
    use crate::part::{set_tooltip, tooltip_of};
    use crate::winrt::IUIElement;
    use windows_core::Interface;
    if !quiet {
        for hidden in TOOLTIPS.with(|t| std::mem::take(&mut *t.borrow_mut())) {
            match hidden.tip.cast::<IUIElement>() {
                Ok(faded) if hidden.faded => {
                    let _ = faded.put_Opacity(1.0);
                }
                _ => {
                    set_tooltip(diagnostics, hidden.owner, Some(&hidden.tip));
                }
            }
        }
        return;
    }
    let mut hidden = TOOLTIPS.with(|t| std::mem::take(&mut *t.borrow_mut()));
    // The strip's notify icon (its tooltip is the shell's: the device and "Audio Tray").
    let strip = crate::decorated_pair().map(|(icon, _)| icon);
    for root in super::targets().into_iter().chain(strip).chain(others.iter().copied()) {
        for owner in std::iter::once(root).chain(descendants(root)) {
            if hidden.iter().any(|h| h.owner == owner) {
                continue;
            }
            // Ours to release, or to give back.
            let Some(tip) = tooltip_of(diagnostics, owner) else { continue };
            let faded = tip.cast::<IUIElement>().is_ok_and(|element| element.put_Opacity(0.0).is_ok());
            if faded || set_tooltip(diagnostics, owner, None) {
                hidden.push(Hidden { owner, tip, faded });
            }
        }
    }
    TOOLTIPS.with(|t| *t.borrow_mut() = hidden);
}

//! Windows' battery icon: where audio-tray's batteries open from, and what it says.
//!
//! **Clicks** (`battery=1` in the init data): the icon is one of the Quick Settings button's
//! (`ControlCenterButton`: network, volume, battery), which opens Quick Settings whatever its XAML
//! handlers do with a press, as the clock's button does. So, as for the clock, the TAP only says
//! when the pointer is over the battery (`WM_TASKBAR_BATTERY_HOVER`) and the app's mouse hook takes
//! its click. The rest of the button stays the shell's. The handlers go on the battery's icon view,
//! behind the short wiring gate, and are never detached: they check [`enabled`] on every event.
//!
//! **Two buttons**: the shell draws one plate (`Border#BackgroundBorder`) behind the whole button.
//! While the battery is audio-tray's, that plate stops short of it (its right margin takes the
//! battery's room, measured from where XAML arranged the two), and the battery's own grid becomes a
//! plate: it wears the shell's plate's look while the pointer is on it (the shell's plate, which
//! lights over the whole button, hidden meanwhile) and while the batteries are open.
//!
//! **What it says** (`tap_proto::BATTERY_MAGIC`): its tooltip, this PC's battery and each device's
//! as the app words them, and a dot at its upper right while a device runs low (an app-drawn PNG,
//! as the clock's badge).

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use crate::log::logf;
use crate::part::{SavedGrid, Unsplittable, ICON, ICON_GRID, INNER_GRID, PLATE, PLATE_GAP, QUICK_SETTINGS};
use crate::tree::{descendants, named};
use crate::winrt::Thickness;
use crate::{tree, xaml, xamlom};
use tap_proto::key;

/// The part's name, in the log and for what it shares with the clock ([`crate::part`]).
const WHO: &str = "battery";
/// A measure off by more than this (DIPs) from what a Quick Settings button can hold is not one to
/// lay the plates out from.
const SLACK: f64 = 8.0;
/// Sweeps a button may go unmeasured (no layout yet) before it is left whole: the sweep would
/// otherwise never settle to its idle pace.
const SPLIT_TRIES: u32 = 20;

static ENABLED: AtomicBool = AtomicBool::new(false);
/// Turned off while split: [`wire`] puts the button back whole at the next sweep (this runs off
/// the XAML thread).
static UNSPLIT_PENDING: AtomicBool = AtomicBool::new(false);
/// Icon views that already have the handlers.
static WIRED: Mutex<Vec<u64>> = Mutex::new(Vec::new());
/// Sections where no battery was found (each reported once): a desktop PC has none.
static MISSING: Mutex<Vec<u64>> = Mutex::new(Vec::new());
/// The section's tree was logged (once per shell).
static DESCRIBED: AtomicBool = AtomicBool::new(false);
/// The pointer is on the battery.
static OVER: AtomicBool = AtomicBool::new(false);
/// What the app asked of the icon is not all shown yet (it came before the icon was found, or the
/// icon was rebuilt, or a panel held the tooltip back): the next sweep shows it.
static LOOK_PENDING: AtomicBool = AtomicBool::new(false);
/// Sweeps the split has waited for a layout, and the tree's version when it last tried.
static TRIES: AtomicU32 = AtomicU32::new(0);
static TRIED_AT: AtomicU64 = AtomicU64::new(u64::MAX);

pub fn set_enabled(on: bool) {
    if ENABLED.swap(on, Ordering::SeqCst) != on {
        logf!("battery: {}", if on { "clicks open audio-tray's batteries" } else { "left to the shell" });
        UNSPLIT_PENDING.store(!on, Ordering::SeqCst);
    }
}

/// Whether a click on the battery is ours: asked for, the TAP live, and the app there to take it.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst) && crate::live() && crate::ipc::receiver_present()
}

/// Any of audio-tray's panels is open: the tooltips are quiet, so a new one waits.
fn panel_open() -> bool {
    crate::clock::open_panel() != tap_proto::OPEN_NONE
}

/// The batteries are open: the battery stays lit.
fn batteries_open() -> bool {
    crate::clock::open_panel() == tap_proto::OPEN_BATTERIES
}

/// Each Quick Settings button and the battery found in it, worked out once per version of the tree
/// (the sweep asks every few seconds; the tree changes far less often).
fn found() -> Vec<(u64, Option<u64>)> {
    type Found = (u64, Vec<(u64, Option<u64>)>);
    static FOUND: Mutex<Found> = Mutex::new((u64::MAX, Vec::new()));
    let version = tree::version();
    let mut found = crate::lock(&FOUND);
    if found.0 != version {
        *found = (version, tree::find_by_name(QUICK_SETTINGS).into_iter().map(|s| (s, battery_of(s))).collect());
    }
    found.1.clone()
}

/// The battery icons found, one per Quick Settings button (the main taskbar's, as a rule).
pub fn icons() -> Vec<u64> {
    if !ENABLED.load(Ordering::SeqCst) {
        return Vec::new();
    }
    found().into_iter().filter_map(|(_, icon)| icon).collect()
}

/// The battery's icon view in `section`: the one holding a battery content class; failing that,
/// the one drawn with unnamed text blocks (measured on 26300: network and volume draw their glyphs
/// in `InnerTextBlock`s, the battery in two unnamed ones, outline and fill).
fn battery_of(section: u64) -> Option<u64> {
    let views: Vec<u64> = descendants(section).into_iter().filter(|h| tree::type_of(*h).as_deref() == Some(ICON)).collect();
    let by_class = views.iter().copied().find(|v| descendants(*v).iter().any(|d| tree::type_of(*d).is_some_and(|t| t.contains("Battery"))));
    by_class.or_else(|| {
        views.iter().copied().find(|v| {
            let blocks: Vec<u64> = descendants(*v).into_iter().filter(|d| tree::type_of(*d).is_some_and(|t| t.ends_with("TextBlock"))).collect();
            !blocks.is_empty() && blocks.iter().all(|b| tree::name_of(*b).unwrap_or_default().is_empty())
        })
    })
}

/// Nothing left to wire: off (and put back), or every battery wired and split (or left whole).
pub fn settled() -> bool {
    if !ENABLED.load(Ordering::SeqCst) {
        return !UNSPLIT_PENDING.load(Ordering::SeqCst);
    }
    let wired = crate::lock(&WIRED).clone();
    let split = crate::lock(&SPLIT).iter().map(|s| s.icon).collect::<Vec<_>>();
    let failed = UNSPLITTABLE.sections();
    let missing = crate::lock(&MISSING).clone();
    found().into_iter().all(|(section, icon)| match icon {
        Some(icon) => wired.contains(&icon) && (split.contains(&icon) || failed.contains(&section)),
        None => missing.contains(&section),
    })
}

/// Wire and split every battery not yet done; then show what the app asked of it.
///
/// # Safety
/// XAML UI thread only, behind the wiring gate.
pub unsafe fn wire(diagnostics: &xamlom::IXamlDiagnostics) {
    if !ENABLED.load(Ordering::SeqCst) {
        if UNSPLIT_PENDING.swap(false, Ordering::SeqCst) {
            restore(diagnostics);
        }
        return;
    }
    // Elements gone from the tree (a taskbar rebuilt) are forgotten: their handles can come back.
    crate::lock(&WIRED).retain(|h| tree::type_of(*h).is_some());
    for (section, icon) in found() {
        let Some(icon) = icon else {
            let mut missing = crate::lock(&MISSING);
            if !missing.contains(&section) {
                missing.push(section);
                drop(missing);
                logf!("battery: no battery icon in the Quick Settings button 0x{section:x} (none on this PC, or another build)");
                if !DESCRIBED.swap(true, Ordering::SeqCst) {
                    crate::part::describe(section, "battery: the Quick Settings button");
                }
            }
            continue;
        };
        if !crate::lock(&WIRED).contains(&icon) {
            crate::lock(&WIRED).push(icon);
            let wired = crate::interact::attach_battery(diagnostics, icon);
            logf!("battery: icon 0x{icon:x} {}", if wired { "wired" } else { "could not be wired" });
            LOOK_PENDING.store(true, Ordering::SeqCst);
            if !DESCRIBED.swap(true, Ordering::SeqCst) {
                crate::part::describe(section, "battery: the Quick Settings button");
            }
        }
        split(diagnostics, section, icon);
    }
    if LOOK_PENDING.load(Ordering::SeqCst) {
        apply_look(diagnostics);
    }
}

// ---- Two buttons: the rest of Quick Settings and the battery, lit apart ----

/// The button split, and what was there before (put back by [`restore`]).
struct Split {
    section: u64,
    plate: u64,
    icon: u64,
    /// The battery's grid, made a plate, and what it held before.
    grid: SavedGrid,
    /// The plate's margin before.
    plate_margin: Thickness,
    /// Where the plate's right edge sat inside its parent besides its margin (a parent's padding):
    /// measured once, as the shell laid it out.
    plate_pad: f64,
    /// The plate's border width, which the battery's plate copies.
    edge: f64,
}

static SPLIT: Mutex<Vec<Split>> = Mutex::new(Vec::new());
/// Sections whose button could not be split (missing parts, an unexpected layout): left whole.
static UNSPLITTABLE: Unsplittable = Unsplittable::new(WHO, "the Quick Settings button");

/// Not laid out yet: tried again at the next sweep, until [`SPLIT_TRIES`] sweeps of an unchanged
/// tree have passed, then left whole.
fn not_laid_out(section: u64) {
    let version = tree::version();
    if TRIED_AT.swap(version, Ordering::SeqCst) != version {
        TRIES.store(0, Ordering::SeqCst);
    }
    if TRIES.fetch_add(1, Ordering::SeqCst) + 1 >= SPLIT_TRIES {
        UNSPLITTABLE.note(section, "never laid out");
    }
}

/// Where `handle` sits inside `ancestor`, in DIPs from its left, summing each level's arranged
/// offset; `None` if a level cannot be read or `ancestor` is not above it.
///
/// # Safety
/// XAML UI thread only.
unsafe fn offset_in(diagnostics: &xamlom::IXamlDiagnostics, handle: u64, ancestor: u64) -> Option<f64> {
    use crate::winrt::{IUIElement10, Vector3};
    let mut x = 0.0f64;
    let mut at = handle;
    for _ in 0..24 {
        if at == ancestor {
            return Some(x);
        }
        let element = xaml::element::<IUIElement10>(diagnostics, at)?;
        let mut offset = Vector3::default();
        if element.get_ActualOffset(&mut offset).is_err() {
            return None;
        }
        x += offset.x as f64;
        at = tree::parent_of(at)?;
    }
    None
}

/// Split `section`'s button (once per battery icon).
///
/// # Safety
/// XAML UI thread only.
unsafe fn split(diagnostics: &xamlom::IXamlDiagnostics, section: u64, icon: u64) {
    if UNSPLITTABLE.contains(section) {
        return;
    }
    {
        let mut split = crate::lock(&SPLIT);
        if split.iter().any(|s| s.icon == icon) {
            return;
        }
        // A battery rebuilt in a section split before: that split is put back, and the new one is
        // measured at a later sweep, once the layout has followed (measured now, the widths would
        // still be the shrunk plate's).
        if let Some(at) = split.iter().position(|s| s.section == section) {
            let old = split.remove(at);
            drop(split);
            put_back(diagnostics, &old);
            return;
        }
    }
    let Some(plate) = named(section, PLATE) else { return UNSPLITTABLE.note(section, "no plate") };
    let Some(grid) = named(icon, ICON_GRID) else { return UNSPLITTABLE.note(section, "no icon grid") };
    let Some(parent) = tree::parent_of(plate) else { return UNSPLITTABLE.note(section, "the plate has no parent") };
    let Some(to_plate) = crate::part::to_plate(diagnostics, grid) else {
        return UNSPLITTABLE.note(section, "the icon grid's interfaces");
    };
    // Laid out yet? Measures before the first layout read zero: tried again at the next sweep.
    let width = |h: u64| xaml::actual_width(diagnostics, h).unwrap_or(0.0);
    let (plate_w, parent_w, icon_w) = (width(plate), width(parent), width(icon));
    if plate_w <= 0.0 || parent_w <= 0.0 || icon_w <= 0.0 {
        return not_laid_out(section);
    }
    let (Some(plate_x), Some(parent_x), Some(icon_x)) = (offset_in(diagnostics, plate, section), offset_in(diagnostics, parent, section), offset_in(diagnostics, icon, section)) else {
        return UNSPLITTABLE.note(section, "where its parts are");
    };
    let (margin, radius, edge) = crate::part::plate_shape(diagnostics, plate);
    let plate_pad = parent_x + parent_w - (plate_x + plate_w + margin.right);
    logf!(
        "battery: splitting the Quick Settings button: plate x {plate_x:.1} w {plate_w:.1} margin {margin:?} pad {plate_pad:.1} radius {}, battery at x {icon_x:.1}",
        radius.top_left
    );
    if icon_x <= plate_x + SLACK || icon_x >= plate_x + plate_w {
        // The battery is not at the plate's right end: the plate cannot stop short of it alone.
        return UNSPLITTABLE.note(section, "the battery is not the last icon");
    }
    // The battery's plate: inset as the shell's (its left half the gap), its corners, its edge.
    let inset = Thickness { left: PLATE_GAP / 2.0, top: margin.top, right: 0.0, bottom: margin.bottom };
    let saved = crate::part::make_plate(to_plate, inset, radius, edge);
    // Filled and faded as the shell's plate is.
    crate::part::dress_like_plate(diagnostics, plate, &[grid]);
    crate::part::watch_plate(diagnostics, plate, "battery: the plate callback", relight_from_plate);
    crate::lock(&SPLIT).push(Split { section, plate, icon, grid: saved, plate_margin: margin, plate_pad, edge });
    TRIES.store(0, Ordering::SeqCst);
    light(diagnostics);
}

/// The shell's plate changed its look: while the pointer is on the battery, its plate follows.
fn relight_from_plate() {
    if OVER.load(Ordering::SeqCst) {
        if let Some(diagnostics) = crate::diagnostics() {
            unsafe { light(&diagnostics) };
        }
    }
}

/// The plate's right margin that ends it [`PLATE_GAP`] before the battery's plate, from where XAML
/// arranged the battery and the plate's parent now (neither moves with that margin, so writing it
/// again changes nothing). `None` when the measures make no sense.
///
/// # Safety
/// XAML UI thread only.
unsafe fn plate_right(diagnostics: &xamlom::IXamlDiagnostics, split: &Split) -> Option<f64> {
    let parent = tree::parent_of(split.plate)?;
    let parent_x = offset_in(diagnostics, parent, split.section)?;
    let parent_w = xaml::actual_width(diagnostics, parent)?;
    let icon_x = offset_in(diagnostics, split.icon, split.section)?;
    // The battery's plate starts at its margin's left ([`split`]).
    let ends_at = icon_x + PLATE_GAP / 2.0 - split.edge - PLATE_GAP;
    let right = parent_x + parent_w - split.plate_pad - ends_at;
    (right > split.plate_margin.right && right < parent_w - SLACK).then_some(right)
}

/// The battery's plate lit as it should be now, and the shell's ending before it.
///
/// # Safety
/// XAML UI thread only.
unsafe fn light(diagnostics: &xamlom::IXamlDiagnostics) {
    use crate::winrt::IFrameworkElement;
    let over = OVER.load(Ordering::SeqCst);
    let open = batteries_open();
    for split in crate::lock(&SPLIT).iter() {
        // Written only when it differs: the battery moves when an icon left of it comes or goes.
        if let (Some(right), Some(framework)) = (plate_right(diagnostics, split), xaml::element::<IFrameworkElement>(diagnostics, split.plate)) {
            let mut now = Thickness::default();
            let _ = framework.get_Margin(&mut now);
            if (now.right - right).abs() > 0.25 {
                let _ = framework.put_Margin(Thickness { right, ..split.plate_margin });
            }
        }
        let look = if over {
            let now = crate::part::plate_look(diagnostics, split.plate);
            if let Some(look) = &now {
                crate::part::remember_look(WHO, look);
            }
            now
        } else if open {
            crate::part::open_look(WHO)
        } else {
            None
        };
        crate::part::wear(diagnostics, split.grid.grid, look.as_ref());
        // The shell's plate lights over the whole button: on the battery, only the battery's shows.
        xaml::set_opacity(diagnostics, split.plate, if over { 0.0 } else { 1.0 });
    }
}

/// The pointer entered or left the battery: its plate follows.
pub fn hover(over: bool) {
    OVER.store(over, Ordering::SeqCst);
    if let Some(diagnostics) = crate::diagnostics() {
        unsafe { light(&diagnostics) };
    }
}

/// audio-tray opened, swapped or closed a panel ([`crate::clock::open_panel`] has it): the battery
/// stays lit while the batteries are open; a tooltip that came meanwhile goes on once none is.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn panel_changed(diagnostics: &xamlom::IXamlDiagnostics) {
    light(diagnostics);
    if !panel_open() {
        apply_look(diagnostics);
    }
}

/// Put one split back as it was.
///
/// # Safety
/// XAML UI thread only.
unsafe fn put_back(diagnostics: &xamlom::IXamlDiagnostics, split: &Split) {
    use crate::winrt::IFrameworkElement;
    xaml::set_opacity(diagnostics, split.plate, 1.0);
    if let Some(framework) = xaml::element::<IFrameworkElement>(diagnostics, split.plate) {
        let _ = framework.put_Margin(split.plate_margin);
    }
    crate::part::restore_grid(diagnostics, &split.grid);
}

/// Put the button back whole, the battery's tooltip and badge with it (a revert, or turned off).
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn restore(diagnostics: &xamlom::IXamlDiagnostics) {
    for split in std::mem::take(&mut *crate::lock(&SPLIT)) {
        put_back(diagnostics, &split);
    }
    UNSPLITTABLE.clear();
    crate::lock(&MISSING).clear();
    TRIES.store(0, Ordering::SeqCst);
    OVER.store(false, Ordering::SeqCst);
    // Shown again once wired again (a handover re-applying, or turned back on).
    LOOK_PENDING.store(true, Ordering::SeqCst);
    give_back_tips(diagnostics);
    crate::part::take_brushes(diagnostics, WHO);
}

// ---- What it says: the tooltip and the low dot ----

/// What the app asked the icon to say.
#[derive(Clone, Default, PartialEq)]
struct Wanted {
    /// Empty: the shell's own tooltip.
    tip: String,
    /// The dot's PNG and its pixels per DIP (shown at its pixel size); `None`: no dot.
    badge: Option<(String, f64)>,
}

static WANTED: Mutex<Option<Wanted>> = Mutex::new(None);

/// A tooltip of ours on one of the shell's elements, and the one it replaced.
struct Tip {
    owner: u64,
    shell: Option<windows_core::IInspectable>,
    text: String,
}

thread_local! {
    /// Tooltips put on the battery (the XAML thread's objects).
    static TIPS: std::cell::RefCell<Vec<Tip>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The app said what the icon says (`tap_proto::BATTERY_MAGIC` pairs); shown at once where it can.
/// A `badge_width` is tolerated and unused: the dot is shown at its pixel size.
pub fn set_look(data: &str) {
    let badge = tap_proto::init_value(data, key::BADGE_IMAGE).map(|path| {
        let scale = tap_proto::init_value(data, key::BADGE_SCALE).and_then(|v| v.parse().ok()).unwrap_or(1.0);
        (path, scale)
    });
    let wanted = Wanted { tip: tap_proto::init_value(data, key::TIP).unwrap_or_default(), badge };
    *crate::lock(&WANTED) = Some(wanted);
    LOOK_PENDING.store(true, Ordering::SeqCst);
    if let Some(diagnostics) = crate::diagnostics() {
        unsafe { apply_look(&diagnostics) };
    }
}

/// Show what the app asked on every battery icon found (the tooltip only while no panel is open).
///
/// # Safety
/// XAML UI thread only.
unsafe fn apply_look(diagnostics: &xamlom::IXamlDiagnostics) {
    if !ENABLED.load(Ordering::SeqCst) || !crate::live() {
        return;
    }
    let Some(wanted) = crate::lock(&WANTED).clone() else { return };
    let icons = icons();
    // Done once shown on an icon, the tooltip too (it waits while a panel is open).
    LOOK_PENDING.store(icons.is_empty() || panel_open(), Ordering::SeqCst);
    for icon in icons {
        if !panel_open() {
            set_tip(diagnostics, icon, &wanted.tip);
        }
        if let Some(grid) = named(icon, INNER_GRID) {
            show_badge(diagnostics, grid, wanted.badge.as_ref().map(|(p, s)| (p.as_str(), *s)));
        }
    }
}

/// Put `text` on the battery as its tooltip (empty: the shell's back). It goes where the shell's
/// is (the icon view, or the element under it that carries one).
///
/// # Safety
/// XAML UI thread only.
unsafe fn set_tip(diagnostics: &xamlom::IXamlDiagnostics, icon: u64, text: &str) {
    use crate::part::{set_tooltip, tooltip_of};
    let mut tips = TIPS.with(|t| std::mem::take(&mut *t.borrow_mut()));
    // Owners gone (a taskbar rebuilt) are forgotten.
    tips.retain(|t| tree::type_of(t.owner).is_some());
    let under: Vec<u64> = std::iter::once(icon).chain(descendants(icon)).collect();
    let mine = tips.iter().position(|t| under.contains(&t.owner));
    match (mine, text.is_empty()) {
        (Some(at), true) => {
            let tip = tips.remove(at);
            set_tooltip(diagnostics, tip.owner, tip.shell.as_ref());
        }
        (None, true) => {}
        (Some(at), false) if tips[at].text == text => {}
        (found, false) => {
            // The element carrying the shell's tooltip (or the one ours went on before).
            let owner = match found {
                Some(at) => tips[at].owner,
                None => under.iter().copied().find(|h| tooltip_of(diagnostics, *h).is_some()).unwrap_or(icon),
            };
            if xaml::element::<crate::winrt::IDependencyObject>(diagnostics, owner).is_none() {
                TIPS.with(|t| *t.borrow_mut() = tips);
                return;
            }
            let markup = format!(r#"<ToolTip xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" Content="{}"/>"#, crate::part::xml_attr(text));
            let Some(ours) = xaml::load_xaml(&markup) else {
                logf!("battery: the tooltip would not load");
                TIPS.with(|t| *t.borrow_mut() = tips);
                return;
            };
            let shell = match found {
                Some(at) => tips.remove(at).shell,
                None => tooltip_of(diagnostics, owner),
            };
            if set_tooltip(diagnostics, owner, Some(&ours)) {
                tips.push(Tip { owner, shell, text: text.to_string() });
            } else {
                logf!("battery: the tooltip was refused");
            }
        }
    }
    TIPS.with(|t| *t.borrow_mut() = tips);
}

/// The shell's tooltips back on the battery.
///
/// # Safety
/// XAML UI thread only.
unsafe fn give_back_tips(diagnostics: &xamlom::IXamlDiagnostics) {
    for tip in TIPS.with(|t| std::mem::take(&mut *t.borrow_mut())) {
        crate::part::set_tooltip(diagnostics, tip.owner, tip.shell.as_ref());
    }
}

/// The dot at the glyph's upper right (`image`: the PNG and its pixels per DIP), or taken off: the
/// inner grid's background (drawn under the glyph, so it sits clear of it), at its pixel size
/// scaled back by `1 / scale`.
///
/// # Safety
/// XAML UI thread only.
unsafe fn show_badge(diagnostics: &xamlom::IXamlDiagnostics, grid: u64, image: Option<(&str, f64)>) {
    let key = image.map_or("", |(path, _)| path);
    crate::part::set_grid_brush(diagnostics, grid, WHO, key, || {
        let (path, scale) = image?;
        crate::part::image_brush(path, scale, ("Right", "Top"), (1.0, 0.0), 0.0)
    });
}

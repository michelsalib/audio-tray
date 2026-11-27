//! The bell beside the clock: its badge (the unread count, on its corner), its glyph (crossed while
//! notifications are quiet, filled while anything waits in Audio Tray's list), and its ring.

use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::log::logf;
use crate::part::{GLYPH_BLOCK, ICON_GRID, INNER_GRID};
use crate::tree::{descendants, named};
use crate::winrt::{IFrameworkElement, Thickness};
use crate::{tree, xaml, xamlom};

/// What the bell is widened by while it wears a badge, in DIPs: the badge sits beside the glyph,
/// at its upper right (a grid's background is drawn under its children, so it cannot go over it).
const BELL_ROOM: f64 = 10.0;
/// The bell's glyph, moved left by the room while the badge shows.
const BELL_GLYPH: &str = "SystemTray.TextIconContent";

/// The bell's glyph in `bell` (what the ring swings and the badge's room moves).
pub(super) fn glyph_of(bell: u64) -> Option<u64> {
    descendants(bell).into_iter().find(|h| tree::type_of(*h).as_deref() == Some(BELL_GLYPH))
}

// ---- The ring ----

/// The bell's ring: the glyph's sideways offsets (DIPs), one per [`RING_MS`], a swing dying down.
const RING: [f64; 9] = [2.5, -2.5, 2.0, -2.0, 1.4, -1.4, 0.8, -0.6, 0.0];
pub const RING_MS: u32 = 45;
/// The ring's next step (`RING.len()`: none running).
static RING_STEP: AtomicUsize = AtomicUsize::new(RING.len());

/// A notification went to the list: the bell rings ([`ring_step`] from the control window's
/// timer). Only the glyph moves, within its own room: nothing else on the taskbar does.
pub fn ring() {
    RING_STEP.store(0, Ordering::SeqCst);
}

/// One step of the ring; `false` once it has died down.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn ring_step() -> bool {
    let step = RING_STEP.load(Ordering::SeqCst);
    let Some(&offset) = RING.get(step) else { return false };
    RING_STEP.store(step + 1, Ordering::SeqCst);
    let Some(diagnostics) = crate::diagnostics() else { return false };
    for bell in super::bells() {
        let Some(glyph) = glyph_of(bell) else { continue };
        // The badge's room stays as [`show_bell_badge`] set it: the swing is on top of it.
        let room = named(bell, INNER_GRID).filter(|g| crate::part::wears_brush(*g)).map_or(0.0, |_| BELL_ROOM);
        if let Some(framework) = xaml::element::<IFrameworkElement>(&diagnostics, glyph) {
            let _ = framework.put_Margin(Thickness { left: offset, right: room - offset, ..Default::default() });
        }
    }
    step + 1 < RING.len()
}

// ---- The badge ----

/// How far below the bell's middle the badge's middle sits, in DIPs (negative: above): level with
/// the glyph's top.
const BELL_BADGE_DROP: f64 = -5.0;

/// The badge beside the bell's glyph, at its upper right (`image`: the PNG, its width in DIPs, its
/// pixels per DIP), or taken off. It is the background of the bell's inner grid, the bell widened
/// by [`BELL_ROOM`] and its glyph moved left meanwhile; the image is shown at its pixel size and
/// scaled back by `1 / scale`.
///
/// # Safety
/// XAML UI thread only.
pub(super) unsafe fn show_bell_badge(diagnostics: &xamlom::IXamlDiagnostics, bell: u64, image: Option<(&str, f64, f64)>) {
    // The inner grid: the outer one is the bell's plate ([`super::plate`]).
    let Some(grid) = named(bell, INNER_GRID) else { return };
    // Room for it: the bell wider (once split, whose width it knows), the glyph to the left. Equal
    // values are no change to XAML, so writing them at each tick costs nothing.
    let room = if image.is_some() { BELL_ROOM } else { 0.0 };
    let outer = named(bell, ICON_GRID);
    let base = outer.and_then(super::plate::bell_base);
    if let (Some(outer), Some((width, min))) = (outer, base) {
        if let Some(framework) = xaml::element::<IFrameworkElement>(diagnostics, outer) {
            let wanted = if room > 0.0 { width + room } else { min };
            let mut was = 0.0;
            let _ = framework.get_MinWidth(&mut was);
            if (was - wanted).abs() > 0.5 {
                let _ = framework.put_MinWidth(wanted);
                // The clock and the bell moved: the rects the app keeps are out of date.
                crate::ipc::forget_part_rects();
            }
        }
        if let Some(framework) = glyph_of(bell).and_then(|g| xaml::element::<IFrameworkElement>(diagnostics, g)) {
            // Written only when it differs (each tick asks; a write is work for XAML).
            let wanted = Thickness { right: room, ..Default::default() };
            let mut now = Thickness::default();
            let _ = framework.get_Margin(&mut now);
            if now != wanted && RING_STEP.load(Ordering::SeqCst) >= RING.len() {
                let _ = framework.put_Margin(wanted);
            }
        }
    }
    let key = image.map_or("", |(path, ..)| path);
    crate::part::set_grid_brush(diagnostics, grid, "clock", key, || {
        let (path, _, scale) = image?;
        // At the glyph's upper right: from the grid's right edge and middle, moved by
        // BELL_BADGE_DROP (relative to its height; negative is up).
        let height = xaml::actual_size(diagnostics, grid).map_or(0.0, |(_, h)| h);
        let drop = if height > 0.0 { BELL_BADGE_DROP / height } else { 0.0 };
        crate::part::image_brush(path, scale, ("Right", "Center"), (1.0, 0.5), drop)
    });
}

// ---- The glyph ----

/// The crossed bell (Segoe's `RingerSilent`), drawn while Windows keeps notifications quiet.
const CROSSED_BELL: &str = "\u{E7ED}";
/// The bell's glyph layer (the shell's filled bell when something is new).
const BASE_LAYER: &str = "Base";

/// The shell's bell with something new (its `Base` layer, filled, in the accent; the underlay and
/// the accent overlay stay empty).
const SHELL_BELL: &str = "\u{F2A5}";
/// The shell's ordinary bell, nothing new (an outline); also the TAP's with nothing waiting in
/// Audio Tray's list. Any glyph other than these two (and the cross) is its quiet bell ("zz"):
/// Windows keeps notifications quiet by a rule of its own (a full-screen app, a game, a
/// presentation) that Audio Tray is not told of.
const SHELL_PLAIN_BELL: &str = "\u{F2A3}";

/// Each bell layer whose glyph was crossed, and the shell's glyph to put back.
static UNCROSSED: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());
/// The bells whose shell drew its own quiet bell, until it draws its ordinary one again.
static SHELL_QUIET: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// The bell's glyph layers (`InnerTextBlock`s under its base, accent and underlay).
fn layers_of(bell: u64) -> impl Iterator<Item = u64> {
    descendants(bell).into_iter().filter(|h| tree::name_of(*h).as_deref() == Some(GLYPH_BLOCK))
}

/// The glyph's code points, for the log.
fn code_points(glyph: &str) -> String {
    glyph.chars().map(|c| format!("U+{:04X}", c as u32)).collect::<Vec<_>>().join(" ")
}

/// Draw `bell` crossed while notifications are quiet, or with the shell's own glyph again. Quiet
/// is what Audio Tray says (`quiet`), or what the shell draws itself: its "zz" bell is crossed
/// too, so the bell always says quiet the same way. Every layer takes the same glyph, so the
/// accent fill (anything new) shows on the crossed bell too. Written only when it differs, and
/// again whenever the shell draws over it.
///
/// # Safety
/// XAML UI thread only.
pub(super) unsafe fn cross_bell(diagnostics: &xamlom::IXamlDiagnostics, bell: u64, quiet: bool) {
    let layers: Vec<u64> = layers_of(bell).collect();
    // Every rewrite by the shell calls back (the shared text callback looks again,
    // [`super::text::tick`]): its quiet bell is crossed at once, and the cross goes when it draws
    // its ordinary bell again.
    for &layer in &layers {
        watch_glyph(diagnostics, layer);
    }
    let texts: Vec<(u64, String)> = layers.iter().filter_map(|&l| Some((l, xaml::text_of(diagnostics, l)?))).collect();
    // What the shell itself says, from what it drew last (the cross is ours, an empty layer
    // says nothing).
    {
        let mut shell_quiet = crate::lock(&SHELL_QUIET);
        let drawn = texts.iter().map(|(_, t)| t.as_str()).find(|t| !t.is_empty() && *t != CROSSED_BELL);
        let was = shell_quiet.contains(&bell);
        // The shell's bell, or Audio Tray's outline in its place ([`fill_bell`]): ordinary.
        let ordinary = |g: &str| g == SHELL_BELL || g == SHELL_PLAIN_BELL;
        match drawn {
            Some(glyph) if ordinary(glyph) && was => {
                shell_quiet.retain(|b| *b != bell);
                logf!("clock: the shell's own quiet is over");
            }
            Some(glyph) if !ordinary(glyph) && !was => {
                shell_quiet.push(bell);
                logf!("clock: the shell went quiet on its own ({}); the bell is crossed", code_points(glyph));
            }
            _ => {}
        }
    }
    let cross = quiet || crate::lock(&SHELL_QUIET).contains(&bell);
    let mut uncrossed = crate::lock(&UNCROSSED);
    for (layer, now) in texts {
        if cross {
            if now == CROSSED_BELL {
                continue;
            }
            // What goes back is the shell's ordinary bell, never its quiet one.
            if !uncrossed.iter().any(|(h, _)| *h == layer) {
                let back = if now.is_empty() || now == SHELL_BELL { now } else { SHELL_BELL.to_string() };
                uncrossed.push((layer, back));
            }
            xaml::set_text(diagnostics, layer, CROSSED_BELL);
        } else if let Some(at) = uncrossed.iter().position(|(h, _)| *h == layer) {
            let (_, glyph) = uncrossed.remove(at);
            // Drawn over by the shell already: its glyph stays.
            if now == CROSSED_BELL {
                xaml::set_text(diagnostics, layer, &glyph);
            }
        }
    }
}

/// The bell's "something new" look (the shell's filled glyph, in the accent): the shell's says what
/// *it* has not shown, which it counts as seen only when its own centre opens, never now that the
/// bell opens Audio Tray's. So it follows Audio Tray's list instead (`Some(waiting)`): the shell's
/// filled bell while anything waits there, its plain one otherwise. `None`: the bell is not Audio
/// Tray's, so the glyph the shell drew is given back (saved before the first write) and left alone.
/// What the bell's layers hold is logged at each change.
///
/// # Safety
/// XAML UI thread only.
pub(super) unsafe fn fill_bell(diagnostics: &xamlom::IXamlDiagnostics, bell: u64, waiting: Option<bool>) {
    /// The layers' texts as last logged.
    static LAYERS_SEEN: Mutex<String> = Mutex::new(String::new());
    /// Each glyph block written here, and the shell's glyph in it before.
    static SHELL_GLYPHS: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());
    let Some(base) = named(bell, BASE_LAYER) else { return };
    let Some(glyph) = descendants(base).into_iter().find(|h| tree::name_of(*h).as_deref() == Some(GLYPH_BLOCK)) else { return };
    let now = xaml::text_of(diagnostics, glyph).unwrap_or_default();
    let Some(filled) = waiting else {
        // Not Audio Tray's: what the shell drew, back (unless it has drawn anew since).
        let mut saved = crate::lock(&SHELL_GLYPHS);
        if let Some(at) = saved.iter().position(|(g, _)| *g == glyph) {
            let (_, shell) = saved.remove(at);
            if now == SHELL_BELL || now == SHELL_PLAIN_BELL {
                xaml::set_text(diagnostics, glyph, &shell);
            }
        }
        return;
    };
    {
        let mut seen = crate::lock(&LAYERS_SEEN);
        let layers = format!("{} (Audio Tray has {})", code_points(&now), if filled { "some waiting" } else { "none waiting" });
        if *seen != layers {
            logf!("clock: the bell's glyph now: {layers}");
            *seen = layers;
        }
    }
    // The cross, when quiet, takes over ([`cross_bell`] writes every layer then, and leaves this
    // glyph alone). Looked at each tick (the cross ending puts back what it found), written only
    // when it differs.
    if now == CROSSED_BELL {
        return;
    }
    let wanted = if filled { SHELL_BELL } else { SHELL_PLAIN_BELL };
    if now != wanted {
        let mut saved = crate::lock(&SHELL_GLYPHS);
        if !saved.iter().any(|(g, _)| *g == glyph) {
            saved.push((glyph, now.clone()));
        }
        xaml::set_text(diagnostics, glyph, wanted);
    }
}

/// `bell` with the shell's own glyph again, whatever the quiet (the TAP lets go).
///
/// # Safety
/// XAML UI thread only.
pub(super) unsafe fn uncross_bell(diagnostics: &xamlom::IXamlDiagnostics, bell: u64) {
    crate::lock(&SHELL_QUIET).retain(|b| *b != bell);
    let mut uncrossed = crate::lock(&UNCROSSED);
    for layer in layers_of(bell) {
        if let Some(at) = uncrossed.iter().position(|(h, _)| *h == layer) {
            let (_, glyph) = uncrossed.remove(at);
            if xaml::text_of(diagnostics, layer).as_deref() == Some(CROSSED_BELL) {
                xaml::set_text(diagnostics, layer, &glyph);
            }
        }
    }
}

/// Bell glyph layers that call back when the shell rewrites them (by handle and element; never
/// detached: the callback only asks for a look, which crosses the bell only while it is quiet).
static GLYPHS_WATCHED: Mutex<Vec<(u64, usize)>> = Mutex::new(Vec::new());

/// Have bell layer `block` call back on a change of its text (once per element).
///
/// # Safety
/// XAML UI thread only.
unsafe fn watch_glyph(diagnostics: &xamlom::IXamlDiagnostics, block: u64) {
    use windows_core::Interface;
    let Some(object) = xaml::element::<crate::winrt::IDependencyObject2>(diagnostics, block) else { return };
    let identity = object.as_raw() as usize;
    if crate::lock(&GLYPHS_WATCHED).contains(&(block, identity)) {
        return;
    }
    let Some(dp) = xaml::text_property() else { return };
    if crate::part::on_change(diagnostics, block, &dp, super::text::TEXT_CALLBACK, super::text::text_changed) {
        let mut watched = crate::lock(&GLYPHS_WATCHED);
        // Gone from the tree (a taskbar rebuilt): forgotten.
        watched.retain(|(h, _)| tree::type_of(*h).is_some() && *h != block);
        watched.push((block, identity));
    }
}

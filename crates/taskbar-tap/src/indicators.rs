//! Explorer's own indicators that audio-tray hides (the volume glyph, the "microphone in use" icon,
//! the keyboard-layout indicator): found from the tree's add events (queued by the callback, read
//! by the sweep), collapsed with their slot once the strip is up, re-checked every sweep (the shell
//! reuses an icon for another indicator without a tree event), and put back when no longer theirs.
//! See FINDINGS "Telling the shell's tray icons apart".

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

use crate::decorate;
use crate::log::logf;
use crate::part::{GLYPH_BLOCK, ICON, QUICK_SETTINGS};
use crate::{lock, reorder, restore, tree, xaml, xamlom};

/// One of Explorer's own indicators that audio-tray hides: found by glyph or by content class
/// within its tray section, collapsed with its slot, restorable.
struct Indicator {
    what: &'static str,
    /// `x:Name` of the `SystemTrayFrameGrid` child it lives in. The same codepoint can mean
    /// something else in another section.
    section: &'static str,
    /// Codepoints that identify it. Names are localised; glyphs are not. Empty for an indicator
    /// found by [`Self::classes`].
    glyphs: &'static [char],
    /// Content classes (the `IconView`'s content, under `ContentGrid`) that identify it, for an
    /// indicator with no glyph. Empty for one found by glyph.
    classes: &'static [&'static str],
    /// Hidden only once our strip is placed: the indicators the strip *replaces*. See FINDINGS
    /// "Only touch the shell's UI once ours is up".
    needs_strip: bool,
    /// Whether audio-tray asked for this one to be hidden.
    wanted: fn(&decorate::StripState) -> bool,
    /// Where it was found, once found.
    slot: Mutex<Option<Found>>,
    /// Collapse re-applications for the current slot. Bounded, because a collapsed element keeps
    /// reporting its old `ActualWidth`, so success cannot be read back.
    retries: AtomicU32,
    /// Whether it has been logged as found.
    announced: AtomicBool,
}

/// An indicator's elements: what identified it (the glyph's `TextBlock`, re-read every sweep since
/// the shell reuses an `IconView` for another indicator by changing its text; or the content
/// element whose class matched), the `IconView`, and the container holding its slot.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Found {
    marker: xamlom::InstanceHandle,
    icon: xamlom::InstanceHandle,
    slot: xamlom::InstanceHandle,
}

impl Indicator {
    /// One the strip replaces, found by glyph.
    const fn by_glyph(
        what: &'static str,
        section: &'static str,
        glyphs: &'static [char],
        wanted: fn(&decorate::StripState) -> bool,
    ) -> Self {
        Self::new(what, section, glyphs, &[], true, wanted)
    }

    const fn new(
        what: &'static str,
        section: &'static str,
        glyphs: &'static [char],
        classes: &'static [&'static str],
        needs_strip: bool,
        wanted: fn(&decorate::StripState) -> bool,
    ) -> Self {
        Self {
            what,
            section,
            glyphs,
            classes,
            needs_strip,
            wanted,
            slot: Mutex::new(None),
            retries: AtomicU32::new(0),
            announced: AtomicBool::new(false),
        }
    }

    fn recorded(&self) -> Option<Found> {
        *lock(&self.slot)
    }

    /// Whether this text, read from an `IconView` in `section`, is this indicator: exactly one
    /// character, one of ours. Anything else (longer text, an unknown glyph) is never hidden.
    fn matches(&self, section: &str, text: &str) -> bool {
        section == self.section && single_glyph(text).is_some_and(|glyph| self.glyphs.contains(&glyph))
    }

    /// Record where it is. A *different* element than last time restarts the retry budget (the
    /// microphone indicator may be rebuilt per recording session).
    fn record(&self, found: Found) {
        let mut held = lock(&self.slot);
        if *held != Some(found) {
            self.retries.store(0, Ordering::SeqCst);
        }
        *held = Some(found);
    }

    fn forget(&self) {
        *lock(&self.slot) = None;
        self.retries.store(0, Ordering::SeqCst);
        self.announced.store(false, Ordering::SeqCst);
    }
}

/// Enough to cover the replay burst without re-applying for the process lifetime.
const HIDE_MAX_RETRIES: u32 = 24;

static SYSTEM_VOLUME: Indicator = Indicator::by_glyph(
    "volume",
    QUICK_SETTINGS,
    decorate::VOLUME_GLYPHS,
    |s| s.hide_system_volume,
);
static SYSTEM_MIC: Indicator =
    Indicator::by_glyph("microphone", "MainStack", decorate::MIC_GLYPHS, |s| s.hide_system_mic);
/// The keyboard-layout indicator ("FRA"). It has no glyph, so it is found by its content class;
/// the IME mode icons beside it (`SystemTray.TextIconContent` / `ImageIconContent`) are left
/// alone. Not replaced by the strip, so hidden whenever the TAP is active, `buttons=0` included.
static SYSTEM_LANGUAGE: Indicator = Indicator::new(
    "keyboard layout",
    "NonActivatableStack",
    &[],
    LANGUAGE_CONTENT,
    false,
    |s| s.hide_system_language,
);

/// The content classes of the keyboard-layout indicator: as text, or drawn as an image.
pub(crate) const LANGUAGE_CONTENT: &[&str] =
    &["SystemTray.LanguageTextIconContent", "SystemTray.LanguageImageIconContent"];

/// The text's only character, if it is exactly one (how the shell draws a system icon).
fn single_glyph(text: &str) -> Option<char> {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(glyph), None) => Some(glyph),
        _ => None,
    }
}

fn indicators() -> [&'static Indicator; 3] {
    [&SYSTEM_VOLUME, &SYSTEM_MIC, &SYSTEM_LANGUAGE]
}

/// Event handles the callback queued for the sweep (the callback may not touch XAML). Pushed only
/// from the tray island's thread.
pub(crate) static PENDING_GLYPHS: Mutex<Vec<xamlom::InstanceHandle>> = Mutex::new(Vec::new());
/// Content elements whose class may identify an indicator ([`LANGUAGE_CONTENT`]).
pub(crate) static PENDING_CONTENTS: Mutex<Vec<xamlom::InstanceHandle>> = Mutex::new(Vec::new());

/// The indicators are only ever found from an *add* event, which this Explorer will not replay
/// (and a stand-down forgot them): offer the recorded tray-icon glyphs and language contents to
/// the sweep again. Tray icons only: the queues are the tray island's, and other islands also
/// have `InnerTextBlock`s.
pub(crate) fn requeue_tray_glyphs() {
    let in_tray = |handle: &xamlom::InstanceHandle| tree::ancestor_of_type(*handle, ICON, 8).is_some();
    let glyphs: Vec<_> = tree::find_by_name(GLYPH_BLOCK).into_iter().filter(in_tray).collect();
    let contents: Vec<_> = LANGUAGE_CONTENT
        .iter()
        .flat_map(|class| tree::find_by_type(class))
        .filter(in_tray)
        .collect();
    for (queue, found) in [(&PENDING_GLYPHS, glyphs), (&PENDING_CONTENTS, contents)] {
        let mut queue = lock(queue);
        for handle in found {
            if !queue.contains(&handle) {
                queue.push(handle);
            }
        }
    }
}

/// Forget every indicator found, watched and reported (a stand-down: the feature can be turned
/// back on without restarting Explorer).
pub(crate) fn forget_all() {
    for indicator in indicators() {
        indicator.forget();
    }
    lock(&WATCHED).clear();
    lock(&UNKNOWN_GLYPHS).clear();
}

/// Note one of Explorer's own indicators if this text block is it.
///
/// Only *records* the slot, ungated; collapsing it is [`enforce_hidden`]'s job, gated on the
/// strip for the indicators it replaces. (The glyph is announced during the replay, long before
/// our icon is decorated.)
///
/// # Safety
/// XAML UI thread only, and only once the event stream is quiet — see [`crate::sweep`].
pub(crate) unsafe fn note_system_indicator(
    diagnostics: &xamlom::IXamlDiagnostics,
    text_block: xamlom::InstanceHandle,
) {
    let Some(state) = crate::strip_state() else {
        return;
    };
    // Only the shell's glyphs sit in a `SystemTray.IconView`; our own strip's, and other islands',
    // fall out here — before the XAML read, which is only valid on this island's elements.
    let Some(icon) = tree::ancestor_of_type(text_block, ICON, 8) else {
        return;
    };
    let Some(section) = section_of(icon) else {
        logf!("tray IconView 0x{icon:x} is in no recorded tray section — not matching it");
        return;
    };
    // Only sections we hide a glyph from are worth the read (the language indicator's "FRA" is
    // found by class, not text).
    if !indicators().iter().any(|i| i.section == section && !i.glyphs.is_empty()) {
        return;
    }
    let Some(text) = xaml::text_of(diagnostics, text_block) else {
        lock(&WATCHED).retain(|&known| known != text_block);
        return;
    };
    let found = indicators()
        .into_iter()
        .find(|i| (i.wanted)(&state) && i.matches(&section, &text));
    let Some(wanted) = found else {
        // In a section we hide a glyph from, it may turn into one of ours later without a tree
        // event.
        {
            let mut watched = lock(&WATCHED);
            if !watched.contains(&text_block) {
                watched.push(text_block);
            }
        }
        note_unknown_glyph(&section, &text);
        return;
    };
    lock(&WATCHED).retain(|&known| known != text_block);
    let glyph = single_glyph(&text).unwrap_or_default();
    let slot = slot_of(icon);
    if !wanted.announced.swap(true, Ordering::SeqCst) {
        logf!(
            "system {} indicator found: glyph {:04X} in {section} IconView 0x{icon:x}, slot 0x{slot:x}{}",
            wanted.what,
            glyph as u32,
            gate_note(wanted)
        );
    }
    // Remembered so it can be re-applied: a collapse before layout has measured it frees nothing.
    wanted.record(Found { marker: text_block, icon, slot });
    enforce_hidden(diagnostics);
}

/// The container to collapse with an `IconView`: the generated `ContentPresenter` around it keeps
/// its layout box, so it must be collapsed too or a hole remains.
fn slot_of(icon: xamlom::InstanceHandle) -> xamlom::InstanceHandle {
    tree::parent_of(icon)
        .filter(|&parent| tree::type_of(parent).as_deref() == Some(crate::winrt::CONTENT_PRESENTER))
        .unwrap_or(icon)
}

/// When a found indicator gets hidden, for its "found" log line.
fn gate_note(indicator: &Indicator) -> &'static str {
    if indicator.needs_strip {
        " (hidden only once our strip is placed)"
    } else {
        ""
    }
}

/// The indicator a content element identifies by its recorded class, with its `IconView`, if the
/// view sits in that indicator's section. Reads only the recorded tree: no XAML call.
fn content_indicator(
    content: xamlom::InstanceHandle,
) -> Option<(&'static Indicator, xamlom::InstanceHandle, String)> {
    let class = tree::type_of(content)?;
    let indicator = indicators().into_iter().find(|i| i.classes.contains(&class.as_str()))?;
    // IconView > ContainerGrid > ContentPresenter > ContentGrid > content.
    let icon = tree::ancestor_of_type(content, ICON, 6)?;
    let section = section_of(icon)?;
    (section == indicator.section).then_some((indicator, icon, section))
}

/// Note an indicator found by its content class (the keyboard-layout indicator) if this element is
/// one. Like [`note_system_indicator`], it records, and [`enforce_hidden`] collapses.
///
/// # Safety
/// XAML UI thread only, and only once the event stream is quiet — see [`crate::sweep`].
pub(crate) unsafe fn note_content_indicator(
    diagnostics: &xamlom::IXamlDiagnostics,
    content: xamlom::InstanceHandle,
) {
    let Some(state) = crate::strip_state() else {
        return;
    };
    let Some((indicator, icon, section)) = content_indicator(content) else {
        logf!(
            "content 0x{content:x} ({}) is not in its indicator's tray section — not hiding it",
            tree::type_of(content).unwrap_or_default()
        );
        return;
    };
    if !(indicator.wanted)(&state) {
        return;
    }
    let slot = slot_of(icon);
    if !indicator.announced.swap(true, Ordering::SeqCst) {
        logf!(
            "system {} indicator found: {} in {section} IconView 0x{icon:x}, slot 0x{slot:x}{}",
            indicator.what,
            tree::type_of(content).unwrap_or_default(),
            gate_note(indicator)
        );
    }
    indicator.record(Found { marker: content, icon, slot });
    enforce_hidden(diagnostics);
}

/// Put a found indicator's elements back and forget it: it no longer is what we hide.
///
/// # Safety
/// XAML UI thread only, and only once the event stream is quiet — see [`crate::sweep`].
unsafe fn put_back_indicator(
    diagnostics: &xamlom::IXamlDiagnostics,
    indicator: &Indicator,
    found: Found,
    now: &str,
) {
    let icon = restore::put_back_layout(diagnostics, found.icon);
    let slot = if found.slot != found.icon {
        restore::put_back_layout(diagnostics, found.slot)
    } else {
        None
    };
    let show = |outcome: Option<decorate::Restored>| match outcome {
        Some(restored) => restored.to_string(),
        None => "never hidden".to_owned(),
    };
    logf!(
        "system {} indicator 0x{:x} now {now} — no longer ours to hide: icon {}, slot {}",
        indicator.what,
        found.icon,
        show(icon),
        show(slot)
    );
    indicator.forget();
}

/// The `x:Name` of the `SystemTrayFrameGrid` child an element sits in (`MainStack`,
/// `ControlCenterButton`, …), from the recorded tree.
fn section_of(start: xamlom::InstanceHandle) -> Option<String> {
    let mut handle = start;
    for _ in 0..24 {
        let parent = tree::parent_of(handle)?;
        if tree::name_of(parent).as_deref() == Some(reorder::FRAME_GRID) {
            return tree::name_of(handle);
        }
        handle = parent;
    }
    None
}

/// Tray text blocks in an indicator's section that did not match when last read. Re-read every
/// sweep by [`recheck_indicators`]: a text change raises no tree event.
static WATCHED: Mutex<Vec<xamlom::InstanceHandle>> = Mutex::new(Vec::new());

/// Re-reads each found indicator's glyph. The shell reuses an `IconView` for another indicator by
/// changing its text (microphone → microphone + location → location), which raises no tree event,
/// so this runs every sweep. A view that no longer matches is put back, forgotten and watched;
/// watched views are offered to [`note_system_indicator`] again, so one that turns (back) into an
/// indicator is hidden. Empty text (the icon is going away) decides nothing.
///
/// # Safety
/// XAML UI thread only, and only once the event stream is quiet — see [`crate::sweep`].
pub(crate) unsafe fn recheck_indicators(diagnostics: &xamlom::IXamlDiagnostics) {
    for indicator in indicators() {
        let Some(found) = indicator.recorded() else {
            continue;
        };
        if !indicator.classes.is_empty() {
            recheck_by_class(diagnostics, indicator, found);
            continue;
        }
        let Some(text) = xaml::text_of(diagnostics, found.marker) else {
            // Gone from the tree: nothing to put back.
            indicator.forget();
            continue;
        };
        if text.is_empty() || indicator.matches(indicator.section, &text) {
            continue;
        }
        put_back_indicator(diagnostics, indicator, found, &format!("shows {}", describe_text(&text)));
        let mut watched = lock(&WATCHED);
        if !watched.contains(&found.marker) {
            watched.push(found.marker);
        }
    }

    let watched = lock(&WATCHED).clone();
    for text_block in watched {
        note_system_indicator(diagnostics, text_block);
    }
}

/// [`recheck_indicators`] for an indicator found by content class, from the recorded tree alone
/// (no XAML read): the `IconView` must still hold a content of one of its classes. A new content
/// of the right class (the shell rebuilt it) is adopted; any other content puts the view back.
/// The view coming back as the indicator later raises an add event, which finds it again.
///
/// # Safety
/// XAML UI thread only, and only once the event stream is quiet — see [`crate::sweep`].
unsafe fn recheck_by_class(diagnostics: &xamlom::IXamlDiagnostics, indicator: &Indicator, found: Found) {
    if tree::type_of(found.icon).is_none() {
        // The view itself is gone: nothing to put back.
        indicator.forget();
        return;
    }
    let still = |content: xamlom::InstanceHandle| {
        content_indicator(content).is_some_and(|(i, icon, _)| core::ptr::eq(i, indicator) && icon == found.icon)
    };
    if still(found.marker) {
        return;
    }
    let replacement = indicator
        .classes
        .iter()
        .flat_map(|class| tree::find_by_type(class))
        .filter(|&content| still(content))
        .max_by_key(|&content| tree::seq_of(content));
    match replacement {
        Some(content) => indicator.record(Found { marker: content, ..found }),
        None => put_back_indicator(diagnostics, indicator, found, "holds other content"),
    }
}

/// A tray text for the log: codepoints, since most are private-use glyphs.
fn describe_text(text: &str) -> String {
    let codes: Vec<String> = text.chars().take(8).map(|c| format!("{:04X}", c as u32)).collect();
    format!("[{}]", codes.join(" "))
}

/// Logs a tray text we do not hide, once per section and text: the diagnostic for a Windows build
/// whose "microphone in use" glyph is missing from [`decorate::MIC_GLYPHS`].
fn note_unknown_glyph(section: &str, text: &str) {
    /// Cap on distinct texts logged.
    const MAX_SEEN: usize = 48;

    let key = format!("{section} {}", describe_text(text));
    let mut seen = lock(&UNKNOWN_GLYPHS);
    if seen.contains(&key) || seen.len() >= MAX_SEEN {
        return;
    }
    logf!("tray glyph {key} is not one we hide");
    seen.push(key);
}

/// Section-and-text keys [`note_unknown_glyph`] has already reported.
static UNKNOWN_GLYPHS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Re-applies each indicator's collapse (bounded by [`HIDE_MAX_RETRIES`]) once layout has measured it.
///
/// # Safety
/// XAML UI thread only, and only once the event stream is quiet — see [`crate::sweep`].
pub(crate) unsafe fn enforce_hidden(diagnostics: &xamlom::IXamlDiagnostics) {
    // Every caller runs behind the sweep's `live()` check; the strip gate is per indicator.
    let placed = crate::strip_placed();
    for indicator in indicators() {
        if indicator.needs_strip && !placed {
            continue;
        }
        if indicator.retries.load(Ordering::SeqCst) >= HIDE_MAX_RETRIES {
            continue;
        }
        let Some(Found { icon, slot, .. }) = indicator.recorded() else {
            continue;
        };
        // Zero means layout has not run yet, and a collapse now would do nothing.
        if xaml::actual_width(diagnostics, slot).is_none_or(|width| width <= 0.0) {
            continue;
        }
        // Before the collapse, never after (we would record our own zero width).
        restore::remember_layout(diagnostics, icon);
        decorate::collapse(diagnostics, icon);
        if slot != icon {
            restore::remember_layout(diagnostics, slot);
            decorate::collapse(diagnostics, slot);
        }
        if indicator.retries.fetch_add(1, Ordering::SeqCst) + 1 == HIDE_MAX_RETRIES {
            logf!(
                "system {} collapse re-applied {HIDE_MAX_RETRIES}x — stopping",
                indicator.what
            );
        }
    }
}

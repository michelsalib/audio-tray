//! M3: putting our own visuals into a tray icon.
//!
//! Two routes were measured and rejected first (see `FINDINGS.md`):
//! `IVisualTreeService::CreateInstance` is `E_NOTIMPL`, and WinRT
//! `Panel.Children.Append` returns `0x800F1000` even though `get_Children` and
//! `get_Size` on the same object succeed. What works — and what Windhawk's
//! Taskbar Styler does — is to leave the tree shape alone and *set a property*:
//!
//!   `XamlReader.Load(markup)` → a live element  →  `ContentPresenter.Content = it`
//!
//! Everything here must run on the XAML UI thread, i.e. from inside
//! `OnVisualTreeChange`. `SetSite` runs on a different thread and WinRT calls
//! from there fail with `RPC_E_WRONG_THREAD`.

use crate::log::logf;
use crate::winrt::{
    IAutomationPropertiesStatics, IContentPresenter, IDependencyObject, IFrameworkElement,
    ITextBlock, IUIElement, IVisualTreeHelperStatics, IXamlReaderStatics, AUTOMATION_PROPERTIES,
    VISIBILITY_COLLAPSED, VISUAL_TREE_HELPER, XAML_READER,
};
use crate::xamlom::{IXamlDiagnostics, InstanceHandle};
use core::ffi::c_void;
use windows::Win32::Foundation::S_OK;
use windows::Win32::System::WinRT::RoGetActivationFactory;
use windows_core::{IInspectable, Interface, HSTRING};

/// Segoe Fluent glyphs. The mute variants match what the flyout already uses, so
/// the taskbar and the flyout never disagree about how "muted" looks.
const GLYPH_MUTE: char = '\u{E74F}';
const GLYPH_MIC_OFF: char = '\u{EC54}';

/// Warm tint on a muted segment — a *second* signal; the glyph swap is the first.
const MUTED_TINT: &str = "#E8836A";

// Pill geometry, agreed from the mockups ("V2 — shell-matched"): it deliberately
// mirrors the Control Center button's metrics so the strip reads as a peer of
// that control rather than as an oversized tray icon.
//
// The chevron segment was dropped: right-click opens the panel, so a permanent
// glyph advertising it was paying 23 epx of scarce notification-area width for
// an affordance the right mouse button already provides. Total width is 64 epx.
//
// Each segment is *half the pill* (32 epx) with no padding on the Border. It was
// once 8 padding + 24 + 24 + 8, which is the same 64 overall but meant a segment
// could not be filled by its own hover plate — the plate was stuck at 24 wide
// against a 26 tall, i.e. permanently taller than wide, which is what made it
// read as a slab dropped on the pill. Owning the full half is what lets the
// hover fill it; the glyphs stay centred, now in 32 rather than 24.
//
// All values are effective pixels; XAML applies the per-monitor scale itself, so
// nothing here needs DPI maths.
//
// `PILL_H` is the fix for the original defect: without an explicit height the
// Border shrink-wrapped the FontIcon layout boxes, and Segoe Fluent glyph ink
// overshoots those boxes — the microphone's stand was being clipped.
const PILL_H: u32 = 32;
const PILL_RADIUS: u32 = 6;

/// Gap between the pill and the edge of the notification-icon slot it sits in,
/// the same on all four sides.
///
/// This is the only lever over how Explorer's *own* hover plate surrounds us: that
/// plate fills the slot, so the surround is whatever is left after the pill.
///
/// **4, and measured rather than reasoned about.** Two wrong answers came before it,
/// both from working off the wrong rectangle:
///
/// ```text
/// slot         80 x 48   ->  8 at the ends, 8 top and bottom
/// hover plate  80 x 40   ->  8 at the ends, 4 top and bottom   <- what is drawn
/// ```
///
/// The *slot* is 48 epx tall, but the plate carries its own 4 epx vertical inset,
/// so the gap the eye sees is 4 — not the 8 the slot implies. The plate's width
/// tracks the slot exactly, so horizontally this margin *is* the whole gap. Setting
/// it to the plate's vertical inset is what makes all four sides equal.
///
/// (`2` left it 2 against 4; `8` overshot to 8 against 4. Both were picked before
/// `Border#BackgroundBorder` had ever been measured.)
///
/// Costs 8 epx of notification-area width. The alternative, growing `PILL_H` to 40
/// to meet the plate instead, is free in width but makes the pill taller than the
/// shell's own icons.
const PILL_MARGIN: u32 = 4;
const SEGMENT_W: u32 = 32;
const GLYPH_PX: u32 = 16;

/// The `x:` namespace. `XamlReader.Load` parses the markup standalone, so a root
/// that uses `x:Name` has to declare this itself — omitting it fails the whole
/// parse with `0x802B000A` and the strip silently never appears.
const XAML_NS_X: &str = "http://schemas.microsoft.com/winfx/2006/xaml";

/// `x:Name`s of the two interactive segments. They are found again by name in the
/// recorded visual tree — our injected elements are announced back to us through
/// `OnVisualTreeChange` like any other, which is how the handlers get attached.
pub const SEGMENT_OUT: &str = "AudioTrayOutput";
pub const SEGMENT_IN: &str = "AudioTrayInput";

/// The lit half is flush on all four sides: it fills its half of the pill
/// exactly, sharing the pill's radius on the outer corners and meeting the other
/// half square in the middle.
///
/// Two earlier attempts are worth not repeating. An inset on all four sides put
/// the plate's rounded corners about 3px inside the pill's own, and at taskbar
/// scale two nested curves that close together read as a smeared double edge. A
/// 2px gap down the middle then read as a crack splitting the pill. Flush is
/// what makes it one control with two halves.
const HOVER_INNER_RADIUS: u32 = 0;

/// Opacity of the white plate used when there is no pill to tint.
const HOVER_OPACITY_PLAIN: f64 = 0.10;
/// Opacity of the accent plate used on the pill.
const HOVER_OPACITY_ACCENT: f64 = 0.30;

/// The hover plate's brush and the opacity it is lit to.
///
/// On the pill the plate is **the accent itself**, not white. White over a
/// saturated fill bleaches it: measured on accent `#D88DE1`, the pill sits at
/// `127,102,147` and a white wash at 0.16 composites to `148,127,164` — lighter
/// but noticeably greyer, which is why the first version read as a grey sticker
/// stuck on the pill rather than as the pill lighting up. Re-tinting with the
/// same hue gives `154,114,170`: brighter *and* more saturated, and it stays
/// on-palette for whatever accent the user has chosen.
///
/// With no pill (bare glyphs on the taskbar) there is no hue to intensify, so it
/// falls back to the shell's own treatment — a white wash at low alpha.
fn hover_plate(accent: Option<[u8; 3]>) -> (String, f64) {
    match accent {
        Some([r, g, b]) => (format!("#FF{r:02X}{g:02X}{b:02X}"), HOVER_OPACITY_ACCENT),
        None => ("#FFFFFFFF".to_string(), HOVER_OPACITY_PLAIN),
    }
}

/// Opacity the hover handlers light the plate to, for the strip as configured.
pub fn hover_opacity(accent: Option<[u8; 3]>) -> f64 {
    hover_plate(accent).1
}

/// Accent alpha ("A4"). A fully opaque accent block is brighter than anything
/// Windows puts in a taskbar; at half alpha the pill sits at the same visual
/// weight as the Control Center button beside it. Applied as alpha rather than a
/// pre-blended colour so the taskbar's real backdrop shows through.
const PILL_ALPHA: u8 = 0x80;

/// What the strip should currently show.
#[derive(Clone, Copy)]
pub struct StripState {
    pub output_glyph: char,
    pub input_glyph: char,
    pub output_muted: bool,
    pub input_muted: bool,
    /// Accent fill for the pill. `None` draws bare glyphs on the taskbar.
    pub accent: Option<[u8; 3]>,
    /// Alpha applied to `accent`. See [`PILL_ALPHA`].
    pub accent_alpha: u8,
    /// Collapse Explorer's own volume glyph, which our strip duplicates.
    pub hide_system_volume: bool,
}

impl Default for StripState {
    fn default() -> Self {
        Self {
            output_glyph: '\u{E767}', // Volume
            input_glyph: '\u{E720}',  // Microphone
            output_muted: false,
            input_muted: false,
            accent: None,
            accent_alpha: PILL_ALPHA,
            hide_system_volume: false,
        }
    }
}

impl StripState {
    /// Parse `out=E767;in=E720;outmuted=1` — the payload the injector passes as
    /// `InitializeXamlDiagnosticsEx` initialization data. Unknown keys are
    /// ignored so the format can grow without breaking an older TAP.
    pub fn parse(data: &str) -> Self {
        let mut state = Self::default();
        for pair in data.split(';') {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            let glyph = || {
                u32::from_str_radix(value.trim(), 16)
                    .ok()
                    .and_then(char::from_u32)
            };
            let flag = || matches!(value.trim(), "1" | "true");
            match key.trim() {
                "out" => state.output_glyph = glyph().unwrap_or(state.output_glyph),
                "in" => state.input_glyph = glyph().unwrap_or(state.input_glyph),
                "outmuted" => state.output_muted = flag(),
                "inmuted" => state.input_muted = flag(),
                "accent" => state.accent = parse_rgb(value.trim()),
                "alpha" => {
                    state.accent_alpha =
                        u8::from_str_radix(value.trim(), 16).unwrap_or(state.accent_alpha)
                }
                "hidevolume" => state.hide_system_volume = flag(),
                _ => {}
            }
        }
        state
    }
}

/// The strip markup: two equal segments, output glyph then input glyph.
///
/// The root needs an explicit `xmlns`: `XamlReader.Load` parses this standalone,
/// with no surrounding document to inherit from. Unmuted glyphs set no
/// `Foreground`, so they inherit the taskbar's own brush and follow light/dark
/// theming for free; only a muted segment overrides it.
fn strip_markup(state: StripState) -> String {
    let base = state.accent.map(|rgb| on_accent(rgb, state.accent_alpha));
    let (plate, _) = hover_plate(state.accent);
    // `leading` is the left-hand half. The hover plate is mirrored between the
    // two: each keeps the pill's radius on its own outer corners and is square
    // where it meets the other, so whichever half lights up the pill still reads
    // as one outline.
    let segment = |name: &str, glyph: char, size: u32, width: u32, muted: bool, leading: bool| {
        let colour = if muted { Some(MUTED_TINT) } else { base };
        let fg = colour.map_or(String::new(), |c| format!(r#" Foreground="{c}""#));
        // XAML order for CornerRadius: top-left, top-right, bottom-right,
        // bottom-left.
        let corners = if leading {
            format!("{PILL_RADIUS},{HOVER_INNER_RADIUS},{HOVER_INNER_RADIUS},{PILL_RADIUS}")
        } else {
            format!("{HOVER_INNER_RADIUS},{PILL_RADIUS},{PILL_RADIUS},{HOVER_INNER_RADIUS}")
        };
        // `Background="Transparent"` is load-bearing: a `null` background is not
        // hit-testable in XAML, so without it the pointer falls straight through
        // to the pill and neither hover nor click can tell the segments apart.
        //
        // The hover plate is pre-built at `Opacity="0"` rather than created on
        // demand, so hovering only has to set a double — no brush has to be
        // constructed inside Explorer.
        format!(
            r##"    <Grid x:Name="{name}" Width="{width}" Background="Transparent">
      <Border x:Name="{name}Hover" Background="{plate}" Opacity="0"
              CornerRadius="{corners}"/>
      <FontIcon FontFamily="Segoe Fluent Icons" Glyph="&#x{:04X};" FontSize="{size}"
                VerticalAlignment="Center" HorizontalAlignment="Center"{fg}/>
    </Grid>"##,
            glyph as u32
        )
    };

    let output = if state.output_muted {
        GLYPH_MUTE
    } else {
        state.output_glyph
    };
    let input = if state.input_muted {
        GLYPH_MIC_OFF
    } else {
        state.input_glyph
    };

    // No divider: it existed to separate the two "act" segments from the "drill
    // in" chevron, and with the chevron gone there is nothing left to separate.
    let strip = format!(
        // No `VerticalAlignment="Center"` here, and that is load-bearing rather
        // than an omission. Centred, the StackPanel sizes to its content, so the
        // segment `Grid`s were only as tall as a 16px glyph (~21 epx) inside a 32
        // epx pill — and a hover plate cannot be taller than the Grid it lives
        // in, so the lit half came out visibly short at the top and bottom no
        // matter what margin it was given. Stretching (the default) makes each
        // segment the full height of the pill, which is what lets the hover fill
        // it. The glyphs stay centred by their own alignment.
        r#"  <StackPanel Orientation="Horizontal">
{}
{}
  </StackPanel>"#,
        segment(SEGMENT_OUT, output, GLYPH_PX, SEGMENT_W, state.output_muted, true),
        segment(SEGMENT_IN, input, GLYPH_PX, SEGMENT_W, state.input_muted, false),
    );

    match state.accent {
        // `r##"…"##`: the markup contains `="#` (an ARGB literal right after an
        // attribute quote), which would terminate a plain `r#"…"#`.
        //
        // The explicit `Height` is load-bearing — see `PILL_H`.
        Some([r, g, b]) => {
            let a = state.accent_alpha;
            format!(
                r##"<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="{XAML_NS_X}"
        AutomationProperties.Name="{STRIP_NAME}"
        Background="#{a:02X}{r:02X}{g:02X}{b:02X}" CornerRadius="{PILL_RADIUS}"
        Height="{PILL_H}" Margin="{PILL_MARGIN}" VerticalAlignment="Center">
{strip}
</Border>"##
            )
        }
        None => format!(
            r#"<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
            xmlns:x="{XAML_NS_X}"
            AutomationProperties.Name="{STRIP_NAME}"
            Orientation="Horizontal" VerticalAlignment="Center" HorizontalAlignment="Center">
{strip}
</StackPanel>"#
        ),
    }
}

fn parse_rgb(value: &str) -> Option<[u8; 3]> {
    let hex = value.trim_start_matches('#');
    (hex.len() == 6)
        .then(|| u32::from_str_radix(hex, 16).ok())
        .flatten()
        .map(|v| [(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// Approximate taskbar ground, used to composite the semi-transparent accent
/// before judging contrast. The real backdrop is acrylic over the wallpaper, but
/// it is always dark in the dark theme and the decision is not close.
const TASKBAR_GROUND: [u8; 3] = [0x1F, 0x1F, 0x1F];

/// Threshold on relative luminance for flipping to a dark foreground.
///
/// Measured against Windows: on accent `#D88DE1` (luminance 0.39) Quick Settings
/// draws *dark* glyphs. An earlier 0.45 threshold therefore picked white where
/// Windows picks black — this is the corrected value.
const DARK_FG_ABOVE: f32 = 0.32;

/// Foreground that stays legible on the accent as it will actually appear —
/// i.e. after `alpha` compositing over the taskbar. The accent is the user's
/// choice, so this has to hold across the whole palette.
fn on_accent(rgb: [u8; 3], alpha: u8) -> &'static str {
    let a = alpha as f32 / 255.0;
    let composite: Vec<f32> = (0..3)
        .map(|i| a * rgb[i] as f32 + (1.0 - a) * TASKBAR_GROUND[i] as f32)
        .collect();

    let channel = |c: f32| {
        let c = c / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance =
        0.2126 * channel(composite[0]) + 0.7152 * channel(composite[1]) + 0.0722 * channel(composite[2]);
    if luminance > DARK_FG_ABOVE {
        "#FF1A1A1A"
    } else {
        "#FFFFFFFF"
    }
}

fn factory<I: Interface>(class: &str) -> Option<I> {
    match unsafe { RoGetActivationFactory(&HSTRING::from(class)) } {
        Ok(f) => Some(f),
        Err(err) => {
            logf!("RoGetActivationFactory({class}) failed: {err}");
            None
        }
    }
}

/// The tooltip text Explorer exposes for a notify icon, used to tell tray icons
/// apart. Index-based matching would break the moment the user reorders them.
///
/// `GetName` takes an **`IDependencyObject*`**, so the `IInspectable` from
/// `GetIInspectableFromHandle` has to be QI'd first. Passing it straight through
/// calls the wrong vtable and yields an empty string for every icon — which is
/// what made tooltip matching silently never match, and why the strip never
/// appeared once the app started passing a real tooltip.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn automation_name(
    diagnostics: &IXamlDiagnostics,
    handle: InstanceHandle,
) -> Option<String> {
    let statics: IAutomationPropertiesStatics = factory(AUTOMATION_PROPERTIES)?;
    let element = object_from_handle(diagnostics, handle)?
        .cast::<IDependencyObject>()
        .ok()?;
    let mut raw: *mut c_void = core::ptr::null_mut();
    if statics.GetName(element.as_raw(), &mut raw) != S_OK || raw.is_null() {
        return None;
    }
    let name = core::mem::transmute::<*mut c_void, HSTRING>(raw);
    Some(name.to_string())
}

/// Automation names of a subtree, for finding what actually identifies an icon.
///
/// Explorer does not necessarily put the tooltip on the `NotifyIconView` itself,
/// so this reports every non-empty name beneath it.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn probe_names(
    diagnostics: &IXamlDiagnostics,
    root: InstanceHandle,
    depth: u32,
) -> Vec<(InstanceHandle, String, String)> {
    let mut found = Vec::new();
    let mut frontier = vec![root];
    for _ in 0..depth {
        let mut next = Vec::new();
        for handle in frontier {
            if let Some(name) = automation_name(diagnostics, handle) {
                if !name.is_empty() {
                    let ty = crate::tree::type_of(handle).unwrap_or_default();
                    found.push((handle, ty, name));
                }
            }
            next.extend(crate::tree::children_of(handle));
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    found
}

/// Replace a tray icon's `ContentPresenter.Content` with our glyph + chevron.
///
/// Returns whether the mutation landed.
///
/// # Safety
/// XAML UI thread only; `presenter` must be a live `ContentPresenter` handle.
pub unsafe fn set_chevron_content(
    diagnostics: &IXamlDiagnostics,
    presenter: InstanceHandle,
    state: StripState,
) -> bool {
    let Some(reader) = factory::<IXamlReaderStatics>(XAML_READER) else {
        return false;
    };

    let markup = HSTRING::from(strip_markup(state));
    let mut created: *mut c_void = core::ptr::null_mut();
    // `HSTRING` is repr(transparent) over the handle; `as_ptr` would hand over the
    // UTF-16 buffer instead, which the callee would misread as a handle.
    let markup_handle = core::mem::transmute_copy::<HSTRING, *mut c_void>(&markup);
    let hr = reader.Load(markup_handle, &mut created);
    if hr != S_OK || created.is_null() {
        logf!("XamlReader.Load failed: 0x{:08x}", hr.0);
        return false;
    }
    let content = core::mem::transmute::<*mut c_void, IInspectable>(created);
    logf!(
        "XamlReader.Load ok -> {}",
        content.GetRuntimeClassName().map(|n| n.to_string()).unwrap_or_default()
    );

    let Some(target) = object_from_handle(diagnostics, presenter) else {
        logf!("could not resolve ContentPresenter 0x{presenter:x}");
        return false;
    };
    let Ok(presenter_iface) = target.cast::<IContentPresenter>() else {
        logf!("handle 0x{presenter:x} is not a ContentPresenter");
        return false;
    };

    // `put_Content` builds our subtree synchronously, which re-enters
    // `OnVisualTreeChange` on this very thread. The thread id is here because
    // this call never returning is the signature of doing it from an island that
    // does not own the element — compare it against "tray island is thread N".
    logf!("setting content on 0x{presenter:x} from thread {}…", crate::tid());
    let hr = presenter_iface.put_Content(content.as_raw());
    if hr == S_OK {
        logf!("MUTATION SUCCEEDED — chevron content set on 0x{presenter:x}");
        true
    } else {
        logf!("put_Content failed: 0x{:08x}", hr.0);
        false
    }
}

/// The `ContentPresenter` inside a tray icon, found by asking XAML rather than
/// by consulting our recorded tree.
///
/// The recorded tree cannot answer this reliably. When audio-tray restarts inside
/// one Explorer session, the new `SystemTray.NotifyIconView` is announced but
/// **nothing under it ever is** — XAML reuses the previous icon's child elements
/// and re-parents them without telling us. Measured: the icon sits there with
/// zero recorded children indefinitely, so the icon+presenter pair can never be
/// formed and the strip never draws until Explorer restarts.
///
/// Walking the live tree is immune to that, and to the announce-children-before-
/// parents ordering that the recorded-tree scan has to work around.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn descendant_presenter(
    diagnostics: &IXamlDiagnostics,
    icon: InstanceHandle,
) -> Option<InstanceHandle> {
    descendant_of_class(diagnostics, icon, "Windows.UI.Xaml.Controls.ContentPresenter")
}

/// Nearest descendant of `root` whose runtime class is `target`, breadth-first.
///
/// Breadth-first matters where a class appears at more than one depth. Searching
/// for `…Controls.Border` under a tray icon finds the shell's own
/// `BackgroundBorder` (a child of `ContainerGrid`) before our pill, which is a
/// level deeper under the `ContentPresenter` — and it is the shell's one that
/// draws the icon-slot hover.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn descendant_of_class(
    diagnostics: &IXamlDiagnostics,
    icon: InstanceHandle,
    target: &str,
) -> Option<InstanceHandle> {
    /// The presenter sits two levels below the icon; the cap is slack rather
    /// than a guess, and it bounds the work done on the shell's UI thread.
    const MAX_DEPTH: usize = 6;
    /// Hard stop, so a tree we did not expect cannot spin inside Explorer.
    const MAX_VISITED: usize = 256;

    let statics: IVisualTreeHelperStatics = factory(VISUAL_TREE_HELPER)?;
    // `VisualTreeHelper` deals in `DependencyObject`; handing it the plain
    // `IInspectable` would call through the wrong vtable.
    let root = object_from_handle(diagnostics, icon)?
        .cast::<IDependencyObject>()
        .ok()?;

    let mut frontier = vec![root];
    let mut visited = 0usize;
    for _ in 0..MAX_DEPTH {
        let mut next: Vec<IDependencyObject> = Vec::new();
        for parent in &frontier {
            let mut count = 0i32;
            if statics.GetChildrenCount(parent.as_raw(), &mut count) != S_OK {
                continue;
            }
            for index in 0..count {
                if visited >= MAX_VISITED {
                    logf!("descendant_presenter: gave up after {MAX_VISITED} elements");
                    return None;
                }
                visited += 1;
                let mut raw: *mut c_void = core::ptr::null_mut();
                if statics.GetChild(parent.as_raw(), index, &mut raw) != S_OK || raw.is_null() {
                    continue;
                }
                // `GetChild` hands back a reference; taking it as an owned
                // `IInspectable` is what releases it again.
                let child = core::mem::transmute::<*mut c_void, IInspectable>(raw);
                if child.GetRuntimeClassName().is_ok_and(|name| name == target) {
                    let mut handle: InstanceHandle = 0;
                    if diagnostics.GetHandleFromIInspectable(child.as_raw(), &mut handle) == S_OK
                        && handle != 0
                    {
                        return Some(handle);
                    }
                }
                if let Ok(node) = child.cast::<IDependencyObject>() {
                    next.push(node);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    None
}

/// Whether `presenter` currently holds *our* strip.
///
/// The shell data-binds this `Content`, so a `put_Content` that lands while the
/// icon is still being set up gets overwritten when the binding evaluates — the
/// mutation reports success and then silently loses. Reading the content back is
/// how we notice and re-apply. Our root is a `Border`; the shell's is an
/// `ImageIconContent`/`Grid`, so the runtime class is enough to tell them apart.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn holds_our_strip(
    diagnostics: &IXamlDiagnostics,
    presenter: InstanceHandle,
) -> bool {
    let Some(object) = object_from_handle(diagnostics, presenter) else {
        return false;
    };
    let Ok(iface) = object.cast::<IContentPresenter>() else {
        return false;
    };
    let mut raw: *mut c_void = core::ptr::null_mut();
    if iface.get_Content(&mut raw) != S_OK || raw.is_null() {
        return false;
    }
    let content = core::mem::transmute::<*mut c_void, IInspectable>(raw);
    // Identity, not shape. This used to accept any `Border`, but the shell can
    // legitimately put its own there — and mistaking one for ours means concluding
    // the strip is up when it is not, which leaves the volume icon hidden and the
    // tray reordered with nothing drawn in their place. Matching the automation
    // name also works for the no-pill mode, whose root is a `StackPanel`.
    let mut handle: InstanceHandle = 0;
    if diagnostics.GetHandleFromIInspectable(content.as_raw(), &mut handle) != S_OK || handle == 0 {
        return false;
    }
    automation_name(diagnostics, handle).as_deref() == Some(STRIP_NAME)
}

/// Automation name on the strip's root, so [`holds_our_strip`] can recognise our
/// own content rather than merely something Border-shaped.
///
/// Not localised and never shown: `AutomationProperties.Name` on a decorative root
/// is the cheapest identity XAML will carry for us, and it is readable back through
/// the statics we already use to identify the tray icon itself.
pub const STRIP_NAME: &str = "AudioTrayStrip";

/// Segoe Fluent codepoints Explorer uses for the taskbar volume indicator:
/// muted, the generic speaker, and the four level glyphs. Matching on the glyph
/// keeps this locale-independent — the element's automation name is translated.
pub const VOLUME_GLYPHS: &[char] = &[
    '\u{E74F}', '\u{E767}', '\u{E992}', '\u{E993}', '\u{E994}', '\u{E995}',
];

/// Reads a `TextBlock`'s `Text`.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn text_of(diagnostics: &IXamlDiagnostics, handle: InstanceHandle) -> Option<String> {
    let object = object_from_handle(diagnostics, handle)?;
    let text_block = object.cast::<ITextBlock>().ok()?;
    let mut raw: *mut c_void = core::ptr::null_mut();
    if text_block.get_Text(&mut raw) != S_OK || raw.is_null() {
        return None;
    }
    let text = core::mem::transmute::<*mut c_void, HSTRING>(raw);
    Some(text.to_string())
}

/// Collapse an element — used on Explorer's own volume glyph, which our strip
/// duplicates.
///
/// This edits the shell's own UI rather than our tray icon, so callers must have
/// recorded the previous state through [`layout_of`] first; [`restore_layout`]
/// is what puts it back when the feature is turned off.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn collapse(diagnostics: &IXamlDiagnostics, handle: InstanceHandle) -> bool {
    let Some(object) = object_from_handle(diagnostics, handle) else {
        return false;
    };
    let Ok(element) = object.cast::<IUIElement>() else {
        logf!("handle 0x{handle:x} is not a UIElement — not collapsing");
        return false;
    };
    let hr = element.put_Visibility(VISIBILITY_COLLAPSED);
    if hr != S_OK {
        logf!("put_Visibility on 0x{handle:x} failed: 0x{:08x}", hr.0);
        return false;
    }

    // `Visibility` alone is not enough here. Inside the Quick Settings button the
    // icons sit in generated containers whose slot survives a collapsed child —
    // measured: the glyph vanishes but wifi and battery do not move by a single
    // pixel. Zeroing the width is what actually closes the gap.
    //
    // Quiet on success: this is re-applied many times to cover a layout race, so
    // only failures are worth a line.
    if let Ok(framework) = object.cast::<IFrameworkElement>() {
        let hr = framework.put_Width(0.0);
        let min = framework.put_MinWidth(0.0);
        if hr != S_OK || min != S_OK {
            logf!(
                "zeroing width of 0x{handle:x}: put_Width -> 0x{:08x}, put_MinWidth -> 0x{:08x}",
                hr.0,
                min.0
            );
        }
    }
    true
}

/// The layout properties [`collapse`] overwrites, as they were beforehand.
///
/// `Width` and `MinWidth` read back as `NaN` when they were never set — that is
/// XAML's "Auto", and it is the value that has to go back. Restoring a literal
/// `0.0` instead would leave the element permanently zero-width, which on screen
/// is indistinguishable from still being hidden.
#[derive(Clone, Copy)]
pub struct Layout {
    visibility: i32,
    width: f64,
    min_width: f64,
}

/// Reads the properties [`collapse`] is about to overwrite.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn layout_of(diagnostics: &IXamlDiagnostics, handle: InstanceHandle) -> Option<Layout> {
    let object = object_from_handle(diagnostics, handle)?;
    let element = object.cast::<IUIElement>().ok()?;
    let mut visibility = 0i32;
    if element.get_Visibility(&mut visibility) != S_OK {
        return None;
    }
    // Not every UIElement is a FrameworkElement; those simply have no width to
    // put back, and `NaN` is the value `put_Width` treats as "unset" anyway.
    let (mut width, mut min_width) = (f64::NAN, f64::NAN);
    if let Ok(framework) = object.cast::<IFrameworkElement>() {
        if framework.get_Width(&mut width) != S_OK {
            width = f64::NAN;
        }
        if framework.get_MinWidth(&mut min_width) != S_OK {
            min_width = f64::NAN;
        }
    }
    Some(Layout {
        visibility,
        width,
        min_width,
    })
}

/// How putting something back turned out.
///
/// `Gone` exists because it is the *expected* outcome half the time and must not
/// read as a failure. Killing audio-tray destroys its notify icon, so by the time
/// the revert runs the presenter we decorated no longer exists — there is nothing
/// left to undo, and that is success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restored {
    /// Put back as it was.
    Done,
    /// The element no longer exists, so nothing needed doing.
    Gone,
    /// The element is still there and refused the write.
    Failed,
}

impl std::fmt::Display for Restored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Done => "restored",
            Self::Gone => "already gone",
            Self::Failed => "FAILED",
        })
    }
}

/// Puts back what [`layout_of`] read.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn restore_layout(
    diagnostics: &IXamlDiagnostics,
    handle: InstanceHandle,
    layout: Layout,
) -> Restored {
    let Some(object) = object_from_handle(diagnostics, handle) else {
        return Restored::Gone;
    };
    let Ok(element) = object.cast::<IUIElement>() else {
        return Restored::Failed;
    };
    let mut ok = element.put_Visibility(layout.visibility) == S_OK;
    if let Ok(framework) = object.cast::<IFrameworkElement>() {
        ok &= framework.put_Width(layout.width) == S_OK;
        ok &= framework.put_MinWidth(layout.min_width) == S_OK;
    }
    if ok {
        Restored::Done
    } else {
        Restored::Failed
    }
}

/// Takes the presenter's current `Content` as an owned raw reference.
///
/// A null content is `Some(null)`, not `None`: "there was nothing here" is a
/// state that has to be restorable, and is a different thing from "we could not
/// read it".
///
/// # Safety
/// XAML UI thread only. The caller owns the returned reference.
pub unsafe fn content_of(
    diagnostics: &IXamlDiagnostics,
    presenter: InstanceHandle,
) -> Option<*mut c_void> {
    let object = object_from_handle(diagnostics, presenter)?;
    let iface = object.cast::<IContentPresenter>().ok()?;
    let mut raw: *mut c_void = core::ptr::null_mut();
    (iface.get_Content(&mut raw) == S_OK).then_some(raw)
}

/// Sets `Content` from a raw pointer — the other half of [`content_of`].
///
/// Does not consume the reference; `put_Content` takes its own.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn set_content_raw(
    diagnostics: &IXamlDiagnostics,
    presenter: InstanceHandle,
    content: *mut c_void,
) -> Restored {
    let Some(object) = object_from_handle(diagnostics, presenter) else {
        return Restored::Gone;
    };
    let Ok(iface) = object.cast::<IContentPresenter>() else {
        return Restored::Failed;
    };
    let hr = iface.put_Content(content);
    if hr != S_OK {
        logf!("restoring content of 0x{presenter:x} failed: 0x{:08x}", hr.0);
        return Restored::Failed;
    }
    Restored::Done
}

/// Sets an element's opacity — how the hover plate is lit and dimmed.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn set_opacity(
    diagnostics: &IXamlDiagnostics,
    handle: InstanceHandle,
    opacity: f64,
) -> bool {
    let Some(object) = object_from_handle(diagnostics, handle) else {
        return false;
    };
    let Ok(element) = object.cast::<IUIElement>() else {
        return false;
    };
    let hr = element.put_Opacity(opacity);
    if hr != S_OK {
        logf!("put_Opacity({opacity}) on 0x{handle:x} -> 0x{:08x}", hr.0);
    }
    hr == S_OK
}

/// The element's laid-out width, or `None` if it cannot be read.
///
/// Zero means one of two very different things — "layout has removed it" or
/// "layout has not run yet" — so callers must only trust a zero *after* having
/// seen a non-zero.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn actual_width(diagnostics: &IXamlDiagnostics, handle: InstanceHandle) -> Option<f64> {
    let object = object_from_handle(diagnostics, handle)?;
    let framework = object.cast::<IFrameworkElement>().ok()?;
    let mut width = 0.0f64;
    (framework.get_ActualWidth(&mut width) == S_OK).then_some(width)
}

/// Laid-out size of an element, once layout has actually run.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn actual_size(
    diagnostics: &IXamlDiagnostics,
    handle: InstanceHandle,
) -> Option<(f64, f64)> {
    let object = object_from_handle(diagnostics, handle)?;
    let framework = object.cast::<IFrameworkElement>().ok()?;
    let mut width = 0.0f64;
    let mut height = 0.0f64;
    (framework.get_ActualWidth(&mut width) == S_OK && framework.get_ActualHeight(&mut height) == S_OK)
        .then_some((width, height))
}

/// Laid-out size of whatever is currently *inside* a presenter — for us, the pill.
///
/// Measuring the presenter itself is useless for judging the surround: it fills the
/// icon slot by definition, so slot-minus-presenter can only ever be zero. The
/// number that matters is slot-minus-pill, and the pill is the presenter's content,
/// which we never hold a handle to.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn content_size(
    diagnostics: &IXamlDiagnostics,
    presenter: InstanceHandle,
) -> Option<(f64, f64)> {
    let target = object_from_handle(diagnostics, presenter)?;
    let iface = target.cast::<IContentPresenter>().ok()?;
    let mut raw: *mut c_void = core::ptr::null_mut();
    if iface.get_Content(&mut raw) != S_OK || raw.is_null() {
        return None;
    }
    // Owned: `get_Content` hands back a reference and this drop releases it.
    let content = core::mem::transmute::<*mut c_void, IInspectable>(raw);
    let framework = content.cast::<IFrameworkElement>().ok()?;
    let mut width = 0.0f64;
    let mut height = 0.0f64;
    (framework.get_ActualWidth(&mut width) == S_OK
        && framework.get_ActualHeight(&mut height) == S_OK)
        .then_some((width, height))
}

unsafe fn object_from_handle(
    diagnostics: &IXamlDiagnostics,
    handle: InstanceHandle,
) -> Option<IInspectable> {
    let mut raw: *mut c_void = core::ptr::null_mut();
    if diagnostics.GetIInspectableFromHandle(handle, &mut raw) != S_OK || raw.is_null() {
        return None;
    }
    Some(core::mem::transmute::<*mut c_void, IInspectable>(raw))
}

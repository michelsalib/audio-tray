//! Design tokens for the flyout: the layout dimensions (in DIPs, scaled by the monitor
//! DPI at show time), the colour palette, the Segoe Fluent control glyphs, the UI fonts,
//! and the user's Windows accent colour. Everything visual and tunable lives here, so the
//! rest of the flyout reads as structure rather than magic numbers.
//!
//! The palette, the endpoint glyphs, the track height and the UI font are `pub(crate)`
//! rather than private to the flyout: [`crate::osd`] is a second surface in the same design
//! language (a volume readout beside the taskbar), and it has to *be* the same dark tint,
//! the same white, the same speaker/microphone glyphs. Only its geometry is its own.

use std::sync::OnceLock;

use ab_glyph::FontVec;
use windows::core::w;

use crate::canvas::{Canvas, Rect};

// Layout, in DIPs (scaled by the monitor DPI at show time). Tuned to the native Win11
// sound flyout: roomy rows, a semibold section header, an accent selection pill.
pub(super) const CORNER: f32 = 8.0;
pub(super) const PAD_V: f32 = 6.0; // top/bottom padding inside the panel
pub(super) const HEADER_FIRST_H: f32 = 30.0; // first section header (modest top gap)
pub(super) const HEADER_H: f32 = 36.0; // later section headers (larger top gap → group separation)
pub(super) const SLIDER_H: f32 = 48.0; // volume-slider row
pub(super) const ITEM_H: f32 = 44.0; // device / action row
pub(super) const ICON_X: f32 = 14.0; // left inset of a row's leading icon
pub(super) const ICON_PX: f32 = 20.0; // leading icon glyph size
pub(super) const TEXT_X: f32 = 48.0; // left inset of a row's label
pub(super) const HEADER_X: f32 = 15.0; // left inset of a section-header label
pub(super) const RIGHT_PAD: f32 = 16.0;
pub(super) const MIN_W: f32 = 340.0; // panel minimum width
pub(super) const MAX_W: f32 = 420.0; // cap on the panel width (driven by device-name length)
pub(super) const ROW_MARGIN: f32 = 4.0; // side margin of the row highlight/pill
pub(super) const ROW_RADIUS: f32 = 4.0; // corner radius of the row highlight
pub(super) const PILL_W: f32 = 3.0; // accent selection-indicator pill width
pub(super) const PILL_H: f32 = 16.0; // accent selection-indicator pill height
pub(super) const PENCIL_W: f32 = 42.0; // right-hand space reserved for the edit affordance (label stops here)
pub(super) const PENCIL_BTN: f32 = 30.0; // the pencil's round hover-button diameter
pub(super) const PENCIL_RIGHT: f32 = 9.0; // gap from the panel's right edge to the button
pub(super) const BATTERY_W: f32 = 96.0; // right-hand space reserved on battery rows (fits battery + hover pencil)
// slider geometry
pub(super) const TRACK_X0: f32 = 52.0; // track left edge
pub(super) const VALUE_W: f32 = 46.0; // reserved right area for the percentage
pub(crate) const TRACK_H: f32 = 4.0;
pub(super) const THUMB_R: f32 = 7.0;
// footer bar (the panel's last row, modelled on the Win11 quick-settings footer): a
// hairline across the full width, a labelled action on the left, a round icon button right
pub(super) const FOOTER_TOP_GAP: f32 = 6.0; // standoff above the hairline (the panel's old closing pad)
pub(super) const FOOTER_H: f32 = 52.0; // the strip itself: hairline down to the panel's bottom edge
pub(super) const FOOTER_ICON_X: f32 = 16.0; // left inset of the footer item's glyph
pub(super) const FOOTER_ICON_PX: f32 = 16.0; // footer glyph size (smaller than a row's)
pub(super) const FOOTER_TEXT_X: f32 = 44.0; // left inset of the footer item's label
pub(super) const FOOTER_TEXT_PX: f32 = 13.5; // footer label em size
pub(super) const FOOTER_ITEM_PAD: f32 = 10.0; // trailing pad inside the left item's hover pill
pub(super) const FOOTER_ITEM_H: f32 = 32.0; // height of the left item's hover pill
pub(super) const FOOTER_BTN: f32 = 32.0; // the right-hand button's round hover target
pub(super) const FOOTER_BTN_RIGHT: f32 = 9.0; // gap from the panel's right edge to the last button
pub(super) const FOOTER_BTN_GAP: f32 = 4.0; // gap between two icon buttons
pub(super) const FOOTER_CTA_A: f32 = 0.20; // accent disc under a call-to-action button (idle)
pub(super) const FOOTER_CTA_HOVER_A: f32 = 0.34; // …and under the pointer
pub(super) const FOOTER_GAP: f32 = 8.0; // minimum gap between the left item and the button
pub(super) const DIVIDER_A: f32 = 0.14; // footer hairline (below ~0.1 it vanishes into the acrylic)
pub(super) const FOOTER_SHADE: [u8; 3] = [0x00, 0x00, 0x00]; // the strip sits a shade darker than the body
pub(super) const FOOTER_SHADE_A: f32 = 0.18;
// icon-picker page (a dedicated screen you slide to from a device's edit pencil)
pub(super) const PICKER_HEADER_H: f32 = 46.0; // back-arrow + device-name title row
pub(super) const BACK_LEFT: f32 = 7.0; // left inset of the back button
pub(super) const BACK_BTN: f32 = 32.0; // back button's round hover target diameter
pub(super) const BACK_GLYPH_PX: f32 = 16.0; // back chevron glyph size
pub(super) const TITLE_PX: f32 = 15.0; // picker title (device name) em size
// wrapping icon grid
pub(super) const GRID_CHIP: f32 = 44.0; // one icon cell (square)
pub(super) const GRID_GAP: f32 = 8.0; // gap between cells (both axes)
pub(super) const GRID_X: f32 = 14.0; // grid side inset (used to size columns)
pub(super) const GRID_TOP_PAD: f32 = 4.0; // gap above the first grid row
pub(super) const GRID_BOTTOM_PAD: f32 = 10.0; // gap below the last grid row
pub(super) const GRID_ICON_RATIO: f32 = 0.55; // glyph size inside a cell

// Fluent glyphs painted directly (not from the built-in IconId set).
const GLYPH_VOLUME: char = '\u{E767}';
const GLYPH_MUTE: char = '\u{E74F}';
const GLYPH_MIC: char = '\u{E720}';
const GLYPH_MIC_OFF: char = '\u{EC54}';
pub(super) const GLYPH_EDIT: char = '\u{E70F}';
pub(super) const GLYPH_SETTINGS: char = '\u{E713}';
pub(super) const GLYPH_CANCEL: char = '\u{E711}';
pub(super) const GLYPH_BACK: char = '\u{E72B}'; // Back (leftward arrow) — the picker's cancel affordance
pub(super) const GLYPH_UPDATE: char = '\u{E72C}'; // Refresh (circular arrow) — restart-to-update button

// Colours (RGB); alpha applied at blend time.
pub(crate) const RECORDING: [u8; 3] = [0xE8, 0x11, 0x23]; // the "an app is recording" dot
pub(crate) const TINT: [u8; 3] = [0x2C, 0x2C, 0x2C]; // panel base (semi-transparent, acrylic shows through)
pub(crate) const TINT_A: f32 = 0.82;
pub(crate) const TEXT: [u8; 3] = [0xFF, 0xFF, 0xFF]; // primary text + glyphs
pub(super) const DARK_GLYPH: [u8; 3] = [0x12, 0x16, 0x1C]; // icon colour on a solid accent chip
pub(super) const HOVER_A: f32 = 0.06; // white overlay for hover
pub(super) const SEL_A: f32 = 0.09; // white overlay for the selected row

// The recording dot, as fractions of the mic glyph's box: the red disc's radius, the white
// ring around it, and the centre both sit on. The **top-right corner**, which is where the
// Segoe microphone's ink does not reach — the capsule runs up the middle, the stand sits
// under it, and the mute variant's "no" circle is bottom-right.
//
// The centre is far enough out that the *ring* clears the capsule too; it is the ring, not
// the disc, that decides how close the badge can sit.
const REC_R: f32 = 0.15;
const REC_BORDER: f32 = 0.05;
const REC_CX: f32 = 0.87;
const REC_CY: f32 = 0.14;

/// Stamp the recording dot on a mic glyph drawn at `(x, y)` in a `size`-pixel box.
///
/// Lives here, beside the glyphs themselves, because both hand-painted surfaces stamp it —
/// the flyout's input slider and the scroll readout — and "an app is recording" has to be
/// the same picture in both. The taskbar strip draws its own (XAML, inside Explorer; see
/// the TAP's `decorate`), which is why that one is not this.
pub(crate) fn recording_dot(cv: &mut Canvas, x: i32, y: i32, size: u32) {
    let box_px = size as f32;
    let r = box_px * REC_R;
    let cx = x as f32 + box_px * REC_CX;
    let cy = y as f32 + box_px * REC_CY;
    // The white ring first, as a plain disc, with the red one over it — a stroke would have
    // to be drawn by hand, and this composites identically. Never thinner than a pixel: at
    // 100% DPI the fraction rounds down to almost nothing, and a ring that faint is the one
    // case the border exists to prevent.
    let ring = (box_px * REC_BORDER).max(1.0);
    let outer = r + ring;
    let disc = |cv: &mut Canvas, r: f32, col: [u8; 3]| {
        cv.fill_round_rect(Rect::new(cx - r, cy - r, cx + r, cy + r), r, col, 1.0);
    };
    disc(cv, outer, TEXT);
    disc(cv, r, RECORDING);
}

/// The glyph for one endpoint's state — the picture of "which direction, and is it muted".
///
/// Shared by the flyout's slider rows and the scroll readout, because the two are seen a few
/// pixels apart and a different speaker in each would read as a mismatch. Only the *colour*
/// differs between them (see [`crate::osd`]'s muted tint), so that stays at the call site.
pub(crate) fn endpoint_glyph(flow: crate::audio::Flow, muted: bool) -> char {
    use crate::audio::Flow;

    match (flow, muted) {
        (Flow::Output, false) => GLYPH_VOLUME,
        (Flow::Output, true) => GLYPH_MUTE,
        (Flow::Input, false) => GLYPH_MIC,
        (Flow::Input, true) => GLYPH_MIC_OFF,
    }
}

pub(crate) fn ui_font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        let bytes = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf").ok()?;
        FontVec::try_from_vec(bytes).ok()
    })
    .as_ref()
}

/// Segoe UI Semibold — the weight Windows uses for the flyout's section captions
/// ("BodyStrong"). Falls back to the regular UI font at the call site if absent.
pub(super) fn ui_font_semibold() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        let bytes = std::fs::read(r"C:\Windows\Fonts\seguisb.ttf").ok()?;
        FontVec::try_from_vec(bytes).ok()
    })
    .as_ref()
}

/// The accent colour to paint (selection pill, slider fill/thumb). On our dark surface
/// Windows uses the *Light2* shade of the accent palette rather than the base accent —
/// matching that keeps us in step with the native flyout. Falls back to the DWM base
/// accent, then the Win11 default.
pub(crate) fn accent_rgb() -> [u8; 3] {
    accent_palette_light2().unwrap_or_else(dwm_accent_rgb)
}

/// The "Light2" accent shade from `Explorer\Accent\AccentPalette` — an 8-entry RGBA blob
/// ordered lightest→darkest `[Light3, Light2, Light1, Accent, Dark1, Dark2, Dark3, …]`.
fn accent_palette_light2() -> Option<[u8; 3]> {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_BINARY};
    let mut buf = [0u8; 32];
    let mut size = buf.len() as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent"),
            w!("AccentPalette"),
            RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    // Light2 is the second entry (bytes 4..7 = R,G,B).
    (ok.0 == 0 && size >= 8).then(|| [buf[4], buf[5], buf[6]])
}

/// The user's Windows accent colour from the DWM registry key (stored `AABBGGRR`).
fn dwm_accent_rgb() -> [u8; 3] {
    match crate::win::hkcu_dword(w!(r"Software\Microsoft\Windows\DWM"), w!("AccentColor")) {
        Some(v) => [(v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8],
        None => [0x60, 0xCD, 0xFF], // fallback Win11 accent
    }
}

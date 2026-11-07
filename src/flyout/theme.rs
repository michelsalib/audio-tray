//! Design tokens for the flyout: layout dimensions (DIPs), palette, Fluent glyphs, UI fonts
//! and the accent colour. The `pub(crate)` ones are shared with [`crate::osd`].

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
pub(super) const BATTERY_W: f32 = 80.0; // right-hand space reserved on battery rows (the readout + RIGHT_PAD)
// slider geometry
pub(super) const TRACK_X0: f32 = 52.0; // track left edge
pub(super) const VALUE_W: f32 = 46.0; // reserved right area for the percentage
pub(crate) const TRACK_H: f32 = 4.0;
pub(super) const THUMB_R: f32 = 7.0;
// footer bar: hairline, a labelled action on the left, round icon buttons on the right
pub(super) const FOOTER_TOP_GAP: f32 = 6.0; // standoff above the hairline (the panel's old closing pad)
pub(super) const FOOTER_H: f32 = 52.0; // the strip itself: hairline down to the panel's bottom edge
pub(super) const NOTE_H: f32 = 26.0; // the one-line integration notice above the footer
pub(super) const NOTE_TEXT_PX: f32 = 12.0;
pub(super) const NOTE_A: f32 = 0.62; // secondary text
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

// Fluent glyphs painted directly (not from the built-in IconId set).
const GLYPH_VOLUME: char = '\u{E767}';
const GLYPH_MUTE: char = '\u{E74F}';
pub(super) const GLYPH_MIC: char = '\u{E720}';
pub(super) const GLYPH_MIC_OFF: char = '\u{EC54}';
pub(super) const GLYPH_SETTINGS: char = '\u{E713}';
pub(super) const GLYPH_CANCEL: char = '\u{E711}';
pub(super) const GLYPH_UPDATE: char = '\u{E72C}'; // Refresh (circular arrow) — restart-to-update button

// Colours (RGB); alpha applied at blend time.
pub(crate) const RECORDING: [u8; 3] = [0xE8, 0x11, 0x23]; // the "an app is recording" dot
pub(crate) const TINT: [u8; 3] = [0x2C, 0x2C, 0x2C]; // panel base (semi-transparent, acrylic shows through)
pub(crate) const TINT_A: f32 = 0.82;
pub(crate) const TEXT: [u8; 3] = [0xFF, 0xFF, 0xFF]; // primary text + glyphs
pub(super) const DARK_GLYPH: [u8; 3] = [0x12, 0x16, 0x1C]; // glyph colour on a solid accent disc
pub(super) const HOVER_A: f32 = 0.06; // white overlay for hover
pub(super) const SEL_A: f32 = 0.09; // white overlay for the selected row

// The recording dot, as fractions of the mic glyph's box: disc radius, white ring, centre.
// Top-right is the one corner the mic glyph (and its mute variant) leaves free; the ring must clear it.
const REC_R: f32 = 0.15;
const REC_BORDER: f32 = 0.05;
const REC_CX: f32 = 0.87;
const REC_CY: f32 = 0.14;

/// Stamp the recording dot on a mic glyph drawn at `(x, y)` in a `size`-pixel box. Shared by
/// the flyout and the OSD; the taskbar strip draws its own in the TAP.
pub(crate) fn recording_dot(cv: &mut Canvas, x: i32, y: i32, size: u32) {
    let box_px = size as f32;
    let r = box_px * REC_R;
    let cx = x as f32 + box_px * REC_CX;
    let cy = y as f32 + box_px * REC_CY;
    // The ring is a white disc under the red one, at least 1px so it survives 100% DPI.
    let ring = (box_px * REC_BORDER).max(1.0);
    let outer = r + ring;
    let disc = |cv: &mut Canvas, r: f32, col: [u8; 3]| {
        cv.fill_round_rect(Rect::new(cx - r, cy - r, cx + r, cy + r), r, col, 1.0);
    };
    disc(cv, outer, TEXT);
    disc(cv, r, RECORDING);
}

/// The glyph for an endpoint's direction and mute state, shared by the flyout and the OSD
/// (colour stays at the call site).
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

/// The accent colour to paint: the palette's Light2 shade (what Windows uses on dark
/// surfaces), else the DWM accent, else the Win11 default.
pub(crate) fn accent_rgb() -> [u8; 3] {
    accent_shade(1).unwrap_or_else(dwm_accent_rgb)
}

/// Shade `index` of `Explorer\Accent\AccentPalette`, an 8-entry RGBA blob ordered
/// lightest→darkest `[Light3, Light2, Light1, Accent, Dark1, Dark2, Dark3, …]`: Light2 (1) is what
/// Windows paints on dark surfaces, Dark1 (4) on light ones.
pub(crate) fn accent_shade(index: usize) -> Option<[u8; 3]> {
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
    let at = index * 4;
    (ok.0 == 0 && index < 8 && size as usize >= at + 4).then(|| [buf[at], buf[at + 1], buf[at + 2]])
}

/// The user's Windows accent colour from the DWM registry key (stored `AABBGGRR`).
fn dwm_accent_rgb() -> [u8; 3] {
    match crate::win::hkcu_dword(w!(r"Software\Microsoft\Windows\DWM"), w!("AccentColor")) {
        Some(v) => [(v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8],
        None => [0x60, 0xCD, 0xFF], // fallback Win11 accent
    }
}

// The redesign's rows: Now Playing, the voice panel, the apps mixer and collapsible device lists.
pub(super) const NOW_PLAYING_H: f32 = 76.0;
pub(super) const ART_X: f32 = 12.0; // left inset of the cover
pub(super) const MEDIA_BTN: f32 = 32.0; // a transport button's round target
pub(super) const MEDIA_BTN_GAP: f32 = 2.0;
pub(super) const VOICE_H: f32 = 60.0;
pub(super) const FACE_DIP: f32 = 26.0; // a face in the voice panel
pub(super) const VOICE_BTN: f32 = 36.0; // the voice panel's mute button
pub(super) const APP_H: f32 = 40.0; // one app in the mixer
pub(super) const APP_ICON_DIP: f32 = 20.0; // an app's own icon, in its row's glyph column
pub(super) const APP_TRACK_X0: f32 = 170.0; // where an app's slider starts (its name sits before)
pub(super) const CHEVRON_PX: f32 = 12.0; // a section header's expand chevron
pub(super) const PAGER_H: f32 = 16.0; // the carousel's pip row, under its card
pub(super) const PIP_STEP: f32 = 16.0; // centre to centre, and each pip's click target
pub(super) const PIP_D: f32 = 6.0; // a pip's diameter
pub(super) const PIP_ON_W: f32 = 14.0; // the shown card's pip, drawn as a short accent bar
pub(super) const GLYPH_PREVIOUS: char = '\u{E892}';
pub(super) const GLYPH_PLAY: char = '\u{E768}';
pub(super) const GLYPH_PAUSE: char = '\u{E769}';
pub(super) const GLYPH_NEXT: char = '\u{E893}';
pub(super) const GLYPH_DOWN: char = '\u{E70D}';
pub(super) const GLYPH_UP: char = '\u{E70E}';
pub(super) const GLYPH_MUSIC: char = '\u{EC4F}';
pub(super) const GLYPH_VIDEO: char = '\u{E714}';
pub(super) const GLYPH_APP: char = '\u{E7F5}';
pub(super) const GLYPH_APP_MUTED: char = '\u{E74F}';
pub(super) const SPEAKING: [u8; 3] = [0x3D, 0xD6, 0x8C]; // the speaking ring, as on the pill

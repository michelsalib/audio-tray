//! Design tokens for the flyout: layout dimensions (DIPs), palette, Fluent glyphs and fonts. The
//! look every panel shares (the mode, the accent, the panel's ground) is [`crate::theme`]'s,
//! re-exported here for the flyout and Settings.

use std::sync::OnceLock;

use ab_glyph::FontVec;

use crate::icons::glyph;
pub(crate) use crate::theme::{
    accent_rgb, accent_shade, endpoint_glyph, light, panel_accent, recording_dot, refresh_mode, text, tint, tint_a, ui_font, DIVIDER_A,
    FOOTER_SHADE_A, PANEL_W, TRACK_H,
};

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
pub(super) const ROW_MARGIN: f32 = 4.0; // side margin of the row highlight/pill
pub(super) const ROW_RADIUS: f32 = 4.0; // corner radius of the row highlight
pub(super) const PILL_W: f32 = 3.0; // accent selection-indicator pill width
pub(super) const PILL_H: f32 = 16.0; // accent selection-indicator pill height
pub(super) const BATTERY_W: f32 = 80.0; // right-hand space reserved on battery rows (the readout + RIGHT_PAD)
// slider geometry
pub(super) const TRACK_X0: f32 = 52.0; // track left edge
pub(super) const VALUE_W: f32 = 46.0; // reserved right area for the percentage
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
pub(super) const FOOTER_SHADE: [u8; 3] = [0x00, 0x00, 0x00]; // the strip sits a shade darker than the body

// Fluent glyphs painted directly (not from the built-in IconId set).
pub(super) const GLYPH_MIC: char = glyph::MIC;
pub(super) const GLYPH_MIC_OFF: char = glyph::MIC_OFF;
pub(super) const GLYPH_SETTINGS: char = glyph::SETTINGS;
pub(super) const GLYPH_SOUND: char = glyph::VOLUME; // the footer's link to Windows' sound settings
pub(super) const GLYPH_UPDATE: char = glyph::UPDATE; // restart-to-update button

/// The footer strip's darkening, the same on every panel: the calendar's light palette takes 0.04
/// too ([`FOOTER_SHADE_A`] on the dark one).
pub(super) fn footer_shade_a() -> f32 {
    if light() { 0.04 } else { FOOTER_SHADE_A }
}

pub(super) const DARK_GLYPH: [u8; 3] = [0x12, 0x16, 0x1C]; // glyph colour on a solid accent disc
pub(super) const HOVER_A: f32 = 0.06; // white overlay for hover
pub(super) const SEL_A: f32 = 0.09; // white overlay for the selected row

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
pub(super) const GLYPH_PREVIOUS: char = glyph::PREVIOUS;
pub(super) const GLYPH_PLAY: char = glyph::PLAY;
pub(super) const GLYPH_PAUSE: char = glyph::PAUSE;
pub(super) const GLYPH_NEXT: char = glyph::NEXT;
pub(super) const GLYPH_DOWN: char = glyph::DOWN;
pub(super) const GLYPH_UP: char = glyph::UP;
pub(super) const GLYPH_MUSIC: char = glyph::MUSIC;
pub(super) const GLYPH_VIDEO: char = glyph::VIDEO;
pub(super) const GLYPH_APP: char = '\u{E7F5}'; // a speaker: an app in the mixer
pub(super) const GLYPH_APP_MUTED: char = glyph::MUTE;
pub(super) const SPEAKING: [u8; 3] = [0x3D, 0xD6, 0x8C]; // the speaking ring, as on the pill

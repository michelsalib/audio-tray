//! Design tokens for the flyout: the layout dimensions (in DIPs, scaled by the monitor
//! DPI at show time), the colour palette, the Segoe Fluent control glyphs, the UI fonts,
//! and the user's Windows accent colour. Everything visual and tunable lives here, so the
//! rest of the flyout reads as structure rather than magic numbers.

use std::sync::OnceLock;

use ab_glyph::FontVec;
use windows::core::w;

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
pub(super) const MENU_MIN_W: f32 = 200.0; // right-click menu minimum width
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
pub(super) const TRACK_H: f32 = 4.0;
pub(super) const THUMB_R: f32 = 7.0;
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
pub(super) const GLYPH_VOLUME: char = '\u{E767}';
pub(super) const GLYPH_MUTE: char = '\u{E74F}';
pub(super) const GLYPH_MIC: char = '\u{E720}';
pub(super) const GLYPH_MIC_OFF: char = '\u{EC54}';
pub(super) const GLYPH_EDIT: char = '\u{E70F}';
pub(super) const GLYPH_SETTINGS: char = '\u{E713}';
pub(super) const GLYPH_CANCEL: char = '\u{E711}';
pub(super) const GLYPH_BACK: char = '\u{E72B}'; // Back (leftward arrow) — the picker's cancel affordance
pub(super) const GLYPH_UPDATE: char = '\u{E72C}'; // Refresh (circular arrow) — restart-to-update banner

// Colours (RGB); alpha applied at blend time.
pub(super) const TINT: [u8; 3] = [0x2C, 0x2C, 0x2C]; // panel base (semi-transparent, acrylic shows through)
pub(super) const TINT_A: f32 = 0.82;
pub(super) const TEXT: [u8; 3] = [0xFF, 0xFF, 0xFF]; // primary text + glyphs
pub(super) const DARK_GLYPH: [u8; 3] = [0x12, 0x16, 0x1C]; // icon colour on a solid accent chip
pub(super) const HOVER_A: f32 = 0.06; // white overlay for hover
pub(super) const SEL_A: f32 = 0.09; // white overlay for the selected row

pub(super) fn ui_font() -> Option<&'static FontVec> {
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
pub(super) fn accent_rgb() -> [u8; 3] {
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
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let mut v: u32 = 0;
    let mut size = 4u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\DWM"),
            w!("AccentColor"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    if ok.0 == 0 {
        [(v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8]
    } else {
        [0x60, 0xCD, 0xFF] // fallback Win11 accent
    }
}

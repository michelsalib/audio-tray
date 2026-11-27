//! The look every panel shares: Windows' light or dark mode, the accent, the panels' width and
//! ground, the UI font, the recording dot and an endpoint's glyph. The flyout, the calendar, the
//! notifications, the OSD, Settings, the pill and the taskbar strip all paint from here.

use std::sync::OnceLock;

use ab_glyph::FontVec;
use windows::core::w;

use crate::canvas::{Canvas, Rect};
use crate::icons::glyph;

/// Every panel's width (the audio flyout's, the calendar's and the notifications' cards).
pub(crate) const PANEL_W: f32 = 380.0;
/// A slider's track thickness, in DIPs: the flyout's and the OSD's.
pub(crate) const TRACK_H: f32 = 4.0;
/// A footer's hairline (below ~0.1 it vanishes into the acrylic).
pub(crate) const DIVIDER_A: f32 = 0.14;
/// The footer strip's darkening on the dark panel.
pub(crate) const FOOTER_SHADE_A: f32 = 0.18;

// Colours (RGB); alpha applied at blend time.
pub(crate) const RECORDING: [u8; 3] = [0xE8, 0x11, 0x23]; // the "an app is recording" dot
pub(crate) const DARK_TINT: [u8; 3] = [0x2C, 0x2C, 0x2C]; // panel base (semi-transparent, acrylic shows through)
pub(crate) const DARK_TINT_A: f32 = 0.82;
/// The light panel's base: the calendar's and the notifications' light ground too.
pub(crate) const LIGHT_TINT: [u8; 3] = [0xEE, 0xEE, 0xEE];
pub(crate) const LIGHT_TINT_A: f32 = 0.86;

/// The panels follow Windows' light or dark mode (the system's, as the taskbar), as the calendar
/// and the notifications do. Read when a panel shows ([`refresh_mode`]), not on every paint.
static LIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Read Windows' mode again (a panel is about to show).
pub(crate) fn refresh_mode() {
    let light = crate::win::hkcu_dword(w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"), w!("SystemUsesLightTheme")) == Some(1);
    #[cfg(feature = "dev")]
    let light = std::env::var("AUDIO_TRAY_THEME").map_or(light, |t| t == "light");
    LIGHT.store(light, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the panels are light, as last read by [`refresh_mode`].
pub(crate) fn light() -> bool {
    LIGHT.load(std::sync::atomic::Ordering::Relaxed)
}

/// Primary text and glyphs; also what hover and selection washes are made of (at a low alpha).
pub(crate) fn text() -> [u8; 3] {
    if light() { [0x00, 0x00, 0x00] } else { [0xFF, 0xFF, 0xFF] }
}

/// The panel's base, translucent over the acrylic.
pub(crate) fn tint() -> [u8; 3] {
    if light() { LIGHT_TINT } else { DARK_TINT }
}

pub(crate) fn tint_a() -> f32 {
    if light() { LIGHT_TINT_A } else { DARK_TINT_A }
}

/// The accent a panel paints with: Windows' lighter shade on the dark panel, its darker on the
/// light one (as the calendar's).
pub(crate) fn panel_accent() -> [u8; 3] {
    if light() { accent_shade(4).unwrap_or([0x00, 0x5F, 0xB8]) } else { accent_rgb() }
}

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
    disc(cv, outer, [0xFF, 0xFF, 0xFF]);
    disc(cv, r, RECORDING);
}

/// The glyph for an endpoint's direction and mute state, shared by the flyout and the OSD
/// (colour stays at the call site).
pub(crate) fn endpoint_glyph(flow: crate::audio::Flow, muted: bool) -> char {
    use crate::audio::Flow;

    match (flow, muted) {
        (Flow::Output, false) => glyph::VOLUME,
        (Flow::Output, true) => glyph::MUTE,
        (Flow::Input, false) => glyph::MIC,
        (Flow::Input, true) => glyph::MIC_OFF,
    }
}

/// Segoe UI, the panels' text.
pub(crate) fn ui_font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        let bytes = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf").ok()?;
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

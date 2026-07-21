//! A custom Windows-11-style flyout, modelled on the system sound flyout: an
//! acrylic-blurred, rounded, dark surface with an accent "pill" on the selected row.
//!
//! Unlike a classic `TrackPopupMenu`, this is a live control surface: volume sliders,
//! mute toggles, output + input device switching, and an inline per-device icon picker,
//! all painted by hand into a per-pixel-alpha *layered* window (`UpdateLayeredWindow`),
//! with acrylic blur from the compositor (`SetWindowCompositionAttribute`) and rounded
//! corners from DWM. Rows and controls are laid out and hit-tested by hand; the flyout is
//! modal via mouse capture, like a menu, but stays open while you operate it.
//!
//! Left-click opens the full [`Trigger::LeftClick`] panel; right-click opens the tiny
//! [`Trigger::RightClick`] menu (Sound settings + Quit).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use windows::core::{s, w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
    HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
    KillTimer, LoadCursorW, PostMessageW, RegisterClassW, SetCursor, SetForegroundWindow, SetTimer,
    ShowWindow, SystemParametersInfoW, TranslateMessage, UpdateLayeredWindow, IDC_ARROW, MSG,
    SPI_GETWORKAREA, SW_SHOWNA, SW_SHOWNORMAL, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, ULW_ALPHA,
    WM_APP, WM_CAPTURECHANGED, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_RBUTTONDOWN, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::audio::wasapi::{Meter, VolumeWatch, WasapiBackend};
use crate::audio::{DeviceId, Flow};
use crate::config::Config;
use crate::icons::{self, IconId};

/// Which entry point opened the flyout.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// The full audio control panel.
    LeftClick,
    /// The tiny quick menu (Sound settings + Quit).
    RightClick,
}

/// What the caller must do after the flyout closes.
pub struct Outcome {
    pub quit: bool,
    pub config_changed: bool,
    /// The default *output* device was switched while the flyout was open. The
    /// endpoint-change notifications that would refresh the tray icon are consumed by our
    /// own modal message loop, so the caller must refresh explicitly.
    pub output_changed: bool,
    /// The user clicked the "restart to update" entry — the caller should relaunch the
    /// (already-updated on disk) exe and exit.
    pub restart: bool,
}

/// Where to open the flyout: horizontally centred on the tray icon (`cx`), sitting just
/// above it (`bottom` = the icon's top), like the native tray flyouts.
#[derive(Clone, Copy)]
pub struct Anchor {
    pub cx: i32,
    pub bottom: i32,
}

// Layout, in DIPs (scaled by the monitor DPI at show time). Tuned to the native Win11
// sound flyout: roomy rows, a semibold section header, an accent selection pill.
const CORNER: f32 = 8.0;
const PAD_V: f32 = 6.0; // top/bottom padding inside the panel
const HEADER_FIRST_H: f32 = 30.0; // first section header (modest top gap)
const HEADER_H: f32 = 36.0; // later section headers (larger top gap → group separation)
const SLIDER_H: f32 = 48.0; // volume-slider row
const ITEM_H: f32 = 44.0; // device / action row
const ICON_X: f32 = 14.0; // left inset of a row's leading icon
const ICON_PX: f32 = 20.0; // leading icon glyph size
const TEXT_X: f32 = 48.0; // left inset of a row's label
const HEADER_X: f32 = 15.0; // left inset of a section-header label
const RIGHT_PAD: f32 = 16.0;
const MIN_W: f32 = 340.0; // panel minimum width
const MENU_MIN_W: f32 = 200.0; // right-click menu minimum width
const MAX_W: f32 = 420.0; // cap on the panel width (driven by device-name length)
const ROW_MARGIN: f32 = 4.0; // side margin of the row highlight/pill
const ROW_RADIUS: f32 = 4.0; // corner radius of the row highlight
const PILL_W: f32 = 3.0; // accent selection-indicator pill width
const PILL_H: f32 = 16.0; // accent selection-indicator pill height
const PENCIL_W: f32 = 42.0; // right-hand space reserved for the edit affordance (label stops here)
const PENCIL_BTN: f32 = 30.0; // the pencil's round hover-button diameter
const PENCIL_RIGHT: f32 = 9.0; // gap from the panel's right edge to the button
const BATTERY_W: f32 = 96.0; // right-hand space reserved on battery rows (fits battery + hover pencil)
// slider geometry
const TRACK_X0: f32 = 52.0; // track left edge
const VALUE_W: f32 = 46.0; // reserved right area for the percentage
const TRACK_H: f32 = 4.0;
const THUMB_R: f32 = 7.0;
// icon-picker page (a dedicated screen you slide to from a device's edit pencil)
const PICKER_HEADER_H: f32 = 46.0; // back-arrow + device-name title row
const BACK_LEFT: f32 = 7.0; // left inset of the back button
const BACK_BTN: f32 = 32.0; // back button's round hover target diameter
const BACK_GLYPH_PX: f32 = 16.0; // back chevron glyph size
const TITLE_PX: f32 = 15.0; // picker title (device name) em size
// wrapping icon grid
const GRID_CHIP: f32 = 44.0; // one icon cell (square)
const GRID_GAP: f32 = 8.0; // gap between cells (both axes)
const GRID_X: f32 = 14.0; // grid side inset (used to size columns)
const GRID_TOP_PAD: f32 = 4.0; // gap above the first grid row
const GRID_BOTTOM_PAD: f32 = 10.0; // gap below the last grid row
const GRID_ICON_RATIO: f32 = 0.55; // glyph size inside a cell

// Posted (by the WASAPI volume callback) when a watched endpoint's volume/mute changes,
// so external changes (media keys, other apps) are reflected live while we're open.
const WM_VOL_CHANGED: u32 = WM_APP + 10;
// Posted by the window proc when we lose mouse capture (another window/app took focus,
// e.g. the Start menu). WM_CAPTURECHANGED is *sent* to the proc, not queued, so we bounce
// it back as a posted message the modal loop can act on to dismiss.
const WM_FLYOUT_CLOSE: u32 = WM_APP + 11;

// A Win32 timer drives ~30 fps sampling of each default endpoint's live peak level
// (IAudioMeterInformation) so the slider fill reacts to real audio while we're open.
const METER_TIMER_ID: usize = 1;
const METER_INTERVAL_MS: u32 = 33;
// Per-tick fall-off of the displayed peak: instant attack, gentle release (a VU-meter feel).
const METER_DECAY: f32 = 0.82;

// Fluent glyphs painted directly (not from the built-in IconId set).
const GLYPH_VOLUME: char = '\u{E767}';
const GLYPH_MUTE: char = '\u{E74F}';
const GLYPH_MIC: char = '\u{E720}';
const GLYPH_MIC_OFF: char = '\u{EC54}';
const GLYPH_EDIT: char = '\u{E70F}';
const GLYPH_SETTINGS: char = '\u{E713}';
const GLYPH_CANCEL: char = '\u{E711}';
const GLYPH_BACK: char = '\u{E72B}'; // Back (leftward arrow) — the picker's cancel affordance
const GLYPH_UPDATE: char = '\u{E72C}'; // Refresh (circular arrow) — restart-to-update banner

// Colours (RGB); alpha applied at blend time.
const TINT: [u8; 3] = [0x2C, 0x2C, 0x2C]; // panel base (semi-transparent, acrylic shows through)
const TINT_A: f32 = 0.82;
const TEXT: [u8; 3] = [0xFF, 0xFF, 0xFF]; // primary text + glyphs
const DARK_GLYPH: [u8; 3] = [0x12, 0x16, 0x1C]; // icon colour on a solid accent chip
const HOVER_A: f32 = 0.06; // white overlay for hover
const SEL_A: f32 = 0.09; // white overlay for the selected row

#[derive(Clone, Copy)]
enum ActionKind {
    SoundSettings,
    Quit,
}

impl ActionKind {
    fn label(self) -> &'static str {
        match self {
            ActionKind::SoundSettings => "Sound settings",
            ActionKind::Quit => "Quit Audio Tray",
        }
    }
    fn glyph(self) -> char {
        match self {
            ActionKind::SoundSettings => GLYPH_SETTINGS,
            ActionKind::Quit => GLYPH_CANCEL,
        }
    }
}

/// Which screen the flyout is showing. The icon picker is a *dedicated* sub-screen you
/// slide to from a device row's edit pencil (rather than an inline row), so it can lay its
/// icons out in a wrapping grid without ever changing the flyout's width.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    /// The main audio panel (sliders + device lists).
    Main,
    /// The per-device icon chooser: a back arrow, the device name as title, and a grid.
    IconPicker { group: usize, dev: usize },
}

#[derive(Clone, Copy)]
enum Elem {
    Header(&'static str),
    Slider { group: usize },
    Device { group: usize, dev: usize },
    /// The icon-picker screen's header: a back arrow + the device name as the title.
    PickerHeader { group: usize, dev: usize },
    /// The icon-picker screen's wrapping grid of selectable icons.
    IconGrid { group: usize, dev: usize },
    /// A "restart to update to v…" call-to-action at the bottom of the main panel, shown
    /// only when a background update has been staged on disk.
    UpdateBanner,
    Action(ActionKind),
}

struct LaidElem {
    elem: Elem,
    top: i32,
    height: i32,
}

struct DeviceRow {
    id: DeviceId,
    label: String,
    icon: IconId,
    selected: bool,
    battery: Option<u8>, // Bluetooth battery 0..=100, if the device reports one
}

struct Group {
    flow: Flow,
    title: &'static str,
    default_id: Option<DeviceId>,
    level: f32, // 0.0..=1.0 of the default endpoint
    muted: bool,
    peak: f32, // smoothed live peak level 0.0..=1.0 of the default endpoint (activity glow)
    devices: Vec<DeviceRow>,
}

struct Flyout<'a> {
    backend: &'a WasapiBackend,
    config: &'a mut Config,
    trigger: Trigger,
    scale: f32,
    accent: [u8; 3],
    width: i32,
    height: i32,
    groups: Vec<Group>,
    view: View,               // which screen is shown (main panel / an icon picker)
    elems: Vec<LaidElem>,
    hover: Option<usize>,
    hover_pencil: bool,          // the cursor is over the hovered device row's edit pencil
    hover_back: bool,            // the cursor is over the picker's back button
    hover_chip: Option<usize>,   // chip index the cursor is over, within the icon grid
    drag: Option<usize>,    // index into elems of the slider being dragged
    pending: Option<usize>, // index pressed on button-down, acted on button-up
    watches: Vec<Option<VolumeWatch>>, // per-group volume/mute change subscriptions
    meters: Vec<Option<Meter>>,        // per-group live peak meters (polled on a timer)
    vol_dirty: Arc<AtomicBool>,        // shared coalescing flag for the volume callbacks
    config_changed: bool,
    output_changed: bool,
    quit: bool,
    restart: bool,
    update: Option<String>, // staged update's version, if any → bottom "restart to update" row
    base: Vec<u8>, // static content, re-rendered on model changes
    buf: Vec<u8>,  // base + dynamic overlays (sliders, hover), presented each frame
    hwnd: HWND,
    x: i32,
    y: i32,
    base_cx: i32,     // horizontal anchor (icon centre / cursor)
    base_bottom: i32, // bottom edge to sit above
    wa: RECT,         // work area
    margin: i32,
}

/// Show the flyout near the tray and operate it until the user dismisses it.
pub fn show(
    backend: &WasapiBackend,
    config: &mut Config,
    anchor: Option<Anchor>,
    trigger: Trigger,
) -> Outcome {
    unsafe { show_inner(backend, config, anchor, trigger, false) }
}

/// Dev preview: open straight onto the first output device's icon-picker screen (so the
/// picker can be iterated on without first hovering + clicking a device's edit pencil).
pub fn show_icons_preview(
    backend: &WasapiBackend,
    config: &mut Config,
    anchor: Option<Anchor>,
) -> Outcome {
    unsafe { show_inner(backend, config, anchor, Trigger::LeftClick, true) }
}

unsafe fn show_inner(
    backend: &WasapiBackend,
    config: &mut Config,
    anchor: Option<Anchor>,
    trigger: Trigger,
    start_icons: bool,
) -> Outcome {
    let scale = (GetDpiForSystem() as f32 / 96.0).max(1.0);
    let accent = accent_rgb();

    let groups = match trigger {
        Trigger::LeftClick => build_groups(backend, config),
        Trigger::RightClick => Vec::new(),
    };

    let mut fly = Flyout {
        backend,
        config,
        trigger,
        scale,
        accent,
        width: 0,
        height: 0,
        groups,
        view: View::Main,
        elems: Vec::new(),
        hover: None,
        hover_pencil: false,
        hover_back: false,
        hover_chip: None,
        drag: None,
        pending: None,
        watches: Vec::new(),
        meters: Vec::new(),
        vol_dirty: Arc::new(AtomicBool::new(false)),
        config_changed: false,
        output_changed: false,
        quit: false,
        restart: false,
        // Only the full left-click panel offers the restart-to-update entry.
        update: match trigger {
            Trigger::LeftClick => crate::update::pending_version(),
            Trigger::RightClick => None,
        },
        base: Vec::new(),
        buf: Vec::new(),
        hwnd: HWND(std::ptr::null_mut()),
        x: 0,
        y: 0,
        base_cx: 0,
        base_bottom: 0,
        wa: RECT::default(),
        margin: (8.0 * scale) as i32,
    };

    // Resolve the anchor: bottom-right above the tray icon, else the cursor.
    let _ = SystemParametersInfoW(
        SPI_GETWORKAREA,
        0,
        Some(&mut fly.wa as *mut _ as *mut std::ffi::c_void),
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    );
    let gap = (8.0 * scale) as i32;
    let (cx, bottom) = match anchor {
        Some(a) => (a.cx, a.bottom - gap),
        None => {
            let mut cur = POINT::default();
            let _ = GetCursorPos(&mut cur);
            (cur.x, cur.y)
        }
    };
    fly.base_cx = cx;
    fly.base_bottom = bottom.min(fly.wa.bottom - fly.margin);

    // Dev preview: jump straight to the first output device's icon picker.
    if start_icons && fly.groups.first().is_some_and(|g| !g.devices.is_empty()) {
        fly.view = View::IconPicker { group: 0, dev: 0 };
    }

    fly.rebuild_layout();
    fly.reposition();

    if let Err(e) = fly.create_window() {
        eprintln!("flyout: create_window failed: {e:?}");
        return Outcome { quit: false, config_changed: false, output_changed: false, restart: false };
    }

    fly.render_base();
    fly.compose();
    let _ = ShowWindow(fly.hwnd, SW_SHOWNA);
    fly.animate_in();
    // Foreground + capture so the flyout behaves like a menu: it sees every mouse
    // move/click, and an outside click reliably dismisses it.
    let _ = SetForegroundWindow(fly.hwnd);
    SetCapture(fly.hwnd);
    // While the mouse is captured Windows stops sending WM_SETCURSOR, so force the arrow.
    let _ = SetCursor(LoadCursorW(None, IDC_ARROW).ok());
    // Subscribe to external volume/mute changes (media keys, other apps) while we're open.
    fly.setup_watches();
    // Poll each endpoint's live peak meter ~30 fps so the slider fill reacts to audio.
    let _ = SetTimer(Some(fly.hwnd), METER_TIMER_ID, METER_INTERVAL_MS, None);

    let mut msg = MSG::default();
    'pump: while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
        match msg.message {
            WM_MOUSEMOVE => {
                let (mx, my) = mouse_xy(msg.lParam);
                if let Some(si) = fly.drag {
                    if let Elem::Slider { group } = fly.elems[si].elem {
                        let level = fly.level_from_x(mx);
                        fly.set_group_level(group, level);
                        fly.compose();
                        fly.present(fly.x, fly.y, 255);
                    }
                } else {
                    let inside = fly.inside(mx, my);
                    let hover = if inside { fly.elem_at(my) } else { None };
                    let kind = hover.map(|i| fly.elems[i].elem);
                    let on_pencil =
                        matches!(kind, Some(Elem::Device { .. })) && fly.over_pencil(mx);
                    let on_back =
                        matches!(kind, Some(Elem::PickerHeader { .. })) && fly.over_back(mx);
                    let on_chip = match (kind, hover) {
                        (Some(Elem::IconGrid { .. }), Some(i)) => {
                            fly.grid_chip_at(mx, my, fly.elems[i].top)
                        }
                        _ => None,
                    };
                    if hover != fly.hover
                        || on_pencil != fly.hover_pencil
                        || on_back != fly.hover_back
                        || on_chip != fly.hover_chip
                    {
                        fly.hover = hover;
                        fly.hover_pencil = on_pencil;
                        fly.hover_back = on_back;
                        fly.hover_chip = on_chip;
                        fly.compose();
                        fly.present(fly.x, fly.y, 255);
                    }
                }
            }
            WM_LBUTTONDOWN => {
                let (mx, my) = mouse_xy(msg.lParam);
                if !fly.inside(mx, my) {
                    break 'pump; // click outside → dismiss
                }
                fly.pending = None;
                if let Some(i) = fly.elem_at(my) {
                    if let Elem::Slider { group } = fly.elems[i].elem {
                        fly.press_slider(i, group, mx);
                    } else {
                        fly.pending = Some(i);
                    }
                }
            }
            WM_LBUTTONUP => {
                if let Some(si) = fly.drag.take() {
                    // Only an *output* volume change plays the "ding"; changing the input
                    // (mic) level shouldn't trigger a notification sound.
                    if let Elem::Slider { group } = fly.elems[si].elem {
                        if fly.groups[group].flow == Flow::Output {
                            beep_volume();
                        }
                    }
                    continue; // stay open
                }
                let (mx, my) = mouse_xy(msg.lParam);
                if fly.inside(mx, my) {
                    if let Some(i) = fly.elem_at(my) {
                        if fly.pending == Some(i) && fly.activate(i, mx, my) {
                            break 'pump;
                        }
                    }
                }
                fly.pending = None;
            }
            WM_RBUTTONDOWN => {
                let (mx, my) = mouse_xy(msg.lParam);
                if !fly.inside(mx, my) {
                    break 'pump;
                }
            }
            WM_VOL_CHANGED => fly.refresh_volumes(),
            WM_TIMER => fly.tick_meters(),
            WM_KEYDOWN if msg.wParam.0 as u16 == VK_ESCAPE.0 => break 'pump,
            WM_FLYOUT_CLOSE => break 'pump, // lost capture (Start menu, Alt-Tab, …)
            _ => {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    let _ = KillTimer(Some(fly.hwnd), METER_TIMER_ID);
    fly.watches.clear(); // unregister the volume callbacks before the window goes away
    fly.meters.clear(); // release the peak-meter interfaces too
    let _ = ReleaseCapture();
    let _ = DestroyWindow(fly.hwnd);
    Outcome {
        quit: fly.quit,
        config_changed: fly.config_changed,
        output_changed: fly.output_changed,
        restart: fly.restart,
    }
}

/// Read the current output + input state into display groups. Groups with no devices are
/// omitted (e.g. a machine with no microphone shows no Input section).
fn build_groups(backend: &WasapiBackend, config: &Config) -> Vec<Group> {
    // Bluetooth battery levels keyed by ContainerId — enumerated once for all devices.
    let batteries = crate::audio::battery::levels();
    let battery_of = |container: &Option<String>| -> Option<u8> {
        let c = container.as_ref()?;
        batteries
            .iter()
            .find(|(id, _)| id.eq_ignore_ascii_case(c))
            .map(|(_, pct)| *pct)
    };

    let mut groups = Vec::new();
    for (flow, title) in [(Flow::Output, "Output"), (Flow::Input, "Input")] {
        let devices = backend.enumerate_flow(flow).unwrap_or_default();
        if devices.is_empty() {
            continue;
        }
        let default_id = backend.default_of(flow).ok().flatten();
        let (level, muted) = match &default_id {
            Some(id) => (
                backend.volume_of(id).unwrap_or(0.0),
                backend.is_muted(id).unwrap_or(false),
            ),
            None => (0.0, false),
        };
        let rows = devices
            .into_iter()
            .map(|d| {
                let icon = config
                    .icon_for(&d.id.0)
                    .unwrap_or_else(|| icons::default_icon(d.form_factor, &d.friendly_name));
                let selected = default_id.as_ref() == Some(&d.id);
                let battery = battery_of(&d.container_id);
                DeviceRow { id: d.id, label: d.friendly_name, icon, selected, battery }
            })
            .collect();
        groups.push(Group { flow, title, default_id, level, muted, peak: 0.0, devices: rows });
    }
    groups
}

impl Flyout<'_> {
    fn reset_hover(&mut self) {
        self.hover = None;
        self.hover_pencil = false;
        self.hover_back = false;
        self.hover_chip = None;
        self.drag = None;
        self.pending = None;
    }

    /// Size the panel and lay out the current view, allocating buffers. Invalidates
    /// transient hit state. Both the width and the height are fixed for the flyout's whole
    /// life — the width from the main list, the height from the tallest screen — so
    /// navigating between the main panel and an icon picker never resizes the window.
    fn rebuild_layout(&mut self) {
        self.reset_hover();
        self.width = self.main_content_width();
        self.height = self.panel_height();
        let (elems, _) = self.build_view(self.view, self.height);
        self.elems = elems;
        let bytes = (self.width * self.height * 4) as usize;
        self.base = vec![0u8; bytes];
        self.buf = vec![0u8; bytes];
    }

    /// The fixed panel height shared by every screen: the taller of the main panel and the
    /// icon picker (in practice the main panel, which has the sliders + device lists).
    fn panel_height(&self) -> i32 {
        let main_h = self.build_view(View::Main, 0).1;
        let picker_h = if matches!(self.trigger, Trigger::LeftClick)
            && self.groups.iter().any(|g| !g.devices.is_empty())
        {
            self.build_view(View::IconPicker { group: 0, dev: 0 }, 0).1
        } else {
            0
        };
        main_h.max(picker_h)
    }

    /// The label for the restart-to-update banner, if an update is staged.
    fn update_label(&self) -> Option<String> {
        self.update.as_ref().map(|v| format!("Restart to update to v{v}"))
    }

    /// The panel width, measured from the *main* view's content only. Constant for the life
    /// of the flyout (the device set and labels don't change while open), so it anchors the
    /// width for every screen — the icon picker wraps its grid into this width rather than
    /// forcing the panel wider.
    fn main_content_width(&self) -> i32 {
        let scale = self.scale;
        let font = ui_font();
        let font_sb = ui_font_semibold().or(font);
        let text_px = 14.0 * scale;
        let hdr_px = 14.0 * scale;
        let mw = |f: Option<&FontVec>, px: f32, s: &str| f.map(|f| measure(f, px, s)).unwrap_or(0.0);

        let mut max_w = 0.0f32;
        match self.trigger {
            Trigger::RightClick => {
                for k in [ActionKind::SoundSettings, ActionKind::Quit] {
                    max_w = max_w.max(TEXT_X * scale + mw(font, text_px, k.label()) + RIGHT_PAD * scale);
                }
            }
            Trigger::LeftClick => {
                for g in &self.groups {
                    max_w = max_w.max(HEADER_X * scale + mw(font_sb, hdr_px, g.title) + RIGHT_PAD * scale);
                    if g.default_id.is_some() {
                        max_w = max_w.max((TRACK_X0 + 130.0 + VALUE_W) * scale);
                    }
                    for row in &g.devices {
                        let reserve = if row.battery.is_some() { BATTERY_W } else { PENCIL_W };
                        max_w = max_w.max(TEXT_X * scale + mw(font, text_px, &row.label) + reserve * scale);
                    }
                }
                if let Some(label) = self.update_label() {
                    max_w = max_w.max(TEXT_X * scale + mw(font, text_px, &label) + RIGHT_PAD * scale);
                }
            }
        }
        let min_w = match self.trigger {
            Trigger::LeftClick => MIN_W,
            Trigger::RightClick => MENU_MIN_W,
        };
        max_w.clamp(min_w * scale, MAX_W * scale).round() as i32
    }

    /// Build (and vertically lay out) the elements for `view`, returning them plus the total
    /// panel height. Pure over `self` — used both to render the current screen and to render
    /// the two screens involved in a slide transition. Uses the fixed `self.width`.
    ///
    /// `fill_h` is the fixed panel height every screen shares (so navigating never resizes
    /// the window): the layout is grown to at least `fill_h`, and on the icon-picker screen
    /// the grid is centred in the slack below the header. Pass `0` to lay out naturally
    /// (used once, to measure each screen's intrinsic height).
    fn build_view(&self, view: View, fill_h: i32) -> (Vec<LaidElem>, i32) {
        let scale = self.scale;
        let d = |v: f32| (v * scale).round() as i32;
        let mut kinds: Vec<Elem> = Vec::new();
        match (self.trigger, view) {
            (Trigger::RightClick, _) => {
                kinds.push(Elem::Action(ActionKind::SoundSettings));
                kinds.push(Elem::Action(ActionKind::Quit));
            }
            (Trigger::LeftClick, View::Main) => {
                for (gi, g) in self.groups.iter().enumerate() {
                    kinds.push(Elem::Header(g.title));
                    if g.default_id.is_some() {
                        kinds.push(Elem::Slider { group: gi });
                    }
                    for di in 0..g.devices.len() {
                        kinds.push(Elem::Device { group: gi, dev: di });
                    }
                }
                // A staged update gets a restart call-to-action pinned to the very bottom.
                if self.update.is_some() {
                    kinds.push(Elem::UpdateBanner);
                }
            }
            (Trigger::LeftClick, View::IconPicker { group, dev }) => {
                kinds.push(Elem::PickerHeader { group, dev });
                kinds.push(Elem::IconGrid { group, dev });
            }
        }

        let mut elems = Vec::with_capacity(kinds.len());
        let mut y = d(PAD_V);
        for (i, elem) in kinds.into_iter().enumerate() {
            let height = match elem {
                // The first header sits at the very top (small gap); a later header separates
                // one group from the one above it.
                Elem::Header(_) => d(if i == 0 { HEADER_FIRST_H } else { HEADER_H }),
                Elem::Slider { .. } => d(SLIDER_H),
                Elem::Device { .. } => d(ITEM_H),
                Elem::PickerHeader { .. } => d(PICKER_HEADER_H),
                Elem::IconGrid { .. } => self.grid_px_height(),
                Elem::UpdateBanner => d(ITEM_H),
                Elem::Action(_) => d(ITEM_H),
            };
            elems.push(LaidElem { elem, top: y, height });
            y += height;
        }
        let natural = y + d(PAD_V);
        let total = natural.max(fill_h);

        // Centre the icon grid in any extra vertical space, so the picker fills the shared
        // panel height without a big empty band at the bottom (the header stays pinned top).
        let slack = total - natural;
        if slack > 0 {
            for le in &mut elems {
                if matches!(le.elem, Elem::IconGrid { .. }) {
                    le.top += slack / 2;
                }
            }
        }
        (elems, total)
    }

    /// Icon-grid geometry for the current width — see the free [`grid_metrics`].
    fn grid_metrics(&self) -> (i32, i32, i32, i32) {
        grid_metrics(self.width, self.scale)
    }

    /// Total pixel height of the wrapping icon grid (top pad + rows + bottom pad).
    fn grid_px_height(&self) -> i32 {
        let scale = self.scale;
        let (cols, _left, chip, step) = self.grid_metrics();
        let gap = step - chip;
        let n = IconId::ALL.len() as i32;
        let rows = (n + cols - 1) / cols;
        (GRID_TOP_PAD * scale).round() as i32
            + rows * chip
            + (rows - 1).max(0) * gap
            + (GRID_BOTTOM_PAD * scale).round() as i32
    }

    /// Position the panel: centred on the anchor, sitting above it, clamped to the work
    /// area. Recomputed whenever the size changes so it keeps its bottom edge.
    fn reposition(&mut self) {
        self.x = (self.base_cx - self.width / 2)
            .min(self.wa.right - self.margin - self.width)
            .max(self.wa.left + self.margin);
        self.y = (self.base_bottom - self.height).max(self.wa.top + self.margin);
    }

    fn inside(&self, mx: i32, my: i32) -> bool {
        (0..self.width).contains(&mx) && (0..self.height).contains(&my)
    }

    /// Index of the actionable element at vertical position `y`.
    fn elem_at(&self, y: i32) -> Option<usize> {
        self.elems.iter().position(|le| {
            let actionable = matches!(
                le.elem,
                Elem::Slider { .. }
                    | Elem::Device { .. }
                    | Elem::PickerHeader { .. }
                    | Elem::IconGrid { .. }
                    | Elem::UpdateBanner
                    | Elem::Action(_)
            );
            actionable && y >= le.top && y < le.top + le.height
        })
    }

    fn level_from_x(&self, mx: i32) -> f32 {
        let scale = self.scale;
        let x0 = TRACK_X0 * scale;
        let x1 = self.width as f32 - VALUE_W * scale;
        (((mx as f32) - x0) / (x1 - x0)).clamp(0.0, 1.0)
    }

    /// Whether `mx` is over the edit pencil's round button (its hover/click target).
    fn over_pencil(&self, mx: i32) -> bool {
        let cx = pencil_center_x(self.width, self.scale);
        ((mx as f32) - cx).abs() <= PENCIL_BTN * self.scale / 2.0
    }

    /// Whether `mx` is over the picker's back button (its hover/click target).
    fn over_back(&self, mx: i32) -> bool {
        let scale = self.scale;
        let x0 = BACK_LEFT * scale;
        let x1 = (BACK_LEFT + BACK_BTN) * scale;
        (mx as f32) >= x0 && (mx as f32) <= x1
    }

    /// Which icon-grid cell (if any) is at `(mx, my)`, given the grid element's top `gy`.
    fn grid_chip_at(&self, mx: i32, my: i32, gy: i32) -> Option<usize> {
        let scale = self.scale;
        let (cols, left, chip, step) = self.grid_metrics();
        let gy0 = gy + (GRID_TOP_PAD * scale).round() as i32;
        if mx < left || my < gy0 {
            return None;
        }
        let col = (mx - left) / step;
        let row = (my - gy0) / step;
        let within_x = (mx - left) - col * step;
        let within_y = (my - gy0) - row * step;
        if col >= cols || within_x > chip || within_y > chip {
            return None;
        }
        let k = (row * cols + col) as usize;
        (k < IconId::ALL.len()).then_some(k)
    }

    fn set_group_level(&mut self, group: usize, level: f32) {
        self.groups[group].level = level;
        if let Some(id) = self.groups[group].default_id.clone() {
            let _ = self.backend.set_volume_of(&id, level);
        }
    }

    /// Subscribe to volume/mute changes and the peak meter on each group's default endpoint.
    fn setup_watches(&mut self) {
        self.watches = (0..self.groups.len()).map(|_| None).collect();
        self.meters = (0..self.groups.len()).map(|_| None).collect();
        for group in 0..self.groups.len() {
            self.rewatch(group);
        }
    }

    /// (Re)subscribe a group to its current default endpoint — called at open and whenever
    /// the default is switched from within the flyout (the old endpoint's watch is dropped,
    /// which unregisters it). Also re-activates the peak meter that feeds the activity glow.
    fn rewatch(&mut self, group: usize) {
        if group >= self.watches.len() {
            return;
        }
        let hwnd = self.hwnd.0 as isize;
        let backend = self.backend;
        let pending = Arc::clone(&self.vol_dirty);
        let flow = self.groups[group].flow;
        let id = self.groups[group].default_id.clone();
        self.watches[group] = id
            .as_ref()
            .and_then(|id| backend.watch_volume(id, hwnd, WM_VOL_CHANGED, pending).ok());
        // The activity glow is output-only: metering an input endpoint needs a running
        // capture stream, which would keep Windows' "microphone in use" indicator lit the
        // whole time the flyout is open. So we only meter output; the input slider stays a
        // plain slider (its peak holds at 0, which the additive glow renders as no glow).
        self.meters[group] = match flow {
            Flow::Output => id.as_ref().and_then(|id| backend.meter_for(id, flow).ok()),
            Flow::Input => None,
        };
    }

    /// Sample each default endpoint's live peak level and fold it into the smoothed `peak`
    /// (fast attack, gentle release), then repaint if anything moved. Driven by the ~30 fps
    /// timer. A muted endpoint reads as silent so its fill settles back to the resting glow.
    fn tick_meters(&mut self) {
        let mut changed = false;
        for group in 0..self.groups.len() {
            let raw = if self.groups[group].muted {
                0.0
            } else {
                self.meters.get(group).and_then(|m| m.as_ref()).map_or(0.0, |m| m.peak())
            };
            let g = &mut self.groups[group];
            let shown = if raw >= g.peak { raw } else { (g.peak * METER_DECAY).max(raw) };
            if (shown - g.peak).abs() > 0.004 {
                changed = true;
            }
            g.peak = shown;
        }
        if changed {
            self.compose();
            self.present(self.x, self.y, 255);
        }
    }

    /// Re-read each default endpoint's volume/mute (from the cached watch interface, no COM
    /// re-activation) so external changes show up live. Skipped mid-drag so it never fights
    /// the user's own slider.
    fn refresh_volumes(&mut self) {
        // Clear the coalescing flag *before* reading, so a change that lands during the
        // read re-arms and posts again (we never miss the final state).
        self.vol_dirty.store(false, Ordering::SeqCst);
        if self.drag.is_some() {
            return;
        }
        let backend = self.backend;
        let mut vol_changed = false;
        let mut mute_changed = false;
        for group in 0..self.groups.len() {
            let reading = match self.watches.get(group).and_then(|w| w.as_ref()) {
                Some(w) => w.read(),
                None => self.groups[group].default_id.as_ref().and_then(|id| {
                    Some((backend.volume_of(id).ok()?, backend.is_muted(id).ok()?))
                }),
            };
            if let Some((v, m)) = reading {
                let g = &mut self.groups[group];
                if (v - g.level).abs() > 0.001 {
                    g.level = v;
                    vol_changed = true;
                }
                if m != g.muted {
                    g.muted = m;
                    mute_changed = true;
                }
            }
        }
        // A mute flip swaps the slider's leading glyph (which lives in `base`); a plain
        // volume change only moves the fill/thumb/number, all drawn in the cheap `compose`
        // overlay — so avoid re-rasterizing every glyph for a mere volume tick (a mic's
        // auto-gain can fire these constantly).
        if mute_changed {
            self.render_base();
        }
        if mute_changed || vol_changed {
            self.compose();
            self.present(self.x, self.y, 255);
        }
    }

    /// A press on a slider row: the leading icon area toggles mute, the rest starts a drag.
    fn press_slider(&mut self, elem: usize, group: usize, mx: i32) {
        let scale = self.scale;
        if (mx as f32) < (TRACK_X0 - 6.0) * scale {
            if let Some(id) = self.groups[group].default_id.clone() {
                let muted = !self.groups[group].muted;
                if let Err(e) = self.backend.set_muted(&id, muted) {
                    eprintln!("mute failed: {e:#}");
                }
                self.groups[group].muted = muted;
                self.render_base();
                self.compose();
                self.present(self.x, self.y, 255);
            }
        } else {
            self.drag = Some(elem);
            let level = self.level_from_x(mx);
            self.set_group_level(group, level);
            self.compose();
            self.present(self.x, self.y, 255);
        }
    }

    /// Act on a click at button-up. Returns true if the flyout should close.
    fn activate(&mut self, i: usize, mx: i32, my: i32) -> bool {
        match self.elems[i].elem {
            Elem::Device { group, dev } => {
                if self.over_pencil(mx) {
                    // Slide to the device's dedicated icon-picker screen.
                    self.navigate(View::IconPicker { group, dev }, true);
                } else {
                    let id = self.groups[group].devices[dev].id.clone();
                    if self.groups[group].default_id.as_ref() != Some(&id) {
                        if let Err(e) = self.backend.set_default_of(&id) {
                            eprintln!("switch failed: {e:#}");
                        }
                        if self.groups[group].flow == Flow::Output {
                            self.output_changed = true;
                        }
                        for row in &mut self.groups[group].devices {
                            row.selected = row.id == id;
                        }
                        self.groups[group].level =
                            self.backend.volume_of(&id).unwrap_or(self.groups[group].level);
                        self.groups[group].muted = self.backend.is_muted(&id).unwrap_or(false);
                        self.groups[group].default_id = Some(id);
                        self.rewatch(group); // follow volume/mute of the new default
                        self.render_base();
                        self.compose();
                        self.present(self.x, self.y, 255);
                    }
                }
                false
            }
            Elem::PickerHeader { .. } => {
                // The back arrow cancels the picker and slides back to the main panel.
                if self.over_back(mx) {
                    self.navigate(View::Main, false);
                }
                false
            }
            Elem::IconGrid { group, dev } => {
                // Clicking an icon validates the choice: persist it, then slide back.
                if let Some(ci) = self.grid_chip_at(mx, my, self.elems[i].top) {
                    let icon = IconId::ALL[ci];
                    let id = self.groups[group].devices[dev].id.0.clone();
                    self.groups[group].devices[dev].icon = icon;
                    self.config.set_icon(id, icon);
                    self.config_changed = true;
                    if let Err(e) = self.config.save() {
                        eprintln!("save config failed: {e:#}");
                    }
                    self.navigate(View::Main, false);
                }
                false
            }
            Elem::UpdateBanner => {
                // The update is already on disk; the caller relaunches the exe and exits.
                self.restart = true;
                true
            }
            Elem::Action(ActionKind::SoundSettings) => {
                open_sound_settings();
                true
            }
            Elem::Action(ActionKind::Quit) => {
                self.quit = true;
                true
            }
            _ => false,
        }
    }

    /// Slide-transition from the current screen to `to`, then commit it. `forward` slides
    /// the new screen in from the right (drilling into the picker); otherwise it comes from
    /// the left (backing out). The window keeps a constant size — the width and height are
    /// both fixed — so this is a pure horizontal slide with no resize.
    fn navigate(&mut self, to: View, forward: bool) {
        let (w, h) = (self.width, self.height);
        let n = (w * h * 4) as usize;
        // Outgoing screen: reuse the current composed frame (keeps its slider fills etc.).
        let src = self.buf.clone();
        let (dst_elems, _) = self.build_view(to, h);
        let mut dst = vec![0u8; n];
        self.render_page(&dst_elems, &mut dst, h);
        let mut frame = vec![0u8; n];

        let frames = 9;
        for i in 1..=frames {
            let t = i as f32 / frames as f32;
            let ease = 1.0 - (1.0 - t) * (1.0 - t); // ease-out quad
            let off = (ease * w as f32).round() as i32;
            // Forward: old slides left out, new enters from the right; back is the mirror.
            let (dx_src, dx_dst) = if forward { (-off, w - off) } else { (off, off - w) };
            for p in frame.iter_mut() {
                *p = 0;
            }
            blit_shift(&mut frame, w, h, &src, dx_src);
            blit_shift(&mut frame, w, h, &dst, dx_dst);
            self.present_buf(&frame, w, h, self.x, self.y, 255);
            std::thread::sleep(std::time::Duration::from_millis(9));
        }

        // Commit the destination screen.
        self.view = to;
        self.elems = dst_elems;
        self.reset_hover();
        self.base = vec![0u8; n];
        self.buf = vec![0u8; n];
        self.render_base();
        self.compose();
        self.present(self.x, self.y, 255);
    }

    /// Render the current view's static layer into `self.base`. Slider fill/thumb/value and
    /// hover live in `compose`.
    fn render_base(&mut self) {
        let mut base = vec![0u8; (self.width * self.height * 4) as usize];
        self.render_page(&self.elems, &mut base, self.height);
        self.base = base;
    }

    /// Render an arbitrary element list into `out` (a `self.width` × `out_h` RGBA buffer):
    /// the panel background, then every element's static content. Pure over `self` so it can
    /// paint any screen at any height — used both for the live `base` and for the two frames
    /// composited during a slide transition. `out_h` also clips drawing to the buffer.
    fn render_page(&self, elems: &[LaidElem], out: &mut [u8], out_h: i32) {
        let scale = self.scale;
        let accent = self.accent;
        let w = self.width;
        let h = out_h;
        let d = |v: f32| (v * scale).round() as i32;
        let groups = &self.groups;
        let buf = out;

        for p in buf.iter_mut() {
            *p = 0;
        }
        fill_round_rect(buf, w, h, 0.0, 0.0, w as f32, h as f32, d(CORNER) as f32, TINT, TINT_A);

        let font = ui_font();
        let font_sb = ui_font_semibold().or(font);
        let text_px = 14.0 * scale;
        let hdr_px = 14.0 * scale;
        let icon_px = (ICON_PX * scale).round() as u32;
        let mx = d(ROW_MARGIN) as f32;

        for le in elems {
            match le.elem {
                Elem::Header(text) => {
                    if let Some(f) = font_sb {
                        let base = le.top as f32 + le.height as f32 - hdr_px * 0.55;
                        draw_text(buf, w, h, f, hdr_px, d(HEADER_X) as f32, base, TEXT, 1.0, text);
                    }
                }
                Elem::Slider { group } => {
                    let g = &groups[group];
                    let cy_i = le.top + le.height / 2;
                    let cy = cy_i as f32;
                    let (glyph, col) = match (g.flow, g.muted) {
                        (Flow::Output, false) => (GLYPH_VOLUME, TEXT),
                        (Flow::Output, true) => (GLYPH_MUTE, accent),
                        (Flow::Input, false) => (GLYPH_MIC, TEXT),
                        (Flow::Input, true) => (GLYPH_MIC_OFF, accent),
                    };
                    if let Ok((rgba, gw, gh)) = icons::render_glyph(glyph, icon_px, col) {
                        blit(buf, w, h, d(ICON_X), cy_i - gh as i32 / 2, &rgba, gw, gh);
                    }
                    // Track background; the accent fill + thumb + value are drawn in compose.
                    let x0 = TRACK_X0 * scale;
                    let x1 = w as f32 - VALUE_W * scale;
                    let th = TRACK_H * scale;
                    fill_round_rect(buf, w, h, x0, cy - th / 2.0, x1, cy + th / 2.0, th / 2.0, TEXT, 0.28);
                }
                Elem::Device { group, dev } => {
                    let row = &groups[group].devices[dev];
                    let ry0 = le.top as f32 + 1.0;
                    let ry1 = (le.top + le.height) as f32 - 1.0;
                    if row.selected {
                        fill_round_rect(buf, w, h, mx, ry0, w as f32 - mx, ry1, d(ROW_RADIUS) as f32, TEXT, SEL_A);
                        let ph = d(PILL_H) as f32;
                        let pw = d(PILL_W) as f32;
                        let py0 = (ry0 + ry1) / 2.0 - ph / 2.0;
                        let px0 = mx + d(2.0) as f32;
                        fill_round_rect(buf, w, h, px0, py0, px0 + pw, py0 + ph, pw / 2.0, accent, 1.0);
                    }
                    let cy = le.top + le.height / 2;
                    if let Ok((rgba, gw, gh)) = row.icon.render(icon_px, TEXT) {
                        blit(buf, w, h, d(ICON_X), cy - gh as i32 / 2, &rgba, gw, gh);
                    }
                    if let Some(f) = font {
                        let base = cy as f32 + text_px * 0.34;
                        // Leave the trailing zone free — truncate a long name so it never
                        // runs under the battery readout (or the hover pencil).
                        let reserve = if row.battery.is_some() { BATTERY_W } else { PENCIL_W };
                        let max_w = w as f32 - d(TEXT_X) as f32 - reserve * scale;
                        let label = fit_label(f, text_px, &row.label, max_w);
                        draw_text(buf, w, h, f, text_px, d(TEXT_X) as f32, base, TEXT, 1.0, &label);
                    }
                    // The battery readout and the edit pencil both live on the right and are
                    // mutually exclusive (pencil on hover, battery otherwise) — drawn in
                    // `compose`, which knows the hover state.
                }
                Elem::PickerHeader { group, dev } => {
                    let cy_i = le.top + le.height / 2;
                    // The back chevron, centred in its button on the left.
                    let bpx = (BACK_GLYPH_PX * scale).round() as u32;
                    if let Ok((rgba, gw, gh)) = icons::render_glyph(GLYPH_BACK, bpx, TEXT) {
                        let bx = ((BACK_LEFT + BACK_BTN / 2.0) * scale).round() as i32 - gw as i32 / 2;
                        blit(buf, w, h, bx, cy_i - gh as i32 / 2, &rgba, gw, gh);
                    }
                    // The device name as the screen title.
                    if let Some(f) = font_sb {
                        let title_px = TITLE_PX * scale;
                        let base = cy_i as f32 + title_px * 0.34;
                        let max_w = w as f32 - d(TEXT_X) as f32 - RIGHT_PAD * scale;
                        let title = fit_label(f, title_px, &groups[group].devices[dev].label, max_w);
                        draw_text(buf, w, h, f, title_px, d(TEXT_X) as f32, base, TEXT, 1.0, &title);
                    }
                }
                Elem::IconGrid { group, dev } => {
                    let sel_icon = groups[group].devices[dev].icon;
                    let (cols, left, chip, step) = self.grid_metrics();
                    let gy0 = le.top + (GRID_TOP_PAD * scale).round() as i32;
                    let inner = (chip as f32 * GRID_ICON_RATIO).round() as u32;
                    let r = d(8.0) as f32;
                    for (idx, icon) in IconId::ALL.iter().enumerate() {
                        let cx0 = left + (idx as i32 % cols) * step;
                        let cy0 = gy0 + (idx as i32 / cols) * step;
                        let selected = *icon == sel_icon;
                        let (bg, a) = if selected { (accent, 1.0) } else { (TEXT, 0.06) };
                        fill_round_rect(buf, w, h, cx0 as f32, cy0 as f32, (cx0 + chip) as f32, (cy0 + chip) as f32, r, bg, a);
                        let gcol = if selected { DARK_GLYPH } else { TEXT };
                        if let Ok((rgba, gw, gh)) = icon.render(inner, gcol) {
                            let ox = cx0 + (chip - gw as i32) / 2;
                            let oy = cy0 + (chip - gh as i32) / 2;
                            blit(buf, w, h, ox, oy, &rgba, gw, gh);
                        }
                    }
                }
                Elem::Action(k) => {
                    let cy = le.top + le.height / 2;
                    if let Ok((rgba, gw, gh)) = icons::render_glyph(k.glyph(), icon_px, TEXT) {
                        blit(buf, w, h, d(ICON_X), cy - gh as i32 / 2, &rgba, gw, gh);
                    }
                    if let Some(f) = font {
                        let base = cy as f32 + text_px * 0.34;
                        draw_text(buf, w, h, f, text_px, d(TEXT_X) as f32, base, TEXT, 1.0, k.label());
                    }
                }
                Elem::UpdateBanner => {
                    let cy = le.top + le.height / 2;
                    // A subtle accent band marks it as a call-to-action.
                    let ry0 = le.top as f32 + 1.0;
                    let ry1 = (le.top + le.height) as f32 - 1.0;
                    fill_round_rect(buf, w, h, mx, ry0, w as f32 - mx, ry1, d(ROW_RADIUS) as f32, accent, 0.16);
                    if let Ok((rgba, gw, gh)) = icons::render_glyph(GLYPH_UPDATE, icon_px, accent) {
                        blit(buf, w, h, d(ICON_X), cy - gh as i32 / 2, &rgba, gw, gh);
                    }
                    if let (Some(f), Some(label)) = (font, self.update_label()) {
                        let base = cy as f32 + text_px * 0.34;
                        let maxw = w as f32 - d(TEXT_X) as f32 - RIGHT_PAD * scale;
                        let label = fit_label(f, text_px, &label, maxw);
                        draw_text(buf, w, h, f, text_px, d(TEXT_X) as f32, base, TEXT, 1.0, &label);
                    }
                }
            }
        }
    }

    /// Copy the static base, then draw the dynamic overlays: slider fill/thumb/value, the
    /// hover highlight, and the edit affordance on the hovered device row.
    fn compose(&mut self) {
        self.buf.copy_from_slice(&self.base);
        let scale = self.scale;
        let accent = self.accent;
        let (w, h) = (self.width, self.height);
        let panel_w = self.width;
        let d = |v: f32| (v * scale).round() as i32;
        let groups = &self.groups;
        let elems = &self.elems;
        let hover = self.hover;
        let hover_pencil = self.hover_pencil;
        let hover_back = self.hover_back;
        let hover_chip = self.hover_chip;
        let font = ui_font();
        let buf = self.buf.as_mut_slice();
        let mx = d(ROW_MARGIN) as f32;

        for le in elems {
            if let Elem::Slider { group } = le.elem {
                let g = &groups[group];
                let cy = le.top as f32 + le.height as f32 / 2.0;
                let x0 = TRACK_X0 * scale;
                let x1 = w as f32 - VALUE_W * scale;
                let level = g.level.clamp(0.0, 1.0);
                let fx = x0 + (x1 - x0) * level;
                let th = TRACK_H * scale;
                let tr = THUMB_R * scale;
                if g.muted {
                    // Muted: a flat, dim fill with no activity glow.
                    if fx > x0 {
                        fill_round_rect(buf, w, h, x0, cy - th / 2.0, fx, cy + th / 2.0, th / 2.0, TEXT, 0.34);
                    }
                    fill_round_rect(buf, w, h, fx - tr, cy - tr, fx + tr, cy + tr, tr, TEXT, 0.5);
                } else {
                    // The fill glows with the endpoint's live peak. The glow is purely
                    // *additive*: at rest (p≈0) it's a normal full-accent slider — matching a
                    // non-metered slider — and as audio rises it lightens toward white and
                    // grows a soft bloom halo, with a pulsing halo around the thumb. `powf`
                    // lifts low/mid levels (speech/music rarely peaks near 1.0) so it reads.
                    let p = g.peak.clamp(0.0, 1.0).powf(0.55);
                    // Outer bloom — the main "glow": a soft lightened-accent halo that grows
                    // tall and more opaque with the level (drawn under the fill so it reads
                    // as a halo above/below the track).
                    if p > 0.01 && fx > x0 {
                        let bloom = lerp3(accent, TEXT, 0.35);
                        let bh = th * (1.5 + 5.0 * p);
                        fill_round_rect(buf, w, h, x0, cy - bh / 2.0, fx, cy + bh / 2.0, bh / 2.0, bloom, 0.30 * p);
                    }
                    // Base fill: full accent, lightening toward white as it glows.
                    let fill_col = lerp3(accent, TEXT, 0.5 * p);
                    if fx > x0 {
                        fill_round_rect(buf, w, h, x0, cy - th / 2.0, fx, cy + th / 2.0, th / 2.0, fill_col, 1.0);
                    }
                    // Thumb: matching glow plus a soft pulsing halo.
                    if p > 0.01 {
                        let halo = lerp3(accent, TEXT, 0.4);
                        let hr = tr * (1.4 + 1.3 * p);
                        fill_round_rect(buf, w, h, fx - hr, cy - hr, fx + hr, cy + hr, hr, halo, 0.32 * p);
                    }
                    fill_round_rect(buf, w, h, fx - tr, cy - tr, fx + tr, cy + tr, tr, fill_col, 1.0);
                }
                if let Some(f) = font {
                    let vpx = 13.0 * scale;
                    let s = (level * 100.0).round().to_string();
                    let tw = measure(f, vpx, &s);
                    let vx = w as f32 - RIGHT_PAD * scale - tw;
                    let val_a = if g.muted { 0.5 } else { 1.0 };
                    draw_text(buf, w, h, f, vpx, vx, cy + vpx * 0.34, TEXT, val_a, &s);
                }
            }
        }

        // Right-hand affordances + hover highlights. On a device row the battery readout
        // and the edit pencil are mutually exclusive: pencil on the hovered row, battery
        // otherwise (so the current device still shows its battery when not hovered).
        for (idx, le) in elems.iter().enumerate() {
            let hovered = hover == Some(idx);
            match le.elem {
                Elem::Device { group, dev } => {
                    let dev_row = &groups[group].devices[dev];
                    let cy = (le.top + le.height / 2) as f32;
                    if hovered {
                        // Whole-row highlight (selected row already carries one in base).
                        if !dev_row.selected {
                            let ry0 = le.top as f32 + 1.0;
                            let ry1 = (le.top + le.height) as f32 - 1.0;
                            fill_round_rect(buf, w, h, mx, ry0, w as f32 - mx, ry1, d(ROW_RADIUS) as f32, TEXT, HOVER_A);
                        }
                        // The battery stays visible but shifts left so the pencil can sit to
                        // its right (rather than replacing it).
                        if let Some(pct) = dev_row.battery {
                            let pencil_left = pencil_center_x(panel_w, scale) - PENCIL_BTN * scale / 2.0;
                            draw_battery(buf, w, h, scale, pencil_left - 6.0 * scale, cy, pct, font);
                        }
                        // Round button behind the pencil, only when the pencil is hovered.
                        if hover_pencil {
                            let r = PENCIL_BTN * scale / 2.0;
                            let cxp = pencil_center_x(panel_w, scale);
                            fill_round_rect(buf, w, h, cxp - r, cy - r, cxp + r, cy + r, r, TEXT, 0.10);
                        }
                        let a = if hover_pencil { 1.0 } else { 0.85 };
                        draw_pencil(buf, w, h, scale, panel_w, cy, a);
                    } else if let Some(pct) = dev_row.battery {
                        draw_battery(buf, w, h, scale, panel_w as f32 - RIGHT_PAD * scale, cy, pct, font);
                    }
                }
                Elem::PickerHeader { .. } => {
                    // Round hover button behind the back arrow.
                    if hovered && hover_back {
                        let cy = (le.top + le.height / 2) as f32;
                        let cxb = (BACK_LEFT + BACK_BTN / 2.0) * scale;
                        let r = BACK_BTN * scale / 2.0;
                        fill_round_rect(buf, w, h, cxb - r, cy - r, cxb + r, cy + r, r, TEXT, 0.10);
                    }
                }
                Elem::IconGrid { group, dev } => {
                    if hovered {
                        if let Some(ci) = hover_chip {
                            let (cols, left, chip, step) = grid_metrics(w, scale);
                            let gy0 = le.top + (GRID_TOP_PAD * scale).round() as i32;
                            let cx0 = left + (ci as i32 % cols) * step;
                            let cy0 = gy0 + (ci as i32 / cols) * step;
                            let r = d(8.0) as f32;
                            // Brighten the hovered chip (accent chips lighten a touch too).
                            let selected = IconId::ALL[ci] == groups[group].devices[dev].icon;
                            let a = if selected { 0.14 } else { 0.10 };
                            fill_round_rect(buf, w, h, cx0 as f32, cy0 as f32, (cx0 + chip) as f32, (cy0 + chip) as f32, r, TEXT, a);
                        }
                    }
                }
                Elem::Action(_) => {
                    if hovered {
                        let ry0 = le.top as f32 + 1.0;
                        let ry1 = (le.top + le.height) as f32 - 1.0;
                        fill_round_rect(buf, w, h, mx, ry0, w as f32 - mx, ry1, d(ROW_RADIUS) as f32, TEXT, HOVER_A);
                    }
                }
                Elem::UpdateBanner => {
                    if hovered {
                        // Deepen the accent band on hover.
                        let ry0 = le.top as f32 + 1.0;
                        let ry1 = (le.top + le.height) as f32 - 1.0;
                        fill_round_rect(buf, w, h, mx, ry0, w as f32 - mx, ry1, d(ROW_RADIUS) as f32, accent, 0.14);
                    }
                }
                _ => {}
            }
        }
    }

    fn create_window(&mut self) -> windows::core::Result<()> {
        static REGISTERED: OnceLock<()> = OnceLock::new();
        let hinstance = HINSTANCE(unsafe { GetModuleHandleW(None) }?.0);
        REGISTERED.get_or_init(|| {
            let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance,
                hCursor: cursor,
                lpszClassName: w!("AudioTrayFlyout"),
                ..Default::default()
            };
            unsafe { RegisterClassW(&wc) };
        });

        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                w!("AudioTrayFlyout"),
                w!("Audio"),
                WS_POPUP,
                self.x,
                self.y,
                self.width,
                self.height,
                None,
                None,
                Some(hinstance),
                None,
            )
        }?;
        self.hwnd = hwnd;

        let dark: i32 = 1;
        let _ = unsafe {
            DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const std::ffi::c_void, 4)
        };
        let round: i32 = 2; // DWMWCP_ROUND
        let _ = unsafe {
            DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const std::ffi::c_void, 4)
        };
        unsafe { enable_acrylic(hwnd) };
        Ok(())
    }

    /// Slide up + fade in, like the native tray flyouts. Runs before the modal loop.
    fn animate_in(&self) {
        let slide = (14.0 * self.scale) as i32;
        let frames = 9;
        for i in 1..=frames {
            let t = i as f32 / frames as f32;
            let ease = 1.0 - (1.0 - t) * (1.0 - t); // ease-out quad
            let yy = self.y + (slide as f32 * (1.0 - ease)) as i32;
            self.present(self.x, yy, (255.0 * ease) as u8);
            std::thread::sleep(std::time::Duration::from_millis(9));
        }
        self.present(self.x, self.y, 255);
    }

    /// Push `self.buf` (the current screen) to the layered window. Thin wrapper over
    /// [`present_buf`](Self::present_buf).
    fn present(&self, x: i32, y: i32, alpha: u8) {
        self.present_buf(&self.buf, self.width, self.height, x, y, alpha);
    }

    /// Push a rendered ARGB buffer (`w`×`h`) to the layered window (premultiplied BGRA),
    /// scaled by a global `alpha` (for fade animations). `UpdateLayeredWindow` also moves +
    /// resizes the window to `(x, y)` and `(w, h)`.
    fn present_buf(&self, src_buf: &[u8], w: i32, h: i32, x: i32, y: i32, alpha: u8) {
        unsafe {
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));

            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let hbm = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0);
            let Ok(hbm) = hbm else {
                let _ = DeleteDC(mem);
                ReleaseDC(None, screen);
                return;
            };

            // straight-alpha RGBA -> premultiplied BGRA
            let px = (w * h) as usize;
            let dst = std::slice::from_raw_parts_mut(bits as *mut u8, px * 4);
            for i in 0..px {
                let r = src_buf[i * 4] as u32;
                let g = src_buf[i * 4 + 1] as u32;
                let b = src_buf[i * 4 + 2] as u32;
                let a = src_buf[i * 4 + 3] as u32;
                dst[i * 4] = ((b * a) / 255) as u8;
                dst[i * 4 + 1] = ((g * a) / 255) as u8;
                dst[i * 4 + 2] = ((r * a) / 255) as u8;
                dst[i * 4 + 3] = a as u8;
            }

            let old = SelectObject(mem, HGDIOBJ(hbm.0));
            let mut src = POINT { x: 0, y: 0 };
            let mut dpos = POINT { x, y };
            let mut size = SIZE { cx: w, cy: h };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: alpha,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                self.hwnd,
                Some(screen),
                Some(&mut dpos),
                Some(&mut size),
                Some(mem),
                Some(&mut src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );

            SelectObject(mem, old);
            let _ = DeleteObject(HGDIOBJ(hbm.0));
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // WM_CAPTURECHANGED is *sent* straight to the proc (it never reaches the modal loop's
    // GetMessage), so losing capture — Start menu, Alt-Tab, another app grabbing focus —
    // would otherwise orphan the flyout. Re-post it as a queued message the loop dismisses
    // on. (Our own ReleaseCapture at teardown also lands here, harmlessly.)
    if msg == WM_CAPTURECHANGED {
        let _ = PostMessageW(Some(hwnd), WM_FLYOUT_CLOSE, WPARAM(0), LPARAM(0));
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// Icon-grid geometry for a given panel `width`: `(cols, left_px, chip_px, step_px)`. The
/// grid wraps to as many equal columns as fit the width and is centred within the panel, so
/// the icons wrap onto multiple rows without ever widening the flyout.
fn grid_metrics(width: i32, scale: f32) -> (i32, i32, i32, i32) {
    let chip = (GRID_CHIP * scale).round() as i32;
    let gap = (GRID_GAP * scale).round() as i32;
    let step = chip + gap;
    let n = IconId::ALL.len() as i32;
    let avail = width - ((GRID_X + RIGHT_PAD) * scale).round() as i32;
    let cols = (((avail + gap) / step).max(1)).min(n);
    let grid_w = cols * chip + (cols - 1) * gap;
    let left = (width - grid_w) / 2;
    (cols, left, chip, step)
}

fn mouse_xy(lp: LPARAM) -> (i32, i32) {
    let x = (lp.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

/// Play the Windows "Default Beep" — the same ding Windows itself plays on a volume
/// change (the `SystemDefault` sound). Async + best-effort; it honours the user's sound
/// scheme and plays at the current level, so it doubles as audible volume feedback.
fn beep_volume() {
    use windows::Win32::System::Diagnostics::Debug::MessageBeep;
    use windows::Win32::UI::WindowsAndMessaging::MB_OK;
    unsafe {
        let _ = MessageBeep(MB_OK);
    }
}

/// Open Settings ▸ System ▸ Sound (the native page the right-click menu offers).
fn open_sound_settings() {
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            w!("ms-settings:sound"),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// Horizontal centre (in px) of the edit pencil's button — shared by hit-testing, the
/// hover highlight, and the glyph so they always coincide.
fn pencil_center_x(panel_w: i32, scale: f32) -> f32 {
    panel_w as f32 - (PENCIL_RIGHT + PENCIL_BTN / 2.0) * scale
}

/// Draw the trailing "edit icon" pencil on a device row, centred on its button.
fn draw_pencil(buf: &mut [u8], w: i32, h: i32, scale: f32, panel_w: i32, cy: f32, alpha: f32) {
    let size = (16.0 * scale).round() as u32;
    if let Ok((rgba, gw, gh)) = icons::render_glyph(GLYPH_EDIT, size, TEXT) {
        let cx = pencil_center_x(panel_w, scale);
        let x = (cx - gw as f32 / 2.0).round() as i32;
        let y = (cy - gh as f32 / 2.0).round() as i32;
        blit_a(buf, w, h, x, y, &rgba, gw, gh, alpha);
    }
}

/// Draw the battery readout (a level glyph + "NN%") ending at right edge `right` (px).
fn draw_battery(buf: &mut [u8], w: i32, h: i32, scale: f32, right: f32, cy: f32, pct: u8, font: Option<&FontVec>) {
    const DIM: f32 = 0.9;
    let pct = pct.min(100);
    let text = format!("{pct}%");
    let px = 12.5 * scale;
    let tw = font.map(|f| measure(f, px, &text)).unwrap_or(0.0);
    if let Some(f) = font {
        draw_text(buf, w, h, f, px, right - tw, cy + px * 0.34, TEXT, DIM, &text);
    }
    // Segoe Fluent battery levels: Battery0 (E850, empty) … Battery10 (E85A, full).
    let level = ((pct as f32 / 10.0).round() as u32).min(10);
    let glyph = char::from_u32(0xE850 + level).unwrap_or('\u{E850}');
    let size = (20.0 * scale).round() as u32;
    let gap = 5.0 * scale;
    if let Ok((rgba, gw, gh)) = icons::render_glyph(glyph, size, TEXT) {
        let x = (right - tw - gap - gw as f32).round() as i32;
        let y = (cy - gh as f32 / 2.0).round() as i32;
        blit_a(buf, w, h, x, y, &rgba, gw, gh, DIM);
    }
}

// --- acrylic + accent -------------------------------------------------------

/// Enable the acrylic blur-behind via the undocumented (but ubiquitous)
/// `SetWindowCompositionAttribute`. Best-effort — if it no-ops, the panel is still a
/// legible semi-transparent dark surface.
unsafe fn enable_acrylic(hwnd: HWND) {
    #[repr(C)]
    struct AccentPolicy {
        accent_state: u32,
        accent_flags: u32,
        gradient_color: u32,
        animation_id: u32,
    }
    #[repr(C)]
    struct WindowCompositionAttributeData {
        attrib: u32,
        pv_data: *mut std::ffi::c_void,
        cb_data: usize,
    }
    type SetWca = unsafe extern "system" fn(HWND, *mut WindowCompositionAttributeData) -> i32;

    let Ok(user32) = GetModuleHandleW(w!("user32.dll")) else {
        return;
    };
    let Some(p) = GetProcAddress(user32, s!("SetWindowCompositionAttribute")) else {
        return;
    };
    let set_wca: SetWca = std::mem::transmute(p);

    const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;
    const WCA_ACCENT_POLICY: u32 = 19;
    let mut policy = AccentPolicy {
        accent_state: ACCENT_ENABLE_ACRYLICBLURBEHIND,
        accent_flags: 0,
        gradient_color: 0x0020_2020,
        animation_id: 0,
    };
    let mut data = WindowCompositionAttributeData {
        attrib: WCA_ACCENT_POLICY,
        pv_data: &mut policy as *mut _ as *mut std::ffi::c_void,
        cb_data: std::mem::size_of::<AccentPolicy>(),
    };
    set_wca(hwnd, &mut data);
}

/// The user's Windows accent colour (registry `DWM\AccentColor`, stored `AABBGGRR`).
/// The accent colour to paint (selection pill, slider fill/thumb). On our dark surface
/// Windows uses the *Light2* shade of the accent palette rather than the base accent —
/// matching that keeps us in step with the native flyout. Falls back to the DWM base
/// accent, then the Win11 default.
fn accent_rgb() -> [u8; 3] {
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

// --- software rendering -----------------------------------------------------

fn ui_font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        let bytes = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf").ok()?;
        FontVec::try_from_vec(bytes).ok()
    })
    .as_ref()
}

/// Segoe UI Semibold — the weight Windows uses for the flyout's section captions
/// ("BodyStrong"). Falls back to the regular UI font at the call site if absent.
fn ui_font_semibold() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        let bytes = std::fs::read(r"C:\Windows\Fonts\seguisb.ttf").ok()?;
        FontVec::try_from_vec(bytes).ok()
    })
    .as_ref()
}

/// Convert a desired **em size** (in px) into the ab_glyph `PxScale` that actually yields
/// it. ab_glyph scales a font by its *height*, so a plain `PxScale::from(px)` renders an
/// em of only ~0.75·px for Segoe UI. Sizing by em keeps our text matched to Windows.
fn em_scale(font: &FontVec, em_px: f32) -> PxScale {
    match font.units_per_em() {
        Some(upem) => PxScale::from(em_px * font.height_unscaled() / upem),
        None => PxScale::from(em_px),
    }
}

fn measure(font: &FontVec, px: f32, text: &str) -> f32 {
    let sf = font.as_scaled(em_scale(font, px));
    text.chars().map(|c| sf.h_advance(font.glyph_id(c))).sum()
}

/// Truncate `text` with a trailing ellipsis so it fits within `max_w` px. Returned as-is
/// when it already fits.
fn fit_label(font: &FontVec, px: f32, text: &str, max_w: f32) -> String {
    let sf = font.as_scaled(em_scale(font, px));
    let advance = |c: char| sf.h_advance(font.glyph_id(c));
    if text.chars().map(advance).sum::<f32>() <= max_w {
        return text.to_string();
    }
    let budget = max_w - advance('…');
    let mut out = String::new();
    let mut acc = 0.0;
    for ch in text.chars() {
        let cw = advance(ch);
        if acc + cw > budget {
            break;
        }
        acc += cw;
        out.push(ch);
    }
    out.push('…');
    out
}

#[allow(clippy::too_many_arguments)]
fn draw_text(buf: &mut [u8], w: i32, h: i32, font: &FontVec, px: f32, mut pen: f32, baseline: f32, col: [u8; 3], alpha: f32, text: &str) {
    let scale = em_scale(font, px);
    let sf = font.as_scaled(scale);
    for ch in text.chars() {
        let gid = font.glyph_id(ch);
        let glyph = ab_glyph::Glyph { id: gid, scale, position: ab_glyph::point(pen, baseline) };
        if let Some(outline) = font.outline_glyph(glyph) {
            let bb = outline.px_bounds();
            outline.draw(|gx, gy, cov| {
                let x = bb.min.x as i32 + gx as i32;
                let y = bb.min.y as i32 + gy as i32;
                blend(buf, w, h, x, y, col, cov * alpha);
            });
        }
        pen += sf.h_advance(gid);
    }
}

/// Copy a `w`×`h` page buffer into `frame` (also `w`×`h`) shifted horizontally by `dx`
/// (opaque copy, no blending), clipping to the frame. Used to slide two pre-rendered
/// screens across each other during a navigation transition.
fn blit_shift(frame: &mut [u8], w: i32, h: i32, page: &[u8], dx: i32) {
    let x_lo = dx.max(0);
    let x_hi = (w + dx).min(w);
    if x_lo >= x_hi {
        return;
    }
    for y in 0..h {
        let row = (y * w) as usize * 4;
        for x in x_lo..x_hi {
            let di = row + x as usize * 4;
            let si = row + (x - dx) as usize * 4;
            frame[di..di + 4].copy_from_slice(&page[si..si + 4]);
        }
    }
}

/// Blit a straight-alpha RGBA sprite (its own colour) onto the buffer.
fn blit(buf: &mut [u8], w: i32, h: i32, x0: i32, y0: i32, rgba: &[u8], sw: u32, sh: u32) {
    blit_a(buf, w, h, x0, y0, rgba, sw, sh, 1.0);
}

/// Blit a straight-alpha RGBA sprite, scaling its alpha by `alpha` (for dimmed glyphs).
#[allow(clippy::too_many_arguments)]
fn blit_a(buf: &mut [u8], w: i32, h: i32, x0: i32, y0: i32, rgba: &[u8], sw: u32, sh: u32, alpha: f32) {
    for sy in 0..sh as i32 {
        for sx in 0..sw as i32 {
            let i = ((sy as u32 * sw + sx as u32) * 4) as usize;
            let a = rgba[i + 3] as f32 / 255.0 * alpha;
            if a <= 0.0 {
                continue;
            }
            blend(buf, w, h, x0 + sx, y0 + sy, [rgba[i], rgba[i + 1], rgba[i + 2]], a);
        }
    }
}


#[allow(clippy::too_many_arguments)]
fn fill_round_rect(buf: &mut [u8], w: i32, h: i32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32, col: [u8; 3], alpha: f32) {
    let cx = (x0 + x1) / 2.0;
    let cy = (y0 + y1) / 2.0;
    let hx = (x1 - x0) / 2.0;
    let hy = (y1 - y0) / 2.0;
    let r = r.min(hx).min(hy);
    for y in y0.floor() as i32..y1.ceil() as i32 {
        for x in x0.floor() as i32..x1.ceil() as i32 {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let qx = (px - cx).abs() - (hx - r);
            let qy = (py - cy).abs() - (hy - r);
            let outside = qx.max(0.0).hypot(qy.max(0.0));
            let inside = qx.max(qy).min(0.0);
            let sd = outside + inside - r;
            let cov = (0.5 - sd).clamp(0.0, 1.0);
            if cov > 0.0 {
                blend(buf, w, h, x, y, col, alpha * cov);
            }
        }
    }
}

/// Linear interpolation between two RGB colours (`t` clamped to 0..=1). Used to lighten
/// the slider fill toward white as live audio activity rises.
fn lerp3(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    [l(a[0], b[0]), l(a[1], b[1]), l(a[2], b[2])]
}

/// Source-over blend of a straight-alpha colour into the straight-alpha RGBA buffer.
fn blend(buf: &mut [u8], w: i32, h: i32, x: i32, y: i32, col: [u8; 3], a: f32) {
    if x < 0 || y < 0 || x >= w || y >= h {
        return;
    }
    let sa = a.clamp(0.0, 1.0);
    if sa <= 0.0 {
        return;
    }
    let idx = ((y * w + x) * 4) as usize;
    let da = buf[idx + 3] as f32 / 255.0;
    let out_a = sa + da * (1.0 - sa);
    if out_a <= 0.0 {
        return;
    }
    for c in 0..3 {
        let s = col[c] as f32 / 255.0;
        let d = buf[idx + c] as f32 / 255.0;
        let o = (s * sa + d * da * (1.0 - sa)) / out_a;
        buf[idx + c] = (o * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    buf[idx + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
}

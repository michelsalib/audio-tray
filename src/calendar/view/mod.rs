//! The panel: one card over the clock's corner ([`scene`]), the calendar (the clock's) or the
//! notification centre (the bell's), as [`Panel`] says. The card is a per-pixel-alpha layered
//! window over Windows' acrylic, drawn off screen with Direct2D ([`paint`]) and presented whole on
//! every change (no `WM_PAINT`). No capture: the tray's mouse hook closes it on a click elsewhere,
//! and that click still lands where it was aimed.
//!
//! It opens from whichever screen edge the taskbar is on, slides in like the shell's own flyouts,
//! and closes on a click elsewhere, Escape, or losing the foreground. Asked for the other card
//! while open, it becomes that card where it stands.
//!
//! **Re-entrancy.** Opening a link or Settings can pump messages: that work runs after the view's
//! state is released ([`After`]).

pub(super) mod banner;
mod drafts;
mod editor;
mod locale;
mod paint;
mod scene;
mod batteries;
pub(crate) mod words;
mod badge;
mod describe;
mod render;
pub use badge::{badge, battery_dot, focus_pie, Badge};
use describe::*;
use render::*;

pub use batteries::{power_changed, set_batteries};

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, ScreenToClient, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VIRTUAL_KEY, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_HOME, VK_LEFT, VK_NEXT, VK_PRIOR, VK_RIGHT, VK_SHIFT,
    VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, GetCursorPos, ShowWindow, SW_SHOWNA, WA_INACTIVE, WHEEL_DELTA, WINDOW_EX_STYLE, WM_ACTIVATE,
    WM_CHAR, WM_DESTROY, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_TIMER, KillTimer, SetTimer, WindowFromPoint, GetAncestor, GA_ROOT,
    WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};

use super::model::{self, Event, Source, When};
use super::notifications::Toast;
use super::rich::{self, Kind};
use super::time::{self, Local, Zone, DAY};
use super::zones::WorldClock;
use super::{lock, Setup};
use crate::config::Config;
use paint::{Color, Gfx, Palette, Pen};
pub use scene::Panel;
use scene::{Draw, Fill, Hit, Ink, Scene, R};

const CLASS: PCWSTR = w!("AudioTrayCalendar");
/// In `Win32_UI_Controls`, which this crate does not otherwise need.
const WM_MOUSELEAVE: u32 = 0x02A3;
/// A click on the clock right after the panel closed is the click that closed it.
const SETTLE: Duration = Duration::from_millis(400);
/// The gap between the panel and the screen's edges, as the shell's flyouts keep.
const MARGIN: f32 = 12.0;
/// How far a wheel notch scrolls a list.
const SCROLL_STEP: f32 = 48.0;
/// How long the pointer rests on a target before its tooltip shows.
const TIP_TIMER: usize = 1;
const TIP_DELAY_MS: u32 = 500;
/// A word said for a moment ([`View::say`]), and how long.
const FLASH_TIMER: usize = 2;
const FLASH_FOR: std::time::Duration = std::time::Duration::from_secs(4);
/// While open, the panel looks again now and then: "in 2 h 19", the focus time left, and Do not
/// disturb switched elsewhere follow.
const REFRESH_TIMER: usize = 3;
const REFRESH_MS: u32 = 30_000;
/// A frame once input has settled ([`View::later`]).
const FRAME_TIMER: usize = 4;
/// The frames of a notification collapsing away and of the card easing to its height.
const ANIM_TIMER: usize = 5;
const ANIM_MS: u32 = 16;
/// How long a dismissed notification takes to collapse.
const COLLAPSE: Duration = Duration::from_millis(200);
/// Each frame, the card's height covers this much of what is left to its own.
const EASE: f32 = 0.35;
/// How far ahead the next event is looked for.
const AHEAD: i64 = 7 * DAY;

thread_local! {
    static VIEW: RefCell<Option<View>> = const { RefCell::new(None) };
    /// The drawing kit of the last panel, kept for the next (its pictures stay decoded).
    static SPARE_GFX: RefCell<Option<Gfx>> = const { RefCell::new(None) };
    static FLAGS: Flags = const {
        Flags { open: Cell::new(0), closed_at: Cell::new(None), closing: Cell::new(false), held: Cell::new(false), raising: Cell::new(false) }
    };
}

/// Where the panel's window stands. Cells, each read or set alone: a message can come in the
/// middle of any step (closing animates, raising pumps), and nothing is held borrowed across one.
struct Flags {
    /// The open panel's window (0: none).
    open: Cell<isize>,
    /// When it last closed.
    closed_at: Cell<Option<Instant>>,
    /// The close animation is running.
    closing: Cell<bool>,
    /// The panel really has the foreground (`GetForegroundWindow` says so), so losing it is the
    /// user going elsewhere. Without rights, `raise` borrows the foreground app's input queue: the
    /// panel looks active while attached and is deactivated on detaching, with that app still in
    /// front. Closing on that deactivation shut the panel as it opened.
    held: Cell<bool>,
    /// [`create`] is in `raise`: activations meanwhile are borrowed, not held.
    raising: Cell<bool>,
}

fn flags<R>(f: impl FnOnce(&Flags) -> R) -> R {
    FLAGS.with(f)
}

/// Whether the panel is open.
pub(super) fn is_open() -> bool {
    flags(|f| f.open.get() != 0)
}

/// Whether `hwnd` is the open panel's window (not one drawn ahead, [`prewarm`]).
fn is_open_panel(hwnd: HWND) -> bool {
    !hwnd.is_invalid() && flags(|f| f.open.get()) == hwnd.0 as isize
}

/// A drawing kit set aside for the next panel: its surface and decoded pictures kept (why it is
/// kept), its last frame's pixels let go.
fn spare(mut gfx: Gfx) -> Gfx {
    gfx.forget_frame();
    gfx
}

/// Whether `hwnd` is the foreground window, as the whole system sees it.
fn foreground_is(hwnd: HWND) -> bool {
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() == hwnd }
}

/// The screen edge the taskbar is on, where the panel opens from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Bottom,
    Top,
    Left,
    Right,
}

/// A frame drawn: its size in pixels, its card, and the drawing.
type Drawn = ((u32, u32), R, Vec<(R, Draw)>);

/// What the card's window shows: its pixels (held in its bitmap), where, and how faded.
struct Sent {
    canvas: crate::layered::Canvas,
    at: (i32, i32),
    alpha: u8,
}

impl Sent {
    const fn new() -> Self {
        Sent { canvas: crate::layered::Canvas::new(), at: (0, 0), alpha: 0 }
    }
}

struct View {
    /// The card's window.
    hwnd: HWND,
    /// The calendar or the notifications.
    panel: Panel,
    gfx: RefCell<Gfx>,
    /// What the window last showed, to skip sending the same again.
    shown: RefCell<Sent>,
    /// The last frame drawn: its size, its card and its drawing, to redraw only what changed.
    last: RefCell<Option<Drawn>>,
    /// Windows keeps notifications quiet (asked of Windows on opening, each refresh and after a
    /// switch: never per frame, each asking costs a WinRT call).
    quiet: bool,
    palette: Palette,
    scale: f32,
    edge: Edge,
    /// The work area it sits in, in pixels.
    work: RECT,
    scene: Scene,
    /// The scene's top-left on screen, in pixels.
    origin: (i32, i32),
    config: Config,
    setup: Setup,
    has_sources: bool,
    today: i32,
    month: (i32, u32),
    selected: i32,
    first_weekday: u32,
    /// Short day names, Monday first.
    weekdays: [String; 7],
    /// The shown calendars' events, in their chosen colours.
    events: Vec<Event>,
    /// When the world clocks were read: their times move on from there while the panel is open.
    clocks_at: i64,
    /// Each event's first and last local day, worked out once per reload (a time-zone sum each:
    /// asked for every day of the month on every frame, they cost).
    spans: Vec<(i32, i32)>,
    /// What the hero and the list show, to open on a click.
    hero: Option<Event>,
    rows: Vec<Event>,
    detail: Option<Event>,
    toasts: Vec<Toast>,
    /// Notifications dismissed and collapsing away (still in `toasts` meanwhile), and since when.
    leaving: Vec<(u32, Instant)>,
    /// The notification card's height while it eases to its own (`None`: at its own).
    shown_h: Option<f32>,
    /// The card the last scene was built for: easing is between two heights of the same card.
    scene_panel: Panel,
    /// The notification whose reply box is open, and what is typed in it (the others' drafts are
    /// kept in [`drafts`]).
    draft: Option<(u32, editor::Editor)>,
    /// The notification whose menu is open.
    menu_for: Option<u32>,
    /// Where the pointer rested on what it hovers (scene point): its tooltip shows there.
    show_tip: Option<(f32, f32)>,
    clocks: Vec<WorldClock>,
    center_scroll: f32,
    list_scroll: f32,
    hover: Option<Hit>,
    pressed: Option<Hit>,
    /// The keyboard's target (Tab), ringed.
    focus: Option<Hit>,
    /// A word for a moment (Windows refused something), and since when.
    flash: Option<(String, Instant)>,
    /// After a keyboard action, the Tab order's place the ring goes back to (the next target).
    refocus: Option<usize>,
    /// This PC's battery, read on each reload (the batteries' card).
    pc: Option<crate::batteries::SystemBattery>,
}

/// Work to do once the view's state is released: it may pump messages.
type After = Box<dyn FnOnce()>;

/// The list under the month, as [`View::agenda`] has it.
struct Agenda {
    hero: Option<scene::Hero>,
    list_title: String,
    rows: Vec<scene::Row>,
    /// What the list says with nothing in it.
    empty: String,
}

/// Open `panel`, close it if it is open, or turn the open panel into it.
pub fn toggle(config: &Config, panel: Panel) {
    if is_open() {
        if with_view(|v| v.panel == panel).unwrap_or(true) {
            println!("calendar: closed by a click on its button");
            close();
        } else {
            println!("calendar: now the {panel:?}");
            with_view(|v| v.switch(panel));
        }
        return;
    }
    if flags(|f| f.closed_at.get()).is_some_and(|at| at.elapsed() < SETTLE) {
        return;
    }
    println!("calendar: opening the {panel:?}");
    if let Err(e) = create(config, panel) {
        eprintln!("calendar: could not open ({e:#})");
    }
}

/// Close the open panel, if any (its closing animation first).
pub fn close() {
    let open = flags(|f| f.open.get());
    if open == 0 || flags(|f| f.closing.replace(true)) {
        return;
    }
    with_view(|v| v.animate(crate::win::Flyout::Close));
    let _ = unsafe { DestroyWindow(HWND(open as *mut _)) };
    flags(|f| f.closing.set(false));
}

/// The events or the notifications changed: show them.
pub fn refresh() {
    with_view(|v| {
        v.reload();
        if let Some((id, draft)) = super::notifications::take_failure() {
            // An action the app did not take: its notification is back, with what was typed.
            if let Some(text) = draft.filter(|t| !t.is_empty()) {
                v.draft = Some((id, editor::Editor::new(text)));
            }
            v.say(words::words().app_did_not_take);
        }
        v.relayout();
    });
}

/// Do not disturb was switched: the moon follows (asked of Windows once, here).
pub fn quiet_changed() {
    with_view(|v| {
        v.quiet = super::focus::active();
        v.relayout();
    });
}

fn with_view<R>(f: impl FnOnce(&mut View) -> R) -> Option<R> {
    VIEW.with(|cell| cell.try_borrow_mut().ok().and_then(|mut v| v.as_mut().map(f)))
}

/// The monitor under `point`: its scale, its rectangle and its work area.
fn monitor(point: POINT) -> (f32, RECT, RECT) {
    let (scale, work) = crate::win::monitor_at(point);
    let handle = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let rect = if unsafe { GetMonitorInfoW(handle, &mut info) }.as_bool() { info.rcMonitor } else { work };
    (scale, rect, work)
}

/// Which edge the taskbar takes from this monitor (the side its work area is short of).
fn edge_of(rect: &RECT, work: &RECT) -> Edge {
    if work.top > rect.top {
        Edge::Top
    } else if work.left > rect.left {
        Edge::Left
    } else if work.right < rect.right {
        Edge::Right
    } else {
        Edge::Bottom
    }
}

/// The card's window, hidden.
fn window() -> Result<HWND> {
    let style = WINDOW_EX_STYLE(WS_EX_LAYERED.0 | WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0);
    let hwnd = crate::win::create_popup(CLASS, w!("Calendar"), Some(wndproc), style, (0, 0, 1, 1)).context("CreateWindowExW")?;
    unsafe { crate::layered::style_panel(hwnd, crate::layered::CORNER_ROUND) };
    Ok(hwnd)
}

/// The panel made, its window hidden, nothing read yet; `windowless` (drawn ahead, never shown):
/// without a window, and without asking Windows about Do not disturb.
fn make(config: &Config, panel: Panel, windowless: bool) -> Result<View> {
    let mut cursor = POINT::default();
    let _ = unsafe { GetCursorPos(&mut cursor) };
    let (scale, rect, work) = monitor(cursor);
    // First what can fail without windows to clean up after. The last panel's, if any: its
    // pictures are decoded already.
    let gfx = match SPARE_GFX.with(|s| s.borrow_mut().take()) {
        Some(gfx) => gfx,
        None => Gfx::new()?,
    };
    let hwnd = if windowless { HWND::default() } else { window()? };
    let today = time::today();
    let (ty, tm, _) = time::civil_from_days(today);
    let setup = config.calendar.setup();
    let first_weekday = config.calendar.first_weekday.map_or_else(locale::first_weekday, |d| d.min(6) as u32);
    let empty = scene::build(&empty_input());
    let view = View {
        hwnd,
        panel,
        gfx: RefCell::new(gfx),
        shown: RefCell::new(Sent::new()),
        last: RefCell::new(None),
        quiet: !windowless && super::focus::active(),
        palette: Palette::current(),
        scale,
        edge: edge_of(&rect, &work),
        work,
        scene: empty,
        origin: (0, 0),
        has_sources: super::has_sources(&setup),
        setup,
        config: config.clone(),
        today,
        month: (ty, tm),
        selected: today,
        first_weekday,
        weekdays: locale::weekday_names(),
        events: Vec::new(),
        spans: Vec::new(),
        clocks_at: 0,
        hero: None,
        rows: Vec::new(),
        detail: None,
        toasts: Vec::new(),
        leaving: Vec::new(),
        shown_h: None,
        scene_panel: panel,
        draft: None,
        menu_for: None,
        clocks: Vec::new(),
        center_scroll: 0.0,
        list_scroll: 0.0,
        hover: None,
        show_tip: None,
        pressed: None,
        focus: None,
        refocus: None,
        flash: None,
        pc: None,
    };
    Ok(view)
}

/// Before an open: the panel drawn once, hidden, so that its pictures are decoded and its text laid
/// out when it opens (a first open drew everything from nothing, ~40 ms more). Tray thread.
pub fn prewarm(config: &Config) {
    thread_local! {
        /// The pictures and logos drawn ahead so far.
        static WARMED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }
    if is_open() {
        return;
    }
    // Only for pictures not drawn yet (or fonts never loaded): a notification gone, or one with
    // nothing new to decode, needs no frame.
    let keys: Vec<String> = super::notifications::list()
        .iter()
        .flat_map(|t| [Some(t.aumid.clone()), t.rich.avatar.as_ref().map(|p| p.key.clone()), t.rich.picture.as_ref().map(|p| p.key.clone())])
        .flatten()
        .collect();
    let warm = SPARE_GFX.with(|s| s.borrow().is_some());
    if warm && WARMED.with(|w| keys.iter().all(|k| w.borrow().contains(k))) {
        return;
    }
    // The notifications: theirs are the pictures to decode.
    let Ok(mut view) = make(config, Panel::Notifications, true) else { return };
    view.reload();
    view.place();
    view.draw_frame();
    SPARE_GFX.with(|s| *s.borrow_mut() = Some(spare(view.gfx.into_inner())));
    WARMED.with(|w| *w.borrow_mut() = keys);
}

fn create(config: &Config, panel: Panel) -> Result<()> {
    // The panel lists what the banners showed.
    banner::clear();
    let mut view = make(config, panel, false)?;
    let hwnd = view.hwnd;
    super::notifications::set_watching(true);
    view.reload();
    view.fetch();
    view.place();
    view.draw_frame();
    VIEW.with(|cell| *cell.borrow_mut() = Some(view));
    flags(|f| f.open.set(hwnd.0 as isize));
    with_view(|v| {
        unsafe {
            let _ = ShowWindow(v.hwnd, SW_SHOWNA);
        }
        v.animate(crate::win::Flyout::Open);
    });
    unsafe { SetTimer(Some(hwnd), REFRESH_TIMER, REFRESH_MS, None) };
    // The click that opened it was swallowed by our hook, so Windows has not lent us the
    // foreground: without it there are no keys (Escape) and no deactivation to close on.
    // Its deactivations are not the user's until it is known to be in front (see [`Flags::held`]).
    flags(|f| {
        f.held.set(false);
        f.raising.set(true);
    });
    let raised = crate::music::player::raise(hwnd);
    let held = foreground_is(hwnd);
    flags(|f| {
        f.raising.set(false);
        f.held.set(held);
    });
    if !held {
        eprintln!("calendar: could not take the foreground ({raised:?}); a click outside closes it");
    }
    // No mouse capture: a click elsewhere goes where it was aimed (the audio icon opens its panel
    // at once), and the tray's mouse hook closes the calendar as it passes ([`keep_rect`]).
    arrow();
    keep_rect();
    crate::taskbar::notify_panel(panel_code(panel));
    Ok(())
}

/// `panel` as the TAP knows it.
fn panel_code(panel: Panel) -> usize {
    match panel {
        Panel::Calendar => tap_proto::OPEN_CALENDAR,
        Panel::Notifications => tap_proto::OPEN_NOTIFICATIONS,
        Panel::Batteries => tap_proto::OPEN_BATTERIES,
    }
}

/// What the scene is built from before anything is read (never shown).
fn empty_input() -> scene::Input {
    scene::Input {
        panel: Panel::Calendar,
        room: 0.0,
        card_h: None,
        grid: scene::Grid::Comfortable,
        rows_max: 5,
        week_numbers: Vec::new(),
        notifications_denied: false,
        groups: Vec::new(),
        unread: 0,
        quiet: false,
        center_scroll: 0.0,
        weekday: String::new(),
        date: String::new(),
        clocks: String::new(),
        month_title: String::new(),
        heads: Default::default(),
        days: Vec::new(),
        hero: None,
        list_title: String::new(),
        rows: Vec::new(),
        empty: String::new(),
        list_scroll: 0.0,
        focus_label: String::new(),
        focus_left: None,
        detail: None,
        hover: None,
        show_tip: None,
        pc: None,
        devices: Vec::new(),
        light: false,
        notice: scene::Notice::None,
        focus: None,
        pressed: None,
        flash: None,
        caret: false,
    }
}

/// Tell the mouse hook where the panel's card is: a click outside it closes it.
fn keep_rect() {
    with_view(|v| v.keep_card());
}

impl View {
    fn px(&self, dip: f32) -> i32 {
        (dip * self.scale).round() as i32
    }

    /// Take the cache's events (shown calendars, chosen colours), Windows' notifications and the
    /// world clocks.
    fn reload(&mut self) {
        let calendar = &self.config.calendar;
        self.events = super::events()
            .0
            .into_iter()
            .filter(|e| !calendar.hidden.contains(&e.calendar))
            .map(|mut e| {
                if let Some(color) = calendar.colors.get(&e.calendar).and_then(|c| super::parse_hex(c)) {
                    e.color = color;
                }
                e
            })
            .collect();
        self.spans = self.events.iter().map(|e| e.days(&Local)).collect();
        let mut toasts = if calendar.bell { super::notifications::list() } else { Vec::new() };
        // Ones collapsing away are gone from Windows' list already: kept where they were until
        // they have.
        for (i, old) in self.toasts.iter().enumerate() {
            if self.leaving.iter().any(|(id, _)| *id == old.id) && !toasts.iter().any(|t| t.id == old.id) {
                toasts.insert(i.min(toasts.len()), old.clone());
            }
        }
        if toasts.iter().map(|t| t.id).ne(self.toasts.iter().map(|t| t.id)) {
            // Targets name a notification by its place, which has moved: a held press or the
            // keyboard's ring would act on another one.
            if self.pressed.is_some_and(|h| scene::toast_of(h).is_some()) {
                self.pressed = None;
            }
            if self.focus.is_some_and(|h| scene::toast_of(h).is_some() || matches!(h, Hit::Group(_) | Hit::DismissGroup(_))) {
                self.focus = None;
            }
        }
        self.toasts = toasts;
        self.clocks = if calendar.world_clocks { super::zones::clocks() } else { Vec::new() };
        self.clocks_at = time::now();
        self.pc = crate::batteries::system();
    }

    fn fetch(&self) {
        if self.has_sources {
            super::fetch(&self.setup, super::days_around(self.month.0, self.month.1), false);
        }
    }

    /// The first day of the week holding `day`.
    fn week_start(&self, day: i32) -> i32 {
        day - ((time::weekday(day) + 7 - self.first_weekday) % 7) as i32
    }

    /// What the scene shows now; keeps the hero's and the rows' events to open on a click.
    fn input(&mut self) -> scene::Input {
        let now = time::now();
        let today = self.today;
        let t = words::words();
        let groups = self.groups();
        // The month shown (without the grid, none).
        let days: Vec<i32> = if self.grid() == scene::Grid::None {
            Vec::new()
        } else {
            let start = self.week_start(time::days_from_civil(self.month.0, self.month.1, 1));
            (0..42).map(|i| start + i).collect()
        };
        let cells = self.month_cells(&days);
        let Agenda { hero: hero_view, list_title, rows: row_views, empty } = self.agenda(now);

        let focus = super::schedule::focus_end();
        let focus_label = match focus {
            Some(end) => (t.focus_stop)(((end - now) + 59) / 60),
            None => (t.focus)(self.config.calendar.focus_minutes),
        };
        let clocks = self
            .clocks
            .iter()
            .take(2)
            .map(|c| format!("{} {}", c.name, locale::clock(time::second_of_day(c.wall + now - self.clocks_at))))
            .collect::<Vec<_>>()
            .join(" · ");
        let detail = self.detail.as_ref().map(|e| detail_view(e, &self.setup, now, self.selected));
        let notice = if !self.has_sources {
            scene::Notice::NoSources
        } else if let Some(problem) = super::problem(&self.setup) {
            scene::Notice::Problem(problem)
        } else if self.events.is_empty() && super::fetching() && super::fetched_at().is_none() {
            scene::Notice::Loading
        } else {
            scene::Notice::None
        };

        let (pc, devices) = batteries::lines(self);
        scene::Input {
            panel: self.panel,
            room: (self.work.bottom - self.work.top) as f32 / self.scale - 2.0 * MARGIN,
            card_h: None,
            groups,
            unread: self.toasts.len(),
            quiet: self.quiet,
            center_scroll: self.center_scroll,
            weekday: capitalised(&locale::format(today, "dddd")),
            date: locale::format(today, "d MMMM"),
            clocks,
            month_title: capitalised(&locale::month_title(self.month.0, self.month.1)),
            heads: std::array::from_fn(|i| self.weekdays[(self.first_weekday as usize + i) % 7].clone()),
            days: cells,
            hero: hero_view,
            list_title,
            rows: row_views,
            empty,
            list_scroll: self.list_scroll,
            focus_label,
            focus_left: focus.map(|end| (((end - now) + 59) / 60).max(1)),
            detail,
            hover: self.hover,
            show_tip: self.show_tip,
            pc,
            devices,
            light: self.palette.bg.0 > 0.5,
            notice,
            focus: self.focus,
            pressed: self.pressed,
            flash: self.flash.as_ref().filter(|(_, at)| at.elapsed() < FLASH_FOR).map(|(text, _)| text.clone()),
            grid: self.grid(),
            rows_max: self.config.calendar.rows.max(1) as usize,
            week_numbers: if self.config.calendar.week_numbers && self.grid() != scene::Grid::None { week_numbers(&days) } else { Vec::new() },
            notifications_denied: super::notifications::denied(),
            caret: editor::caret_shown(),
        }
    }

    /// The notifications, by app, every one: the card has the room (it scrolls past the screen's).
    /// None on the other cards.
    fn groups(&self) -> Vec<scene::Group> {
        if self.panel != Panel::Notifications {
            return Vec::new();
        }
        let toast = |t: &Toast| {
            // Its banner's look at rest: its picture and its app's colour.
            let (art, tint) = banner::list_look(t);
            scene::Toast {
                index: self.toasts.iter().position(|x| x.id == t.id).unwrap_or(0),
                when: arrived(t.at),
                draft: self.draft.as_ref().filter(|(id, _)| *id == t.id).map(|(_, editor)| editor.clone()),
                menu_open: self.menu_for == Some(t.id),
                collapse: self.collapse_of(t.id),
                art,
                tint,
                ..scene::Toast::of(t)
            }
        };
        super::notifications::grouped(&self.toasts)
            .into_iter()
            .map(|(_, list)| scene::Group {
                name: list.first().map_or_else(String::new, |t| t.app.clone()),
                logo: list.first().and_then(|t| t.logo.clone().map(|l| (t.aumid.clone(), l))),
                toasts: list.iter().map(|t| toast(t)).collect(),
                when: list.first().map_or_else(String::new, |t| arrived(t.at)),
            })
            .collect()
    }

    /// The grid's cells for `days`: each one's number, today, the picked day, and how busy it is.
    fn month_cells(&self, days: &[i32]) -> Vec<scene::DayCell> {
        let busy = |day: i32| self.spans.iter().filter(|(first, last)| *first <= day && day <= *last).count().min(2) as u8;
        days.iter()
            .map(|&day| {
                let (_, m, d) = time::civil_from_days(day);
                scene::DayCell { day, label: d.to_string(), today: day == self.today, selected: day == self.selected, dim: m != self.month.1, busy: busy(day) }
            })
            .collect()
    }

    /// The list under the month: from today, the next event (the hero) and the rest of the next
    /// seven days; or the picked day's. Keeps the hero's and the rows' events to open on a click.
    fn agenda(&mut self, now: i64) -> Agenda {
        let t = words::words();
        let today = self.today;
        let mut hero = None;
        let rows: Vec<Event>;
        let list_title;
        let empty;
        if self.selected == today {
            let mut ahead: Vec<&Event> = self
                .events
                .iter()
                .filter(|e| match e.when {
                    When::At { start, end } => end > now && start < now + AHEAD,
                    When::AllDay { first, end } => end > today && first < today + 7,
                })
                .collect();
            ahead.sort_by_key(|e| e.order());
            // Timed events lead: an all-day one is rarely what comes "next".
            let next = ahead.iter().find(|e| matches!(e.when, When::At { .. })).or(ahead.first()).copied();
            // The next seven days, not the calendar week: on a Saturday, Monday's meetings show.
            let week_end = today + 7;
            rows = ahead
                .iter()
                .filter(|e| Some(**e) != next && e.days(&Local).0 < week_end)
                .take(12)
                .map(|e| (*e).clone())
                .collect();
            hero = next.cloned();
            list_title = t.coming_up.to_string();
            // "Nothing else" only after a next event.
            empty = if next.is_some() { t.nothing_else } else { t.nothing }.to_string();
        } else {
            rows = model::on_day(&self.events, &self.spans, self.selected).into_iter().cloned().collect();
            list_title = capitalised(&locale::day_title(self.selected));
            empty = t.no_events.to_string();
        }
        let hero_view = hero.as_ref().map(|e| scene::Hero {
            caption: soon(e, now),
            title: e.title.clone(),
            when: join_parts(&[span_words(e), e.location.clone()]),
            color: e.color,
            joinable: join_now(e, now),
        });
        let row_views = rows
            .iter()
            .map(|e| scene::Row {
                left: if self.selected == today { week_label(e, today) } else { day_label(e, self.selected) },
                title: e.title.clone(),
                color: e.color,
                right: if self.selected == today { String::new() } else { duration_words(e) },
                joinable: join_now(e, now),
            })
            .collect();
        self.hero = hero;
        self.rows = rows;
        Agenda { hero: hero_view, list_title, rows: row_views, empty }
    }

    /// Become the other card where the panel stands (the clock's or the bell's, clicked while the
    /// panel is open): what belonged to the card shown goes.
    fn switch(&mut self, panel: Panel) {
        self.panel = panel;
        self.pc = crate::batteries::system();
        self.detail = None;
        self.menu_for = None;
        self.hover = None;
        self.pressed = None;
        self.focus = None;
        self.show_tip = None;
        self.flash = None;
        self.center_scroll = 0.0;
        self.list_scroll = 0.0;
        // The other card at its own height at once (easing is within one card).
        self.shown_h = None;
        self.relayout();
        crate::taskbar::notify_panel(panel_code(panel));
    }

    /// The days' grid, as Settings' Layout has it.
    fn grid(&self) -> scene::Grid {
        match self.config.calendar.layout {
            crate::config::CalendarLayout::Comfortable => scene::Grid::Comfortable,
            crate::config::CalendarLayout::Compact => scene::Grid::Compact,
            crate::config::CalendarLayout::AgendaOnly => scene::Grid::None,
        }
    }

    /// How much of notification `id` shows: 1, or less while it collapses away (eased in and out).
    fn collapse_of(&self, id: u32) -> f32 {
        let Some((_, since)) = self.leaving.iter().find(|(i, _)| *i == id) else { return 1.0 };
        let t = (since.elapsed().as_secs_f32() / COLLAPSE.as_secs_f32()).clamp(0.0, 1.0);
        let eased = if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 };
        1.0 - eased
    }

    /// Dismissed: notifications `ids` collapse away (Windows has them removed already).
    fn leave(&mut self, ids: &[u32]) {
        let now = Instant::now();
        for &id in ids {
            if !self.leaving.iter().any(|(i, _)| *i == id) {
                self.leaving.push((id, now));
            }
        }
        self.hover = None;
        self.animate_soon();
    }

    /// Run the animation frames (until [`View::step`] says they are done).
    fn animate_soon(&self) {
        if !self.hwnd.is_invalid() {
            unsafe { SetTimer(Some(self.hwnd), ANIM_TIMER, ANIM_MS, None) };
        }
    }

    /// One frame: notifications done collapsing go for good, the card's height comes nearer its
    /// own, and the scene is shown. `false` once nothing moves any more.
    fn step(&mut self) -> bool {
        let done: Vec<u32> = self.leaving.iter().filter(|(_, since)| since.elapsed() >= COLLAPSE).map(|(id, _)| *id).collect();
        if !done.is_empty() {
            self.toasts.retain(|t| !done.contains(&t.id));
            self.leaving.retain(|(id, _)| !done.contains(id));
        }
        if let Some(shown) = self.shown_h {
            let target = scene::natural_height(&self.input());
            let next = shown + (target - shown) * EASE;
            self.shown_h = ((target - next).abs() >= 0.5).then_some(next);
        }
        self.settle();
        !self.leaving.is_empty() || self.shown_h.is_some()
    }

    /// Lay out for what there is now, and put the panel in its corner. The open card does not jump
    /// to a new height: it eases there ([`View::step`]).
    fn place(&mut self) {
        let mut input = self.input();
        let natural = scene::natural_height(&input);
        let current = self.scene.main.h;
        let open = is_open_panel(self.hwnd);
        if open && self.shown_h.is_none() && self.scene_panel == self.panel && current > 0.0 && (natural - current).abs() > 0.5 {
            self.shown_h = Some(current);
            self.animate_soon();
        }
        input.card_h = self.shown_h;
        self.scene = scene::build(&input);
        self.scene_panel = self.panel;
        let (w, h) = (self.px(self.scene.w), self.px(self.scene.h));
        let m = self.px(MARGIN);
        let work = self.work;
        self.origin = match self.edge {
            Edge::Bottom | Edge::Right => (work.right - w - m, work.bottom - h - m),
            Edge::Top => (work.right - w - m, work.top + m),
            Edge::Left => (work.left + m, work.bottom - h - m),
        };
        // Never above the screen's top, whatever did not fit.
        self.origin.1 = self.origin.1.max(work.top + m);
        // The open panel's card follows every layout (it shrinks as its last notification goes).
        if open {
            self.keep_card();
        }
    }

    /// The card's screen rectangle to the mouse hook ([`super::OPEN_CARD`]).
    fn keep_card(&self) {
        let (x, y) = self.origin;
        let card = self.scene.main;
        let rect = RECT { left: x + self.px(card.x), top: y + self.px(card.y), right: x + self.px(card.right()), bottom: y + self.px(card.bottom()) };
        *lock(&super::OPEN_CARD) = Some(rect);
    }

    /// Lay out again (something came, went or was picked) and show it.
    fn relayout(&mut self) {
        self.place();
        if let Some(at) = self.refocus.take() {
            let order = self.tab_order();
            if !order.is_empty() {
                self.focus = Some(order[at.min(order.len() - 1)].1);
                self.place();
            }
        }
        self.render();
    }

    /// Lay out and draw once input has settled: a wheel's burst or a sweep across targets makes
    /// one frame, not one each (WM_TIMER comes only when no other message waits).
    fn later(&self) {
        unsafe { SetTimer(Some(self.hwnd), FRAME_TIMER, 1, None) };
    }

    /// Lay out, take the hover from where the pointer is now (what scrolled under it), and show it.
    fn settle(&mut self) {
        self.place();
        let mut point = cursor();
        let _ = unsafe { ScreenToClient(self.hwnd, &mut point) };
        let hover = self.hit(point.x, point.y);
        // Laid out again only if the hover changed (it lights what it is over).
        if self.note_hover(hover) {
            self.place();
        }
        self.render();
    }

    /// Slide in or out from the taskbar's edge, on the last frame drawn.
    fn animate(&self, motion: crate::win::Flyout) {
        let slide = crate::win::FLYOUT_SLIDE * self.scale;
        motion.run(slide, |offset, alpha| {
            let shift = match self.edge {
                Edge::Bottom => (0, offset),
                Edge::Top => (0, -offset),
                Edge::Left => (-offset, 0),
                Edge::Right => (offset, 0),
            };
            self.present(shift, alpha);
        });
    }

    fn select(&mut self, day: i32) {
        if day != self.selected {
            self.list_scroll = 0.0;
            self.detail = None;
        }
        self.selected = day;
        let (y, m, _) = time::civil_from_days(day);
        if (y, m) != self.month {
            self.month = (y, m);
            self.fetch();
        }
    }

    fn show_month(&mut self, delta: i32) {
        self.month = time::add_months(self.month.0, self.month.1, delta);
        self.fetch();
    }

    /// The scene's point under client pixel `(x, y)` of the calendar's window.
    fn scene_point(&self, x: i32, y: i32) -> (f32, f32) {
        (x as f32 / self.scale + self.scene.main.x, y as f32 / self.scale + self.scene.main.y)
    }

    fn hit(&self, x: i32, y: i32) -> Option<Hit> {
        let (x, y) = self.scene_point(x, y);
        self.scene.hit(x, y)
    }

    /// The pointer is over `hover` now: shown at the next frame ([`View::later`]).
    fn set_hover(&mut self, hover: Option<Hit>) {
        if self.note_hover(hover) {
            self.later();
        }
    }

    /// Take `hover` without drawing; `true` if it changed.
    fn note_hover(&mut self, hover: Option<Hit>) -> bool {
        if self.hover == hover {
            return false;
        }
        self.hover = hover;
        // A tooltip waits for the pointer to rest.
        self.show_tip = None;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIP_TIMER);
            if hover.is_some() {
                SetTimer(Some(self.hwnd), TIP_TIMER, TIP_DELAY_MS, None);
            }
        }
        true
    }

    /// Act on a click; what needs the state released comes back.
    fn activate(&mut self, hit: Hit) -> Option<After> {
        let open = |url: String| -> Option<After> {
            // A feed's link is anyone's text: only a web address opens (never a file or a share).
            if !super::web_link(&url) {
                eprintln!("calendar: not opening {url:?}, not a web address");
                return None;
            }
            Some(Box::new(move || {
                close();
                crate::win::shell_open(&url, None);
            }))
        };
        match hit {
            // Every group shows in full: its line only lights its clear button.
            Hit::Group(_) => {}
            Hit::DismissGroup(g) => {
                let ids: Vec<u32> = super::notifications::grouped(&self.toasts).get(g)?.1.iter().map(|t| t.id).collect();
                self.leave(&ids);
                super::notifications::remove(ids);
            }
            Hit::Toast(i) => {
                let toast = self.toasts.get(i)?.clone();
                super::notifications::hold(toast.id);
                return Some(Box::new(move || {
                    close();
                    rich::open(&toast);
                }));
            }
            Hit::Action(i, n) => {
                let toast = self.toasts.get(i)?.clone();
                let action = toast.rich.buttons.get(n)?.clone();
                return Some(self.carry_out(toast, action, Vec::new()));
            }
            Hit::MenuItem(i, n) => {
                let toast = self.toasts.get(i)?.clone();
                let action = toast.rich.menu.get(n)?.clone();
                self.menu_for = None;
                return Some(self.carry_out(toast, action, Vec::new()));
            }
            Hit::Menu(i) => {
                let id = self.toasts.get(i)?.id;
                self.menu_for = if self.menu_for == Some(id) { None } else { Some(id) };
                self.relayout();
                self.reveal(id);
                return None;
            }
            Hit::Reply(i) => {
                let id = self.toasts.get(i)?.id;
                // Another reply typed meanwhile is kept for when its box opens again.
                if let Some((left, editor)) = self.draft.take() {
                    drafts::keep(left, editor.text());
                }
                self.draft = Some((id, drafts::take(id)));
                editor::wake_caret(self.hwnd);
                self.menu_for = None;
                self.relayout();
                self.reveal(id);
                return None;
            }
            Hit::Draft(_) => return None,
            // Reactions are the banners' only.
            Hit::Quick(..) => return None,
            Hit::Send(i) => return self.send_reply(i),
            Hit::Dismiss(i) => {
                let id = self.toasts.get(i)?.id;
                self.leave(&[id]);
                super::notifications::remove(vec![id]);
            }
            Hit::ClearAll => {
                let ids: Vec<u32> = self.toasts.iter().map(|t| t.id).filter(|id| !self.leaving.iter().any(|(l, _)| l == id)).collect();
                let n = ids.len();
                super::notifications::clear();
                self.leave(&ids);
                // Said in the emptied card's own line, once they have gone.
                self.say(&(words::words().cleared)(n));
            }
            Hit::Allow => {
                return Some(Box::new(|| {
                    close();
                    crate::win::shell_open("ms-settings:privacy-notifications", None);
                }))
            }
            Hit::Quiet => {
                let was = super::focus::active();
                let on = super::focus::toggle();
                if on == was {
                    let t = words::words();
                    self.say(if was { t.dnd_refused_off } else { t.dnd_refused_on });
                }
                self.quiet = super::focus::active();
                crate::tray::wake_calendar();
            }
            Hit::Today => {
                self.today = time::today();
                self.select(self.today);
            }
            Hit::Prev => self.show_month(-1),
            Hit::Next => self.show_month(1),
            Hit::Day(day) => self.select(day),
            Hit::Hero => self.detail = self.hero.clone(),
            Hit::HeroJoin => return open(self.hero.as_ref()?.join.clone()?),
            Hit::Row(i) => self.detail = self.rows.get(i).cloned(),
            Hit::Join(i) => return open(self.rows.get(i)?.join.clone()?),
            Hit::Back => self.detail = None,
            Hit::DetailJoin => return open(self.detail.as_ref()?.join.clone()?),
            Hit::DetailOpen => return open(self.detail.as_ref()?.link.clone()?),
            Hit::Directions => {
                let place = self.detail.as_ref()?.location.clone();
                return open(format!("https://www.google.com/maps/search/?api=1&query={}", super::oauth::encode(&place)));
            }
            Hit::Focus => {
                if super::schedule::focus_end().is_some() {
                    super::schedule::stop_focus();
                } else if !super::schedule::start_focus(self.config.calendar.focus_minutes) && !super::focus::active() {
                    self.say(words::words().timer_not_quiet);
                }
                self.quiet = super::focus::active();
                // The clock and the timer follow.
                crate::tray::wake_calendar();
            }
            Hit::Settings => {
                // The page about this card.
                let page = match self.panel {
                    Panel::Calendar => crate::settings::OPEN_CALENDAR,
                    Panel::Notifications => crate::settings::OPEN_NOTIFICATIONS,
                    Panel::Batteries => crate::settings::OPEN_BATTERIES,
                };
                return Some(Box::new(move || {
                    close();
                    crate::tray::post(crate::settings::WM_OPEN_SETTINGS, page, 0);
                }))
            }
            Hit::WindowsSettings => {
                let page = match self.panel {
                    Panel::Calendar => "ms-settings:dateandtime",
                    Panel::Notifications => "ms-settings:notifications",
                    Panel::Batteries => batteries::windows_settings(self),
                };
                return Some(Box::new(move || {
                    close();
                    crate::win::shell_open(page, None);
                }));
            }
            Hit::Clocks => {
                return Some(Box::new(|| {
                    close();
                    super::zones::edit();
                }))
            }
        }
        // The open panel's card follows the layout ([`View::place`]): nothing left to do.
        self.relayout();
        None
    }

    /// Carry out a notification's button: it leaves the list, as in Windows' centre, and the panel
    /// closes when the app comes up.
    fn carry_out(&mut self, toast: Toast, action: rich::Action, inputs: Vec<(String, String)>) -> After {
        self.toasts.retain(|t| t.id != toast.id);
        // Off the list while the app takes it; back, with what was typed, if it does not.
        super::notifications::hold(toast.id);
        self.hover = None;
        let quiet = action.kind == Kind::Background;
        Box::new(move || {
            if !quiet {
                close();
            }
            rich::carry_out(&toast, &action, inputs);
            if quiet {
                refresh();
            }
        })
    }

    /// Send what is typed in notification `i`'s reply box.
    fn send_reply(&mut self, i: usize) -> Option<After> {
        let toast = self.toasts.get(i)?.clone();
        let reply = toast.rich.reply.clone()?;
        let editor = self.draft.take().filter(|(id, _)| *id == toast.id).map(|(_, editor)| editor)?;
        if editor.text().trim().is_empty() {
            self.draft = Some((toast.id, editor));
            return None;
        }
        Some(self.carry_out(toast, reply.send, vec![(reply.input, editor.text().to_string())]))
    }

    /// A character typed while a reply box is open.
    fn type_char(&mut self, c: char) {
        let Some((_, draft)) = self.draft.as_mut() else { return };
        if editor::on_char(draft, self.hwnd, c) == editor::Outcome::Edited {
            self.relayout();
        }
    }

    fn key(&mut self, key: VIRTUAL_KEY) -> Option<After> {
        if let Some((id, draft)) = self.draft.as_mut() {
            match editor::on_key(draft, self.hwnd, key.0) {
                editor::Outcome::Edited => {}
                editor::Outcome::Cancel => self.draft = None,
                editor::Outcome::Send => {
                    let id = *id;
                    let i = self.toasts.iter().position(|t| t.id == id)?;
                    let after = self.send_reply(i);
                    self.relayout();
                    return after;
                }
                editor::Outcome::NotMine => return None,
            }
            self.relayout();
            return None;
        }
        if key == VK_ESCAPE {
            // A step back at a time: the menu, an event's details; then the panel.
            let stepped = self.menu_for.take().is_some() || self.detail.take().is_some();
            if !stepped {
                return Some(Box::new(close));
            }
            self.focus = None;
            self.relayout();
            return None;
        }
        if key == VK_TAB {
            let back = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
            self.move_focus(back);
            self.relayout();
            return None;
        }
        if let Some(focus) = self.focus {
            // After the action, the ring goes to what then stands in the same place (the next
            // notification after a dismissal), not back to the top.
            let at = self.tab_order().iter().position(|(_, h)| *h == focus);
            match key {
                VK_RETURN | VK_SPACE => {
                    self.focus = None;
                    self.refocus = at;
                    return self.activate(focus);
                }
                VK_DELETE => {
                    if let Some(i) = scene::toast_of(focus) {
                        self.focus = None;
                        self.refocus = at;
                        return self.activate(Hit::Dismiss(i));
                    }
                    return None;
                }
                _ => {}
            }
            // The arrows move the day only from a day (on a notification, the list would change
            // under it).
            if !matches!(focus, Hit::Day(_)) {
                return None;
            }
        }
        let day = match key {
            VK_LEFT => self.selected - 1,
            VK_RIGHT => self.selected + 1,
            VK_UP => self.selected - 7,
            VK_DOWN => self.selected + 7,
            VK_HOME => time::today(),
            VK_PRIOR | VK_NEXT => {
                let (y, m, d) = time::civil_from_days(self.selected);
                let (y, m) = time::add_months(y, m, if key == VK_PRIOR { -1 } else { 1 });
                time::days_from_civil(y, m, d.min(time::days_in_month(y, m)))
            }
            _ => return None,
        };
        self.select(day);
        if self.focus.is_some() {
            self.focus = Some(Hit::Day(day));
        }
        self.relayout();
        None
    }

    fn wheel(&mut self, delta: i32, cx: i32, cy: i32) {
        let notches = delta as f32 / WHEEL_DELTA as f32;
        let (x, y) = self.scene_point(cx, cy);
        if let Some((_, max)) = self.scene.center_view.filter(|(a, _)| a.contains(x, y)) {
            self.center_scroll = (self.center_scroll - notches * SCROLL_STEP).clamp(0.0, max);
        } else if let Some((_, max)) = self.scene.list_view.filter(|(a, _)| a.contains(x, y)) {
            self.list_scroll = (self.list_scroll - notches * SCROLL_STEP).clamp(0.0, max);
        } else if delta != 0 && matches!(self.scene.hit(x, y), Some(Hit::Day(_) | Hit::Prev | Hit::Next)) {
            // Over the month, as in the shell's calendar: a notch is a month (not over the rest of
            // the card, an event's details least of all).
            self.show_month(if delta > 0 { -1 } else { 1 });
        } else {
            return;
        }
        // One frame for the burst; what scrolled under the pointer is hovered then.
        self.later();
    }

    /// Say `text` for a moment, over the card's foot.
    fn say(&mut self, text: &str) {
        self.flash = Some((text.to_string(), Instant::now()));
        unsafe { SetTimer(Some(self.hwnd), FLASH_TIMER, FLASH_FOR.as_millis() as u32, None) };
    }

    /// Scroll notification `id` whole into view (its menu or reply box opened under the fold).
    fn reveal(&mut self, id: u32) {
        let Some(index) = self.toasts.iter().position(|t| t.id == id) else { return };
        let Some((area, max)) = self.scene.center_view else { return };
        let Some(r) = self.scene.toasts.iter().find(|(i, _)| *i == index).map(|(_, r)| *r) else { return };
        let shift = if r.bottom() > area.bottom() {
            (r.bottom() - area.bottom() + 8.0).min(r.y - area.y)
        } else if r.y < area.y {
            r.y - area.y
        } else {
            return;
        };
        self.center_scroll = (self.center_scroll + shift).clamp(0.0, max);
        self.relayout();
    }

    /// Tab: the next target the keyboard can take (Shift+Tab: the one before), in the order the
    /// panel is laid out.
    fn move_focus(&mut self, back: bool) {
        let order = self.tab_order();
        if order.is_empty() {
            return;
        }
        let at = self.focus.and_then(|f| order.iter().position(|(_, h)| *h == f));
        let next = match (at, back) {
            (None, false) => 0,
            (None, true) => order.len() - 1,
            (Some(i), false) => (i + 1) % order.len(),
            (Some(i), true) => (i + order.len() - 1) % order.len(),
        };
        self.focus = Some(order[next].1);
        self.reveal_focus(order[next].0);
    }

    /// Every target the keyboard can take, in the order the panel is laid out: out of sight too
    /// (it scrolls into view), each once.
    fn tab_order(&self) -> Vec<(R, Hit)> {
        let mut order: Vec<(R, Hit)> = Vec::new();
        for &(r, hit) in &self.scene.order {
            if !matches!(hit, Hit::Draft(_)) && !order.iter().any(|(_, h)| *h == hit) {
                order.push((r, hit));
            }
        }
        order
    }

    /// Scroll the list holding the focused target (at `r`, unscrolled place as drawn) so it shows.
    fn reveal_focus(&mut self, r: R) {
        for (view, scroll) in [(self.scene.center_view, &mut self.center_scroll), (self.scene.list_view, &mut self.list_scroll)] {
            let Some((area, max)) = view else { continue };
            if r.x < area.x || r.right() > area.right() + 0.5 {
                continue;
            }
            if r.bottom() > area.bottom() {
                *scroll = (*scroll + r.bottom() - area.bottom() + 4.0).clamp(0.0, max);
            } else if r.y < area.y {
                *scroll = (*scroll - (area.y - r.y) - 4.0).clamp(0.0, max);
            }
        }
    }
}

/// The arrow cursor over the card, whatever the pointer showed where it came from (a resize edge,
/// a text beam) and whatever is under the panel.
fn arrow() {
    use windows::Win32::UI::WindowsAndMessaging::{LoadCursorW, SetCursor, IDC_ARROW};
    unsafe { SetCursor(LoadCursorW(None, IDC_ARROW).ok()) };
}

fn client_xy(lparam: LPARAM) -> (i32, i32) {
    ((lparam.0 & 0xFFFF) as u16 as i16 as i32, ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32)
}

/// Where the pointer is, on the screen.
fn cursor() -> POINT {
    let mut pt = POINT::default();
    let _ = unsafe { GetCursorPos(&mut pt) };
    pt
}

/// Ask for `WM_MOUSELEAVE` when the pointer leaves `hwnd` (once per entry).
fn track_leave(hwnd: HWND) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
    let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
    let _ = unsafe { TrackMouseEvent(&mut track) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // A panic must not unwind into user32.
    let handled = std::panic::catch_unwind(|| handle(hwnd, msg, wparam, lparam)).unwrap_or_else(|_| {
        eprintln!("calendar: a message handler panicked");
        Some(LRESULT(0))
    });
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) })
}

fn handle(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let mut after: Option<After> = None;
    match msg {
        WM_MOUSEMOVE => {
            arrow();
            track_leave(hwnd);
            let (x, y) = client_xy(lparam);
            with_view(|v| {
                let hit = v.hit(x, y);
                v.set_hover(hit);
            });
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            let (x, y) = client_xy(lparam);
            let inside = with_view(|v| {
                let (sx, sy) = v.scene_point(x, y);
                v.pressed = v.hit(x, y);
                // The press shows; a tooltip goes (what it says may be about to change).
                v.show_tip = None;
                let _ = unsafe { KillTimer(Some(v.hwnd), TIP_TIMER) };
                v.relayout();
                v.scene.on_card(sx, sy)
            });
            if inside == Some(false) {
                // A click on the panel's window but off its card closes it.
                after = Some(Box::new(close));
            }
        }
        WM_LBUTTONUP => {
            let (x, y) = client_xy(lparam);
            after = with_view(|v| {
                let pressed = v.pressed.take();
                let hit = v.hit(x, y);
                let done = pressed.filter(|p| Some(*p) == hit).and_then(|hit| v.activate(hit));
                if done.is_none() {
                    // Released elsewhere (or nothing to do): the press is let go of.
                    v.relayout();
                }
                done
            })
            .flatten();
        }
        WM_MOUSEWHEEL => {
            let delta = (wparam.0 >> 16) as u16 as i16 as i32;
            // The wheel's point is on the screen.
            let mut point = POINT { x: (lparam.0 & 0xFFFF) as u16 as i16 as i32, y: ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32 };
            let _ = unsafe { ScreenToClient(hwnd, &mut point) };
            with_view(|v| v.wheel(delta, point.x, point.y));
        }
        WM_KEYDOWN => {
            after = with_view(|v| v.key(VIRTUAL_KEY(wparam.0 as u16))).flatten();
        }
        WM_CHAR => {
            if let Some(c) = char::from_u32(wparam.0 as u32) {
                with_view(|v| v.type_char(c));
            }
        }
        // Activated for real later (a click on it, with the input that grants it): from then on,
        // leaving it closes it. Not while `raise` runs: borrowing a queue activates it for show.
        WM_ACTIVATE if (wparam.0 & 0xFFFF) as u32 != WA_INACTIVE => {
            if !flags(|f| f.raising.get()) && foreground_is(hwnd) {
                flags(|f| f.held.set(true));
            }
            // On to `DefWindowProc`, which gives the panel the keyboard focus.
            return None;
        }
        WM_ACTIVATE if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE => {
            let to = HWND(lparam.0 as *mut _);
            if !flags(|f| f.held.get()) {
                // The deactivation of a borrowed activation (see [`Flags::held`]): the app it was borrowed
                // from is still in front, and the user has not gone anywhere.
                println!("calendar: deactivated without having had the foreground; staying open");
            } else {
                println!("calendar: closed, the foreground went to {:?} ({})", to.0, crate::win::class_name(to));
                after = Some(Box::new(close));
            }
        }
        // The reply box's caret blinks while it is open.
        WM_TIMER if wparam.0 == editor::CARET_TIMER => {
            let open = with_view(|v| v.draft.is_some()).unwrap_or(false);
            if editor::caret_tick(hwnd, open) {
                with_view(View::relayout);
            }
        }
        WM_TIMER if wparam.0 == FLASH_TIMER => {
            let _ = unsafe { KillTimer(Some(hwnd), FLASH_TIMER) };
            with_view(|v| {
                v.flash = None;
                v.relayout();
            });
        }
        WM_TIMER if wparam.0 == FRAME_TIMER => {
            let _ = unsafe { KillTimer(Some(hwnd), FRAME_TIMER) };
            with_view(View::settle);
        }
        WM_TIMER if wparam.0 == ANIM_TIMER => {
            if !with_view(View::step).unwrap_or(false) {
                let _ = unsafe { KillTimer(Some(hwnd), ANIM_TIMER) };
            }
        }
        WM_TIMER if wparam.0 == REFRESH_TIMER => {
            with_view(|v| {
                v.quiet = super::focus::active();
                v.relayout();
            });
        }
        WM_TIMER if wparam.0 == TIP_TIMER => {
            let _ = unsafe { KillTimer(Some(hwnd), TIP_TIMER) };
            with_view(|v| {
                if v.hover.is_some() && v.show_tip.is_none() {
                    // Where the pointer rests: the same target can sit in two places.
                    let mut point = cursor();
                    let _ = unsafe { ScreenToClient(v.hwnd, &mut point) };
                    v.show_tip = Some(v.scene_point(point.x, point.y));
                    v.relayout();
                }
            });
        }
        WM_MOUSELEAVE => {
            // Left the card: no hover (unless the pointer is back on it already).
            let under = unsafe { GetAncestor(WindowFromPoint(cursor()), GA_ROOT) };
            if under != hwnd {
                with_view(|v| v.set_hover(None));
            }
        }
        // The open panel's (not one drawn ahead, [`prewarm`]).
        WM_DESTROY if is_open_panel(hwnd) => {
            flags(|f| {
                f.open.set(0);
                f.closed_at.set(Some(Instant::now()));
            });
            super::notifications::set_watching(false);
            *lock(&super::OPEN_CARD) = None;
            crate::taskbar::notify_panel(tap_proto::OPEN_NONE);
            if let Some(v) = VIEW.with(|cell| cell.borrow_mut().take()) {
                // Drafts of notifications still waiting, for the next open.
                if let Some((id, editor)) = &v.draft {
                    drafts::keep(*id, editor.text());
                }
                drafts::retain_listed(&v.toasts.iter().map(|t| t.id).collect::<Vec<_>>());
                SPARE_GFX.with(|s| *s.borrow_mut() = Some(spare(v.gfx.into_inner())));
            }
        }
        _ => return None,
    }
    if let Some(work) = after {
        work();
    }
    Some(LRESULT(0))
}

/// Dev: the panel alone, on the config's sources, until it closes.
#[cfg(feature = "dev")]
pub fn preview(config: &Config, panel: Panel) {
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, SetTimer, TranslateMessage, MSG};
    super::notifications::start();
    if let Err(e) = create(config, panel) {
        eprintln!("calendar: {e:#}");
        return;
    }
    // A thread timer, so a finished fetch shows without waiting for the pointer to move.
    unsafe { SetTimer(None, 0, 250, None) };
    let mut msg = MSG::default();
    while is_open() && unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        refresh_if_done();
    }
}

#[cfg(feature = "dev")]
fn refresh_if_done() {
    thread_local! {
        static WAS_FETCHING: Cell<bool> = const { Cell::new(false) };
        static TOASTS: Cell<usize> = const { Cell::new(usize::MAX) };
    }
    let now = super::fetching();
    let toasts = super::notifications::count();
    let fetched = WAS_FETCHING.with(|w| w.replace(now)) && !now;
    if fetched || TOASTS.with(|t| t.replace(toasts)) != toasts {
        refresh();
    }
}

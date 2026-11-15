//! The panel: two cards over the clock's corner, the notification centre above the calendar
//! ([`scene`]). Each card is a per-pixel-alpha layered window over Windows' acrylic, so each has
//! its own rounded corners and the gap between them is real; both are drawn as one scene off
//! screen with Direct2D ([`paint`]) and presented whole on every change (no `WM_PAINT`). The
//! calendar's window holds the mouse capture and takes the input for both.
//!
//! It opens from whichever screen edge the taskbar is on, slides in like the shell's own flyouts,
//! and closes on a click elsewhere, Escape, or losing the foreground.
//!
//! **Re-entrancy.** Opening a link or Settings can pump messages: that work runs after the view's
//! state is released ([`After`]).

mod locale;
mod paint;
mod scene;

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, ScreenToClient, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, ReleaseCapture, SetCapture, VIRTUAL_KEY, VK_BACK, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_HOME, VK_LEFT, VK_NEXT, VK_PRIOR, VK_RIGHT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, GetCursorPos, ShowWindow, SW_HIDE, SW_SHOWNA, WA_INACTIVE, WHEEL_DELTA, WINDOW_EX_STYLE, WM_ACTIVATE,
    WM_CAPTURECHANGED, WM_CHAR, WM_DESTROY, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN,
    WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};

use super::model::{self, Event, Source, When};
use super::notifications::Toast;
use super::rich::{self, Kind};
use super::time::{self, Local, Zone, DAY};
use super::zones::WorldClock;
use super::Setup;
use crate::config::Config;
use paint::{Color, Gfx, Palette, Pen};
use scene::{Draw, Fill, Hit, Ink, Scene, R};

const CLASS: PCWSTR = w!("AudioTrayCalendar");
const CLASS_TOP: PCWSTR = w!("AudioTrayNotifications");
/// A click on the clock right after the panel closed is the click that closed it.
const SETTLE: Duration = Duration::from_millis(400);
/// The gap between the panel and the screen's edges, as the shell's flyouts keep.
const MARGIN: f32 = 12.0;
/// How far a wheel notch scrolls a list.
const SCROLL_STEP: f32 = 48.0;
/// How far ahead the next event is looked for.
const AHEAD: i64 = 7 * DAY;

thread_local! {
    static VIEW: RefCell<Option<View>> = const { RefCell::new(None) };
    static OPEN: Cell<isize> = const { Cell::new(0) };
    static CLOSED_AT: Cell<Option<Instant>> = const { Cell::new(None) };
    /// The close animation is running.
    static CLOSING: Cell<bool> = const { Cell::new(false) };
}

/// The screen edge the taskbar is on, where the panel opens from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Bottom,
    Top,
    Left,
    Right,
}

struct View {
    /// The calendar's window (input, capture) and the notification centre's.
    hwnd: HWND,
    top_hwnd: HWND,
    gfx: RefCell<Gfx>,
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
    month_open: bool,
    first_weekday: u32,
    /// Short day names, Monday first.
    weekdays: [String; 7],
    /// The shown calendars' events, in their chosen colours.
    events: Vec<Event>,
    /// What the hero and the list show, to open on a click.
    hero: Option<Event>,
    rows: Vec<Event>,
    detail: Option<Event>,
    toasts: Vec<Toast>,
    /// Apps whose notifications are open (`None`: the newest app's), and shown in full.
    open_groups: Option<Vec<String>>,
    full_groups: Vec<String>,
    /// The notification whose reply box is open, and what is typed in it.
    draft: Option<(u32, String)>,
    /// The notification whose menu is open.
    menu_for: Option<u32>,
    clocks: Vec<WorldClock>,
    center_scroll: f32,
    list_scroll: f32,
    hover: Option<Hit>,
    pressed: Option<Hit>,
}

/// Work to do once the view's state is released: it may pump messages.
type After = Box<dyn FnOnce()>;

/// Open the panel, or close it if it is open.
pub fn toggle(config: &Config) {
    if OPEN.with(Cell::get) != 0 {
        close();
        return;
    }
    if CLOSED_AT.with(Cell::get).is_some_and(|at| at.elapsed() < SETTLE) {
        return;
    }
    if let Err(e) = create(config) {
        eprintln!("calendar: could not open ({e:#})");
    }
}

pub fn close() {
    let open = OPEN.with(Cell::get);
    if open == 0 || CLOSING.with(|c| c.replace(true)) {
        return;
    }
    let top = with_view(|v| {
        v.animate(crate::win::Flyout::Close);
        v.top_hwnd
    });
    let _ = unsafe { DestroyWindow(HWND(open as *mut _)) };
    if let Some(top) = top {
        let _ = unsafe { DestroyWindow(top) };
    }
    CLOSING.with(|c| c.set(false));
}

/// The clock's unread badge: an accent pill with `count`, drawn at the taskbar's scale into a PNG
/// the TAP paints beside the clock. Gives the file, its width in DIPs and the taskbar's DPI.
/// Each look (count, colour, scale) is its own file, drawn once: XAML caches images by address.
pub fn badge(count: u32) -> Option<(String, u32, u32)> {
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    const H: f32 = 18.0;
    let tray = unsafe { FindWindowW(w!("Shell_TrayWnd"), None) }.ok();
    let dpi = tray.map_or(96, |t| unsafe { GetDpiForWindow(t) }).max(96);
    let scale = dpi as f32 / 96.0;
    let p = Palette::current();
    let label = if count > 99 { "99+".to_string() } else { count.to_string() };
    let w = (H + 7.0 * (label.chars().count().max(1) - 1) as f32).round();
    let ink = {
        let Color(r, g, b, _) = p.accent;
        // Dark figures on a light accent, as the shell's own badges.
        if 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.55 { Color::hex(0x000000, 0.9) } else { Color::hex(0xFFFFFF, 1.0) }
    };
    let hex = |c: Color| format!("{:02x}{:02x}{:02x}", (c.0 * 255.0) as u8, (c.1 * 255.0) as u8, (c.2 * 255.0) as u8);
    let dir = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("audio-tray").join("badges");
    let path = dir.join(format!("{label}-{}-{}-{dpi}.png", hex(p.accent), hex(ink)).replace('+', "p"));
    if !path.exists() {
        let _ = std::fs::create_dir_all(&dir);
        let size = ((w * scale).round() as u32, (H * scale).round() as u32);
        let drawn = Gfx::new().and_then(|mut gfx| {
            gfx.frame(size, scale, |pen| {
                pen.clear(Color(0.0, 0.0, 0.0, 0.0));
                pen.fill(R::new(0.0, 0.0, w, H), H / 2.0, p.accent);
                pen.text(&label, paint::Font::CaptionStrong, R::new(0.0, -0.5, w, H), ink, paint::Align::Center);
            })?;
            gfx.save_png(&path)
        });
        if let Err(e) = drawn {
            eprintln!("calendar: could not draw the clock's badge ({e:#})");
            return None;
        }
    }
    Some((path.to_string_lossy().into_owned(), w as u32, dpi))
}

/// The events or the notifications changed: show them.
pub fn refresh() {
    with_view(|v| {
        v.reload();
        v.relayout();
    });
    keep_rect();
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

fn create(config: &Config) -> Result<()> {
    let mut cursor = POINT::default();
    let _ = unsafe { GetCursorPos(&mut cursor) };
    let (scale, rect, work) = monitor(cursor);
    let style = WINDOW_EX_STYLE(WS_EX_LAYERED.0 | WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0);
    let hwnd = crate::win::create_popup(CLASS, w!("Calendar"), Some(wndproc), style, (0, 0, 1, 1)).context("CreateWindowExW")?;
    let top_hwnd = crate::win::create_popup(CLASS_TOP, w!("Notifications"), Some(top_proc), style, (0, 0, 1, 1)).context("CreateWindowExW")?;
    unsafe {
        crate::layered::style_panel(hwnd, crate::layered::CORNER_ROUND);
        crate::layered::style_panel(top_hwnd, crate::layered::CORNER_ROUND);
    }
    let today = time::today();
    let (ty, tm, _) = time::civil_from_days(today);
    let setup = config.calendar.setup();
    let first_weekday = config.calendar.first_weekday.map_or_else(locale::first_weekday, |d| d.min(6) as u32);
    let empty = scene::build(&empty_input());
    let mut view = View {
        hwnd,
        top_hwnd,
        gfx: RefCell::new(Gfx::new()?),
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
        month_open: false,
        first_weekday,
        weekdays: locale::weekday_names(),
        events: Vec::new(),
        hero: None,
        rows: Vec::new(),
        detail: None,
        toasts: Vec::new(),
        open_groups: None,
        full_groups: Vec::new(),
        draft: None,
        menu_for: None,
        clocks: Vec::new(),
        center_scroll: 0.0,
        list_scroll: 0.0,
        hover: None,
        pressed: None,
    };
    super::notifications::set_watching(true);
    view.reload();
    view.fetch();
    view.place();
    view.draw_frame();
    VIEW.with(|cell| *cell.borrow_mut() = Some(view));
    OPEN.with(|o| o.set(hwnd.0 as isize));
    with_view(|v| {
        unsafe {
            let _ = ShowWindow(v.hwnd, SW_SHOWNA);
            if v.scene.top.is_some() {
                let _ = ShowWindow(v.top_hwnd, SW_SHOWNA);
            }
        }
        v.animate(crate::win::Flyout::Open);
    });
    // The click that opened it was swallowed by our hook, so Windows has not lent us the
    // foreground: without it there are no keys (Escape) and no deactivation to close on.
    if crate::music::player::raise(hwnd) == crate::music::player::Raise::Refused {
        eprintln!("calendar: could not take the foreground");
    }
    unsafe {
        // Like a menu: a click anywhere else comes here first, and closes it.
        SetCapture(hwnd);
    }
    arrow();
    keep_rect();
    Ok(())
}

/// What the scene is built from before anything is read (never shown).
fn empty_input() -> scene::Input {
    scene::Input {
        room: 0.0,
        groups: None,
        unread: 0,
        quiet: false,
        center_scroll: 0.0,
        weekday: String::new(),
        date: String::new(),
        clocks: String::new(),
        month_open: false,
        month_title: String::new(),
        heads: Default::default(),
        days: Vec::new(),
        hero: None,
        list_title: String::new(),
        rows: Vec::new(),
        empty: String::new(),
        list_scroll: 0.0,
        focus_label: String::new(),
        focus_running: false,
        detail: None,
        hover: None,
    }
}

/// Tell the mouse hook where the panel is: a click outside it closes it.
fn keep_rect() {
    let rect = with_view(|v| {
        let (x, y) = v.origin;
        RECT { left: x, top: y, right: x + v.px(scene::W), bottom: y + v.px(v.scene.h) }
    });
    *super::OPEN_RECT.lock().unwrap_or_else(|e| e.into_inner()) = rect;
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
        self.toasts = if calendar.notifications { super::notifications::list() } else { Vec::new() };
        self.clocks = if calendar.world_clocks { super::zones::clocks() } else { Vec::new() };
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
        let zone = Local;
        let today = self.today;

        // Notifications, by app; the newest app's open unless the user chose.
        let groups = (!self.toasts.is_empty()).then(|| {
            super::notifications::grouped(&self.toasts)
                .into_iter()
                .enumerate()
                .map(|(g, (key, list))| {
                    let open = match &self.open_groups {
                        Some(open) => open.contains(&key),
                        None => g == 0,
                    };
                    scene::Group {
                        name: list.first().map_or_else(String::new, |t| t.app.clone()),
                        logo: list.first().and_then(|t| t.logo.clone().map(|l| (t.aumid.clone(), l))),
                        toasts: list
                            .iter()
                            .map(|t| scene::Toast {
                                index: self.toasts.iter().position(|x| x.id == t.id).unwrap_or(0),
                                title: if t.title.is_empty() { t.app.clone() } else { t.title.clone() },
                                body: t.body.clone(),
                                avatar: t.rich.avatar.as_ref().map(|p| (p.key.clone(), p.bytes.clone())),
                                picture: t.rich.picture.as_ref().map(|p| (p.key.clone(), p.bytes.clone())),
                                buttons: t.rich.buttons.iter().map(|a| a.label.clone()).collect(),
                                reply: t.rich.reply.as_ref().map(|r| if r.placeholder.is_empty() { "Reply".to_string() } else { r.placeholder.clone() }),
                                menu: t.rich.menu.iter().map(|a| a.label.clone()).collect(),
                                draft: self.draft.as_ref().filter(|(id, _)| *id == t.id).map(|(_, text)| text.clone()),
                                menu_open: self.menu_for == Some(t.id),
                            })
                            .collect(),
                        when: list.first().map_or_else(String::new, |t| arrived(t.at)),
                        all: self.full_groups.contains(&key),
                        open,
                    }
                })
                .collect()
        });

        // The week of the selected day, or the month shown.
        let days: Vec<i32> = if self.month_open {
            let start = self.week_start(time::days_from_civil(self.month.0, self.month.1, 1));
            (0..42).map(|i| start + i).collect()
        } else {
            let start = self.week_start(self.selected);
            (0..7).map(|i| start + i).collect()
        };
        let busy = |day: i32| self.events.iter().filter(|e| e.on_day(day, &zone)).count().min(2) as u8;
        let cells = days
            .iter()
            .map(|&day| {
                let (_, m, d) = time::civil_from_days(day);
                scene::DayCell {
                    day,
                    label: d.to_string(),
                    today: day == today,
                    selected: day == self.selected,
                    dim: self.month_open && m != self.month.1,
                    busy: busy(day),
                }
            })
            .collect();

        // The next event (from today), then the rest of its week; or the picked day.
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
            let week_end = self.week_start(today) + 7;
            rows = ahead
                .iter()
                .filter(|e| Some(**e) != next && e.days(&zone).0 < week_end)
                .take(12)
                .map(|e| (*e).clone())
                .collect();
            hero = next.cloned();
            list_title = "Later this week".to_string();
            empty = "Nothing else this week".to_string();
        } else {
            rows = model::on_day(&self.events, self.selected, &zone).into_iter().cloned().collect();
            list_title = capitalised(&locale::day_title(self.selected));
            empty = "No events".to_string();
        }
        let hero_view = hero.as_ref().map(|e| scene::Hero {
            caption: soon(e, now).to_uppercase(),
            title: e.title.clone(),
            when: join_parts(&[span_words(e), e.location.clone()]),
            color: e.color,
            joinable: e.join.is_some(),
        });
        let row_views = rows
            .iter()
            .map(|e| scene::Row {
                left: if self.selected == today { week_label(e, today) } else { day_label(e, self.selected) },
                title: e.title.clone(),
                color: e.color,
                right: if self.selected == today { String::new() } else { duration_words(e) },
                joinable: e.join.is_some(),
            })
            .collect();
        self.hero = hero;
        self.rows = rows;

        let focus = super::schedule::focus_end();
        let focus_label = match focus {
            Some(end) => format!("Focus · {} min left", ((end - now) + 59) / 60),
            None => format!("Focus {} min", self.config.calendar.focus_minutes),
        };
        let clocks = self
            .clocks
            .iter()
            .take(2)
            .map(|c| format!("{} {}", c.name, locale::clock(time::second_of_day(c.wall))))
            .collect::<Vec<_>>()
            .join(" · ");
        let detail = self.detail.as_ref().map(|e| detail_view(e, &self.setup, now, self.selected));

        scene::Input {
            room: (self.work.bottom - self.work.top) as f32 / self.scale - 2.0 * MARGIN,
            groups,
            unread: self.toasts.len(),
            quiet: super::focus::active(),
            center_scroll: self.center_scroll,
            weekday: capitalised(&locale::format(today, "dddd")),
            date: locale::format(today, "d MMMM"),
            clocks,
            month_open: self.month_open,
            month_title: capitalised(&locale::month_title(self.month.0, self.month.1)),
            heads: std::array::from_fn(|i| self.weekdays[(self.first_weekday as usize + i) % 7].clone()),
            days: cells,
            hero: hero_view,
            list_title,
            rows: row_views,
            empty,
            list_scroll: self.list_scroll,
            focus_label,
            focus_running: focus.is_some(),
            detail,
            hover: self.hover,
        }
    }

    /// Lay out for what there is now, and put the panel in its corner.
    fn place(&mut self) {
        let input = self.input();
        self.scene = scene::build(&input);
        let (w, h) = (self.px(scene::W), self.px(self.scene.h));
        let m = self.px(MARGIN);
        let work = self.work;
        self.origin = match self.edge {
            Edge::Bottom | Edge::Right => (work.right - w - m, work.bottom - h - m),
            Edge::Top => (work.right - w - m, work.top + m),
            Edge::Left => (work.left + m, work.bottom - h - m),
        };
    }

    /// Lay out again (something came, went or was picked) and show it.
    fn relayout(&mut self) {
        let had_top = self.scene.top.is_some();
        self.place();
        let has_top = self.scene.top.is_some();
        if had_top != has_top {
            unsafe {
                let _ = ShowWindow(self.top_hwnd, if has_top { SW_SHOWNA } else { SW_HIDE });
            }
        }
        self.render();
    }

    /// Draw the scene off screen.
    fn draw_frame(&self) {
        let size = (self.px(scene::W) as u32, self.px(self.scene.h) as u32);
        if let Err(e) = self.gfx.borrow_mut().frame(size, self.scale, |pen| draw(pen, &self.scene, &self.palette)) {
            eprintln!("calendar: could not draw ({e:#})");
        }
    }

    /// Draw and put on screen.
    fn render(&self) {
        self.draw_frame();
        self.present((0, 0), 255);
    }

    /// Each card's part of the frame to its window, `offset` from its place.
    fn present(&self, (dx, dy): (i32, i32), alpha: u8) {
        let gfx = self.gfx.borrow();
        let full_w = self.px(scene::W);
        let cut = |card: R| -> (Vec<u8>, i32, i32, i32) {
            let (y0, h) = (self.px(card.y), self.px(card.h));
            let row = full_w as usize * 4;
            let start = (y0.max(0) as usize * row).min(gfx.pixels.len());
            let end = ((y0 + h).max(0) as usize * row).min(gfx.pixels.len());
            (gfx.pixels[start..end].to_vec(), full_w, (end - start) as i32 / row.max(1) as i32, y0)
        };
        let (x, y) = (self.origin.0 + dx, self.origin.1 + dy);
        let (pixels, w, h, y0) = cut(self.scene.main);
        crate::layered::present_premultiplied(self.hwnd, &pixels, w, h, x, y + y0, alpha);
        if let Some(top) = self.scene.top {
            let (pixels, w, h, y0) = cut(top);
            crate::layered::present_premultiplied(self.top_hwnd, &pixels, w, h, x, y + y0, alpha);
        }
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
        (x as f32 / self.scale, y as f32 / self.scale + self.scene.main.y)
    }

    fn hit(&self, x: i32, y: i32) -> Option<Hit> {
        let (x, y) = self.scene_point(x, y);
        self.scene.hit(x, y)
    }

    fn set_hover(&mut self, hover: Option<Hit>) {
        if self.hover != hover {
            self.hover = hover;
            self.relayout();
        }
    }

    /// The key of notification group `g`.
    fn group_key(&self, g: usize) -> Option<String> {
        super::notifications::grouped(&self.toasts).get(g).map(|(k, _)| k.clone())
    }

    /// Act on a click; what needs the state released comes back.
    fn activate(&mut self, hit: Hit) -> Option<After> {
        let open = |url: String| -> Option<After> {
            Some(Box::new(move || {
                close();
                crate::win::shell_open(&url, None);
            }))
        };
        match hit {
            Hit::Center => {
                self.month_open = false;
                self.detail = None;
            }
            Hit::Group(g) => {
                let key = self.group_key(g)?;
                let groups = super::notifications::grouped(&self.toasts);
                let mut open: Vec<String> = self.open_groups.clone().unwrap_or_else(|| groups.first().map(|(k, _)| k.clone()).into_iter().collect());
                if open.contains(&key) {
                    open.retain(|k| *k != key);
                } else {
                    open.push(key);
                }
                self.open_groups = Some(open);
            }
            Hit::More(g) => {
                let key = self.group_key(g)?;
                self.full_groups.push(key);
            }
            Hit::Toast(i) => {
                let toast = self.toasts.get(i)?.clone();
                super::notifications::remove(vec![toast.id]);
                return Some(Box::new(move || {
                    close();
                    let rich = &toast.rich;
                    if rich.launch.is_empty() && rich.launch_kind != Kind::Protocol {
                        super::notifications::open_app(&toast.aumid);
                    } else {
                        rich::invoke(&toast.aumid, rich.activator, &rich.launch, rich.launch_kind, Vec::new());
                    }
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
            }
            Hit::Reply(i) => {
                let id = self.toasts.get(i)?.id;
                self.draft = Some((id, String::new()));
                self.menu_for = None;
            }
            Hit::Draft(_) => return None,
            Hit::Send(i) => return self.send_reply(i),
            Hit::Dismiss(i) => {
                let id = self.toasts.get(i)?.id;
                self.toasts.retain(|t| t.id != id);
                super::notifications::remove(vec![id]);
                self.hover = None;
            }
            Hit::ClearAll => {
                super::notifications::clear();
                self.toasts.clear();
                self.hover = None;
            }
            Hit::Quiet => {
                super::focus::toggle();
                crate::tray::wake_calendar();
            }
            Hit::Today => {
                self.today = time::today();
                self.select(self.today);
            }
            Hit::Month => {
                self.month_open = !self.month_open;
                let (y, m, _) = time::civil_from_days(self.selected);
                self.month = (y, m);
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
                } else {
                    super::schedule::start_focus(self.config.calendar.focus_minutes);
                }
                // The clock and the timer follow.
                crate::tray::wake_calendar();
            }
            Hit::Settings => {
                return Some(Box::new(|| {
                    close();
                    crate::tray::post(crate::settings::WM_OPEN_SETTINGS, crate::settings::OPEN_CALENDAR, 0);
                }))
            }
            Hit::Clocks => {
                return Some(Box::new(|| {
                    close();
                    super::zones::edit();
                }))
            }
        }
        self.relayout();
        Some(Box::new(keep_rect))
    }

    /// Carry out a notification's button: it leaves the list, as in Windows' centre, and the panel
    /// closes when the app comes up.
    fn carry_out(&mut self, toast: Toast, action: rich::Action, inputs: Vec<(String, String)>) -> After {
        self.toasts.retain(|t| t.id != toast.id);
        super::notifications::remove(vec![toast.id]);
        self.hover = None;
        let quiet = action.kind == Kind::Background;
        Box::new(move || {
            if !quiet {
                close();
            }
            rich::invoke(&toast.aumid, toast.rich.activator, &action.arguments, action.kind, inputs);
            if quiet {
                refresh();
                keep_rect();
            }
        })
    }

    /// Send what is typed in notification `i`'s reply box.
    fn send_reply(&mut self, i: usize) -> Option<After> {
        let toast = self.toasts.get(i)?.clone();
        let reply = toast.rich.reply.clone()?;
        let text = self.draft.take().filter(|(id, _)| *id == toast.id).map(|(_, text)| text)?;
        if text.trim().is_empty() {
            self.draft = Some((toast.id, text));
            return None;
        }
        Some(self.carry_out(toast, reply.send, vec![(reply.input, text)]))
    }

    /// A character typed while a reply box is open. `true`: it was the reply box's.
    fn type_char(&mut self, c: char) -> bool {
        let Some((_, draft)) = self.draft.as_mut() else { return false };
        match c {
            // Backspace, Enter and Escape come as keys.
            '\u{8}' | '\r' | '\n' | '\u{1b}' => return true,
            // Ctrl+Backspace: the last word.
            '\u{7f}' => {
                let kept = draft.trim_end().rfind(' ').map_or(0, |i| i + 1);
                draft.truncate(kept);
            }
            // Ctrl+V.
            '\u{16}' => draft.push_str(&crate::win::clipboard_text().ok().flatten().unwrap_or_default().replace(['\r', '\n'], " ")),
            c if !c.is_control() => draft.push(c),
            _ => return true,
        }
        self.relayout();
        true
    }

    fn key(&mut self, key: VIRTUAL_KEY) -> Option<After> {
        if let Some((id, draft)) = self.draft.as_mut() {
            match key {
                VK_ESCAPE => self.draft = None,
                VK_BACK => {
                    draft.pop();
                }
                VK_RETURN => {
                    let id = *id;
                    let i = self.toasts.iter().position(|t| t.id == id)?;
                    let after = self.send_reply(i);
                    self.relayout();
                    return after;
                }
                _ => return None,
            }
            self.relayout();
            return None;
        }
        if key == VK_ESCAPE && self.menu_for.take().is_some() {
            self.relayout();
            return None;
        }
        if key == VK_ESCAPE {
            if self.detail.take().is_some() {
                self.relayout();
                return None;
            }
            return Some(Box::new(close));
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
        self.relayout();
        Some(Box::new(keep_rect))
    }

    fn wheel(&mut self, delta: i32, x: i32, y: i32) {
        let notches = delta as f32 / WHEEL_DELTA as f32;
        let (x, y) = self.scene_point(x, y);
        if let Some((_, max)) = self.scene.center_view.filter(|(a, _)| a.contains(x, y)) {
            self.center_scroll = (self.center_scroll - notches * SCROLL_STEP).clamp(0.0, max);
        } else if let Some((_, max)) = self.scene.list_view.filter(|(a, _)| a.contains(x, y)) {
            self.list_scroll = (self.list_scroll - notches * SCROLL_STEP).clamp(0.0, max);
        } else if self.month_open && self.scene.main.contains(x, y) && delta != 0 {
            // Over the month, as in the shell's calendar: a notch is a month.
            self.show_month(if delta > 0 { -1 } else { 1 });
        } else {
            return;
        }
        self.relayout();
    }
}

/// "Next · in 6 h 35", "Now · until 21:03", "Next · Monday".
fn soon(event: &Event, now: i64) -> String {
    match event.when {
        When::At { start, end } if start <= now => format!("Now · until {}", locale::clock(time::second_of_day(Local.to_local(end)))),
        When::At { start, .. } => {
            let minutes = (start - now + 59) / 60;
            let day = time::day_of(Local.to_local(start));
            if day == time::today() || minutes < 12 * 60 {
                match minutes {
                    0..=59 => format!("Next · in {minutes} min"),
                    _ => format!("Next · in {} h {:02}", minutes / 60, minutes % 60),
                }
            } else {
                format!("Next · {}", capitalised(&locale::format(day, "dddd")))
            }
        }
        When::AllDay { first, .. } if first <= time::today() => "Today · all day".into(),
        When::AllDay { first, .. } => format!("Next · {}", capitalised(&locale::format(first, "dddd"))),
    }
}

/// "15:59 – 21:03", "All day".
fn span_words(event: &Event) -> String {
    match event.when {
        When::AllDay { first, end } if end - first > 1 => format!("{} – {}", locale::format(first, "d MMM"), locale::format(end - 1, "d MMM")),
        When::AllDay { .. } => "All day".into(),
        When::At { start, end } => {
            let (s, e) = (Local.to_local(start), Local.to_local(end));
            let first = locale::clock(time::second_of_day(s));
            format!("{first} – {}", locale::clock(time::second_of_day(e)))
        }
    }
}

/// A row's day and time in the week's list: "Mon 09:15", "Tue".
fn week_label(event: &Event, today: i32) -> String {
    let (first, _) = event.days(&Local);
    let day = if first <= today { "Today".to_string() } else { capitalised(&locale::format(first, "ddd")).trim_end_matches('.').to_string() };
    match event.when {
        When::At { start, .. } => format!("{day} {}", locale::clock(time::second_of_day(Local.to_local(start)))),
        When::AllDay { .. } => day,
    }
}

/// A row's time on its day: "09:15", "All day".
fn day_label(event: &Event, day: i32) -> String {
    match event.when {
        When::AllDay { .. } => "All day".into(),
        When::At { start, .. } => {
            let s = Local.to_local(start);
            if time::day_of(s) == day { locale::clock(time::second_of_day(s)) } else { "…".into() }
        }
    }
}

/// "1 h 30", "45 min"; nothing for an all-day event.
fn duration_words(event: &Event) -> String {
    match event.when {
        When::At { start, end } if end > start => {
            let minutes = (end - start) / 60;
            match (minutes / 60, minutes % 60) {
                (0, m) => format!("{m} min"),
                (h, 0) => format!("{h} h"),
                (h, m) => format!("{h} h {m:02}"),
            }
        }
        _ => String::new(),
    }
}

fn join_parts(parts: &[String]) -> String {
    parts.iter().filter(|p| !p.is_empty()).cloned().collect::<Vec<_>>().join(" · ")
}

/// An event's details, in words.
fn detail_view(event: &Event, setup: &Setup, now: i64, day: i32) -> scene::Detail {
    let d = &event.details;
    let initials = d
        .people
        .iter()
        .take(3)
        .map(|name| name.split([' ', '.', '@']).filter(|w| !w.is_empty()).take(2).filter_map(|w| w.chars().next()).flat_map(char::to_uppercase).collect())
        .collect();
    let mut people: Vec<String> = d.people.iter().take(2).map(|p| p.split(' ').next().unwrap_or(p).to_string()).collect();
    if d.people.len() > 2 {
        people.push(format!("+{}", d.people.len() - 2));
    }
    let mut people = people.join(", ");
    if !d.response.is_empty() {
        people = join_parts(&[people, format!("you: {}", d.response.to_lowercase())]);
    }
    let source = super::status(setup).calendars.iter().find(|c| c.key == event.calendar).map_or_else(
        || match event.source {
            Source::Google => "Google".to_string(),
            Source::Microsoft => "Outlook".to_string(),
            Source::Link(i) => setup.links.get(i).map_or_else(|| "Calendar".into(), |l| l.name.clone()),
        },
        |c| c.name.clone(),
    );
    let open = event.link.as_ref().map(|_| match event.source {
        Source::Google => "Open in Google Calendar".to_string(),
        Source::Microsoft => "Open in Outlook".to_string(),
        Source::Link(_) => "Open".to_string(),
    });
    let (first, _) = event.days(&Local);
    scene::Detail {
        back: capitalised(&locale::day_title(first.max(day.min(first)))),
        title: event.title.clone(),
        when: join_parts(&[span_words(event), duration_words(event)]),
        soon: soon(event, now),
        color: event.color,
        place: event.location.clone(),
        initials,
        people,
        notes: d.notes.clone(),
        source,
        open,
        joinable: event.join.is_some(),
    }
}

/// When a notification arrived, in a few words.
fn arrived(at: i64) -> String {
    let now = time::now();
    let minutes = (now - at).max(0) / 60;
    let wall = Local.to_local(at);
    match minutes {
        0 => "now".into(),
        1..=59 => format!("{minutes} min"),
        _ if time::day_of(wall) == time::day_of(Local.to_local(now)) => locale::clock(time::second_of_day(wall)),
        _ => capitalised(&locale::format(time::day_of(wall), "ddd d")),
    }
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

/// Paint the scene.
fn draw(pen: &Pen, scene: &Scene, p: &Palette) {
    pen.clear(Color(0.0, 0.0, 0.0, 0.0));
    let ink = |ink: Ink| match ink {
        Ink::Text => p.text,
        Ink::Text2 => p.text2,
        Ink::Text3 => p.text3,
        Ink::Accent => p.accent,
        Ink::OnAccent => p.on_accent,
    };
    let fill = |fill: Fill| match fill {
        Fill::Raised => p.raised,
        Fill::Hover => p.hover,
        Fill::Accent => p.accent,
        Fill::Rule => p.text.alpha(0.08),
        Fill::Mark => p.text.alpha(0.5),
        Fill::Rgb(rgb) => Color::rgb(rgb),
    };
    for (r, d) in &scene.items {
        match d {
            Draw::Card => pen.fill(*r, 0.0, p.bg),
            Draw::Rect { fill: f, radius } => pen.fill(*r, *radius, fill(*f)),
            Draw::Outline { ink: i, radius } => pen.stroke(*r, *radius, 1.0, ink(*i).alpha(0.6)),
            Draw::Text { text, font, ink: i, align } => pen.text(text, *font, *r, ink(*i), *align),
            Draw::Glyph { glyph, font, ink: i } => pen.glyph(*glyph, *font, *r, ink(*i)),
            Draw::Logo { key, bytes } => pen.image(key, bytes, *r),
            Draw::Avatar { key, bytes } => pen.avatar(key, bytes, *r),
            Draw::Picture { key, bytes } => pen.picture(key, bytes, *r),
            Draw::Clip => pen.clip(*r),
            Draw::Unclip => pen.unclip(),
        }
    }
}

/// The arrow cursor, whatever was showing when the capture began.
fn arrow() {
    use windows::Win32::UI::WindowsAndMessaging::{LoadCursorW, SetCursor, IDC_ARROW};
    unsafe { SetCursor(LoadCursorW(None, IDC_ARROW).ok()) };
}

fn client_xy(lparam: LPARAM) -> (i32, i32) {
    ((lparam.0 & 0xFFFF) as u16 as i16 as i32, ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32)
}

/// The notification centre's window: input goes to the calendar's, which holds the capture.
unsafe extern "system" fn top_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
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
                v.scene.on_card(sx, sy)
            });
            if inside == Some(false) {
                // Under capture: a click outside the panel closes it.
                after = Some(Box::new(close));
            }
        }
        WM_LBUTTONUP => {
            let (x, y) = client_xy(lparam);
            after = with_view(|v| {
                let pressed = v.pressed.take();
                let hit = v.hit(x, y);
                pressed.filter(|p| Some(*p) == hit).and_then(|hit| v.activate(hit))
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
        WM_ACTIVATE if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE => {
            after = Some(Box::new(close));
        }
        WM_CAPTURECHANGED => {
            // Capture taken away (another app came up): give way to it. Our own release in
            // WM_DESTROY arrives while closing, when there is nothing left to close.
            if OPEN.with(Cell::get) == hwnd.0 as isize && unsafe { GetCapture() } != hwnd {
                after = Some(Box::new(close));
            }
        }
        WM_DESTROY => {
            OPEN.with(|o| o.set(0));
            super::notifications::set_watching(false);
            *super::OPEN_RECT.lock().unwrap_or_else(|e| e.into_inner()) = None;
            CLOSED_AT.with(|c| c.set(Some(Instant::now())));
            let top = VIEW.with(|cell| cell.borrow_mut().take()).map(|v| v.top_hwnd);
            if let Some(top) = top {
                let _ = unsafe { DestroyWindow(top) };
            }
            // Only a capture of our own: capture is per thread, and the audio flyout (on this same
            // thread) may hold it by now; ReleaseCapture would take it from the flyout, which
            // then closes at once.
            if unsafe { GetCapture() } == hwnd {
                let _ = unsafe { ReleaseCapture() };
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
pub fn preview(config: &Config) {
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, SetTimer, TranslateMessage, MSG};
    super::notifications::start();
    if let Err(e) = create(config) {
        eprintln!("calendar: {e:#}");
        return;
    }
    // A thread timer, so a finished fetch shows without waiting for the pointer to move.
    unsafe { SetTimer(None, 0, 250, None) };
    let mut msg = MSG::default();
    while OPEN.with(Cell::get) != 0 && unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
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

//! The settings window: a normal, resizable app window (it stays open when clicked away from,
//! unlike the flyout), in Windows 11's settings style. A navigation pane on the left — Home, Taskbar,
//! Calendar, Notifications, Edge pill, Motion, Audio devices, Batteries, Keyboards, Connectors,
//! About ([`layout::Page`]) — and the page on the right, drawn with Direct2D ([`paint`]) from a
//! pure layout ([`layout`]).
//!
//! **Its own copy of the config.** Each change is made to the copy, then handed to the tray as
//! [`take_pending`] behind a posted [`WM_SETTINGS`]: the tray applies it (saves, re-places the
//! pill, redraws the strip, restarts a connector) from its own queue, never from inside this
//! window's procedure. The tray hands changes made elsewhere back through [`sync`], and what the
//! hub knows through [`follow`].
//!
//! **Re-entrancy.** A call that can pump messages (opening a page in the shell, moving the window)
//! runs after the window's state is released: see [`Window::activate`].

mod layout;
mod model;
mod paint;

use std::cell::{Cell, RefCell};

use anyhow::{Context, Result};
use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, IsIconic, KillTimer, SetTimer, WM_TIMER, LoadCursorW, LoadIconW, PostMessageW,
    RegisterClassW, SetForegroundWindow, SetWindowPos, ShowWindow, CS_HREDRAW, CS_VREDRAW, IDC_ARROW, MINMAXINFO,
    SWP_NOACTIVATE, SWP_NOZORDER, SW_RESTORE, SW_SHOWNORMAL, WHEEL_DELTA, WINDOW_EX_STYLE, WM_APP, WM_DESTROY,
    WM_ACTIVATE, WM_CHAR, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ERASEBKGND, WM_GETMINMAXINFO, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_PAINT, WM_SETTINGCHANGE, WM_SIZE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

use crate::audio::wasapi::WasapiBackend;
use crate::config::Config;
use crate::lang::tr;
use crate::tf;
use layout::{Button, Choice, Laid, Page, Target, Toggle};
use model::{Model, Status};
use paint::{Font, Gfx, Palette, Pointer};

/// Posted to the tray's message window: `wParam` is [`APPLY`], [`QUIT`] or [`RESTART`].
pub const WM_SETTINGS: u32 = WM_APP + 38;
/// Posted to the tray's message window to open this window (a second launch of the app does).
pub const WM_OPEN_SETTINGS: u32 = WM_APP + 39;
/// Take the config from [`take_pending`].
pub const APPLY: usize = 0;
/// Quit Audio Tray.
pub const QUIT: usize = 1;
/// Relaunch into a staged update.
pub const RESTART: usize = 2;
/// [`WM_OPEN_SETTINGS`]'s `wParam` to open on a page: each panel's gear opens the page about it.
pub const OPEN_CALENDAR: usize = 1;
pub const OPEN_TASKBAR: usize = 2;
pub const OPEN_NOTIFICATIONS: usize = 3;
pub const OPEN_BATTERIES: usize = 4;

/// Posted to this window: something in [`INBOX`] to read.
const WM_FOLLOW: u32 = WM_APP + 1;
/// Posted to this window by the keyboard switcher's thread: a keyboard was typed on during a
/// capture ([`crate::keyboards::take_captured`]).
const WM_KEYBOARD_CAPTURED: u32 = WM_APP + 2;
/// The longest name a keyboard can be given.
const NAME_MAX: usize = 40;
/// `WM_MOUSELEAVE`, filed under UI::Controls in windows-rs.
const WM_MOUSELEAVE: u32 = 0x02A3;
const CLASS: PCWSTR = w!("AudioTraySettings");
/// The window's size when it opens, and the least it can be, in DIPs.
const OPEN_SIZE: (f32, f32) = (1040.0, 760.0);
const MIN_SIZE: (f32, f32) = (720.0, 480.0);
/// How far one wheel notch scrolls the page.
const SCROLL_STEP: f32 = 64.0;
/// Repeating while a page shows moving pictures (Home, Motion): each tick repaints them.
const ANIMATION_TIMER: usize = 1;
const ANIMATION_MS: u32 = 33;

thread_local! {
    static WINDOW: RefCell<Option<Window>> = const { RefCell::new(None) };
    /// The open window, readable while [`WINDOW`] is borrowed.
    static HWND_OPEN: Cell<isize> = const { Cell::new(0) };
    /// The latest config, for the tray to apply.
    static PENDING: RefCell<Option<Config>> = const { RefCell::new(None) };
    /// What the tray has handed over since this window last looked.
    static INBOX: RefCell<Inbox> = RefCell::new(Inbox::default());
}

#[derive(Default)]
struct Inbox {
    config: Option<Config>,
    status: Option<Status>,
    devices: bool,
    calendar: bool,
}

struct Window {
    hwnd: HWND,
    gfx: Gfx,
    palette: Palette,
    backend: WasapiBackend,
    model: Model,
    nav: Laid,
    page: Laid,
    pointer: Pointer,
    scroll: f32,
    /// Pixels per DIP, and the client area in pixels.
    scale: f32,
    size: (u32, u32),
    /// The pictures' clock: they move from when the window opened.
    clock: std::time::Instant,
    /// Whether [`ANIMATION_TIMER`] is running.
    animating: bool,
}

/// Open the window on `config`, or bring the open one to the front.
pub fn open(config: &Config) {
    let open = HWND_OPEN.with(Cell::get);
    if open != 0 {
        let hwnd = HWND(open as *mut _);
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        }
        return;
    }
    if let Err(e) = create(config) {
        eprintln!("settings: could not open the window ({e:#})");
    }
}

/// The config the tray last applied, changed elsewhere (an icon picked in the flyout, a token
/// Teams handed over). Ignored while a change of this window's own is on its way.
pub fn sync(config: &Config) {
    if HWND_OPEN.with(Cell::get) == 0 || PENDING.with(|p| p.borrow().is_some()) {
        return;
    }
    INBOX.with(|inbox| inbox.borrow_mut().config = Some(config.clone()));
    wake();
}

/// The hub's state moved on: the connectors' status may have.
pub fn follow(state: &crate::hub::State) {
    if HWND_OPEN.with(Cell::get) == 0 {
        return;
    }
    INBOX.with(|inbox| inbox.borrow_mut().status = Some(Status::of(state)));
    wake();
}

/// A device came, went or became the default.
pub fn devices_changed() {
    if HWND_OPEN.with(Cell::get) == 0 {
        return;
    }
    INBOX.with(|inbox| inbox.borrow_mut().devices = true);
    wake();
}

/// A calendar sign-in or fetch finished: its accounts' state may have changed.
pub fn calendar_changed() {
    if HWND_OPEN.with(Cell::get) == 0 {
        return;
    }
    INBOX.with(|inbox| inbox.borrow_mut().calendar = true);
    wake();
}

/// Show the page [`WM_OPEN_SETTINGS`]'s `wParam` names ([`OPEN_CALENDAR`] …): each panel's gear
/// opens the page about it.
pub fn show_page(code: usize) {
    let page = match code {
        OPEN_CALENDAR => Page::Calendar,
        OPEN_TASKBAR => Page::Taskbar,
        OPEN_NOTIFICATIONS => Page::Notifications,
        OPEN_BATTERIES => Page::Batteries,
        _ => return,
    };
    with_window(|w| w.go_to(page));
}

/// The config this window changed, for the tray to apply ([`WM_SETTINGS`]).
pub fn take_pending() -> Option<Config> {
    PENDING.with(|p| p.borrow_mut().take())
}

/// Close the window (the app is quitting).
pub fn destroy() {
    let open = HWND_OPEN.with(Cell::get);
    if open != 0 {
        let _ = unsafe { DestroyWindow(HWND(open as *mut _)) };
    }
}

fn wake() {
    let open = HWND_OPEN.with(Cell::get);
    if open != 0 {
        let _ = unsafe { PostMessageW(Some(HWND(open as *mut _)), WM_FOLLOW, WPARAM(0), LPARAM(0)) };
    }
}

fn with_window<R>(f: impl FnOnce(&mut Window) -> R) -> Option<R> {
    WINDOW.with(|cell| cell.try_borrow_mut().ok().and_then(|mut w| w.as_mut().map(f)))
}

fn create(config: &Config) -> Result<()> {
    let instance = unsafe { GetModuleHandleW(None) }?;
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wndproc),
        hInstance: instance.into(),
        // The app's icon is resource 1 (build.rs).
        hIcon: unsafe { LoadIconW(Some(instance.into()), PCWSTR(std::ptr::without_provenance(1))) }.unwrap_or_default(),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
        lpszClassName: CLASS,
        ..Default::default()
    };
    unsafe { RegisterClassW(&class) };
    // On the display under the pointer, in the middle of it.
    let mut cursor = POINT::default();
    let _ = unsafe { GetCursorPos(&mut cursor) };
    let (scale, work) = crate::win::monitor_at(cursor);
    let (ww, wh) = (work.right - work.left, work.bottom - work.top);
    let w = ((OPEN_SIZE.0 * scale) as i32).min(ww * 9 / 10);
    let h = ((OPEN_SIZE.1 * scale) as i32).min(wh * 9 / 10);
    let (x, y) = (work.left + (ww - w) / 2, work.top + (wh - h) / 2);
    let hwnd = unsafe {
        CreateWindowExW(WINDOW_EX_STYLE(0), CLASS, &windows::core::HSTRING::from(tr("Audio Tray Settings", "Paramètres d’Audio Tray")), WS_OVERLAPPEDWINDOW, x, y, w, h, None, None, Some(instance.into()), None)
    }
    .context("CreateWindowExW")?;

    let palette = Palette::current();
    frame_theme(hwnd, &palette);
    let backend = WasapiBackend::new().context("audio devices")?;
    let mut model = Model::new(config.clone());
    model.read_devices(&backend);
    model.monitors = crate::win::monitors();
    // The preview has no tray: its batteries are read here.
    model.status = crate::tray::hub_snapshot()
        .map(|(_, state)| Status::of(&state))
        .unwrap_or_else(|| Status { batteries: crate::batteries::read(), ..Default::default() });
    model.update = crate::update::pending_version();
    model.read_keyboards();
    model.calendar = crate::calendar::status(&config.calendar.setup());
    let mut window = Window {
        hwnd,
        gfx: Gfx::new()?,
        palette,
        backend,
        model,
        nav: Laid::default(),
        page: Laid::default(),
        pointer: Pointer::default(),
        scroll: 0.0,
        scale: dpi_scale(hwnd),
        size: client_size(hwnd),
        clock: std::time::Instant::now(),
        animating: false,
    };
    window.relayout();
    WINDOW.with(|cell| *cell.borrow_mut() = Some(window));
    HWND_OPEN.with(|h| h.set(hwnd.0 as isize));
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        // A process's first ShowWindow takes the launcher's show state instead (a shortcut set to
        // minimised, a script): the window must open restored all the same.
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}

/// The title bar in the app's theme. No Mica: the page is opaque, and a Mica title bar over it
/// shows a seam of another shade.
fn frame_theme(hwnd: HWND, palette: &Palette) {
    let dark = BOOL::from(palette.dark());
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, std::mem::size_of::<BOOL>() as u32);
    }
}

fn dpi_scale(hwnd: HWND) -> f32 {
    (unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0).max(1.0)
}

fn client_size(hwnd: HWND) -> (u32, u32) {
    let mut rect = RECT::default();
    let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rect) };
    ((rect.right - rect.left).max(0) as u32, (rect.bottom - rect.top).max(0) as u32)
}

/// Work to do once the window's state is released: it may pump messages.
type After = Box<dyn FnOnce()>;

/// Open `url` (a web page, or one of Windows' settings), once the window's state is released.
fn open_page(url: &'static str) -> Option<After> {
    Some(Box::new(move || {
        crate::win::shell_open(url, None);
    }))
}

/// The page outside Settings `button` opens, when that is all it does.
fn web_page(button: Button) -> Option<&'static str> {
    use layout::Cal;
    Some(match button {
        Button::SoundSettings => "ms-settings:sound",
        Button::QuietRules => "ms-settings:notifications",
        Button::AllowNotifications => "ms-settings:privacy-notifications",
        Button::DiscordPortal => "https://discord.com/developers/applications",
        Button::Releases => "https://github.com/michelsalib/audio-tray/releases",
        Button::Cal(Cal::GoogleApi) => "https://console.cloud.google.com/apis/library/calendar-json.googleapis.com",
        Button::Cal(Cal::GoogleCredentials) => "https://console.cloud.google.com/apis/credentials",
        Button::Cal(Cal::MicrosoftPortal) => "https://entra.microsoft.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade/quickStartType~/null/sourceType/Microsoft_AAD_IAM",
        _ => return None,
    })
}

impl Window {
    /// The client area in DIPs.
    fn dips(&self) -> (f32, f32) {
        (self.size.0 as f32 / self.scale, self.size.1 as f32 / self.scale)
    }

    fn relayout(&mut self) {
        let (w, h) = self.dips();
        self.nav = layout::nav(h);
        let gfx = &self.gfx;
        self.page = layout::page(&self.model, w - layout::NAV_W, &|text| gfx.measure(Font::Body, text));
        self.scroll = self.scroll.clamp(0.0, (self.page.height - h).max(0.0));
        self.follow_animation();
    }

    /// Run [`ANIMATION_TIMER`] while the page shows moving pictures, and only then.
    fn follow_animation(&mut self) {
        let wanted = matches!(self.model.page, Page::Home | Page::Motion);
        if wanted != self.animating {
            self.animating = wanted;
            unsafe {
                if wanted {
                    SetTimer(Some(self.hwnd), ANIMATION_TIMER, ANIMATION_MS, None);
                } else {
                    let _ = KillTimer(Some(self.hwnd), ANIMATION_TIMER);
                }
            }
        }
    }

    /// Lay the page out again and draw it.
    fn refresh(&mut self) {
        self.relayout();
        self.invalidate();
    }

    /// Show `page` from its top, with what the calendar knows read afresh (Home, the Calendar and
    /// the Notifications pages show it).
    fn go_to(&mut self, page: Page) {
        self.model.page = page;
        self.model.expanded = None;
        self.model.hint = None;
        self.stop_capture();
        if page == Page::Keyboards {
            self.model.read_keyboards();
        }
        self.model.calendar = crate::calendar::status(&self.model.config.calendar.setup());
        self.scroll = 0.0;
        self.refresh();
    }

    fn invalidate(&self) {
        let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
    }

    fn paint(&mut self) {
        let scene = paint::Scene {
            model: &self.model,
            page: &self.page,
            pointer: self.pointer,
            palette: &self.palette,
            size: self.dips(),
            scroll: self.scroll,
            time: self.clock.elapsed().as_secs_f32(),
        };
        if let Err(e) = self.gfx.frame(self.hwnd, self.size, self.scale, |pen| paint::draw(pen, &scene)) {
            eprintln!("settings: could not paint ({e:#})");
        }
    }

    /// The target under client pixel `(x, y)`.
    fn hit(&self, x: i32, y: i32) -> Option<Target> {
        let (x, y) = (x as f32 / self.scale, y as f32 / self.scale);
        if x < layout::NAV_W {
            self.nav.hit(x, y)
        } else {
            self.page.hit(x - layout::NAV_W, y + self.scroll)
        }
    }

    fn set_hover(&mut self, hover: Option<Target>) {
        if self.pointer.hover != hover {
            self.pointer.hover = hover;
            self.invalidate();
        }
    }

    /// Make a change to the config and hand it to the tray.
    fn change(&mut self, f: impl FnOnce(&mut Config)) {
        f(&mut self.model.config);
        self.model.hint = None;
        PENDING.with(|p| *p.borrow_mut() = Some(self.model.config.clone()));
        crate::tray::post(WM_SETTINGS, APPLY, 0);
        self.refresh();
    }

    /// The display to keep for display `i` with the pill on `edge`: none (automatic) while automatic
    /// would pick it anyway, so the choice follows the desktop when it changes.
    fn pin(&self, i: usize, edge: crate::config::PillEdge) -> Option<String> {
        let pill = &self.model.config.pill;
        let auto = crate::pill::display_for(edge, None, &self.model.monitors) == Some(i);
        if pill.monitor.is_none() && auto {
            return None;
        }
        self.model.monitors.get(i).map(|m| m.name.clone())
    }

    /// Show `hint` in place of `button`'s value.
    fn hint(&mut self, button: Button, hint: &'static str) {
        self.say(button, hint, model::Tone::Warn);
    }

    /// Show `text` in place of `button`'s value, in `tone`.
    fn say(&mut self, button: Button, text: &'static str, tone: model::Tone) {
        self.model.hint = Some((button, text, tone));
        self.refresh();
    }

    /// Act on a click on `target`; what comes back runs once the state is released.
    fn activate(&mut self, target: Target) -> Option<After> {
        // A click anywhere else keeps the name being typed, as a field losing the focus does.
        if self.model.renaming.is_some() && !matches!(target, Target::Button(Button::SaveName | Button::CancelRename)) {
            self.save_name();
        }
        match target {
            Target::Nav(page) => return self.navigate(page),
            Target::Toggle(toggle) => self.flip(toggle)?,
            Target::Choice(Choice::KeyboardLayout(k), i) => {
                let hkl = self.model.layouts.get(i)?.0;
                self.set_layout(k, hkl);
            }
            Target::Choice(choice, i) => {
                let monitors: Vec<String> = self.model.monitors.iter().map(|m| m.name.clone()).collect();
                self.change(|c| choice.apply(i, c, &monitors));
            }
            Target::Slot(i, edge, align) => {
                let name = self.pin(i, edge);
                self.change(|c| {
                    c.pill.monitor = name;
                    c.pill.edge = edge;
                    c.pill.align = align;
                });
            }
            Target::Monitor(i) => {
                let name = self.pin(i, self.model.config.pill.edge);
                self.change(|c| c.pill.monitor = name);
            }
            Target::Swatch(i, j) => {
                let key = self.model.calendar.calendars.get(i)?.key.clone();
                self.change(|c| match j.checked_sub(1).and_then(|j| layout::SWATCHES.get(j)) {
                    Some([r, g, b]) => {
                        c.calendar.colors.insert(key, format!("#{r:02X}{g:02X}{b:02X}"));
                    }
                    None => {
                        c.calendar.colors.remove(&key);
                    }
                });
            }
            Target::Connector(connector) => {
                self.model.open = if self.model.open == Some(connector) { None } else { Some(connector) };
                self.model.hint = None;
                self.refresh();
            }
            Target::Device(i) => {
                self.model.expanded = if self.model.expanded == Some(i) { None } else { Some(i) };
                self.refresh();
            }
            Target::Icon(i, icon) => {
                let id = self.model.devices.get(i).map(|d| d.id.0.clone())?;
                self.model.devices[i].icon = icon;
                self.model.expanded = None;
                self.change(|c| c.set_icon(id, icon));
            }
            Target::Button(button) => return self.press(button),
        }
        None
    }

    /// A click on `page` in the navigation.
    fn navigate(&mut self, page: Page) -> Option<After> {
        if self.model.page != page {
            self.go_to(page);
        }
        if page != Page::Connectors {
            return None;
        }
        self.model.mpc = crate::integrations::mpc::shares_media();
        self.model.edge_sleep = crate::integrations::edge::players_at_risk();
        self.refresh();
        // A redirect added since the last failure: try again now, not at the next call.
        Some(Box::new(|| {
            crate::integrations::discord_rpc::retry();
            crate::integrations::poke_teams();
        }))
    }

    /// Flip a switch; `None` when it names a calendar or a site no longer there.
    fn flip(&mut self, toggle: Toggle) -> Option<()> {
        match toggle {
            Toggle::CalendarShown(i) => {
                let key = self.model.calendar.calendars.get(i)?.key.clone();
                self.change(|c| {
                    if c.calendar.hidden.contains(&key) {
                        c.calendar.hidden.retain(|k| *k != key);
                    } else {
                        c.calendar.hidden.push(key);
                    }
                });
            }
            Toggle::Site(i) => {
                let (key, _, _) = crate::integrations::browser::SITES.get(i)?;
                self.change(|c| {
                    if c.browser.sites.iter().any(|s| s == key) {
                        c.browser.sites.retain(|s| s != key);
                    } else {
                        c.browser.sites.push(key.to_string());
                    }
                });
            }
            _ => self.change(|c| {
                if let Some(flag) = toggle.flag(c) {
                    *flag = !*flag;
                }
            }),
        }
        Some(())
    }

    fn press(&mut self, button: Button) -> Option<After> {
        if let Some(url) = web_page(button) {
            return open_page(url);
        }
        match button {
            Button::Mpc => match crate::integrations::mpc::share_media() {
                Ok(()) => {
                    self.model.mpc = Some(true);
                    self.say(Button::Mpc, tr("On from MPC-HC's next start", "Actif au prochain lancement de MPC-HC"), model::Tone::Good);
                }
                Err(hint) => self.hint(Button::Mpc, hint),
            },
            // The bell's cross, the panel's moon and this page follow (Windows says it changed).
            Button::Quiet => return Some(Box::new(|| {
                crate::calendar::focus::toggle();
                crate::calendar::quiet_changed();
            })),
            // System and performance, where the "Never put these sites to sleep" list is.
            Button::EdgeSleep => return Some(Box::new(|| {
                crate::win::shell_open("msedge.exe", Some("edge://settings/system"));
            })),
            Button::ExtensionFolder => {
                // The browser's page (for Developer mode and "Load unpacked"), then the folder over it
                // with its path copied for that dialog.
                return Some(Box::new(|| {
                    crate::integrations::browser::open_extensions_page();
                    if !crate::integrations::browser::show_extension_dir() {
                        with_window(|w| w.hint(Button::ExtensionFolder, tr("The extension's folder was not found next to Audio Tray", "Le dossier de l’extension est introuvable à côté d’Audio Tray")));
                    }
                }));
            }
            Button::DiscordRedirect => {
                let copied = crate::win::set_clipboard_text(crate::integrations::discord_rpc::REDIRECT);
                match copied {
                    true => self.say(button, tr("Copied: paste it in OAuth2 > Redirects", "Copié : collez-le dans OAuth2 > Redirects"), model::Tone::Good),
                    false => self.hint(button, tr("The clipboard is busy: copy again", "Presse-papiers occupé : copiez à nouveau")),
                }
            }
            Button::DiscordId | Button::DiscordSecret => match crate::win::clipboard_text() {
                Err(()) => self.hint(button, tr("The clipboard is busy: paste again", "Presse-papiers occupé : collez à nouveau")),
                Ok(text) => {
                    let text = text.map(|t| t.trim().to_string()).unwrap_or_default();
                    match model::paste_key(button, &text) {
                        Ok(()) if button == Button::DiscordId => self.change(|c| c.discord.client_id = Some(text)),
                        Ok(()) => self.change(|c| c.discord.client_secret = Some(text)),
                        Err(hint) => self.hint(button, hint),
                    }
                }
            },
            Button::Cal(cal) => return self.press_calendar(cal),
            Button::Restart => {
                crate::tray::post(WM_SETTINGS, RESTART, 0);
            }
            Button::Quit => {
                crate::tray::post(WM_SETTINGS, QUIT, 0);
            }
            Button::ConfigFolder => {
                let dir = Config::path().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
                return Some(Box::new(move || {
                    if let Some(dir) = dir {
                        let _ = std::fs::create_dir_all(&dir);
                        crate::win::shell_open(&dir.to_string_lossy(), None);
                    }
                }));
            }
            Button::AddKeyboard
            | Button::AddConnection(_)
            | Button::CancelCapture
            | Button::RenameKeyboard(_)
            | Button::SaveName
            | Button::CancelRename
            | Button::RemoveKeyboard(_)
            | Button::RemoveConnection(..) => self.press_keyboard(button)?,
            // Opened above.
            Button::SoundSettings | Button::QuietRules | Button::AllowNotifications | Button::DiscordPortal | Button::Releases => {}
        }
        None
    }

    /// The keyboards' buttons; `None` when the keyboard is no longer there.
    fn press_keyboard(&mut self, button: Button) -> Option<()> {
        match button {
            Button::AddKeyboard | Button::AddConnection(_) => {
                crate::keyboards::capture(self.hwnd, WM_KEYBOARD_CAPTURED);
                self.model.capture = Some(None);
                self.model.capture_into = match button {
                    Button::AddConnection(i) => Some(i),
                    _ => None,
                };
                self.model.read_keyboards();
                self.refresh();
            }
            Button::CancelCapture => {
                self.stop_capture();
                self.refresh();
            }
            Button::RenameKeyboard(i) => {
                let name = self.model.config.keyboards.assigned.get(i)?.name.clone();
                self.model.renaming = Some((i, name));
                self.refresh();
            }
            // Saved by any click ([`Window::activate`]).
            Button::SaveName => self.save_name(),
            Button::CancelRename => {
                self.model.renaming = None;
                self.refresh();
            }
            Button::RemoveKeyboard(i) => {
                self.stop_capture();
                self.change(|c| {
                    if i < c.keyboards.assigned.len() {
                        c.keyboards.assigned.remove(i);
                    }
                })
            }
            Button::RemoveConnection(i, j) => self.change(|c| {
                if let Some(keyboard) = c.keyboards.assigned.get_mut(i) {
                    keyboard.remove_path(j);
                }
            }),
            _ => {}
        }
        Some(())
    }

    /// The calendar connector's buttons.
    fn press_calendar(&mut self, cal: layout::Cal) -> Option<After> {
        use layout::Cal;
        use model::CalendarKey;
        let button = Button::Cal(cal);
        match cal {
            Cal::MicrosoftRedirect => match crate::win::set_clipboard_text("http://localhost") {
                true => self.say(button, tr("Copied: add it under Mobile and desktop applications", "Copié : ajoutez-le sous Applications mobiles et de bureau"), model::Tone::Good),
                false => self.hint(button, tr("The clipboard is busy: copy again", "Presse-papiers occupé : copiez à nouveau")),
            },
            Cal::GoogleId | Cal::GoogleSecret | Cal::MicrosoftId | Cal::AddLink => {
                let key = match cal {
                    Cal::GoogleId => CalendarKey::GoogleId,
                    Cal::GoogleSecret => CalendarKey::GoogleSecret,
                    Cal::MicrosoftId => CalendarKey::MicrosoftId,
                    _ => CalendarKey::Link,
                };
                let text = match crate::win::clipboard_text() {
                    Err(()) => {
                        self.hint(button, tr("The clipboard is busy: paste again", "Presse-papiers occupé : collez à nouveau"));
                        return None;
                    }
                    Ok(text) => text.unwrap_or_default(),
                };
                match model::paste_calendar(key, &text) {
                    Err(hint) => self.hint(button, hint),
                    Ok(value) => match key {
                        CalendarKey::GoogleId => self.change(|c| c.calendar.google_client_id = Some(value)),
                        CalendarKey::GoogleSecret => self.change(|c| c.calendar.google_client_secret = Some(value)),
                        CalendarKey::MicrosoftId => self.change(|c| c.calendar.microsoft_client_id = Some(value)),
                        CalendarKey::Link => {
                            if self.model.config.calendar.links.iter().any(|l| l.url == value) {
                                self.hint(button, tr("That calendar is already here", "Cet agenda est déjà là"));
                            } else {
                                let name = crate::calendar::link_name(&value);
                                self.change(|c| c.calendar.links.push(crate::calendar::Link { name, url: value }));
                            }
                        }
                    },
                }
            }
            Cal::RemoveLink(i) => self.change(|c| {
                if i < c.calendar.links.len() {
                    c.calendar.links.remove(i);
                }
            }),
            Cal::SignIn(provider) => {
                let setup = self.model.config.calendar.setup();
                // The browser comes up: started once the window's state is released.
                return Some(Box::new(move || crate::calendar::sign_in(provider, &setup)));
            }
            Cal::CancelSignIn(provider) => crate::calendar::cancel_sign_in(provider),
            Cal::SignOut(provider) => crate::calendar::sign_out(provider),
            Cal::EditClocks => return Some(Box::new(crate::calendar::zones::edit)),
            // Opened by [`Window::press`].
            Cal::GoogleApi | Cal::GoogleCredentials | Cal::MicrosoftPortal => {}
        }
        None
    }

    fn set_layout(&mut self, keyboard: usize, hkl: u64) {
        self.change(|c| {
            if let Some(entry) = c.keyboards.assigned.get_mut(keyboard) {
                entry.hkl = format!("{hkl:08X}");
            }
        });
    }

    /// Keep the name being typed (an empty one keeps the old name).
    fn save_name(&mut self) {
        let Some((i, name)) = self.model.renaming.take() else {
            return;
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            self.refresh();
            return;
        }
        self.change(|c| {
            if let Some(entry) = c.keyboards.assigned.get_mut(i) {
                entry.name = name;
            }
        });
    }

    fn stop_capture(&mut self) {
        if self.model.capture.take().is_some() {
            crate::keyboards::cancel_capture();
        }
        self.model.capture_into = None;
    }

    /// A character typed into the window: the name being typed takes it.
    fn type_char(&mut self, unit: u16) -> bool {
        if self.model.renaming.is_none() {
            return false;
        }
        match unit {
            0x0D => {
                self.save_name();
                return true;
            }
            0x1B => self.model.renaming = None,
            // Other controls, and halves of a surrogate pair (no emoji in a keyboard's name).
            0x00..=0x07 | 0x09..=0x1F | 0x7F | 0xD800..=0xDFFF => return true,
            _ => {
                let Some((_, text)) = self.model.renaming.as_mut() else {
                    return false;
                };
                if unit == 0x08 {
                    text.pop();
                } else if text.chars().count() < NAME_MAX {
                    text.extend(char::from_u32(u32::from(unit)));
                }
            }
        }
        self.refresh();
        true
    }

    /// The switcher's answer to a capture: add the keyboard, or say it is already there.
    fn captured(&mut self) {
        let Some(captured) = crate::keyboards::take_captured() else {
            return;
        };
        if self.model.capture.is_none() {
            return;
        }
        let into = self.model.capture_into;
        if let Some(name) = captured.assigned {
            let mine = into.and_then(|i| self.model.config.keyboards.assigned.get(i)).is_some_and(|k| k.has(&captured.path));
            let hint = if mine {
                tf!("That connection is already part of \"{name}\": connect it another way, then press a key", "Cette connexion fait déjà partie de « {name} » : branchez le clavier autrement, puis appuyez sur une touche")
            } else {
                tf!("That one is already here, as \"{name}\": press a key on another keyboard", "Celui-ci est déjà là, sous le nom « {name} » : appuyez sur une touche d’un autre clavier")
            };
            self.model.capture = Some(Some(hint));
            self.refresh();
            return;
        }
        self.model.capture = None;
        self.model.capture_into = None;
        self.model.read_keyboards();
        if let Some(i) = into {
            self.change(|c| {
                if let Some(keyboard) = c.keyboards.assigned.get_mut(i) {
                    keyboard.more_paths.push(captured.path);
                }
            });
            return;
        }
        let taken: Vec<String> = self.model.config.keyboards.assigned.iter().map(|k| k.name.clone()).collect();
        let name = crate::keyboards::default_name(&captured.path, &captured.names, &taken);
        let hkl = self.model.unused_layout().map(|hkl| format!("{hkl:08X}")).unwrap_or_default();
        self.change(|c| c.keyboards.assigned.push(crate::config::AssignedKeyboard { device_path: captured.path, name, hkl, ..Default::default() }));
    }

    /// Read what the tray handed over.
    fn follow(&mut self) {
        let inbox = INBOX.with(|inbox| std::mem::take(&mut *inbox.borrow_mut()));
        // Only the hub's same status again (it moves on for every song): nothing to draw. An empty
        // inbox is a paint that was missed, and draws.
        if inbox.config.is_none() && !inbox.devices && !inbox.calendar && inbox.status.as_ref() == Some(&self.model.status) {
            return;
        }
        if let Some(config) = inbox.config {
            if PENDING.with(|p| p.borrow().is_none()) {
                self.model.config = config;
            }
        }
        if let Some(status) = inbox.status {
            self.model.status = status;
        }
        if inbox.devices {
            self.model.read_devices(&self.backend);
        }
        if inbox.calendar {
            self.model.calendar = crate::calendar::status(&self.model.config.calendar.setup());
        }
        self.model.update = crate::update::pending_version();
        self.refresh();
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let handled = std::panic::catch_unwind(|| handle(hwnd, msg, wparam, lparam)).unwrap_or_else(|_| {
        eprintln!("settings: a message handler panicked");
        None
    });
    match handled {
        Some(result) => result,
        None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn handle(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let point = || ((lparam.0 & 0xFFFF) as i16 as i32, ((lparam.0 >> 16) & 0xFFFF) as i16 as i32);
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            unsafe { BeginPaint(hwnd, &mut ps) };
            if with_window(Window::paint).is_none() {
                // Busy further up the stack: paint once it is done.
                wake();
            }
            let _ = unsafe { EndPaint(hwnd, &ps) };
            Some(LRESULT(0))
        }
        WM_ERASEBKGND => Some(LRESULT(1)),
        WM_TIMER if wparam.0 == ANIMATION_TIMER => {
            with_window(|w| w.invalidate());
            Some(LRESULT(0))
        }
        WM_SIZE => {
            with_window(|w| {
                w.size = ((lparam.0 & 0xFFFF) as u32, ((lparam.0 >> 16) & 0xFFFF) as u32);
                w.gfx.resize(w.size.0, w.size.1);
                w.refresh();
            });
            Some(LRESULT(0))
        }
        WM_DPICHANGED => {
            with_window(|w| w.scale = ((wparam.0 & 0xFFFF) as f32 / 96.0).max(1.0));
            // Moved to the size Windows suggests, which sends WM_SIZE: outside the borrow.
            let r = unsafe { *(lparam.0 as *const RECT) };
            let _ = unsafe { SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE) };
            with_window(|w| {
                w.refresh();
            });
            Some(LRESULT(0))
        }
        WM_GETMINMAXINFO => {
            let scale = dpi_scale(hwnd);
            let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            info.ptMinTrackSize = POINT { x: (MIN_SIZE.0 * scale) as i32, y: (MIN_SIZE.1 * scale) as i32 };
            Some(LRESULT(0))
        }
        WM_MOUSEMOVE => {
            let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            let _ = unsafe { TrackMouseEvent(&mut track) };
            let (x, y) = point();
            with_window(|w| {
                let hover = w.hit(x, y);
                w.set_hover(hover);
            });
            Some(LRESULT(0))
        }
        WM_MOUSELEAVE => {
            with_window(|w| w.set_hover(None));
            Some(LRESULT(0))
        }
        WM_LBUTTONDOWN => {
            let (x, y) = point();
            with_window(|w| {
                w.pointer.pressed = w.hit(x, y);
                w.invalidate();
            });
            unsafe { SetCapture(hwnd) };
            Some(LRESULT(0))
        }
        WM_LBUTTONUP => {
            let _ = unsafe { ReleaseCapture() };
            let (x, y) = point();
            let after = with_window(|w| {
                let pressed = w.pointer.pressed.take();
                let hit = w.hit(x, y);
                w.pointer.hover = hit;
                w.invalidate();
                match (pressed, hit) {
                    (Some(a), Some(b)) if a == b => w.activate(a),
                    _ => None,
                }
            })
            .flatten();
            if let Some(after) = after {
                after();
            }
            Some(LRESULT(0))
        }
        WM_MOUSEWHEEL => {
            let notches = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32 / WHEEL_DELTA as f32;
            with_window(|w| {
                let (_, h) = w.dips();
                let before = w.scroll;
                w.scroll = (w.scroll - notches * SCROLL_STEP).clamp(0.0, (w.page.height - h).max(0.0));
                if w.scroll != before {
                    w.invalidate();
                }
            });
            Some(LRESULT(0))
        }
        WM_SETTINGCHANGE => {
            // A theme or accent change: draw in the new one.
            with_window(|w| {
                w.palette = Palette::current();
                frame_theme(w.hwnd, &w.palette);
                w.invalidate();
            });
            None
        }
        WM_DISPLAYCHANGE => {
            with_window(|w| {
                w.model.monitors = crate::win::monitors();
                w.refresh();
            });
            None
        }
        // Back from another window (Edge's settings, MPC-HC): its state may have changed there.
        WM_ACTIVATE if wparam.0 & 0xFFFF != 0 => {
            with_window(|w| {
                let edge_sleep = crate::integrations::edge::players_at_risk();
                let mpc = crate::integrations::mpc::shares_media();
                if (&edge_sleep, mpc) != (&w.model.edge_sleep, w.model.mpc) {
                    w.model.edge_sleep = edge_sleep;
                    w.model.mpc = mpc;
                    w.refresh();
                }
            });
            None
        }
        WM_FOLLOW => {
            with_window(Window::follow);
            Some(LRESULT(0))
        }
        WM_KEYBOARD_CAPTURED => {
            with_window(Window::captured);
            Some(LRESULT(0))
        }
        WM_CHAR => with_window(|w| w.type_char(wparam.0 as u16)).filter(|typed| *typed).map(|_| LRESULT(0)),
        WM_DESTROY => {
            crate::keyboards::cancel_capture();
            HWND_OPEN.with(|h| h.set(0));
            WINDOW.with(|cell| {
                if let Ok(mut window) = cell.try_borrow_mut() {
                    *window = None;
                }
            });
            None
        }
        _ => None,
    }
}

/// Dev preview: the window alone, opened on the page whose title holds `page`, until it is closed.
#[cfg(feature = "dev")]
pub fn preview(config: &Config, page: &str) {
    open(config);
    with_window(|w| {
        // Its English or its French title.
        let named = |title: &str| title.to_lowercase().contains(&page.to_lowercase());
        if let Some(page) = Page::ALL.into_iter().find(|p| named(p.titles().0) || named(p.titles().1)) {
            w.go_to(page);
        }
    });
    let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
    while HWND_OPEN.with(Cell::get) != 0 && crate::wait::get_message(&mut msg) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
            windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
        }
    }
}

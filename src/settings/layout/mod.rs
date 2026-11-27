//! Pure layout for the settings window: each page as [`Item`]s to paint and [`Target`]s to hit, in
//! DIPs. No Win32 here, so it stays unit-testable; text is measured through a function the caller
//! passes in.
//!
//! The types and the navigation are here; [`column`] holds the building blocks a page is laid out
//! with, [`choice`] each choice's options, and [`pages`] one module per page.

mod choice;
mod column;
mod pages;

use crate::config::{Config, MiddleClick, MotionStyle, PillSize, TileStyle, WaveColour};
use crate::icons::IconId;
use crate::lang::tr;

use super::model::{Model, Tone};
use column::Column;

pub(super) use choice::{Choice, SWATCHES};
pub(super) use pages::pill::{align_labels, map_rects, slot_rect, ALIGNS, EDGES};

/// The navigation pane's width; the page fills the rest.
pub(super) const NAV_W: f32 = 280.0;
const NAV_TOP: f32 = 12.0;
const NAV_ITEM_H: f32 = 36.0;
const NAV_STEP: f32 = 40.0;
const NAV_INSET: f32 = 8.0;
/// The page's side padding, and the widest its cards grow.
const PAGE_PAD: f32 = 36.0;
const PAGE_MAX_W: f32 = 1000.0;
const TITLE_H: f32 = 72.0;
const SECTION_H: f32 = 44.0;
const CARD_H: f32 = 68.0;
/// A row inside a grouped card.
const ROW_H: f32 = 64.0;
const CARD_GAP: f32 = 4.0;
/// A card's text starts after its glyph; a nested card's at the same place, without one.
pub(super) const CARD_TEXT_X: f32 = 56.0;
pub(super) const CARD_PAD: f32 = 16.0;
const CONTROL_H: f32 = 32.0;
pub(super) const TOGGLE_W: f32 = 40.0;
pub(super) const TOGGLE_H: f32 = 20.0;
/// "On" / "Off" beside a switch ("Activé" / "Désactivé" in French, wider).
pub(super) fn toggle_label_w() -> f32 {
    if crate::lang::french() { 64.0 } else { 36.0 }
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub(super) struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl R {
    pub(super) const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        R { x, y, w, h }
    }

    pub(super) fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub(super) fn right(&self) -> f32 {
        self.x + self.w
    }

    pub(super) fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Page {
    /// What Audio Tray is doing, at a glance, with the switches most changed.
    Home,
    Taskbar,
    /// The calendar the clock opens: its accounts and links, how it looks, its reminders.
    Calendar,
    /// The notifications the bell opens, and keeping them quiet.
    Notifications,
    Pill,
    /// How the bars move: the pill's ring, the card's bar, the volume sliders.
    Motion,
    /// The audio devices.
    Devices,
    /// The battery levels of the other devices.
    Batteries,
    Keyboards,
    Connectors,
    About,
}

impl Page {
    pub(super) const ALL: [Page; 11] =
        [Page::Home, Page::Taskbar, Page::Calendar, Page::Notifications, Page::Pill, Page::Motion, Page::Devices, Page::Batteries, Page::Keyboards, Page::Connectors, Page::About];

    pub(super) fn title(self) -> &'static str {
        let (en, fr) = self.titles();
        tr(en, fr)
    }

    /// Its title in English and in French.
    pub(super) fn titles(self) -> (&'static str, &'static str) {
        match self {
            Page::Home => ("Home", "Accueil"),
            Page::Taskbar => ("Taskbar", "Barre des tâches"),
            Page::Calendar => ("Calendar", "Agenda"),
            Page::Notifications => ("Notifications", "Notifications"),
            Page::Pill => ("Edge pill", "Pastille"),
            Page::Motion => ("Motion", "Animations"),
            Page::Devices => ("Audio devices", "Périphériques audio"),
            Page::Batteries => ("Batteries", "Batteries"),
            Page::Keyboards => ("Keyboards", "Claviers"),
            Page::Connectors => ("Connectors", "Connecteurs"),
            Page::About => ("About", "À propos"),
        }
    }

    pub(super) fn glyph(self) -> char {
        match self {
            Page::Home => '\u{E80F}',
            Page::Taskbar => '\u{E75B}',
            Page::Calendar => '\u{E787}',
            Page::Notifications => '\u{EA8F}',
            Page::Pill => '\u{E7F4}',
            Page::Motion => '\u{E771}',
            Page::Devices => '\u{E767}',
            Page::Batteries => '\u{E83F}',
            Page::Keyboards => '\u{E765}',
            Page::Connectors => '\u{E71B}',
            Page::About => '\u{E946}',
        }
    }

    /// The heading a page sits under in the navigation, starting a group; `None` continues the
    /// one above (Home and About stand alone).
    fn heading(self) -> Option<&'static str> {
        match self {
            Page::Taskbar => Some(tr("Where Audio Tray shows", "Où Audio Tray s’affiche")),
            Page::Devices => Some(tr("What Audio Tray follows", "Ce qu’Audio Tray suit")),
            _ => None,
        }
    }
}

/// A switch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Toggle {
    /// Follow MPC-HC's session.
    Mpc,
    Strip,
    /// Win+Ctrl+V opens the audio panel.
    StripShortcut,
    /// Move the buttons next to Wi-Fi.
    NextToWifi,
    HideVolume,
    HideMic,
    HideLanguage,
    /// Switch the layout by keyboard.
    Keyboards,
    /// Show each layout change, in a small banner.
    KeyboardReadout,
    Music,
    Pill,
    AnnounceTracks,
    ListeningAlerts,
    /// Follow battery levels.
    Batteries,
    BatteryTaskbarIcon,
    BatteryFlyout,
    BatteryTooltip,
    BatteryToast,
    Browser,
    /// Follow site `i` of [`crate::integrations::browser::SITES`] when not every site is.
    Site(usize),
    Discord,
    Teams,
    /// The calendar replaces the taskbar clock's.
    Calendar,
    ClockSeconds,
    ClockWeekday,
    ClockOneLine,
    ClockHideDate,
    ClockUnread,
    WeekNumbers,
    WorldClocks,
    /// The bell opens Audio Tray's notifications.
    Bell,
    /// Audio Tray shows every notification (Windows' banners off).
    TakeOver,
    CalendarShortcut,
    QuietMeetings,
    QuietCalls,
    /// Calendar `i` of [`super::model::Model::calendar`]'s list shown.
    CalendarShown(usize),
}

/// Each switch's flag in the config, named once: [`Toggle::flag`] flips it, [`Toggle::get`] reads it.
macro_rules! switches {
    ($($toggle:ident => $($field:ident).+,)*) => {
        impl Toggle {
            /// The flag it flips; `None` for a switch over a list ([`Toggle::Site`],
            /// [`Toggle::CalendarShown`]), which the window edits itself.
            pub(super) fn flag(self, c: &mut Config) -> Option<&mut bool> {
                match self {
                    $(Toggle::$toggle => Some(&mut c.$($field).+),)*
                    Toggle::Site(_) | Toggle::CalendarShown(_) => None,
                }
            }

            /// Whether it is on. A site is while it is listed; a calendar's switch reads the
            /// calendars found, which the config alone does not know: off here.
            pub(super) fn get(self, c: &Config) -> bool {
                match self {
                    $(Toggle::$toggle => c.$($field).+,)*
                    Toggle::Site(i) => crate::integrations::browser::SITES.get(i).is_some_and(|(key, _, _)| c.browser.sites.iter().any(|s| s == key)),
                    Toggle::CalendarShown(_) => false,
                }
            }
        }
    };
}

switches! {
    Strip => strip.enabled,
    StripShortcut => strip.shortcut,
    NextToWifi => strip.next_to_wifi,
    HideVolume => strip.hide_windows_volume,
    HideMic => strip.hide_windows_mic,
    HideLanguage => strip.hide_windows_language,
    Keyboards => keyboards.enabled,
    KeyboardReadout => keyboards.show_readout,
    Music => music.enabled,
    Pill => pill.enabled,
    AnnounceTracks => pill.announce_tracks,
    ListeningAlerts => pill.listening_alerts,
    Browser => browser.enabled,
    Mpc => mpc.enabled,
    Discord => discord.enabled,
    Teams => teams.enabled,
    Calendar => calendar.enabled,
    ClockSeconds => clock.seconds,
    ClockWeekday => clock.weekday,
    ClockOneLine => clock.one_line,
    ClockHideDate => clock.hide_date,
    ClockUnread => clock.unread,
    WeekNumbers => calendar.week_numbers,
    WorldClocks => calendar.world_clocks,
    Bell => calendar.bell,
    TakeOver => calendar.take_over,
    CalendarShortcut => calendar.shortcut,
    QuietMeetings => calendar.quiet_in_meetings,
    QuietCalls => calendar.quiet_in_calls,
    Batteries => batteries.enabled,
    BatteryTaskbarIcon => batteries.taskbar_icon,
    BatteryFlyout => batteries.flyout,
    BatteryTooltip => batteries.tooltip,
    BatteryToast => batteries.toast,
}

/// The calendar connector's buttons.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Cal {
    /// Google Cloud's console, where the Calendar API is enabled.
    GoogleApi,
    /// Its credentials page, where the OAuth client is made.
    GoogleCredentials,
    GoogleId,
    GoogleSecret,
    /// Entra's app registrations.
    MicrosoftPortal,
    /// Copy the redirect the registration needs.
    MicrosoftRedirect,
    MicrosoftId,
    SignIn(crate::calendar::Provider),
    CancelSignIn(crate::calendar::Provider),
    SignOut(crate::calendar::Provider),
    AddLink,
    RemoveLink(usize),
    /// Windows' Date and time settings, where the world clocks are kept.
    EditClocks,
}

/// A button (or a whole card that acts like one).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Button {
    /// Edge's settings page, where sites are kept awake.
    EdgeSleep,
    /// Turn MPC-HC's media controls on.
    Mpc,
    SoundSettings,
    ExtensionFolder,
    DiscordPortal,
    /// Copy the redirect step 2 asks for.
    DiscordRedirect,
    DiscordId,
    DiscordSecret,
    Cal(Cal),
    /// Switch Do not disturb on or off now.
    Quiet,
    /// Windows' notification settings: its automatic Do not disturb rules.
    QuietRules,
    /// Windows' page where Audio Tray is allowed to read notifications.
    AllowNotifications,
    Restart,
    Releases,
    ConfigFolder,
    Quit,
    /// Take the next keyboard typed on.
    AddKeyboard,
    CancelCapture,
    RenameKeyboard(usize),
    SaveName,
    CancelRename,
    RemoveKeyboard(usize),
    /// Take the next keyboard typed on as another connection of this one.
    AddConnection(usize),
    /// Forget a keyboard's connection: `(keyboard, connection)`.
    RemoveConnection(usize, usize),
}

/// What a click lands on.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Target {
    Nav(Page),
    Toggle(Toggle),
    Choice(Choice, usize),
    Button(Button),
    /// A device's card: show or hide its icons.
    Device(usize),
    /// An icon for a device.
    Icon(usize, IconId),
    /// A connector's card: open or close it.
    Connector(Connector),
    /// A place on a display, in the pill's map.
    Slot(usize, crate::config::PillEdge, crate::config::PillAlign),
    /// A display in the pill's map.
    Monitor(usize),
    /// Colour `j` of [`SWATCHES`] (0: the calendar's own) for calendar `i`.
    Swatch(usize, usize),
}

/// What to paint. Controls are separate items inside their card's rectangle.
#[derive(Clone, PartialEq, Debug)]
pub(super) enum Kind {
    Title(&'static str),
    Section(&'static str),
    Card { glyph: Option<char>, title: String, desc: Option<(String, Tone)>, dim: bool, target: Option<Target> },
    /// The trailing glyph of a card that opens something.
    Trailing(char),
    Toggle { on: bool, dim: bool, toggle: Toggle },
    Segment { label: String, selected: bool, first: bool, last: bool, dim: bool, target: Target },
    /// Dimmed, it is moot, as its row is: drawn faded, and not a target.
    Button { label: String, accent: bool, glyph: Option<char>, dim: bool, target: Target },
    /// Rows inside one card: a hairline between two.
    Divider,
    /// A setup step's number in a ring, or a tick once it is done.
    Badge { n: u8, done: bool },
    /// A row's title and second line, inside a grouped card.
    Label { title: String, desc: Option<(String, Tone)>, dim: bool },
    /// Secondary text, right-aligned in its box.
    Value(String),
    /// The displays, drawn to scale, with the pill's places on them.
    Map { dim: bool },
    Chip { icon: IconId, selected: bool, target: Target },
    /// A device's icon, in its card's glyph column (the earbuds are drawn, not a font's).
    Icon(IconId),
    /// A text being typed, with the caret at its end.
    Field(String),
    /// A choice shown as a picture of what it does, with a radio and its name under it.
    Tile { picture: Picture, title: &'static str, desc: &'static str, selected: bool, dim: bool, target: Target },
    /// A tile on Home: a part of Audio Tray, its state in a few words, and the page it opens.
    Glance { title: &'static str, head: String, line: String, tone: Tone, target: Target },
    /// A line of secondary text.
    Text(String),
    /// An accent outline around the card that is open.
    Outline,
    /// A battery's level: a bar and the number, amber when low.
    Level { level: u8, low: bool },
    /// One choice among many, as a rounded chip that wraps with the others.
    Pick { label: String, selected: bool, dim: bool, target: Target },
    /// The app's logo, as its icon draws it.
    Logo,
    /// A colour to pick, as a dot; ringed when it is the one.
    Swatch { rgb: [u8; 3], selected: bool, target: Target },
}

/// What a [`Kind::Tile`] draws.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Picture {
    Size(PillSize),
    Ring(MotionStyle),
    Bar(MotionStyle),
    Volume(MotionStyle),
    Colour(WaveColour),
    /// What a middle click on a taskbar audio button does.
    Click(MiddleClick),
    /// What a middle click on the bell does.
    Bell(crate::config::BellMiddleClick),
    /// What the music player's taskbar button shows.
    PlayerButton(TileStyle),
}

#[derive(Clone, PartialEq, Debug)]
pub(super) struct Item {
    pub r: R,
    pub kind: Kind,
}

/// A laid-out page: what to paint, what to hit (topmost last), and how tall it is.
#[derive(Default)]
pub(super) struct Laid {
    pub items: Vec<Item>,
    pub hits: Vec<(R, Target)>,
    pub height: f32,
}

impl Laid {
    /// The topmost target at `(x, y)`.
    pub(super) fn hit(&self, x: f32, y: f32) -> Option<Target> {
        self.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map(|(_, t)| *t)
    }

    fn push(&mut self, r: R, kind: Kind) {
        self.items.push(Item { r, kind });
    }
}

/// A connector, on its own card in Settings > Connectors.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Connector {
    Browser,
    Mpc,
    Discord,
    Teams,
}

/// The navigation's top: the app's name and version above the pages.
pub(super) const NAV_HEADER_H: f32 = 64.0;
/// A group's heading in the navigation.
const NAV_HEADING_H: f32 = 32.0;

/// A group's heading in the navigation, and where it goes.
pub(super) type NavHeading = (&'static str, R);

/// What the navigation pane shows, in a window `height` DIPs tall: each group's heading and each
/// page's item. About sits at the bottom, apart, when there is room.
pub(super) fn nav_entries(height: f32) -> (Vec<NavHeading>, Vec<(Page, R)>) {
    let mut headings = Vec::new();
    let mut pages = Vec::new();
    let mut y = NAV_TOP + NAV_HEADER_H;
    let item = |y: f32| R::new(NAV_INSET, y, NAV_W - 2.0 * NAV_INSET, NAV_ITEM_H);
    for page in Page::ALL {
        if page == Page::About {
            // At the bottom, unless the window is too short for it to clear the others.
            let bottom = height - NAV_TOP - NAV_ITEM_H;
            pages.push((page, item(bottom.max(y + 12.0))));
            continue;
        }
        if let Some(heading) = page.heading() {
            y += 8.0;
            headings.push((heading, R::new(NAV_INSET + 12.0, y, NAV_W - 2.0 * NAV_INSET - 12.0, NAV_HEADING_H)));
            y += NAV_HEADING_H;
        }
        pages.push((page, item(y)));
        y += NAV_STEP;
    }
    (headings, pages)
}

/// Where the app's name and version go, above the pages.
pub(super) fn nav_header() -> R {
    R::new(NAV_INSET + 8.0, NAV_TOP, NAV_W - 2.0 * NAV_INSET - 8.0, NAV_HEADER_H)
}

/// The navigation pane, fixed (it does not scroll with the page).
pub(super) fn nav(height: f32) -> Laid {
    let mut laid = Laid::default();
    let (_, pages) = nav_entries(height);
    for (page, r) in &pages {
        laid.hits.push((*r, Target::Nav(*page)));
    }
    laid.height = pages.iter().map(|(_, r)| r.bottom()).fold(0.0, f32::max) + NAV_TOP;
    laid
}

/// How many things on `page` want the user's attention, for a badge on its navigation item: a
/// connector that needs a step, the taskbar's trouble, a battery running low.
pub(super) fn attention(model: &Model, page: Page) -> usize {
    let s = &model.status;
    match page {
        Page::Connectors => usize::from(s.discord_problem.is_some()) + usize::from(s.teams_problem.is_some()),
        Page::Calendar => {
            let cal = &model.calendar;
            let accounts = [&cal.google, &cal.microsoft].into_iter().filter(|a| a.problem.is_some()).count();
            if model.config.calendar.enabled { accounts + cal.links.iter().filter(|l| l.is_some()).count() } else { 0 }
        }
        Page::Taskbar => usize::from(s.notice.is_some()),
        // Windows does not let Audio Tray read them: the bell's card would stay empty.
        Page::Notifications => {
            let c = &model.config.calendar;
            usize::from(c.bell && model.calendar.notifications_denied)
        }
        Page::Batteries => {
            let b = &model.config.batteries;
            if b.enabled { s.batteries.iter().filter(|x| x.level <= b.low_at).count() } else { 0 }
        }
        _ => 0,
    }
}

/// Lay out `model`'s page for a content area `width` DIPs wide (the window minus the nav).
pub(super) fn page(model: &Model, width: f32, measure: &dyn Fn(&str) -> f32) -> Laid {
    let w = (width - 2.0 * PAGE_PAD).clamp(200.0, PAGE_MAX_W);
    let mut col = Column { laid: Laid::default(), x: PAGE_PAD, w, y: 0.0, measure, config: &model.config };
    col.title(model.page.title());
    match model.page {
        Page::Home => pages::home::home(&mut col, model),
        Page::Taskbar => pages::taskbar::taskbar(&mut col, model),
        Page::Pill => pages::pill::pill(&mut col, model),
        Page::Motion => pages::motion::motion(&mut col, model),
        Page::Devices => {
            pages::devices::devices(&mut col, model);
            pages::devices::windows_sound(&mut col);
        }
        Page::Batteries => pages::batteries::batteries(&mut col, model),
        Page::Keyboards => pages::keyboards::keyboards(&mut col, model),
        Page::Connectors => pages::connectors::connectors(&mut col, model),
        Page::Calendar => pages::calendar::calendar(&mut col, model),
        Page::Notifications => pages::notifications::notifications(&mut col, model),
        Page::About => pages::about::about(&mut col, model),
    }
    col.laid.height = col.y + PAGE_PAD;
    col.laid
}

/// A second line in the plain tone.
fn plain(text: &str) -> Option<(String, Tone)> {
    Some((text.to_string(), Tone::Plain))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::config::Config;
    use crate::settings::model::{DeviceRow, Model};
    use crate::win::Monitor;
    use windows::Win32::Foundation::RECT;

    pub(in crate::settings) fn measure(text: &str) -> f32 {
        text.len() as f32 * 7.0
    }

    fn monitor(name: &str, left: i32, top: i32, w: i32, h: i32, primary: bool) -> Monitor {
        let rect = RECT { left, top, right: left + w, bottom: top + h };
        Monitor { name: name.to_string(), id: name.to_string(), rect, work: rect, primary }
    }

    pub(in crate::settings) fn model(page: Page) -> Model {
        let mut m = Model::new(Config::default());
        m.page = page;
        m.monitors = vec![monitor("A", 0, 0, 2560, 1440, true), monitor("B", 2560, 200, 1920, 1080, false)];
        m.devices = vec![DeviceRow {
            id: crate::audio::DeviceId("d".into()),
            name: "Speakers".into(),
            icon: IconId::Speakers,
            flow: crate::audio::Flow::Output,
            default: true,
        }];
        m
    }

    /// Every item of `laid` inside a page `width` wide.
    pub(in crate::settings) fn assert_inside(laid: &Laid, width: f32, what: &str) {
        for item in &laid.items {
            assert!(item.r.x >= 0.0 && item.r.right() <= width.max(200.0 + 2.0 * PAGE_PAD) + 0.5, "{what} {item:?} overflows {width}");
        }
    }

    #[test]
    fn every_page_lays_out_inside_its_width() {
        for page in Page::ALL {
            let m = model(page);
            for width in [500.0, 900.0, 2000.0] {
                let laid = super::page(&m, width, &measure);
                assert!(laid.height > TITLE_H, "{page:?} is empty");
                assert_inside(&laid, width, &format!("{page:?}"));
            }
        }
    }

    #[test]
    fn each_switch_reads_and_flips_its_own_flag() {
        let mut c = Config::default();
        for toggle in [Toggle::Strip, Toggle::HideLanguage, Toggle::Bell, Toggle::BatteryFlyout, Toggle::ClockUnread] {
            let before = toggle.get(&c);
            let flag = toggle.flag(&mut c).unwrap();
            *flag = !*flag;
            assert_eq!(toggle.get(&c), !before, "{toggle:?}");
        }
        assert!(Toggle::Site(0).flag(&mut c).is_none());
        assert!(Toggle::Site(0).get(&c), "YouTube is followed by default");
    }

    #[test]
    fn dimmed_buttons_are_drawn_but_not_hit() {
        // Keyboards off: their rows dim, and so do their buttons.
        let mut m = model(Page::Keyboards);
        m.config.keyboards.assigned = vec![crate::config::AssignedKeyboard { device_path: "a".into(), name: "Laptop".into(), ..Default::default() }];
        let laid = page(&m, 900.0, &measure);
        assert!(laid.items.iter().any(|i| matches!(i.kind, Kind::Button { target: Target::Button(Button::RemoveKeyboard(0)), dim: true, .. })));
        assert!(!laid.hits.iter().any(|(_, t)| matches!(t, Target::Button(Button::RenameKeyboard(_) | Button::RemoveKeyboard(_)))));
        // The calendar off: Windows' world clocks are not a click away either.
        let laid = page(&model(Page::Calendar), 900.0, &measure);
        assert!(!laid.hits.iter().any(|(_, t)| *t == Target::Button(Button::Cal(Cal::EditClocks))));
    }

    #[test]
    fn nav_hits_each_page() {
        let laid = nav(760.0);
        let (headings, pages) = nav_entries(760.0);
        assert_eq!(pages.len(), Page::ALL.len());
        for (page, r) in pages {
            assert_eq!(laid.hit(r.x + 4.0, r.y + 4.0), Some(Target::Nav(page)));
            // No heading sits over an item.
            assert!(headings.iter().all(|(_, h)| h.bottom() <= r.y || h.y >= r.bottom()));
        }
    }

    #[test]
    fn about_sits_at_the_bottom_unless_the_window_is_short() {
        let about = |h: f32| nav_entries(h).1.into_iter().find(|(p, _)| *p == Page::About).unwrap().1;
        assert!(about(900.0).bottom() > 860.0);
        let short = nav_entries(300.0).1;
        let last_other = short.iter().filter(|(p, _)| *p != Page::About).map(|(_, r)| r.bottom()).fold(0.0, f32::max);
        assert!(about(300.0).y > last_other);
    }
}

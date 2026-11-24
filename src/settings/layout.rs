//! Pure layout for the settings window: each page as [`Item`]s to paint and [`Target`]s to hit, in
//! DIPs. No Win32 here, so it stays unit-testable; text is measured through a function the caller
//! passes in.

use crate::config::{BellMiddleClick, MiddleClick, MotionStyle, PillAlign, PillEdge, PillSize, TileStyle, WaveColour};
use crate::icons::IconId;
use crate::lang::tr;

use super::model::{Model, Tone};

/// `format!` in Windows' language: the English pattern, or the French one.
macro_rules! tf {
    ($en:literal, $fr:literal $(, $arg:expr)* $(,)?) => {
        if crate::lang::french() { format!($fr $(, $arg)*) } else { format!($en $(, $arg)*) }
    };
}

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
const MAP_H: f32 = 260.0;
pub(super) const CHIP: f32 = 48.0;
const CHIP_GAP: f32 = 8.0;

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
            Page::Pill => ("Edge pill", "Pastille latérale"),
            Page::Motion => ("Look & motion", "Apparence et animations"),
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
            Page::Taskbar => Some(tr("Where it shows", "Où il s'affiche")),
            Page::Devices => Some(tr("What it follows", "Ce qu'il suit")),
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
    /// Show layout changes on the pill.
    KeyboardReadout,
    Music,
    Pill,
    PillDevices,
    AnnounceTracks,
    ListeningAlerts,
    /// Follow battery levels.
    Batteries,
    BatteryFlyout,
    BatteryTooltip,
    BatteryPillCard,
    BatteryPill,
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
    CalendarNotifications,
    /// Audio Tray shows every notification (Windows' banners off).
    TakeOver,
    CalendarShortcut,
    QuietMeetings,
    QuietCalls,
    /// Calendar `i` of [`super::model::Model::calendar`]'s list shown.
    CalendarShown(usize),
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

/// A segmented choice.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Choice {
    Tile,
    /// What a middle click on a taskbar audio button does: [`MIDDLE_CLICKS`].
    MiddleClick,
    /// What a middle click on the bell does: [`BELL_MIDDLE_CLICKS`].
    BellMiddle,
    PillSize,
    /// Every tab, or only the players'.
    BrowserTabs,
    /// Automatic, then each display in [`crate::win::monitors`]' order.
    Display,
    /// The low-battery level: [`crate::config::Batteries::LOW_LEVELS`].
    LowBattery,
    /// The layout of assigned keyboard `i`: the index is into [`super::model::Model::layouts`].
    KeyboardLayout(usize),
    /// A bar's motion: [`crate::config::MotionStyle::ALL`].
    RingMotion,
    BarMotion,
    VolumeMotion,
    /// Accent, or the cover's colour.
    WaveColour,
    /// [`CALENDAR_LAYOUTS`].
    CalendarLayout,
    /// [`AGENDA_ROWS`].
    AgendaRows,
    /// The locale's, Monday or Sunday.
    WeekStart,
    /// [`REMINDERS`].
    Reminder,
    /// [`FOCUS_LENGTHS`].
    FocusLength,
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
    Slot(usize, PillEdge, PillAlign),
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
    Button { label: String, accent: bool, glyph: Option<char>, target: Target },
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
    /// The edge pill as it looks now, drawn small on a screen's edge.
    PillPreview { on: bool },
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
            usize::from(c.enabled && c.notifications && model.calendar.notifications_denied)
        }
        Page::Batteries => {
            let b = &model.config.batteries;
            if b.enabled { s.batteries.iter().filter(|x| x.level <= b.low_at).count() } else { 0 }
        }
        _ => 0,
    }
}

/// Lays a page out top to bottom.
struct Column<'a> {
    laid: Laid,
    x: f32,
    w: f32,
    y: f32,
    measure: &'a dyn Fn(&str) -> f32,
}

impl Column<'_> {
    fn title(&mut self, text: &'static str) {
        self.laid.push(R::new(self.x, self.y, self.w, TITLE_H), Kind::Title(text));
        self.y += TITLE_H;
    }

    fn section(&mut self, text: &'static str) {
        self.laid.push(R::new(self.x, self.y, self.w, SECTION_H), Kind::Section(text));
        self.y += SECTION_H;
    }

    /// A card, and the rectangle its control goes in (right-aligned, vertically centred).
    fn card(&mut self, glyph: Option<char>, title: impl Into<String>, desc: Option<(String, Tone)>, dim: bool, target: Option<Target>) -> R {
        let r = R::new(self.x, self.y, self.w, CARD_H);
        self.laid.push(r, Kind::Card { glyph, title: title.into(), desc, dim, target });
        if let Some(target) = target {
            self.laid.hits.push((r, target));
        }
        self.y += CARD_H + CARD_GAP;
        r
    }

    fn toggle(&mut self, glyph: Option<char>, title: &str, desc: Option<(String, Tone)>, on: bool, dim: bool, toggle: Toggle) {
        // Dimmed, it is moot: not a target.
        let card = self.card(glyph, title, desc, dim, (!dim).then_some(Target::Toggle(toggle)));
        let r = R::new(card.right() - CARD_PAD - TOGGLE_W, card.y + (CARD_H - TOGGLE_H) / 2.0, TOGGLE_W, TOGGLE_H);
        self.laid.push(r, Kind::Toggle { on, dim, toggle });
    }


    fn button_card(&mut self, glyph: Option<char>, title: &str, desc: Option<(String, Tone)>, label: &str, accent: bool, button: Button) {
        let card = self.card(glyph, title, desc, false, None);
        self.button_in(card, label, accent, None, button);
    }

    fn button_in(&mut self, card: R, label: &str, accent: bool, glyph: Option<char>, button: Button) {
        let w = button_width(self.measure, label, glyph);
        let r = R::new(card.right() - CARD_PAD - w, card.y + (card.h - CONTROL_H) / 2.0, w, CONTROL_H);
        let target = Target::Button(button);
        self.laid.push(r, Kind::Button { label: label.to_string(), accent, glyph, target });
        self.laid.hits.push((r, target));
    }

    /// A card that opens something: all of it is the button, with a trailing glyph.
    fn link(&mut self, glyph: Option<char>, title: &str, desc: Option<(String, Tone)>, trailing: char, button: Button) {
        let card = self.card(glyph, title, desc, false, Some(Target::Button(button)));
        let r = R::new(card.right() - CARD_PAD - 20.0, card.y, 20.0, CARD_H);
        self.laid.push(r, Kind::Trailing(trailing));
    }

    fn gap(&mut self, dy: f32) {
        self.y += dy;
    }

    /// Start a card that holds rows ([`Column::row`]); [`Column::end_group`] sizes it.
    fn group(&mut self) -> Group {
        let at = self.laid.items.len();
        self.laid.push(R::new(self.x, self.y, self.w, 0.0), Kind::Card { glyph: None, title: String::new(), desc: None, dim: false, target: None });
        Group { at, top: self.y, rows: 0 }
    }

    /// One row of a group: a step's badge or nothing, a title and second line, and its control.
    fn row(&mut self, group: &mut Group, badge: Option<(u8, bool)>, title: &str, desc: Option<(String, Tone)>, dim: bool, control: Control) {
        if group.rows > 0 {
            self.laid.push(R::new(self.x, self.y, self.w, 1.0), Kind::Divider);
        }
        group.rows += 1;
        let row = R::new(self.x, self.y, self.w, ROW_H);
        if let Some((n, done)) = badge {
            self.laid.push(R::new(row.x + CARD_PAD, row.y, 24.0, ROW_H), Kind::Badge { n, done });
        }
        let used = match control {
            Control::None => 0.0,
            Control::Button(label, accent, glyph, button) => {
                self.button_in(row, label, accent, glyph, button);
                button_width(self.measure, label, glyph)
            }
            Control::Toggle(on, toggle) => {
                // The whole row flips it, as a switch's card does.
                if !dim {
                    self.laid.hits.push((row, Target::Toggle(toggle)));
                }
                self.toggle_in(row, row.right() - CARD_PAD, on, dim, toggle)
            }
            Control::Segments(labels, selected, choice) => self.segments_in(row, &labels, selected, dim, choice),
            Control::Buttons(buttons) => self.buttons_in(row, &buttons),
        };
        let end = row.right() - CARD_PAD - if used > 0.0 { used + 16.0 } else { 0.0 };
        let x = row.x + CARD_TEXT_X;
        self.laid.push(R::new(x, row.y, end - x, ROW_H), Kind::Label { title: title.to_string(), desc, dim });
        self.y += ROW_H;
    }

    /// Buttons right-aligned in `row`, in order; returns the room they take.
    fn buttons_in(&mut self, row: R, buttons: &[(String, bool, Option<char>, Button)]) -> f32 {
        let mut right = row.right() - CARD_PAD;
        for (label, accent, glyph, button) in buttons.iter().rev() {
            let w = button_width(self.measure, label, *glyph);
            let r = R::new(right - w, row.y + (row.h - CONTROL_H) / 2.0, w, CONTROL_H);
            let target = Target::Button(*button);
            self.laid.push(r, Kind::Button { label: label.clone(), accent: *accent, glyph: *glyph, target });
            self.laid.hits.push((r, target));
            right -= w + 8.0;
        }
        row.right() - CARD_PAD - right - 8.0
    }

    /// A row whose title is being typed: the field, then its buttons.
    fn field_row(&mut self, group: &mut Group, text: &str, buttons: Vec<(String, bool, Option<char>, Button)>) {
        if group.rows > 0 {
            self.laid.push(R::new(self.x, self.y, self.w, 1.0), Kind::Divider);
        }
        group.rows += 1;
        let row = R::new(self.x, self.y, self.w, ROW_H);
        let used = self.buttons_in(row, &buttons);
        let x = row.x + CARD_TEXT_X;
        let end = row.right() - CARD_PAD - used - 16.0;
        self.laid.push(R::new(x, row.y + (ROW_H - CONTROL_H) / 2.0, (end - x).min(360.0), CONTROL_H), Kind::Field(text.to_string()));
        self.y += ROW_H;
    }

    /// A switch ending at `right`, centred in `card`; returns the room it takes, its label included.
    fn toggle_in(&mut self, card: R, right: f32, on: bool, dim: bool, toggle: Toggle) -> f32 {
        let r = R::new(right - TOGGLE_W, card.y + (card.h - TOGGLE_H) / 2.0, TOGGLE_W, TOGGLE_H);
        self.laid.push(r, Kind::Toggle { on, dim, toggle });
        TOGGLE_W + toggle_label_w() + 8.0
    }

    /// A segmented choice right-aligned in `card`; returns its width.
    fn segments_in<S: AsRef<str>>(&mut self, card: R, labels: &[S], selected: usize, dim: bool, choice: Choice) -> f32 {
        let widths: Vec<f32> = labels.iter().map(|l| ((self.measure)(l.as_ref()) + 28.0).max(48.0)).collect();
        let total = widths.iter().sum::<f32>();
        let mut x = card.right() - CARD_PAD - total;
        let y = card.y + (card.h - CONTROL_H) / 2.0;
        for (i, (label, seg_w)) in labels.iter().zip(widths).enumerate() {
            let r = R::new(x, y, seg_w, CONTROL_H);
            x += seg_w;
            let target = Target::Choice(choice, i);
            let kind = Kind::Segment { label: label.as_ref().to_string(), selected: i == selected, first: i == 0, last: i + 1 == labels.len(), dim, target };
            self.laid.push(r, kind);
            if !dim {
                self.laid.hits.push((r, target));
            }
        }
        total
    }

    fn segments<S: AsRef<str>>(&mut self, title: &str, desc: Option<(String, Tone)>, labels: &[S], selected: usize, dim: bool, choice: Choice) {
        let card = self.card(None, title, desc, dim, None);
        self.segments_in(card, labels, selected, dim, choice);
    }

    /// A row of picture tiles, after a `lead` naming what they choose when given (else full
    /// width); dimmed, they are moot and not targets.
    fn tiles(&mut self, lead: Option<(&'static str, &'static str)>, tiles: &[(Picture, &'static str, &'static str, bool, Target)], dim: bool, h: f32) {
        const LEAD_W: f32 = 180.0;
        const GAP: f32 = 10.0;
        let mut x = self.x;
        if let Some((title, desc)) = lead {
            self.laid.push(R::new(x, self.y, LEAD_W - GAP, h), Kind::Label { title: title.to_string(), desc: plain(desc), dim });
            x += LEAD_W;
        }
        let n = tiles.len().max(3) as f32;
        let w = (self.x + self.w - x - GAP * (n - 1.0)) / n;
        for (picture, title, desc, selected, target) in tiles {
            let r = R::new(x, self.y, w, h);
            self.laid.push(r, Kind::Tile { picture: *picture, title, desc, selected: *selected, dim, target: *target });
            if !dim {
                self.laid.hits.push((r, *target));
            }
            x += w + GAP;
        }
        self.y += h + GAP;
    }

    /// Switches in two columns of half-width cards: title, second line, on, dimmed.
    fn toggle_grid(&mut self, toggles: &[(&str, &str, bool, bool, Toggle)]) {
        const GAP: f32 = 8.0;
        let w = (self.w - GAP) / 2.0;
        for (i, (title, desc, on, dim, toggle)) in toggles.iter().enumerate() {
            let r = R::new(self.x + (i % 2) as f32 * (w + GAP), self.y, w, CARD_H);
            let target = (!dim).then_some(Target::Toggle(*toggle));
            self.laid.push(r, Kind::Card { glyph: None, title: title.to_string(), desc: plain(desc), dim: *dim, target });
            if let Some(target) = target {
                self.laid.hits.push((r, target));
            }
            let t = R::new(r.right() - CARD_PAD - TOGGLE_W, r.y + (CARD_H - TOGGLE_H) / 2.0, TOGGLE_W, TOGGLE_H);
            self.laid.push(t, Kind::Toggle { on: *on, dim: *dim, toggle: *toggle });
            if i % 2 == 1 || i + 1 == toggles.len() {
                self.y += CARD_H + GAP;
            }
        }
    }

    /// A row of a group holding a title, then choices as chips that wrap to as many lines as they
    /// need: every one a click away, however many there are.
    fn pick_row(&mut self, group: &mut Group, title: &str, desc: Option<(String, Tone)>, picks: &[(String, bool, Target)], dim: bool) {
        const CHIP_H: f32 = 32.0;
        const GAP: f32 = 8.0;
        if group.rows > 0 {
            self.laid.push(R::new(self.x, self.y, self.w, 1.0), Kind::Divider);
        }
        group.rows += 1;
        let x0 = self.x + CARD_TEXT_X;
        let right = self.x + self.w - CARD_PAD;
        let head = if desc.is_some() { 46.0 } else { 28.0 };
        self.laid.push(R::new(x0, self.y + 10.0, right - x0, head), Kind::Label { title: title.to_string(), desc, dim });
        let (mut x, mut y) = (x0, self.y + 10.0 + head + 6.0);
        for (label, selected, target) in picks {
            let w = ((self.measure)(label) + 28.0).max(56.0).min(right - x0);
            if x + w > right && x > x0 {
                x = x0;
                y += CHIP_H + GAP;
            }
            let r = R::new(x, y, w, CHIP_H);
            self.laid.push(r, Kind::Pick { label: label.clone(), selected: *selected, dim, target: *target });
            if !dim {
                self.laid.hits.push((r, *target));
            }
            x += w + GAP;
        }
        self.y = y + CHIP_H + 14.0;
    }

    /// Connector cards two by two: each opens its steps when clicked and has its switch; the open
    /// one is outlined.
    fn connector_grid(&mut self, cards: &[ConnectorCard], open: Option<Connector>) {
        const GAP: f32 = 8.0;
        let w = (self.w - GAP) / 2.0;
        for (i, card) in cards.iter().enumerate() {
            let r = R::new(self.x + (i % 2) as f32 * (w + GAP), self.y + (i / 2) as f32 * (CARD_H + GAP), w, CARD_H);
            let target = Target::Connector(card.connector);
            self.laid.push(r, Kind::Card { glyph: Some(card.glyph), title: card.title.to_string(), desc: Some(card.state.clone()), dim: false, target: Some(target) });
            self.laid.hits.push((r, target));
            if open == Some(card.connector) {
                self.laid.push(r, Kind::Outline);
            }
            let t = R::new(r.right() - CARD_PAD - TOGGLE_W, r.y + (CARD_H - TOGGLE_H) / 2.0, TOGGLE_W, TOGGLE_H);
            self.laid.push(t, Kind::Toggle { on: card.on, dim: false, toggle: card.toggle });
            // The switch, not the card, under the pointer: it flips without opening.
            self.laid.hits.push((R::new(t.x - 4.0, t.y - 6.0, t.w + 8.0, t.h + 12.0), Target::Toggle(card.toggle)));
        }
        self.y += cards.len().div_ceil(2) as f32 * (CARD_H + GAP) + 4.0;
    }

    fn end_group(&mut self, group: Group) {
        self.laid.items[group.at].r.h = self.y - group.top;
        self.y += CARD_GAP;
    }
}

/// What a grouped row holds on its right.
enum Control<'a> {
    None,
    /// Label, accent, glyph, what it does.
    Button(&'a str, bool, Option<char>, Button),
    Toggle(bool, Toggle),
    Segments(Vec<String>, usize, Choice),
    /// Label, accent, glyph, what it does; laid out left to right.
    Buttons(Vec<(String, bool, Option<char>, Button)>),
}

/// A connector, on its own card in Settings > Connectors.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Connector {
    Browser,
    Mpc,
    Discord,
    Teams,
}

/// A grouped card being laid out: its item, where it starts, how many rows it has.
struct Group {
    at: usize,
    top: f32,
    rows: usize,
}

/// A button's width for `label`, with room for its glyph.
fn button_width(measure: &dyn Fn(&str) -> f32, label: &str, glyph: Option<char>) -> f32 {
    (measure(label) + 24.0 + if glyph.is_some() { 24.0 } else { 0.0 }).max(96.0)
}

/// Lay out `model`'s page for a content area `width` DIPs wide (the window minus the nav).
pub(super) fn page(model: &Model, width: f32, measure: &dyn Fn(&str) -> f32) -> Laid {
    let w = (width - 2.0 * PAGE_PAD).clamp(200.0, PAGE_MAX_W);
    let mut col = Column { laid: Laid::default(), x: PAGE_PAD, w, y: 0.0, measure };
    col.title(model.page.title());
    match model.page {
        Page::Home => home(&mut col, model),
        Page::Taskbar => taskbar(&mut col, model),
        Page::Pill => pill(&mut col, model),
        Page::Motion => motion(&mut col, model),
        Page::Devices => {
            devices(&mut col, model);
            windows_sound(&mut col);
        }
        Page::Batteries => batteries(&mut col, model),
        Page::Keyboards => keyboards(&mut col, model),
        Page::Connectors => connectors(&mut col, model),
        Page::Calendar => calendar_steps(&mut col, model),
        Page::Notifications => notifications_page(&mut col, model),
        Page::About => about(&mut col, model),
    }
    col.laid.height = col.y + PAGE_PAD;
    col.laid
}

fn plain(text: &str) -> Option<(String, Tone)> {
    Some((text.to_string(), Tone::Plain))
}

/// The pill's card on Home: how tall it is.
const HERO_H: f32 = 196.0;
const GLANCE_H: f32 = 100.0;

/// What Audio Tray is doing, at a glance: the pill as it looks, a tile per part of the app saying
/// how it is and opening its page, and the switches most often changed.
fn home(col: &mut Column, model: &Model) {
    let c = &model.config;
    let p = &c.pill;
    let hero = R::new(col.x, col.y, col.w, HERO_H);
    col.laid.push(hero, Kind::Card { glyph: None, title: String::new(), desc: None, dim: false, target: None });
    // The picture only where it leaves the words and buttons room.
    let wide = col.w >= 600.0;
    if wide {
        col.laid.push(R::new(hero.x + CARD_PAD, hero.y + CARD_PAD, 240.0, HERO_H - 2.0 * CARD_PAD), Kind::PillPreview { on: p.enabled });
    }
    let x = hero.x + CARD_PAD + if wide { 240.0 + 24.0 } else { 0.0 };
    let w = hero.right() - CARD_PAD - x;
    let head = R::new(x, hero.y + CARD_PAD, w, 48.0);
    let used = col.toggle_in(head, head.right(), p.enabled, false, Toggle::Pill);
    col.laid.hits.push((R::new(head.right() - TOGGLE_W - 4.0, head.y + 8.0, TOGGLE_W + 8.0, 32.0), Target::Toggle(Toggle::Pill)));
    col.laid.push(R::new(x, head.y, w - used - 8.0, head.h), Kind::Label { title: tr("Edge pill", "Pastille latérale").into(), desc: Some((place_words(p.edge, p.align), Tone::Plain)), dim: false });
    let m = c.motion;
    let style = |s: MotionStyle| match s {
        MotionStyle::None => tr("still", "immobile"),
        MotionStyle::Google => "Google",
        MotionStyle::Samsung => "Samsung",
    };
    let size = match p.size {
        PillSize::Hidden => tr("Hidden at rest", "Masquée au repos"),
        PillSize::Small => tr("Small at rest", "Petite au repos"),
        PillSize::Big => tr("Big at rest", "Grande au repos"),
    };
    col.laid.push(R::new(x, head.bottom() + 4.0, w, 20.0), Kind::Text(tf!("{size}; hovering opens the card.", "{size} ; le survol ouvre la carte.")));
    col.laid.push(R::new(x, head.bottom() + 26.0, w, 20.0), Kind::Text(tf!("Ring {}, track bar {}, volume {}.", "Anneau {}, barre de lecture {}, volume {}.", style(m.ring), style(m.bar), style(m.volume))));
    let mut bx = x;
    for (label, page) in [(tr("Change its place", "Changer sa position"), Page::Pill), (tr("Look & motion", "Apparence et animations"), Page::Motion)] {
        let bw = button_width(col.measure, label, None);
        let r = R::new(bx, hero.bottom() - CARD_PAD - CONTROL_H, bw, CONTROL_H);
        let target = Target::Nav(page);
        col.laid.push(r, Kind::Button { label: label.to_string(), accent: false, glyph: None, target });
        col.laid.hits.push((r, target));
        bx += bw + 8.0;
    }
    col.y += HERO_H + 12.0;

    for (i, glance) in glances(model).into_iter().enumerate() {
        const GAP: f32 = 10.0;
        let w = (col.w - 2.0 * GAP) / 3.0;
        let r = R::new(col.x + (i % 3) as f32 * (w + GAP), col.y + (i / 3) as f32 * (GLANCE_H + GAP), w, GLANCE_H);
        let target = glance.4;
        col.laid.push(r, Kind::Glance { title: glance.0, head: glance.1, line: glance.2, tone: glance.3, target });
        col.laid.hits.push((r, target));
    }
    let rows = glances(model).len().div_ceil(3) as f32;
    col.y += rows * GLANCE_H + rows * 10.0;

    col.section(tr("Quick settings", "Paramètres rapides"));
    let pill_off = !p.enabled;
    let strip_off = !c.strip.enabled;
    let cal = &c.calendar;
    col.toggle_grid(&[
        (tr("The bell opens notifications", "La cloche ouvre les notifications"), tr("Audio Tray's, every app's", "Celles d'Audio Tray, de toutes les applis"), cal.notifications, !cal.enabled, Toggle::CalendarNotifications),
        (tr("Do not disturb in calls", "Ne pas déranger en appel"), tr("Teams, Discord, a meeting app", "Teams, Discord, une appli de réunion"), cal.quiet_in_calls, !cal.enabled, Toggle::QuietCalls),
        (tr("Show each new song", "Afficher chaque nouveau morceau"), tr("Its title on the pill, for a moment", "Son titre sur la pastille, un instant"), p.announce_tracks, pill_off, Toggle::AnnounceTracks),
        (tr("Hide Windows' volume icon", "Masquer l'icône de volume de Windows"), tr("The output button does its job", "Le bouton de sortie le remplace"), c.strip.hide_windows_volume, strip_off, Toggle::HideVolume),
    ]);
}

/// "Left edge, middle".
fn place_words(edge: PillEdge, align: PillAlign) -> String {
    let edge_word = match edge {
        PillEdge::Left => tr("Left edge", "Bord gauche"),
        PillEdge::Right => tr("Right edge", "Bord droit"),
        PillEdge::Top => tr("Top edge", "Bord supérieur"),
    };
    let labels = align_labels(edge);
    let at = ALIGNS.iter().position(|a| *a == align).unwrap_or(1);
    let place = if align == PillAlign::Center { tr("middle", "milieu").to_string() } else { labels[at].to_lowercase() };
    format!("{edge_word}, {place}")
}

/// Home's tiles: a part of the app, its state in a few words and a line more, how it is going, and
/// the page it opens.
fn glances(model: &Model) -> Vec<(&'static str, String, String, Tone, Target)> {
    let c = &model.config;
    let s = &model.status;
    let mut tiles = Vec::new();
    let (head, tone) = match (c.strip.enabled, s.notice) {
        (_, Some(_)) => (tr("Needs attention", "Nécessite votre attention").to_string(), Tone::Warn),
        (true, None) => (tr("Output and input buttons", "Boutons de sortie et d'entrée").to_string(), Tone::Good),
        (false, None) => (tr("Off", "Désactivé").to_string(), Tone::Plain),
    };
    let line = if !c.strip.enabled { tr("Windows' own icons only", "Seulement les icônes de Windows") } else if c.strip.next_to_wifi { tr("Next to Wi-Fi", "À côté du Wi-Fi") } else { tr("Beside the notification icons", "À côté des icônes de notification") };
    tiles.push((tr("Taskbar", "Barre des tâches"), head, line.to_string(), tone, Target::Nav(Page::Taskbar)));

    // The clock's and the bell's panels.
    let cal = &c.calendar;
    let cs = &model.calendar;
    let problem = [&cs.google, &cs.microsoft].into_iter().find_map(|a| a.problem.clone()).or_else(|| cs.links.iter().flatten().next().cloned());
    let (head, line, tone) = match (cal.enabled, problem) {
        (false, _) => (tr("Off", "Désactivé").to_string(), tr("The clock opens Windows' calendar", "L'horloge ouvre le calendrier de Windows").to_string(), Tone::Plain),
        (true, Some(problem)) => (tr("Needs attention", "Nécessite votre attention").to_string(), problem, Tone::Warn),
        (true, None) if cs.calendars.is_empty() && cal.links.is_empty() => (tr("No calendar yet", "Pas encore d'agenda").to_string(), tr("Sign in, or add a link", "Connectez-vous, ou ajoutez un lien").to_string(), Tone::Plain),
        (true, None) => {
            let n = cs.calendars.len();
            (if n == 1 { tr("1 calendar", "1 agenda").to_string() } else { tf!("{n} calendars", "{n} agendas") }, tr("Opened from the clock", "Ouvert depuis l'horloge").to_string(), Tone::Good)
        }
    };
    tiles.push((tr("Calendar", "Agenda"), head, line, tone, Target::Nav(Page::Calendar)));
    let (head, line, tone) = match (cal.enabled && cal.notifications, cs.notifications_denied, cs.quiet) {
        (false, _, _) => (tr("Off", "Désactivé").to_string(), tr("The bell opens Windows' own", "La cloche ouvre celles de Windows").to_string(), Tone::Plain),
        (true, true, _) => (tr("Not allowed", "Non autorisé").to_string(), tr("Windows keeps them from Audio Tray", "Windows les refuse à Audio Tray").to_string(), Tone::Warn),
        (true, false, true) => (tr("Quiet", "Silence").to_string(), tr("Do not disturb is on", "Ne pas déranger est activé").to_string(), Tone::Plain),
        (true, false, false) => (tr("Opened from the bell", "Ouvertes depuis la cloche").to_string(), tr("Do not disturb is off", "Ne pas déranger est désactivé").to_string(), Tone::Good),
    };
    tiles.push(("Notifications", head, line, tone, Target::Nav(Page::Notifications)));

    let (head, tone) = if c.music.enabled { (tr("Following YouTube Music", "Suit YouTube Music").to_string(), Tone::Good) } else { (tr("YouTube Music not followed", "YouTube Music non suivi").to_string(), Tone::Plain) };
    let line = match c.music.tile_style {
        TileStyle::Off => tr("Nothing drawn in its button", "Rien dessiné dans son bouton"),
        TileStyle::Cover => tr("Its cover in its taskbar button", "Sa pochette dans son bouton de la barre des tâches"),
        TileStyle::Full => tr("Cover and title in its taskbar button", "Pochette et titre dans son bouton de la barre des tâches"),
    };
    tiles.push((tr("Music", "Musique"), head, line.to_string(), tone, Target::Nav(Page::Taskbar)));

    let connected: Vec<&str> = s.integrations.iter().filter(|(_, on)| *on).map(|(name, _)| *name).collect();
    let needs = attention(model, Page::Connectors);
    let (head, tone) = match (needs, connected.len()) {
        (0, 0) => (tr("None connected", "Aucun connecté").to_string(), Tone::Plain),
        (0, 1) => (tr("1 connected", "1 connecté").to_string(), Tone::Good),
        (0, n) => (tf!("{n} connected", "{n} connectés"), Tone::Good),
        (1, _) => (tr("One needs a step", "Un demande une étape").to_string(), Tone::Warn),
        (n, _) => (tf!("{n} need a step", "{n} demandent une étape"), Tone::Warn),
    };
    let line = if connected.is_empty() { tr("Browser, Discord, Teams, MPC-HC", "Navigateur, Discord, Teams, MPC-HC").to_string() } else { join_names(&connected) };
    tiles.push((tr("Connectors", "Connecteurs"), head, line, tone, Target::Nav(Page::Connectors)));

    let b = &c.batteries;
    let lowest = s.batteries.iter().min_by_key(|x| x.level);
    let (head, line, tone) = match (b.enabled, lowest) {
        (false, _) => (tr("Off", "Désactivé").to_string(), tr("Battery levels are not followed", "Les niveaux de batterie ne sont pas suivis").to_string(), Tone::Plain),
        (true, None) => (tr("None reporting", "Aucun ne le signale").to_string(), tr("Wireless devices show once connected", "Les appareils sans fil apparaissent une fois connectés").to_string(), Tone::Plain),
        (true, Some(low)) => {
            let others = s.batteries.len() - 1;
            let line = match others {
                0 => tr("The only one reporting", "Le seul à le signaler").to_string(),
                1 => tr("1 other device", "1 autre appareil").to_string(),
                n => tf!("{n} other devices", "{n} autres appareils"),
            };
            (tf!("{} at {}%", "{} à {} %", low.name, low.level), line, if low.level <= b.low_at { Tone::Warn } else { Tone::Good })
        }
    };
    tiles.push(("Batteries", head, line, tone, Target::Nav(Page::Batteries)));

    let k = &c.keyboards;
    let head = match (k.enabled, k.assigned.len()) {
        (false, _) => tr("Off", "Désactivé").to_string(),
        (true, 0) => tr("No keyboard yet", "Pas encore de clavier").to_string(),
        (true, 1) => tr("1 keyboard", "1 clavier").to_string(),
        (true, n) => tf!("{n} keyboards", "{n} claviers"),
    };
    let line = if k.enabled { tr("A layout for each keyboard", "Une disposition par clavier") } else { tr("One layout for every keyboard", "Une disposition pour tous les claviers") };
    tiles.push((tr("Keyboards", "Claviers"), head, line.to_string(), if k.enabled { Tone::Good } else { Tone::Plain }, Target::Nav(Page::Keyboards)));

    let version = env!("CARGO_PKG_VERSION");
    let (head, line, tone) = match &model.update {
        Some(next) => (tf!("Version {next} is ready", "La version {next} est prête"), tr("Restart Audio Tray to use it", "Redémarrez Audio Tray pour l'utiliser").to_string(), Tone::Warn),
        None => (tr("Up to date", "À jour").to_string(), format!("Version {version}"), Tone::Good),
    };
    tiles.push((tr("Updates", "Mises à jour"), head, line, tone, Target::Nav(Page::About)));
    tiles
}

/// How the bars move, each part on its own, picked by what it looks like.
fn motion(col: &mut Column, model: &Model) {
    let m = model.config.motion;
    let pill_off = !model.config.pill.enabled;
    let styles = MotionStyle::ALL;
    let row = |picture: fn(MotionStyle) -> Picture, current: MotionStyle, choice: Choice, words: [&'static str; 3]| -> Vec<(Picture, &'static str, &'static str, bool, Target)> {
        styles
            .iter()
            .enumerate()
            .map(|(i, s)| (picture(*s), [tr("None", "Aucune"), "Google", "Samsung"][i], words[i], *s == current, Target::Choice(choice, i)))
            .collect()
    };
    col.gap(4.0);
    let ring = row(Picture::Ring, m.ring, Choice::RingMotion, [tr("Fills, nothing more", "Se remplit, sans plus"), tr("Ripples; flat on pause", "Ondule ; plat en pause"), tr("Layered waves", "Vagues superposées")]);
    col.tiles(Some((tr("Progress ring", "Anneau de progression"), tr("Around each cover on the pill", "Autour de chaque pochette sur la pastille"))), &ring, pill_off, TILE_H);
    let bar = row(Picture::Bar, m.bar, Choice::BarMotion, [tr("A plain bar", "Une barre simple"), tr("A rippling line", "Une ligne ondulante"), tr("Waves over the line", "Des vagues sur la ligne")]);
    col.tiles(Some((tr("Track bar", "Barre de lecture"), tr("Under each song on the card", "Sous chaque morceau sur la carte"))), &bar, pill_off, TILE_H);
    let volume = row(Picture::Volume, m.volume, Choice::VolumeMotion, [tr("Jumps to the level", "Saute au niveau"), tr("Springs to the level", "Rebondit jusqu'au niveau"), tr("A stretchy capsule", "Une capsule élastique")]);
    col.tiles(Some((tr("Volume sliders", "Curseurs de volume"), tr("In the panel and readout", "Dans le panneau et l'indicateur"))), &volume, false, TILE_H);
    let colours = [
        (Picture::Colour(WaveColour::Accent), "Accent", tr("The Windows accent", "La couleur d'accent de Windows"), m.colour == WaveColour::Accent, Target::Choice(Choice::WaveColour, 0)),
        (Picture::Colour(WaveColour::Cover), tr("Cover", "Pochette"), tr("Each cover's own colour", "La couleur de chaque pochette"), m.colour == WaveColour::Cover, Target::Choice(Choice::WaveColour, 1)),
    ];
    col.tiles(Some((tr("Wave colour", "Couleur de la vague"), tr("For the ring and the bar", "Pour l'anneau et la barre"))), &colours, pill_off, TILE_H);
}

/// A picture tile's height on Look & motion.
const TILE_H: f32 = 150.0;

/// The Taskbar page: Audio Tray's three buttons in the order they sit on the taskbar (the audio
/// buttons, the clock, the bell), each in the same four parts: on or off, what a middle click
/// does, what the button shows, its shortcut. Then the music player's button.
fn taskbar(col: &mut Column, model: &Model) {
    let c = &model.config;
    if let Some(notice) = model.status.notice {
        col.card(Some('\u{E7BA}'), tr("The taskbar needs attention", "La barre des tâches nécessite votre attention"), Some((notice.to_string(), Tone::Warn)), false, None);
        col.gap(8.0);
    }
    note(col, tr("On each button, a left click opens its panel and a right click its menu", "Sur chaque bouton, un clic gauche ouvre son panneau et un clic droit son menu"));

    // The audio buttons.
    col.section(tr("Audio buttons", "Boutons audio"));
    col.toggle(
        Some('\u{E767}'),
        tr("Audio buttons in the tray", "Boutons audio dans la zone de notification"),
        plain(tr("Output and input buttons drawn into the taskbar, over Audio Tray's icon", "Boutons de sortie et d'entrée dessinés dans la barre des tâches, par-dessus l'icône d'Audio Tray")),
        c.strip.enabled,
        false,
        Toggle::Strip,
    );
    let off = !c.strip.enabled;
    let clicks = [
        (MiddleClick::Mute, tr("Mute", "Couper le son"), tr("Mutes it; again unmutes", "Coupe le son ; rétablit au second clic")),
        (MiddleClick::Switch, tr("Switch device", "Changer de périphérique"), tr("Goes to the next device", "Passe au périphérique suivant")),
        (MiddleClick::SwitchMute, tr("Switch, then mute", "Changer, puis couper"), tr("Each device, then muted", "Chaque périphérique, puis coupé")),
    ];
    let tiles: Vec<_> = clicks
        .iter()
        .map(|(click, title, desc)| {
            let i = MIDDLE_CLICKS.iter().position(|m| m == click).unwrap_or(0);
            (Picture::Click(*click), *title, *desc, c.strip.middle_click == *click, Target::Choice(Choice::MiddleClick, i))
        })
        .collect();
    col.tiles(Some((tr("Middle click", "Clic du milieu"), tr("On the output or the input button", "Sur le bouton de sortie ou d'entrée"))), &tiles, off, BUTTON_TILE_H);
    // The keyboard layout indicator is Windows' icon the buttons sit beside; hiding it needs the
    // code drawn into the taskbar (the buttons, or the music tile).
    let no_tap = !c.injects();
    col.toggle_grid(&[
        (tr("Next to Wi-Fi", "À côté du Wi-Fi"), tr("Between the keyboard layout and Wi-Fi", "Entre la disposition du clavier et le Wi-Fi"), c.strip.next_to_wifi, off, Toggle::NextToWifi),
        (tr("Hide Windows' volume icon", "Masquer l'icône de volume de Windows"), tr("The output button does its job", "Le bouton de sortie le remplace"), c.strip.hide_windows_volume, off, Toggle::HideVolume),
        (tr("Hide Windows' microphone icon", "Masquer l'icône du micro de Windows"), tr("The input button's dot says who listens", "Le point du bouton d'entrée indique qui écoute"), c.strip.hide_windows_mic, off, Toggle::HideMic),
        (tr("Hide Windows' keyboard layout", "Masquer la disposition de clavier de Windows"), tr("Win+Space still switches", "Win+Espace change toujours"), c.strip.hide_windows_language, no_tap, Toggle::HideLanguage),
    ]);
    col.toggle(None, tr("Win+Ctrl+V opens the audio panel", "Win+Ctrl+V ouvre le panneau audio"), plain(tr("Windows' shortcut for its sound output", "Le raccourci de Windows pour sa sortie audio")), c.strip.shortcut, false, Toggle::StripShortcut);

    // The clock.
    let cal = &c.calendar;
    let k = &c.clock;
    col.section(tr("Clock", "Horloge"));
    col.toggle(
        Some('\u{E787}'),
        tr("The clock opens Audio Tray's calendar", "L'horloge ouvre l'agenda d'Audio Tray"),
        plain(tr("The month, your events and meetings, in place of Windows' calendar", "Le mois, vos événements et réunions, à la place du calendrier de Windows")),
        cal.enabled,
        false,
        Toggle::Calendar,
    );
    note(col, tr("A middle click on the clock is Windows'", "Le clic du milieu sur l'horloge reste à Windows"));
    col.toggle_grid(&[
        (tr("Seconds", "Secondes"), tr("After the minutes", "Après les minutes"), k.seconds, false, Toggle::ClockSeconds),
        (tr("Day of the week", "Jour de la semaine"), tr("Before the date", "Avant la date"), k.weekday, false, Toggle::ClockWeekday),
        (tr("One line", "Une ligne"), tr("The date after the time", "La date après l'heure"), k.one_line, false, Toggle::ClockOneLine),
        (tr("Hide the date", "Masquer la date"), tr("The time alone", "L'heure seule"), k.hide_date, false, Toggle::ClockHideDate),
    ]);
    note(col, tr("Your own patterns: time_pattern and date_pattern under [clock] in config.toml", "Vos propres formats : time_pattern et date_pattern sous [clock] dans config.toml"));
    col.toggle(None, tr("Win+Alt+D and Win+N open them", "Win+Alt+D et Win+N les ouvrent"), plain(tr("Windows' shortcuts: the calendar, and the bell's notifications", "Les raccourcis de Windows : l'agenda, et les notifications de la cloche")), cal.shortcut, !cal.enabled, Toggle::CalendarShortcut);

    // The bell.
    let bell_off = !cal.enabled || !cal.notifications;
    col.section(tr("Bell", "Cloche"));
    col.toggle(
        Some('\u{EA8F}'),
        tr("The bell opens Audio Tray's notifications", "La cloche ouvre les notifications d'Audio Tray"),
        plain(tr("Windows' bell beside the clock: every app's, with Do not disturb and the focus timer", "La cloche de Windows à côté de l'horloge : celles de toutes les applis, avec Ne pas déranger et le minuteur de concentration")),
        cal.notifications,
        !cal.enabled,
        Toggle::CalendarNotifications,
    );
    let clicks = [
        (BellMiddleClick::DoNotDisturb, tr("Do not disturb", "Ne pas déranger"), tr("On, and off again", "Activer, puis désactiver")),
        (BellMiddleClick::ClearAll, tr("Clear all", "Tout effacer"), tr("Every notification", "Toutes les notifications")),
        (BellMiddleClick::Nothing, tr("Nothing", "Rien"), tr("Windows' own", "Celui de Windows")),
    ];
    let tiles: Vec<_> = clicks
        .iter()
        .map(|(click, title, desc)| {
            let i = BELL_MIDDLE_CLICKS.iter().position(|m| m == click).unwrap_or(0);
            (Picture::Bell(*click), *title, *desc, cal.bell_middle_click == *click, Target::Choice(Choice::BellMiddle, i))
        })
        .collect();
    col.tiles(Some((tr("Middle click", "Clic du milieu"), tr("On the bell", "Sur la cloche"))), &tiles, bell_off, BUTTON_TILE_H);
    col.toggle(None, tr("How many wait", "Nombre en attente"), plain(tr("A count on the bell's corner; crossed out while Windows keeps them quiet", "Un compteur au coin de la cloche ; barré quand Windows les met en silence")), k.unread, bell_off, Toggle::ClockUnread);

    col.section(tr("Music", "Musique"));
    col.toggle(
        Some('\u{EC4F}'),
        tr("Follow YouTube Music", "Suivre YouTube Music"),
        plain(tr("Its progress on the player's taskbar button", "Sa progression sur le bouton du lecteur dans la barre des tâches")),
        c.music.enabled,
        false,
        Toggle::Music,
    );
    let styles = [
        (TileStyle::Off, tr("Off", "Désactivé"), tr("The player's own icon", "L'icône du lecteur")),
        (TileStyle::Cover, tr("Cover", "Pochette"), tr("The song's cover in its button", "La pochette du morceau dans son bouton")),
        (TileStyle::Full, tr("Full", "Complet"), tr("The cover and the title", "La pochette et le titre")),
    ];
    let tiles: Vec<_> = styles
        .iter()
        .enumerate()
        .map(|(i, (style, title, desc))| (Picture::PlayerButton(*style), *title, *desc, c.music.tile_style == *style, Target::Choice(Choice::Tile, i)))
        .collect();
    col.tiles(None, &tiles, !c.music.enabled, BUTTON_TILE_H);
}

/// A taskbar picture tile's height.
const BUTTON_TILE_H: f32 = 140.0;

/// A line of secondary text under what is above it.
fn note(col: &mut Column, text: &str) {
    col.laid.push(R::new(col.x + 2.0, col.y - 4.0, col.w, 18.0), Kind::Text(text.into()));
    col.y += 20.0;
}

/// The bell's middle clicks, in their tiles' order.
pub(super) const BELL_MIDDLE_CLICKS: [BellMiddleClick; 3] = [BellMiddleClick::DoNotDisturb, BellMiddleClick::ClearAll, BellMiddleClick::Nothing];

fn keyboards(col: &mut Column, model: &Model) {
    let c = &model.config;
    let k = &c.keyboards;
    col.toggle(
        Some('\u{E765}'),
        tr("A layout for each keyboard", "Une disposition par clavier"),
        plain(tr("Typing on a keyboard switches to its layout. Press Shift first: the first key still types in the old one", "Taper sur un clavier passe à sa disposition. Appuyez d'abord sur Maj : la première touche tape encore dans l'ancienne")),
        k.enabled,
        false,
        Toggle::Keyboards,
    );
    let off = !k.enabled;
    // Showing the layout on the pill is with the pill's other switches, on its page; hiding
    // Windows' layout icon, with the taskbar's buttons.

    col.section(tr("Your keyboards", "Vos claviers"));
    let labels: Vec<String> = model.layouts.iter().map(|(_, label)| label.clone()).collect();
    for (i, keyboard) in k.assigned.iter().enumerate() {
        let mut group = col.group();
        let is_connected = |path: &str| model.connected.iter().any(|known| known.eq_ignore_ascii_case(path));
        let paths: Vec<&str> = keyboard.paths().collect();
        let now = paths.iter().copied().find(|path| is_connected(path));
        let connected = now.is_some();
        let kind = |path: &str| crate::keyboards::connection(path).label();
        let state = match now {
            Some(path) => tf!("{} · connected", "{} · connecté", kind(path)),
            None if paths.len() == 1 => tf!("{} · not connected", "{} · non connecté", kind(paths[0])),
            None => tr("Not connected", "Non connecté").to_string(),
        };
        match &model.renaming {
            Some((at, text)) if *at == i => col.field_row(
                &mut group,
                text,
                vec![(tr("Save", "Enregistrer").into(), true, None, Button::SaveName), (tr("Cancel", "Annuler").into(), false, None, Button::CancelRename)],
            ),
            _ => col.row(
                &mut group,
                None,
                &keyboard.name,
                Some((state, if connected { Tone::Good } else { Tone::Plain })),
                off,
                Control::Buttons(vec![
                    (tr("Rename", "Renommer").into(), false, Some('\u{E8AC}'), Button::RenameKeyboard(i)),
                    (tr("Remove", "Supprimer").into(), false, Some('\u{E74D}'), Button::RemoveKeyboard(i)),
                ]),
            ),
        }
        let selected = keyboard.hkl().and_then(|hkl| model.layouts.iter().position(|(known, _)| *known == hkl));
        let desc = match selected {
            Some(_) => None,
            None => Some((tr("Its layout is no longer installed: pick another", "Sa disposition n'est plus installée : choisissez-en une autre").to_string(), Tone::Warn)),
        };
        // Every installed layout as a chip, wrapping: one click, however many there are.
        let picks: Vec<(String, bool, Target)> = labels.iter().enumerate().map(|(j, label)| (label.clone(), selected == Some(j), Target::Choice(Choice::KeyboardLayout(i), j))).collect();
        col.pick_row(&mut group, tr("Layout", "Disposition"), desc, &picks, off);
        // Its connections, once it has more than one: each can go on its own.
        if paths.len() > 1 {
            for (j, path) in paths.iter().enumerate() {
                let title = match crate::keyboards::vid_pid(path) {
                    Some(id) => format!("{} · {id}", kind(path)),
                    None => kind(path).to_string(),
                };
                let state = if is_connected(path) { (tr("Connected", "Connecté").to_string(), Tone::Good) } else { (tr("Not connected", "Non connecté").to_string(), Tone::Plain) };
                col.row(&mut group, None, &title, Some(state), off, Control::Buttons(vec![(tr("Remove", "Supprimer").into(), false, Some('\u{E74D}'), Button::RemoveConnection(i, j))]));
            }
        }
        let capturing_here = model.capture.is_some() && model.capture_into == Some(i);
        let control = if off || model.capture.is_some() { Control::None } else { Control::Button(tr("Add", "Ajouter"), false, Some('\u{E710}'), Button::AddConnection(i)) };
        let desc = if capturing_here { tr("Press a key on it, connected the new way…", "Appuyez sur une de ses touches, connecté de la nouvelle façon…") } else { tr("Its cable, receiver or Bluetooth: each is told apart", "Son câble, son récepteur ou le Bluetooth : chacun est distingué") };
        col.row(&mut group, None, tr("Another connection", "Autre connexion"), Some((desc.to_string(), if capturing_here { Tone::Warn } else { Tone::Plain })), off, control);
        col.end_group(group);
    }
    if off {
        col.card(Some('\u{E710}'), tr("Add a keyboard", "Ajouter un clavier"), plain(tr("Turn on a layout for each keyboard first", "Activez d'abord une disposition par clavier")), true, None);
    } else if let Some(hint) = &model.capture {
        let desc = match hint {
            Some(hint) => (hint.clone(), Tone::Warn),
            None => (tr("Only which keyboard it comes from is read, never the key", "Seul le clavier d'origine est lu, jamais la touche").to_string(), Tone::Plain),
        };
        let title = match model.capture_into.and_then(|i| k.assigned.get(i)) {
            Some(keyboard) => tf!("Press a key on \u{201C}{}\u{201D}, connected the new way…", "Appuyez sur une touche de « {} », connecté de la nouvelle façon…", keyboard.name),
            None => tr("Press a key on the keyboard to add…", "Appuyez sur une touche du clavier à ajouter…").to_string(),
        };
        let card = col.card(Some('\u{E765}'), title, Some(desc), false, None);
        col.button_in(card, tr("Cancel", "Annuler"), false, None, Button::CancelCapture);
    } else {
        let desc = if k.assigned.is_empty() { tr("Then press a key on it. Keyboards not added switch nothing", "Puis appuyez sur une de ses touches. Les claviers non ajoutés ne changent rien") } else { tr("Then press a key on it", "Puis appuyez sur une de ses touches") };
        col.button_card(Some('\u{E710}'), tr("Add a keyboard", "Ajouter un clavier"), plain(desc), tr("Add", "Ajouter"), true, Button::AddKeyboard);
    }
}

/// The labels for the places along `edge`.
pub(super) fn align_labels(edge: PillEdge) -> [&'static str; 3] {
    if edge.vertical() {
        [tr("Top", "Haut"), tr("Middle", "Milieu"), tr("Bottom", "Bas")]
    } else {
        [tr("Left", "Gauche"), tr("Middle", "Milieu"), tr("Right", "Droite")]
    }
}

pub(super) const MIDDLE_CLICKS: [MiddleClick; 3] = [MiddleClick::Mute, MiddleClick::Switch, MiddleClick::SwitchMute];
pub(super) const EDGES: [PillEdge; 3] = [PillEdge::Left, PillEdge::Top, PillEdge::Right];
pub(super) const ALIGNS: [PillAlign; 3] = [PillAlign::Start, PillAlign::Center, PillAlign::End];

fn pill(col: &mut Column, model: &Model) {
    let c = &model.config;
    let p = &c.pill;
    col.toggle(Some('\u{E7F4}'), tr("Show the edge pill", "Afficher la pastille latérale"), plain(tr("Music, calls and devices at a glance, on the edge of a screen", "Musique, appels et périphériques en un coup d'œil, au bord d'un écran")), p.enabled, false, Toggle::Pill);
    let off = !p.enabled;
    // The map is where the place is picked: click one of the dots along a screen's edge.
    col.section(tr("Place", "Position"));
    let map = R::new(col.x, col.y, col.w, MAP_H);
    col.laid.push(map, Kind::Map { dim: off });
    if !off {
        for (r, target) in map_targets(model, map) {
            col.laid.hits.push((r, target));
        }
    }
    col.y += MAP_H + CARD_GAP;
    // Automatic, then the displays as numbered on the map; a display not connected is automatic.
    let mut displays = vec![tr("Automatic", "Automatique").to_string()];
    displays.extend((1..=model.monitors.len()).map(|n| n.to_string()));
    let display = p.monitor.as_ref().and_then(|name| model.monitors.iter().position(|m| &m.name == name)).map_or(0, |i| i + 1);
    let auto = plain(tr("Automatic is the screen at that edge of the desktop", "Automatique : l'écran à ce bord du bureau"));
    col.segments(tr("Display", "Écran"), auto, &displays, display, off, Choice::Display);

    col.section(tr("Size at rest", "Taille au repos"));
    let sizes = [
        (PillSize::Hidden, tr("Hidden", "Masquée"), tr("Only a sliver on the edge", "Juste un liseré sur le bord")),
        (PillSize::Small, tr("Small", "Petite"), tr("Small covers and device icons", "Petites pochettes et icônes des périphériques")),
        (PillSize::Big, tr("Big", "Grande"), tr("Covers with rings, device buttons", "Pochettes avec anneaux, boutons des périphériques")),
    ];
    let tiles: Vec<_> = sizes.iter().enumerate().map(|(i, (size, title, desc))| (Picture::Size(*size), *title, *desc, p.size == *size, Target::Choice(Choice::PillSize, i))).collect();
    col.tiles(None, &tiles, off, SIZE_TILE_H);
    col.laid.push(R::new(col.x, col.y - 4.0, col.w, 20.0), Kind::Text(tr("Hovering the edge always opens the full card; warnings and alerts always show in full.", "Survoler le bord ouvre toujours la carte complète ; avertissements et alertes s'affichent toujours en entier.").into()));
    col.y += 24.0;

    // Everything that shows on the pill, gathered here: the keyboards' and the batteries' too.
    col.section(tr("On the pill", "Sur la pastille"));
    let k = &c.keyboards;
    let b = &c.batteries;
    col.toggle_grid(&[
        (tr("Audio devices", "Périphériques audio"), tr("Click one to switch to the next device", "Cliquez sur l'un pour passer au suivant"), p.devices, off, Toggle::PillDevices),
        (tr("Each new song", "Chaque nouveau morceau"), tr("Its title, for a moment", "Son titre, un instant"), p.announce_tracks, off, Toggle::AnnounceTracks),
        (tr("Alert when an app listens", "Alerte quand une appli écoute"), tr("When an app records outside a call", "Quand une appli enregistre hors appel"), p.listening_alerts, off, Toggle::ListeningAlerts),
        (tr("Keyboard layout changes", "Changements de disposition"), if k.enabled { tr("By keyboard or with Win+Space", "Par clavier ou avec Win+Espace") } else { tr("Needs a layout for each keyboard", "Nécessite une disposition par clavier") }, k.show_readout, off || !k.enabled, Toggle::KeyboardReadout),
        (tr("Low battery warning", "Alerte de batterie faible"), if b.enabled { tr("Amber, for a few seconds", "En orange, quelques secondes") } else { tr("Needs battery levels", "Nécessite les niveaux de batterie") }, b.pill_alert, off || !b.enabled, Toggle::BatteryPill),
        (tr("Battery levels on the card", "Niveaux de batterie sur la carte"), if b.enabled { tr("Under the devices, when you hover it", "Sous les périphériques, au survol") } else { tr("Needs battery levels", "Nécessite les niveaux de batterie") }, b.pill_card, off || !b.enabled, Toggle::BatteryPillCard),
    ]);
}

/// A size tile's height on the pill's page.
const SIZE_TILE_H: f32 = 156.0;

/// The displays scaled into `area`, keeping their arrangement: one rectangle per display.
pub(super) fn map_rects(model: &Model, area: R) -> Vec<R> {
    let ms = &model.monitors;
    if ms.is_empty() {
        return Vec::new();
    }
    let left = ms.iter().map(|m| m.rect.left).min().unwrap_or(0) as f32;
    let top = ms.iter().map(|m| m.rect.top).min().unwrap_or(0) as f32;
    let right = ms.iter().map(|m| m.rect.right).max().unwrap_or(1) as f32;
    let bottom = ms.iter().map(|m| m.rect.bottom).max().unwrap_or(1) as f32;
    let inner = R::new(area.x + 24.0, area.y + 20.0, area.w - 48.0, area.h - 56.0);
    let k = (inner.w / (right - left).max(1.0)).min(inner.h / (bottom - top).max(1.0));
    let (dx, dy) = (inner.x + (inner.w - (right - left) * k) / 2.0, inner.y + (inner.h - (bottom - top) * k) / 2.0);
    // A hairline apart, so displays side by side read as two.
    ms.iter()
        .map(|m| {
            let x = dx + (m.rect.left as f32 - left) * k;
            let y = dy + (m.rect.top as f32 - top) * k;
            R::new(x + 2.0, y + 2.0, (m.rect.right - m.rect.left) as f32 * k - 4.0, (m.rect.bottom - m.rect.top) as f32 * k - 4.0)
        })
        .collect()
}

/// Where the pill would sit on a display drawn at `screen`, at `edge` and `align`.
pub(super) fn slot_rect(screen: R, edge: PillEdge, align: PillAlign) -> R {
    const THICK: f32 = 6.0;
    let len = if edge.vertical() { (screen.h * 0.2).clamp(10.0, 36.0) } else { (screen.w * 0.14).clamp(10.0, 36.0) };
    let span = if edge.vertical() { (screen.y, screen.h) } else { (screen.x, screen.w) };
    // Clear of the corners, where the other edges' places are.
    let margin = THICK + 8.0;
    let at = match align {
        PillAlign::Start => span.0 + margin,
        PillAlign::Center => span.0 + (span.1 - len) / 2.0,
        PillAlign::End => span.0 + span.1 - margin - len,
    };
    match edge {
        PillEdge::Left => R::new(screen.x + 3.0, at, THICK, len),
        PillEdge::Right => R::new(screen.right() - 3.0 - THICK, at, THICK, len),
        PillEdge::Top => R::new(at, screen.y + 3.0, len, THICK),
    }
}

/// The map's click targets: each display, then the nine places on it (on top, padded to be easy
/// to hit).
fn map_targets(model: &Model, map: R) -> Vec<(R, Target)> {
    let mut targets = Vec::new();
    for (i, screen) in map_rects(model, map).into_iter().enumerate() {
        targets.push((screen, Target::Monitor(i)));
        for edge in EDGES {
            for align in ALIGNS {
                let s = slot_rect(screen, edge, align);
                targets.push((R::new(s.x - 6.0, s.y - 6.0, s.w + 12.0, s.h + 12.0), Target::Slot(i, edge, align)));
            }
        }
    }
    targets
}

fn devices(col: &mut Column, model: &Model) {
    for (flow, title) in [(crate::audio::Flow::Output, tr("Output", "Sortie")), (crate::audio::Flow::Input, tr("Input", "Entrée"))] {
        let rows: Vec<usize> = (0..model.devices.len()).filter(|&i| model.devices[i].flow == flow).collect();
        if rows.is_empty() {
            continue;
        }
        col.section(title);
        for i in rows {
            let d = &model.devices[i];
            let open = model.expanded == Some(i);
            let desc = d.default.then(|| (tr("Default device", "Périphérique par défaut").to_string(), Tone::Plain));
            let card = col.card(None, d.name.clone(), desc, false, Some(Target::Device(i)));
            col.laid.push(R::new(card.x + CARD_PAD, card.y, 24.0, CARD_H), Kind::Icon(d.icon));
            let label = if open { tr("Done", "Terminé") } else { tr("Change icon", "Changer l'icône") };
            let r = R::new(card.right() - CARD_PAD - 28.0 - 140.0, card.y, 140.0, CARD_H);
            col.laid.push(r, Kind::Value(label.to_string()));
            let chevron = R::new(card.right() - CARD_PAD - 20.0, card.y, 20.0, CARD_H);
            col.laid.push(chevron, Kind::Trailing(if open { '\u{E70E}' } else { '\u{E70D}' }));
            if open {
                icon_grid(col, i, d.icon);
            }
        }
    }
}

/// Windows' own sound settings, at the end of the audio devices.
fn windows_sound(col: &mut Column) {
    col.section("Windows");
    col.link(Some('\u{E713}'), tr("Windows sound settings", "Paramètres de son de Windows"), plain(tr("Volume levels, device properties, spatial sound", "Niveaux de volume, propriétés des périphériques, son spatial")), '\u{E8A7}', Button::SoundSettings);
}

/// The batteries: the devices reporting one first, each with its level, then where they show and
/// when one counts as low.
fn batteries(col: &mut Column, model: &Model) {
    let b = &model.config.batteries;
    col.toggle(
        Some('\u{E83F}'),
        tr("Battery levels", "Niveaux de batterie"),
        plain(tr("Keyboards, mice, headsets and other wireless devices that report one", "Claviers, souris, casques et autres appareils sans fil qui la signalent")),
        b.enabled,
        false,
        Toggle::Batteries,
    );
    let off = !b.enabled;
    col.section(tr("Connected now", "Connectés maintenant"));
    let batteries = &model.status.batteries;
    if batteries.is_empty() {
        let why = if off { tr("Turned off", "Désactivé") } else { tr("Bluetooth keyboards, mice and headsets report one once connected", "Les claviers, souris et casques Bluetooth la signalent une fois connectés") };
        col.card(None, tr("No device reports a battery", "Aucun appareil ne signale de batterie"), plain(why), true, None);
    }
    // Lowest first: the one to charge.
    let mut sorted: Vec<_> = batteries.iter().collect();
    sorted.sort_by_key(|x| x.level);
    for battery in sorted {
        let low = battery.level <= b.low_at;
        let desc = low.then(|| (tr("Low: charge it soon", "Faible : rechargez-le bientôt").to_string(), Tone::Warn));
        let card = col.card(Some(battery.kind.glyph()), battery.name.clone(), desc, off, None);
        let r = R::new(card.right() - CARD_PAD - LEVEL_W, card.y, LEVEL_W, CARD_H);
        col.laid.push(r, Kind::Level { level: battery.level, low });
    }
    col.section(tr("Where they show", "Où ils s'affichent"));
    col.toggle_grid(&[
        (tr("In the panel", "Dans le panneau"), tr("A Batteries section under the audio devices", "Une section Batteries sous les périphériques audio"), b.flyout, off, Toggle::BatteryFlyout),
        (tr("In the tray icon's tooltip", "Dans l'info-bulle de l'icône"), tr("Lowest first", "Le plus faible en premier"), b.tooltip, off, Toggle::BatteryTooltip),
        (tr("Windows notification when low", "Notification Windows si faible"), tr("Once, until it is charged again", "Une fois, jusqu'à la prochaine charge"), b.toast, off, Toggle::BatteryToast),
    ]);
    col.laid.push(R::new(col.x, col.y - 4.0, col.w, 20.0), Kind::Text(tr("The pill's warning and the levels on its card are with the pill's other switches, on Edge pill.", "L'alerte de la pastille et les niveaux sur sa carte sont avec ses autres options, page Pastille latérale.").into()));
    col.y += 24.0;
    let labels: Vec<String> = crate::config::Batteries::LOW_LEVELS.iter().map(|level| format!("{level}%")).collect();
    let at = crate::config::Batteries::LOW_LEVELS.iter().position(|level| *level == b.low_at).unwrap_or(2);
    col.segments(tr("Low at", "Faible à"), plain(tr("At or under this level", "À ce niveau ou en dessous")), &labels, at, off, Choice::LowBattery);
}

/// A battery level's bar and number.
const LEVEL_W: f32 = 180.0;

/// The icons for device `device`, wrapping, in a card under its own.
fn icon_grid(col: &mut Column, device: usize, current: IconId) {
    let inner = col.w - 2.0 * CARD_PAD - CARD_TEXT_X + CARD_PAD;
    let per_row = (((inner + CHIP_GAP) / (CHIP + CHIP_GAP)).floor() as usize).max(1);
    let rows = IconId::ALL.len().div_ceil(per_row);
    let h = CARD_PAD * 2.0 + rows as f32 * CHIP + (rows as f32 - 1.0) * CHIP_GAP;
    let card = R::new(col.x, col.y, col.w, h);
    col.laid.push(card, Kind::Card { glyph: None, title: String::new(), desc: None, dim: false, target: None });
    for (k, icon) in IconId::ALL.into_iter().enumerate() {
        let (row, c) = (k / per_row, k % per_row);
        let r = R::new(card.x + CARD_TEXT_X + c as f32 * (CHIP + CHIP_GAP), card.y + CARD_PAD + row as f32 * (CHIP + CHIP_GAP), CHIP, CHIP);
        let target = Target::Icon(device, icon);
        col.laid.push(r, Kind::Chip { icon, selected: icon == current, target });
        col.laid.hits.push((r, target));
    }
    col.y += h + CARD_GAP;
}

/// A connector's state, as its card's second line says it.
fn off() -> (String, Tone) {
    (tr("Off", "Désactivé").to_string(), Tone::Plain)
}

fn not_set_up(why: &str) -> (String, Tone) {
    (tf!("Not set up: {why}", "Non configuré : {why}"), Tone::Warn)
}

fn set_up(how: &str) -> (String, Tone) {
    (how.to_string(), Tone::Good)
}

/// "A, B and C".
fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => tf!("{} and {last}", "{} et {last}", rest.join(", ")),
    }
}

/// A connector's card in the grid: its glyph, name, state, switch, and which one it is.
struct ConnectorCard {
    glyph: char,
    title: &'static str,
    state: (String, Tone),
    on: bool,
    toggle: Toggle,
    connector: Connector,
}

/// The connectors two by two, each with its state and its switch; the one opened shows its steps
/// under the grid.
fn connectors(col: &mut Column, model: &Model) {
    use crate::integrations::{browser, discord, discord_rpc, teams, teams_ui};
    let c = &model.config;
    let s = &model.status;
    let connected = |name: &str| s.integrations.iter().any(|(n, on)| *n == name && *on);

    // Edge freezes a paused player it has put to sleep: play from here does nothing until its window
    // is opened. Only while one of the players is not on its "never sleep" list.
    if !model.edge_sleep.is_empty() {
        let names = join_names(&model.edge_sleep);
        let card = col.card(
            Some('\u{E7BA}'),
            tf!("Edge can put {names} to sleep", "Edge peut mettre {names} en veille"),
            Some((tr("Paused a while, play then does nothing until you open it. In Edge: Never put these sites to sleep", "Après une pause, la lecture ne fait rien tant que vous ne l'ouvrez pas. Dans Edge : Ne jamais mettre ces sites en veille").to_string(), Tone::Warn)),
            false,
            None,
        );
        col.button_in(card, tr("Open Edge settings", "Ouvrir les paramètres d'Edge"), false, Some('\u{E8A7}'), Button::EdgeSleep);
        col.gap(8.0);
    }

    let mut cards = Vec::new();
    let state = match (c.browser.enabled, connected(browser::NAME)) {
        (false, _) => off(),
        (true, true) => set_up(tr("Connected", "Connecté")),
        (true, false) => not_set_up(tr("the extension", "l'extension")),
    };
    cards.push(ConnectorCard { glyph: '\u{E774}', title: tr("Browser tabs", "Onglets du navigateur"), state, on: c.browser.enabled, toggle: Toggle::Browser, connector: Connector::Browser });
    // A player that shares what it plays only once told to; no card where it is not installed.
    if let Some(shares) = model.mpc {
        let state = match (c.mpc.enabled, shares) {
            (false, _) => off(),
            (true, true) => set_up(tr("Followed like any player", "Suivi comme tout lecteur")),
            (true, false) => not_set_up(tr("its media controls", "ses contrôles multimédias")),
        };
        cards.push(ConnectorCard { glyph: '\u{E714}', title: "MPC-HC", state, on: c.mpc.enabled, toggle: Toggle::Mpc, connector: Connector::Mpc });
    }
    let discord_on = connected(discord_rpc::NAME) || connected(discord::NAME);
    let keys = c.discord.client_id.is_some() && c.discord.client_secret.is_some();
    let state = match (c.discord.enabled, discord_on, keys && s.discord_problem.is_none()) {
        (false, _, _) => off(),
        (true, true, _) => set_up(tr("Connected", "Connecté")),
        (true, false, true) => (tr("Set up, waiting for Discord", "Configuré, en attente de Discord").to_string(), Tone::Plain),
        (true, false, false) => not_set_up(if keys { tr("see its steps", "voir ses étapes") } else { tr("an application of your own", "une application à vous") }),
    };
    cards.push(ConnectorCard { glyph: '\u{E8BD}', title: tr("Discord calls", "Appels Discord"), state, on: c.discord.enabled, toggle: Toggle::Discord, connector: Connector::Discord });
    let (api, window, paired) = (connected(teams::NAME), connected(teams_ui::NAME), c.teams.token.is_some());
    let state = match (c.teams.enabled, api, paired, window) {
        (false, ..) => off(),
        (true, true, true, _) => set_up(tr("Connected", "Connecté")),
        (true, _, _, true) => set_up(tr("Following the meeting window", "Suit la fenêtre de réunion")),
        (true, true, false, _) => not_set_up(tr("allow it in a meeting", "autorisez-le en réunion")),
        (true, false, true, false) => (tr("Set up, waiting for Teams", "Configuré, en attente de Teams").to_string(), Tone::Plain),
        (true, false, false, false) => not_set_up(tr("its API is off", "son API est désactivée")),
    };
    cards.push(ConnectorCard { glyph: '\u{E716}', title: tr("Teams meetings", "Réunions Teams"), state, on: c.teams.enabled, toggle: Toggle::Teams, connector: Connector::Teams });

    col.connector_grid(&cards, model.open);
    if let Some(card) = model.open.and_then(|open| cards.iter().find(|card| card.connector == open)) {
        col.section(card.title);
        connector_steps(col, model, card.connector);
    } else {
        col.laid.push(R::new(col.x, col.y, col.w, 20.0), Kind::Text(tr("Click a connector for its steps and options.", "Cliquez sur un connecteur pour ses étapes et options.").into()));
        col.y += 24.0;
    }
}

/// The Calendar page (the clock's panel): where its events come from first (the calendars found,
/// the accounts, the links), then how it looks, then its reminders. Whether the clock opens it is
/// with the clock, on the Taskbar page.
fn calendar_steps(col: &mut Column, model: &Model) {
    use crate::calendar::Provider;
    let c = &model.config.calendar;
    let s = &model.calendar;
    let hint = |button| model.hint.filter(|(b, _, _)| *b == button).map(|(_, h, tone)| (h.to_string(), tone));
    let dim = !c.enabled;
    if dim {
        let why = plain(tr("Turn it on with the clock, on the Taskbar page", "Activez-le avec l'horloge, page Barre des tâches"));
        col.card(Some('\u{E787}'), tr("The clock opens Windows' calendar", "L'horloge ouvre le calendrier de Windows"), why, false, Some(Target::Nav(Page::Taskbar)));
        col.gap(8.0);
    }

    // Each calendar found: shown or not, and its colour.
    if !s.calendars.is_empty() {
        col.section(tr("Calendars", "Agendas"));
        let mut body = col.group();
        for (i, calendar) in s.calendars.iter().enumerate() {
            let shown = !c.hidden.contains(&calendar.key);
            let chosen = c.colors.get(&calendar.key).and_then(|hex| crate::calendar::parse_hex(hex));
            col.calendar_row(&mut body, i, &calendar.name, calendar.color, chosen, shown, dim);
        }
        col.end_group(body);
    }

    for provider in Provider::ALL {
        let account = s.account(provider);
        let google = provider == Provider::Google;
        col.section(if google { tr("Google account", "Compte Google") } else { tr("Microsoft account (Outlook, work or school)", "Compte Microsoft (Outlook, professionnel ou scolaire)") });
        let mut body = col.group();
        let sign_in = Button::Cal(Cal::SignIn(provider));
        let cancel = Button::Cal(Cal::CancelSignIn(provider));
        if let Some(email) = &account.email {
            let (desc, buttons) = match &account.problem {
                Some(problem) => (
                    (problem.clone(), Tone::Warn),
                    vec![(tr("Sign in again", "Se reconnecter").to_string(), true, None, sign_in), (tr("Sign out", "Se déconnecter").to_string(), false, None, Button::Cal(Cal::SignOut(provider)))],
                ),
                None => ((tr("Signed in", "Connecté").to_string(), Tone::Good), vec![(tr("Sign out", "Se déconnecter").to_string(), false, None, Button::Cal(Cal::SignOut(provider)))]),
            };
            let control = if account.signing_in { Control::Button(tr("Cancel", "Annuler"), false, None, cancel) } else { Control::Buttons(buttons) };
            let desc = if account.signing_in { (tr("Finish signing in in your browser", "Terminez la connexion dans votre navigateur").to_string(), Tone::Plain) } else { desc };
            col.row(&mut body, None, email, Some(desc), false, control);
            col.end_group(body);
            continue;
        }
        let registered = if google { c.google_client_id.is_some() && c.google_client_secret.is_some() } else { c.microsoft_client_id.is_some() };
        if google {
            let api = plain(tr("Enable it, then set up the consent screen: External, and Publish it", "Activez-la, puis configurez l'écran de consentement : Externe, et publiez-le"));
            col.row(&mut body, Some((1, registered)), tr("Enable the Calendar API", "Activer l'API Calendar"), api, false, Control::Button(tr("Open", "Ouvrir"), false, Some('\u{E8A7}'), Button::Cal(Cal::GoogleApi)));
            let client = plain(tr("Credentials > Create > OAuth client ID > Desktop app", "Identifiants > Créer > ID client OAuth > Application de bureau"));
            col.row(&mut body, Some((2, registered)), tr("Create a desktop client", "Créer un client de bureau"), client, false, Control::Button(tr("Open", "Ouvrir"), false, Some('\u{E8A7}'), Button::Cal(Cal::GoogleCredentials)));
            let id = hint(Button::Cal(Cal::GoogleId)).or_else(|| c.google_client_id.clone().map(|id| (id, Tone::Plain)));
            col.row(&mut body, None, tr("Client ID", "ID client"), id.or(plain(tr("Not set", "Non défini"))), false, Control::Button(tr("Paste", "Coller"), false, Some('\u{E77F}'), Button::Cal(Cal::GoogleId)));
            let secret = hint(Button::Cal(Cal::GoogleSecret)).or_else(|| c.google_client_secret.as_deref().map(|s| (tail(s), Tone::Plain)));
            col.row(&mut body, None, tr("Client secret", "Secret client"), secret.or(plain(tr("Not set", "Non défini"))), false, Control::Button(tr("Paste", "Coller"), false, Some('\u{E77F}'), Button::Cal(Cal::GoogleSecret)));
        } else {
            let register = plain(tr("New registration: any organisation + personal; then Calendars.Read", "Nouvelle inscription : toute organisation + personnel ; puis Calendars.Read"));
            col.row(&mut body, Some((1, registered)), tr("Register an app", "Inscrire une application"), register, false, Control::Button(tr("Open", "Ouvrir"), false, Some('\u{E8A7}'), Button::Cal(Cal::MicrosoftPortal)));
            let redirect = hint(Button::Cal(Cal::MicrosoftRedirect)).or(plain(tr("Authentication > Mobile and desktop applications: http://localhost", "Authentification > Applications mobiles et de bureau : http://localhost")));
            col.row(&mut body, Some((2, false)), tr("Add the redirect", "Ajouter la redirection"), redirect, false, Control::Button(tr("Copy", "Copier"), false, Some('\u{E8C8}'), Button::Cal(Cal::MicrosoftRedirect)));
            let id = hint(Button::Cal(Cal::MicrosoftId)).or_else(|| c.microsoft_client_id.clone().map(|id| (id, Tone::Plain)));
            col.row(&mut body, None, tr("Application (client) ID", "ID d'application (client)"), id.or(plain(tr("Not set", "Non défini"))), false, Control::Button(tr("Paste", "Coller"), false, Some('\u{E77F}'), Button::Cal(Cal::MicrosoftId)));
        }
        let (desc, control) = if account.signing_in {
            (plain(tr("Finish signing in in your browser", "Terminez la connexion dans votre navigateur")), Control::Button(tr("Cancel", "Annuler"), false, None, cancel))
        } else {
            let desc = account.problem.clone().map(|p| (p, Tone::Warn)).or(plain(tr("Opens the sign-in in your browser", "Ouvre la connexion dans votre navigateur")));
            (desc, Control::Button(tr("Sign in", "Se connecter"), registered, None, sign_in))
        };
        col.row(&mut body, Some((3, false)), tr("Sign in", "Se connecter"), desc, !registered, control);
        col.end_group(body);
    }

    col.section(tr("Calendar links", "Liens d'agenda"));
    let mut body = col.group();
    for (i, link) in c.links.iter().enumerate() {
        let desc = match s.links.get(i).cloned().flatten() {
            Some(problem) => (problem, Tone::Warn),
            None => (link_host(&link.url), Tone::Plain),
        };
        col.row(&mut body, None, &link.name, Some(desc), false, Control::Button(tr("Remove", "Supprimer"), false, Some('\u{E74D}'), Button::Cal(Cal::RemoveLink(i))));
    }
    let add = hint(Button::Cal(Cal::AddLink))
        .or(plain(tr("A secret iCal address: Google's Integrate calendar, Outlook's Publish", "Une adresse iCal secrète : Intégrer l'agenda de Google, Publier d'Outlook")));
    col.row(&mut body, None, tr("Add a link", "Ajouter un lien"), add, false, Control::Button(tr("Paste link", "Coller le lien"), c.links.is_empty(), Some('\u{E77F}'), Button::Cal(Cal::AddLink)));
    col.end_group(body);

    col.section(tr("How it looks", "Apparence"));
    let mut body = col.group();
    let layout_at = CALENDAR_LAYOUTS.iter().position(|l| *l == c.layout).unwrap_or(0);
    col.row(&mut body, None, tr("Layout", "Disposition"), None, dim, Control::Segments(vec![tr("Comfortable", "Confortable").into(), "Compact".into(), tr("Agenda only", "Agenda seul").into()], layout_at, Choice::CalendarLayout));
    let rows_at = AGENDA_ROWS.iter().position(|r| *r == c.rows).unwrap_or(1);
    col.row(&mut body, None, tr("Agenda rows", "Lignes de l'agenda"), plain(tr("At most; fewer on a short screen", "Au plus ; moins sur un petit écran")), dim, Control::Segments(AGENDA_ROWS.iter().map(|r| r.to_string()).collect(), rows_at, Choice::AgendaRows));
    let week_at = match c.first_weekday {
        None => 0,
        Some(0) => 1,
        Some(_) => 2,
    };
    col.row(&mut body, None, tr("Week starts", "Début de semaine"), None, dim, Control::Segments(vec![tr("As Windows", "Comme Windows").into(), tr("Monday", "Lundi").into(), tr("Sunday", "Dimanche").into()], week_at, Choice::WeekStart));
    col.row(&mut body, None, tr("Week numbers", "Numéros de semaine"), None, dim, Control::Toggle(c.week_numbers, Toggle::WeekNumbers));
    let clocks = hint(Button::Cal(Cal::EditClocks)).or(plain(tr("Windows' additional clocks, under the date", "Les horloges supplémentaires de Windows, sous la date")));
    col.row(&mut body, None, tr("World clocks", "Horloges supplémentaires"), clocks, dim, Control::Toggle(c.world_clocks, Toggle::WorldClocks));
    col.row(&mut body, None, tr("Choose the world clocks", "Choisir les horloges supplémentaires"), plain(tr("In Windows' Date and time settings", "Dans les paramètres Date et heure de Windows")), dim, Control::Button(tr("Open", "Ouvrir"), false, Some('\u{E8A7}'), Button::Cal(Cal::EditClocks)));
    col.end_group(body);

    col.section(tr("Reminders", "Rappels"));
    let mut body = col.group();
    let remind_at = REMINDERS.iter().position(|r| *r == c.remind_minutes).unwrap_or(2);
    let labels = REMINDERS.iter().map(|r| if *r == 0 { tr("Off", "Désactivé").to_string() } else { format!("{r} min") }).collect();
    // A notification under the bell when the bell is Audio Tray's, else on the pill.
    let (remind, how) = if c.notifications {
        (tr("Remind me", "Me rappeler"), tr("Before an event, a notification with Join", "Avant un événement, une notification avec Rejoindre"))
    } else {
        (tr("Remind me on the pill", "Me rappeler sur la pastille"), tr("Before an event, with Join", "Avant un événement, avec Rejoindre"))
    };
    col.row(&mut body, None, remind, plain(how), dim, Control::Segments(labels, remind_at, Choice::Reminder));
    col.end_group(body);
}

/// The Notifications page (the bell's panel): whether the bell opens Audio Tray's, whether Windows
/// lets it read them, then keeping them quiet: now, in meetings and calls, the focus timer.
fn notifications_page(col: &mut Column, model: &Model) {
    let c = &model.config.calendar;
    let s = &model.calendar;
    let off = !c.enabled || !c.notifications;
    let mut body = col.group();
    let what = plain(tr("Windows' bell beside the clock: every app's notifications and Audio Tray's own (batteries, keyboards, audio devices, reminders)", "La cloche de Windows à côté de l'horloge : les notifications de toutes les applis et celles d'Audio Tray (batteries, claviers, sorties audio, rappels)"));
    col.row(&mut body, None, tr("The bell opens Audio Tray's notifications", "La cloche ouvre les notifications d'Audio Tray"), what, !c.enabled, Control::Toggle(c.notifications, Toggle::CalendarNotifications));
    let banners = plain(tr(
        "Its banners over the bell replace Windows': each app's banners are turned off in Windows' settings, and back on with this",
        "Ses bannières au-dessus de la cloche remplacent celles de Windows : les bannières de chaque appli sont désactivées dans les paramètres de Windows, et réactivées avec ce réglage",
    ));
    col.row(&mut body, None, tr("Audio Tray shows every notification", "Audio Tray affiche toutes les notifications"), banners, off, Control::Toggle(c.take_over, Toggle::TakeOver));
    if !c.enabled {
        col.row(&mut body, None, tr("Needs Audio Tray's calendar", "Nécessite l'agenda d'Audio Tray"), plain(tr("Turn it on with the clock, on the Taskbar page", "Activez-le avec l'horloge, page Barre des tâches")), false, Control::None);
    } else if s.notifications_denied {
        let why = Some((tr("Windows does not let Audio Tray read them", "Windows ne laisse pas Audio Tray les lire").to_string(), Tone::Warn));
        col.row(&mut body, None, tr("Allow Audio Tray", "Autoriser Audio Tray"), why, false, Control::Button(tr("Open", "Ouvrir"), true, Some('\u{E8A7}'), Button::AllowNotifications));
    }
    col.end_group(body);

    col.section(tr("Quiet", "Silence"));
    let mut body = col.group();
    let now = if s.quiet { (tr("On: Windows keeps notifications quiet", "Activé : Windows met les notifications en silence").to_string(), Tone::Good) } else { (tr("Off", "Désactivé").to_string(), Tone::Plain) };
    let switch = if s.quiet { tr("Turn off", "Désactiver") } else { tr("Turn on", "Activer") };
    col.row(&mut body, None, tr("Do not disturb", "Ne pas déranger"), Some(now), false, Control::Button(switch, false, Some('\u{E708}'), Button::Quiet));
    col.row(&mut body, None, tr("Do not disturb in meetings", "Ne pas déranger en réunion"), plain(tr("While a calendar event runs", "Pendant un événement de l'agenda")), !c.enabled, Control::Toggle(c.quiet_in_meetings, Toggle::QuietMeetings));
    col.row(&mut body, None, tr("Do not disturb in calls", "Ne pas déranger en appel"), plain(tr("Teams, Discord, or a meeting app on the microphone", "Teams, Discord, ou une appli de réunion sur le micro")), !c.enabled, Control::Toggle(c.quiet_in_calls, Toggle::QuietCalls));
    let focus_at = FOCUS_LENGTHS.iter().position(|f| *f == c.focus_minutes).unwrap_or(1);
    let labels = FOCUS_LENGTHS.iter().map(|f| format!("{f} min")).collect();
    col.row(&mut body, None, tr("Focus timer", "Minuteur de concentration"), plain(tr("Its length; notifications stay quiet while it runs", "Sa durée ; les notifications restent silencieuses pendant ce temps")), off, Control::Segments(labels, focus_at, Choice::FocusLength));
    let rules = plain(tr("A full-screen app, a game, a duplicated display: the bell and the moon follow them too", "Une appli en plein écran, un jeu, un écran dupliqué : la cloche et la lune les suivent aussi"));
    col.row(&mut body, None, tr("Windows' own rules", "Règles de Windows"), rules, false, Control::Button(tr("Open", "Ouvrir"), false, Some('\u{E8A7}'), Button::QuietRules));
    col.end_group(body);
}

/// The calendar's layouts, in their segments' order.
pub(super) const CALENDAR_LAYOUTS: [crate::config::CalendarLayout; 3] =
    [crate::config::CalendarLayout::Comfortable, crate::config::CalendarLayout::Compact, crate::config::CalendarLayout::AgendaOnly];
pub(super) const AGENDA_ROWS: [u8; 3] = [3, 5, 8];
/// Minutes before an event; 0 is off.
pub(super) const REMINDERS: [u32; 4] = [0, 1, 5, 10];
pub(super) const FOCUS_LENGTHS: [u32; 4] = [15, 25, 45, 60];
/// The colours a calendar can take, after its own.
pub(super) const SWATCHES: [[u8; 3]; 8] = [
    [0xE8, 0x83, 0x6A],
    [0xF7, 0xB3, 0x2B],
    [0x6C, 0xCB, 0x5F],
    [0x4F, 0xC3, 0xF7],
    [0x47, 0x9E, 0xF5],
    [0x7B, 0x86, 0xC6],
    [0xC3, 0x9B, 0xF5],
    [0xF0, 0x62, 0x92],
];

impl Column<'_> {
    /// A calendar's row: its name, its colours to pick from (its own first), and its switch.
    #[allow(clippy::too_many_arguments)]
    fn calendar_row(&mut self, group: &mut Group, i: usize, name: &str, own: [u8; 3], chosen: Option<[u8; 3]>, shown: bool, dim: bool) {
        if group.rows > 0 {
            self.laid.push(R::new(self.x, self.y, self.w, 1.0), Kind::Divider);
        }
        group.rows += 1;
        let row = R::new(self.x, self.y, self.w, ROW_H);
        let switch = self.toggle_in(row, row.right() - CARD_PAD, shown, dim, Toggle::CalendarShown(i));
        if !dim {
            self.laid.hits.push((R::new(row.right() - CARD_PAD - switch, row.y, switch, row.h), Target::Toggle(Toggle::CalendarShown(i))));
        }
        const DOT: f32 = 18.0;
        const STEP: f32 = 24.0;
        let colours: Vec<[u8; 3]> = std::iter::once(own).chain(SWATCHES).collect();
        let mut x = row.right() - CARD_PAD - switch - 20.0 - colours.len() as f32 * STEP;
        let current = chosen.unwrap_or(own);
        for (j, rgb) in colours.iter().enumerate() {
            let r = R::new(x + (STEP - DOT) / 2.0, row.y + (row.h - DOT) / 2.0, DOT, DOT);
            let target = Target::Swatch(i, j);
            let selected = if j == 0 { chosen.is_none() } else { *rgb == current && chosen.is_some() };
            self.laid.push(r, Kind::Swatch { rgb: *rgb, selected, target });
            if !dim {
                self.laid.hits.push((r, target));
            }
            x += STEP;
        }
        let end = row.right() - CARD_PAD - switch - 20.0 - colours.len() as f32 * STEP - 12.0;
        let tx = row.x + CARD_TEXT_X;
        self.laid.push(R::new(tx, row.y, end - tx, ROW_H), Kind::Label { title: name.to_string(), desc: None, dim: dim || !shown });
        self.y += ROW_H;
    }
}

/// A secret's last characters: it is never shown whole.
fn tail(secret: &str) -> String {
    let tail: String = secret.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("••••{tail}")
}

/// Where a link points, without the secret part of its address.
fn link_host(url: &str) -> String {
    let url = crate::calendar::feed_url(url);
    let host = url.split("://").nth(1).and_then(|rest| rest.split('/').next()).unwrap_or_default();
    format!("{host}/••••")
}

/// What an opened connector shows under the grid: its setup steps until it is set up, then what can
/// still be changed.
fn connector_steps(col: &mut Column, model: &Model, connector: Connector) {
    use crate::integrations::{browser, discord, discord_rpc, teams};
    let c = &model.config;
    let s = &model.status;
    let connected = |name: &str| s.integrations.iter().any(|(n, on)| *n == name && *on);
    let hint = |button| model.hint.filter(|(b, _, _)| *b == button).map(|(_, h, tone)| (h.to_string(), tone));
    let mut body = col.group();
    match connector {
        Connector::Browser => {
            let dim = !c.browser.enabled;
            if !connected(browser::NAME) {
                let why = hint(Button::ExtensionFolder).or(plain(tr("Opens the extensions page, and the folder to Load unpacked (its path copied)", "Ouvre la page des extensions, et le dossier à charger décompressé (son chemin copié)")));
                col.row(&mut body, Some((1, false)), tr("Install the extension", "Installer l'extension"), why, dim, Control::Button(tr("Open", "Ouvrir"), true, None, Button::ExtensionFolder));
                col.row(&mut body, Some((2, false)), tr("Play something in a tab", "Lisez quelque chose dans un onglet"), plain(tr("The extension connects with the first video or song", "L'extension se connecte à la première vidéo ou au premier morceau")), dim, Control::None);
            }
            let labels = vec![tr("Every site", "Tous les sites").to_string(), tr("Chosen sites", "Sites choisis").to_string()];
            col.row(&mut body, None, tr("Tabs followed", "Onglets suivis"), plain(tr("Every tab that plays, or only the sites below", "Chaque onglet qui joue, ou seulement les sites ci-dessous")), dim, Control::Segments(labels, usize::from(!c.browser.every_site), Choice::BrowserTabs));
            // One switch per site, so more can be offered later.
            for (i, (key, name, _)) in browser::SITES.iter().enumerate() {
                let on = c.browser.sites.iter().any(|s| s == key);
                col.row(&mut body, None, name, None, dim || c.browser.every_site, Control::Toggle(on, Toggle::Site(i)));
            }
        }
        Connector::Mpc => {
            let dim = !c.mpc.enabled;
            if model.mpc == Some(true) {
                col.row(&mut body, None, tr("Its Windows media controls are on", "Ses contrôles multimédias Windows sont activés"), plain(tr("What it plays is shared with Windows; Off here only leaves it out of Audio Tray", "Ce qu'il lit est partagé avec Windows ; Désactivé ici l'écarte seulement d'Audio Tray")), dim, Control::None);
            } else {
                col.row(&mut body, Some((1, false)), tr("Close MPC-HC", "Fermer MPC-HC"), plain(tr("It saves its own settings as it closes, over any change made meanwhile", "Il enregistre ses paramètres en se fermant, par-dessus toute modification faite entre-temps")), dim, Control::None);
                let why = hint(Button::Mpc).or(plain(tr("Its Options > Player > Control via Windows UI (SMTC)", "Ses Options > Player > Control via Windows UI (SMTC)")));
                col.row(&mut body, Some((2, false)), tr("Turn on its Windows media controls", "Activer ses contrôles multimédias Windows"), why, dim, Control::Button(tr("Turn on", "Activer"), true, None, Button::Mpc));
            }
        }
        Connector::Discord => {
            let dim = !c.discord.enabled;
            let discord_on = connected(discord_rpc::NAME) || connected(discord::NAME);
            let has_id = c.discord.client_id.is_some();
            let keys = has_id && c.discord.client_secret.is_some();
            if !(keys && s.discord_problem.is_none()) && !discord_on {
                if let Some(problem) = s.discord_problem.filter(|_| keys) {
                    col.row(&mut body, None, tr("Not connected yet", "Pas encore connecté"), Some((problem.to_string(), Tone::Warn)), dim, Control::None);
                }
                let create = plain(tr("Discord shares calls only with an app of your own: make one on its developer portal", "Discord ne partage les appels qu'avec votre propre appli : créez-en une sur son portail développeur"));
                col.row(&mut body, Some((1, has_id)), tr("Create an application", "Créer une application"), create, dim, Control::Button(tr("Open portal", "Ouvrir le portail"), !has_id, Some('\u{E8A7}'), Button::DiscordPortal));
                let redirect = hint(Button::DiscordRedirect).or(plain(&tf!("In OAuth2 > Redirects, add {}", "Dans OAuth2 > Redirects, ajoutez {}", discord_rpc::REDIRECT)));
                col.row(&mut body, Some((2, false)), tr("Add the redirect", "Ajouter la redirection"), redirect, dim, Control::Button(tr("Copy", "Copier"), false, Some('\u{E8C8}'), Button::DiscordRedirect));
                col.row(&mut body, Some((3, keys)), tr("Paste its keys", "Coller ses clés"), plain(tr("From OAuth2: copy each one, then press its Paste", "Depuis OAuth2 : copiez chacune, puis appuyez sur son Coller")), dim, Control::None);
            }
            let id = hint(Button::DiscordId).or_else(|| c.discord.client_id.clone().map(|id| (id, Tone::Plain)));
            col.row(&mut body, None, tr("Application ID", "ID d'application"), id.or(plain(tr("Not set", "Non défini"))), dim, Control::Button(tr("Paste", "Coller"), false, Some('\u{E77F}'), Button::DiscordId));
            let secret = hint(Button::DiscordSecret).or_else(|| model.secret_tail().map(|t| (t, Tone::Plain)));
            col.row(&mut body, None, tr("Client secret", "Secret client"), secret.or(plain(tr("Not set", "Non défini"))), dim, Control::Button(tr("Paste", "Coller"), false, Some('\u{E77F}'), Button::DiscordSecret));
        }
        Connector::Teams => {
            let dim = !c.teams.enabled;
            if c.teams.token.is_some() {
                col.row(&mut body, None, tr("Audio Tray is allowed", "Audio Tray est autorisé"), plain(tr("Teams asked once, in a meeting", "Teams l'a demandé une fois, en réunion")), dim, Control::None);
            } else {
                if let Some(problem) = s.teams_problem {
                    col.row(&mut body, None, tr("Not connected yet", "Pas encore connecté"), Some((problem.to_string(), Tone::Warn)), dim, Control::None);
                }
                col.row(&mut body, Some((1, connected(teams::NAME))), tr("Turn on Teams' API", "Activer l'API de Teams"), plain(tr("In Teams: Settings > Privacy > Third-party app API", "Dans Teams : Paramètres > Confidentialité > API d'application tierce")), dim, Control::None);
                col.row(&mut body, Some((2, false)), tr("Allow Audio Tray", "Autoriser Audio Tray"), plain(tr("In a meeting, when Teams asks", "En réunion, quand Teams le demande")), dim, Control::None);
                col.row(&mut body, None, tr("Or nothing at all", "Ou rien du tout"), plain(tr("Without its API, Teams' meeting window is followed: the mute, not who talks", "Sans son API, la fenêtre de réunion de Teams est suivie : le micro coupé, pas qui parle")), dim, Control::None);
            }
        }
    }
    col.end_group(body);
}

/// The app: its logo, version and whether an update waits, then where it keeps things.
fn about(col: &mut Column, model: &Model) {
    let hero = R::new(col.x, col.y, col.w, ABOUT_H);
    col.laid.push(hero, Kind::Card { glyph: None, title: String::new(), desc: None, dim: false, target: None });
    col.laid.push(R::new(hero.x + CARD_PAD + 4.0, hero.y + (ABOUT_H - 64.0) / 2.0, 64.0, 64.0), Kind::Logo);
    let x = hero.x + CARD_PAD + 4.0 + 64.0 + 20.0;
    let version = format!("Version {}", env!("CARGO_PKG_VERSION"));
    let state = match &model.update {
        Some(next) => (tf!("{version}; {next} is ready", "{version} ; la {next} est prête"), Tone::Good),
        None => (tf!("{version}; up to date", "{version} ; à jour"), Tone::Plain),
    };
    let mut right = hero.right() - CARD_PAD;
    if model.update.is_some() {
        let label = tr("Restart to update", "Redémarrer pour mettre à jour");
        let w = button_width(col.measure, label, None);
        let r = R::new(right - w, hero.y + (ABOUT_H - CONTROL_H) / 2.0, w, CONTROL_H);
        let target = Target::Button(Button::Restart);
        col.laid.push(r, Kind::Button { label: label.to_string(), accent: true, glyph: None, target });
        col.laid.hits.push((r, target));
        right -= w + 16.0;
    }
    col.laid.push(R::new(x, hero.y + (ABOUT_H - 46.0) / 2.0, right - x, 46.0), Kind::Label { title: "Audio Tray".into(), desc: Some(state), dim: false });
    col.y += ABOUT_H + CARD_GAP;
    col.section(tr("More", "Plus"));
    col.link(Some('\u{E71B}'), tr("Releases and source", "Versions et code source"), plain("github.com/michelsalib/audio-tray"), '\u{E8A7}', Button::Releases);
    col.link(Some('\u{E8B7}'), tr("Settings folder", "Dossier des paramètres"), plain(tr("Where config.toml is kept", "Là où se trouve config.toml")), '\u{E8A7}', Button::ConfigFolder);
    col.button_card(Some('\u{E7E8}'), tr("Quit Audio Tray", "Quitter Audio Tray"), plain(tr("Puts the taskbar back as Windows draws it", "Rend la barre des tâches telle que Windows la dessine")), tr("Quit", "Quitter"), false, Button::Quit);
}

/// About's header card.
const ABOUT_H: f32 = 104.0;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::settings::model::{DeviceRow, Model};
    use crate::win::Monitor;
    use windows::Win32::Foundation::RECT;

    fn measure(text: &str) -> f32 {
        text.len() as f32 * 7.0
    }

    fn monitor(name: &str, left: i32, top: i32, w: i32, h: i32, primary: bool) -> Monitor {
        let rect = RECT { left, top, right: left + w, bottom: top + h };
        Monitor { name: name.to_string(), id: name.to_string(), rect, work: rect, primary }
    }

    fn model(page: Page) -> Model {
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

    #[test]
    fn every_page_lays_out_inside_its_width() {
        for page in Page::ALL {
            let m = model(page);
            for width in [500.0, 900.0, 2000.0] {
                let laid = super::page(&m, width, &measure);
                assert!(laid.height > TITLE_H, "{page:?} is empty");
                for item in &laid.items {
                    assert!(item.r.x >= 0.0 && item.r.right() <= width.max(200.0 + 2.0 * PAGE_PAD) + 0.5, "{page:?} {item:?} overflows {width}");
                }
            }
        }
    }

    #[test]
    fn a_switch_is_hit_anywhere_on_its_card() {
        let laid = page(&model(Page::Pill), 900.0, &measure);
        let card = laid.items.iter().find(|i| matches!(&i.kind, Kind::Card { title, .. } if title == "Show the edge pill")).unwrap();
        assert_eq!(laid.hit(card.r.x + 10.0, card.r.y + 10.0), Some(Target::Toggle(Toggle::Pill)));
        let switch = laid.items.iter().find(|i| matches!(i.kind, Kind::Toggle { toggle: Toggle::Pill, .. })).unwrap();
        assert_eq!(laid.hit(switch.r.x + 2.0, switch.r.y + 2.0), Some(Target::Toggle(Toggle::Pill)));
    }

    #[test]
    fn choices_are_hit_where_they_are_drawn_and_not_while_dimmed() {
        let mut m = model(Page::Pill);
        let laid = page(&m, 900.0, &measure);
        // The size, as picture tiles.
        for i in 0..3 {
            let tile = laid.items.iter().find(|it| matches!(it.kind, Kind::Tile { target: Target::Choice(Choice::PillSize, j), .. } if j == i)).unwrap();
            assert_eq!(laid.hit(tile.r.x + tile.r.w / 2.0, tile.r.y + tile.r.h / 2.0), Some(Target::Choice(Choice::PillSize, i)));
        }
        m.config.pill.enabled = false;
        let laid = page(&m, 900.0, &measure);
        assert!(!laid.hits.iter().any(|(_, t)| matches!(t, Target::Choice(..) | Target::Slot(..) | Target::Monitor(_))));
        // On Look & motion, the volume sliders' style stays: they are the panel's and the readout's.
        m.page = Page::Motion;
        let laid = page(&m, 900.0, &measure);
        assert!(laid.hits.iter().any(|(_, t)| matches!(t, Target::Choice(Choice::VolumeMotion, _))));
        assert!(!laid.hits.iter().any(|(_, t)| matches!(t, Target::Choice(Choice::RingMotion, _))));
    }

    #[test]
    fn the_map_keeps_the_displays_apart_and_its_places_on_them() {
        let m = model(Page::Pill);
        let area = R::new(0.0, 0.0, 800.0, MAP_H);
        let screens = map_rects(&m, area);
        assert_eq!(screens.len(), 2);
        assert!(screens[0].right() <= screens[1].x, "side by side, not overlapping");
        assert!(screens[1].y > screens[0].y, "the second sits lower, as arranged");
        for screen in &screens {
            assert!(screen.x >= area.x && screen.right() <= area.right() && screen.bottom() <= area.bottom());
            for edge in EDGES {
                for align in ALIGNS {
                    let s = slot_rect(*screen, edge, align);
                    assert!(s.x >= screen.x && s.right() <= screen.right() && s.y >= screen.y && s.bottom() <= screen.bottom(), "{edge:?} {align:?}");
                }
            }
        }
        // A place wins over the display under it.
        let laid = page(&m, 900.0, &measure);
        let map = laid.items.iter().find(|i| matches!(i.kind, Kind::Map { .. })).unwrap().r;
        let screen = map_rects(&m, map)[1];
        let s = slot_rect(screen, PillEdge::Right, PillAlign::End);
        assert_eq!(laid.hit(s.x + 1.0, s.y + 1.0), Some(Target::Slot(1, PillEdge::Right, PillAlign::End)));
        assert_eq!(laid.hit(screen.x + screen.w / 2.0, screen.y + screen.h / 2.0), Some(Target::Monitor(1)));
    }

    #[test]
    fn an_open_device_shows_every_icon_once() {
        let mut m = model(Page::Devices);
        m.expanded = Some(0);
        let laid = page(&m, 700.0, &measure);
        let icons: Vec<IconId> = laid.hits.iter().filter_map(|(_, t)| match t {
            Target::Icon(0, icon) => Some(*icon),
            _ => None,
        }).collect();
        assert_eq!(icons, IconId::ALL.to_vec());
    }

    #[test]
    fn a_grouped_card_holds_its_rows_and_their_buttons_work() {
        let mut m = model(Page::Connectors);
        m.open = Some(Connector::Discord);
        let laid = page(&m, 900.0, &measure);
        let copy = laid.items.iter().find(|i| matches!(i.kind, Kind::Button { target: Target::Button(Button::DiscordRedirect), .. })).unwrap().r;
        assert_eq!(laid.hit(copy.x + 4.0, copy.y + 4.0), Some(Target::Button(Button::DiscordRedirect)));
        let group = laid.items.iter().find(|i| matches!(&i.kind, Kind::Card { title, .. } if title.is_empty()) && i.r.contains(copy.x, copy.y)).unwrap().r;
        let rows: Vec<R> = laid
            .items
            .iter()
            .filter(|i| matches!(i.kind, Kind::Label { .. }) && i.r.y >= group.y && i.r.bottom() <= group.bottom() + 0.5)
            .map(|i| i.r)
            .collect();
        assert_eq!(rows.len(), 5, "three steps, two keys");
        let beside = rows.iter().find(|r| r.contains(r.x, copy.y + copy.h / 2.0)).unwrap();
        assert!(beside.right() <= copy.x, "a row's text stops before its button");
    }

    fn keyboards_model() -> Model {
        let mut m = model(Page::Keyboards);
        m.config.keyboards.enabled = true;
        let keyboard = |path: &str, name: &str, hkl: &str| crate::config::AssignedKeyboard { device_path: path.into(), name: name.into(), hkl: hkl.into(), ..Default::default() };
        m.config.keyboards.assigned = vec![keyboard(r"\\?\ACPI#HPQ8002#x#{guid}", "Laptop", "040C040C"), keyboard(r"\\?\HID#VID_19F5&PID_2620#x#{guid}", "External", "0409040C")];
        m.layouts = vec![(0x040C_040C, "AZERTY".into()), (0x0409_040C, "US".into())];
        m.connected = vec![r"\\?\ACPI#HPQ8002#x#{guid}".into()];
        m
    }

    #[test]
    fn each_keyboard_picks_its_layout_and_shows_whether_it_is_connected() {
        let m = keyboards_model();
        for width in [500.0, 900.0, 2000.0] {
            let laid = page(&m, width, &measure);
            for item in &laid.items {
                assert!(item.r.x >= 0.0 && item.r.right() <= width.max(200.0 + 2.0 * PAGE_PAD) + 0.5, "{item:?} overflows {width}");
            }
        }
        let laid = page(&m, 900.0, &measure);
        // The second keyboard is on US: its second chip is the selected one, and hits.
        let us = laid.items.iter().find(|i| matches!(i.kind, Kind::Pick { target: Target::Choice(Choice::KeyboardLayout(1), 1), .. })).unwrap();
        assert!(matches!(us.kind, Kind::Pick { selected: true, .. }));
        assert_eq!(laid.hit(us.r.x + 4.0, us.r.y + 4.0), Some(Target::Choice(Choice::KeyboardLayout(1), 1)));
        let states: Vec<String> = laid.items.iter().filter_map(|i| match &i.kind {
            Kind::Label { desc: Some((desc, _)), .. } if desc.contains("connected") => Some(desc.clone()),
            _ => None,
        }).collect();
        assert_eq!(states, ["Built-in · connected", "USB · not connected"]);
        assert!(laid.hits.iter().any(|(_, t)| *t == Target::Button(Button::AddKeyboard)));
    }

    #[test]
    fn a_keyboard_with_several_connections_lists_each_and_is_connected_by_any() {
        let mut m = keyboards_model();
        let cable = r"\\?\HID#VID_3151&PID_4015&MI_00#x#{guid}";
        let ble = r"\\?\HID#{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&023151_PID&4013#x#{guid}";
        m.config.keyboards.assigned[1].more_paths = vec![cable.into(), ble.into()];
        m.connected.push(cable.into());
        let laid = page(&m, 900.0, &measure);
        let titles: Vec<&str> = laid.items.iter().filter_map(|i| match &i.kind {
            Kind::Label { title, .. } => Some(title.as_str()),
            _ => None,
        }).collect();
        for title in ["USB · 19F5:2620", "USB · 3151:4015", "Bluetooth · 3151:4013"] {
            assert!(titles.contains(&title), "{title} in {titles:?}");
        }
        // The laptop has one connection: no list for it.
        assert!(!laid.hits.iter().any(|(_, t)| matches!(t, Target::Button(Button::RemoveConnection(0, _)))));
        assert!(laid.hits.iter().any(|(_, t)| *t == Target::Button(Button::RemoveConnection(1, 2))));
        assert!(laid.hits.iter().any(|(_, t)| *t == Target::Button(Button::AddConnection(0))));
        assert!(laid.items.iter().any(|i| matches!(&i.kind, Kind::Label { desc: Some((d, Tone::Good)), .. } if d == "USB · connected")));
        for width in [500.0, 2000.0] {
            for item in &page(&m, width, &measure).items {
                assert!(item.r.x >= 0.0 && item.r.right() <= width.max(200.0 + 2.0 * PAGE_PAD) + 0.5, "{item:?} overflows {width}");
            }
        }
        // While a key is awaited, no other capture can start.
        m.capture = Some(None);
        m.capture_into = Some(1);
        let laid = page(&m, 900.0, &measure);
        assert!(!laid.hits.iter().any(|(_, t)| matches!(t, Target::Button(Button::AddConnection(_)))));
    }

    #[test]
    fn a_keyboard_forgets_one_connection_but_keeps_its_last() {
        let mut k = crate::config::AssignedKeyboard { device_path: "a".into(), more_paths: vec!["b".into(), "c".into()], ..Default::default() };
        k.remove_path(0);
        assert_eq!(k.paths().collect::<Vec<_>>(), ["b", "c"]);
        k.remove_path(1);
        assert_eq!(k.paths().collect::<Vec<_>>(), ["b"]);
        k.remove_path(0);
        assert_eq!(k.paths().collect::<Vec<_>>(), ["b"]);
        assert!(k.has("B"));
    }

    #[test]
    fn many_layouts_wrap_and_each_is_one_click() {
        let mut m = keyboards_model();
        m.layouts = (0..8).map(|i| (0x0409_0400 + i, format!("A rather long layout name {i}"))).collect();
        let laid = page(&m, 900.0, &measure);
        let chips: Vec<&Item> = laid.items.iter().filter(|i| matches!(i.kind, Kind::Pick { target: Target::Choice(Choice::KeyboardLayout(0), _), .. })).collect();
        assert_eq!(chips.len(), 8);
        assert!(chips.iter().any(|c| c.r.y > chips[0].r.y), "they wrap onto another line");
        for (j, chip) in chips.iter().enumerate() {
            assert_eq!(laid.hit(chip.r.x + 4.0, chip.r.y + 4.0), Some(Target::Choice(Choice::KeyboardLayout(0), j)));
        }
        // The second keyboard's layout is not installed any more: said so.
        assert!(laid.items.iter().any(|i| matches!(&i.kind, Kind::Label { desc: Some((d, Tone::Warn)), .. } if d.contains("no longer installed"))));
    }

    #[test]
    fn a_name_being_typed_is_a_field_beside_its_buttons() {
        let mut m = keyboards_model();
        m.renaming = Some((0, "Lap".into()));
        let laid = page(&m, 700.0, &measure);
        let field = laid.items.iter().find(|i| matches!(&i.kind, Kind::Field(t) if t == "Lap")).unwrap().r;
        let save = laid.items.iter().find(|i| matches!(i.kind, Kind::Button { target: Target::Button(Button::SaveName), .. })).unwrap().r;
        assert!(field.right() <= save.x, "the field stops before Save");
        assert!(!laid.hits.iter().any(|(_, t)| *t == Target::Button(Button::RenameKeyboard(0))));
    }

    #[test]
    fn a_capture_waits_with_a_way_out_and_nothing_is_added_while_off() {
        let mut m = keyboards_model();
        m.capture = Some(None);
        let laid = page(&m, 900.0, &measure);
        assert!(laid.hits.iter().any(|(_, t)| *t == Target::Button(Button::CancelCapture)));
        assert!(!laid.hits.iter().any(|(_, t)| *t == Target::Button(Button::AddKeyboard)));
        m.capture = None;
        m.config.keyboards.enabled = false;
        let laid = page(&m, 900.0, &measure);
        assert!(!laid.hits.iter().any(|(_, t)| matches!(t, Target::Button(Button::AddKeyboard) | Target::Choice(Choice::KeyboardLayout(_), _))));
    }

    #[test]
    fn the_calendar_turns_on_from_off() {
        // Off is the default: its own switch, with the clock's, must still take a click (it once
        // dimmed itself).
        let mut m = model(Page::Taskbar);
        m.config.calendar.enabled = false;
        let laid = page(&m, 900.0, &measure);
        assert!(laid.hits.iter().any(|(_, t)| *t == Target::Toggle(Toggle::Calendar)));
        // The Calendar page says where.
        m.page = Page::Calendar;
        let laid = page(&m, 900.0, &measure);
        assert!(laid.hits.iter().any(|(_, t)| *t == Target::Nav(Page::Taskbar)));
    }

    #[test]
    fn every_taskbar_button_has_its_section_and_the_bell_its_page() {
        let mut m = model(Page::Taskbar);
        m.config.calendar.enabled = true;
        let laid = page(&m, 900.0, &measure);
        let sections: Vec<&str> = laid.items.iter().filter_map(|i| if let Kind::Section(s) = i.kind { Some(s) } else { None }).collect();
        assert_eq!(&sections[..3], ["Audio buttons", "Clock", "Bell"]);
        assert!(laid.hits.iter().any(|(_, t)| matches!(t, Target::Choice(Choice::BellMiddle, _))));
        m.page = Page::Notifications;
        m.calendar.notifications_denied = true;
        let laid = page(&m, 900.0, &measure);
        assert!(laid.hits.iter().any(|(_, t)| *t == Target::Button(Button::AllowNotifications)));
        assert!(laid.hits.iter().any(|(_, t)| *t == Target::Button(Button::Quiet)));
        assert_eq!(attention(&m, Page::Notifications), 1);
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

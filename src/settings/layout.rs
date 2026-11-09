//! Pure layout for the settings window: each page as [`Item`]s to paint and [`Target`]s to hit, in
//! DIPs. No Win32 here, so it stays unit-testable; text is measured through a function the caller
//! passes in.

use crate::config::{LeftClick, PillAlign, PillEdge, PillSize, TileStyle};
use crate::icons::IconId;

use super::model::{Model, Tone};

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
/// "On" / "Off" beside a switch.
pub(super) const TOGGLE_LABEL_W: f32 = 36.0;
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
    Taskbar,
    Pill,
    Devices,
    Batteries,
    Keyboards,
    Connectors,
    About,
}

impl Page {
    pub(super) const ALL: [Page; 7] = [Page::Taskbar, Page::Pill, Page::Devices, Page::Batteries, Page::Keyboards, Page::Connectors, Page::About];

    pub(super) fn title(self) -> &'static str {
        match self {
            Page::Taskbar => "Taskbar",
            Page::Pill => "Edge pill",
            Page::Devices => "Devices",
            Page::Batteries => "Batteries",
            Page::Keyboards => "Keyboards",
            Page::Connectors => "Connectors",
            Page::About => "About",
        }
    }

    pub(super) fn glyph(self) -> char {
        match self {
            Page::Taskbar => '\u{E75B}',
            Page::Pill => '\u{E7F4}',
            Page::Devices => '\u{E767}',
            Page::Batteries => '\u{E83F}',
            Page::Keyboards => '\u{E765}',
            Page::Connectors => '\u{E71B}',
            Page::About => '\u{E946}',
        }
    }
}

/// A switch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Toggle {
    /// Follow MPC-HC's session.
    Mpc,
    Strip,
    /// The device rotation's muted stop.
    MuteInCycle,
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
}

/// A segmented choice.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Choice {
    Tile,
    /// What a left click on a taskbar audio button does: [`LEFT_CLICKS`].
    LeftClick,
    PillSize,
    Edge,
    Align,
    /// Every tab, or only the players'.
    BrowserTabs,
    /// Automatic, then each display in [`crate::win::monitors`]' order.
    Display,
    /// The low-battery level: [`crate::config::Batteries::LOW_LEVELS`].
    LowBattery,
    /// The layout of assigned keyboard `i`: the index is into [`super::model::Model::layouts`].
    KeyboardLayout(usize),
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
    /// The next installed layout, when they are too many for a segmented choice.
    NextLayout(usize),
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

/// The navigation pane, fixed (it does not scroll with the page).
pub(super) fn nav() -> Laid {
    let mut laid = Laid::default();
    for (i, page) in Page::ALL.into_iter().enumerate() {
        let r = R::new(NAV_INSET, NAV_TOP + i as f32 * NAV_STEP, NAV_W - 2.0 * NAV_INSET, NAV_ITEM_H);
        laid.hits.push((r, Target::Nav(page)));
    }
    laid.height = NAV_TOP + Page::ALL.len() as f32 * NAV_STEP;
    laid
}

/// The navigation item's rectangle for `page`.
pub(super) fn nav_rect(page: Page) -> R {
    let i = Page::ALL.iter().position(|p| *p == page).unwrap_or(0);
    R::new(NAV_INSET, NAV_TOP + i as f32 * NAV_STEP, NAV_W - 2.0 * NAV_INSET, NAV_ITEM_H)
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
        TOGGLE_W + TOGGLE_LABEL_W + 8.0
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

    /// A connector's card: its state on the second line, its switch, and a chevron; clicked, it
    /// opens on the rows that follow ([`Column::body`]).
    fn expander(&mut self, glyph: char, title: &str, state: (String, Tone), switch: Option<(bool, Toggle)>, open: bool, connector: Connector) {
        let card = self.card(Some(glyph), title, Some(state), false, Some(Target::Connector(connector)));
        let chevron = R::new(card.right() - CARD_PAD - 20.0, card.y, 20.0, CARD_H);
        self.laid.push(chevron, Kind::Trailing(if open { '\u{E70E}' } else { '\u{E70D}' }));
        if let Some((on, toggle)) = switch {
            let r = R::new(chevron.x - 16.0 - TOGGLE_W, card.y + (CARD_H - TOGGLE_H) / 2.0, TOGGLE_W, TOGGLE_H);
            self.toggle_in(card, r.right(), on, false, toggle);
            // The switch, not the card, under the pointer: it flips without opening.
            self.laid.hits.push((R::new(r.x - 4.0, r.y - 6.0, r.w + 8.0, r.h + 12.0), Target::Toggle(toggle)));
        }
    }

    /// The opened part of an expander, right under its header.
    fn body(&mut self) -> Group {
        self.y -= CARD_GAP - 1.0;
        self.group()
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
        Page::Taskbar => taskbar(&mut col, model),
        Page::Pill => pill(&mut col, model),
        Page::Devices => devices(&mut col, model),
        Page::Batteries => batteries(&mut col, model),
        Page::Keyboards => keyboards(&mut col, model),
        Page::Connectors => connectors(&mut col, model),
        Page::About => about(&mut col, model),
    }
    col.laid.height = col.y + PAGE_PAD;
    col.laid
}

fn plain(text: &str) -> Option<(String, Tone)> {
    Some((text.to_string(), Tone::Plain))
}

fn taskbar(col: &mut Column, model: &Model) {
    let c = &model.config;
    if let Some(notice) = model.status.notice {
        col.card(Some('\u{E7BA}'), "The taskbar needs attention", Some((notice.to_string(), Tone::Warn)), false, None);
        col.gap(8.0);
    }
    col.section("Audio buttons");
    col.toggle(
        Some('\u{E767}'),
        "Audio buttons in the tray",
        plain("Output and input buttons drawn into the taskbar, over Audio Tray's icon"),
        c.strip.enabled,
        false,
        Toggle::Strip,
    );
    let off = !c.strip.enabled;
    col.segments(
        "Left click on a button",
        plain("What the output or input button does"),
        &["Switch device", "Mute", "Open panel"],
        LEFT_CLICKS.iter().position(|l| *l == c.strip.left_click).unwrap_or(0),
        off,
        Choice::LeftClick,
    );
    col.toggle(
        None,
        "Mute after the last device",
        plain("Clicking goes through each device, then mutes, then starts over"),
        c.strip.mute_in_cycle,
        off || c.strip.left_click != LeftClick::Switch,
        Toggle::MuteInCycle,
    );
    col.toggle(None, "Next to Wi-Fi", plain("Move the notification icons between the keyboard layout and Wi-Fi"), c.strip.next_to_wifi, off, Toggle::NextToWifi);
    col.toggle(None, "Hide Windows' volume icon", plain("The output button does its job"), c.strip.hide_windows_volume, off, Toggle::HideVolume);
    col.toggle(None, "Hide Windows' microphone icon", plain("The input button's red dot says an app is listening"), c.strip.hide_windows_mic, off, Toggle::HideMic);
    col.section("Music");
    col.toggle(
        Some('\u{EC4F}'),
        "Follow YouTube Music",
        plain("Its progress on the player's taskbar button"),
        c.music.enabled,
        false,
        Toggle::Music,
    );
    let selected = match c.music.tile_style {
        TileStyle::Off => 0,
        TileStyle::Cover => 1,
        TileStyle::Full => 2,
    };
    col.segments(
        "Music in the player's button",
        plain("The cover, or the cover with the title, drawn into the player's taskbar button"),
        &["Off", "Cover", "Full"],
        selected,
        !c.music.enabled,
        Choice::Tile,
    );
}

/// A segmented choice gives way to a button that steps through the layouts past this width.
const LAYOUT_SEGMENTS_MAX_W: f32 = 520.0;

fn keyboards(col: &mut Column, model: &Model) {
    let c = &model.config;
    let k = &c.keyboards;
    col.toggle(
        Some('\u{E765}'),
        "A layout for each keyboard",
        plain("Typing on a keyboard switches to its layout. Press Shift first: the first key still types in the old one"),
        k.enabled,
        false,
        Toggle::Keyboards,
    );
    let off = !k.enabled;
    col.toggle(None, "Show the layout on the pill", plain("When it changes, by keyboard or with Win+Space"), k.show_readout, off, Toggle::KeyboardReadout);
    // Hidden by the code drawn into the taskbar, so it needs the buttons or the music tile.
    let injects = c.injects();
    let note = if injects { "Win+Space still switches layouts" } else { "Needs the taskbar's audio buttons or the music tile" };
    col.toggle(None, "Hide Windows' keyboard layout icon", plain(note), c.strip.hide_windows_language, !injects, Toggle::HideLanguage);

    col.section("Your keyboards");
    let labels: Vec<String> = model.layouts.iter().map(|(_, label)| label.clone()).collect();
    for (i, keyboard) in k.assigned.iter().enumerate() {
        let mut group = col.group();
        let is_connected = |path: &str| model.connected.iter().any(|known| known.eq_ignore_ascii_case(path));
        let paths: Vec<&str> = keyboard.paths().collect();
        let now = paths.iter().copied().find(|path| is_connected(path));
        let connected = now.is_some();
        let kind = |path: &str| crate::keyboards::connection(path).label();
        let state = match now {
            Some(path) => format!("{} · connected", kind(path)),
            None if paths.len() == 1 => format!("{} · not connected", kind(paths[0])),
            None => "Not connected".to_string(),
        };
        match &model.renaming {
            Some((at, text)) if *at == i => col.field_row(
                &mut group,
                text,
                vec![("Save".into(), true, None, Button::SaveName), ("Cancel".into(), false, None, Button::CancelRename)],
            ),
            _ => col.row(
                &mut group,
                None,
                &keyboard.name,
                Some((state, if connected { Tone::Good } else { Tone::Plain })),
                off,
                Control::Buttons(vec![
                    ("Rename".into(), false, Some('\u{E8AC}'), Button::RenameKeyboard(i)),
                    ("Remove".into(), false, Some('\u{E74D}'), Button::RemoveKeyboard(i)),
                ]),
            ),
        }
        let selected = keyboard.hkl().and_then(|hkl| model.layouts.iter().position(|(known, _)| *known == hkl));
        let desc = match selected {
            Some(_) => None,
            None => Some(("Its layout is no longer installed: pick another".to_string(), Tone::Warn)),
        };
        let fits = labels.iter().map(|l| ((col.measure)(l) + 28.0).max(48.0)).sum::<f32>() <= LAYOUT_SEGMENTS_MAX_W.min(col.w - 200.0);
        let control = if fits {
            Control::Segments(labels.clone(), selected.unwrap_or(usize::MAX), Choice::KeyboardLayout(i))
        } else {
            let current = selected.map_or("Pick a layout", |at| labels[at].as_str()).to_string();
            Control::Buttons(vec![(current, false, Some('\u{E8AB}'), Button::NextLayout(i))])
        };
        col.row(&mut group, None, "Layout", desc, off, control);
        // Its connections, once it has more than one: each can go on its own.
        if paths.len() > 1 {
            for (j, path) in paths.iter().enumerate() {
                let title = match crate::keyboards::vid_pid(path) {
                    Some(id) => format!("{} · {id}", kind(path)),
                    None => kind(path).to_string(),
                };
                let state = if is_connected(path) { ("Connected".to_string(), Tone::Good) } else { ("Not connected".to_string(), Tone::Plain) };
                col.row(&mut group, None, &title, Some(state), off, Control::Buttons(vec![("Remove".into(), false, Some('\u{E74D}'), Button::RemoveConnection(i, j))]));
            }
        }
        let capturing_here = model.capture.is_some() && model.capture_into == Some(i);
        let control = if off || model.capture.is_some() { Control::None } else { Control::Button("Add", false, Some('\u{E710}'), Button::AddConnection(i)) };
        let desc = if capturing_here { "Press a key on it, connected the new way…" } else { "Its cable, receiver or Bluetooth: each is told apart" };
        col.row(&mut group, None, "Another connection", Some((desc.to_string(), if capturing_here { Tone::Warn } else { Tone::Plain })), off, control);
        col.end_group(group);
    }
    if off {
        col.card(Some('\u{E710}'), "Add a keyboard", plain("Turn on a layout for each keyboard first"), true, None);
    } else if let Some(hint) = &model.capture {
        let desc = match hint {
            Some(hint) => (hint.clone(), Tone::Warn),
            None => ("Only which keyboard it comes from is read, never the key".to_string(), Tone::Plain),
        };
        let title = match model.capture_into.and_then(|i| k.assigned.get(i)) {
            Some(keyboard) => format!("Press a key on \u{201C}{}\u{201D}, connected the new way…", keyboard.name),
            None => "Press a key on the keyboard to add…".to_string(),
        };
        let card = col.card(Some('\u{E765}'), title, Some(desc), false, None);
        col.button_in(card, "Cancel", false, None, Button::CancelCapture);
    } else {
        let desc = if k.assigned.is_empty() { "Then press a key on it. Keyboards not added switch nothing" } else { "Then press a key on it" };
        col.button_card(Some('\u{E710}'), "Add a keyboard", plain(desc), "Add", true, Button::AddKeyboard);
    }
}

/// The labels for the places along `edge`.
pub(super) fn align_labels(edge: PillEdge) -> [&'static str; 3] {
    if edge.vertical() {
        ["Top", "Middle", "Bottom"]
    } else {
        ["Left", "Middle", "Right"]
    }
}

pub(super) const LEFT_CLICKS: [LeftClick; 3] = [LeftClick::Switch, LeftClick::Mute, LeftClick::Panel];
pub(super) const EDGES: [PillEdge; 3] = [PillEdge::Left, PillEdge::Top, PillEdge::Right];
pub(super) const ALIGNS: [PillAlign; 3] = [PillAlign::Start, PillAlign::Center, PillAlign::End];

fn pill(col: &mut Column, model: &Model) {
    let p = &model.config.pill;
    col.toggle(Some('\u{E7F4}'), "Show the edge pill", plain("Music, calls and devices at a glance, on the edge of a screen"), p.enabled, false, Toggle::Pill);
    let off = !p.enabled;
    col.section("Position");
    let map = R::new(col.x, col.y, col.w, MAP_H);
    col.laid.push(map, Kind::Map { dim: off });
    if !off {
        for (r, target) in map_targets(model, map) {
            col.laid.hits.push((r, target));
        }
    }
    col.y += MAP_H + CARD_GAP;
    // Automatic, then the displays as numbered on the map; a display not connected is automatic.
    let mut displays = vec!["Automatic".to_string()];
    displays.extend((1..=model.monitors.len()).map(|n| n.to_string()));
    let display = p.monitor.as_ref().and_then(|name| model.monitors.iter().position(|m| &m.name == name)).map_or(0, |i| i + 1);
    let auto = plain("Automatic is the screen at that edge of the desktop");
    col.segments("Display", auto, &displays, display, off, Choice::Display);
    let edge = EDGES.iter().position(|e| *e == p.edge).unwrap_or(0);
    col.segments("Edge", None, &["Left", "Top", "Right"], edge, off, Choice::Edge);
    let align = ALIGNS.iter().position(|a| *a == p.align).unwrap_or(1);
    col.segments("Place on the edge", None, &align_labels(p.edge), align, off, Choice::Align);
    col.section("Appearance");
    let size = match p.size {
        PillSize::Hidden => 0,
        PillSize::Small => 1,
        PillSize::Big => 2,
    };
    col.segments("Size at rest", plain("Hovering the edge always opens the full card"), &["Hidden", "Small", "Big"], size, off, Choice::PillSize);
    col.toggle(None, "Audio devices on the pill", plain("Click one to switch to the next device"), p.devices, off, Toggle::PillDevices);
    col.toggle(None, "Show each new song", plain("Its title, for a moment"), p.announce_tracks, off, Toggle::AnnounceTracks);
    col.toggle(None, "Alert when an app listens", plain("When an app records outside a call"), p.listening_alerts, off, Toggle::ListeningAlerts);
}

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
    for (flow, title) in [(crate::audio::Flow::Output, "Output"), (crate::audio::Flow::Input, "Input")] {
        let rows: Vec<usize> = (0..model.devices.len()).filter(|&i| model.devices[i].flow == flow).collect();
        if rows.is_empty() {
            continue;
        }
        col.section(title);
        for i in rows {
            let d = &model.devices[i];
            let open = model.expanded == Some(i);
            let desc = d.default.then(|| ("Default device".to_string(), Tone::Plain));
            let card = col.card(None, d.name.clone(), desc, false, Some(Target::Device(i)));
            col.laid.push(R::new(card.x + CARD_PAD, card.y, 24.0, CARD_H), Kind::Icon(d.icon));
            let label = if open { "Done" } else { "Change icon" };
            let r = R::new(card.right() - CARD_PAD - 28.0 - 140.0, card.y, 140.0, CARD_H);
            col.laid.push(r, Kind::Value(label.to_string()));
            let chevron = R::new(card.right() - CARD_PAD - 20.0, card.y, 20.0, CARD_H);
            col.laid.push(chevron, Kind::Trailing(if open { '\u{E70E}' } else { '\u{E70D}' }));
            if open {
                icon_grid(col, i, d.icon);
            }
        }
    }
    col.section("Windows");
    col.link(Some('\u{E713}'), "Windows sound settings", plain("Volume levels, device properties, spatial sound"), '\u{E8A7}', Button::SoundSettings);
}

fn batteries(col: &mut Column, model: &Model) {
    let b = &model.config.batteries;
    col.toggle(
        Some('\u{E83F}'),
        "Battery levels",
        plain("Keyboards, mice, headsets and other wireless devices that report one"),
        b.enabled,
        false,
        Toggle::Batteries,
    );
    let off = !b.enabled;
    col.section("Where they show");
    col.toggle(None, "In the panel", plain("A Batteries section under the audio devices"), b.flyout, off, Toggle::BatteryFlyout);
    col.toggle(None, "In the tray icon's tooltip", plain("Lowest first"), b.tooltip, off, Toggle::BatteryTooltip);
    let pill_off = !model.config.pill.enabled;
    let card_note = if pill_off { "Needs the edge pill" } else { "Under the devices, when you hover it" };
    col.toggle(None, "On the edge pill's card", plain(card_note), b.pill_card, off || pill_off, Toggle::BatteryPillCard);
    col.section("When one runs low");
    let labels: Vec<String> = crate::config::Batteries::LOW_LEVELS.iter().map(|level| format!("{level}%")).collect();
    let at = crate::config::Batteries::LOW_LEVELS.iter().position(|level| *level == b.low_at).unwrap_or(2);
    col.segments("Low at", plain("At or under this level"), &labels, at, off, Choice::LowBattery);
    let pill_note = if pill_off { "Needs the edge pill" } else { "Amber, for a few seconds" };
    col.toggle(None, "Warn on the edge pill", plain(pill_note), b.pill_alert, off || pill_off, Toggle::BatteryPill);
    col.toggle(None, "Windows notification", plain("Once, until it is charged again"), b.toast, off, Toggle::BatteryToast);
    col.section("Connected now");
    let batteries = &model.status.batteries;
    if batteries.is_empty() {
        let why = if off { "Turned off" } else { "Bluetooth keyboards, mice and headsets report one once connected" };
        col.card(None, "No device reports a battery", plain(why), true, None);
    }
    for battery in batteries {
        let card = col.card(Some(battery.kind.glyph()), battery.name.clone(), None, off, None);
        let r = R::new(card.right() - CARD_PAD - 140.0, card.y, 140.0, CARD_H);
        col.laid.push(r, Kind::Value(format!("{}%", battery.level)));
    }
}

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
    ("Off".to_string(), Tone::Plain)
}

fn not_set_up(why: &str) -> (String, Tone) {
    (format!("Not set up: {why}"), Tone::Warn)
}

fn set_up(how: &str) -> (String, Tone) {
    (how.to_string(), Tone::Good)
}

/// "A, B and C".
fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// One card per connector, saying whether it is off, not set up or set up. Opened, it walks
/// through the setup until it is set up, then shows only what can still be changed.
fn connectors(col: &mut Column, model: &Model) {
    use crate::integrations::{browser, discord, discord_rpc, teams, teams_ui};
    let c = &model.config;
    let s = &model.status;
    let connected = |name: &str| s.integrations.iter().any(|(n, on)| *n == name && *on);
    let open = |connector| model.open == Some(connector);
    let hint = |button| model.hint.filter(|(b, _, _)| *b == button).map(|(_, h, tone)| (h.to_string(), tone));

    // Edge freezes a paused player it has put to sleep: play from here does nothing until its window
    // is opened. Only while one of the players is not on its "never sleep" list.
    if !model.edge_sleep.is_empty() {
        let names = join_names(&model.edge_sleep);
        let card = col.card(
            Some('\u{E7BA}'),
            format!("Edge can put {names} to sleep"),
            Some(("Paused a while, play then does nothing until you open it. In Edge: Never put these sites to sleep".to_string(), Tone::Warn)),
            false,
            None,
        );
        col.button_in(card, "Open Edge settings", false, Some('\u{E8A7}'), Button::EdgeSleep);
        col.gap(8.0);
    }

    // The browser: the extension, then which tabs.
    let tabs = connected(browser::NAME);
    let state = match (c.browser.enabled, tabs) {
        (false, _) => off(),
        (true, true) => set_up("Connected"),
        (true, false) => not_set_up("the extension has not connected"),
    };
    col.expander('\u{E774}', "Browser tabs", state, Some((c.browser.enabled, Toggle::Browser)), open(Connector::Browser), Connector::Browser);
    if open(Connector::Browser) {
        let dim = !c.browser.enabled;
        let mut body = col.body();
        if !tabs {
            let why = hint(Button::ExtensionFolder).or(plain("Opens the extensions page, and the folder to Load unpacked (its path copied)"));
            col.row(&mut body, Some((1, false)), "Install the extension", why, dim, Control::Button("Open", true, None, Button::ExtensionFolder));
            col.row(&mut body, Some((2, false)), "Play something in a tab", plain("The extension connects with the first video or song"), dim, Control::None);
        }
        let labels = vec!["Every site".to_string(), "Chosen sites".to_string()];
        col.row(&mut body, None, "Tabs followed", plain("Every tab that plays, or only the sites below"), dim, Control::Segments(labels, usize::from(!c.browser.every_site), Choice::BrowserTabs));
        // One switch per site, so more can be offered later.
        for (i, (key, name, _)) in browser::SITES.iter().enumerate() {
            let on = c.browser.sites.iter().any(|s| s == key);
            col.row(&mut body, None, name, None, dim || c.browser.every_site, Control::Toggle(on, Toggle::Site(i)));
        }
        col.end_group(body);
    }

    // A player that shares what it plays only once told to; no card where it is not installed.
    if let Some(shares) = model.mpc {
        let on = c.mpc.enabled;
        let state = match (on, shares) {
            (false, _) => off(),
            (true, true) => set_up("Followed like any player"),
            (true, false) => not_set_up("its Windows media controls are off"),
        };
        col.expander('\u{E714}', "MPC-HC", state, Some((on, Toggle::Mpc)), open(Connector::Mpc), Connector::Mpc);
        if open(Connector::Mpc) {
            let dim = !on;
            let mut body = col.body();
            if shares {
                col.row(&mut body, None, "Its Windows media controls are on", plain("What it plays is shared with Windows; Off here only leaves it out of Audio Tray"), dim, Control::None);
            } else {
                col.row(&mut body, Some((1, false)), "Close MPC-HC", plain("It saves its own settings as it closes, over any change made meanwhile"), dim, Control::None);
                let why = hint(Button::Mpc).or(plain("Its Options > Player > Control via Windows UI (SMTC)"));
                col.row(&mut body, Some((2, false)), "Turn on its Windows media controls", why, dim, Control::Button("Turn on", true, None, Button::Mpc));
            }
            col.end_group(body);
        }
    }

    // Discord: your own application's keys, then Discord running.
    let discord_on = connected(discord_rpc::NAME) || connected(discord::NAME);
    let has_id = c.discord.client_id.is_some();
    let keys = has_id && c.discord.client_secret.is_some();
    let set = keys && s.discord_problem.is_none();
    let state = match (c.discord.enabled, discord_on, set) {
        (false, _, _) => off(),
        (true, true, _) => set_up("Connected"),
        (true, false, true) => ("Set up, waiting for Discord".to_string(), Tone::Plain),
        (true, false, false) => not_set_up(s.discord_problem.filter(|_| keys).unwrap_or("needs an application of your own")),
    };
    col.expander('\u{E8BD}', "Discord calls", state, Some((c.discord.enabled, Toggle::Discord)), open(Connector::Discord), Connector::Discord);
    if open(Connector::Discord) {
        let dim = !c.discord.enabled;
        let mut body = col.body();
        if !set && !discord_on {
            let create = plain("Discord shares calls only with an app of your own: make one on its developer portal");
            col.row(&mut body, Some((1, has_id)), "Create an application", create, dim, Control::Button("Open portal", !has_id, Some('\u{E8A7}'), Button::DiscordPortal));
            let redirect = hint(Button::DiscordRedirect).or(plain(&format!("In OAuth2 > Redirects, add {}", discord_rpc::REDIRECT)));
            col.row(&mut body, Some((2, false)), "Add the redirect", redirect, dim, Control::Button("Copy", false, Some('\u{E8C8}'), Button::DiscordRedirect));
            col.row(&mut body, Some((3, keys)), "Paste its keys", plain("From OAuth2: copy each one, then press its Paste"), dim, Control::None);
        }
        let id = hint(Button::DiscordId).or_else(|| c.discord.client_id.clone().map(|id| (id, Tone::Plain)));
        col.row(&mut body, None, "Application ID", id.or(plain("Not set")), dim, Control::Button("Paste", false, Some('\u{E77F}'), Button::DiscordId));
        let secret = hint(Button::DiscordSecret).or_else(|| model.secret_tail().map(|t| (t, Tone::Plain)));
        col.row(&mut body, None, "Client secret", secret.or(plain("Not set")), dim, Control::Button("Paste", false, Some('\u{E77F}'), Button::DiscordSecret));
        col.end_group(body);
    }

    // Teams: its API, allowed once in a meeting; else its meeting window, followed.
    let api = connected(teams::NAME);
    let window = connected(teams_ui::NAME);
    let paired = c.teams.token.is_some();
    let state = match (c.teams.enabled, api, paired, window) {
        (false, ..) => off(),
        (true, true, true, _) => set_up("Connected"),
        (true, _, _, true) => set_up("Following the meeting window"),
        (true, true, false, _) => not_set_up("allow Audio Tray in your next meeting"),
        (true, false, true, false) => ("Set up, waiting for Teams".to_string(), Tone::Plain),
        (true, false, false, false) => not_set_up(s.teams_problem.unwrap_or("its API is not on")),
    };
    col.expander('\u{E716}', "Teams meetings", state, Some((c.teams.enabled, Toggle::Teams)), open(Connector::Teams), Connector::Teams);
    if open(Connector::Teams) {
        let dim = !c.teams.enabled;
        let mut body = col.body();
        if paired {
            col.row(&mut body, None, "Audio Tray is allowed", plain("Teams asked once, in a meeting"), dim, Control::None);
        } else {
            col.row(&mut body, Some((1, api)), "Turn on Teams' API", plain("In Teams: Settings > Privacy > Third-party app API"), dim, Control::None);
            col.row(&mut body, Some((2, false)), "Allow Audio Tray", plain("In a meeting, when Teams asks"), dim, Control::None);
            col.row(&mut body, None, "Or nothing at all", plain("Without its API, Teams' meeting window is followed: the mute, not who talks"), dim, Control::None);
        }
        col.end_group(body);
    }
}

fn about(col: &mut Column, model: &Model) {
    let version = format!("Version {}", env!("CARGO_PKG_VERSION"));
    match &model.update {
        Some(next) => col.button_card(Some('\u{E946}'), "Audio Tray", Some((format!("{version}; v{next} is ready"), Tone::Good)), "Restart to update", true, Button::Restart),
        None => {
            col.card(Some('\u{E946}'), "Audio Tray", Some((version, Tone::Plain)), false, None);
        }
    }
    col.link(Some('\u{E71B}'), "Releases and source", plain("github.com/michelsalib/audio-tray"), '\u{E8A7}', Button::Releases);
    col.link(Some('\u{E8B7}'), "Settings folder", plain("Where config.toml is kept"), '\u{E8A7}', Button::ConfigFolder);
    col.button_card(Some('\u{E7E8}'), "Quit Audio Tray", plain("Puts the taskbar back as Windows draws it"), "Quit", false, Button::Quit);
}

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
        Monitor { name: name.to_string(), rect, work: rect, primary }
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
    fn segments_are_hit_where_they_are_drawn_and_not_while_dimmed() {
        let mut m = model(Page::Pill);
        let laid = page(&m, 900.0, &measure);
        for i in 0..3 {
            let seg = laid.items.iter().find(|it| matches!(it.kind, Kind::Segment { target: Target::Choice(Choice::Edge, j), .. } if j == i)).unwrap();
            assert_eq!(laid.hit(seg.r.x + seg.r.w / 2.0, seg.r.y + seg.r.h / 2.0), Some(Target::Choice(Choice::Edge, i)));
        }
        m.config.pill.enabled = false;
        let laid = page(&m, 900.0, &measure);
        assert!(!laid.hits.iter().any(|(_, t)| matches!(t, Target::Choice(..) | Target::Slot(..) | Target::Monitor(_))));
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
        // The second keyboard is on US: its second segment is the selected one, and hits.
        let us = laid.items.iter().find(|i| matches!(i.kind, Kind::Segment { target: Target::Choice(Choice::KeyboardLayout(1), 1), .. })).unwrap();
        assert!(matches!(us.kind, Kind::Segment { selected: true, .. }));
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
    fn too_many_layouts_step_through_a_button_instead() {
        let mut m = keyboards_model();
        m.layouts = (0..8).map(|i| (0x0409_0400 + i, format!("A rather long layout name {i}"))).collect();
        let laid = page(&m, 900.0, &measure);
        assert!(!laid.items.iter().any(|i| matches!(i.kind, Kind::Segment { target: Target::Choice(Choice::KeyboardLayout(_), _), .. })));
        assert!(laid.hits.iter().any(|(_, t)| *t == Target::Button(Button::NextLayout(0))));
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
    fn nav_hits_each_page() {
        let laid = nav();
        for page in Page::ALL {
            let r = nav_rect(page);
            assert_eq!(laid.hit(r.x + 4.0, r.y + 4.0), Some(Target::Nav(page)));
        }
    }
}

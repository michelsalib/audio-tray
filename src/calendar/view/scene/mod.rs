//! The panel's card laid out, in DIPs: what to draw where, and what each place does when clicked.
//! Pure: the view hands in what there is to show (its strings already in the user's language, the
//! caret's blink) and paints the result; tests check the geometry. Only the card's own fixed labels
//! (Join, Reply, the tooltips, the headers) are looked up here, in [`words`]: Windows' display
//! language, read once (English under test).
//!
//! One card, one job ([`Panel`]): the clock's calendar (the date and world clocks, the month, the
//! next event, the rest of the week or the picked day), or one event's details; or the bell's
//! notification centre (every app's notifications, Do not disturb, and the focus timer).

use std::sync::Arc;

use super::paint::{Align, Font};
use super::words::words;

mod banner;
mod batteries;
mod calendar;
mod notification;
pub use banner::*;
use batteries::*;
use calendar::*;
use notification::*;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl R {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        R { x, y, w, h }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// The card's width.
pub const W: f32 = crate::theme::PANEL_W;
const PAD: f32 = 16.0;
const TOAST_H: f32 = 76.0;
/// A notification's large picture.
const PICTURE_H: f32 = 150.0;
/// The row of a notification's buttons (or its reply box), and a line of its menu.
const ACTIONS_H: f32 = 32.0;
const MENU_ROW_H: f32 = 30.0;
const GROUP_H: f32 = 28.0;
const ROW_H: f32 = 32.0;
const FOOTER_H: f32 = 52.0;
/// The notification centre's line when it has none.
const EMPTY_H: f32 = 36.0;
/// A failed source's line over the list.
const PROBLEM_H: f32 = 24.0;
/// A status line ("Windows refused…") on the footer's top edge, as the audio panel's notice.
const STATUS_H: f32 = 28.0;
/// The notification centre with any waiting never takes less than this (the screen permitting).
const CENTER_MIN: f32 = 150.0;
/// The notification centre's header: its title, Do not disturb and Clear all.
const CENTER_HEAD_H: f32 = 12.0 + 32.0 + 6.0;

/// Which card the panel shows: the clock opens the calendar, the bell the notifications, Windows'
/// battery icon the batteries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Calendar,
    Notifications,
    Batteries,
}

/// This PC's battery, as the batteries' card shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct PcBattery {
    pub level: u8,
    pub tone: crate::batteries::Tone,
    pub charging: bool,
    /// Its state in words ("Charging", "2 h 35 left").
    pub state: String,
}

/// A device's battery, as the batteries' card lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceBattery {
    pub glyph: char,
    pub name: String,
    pub level: u8,
    pub tone: crate::batteries::Tone,
}

/// A colour of the palette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ink {
    Text,
    Text2,
    Text3,
    Accent,
    OnAccent,
    /// Secondary text on the accent.
    OnAccent2,
    /// A colour of its own, at an opacity (text on a coloured button).
    Rgba([u8; 3], u8),
}

/// A surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fill {
    /// A notification, the next event: a little lighter than the card.
    Raised,
    Hover,
    /// Under a press: a step past the hover.
    Pressed,
    Accent,
    /// An accent button under the pointer, and pressed.
    AccentHover,
    AccentPressed,
    /// The footer's strip, and the hairline over it (the audio panel's).
    Shade,
    Divider,
    /// The marks under busy days.
    Mark,
    /// A tooltip's ground: opaque, over anything.
    Tip,
    /// A wash of the on-accent colour over the accent (a glance's glyph disc, its countdown).
    OnAccentWash,
    /// A colour of its own, at an opacity (0..255): an app's, a call's buttons, confetti.
    Rgba([u8; 3], u8),
    Rgb([u8; 3]),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Draw {
    Card,
    Rect { fill: Fill, radius: f32 },
    /// An outline, inside the rectangle.
    Outline { ink: Ink, radius: f32 },
    Text { text: String, font: Font, ink: Ink, align: Align },
    Glyph { glyph: char, font: Font, ink: Ink },
    Logo { key: String, bytes: Arc<Vec<u8>> },
    /// A photo cut to a circle (a sender's).
    Avatar { key: String, bytes: Arc<Vec<u8>> },
    /// A picture whole, fitted and centred in the rectangle.
    Picture { key: String, bytes: Arc<Vec<u8>> },
    /// A colour washed in from the left, gone by `reach` of the width (0..1): an app's, on its banner.
    Wash { rgb: [u8; 3], alpha: u8, reach: f32, radius: f32 },
    /// A circle's outline inside the rectangle.
    Ring { rgb: [u8; 3], alpha: u8, width: f32 },
    /// A one-line text box's text, its caret and selection placed as DirectWrite measures them,
    /// scrolled so the caret shows (`caret`: drawn, or off in its blink).
    Field { editor: super::editor::Editor, font: Font, caret: bool },
    Clip,
    Unclip,
}

/// What a click lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// An app's line (it lights its clear button).
    Group(usize),
    /// Clear all of an app's notifications.
    DismissGroup(usize),
    /// Windows' page where Audio Tray is allowed to read notifications.
    Allow,
    /// A notification (by its place in the list): opens its app.
    Toast(usize),
    Dismiss(usize),
    /// A notification's button, by its place.
    Action(usize, usize),
    /// Open the reply box; its field; send what is typed.
    Reply(usize),
    Draft(usize),
    Send(usize),
    /// Open or close a notification's menu; an entry of it.
    Menu(usize),
    MenuItem(usize, usize),
    ClearAll,
    Quiet,
    Today,
    Prev,
    Next,
    /// A day, by its number.
    Day(i32),
    /// The next event: its details.
    Hero,
    HeroJoin,
    /// A row of the list: its details.
    Row(usize),
    Join(usize),
    Back,
    DetailJoin,
    DetailOpen,
    Directions,
    Focus,
    /// Audio Tray's settings (the footer's gear).
    Settings,
    /// Windows' own settings for what the card shows (the footer's labelled link).
    WindowsSettings,
    Clocks,
    /// A banner's quick reaction (an emoji sent as the reply), by its place.
    Quick(usize, usize),
}

pub struct Toast {
    /// Its place in the notification list.
    pub index: usize,
    pub title: String,
    /// When it came, in a few words.
    pub when: String,
    pub body: String,
    pub avatar: Option<(String, Arc<Vec<u8>>)>,
    pub picture: Option<(String, Arc<Vec<u8>>)>,
    pub buttons: Vec<String>,
    /// The reply box's placeholder, when it takes a reply.
    pub reply: Option<String>,
    pub menu: Vec<String>,
    /// What is typed in its reply box, while that is open.
    pub draft: Option<super::editor::Editor>,
    pub menu_open: bool,
    /// How much of it shows: 1, or less while it collapses away (dismissed), its height with it.
    pub collapse: f32,
    /// One-tap reactions beside Reply (a banner's), sent as the reply.
    pub quick: Vec<String>,
    /// Sent: its buttons give way to this word for a moment.
    pub sent: Option<String>,
    /// A call: its first button green (answer), its last red (decline).
    pub call: bool,
    /// A picture in its photo's place (Audio Tray's own notices, a conversation's faces).
    pub art: Art,
    /// Its app's colour, washed in from the card's left edge.
    pub tint: Option<[u8; 3]>,
}

impl Toast {
    /// Notification `t` as the list and a banner draw it alike: its words, pictures, buttons, reply
    /// box and menu, at rest (the first in the list, whole, nothing typed, no picture of Audio
    /// Tray's, no colour); each sets the rest.
    pub fn of(t: &crate::calendar::notifications::Toast) -> Toast {
        let picture = |p: &crate::calendar::rich::Picture| (p.key.clone(), p.bytes.clone());
        Toast {
            index: 0,
            title: if t.title.is_empty() { t.app.clone() } else { t.title.clone() },
            when: String::new(),
            body: t.body.clone(),
            avatar: t.rich.avatar.as_ref().map(picture),
            picture: t.rich.picture.as_ref().map(picture),
            buttons: t.rich.buttons.iter().map(|a| a.label.clone()).collect(),
            reply: t.rich.reply.as_ref().map(|r| if r.placeholder.is_empty() { words().reply.to_string() } else { r.placeholder.clone() }),
            menu: t.rich.menu.iter().map(|a| a.label.clone()).collect(),
            draft: None,
            menu_open: false,
            collapse: 1.0,
            quick: Vec::new(),
            sent: None,
            call: t.rich.call,
            art: Art::None,
            tint: None,
        }
    }

    /// A row of buttons under it (its buttons, its reply box). A menu alone takes no row: its "…"
    /// shows beside the X.
    fn has_actions(&self) -> bool {
        !self.buttons.is_empty() || self.reply.is_some()
    }

    /// The room it takes in the list, the gap after it included, as far as it is collapsed.
    fn room(&self) -> f32 {
        (self.height() + 4.0) * self.collapse.clamp(0.0, 1.0)
    }

    fn height(&self) -> f32 {
        let mut h = TOAST_H;
        if self.picture.is_some() {
            h += PICTURE_H + 8.0;
        }
        if self.has_actions() {
            h += ACTIONS_H + 8.0;
        }
        if self.menu_open {
            h += self.menu.len() as f32 * MENU_ROW_H + 6.0;
        }
        h
    }
}

/// The notification a target belongs to.
pub fn toast_of(hit: Hit) -> Option<usize> {
    match hit {
        Hit::Toast(i) | Hit::Dismiss(i) | Hit::Action(i, _) | Hit::Reply(i) | Hit::Draft(i) | Hit::Send(i) | Hit::Menu(i) | Hit::MenuItem(i, _) | Hit::Quick(i, _) => Some(i),
        _ => None,
    }
}

pub struct Group {
    pub name: String,
    pub logo: Option<(String, Arc<Vec<u8>>)>,
    /// Newest first.
    pub toasts: Vec<Toast>,
    /// When the newest arrived, in a few words.
    pub when: String,
}

impl Group {
    /// How much of its line shows: it collapses with its last notifications.
    fn collapse(&self) -> f32 {
        self.toasts.iter().map(|t| t.collapse).fold(0.0, f32::max).clamp(0.0, 1.0)
    }
}

pub struct DayCell {
    pub day: i32,
    pub label: String,
    pub today: bool,
    pub selected: bool,
    /// Outside the month shown.
    pub dim: bool,
    /// Events that day: 0, 1, or more.
    pub busy: u8,
}

#[derive(Clone)]
pub struct Hero {
    pub caption: String,
    pub title: String,
    pub when: String,
    pub color: [u8; 3],
    pub joinable: bool,
}

pub struct Row {
    pub left: String,
    pub title: String,
    pub color: [u8; 3],
    pub right: String,
    pub joinable: bool,
}

pub struct Detail {
    pub back: String,
    pub title: String,
    pub when: String,
    pub soon: String,
    pub color: [u8; 3],
    pub place: String,
    pub initials: Vec<String>,
    pub people: String,
    pub notes: String,
    pub source: String,
    pub open: Option<String>,
    pub joinable: bool,
    /// The place is one to go to (not an online meeting's): directions offered.
    pub directions: bool,
}

/// How the calendar's sources stand, said where events would be.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Notice {
    #[default]
    None,
    /// No calendar connected.
    NoSources,
    /// The first fetch is running, nothing cached yet.
    Loading,
    /// A source failed (an expired sign-in, a link down), in a few words.
    Problem(String),
}

/// Everything the panel shows.
pub struct Input {
    /// The calendar or the notifications.
    pub panel: Panel,
    /// How the sources stand.
    pub notice: Notice,
    /// The target the keyboard is on (Tab), ringed.
    pub focus: Option<Hit>,
    /// The target under a press.
    pub pressed: Option<Hit>,
    /// A word for a moment (Windows refused something).
    pub flash: Option<String>,
    /// The height the card may take.
    pub room: f32,
    /// The card's height while it eases to its own ([`natural_height`]); `None`: its own.
    pub card_h: Option<f32>,
    pub grid: Grid,
    /// The agenda's rows at most (Settings' Agenda rows).
    pub rows_max: usize,
    /// Each grid row's ISO week number; empty when not shown.
    pub week_numbers: Vec<u32>,
    /// The notifications, by app (empty: none waiting).
    pub groups: Vec<Group>,
    /// Windows does not let Audio Tray read them.
    pub notifications_denied: bool,
    pub unread: usize,
    pub quiet: bool,
    pub center_scroll: f32,
    pub weekday: String,
    pub date: String,
    pub clocks: String,
    pub month_title: String,
    pub heads: [String; 7],
    /// The month's six weeks (empty without the grid).
    pub days: Vec<DayCell>,
    pub hero: Option<Hero>,
    pub list_title: String,
    pub rows: Vec<Row>,
    pub empty: String,
    pub list_scroll: f32,
    pub focus_label: String,
    /// The focus timer's minutes left, while it runs (shown on its button).
    pub focus_left: Option<i64>,
    pub detail: Option<Detail>,
    pub hover: Option<Hit>,
    /// Where the pointer rested on `hover`: its tooltip shows by that target.
    pub show_tip: Option<(f32, f32)>,
    /// The card is drawn light (its levels' colours follow).
    pub light: bool,
    /// The batteries' card: this PC's (none on a desktop), then each device's, lowest first.
    pub pc: Option<PcBattery>,
    pub devices: Vec<DeviceBattery>,
    /// The open reply box's caret is drawn (it blinks).
    pub caret: bool,
}

/// The days' grid, as Settings has it (Layout).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grid {
    Comfortable,
    /// Shorter days.
    Compact,
    /// No grid: the agenda alone.
    None,
}

#[derive(Default)]
pub struct Scene {
    /// The card.
    pub main: R,
    /// The frame's size: the card's.
    pub w: f32,
    pub h: f32,
    pub items: Vec<(R, Draw)>,
    pub hits: Vec<(R, Hit)>,
    /// Where each list scrolls, and how far at most.
    pub center_view: Option<(R, f32)>,
    pub list_view: Option<(R, f32)>,
    /// Each notification drawn (by its place in the list), whole: unclipped.
    pub toasts: Vec<(usize, R)>,
    /// Every target in order, out of sight too (unclipped): the keyboard's Tab order.
    pub order: Vec<(R, Hit)>,
}

impl Scene {
    /// The topmost target at `(x, y)`.
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map(|(_, h)| *h)
    }

    /// Whether `(x, y)` is on the card (else, a click there closes the panel).
    pub fn on_card(&self, x: f32, y: f32) -> bool {
        self.main.contains(x, y)
    }
}

/// A scene being laid out: drawing in order, targets topmost last.
struct Layout {
    items: Vec<(R, Draw)>,
    hits: Vec<(R, Hit)>,
    hover: Option<Hit>,
    pressed: Option<Hit>,
    /// The keyboard's target: lit as the pointer's is (a notification's X shows for it).
    focus: Option<Hit>,
    /// An open reply box's caret is drawn (it blinks).
    caret: bool,
    /// Where each notification was drawn, unclipped.
    places: Vec<(usize, R)>,
    /// What is drawn takes no clicks (a notification collapsing away).
    inert: bool,
    /// Every target in order, out of sight too: the keyboard's Tab goes through them all,
    /// scrolling each into view.
    order: Vec<(R, Hit)>,
    /// The scrolled area drawing is clipped to: targets are clipped the same, so what scrolled
    /// out of sight takes no clicks (and none over the header above it).
    /// Nested (the reply box clips inside the list): each entry is already cut to the one under it.
    clips: Vec<R>,
}

/// Where `a` and `b` overlap, if they do.
fn overlap(a: R, b: R) -> Option<R> {
    let (x0, y0) = (a.x.max(b.x), a.y.max(b.y));
    let (x1, y1) = (a.right().min(b.right()), a.bottom().min(b.bottom()));
    (x1 > x0 && y1 > y0).then(|| R::new(x0, y0, x1 - x0, y1 - y0))
}

impl Layout {
    fn new(hover: Option<Hit>, pressed: Option<Hit>, focus: Option<Hit>, caret: bool) -> Self {
        Layout { items: Vec::new(), hits: Vec::new(), hover, pressed, focus, caret, places: Vec::new(), inert: false, order: Vec::new(), clips: Vec::new() }
    }

    /// The scene laid out, its frame the card `main` (from the origin); no list scrolls in it.
    fn into_scene(self, main: R) -> Scene {
        Scene { main, w: main.right(), h: main.bottom(), items: self.items, hits: self.hits, center_view: None, list_view: None, toasts: self.places, order: self.order }
    }

    fn draw(&mut self, r: R, d: Draw) {
        match d {
            Draw::Clip => {
                let cut = match self.clips.last() {
                    Some(&c) => overlap(r, c).unwrap_or(R::new(r.x, r.y, 0.0, 0.0)),
                    None => r,
                };
                self.clips.push(cut);
            }
            Draw::Unclip => {
                self.clips.pop();
            }
            // Scrolled out of sight: not drawn (a long list would lay out, and decode the
            // pictures of, every notification on each frame).
            _ if self.clips.last().is_some_and(|&c| overlap(r, c).is_none()) => return,
            _ => {}
        }
        self.items.push((r, d));
    }

    fn text(&mut self, r: R, text: impl Into<String>, font: Font, ink: Ink, align: Align) {
        self.draw(r, Draw::Text { text: text.into(), font, ink, align });
    }

    fn glyph(&mut self, r: R, glyph: char, font: Font, ink: Ink) {
        self.draw(r, Draw::Glyph { glyph, font, ink });
    }

    fn hit(&mut self, r: R, hit: Hit) {
        if self.inert {
            return;
        }
        self.order.push((r, hit));
        let r = match self.clips.last() {
            Some(&c) => match overlap(r, c) {
                Some(cut) => cut,
                None => return,
            },
            None => r,
        };
        self.hits.push((r, hit));
    }

    fn lit(&self, hit: Hit) -> bool {
        self.hover == Some(hit) || self.focus == Some(hit)
    }

    /// An accent button's fill: lighter under the pointer, lighter still pressed.
    fn accent(&self, hit: Hit) -> Fill {
        if self.pressed == Some(hit) && self.lit(hit) {
            Fill::AccentPressed
        } else if self.hover == Some(hit) {
            Fill::AccentHover
        } else {
            Fill::Accent
        }
    }

    /// Under a press: the hover's fill, a step further.
    fn surface(&self, hit: Hit, rest: Fill) -> Fill {
        if self.pressed == Some(hit) && self.lit(hit) {
            Fill::Pressed
        } else if self.lit(hit) {
            Fill::Hover
        } else {
            rest
        }
    }

    /// A hover plate under a target.
    fn plate(&mut self, r: R, hit: Hit, radius: f32) {
        if self.lit(hit) {
            let fill = self.surface(hit, Fill::Hover);
            self.draw(r, Draw::Rect { fill, radius });
        }
        self.hit(r, hit);
    }

    /// A round button with a glyph, as every panel's (the footer's gear, the audio panel's):
    /// filled with the accent while `on`.
    fn icon_button(&mut self, r: R, glyph: char, hit: Hit, on: bool) {
        let round = r.h.min(r.w) / 2.0;
        if on {
            let fill = self.accent(hit);
            self.draw(r, Draw::Rect { fill, radius: round });
            self.hit(r, hit);
            self.glyph(r, glyph, Font::Icon, Ink::OnAccent);
        } else {
            self.plate(r, hit, round);
            self.glyph(r, glyph, Font::Icon, Ink::Text);
        }
    }

    /// A pill with a label; `on` fills it with the accent.
    fn pill(&mut self, r: R, label: &str, hit: Hit, on: bool) {
        let fill = if on { self.accent(hit) } else { self.surface(hit, Fill::Raised) };
        self.draw(r, Draw::Rect { fill, radius: r.h / 2.0 });
        if !on {
            self.draw(r, Draw::Outline { ink: Ink::Text3, radius: r.h / 2.0 });
        }
        self.text(r, label, Font::Caption, if on { Ink::OnAccent } else { Ink::Text }, Align::Center);
        self.hit(r, hit);
    }

    /// An accent Join button (its label measured: wider in French), right-aligned at `right`.
    fn join(&mut self, right: f32, y: f32, h: f32, hit: Hit) -> R {
        let r = R::new(right - join_w(), y, join_w(), h);
        let fill = self.accent(hit);
        self.draw(r, Draw::Rect { fill, radius: 4.0 });
        self.text(r, words().join, Font::CaptionStrong, Ink::OnAccent, Align::Center);
        self.hit(r, hit);
        r
    }
}

/// A Join button's width, as its label needs.
fn join_w() -> f32 {
    (text_w(words().join, Font::CaptionStrong) + 24.0).max(52.0)
}

/// The card's own height for what it shows (the view eases the notification card to it).
pub fn natural_height(input: &Input) -> f32 {
    match input.panel {
        Panel::Notifications => center_card_height(input),
        Panel::Calendar => calendar_plan(input).0,
        Panel::Batteries => batteries_height(input),
    }
}

pub fn build(input: &Input) -> Scene {
    let mut ink = Layout::new(input.hover, input.pressed, input.focus, input.caret);
    let mut center_view = None;
    let mut list_view = None;
    let main = match input.panel {
        Panel::Notifications => {
            let card = R::new(0.0, 0.0, W, input.card_h.unwrap_or_else(|| center_card_height(input)));
            ink.draw(card, Draw::Card);
            center_view = Some(center(&mut ink, card, input));
            card
        }
        Panel::Batteries => {
            let card = R::new(0.0, 0.0, W, input.card_h.unwrap_or_else(|| batteries_height(input)));
            ink.draw(card, Draw::Card);
            batteries(&mut ink, card, input);
            card
        }
        Panel::Calendar => {
            let (h, rows, hero) = calendar_plan(input);
            // Held at the height it eases from while it changes (a day picked, an event opened).
            let card = R::new(0.0, 0.0, W, input.card_h.unwrap_or(h));
            ink.draw(card, Draw::Card);
            match &input.detail {
                Some(detail) => detail_card(&mut ink, card, detail),
                None => list_view = calendar(&mut ink, card, input, rows, hero),
            }
            card
        }
    };
    // Over the card's foot; two lines when it needs them. With no notification left, in the list's
    // own line instead ("3 notifications cleared"), not a box over the empty card.
    let inline = input.panel == Panel::Notifications && input.groups.is_empty();
    if let Some(text) = input.flash.as_ref().filter(|_| !inline) {
        // A status line on the footer's top edge, in the footer's own shade: every panel says
        // things there (the audio panel's notice too), never in a box over what it shows.
        let r = R::new(main.x, main.bottom() - FOOTER_H - STATUS_H, main.w, STATUS_H);
        ink.draw(r, Draw::Rect { fill: Fill::Tip, radius: 0.0 });
        ink.draw(r, Draw::Rect { fill: Fill::Shade, radius: 0.0 });
        ink.draw(R::new(r.x, r.y, r.w, 1.0), Draw::Rect { fill: Fill::Divider, radius: 0.0 });
        ink.glyph(R::new(r.x + 16.0, r.y, 16.0, r.h), '\u{E946}', Font::IconSmall, Ink::Text2);
        ink.text(R::new(r.x + 44.0, r.y, r.w - 44.0 - PAD, r.h), text.clone(), Font::Caption, Ink::Text2, Align::Left);
    }
    if let Some(focus) = input.focus {
        // The keyboard's target, ringed (twice over: a 2 DIP ring).
        if let Some(r) = ink.hits.iter().rev().find(|(_, h)| *h == focus).map(|(r, _)| *r) {
            // A day's ring goes round its circle, not its cell.
            let (r, radius) = if matches!(focus, Hit::Day(_)) { (R::new(r.x + (r.w - 34.0) / 2.0, r.y, 34.0, 34.0), 17.0) } else { (r, 5.0) };
            for inset in [0.0, 1.0] {
                ink.draw(R::new(r.x + inset, r.y + inset, r.w - 2.0 * inset, r.h - 2.0 * inset), Draw::Outline { ink: Ink::Accent, radius: radius - inset });
            }
        }
    }
    if let Some(at) = input.show_tip {
        tooltip(&mut ink, input, at, main);
    }
    Scene { center_view, list_view, ..ink.into_scene(main) }
}

/// Wider targets carry a label, so they get no tooltip.
const TIP_TARGET_MAX: f32 = 56.0;

/// What an icon-only target does, in words (`None`: it says it already).
fn tip_text(hit: Hit, input: &Input) -> Option<String> {
    let t = words();
    if hit == Hit::Focus {
        // "Focus 25 min", or the time left and how to stop.
        return Some(input.focus_label.clone());
    }
    Some(match hit {
        Hit::Quiet if input.quiet => t.turn_off_dnd,
        Hit::Quiet => t.do_not_disturb,
        Hit::Prev => t.previous_month,
        Hit::Next => t.next_month,
        Hit::Settings => t.settings,
        Hit::Back => t.back,
        Hit::Dismiss(_) => t.dismiss,
        Hit::DismissGroup(_) => t.clear_app,
        Hit::Menu(_) => t.more_options,
        Hit::Send(_) => t.send,
        Hit::Clocks => t.change_clocks,
        Hit::Today => t.go_to_today,
        Hit::ClearAll => t.clear_all,
        _ => return None,
    }
    .to_string())
}

/// About how wide `text` is in `font`, in DIPs: Segoe UI Variable's average advance, enough to
/// size a button to its label in either language.
fn text_w(text: &str, font: Font) -> f32 {
    let per = match font {
        Font::Tiny | Font::TinyStrong => 6.0,
        Font::Caption => 6.4,
        Font::CaptionStrong => 6.8,
        Font::Body | Font::Wrap => 7.2,
        Font::Strong => 7.6,
        Font::Title | Font::TitleWrap => 9.5,
        _ => 9.0,
    };
    text.chars().count() as f32 * per
}

/// The hovered target's tooltip, under it (above it near the card's foot), inside the card.
fn tooltip(ink: &mut Layout, input: &Input, (px, py): (f32, f32), card: R) {
    let Some(hit) = input.hover else { return };
    let Some(text) = tip_text(hit, input) else { return };
    // The target under the pointer.
    let Some(r) = ink.hits.iter().rev().find(|(r, h)| *h == hit && r.contains(px, py)).map(|(r, _)| *r) else { return };
    if r.w > TIP_TARGET_MAX && !matches!(hit, Hit::Clocks | Hit::Today) {
        // A labelled button says it already.
        return;
    }
    let w = (text_w(&text, Font::Caption) + 20.0).min(card.w - 16.0);
    let h = 26.0;
    let x = (r.x + r.w / 2.0 - w / 2.0).clamp(card.x + 8.0, card.right() - 8.0 - w);
    let below = r.bottom() + 6.0;
    let y = if below + h <= card.bottom() - 4.0 { below } else { r.y - 6.0 - h };
    let tip = R::new(x, y.max(card.y + 4.0), w, h);
    ink.draw(tip, Draw::Rect { fill: Fill::Tip, radius: 4.0 });
    ink.draw(tip, Draw::Outline { ink: Ink::Text3, radius: 4.0 });
    ink.text(tip, text, Font::Caption, Ink::Text, Align::Center);
}

/// A thin bar at `area`'s right saying there is more than it shows, and where it is: `content` is
/// the scrolled height, `scroll` how far down it is.
fn scrollbar(ink: &mut Layout, area: R, content: f32, scroll: f32) {
    if content <= area.h + 0.5 {
        return;
    }
    let h = (area.h * area.h / content).max(24.0);
    let y = area.y + (area.h - h) * (scroll / (content - area.h)).clamp(0.0, 1.0);
    ink.draw(R::new(area.right() - 6.0, y, 3.0, h), Draw::Rect { fill: Fill::Mark, radius: 1.5 });
}

/// A card's foot, the same on every panel (the audio panel's too): a darker strip under a
/// hairline, on the left a labelled link to Windows' own settings for what the card shows
/// (`glyph`, `label`), on the right Audio Tray's settings as a round gear.
fn footer(ink: &mut Layout, card: R, glyph: char, label: &str) {
    // The audio panel's measures (`flyout::theme`): glyph at 16, label at 44, a 32-high pill from
    // 4 in; the round button 9 from the edge.
    let fy = card.bottom() - FOOTER_H;
    ink.draw(R::new(card.x, fy, card.w, FOOTER_H), Draw::Rect { fill: Fill::Shade, radius: 0.0 });
    ink.draw(R::new(card.x, fy, card.w, 1.0), Draw::Rect { fill: Fill::Divider, radius: 0.0 });
    let cy = fy + FOOTER_H / 2.0;
    let gear = R::new(card.right() - 9.0 - 32.0, cy - 16.0, 32.0, 32.0);
    let label_w = (text_w(label, Font::Body) + 4.0).min(gear.x - 8.0 - (card.x + 44.0));
    let link = R::new(card.x + 4.0, cy - 16.0, 44.0 - 4.0 + label_w + 10.0, 32.0);
    ink.plate(link, Hit::WindowsSettings, 4.0);
    // 16 DIP glyphs, as the audio panel's footer.
    ink.glyph(R::new(card.x + 12.0, cy - 12.0, 24.0, 24.0), glyph, Font::Icon, Ink::Text);
    ink.text(R::new(card.x + 44.0, cy - 10.0, label_w, 20.0), label, Font::Body, Ink::Text, Align::Left);
    ink.plate(gear, Hit::Settings, 16.0);
    ink.glyph(gear, '\u{E713}', Font::Icon, Ink::Text);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> Input {
        Input {
            panel: Panel::Calendar,
            notice: Notice::None,
            focus: None,
            pressed: None,
            flash: None,
            room: 760.0,
            card_h: None,
            grid: Grid::Comfortable,
            rows_max: 5,
            week_numbers: Vec::new(),
            notifications_denied: false,
            groups: Vec::new(),
            unread: 0,
            quiet: false,
            center_scroll: 0.0,
            weekday: "Sunday".into(),
            date: "15 November".into(),
            clocks: String::new(),
            month_title: "November 2026".into(),
            heads: std::array::from_fn(|i| format!("d{i}")),
            days: month(),
            hero: None,
            list_title: "Later this week".into(),
            rows: Vec::new(),
            empty: "Nothing else this week".into(),
            list_scroll: 0.0,
            focus_label: "Focus 25 min".into(),
            focus_left: None,
            detail: None,
            hover: None,
            show_tip: None,
            pc: None,
            devices: Vec::new(),
            light: false,
            caret: false,
        }
    }

    /// The calendar card is as tall as what it draws: its list ends just over the footer's gap,
    /// with the month or without, with the next event or without.
    #[test]
    fn the_calendar_card_is_measured_as_it_is_drawn() {
        let hero = Hero { caption: "NEXT".into(), title: "Stand-up".into(), when: "9:00".into(), color: [1, 2, 3], joinable: false };
        for grid in [Grid::Comfortable, Grid::Compact, Grid::None] {
            for with_hero in [false, true] {
                let mut i = Input { grid, ..input() };
                if grid == Grid::None {
                    i.days = Vec::new();
                }
                i.hero = with_hero.then(|| hero.clone());
                let s = build(&i);
                let (list, _) = s.list_view.unwrap();
                assert!((list.bottom() + FOOT_GAP + FOOTER_H - s.main.bottom()).abs() < 0.01, "{grid:?}, hero {with_hero}: list ends at {}, card {}", list.bottom(), s.main.bottom());
            }
        }
    }

    fn batteries_input(pc: bool, devices: usize) -> Input {
        let level = |n: usize| (10 + n * 30).min(100) as u8;
        Input {
            panel: Panel::Batteries,
            pc: pc.then(|| PcBattery { level: 43, tone: crate::batteries::Tone::Fair, charging: true, state: "Charging".into() }),
            devices: (0..devices).map(|n| DeviceBattery { glyph: '\u{E962}', name: format!("Device {n}"), level: level(n), tone: crate::batteries::Tone::of(level(n), 20) }).collect(),
            ..input()
        }
    }

    /// The batteries' card holds this PC's line and every device, the footer under them, and has no
    /// target but the footer's two.
    #[test]
    fn the_batteries_card_fits_what_it_lists() {
        let empty = build(&batteries_input(false, 0));
        let full = build(&batteries_input(true, 3));
        assert_eq!(full.main.h - empty.main.h, PC_H + 8.0 + 3.0 * DEVICE_H - EMPTY_H);
        assert_eq!(full.main.h, natural_height(&batteries_input(true, 3)));
        let targets: Vec<Hit> = full.hits.iter().map(|(_, h)| *h).collect();
        assert_eq!(targets, vec![Hit::WindowsSettings, Hit::Settings]);
        // Each device's name is drawn, inside the card.
        for n in 0..3 {
            let name = format!("Device {n}");
            let r = full.items.iter().find(|(_, d)| matches!(d, Draw::Text { text, .. } if *text == name)).map(|(r, _)| *r).expect("drawn");
            assert!(r.bottom() <= full.main.bottom() - FOOTER_H);
        }
    }

    /// Many devices on a short screen: the card stops at the room it has.
    #[test]
    fn the_batteries_card_never_outgrows_the_screen() {
        let crowded = Input { room: 300.0, ..batteries_input(true, 12) };
        assert!(build(&crowded).main.h <= 300.0);
    }

    fn month() -> Vec<DayCell> {
        (0..42).map(|n| DayCell { day: 100 + n, label: n.to_string(), today: n == 6, selected: n == 6, dim: false, busy: 0 }).collect()
    }

    fn notifications() -> Input {
        Input { panel: Panel::Notifications, ..input() }
    }

    fn toast(i: usize) -> Toast {
        Toast { index: i, title: format!("t{i}"), when: String::new(), body: String::new(), avatar: None, picture: None, buttons: Vec::new(), reply: None, menu: Vec::new(), draft: None, menu_open: false, collapse: 1.0, quick: Vec::new(), sent: None, call: false, art: Art::None, tint: None }
    }

    #[test]
    fn a_dismissed_notification_collapses_and_takes_no_clicks() {
        let mut i = notifications();
        i.groups = vec![group("WhatsApp", 0, 2), group("Edge", 2, 1)];
        let whole = build(&i);
        let below = |s: &Scene| s.hits.iter().find(|(_, h)| *h == Hit::Toast(1)).unwrap().0.y;
        i.groups[0].toasts[0].collapse = 0.5;
        let half = build(&i);
        assert!(half.h < whole.h && below(&half) < below(&whole), "shorter, the next one coming up");
        assert!(!has(&half, Hit::Toast(0)) && !has(&half, Hit::Dismiss(0)), "no target in one going");
        // An app's last one going takes its line with it.
        i.groups[1].toasts[0].collapse = 0.0;
        let gone = build(&i);
        assert!(!has(&gone, Hit::Group(1)) && !has(&gone, Hit::Toast(2)));
        // The card can be held at a height while it eases to its own.
        i.card_h = Some(400.0);
        assert!((build(&i).h - 400.0).abs() < 0.5);
    }

    #[test]
    fn what_clear_all_says_is_the_empty_card_s_own_line() {
        let mut i = notifications();
        i.flash = Some("3 notifications cleared".into());
        let s = build(&i);
        assert!(s.items.iter().any(|(_, d)| matches!(d, Draw::Text { text, .. } if text == "3 notifications cleared")));
        assert!(!s.items.iter().any(|(_, d)| matches!(d, Draw::Rect { fill: Fill::Tip, .. })), "no box over the empty card");
    }

    fn group(name: &str, first: usize, n: usize) -> Group {
        Group { name: name.into(), logo: None, toasts: (first..first + n).map(toast).collect(), when: "2 min".into() }
    }

    fn center_of(r: R) -> (f32, f32) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    fn has(s: &Scene, hit: Hit) -> bool {
        s.hits.iter().any(|(_, h)| *h == hit)
    }

    #[test]
    fn the_clock_s_card_is_the_calendar_alone() {
        let mut i = input();
        // Notifications waiting change nothing here: they are the bell's.
        i.groups = vec![group("WhatsApp", 0, 3)];
        i.unread = 3;
        let s = build(&i);
        assert_eq!((s.main.x, s.main.y, s.w), (0.0, 0.0, W));
        assert!(!s.hits.iter().any(|(_, h)| toast_of(*h).is_some() || matches!(h, Hit::ClearAll | Hit::Group(_))));
        let day = s.hits.iter().find(|(_, h)| *h == Hit::Day(110)).unwrap().0;
        let (x, y) = center_of(day);
        assert_eq!(s.hit(x, y), Some(Hit::Day(110)));
        // The month always: its title and its arrows.
        assert!(has(&s, Hit::Prev) && has(&s, Hit::Next));
    }

    #[test]
    fn the_bell_s_card_lists_every_notification() {
        let mut i = notifications();
        i.groups = vec![group("WhatsApp", 0, 3), group("Edge", 3, 2)];
        i.unread = 5;
        let s = build(&i);
        assert!((0..5).all(|n| has(&s, Hit::Toast(n))), "every app's, every notification");
        assert!(has(&s, Hit::Group(1)) && has(&s, Hit::ClearAll));
        assert!(!s.hits.iter().any(|(_, h)| matches!(h, Hit::Day(_) | Hit::Hero | Hit::Row(_))), "no calendar");
        // Focus and Do not disturb in its header, once each.
        assert_eq!(s.hits.iter().filter(|(_, h)| *h == Hit::Quiet).count(), 1);
        assert_eq!(s.hits.iter().filter(|(_, h)| *h == Hit::Focus).count(), 1);
        let focus = s.hits.iter().find(|(_, h)| *h == Hit::Focus).unwrap().0;
        assert!(focus.y < s.main.y + 60.0, "in the header: {focus:?}");
    }

    #[test]
    fn every_card_ends_with_the_same_footer() {
        for i in [input(), Input { groups: vec![group("WhatsApp", 0, 2)], ..notifications() }] {
            let s = build(&i);
            let foot = s.main.bottom() - FOOTER_H;
            // The strip and its hairline, Windows' settings on the left, Audio Tray's gear on the right.
            assert!(s.items.iter().any(|(r, d)| matches!(d, Draw::Rect { fill: Fill::Shade, .. }) && (r.y - foot).abs() < 0.5));
            assert!(s.items.iter().any(|(r, d)| matches!(d, Draw::Rect { fill: Fill::Divider, .. }) && (r.y - foot).abs() < 0.5));
            let link = s.hits.iter().find(|(_, h)| *h == Hit::WindowsSettings).unwrap().0;
            let gear = s.hits.iter().find(|(_, h)| *h == Hit::Settings).unwrap().0;
            assert!(link.y > foot && gear.y > foot && link.right() < gear.x);
            assert!((s.main.right() - gear.right() - 9.0).abs() < 0.5, "the gear 9 from the edge");
            // Nothing else down there.
            assert!(s.hits.iter().filter(|(r, _)| r.y > foot).all(|(_, h)| matches!(h, Hit::WindowsSettings | Hit::Settings)));
        }
    }

    #[test]
    fn notifications_take_the_room_they_need_up_to_the_screen() {
        let mut i = notifications();
        i.groups = vec![group("WhatsApp", 0, 2)];
        let short = build(&i);
        assert!(short.h < i.room, "two notifications: {}", short.h);
        i.groups = vec![group("WhatsApp", 0, 30)];
        let long = build(&i);
        assert!((long.h - i.room).abs() < 0.5, "thirty fill the screen: {}", long.h);
        assert!(long.center_view.is_some_and(|(_, max)| max > 0.0), "and scroll");
    }

    #[test]
    fn notifications_scrolled_out_of_sight_take_no_clicks() {
        let mut i = notifications();
        i.groups = vec![group("WhatsApp", 0, 30)];
        i.unread = 30;
        i.center_scroll = 10_000.0;
        let s = build(&i);
        let (area, _) = s.center_view.unwrap();
        // The header's buttons stay theirs, whatever scrolled under them.
        let clear = s.hits.iter().find(|(_, h)| *h == Hit::ClearAll).unwrap().0;
        assert_eq!(s.hit(clear.x + 2.0, clear.y + 2.0), Some(Hit::ClearAll));
        // No notification target reaches past the list, over the header or the foot.
        assert!(s.hits.iter().filter(|(_, h)| toast_of(*h).is_some()).all(|(r, _)| r.y >= area.y && r.bottom() <= area.bottom()));
        let focus = s.hits.iter().find(|(_, h)| *h == Hit::Focus).unwrap().0;
        assert_eq!(s.hit(focus.x + 2.0, focus.y + 2.0), Some(Hit::Focus));
        // Nor is any drawn outside the card.
        assert!(s.items.iter().filter(|(_, d)| !matches!(d, Draw::Clip | Draw::Unclip)).all(|(r, _)| overlap(*r, s.main).is_some()));
        // And the list says it scrolls.
        assert!(s.items.iter().any(|(r, d)| matches!(d, Draw::Rect { fill: Fill::Mark, .. }) && r.w == 3.0));
    }

    #[test]
    fn with_none_waiting_the_bell_s_card_says_so_and_keeps_its_moon() {
        let mut i = notifications();
        let s = build(&i);
        assert!(s.h < CENTER_MIN + FOOTER_H, "one line and its foot: {}", s.h);
        assert!(has(&s, Hit::Quiet) && !has(&s, Hit::ClearAll));
        assert!(s.items.iter().any(|(_, d)| matches!(d, Draw::Text { text, .. } if text == words().no_notifications)));
        i.notifications_denied = true;
        assert!(has(&build(&i), Hit::Allow), "says where to allow it");
    }

    #[test]
    fn the_calendar_leaves_focus_and_do_not_disturb_to_the_bell() {
        let s = build(&input());
        assert!(!has(&s, Hit::Focus) && !has(&s, Hit::Quiet) && has(&s, Hit::Settings));
    }

    #[test]
    fn the_month_on_a_short_screen_goes_without_the_next_event_and_fits() {
        let mut i = input();
        i.hero = Some(Hero { caption: "NEXT".into(), title: "Stand-up".into(), when: "9:00".into(), color: [1, 2, 3], joinable: false });
        i.room = main_height(&i, 1, false) + 4.0;
        let s = build(&i);
        assert!(s.h <= i.room + 0.5);
        assert!(!has(&s, Hit::Hero));
    }

    #[test]
    fn rows_scroll_and_join_buttons_are_their_own_targets() {
        let mut i = input();
        i.rows = (0..8).map(|n| Row { left: "Mon".into(), title: format!("e{n}"), color: [0; 3], right: String::new(), joinable: n == 0 }).collect();
        let s = build(&i);
        let (area, max) = s.list_view.unwrap();
        assert!(max > 0.0 && area.h <= 5.0 * ROW_H);
        let join = s.hits.iter().find(|(_, h)| *h == Hit::Join(0)).unwrap().0;
        let (x, y) = center_of(join);
        assert_eq!(s.hit(x, y), Some(Hit::Join(0)));
    }

    #[test]
    fn buttons_that_do_not_fit_are_left_out_not_a_panic() {
        let mut i = notifications();
        let mut many = toast(0);
        many.buttons = (0..5).map(|n| format!("A rather long button label {n}")).collect();
        many.reply = Some("Reply".into());
        many.menu = vec!["Turn off".into()];
        i.groups = vec![Group { toasts: vec![many], ..group("Teams", 0, 1) }];
        let s = build(&i);
        assert!(has(&s, Hit::Reply(0)));
        assert!(s.hits.iter().filter(|(_, h)| matches!(h, Hit::Action(..))).all(|(r, _)| r.right() <= s.main.right()));
    }

    #[test]
    fn the_layout_setting_shapes_the_grid() {
        let mut i = input();
        let full = build(&i);
        i.grid = Grid::Compact;
        let compact = build(&i);
        assert!(compact.main.h < full.main.h);
        i.grid = Grid::None;
        i.days = Vec::new();
        let agenda = build(&i);
        assert!(!agenda.hits.iter().any(|(_, h)| matches!(h, Hit::Day(_) | Hit::Prev)));
        assert!(agenda.main.h < compact.main.h);
    }

    #[test]
    fn week_numbers_take_a_column_before_the_days() {
        let mut i = input();
        let first_day = |s: &Scene| s.hits.iter().find(|(_, h)| *h == Hit::Day(100)).unwrap().0.x;
        let without = first_day(&build(&i));
        i.week_numbers = vec![45, 46, 47, 48, 49, 50];
        let with = build(&i);
        assert!(first_day(&with) > without);
        assert!(with.items.iter().any(|(_, d)| matches!(d, Draw::Text { text, .. } if text == "47")));
    }

    #[test]
    fn the_agenda_rows_setting_caps_the_list() {
        let mut i = input();
        i.rows = (0..8).map(|n| Row { left: "Mon".into(), title: format!("e{n}"), color: [0; 3], right: String::new(), joinable: false }).collect();
        i.rows_max = 3;
        let (area, _) = build(&i).list_view.unwrap();
        assert!((area.h - 3.0 * ROW_H).abs() < 0.5);
    }
}

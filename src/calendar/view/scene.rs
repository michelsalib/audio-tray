//! The panel's card laid out, in DIPs: what to draw where, and what each place does when clicked.
//! Pure: the view hands in what there is to show (strings already in the user's language) and
//! paints the result; tests check the geometry.
//!
//! One card, one job ([`Panel`]): the clock's calendar (the date and world clocks, the month, the
//! next event, the rest of the week or the picked day), or one event's details; or the bell's
//! notification centre (every app's notifications, Do not disturb, and the focus timer).

use std::sync::Arc;

use super::paint::{Align, Font};
use super::words::words;

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
pub const W: f32 = crate::flyout::theme::PANEL_W;
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

/// Which card the panel shows: the clock opens the calendar, the bell the notifications.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Calendar,
    Notifications,
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
    pub draft: Option<String>,
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
    pub focus_running: bool,
    pub detail: Option<Detail>,
    pub hover: Option<Hit>,
    /// Where the pointer rested on `hover`: its tooltip shows by that target.
    pub show_tip: Option<(f32, f32)>,
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

/// A builder: drawing in order, targets topmost last.
struct Ink2 {
    items: Vec<(R, Draw)>,
    hits: Vec<(R, Hit)>,
    hover: Option<Hit>,
    pressed: Option<Hit>,
    /// The keyboard's target: lit as the pointer's is (a notification's X shows for it).
    focus: Option<Hit>,
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

impl Ink2 {
    fn new(hover: Option<Hit>, pressed: Option<Hit>, focus: Option<Hit>) -> Self {
        Ink2 { items: Vec::new(), hits: Vec::new(), hover, pressed, focus, places: Vec::new(), inert: false, order: Vec::new(), clips: Vec::new() }
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

/// The notification centre's natural height (its header and every notification, as far as each is
/// collapsed), for `groups`.
fn center_height(groups: &[Group]) -> f32 {
    let h = CENTER_HEAD_H;
    if groups.is_empty() {
        return h + EMPTY_H + 8.0;
    }
    h + groups.iter().map(|g| GROUP_H * g.collapse() + g.toasts.iter().map(Toast::room).sum::<f32>()).sum::<f32>() + 8.0
}

/// The card's own height for what it shows (the view eases the notification card to it).
pub fn natural_height(input: &Input) -> f32 {
    match input.panel {
        Panel::Notifications => center_card_height(input),
        Panel::Calendar => calendar_plan(input).0,
    }
}

/// An event's details at most (shorter when the screen is).
const DETAIL_H: f32 = 480.0;

/// The calendar card's height, the list's rows at most `rows`, with the next event or not.
fn main_height(input: &Input, rows: usize, hero: bool) -> f32 {
    if input.detail.is_some() {
        return DETAIL_H;
    }
    let mut h = PAD + 44.0 + 14.0;
    h += grid_height(input);
    if hero {
        h += 14.0 + 76.0;
    }
    h += 14.0 + 24.0 + rows.max(1) as f32 * ROW_H;
    if matches!(input.notice, Notice::Problem(_)) {
        h += PROBLEM_H;
    }
    h + 8.0 + FOOTER_H
}

/// The calendar's height and what it shows: its list's rows (fewer on a short screen), and the
/// next event (left out when even one row would not fit with it).
fn calendar_plan(input: &Input) -> (f32, usize, bool) {
    let room = input.room;
    let hero = input.hero.is_some() && main_height(input, 1, true) <= room;
    let mut rows = input.rows.len().clamp(1, input.rows_max.max(1));
    while rows > 1 && main_height(input, rows, hero) > room {
        rows -= 1;
    }
    let mut h = main_height(input, rows, hero);
    if input.detail.is_some() {
        // An event's details take what there is, never more than their height.
        h = h.min(room).max(240.0);
    }
    (h, rows, hero)
}

/// The notification centre's height: all it lists, up to the screen's; one line when none wait.
fn center_card_height(input: &Input) -> f32 {
    let least = if input.groups.is_empty() { 0.0 } else { CENTER_MIN.min(input.room) };
    (center_height(&input.groups) + FOOTER_H).min(input.room).max(least)
}

pub fn build(input: &Input) -> Scene {
    let mut ink = Ink2::new(input.hover, input.pressed, input.focus);
    let mut center_view = None;
    let mut list_view = None;
    let main = match input.panel {
        Panel::Notifications => {
            let card = R::new(0.0, 0.0, W, input.card_h.unwrap_or_else(|| center_card_height(input)));
            ink.draw(card, Draw::Card);
            center_view = Some(center(&mut ink, card, input));
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
    Scene { main, w: main.right(), h: main.bottom(), items: ink.items, hits: ink.hits, center_view, list_view, toasts: ink.places, order: ink.order }
}

/// A banner's app line: its logo, its name and when it came.
const BANNER_HEAD_H: f32 = 30.0;

/// What a banner draws in its photo's place: Audio Tray's own notices illustrated, a
/// conversation's faces.
#[derive(Clone, Debug, PartialEq)]
pub enum Art {
    None,
    /// A battery filled to `level` (0..1): amber, red once low.
    Battery { level: f32, low: bool },
    /// A speaker playing: its four bars' heights (0..1).
    Speaker { bars: [f32; 4] },
    /// A glyph on a disc of its colour (a meeting's calendar, focus done, the digest's moon).
    Glyph { glyph: char, rgb: [u8; 3] },
    /// A conversation's senders, overlapping (photos, or their initials on a colour).
    Faces(Vec<Face>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Face {
    Photo(String, Arc<Vec<u8>>),
    Initials(String, [u8; 3]),
}

/// A banner: one notification arriving, on a card of its own over the bell, drawn as the
/// notification centre draws it under its app's line, in its app's colour.
pub struct Banner {
    pub app: String,
    pub logo: Option<(String, Arc<Vec<u8>>)>,
    pub when: String,
    /// Its notification (`index` 0: a banner holds one).
    pub toast: Toast,
    pub hover: Option<Hit>,
    pub pressed: Option<Hit>,
    /// The app's colour, from its logo: washed in from the left, on its countdown and its art.
    pub tint: Option<[u8; 3]>,
    /// Its time left (1 to 0), a line along its foot; `None`: it stays (a call, a reminder).
    pub left: Option<f32>,
    /// The pointer holds it: the line stops, brighter.
    pub paused: bool,
    /// A conversation's banner: how many more came since its first, and how big that count is
    /// drawn (1, more for a moment as it grows).
    pub more: usize,
    pub more_pop: f32,
    /// A call: where its rings are in their pulse (0..1).
    pub pulse: Option<f32>,
    /// Confetti over the card (focus done): each piece's place, colour and opacity.
    pub confetti: Vec<(f32, f32, [u8; 3], u8)>,
}

/// A banner's card.
pub fn banner(b: &Banner) -> Scene {
    let h = 6.0 + BANNER_HEAD_H + b.toast.height() + 8.0;
    let main = R::new(0.0, 0.0, W, h);
    let mut ink = Ink2::new(b.hover, b.pressed, None);
    ink.draw(main, Draw::Card);
    if let Some(rgb) = b.tint {
        ink.draw(main, Draw::Wash { rgb, alpha: 84, reach: 0.6, radius: 0.0 });
    }
    let line = R::new(8.0, 6.0, W - 16.0, BANNER_HEAD_H);
    let mut tx = line.x + 8.0;
    if let Some((key, bytes)) = &b.logo {
        ink.draw(R::new(tx, line.y + (line.h - 16.0) / 2.0, 16.0, 16.0), Draw::Logo { key: key.clone(), bytes: bytes.clone() });
        tx += 24.0;
    }
    ink.text(R::new(tx, line.y, line.right() - tx - 140.0, line.h), b.app.clone(), Font::CaptionStrong, Ink::Text2, Align::Left);
    let when_w = text_w(&b.when, Font::Caption).max(30.0);
    ink.text(R::new(line.right() - 8.0 - when_w, line.y, when_w, line.h), b.when.clone(), Font::Caption, Ink::Text3, Align::Right);
    if b.more > 0 {
        // "+2 more", before its time, growing for a moment when it counts up.
        let label = (words().more)(b.more);
        let lw = text_w(&label, Font::CaptionStrong) * b.more_pop + 4.0;
        let at = R::new(line.right() - 16.0 - when_w - lw, line.y, lw, line.h);
        ink.text(at, label, Font::CaptionStrong, Ink::Accent, Align::Right);
    }
    let r = R::new(8.0, line.bottom(), W - 16.0, b.toast.height());
    notification(&mut ink, r, &b.toast);
    let spot = R::new(r.x + 12.0, r.y + 12.0, 36.0, 36.0);
    let tint = b.tint.unwrap_or([0xB5, 0x5C, 0xC4]);
    if let Some(phase) = b.pulse {
        // A call: two rings going out from the caller, one after the other.
        for k in 0..2 {
            let p = (phase + k as f32 * 0.5) % 1.0;
            let grow = 2.0 + p * 10.0;
            let ring = R::new(spot.x - grow, spot.y - grow, spot.w + 2.0 * grow, spot.h + 2.0 * grow);
            ink.draw(ring, Draw::Ring { rgb: tint, alpha: ((1.0 - p) * 200.0) as u8, width: 2.0 });
        }
    }
    if let Some(left) = b.left {
        // Its time left, along its foot: brighter while the pointer holds it.
        let track = R::new(20.0, h - 5.0, W - 40.0, 2.0);
        ink.draw(track, Draw::Rect { fill: Fill::Rgba(tint, 40), radius: 1.0 });
        let run = R::new(track.x, track.y, track.w * left.clamp(0.0, 1.0), track.h);
        ink.draw(run, Draw::Rect { fill: Fill::Rgba(tint, if b.paused { 255 } else { 190 }), radius: 1.0 });
    }
    for &(x, y, rgb, alpha) in &b.confetti {
        ink.draw(R::new(x, y, 5.0, 8.0), Draw::Rect { fill: Fill::Rgba(rgb, alpha), radius: 1.5 });
    }
    Scene { main, w: W, h, items: ink.items, hits: ink.hits, center_view: None, list_view: None, toasts: ink.places, order: ink.order }
}

/// A glance's height: a pill, as the edge pill's notices.
const GLANCE_H: f32 = 52.0;

/// A glance: news for a moment, never listed (a keyboard layout switched). Not a notification's
/// card but Audio Tray's own solid accent pill: its glyph in a disc, its words, and a hairline
/// running down the time it has left (`left`, 1 to 0).
pub fn glance(glyph: char, title: &str, body: &str, left: f32, hover: bool) -> Scene {
    let words_w = text_w(title, Font::Strong).max(text_w(body, Font::Caption));
    let w = (8.0 + 36.0 + 12.0 + words_w + 24.0).clamp(180.0, W);
    let main = R::new(0.0, 0.0, w, GLANCE_H);
    let mut ink = Ink2::new(None, None, None);
    let round = GLANCE_H / 2.0;
    ink.draw(main, Draw::Rect { fill: if hover { Fill::AccentHover } else { Fill::Accent }, radius: round });
    ink.hit(main, Hit::Dismiss(0));
    let disc = R::new(8.0, 8.0, 36.0, 36.0);
    ink.draw(disc, Draw::Rect { fill: Fill::OnAccentWash, radius: 18.0 });
    ink.glyph(disc, glyph, Font::Icon, Ink::OnAccent);
    let tx = disc.right() + 12.0;
    let tw = w - tx - 20.0;
    if body.is_empty() {
        ink.text(R::new(tx, 0.0, tw, GLANCE_H), title, Font::Strong, Ink::OnAccent, Align::Left);
    } else {
        ink.text(R::new(tx, 7.0, tw, 20.0), title, Font::Strong, Ink::OnAccent, Align::Left);
        ink.text(R::new(tx, 26.0, tw, 18.0), body, Font::Caption, Ink::OnAccent2, Align::Left);
    }
    // The time left, along the foot inside the curve.
    let run = w - 2.0 * round;
    if left > 0.0 && run > 0.0 {
        ink.draw(R::new(round, GLANCE_H - 5.0, run * left.clamp(0.0, 1.0), 2.0), Draw::Rect { fill: Fill::OnAccentWash, radius: 1.0 });
    }
    Scene { main, w, h: GLANCE_H, items: ink.items, hits: ink.hits, center_view: None, list_view: None, toasts: Vec::new(), order: ink.order }
}

/// A banner's picture in its photo's place.
fn art(ink: &mut Ink2, spot: R, art: &Art) {
    match art {
        Art::None => {}
        Art::Battery { level, low } => {
            let body = R::new(spot.x + 4.0, spot.y + 11.0, 25.0, 14.0);
            ink.draw(body, Draw::Outline { ink: Ink::Text, radius: 3.0 });
            ink.draw(R::new(body.right(), body.y + 4.0, 3.0, 6.0), Draw::Rect { fill: Fill::Rgba([0xFF; 3], 200), radius: 1.0 });
            let fill = if *low { [0xE5, 0x48, 0x4D] } else { [0xF2, 0xA3, 0x3A] };
            let inner = R::new(body.x + 2.5, body.y + 2.5, (body.w - 5.0) * level.clamp(0.04, 1.0), body.h - 5.0);
            ink.draw(inner, Draw::Rect { fill: Fill::Rgba(fill, 255), radius: 1.5 });
        }
        Art::Speaker { bars } => {
            ink.draw(spot, Draw::Rect { fill: Fill::Accent, radius: 18.0 });
            for (n, height) in bars.iter().enumerate() {
                let bh = 4.0 + height.clamp(0.0, 1.0) * 16.0;
                let bar = R::new(spot.x + 9.0 + n as f32 * 5.0, spot.y + (spot.h - bh) / 2.0, 3.0, bh);
                ink.draw(bar, Draw::Rect { fill: Fill::Rgba([0xFF; 3], 240), radius: 1.5 });
            }
        }
        Art::Glyph { glyph, rgb } => {
            ink.draw(spot, Draw::Rect { fill: Fill::Rgba(*rgb, 255), radius: 18.0 });
            ink.glyph(spot, *glyph, Font::Icon, Ink::Rgba([0xFF; 3], 255));
        }
        Art::Faces(faces) => {
            // Up to three, each a step to the right and down, the newest in front.
            let shown: Vec<&Face> = faces.iter().take(3).collect();
            let many = shown.len() > 1;
            let size = if many { 26.0 } else { 36.0 };
            for (n, face) in shown.iter().enumerate().rev() {
                let (dx, dy) = if many { (n as f32 * 9.0 - 4.0, n as f32 * 3.0 - 3.0) } else { (0.0, 0.0) };
                let f = R::new(spot.x + dx, spot.y + (spot.h - size) / 2.0 + dy, size, size);
                match face {
                    Face::Photo(key, bytes) => ink.draw(f, Draw::Avatar { key: key.clone(), bytes: bytes.clone() }),
                    Face::Initials(text, rgb) => {
                        ink.draw(f, Draw::Rect { fill: Fill::Rgba(*rgb, 255), radius: size / 2.0 });
                        ink.text(f, text.clone(), Font::CaptionStrong, Ink::Rgba([0xFF; 3], 255), Align::Center);
                    }
                }
            }
        }
    }
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
        _ => 9.0,
    };
    text.chars().count() as f32 * per
}

/// The hovered target's tooltip, under it (above it near the card's foot), inside the card.
fn tooltip(ink: &mut Ink2, input: &Input, (px, py): (f32, f32), card: R) {
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

/// One notification in `r`: the sender's photo, its text, its picture, then its buttons (or its
/// reply box, while open) and its menu.
fn notification(ink: &mut Ink2, r: R, toast: &Toast) {
    let i = toast.index;
    if !ink.inert {
        ink.places.push((i, r));
    }
    let lit = ink.hover.and_then(toast_of) == Some(i) || ink.focus.and_then(toast_of) == Some(i) || toast.draft.is_some();
    let fill = if ink.pressed == Some(Hit::Toast(i)) && ink.lit(Hit::Toast(i)) { Fill::Pressed } else if lit { Fill::Hover } else { Fill::Raised };
    ink.draw(r, Draw::Rect { fill, radius: 6.0 });
    if let Some(rgb) = toast.tint {
        // Its app's colour, from the left edge: the list as colourful as the banners.
        ink.draw(r, Draw::Wash { rgb, alpha: 64, reach: 0.55, radius: 6.0 });
    }
    ink.hit(r, Hit::Toast(i));
    let mut tx = r.x + 12.0;
    if toast.art != Art::None {
        // Its picture (a battery, a speaker, a conversation's faces) where a photo goes.
        art(ink, R::new(tx, r.y + 12.0, 36.0, 36.0), &toast.art);
        tx += 48.0;
    } else if let Some((key, bytes)) = &toast.avatar {
        ink.draw(R::new(tx, r.y + 12.0, 36.0, 36.0), Draw::Avatar { key: key.clone(), bytes: bytes.clone() });
        tx += 48.0;
    }
    ink.text(R::new(tx, r.y + 8.0, r.right() - tx - 76.0, 20.0), toast.title.clone(), Font::Strong, Ink::Text, Align::Left);
    ink.text(R::new(tx, r.y + 30.0, r.right() - tx - 12.0, 38.0), toast.body.clone(), Font::Wrap, Ink::Text2, Align::Left);
    if lit {
        // A full-size target, under the pointer or the keyboard.
        let x_r = R::new(r.right() - 38.0, r.y + 4.0, 32.0, 32.0);
        ink.plate(x_r, Hit::Dismiss(i), x_r.h / 2.0);
        ink.glyph(x_r, '\u{E711}', Font::IconSmall, Ink::Text2);
        if !toast.has_actions() && !toast.menu.is_empty() {
            // A menu with no buttons: its "…" beside the X, no row of its own.
            let more = R::new(x_r.x - 34.0, x_r.y, 32.0, 32.0);
            ink.plate(more, Hit::Menu(i), more.h / 2.0);
            ink.glyph(more, '\u{E712}', Font::IconSmall, Ink::Text2);
        }
    } else {
        // Its own time, in the X's place.
        ink.text(R::new(r.right() - 76.0, r.y + 8.0, 64.0, 20.0), toast.when.clone(), Font::Caption, Ink::Text3, Align::Right);
    }
    let mut y = r.y + TOAST_H - 4.0;
    if let Some((key, bytes)) = &toast.picture {
        ink.draw(R::new(r.x + 12.0, y, r.w - 24.0, PICTURE_H), Draw::Picture { key: key.clone(), bytes: bytes.clone() });
        y += PICTURE_H + 8.0;
    }
    if toast.has_actions() {
        let row = R::new(r.x + 12.0, y, r.w - 24.0, ACTIONS_H);
        match &toast.draft {
            // Sent: one word in the accent where its buttons were, for a moment.
            _ if toast.sent.is_some() => {
                let word = toast.sent.clone().unwrap_or_default();
                let pw = (text_w(&word, Font::CaptionStrong) + 32.0).min(row.w);
                let pill = R::new(row.x, row.y + 2.0, pw, row.h - 4.0);
                ink.draw(pill, Draw::Rect { fill: Fill::Accent, radius: pill.h / 2.0 });
                ink.text(pill, word, Font::CaptionStrong, Ink::OnAccent, Align::Center);
            }
            Some(draft) => {
                let field = R::new(row.x, row.y, row.w - ACTIONS_H - 6.0, row.h);
                ink.draw(field, Draw::Rect { fill: Fill::Raised, radius: 4.0 });
                ink.draw(field, Draw::Outline { ink: Ink::Accent, radius: 4.0 });
                ink.hit(field, Hit::Draft(i));
                let inner = R::new(field.x + 10.0, field.y, field.w - 20.0, field.h);
                if draft.is_empty() {
                    ink.draw(R::new(inner.x, inner.y + 8.0, 1.5, 16.0), Draw::Rect { fill: Fill::Accent, radius: 0.0 });
                    let hint = toast.reply.clone().unwrap_or_default();
                    ink.text(R::new(inner.x + 4.0, inner.y, inner.w - 4.0, inner.h), hint, Font::Body, Ink::Text3, Align::Left);
                } else {
                    // The end of what is typed stays in view, the caret after it.
                    let long = draft.chars().count() > 36;
                    ink.draw(inner, Draw::Clip);
                    ink.text(inner, format!("{draft}\u{2009}|"), Font::Body, Ink::Text, if long { Align::Right } else { Align::Left });
                    ink.draw(inner, Draw::Unclip);
                }
                let send = R::new(row.right() - ACTIONS_H, row.y, ACTIONS_H, ACTIONS_H);
                let fill = ink.accent(Hit::Send(i));
                ink.draw(send, Draw::Rect { fill, radius: 4.0 });
                ink.glyph(send, '\u{E724}', Font::IconSmall, Ink::OnAccent);
                ink.hit(send, Hit::Send(i));
            }
            None => {
                let mut bx = row.x;
                let end = if toast.menu.is_empty() { row.right() } else { row.right() - ACTIONS_H - 6.0 };
                let mut pills: Vec<(String, Hit)> = toast.buttons.iter().enumerate().map(|(n, label)| (label.clone(), Hit::Action(i, n))).collect();
                if toast.reply.is_some() {
                    pills.insert(0, (words().reply.to_string(), Hit::Reply(i)));
                    // The reactions after Reply, before the app's own buttons.
                    for (n, emoji) in toast.quick.iter().enumerate() {
                        pills.insert(1 + n, (emoji.clone(), Hit::Quick(i, n)));
                    }
                }
                let buttons = toast.buttons.len();
                for (label, hit) in pills {
                    // What room is left, before sizing: a clamp with less than its minimum panics.
                    let room = end - bx;
                    if room < 32.0 {
                        break;
                    }
                    if matches!(hit, Hit::Quick(..)) {
                        // A reaction: a small round button, the emoji in its colours.
                        let q = R::new(bx, row.y + 2.0, (row.h - 4.0).min(room), row.h - 4.0);
                        let fill = ink.surface(hit, Fill::Raised);
                        ink.draw(q, Draw::Rect { fill, radius: q.h / 2.0 });
                        ink.draw(q, Draw::Outline { ink: Ink::Text3, radius: q.h / 2.0 });
                        ink.text(q, label, Font::Body, Ink::Text, Align::Center);
                        ink.hit(q, hit);
                        bx += q.w + 6.0;
                        continue;
                    }
                    let pw = (label.chars().count() as f32 * 7.0 + 28.0).max(64.0).min(room);
                    let pill = R::new(bx, row.y + 2.0, pw, row.h - 4.0);
                    // A call answers in green and declines in red, as phones do.
                    let call = match hit {
                        Hit::Action(_, 0) if toast.call && buttons >= 2 => Some(([0x3F, 0xBF, 0x7F], [0x08, 0x15, 0x0E])),
                        Hit::Action(_, n) if toast.call && buttons >= 2 && n + 1 == buttons => Some(([0xE5, 0x48, 0x4D], [0xFF, 0xFF, 0xFF])),
                        _ => None,
                    };
                    match call {
                        Some((ground, text)) => {
                            let lift = if ink.lit(hit) { 230 } else { 255 };
                            ink.draw(pill, Draw::Rect { fill: Fill::Rgba(ground, lift), radius: pill.h / 2.0 });
                            ink.text(pill, label, Font::CaptionStrong, Ink::Rgba(text, 255), Align::Center);
                            ink.hit(pill, hit);
                        }
                        None => ink.pill(pill, &label, hit, false),
                    }
                    bx += pw + 6.0;
                }
                if !toast.menu.is_empty() {
                    let more = R::new(row.right() - ACTIONS_H, row.y, ACTIONS_H, ACTIONS_H);
                    ink.plate(more, Hit::Menu(i), more.h / 2.0);
                    ink.glyph(more, '\u{E712}', Font::IconSmall, Ink::Text2);
                }
            }
        }
        y += ACTIONS_H + 8.0;
    }
    if toast.menu_open {
        for (n, label) in toast.menu.iter().enumerate() {
            let line = R::new(r.x + 8.0, y, r.w - 16.0, MENU_ROW_H);
            ink.plate(line, Hit::MenuItem(i, n), 4.0);
            ink.text(R::new(line.x + 12.0, line.y, line.w - 24.0, line.h), label.clone(), Font::Body, Ink::Text, Align::Left);
            y += MENU_ROW_H;
        }
    }
}

/// A thin bar at `area`'s right saying there is more than it shows, and where it is: `content` is
/// the scrolled height, `scroll` how far down it is.
fn scrollbar(ink: &mut Ink2, area: R, content: f32, scroll: f32) {
    if content <= area.h + 0.5 {
        return;
    }
    let h = (area.h * area.h / content).max(24.0);
    let y = area.y + (area.h - h) * (scroll / (content - area.h)).clamp(0.0, 1.0);
    ink.draw(R::new(area.right() - 6.0, y, 3.0, h), Draw::Rect { fill: Fill::Mark, radius: 1.5 });
}

/// How many are waiting in all, in an outlined chip at `(x, y)`, as wide as its number (99+ at
/// most). Grey, not the accent: the clock's accent badge counts only the new ones.
fn count_chip(ink: &mut Ink2, x: f32, y: f32, unread: usize) -> R {
    let label = if unread > 99 { "99+".to_string() } else { unread.to_string() };
    let chip = R::new(x, y, (label.chars().count() as f32 * 7.0 + 10.0).max(24.0), 18.0);
    ink.draw(chip, Draw::Outline { ink: Ink::Text3, radius: 9.0 });
    ink.text(chip, label, Font::CaptionStrong, Ink::Text2, Align::Center);
    chip
}

/// The notification centre: its header, each app's notifications, and the focus timer at its
/// foot. Returns its scrolling area.
fn center(ink: &mut Ink2, card: R, input: &Input) -> (R, f32) {
    let groups = &input.groups;
    let x = card.x + 8.0;
    let w = card.w - 16.0;
    let t = words();
    let head = R::new(x, card.y + 10.0, w, 32.0);
    let title_w = text_w(t.notifications, Font::Strong) + 4.0;
    ink.text(R::new(head.x + 8.0, head.y, title_w, head.h), t.notifications, Font::Strong, Ink::Text, Align::Left);
    if input.unread > 0 {
        count_chip(ink, head.x + 8.0 + title_w + 4.0, head.y + 7.0, input.unread);
    }
    // Round buttons from the right, as every panel's header: clear all (when there is anything),
    // Do not disturb, the focus timer (lit while it runs; its tooltip says the time left).
    let mut right = head.right();
    let mut button = |ink: &mut Ink2, glyph: char, hit: Hit, on: bool| {
        let r = R::new(right - 32.0, head.y, 32.0, 32.0);
        ink.icon_button(r, glyph, hit, on);
        right = r.x - 4.0;
    };
    if input.unread > 0 {
        button(ink, '\u{E74D}', Hit::ClearAll, false);
    }
    button(ink, '\u{E708}', Hit::Quiet, input.quiet);
    button(ink, '\u{E916}', Hit::Focus, input.focus_running);
    footer(ink, card, '\u{EA8F}', words().notification_settings);

    let top = head.bottom() + 6.0;
    let area = R::new(card.x, top, card.w, card.bottom() - FOOTER_H - 4.0 - top);
    if groups.is_empty() {
        // None waiting, as Windows' centre says it; or none readable, and where to allow it.
        let line = R::new(x + 8.0, area.y, w - 16.0, EMPTY_H - 8.0);
        if input.notifications_denied {
            ink.hit(line, Hit::Allow);
            let tint = if ink.lit(Hit::Allow) { Ink::Accent } else { Ink::Text2 };
            ink.text(line, t.not_allowed, Font::Body, tint, Align::Left);
        } else {
            // What was just done ("3 notifications cleared") for a moment, then the empty state.
            let said = input.flash.clone().unwrap_or_else(|| t.no_notifications.to_string());
            ink.text(line, said, Font::Body, Ink::Text3, Align::Left);
        }
        return (area, 0.0);
    }
    let content = center_height(groups) - CENTER_HEAD_H - 8.0;
    let max = (content - area.h).max(0.0);
    let scroll = input.center_scroll.clamp(0.0, max);
    ink.draw(area, Draw::Clip);
    let mut y = area.y - scroll;
    for (g, group) in groups.iter().enumerate() {
        // A group whose last notifications are going collapses with them, its line too.
        let shown = group.collapse();
        let line = R::new(x, y, w, GROUP_H);
        let folding = shown < 1.0;
        if folding {
            ink.inert = true;
            ink.draw(R::new(line.x, line.y, line.w, line.h * shown), Draw::Clip);
        }
        // No plate: one would run into the notification under it. The name brightens instead.
        ink.hit(line, Hit::Group(g));
        let lit = ink.lit(Hit::Group(g)) || ink.lit(Hit::DismissGroup(g));
        let mut tx = line.x + 8.0;
        if let Some((key, bytes)) = &group.logo {
            ink.draw(R::new(tx, line.y + (line.h - 16.0) / 2.0, 16.0, 16.0), Draw::Logo { key: key.clone(), bytes: bytes.clone() });
            tx += 24.0;
        }
        ink.text(R::new(tx, line.y, line.right() - tx - 80.0, line.h), group.name.clone(), Font::CaptionStrong, if lit { Ink::Text } else { Ink::Text2 }, Align::Left);
        if lit {
            // Under the pointer: clear all of this app's, in place of its time.
            let x_r = R::new(line.right() - 30.0, line.y + (line.h - 24.0) / 2.0, 24.0, 24.0);
            ink.plate(x_r, Hit::DismissGroup(g), x_r.h / 2.0);
            ink.glyph(x_r, '\u{E711}', Font::IconSmall, Ink::Text2);
        } else {
            ink.text(R::new(line.right() - 80.0, line.y, 72.0, line.h), group.when.clone(), Font::Caption, Ink::Text3, Align::Right);
        }
        if folding {
            ink.draw(line, Draw::Unclip);
            ink.inert = false;
        }
        y += line.h * shown;
        for toast in &group.toasts {
            let r = R::new(x, y, w, toast.height());
            if toast.collapse < 1.0 {
                // Going: drawn in what is left of it (its foot first), the ones under it coming
                // up, and no target in it.
                ink.inert = true;
                ink.draw(R::new(r.x, r.y, r.w, r.h * toast.collapse.max(0.0)), Draw::Clip);
                notification(ink, r, toast);
                ink.draw(r, Draw::Unclip);
                ink.inert = false;
            } else {
                notification(ink, r, toast);
            }
            y += toast.room();
        }
    }
    ink.draw(area, Draw::Unclip);
    scrollbar(ink, area, content, scroll);
    (area, max)
}

/// The calendar card. Returns the list's scrolling area.
fn calendar(ink: &mut Ink2, card: R, input: &Input, rows: usize, hero: bool) -> Option<(R, f32)> {
    let x = card.x + PAD;
    let w = card.w - 2.0 * PAD;
    let mut y = card.y + PAD;

    // The date and the world clocks.
    // Hovers here tint the text (a link's), not a plate: plates around short text read as boxes.
    let today = R::new(x, y, 200.0, 44.0);
    ink.hit(today, Hit::Today);
    let lit = ink.lit(Hit::Today);
    ink.text(R::new(x, y, 200.0, 16.0), input.weekday.clone(), Font::Caption, if lit { Ink::Accent } else { Ink::Text2 }, Align::Left);
    ink.text(R::new(x, y + 16.0, 220.0, 28.0), input.date.clone(), Font::Big, if lit { Ink::Accent } else { Ink::Text }, Align::Left);
    let edge = card.right() - PAD + 12.0;
    if !input.clocks.is_empty() {
        // On the weekday's line, at the card's edge: the big date beneath keeps its room (with
        // two clocks, they ran into it).
        let tw = (text_w(&input.clocks, Font::Caption) + 4.0).min(edge - 12.0 - x - 90.0).max(40.0);
        let r = R::new(edge - 12.0 - tw, y - 2.0, tw, 18.0);
        ink.hit(r, Hit::Clocks);
        let tint = if ink.lit(Hit::Clocks) { Ink::Accent } else { Ink::Text2 };
        ink.text(r, input.clocks.clone(), Font::Caption, tint, Align::Right);
    }
    y += 44.0 + 14.0;

    // The month; nothing with the agenda alone.
    if input.grid != Grid::None {
        y = days_grid(ink, card, input, y);
    }

    // The next event.
    if let Some(hero) = input.hero.as_ref().filter(|_| hero) {
        y += 10.0;
        let r = R::new(x, y, w, 76.0);
        let fill = ink.surface(Hit::Hero, Fill::Raised);
        ink.draw(r, Draw::Rect { fill, radius: 6.0 });
        ink.hit(r, Hit::Hero);
        ink.draw(R::new(r.x + 12.0, r.y + 12.0, 3.0, r.h - 24.0), Draw::Rect { fill: Fill::Rgb(hero.color), radius: 1.5 });
        let tw = r.w - 40.0 - if hero.joinable { join_w() + 12.0 } else { 0.0 };
        ink.text(R::new(r.x + 26.0, r.y + 10.0, tw, 16.0), hero.caption.clone(), Font::TinyStrong, Ink::Accent, Align::Left);
        ink.text(R::new(r.x + 26.0, r.y + 27.0, tw, 20.0), hero.title.clone(), Font::Strong, Ink::Text, Align::Left);
        ink.text(R::new(r.x + 26.0, r.y + 48.0, tw, 18.0), hero.when.clone(), Font::Caption, Ink::Text2, Align::Left);
        if hero.joinable {
            ink.join(r.right() - 12.0, r.y + 25.0, 26.0, Hit::HeroJoin);
        }
        y += 76.0;
    }

    // The rest of the week, or the picked day.
    y += 14.0;
    ink.text(R::new(x, y, w, 20.0), input.list_title.clone(), Font::CaptionStrong, Ink::Text2, Align::Left);
    y += 24.0;
    if let Notice::Problem(problem) = &input.notice {
        // A source failed: a line of its own, in the accent, a link to where it is mended (the
        // events under it may be old).
        let r = R::new(x, y, w, PROBLEM_H - 4.0);
        ink.hit(r, Hit::Settings);
        ink.glyph(R::new(r.x, r.y, 16.0, r.h), '\u{E7BA}', Font::IconSmall, Ink::Accent);
        let tint = if ink.lit(Hit::Settings) { Ink::Text } else { Ink::Accent };
        ink.text(R::new(r.x + 24.0, r.y, r.w - 24.0, r.h), format!("{problem} \u{2192}"), Font::Caption, tint, Align::Left);
        y += PROBLEM_H;
    }
    let area = R::new(card.x, y, card.w, rows.max(1) as f32 * ROW_H);
    let max = (input.rows.len() as f32 * ROW_H - area.h).max(0.0);
    let scroll = input.list_scroll.clamp(0.0, max);
    if input.rows.is_empty() {
        let line = R::new(x, y, w, ROW_H);
        match &input.notice {
            Notice::NoSources => {
                ink.hit(line, Hit::Settings);
                let tint = if ink.lit(Hit::Settings) { Ink::Accent } else { Ink::Text2 };
                ink.text(line, words().connect, Font::Body, tint, Align::Left);
            }
            Notice::Loading => ink.text(line, words().loading, Font::Body, Ink::Text3, Align::Left),
            _ => ink.text(line, input.empty.clone(), Font::Body, Ink::Text3, Align::Left),
        }
    }
    // The when column as wide as its longest label (12-hour times are longer), within reason.
    let left_w = input.rows.iter().map(|r| r.left.chars().count() as f32 * 6.5 + 4.0).fold(76.0, f32::max).min(120.0);
    ink.draw(area, Draw::Clip);
    for (i, row) in input.rows.iter().enumerate() {
        let r = R::new(x - 6.0, area.y + i as f32 * ROW_H - scroll, w + 12.0, ROW_H);
        if r.bottom() <= area.y || r.y >= area.bottom() {
            continue;
        }
        ink.plate(r, Hit::Row(i), 4.0);
        ink.text(R::new(r.x + 6.0, r.y, left_w, r.h), row.left.clone(), Font::Caption, Ink::Text2, Align::Left);
        ink.draw(R::new(r.x + left_w + 12.0, r.y + 13.0, 6.0, 6.0), Draw::Rect { fill: Fill::Rgb(row.color), radius: 3.0 });
        let right_w = if row.joinable { join_w() + 8.0 } else if row.right.is_empty() { 0.0 } else { 56.0 };
        let tx = r.x + left_w + 26.0;
        ink.text(R::new(tx, r.y, r.right() - tx - 6.0 - right_w, r.h), row.title.clone(), Font::Body, Ink::Text, Align::Left);
        if row.joinable {
            ink.join(r.right() - 6.0, r.y + 4.0, 24.0, Hit::Join(i));
        } else if !row.right.is_empty() {
            ink.text(R::new(r.right() - 62.0, r.y, 56.0, r.h), row.right.clone(), Font::Caption, Ink::Text3, Align::Right);
        }
    }
    ink.draw(area, Draw::Unclip);
    scrollbar(ink, area, input.rows.len() as f32 * ROW_H, scroll);

    footer(ink, card, '\u{E787}', words().date_settings);
    Some((area, max))
}

/// A card's foot, the same on every panel (the audio panel's too): a darker strip under a
/// hairline, on the left a labelled link to Windows' own settings for what the card shows
/// (`glyph`, `label`), on the right Audio Tray's settings as a round gear.
fn footer(ink: &mut Ink2, card: R, glyph: char, label: &str) {
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


/// A day cell's height and its circle's size, for the grid.
fn cell_size(grid: Grid) -> (f32, f32) {
    match grid {
        Grid::Compact => (32.0, 24.0),
        _ => (40.0, 30.0),
    }
}

/// The grid's height (its title, the weekdays, the days).
fn grid_height(input: &Input) -> f32 {
    if input.grid == Grid::None {
        return 0.0;
    }
    let (cell, _) = cell_size(input.grid);
    32.0 + 22.0 + 6.0 * cell
}

/// The month's days from `y`, with week numbers when asked. Returns where it ends.
fn days_grid(ink: &mut Ink2, card: R, input: &Input, mut y: f32) -> f32 {
    let x = card.x + PAD;
    let w = card.w - 2.0 * PAD;
    ink.text(R::new(x, y, w - 72.0, 28.0), input.month_title.clone(), Font::Strong, Ink::Text, Align::Left);
    let next = R::new(card.right() - PAD - 28.0, y, 28.0, 28.0);
    let prev = R::new(next.x - 32.0, y, 28.0, 28.0);
    ink.plate(prev, Hit::Prev, prev.h / 2.0);
    ink.glyph(prev, '\u{E76B}', Font::IconSmall, Ink::Text2);
    ink.plate(next, Hit::Next, next.h / 2.0);
    ink.glyph(next, '\u{E76C}', Font::IconSmall, Ink::Text2);
    y += 32.0;
    // Week numbers in a narrow column of their own, before Monday (or the week's first day).
    let numbers = if input.week_numbers.is_empty() { 0.0 } else { 22.0 };
    let gx = x + numbers;
    let col = (w - numbers) / 7.0;
    for (i, head) in input.heads.iter().enumerate() {
        ink.text(R::new(gx + i as f32 * col, y, col, 18.0), head.clone(), Font::Tiny, Ink::Text3, Align::Center);
    }
    y += 22.0;
    let (cell_h, d) = cell_size(input.grid);
    for (row, number) in input.week_numbers.iter().enumerate() {
        ink.text(R::new(x, y + row as f32 * cell_h + 2.0, numbers - 4.0, d), number.to_string(), Font::Tiny, Ink::Text3, Align::Left);
    }
    for (i, cell) in input.days.iter().enumerate() {
        let r = R::new(gx + (i % 7) as f32 * col, y + (i / 7) as f32 * cell_h, col, cell_h);
        let circle = R::new(r.x + (r.w - d) / 2.0, r.y + 2.0, d, d);
        if cell.today {
            ink.draw(circle, Draw::Rect { fill: Fill::Accent, radius: d / 2.0 });
        } else if ink.lit(Hit::Day(cell.day)) {
            ink.draw(circle, Draw::Rect { fill: Fill::Hover, radius: d / 2.0 });
        }
        if cell.selected && !cell.today {
            ink.draw(circle, Draw::Outline { ink: Ink::Accent, radius: d / 2.0 });
        }
        let color = if cell.today { Ink::OnAccent } else if cell.dim { Ink::Text3 } else { Ink::Text };
        ink.text(circle, cell.label.clone(), Font::Body, color, Align::Center);
        if cell.busy > 0 {
            let bw = if cell.busy > 1 { 10.0 } else { 4.0 };
            let mark = R::new(r.x + (r.w - bw) / 2.0, (circle.bottom() + 3.0).min(r.bottom() - 4.0), bw, 4.0);
            ink.draw(mark, Draw::Rect { fill: if cell.today { Fill::Accent } else { Fill::Mark }, radius: 2.0 });
        }
        ink.hit(r, Hit::Day(cell.day));
    }
    y + (input.days.len().div_ceil(7)) as f32 * cell_h + 4.0
}

/// One event's details, in place of the calendar.
fn detail_card(ink: &mut Ink2, card: R, d: &Detail) {
    let x = card.x + PAD;
    let w = card.w - 2.0 * PAD;
    let mut y = card.y + 12.0;
    let back = R::new(x - 8.0, y, 32.0, 32.0);
    ink.plate(back, Hit::Back, back.h / 2.0);
    ink.glyph(back, '\u{E72B}', Font::IconSmall, Ink::Text);
    ink.text(R::new(x + 28.0, y, w - 28.0, 32.0), d.back.clone(), Font::Caption, Ink::Text2, Align::Left);
    y += 44.0;

    // The title whole, on two lines when it needs them (about 9.5 DIPs a character at 18).
    let two = d.title.chars().count() as f32 * 9.5 > w - 16.0;
    let title_h = if two { 48.0 } else { 24.0 };
    ink.draw(R::new(x, y + 2.0, 4.0, 38.0 + title_h), Draw::Rect { fill: Fill::Rgb(d.color), radius: 2.0 });
    ink.text(R::new(x + 16.0, y, w - 16.0, title_h), d.title.clone(), if two { Font::TitleWrap } else { Font::Title }, Ink::Text, Align::Left);
    ink.text(R::new(x + 16.0, y + title_h + 2.0, w - 16.0, 20.0), d.when.clone(), Font::Body, Ink::Text2, Align::Left);
    ink.text(R::new(x + 16.0, y + title_h + 22.0, w - 16.0, 18.0), d.soon.clone(), Font::Caption, Ink::Accent, Align::Left);
    y += 58.0 + title_h;

    let line = |ink: &mut Ink2, y: &mut f32, glyph: char, text: &str, h: f32| {
        ink.glyph(R::new(x, *y, 16.0, 20.0), glyph, Font::IconSmall, Ink::Text2);
        ink.text(R::new(x + 30.0, *y, w - 30.0, h), text, if h > 20.0 { Font::Wrap } else { Font::Body }, Ink::Text, Align::Left);
        *y += h + 10.0;
    };
    if !d.place.is_empty() {
        line(ink, &mut y, '\u{E707}', &d.place, 20.0);
        // A place one can go to, not an online meeting's.
        if d.directions {
            let dir = R::new(x + 26.0, y - 8.0, text_w(words().directions, Font::Caption) + 12.0, 20.0);
            ink.plate(dir, Hit::Directions, 4.0);
            ink.text(R::new(dir.x + 4.0, dir.y, dir.w, dir.h), words().directions, Font::Caption, Ink::Accent, Align::Left);
            y += 16.0;
        }
    }
    if !d.people.is_empty() {
        ink.glyph(R::new(x, y + 2.0, 16.0, 24.0), '\u{E716}', Font::IconSmall, Ink::Text2);
        let mut px = x + 30.0;
        for initials in d.initials.iter().take(3) {
            let c = R::new(px, y, 26.0, 26.0);
            ink.draw(c, Draw::Rect { fill: Fill::Raised, radius: 13.0 });
            ink.text(c, initials.clone(), Font::Tiny, Ink::Text, Align::Center);
            px += 30.0;
        }
        ink.text(R::new(px + 4.0, y, x + w - px - 4.0, 26.0), d.people.clone(), Font::Caption, Ink::Text2, Align::Left);
        y += 36.0;
    }
    let buttons_y = card.bottom() - PAD - 34.0;
    if !d.notes.is_empty() {
        let h = (buttons_y - 70.0 - y).clamp(20.0, 120.0);
        line(ink, &mut y, '\u{E70B}', &d.notes, h);
    }
    line(ink, &mut y, '\u{E787}', &d.source, 20.0);

    let mut bx = x;
    if d.joinable {
        let r = R::new(bx, buttons_y, (text_w(words().join, Font::CaptionStrong) + 40.0).max(96.0), 34.0);
        let fill = ink.accent(Hit::DetailJoin);
        ink.draw(r, Draw::Rect { fill, radius: 4.0 });
        ink.text(r, words().join, Font::CaptionStrong, Ink::OnAccent, Align::Center);
        ink.hit(r, Hit::DetailJoin);
        bx += r.w + 8.0;
    }
    if let Some(open) = &d.open {
        let r = R::new(bx, buttons_y, x + w - bx, 34.0);
        let fill = ink.surface(Hit::DetailOpen, Fill::Raised);
        ink.draw(r, Draw::Rect { fill, radius: 4.0 });
        ink.text(r, open.clone(), Font::Caption, Ink::Text, Align::Center);
        ink.hit(r, Hit::DetailOpen);
    }
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
            focus_running: false,
            detail: None,
            hover: None,
            show_tip: None,
        }
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

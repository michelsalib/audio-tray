//! The panel's two cards laid out, in DIPs: what to draw where, and what each place does when
//! clicked. Pure: the view hands in what there is to show (strings already in the user's
//! language) and paints the result; tests check the geometry.
//!
//! Top, when there are any: the notification centre (grouped by app; the newest group open, the
//! others one line each), or its one-line summary while the month is open. Below: the calendar
//! (the date and world clocks, this week or the whole month, the next event, the rest of the week
//! or the picked day, and the focus and Do not disturb switches), or one event's details.

use std::sync::Arc;

use super::paint::{Align, Font};

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

/// The cards' width, and the gap between them.
pub const W: f32 = 360.0;
pub const GAP: f32 = 12.0;
const PAD: f32 = 16.0;
const TOAST_H: f32 = 76.0;
/// A notification's large picture.
const PICTURE_H: f32 = 150.0;
/// The row of a notification's buttons (or its reply box), and a line of its menu.
const ACTIONS_H: f32 = 32.0;
const MENU_ROW_H: f32 = 30.0;
const GROUP_H: f32 = 28.0;
const FOLDED_H: f32 = 36.0;
const SUMMARY_H: f32 = 48.0;
const ROW_H: f32 = 32.0;
const FOOTER_H: f32 = 52.0;
/// The notification centre never takes less than this; below it, it folds to its summary.
const CENTER_MIN: f32 = 150.0;

/// A colour of the palette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ink {
    Text,
    Text2,
    Text3,
    Accent,
    OnAccent,
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
    Rule,
    /// The marks under busy days.
    Mark,
    /// A tooltip's ground: opaque, over anything.
    Tip,
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
    Clip,
    Unclip,
}

/// What a click lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// The notification centre's summary (opens it, closing the month).
    Center,
    /// An app's line: open or fold its notifications.
    Group(usize),
    /// "N more from …": every notification of that app.
    More(usize),
    /// Clear all of an app's notifications.
    DismissGroup(usize),
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
    /// The week or the whole month.
    Month,
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
    Settings,
    Clocks,
    /// Give the notification centre the whole panel, or take it back.
    Expand,
    /// The calendar folded to one line: unfold it.
    Calendar,
}

pub struct Toast {
    /// Its place in the notification list.
    pub index: usize,
    pub title: String,
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
}

impl Toast {
    fn has_actions(&self) -> bool {
        !self.buttons.is_empty() || self.reply.is_some() || !self.menu.is_empty()
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
        Hit::Toast(i) | Hit::Dismiss(i) | Hit::Action(i, _) | Hit::Reply(i) | Hit::Draft(i) | Hit::Send(i) | Hit::Menu(i) | Hit::MenuItem(i, _) => Some(i),
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
    pub open: bool,
    /// Every notification shown, not only the newest.
    pub all: bool,
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
    /// How the sources stand.
    pub notice: Notice,
    /// The target the keyboard is on (Tab), ringed.
    pub focus: Option<Hit>,
    /// The target under a press.
    pub pressed: Option<Hit>,
    /// A word for a moment, over the calendar's foot (Windows refused something).
    pub flash: Option<String>,
    /// The height the cards may take together (stacked), or each (side by side).
    pub room: f32,
    /// Side by side, where the screen is wide enough: `None` stacks them.
    pub columns: Option<Columns>,
    /// `None`: no notification centre (off, or nothing waiting).
    pub groups: Option<Vec<Group>>,
    pub unread: usize,
    pub quiet: bool,
    pub center_scroll: f32,
    pub weekday: String,
    pub date: String,
    pub clocks: String,
    pub month_open: bool,
    pub month_title: String,
    pub heads: [String; 7],
    /// The week (7) or the month (42).
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
    /// The notification centre takes the whole panel, the calendar folded to one line.
    pub center_full: bool,
    /// Where the pointer rested on `hover`: its tooltip shows by that target.
    pub show_tip: Option<(f32, f32)>,
}

/// The two cards side by side, each the screen's height to fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Columns {
    /// The calendar on the left (the taskbar is): it stays by the clock.
    pub calendar_left: bool,
    /// Both hang from the top (the taskbar is there), else stand on the bottom.
    pub from_top: bool,
}

pub struct Scene {
    /// The notification centre's card, when shown, and the calendar's.
    pub top: Option<R>,
    pub main: R,
    /// The frame's size: both cards and what lies between them.
    pub w: f32,
    pub h: f32,
    pub items: Vec<(R, Draw)>,
    pub hits: Vec<(R, Hit)>,
    /// Where each list scrolls, and how far at most.
    pub center_view: Option<(R, f32)>,
    pub list_view: Option<(R, f32)>,
    /// Each notification drawn (by its place in the list), whole: unclipped.
    pub toasts: Vec<(usize, R)>,
}

impl Scene {
    /// The topmost target at `(x, y)`.
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map(|(_, h)| *h)
    }

    /// Whether `(x, y)` is on a card (else, a click there closes the panel).
    pub fn on_card(&self, x: f32, y: f32) -> bool {
        self.main.contains(x, y) || self.top.is_some_and(|t| t.contains(x, y))
    }
}

/// A builder: drawing in order, targets topmost last.
struct Ink2 {
    items: Vec<(R, Draw)>,
    hits: Vec<(R, Hit)>,
    hover: Option<Hit>,
    pressed: Option<Hit>,
    /// Where each notification was drawn, unclipped.
    places: Vec<(usize, R)>,
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
    fn new(hover: Option<Hit>, pressed: Option<Hit>) -> Self {
        Ink2 { items: Vec::new(), hits: Vec::new(), hover, pressed, places: Vec::new(), clips: Vec::new() }
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
        self.hover == Some(hit)
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

    /// A small square button with a glyph.
    fn icon_button(&mut self, r: R, glyph: char, hit: Hit, on: bool) {
        if on {
            self.draw(r, Draw::Rect { fill: Fill::Accent, radius: 4.0 });
            self.hit(r, hit);
            self.glyph(r, glyph, Font::Icon, Ink::OnAccent);
        } else {
            self.plate(r, hit, 4.0);
            self.glyph(r, glyph, Font::Icon, Ink::Text);
        }
    }

    /// A pill with a label; `on` fills it with the accent.
    fn pill(&mut self, r: R, label: &str, hit: Hit, on: bool) {
        let fill = if on { Fill::Accent } else { self.surface(hit, Fill::Raised) };
        self.draw(r, Draw::Rect { fill, radius: r.h / 2.0 });
        if !on {
            self.draw(r, Draw::Outline { ink: Ink::Text3, radius: r.h / 2.0 });
        }
        self.text(r, label, Font::Caption, if on { Ink::OnAccent } else { Ink::Text }, Align::Center);
        self.hit(r, hit);
    }

    fn join(&mut self, r: R, hit: Hit) {
        self.draw(r, Draw::Rect { fill: Fill::Accent, radius: 4.0 });
        self.text(r, "Join", Font::CaptionStrong, Ink::OnAccent, Align::Center);
        self.hit(r, hit);
    }
}

/// The notification centre's natural height (open), for `groups`.
fn center_height(groups: &[Group]) -> f32 {
    let mut h = 12.0 + 32.0 + 6.0;
    for g in groups {
        if g.open {
            let shown = if g.all { g.toasts.len() } else { 1 };
            h += GROUP_H + g.toasts.iter().take(shown).map(|t| t.height() + 4.0).sum::<f32>();
            if !g.all && g.toasts.len() > 1 {
                h += 6.0 + 22.0;
            }
        } else {
            h += FOLDED_H;
        }
    }
    h + 8.0
}

/// An event's details at most (shorter when the screen is).
const DETAIL_H: f32 = 480.0;

/// The calendar card's height, the list's rows at most `rows`, with the next event or not.
fn main_height(input: &Input, rows: usize, hero: bool) -> f32 {
    if input.detail.is_some() {
        return DETAIL_H;
    }
    let mut h = PAD + 44.0 + 14.0;
    h += if input.month_open { 32.0 + 22.0 + 6.0 * 40.0 } else { 62.0 };
    if hero {
        h += 14.0 + 76.0;
    }
    h += 14.0 + 24.0 + rows.max(1) as f32 * ROW_H;
    h + 8.0 + FOOTER_H
}

/// Where the cards go, and how much each shows.
struct Plan {
    top: Option<R>,
    main: R,
    /// The notification centre folded to its summary.
    folded: bool,
    /// The notification centre given the whole panel, the calendar folded.
    full: bool,
    rows: usize,
    hero: bool,
}

/// Side by side, a list shows this many events at most (more scroll).
const ROWS_WIDE: usize = 12;

/// Side by side: each card takes the height it needs, up to the screen's, and neither squeezes the
/// other. Both stand on the taskbar's edge (or hang from it at the top).
fn side_by_side(input: &Input, columns: Columns, groups: &[Group]) -> Plan {
    let room = input.room;
    let hero = input.hero.is_some() && (!input.month_open || main_height(input, 1, true) <= room);
    let mut rows = input.rows.len().clamp(1, ROWS_WIDE);
    while rows > 1 && main_height(input, rows, hero) > room {
        rows -= 1;
    }
    let mut main_h = main_height(input, rows, hero);
    if input.detail.is_some() {
        main_h = main_h.min(room).max(240.0);
    }
    let center_h = center_height(groups).min(room).max(CENTER_MIN.min(room));
    let h = main_h.max(center_h);
    let y = |card_h: f32| if columns.from_top { 0.0 } else { h - card_h };
    let (main_x, top_x) = if columns.calendar_left { (0.0, W + GAP) } else { (W + GAP, 0.0) };
    Plan { top: Some(R::new(top_x, y(center_h), W, center_h)), main: R::new(main_x, y(main_h), W, main_h), folded: false, full: false, rows, hero }
}

pub fn build(input: &Input) -> Scene {
    let mut ink = Ink2::new(input.hover, input.pressed);
    let plan = match (input.columns, &input.groups) {
        (Some(columns), Some(groups)) => side_by_side(input, columns, groups),
        _ => stacked(input),
    };
    let Plan { top, main, folded, full, rows, hero } = plan;
    let mut center_view = None;
    if let (Some(card), Some(groups)) = (top, &input.groups) {
        ink.draw(card, Draw::Card);
        if folded {
            center_summary(&mut ink, card, input, groups);
        } else {
            center_view = Some(center(&mut ink, card, input, groups));
        }
    }
    ink.draw(main, Draw::Card);
    let list_view = match &input.detail {
        _ if full => {
            calendar_summary(&mut ink, main, input);
            None
        }
        Some(detail) => {
            detail_card(&mut ink, main, detail);
            None
        }
        None => calendar(&mut ink, main, input, rows, hero),
    };
    if let Some(text) = &input.flash {
        let r = R::new(main.x + PAD, main.bottom() - FOOTER_H - 40.0, main.w - 2.0 * PAD, 32.0);
        ink.draw(r, Draw::Rect { fill: Fill::Tip, radius: 6.0 });
        ink.draw(r, Draw::Outline { ink: Ink::Text3, radius: 6.0 });
        ink.text(r, text.clone(), Font::Caption, Ink::Text, Align::Center);
    }
    if let Some(focus) = input.focus {
        // The keyboard's target, ringed (twice over: a 2 DIP ring).
        if let Some(r) = ink.hits.iter().rev().find(|(_, h)| *h == focus).map(|(r, _)| *r) {
            for inset in [0.0, 1.0] {
                ink.draw(R::new(r.x + inset, r.y + inset, r.w - 2.0 * inset, r.h - 2.0 * inset), Draw::Outline { ink: Ink::Accent, radius: 5.0 });
            }
        }
    }
    if let Some(at) = input.show_tip {
        tooltip(&mut ink, input, at, top, main);
    }
    let w = top.map_or(main.right(), |t| t.right().max(main.right()));
    let h = top.map_or(main.bottom(), |t| t.bottom().max(main.bottom()));
    Scene { top, main, w, h, items: ink.items, hits: ink.hits, center_view, list_view, toasts: ink.places }
}

/// One above the other: the calendar takes what it needs first, the notification centre what is
/// left (folded to its summary below [`CENTER_MIN`]), unless it was given the whole panel.
fn stacked(input: &Input) -> Plan {
    // How many list rows fit, and how tall the notification centre may be.
    let mut rows = input.rows.len().clamp(1, 5);
    let summary = input.month_open || input.detail.is_some();
    let mut center_h = match &input.groups {
        Some(groups) if summary => SUMMARY_H,
        Some(groups) => center_height(groups),
        None => 0.0,
    };
    let gap = if center_h > 0.0 { GAP } else { 0.0 };
    let fits = |h: f32| h + gap + center_h.min(CENTER_MIN) <= input.room;
    // On a short screen the month goes without the next event (it is in the month's marks).
    let hero = input.hero.is_some() && (!input.month_open || fits(main_height(input, 1, true)));
    while rows > 1 && !fits(main_height(input, rows, hero)) {
        rows -= 1;
    }
    // An event's details take what there is, never more than their height.
    let mut main_h = main_height(input, rows, hero);
    if input.detail.is_some() {
        main_h = main_h.min(input.room - gap - center_h.min(CENTER_MIN)).max(240.0);
    }
    let mut folded = summary;
    let full = input.center_full && input.groups.is_some();
    let main_h = if full { SUMMARY_H } else { main_h };
    if let (true, Some(groups)) = (full, &input.groups) {
        center_h = center_height(groups).min(input.room - SUMMARY_H - GAP).max(CENTER_MIN);
        folded = false;
    } else if center_h > 0.0 && !summary {
        let spare = input.room - main_h - GAP;
        if spare < CENTER_MIN {
            folded = true;
            center_h = SUMMARY_H;
        } else {
            center_h = center_h.min(spare);
        }
    }

    let top = (center_h > 0.0).then(|| R::new(0.0, 0.0, W, center_h));
    let main_y = if top.is_some() { center_h + GAP } else { 0.0 };
    Plan { top, main: R::new(0.0, main_y, W, main_h), folded, full, rows, hero }
}

/// Wider targets carry a label, so they get no tooltip.
const TIP_TARGET_MAX: f32 = 56.0;

/// What an icon-only target does, in words (`None`: it says it already).
fn tip_text(hit: Hit, input: &Input) -> Option<&'static str> {
    Some(match hit {
        Hit::Quiet if input.quiet => "Turn off Do not disturb",
        Hit::Quiet => "Do not disturb",
        Hit::Expand if input.center_full => "Back to the calendar",
        Hit::Expand => "Show all notifications",
        Hit::Month if input.month_open => "Show the week",
        Hit::Month => "Show the month",
        Hit::Prev => "Previous month",
        Hit::Next => "Next month",
        Hit::Settings => "Settings",
        Hit::Back => "Back",
        Hit::Dismiss(_) => "Dismiss",
        Hit::DismissGroup(_) => "Clear all from this app",
        Hit::Menu(_) => "More options",
        Hit::Send(_) => "Send",
        Hit::Clocks => "Change the clocks",
        Hit::Today => "Go to today",
        Hit::Calendar => "Back to the calendar",
        _ => return None,
    })
}

/// The hovered target's tooltip, under it (above it near its card's foot), inside its card.
fn tooltip(ink: &mut Ink2, input: &Input, (px, py): (f32, f32), top: Option<R>, main: R) {
    let Some(hit) = input.hover else { return };
    let Some(text) = tip_text(hit, input) else { return };
    // The target under the pointer: the same one can sit in two places (the header's moon and the
    // footer's Do not disturb button).
    let Some(r) = ink.hits.iter().rev().find(|(r, h)| *h == hit && r.contains(px, py)).map(|(r, _)| *r) else { return };
    if r.w > TIP_TARGET_MAX && hit != Hit::Clocks {
        // A labelled button says it already.
        return;
    }
    let card = top.filter(|t| t.contains(r.x + 1.0, r.y + 1.0)).unwrap_or(main);
    let w = (text.chars().count() as f32 * 6.5 + 20.0).min(card.w - 16.0);
    let h = 26.0;
    let x = (r.x + r.w / 2.0 - w / 2.0).clamp(card.x + 8.0, card.right() - 8.0 - w);
    let below = r.bottom() + 6.0;
    let y = if below + h <= card.bottom() - 4.0 { below } else { r.y - 6.0 - h };
    let tip = R::new(x, y.max(card.y + 4.0), w, h);
    ink.draw(tip, Draw::Rect { fill: Fill::Tip, radius: 4.0 });
    ink.draw(tip, Draw::Outline { ink: Ink::Text3, radius: 4.0 });
    ink.text(tip, text, Font::Caption, Ink::Text, Align::Center);
}

/// The calendar folded to one line, under the notification centre given the whole panel.
fn calendar_summary(ink: &mut Ink2, card: R, input: &Input) {
    ink.plate(R::new(card.x + 4.0, card.y + 4.0, card.w - 8.0, card.h - 8.0), Hit::Calendar, 6.0);
    let y = card.y + (card.h - 20.0) / 2.0;
    ink.glyph(R::new(card.x + PAD, y, 16.0, 20.0), '\u{E787}', Font::IconSmall, Ink::Text2);
    let line = format!("{} {}", input.weekday, input.date);
    ink.text(R::new(card.x + PAD + 28.0, y, card.w - 2.0 * PAD - 60.0, 20.0), line, Font::Strong, Ink::Text, Align::Left);
    ink.glyph(R::new(card.right() - PAD - 16.0, y, 16.0, 20.0), '\u{E73F}', Font::IconSmall, Ink::Text2);
}

/// The notification centre folded to one line: its count and the apps' logos.
fn center_summary(ink: &mut Ink2, card: R, input: &Input, groups: &[Group]) {
    ink.plate(R::new(card.x + 4.0, card.y + 4.0, card.w - 8.0, card.h - 8.0), Hit::Center, 6.0);
    let y = card.y + (card.h - 20.0) / 2.0;
    ink.text(R::new(card.x + PAD, y, 110.0, 20.0), "Notifications", Font::Strong, Ink::Text, Align::Left);
    let chip = count_chip(ink, card.x + PAD + 104.0, y + 1.0, input.unread);
    let mut x = chip.right() + 10.0;
    for group in groups.iter().take(5) {
        if let Some((key, bytes)) = &group.logo {
            ink.draw(R::new(x, y + 2.0, 16.0, 16.0), Draw::Logo { key: key.clone(), bytes: bytes.clone() });
            x += 20.0;
        }
    }
    ink.glyph(R::new(card.right() - PAD - 16.0, y, 16.0, 20.0), '\u{E70E}', Font::IconSmall, Ink::Text2);
}

/// One notification in `r`: the sender's photo, its text, its picture, then its buttons (or its
/// reply box, while open) and its menu.
fn notification(ink: &mut Ink2, r: R, toast: &Toast) {
    let i = toast.index;
    ink.places.push((i, r));
    let lit = ink.hover.and_then(toast_of) == Some(i) || toast.draft.is_some();
    let fill = if ink.pressed == Some(Hit::Toast(i)) && ink.lit(Hit::Toast(i)) { Fill::Pressed } else if lit { Fill::Hover } else { Fill::Raised };
    ink.draw(r, Draw::Rect { fill, radius: 6.0 });
    ink.hit(r, Hit::Toast(i));
    let mut tx = r.x + 12.0;
    if let Some((key, bytes)) = &toast.avatar {
        ink.draw(R::new(tx, r.y + 12.0, 36.0, 36.0), Draw::Avatar { key: key.clone(), bytes: bytes.clone() });
        tx += 48.0;
    }
    ink.text(R::new(tx, r.y + 8.0, r.right() - tx - 40.0, 20.0), toast.title.clone(), Font::Strong, Ink::Text, Align::Left);
    ink.text(R::new(tx, r.y + 30.0, r.right() - tx - 12.0, 38.0), toast.body.clone(), Font::Wrap, Ink::Text2, Align::Left);
    if lit {
        let x_r = R::new(r.right() - 32.0, r.y + 6.0, 26.0, 26.0);
        ink.plate(x_r, Hit::Dismiss(i), 4.0);
        ink.glyph(x_r, '\u{E711}', Font::IconSmall, Ink::Text2);
    }
    let mut y = r.y + TOAST_H - 4.0;
    if let Some((key, bytes)) = &toast.picture {
        ink.draw(R::new(r.x + 12.0, y, r.w - 24.0, PICTURE_H), Draw::Picture { key: key.clone(), bytes: bytes.clone() });
        y += PICTURE_H + 8.0;
    }
    if toast.has_actions() {
        let row = R::new(r.x + 12.0, y, r.w - 24.0, ACTIONS_H);
        match &toast.draft {
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
                ink.draw(send, Draw::Rect { fill: Fill::Accent, radius: 4.0 });
                ink.glyph(send, '\u{E724}', Font::IconSmall, Ink::OnAccent);
                ink.hit(send, Hit::Send(i));
            }
            None => {
                let mut bx = row.x;
                let end = if toast.menu.is_empty() { row.right() } else { row.right() - ACTIONS_H - 6.0 };
                let mut pills: Vec<(String, Hit)> = toast.buttons.iter().enumerate().map(|(n, label)| (label.clone(), Hit::Action(i, n))).collect();
                if toast.reply.is_some() {
                    pills.insert(0, ("Reply".to_string(), Hit::Reply(i)));
                }
                for (label, hit) in pills {
                    // What room is left, before sizing: a clamp with less than its minimum panics.
                    let room = end - bx;
                    if room < 40.0 {
                        break;
                    }
                    let pw = (label.chars().count() as f32 * 7.0 + 28.0).max(64.0).min(room);
                    ink.pill(R::new(bx, row.y + 2.0, pw, row.h - 4.0), &label, hit, false);
                    bx += pw + 6.0;
                }
                if !toast.menu.is_empty() {
                    let more = R::new(row.right() - ACTIONS_H, row.y, ACTIONS_H, ACTIONS_H);
                    ink.plate(more, Hit::Menu(i), 4.0);
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

/// The unread count in an accent chip at `(x, y)`, as wide as its number (99+ at most).
fn count_chip(ink: &mut Ink2, x: f32, y: f32, unread: usize) -> R {
    let label = if unread > 99 { "99+".to_string() } else { unread.to_string() };
    let chip = R::new(x, y, (label.chars().count() as f32 * 7.0 + 10.0).max(24.0), 18.0);
    ink.draw(chip, Draw::Rect { fill: Fill::Accent, radius: 9.0 });
    ink.text(chip, label, Font::CaptionStrong, Ink::OnAccent, Align::Center);
    chip
}

/// The notification centre open: its header, then each app's group. Returns its scrolling area.
fn center(ink: &mut Ink2, card: R, input: &Input, groups: &[Group]) -> (R, f32) {
    let x = card.x + 8.0;
    let w = card.w - 16.0;
    let head = R::new(x, card.y + 10.0, w, 32.0);
    ink.text(R::new(head.x + 8.0, head.y, 110.0, head.h), "Notifications", Font::Strong, Ink::Text, Align::Left);
    count_chip(ink, head.x + 112.0, head.y + 7.0, input.unread);
    // A text link, tinted under the pointer like the calendar's.
    let clear = R::new(head.right() - 80.0, head.y + 2.0, 80.0, 28.0);
    let lit = ink.lit(Hit::ClearAll);
    ink.text(clear, "Clear all", Font::Caption, if lit { Ink::Accent } else { Ink::Text }, Align::Center);
    ink.hit(clear, Hit::ClearAll);
    ink.icon_button(R::new(clear.x - 40.0, head.y, 32.0, 32.0), '\u{E708}', Hit::Quiet, input.quiet);
    // The whole panel for the notifications, or back (side by side, they have it already).
    if input.columns.is_none() {
        let expand = if input.center_full { '\u{E73F}' } else { '\u{E740}' };
        ink.icon_button(R::new(clear.x - 76.0, head.y, 32.0, 32.0), expand, Hit::Expand, false);
    }

    let area = R::new(card.x, head.bottom() + 6.0, card.w, card.bottom() - 8.0 - (head.bottom() + 6.0));
    let content = center_height(groups) - (12.0 + 32.0 + 6.0) - 8.0;
    let max = (content - area.h).max(0.0);
    let scroll = input.center_scroll.clamp(0.0, max);
    ink.draw(area, Draw::Clip);
    let mut y = area.y - scroll;
    for (g, group) in groups.iter().enumerate() {
        let line = R::new(x, y, w, if group.open { GROUP_H } else { FOLDED_H });
        // No plate: one would run into the notification under it. The name brightens instead.
        ink.hit(line, Hit::Group(g));
        let lit = ink.lit(Hit::Group(g)) || ink.lit(Hit::DismissGroup(g));
        let mut tx = line.x + 8.0;
        if let Some((key, bytes)) = &group.logo {
            ink.draw(R::new(tx, line.y + (line.h - 16.0) / 2.0, 16.0, 16.0), Draw::Logo { key: key.clone(), bytes: bytes.clone() });
            tx += 24.0;
        }
        let name = if group.open { group.name.clone() } else { format!("{} · {}", group.name, group.toasts.len()) };
        ink.text(R::new(tx, line.y, line.right() - tx - 80.0, line.h), name, Font::CaptionStrong, if lit { Ink::Text } else { Ink::Text2 }, Align::Left);
        if lit {
            // Under the pointer: clear all of this app's, in place of its time or chevron.
            let x_r = R::new(line.right() - 30.0, line.y + (line.h - 24.0) / 2.0, 24.0, 24.0);
            ink.plate(x_r, Hit::DismissGroup(g), 4.0);
            ink.glyph(x_r, '\u{E711}', Font::IconSmall, Ink::Text2);
        } else if input.columns.is_some() {
            // Side by side nothing folds: its time only.
            ink.text(R::new(line.right() - 80.0, line.y, 72.0, line.h), group.when.clone(), Font::Caption, Ink::Text3, Align::Right);
        } else if group.open {
            // Its time, and the chevron saying the line folds it.
            ink.text(R::new(line.right() - 100.0, line.y, 72.0, line.h), group.when.clone(), Font::Caption, Ink::Text3, Align::Right);
            ink.glyph(R::new(line.right() - 24.0, line.y, 16.0, line.h), '\u{E70E}', Font::IconSmall, Ink::Text3);
        } else {
            ink.glyph(R::new(line.right() - 24.0, line.y, 16.0, line.h), '\u{E70D}', Font::IconSmall, Ink::Text2);
        }
        y += line.h;
        if !group.open {
            continue;
        }
        let shown = if group.all { group.toasts.len() } else { 1 };
        for toast in group.toasts.iter().take(shown) {
            let r = R::new(x, y, w, toast.height());
            notification(ink, r, toast);
            y += r.h + 4.0;
        }
        if !group.all && group.toasts.len() > 1 {
            ink.draw(R::new(x + 8.0, y - 4.0, w - 16.0, 6.0), Draw::Rect { fill: Fill::Raised, radius: 3.0 });
            y += 6.0;
            let more = R::new(x + 4.0, y, w - 8.0, 22.0);
            ink.plate(more, Hit::More(g), 4.0);
            let n = group.toasts.len() - 1;
            ink.text(R::new(more.x + 8.0, more.y, more.w - 16.0, more.h), format!("{n} more from {}", group.name), Font::Caption, Ink::Accent, Align::Left);
            y += 22.0;
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

    // The date, the world clocks, and the week/month switch.
    // Hovers here tint the text (a link's), not a plate: plates around short text read as boxes.
    let today = R::new(x, y, 200.0, 44.0);
    ink.hit(today, Hit::Today);
    let lit = ink.lit(Hit::Today);
    ink.text(R::new(x, y, 200.0, 16.0), input.weekday.clone(), Font::Caption, if lit { Ink::Accent } else { Ink::Text2 }, Align::Left);
    ink.text(R::new(x, y + 16.0, 220.0, 28.0), input.date.clone(), Font::Big, if lit { Ink::Accent } else { Ink::Text }, Align::Left);
    // Expand to the month, reduce to the week: the same pair as the notification centre's. Side by
    // side the month always shows, so there is no switch.
    let toggle = R::new(card.right() - PAD - 32.0, y + 10.0, 32.0, 32.0);
    let edge = if input.columns.is_some() {
        card.right() - PAD + 12.0
    } else {
        ink.icon_button(toggle, if input.month_open { '\u{E73F}' } else { '\u{E740}' }, Hit::Month, false);
        toggle.x
    };
    if !input.clocks.is_empty() {
        // As wide as its text (about 6.5 DIPs a character at 12), right against the switch.
        let tw = (input.clocks.chars().count() as f32 * 6.5 + 4.0).min(edge - x - 150.0).max(40.0);
        let r = R::new(edge - 12.0 - tw, y + 18.0, tw, 20.0);
        ink.hit(r, Hit::Clocks);
        let tint = if ink.lit(Hit::Clocks) { Ink::Accent } else { Ink::Text2 };
        ink.text(r, input.clocks.clone(), Font::Caption, tint, Align::Right);
    }
    y += 44.0 + 14.0;

    // This week, or the month.
    if input.month_open {
        ink.text(R::new(x, y, w - 72.0, 28.0), input.month_title.clone(), Font::Strong, Ink::Text, Align::Left);
        let next = R::new(card.right() - PAD - 28.0, y, 28.0, 28.0);
        let prev = R::new(next.x - 32.0, y, 28.0, 28.0);
        ink.plate(prev, Hit::Prev, 4.0);
        ink.glyph(prev, '\u{E76B}', Font::IconSmall, Ink::Text2);
        ink.plate(next, Hit::Next, 4.0);
        ink.glyph(next, '\u{E76C}', Font::IconSmall, Ink::Text2);
        y += 32.0;
    }
    let col = w / 7.0;
    for (i, head) in input.heads.iter().enumerate() {
        ink.text(R::new(x + i as f32 * col, y, col, 18.0), head.clone(), Font::Tiny, Ink::Text3, Align::Center);
    }
    y += if input.month_open { 22.0 } else { 18.0 };
    let cell_h = if input.month_open { 40.0 } else { 44.0 };
    for (i, cell) in input.days.iter().enumerate() {
        let r = R::new(x + (i % 7) as f32 * col, y + (i / 7) as f32 * cell_h, col, cell_h);
        let circle = R::new(r.x + (r.w - 30.0) / 2.0, r.y + 2.0, 30.0, 30.0);
        if cell.today {
            ink.draw(circle, Draw::Rect { fill: Fill::Accent, radius: 15.0 });
        } else if ink.lit(Hit::Day(cell.day)) {
            ink.draw(circle, Draw::Rect { fill: Fill::Hover, radius: 15.0 });
        }
        if cell.selected && !cell.today {
            ink.draw(circle, Draw::Outline { ink: Ink::Accent, radius: 15.0 });
        }
        let color = if cell.today { Ink::OnAccent } else if cell.dim { Ink::Text3 } else { Ink::Text };
        ink.text(circle, cell.label.clone(), Font::Body, color, Align::Center);
        if cell.busy > 0 {
            let bw = if cell.busy > 1 { 10.0 } else { 4.0 };
            let mark = R::new(r.x + (r.w - bw) / 2.0, circle.bottom() + 3.0, bw, 4.0);
            ink.draw(mark, Draw::Rect { fill: if cell.today { Fill::Accent } else { Fill::Mark }, radius: 2.0 });
        }
        ink.hit(r, Hit::Day(cell.day));
    }
    y += (input.days.len().div_ceil(7)) as f32 * cell_h + 4.0;

    // The next event.
    if let Some(hero) = input.hero.as_ref().filter(|_| hero) {
        y += 10.0;
        let r = R::new(x, y, w, 76.0);
        let fill = ink.surface(Hit::Hero, Fill::Raised);
        ink.draw(r, Draw::Rect { fill, radius: 6.0 });
        ink.hit(r, Hit::Hero);
        ink.draw(R::new(r.x + 12.0, r.y + 12.0, 3.0, r.h - 24.0), Draw::Rect { fill: Fill::Rgb(hero.color), radius: 1.5 });
        let tw = r.w - 40.0 - if hero.joinable { 64.0 } else { 0.0 };
        ink.text(R::new(r.x + 26.0, r.y + 10.0, tw, 16.0), hero.caption.clone(), Font::TinyStrong, Ink::Accent, Align::Left);
        ink.text(R::new(r.x + 26.0, r.y + 27.0, tw, 20.0), hero.title.clone(), Font::Strong, Ink::Text, Align::Left);
        ink.text(R::new(r.x + 26.0, r.y + 48.0, tw, 18.0), hero.when.clone(), Font::Caption, Ink::Text2, Align::Left);
        if hero.joinable {
            ink.join(R::new(r.right() - 64.0, r.y + 25.0, 52.0, 26.0), Hit::HeroJoin);
        }
        y += 76.0;
    }

    // The rest of the week, or the picked day.
    y += 14.0;
    ink.text(R::new(x, y, w, 20.0), input.list_title.clone(), Font::CaptionStrong, Ink::Text2, Align::Left);
    if let Notice::Problem(problem) = &input.notice {
        // A source failed: said beside the title, a link to where it is mended.
        let r = R::new(x + w - 190.0, y, 190.0, 20.0);
        ink.hit(r, Hit::Settings);
        let tint = if ink.lit(Hit::Settings) { Ink::Accent } else { Ink::Text3 };
        ink.text(r, problem.clone(), Font::Caption, tint, Align::Right);
    }
    y += 24.0;
    let area = R::new(card.x, y, card.w, rows.max(1) as f32 * ROW_H);
    let max = (input.rows.len() as f32 * ROW_H - area.h).max(0.0);
    let scroll = input.list_scroll.clamp(0.0, max);
    if input.rows.is_empty() {
        let line = R::new(x, y, w, ROW_H);
        match &input.notice {
            Notice::NoSources => {
                ink.hit(line, Hit::Settings);
                let tint = if ink.lit(Hit::Settings) { Ink::Accent } else { Ink::Text2 };
                ink.text(line, "Connect a calendar in Settings", Font::Body, tint, Align::Left);
            }
            Notice::Loading => ink.text(line, "Loading…", Font::Body, Ink::Text3, Align::Left),
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
        let right_w = if row.joinable { 60.0 } else if row.right.is_empty() { 0.0 } else { 56.0 };
        let tx = r.x + left_w + 26.0;
        ink.text(R::new(tx, r.y, r.right() - tx - 6.0 - right_w, r.h), row.title.clone(), Font::Body, Ink::Text, Align::Left);
        if row.joinable {
            ink.join(R::new(r.right() - 58.0, r.y + 4.0, 52.0, 24.0), Hit::Join(i));
        } else if !row.right.is_empty() {
            ink.text(R::new(r.right() - 62.0, r.y, 56.0, r.h), row.right.clone(), Font::Caption, Ink::Text3, Align::Right);
        }
    }
    ink.draw(area, Draw::Unclip);
    scrollbar(ink, area, input.rows.len() as f32 * ROW_H, scroll);

    // Focus, Do not disturb, Settings.
    let fy = card.bottom() - FOOTER_H;
    ink.draw(R::new(card.x, fy, card.w, 1.0), Draw::Rect { fill: Fill::Rule, radius: 0.0 });
    let by = fy + (FOOTER_H - 32.0) / 2.0;
    let focus_w = if input.focus_running { 150.0 } else { 112.0 };
    ink.pill(R::new(x, by, focus_w, 32.0), &input.focus_label, Hit::Focus, input.focus_running);
    // Do not disturb once: the notification centre's moon when it is shown, else here.
    if input.groups.is_none() {
        ink.pill(R::new(x + focus_w + 8.0, by, 118.0, 32.0), "Do not disturb", Hit::Quiet, input.quiet);
    }
    let gear = R::new(card.right() - PAD - 32.0, by, 32.0, 32.0);
    ink.plate(gear, Hit::Settings, 4.0);
    ink.glyph(gear, '\u{E713}', Font::Icon, Ink::Text2);
    Some((area, max))
}

/// One event's details, in place of the calendar.
fn detail_card(ink: &mut Ink2, card: R, d: &Detail) {
    let x = card.x + PAD;
    let w = card.w - 2.0 * PAD;
    let mut y = card.y + 12.0;
    let back = R::new(x - 8.0, y, 32.0, 32.0);
    ink.plate(back, Hit::Back, 4.0);
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
        let dir = R::new(x + 26.0, y - 8.0, 80.0, 20.0);
        ink.plate(dir, Hit::Directions, 4.0);
        ink.text(R::new(dir.x + 4.0, dir.y, dir.w, dir.h), "Directions", Font::Caption, Ink::Accent, Align::Left);
        y += 16.0;
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
        let r = R::new(bx, buttons_y, 96.0, 34.0);
        ink.draw(r, Draw::Rect { fill: Fill::Accent, radius: 4.0 });
        ink.text(r, "Join", Font::CaptionStrong, Ink::OnAccent, Align::Center);
        ink.hit(r, Hit::DetailJoin);
        bx += 104.0;
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
            notice: Notice::None,
            focus: None,
            pressed: None,
            flash: None,
            room: 760.0,
            columns: None,
            groups: None,
            unread: 0,
            quiet: false,
            center_scroll: 0.0,
            weekday: "Sunday".into(),
            date: "15 November".into(),
            clocks: String::new(),
            month_open: false,
            month_title: "November 2026".into(),
            heads: std::array::from_fn(|i| format!("d{i}")),
            days: (0..7).map(|i| DayCell { day: 100 + i, label: i.to_string(), today: i == 6, selected: i == 6, dim: false, busy: 0 }).collect(),
            hero: None,
            list_title: "Later this week".into(),
            rows: Vec::new(),
            empty: "Nothing else this week".into(),
            list_scroll: 0.0,
            focus_label: "Focus 25 min".into(),
            focus_running: false,
            detail: None,
            hover: None,
            center_full: false,
            show_tip: None,
        }
    }

    fn toast(i: usize) -> Toast {
        Toast { index: i, title: format!("t{i}"), body: String::new(), avatar: None, picture: None, buttons: Vec::new(), reply: None, menu: Vec::new(), draft: None, menu_open: false }
    }

    fn group(name: &str, n: usize, open: bool) -> Group {
        Group {
            name: name.into(),
            logo: None,
            toasts: (0..n).map(toast).collect(),
            when: "2 min".into(),
            open,
            all: false,
        }
    }

    fn center_of(r: R) -> (f32, f32) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    #[test]
    fn without_notifications_there_is_one_card() {
        let s = build(&input());
        assert!(s.top.is_none());
        assert_eq!(s.main.y, 0.0);
        let day = s.hits.iter().find(|(_, h)| *h == Hit::Day(103)).unwrap().0;
        let (x, y) = center_of(day);
        assert_eq!(s.hit(x, y), Some(Hit::Day(103)));
    }

    #[test]
    fn notifications_sit_above_with_a_gap_and_fold_when_the_month_opens() {
        let mut i = input();
        i.groups = Some(vec![group("WhatsApp", 3, true), group("Edge", 2, false)]);
        i.unread = 5;
        let s = build(&i);
        let top = s.top.unwrap();
        assert_eq!(s.main.y, top.h + GAP);
        assert!(s.hits.iter().any(|(_, h)| *h == Hit::More(0)));
        assert!(s.hits.iter().any(|(_, h)| *h == Hit::Group(1)));
        let (x, y) = center_of(top);
        assert!(s.on_card(x, y) && !s.on_card(x, top.bottom() + GAP / 2.0));
        i.month_open = true;
        let s = build(&i);
        assert_eq!(s.top.unwrap().h, SUMMARY_H);
        let (x, y) = center_of(s.top.unwrap());
        assert_eq!(s.hit(x, y), Some(Hit::Center));
    }

    #[test]
    fn notifications_scrolled_out_of_sight_take_no_clicks() {
        let mut i = input();
        let mut whatsapp = group("WhatsApp", 6, true);
        whatsapp.all = true;
        i.groups = Some(vec![whatsapp]);
        i.center_scroll = 10_000.0;
        let s = build(&i);
        let top = s.top.unwrap();
        // The header's buttons stay theirs, whatever scrolled under them.
        let clear = s.hits.iter().find(|(_, h)| *h == Hit::ClearAll).unwrap().0;
        assert_eq!(s.hit(clear.x + 2.0, clear.y + 2.0), Some(Hit::ClearAll));
        // No notification target reaches past its card.
        assert!(s.hits.iter().filter(|(_, h)| toast_of(*h).is_some()).all(|(r, _)| r.y >= top.y && r.bottom() <= top.bottom()));
        // Nor is any drawn there: out of sight is out of the frame's work.
        assert!(s.items.iter().filter(|(_, d)| !matches!(d, Draw::Clip | Draw::Unclip)).all(|(r, _)| overlap(*r, top).is_some() || overlap(*r, s.main).is_some()));
        // And the list says it scrolls.
        assert!(s.items.iter().any(|(r, d)| matches!(d, Draw::Rect { fill: Fill::Mark, .. }) && r.w == 3.0));
    }

    #[test]
    fn the_month_on_a_short_screen_goes_without_the_next_event_and_fits() {
        let mut i = input();
        i.month_open = true;
        i.days = (0..42).map(|n| DayCell { day: 100 + n, label: n.to_string(), today: n == 6, selected: n == 6, dim: false, busy: 0 }).collect();
        i.hero = Some(Hero { caption: "NEXT".into(), title: "Stand-up".into(), when: "9:00".into(), color: [1, 2, 3], joinable: false });
        i.room = main_height(&i, 1, false) + 4.0;
        let s = build(&i);
        assert!(s.h <= i.room + 0.5);
        assert!(!s.hits.iter().any(|(_, h)| *h == Hit::Hero));
    }

    #[test]
    fn do_not_disturb_is_offered_once() {
        let mut i = input();
        assert_eq!(build(&i).hits.iter().filter(|(_, h)| *h == Hit::Quiet).count(), 1, "the footer's, with no notifications");
        i.groups = Some(vec![group("WhatsApp", 1, true)]);
        assert_eq!(build(&i).hits.iter().filter(|(_, h)| *h == Hit::Quiet).count(), 1, "the notification centre's moon");
    }

    #[test]
    fn a_short_screen_folds_the_notifications_rather_than_the_calendar() {
        let mut i = input();
        i.groups = Some(vec![group("WhatsApp", 3, true)]);
        i.room = main_height(&i, 1, i.hero.is_some()) + GAP + 60.0;
        let s = build(&i);
        assert_eq!(s.top.unwrap().h, SUMMARY_H);
        assert!(s.h <= i.room + 0.5);
    }

    #[test]
    fn expanded_notifications_take_the_panel_and_the_calendar_folds() {
        let mut i = input();
        // As the view gives it: every notification shown.
        i.groups = Some(vec![Group { all: true, ..group("WhatsApp", 30, true) }]);
        i.room = 900.0;
        i.center_full = true;
        let s = build(&i);
        assert_eq!(s.main.h, SUMMARY_H);
        assert!((s.h - i.room).abs() < 0.5, "the cards fill the room: {}", s.h);
        assert_eq!(s.hit(s.main.x + 20.0, s.main.y + 20.0), Some(Hit::Calendar));
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
        let mut i = input();
        let mut many = toast(0);
        many.buttons = (0..5).map(|n| format!("A rather long button label {n}")).collect();
        many.reply = Some("Reply".into());
        many.menu = vec!["Turn off".into()];
        i.groups = Some(vec![Group { toasts: vec![many], ..group("Teams", 1, true) }]);
        let s = build(&i);
        let card = s.top.unwrap();
        assert!(s.hits.iter().any(|(_, h)| *h == Hit::Reply(0)));
        assert!(s.hits.iter().filter(|(_, h)| matches!(h, Hit::Action(..))).all(|(r, _)| r.right() <= card.right()));
    }

    const BY_THE_CLOCK: Columns = Columns { calendar_left: false, from_top: false };

    #[test]
    fn side_by_side_neither_card_squeezes_the_other() {
        let mut i = input();
        i.columns = Some(BY_THE_CLOCK);
        i.groups = Some(vec![Group { all: true, ..group("WhatsApp", 12, true) }, group("Edge", 2, true)]);
        i.rows = (0..10).map(|n| Row { left: "Mon".into(), title: format!("e{n}"), color: [0; 3], right: String::new(), joinable: false }).collect();
        i.month_open = true;
        i.days = (0..42).map(|n| DayCell { day: 100 + n, label: n.to_string(), today: n == 6, selected: n == 6, dim: false, busy: 0 }).collect();
        let s = build(&i);
        let (top, main) = (s.top.unwrap(), s.main);
        // Notifications left, the calendar by the clock; both on the taskbar's edge.
        assert_eq!((top.x, main.x), (0.0, W + GAP));
        assert_eq!((s.w, top.bottom(), main.bottom()), (2.0 * W + GAP, s.h, s.h));
        // The month open, the notifications still listed in full, up to the room.
        assert!((top.h - i.room).abs() < 0.5, "notifications take the height: {}", top.h);
        assert!(s.hits.iter().any(|(_, h)| *h == Hit::Group(0)) && !s.hits.iter().any(|(_, h)| *h == Hit::Center));
        // More events than stacked, and no way to expand what is shown already.
        let (area, _) = s.list_view.unwrap();
        assert!(area.h > 5.0 * ROW_H);
        assert!(!s.hits.iter().any(|(_, h)| matches!(h, Hit::Expand | Hit::Month)));
        // Each card's targets are its own.
        let day = s.hits.iter().find(|(_, h)| *h == Hit::Day(110)).unwrap().0;
        assert!(main.contains(day.x, day.y));
        let (x, y) = center_of(day);
        assert_eq!(s.hit(x, y), Some(Hit::Day(110)));
    }

    #[test]
    fn side_by_side_a_left_or_top_taskbar_keeps_the_calendar_by_it() {
        let mut i = input();
        i.groups = Some(vec![group("WhatsApp", 1, true)]);
        i.columns = Some(Columns { calendar_left: true, from_top: true });
        let s = build(&i);
        assert_eq!((s.main.x, s.top.unwrap().x), (0.0, W + GAP));
        assert_eq!((s.main.y, s.top.unwrap().y), (0.0, 0.0));
    }

    #[test]
    fn side_by_side_without_notifications_is_the_calendar_alone() {
        let mut i = input();
        i.columns = Some(BY_THE_CLOCK);
        let s = build(&i);
        assert!(s.top.is_none());
        assert_eq!((s.main.x, s.w), (0.0, W));
    }
}

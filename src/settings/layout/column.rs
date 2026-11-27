//! What a page is laid out with: a [`Column`] that stacks cards, grouped rows and their controls top
//! to bottom, and a [`Gate`] that says whether a control can be changed.

use crate::config::Config;

use super::{
    plain, toggle_label_w, Button, Choice, Connector, Kind, Laid, Page, Picture, Target, Toggle, Tone, CARD_GAP, CARD_H, CARD_PAD, CARD_TEXT_X, CONTROL_H, R, ROW_H,
    SECTION_H, TITLE_H, TOGGLE_H, TOGGLE_W,
};

/// Lays a page out top to bottom.
pub(super) struct Column<'a> {
    pub(super) laid: Laid,
    pub(super) x: f32,
    pub(super) w: f32,
    pub(super) y: f32,
    pub(super) measure: &'a dyn Fn(&str) -> f32,
    /// The config the switches read.
    pub(super) config: &'a Config,
}

/// Whether a control can be changed: open, moot (dimmed), or waiting on another switch, which it
/// names in place of its second line and, on another page, opens when clicked.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Gate {
    Open,
    Off,
    Needs(&'static str, Option<Page>),
}

impl From<bool> for Gate {
    /// Dimmed or not, with nothing more to say.
    fn from(dim: bool) -> Self {
        if dim { Gate::Off } else { Gate::Open }
    }
}

impl Gate {
    /// `Needs(why, page)` when `dim`, else open.
    pub(super) fn needs(dim: bool, why: &'static str, page: Option<Page>) -> Self {
        if dim { Gate::Needs(why, page) } else { Gate::Open }
    }

    pub(super) fn dim(self) -> bool {
        self != Gate::Open
    }

    /// What a click does: `own` while open, the page that turns on what it needs, else nothing.
    pub(super) fn target(self, own: Target) -> Option<Target> {
        match self {
            Gate::Open => Some(own),
            Gate::Off | Gate::Needs(_, None) => None,
            Gate::Needs(_, Some(page)) => Some(Target::Nav(page)),
        }
    }

    /// The page that turns on what it needs, as a click's target, while it waits on one.
    pub(super) fn nav(self) -> Option<Target> {
        match self {
            Gate::Needs(_, Some(page)) => Some(Target::Nav(page)),
            _ => None,
        }
    }

    /// Its second line: what it needs, while it waits on it.
    pub(super) fn desc(self, usual: Option<(String, Tone)>) -> Option<(String, Tone)> {
        match self {
            Gate::Needs(why, _) => plain(why),
            _ => usual,
        }
    }
}

/// A button in a row or on a card: its label, whether it is the accent one, its glyph, and what it
/// does.
#[derive(Clone)]
pub(super) struct RowButton {
    label: String,
    accent: bool,
    glyph: Option<char>,
    button: Button,
}

impl RowButton {
    pub(super) fn new(label: impl Into<String>, button: Button) -> Self {
        RowButton { label: label.into(), accent: false, glyph: None, button }
    }

    pub(super) fn accent(self, accent: bool) -> Self {
        RowButton { accent, ..self }
    }

    pub(super) fn glyph(self, glyph: char) -> Self {
        RowButton { glyph: Some(glyph), ..self }
    }
}

/// What a grouped row holds on its right.
pub(super) enum Control {
    None,
    Button(RowButton),
    /// A switch, on as the config has it; the whole row flips it.
    Toggle(Toggle),
    Segments(Vec<String>, usize, Choice),
    /// Laid out left to right.
    Buttons(Vec<RowButton>),
}

/// A picture tile: what it draws, its name and line, whether it is the one chosen, what it picks.
pub(super) struct Tile {
    pub(super) picture: Picture,
    pub(super) title: &'static str,
    pub(super) desc: &'static str,
    pub(super) selected: bool,
    pub(super) target: Target,
}

/// Picture tiles for `choice`: one per `(option, name, line)`, picking the option's place in `all`
/// (the choice's order, [`super::choice`]), the one `current` selected.
pub(super) fn choice_tiles<T: Copy + PartialEq>(options: &[(T, &'static str, &'static str)], all: &[T], current: T, picture: fn(T) -> Picture, choice: Choice) -> Vec<Tile> {
    options
        .iter()
        .map(|&(option, title, desc)| {
            let i = all.iter().position(|o| *o == option).unwrap_or(0);
            Tile { picture: picture(option), title, desc, selected: option == current, target: Target::Choice(choice, i) }
        })
        .collect()
}

/// A switch on a half-width card of [`Column::toggle_grid`]: its title and second line, whether it
/// is on and can be changed, which it is, and the glyph before its title, when it has one.
pub(super) struct Switch {
    pub(super) title: &'static str,
    pub(super) desc: &'static str,
    pub(super) on: bool,
    pub(super) gate: Gate,
    pub(super) toggle: Toggle,
    pub(super) glyph: Option<char>,
}

impl Switch {
    /// On as the config has it.
    pub(super) fn new(c: &Config, title: &'static str, desc: &'static str, gate: impl Into<Gate>, toggle: Toggle) -> Self {
        Switch { title, desc, on: toggle.get(c), gate: gate.into(), toggle, glyph: None }
    }

    /// With `glyph` before its title (Segoe Fluent Icons), as a page's own cards have.
    pub(super) fn glyph(self, glyph: char) -> Self {
        Switch { glyph: Some(glyph), ..self }
    }
}

/// A connector's card in the grid: its glyph, name, state, switch, and which one it is.
pub(super) struct ConnectorCard {
    pub(super) glyph: char,
    pub(super) title: &'static str,
    pub(super) state: (String, Tone),
    pub(super) on: bool,
    pub(super) toggle: Toggle,
    pub(super) connector: Connector,
}

/// A grouped card being laid out: its item, where it starts, how many rows it has.
pub(super) struct Group {
    at: usize,
    top: f32,
    rows: usize,
}

/// A button's width for `label`, with room for its glyph.
pub(super) fn button_width(measure: &dyn Fn(&str) -> f32, label: &str, glyph: Option<char>) -> f32 {
    (measure(label) + 24.0 + if glyph.is_some() { 24.0 } else { 0.0 }).max(96.0)
}

/// The room a switch takes, its "On" or "Off" included.
pub(super) fn switch_room() -> f32 {
    TOGGLE_W + toggle_label_w() + 8.0
}

impl Column<'_> {
    pub(super) fn title(&mut self, text: &'static str) {
        self.laid.push(R::new(self.x, self.y, self.w, TITLE_H), Kind::Title(text));
        self.y += TITLE_H;
    }

    pub(super) fn section(&mut self, text: &'static str) {
        self.laid.push(R::new(self.x, self.y, self.w, SECTION_H), Kind::Section(text));
        self.y += SECTION_H;
    }

    /// A card, and the rectangle its control goes in (right-aligned, vertically centred).
    pub(super) fn card(&mut self, glyph: Option<char>, title: impl Into<String>, desc: Option<(String, Tone)>, dim: bool, target: Option<Target>) -> R {
        let r = R::new(self.x, self.y, self.w, CARD_H);
        self.laid.push(r, Kind::Card { glyph, title: title.into(), desc, dim, target });
        if let Some(target) = target {
            self.laid.hits.push((r, target));
        }
        self.y += CARD_H + CARD_GAP;
        r
    }

    /// A switch's card, on as the config has it.
    pub(super) fn toggle(&mut self, glyph: Option<char>, title: &str, desc: Option<(String, Tone)>, gate: impl Into<Gate>, toggle: Toggle) {
        let gate = gate.into();
        // Dimmed, it is moot: not a target, unless it opens the page that turns on what it needs.
        let card = self.card(glyph, title, gate.desc(desc), gate.dim(), gate.target(Target::Toggle(toggle)));
        self.toggle_in(card, card.right() - CARD_PAD, toggle.get(self.config), gate.dim(), toggle);
    }

    pub(super) fn button_card(&mut self, glyph: Option<char>, title: &str, desc: Option<(String, Tone)>, button: RowButton) {
        let card = self.card(glyph, title, desc, false, None);
        self.buttons_in(card, &[button], false);
    }

    /// A button at `r`: drawn, and a target unless dimmed.
    pub(super) fn push_button(&mut self, r: R, label: &str, accent: bool, glyph: Option<char>, dim: bool, target: Target) {
        self.laid.push(r, Kind::Button { label: label.to_string(), accent, glyph, dim, target });
        if !dim {
            self.laid.hits.push((r, target));
        }
    }

    /// A card that opens something: all of it is the button, with a trailing glyph.
    pub(super) fn link(&mut self, glyph: Option<char>, title: &str, desc: Option<(String, Tone)>, trailing: char, button: Button) {
        let card = self.card(glyph, title, desc, false, Some(Target::Button(button)));
        let r = R::new(card.right() - CARD_PAD - 20.0, card.y, 20.0, CARD_H);
        self.laid.push(r, Kind::Trailing(trailing));
    }

    /// A card that opens another page of Settings, with a chevron.
    pub(super) fn nav_link(&mut self, glyph: Option<char>, title: &str, desc: Option<(String, Tone)>, page: Page) {
        let card = self.card(glyph, title, desc, false, Some(Target::Nav(page)));
        let r = R::new(card.right() - CARD_PAD - 20.0, card.y, 20.0, CARD_H);
        self.laid.push(r, Kind::Trailing('\u{E76C}'));
    }

    pub(super) fn gap(&mut self, dy: f32) {
        self.y += dy;
    }

    /// Start a card that holds rows ([`Column::row`]); [`Column::end_group`] sizes it.
    pub(super) fn group(&mut self) -> Group {
        let at = self.laid.items.len();
        self.laid.push(R::new(self.x, self.y, self.w, 0.0), Kind::Card { glyph: None, title: String::new(), desc: None, dim: false, target: None });
        Group { at, top: self.y, rows: 0 }
    }

    /// The next row of `group`, under a hairline when it is not the first: where it goes, a
    /// [`ROW_H`] tall.
    pub(super) fn begin_row(&mut self, group: &mut Group) -> R {
        if group.rows > 0 {
            self.laid.push(R::new(self.x, self.y, self.w, 1.0), Kind::Divider);
        }
        group.rows += 1;
        R::new(self.x, self.y, self.w, ROW_H)
    }

    /// One row of a group: a step's badge or nothing, a title and second line, and its control.
    /// Dimmed, the whole row is moot, its buttons too; waiting on another page, a click on it opens
    /// that page.
    pub(super) fn row(&mut self, group: &mut Group, badge: Option<(u8, bool)>, title: &str, desc: Option<(String, Tone)>, gate: impl Into<Gate>, control: Control) {
        let gate = gate.into();
        let (dim, desc) = (gate.dim(), gate.desc(desc));
        let row = self.begin_row(group);
        if let Some(target) = gate.nav() {
            self.laid.hits.push((row, target));
        }
        if let Some((n, done)) = badge {
            self.laid.push(R::new(row.x + CARD_PAD, row.y, 24.0, ROW_H), Kind::Badge { n, done });
        }
        let used = match control {
            Control::None => 0.0,
            Control::Button(button) => self.buttons_in(row, &[button], dim),
            Control::Toggle(toggle) => {
                // The whole row flips it, as a switch's card does.
                if !dim {
                    self.laid.hits.push((row, Target::Toggle(toggle)));
                }
                self.toggle_in(row, row.right() - CARD_PAD, toggle.get(self.config), dim, toggle);
                switch_room()
            }
            Control::Segments(labels, selected, choice) => self.segments_in(row, &labels, selected, dim, choice),
            Control::Buttons(buttons) => self.buttons_in(row, &buttons, dim),
        };
        let end = row.right() - CARD_PAD - if used > 0.0 { used + 16.0 } else { 0.0 };
        let x = row.x + CARD_TEXT_X;
        self.laid.push(R::new(x, row.y, end - x, ROW_H), Kind::Label { title: title.to_string(), desc, dim });
        self.y += ROW_H;
    }

    /// Buttons right-aligned in `row`, in order, not targets while `dim`; returns the room they take.
    pub(super) fn buttons_in(&mut self, row: R, buttons: &[RowButton], dim: bool) -> f32 {
        let mut right = row.right() - CARD_PAD;
        for b in buttons.iter().rev() {
            let w = button_width(self.measure, &b.label, b.glyph);
            let r = R::new(right - w, row.y + (row.h - CONTROL_H) / 2.0, w, CONTROL_H);
            self.push_button(r, &b.label, b.accent, b.glyph, dim, Target::Button(b.button));
            right -= w + 8.0;
        }
        row.right() - CARD_PAD - right - 8.0
    }

    /// A row whose title is being typed: the field, then its buttons.
    pub(super) fn field_row(&mut self, group: &mut Group, text: &str, buttons: &[RowButton]) {
        let row = self.begin_row(group);
        let used = self.buttons_in(row, buttons, false);
        let x = row.x + CARD_TEXT_X;
        let end = row.right() - CARD_PAD - used - 16.0;
        self.laid.push(R::new(x, row.y + (ROW_H - CONTROL_H) / 2.0, (end - x).min(360.0), CONTROL_H), Kind::Field(text.to_string()));
        self.y += ROW_H;
    }

    /// A switch ending at `right`, centred in `card`; returns where it is drawn (its label, before
    /// it, takes [`switch_room`] in all).
    pub(super) fn toggle_in(&mut self, card: R, right: f32, on: bool, dim: bool, toggle: Toggle) -> R {
        let r = R::new(right - TOGGLE_W, card.y + (card.h - TOGGLE_H) / 2.0, TOGGLE_W, TOGGLE_H);
        self.laid.push(r, Kind::Toggle { on, dim, toggle });
        r
    }

    /// A segmented choice right-aligned in `card`; returns its width.
    pub(super) fn segments_in<S: AsRef<str>>(&mut self, card: R, labels: &[S], selected: usize, dim: bool, choice: Choice) -> f32 {
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

    /// A card holding a segmented choice; waiting on another page, a click on it opens that page.
    pub(super) fn segments<S: AsRef<str>>(&mut self, title: &str, desc: Option<(String, Tone)>, labels: &[S], selected: usize, gate: impl Into<Gate>, choice: Choice) {
        let gate = gate.into();
        let card = self.card(None, title, gate.desc(desc), gate.dim(), gate.nav());
        self.segments_in(card, labels, selected, gate.dim(), choice);
    }

    /// A row of picture tiles, after a `lead` naming what they choose when given (else full
    /// width); dimmed, they are moot and not targets (waiting on another switch, the lead says
    /// which, and they open its page when it is another).
    pub(super) fn tiles(&mut self, lead: Option<(&'static str, &'static str)>, tiles: &[Tile], gate: impl Into<Gate>, h: f32) {
        const LEAD_W: f32 = 180.0;
        const GAP: f32 = 10.0;
        let gate = gate.into();
        let dim = gate.dim();
        let mut x = self.x;
        if let Some((title, desc)) = lead {
            self.laid.push(R::new(x, self.y, LEAD_W - GAP, h), Kind::Label { title: title.to_string(), desc: gate.desc(plain(desc)), dim });
            x += LEAD_W;
        }
        let n = tiles.len().max(3) as f32;
        let w = (self.x + self.w - x - GAP * (n - 1.0)) / n;
        for tile in tiles {
            let r = R::new(x, self.y, w, h);
            self.laid.push(r, Kind::Tile { picture: tile.picture, title: tile.title, desc: tile.desc, selected: tile.selected, dim, target: tile.target });
            if let Some(target) = gate.target(tile.target) {
                self.laid.hits.push((r, target));
            }
            x += w + GAP;
        }
        self.y += h + GAP;
    }

    /// Switches in two columns of half-width cards.
    pub(super) fn toggle_grid(&mut self, switches: &[Switch]) {
        const GAP: f32 = 8.0;
        let w = (self.w - GAP) / 2.0;
        for (i, s) in switches.iter().enumerate() {
            let r = R::new(self.x + (i % 2) as f32 * (w + GAP), self.y, w, CARD_H);
            let target = s.gate.target(Target::Toggle(s.toggle));
            self.laid.push(r, Kind::Card { glyph: s.glyph, title: s.title.to_string(), desc: s.gate.desc(plain(s.desc)), dim: s.gate.dim(), target });
            if let Some(target) = target {
                self.laid.hits.push((r, target));
            }
            self.toggle_in(r, r.right() - CARD_PAD, s.on, s.gate.dim(), s.toggle);
            if i % 2 == 1 || i + 1 == switches.len() {
                self.y += CARD_H + GAP;
            }
        }
    }

    /// A row of a group holding a title, then choices as chips that wrap to as many lines as they
    /// need: every one a click away, however many there are.
    pub(super) fn pick_row(&mut self, group: &mut Group, title: &str, desc: Option<(String, Tone)>, picks: &[(String, bool, Target)], gate: impl Into<Gate>) {
        const CHIP_H: f32 = 32.0;
        const GAP: f32 = 8.0;
        let gate = gate.into();
        let (dim, desc) = (gate.dim(), gate.desc(desc));
        self.begin_row(group);
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
    pub(super) fn connector_grid(&mut self, cards: &[ConnectorCard], open: Option<Connector>) {
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
            let t = self.toggle_in(r, r.right() - CARD_PAD, card.on, false, card.toggle);
            // The switch, not the card, under the pointer: it flips without opening.
            self.laid.hits.push((R::new(t.x - 4.0, t.y - 6.0, t.w + 8.0, t.h + 12.0), Target::Toggle(card.toggle)));
        }
        self.y += cards.len().div_ceil(2) as f32 * (CARD_H + GAP) + 4.0;
    }

    pub(super) fn end_group(&mut self, group: Group) {
        self.laid.items[group.at].r.h = self.y - group.top;
        self.y += CARD_GAP;
    }
}

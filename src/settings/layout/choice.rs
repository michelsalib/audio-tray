//! The segmented and tiled choices: each one's options in their order on screen, and what picking
//! option `i` sets. The pages read which option is selected from the same arrays the window sets
//! from ([`Choice::apply`]), so an order is written once.

use crate::config::{Batteries, BatteryIconStyle, BellMiddleClick, CalendarLayout, Config, MiddleClick, MotionStyle, PillSize, TileStyle, WaveColour};

/// A segmented choice.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::settings) enum Choice {
    /// What the music player's taskbar button shows: [`TILE_STYLES`].
    Tile,
    /// What a middle click on a taskbar audio button does: [`MIDDLE_CLICKS`].
    MiddleClick,
    /// What a middle click on the bell does: [`BELL_MIDDLE_CLICKS`].
    BellMiddle,
    /// [`PILL_SIZES`].
    PillSize,
    /// Every tab, or only the players'.
    BrowserTabs,
    /// Automatic, then each display in [`crate::win::monitors`]' order.
    Display,
    /// The low-battery level: [`crate::config::Batteries::LOW_LEVELS`].
    LowBattery,
    /// [`crate::config::BatteryIconStyle::ALL`].
    BatteryIconStyle,
    /// The layout of assigned keyboard `i`: the index is into [`crate::settings::model::Model::layouts`].
    KeyboardLayout(usize),
    /// A bar's motion: [`crate::config::MotionStyle::ALL`].
    RingMotion,
    BarMotion,
    VolumeMotion,
    /// Accent, or the cover's colour: [`WAVE_COLOURS`].
    WaveColour,
    /// [`CALENDAR_LAYOUTS`].
    CalendarLayout,
    /// [`AGENDA_ROWS`].
    AgendaRows,
    /// The locale's, Monday or Sunday: [`WEEK_STARTS`].
    WeekStart,
    /// [`REMINDERS`].
    Reminder,
    /// [`FOCUS_LENGTHS`].
    FocusLength,
}

/// What the music player's taskbar button shows, in its tiles' order.
pub(super) const TILE_STYLES: [TileStyle; 3] = [TileStyle::Off, TileStyle::Cover, TileStyle::Full];
/// A taskbar audio button's middle clicks, in their tiles' order.
pub(super) const MIDDLE_CLICKS: [MiddleClick; 3] = [MiddleClick::Mute, MiddleClick::Switch, MiddleClick::SwitchMute];
/// The bell's middle clicks, in their tiles' order.
pub(super) const BELL_MIDDLE_CLICKS: [BellMiddleClick; 3] = [BellMiddleClick::DoNotDisturb, BellMiddleClick::ClearAll, BellMiddleClick::Nothing];
/// The pill's sizes at rest, in their tiles' order.
pub(super) const PILL_SIZES: [PillSize; 3] = [PillSize::Hidden, PillSize::Small, PillSize::Big];
pub(super) const WAVE_COLOURS: [WaveColour; 2] = [WaveColour::Accent, WaveColour::Cover];
/// The calendar's layouts, in their segments' order.
pub(super) const CALENDAR_LAYOUTS: [CalendarLayout; 3] = [CalendarLayout::Comfortable, CalendarLayout::Compact, CalendarLayout::AgendaOnly];
pub(super) const AGENDA_ROWS: [u8; 3] = [3, 5, 8];
/// The week's first day: the locale's, Monday (0) or Sunday (6).
pub(super) const WEEK_STARTS: [Option<u8>; 3] = [None, Some(0), Some(6)];
/// Minutes before an event; 0 is off.
pub(super) const REMINDERS: [u32; 4] = [0, 1, 5, 10];
pub(super) const FOCUS_LENGTHS: [u32; 4] = [15, 25, 45, 60];
/// The colours a calendar can take, after its own.
pub(in crate::settings) const SWATCHES: [[u8; 3]; 8] = [
    [0xE8, 0x83, 0x6A],
    [0xF7, 0xB3, 0x2B],
    [0x6C, 0xCB, 0x5F],
    [0x4F, 0xC3, 0xF7],
    [0x47, 0x9E, 0xF5],
    [0x7B, 0x86, 0xC6],
    [0xC3, 0x9B, 0xF5],
    [0xF0, 0x62, 0x92],
];

/// Option `i` of `all`; the last one for an index past the end.
fn pick<T: Copy>(all: &[T], i: usize) -> T {
    all[i.min(all.len() - 1)]
}

impl Choice {
    /// Set option `i` in `c`. `monitors` are the displays' names, in [`crate::win::monitors`]'
    /// order. A keyboard's layout is not set here: it is picked among the installed layouts, which
    /// the window reads.
    pub(in crate::settings) fn apply(self, i: usize, c: &mut Config, monitors: &[String]) {
        match self {
            Choice::Tile => c.music.tile_style = pick(&TILE_STYLES, i),
            Choice::MiddleClick => c.strip.middle_click = pick(&MIDDLE_CLICKS, i),
            Choice::BellMiddle => c.calendar.bell_middle_click = pick(&BELL_MIDDLE_CLICKS, i),
            Choice::PillSize => c.pill.size = pick(&PILL_SIZES, i),
            Choice::BrowserTabs => c.browser.every_site = i == 0,
            // Automatic, then each display.
            Choice::Display => c.pill.monitor = i.checked_sub(1).and_then(|m| monitors.get(m).cloned()),
            Choice::LowBattery => c.batteries.low_at = pick(&Batteries::LOW_LEVELS, i),
            Choice::BatteryIconStyle => c.batteries.icon_style = pick(&BatteryIconStyle::ALL, i),
            Choice::KeyboardLayout(_) => {}
            Choice::RingMotion => c.motion.ring = pick(&MotionStyle::ALL, i),
            Choice::BarMotion => c.motion.bar = pick(&MotionStyle::ALL, i),
            Choice::VolumeMotion => c.motion.volume = pick(&MotionStyle::ALL, i),
            Choice::WaveColour => c.motion.colour = pick(&WAVE_COLOURS, i),
            Choice::CalendarLayout => c.calendar.layout = pick(&CALENDAR_LAYOUTS, i),
            Choice::AgendaRows => c.calendar.rows = pick(&AGENDA_ROWS, i),
            Choice::WeekStart => c.calendar.first_weekday = pick(&WEEK_STARTS, i),
            Choice::Reminder => c.calendar.remind_minutes = pick(&REMINDERS, i),
            Choice::FocusLength => c.calendar.focus_minutes = pick(&FOCUS_LENGTHS, i),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_sets_the_option_shown_at_its_place() {
        let mut c = Config::default();
        Choice::WeekStart.apply(2, &mut c, &[]);
        assert_eq!(c.calendar.first_weekday, Some(6));
        Choice::PillSize.apply(0, &mut c, &[]);
        assert_eq!(c.pill.size, PillSize::Hidden);
        Choice::WaveColour.apply(1, &mut c, &[]);
        assert_eq!(c.motion.colour, WaveColour::Cover);
        // Past the end: the last.
        Choice::Reminder.apply(9, &mut c, &[]);
        assert_eq!(c.calendar.remind_minutes, 10);
        Choice::Display.apply(2, &mut c, &["A".into(), "B".into()]);
        assert_eq!(c.pill.monitor.as_deref(), Some("B"));
    }
}

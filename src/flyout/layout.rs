//! Pure layout and hit-testing for the flyout: [`Elem`]s stacked into [`LaidElem`]s by
//! [`build_view`], panel size, and what a mouse coordinate hits.
//! No Win32 or pixel buffers here, so it stays unit-testable.

use ab_glyph::FontVec;

use crate::canvas::measure;
use super::model::Model;
use super::theme::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ActionKind {
    /// Audio Tray's settings window.
    Settings,
    Quit,
    /// Relaunch into an update already staged on disk. Only offered while there is one.
    Restart,
}

impl ActionKind {
    /// The footer's labelled left-hand item (glyph + text).
    pub(super) const LEFT: ActionKind = ActionKind::Quit;

    pub(super) fn label(self) -> &'static str {
        match self {
            ActionKind::Settings => "Settings",
            ActionKind::Quit => "Quit Audio Tray",
            ActionKind::Restart => "Restart to update",
        }
    }
    pub(super) fn glyph(self) -> char {
        match self {
            ActionKind::Settings => GLYPH_SETTINGS,
            ActionKind::Quit => GLYPH_CANCEL,
            ActionKind::Restart => GLYPH_UPDATE,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Elem {
    Header(&'static str),
    /// A device section's header: clicking it shows or hides the devices that are not selected.
    GroupHeader { group: usize },
    /// One media session the pill shows, with transport buttons ([`media_button_at`]).
    NowPlaying { index: usize },
    /// The call: who is in it, and the user's mute ([`over_voice_button`]).
    Voice,
    /// One app's volume in the mixer.
    AppSlider { app: usize },
    Slider { group: usize },
    Device { group: usize, dev: usize },
    /// The panel's closing strip: a hairline, [`ActionKind::LEFT`] as a labelled item on
    /// the left and [`footer_buttons`] as round icon buttons on the right.
    Footer,
    /// One line of secondary text above the footer: [`Model::notice`].
    Note,
}

pub(super) struct LaidElem {
    pub elem: Elem,
    pub top: i32,
    pub height: i32,
}

/// The panel width, measured from the rows' content.
pub(super) fn content_width(model: &Model, scale: f32) -> i32 {
    let font = ui_font();
    let font_sb = ui_font_semibold().or(font);
    let text_px = 14.0 * scale;
    let hdr_px = 14.0 * scale;
    let mw = |f: Option<&FontVec>, px: f32, s: &str| f.map(|f| measure(f, px, s)).unwrap_or(0.0);

    let mut max_w = 0.0f32;
    for g in &model.groups {
        max_w = max_w.max(HEADER_X * scale + mw(font_sb, hdr_px, g.title) + RIGHT_PAD * scale);
        if g.default_id.is_some() {
            max_w = max_w.max((TRACK_X0 + 130.0 + VALUE_W) * scale);
        }
        for row in &g.devices {
            let reserve = if row.battery.is_some() { BATTERY_W } else { RIGHT_PAD };
            max_w = max_w.max(TEXT_X * scale + mw(font, text_px, &row.label) + reserve * scale);
        }
    }
    // Wide enough for the footer's label and buttons on one line.
    max_w = max_w.max(footer_min_width(model, scale));
    // An app's slider needs room after its name.
    if !model.apps.is_empty() {
        max_w = max_w.max((APP_TRACK_X0 + 130.0 + VALUE_W) * scale);
    }
    // `ceil`, not `round`, or `fit_label` ellipsises text that was measured to fit.
    max_w.clamp(MIN_W * scale, MAX_W * scale).ceil() as i32
}

/// Lay out the panel's elements, returning them and the panel's total height.
pub(super) fn build_view(model: &Model, scale: f32) -> (Vec<LaidElem>, i32) {
    let d = |v: f32| (v * scale).round() as i32;
    let mut kinds: Vec<Elem> = Vec::new();
    for index in 0..model.now_playing.len() {
        kinds.push(Elem::NowPlaying { index });
    }
    if model.voice.is_some() {
        kinds.push(Elem::Voice);
    }
    for (gi, g) in model.groups.iter().enumerate() {
        kinds.push(Elem::GroupHeader { group: gi });
        if g.default_id.is_some() {
            kinds.push(Elem::Slider { group: gi });
        }
        for di in shown_devices(model, gi) {
            kinds.push(Elem::Device { group: gi, dev: di });
        }
    }
    if !model.apps.is_empty() {
        kinds.push(Elem::Header("Apps"));
        for app in 0..model.apps.len() {
            kinds.push(Elem::AppSlider { app });
        }
    }
    if model.notice.is_some() {
        kinds.push(Elem::Note);
    }
    // The closing strip (settings, quit, and a staged update; see [`footer_buttons`]).
    kinds.push(Elem::Footer);

    let mut elems = Vec::with_capacity(kinds.len());
    let mut y = d(PAD_V);
    for (i, elem) in kinds.into_iter().enumerate() {
        // The gap stays outside the element, so the footer's hit band starts at its hairline.
        if matches!(elem, Elem::Footer) {
            y += d(FOOTER_TOP_GAP);
        }
        let height = match elem {
            // Later headers get a larger top gap to separate groups.
            Elem::Header(_) | Elem::GroupHeader { .. } => d(if i == 0 { HEADER_FIRST_H } else { HEADER_H }),
            Elem::NowPlaying { .. } => d(NOW_PLAYING_H),
            Elem::Voice => d(VOICE_H),
            Elem::AppSlider { .. } => d(APP_H),
            Elem::Slider { .. } => d(SLIDER_H),
            Elem::Device { .. } => d(ITEM_H),
            Elem::Footer => d(FOOTER_H),
            Elem::Note => d(NOTE_H),
        };
        elems.push(LaidElem { elem, top: y, height });
        y += height;
    }
    // The footer runs to the bottom edge, replacing the closing gap.
    let ends_flush = matches!(elems.last().map(|le| le.elem), Some(Elem::Footer));
    let total = y + if ends_flush { 0 } else { d(PAD_V) };
    (elems, total)
}

/// The footer's round icon buttons, rightmost first: the settings gear, then restart-to-update
/// while an update is staged. No "restart Explorer" button by design (see docs/NOTES.md).
pub(super) fn footer_buttons(model: &Model) -> Vec<ActionKind> {
    let mut buttons = vec![ActionKind::Settings];
    if model.update.is_some() {
        buttons.push(ActionKind::Restart);
    }
    buttons
}

/// Horizontal centre (px) of footer button `i` from the right; shared by hit-test and paint.
pub(super) fn footer_btn_center_x(width: i32, scale: f32, i: usize) -> f32 {
    let step = (FOOTER_BTN + FOOTER_BTN_GAP) * scale;
    width as f32 - (FOOTER_BTN_RIGHT + FOOTER_BTN / 2.0) * scale - i as f32 * step
}

/// Right edge (px) of the footer's labelled item, for both its hover pill and its hit target.
pub(super) fn footer_item_right(scale: f32) -> f32 {
    let label_w = ui_font()
        .map(|f| measure(f, FOOTER_TEXT_PX * scale, ActionKind::LEFT.label()))
        .unwrap_or(0.0);
    (FOOTER_TEXT_X + FOOTER_ITEM_PAD) * scale + label_w
}

/// The narrowest panel the footer fits in: the left item, a gap, then the icon buttons.
pub(super) fn footer_min_width(model: &Model, scale: f32) -> f32 {
    let n = footer_buttons(model).len() as f32;
    let buttons = n * FOOTER_BTN + (n - 1.0) * FOOTER_BTN_GAP;
    footer_item_right(scale) + (FOOTER_GAP + buttons + FOOTER_BTN_RIGHT) * scale
}

/// Which footer action (if any) is under `mx`; the gap between item and buttons is inert.
pub(super) fn footer_hit(model: &Model, width: i32, scale: f32, mx: i32) -> Option<ActionKind> {
    let x = mx as f32;
    for (i, k) in footer_buttons(model).into_iter().enumerate() {
        if (x - footer_btn_center_x(width, scale, i)).abs() <= FOOTER_BTN * scale / 2.0 {
            return Some(k);
        }
    }
    (x >= ROW_MARGIN * scale && x <= footer_item_right(scale)).then_some(ActionKind::LEFT)
}

pub(super) fn inside(width: i32, height: i32, mx: i32, my: i32) -> bool {
    (0..width).contains(&mx) && (0..height).contains(&my)
}

/// Index of the actionable element at vertical position `y`.
pub(super) fn elem_at(elems: &[LaidElem], y: i32) -> Option<usize> {
    elems.iter().position(|le| {
        let actionable = matches!(
            le.elem,
            Elem::Slider { .. }
                | Elem::GroupHeader { .. }
                | Elem::NowPlaying { .. }
                | Elem::Voice
                | Elem::AppSlider { .. }
                | Elem::Device { .. }
                | Elem::Footer
        );
        actionable && y >= le.top && y < le.top + le.height
    })
}

/// The devices a group lists: all of them when expanded, else only the selected one (the first
/// when none is).
pub(super) fn shown_devices(model: &Model, group: usize) -> Vec<usize> {
    let g = &model.groups[group];
    if model.expanded.get(group).copied().unwrap_or(false) || g.devices.len() <= 1 {
        return (0..g.devices.len()).collect();
    }
    vec![g.devices.iter().position(|d| d.selected).unwrap_or(0)]
}

/// Which transport button of the Now Playing card is under `mx`: 0 previous, 1 play/pause,
/// 2 next. Shared by hit-test and paint ([`media_button_center_x`]).
pub(super) fn media_button_at(width: i32, scale: f32, mx: i32) -> Option<usize> {
    (0..3).find(|&i| ((mx as f32) - media_button_center_x(width, scale, i)).abs() <= MEDIA_BTN * scale / 2.0)
}

pub(super) fn media_button_center_x(width: i32, scale: f32, i: usize) -> f32 {
    let step = (MEDIA_BTN + MEDIA_BTN_GAP) * scale;
    width as f32 - (RIGHT_PAD - 6.0 + MEDIA_BTN / 2.0) * scale - (2 - i) as f32 * step
}

/// Whether `mx` is over the voice panel's mute button.
pub(super) fn over_voice_button(width: i32, scale: f32, mx: i32) -> bool {
    ((mx as f32) - voice_button_center_x(width, scale)).abs() <= VOICE_BTN * scale / 2.0
}

pub(super) fn voice_button_center_x(width: i32, scale: f32) -> f32 {
    width as f32 - (RIGHT_PAD - 4.0 + VOICE_BTN / 2.0) * scale
}


/// An app slider's level at `mx`.
pub(super) fn app_level_from_x(width: i32, scale: f32, mx: i32) -> f32 {
    let x0 = APP_TRACK_X0 * scale;
    let x1 = width as f32 - VALUE_W * scale;
    (((mx as f32) - x0) / (x1 - x0)).clamp(0.0, 1.0)
}

pub(super) fn level_from_x(width: i32, scale: f32, mx: i32) -> f32 {
    let x0 = TRACK_X0 * scale;
    let x1 = width as f32 - VALUE_W * scale;
    (((mx as f32) - x0) / (x1 - x0)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{DeviceId, Flow};
    use crate::flyout::model::{DeviceRow, Group, Model};
    use crate::icons::IconId;

    fn dev(label: &str, battery: Option<u8>) -> DeviceRow {
        DeviceRow {
            id: DeviceId(label.to_string()),
            label: label.to_string(),
            icon: IconId::Speakers,
            selected: false,
            battery,
        }
    }

    fn model(groups: Vec<Group>, update: Option<&str>) -> Model {
        Model::new(groups, update.map(str::to_string))
    }

    fn output_group(devices: Vec<DeviceRow>) -> Group {
        Group {
            flow: Flow::Output,
            title: "Output",
            default_id: Some(DeviceId("d".into())),
            level: 0.5,
            muted: false,
            peak: 0.0,
            recording: false,
            devices,
        }
    }

    #[test]
    fn a_folded_group_lists_only_its_selected_device() {
        let mut speakers = dev("Speakers", None);
        speakers.selected = true;
        let mut m = model(vec![output_group(vec![dev("Headphones", None), speakers, dev("TV", None)])], None);
        let devices = |m: &Model| {
            build_view(m, 1.0)
                .0
                .iter()
                .filter_map(|le| match le.elem {
                    Elem::Device { dev, .. } => Some(dev),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(devices(&m), vec![1], "folded: the selected one only");
        m.expanded[0] = true;
        assert_eq!(devices(&m), vec![0, 1, 2]);
    }

    #[test]
    fn media_buttons_are_hit_where_they_are_painted() {
        for scale in [1.0_f32, 1.5, 2.0] {
            let width = 400;
            for i in 0..3 {
                let x = media_button_center_x(width, scale, i).round() as i32;
                assert_eq!(media_button_at(width, scale, x), Some(i));
            }
            assert_eq!(media_button_at(width, scale, 40), None, "the art is not a button");
        }
    }

    #[test]
    fn content_width_always_within_min_max() {
        for scale in [1.0_f32, 1.5, 2.0] {
            let m = model(vec![output_group(vec![dev("Speakers", None)])], None);
            let w = content_width(&m, scale);
            assert!(w >= (MIN_W * scale).round() as i32, "w={w} below min at scale {scale}");
            assert!(w <= (MAX_W * scale).round() as i32, "w={w} above max at scale {scale}");
        }
    }

    #[test]
    fn content_width_fits_the_footer_even_with_no_devices() {
        // With no devices the width comes purely from the footer, whose labelled item and
        // icon button share one line — too narrow and they would overlap.
        for scale in [1.0_f32, 1.5, 2.0] {
            let m = model(vec![], None);
            let w = content_width(&m, scale);
            assert!(w >= (MIN_W * scale).round() as i32, "w={w} below the panel floor");
            assert!(w <= (MAX_W * scale).round() as i32, "w={w} above the cap");
            assert!(
                w as f32 >= footer_min_width(&m, scale),
                "w={w} squeezes the footer at scale {scale}"
            );
        }
    }

    #[test]
    fn main_view_always_ends_with_the_footer() {
        // Sound settings and Quit are only reachable through the footer.
        let m = model(vec![output_group(vec![dev("Speakers", None)])], None);
        let (elems, total) = build_view(&m, 1.0);
        let last = elems.last().expect("main view is never empty");
        assert!(matches!(last.elem, Elem::Footer));
        // Flush with the bottom edge — no padding band under the strip.
        assert_eq!(last.top + last.height, total);
    }

    /// The integration notice sits right above the footer and is not clickable.
    #[test]
    fn a_notice_sits_on_the_footer() {
        let mut m = model(vec![output_group(vec![dev("Speakers", None)])], None);
        let (plain, _) = build_view(&m, 1.0);
        m.notice = Some("Taskbar buttons could not be placed");
        let (elems, total) = build_view(&m, 1.0);
        assert_eq!(elems.len(), plain.len() + 1);
        let n = elems.len();
        let (note, footer) = (&elems[n - 2], &elems[n - 1]);
        assert!(matches!(note.elem, Elem::Note) && matches!(footer.elem, Elem::Footer));
        assert!(note.top + note.height <= footer.top);
        assert_eq!(footer.top + footer.height, total);
        assert_eq!(elem_at(&elems, note.top + 1), None);
    }

    #[test]
    fn footer_hit_targets_its_actions_and_nothing_between() {
        for scale in [1.0_f32, 1.5, 2.0] {
            let m = model(vec![], None);
            let width = content_width(&m, scale);
            let btn = footer_btn_center_x(width, scale, 0).round() as i32;
            assert_eq!(footer_hit(&m, width, scale, btn), Some(ActionKind::Settings));
            let item = ((FOOTER_TEXT_X + 2.0) * scale).round() as i32;
            assert_eq!(footer_hit(&m, width, scale, item), Some(ActionKind::LEFT));
            // The gap between the label and the buttons is inert.
            let gap = ((footer_item_right(scale) + footer_btn_center_x(width, scale, 0)
                - FOOTER_BTN * scale / 2.0)
                / 2.0)
                .round() as i32;
            assert_eq!(footer_hit(&m, width, scale, gap), None, "gap at {gap} is clickable");
        }
    }

    #[test]
    fn a_staged_update_adds_a_restart_button_left_of_the_gear() {
        // The update button must not land on top of the gear.
        let scale = 1.5;
        let staged = model(vec![output_group(vec![dev("Speakers", None)])], Some("9.9.9"));
        let plain = model(vec![output_group(vec![dev("Speakers", None)])], None);
        assert_eq!(footer_buttons(&plain), vec![ActionKind::Settings]);
        assert_eq!(
            footer_buttons(&staged),
            vec![ActionKind::Settings, ActionKind::Restart]
        );

        let width = content_width(&staged, scale);
        let gear = footer_btn_center_x(width, scale, 0);
        let restart = footer_btn_center_x(width, scale, 1);
        assert!(restart < gear - FOOTER_BTN * scale, "buttons overlap: {restart} vs {gear}");
        assert_eq!(
            footer_hit(&staged, width, scale, restart.round() as i32),
            Some(ActionKind::Restart)
        );
        // The same spot is the inert middle when nothing is staged.
        assert_eq!(footer_hit(&plain, width, scale, restart.round() as i32), None);
        // …and the panel is wide enough to hold the extra button clear of the label.
        assert!(restart - FOOTER_BTN * scale / 2.0 > footer_item_right(scale));
    }

    #[test]
    fn level_from_x_clamps_to_unit_range() {
        let (width, scale) = (510, 1.5);
        let x0 = (TRACK_X0 * scale) as i32;
        let x1 = width - (VALUE_W * scale) as i32;
        assert_eq!(level_from_x(width, scale, x0 - 50), 0.0);
        assert_eq!(level_from_x(width, scale, x1 + 50), 1.0);
        let mid = level_from_x(width, scale, (x0 + x1) / 2);
        assert!((mid - 0.5).abs() < 0.02, "mid={mid}");
    }

    #[test]
    fn elem_at_skips_non_actionable_headers() {
        // A header (non-actionable) followed by a device row.
        let elems = vec![
            LaidElem { elem: Elem::Header("Output"), top: 0, height: 40 },
            LaidElem { elem: Elem::Device { group: 0, dev: 0 }, top: 40, height: 60 },
        ];
        assert_eq!(elem_at(&elems, 20), None); // inside the header band
        assert_eq!(elem_at(&elems, 70), Some(1)); // inside the device row
        assert_eq!(elem_at(&elems, 500), None); // past the end
    }
}

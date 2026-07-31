//! Pure layout + hit-testing for the flyout: the element taxonomy ([`Elem`]), how a screen
//! is measured and stacked into positioned [`LaidElem`]s ([`build_view`]), the panel's
//! width/height, the wrapping icon-grid geometry, and where a mouse coordinate lands.
//!
//! Every function here is pure — it takes the display [`Model`] (or bare geometry) plus the
//! DPI `scale` and returns numbers, touching no `self`, no Win32, and no pixel buffer. That
//! is what makes the fiddly geometry unit-testable (see the tests at the bottom).

use ab_glyph::FontVec;

use crate::icons::IconId;

use super::canvas::measure;
use super::model::Model;
use super::theme::*;

/// Which screen the flyout is showing. The icon picker is a *dedicated* sub-screen you
/// slide to from a device row's edit pencil (rather than an inline row), so it can lay its
/// icons out in a wrapping grid without ever changing the flyout's width.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum View {
    /// The main audio panel (sliders + device lists).
    Main,
    /// The per-device icon chooser: a back arrow, the device name as title, and a grid.
    IconPicker { group: usize, dev: usize },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ActionKind {
    SoundSettings,
    Quit,
    /// Relaunch into an update already staged on disk. Only offered while there is one.
    Restart,
    /// Restart `explorer.exe`, rebuilding the strip with it. Always offered — see
    /// [`footer_buttons`] for why it cannot be conditional.
    RestartExplorer,
}

impl ActionKind {
    /// The footer's labelled left-hand item (glyph + text, like the battery readout in the
    /// Win11 quick-settings footer).
    pub(super) const LEFT: ActionKind = ActionKind::Quit;

    pub(super) fn label(self) -> &'static str {
        match self {
            ActionKind::SoundSettings => "Sound settings",
            ActionKind::Quit => "Quit Audio Tray",
            ActionKind::Restart => "Restart to update",
            ActionKind::RestartExplorer => "Restart Explorer",
        }
    }
    pub(super) fn glyph(self) -> char {
        match self {
            ActionKind::SoundSettings => GLYPH_SETTINGS,
            ActionKind::Quit => GLYPH_CANCEL,
            ActionKind::Restart => GLYPH_UPDATE,
            ActionKind::RestartExplorer => GLYPH_RESTART_SHELL,
        }
    }

    /// Whether the button wears a standing accent disc rather than being a bare glyph — i.e.
    /// whether there is a reason to *notice* it on a panel opened to change the volume.
    ///
    /// `Restart` always: it only exists while an update is staged. `RestartExplorer` is always
    /// present (see [`footer_buttons`]) and so has to earn its accent — from a staged update,
    /// or from a `strip_up` that is false. Note this only trusts that flag in the direction it
    /// is reliable: `false` means we know the injection failed or was reverted, while `true` is
    /// merely optimistic, which is why it no longer decides whether the button appears.
    pub(super) fn is_cta(self, model: &Model) -> bool {
        match self {
            ActionKind::Restart => true,
            ActionKind::RestartExplorer => !model.strip_up || model.update.is_some(),
            ActionKind::SoundSettings | ActionKind::Quit => false,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Elem {
    Header(&'static str),
    Slider { group: usize },
    Device { group: usize, dev: usize },
    /// The icon-picker screen's header: a back arrow + the device name as the title.
    PickerHeader { group: usize, dev: usize },
    /// The icon-picker screen's wrapping grid of selectable icons.
    IconGrid { group: usize, dev: usize },
    /// The panel's closing strip: a hairline, [`ActionKind::LEFT`] as a labelled item on
    /// the left and [`footer_buttons`] as round icon buttons on the right.
    Footer,
}

pub(super) struct LaidElem {
    pub elem: Elem,
    pub top: i32,
    pub height: i32,
}

/// The panel width, measured from the *main* view's content only. Constant for the life of
/// the flyout (the device set and labels don't change while open), so it anchors the width
/// for every screen — the icon picker wraps its grid into this width rather than forcing the
/// panel wider.
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
            let reserve = if row.battery.is_some() { BATTERY_W } else { PENCIL_W };
            max_w = max_w.max(TEXT_X * scale + mw(font, text_px, &row.label) + reserve * scale);
        }
    }
    // The former right-click menu now lives in the footer strip: its labelled item and its
    // icon buttons sit on the same line, so the panel has to be wide enough for both.
    max_w = max_w.max(footer_min_width(model, scale));
    // `ceil`, not `round`: rounding down by a fraction of a pixel makes the panel
    // narrower than the text it was measured from, which then gets ellipsised by
    // `fit_label` even though it was supposed to fit exactly.
    max_w.clamp(MIN_W * scale, MAX_W * scale).ceil() as i32
}

/// The fixed panel height shared by every screen: the taller of the main panel and the icon
/// picker (in practice the main panel, which has the sliders + device lists).
pub(super) fn panel_height(model: &Model, scale: f32, width: i32) -> i32 {
    let main_h = build_view(model, scale, width, View::Main, 0).1;
    let picker_h = if model.groups.iter().any(|g| !g.devices.is_empty()) {
        build_view(model, scale, width, View::IconPicker { group: 0, dev: 0 }, 0).1
    } else {
        0
    };
    main_h.max(picker_h)
}

/// Build (and vertically lay out) the elements for `view`, returning them plus the total
/// panel height. Pure — used both to render the current screen and to render the two screens
/// involved in a slide transition. Uses the fixed `width`.
///
/// `fill_h` is the fixed panel height every screen shares (so navigating never resizes the
/// window): the layout is grown to at least `fill_h`, and on the icon-picker screen the grid
/// is centred in the slack below the header. Pass `0` to lay out naturally (used once, to
/// measure each screen's intrinsic height).
pub(super) fn build_view(model: &Model, scale: f32, width: i32, view: View, fill_h: i32) -> (Vec<LaidElem>, i32) {
    let d = |v: f32| (v * scale).round() as i32;
    let mut kinds: Vec<Elem> = Vec::new();
    match view {
        View::Main => {
            for (gi, g) in model.groups.iter().enumerate() {
                kinds.push(Elem::Header(g.title));
                if g.default_id.is_some() {
                    kinds.push(Elem::Slider { group: gi });
                }
                for di in 0..g.devices.len() {
                    kinds.push(Elem::Device { group: gi, dev: di });
                }
            }
            // The former right-click menu, now the strip that closes the panel. A staged
            // update rides in it too, as an extra button (see [`footer_buttons`]).
            kinds.push(Elem::Footer);
        }
        View::IconPicker { group, dev } => {
            kinds.push(Elem::PickerHeader { group, dev });
            kinds.push(Elem::IconGrid { group, dev });
        }
    }

    let mut elems = Vec::with_capacity(kinds.len());
    let mut y = d(PAD_V);
    for (i, elem) in kinds.into_iter().enumerate() {
        // The footer stands off from the row above it, and the gap stays *outside* the
        // element — so the strip's hover/hit band starts at its hairline, not under the
        // last device row.
        if matches!(elem, Elem::Footer) {
            y += d(FOOTER_TOP_GAP);
        }
        let height = match elem {
            // The first header sits at the very top (small gap); a later header separates
            // one group from the one above it.
            Elem::Header(_) => d(if i == 0 { HEADER_FIRST_H } else { HEADER_H }),
            Elem::Slider { .. } => d(SLIDER_H),
            Elem::Device { .. } => d(ITEM_H),
            Elem::PickerHeader { .. } => d(PICKER_HEADER_H),
            Elem::IconGrid { .. } => grid_px_height(width, scale),
            Elem::Footer => d(FOOTER_H),
        };
        elems.push(LaidElem { elem, top: y, height });
        y += height;
    }
    // The footer runs to the panel's bottom edge (its own height is the padding), so it
    // replaces the closing gap rather than sitting above one.
    let ends_flush = matches!(elems.last().map(|le| le.elem), Some(Elem::Footer));
    let natural = y + if ends_flush { 0 } else { d(PAD_V) };
    let total = natural.max(fill_h);

    // Spend any extra vertical space: centre the icon grid in it, so the picker fills the
    // shared panel height without a big empty band at the bottom (the header stays pinned
    // top), and keep the footer flush against the bottom edge.
    let slack = total - natural;
    if slack > 0 {
        for le in &mut elems {
            match le.elem {
                Elem::IconGrid { .. } => le.top += slack / 2,
                Elem::Footer => le.top += slack,
                _ => {}
            }
        }
    }
    (elems, total)
}

/// Icon-grid geometry for a given panel `width`: `(cols, left_px, chip_px, step_px)`. The
/// grid wraps to as many equal columns as fit the width and is centred within the panel, so
/// the icons wrap onto multiple rows without ever widening the flyout.
pub(super) fn grid_metrics(width: i32, scale: f32) -> (i32, i32, i32, i32) {
    let chip = (GRID_CHIP * scale).round() as i32;
    let gap = (GRID_GAP * scale).round() as i32;
    let step = chip + gap;
    let n = IconId::ALL.len() as i32;
    let avail = width - ((GRID_X + RIGHT_PAD) * scale).round() as i32;
    let cols = (((avail + gap) / step).max(1)).min(n);
    let grid_w = cols * chip + (cols - 1) * gap;
    let left = (width - grid_w) / 2;
    (cols, left, chip, step)
}

/// Total pixel height of the wrapping icon grid (top pad + rows + bottom pad).
pub(super) fn grid_px_height(width: i32, scale: f32) -> i32 {
    let (cols, _left, chip, step) = grid_metrics(width, scale);
    let gap = step - chip;
    let n = IconId::ALL.len() as i32;
    let rows = (n + cols - 1) / cols;
    (GRID_TOP_PAD * scale).round() as i32
        + rows * chip
        + (rows - 1).max(0) * gap
        + (GRID_BOTTOM_PAD * scale).round() as i32
}

/// The footer's round icon buttons, **rightmost first**: the settings gear, the
/// restart-Explorer button, and — only while an update is staged — restart-to-update.
///
/// `RestartExplorer` is **permanent furniture**, which took a wrong turn to arrive at. It
/// began as conditional, shown when `strip_up` was false or an update was staged. Both of
/// those are the situations a fresh Explorer fixes, so the rule looked right; it is not,
/// because *audio-tray cannot tell whether the strip is actually drawn*. `strip_up` records
/// that an injection was asked for and accepted, and `taskbar::control_window` only proves the
/// TAP is receiving XAML callbacks — observed live, both said "up" while the taskbar showed a
/// bare notification icon and Explorer's own volume slot was still its own. So the button hid
/// itself in exactly the case it exists for.
///
/// Hence: always offered, as a plain "the taskbar controls are wrong, rebuild them" action.
/// The two conditions survive in [`ActionKind::is_cta`], which decides whether it *draws
/// attention* — a thing it is safe to be wrong about, unlike whether it exists at all.
///
/// Order matters: the always-present buttons keep fixed positions and the transient
/// `Restart` appears to their left, so a button never moves under the pointer.
pub(super) fn footer_buttons(model: &Model) -> Vec<ActionKind> {
    let mut buttons = vec![ActionKind::SoundSettings, ActionKind::RestartExplorer];
    if model.update.is_some() {
        buttons.push(ActionKind::Restart);
    }
    buttons
}

/// Horizontal centre (px) of footer button `i`, counted from the right — shared by
/// hit-testing, the hover circle, and the glyph so they always coincide.
pub(super) fn footer_btn_center_x(width: i32, scale: f32, i: usize) -> f32 {
    let step = (FOOTER_BTN + FOOTER_BTN_GAP) * scale;
    width as f32 - (FOOTER_BTN_RIGHT + FOOTER_BTN / 2.0) * scale - i as f32 * step
}

/// Right edge (px) of the footer's labelled left item: its hover pill *and* its hit target,
/// so the pill is exactly the area that responds — it never stretches across the strip.
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

/// Which footer action (if any) is under `mx`. The strip between the labelled item and the
/// buttons is inert, so a click on the empty middle does nothing rather than quitting.
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
                | Elem::Device { .. }
                | Elem::PickerHeader { .. }
                | Elem::IconGrid { .. }
                | Elem::Footer
        );
        actionable && y >= le.top && y < le.top + le.height
    })
}

pub(super) fn level_from_x(width: i32, scale: f32, mx: i32) -> f32 {
    let x0 = TRACK_X0 * scale;
    let x1 = width as f32 - VALUE_W * scale;
    (((mx as f32) - x0) / (x1 - x0)).clamp(0.0, 1.0)
}

/// Whether `mx` is over the edit pencil's round button (its hover/click target).
pub(super) fn over_pencil(width: i32, scale: f32, mx: i32) -> bool {
    let cx = pencil_center_x(width, scale);
    ((mx as f32) - cx).abs() <= PENCIL_BTN * scale / 2.0
}

/// Whether `mx` is over the picker's back button (its hover/click target).
pub(super) fn over_back(scale: f32, mx: i32) -> bool {
    let x0 = BACK_LEFT * scale;
    let x1 = (BACK_LEFT + BACK_BTN) * scale;
    (mx as f32) >= x0 && (mx as f32) <= x1
}

/// Which icon-grid cell (if any) is at `(mx, my)`, given the grid element's top `gy`.
pub(super) fn grid_chip_at(width: i32, scale: f32, mx: i32, my: i32, gy: i32) -> Option<usize> {
    let (cols, left, chip, step) = grid_metrics(width, scale);
    let gy0 = gy + (GRID_TOP_PAD * scale).round() as i32;
    if mx < left || my < gy0 {
        return None;
    }
    let col = (mx - left) / step;
    let row = (my - gy0) / step;
    let within_x = (mx - left) - col * step;
    let within_y = (my - gy0) - row * step;
    if col >= cols || within_x > chip || within_y > chip {
        return None;
    }
    let k = (row * cols + col) as usize;
    (k < IconId::ALL.len()).then_some(k)
}

/// Horizontal centre (in px) of the edit pencil's button — shared by hit-testing, the hover
/// highlight, and the glyph so they always coincide.
pub(super) fn pencil_center_x(width: i32, scale: f32) -> f32 {
    width as f32 - (PENCIL_RIGHT + PENCIL_BTN / 2.0) * scale
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{DeviceId, Flow};
    use crate::flyout::model::{DeviceRow, Group, Model};

    fn dev(label: &str, battery: Option<u8>) -> DeviceRow {
        DeviceRow {
            id: DeviceId(label.to_string()),
            label: label.to_string(),
            icon: IconId::Speakers,
            selected: false,
            battery,
        }
    }

    /// A model with the strip up — the ordinary case, and the one where the footer carries
    /// nothing but the gear. [`model_no_strip`] covers the other side.
    fn model(groups: Vec<Group>, update: Option<&str>) -> Model {
        Model::new(groups, update.map(str::to_string), true)
    }

    fn model_no_strip(groups: Vec<Group>, update: Option<&str>) -> Model {
        Model::new(groups, update.map(str::to_string), false)
    }

    fn output_group(devices: Vec<DeviceRow>) -> Group {
        Group {
            flow: Flow::Output,
            title: "Output",
            default_id: Some(DeviceId("d".into())),
            level: 0.5,
            muted: false,
            peak: 0.0,
            devices,
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
        // The former right-click menu is only reachable through the footer now, so losing
        // it would strand Sound settings and Quit.
        let m = model(vec![output_group(vec![dev("Speakers", None)])], None);
        let (elems, total) = build_view(&m, 1.0, 400, View::Main, 0);
        let last = elems.last().expect("main view is never empty");
        assert!(matches!(last.elem, Elem::Footer));
        // Flush with the bottom edge — no padding band under the strip.
        assert_eq!(last.top + last.height, total);
    }

    #[test]
    fn footer_stays_pinned_to_the_bottom_when_the_panel_is_stretched() {
        // The panel is as tall as its tallest screen, so the main view can be handed more
        // height than it needs; the footer must follow the bottom edge, not float.
        let m = model(vec![output_group(vec![dev("Speakers", None)])], None);
        let (elems, natural) = build_view(&m, 1.0, 400, View::Main, 0);
        let natural_top = elems.last().map(|le| le.top).unwrap();
        let (elems, total) = build_view(&m, 1.0, 400, View::Main, natural + 120);
        let last = elems.last().unwrap();
        assert_eq!(total, natural + 120);
        assert_eq!(last.top, natural_top + 120);
        assert_eq!(last.top + last.height, total);
    }

    #[test]
    fn footer_hit_targets_its_actions_and_nothing_between() {
        for scale in [1.0_f32, 1.5, 2.0] {
            let m = model(vec![], None);
            let width = content_width(&m, scale);
            let btn = footer_btn_center_x(width, scale, 0).round() as i32;
            assert_eq!(footer_hit(&m, width, scale, btn), Some(ActionKind::SoundSettings));
            let item = ((FOOTER_TEXT_X + 2.0) * scale).round() as i32;
            assert_eq!(footer_hit(&m, width, scale, item), Some(ActionKind::LEFT));
            // The gap between the label and the buttons is inert. Measured to the *leftmost*
            // button, since there is always more than one.
            let leftmost = footer_btn_center_x(width, scale, footer_buttons(&m).len() - 1);
            let gap = ((footer_item_right(scale) + leftmost - FOOTER_BTN * scale / 2.0) / 2.0)
                .round() as i32;
            assert_eq!(footer_hit(&m, width, scale, gap), None, "gap at {gap} is clickable");
        }
    }

    #[test]
    fn a_staged_update_adds_a_restart_button_left_of_the_others() {
        // The banner row is gone, so this button is the only way to take a staged
        // update from the panel — and it must not land on top of its neighbours.
        let scale = 1.5;
        let staged = model(vec![output_group(vec![dev("Speakers", None)])], Some("9.9.9"));
        let plain = model(vec![output_group(vec![dev("Speakers", None)])], None);
        assert_eq!(
            footer_buttons(&plain),
            vec![ActionKind::SoundSettings, ActionKind::RestartExplorer]
        );
        // `Restart` is transient, so it goes on the *left* end — the permanent buttons must
        // not shift under the pointer when an update lands.
        assert_eq!(
            footer_buttons(&staged),
            vec![ActionKind::SoundSettings, ActionKind::RestartExplorer, ActionKind::Restart]
        );

        let width = content_width(&staged, scale);
        let neighbour = footer_btn_center_x(width, scale, 1);
        let restart = footer_btn_center_x(width, scale, 2);
        assert!(restart < neighbour - FOOTER_BTN * scale, "buttons overlap: {restart}");
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
    fn the_restart_explorer_button_is_always_there_and_accents_only_when_needed() {
        let devs = || vec![output_group(vec![dev("Speakers", None)])];
        // Present in every state, because nothing here can tell whether the strip is really
        // drawn — a `strip_up` of true is optimistic, so hiding on it hid the button in the
        // very case it exists for.
        for m in [
            model(devs(), None),
            model_no_strip(devs(), None),
            model(devs(), Some("9.9.9")),
            model_no_strip(devs(), Some("9.9.9")),
        ] {
            assert!(footer_buttons(&m).contains(&ActionKind::RestartExplorer));
            // Offered exactly once, however many reasons apply.
            assert_eq!(
                footer_buttons(&m).iter().filter(|k| **k == ActionKind::RestartExplorer).count(),
                1
            );
        }
        // Quiet when all is well, accented once there is a reason: a known-failed injection…
        assert!(!ActionKind::RestartExplorer.is_cta(&model(devs(), None)));
        assert!(ActionKind::RestartExplorer.is_cta(&model_no_strip(devs(), None)));
        // …or an update whose TAP is stuck behind the DLL Explorer holds open.
        assert!(ActionKind::RestartExplorer.is_cta(&model(devs(), Some("9.9.9"))));
        // The gear is furniture and never shouts; `Restart` only exists when it should.
        assert!(!ActionKind::SoundSettings.is_cta(&model_no_strip(devs(), Some("9.9.9"))));
        assert!(ActionKind::Restart.is_cta(&model(devs(), Some("9.9.9"))));
    }

    #[test]
    fn three_footer_buttons_stay_clear_of_each_other_and_the_label() {
        // The widest footer there is: gear + restart-to-update + restart-Explorer. The panel
        // has to grow for it, or the buttons overlap the "Quit Audio Tray" item.
        for scale in [1.0_f32, 1.5, 2.0] {
            let m = model_no_strip(vec![output_group(vec![dev("Speakers", None)])], Some("9.9.9"));
            let buttons = footer_buttons(&m);
            assert_eq!(buttons.len(), 3);
            let width = content_width(&m, scale);
            let mut prev = f32::MAX;
            for (i, k) in buttons.iter().enumerate() {
                let cx = footer_btn_center_x(width, scale, i);
                // Each button hit-tests to itself…
                assert_eq!(footer_hit(&m, width, scale, cx.round() as i32), Some(*k));
                // …sits fully left of the one before it…
                assert!(cx < prev - FOOTER_BTN * scale, "buttons overlap at scale {scale}");
                prev = cx;
            }
            // …and the leftmost still clears the labelled item.
            let leftmost = footer_btn_center_x(width, scale, buttons.len() - 1);
            assert!(
                leftmost - FOOTER_BTN * scale / 2.0 > footer_item_right(scale),
                "the footer buttons squeeze the label at scale {scale}"
            );
        }
    }

    #[test]
    fn grid_metrics_fits_and_centres() {
        let scale = 1.5;
        let width = content_width(&model(vec![output_group(vec![])], None), scale);
        let (cols, left, chip, step) = grid_metrics(width, scale);
        assert!(cols >= 1 && cols <= IconId::ALL.len() as i32);
        assert_eq!(chip, (GRID_CHIP * scale).round() as i32);
        assert_eq!(step, chip + (GRID_GAP * scale).round() as i32);
        // The centred grid must sit inside the panel with a non-negative left inset.
        let grid_w = cols * chip + (cols - 1) * (step - chip);
        assert!(left >= 0);
        assert!(left * 2 + grid_w <= width + 1); // symmetric within rounding
    }

    #[test]
    fn grid_chip_at_hits_cells_and_misses_gaps() {
        let (width, scale, gy) = (510, 1.5, 60);
        let (cols, left, chip, step) = grid_metrics(width, scale);
        let gy0 = gy + (GRID_TOP_PAD * scale).round() as i32;
        // Centre of cell 0.
        assert_eq!(grid_chip_at(width, scale, left + chip / 2, gy0 + chip / 2, gy), Some(0));
        // Centre of cell 1 (next column), when there is more than one column.
        if cols > 1 {
            assert_eq!(grid_chip_at(width, scale, left + step + chip / 2, gy0 + chip / 2, gy), Some(1));
            // In the gap between columns 0 and 1 → no cell.
            assert_eq!(grid_chip_at(width, scale, left + chip + 1, gy0 + chip / 2, gy), None);
        }
        // Above the grid → no cell.
        assert_eq!(grid_chip_at(width, scale, left + chip / 2, gy0 - 5, gy), None);
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
    fn over_pencil_and_back_target_their_buttons() {
        let (width, scale) = (510, 1.5);
        let cx = pencil_center_x(width, scale).round() as i32;
        assert!(over_pencil(width, scale, cx));
        assert!(!over_pencil(width, scale, 10)); // far left, over the label
        let back_cx = ((BACK_LEFT + BACK_BTN / 2.0) * scale) as i32;
        assert!(over_back(scale, back_cx));
        assert!(!over_back(scale, width - 10)); // far right
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

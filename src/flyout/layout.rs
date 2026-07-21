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
use super::Trigger;

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

#[derive(Clone, Copy)]
pub(super) enum ActionKind {
    SoundSettings,
    Quit,
}

impl ActionKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            ActionKind::SoundSettings => "Sound settings",
            ActionKind::Quit => "Quit Audio Tray",
        }
    }
    pub(super) fn glyph(self) -> char {
        match self {
            ActionKind::SoundSettings => GLYPH_SETTINGS,
            ActionKind::Quit => GLYPH_CANCEL,
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
    /// A "restart to update to v…" call-to-action at the bottom of the main panel, shown
    /// only when a background update has been staged on disk.
    UpdateBanner,
    Action(ActionKind),
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
    match model.trigger {
        Trigger::RightClick => {
            for k in [ActionKind::SoundSettings, ActionKind::Quit] {
                max_w = max_w.max(TEXT_X * scale + mw(font, text_px, k.label()) + RIGHT_PAD * scale);
            }
        }
        Trigger::LeftClick => {
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
            if let Some(label) = model.update_label() {
                max_w = max_w.max(TEXT_X * scale + mw(font, text_px, &label) + RIGHT_PAD * scale);
            }
        }
    }
    let min_w = match model.trigger {
        Trigger::LeftClick => MIN_W,
        Trigger::RightClick => MENU_MIN_W,
    };
    max_w.clamp(min_w * scale, MAX_W * scale).round() as i32
}

/// The fixed panel height shared by every screen: the taller of the main panel and the icon
/// picker (in practice the main panel, which has the sliders + device lists).
pub(super) fn panel_height(model: &Model, scale: f32, width: i32) -> i32 {
    let main_h = build_view(model, scale, width, View::Main, 0).1;
    let picker_h = if matches!(model.trigger, Trigger::LeftClick)
        && model.groups.iter().any(|g| !g.devices.is_empty())
    {
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
    match (model.trigger, view) {
        (Trigger::RightClick, _) => {
            kinds.push(Elem::Action(ActionKind::SoundSettings));
            kinds.push(Elem::Action(ActionKind::Quit));
        }
        (Trigger::LeftClick, View::Main) => {
            for (gi, g) in model.groups.iter().enumerate() {
                kinds.push(Elem::Header(g.title));
                if g.default_id.is_some() {
                    kinds.push(Elem::Slider { group: gi });
                }
                for di in 0..g.devices.len() {
                    kinds.push(Elem::Device { group: gi, dev: di });
                }
            }
            // A staged update gets a restart call-to-action pinned to the very bottom.
            if model.update.is_some() {
                kinds.push(Elem::UpdateBanner);
            }
        }
        (Trigger::LeftClick, View::IconPicker { group, dev }) => {
            kinds.push(Elem::PickerHeader { group, dev });
            kinds.push(Elem::IconGrid { group, dev });
        }
    }

    let mut elems = Vec::with_capacity(kinds.len());
    let mut y = d(PAD_V);
    for (i, elem) in kinds.into_iter().enumerate() {
        let height = match elem {
            // The first header sits at the very top (small gap); a later header separates
            // one group from the one above it.
            Elem::Header(_) => d(if i == 0 { HEADER_FIRST_H } else { HEADER_H }),
            Elem::Slider { .. } => d(SLIDER_H),
            Elem::Device { .. } => d(ITEM_H),
            Elem::PickerHeader { .. } => d(PICKER_HEADER_H),
            Elem::IconGrid { .. } => grid_px_height(width, scale),
            Elem::UpdateBanner => d(ITEM_H),
            Elem::Action(_) => d(ITEM_H),
        };
        elems.push(LaidElem { elem, top: y, height });
        y += height;
    }
    let natural = y + d(PAD_V);
    let total = natural.max(fill_h);

    // Centre the icon grid in any extra vertical space, so the picker fills the shared panel
    // height without a big empty band at the bottom (the header stays pinned top).
    let slack = total - natural;
    if slack > 0 {
        for le in &mut elems {
            if matches!(le.elem, Elem::IconGrid { .. }) {
                le.top += slack / 2;
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
                | Elem::UpdateBanner
                | Elem::Action(_)
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
    use crate::flyout::Trigger;

    fn dev(label: &str, battery: Option<u8>) -> DeviceRow {
        DeviceRow {
            id: DeviceId(label.to_string()),
            label: label.to_string(),
            icon: IconId::Speakers,
            selected: false,
            battery,
        }
    }

    fn model(trigger: Trigger, groups: Vec<Group>, update: Option<&str>) -> Model {
        Model::new(trigger, groups, update.map(str::to_string))
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
            let m = model(Trigger::LeftClick, vec![output_group(vec![dev("Speakers", None)])], None);
            let w = content_width(&m, scale);
            assert!(w >= (MIN_W * scale).round() as i32, "w={w} below min at scale {scale}");
            assert!(w <= (MAX_W * scale).round() as i32, "w={w} above max at scale {scale}");
        }
    }

    #[test]
    fn content_width_empty_menu_is_menu_min() {
        // No fonts needed: with no measurable text the width collapses to the clamp floor.
        let m = model(Trigger::RightClick, vec![], None);
        assert_eq!(content_width(&m, 1.0), MENU_MIN_W.round() as i32);
    }

    #[test]
    fn grid_metrics_fits_and_centres() {
        let scale = 1.5;
        let width = content_width(&model(Trigger::LeftClick, vec![output_group(vec![])], None), scale);
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

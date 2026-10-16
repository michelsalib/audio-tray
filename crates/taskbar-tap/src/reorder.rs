//! Moving our strip along the tray: putting it between the keyboard-layout
//! indicator and the wifi/battery button.
//!
//! We cannot simply reparent our element — `Panel.Children` mutation is refused
//! (`0x800F1000`, see `FINDINGS.md`), so there is no way to insert our pill as a
//! new child of the tray's root grid. What *is* available is the layout itself:
//! the tray's sections are children of `Grid#SystemTrayFrameGrid`, placed by the
//! `Grid.Column` attached property, and that property is settable.
//!
//! So the move is a **column swap** between the section holding notify icons and
//! the one holding the language indicator.
//!
//! On 26300 the same `x:Name` is a horizontal `StackPanel`, where `Grid.Column`
//! is inert; there the move is done with margins instead (see [`shift_by_margins`]).
//!
//! Consequence worth knowing: this moves the whole notification-area section, so
//! *every* notify icon travels with ours. There is no per-icon alternative while
//! reparenting is blocked.
//!
//! Must run on the XAML UI thread.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use crate::budget::Budget;
use crate::log::logf;
use crate::tree;
use crate::winrt::{IFrameworkElement, IGridStatics, IUIElement, Thickness, GRID, VISIBILITY_COLLAPSED};
use crate::xamlom::{IXamlDiagnostics, InstanceHandle};
use windows::Win32::Foundation::S_OK;
use windows_core::Interface;

/// The tray's root panel; its children are the sections we reorder. A `Grid` up to 26200, a
/// `StackPanel` from 26300.
const FRAME_GRID: &str = "SystemTrayFrameGrid";

const STACK_PANEL: &str = "Windows.UI.Xaml.Controls.StackPanel";
const GRID_TYPE: &str = "Windows.UI.Xaml.Controls.Grid";

const ICONS_TYPE: &str = "SystemTray.NotificationAreaIcons";
const CONTROL_CENTRE: &str = "ControlCenterButton";

fn grid_statics() -> Option<IGridStatics> {
    crate::decorate::factory(GRID)
}

/// Whether the move must be kept up rather than done once: on a `StackPanel` it is margins
/// computed from live widths, which change with the language indicator and the icon count.
fn needs_upkeep() -> bool {
    frame().is_some_and(|frame| tree::type_of(frame).as_deref() == Some(STACK_PANEL))
}

/// The main taskbar's tray frame: the newest recorded one holding the notify icons. Secondary
/// taskbars have a frame of the same name, and a stale one may linger when its `Remove` never
/// arrived; an arbitrary pick would flip the margins between them.
fn frame() -> Option<InstanceHandle> {
    tree::newest(tree::find_by_name(FRAME_GRID).into_iter().filter(|&frame| {
        tree::children_of(frame)
            .iter()
            .any(|&child| tree::type_of(child).as_deref() == Some(ICONS_TYPE))
    }))
}

/// Cheap, tree-only precondition: both sections we need to reorder are recorded.
/// Checked before any COM call so the common "not yet" case costs nothing.
fn sections_ready() -> bool {
    let Some(grid) = frame() else {
        return false;
    };
    let children = tree::children_of(grid);
    let has_icons = children
        .iter()
        .any(|&c| tree::type_of(c).as_deref() == Some(ICONS_TYPE));
    let has_cc = children
        .iter()
        .any(|&c| tree::name_of(c).as_deref() == Some(CONTROL_CENTRE));
    has_icons && has_cc
}

/// The element as an `IFrameworkElement`, which is what the `Grid` statics take.
unsafe fn framework_element(
    diagnostics: &IXamlDiagnostics,
    handle: InstanceHandle,
) -> Option<IFrameworkElement> {
    let object = crate::decorate::object_from_handle(diagnostics, handle)?;
    match object.cast::<IFrameworkElement>() {
        Ok(element) => Some(element),
        Err(err) => {
            logf!("QI IFrameworkElement on 0x{handle:x} failed: {err}");
            None
        }
    }
}

unsafe fn column_of(
    statics: &IGridStatics,
    diagnostics: &IXamlDiagnostics,
    handle: InstanceHandle,
) -> Option<i32> {
    let element = framework_element(diagnostics, handle)?;
    let mut column = 0i32;
    let hr = statics.GetColumn(element.as_raw(), &mut column);
    if hr != S_OK {
        logf!("GetColumn(0x{handle:x}) -> 0x{:08x}", hr.0);
        return None;
    }
    Some(column)
}

unsafe fn set_column(
    statics: &IGridStatics,
    diagnostics: &IXamlDiagnostics,
    handle: InstanceHandle,
    column: i32,
) -> bool {
    let Some(element) = framework_element(diagnostics, handle) else {
        return false;
    };
    let hr = statics.SetColumn(element.as_raw(), column);
    if hr != S_OK {
        logf!("SetColumn(0x{handle:x}, {column}) -> 0x{:08x}", hr.0);
    }
    hr == S_OK
}

/// Puts a section back in the column it started in.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn restore_column(
    diagnostics: &IXamlDiagnostics,
    handle: InstanceHandle,
    column: i32,
) -> bool {
    let Some(statics) = grid_statics() else {
        return false;
    };
    set_column(&statics, diagnostics, handle, column)
}

/// Puts a section's margin back.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn restore_margin(
    diagnostics: &IXamlDiagnostics,
    handle: InstanceHandle,
    margin: Thickness,
) -> bool {
    framework_element(diagnostics, handle).is_some_and(|element| element.put_Margin(margin) == S_OK)
}

/// Margins this module last wrote, to tell our own value from one the shell has since set.
static WRITTEN: Mutex<Vec<(InstanceHandle, Thickness)>> = Mutex::new(Vec::new());

fn written(handle: InstanceHandle) -> Option<Thickness> {
    crate::lock(&WRITTEN)
        .iter()
        .find(|&&(known, _)| known == handle)
        .map(|&(_, margin)| margin)
}

fn note_written(handle: InstanceHandle, margin: Thickness) {
    let mut all = crate::lock(&WRITTEN);
    all.retain(|&(known, _)| known != handle);
    all.push((handle, margin));
}

fn same(a: Thickness, b: Thickness) -> bool {
    let close = |x: f64, y: f64| (x - y).abs() < 0.5;
    close(a.left, b.left) && close(a.top, b.top) && close(a.right, b.right) && close(a.bottom, b.bottom)
}

unsafe fn live_margin(diagnostics: &IXamlDiagnostics, handle: InstanceHandle) -> Option<Thickness> {
    let element = framework_element(diagnostics, handle)?;
    let mut margin = Thickness::default();
    (element.get_Margin(&mut margin) == S_OK).then_some(margin)
}

/// The margin the shell gave a section: the recorded original once we have edited it, unless the
/// shell has rewritten it since (a theme or DPI change) — then its new value is the original.
unsafe fn original_margin(diagnostics: &IXamlDiagnostics, handle: InstanceHandle) -> Option<Thickness> {
    let live = live_margin(diagnostics, handle)?;
    let Some(recorded) = crate::restore::original_margin(handle) else {
        return Some(live);
    };
    match written(handle) {
        Some(ours) if !same(live, ours) => {
            crate::restore::replace_margin(handle, live);
            Some(live)
        }
        _ => Some(recorded),
    }
}

/// The width a section takes in the panel with the shell's own margins; 0 when collapsed.
unsafe fn slot_width(diagnostics: &IXamlDiagnostics, handle: InstanceHandle) -> Option<f64> {
    let visibility = crate::decorate::element::<IUIElement>(diagnostics, handle).and_then(|element| {
        let mut value = 0i32;
        (element.get_Visibility(&mut value) == S_OK).then_some(value)
    })?;
    if visibility == VISIBILITY_COLLAPSED {
        return Some(0.0);
    }
    let (width, _) = crate::decorate::actual_size(diagnostics, handle)?;
    let margin = original_margin(diagnostics, handle)?;
    Some(width + margin.left + margin.right)
}

/// Sets a margin unless it is already there (writing it again would re-run layout every sweep),
/// recording the original first. Returns whether the margin is in place.
unsafe fn ensure_margin(diagnostics: &IXamlDiagnostics, handle: InstanceHandle, wanted: Thickness) -> bool {
    let Some(element) = framework_element(diagnostics, handle) else {
        return false;
    };
    let mut live = Thickness::default();
    if element.get_Margin(&mut live) != S_OK {
        return false;
    }
    if same(live, wanted) {
        return true;
    }
    crate::restore::remember_margin(handle, live);
    let hr = element.put_Margin(wanted);
    if hr != S_OK {
        logf!("put_Margin(0x{handle:x}) -> 0x{:08x}", hr.0);
        return false;
    }
    note_written(handle, wanted);
    true
}

/// The margins that move the icons past the sections after them on a horizontal `StackPanel`.
///
/// The icons keep their place in the panel but draw `stepped` further right and hand their slot
/// back (left `+B`, right `-B - W`: a zero-wide slot), so the sections they step over close up;
/// the control centre then makes room with `+W` on its left. `stepped` (B) is the stepped-over
/// sections' width and `icons_width` (W) the icons' own, both with the shell's margins.
fn plan_margins(
    stepped: f64,
    icons_width: f64,
    icons: Thickness,
    control_centre: Thickness,
) -> (Thickness, Thickness) {
    (
        Thickness {
            left: icons.left + stepped,
            right: icons.right - stepped - icons_width,
            ..icons
        },
        Thickness {
            left: control_centre.left + icons_width,
            ..control_centre
        },
    )
}

/// Where a reorder attempt left things.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The notify icons sit right before the control centre.
    InPlace,
    /// Something is not laid out or not readable yet; try again on a later sweep.
    Retry,
    /// This layout cannot be reordered (logged once); do not try again.
    Impossible(String),
}

/// The move on a horizontal `StackPanel`, whose children cannot be reordered or re-columned:
/// see [`plan_margins`]. Margins, not a transform: this is real layout, so the shell's icon rects
/// (`Shell_NotifyIconGetRect`, used for the wheel hook and the OSD) report where the icons are
/// drawn. Recomputed every sweep and rewritten only when a width changed.
unsafe fn shift_by_margins(diagnostics: &IXamlDiagnostics, frame: InstanceHandle) -> Outcome {
    let children = tree::children_of(frame);
    let icons = children
        .iter()
        .position(|&c| tree::type_of(c).as_deref() == Some(ICONS_TYPE));
    let control_centre = children
        .iter()
        .position(|&c| tree::name_of(c).as_deref() == Some(CONTROL_CENTRE));
    let (Some(icons), Some(control_centre)) = (icons, control_centre) else {
        return Outcome::Retry;
    };
    if icons > control_centre {
        return Outcome::InPlace; // already right of it: nothing to step over
    }

    let mut stepped = 0.0;
    for &section in &children[icons + 1..control_centre] {
        let Some(width) = slot_width(diagnostics, section) else {
            return Outcome::Retry;
        };
        stepped += width;
    }
    let icons_handle = children[icons];
    let cc_handle = children[control_centre];
    let (Some(icons_width), Some(icons_margin), Some(cc_margin)) = (
        slot_width(diagnostics, icons_handle),
        original_margin(diagnostics, icons_handle),
        original_margin(diagnostics, cc_handle),
    ) else {
        return Outcome::Retry;
    };

    let (icons_wanted, cc_wanted) = plan_margins(stepped, icons_width, icons_margin, cc_margin);
    let before = crate::restore::original_margin(icons_handle).is_some();
    if !ensure_margin(diagnostics, icons_handle, icons_wanted) {
        return Outcome::Retry;
    }
    if !ensure_margin(diagnostics, cc_handle, cc_wanted) {
        // Half a move overlaps the icons with the control centre: undo the half that landed.
        if restore_margin(diagnostics, icons_handle, icons_margin) {
            note_written(icons_handle, icons_margin);
        }
        return Outcome::Retry;
    }
    if !before {
        logf!("reorder: notify icons drawn {stepped} epx right, {CONTROL_CENTRE} +{icons_width} (StackPanel)");
    }
    Outcome::InPlace
}

/// Logs every tray section with its column — the map the reorder is planned from.
///
/// # Safety
/// XAML UI thread only.
unsafe fn report(diagnostics: &IXamlDiagnostics) {
    let Some(statics) = grid_statics() else {
        return;
    };
    let Some(grid) = frame() else {
        logf!("no {FRAME_GRID} in the tree yet");
        return;
    };
    let frame_type = tree::type_of(grid).unwrap_or_default();
    logf!("===== {FRAME_GRID} 0x{grid:x} ({frame_type}) children =====");
    for child in tree::children_of(grid) {
        let ty = tree::type_of(child).unwrap_or_default();
        let name = tree::name_of(child).unwrap_or_default();
        let column = column_of(&statics, diagnostics, child);
        logf!("  col={:?}  {ty}#{name}  [0x{child:x}]", column);
    }
    logf!("===== end sections =====");
}

/// The column moves for a `Grid` frame: every section strictly between the icons and the control
/// centre shifts one column left, and the icons take the column right before the control centre.
/// That preserves the relative order of the sections stepped over (the language indicator among
/// them) rather than swapping two of them. Returns `(index into others, new column)`; empty when
/// already in place; `Err` when the columns cannot express the move.
fn plan_columns(icons_col: i32, cc_col: i32, others: &[i32]) -> Result<Vec<(usize, i32)>, String> {
    if cc_col <= icons_col {
        return Err(format!("{CONTROL_CENTRE} at column {cc_col} is not right of the icons at {icons_col}"));
    }
    if icons_col + 1 == cc_col {
        return Ok(Vec::new());
    }
    let target = cc_col - 1;
    Ok(others
        .iter()
        .enumerate()
        .filter(|&(_, &column)| column > icons_col && column <= target)
        .map(|(index, &column)| (index, column - 1))
        .collect())
}

/// Moves the notification-area section to sit immediately before the wifi/battery
/// button, i.e. just after the keyboard-layout indicator.
///
/// # Safety
/// XAML UI thread only.
unsafe fn move_after_language(diagnostics: &IXamlDiagnostics) -> Outcome {
    let Some(grid) = frame() else {
        return Outcome::Retry;
    };
    match tree::type_of(grid).as_deref() {
        Some(STACK_PANEL) => return shift_by_margins(diagnostics, grid),
        Some(GRID_TYPE) => {}
        other => return Outcome::Impossible(format!("{FRAME_GRID} is a {}", other.unwrap_or("?"))),
    }
    let Some(statics) = grid_statics() else {
        return Outcome::Retry;
    };

    // `NotificationAreaIcons` holds the notify icons (ours among them); the control-centre button
    // is the wifi + battery group we want to end up to the right of.
    let mut icons: Option<(InstanceHandle, i32)> = None;
    let mut control_centre: Option<i32> = None;
    let mut others: Vec<(InstanceHandle, i32, String)> = Vec::new();

    for child in tree::children_of(grid) {
        let Some(column) = column_of(&statics, diagnostics, child) else {
            return Outcome::Retry;
        };
        let ty = tree::type_of(child).unwrap_or_default();
        let name = tree::name_of(child).unwrap_or_default();
        if ty == ICONS_TYPE {
            icons = Some((child, column));
            continue;
        }
        if name == CONTROL_CENTRE {
            control_centre = Some(column);
        }
        others.push((child, column, format!("{ty}#{name}")));
    }

    let (Some((icons_handle, icons_col)), Some(cc_col)) = (icons, control_centre) else {
        return Outcome::Retry;
    };
    let columns: Vec<i32> = others.iter().map(|&(_, column, _)| column).collect();
    let moves = match plan_columns(icons_col, cc_col, &columns) {
        Ok(moves) if moves.is_empty() => return Outcome::InPlace,
        Ok(moves) => moves,
        Err(why) => return Outcome::Impossible(why),
    };

    let target = cc_col - 1;
    logf!("reorder: notify icons col {icons_col} -> {target} (ControlCenterButton at {cc_col})");
    for (index, column) in moves {
        let (handle, old, label) = &others[index];
        crate::restore::remember_column(*handle, *old);
        if set_column(&statics, diagnostics, *handle, column) {
            logf!("  {label}: col {old} -> {column}");
        }
    }
    crate::restore::remember_column(icons_handle, icons_col);
    if set_column(&statics, diagnostics, icons_handle, target) {
        Outcome::InPlace
    } else {
        Outcome::Retry
    }
}

/// The frame the move was last completed for (0: none). A rebuilt frame is a new handle, so it
/// gets moved again; a `StackPanel` frame is re-checked every sweep regardless.
static DONE_FOR: AtomicU64 = AtomicU64::new(0);

static BUDGET: Budget = Budget::new("reorder");

/// The app's `[strip] next_to_wifi` switch (init data `reorder=`).
static ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
}

/// Whether the sweep should run [`apply`] now. Tree-only, so the common "nothing to do" case costs
/// no COM call.
pub fn wanted() -> bool {
    if !ENABLED.load(Ordering::SeqCst) || !BUDGET.allowed() || !sections_ready() {
        return false;
    }
    let done_for = DONE_FOR.load(Ordering::SeqCst);
    needs_upkeep() || frame().is_none_or(|frame| frame != done_for)
}

/// Whether the move was wanted but given up (failure budget spent, or impossible on this layout).
pub fn gave_up() -> bool {
    ENABLED.load(Ordering::SeqCst) && !BUDGET.allowed()
}

/// Whether nothing is left for the reorder to do (for the sweep's idle pace).
pub fn settled() -> bool {
    !ENABLED.load(Ordering::SeqCst) || !BUDGET.allowed() || frame().is_some_and(|frame| frame == DONE_FOR.load(Ordering::SeqCst))
}

/// Runs the move, logging the section map the first time and spending the failure budget on
/// every attempt that does not end in place.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn apply(diagnostics: &IXamlDiagnostics) {
    if !REPORTED.swap(true, Ordering::SeqCst) {
        report(diagnostics);
    }
    match move_after_language(diagnostics) {
        Outcome::InPlace => {
            if let Some(frame) = frame() {
                DONE_FOR.store(frame, Ordering::SeqCst);
            }
            BUDGET.record(true);
        }
        Outcome::Retry => {
            BUDGET.record(false);
            if !BUDGET.allowed() {
                undo(diagnostics);
            }
        }
        Outcome::Impossible(why) => {
            logf!("reorder: not possible on this layout — {why}; leaving the tray order alone");
            BUDGET.spend();
            undo(diagnostics);
        }
    }
}

/// After giving up: a move left half-maintained (margins that no longer match live widths) looks
/// worse than none, so put back whatever was moved.
unsafe fn undo(diagnostics: &IXamlDiagnostics) {
    crate::restore::revert_reorder(diagnostics);
    DONE_FOR.store(0, Ordering::SeqCst);
    crate::lock(&WRITTEN).clear();
}

/// Whether the app's `[strip] next_to_wifi` switch is on.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

/// Set once the section map has been logged (it only needs saying once per stand-up).
static REPORTED: AtomicBool = AtomicBool::new(false);

/// Forget everything for a fresh start after a stand-down.
pub fn forget() {
    DONE_FOR.store(0, Ordering::SeqCst);
    REPORTED.store(false, Ordering::SeqCst);
    BUDGET.reset();
    crate::lock(&WRITTEN).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(left: f64, right: f64) -> Thickness {
        Thickness { left, top: 0.0, right, bottom: 0.0 }
    }

    #[test]
    fn margins_zero_the_icons_slot_and_widen_the_control_centre() {
        let (icons, cc) = plan_margins(76.0, 72.0, t(0.0, 0.0), t(2.0, 2.0));
        assert_eq!(icons, t(76.0, -148.0));
        assert_eq!(cc, t(74.0, 2.0));
        // The icons' slot in the panel: width plus margins.
        assert_eq!(72.0 + icons.left + icons.right, 0.0);
    }

    #[test]
    fn margins_keep_the_shells_own_icon_margins() {
        let (icons, _) = plan_margins(10.0, 20.0, t(4.0, 6.0), t(0.0, 0.0));
        assert_eq!(icons, t(14.0, -24.0));
    }

    /// The 26200 map from FINDINGS, minus the icons (col 1): sections at 2..4, control centre at 5.
    #[test]
    fn columns_shift_the_stepped_over_sections_left() {
        let others = [0, 2, 3, 4, 5, 6, 7];
        let moves = plan_columns(1, 5, &others).unwrap();
        assert_eq!(moves, vec![(1, 1), (2, 2), (3, 3)]);
    }

    #[test]
    fn columns_already_adjacent_need_nothing() {
        assert_eq!(plan_columns(4, 5, &[0, 1, 2, 3, 5]).unwrap(), Vec::new());
    }

    /// 26300's `StackPanel` read through `Grid.Column`: everything is column 0. This used to plan
    /// column -1 and retry it every 250 ms forever.
    #[test]
    fn columns_that_cannot_express_the_move_are_refused() {
        assert!(plan_columns(0, 0, &[0, 0, 0]).is_err());
        assert!(plan_columns(5, 1, &[0, 2]).is_err());
    }
}

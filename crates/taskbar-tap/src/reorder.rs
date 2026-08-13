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
//! Consequence worth knowing: this moves the whole notification-area section, so
//! *every* notify icon travels with ours. There is no per-icon alternative while
//! reparenting is blocked.
//!
//! Must run on the XAML UI thread.

use crate::log::logf;
use crate::tree;
use crate::winrt::{IFrameworkElement, IGridStatics, GRID};
use crate::xamlom::{IXamlDiagnostics, InstanceHandle};
use windows::Win32::Foundation::S_OK;
use windows::Win32::System::WinRT::RoGetActivationFactory;
use windows_core::{Interface, HSTRING};

/// The tray's root grid; its children are the sections we reorder.
const FRAME_GRID: &str = "SystemTrayFrameGrid";

thread_local! {
    /// Cached because this is reached from the visual-tree callback, which fires
    /// for every element in the replay — re-acquiring the factory each time made
    /// the replay crawl. COM interfaces are not `Send`, hence thread-local rather
    /// than a static.
    static GRID_STATICS: std::cell::OnceCell<Option<IGridStatics>> = const { std::cell::OnceCell::new() };
}

fn grid_statics() -> Option<IGridStatics> {
    GRID_STATICS.with(|cell| {
        cell.get_or_init(|| match unsafe { RoGetActivationFactory(&HSTRING::from(GRID)) } {
            Ok(statics) => Some(statics),
            Err(err) => {
                logf!("RoGetActivationFactory({GRID}) failed: {err}");
                None
            }
        })
        .clone()
    })
}

/// Cheap, tree-only precondition: both sections we need to reorder are recorded.
/// Checked before any COM call so the common "not yet" case costs nothing.
pub fn sections_ready() -> bool {
    let Some(&grid) = tree::find_by_name(FRAME_GRID).first() else {
        return false;
    };
    let children = tree::children_of(grid);
    let has_icons = children
        .iter()
        .any(|&c| tree::type_of(c).as_deref() == Some("SystemTray.NotificationAreaIcons"));
    let has_cc = children
        .iter()
        .any(|&c| tree::name_of(c).as_deref() == Some("ControlCenterButton"));
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

/// Logs every tray section with its column — the map the reorder is planned from.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn report(diagnostics: &IXamlDiagnostics) {
    let Some(statics) = grid_statics() else {
        return;
    };
    let Some(&grid) = tree::find_by_name(FRAME_GRID).first() else {
        logf!("no {FRAME_GRID} in the tree yet");
        return;
    };
    logf!("===== {FRAME_GRID} 0x{grid:x} children =====");
    for child in tree::children_of(grid) {
        let ty = tree::type_of(child).unwrap_or_default();
        let name = tree::name_of(child).unwrap_or_default();
        let column = column_of(&statics, diagnostics, child);
        logf!("  col={:?}  {ty}#{name}  [0x{child:x}]", column);
    }
    logf!("===== end sections =====");
}

/// Moves the notification-area section to sit immediately before the wifi/battery
/// button, i.e. just after the keyboard-layout indicator.
///
/// Returns whether anything was changed.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn move_after_language(diagnostics: &IXamlDiagnostics) -> bool {
    let Some(statics) = grid_statics() else {
        return false;
    };
    let Some(&grid) = tree::find_by_name(FRAME_GRID).first() else {
        return false;
    };

    // The two sections we need to exchange. `NotificationAreaIcons` holds the
    // notify icons (ours among them); the control-centre button is the wifi +
    // battery group we want to end up to the right of.
    let mut icons: Option<(InstanceHandle, i32)> = None;
    let mut control_centre: Option<(InstanceHandle, i32)> = None;
    let mut sections: Vec<(InstanceHandle, i32, String)> = Vec::new();

    for child in tree::children_of(grid) {
        let Some(column) = column_of(&statics, diagnostics, child) else {
            continue;
        };
        let ty = tree::type_of(child).unwrap_or_default();
        let name = tree::name_of(child).unwrap_or_default();
        if ty == "SystemTray.NotificationAreaIcons" {
            icons = Some((child, column));
        }
        if name == "ControlCenterButton" {
            control_centre = Some((child, column));
        }
        sections.push((child, column, format!("{ty}#{name}")));
    }

    let (Some((icons_handle, icons_col)), Some((_, cc_col))) = (icons, control_centre) else {
        logf!("reorder: could not find both the notify-icon section and ControlCenterButton");
        return false;
    };
    if icons_col + 1 == cc_col {
        return false; // already immediately left of wifi/battery
    }

    // Everything strictly between the two shifts one column left; our section
    // takes the slot directly before the control centre. That preserves the
    // relative order of the sections we are stepping over (the language
    // indicator among them) rather than blindly swapping two of them.
    let target = cc_col - 1;
    logf!("reorder: notify icons col {icons_col} -> {target} (ControlCenterButton at {cc_col})");
    let mut changed = false;
    for (handle, column, label) in &sections {
        if *handle == icons_handle {
            continue;
        }
        if *column > icons_col && *column <= target {
            crate::restore::remember_column(*handle, *column);
            if set_column(&statics, diagnostics, *handle, column - 1) {
                logf!("  {label}: col {column} -> {}", column - 1);
                changed = true;
            }
        }
    }
    crate::restore::remember_column(icons_handle, icons_col);
    changed |= set_column(&statics, diagnostics, icons_handle, target);
    changed
}

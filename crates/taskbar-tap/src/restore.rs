//! The shell's original values for everything we change. **Nothing is changed before its original
//! is captured here**; [`revert`] plays them back, skipping elements that have since died.

use core::ffi::c_void;
use std::sync::{Mutex, MutexGuard};

use windows_core::IInspectable;

use crate::decorate::{self, Layout};
use crate::log::logf;
use crate::reorder;
use crate::xamlom::{IXamlDiagnostics, InstanceHandle};

#[derive(Default)]
struct Original {
    columns: Vec<(InstanceHandle, i32)>,
    layouts: Vec<(InstanceHandle, Layout)>,
    /// The presenter we took over and the content we displaced: an owned reference, so the shell's
    /// visual is not collected while out of the tree. Null is a legitimate value.
    content: Option<(InstanceHandle, usize)>,
}

/// A leaf lock, only taken on the tray thread.
static ORIGINAL: Mutex<Original> = Mutex::new(Original {
    columns: Vec::new(),
    layouts: Vec::new(),
    content: None,
});

fn lock() -> MutexGuard<'static, Original> {
    crate::lock(&ORIGINAL)
}

/// Records a section's column (already read by the reorder). First-wins: what goes back is the
/// value from before our *first* edit.
pub fn remember_column(handle: InstanceHandle, column: i32) {
    let mut original = lock();
    if original.columns.iter().any(|&(known, _)| known == handle) {
        return;
    }
    original.columns.push((handle, column));
}

/// Records an element's visibility and width. First-wins: the collapse is re-applied, and later
/// reads would record our own zero width.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn remember_layout(diagnostics: &IXamlDiagnostics, handle: InstanceHandle) {
    if lock().layouts.iter().any(|&(known, _)| known == handle) {
        return;
    }
    let Some(layout) = decorate::layout_of(diagnostics, handle) else {
        return;
    };
    lock().layouts.push((handle, layout));
}

/// Records the content we are about to displace from a presenter. Last-wins (the shell may
/// rebuild its visual, and the newest is what goes back), but **never records our own strip**,
/// or a redraw would lose the shell's visual for good.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn remember_content(diagnostics: &IXamlDiagnostics, presenter: InstanceHandle) {
    if decorate::holds_our_strip(diagnostics, presenter) {
        return;
    }
    let Some(raw) = decorate::content_of(diagnostics, presenter) else {
        return;
    };
    let previous = lock().content.replace((presenter, raw as usize));
    if let Some((_, stale)) = previous {
        release(stale);
    }
}

/// Puts everything back and forgets it. Idempotent (the record is taken).
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn revert(diagnostics: &IXamlDiagnostics) {
    let original = std::mem::take(&mut *lock());
    if original.columns.is_empty() && original.layouts.is_empty() && original.content.is_none() {
        logf!("revert: nothing was changed");
        return;
    }

    if let Some((presenter, raw)) = original.content {
        let outcome = decorate::set_content_raw(diagnostics, presenter, raw as *mut c_void);
        logf!("revert: content of presenter 0x{presenter:x} {outcome}");
        // `put_Content` took its own reference; ours is done either way.
        release(raw);
    }

    for (handle, layout) in original.layouts {
        let outcome = decorate::restore_layout(diagnostics, handle, layout);
        logf!("revert: layout of 0x{handle:x} {outcome}");
    }

    for (handle, column) in original.columns {
        let ok = reorder::restore_column(diagnostics, handle, column);
        logf!("revert: 0x{handle:x} back to column {column} = {ok}");
    }
}

/// # Safety
/// XAML UI thread only (the objects are not agile).
unsafe fn release(raw: usize) {
    if raw != 0 {
        drop(core::mem::transmute::<*mut c_void, IInspectable>(
            raw as *mut c_void,
        ));
    }
}

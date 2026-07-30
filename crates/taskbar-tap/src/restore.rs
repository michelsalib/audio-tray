//! What the shell looked like before we touched it.
//!
//! The TAP changes three things that belong to Explorer, not to us: the tray
//! icon's `ContentPresenter.Content`, the `Grid.Column` of every tray section it
//! steps over, and the visibility/width of Explorer's own volume slot. None of
//! those can be undone by unloading the DLL — `DLL_PROCESS_DETACH` runs under the
//! loader lock, on the wrong thread, while diagnostics still holds callbacks into
//! our code. The undo has to be an ordinary edit, made from the same thread that
//! made the original one.
//!
//! So every mutation site records the previous value here first, and [`revert`]
//! plays them back. The rule the module exists to enforce: **nothing is changed
//! before its original is captured.**
//!
//! Handles die. Explorer rebuilds the tray on DPI, theme and monitor changes, so
//! by the time a revert runs an element may be long gone. Every step re-resolves
//! its handle and skips what it cannot find — a missing element is not an error,
//! it is a thing that no longer needs putting back.

use core::ffi::c_void;
use std::sync::{Mutex, MutexGuard};

use windows_core::IInspectable;

use crate::decorate::{self, Layout};
use crate::log::logf;
use crate::reorder;
use crate::xamlom::{IXamlDiagnostics, InstanceHandle};

#[derive(Default)]
struct Original {
    /// Each tray section's `Grid.Column` before the reorder.
    columns: Vec<(InstanceHandle, i32)>,
    /// Visibility and width of everything we collapsed.
    layouts: Vec<(InstanceHandle, Layout)>,
    /// The presenter we took over, and the content we displaced — an owned
    /// reference, held so the shell's own visual cannot be collected while we
    /// have it out of the tree. Null is a legitimate value.
    content: Option<(InstanceHandle, usize)>,
}

/// Only ever locked from the visual-tree callback thread — both the mutation
/// sites and the revert handler run there — so this is a leaf lock with no
/// ordering to respect.
static ORIGINAL: Mutex<Original> = Mutex::new(Original {
    columns: Vec::new(),
    layouts: Vec::new(),
    content: None,
});

fn lock() -> MutexGuard<'static, Original> {
    match ORIGINAL.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Records a section's column, the first time we touch it.
///
/// Takes the value rather than reading it, because the reorder has already read
/// every section's column to plan the move — and because the recording then sits
/// in the same function as the mutation, where the two cannot drift apart.
///
/// First-wins: the reorder can run again after a tray rebuild, and it is the
/// column the section had before *our* first edit that has to go back, not the
/// one it had in between.
pub fn remember_column(handle: InstanceHandle, column: i32) {
    let mut original = lock();
    if original.columns.iter().any(|&(known, _)| known == handle) {
        return;
    }
    original.columns.push((handle, column));
}

/// Records an element's visibility and width, the first time we collapse it.
///
/// First-wins for the same reason as [`remember_column`], and it matters more
/// here: the collapse is re-applied many times to cover a layout race, and every
/// re-read after the first would record our own zero width as the original.
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

/// Records the content we are about to displace from a presenter.
///
/// Last-wins, unlike the two above. The shell data-binds this property, so it can
/// overwrite our strip with a freshly built visual of its own; when we re-apply
/// after that, the thing to put back at the end is the *new* shell visual, not
/// the stale one from the first time round.
///
/// With one exception: **never record our own strip.** Redrawing it — which a device
/// switch does, via `crate::restyle` — would otherwise make the strip itself the
/// thing we "restore", and the shell's original visual would be lost for good. This
/// is the guard that makes redrawing safe, and it is here rather than at the call
/// site so every path gets it.
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

/// Puts everything back and forgets it.
///
/// Safe to call when nothing was ever changed, and safe to call twice — the
/// record is taken, so the second call has nothing left to do. That matters
/// because the two triggers overlap: a user who toggles the feature off and then
/// quits sends both.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn revert(diagnostics: &IXamlDiagnostics) {
    let original = std::mem::take(&mut *lock());
    if original.columns.is_empty() && original.layouts.is_empty() && original.content.is_none() {
        logf!("revert: nothing was changed");
        return;
    }

    // Our strip goes first, so what follows happens behind a taskbar that no
    // longer shows it.
    if let Some((presenter, raw)) = original.content {
        let outcome = decorate::set_content_raw(diagnostics, presenter, raw as *mut c_void);
        logf!("revert: content of presenter 0x{presenter:x} {outcome}");
        // `put_Content` took its own reference; ours is done either way.
        release(raw);
    }

    // Then Explorer's own volume icon comes back...
    for (handle, layout) in original.layouts {
        let outcome = decorate::restore_layout(diagnostics, handle, layout);
        logf!("revert: layout of 0x{handle:x} {outcome}");
    }

    // ...and the tray sections return to the columns they started in.
    for (handle, column) in original.columns {
        let ok = reorder::restore_column(diagnostics, handle, column);
        logf!("revert: 0x{handle:x} back to column {column} = {ok}");
    }
}

/// Drops a reference we were holding on the shell's behalf.
///
/// # Safety
/// XAML UI thread only — the objects are not agile.
unsafe fn release(raw: usize) {
    if raw != 0 {
        drop(core::mem::transmute::<*mut c_void, IInspectable>(
            raw as *mut c_void,
        ));
    }
}

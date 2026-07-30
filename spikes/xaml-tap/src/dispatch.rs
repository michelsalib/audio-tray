//! Answering "which thread am I on?", which every failed mutation here turned on.
//!
//! The conclusion is that **nothing should be dispatched** — tray work runs
//! inline on the visual-tree callback thread. What is left in this module is the
//! measurement that establishes why, because the obvious reading of it is wrong.
//!
//! `IXamlDiagnostics::GetDispatcher` returns a `CoreDispatcher`, and asking it
//! `HasThreadAccess` from the callback thread answers `false`. That looks like
//! "you are on the wrong thread, go marshal". It is not. Explorer hosts several
//! XAML islands; that dispatcher belongs to one of the *others*. Marshalling to
//! it does land on its thread (`HasThreadAccess` becomes `true` there) and then
//! every call against a tray element fails `RPC_E_WRONG_THREAD` — while the same
//! calls succeed inline on the callback thread. Tray elements also report a null
//! `CoreDispatcher` of their own, so there is no correct queue to post to.

use crate::log::logf;
use crate::winrt::ICoreDispatcher;
use crate::xamlom::IXamlDiagnostics;
use core::ffi::c_void;
use windows::Win32::Foundation::S_OK;
use windows_core::{IInspectable, Interface};

/// The `CoreDispatcher` reported by diagnostics.
///
/// Kept for the `HasThreadAccess` measurement only — see the module note before
/// using it as a place to run anything.
///
/// # Safety
/// `diagnostics` must be live.
pub unsafe fn dispatcher(diagnostics: &IXamlDiagnostics) -> Option<ICoreDispatcher> {
    let mut raw: *mut c_void = core::ptr::null_mut();
    let hr = diagnostics.GetDispatcher(&mut raw);
    if hr != S_OK || raw.is_null() {
        logf!("GetDispatcher -> 0x{:08x}", hr.0);
        return None;
    }
    let object = core::mem::transmute::<*mut c_void, IInspectable>(raw);
    object.cast::<ICoreDispatcher>().ok()
}

/// Whether the calling thread is the one this dispatcher serves.
///
/// # Safety
/// XAML must be live.
pub unsafe fn has_thread_access(dispatcher: &ICoreDispatcher) -> Option<bool> {
    let mut value = 0u8;
    (dispatcher.get_HasThreadAccess(&mut value) == S_OK).then_some(value != 0)
}

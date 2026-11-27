//! The XAML accessors the whole TAP shares: a recorded handle back to its live element, activation
//! factories, markup loaded into elements, and the few properties read and written everywhere
//! (text, visibility, opacity, laid-out size).
//!
//! Everything here must run on the XAML UI thread; calls from any other thread fail
//! with `RPC_E_WRONG_THREAD`.

use crate::log::logf;
use crate::winrt::{IFrameworkElement, ITextBlock, ITextBlockStatics, IUIElement, IXamlReaderStatics, TEXT_BLOCK, VISIBILITY_COLLAPSED, VISIBILITY_VISIBLE, XAML_READER};
use crate::xamlom::{IXamlDiagnostics, InstanceHandle};
use core::ffi::c_void;
use windows::Win32::Foundation::S_OK;
use windows::Win32::System::WinRT::RoGetActivationFactory;
use windows_core::{IInspectable, IUnknown, Interface, HSTRING};

/// A WinRT activation factory, cached per class on the XAML thread (the sweep asks often).
/// Thread-local because interfaces are not `Send`.
pub(crate) fn factory<I: Interface>(class: &'static str) -> Option<I> {
    thread_local! {
        static CACHE: std::cell::RefCell<Vec<(&'static str, IUnknown)>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    let cached = CACHE.with(|cache| cache.borrow().iter().find(|(name, _)| *name == class).map(|(_, f)| f.clone()));
    let unknown = match cached {
        Some(unknown) => unknown,
        None => match unsafe { RoGetActivationFactory::<IUnknown>(&HSTRING::from(class)) } {
            Ok(unknown) => {
                CACHE.with(|cache| cache.borrow_mut().push((class, unknown.clone())));
                unknown
            }
            Err(err) => {
                logf!("RoGetActivationFactory({class}) failed: {err}");
                return None;
            }
        },
    };
    unknown.cast().ok()
}

/// Turn a recorded handle back into the live XAML object.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn object_from_handle(diagnostics: &IXamlDiagnostics, handle: InstanceHandle) -> Option<IInspectable> {
    let mut raw: *mut c_void = core::ptr::null_mut();
    if diagnostics.GetIInspectableFromHandle(handle, &mut raw) != S_OK || raw.is_null() {
        return None;
    }
    // Owned: dropping it releases the out-parameter's reference.
    Some(IInspectable::from_raw(raw))
}

/// The live object behind `handle`, as interface `I`.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn element<I: Interface>(diagnostics: &IXamlDiagnostics, handle: InstanceHandle) -> Option<I> {
    object_from_handle(diagnostics, handle)?.cast().ok()
}

/// An `HSTRING` as the raw handle our hand-rolled vtables take. `HSTRING` is repr(transparent) over
/// the handle; `as_ptr` would hand over the UTF-16 buffer instead. `value` must outlive the call.
pub(crate) fn hstring_abi(value: &HSTRING) -> *mut c_void {
    unsafe { core::mem::transmute_copy::<HSTRING, *mut c_void>(value) }
}

/// `XamlReader.Load`: a live element built from markup.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn load_xaml(markup: &str) -> Option<IInspectable> {
    let reader: IXamlReaderStatics = factory(XAML_READER)?;
    let markup = HSTRING::from(markup);
    let mut created: *mut c_void = core::ptr::null_mut();
    let hr = reader.Load(hstring_abi(&markup), &mut created);
    if hr != S_OK || created.is_null() {
        logf!("XamlReader.Load failed: 0x{:08x}", hr.0);
        return None;
    }
    Some(IInspectable::from_raw(created))
}

/// `TextBlock.TextProperty`, for following a text block's changes.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn text_property() -> Option<IUnknown> {
    let statics = factory::<ITextBlockStatics>(TEXT_BLOCK)?;
    let mut dp: *mut c_void = core::ptr::null_mut();
    if statics.get_TextProperty(&mut dp).is_err() || dp.is_null() {
        return None;
    }
    // Ours to release.
    Some(IUnknown::from_raw(dp))
}

/// Reads a `TextBlock`'s `Text`.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn text_of(diagnostics: &IXamlDiagnostics, handle: InstanceHandle) -> Option<String> {
    let text_block = element::<ITextBlock>(diagnostics, handle)?;
    let mut raw: *mut c_void = core::ptr::null_mut();
    if text_block.get_Text(&mut raw) != S_OK || raw.is_null() {
        return None;
    }
    let text = core::mem::transmute::<*mut c_void, HSTRING>(raw);
    Some(text.to_string())
}

/// Set a `TextBlock`'s `Text`: updates text in place without rebuilding (and re-wiring) the tile.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn set_text(diagnostics: &IXamlDiagnostics, handle: InstanceHandle, text: &str) -> bool {
    let Some(block) = element::<ITextBlock>(diagnostics, handle) else {
        return false;
    };
    let value = HSTRING::from(text);
    block.put_Text(hstring_abi(&value)) == S_OK
}

/// Show or collapse an element.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn set_visible(diagnostics: &IXamlDiagnostics, handle: InstanceHandle, visible: bool) -> bool {
    let Some(element) = element::<IUIElement>(diagnostics, handle) else {
        return false;
    };
    let value = if visible { VISIBILITY_VISIBLE } else { VISIBILITY_COLLAPSED };
    element.put_Visibility(value) == S_OK
}

/// Sets an element's opacity — how the hover plate is lit and dimmed.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn set_opacity(diagnostics: &IXamlDiagnostics, handle: InstanceHandle, opacity: f64) -> bool {
    let Some(element) = element::<IUIElement>(diagnostics, handle) else {
        return false;
    };
    let hr = element.put_Opacity(opacity);
    if hr != S_OK {
        logf!("put_Opacity({opacity}) on 0x{handle:x} -> 0x{:08x}", hr.0);
    }
    hr == S_OK
}

/// The element's laid-out width. Zero is ambiguous (removed, or not laid out yet): trust it
/// only after having seen a non-zero.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn actual_width(diagnostics: &IXamlDiagnostics, handle: InstanceHandle) -> Option<f64> {
    actual_size(diagnostics, handle).map(|(width, _)| width)
}

/// Laid-out size of an element, once layout has actually run.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn actual_size(diagnostics: &IXamlDiagnostics, handle: InstanceHandle) -> Option<(f64, f64)> {
    let framework = element::<IFrameworkElement>(diagnostics, handle)?;
    let mut width = 0.0f64;
    let mut height = 0.0f64;
    (framework.get_ActualWidth(&mut width) == S_OK && framework.get_ActualHeight(&mut height) == S_OK).then_some((width, height))
}

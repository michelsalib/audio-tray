//! On-demand enumeration of the visual tree — a dead end, kept as a probe.
//!
//! This module was written under a wrong conclusion: that
//! `AdviseVisualTreeChange` only ever streams live mutations, because attaching
//! to a settled Explorer delivered nothing but two empty roots. The real cause
//! was that the callback implemented only `IVisualTreeServiceCallback`. With the
//! **v2** callback the same call replays the entire tree (280+ elements), so
//! none of this manual enumeration is needed.
//!
//! What the probes below did establish, and what is still true on Win11 26200:
//!
//!   * `IVisualTreeService`'s property/collection API is a **dead end**.
//!     `GetPropertyIndex` returns `E_INVALIDARG` (0x80070057) for every property
//!     name on every handle, `GetCollectionCount` returns `E_NOTFOUND`
//!     (0x80070490), and `IXamlDiagnostics::HitTest` returns `E_INVALIDARG`.
//!     These are real, considered error codes — the calls execute, the vtables
//!     are correct — so this is Explorer's XAML declining to implement them, not
//!     a binding bug. `probe_properties` below exists to keep proving that.
//!
//!   * `IXamlDiagnostics::GetIInspectableFromHandle` **works**, returning live
//!     XAML objects (`GetRuntimeClassName` reports `Windows.UI.Xaml.Controls.Grid`
//!     for the UI layer). That is the supported route, and the one the C++ TAPs
//!     use: convert a handle to an object, then drive it with the ordinary WinRT
//!     XAML API.
//!
//! Everything here must run on the XAML UI thread. `IVisualTreeService` is not
//! agile; calling it from a worker thread is how you corrupt explorer.

// Retained as a regression probe: these routes are non-functional in Explorer today
// (see FINDINGS.md), so nothing calls them, but re-running them is how we'd notice
// a future Windows build changing that.
#![allow(dead_code)]

use crate::log::logf;
use crate::wide;
use crate::xamlom::{
    bstr_to_string, CollectionElementValue, IVisualTreeService3, IXamlDiagnostics, InstanceHandle,
    IS_VALUE_HANDLE,
};
use core::ffi::c_void;
use windows::Win32::Foundation::{RECT, S_OK};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowRect};
use windows_core::Interface;

/// Total elements a single walk will log, so a pathological tree can't spin
/// inside the shell's UI thread.
const BUDGET: u32 = 4000;
const MAX_DEPTH: usize = 40;

/// Property names worth trying when looking for a way further into the tree.
const CANDIDATE_PROPERTIES: &[&str] = &[
    "Children", "Parent", "Content", "Child", "Items", "RootVisual", "XamlRoot",
];

/// Explores the tree from every root we can reach and logs what it finds.
///
/// # Safety
/// Must be called on the XAML UI thread with a live site.
pub unsafe fn explore(
    diagnostics: &IXamlDiagnostics,
    service: &IVisualTreeService3,
    feed_roots: &[InstanceHandle],
) {
    logf!("===== exploring =====");

    // 1. The documented entry point.
    let mut layer: *mut c_void = core::ptr::null_mut();
    let hr = diagnostics.GetUiLayer(&mut layer);
    if hr == S_OK && !layer.is_null() {
        let owned = core::mem::transmute::<*mut c_void, windows_core::IUnknown>(layer);
        let mut handle: InstanceHandle = 0;
        if diagnostics.GetHandleFromIInspectable(owned.as_raw(), &mut handle) == S_OK {
            logf!("-- UI layer handle 0x{handle:x}");
            probe_inspectable(diagnostics, handle);
            probe_properties(service, handle);
            walk(service, handle, "UiLayer", 0, &mut BUDGET.clone());
        }
    } else {
        logf!("GetUiLayer failed: 0x{:08x}", hr.0);
    }

    // 2. The roots the live feed handed us during Advise.
    for &root in feed_roots {
        logf!("-- feed root 0x{root:x}");
        probe_inspectable(diagnostics, root);
        probe_properties(service, root);
    }

    // 3. Hit-test the taskbar. This is the one that matters for M2: it lands
    //    directly on the elements we eventually want to decorate.
    hit_test_taskbar(diagnostics, service);

    logf!("===== end of exploration =====");
}

unsafe fn hit_test_taskbar(diagnostics: &IXamlDiagnostics, service: &IVisualTreeService3) {
    let class = wide("Shell_TrayWnd");
    let Ok(tray) = FindWindowW(windows_core::PCWSTR(class.as_ptr()), None) else {
        logf!("Shell_TrayWnd not found");
        return;
    };
    let mut rect = RECT::default();
    if GetWindowRect(tray, &mut rect).is_err() {
        logf!("GetWindowRect failed");
        return;
    }
    logf!(
        "-- hit-testing taskbar rect {},{} {}x{}",
        rect.left,
        rect.top,
        rect.right - rect.left,
        rect.bottom - rect.top
    );

    let mut count = 0u32;
    let mut handles: *mut InstanceHandle = core::ptr::null_mut();
    let hr = diagnostics.HitTest(rect, &mut count, &mut handles);
    logf!("HitTest -> 0x{:08x} count={count}", hr.0);
    if hr != S_OK || handles.is_null() || count == 0 {
        return;
    }

    let hits = core::slice::from_raw_parts(handles, count as usize);
    for (n, &handle) in hits.iter().enumerate().take(8) {
        logf!("hit[{n}] 0x{handle:x}");
        probe_properties(service, handle);
    }
    CoTaskMemFree(Some(handles as *const c_void));
}

/// Turns a handle back into the live XAML object and asks it what it is.
///
/// This is the route the plan actually depends on — and, unlike the
/// property/collection APIs, the route the known-good C++ TAPs use.
unsafe fn probe_inspectable(diagnostics: &IXamlDiagnostics, handle: InstanceHandle) {
    let mut raw: *mut c_void = core::ptr::null_mut();
    let hr = diagnostics.GetIInspectableFromHandle(handle, &mut raw);
    if hr != S_OK || raw.is_null() {
        logf!("   [obj] GetIInspectableFromHandle -> 0x{:08x}", hr.0);
        return;
    }
    let object = core::mem::transmute::<*mut c_void, windows_core::IInspectable>(raw);
    match object.GetRuntimeClassName() {
        Ok(name) => logf!("   [obj] live XAML object, runtime class = {name}"),
        Err(err) => logf!("   [obj] got object but GetRuntimeClassName failed: {err}"),
    }
}

/// Logs which of the candidate properties actually resolve on a handle, and what
/// they point at. This is how we learn the shape of the tree rather than guess.
pub unsafe fn probe_properties(service: &IVisualTreeService3, handle: InstanceHandle) {
    // A v1-only call first. If this works while the v2/v3 calls below do not,
    // the fault is in how the inherited vtables are chained, not in the handles.
    let mut v1_count = 0u32;
    let v1_hr = service.GetCollectionCount(handle, &mut v1_count);
    logf!("   [v1] GetCollectionCount -> 0x{:08x} count={v1_count}", v1_hr.0);

    for name in CANDIDATE_PROPERTIES {
        let wide_name = wide(name);
        let mut index = 0u32;
        let hr = service.GetPropertyIndex(handle, wide_name.as_ptr(), &mut index);
        if hr != S_OK {
            logf!("   .{name}: GetPropertyIndex -> 0x{:08x}", hr.0);
            continue;
        }
        let mut value: InstanceHandle = 0;
        let value_hr = service.GetProperty(handle, index, &mut value);
        let mut count = 0u32;
        let count_hr = service.GetCollectionCount(value, &mut count);
        logf!(
            "   .{name}: idx={index} value=0x{value:x} (0x{:08x}) count={count} (0x{:08x})",
            value_hr.0,
            count_hr.0
        );
    }
}

unsafe fn walk(
    service: &IVisualTreeService3,
    handle: InstanceHandle,
    type_name: &str,
    depth: usize,
    budget: &mut u32,
) {
    if *budget == 0 || depth > MAX_DEPTH {
        return;
    }
    *budget -= 1;
    logf!("{}{type_name}  [0x{handle:x}]", "  ".repeat(depth));

    for (child, child_type) in children(service, handle) {
        walk(service, child, &child_type, depth + 1, budget);
    }
}

/// Reads an element's `Children` collection as `(handle, type name)` pairs.
unsafe fn children(
    service: &IVisualTreeService3,
    handle: InstanceHandle,
) -> Vec<(InstanceHandle, String)> {
    let property = wide("Children");
    let mut index = 0u32;
    if service.GetPropertyIndex(handle, property.as_ptr(), &mut index) != S_OK {
        return Vec::new();
    }

    let mut collection: InstanceHandle = 0;
    if service.GetProperty(handle, index, &mut collection) != S_OK || collection == 0 {
        return Vec::new();
    }

    let mut count = 0u32;
    if service.GetCollectionCount(collection, &mut count) != S_OK || count == 0 {
        return Vec::new();
    }

    let mut returned = count;
    let mut values: *mut c_void = core::ptr::null_mut();
    if service.GetCollectionElements(collection, 0, &mut returned, &mut values) != S_OK
        || values.is_null()
    {
        return Vec::new();
    }

    let items = core::slice::from_raw_parts(values as *const CollectionElementValue, returned as _);
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let value_type = bstr_to_string(item.value_type);
        let value = bstr_to_string(item.value);
        if item.metadata_bits & IS_VALUE_HANDLE != 0 {
            match parse_handle(&value) {
                Some(child) => out.push((child, value_type)),
                None => logf!("unparsable child handle {value:?} (type {value_type:?})"),
            }
        }
        // The struct's BSTRs are ours to free; `BSTR::from_raw` + drop does it.
        drop(windows_core::BSTR::from_raw(item.value_type));
        drop(windows_core::BSTR::from_raw(item.value));
    }
    CoTaskMemFree(Some(values as *const c_void));
    out
}

/// The collection API renders handles as strings; the exact base is undocumented,
/// so accept decimal and hex.
fn parse_handle(value: &str) -> Option<InstanceHandle> {
    let trimmed = value.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        return u64::from_str_radix(hex, 16).ok();
    }
    trimmed
        .parse::<u64>()
        .ok()
        .or_else(|| u64::from_str_radix(trimmed, 16).ok())
}

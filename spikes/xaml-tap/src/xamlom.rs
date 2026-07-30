//! Hand-rolled bindings for `xamlOM.h` (the XAML Diagnostics object model).
//!
//! These are NOT available in windows-rs. Neither `windows` 0.61 nor 0.62 ships a
//! `Win32_UI_Xaml_Diagnostics` feature — the win32metadata that generates the crate
//! simply does not cover `xamlOM.h`, so there is nothing to turn on. Everything
//! below is transcribed by hand from
//! `%ProgramFiles(x86)%\Windows Kits\10\Include\10.0.26100.0\um\xamlOM.h`.
//!
//! Layout rules that matter, because a mistake here corrupts explorer.exe rather
//! than failing to compile:
//!   * every struct is `#[repr(C)]` and mirrors the header field-for-field,
//!   * `InstanceHandle` is `MIDL_uhyper` = `u64`,
//!   * `BSTR`s that arrive as *by-value in-params* are modelled as raw `*mut u16`,
//!     never `windows_core::BSTR`. The caller owns them; a real `BSTR` would run
//!     `SysFreeString` in its `Drop` and hand explorer a double free.

// Method names are kept exactly as `xamlOM.h` spells them so this file can be
// diffed against the header.
#![allow(non_snake_case)]

use core::ffi::c_void;
// `IUnknown_Vtbl` looks unused but is not: the `#[interface]` macro names it
// unqualified when it builds each vtable, so it has to be in scope here.
use windows_core::{interface, IUnknown, IUnknown_Vtbl, HRESULT};

/// `typedef MIDL_uhyper InstanceHandle;`
pub type InstanceHandle = u64;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SourceInfo {
    pub file_name: *mut u16,
    pub line_number: u32,
    pub column_number: u32,
    pub char_position: u32,
    pub hash: *mut u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ParentChildRelation {
    pub parent: InstanceHandle,
    pub child: InstanceHandle,
    pub child_index: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VisualElement {
    pub handle: InstanceHandle,
    pub src_info: SourceInfo,
    pub type_name: *mut u16,
    pub name: *mut u16,
    pub num_children: u32,
}

#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct VisualMutationType(pub i32);

impl VisualMutationType {
    pub const ADD: Self = Self(0);
    pub const REMOVE: Self = Self(1);
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CollectionElementValue {
    pub index: u32,
    pub value_type: *mut u16,
    pub value: *mut u16,
    pub metadata_bits: i64,
}

/// `MetadataBit::IsValueHandle` — the element's `Value` string is an
/// `InstanceHandle` rather than a literal.
pub const IS_VALUE_HANDLE: i64 = 0x1;

/// `enum ResourceType`
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct ResourceType(pub i32);

/// `enum RenderTargetBitmapOptions`
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct RenderTargetBitmapOptions(pub i32);

#[interface("AA7A8931-80E4-4FEC-8F3B-553F87B4966E")]
pub unsafe trait IVisualTreeServiceCallback: IUnknown {
    pub fn OnVisualTreeChange(
        &self,
        relation: ParentChildRelation,
        element: VisualElement,
        mutation_type: VisualMutationType,
    ) -> HRESULT;
}

/// `enum VisualElementState`
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct VisualElementState(pub i32);

/// The v2 callback. Explorer's XAML may hand a richer session to consumers that
/// support it — the known-good C++ TAPs all implement this, not just v1.
#[interface("BAD9EB88-AE77-4397-B948-5FA2DB0A19EA")]
pub unsafe trait IVisualTreeServiceCallback2: IVisualTreeServiceCallback {
    pub fn OnElementStateChanged(
        &self,
        element: InstanceHandle,
        element_state: VisualElementState,
        context: *const u16,
    ) -> HRESULT;
}

#[interface("A593B11A-D17F-48BB-8F66-83910731C8A5")]
pub unsafe trait IVisualTreeService: IUnknown {
    pub fn AdviseVisualTreeChange(&self, callback: *mut c_void) -> HRESULT;
    pub fn UnadviseVisualTreeChange(&self, callback: *mut c_void) -> HRESULT;
    pub fn GetEnums(&self, count: *mut u32, enums: *mut *mut c_void) -> HRESULT;
    pub fn CreateInstance(
        &self,
        type_name: *mut u16,
        value: *mut u16,
        instance_handle: *mut InstanceHandle,
    ) -> HRESULT;
    pub fn GetPropertyValuesChain(
        &self,
        instance_handle: InstanceHandle,
        source_count: *mut u32,
        property_sources: *mut *mut c_void,
        property_count: *mut u32,
        property_values: *mut *mut c_void,
    ) -> HRESULT;
    pub fn SetProperty(
        &self,
        instance_handle: InstanceHandle,
        value: InstanceHandle,
        property_index: u32,
    ) -> HRESULT;
    pub fn ClearProperty(&self, instance_handle: InstanceHandle, property_index: u32) -> HRESULT;
    pub fn GetCollectionCount(
        &self,
        instance_handle: InstanceHandle,
        collection_size: *mut u32,
    ) -> HRESULT;
    pub fn GetCollectionElements(
        &self,
        instance_handle: InstanceHandle,
        start_index: u32,
        element_count: *mut u32,
        element_values: *mut *mut c_void,
    ) -> HRESULT;
    pub fn AddChild(&self, parent: InstanceHandle, child: InstanceHandle, index: u32) -> HRESULT;
    pub fn RemoveChild(&self, parent: InstanceHandle, index: u32) -> HRESULT;
    pub fn ClearChildren(&self, parent: InstanceHandle) -> HRESULT;
}

#[interface("130F5136-EC43-4F61-89C7-9801A36D2E95")]
pub unsafe trait IVisualTreeService2: IVisualTreeService {
    pub fn GetPropertyIndex(
        &self,
        object: InstanceHandle,
        property_name: *const u16,
        property_index: *mut u32,
    ) -> HRESULT;
    pub fn GetProperty(
        &self,
        object: InstanceHandle,
        property_index: u32,
        value: *mut InstanceHandle,
    ) -> HRESULT;
    pub fn ReplaceResource(
        &self,
        resource_dictionary: InstanceHandle,
        key: InstanceHandle,
        new_value: InstanceHandle,
    ) -> HRESULT;
    pub fn RenderTargetBitmap(
        &self,
        handle: InstanceHandle,
        options: RenderTargetBitmapOptions,
        max_pixel_width: u32,
        max_pixel_height: u32,
        bitmap_data: *mut *mut c_void,
    ) -> HRESULT;
}

#[interface("0E79C6E0-85A0-4BE8-B41A-655CF1FD19BD")]
pub unsafe trait IVisualTreeService3: IVisualTreeService2 {
    pub fn ResolveResource(
        &self,
        resource_context: InstanceHandle,
        resource_name: *const u16,
        resource_type: ResourceType,
        property_index: u32,
    ) -> HRESULT;
    pub fn GetDictionaryItem(
        &self,
        dictionary_handle: InstanceHandle,
        resource_name: *const u16,
        resource_is_implicit_style: windows_core::BOOL,
        resource_handle: *mut InstanceHandle,
    ) -> HRESULT;
    pub fn AddDictionaryItem(
        &self,
        dictionary_handle: InstanceHandle,
        resource_key: InstanceHandle,
        resource_handle: InstanceHandle,
    ) -> HRESULT;
    pub fn RemoveDictionaryItem(
        &self,
        dictionary_handle: InstanceHandle,
        resource_key: InstanceHandle,
    ) -> HRESULT;
}

#[interface("18C9E2B6-3F43-4116-9F2B-FF935D7770D2")]
pub unsafe trait IXamlDiagnostics: IUnknown {
    pub fn GetDispatcher(&self, dispatcher: *mut *mut c_void) -> HRESULT;
    pub fn GetUiLayer(&self, layer: *mut *mut c_void) -> HRESULT;
    pub fn GetApplication(&self, application: *mut *mut c_void) -> HRESULT;
    pub fn GetIInspectableFromHandle(
        &self,
        instance_handle: InstanceHandle,
        instance: *mut *mut c_void,
    ) -> HRESULT;
    pub fn GetHandleFromIInspectable(
        &self,
        instance: *mut c_void,
        handle: *mut InstanceHandle,
    ) -> HRESULT;
    pub fn HitTest(
        &self,
        rect: windows::Win32::Foundation::RECT,
        count: *mut u32,
        instance_handles: *mut *mut InstanceHandle,
    ) -> HRESULT;
    pub fn RegisterInstance(
        &self,
        instance: *mut c_void,
        instance_handle: *mut InstanceHandle,
    ) -> HRESULT;
    pub fn GetInitializationData(&self, initialization_data: *mut *mut u16) -> HRESULT;
}

/// Reads a caller-owned `BSTR` without taking ownership of it.
///
/// A `BSTR` stores its byte length in the 4 bytes preceding the pointer, so this
/// is exact (and tolerates embedded NULs) without calling `SysStringLen`.
///
/// # Safety
/// `p` must be null or a live `BSTR`. The returned `String` is a copy; ownership
/// of `p` stays with the caller.
pub unsafe fn bstr_to_string(p: *mut u16) -> String {
    /// XAML type and element names are short. A length beyond this means the
    /// pointer is not a `BSTR`, and reading it would fault inside explorer.
    const SANITY_LIMIT: u32 = 64 * 1024;

    if p.is_null() {
        return String::new();
    }
    let bytes = *(p as *const u32).offset(-1);
    if bytes > SANITY_LIMIT {
        return format!("<bogus BSTR: {bytes} bytes>");
    }
    let len = (bytes / 2) as usize;
    String::from_utf16_lossy(core::slice::from_raw_parts(p, len))
}

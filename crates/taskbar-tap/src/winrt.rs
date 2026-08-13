//! Hand-rolled WinRT projections for the bits of `Windows.UI.Xaml` we need.
//!
//! windows-rs 0.61 does not project `Windows.UI.Xaml` any more than it projects
//! `xamlOM.h`, so these are transcribed from
//! `%ProgramFiles(x86)%\Windows Kits\10\Include\10.0.26100.0\winrt\windows.ui.xaml*.h`.
//!
//! Only the slots we actually call carry real signatures. The rest exist purely
//! to occupy their vtable position — a function pointer is a function pointer,
//! so an unused slot's parameter types are irrelevant as long as the *count* and
//! *order* match the header exactly.
//!
//! Both interfaces derive from `IInspectable` in the header, but `#[interface]`
//! can't express that: it wants the parent's `_Impl` trait, and windows-core has
//! no `IInspectable_Impl` because `IInspectable` isn't implementable that way.
//! We only ever *call* these, so the parent is declared as `IUnknown` and
//! `IInspectable`'s three slots are spelled out at the top instead. The vtable
//! layout is byte-identical either way.

#![allow(non_snake_case)]

use core::ffi::c_void;
use windows_core::{interface, IUnknown, IUnknown_Vtbl, HRESULT};

// The three `IInspectable` slots are written out at the top of each trait below;
// they can't be factored into a macro because `#[interface]` parses the trait
// body before any `macro_rules!` expansion would happen.

/// `Windows.Foundation.Point` / `Rect`, needed only so the unused `Find*` slots
/// carry FFI-safe signatures.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// `Windows.UI.Xaml.IDependencyObject`. We never call its methods — it is the
/// currency `IVisualTreeHelperStatics` deals in, so we only need its IID.
#[interface("5c526665-f60e-4912-af59-5fe0680f089d")]
pub unsafe trait IDependencyObject: IUnknown {
    pub fn GetIids(&self, count: *mut u32, iids: *mut *mut windows_core::GUID) -> HRESULT;
    pub fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    pub fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    pub fn GetValue(&self, dp: *mut c_void, value: *mut *mut c_void) -> HRESULT;
    pub fn SetValue(&self, dp: *mut c_void, value: *mut c_void) -> HRESULT;
    pub fn ClearValue(&self, dp: *mut c_void) -> HRESULT;
    pub fn ReadLocalValue(&self, dp: *mut c_void, value: *mut *mut c_void) -> HRESULT;
    pub fn GetAnimationBaseValue(&self, dp: *mut c_void, value: *mut *mut c_void) -> HRESULT;
    pub fn RegisterPropertyChangedCallback(
        &self,
        dp: *mut c_void,
        callback: *mut c_void,
        token: *mut i64,
    ) -> HRESULT;
    pub fn UnregisterPropertyChangedCallback(&self, dp: *mut c_void, token: i64) -> HRESULT;
    pub fn get_Dispatcher(&self, value: *mut *mut c_void) -> HRESULT;
}

/// `Windows.UI.Xaml.Media.IVisualTreeHelperStatics`.
///
/// The four `Find*` slots come first in the header and are unused here.
#[interface("e75758c4-d25d-4b1d-971f-596f17f12baa")]
pub unsafe trait IVisualTreeHelperStatics: IUnknown {
    pub fn GetIids(&self, count: *mut u32, iids: *mut *mut windows_core::GUID) -> HRESULT;
    pub fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    pub fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    pub fn FindElementsInHostCoordinatesPoint(
        &self,
        point: Point,
        subtree: *mut c_void,
        result: *mut *mut c_void,
    ) -> HRESULT;
    pub fn FindElementsInHostCoordinatesRect(
        &self,
        rect: Rect,
        subtree: *mut c_void,
        result: *mut *mut c_void,
    ) -> HRESULT;
    pub fn FindAllElementsInHostCoordinatesPoint(
        &self,
        point: Point,
        subtree: *mut c_void,
        include_all: u8,
        result: *mut *mut c_void,
    ) -> HRESULT;
    pub fn FindAllElementsInHostCoordinatesRect(
        &self,
        rect: Rect,
        subtree: *mut c_void,
        include_all: u8,
        result: *mut *mut c_void,
    ) -> HRESULT;
    pub fn GetChild(
        &self,
        reference: *mut c_void,
        child_index: i32,
        result: *mut *mut c_void,
    ) -> HRESULT;
    pub fn GetChildrenCount(&self, reference: *mut c_void, result: *mut i32) -> HRESULT;
    pub fn GetParent(&self, reference: *mut c_void, result: *mut *mut c_void) -> HRESULT;
    pub fn DisconnectChildrenRecursive(&self, element: *mut c_void) -> HRESULT;
}

/// `Windows.UI.Xaml.Visibility`.
#[allow(dead_code)] // the other half of the enum; kept so the pair reads correctly
pub const VISIBILITY_VISIBLE: i32 = 0;
pub const VISIBILITY_COLLAPSED: i32 = 1;

/// `Windows.UI.Xaml.IUIElement`.
///
/// Only the members this spike calls are named; every other slot is a
/// placeholder purely to keep the vtable offsets right. The numbering follows
/// the SDK header — `get_Opacity` is slot 9, `get_Visibility` 21, the pointer
/// and tap events 57..80 — and a miscount here calls the wrong function
/// pointer, which is how `GetColumn` once hung the shell.
#[interface("676d0be9-b65c-41c6-ba40-58cf87f201c1")]
pub unsafe trait IUIElement: IUnknown {
    pub fn GetIids(&self, count: *mut u32, iids: *mut *mut windows_core::GUID) -> HRESULT;
    pub fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    pub fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    pub fn _reserved06(&self) -> HRESULT;
    pub fn _reserved07(&self) -> HRESULT;
    pub fn _reserved08(&self) -> HRESULT;
    pub fn get_Opacity(&self, value: *mut f64) -> HRESULT;
    pub fn put_Opacity(&self, value: f64) -> HRESULT;
    pub fn _reserved11(&self) -> HRESULT;
    pub fn _reserved12(&self) -> HRESULT;
    pub fn _reserved13(&self) -> HRESULT;
    pub fn _reserved14(&self) -> HRESULT;
    pub fn _reserved15(&self) -> HRESULT;
    pub fn _reserved16(&self) -> HRESULT;
    pub fn _reserved17(&self) -> HRESULT;
    pub fn _reserved18(&self) -> HRESULT;
    pub fn _reserved19(&self) -> HRESULT;
    pub fn _reserved20(&self) -> HRESULT;
    pub fn get_Visibility(&self, value: *mut i32) -> HRESULT;
    pub fn put_Visibility(&self, value: i32) -> HRESULT;
    // Slots 23..56 — RenderSize, transitions, drag/drop and friends.
    pub fn _reserved23(&self) -> HRESULT;
    pub fn _reserved24(&self) -> HRESULT;
    pub fn _reserved25(&self) -> HRESULT;
    pub fn _reserved26(&self) -> HRESULT;
    pub fn _reserved27(&self) -> HRESULT;
    pub fn _reserved28(&self) -> HRESULT;
    pub fn _reserved29(&self) -> HRESULT;
    pub fn _reserved30(&self) -> HRESULT;
    pub fn _reserved31(&self) -> HRESULT;
    pub fn _reserved32(&self) -> HRESULT;
    pub fn _reserved33(&self) -> HRESULT;
    pub fn _reserved34(&self) -> HRESULT;
    pub fn _reserved35(&self) -> HRESULT;
    pub fn _reserved36(&self) -> HRESULT;
    pub fn _reserved37(&self) -> HRESULT;
    pub fn _reserved38(&self) -> HRESULT;
    pub fn _reserved39(&self) -> HRESULT;
    pub fn _reserved40(&self) -> HRESULT;
    pub fn _reserved41(&self) -> HRESULT;
    pub fn _reserved42(&self) -> HRESULT;
    pub fn _reserved43(&self) -> HRESULT;
    pub fn _reserved44(&self) -> HRESULT;
    pub fn _reserved45(&self) -> HRESULT;
    pub fn _reserved46(&self) -> HRESULT;
    pub fn _reserved47(&self) -> HRESULT;
    pub fn _reserved48(&self) -> HRESULT;
    pub fn _reserved49(&self) -> HRESULT;
    pub fn _reserved50(&self) -> HRESULT;
    pub fn _reserved51(&self) -> HRESULT;
    pub fn _reserved52(&self) -> HRESULT;
    pub fn _reserved53(&self) -> HRESULT;
    pub fn _reserved54(&self) -> HRESULT;
    pub fn _reserved55(&self) -> HRESULT;
    pub fn _reserved56(&self) -> HRESULT;
    pub fn add_PointerPressed(&self, handler: *mut c_void, token: *mut i64) -> HRESULT;
    pub fn remove_PointerPressed(&self, token: i64) -> HRESULT;
    pub fn add_PointerMoved(&self, handler: *mut c_void, token: *mut i64) -> HRESULT;
    pub fn remove_PointerMoved(&self, token: i64) -> HRESULT;
    pub fn add_PointerReleased(&self, handler: *mut c_void, token: *mut i64) -> HRESULT;
    pub fn remove_PointerReleased(&self, token: i64) -> HRESULT;
    pub fn add_PointerEntered(&self, handler: *mut c_void, token: *mut i64) -> HRESULT;
    pub fn remove_PointerEntered(&self, token: i64) -> HRESULT;
    pub fn add_PointerExited(&self, handler: *mut c_void, token: *mut i64) -> HRESULT;
    pub fn remove_PointerExited(&self, token: i64) -> HRESULT;
    pub fn add_PointerCaptureLost(&self, handler: *mut c_void, token: *mut i64) -> HRESULT;
    pub fn remove_PointerCaptureLost(&self, token: i64) -> HRESULT;
    pub fn add_PointerCanceled(&self, handler: *mut c_void, token: *mut i64) -> HRESULT;
    pub fn remove_PointerCanceled(&self, token: i64) -> HRESULT;
    pub fn add_PointerWheelChanged(&self, handler: *mut c_void, token: *mut i64) -> HRESULT;
    pub fn remove_PointerWheelChanged(&self, token: i64) -> HRESULT;
    pub fn add_Tapped(&self, handler: *mut c_void, token: *mut i64) -> HRESULT;
    pub fn remove_Tapped(&self, token: i64) -> HRESULT;
    pub fn add_DoubleTapped(&self, handler: *mut c_void, token: *mut i64) -> HRESULT;
    pub fn remove_DoubleTapped(&self, token: i64) -> HRESULT;
    pub fn add_Holding(&self, handler: *mut c_void, token: *mut i64) -> HRESULT;
    pub fn remove_Holding(&self, token: i64) -> HRESULT;
    pub fn add_RightTapped(&self, handler: *mut c_void, token: *mut i64) -> HRESULT;
    pub fn remove_RightTapped(&self, token: i64) -> HRESULT;
}

// Pointer event args, for the one handler that reads them. Hover and the two tap handlers
// ignore theirs entirely — which element was hit and which handler fired is all they need —
// but a scroll has to get at the wheel delta, and that sits two hops down: the args yield a
// `PointerPoint`, whose properties carry it.

/// `Windows.UI.Xaml.Input.IPointerRoutedEventArgs` — the args of `PointerWheelChanged`
/// (and of every other pointer event, whose args we never look at).
///
/// Needed for two things a scroll cannot do without. The delta is *not* on the args — it
/// comes from `GetCurrentPoint(null).Properties.MouseWheelDelta`, which is why
/// [`IPointerPoint`] and [`IPointerPointProperties`] are transcribed below as well — and
/// `put_Handled` is how a scroll we have acted on is kept from also reaching the shell.
///
/// Slot order is the header's: the three `IInspectable` ones, then `Pointer`,
/// `KeyModifiers`, `Handled` (get and put), and only then `GetCurrentPoint`.
#[interface("da628f0a-9752-49e2-bde2-49eccab9194d")]
pub unsafe trait IPointerRoutedEventArgs: IUnknown {
    pub fn GetIids(&self, count: *mut u32, iids: *mut *mut windows_core::GUID) -> HRESULT;
    pub fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    pub fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    pub fn get_Pointer(&self, value: *mut *mut c_void) -> HRESULT;
    pub fn get_KeyModifiers(&self, value: *mut i32) -> HRESULT;
    pub fn get_Handled(&self, value: *mut u8) -> HRESULT;
    pub fn put_Handled(&self, value: u8) -> HRESULT;
    pub fn GetCurrentPoint(
        &self,
        relative_to: *mut c_void,
        result: *mut *mut c_void,
    ) -> HRESULT;
    pub fn GetIntermediatePoints(
        &self,
        relative_to: *mut c_void,
        result: *mut *mut c_void,
    ) -> HRESULT;
}

/// `Windows.UI.Input.IPointerPoint`. Only `get_Properties` is called; the seven slots
/// before it are the header's own order.
#[interface("e995317d-7296-42d9-8233-c5be73b74a4a")]
pub unsafe trait IPointerPoint: IUnknown {
    pub fn GetIids(&self, count: *mut u32, iids: *mut *mut windows_core::GUID) -> HRESULT;
    pub fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    pub fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    pub fn get_PointerDevice(&self, value: *mut *mut c_void) -> HRESULT;
    pub fn get_Position(&self, value: *mut Point) -> HRESULT;
    pub fn get_RawPosition(&self, value: *mut Point) -> HRESULT;
    pub fn get_PointerId(&self, value: *mut u32) -> HRESULT;
    pub fn get_FrameId(&self, value: *mut u32) -> HRESULT;
    pub fn get_Timestamp(&self, value: *mut u64) -> HRESULT;
    pub fn get_IsInContact(&self, value: *mut u8) -> HRESULT;
    pub fn get_Properties(&self, value: *mut *mut c_void) -> HRESULT;
}

/// `Windows.UI.Input.IPointerPointProperties`, for the two members a scroll needs.
///
/// `get_MouseWheelDelta` is the 14th of the interface's own methods and
/// `get_IsHorizontalMouseWheel` the 15th, so the thirteen pen/touch/button properties ahead
/// of them are placeholders. The horizontal flag is not optional: a touchpad's *sideways*
/// two-finger scroll arrives as this same event, and without the test it would change the
/// volume too.
#[interface("c79d8a4b-c163-4ee7-803f-67ce79f9972d")]
pub unsafe trait IPointerPointProperties: IUnknown {
    pub fn GetIids(&self, count: *mut u32, iids: *mut *mut windows_core::GUID) -> HRESULT;
    pub fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    pub fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    pub fn _reserved01(&self) -> HRESULT; // Pressure
    pub fn _reserved02(&self) -> HRESULT; // IsInverted
    pub fn _reserved03(&self) -> HRESULT; // IsEraser
    pub fn _reserved04(&self) -> HRESULT; // Orientation
    pub fn _reserved05(&self) -> HRESULT; // XTilt
    pub fn _reserved06(&self) -> HRESULT; // YTilt
    pub fn _reserved07(&self) -> HRESULT; // Twist
    pub fn _reserved08(&self) -> HRESULT; // ContactRect
    pub fn _reserved09(&self) -> HRESULT; // ContactRectRaw
    pub fn _reserved10(&self) -> HRESULT; // TouchConfidence
    pub fn _reserved11(&self) -> HRESULT; // IsLeftButtonPressed
    pub fn _reserved12(&self) -> HRESULT; // IsRightButtonPressed
    pub fn _reserved13(&self) -> HRESULT; // IsMiddleButtonPressed
    pub fn get_MouseWheelDelta(&self, value: *mut i32) -> HRESULT;
    pub fn get_IsHorizontalMouseWheel(&self, value: *mut u8) -> HRESULT;
}

// WinRT delegates. Unlike the interfaces above these derive from `IUnknown`, not
// `IInspectable` — `Invoke` is slot 3 with no `GetIids`/`GetRuntimeClassName`/
// `GetTrustLevel` ahead of it. Getting that wrong calls the wrong function
// pointer, so it is worth stating explicitly.

/// `Windows.UI.Xaml.Input.PointerEventHandler`, for hover enter/exit — and for
/// `PointerWheelChanged`, which uses this same delegate type.
#[interface("e4385929-c004-4bcf-8970-359486e39f88")]
pub unsafe trait IPointerEventHandler: IUnknown {
    pub fn Invoke(&self, sender: *mut c_void, args: *mut c_void) -> HRESULT;
}

/// `Windows.UI.Xaml.Input.TappedEventHandler` — a completed left click.
#[interface("68d940cc-9ff0-49ce-b141-3f07ec477b97")]
pub unsafe trait ITappedEventHandler: IUnknown {
    pub fn Invoke(&self, sender: *mut c_void, args: *mut c_void) -> HRESULT;
}

/// `Windows.UI.Xaml.Input.RightTappedEventHandler` — a completed right click.
#[interface("2532a062-f447-4950-9c46-f1e34a2c2238")]
pub unsafe trait IRightTappedEventHandler: IUnknown {
    pub fn Invoke(&self, sender: *mut c_void, args: *mut c_void) -> HRESULT;
}

/// `Windows.UI.Xaml.Controls.IPanel`. `get_Children` is its first own slot.
#[interface("a50a4bbd-8361-469c-90da-e9a40c7474df")]
pub unsafe trait IPanel: IUnknown {
    pub fn GetIids(&self, count: *mut u32, iids: *mut *mut windows_core::GUID) -> HRESULT;
    pub fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    pub fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    pub fn get_Children(&self, value: *mut *mut c_void) -> HRESULT;
}

/// `Windows.Foundation.Collections.IVector<Windows.UI.Xaml.UIElement>`.
///
/// The IID is the parameterized-interface GUID for this exact specialisation,
/// taken from the header — it is not `IVector`'s own IID.
#[interface("b4c1e3ac-8768-5b9d-a661-f63330b8507b")]
pub unsafe trait IVectorUIElement: IUnknown {
    pub fn GetIids(&self, count: *mut u32, iids: *mut *mut windows_core::GUID) -> HRESULT;
    pub fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    pub fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    pub fn GetAt(&self, index: u32, item: *mut *mut c_void) -> HRESULT;
    pub fn get_Size(&self, value: *mut u32) -> HRESULT;
    pub fn GetView(&self, view: *mut *mut c_void) -> HRESULT;
    pub fn IndexOf(&self, item: *mut c_void, index: *mut u32, found: *mut u8) -> HRESULT;
    pub fn SetAt(&self, index: u32, item: *mut c_void) -> HRESULT;
    pub fn InsertAt(&self, index: u32, item: *mut c_void) -> HRESULT;
    pub fn RemoveAt(&self, index: u32) -> HRESULT;
    pub fn Append(&self, item: *mut c_void) -> HRESULT;
    pub fn RemoveAtEnd(&self) -> HRESULT;
    pub fn Clear(&self) -> HRESULT;
}

/// `Windows.UI.Xaml.Controls.ITextBlock`. `put_Text` is slot 22 of the
/// interface's own methods, so the 21 before it are placeholders.
#[interface("ae2d9271-3b4a-45fc-8468-f7949548f4d5")]
pub unsafe trait ITextBlock: IUnknown {
    pub fn GetIids(&self, count: *mut u32, iids: *mut *mut windows_core::GUID) -> HRESULT;
    pub fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    pub fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    pub fn _reserved01(&self) -> HRESULT;
    pub fn _reserved02(&self) -> HRESULT;
    pub fn _reserved03(&self) -> HRESULT;
    pub fn _reserved04(&self) -> HRESULT;
    pub fn _reserved05(&self) -> HRESULT;
    pub fn _reserved06(&self) -> HRESULT;
    pub fn _reserved07(&self) -> HRESULT;
    pub fn _reserved08(&self) -> HRESULT;
    pub fn _reserved09(&self) -> HRESULT;
    pub fn _reserved10(&self) -> HRESULT;
    pub fn _reserved11(&self) -> HRESULT;
    pub fn _reserved12(&self) -> HRESULT;
    pub fn _reserved13(&self) -> HRESULT;
    pub fn _reserved14(&self) -> HRESULT;
    pub fn _reserved15(&self) -> HRESULT;
    pub fn _reserved16(&self) -> HRESULT;
    pub fn _reserved17(&self) -> HRESULT;
    pub fn _reserved18(&self) -> HRESULT;
    pub fn _reserved19(&self) -> HRESULT;
    pub fn _reserved20(&self) -> HRESULT;
    pub fn get_Text(&self, value: *mut *mut c_void) -> HRESULT;
    pub fn put_Text(&self, value: *mut c_void) -> HRESULT;
}

/// `Windows.UI.Xaml.Markup.IXamlReaderStatics`.
///
/// `XamlReader.Load` is how elements get created: `IVisualTreeService::CreateInstance`
/// is `E_NOTIMPL` in Explorer, and this is the route the known-good C++ TAPs use.
#[interface("9891c6bd-534f-4955-b85a-8a8dc0dca602")]
pub unsafe trait IXamlReaderStatics: IUnknown {
    pub fn GetIids(&self, count: *mut u32, iids: *mut *mut windows_core::GUID) -> HRESULT;
    pub fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    pub fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    pub fn Load(&self, xaml: *mut c_void, result: *mut *mut c_void) -> HRESULT;
    pub fn LoadWithInitialTemplateValidation(
        &self,
        xaml: *mut c_void,
        result: *mut *mut c_void,
    ) -> HRESULT;
}

/// `Windows.UI.Xaml.Controls.IContentPresenter`. Setting `Content` is the
/// supported way to put our own visuals into a tray icon — `Panel.Children`
/// mutation is refused (`0x800F1000`).
#[interface("79fde5b4-cd37-491c-8845-daf472defff6")]
pub unsafe trait IContentPresenter: IUnknown {
    pub fn GetIids(&self, count: *mut u32, iids: *mut *mut windows_core::GUID) -> HRESULT;
    pub fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    pub fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    pub fn get_Content(&self, value: *mut *mut c_void) -> HRESULT;
    pub fn put_Content(&self, value: *mut c_void) -> HRESULT;
}

/// `Windows.UI.Xaml.Automation.IAutomationPropertiesStatics`.
///
/// `GetName` carries the notify icon's tooltip text, which is how a specific tray
/// icon is identified (Windhawk's selectors use `[AutomationProperties.Name=…]`).
/// `GetName` is slot 26 of the interface's own methods.
#[interface("b618fd7b-32d0-4970-9c42-7c039ac7be78")]
pub unsafe trait IAutomationPropertiesStatics: IUnknown {
    pub fn GetIids(&self, count: *mut u32, iids: *mut *mut windows_core::GUID) -> HRESULT;
    pub fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    pub fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    pub fn _reserved01(&self) -> HRESULT;
    pub fn _reserved02(&self) -> HRESULT;
    pub fn _reserved03(&self) -> HRESULT;
    pub fn _reserved04(&self) -> HRESULT;
    pub fn _reserved05(&self) -> HRESULT;
    pub fn _reserved06(&self) -> HRESULT;
    pub fn _reserved07(&self) -> HRESULT;
    pub fn _reserved08(&self) -> HRESULT;
    pub fn _reserved09(&self) -> HRESULT;
    pub fn _reserved10(&self) -> HRESULT;
    pub fn _reserved11(&self) -> HRESULT;
    pub fn _reserved12(&self) -> HRESULT;
    pub fn _reserved13(&self) -> HRESULT;
    pub fn _reserved14(&self) -> HRESULT;
    pub fn _reserved15(&self) -> HRESULT;
    pub fn _reserved16(&self) -> HRESULT;
    pub fn _reserved17(&self) -> HRESULT;
    pub fn _reserved18(&self) -> HRESULT;
    pub fn _reserved19(&self) -> HRESULT;
    pub fn _reserved20(&self) -> HRESULT;
    pub fn _reserved21(&self) -> HRESULT;
    pub fn _reserved22(&self) -> HRESULT;
    pub fn _reserved23(&self) -> HRESULT;
    pub fn _reserved24(&self) -> HRESULT;
    pub fn _reserved25(&self) -> HRESULT;
    pub fn GetName(&self, element: *mut c_void, value: *mut *mut c_void) -> HRESULT;
}

/// `Windows.UI.Xaml.IFrameworkElement`. Primarily a QI target: the `Grid`
/// statics take an `IFrameworkElement*`, and COM requires that exact interface
/// pointer — handing over the `IInspectable` instead calls through the wrong
/// vtable.
///
/// `put_Width` earns its place separately. Collapsing the system volume icon
/// hides the glyph but does *not* free its slot inside the Quick Settings
/// button, so the width has to be zeroed explicitly (see `decorate::collapse`).
///
/// `HorizontalAlignment` and `Margin` came with the music tile, and each pays for a defect that is
/// invisible from the code:
///
/// * An element left at `Stretch` and then given an explicit `Width` is **centred**, not
///   left-aligned. That slid the strip 40 epx right — half of `ask − content` — taking the `next`
///   glyph off the end with it.
/// * The shell centres its `RunningIndicator` and `ProgressIndicator` in the *button*. Fine at 44
///   epx; at the 244 the strip needs, the running pill lands under the middle of the title and reads
///   as a stray dot. Alignment moves them to the button's left edge; the margin places them under the
///   app icon.
///
/// Verified against the SDK header, whose members after `MaxWidth` run **MinHeight g/p, MaxHeight
/// g/p**, HorizontalAlignment g/p, **VerticalAlignment g/p**, Margin g/p, Name g/p, … — so four
/// placeholders separate `put_MaxWidth` from `get_HorizontalAlignment`, and two more separate that
/// from `get_Margin`. Count these against the header, never by eye: one slot out returns `S_OK` and
/// silently does something else.
#[interface("a391d09b-4a99-4b7c-9d8d-6fa5d01f6fbf")]
pub unsafe trait IFrameworkElement: IUnknown {
    pub fn GetIids(&self, count: *mut u32, iids: *mut *mut windows_core::GUID) -> HRESULT;
    pub fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    pub fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    pub fn _reserved01(&self) -> HRESULT;
    pub fn _reserved02(&self) -> HRESULT;
    pub fn _reserved03(&self) -> HRESULT;
    pub fn _reserved04(&self) -> HRESULT;
    pub fn _reserved05(&self) -> HRESULT;
    pub fn _reserved06(&self) -> HRESULT;
    pub fn _reserved07(&self) -> HRESULT;
    pub fn get_ActualWidth(&self, value: *mut f64) -> HRESULT;
    pub fn get_ActualHeight(&self, value: *mut f64) -> HRESULT;
    pub fn get_Width(&self, value: *mut f64) -> HRESULT;
    pub fn put_Width(&self, value: f64) -> HRESULT;
    pub fn get_Height(&self, value: *mut f64) -> HRESULT;
    pub fn put_Height(&self, value: f64) -> HRESULT;
    pub fn get_MinWidth(&self, value: *mut f64) -> HRESULT;
    pub fn put_MinWidth(&self, value: f64) -> HRESULT;
    pub fn get_MaxWidth(&self, value: *mut f64) -> HRESULT;
    pub fn put_MaxWidth(&self, value: f64) -> HRESULT;
    pub fn _reserved_min_height_get(&self) -> HRESULT;
    pub fn _reserved_min_height_put(&self) -> HRESULT;
    pub fn _reserved_max_height_get(&self) -> HRESULT;
    pub fn _reserved_max_height_put(&self) -> HRESULT;
    pub fn get_HorizontalAlignment(&self, value: *mut i32) -> HRESULT;
    pub fn put_HorizontalAlignment(&self, value: i32) -> HRESULT;
    pub fn _reserved_vertical_alignment_get(&self) -> HRESULT;
    pub fn _reserved_vertical_alignment_put(&self) -> HRESULT;
    pub fn get_Margin(&self, value: *mut Thickness) -> HRESULT;
    pub fn put_Margin(&self, value: Thickness) -> HRESULT;
}

/// `Windows.UI.Xaml.HorizontalAlignment`.
pub const HORIZONTAL_ALIGNMENT_LEFT: i32 = 0;
#[allow(dead_code)] // the value everything starts at; kept so the pair reads correctly
pub const HORIZONTAL_ALIGNMENT_STRETCH: i32 = 3;

/// `Windows.UI.Xaml.Thickness` — four `DOUBLE`s, in XAML's `left,top,right,bottom` order.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Thickness {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

/// `Windows.UI.Xaml.Controls.IBorder`.
///
/// **The one property that makes a taskbar *button* usable as a host.** Nothing at that end of the
/// taskbar is a `ContentControl`, so the notification area's `ContentPresenter.Content` route has no
/// counterpart there — but every `TaskListButton` template contains a `Border#BackgroundElement`, and
/// `Border.Child` is a single-value property rather than a `UIElementCollection`, which sidesteps the
/// `0x800F1000` that blocks `Panel.Children.Append`.
///
/// Verified against the SDK header: the interface's own members run BorderBrush g/p, BorderThickness
/// g/p, Background g/p, CornerRadius g/p, Padding g/p, **Child g/p** — so `put_Child` is the
/// fourteenth slot, and the ten before it have to be spelled out to reach it. The `Thickness` and
/// `CornerRadius` ones are declared with `*mut c_void` operands because nothing here calls them; only
/// their *width in the vtable* matters.
#[interface("797c4539-45bd-4633-a044-bfb02ef5170f")]
pub unsafe trait IBorder: IUnknown {
    pub fn GetIids(&self, count: *mut u32, iids: *mut *mut windows_core::GUID) -> HRESULT;
    pub fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    pub fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    pub fn get_BorderBrush(&self, value: *mut *mut c_void) -> HRESULT;
    pub fn put_BorderBrush(&self, value: *mut c_void) -> HRESULT;
    pub fn get_BorderThickness(&self, value: *mut c_void) -> HRESULT;
    pub fn put_BorderThickness(&self, value: *mut c_void) -> HRESULT;
    pub fn get_Background(&self, value: *mut *mut c_void) -> HRESULT;
    pub fn put_Background(&self, value: *mut c_void) -> HRESULT;
    pub fn get_CornerRadius(&self, value: *mut c_void) -> HRESULT;
    pub fn put_CornerRadius(&self, value: *mut c_void) -> HRESULT;
    pub fn get_Padding(&self, value: *mut c_void) -> HRESULT;
    pub fn put_Padding(&self, value: *mut c_void) -> HRESULT;
    pub fn get_Child(&self, value: *mut *mut c_void) -> HRESULT;
    pub fn put_Child(&self, value: *mut c_void) -> HRESULT;
}

/// `Windows.UI.Xaml.Controls.IImage`.
///
/// Only `put_Source`, and only so cover art can be swapped **in place**. Rebuilding the strip to
/// change the artwork would replace every element in it — including the ones the click handlers are
/// attached to — so a track change would silently break the transport buttons.
///
/// Verified against the SDK header: `Source` g/p are the interface's first two own members.
#[interface("495b7402-9af3-4e50-aa90-03388f3086d2")]
pub unsafe trait IImage: IUnknown {
    pub fn GetIids(&self, count: *mut u32, iids: *mut *mut windows_core::GUID) -> HRESULT;
    pub fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    pub fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    pub fn get_Source(&self, value: *mut *mut c_void) -> HRESULT;
    pub fn put_Source(&self, value: *mut c_void) -> HRESULT;
}

/// `Windows.UI.Xaml.Controls.IGridStatics`.
///
/// The tray's sections are children of one `Grid`, ordered by the `Grid.Column`
/// attached property. These helpers take a plain `i32`, which avoids needing a
/// `DependencyProperty` and the `IPropertyValue` boxing dance.
#[interface("64fe2e9f-f951-42b6-a9ce-bb179af11595")]
pub unsafe trait IGridStatics: IUnknown {
    pub fn GetIids(&self, count: *mut u32, iids: *mut *mut windows_core::GUID) -> HRESULT;
    pub fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    pub fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    pub fn get_RowProperty(&self, value: *mut *mut c_void) -> HRESULT;
    pub fn GetRow(&self, element: *mut c_void, value: *mut i32) -> HRESULT;
    pub fn SetRow(&self, element: *mut c_void, value: i32) -> HRESULT;
    pub fn get_ColumnProperty(&self, value: *mut *mut c_void) -> HRESULT;
    pub fn GetColumn(&self, element: *mut c_void, value: *mut i32) -> HRESULT;
    pub fn SetColumn(&self, element: *mut c_void, value: i32) -> HRESULT;
}

// There is deliberately no `ICoreDispatcher` binding. Marshalling XAML work through the
// dispatcher `IXamlDiagnostics::GetDispatcher` hands back is a dead end — it belongs to
// another of Explorer's XAML islands, and every call against a tray element from there fails
// `RPC_E_WRONG_THREAD`. See "GetDispatcher is a trap" in FINDINGS.md; the TAP learns the
// tray's own thread instead (`lifecycle::adopt_tray_thread`) and runs inline on it.

/// The runtime class whose activation factory implements the statics above.
pub const VISUAL_TREE_HELPER: &str = "Windows.UI.Xaml.Media.VisualTreeHelper";
pub const GRID: &str = "Windows.UI.Xaml.Controls.Grid";
pub const XAML_READER: &str = "Windows.UI.Xaml.Markup.XamlReader";
pub const AUTOMATION_PROPERTIES: &str = "Windows.UI.Xaml.Automation.AutomationProperties";


/// `Windows.UI.Xaml.Input.ITappedRoutedEventArgs`, for the `put_Handled` lever on a completed tap.
///
/// The music tile's transport glyphs need it: a tap they act on must not also reach the app button
/// underneath, or pressing play activates YouTube Music on top of the track change.
///
/// Verified against the SDK header: `PointerDeviceType`, `Handled` g/p, `GetPosition` — so `put_Handled`
/// is the sixth slot after the three `IInspectable` ones.
#[interface("a099e6be-e624-459a-bb1d-e05c73e2cc66")]
pub unsafe trait ITappedRoutedEventArgs: IUnknown {
    pub fn GetIids(&self, count: *mut u32, iids: *mut *mut windows_core::GUID) -> HRESULT;
    pub fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    pub fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    pub fn get_PointerDeviceType(&self, value: *mut i32) -> HRESULT;
    pub fn get_Handled(&self, value: *mut u8) -> HRESULT;
    pub fn put_Handled(&self, value: u8) -> HRESULT;
    pub fn GetPosition(&self, relative_to: *mut c_void, value: *mut c_void) -> HRESULT;
}

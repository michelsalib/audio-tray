//! Hand-rolled WinRT projections for the bits of `Windows.UI.Xaml` we need (windows-rs does not
//! project it), transcribed from the SDK's `winrt\windows.ui.xaml*.h`.
//!
//! **Slot count and order must match the header exactly**: one slot out returns `S_OK` and silently
//! calls something else. Unused slots are placeholders. Interfaces are declared on `IUnknown` with
//! `IInspectable`'s three slots spelled out (`#[interface]` cannot derive from `IInspectable`, nor
//! expand a macro for them); the layout is identical.

#![allow(non_snake_case)]

use core::ffi::c_void;
use windows_core::{interface, IUnknown, IUnknown_Vtbl, HRESULT};

/// `Windows.Foundation.Point` / `Rect`, so the unused `Find*` slots have FFI-safe signatures.
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

/// `Windows.UI.Xaml.IDependencyObject`; only its IID is used.
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

/// `Windows.UI.Xaml.Media.IVisualTreeHelperStatics` (the `Find*` slots are unused).
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
pub const VISIBILITY_COLLAPSED: i32 = 1;

/// `Windows.UI.Xaml.IUIElement`. Slot numbers follow the header: `get_Opacity` 9,
/// `get_Visibility` 21, the pointer and tap events 57..80.
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

/// `Windows.UI.Xaml.Input.IPointerRoutedEventArgs`: `put_Handled`, and `GetCurrentPoint` (the
/// wheel delta is at `GetCurrentPoint(null).Properties.MouseWheelDelta`).
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

/// `Windows.UI.Input.IPointerPoint`. Only `get_Properties` is called.
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

/// `Windows.UI.Input.IPointerPointProperties`: `get_MouseWheelDelta` (14th own method) and
/// `get_IsHorizontalMouseWheel` (15th; a sideways swipe raises the same event).
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

// WinRT delegates derive from `IUnknown`, not `IInspectable`: `Invoke` is slot 3.

/// `Windows.UI.Xaml.Input.PointerEventHandler` (hover, press and wheel events).
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

/// `Windows.UI.Xaml.Markup.IXamlReaderStatics`. `Load` is how elements get created
/// (`IVisualTreeService::CreateInstance` is `E_NOTIMPL` in Explorer).
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

/// `Windows.UI.Xaml.Controls.IContentPresenter`. `Content` is how our visuals enter a tray icon
/// (`Panel.Children` mutation is refused, `0x800F1000`).
#[interface("79fde5b4-cd37-491c-8845-daf472defff6")]
pub unsafe trait IContentPresenter: IUnknown {
    pub fn GetIids(&self, count: *mut u32, iids: *mut *mut windows_core::GUID) -> HRESULT;
    pub fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    pub fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    pub fn get_Content(&self, value: *mut *mut c_void) -> HRESULT;
    pub fn put_Content(&self, value: *mut c_void) -> HRESULT;
}

/// `Windows.UI.Xaml.Automation.IAutomationPropertiesStatics`. `GetName` (26th own method) carries
/// a tray icon's tooltip, which is how ours is identified.
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

/// `Windows.UI.Xaml.IFrameworkElement`. Also a QI target: the `Grid` statics need this exact
/// interface pointer (an `IInspectable` calls through the wrong vtable and hangs).
///
/// `Width`/`MinWidth` zero a collapsed slot (see `decorate::collapse`); `HorizontalAlignment` and
/// `Margin` pin the music tile's strip and the shell's indicators (a `Stretch` element given an
/// explicit `Width` is centred). After `MaxWidth` the header has MinHeight and MaxHeight g/p, then
/// HorizontalAlignment, VerticalAlignment, Margin: count against the header, never by eye.
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

/// `Windows.UI.Xaml.Thickness` — four `DOUBLE`s, in XAML's `left,top,right,bottom` order.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Thickness {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

/// `Windows.UI.Xaml.Controls.IBorder`. `Border.Child` (single-valued, so not refused like
/// `Panel.Children`) on a `TaskListButton`'s `Border#BackgroundElement` is how the music tile is
/// hosted. Uncalled slots take `*mut c_void`; only their position matters.
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

/// `Windows.UI.Xaml.Controls.IGridStatics`: `Grid.Column` (which orders the tray's sections) as a
/// plain `i32`, without `DependencyProperty` boxing.
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

// Deliberately no `ICoreDispatcher`: `GetDispatcher` returns another island's dispatcher
// (`RPC_E_WRONG_THREAD`). See FINDINGS.md, "Threading, settled".

/// The runtime class whose activation factory implements the statics above.
pub const VISUAL_TREE_HELPER: &str = "Windows.UI.Xaml.Media.VisualTreeHelper";
pub const GRID: &str = "Windows.UI.Xaml.Controls.Grid";
pub const XAML_READER: &str = "Windows.UI.Xaml.Markup.XamlReader";
pub const AUTOMATION_PROPERTIES: &str = "Windows.UI.Xaml.Automation.AutomationProperties";


/// `Windows.UI.Xaml.Input.ITappedRoutedEventArgs`, for `put_Handled` on a transport tap (so it
/// does not also activate the app).
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

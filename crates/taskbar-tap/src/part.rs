//! What the taskbar parts the TAP takes over share (the clock's button, the bell, Windows' battery
//! icon): the shell's names for their elements, the shell's plate behind a button and its look, a
//! grid of theirs made a plate of its own, a picture worn as a grid's brush, their tooltips, and
//! property-change callbacks.

use std::cell::RefCell;
use std::sync::Mutex;

use crate::log::logf;
use crate::winrt::{CornerRadius, IDependencyPropertyChangedCallback, IDependencyPropertyChangedCallback_Impl, IFrameworkElement, IGrid2, Thickness};
use crate::{tree, xaml, xamlom};
use windows_core::{implement, IInspectable, IUnknown, Interface};

// ---- The shell's names (`x:Name`s and XAML types) ----

/// An icon of the tray (the clock's, the bell's, the battery's, the shell's indicators').
pub(crate) const ICON: &str = "SystemTray.IconView";
/// The Quick Settings button (network, volume, battery), by `x:Name`.
pub(crate) const QUICK_SETTINGS: &str = "ControlCenterButton";
/// A tray icon's glyph, by `x:Name`: the shell's indicators' text blocks, the bell's layers.
pub(crate) const GLYPH_BLOCK: &str = "InnerTextBlock";
/// The shell's plate behind the whole button.
pub(crate) const PLATE: &str = "BackgroundBorder";
/// Each icon's outer grid: its plate.
pub(crate) const ICON_GRID: &str = "ContainerGrid";
/// An icon's inner grid: its badge.
pub(crate) const INNER_GRID: &str = "ContentGrid";
/// The room between the two plates, in DIPs.
pub(crate) const PLATE_GAP: f64 = 2.0;

// ---- The shell's plate and its look ----

/// The shell's plate's look while the pointer was over it: its brush, border brush and border
/// width. Kept for the open card's button once the pointer has gone (the shell's plate is at rest
/// then). XAML thread's, as the brushes are.
#[derive(Clone)]
pub(crate) struct Look {
    pub(crate) background: IUnknown,
    pub(crate) border: Option<IUnknown>,
    pub(crate) thickness: Thickness,
}

/// The shell's plate's look now (`None`: no background, at rest).
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn plate_look(diagnostics: &xamlom::IXamlDiagnostics, plate: u64) -> Option<Look> {
    let border = xaml::element::<crate::winrt::IBorder>(diagnostics, plate)?;
    let mut raw: *mut core::ffi::c_void = core::ptr::null_mut();
    if border.get_Background(&mut raw).is_err() || raw.is_null() {
        return None;
    }
    let background = IUnknown::from_raw(raw);
    let mut raw: *mut core::ffi::c_void = core::ptr::null_mut();
    let _ = border.get_BorderBrush(&mut raw);
    let border_brush = (!raw.is_null()).then(|| IUnknown::from_raw(raw));
    let mut thickness = Thickness::default();
    let _ = border.get_BorderThickness(&mut thickness);
    Some(Look { background, border: border_brush, thickness })
}

thread_local! {
    /// The shell's plate's look last seen while the pointer was on a part, per part (`who`): worn
    /// while its card is open, once the pointer has gone.
    static LIT: RefCell<Vec<(&'static str, Look)>> = const { RefCell::new(Vec::new()) };
    /// The wash a card opened before any hover wears ([`open_look`]), and whether it was built for
    /// a light taskbar.
    static FALLBACK: RefCell<Option<(bool, Look)>> = const { RefCell::new(None) };
}

/// Remember `look`, seen on the shell's plate while the pointer was on `who`'s part.
pub(crate) fn remember_look(who: &'static str, look: &Look) {
    LIT.with(|lit| {
        let mut lit = lit.borrow_mut();
        lit.retain(|(w, _)| *w != who);
        lit.push((who, look.clone()));
    });
}

/// The look of `who`'s part while its card is open: the one last seen under the pointer, else (a
/// card opened before the pointer was ever over it: Win+N) a wash near the shell's, built once per
/// taskbar theme.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn open_look(who: &'static str) -> Option<Look> {
    if let Some(look) = LIT.with(|lit| lit.borrow().iter().find(|(w, _)| *w == who).map(|(_, look)| look.clone())) {
        return Some(look);
    }
    let light = light_taskbar();
    if let Some(look) = FALLBACK.with(|f| f.borrow().as_ref().filter(|(l, _)| *l == light).map(|(_, look)| look.clone())) {
        return Some(look);
    }
    let color = if light { "#0A000000" } else { "#12FFFFFF" };
    let markup = format!(r#"<SolidColorBrush xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" Color="{color}"/>"#);
    let brush = xaml::load_xaml(&markup)?;
    let look = Look { background: brush.cast().ok()?, border: None, thickness: Thickness::default() };
    FALLBACK.with(|f| *f.borrow_mut() = Some((light, look.clone())));
    Some(look)
}

/// Whether the taskbar is light (Windows' "default app mode" is the apps'; the taskbar follows the
/// system's).
pub(crate) fn light_taskbar() -> bool {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let mut value = 0u32;
    let mut size = 4u32;
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    read.is_ok() && value != 0
}

/// The shell's plate's inset in its button, its corners (4 DIPs when it has none) and its border
/// width (at least 1 DIP): what a part's own plate copies.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn plate_shape(diagnostics: &xamlom::IXamlDiagnostics, plate: u64) -> (Thickness, CornerRadius, f64) {
    let mut margin = Thickness::default();
    let mut radius = CornerRadius::default();
    let mut edge = Thickness::default();
    if let Some(border) = xaml::element::<IFrameworkElement>(diagnostics, plate) {
        let _ = border.get_Margin(&mut margin);
    }
    if let Some(border) = xaml::element::<crate::winrt::IBorder>(diagnostics, plate) {
        let _ = border.get_CornerRadius(&mut radius);
        let _ = border.get_BorderThickness(&mut edge);
    }
    if radius.top_left <= 0.0 {
        radius = CornerRadius { top_left: 4.0, top_right: 4.0, bottom_right: 4.0, bottom_left: 4.0 };
    }
    (margin, radius, edge.top.max(edge.left).max(1.0))
}

/// Give each of `grids` the shell's plate's background sizing and, when the plate fades, a fade of
/// its own (one transition instance cannot serve two elements). Says whether the plate fades.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn dress_like_plate(diagnostics: &xamlom::IXamlDiagnostics, plate: u64, grids: &[u64]) -> Option<bool> {
    let shell = xaml::element::<crate::winrt::IBorder2>(diagnostics, plate)?;
    let mut sizing = 0i32;
    let _ = shell.get_BackgroundSizing(&mut sizing);
    let mut transition: *mut core::ffi::c_void = core::ptr::null_mut();
    let _ = shell.get_BackgroundTransition(&mut transition);
    // Ours to release.
    let transition = (!transition.is_null()).then(|| IUnknown::from_raw(transition));
    for &grid in grids {
        if let Some(sized) = xaml::element::<crate::winrt::IGrid4>(diagnostics, grid) {
            let _ = sized.put_BackgroundSizing(sizing);
        }
        let own = transition.as_ref().and_then(|_| xaml::load_xaml(r#"<BrushTransition xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" Duration="0:0:0.083"/>"#));
        if let (Some(fading), Some(own)) = (xaml::element::<crate::winrt::IPanel2>(diagnostics, grid), own) {
            let _ = fading.put_BackgroundTransition(own.as_raw());
        }
    }
    Some(transition.is_some())
}

/// Have the shell's `plate` run `run` when its background or border brush changes (`what` names
/// the callback in the log). Once per plate, never detached: `run` must only act on what is split.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn watch_plate(diagnostics: &xamlom::IXamlDiagnostics, plate: u64, what: &'static str, run: fn()) {
    /// Plates already watched: a re-split after a stand-down (callbacks never detached) must not
    /// pile another pair on.
    static WATCHED: Mutex<Vec<u64>> = Mutex::new(Vec::new());
    {
        let mut watched = crate::lock(&WATCHED);
        watched.retain(|h| tree::type_of(*h).is_some());
        if watched.contains(&plate) {
            return;
        }
        watched.push(plate);
    }
    let Some(statics) = xaml::factory::<crate::winrt::IBorderStatics>(crate::winrt::BORDER) else { return };
    let mut ok = true;
    for get in [crate::winrt::IBorderStatics::get_BackgroundProperty, crate::winrt::IBorderStatics::get_BorderBrushProperty] {
        let mut dp: *mut core::ffi::c_void = core::ptr::null_mut();
        if get(&statics, &mut dp).is_err() || dp.is_null() {
            ok = false;
            continue;
        }
        let dp = IUnknown::from_raw(dp);
        ok &= on_change(diagnostics, plate, &dp, what, run);
    }
    if !ok {
        logf!("{what}: the shell's plate would not report its changes; its parts light on entering only");
    }
}

// ---- Property-change callbacks ----

/// Callbacks already seen called (each said once, as proof they work).
static CALLED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// A watched property changed: `run` runs (`what` names it in the log).
#[implement(IDependencyPropertyChangedCallback)]
struct Changed {
    what: &'static str,
    run: fn(),
}

impl IDependencyPropertyChangedCallback_Impl for Changed_Impl {
    unsafe fn Invoke(&self, _sender: *mut core::ffi::c_void, _dp: *mut core::ffi::c_void) -> windows_core::HRESULT {
        crate::guard(self.what, || {
            let mut called = crate::lock(&CALLED);
            if !called.contains(&self.what) {
                called.push(self.what);
                logf!("{} runs", self.what);
            }
            drop(called);
            (self.run)();
        });
        windows_core::HRESULT(0)
    }
}

/// Have the element `handle` run `run` when its property `dp` changes (`what` names the callback
/// in the log, as `"clock: the text callback"`); `false` if XAML would not. Never detached: `run`
/// must only act on what is still the TAP's.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn on_change(diagnostics: &xamlom::IXamlDiagnostics, handle: u64, dp: &IUnknown, what: &'static str, run: fn()) -> bool {
    let Some(object) = xaml::element::<crate::winrt::IDependencyObject2>(diagnostics, handle) else { return false };
    let callback: IDependencyPropertyChangedCallback = Changed { what, run }.into();
    let mut token = 0i64;
    object.RegisterPropertyChangedCallback(dp.as_raw(), callback.as_raw(), &mut token).is_ok()
}

// ---- A grid made a plate of its own ----

/// What a grid made a plate held before ([`restore_grid`] puts it back).
#[derive(Clone, Copy)]
pub(crate) struct SavedGrid {
    pub(crate) grid: u64,
    margin: Thickness,
    radius: CornerRadius,
}

/// A grid about to be made a plate: its interfaces and what it holds now, read by [`to_plate`]
/// before any grid of a split is written, so half a split is never left behind.
pub(crate) struct ToPlate {
    framework: IFrameworkElement,
    rounded: IGrid2,
    saved: SavedGrid,
}

/// `grid` ready to be made a plate; `None` without the interfaces that takes.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn to_plate(diagnostics: &xamlom::IXamlDiagnostics, grid: u64) -> Option<ToPlate> {
    let framework = xaml::element::<IFrameworkElement>(diagnostics, grid)?;
    let rounded = xaml::element::<IGrid2>(diagnostics, grid)?;
    let mut margin = Thickness::default();
    let mut radius = CornerRadius::default();
    let _ = framework.get_Margin(&mut margin);
    let _ = rounded.get_CornerRadius(&mut radius);
    Some(ToPlate { framework, rounded, saved: SavedGrid { grid, margin, radius } })
}

/// Make the grid a plate: inset by `inset` in its button, `radius` corners, an `edge`-wide border.
/// The border's width is fixed from the start (on a grid a border takes room from what it holds,
/// so one that came and went with the pointer would move the icons, and the taskbar) and taken out
/// of the margin, so the button keeps its size. Its brushes come with [`wear`].
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn make_plate(found: ToPlate, inset: Thickness, radius: CornerRadius, edge: f64) -> SavedGrid {
    let _ = found.framework.put_Margin(Thickness { left: inset.left - edge, top: inset.top - edge, right: inset.right - edge, bottom: inset.bottom - edge });
    let _ = found.rounded.put_BorderThickness(Thickness { left: edge, top: edge, right: edge, bottom: edge });
    let _ = found.rounded.put_CornerRadius(radius);
    found.saved
}

/// Dress a grid made a plate in `look`, or bare it (`None`): brushes only, as its border's width is
/// fixed ([`make_plate`]), so nothing moves.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn wear(diagnostics: &xamlom::IXamlDiagnostics, grid: u64, look: Option<&Look>) {
    use crate::winrt::IPanel;
    let (Some(panel), Some(edged)) = (xaml::element::<IPanel>(diagnostics, grid), xaml::element::<IGrid2>(diagnostics, grid)) else { return };
    match look {
        Some(look) => {
            let _ = panel.put_Background(look.background.as_raw());
            let edge = look.border.as_ref().filter(|_| look.thickness.top > 0.0 || look.thickness.left > 0.0);
            let _ = edged.put_BorderBrush(edge.map_or(core::ptr::null_mut(), |b| b.as_raw()));
        }
        None => {
            let _ = panel.put_Background(core::ptr::null_mut());
            let _ = edged.put_BorderBrush(core::ptr::null_mut());
        }
    }
}

/// Put a grid made a plate back as it was: its margin and corners, no border, no background.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn restore_grid(diagnostics: &xamlom::IXamlDiagnostics, saved: &SavedGrid) {
    if let Some(framework) = xaml::element::<IFrameworkElement>(diagnostics, saved.grid) {
        let _ = framework.put_Margin(saved.margin);
    }
    if let Some(rounded) = xaml::element::<IGrid2>(diagnostics, saved.grid) {
        let _ = rounded.put_CornerRadius(saved.radius);
        let _ = rounded.put_BorderThickness(Thickness::default());
        let _ = rounded.put_BorderBrush(core::ptr::null_mut());
    }
    if let Some(panel) = xaml::element::<crate::winrt::IPanel>(diagnostics, saved.grid) {
        let _ = panel.put_Background(core::ptr::null_mut());
    }
}

/// The sections whose button a part could not split (missing parts, an unexpected layout): left
/// whole, each reported once, and not retried until a revert forgets them.
pub(crate) struct Unsplittable {
    /// The part, and its button, for the log.
    who: &'static str,
    button: &'static str,
    sections: Mutex<Vec<u64>>,
}

impl Unsplittable {
    pub(crate) const fn new(who: &'static str, button: &'static str) -> Self {
        Self { who, button, sections: Mutex::new(Vec::new()) }
    }

    /// `section`'s button is left whole: `why` (logged once).
    pub(crate) fn note(&self, section: u64, why: &str) {
        let mut sections = crate::lock(&self.sections);
        if !sections.contains(&section) {
            logf!("{}: {} cannot be split ({why}); left whole", self.who, self.button);
            sections.push(section);
        }
    }

    pub(crate) fn contains(&self, section: u64) -> bool {
        crate::lock(&self.sections).contains(&section)
    }

    pub(crate) fn sections(&self) -> Vec<u64> {
        crate::lock(&self.sections).clone()
    }

    pub(crate) fn clear(&self) {
        crate::lock(&self.sections).clear();
    }
}

// ---- A picture worn as a grid's brush ----

/// Grids wearing a brush of a part's ([`set_grid_brush`]): the grid, its part, and the key of the
/// brush.
static BRUSHED: Mutex<Vec<(u64, &'static str, String)>> = Mutex::new(Vec::new());

/// What [`set_grid_brush`] did.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Brushed {
    /// The grid already wore it: nothing written.
    Already,
    /// The grid wears it now.
    Changed,
    /// The grid was not found, the brush could not be made, or it was refused (logged).
    Refused,
}

/// Have `grid` (a panel) wear the brush `key` names, `brush` making it only when it changes (an
/// empty `key`: none). `owner` names the part, for the log and [`take_brushes`].
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn set_grid_brush(diagnostics: &xamlom::IXamlDiagnostics, grid: u64, owner: &'static str, key: &str, brush: impl FnOnce() -> Option<IInspectable>) -> Brushed {
    let Some(panel) = xaml::element::<crate::winrt::IPanel>(diagnostics, grid) else { return Brushed::Refused };
    {
        let mut brushed = crate::lock(&BRUSHED);
        // Grids gone from the tree (a taskbar rebuilt) are forgotten: their handles can come back.
        brushed.retain(|(h, ..)| tree::type_of(*h).is_some());
        let current = brushed.iter().find(|(h, ..)| *h == grid).map_or("", |(.., k)| k.as_str());
        if current == key {
            return Brushed::Already;
        }
    }
    // Not under the lock: a message pumped inside a XAML call can come back here.
    if key.is_empty() {
        let _ = panel.put_Background(core::ptr::null_mut());
    } else {
        let Some(brush) = brush() else { return Brushed::Refused };
        let hr = panel.put_Background(brush.as_raw());
        if hr.is_err() {
            logf!("{owner}: the badge on 0x{grid:x} was refused: 0x{:08x}", hr.0);
            return Brushed::Refused;
        }
    }
    let mut brushed = crate::lock(&BRUSHED);
    brushed.retain(|(h, ..)| *h != grid);
    if !key.is_empty() {
        brushed.push((grid, owner, key.to_string()));
    }
    Brushed::Changed
}

/// Whether `grid` wears a brush of a part's.
pub(crate) fn wears_brush(grid: u64) -> bool {
    crate::lock(&BRUSHED).iter().any(|(h, ..)| *h == grid)
}

/// Take every brush of `owner`'s off its grid (a revert).
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn take_brushes(diagnostics: &xamlom::IXamlDiagnostics, owner: &'static str) {
    let taken: Vec<u64> = {
        let mut brushed = crate::lock(&BRUSHED);
        let taken = brushed.iter().filter(|(_, o, _)| *o == owner).map(|(h, ..)| *h).collect();
        brushed.retain(|(_, o, _)| *o != owner);
        taken
    };
    for grid in taken {
        if let Some(panel) = xaml::element::<crate::winrt::IPanel>(diagnostics, grid) {
            let _ = panel.put_Background(core::ptr::null_mut());
        }
    }
}

/// An app-drawn picture (`path`, drawn at `scale` pixels per DIP) as a brush shown at its pixel
/// size: aligned to `align` (`AlignmentX`, `AlignmentY`), scaled back about `centre` (relative),
/// then moved down by `drop` (relative to the element's height).
pub(crate) fn image_brush(path: &str, scale: f64, align: (&str, &str), centre: (f64, f64), drop: f64) -> Option<IInspectable> {
    let shrink = 1.0 / scale.max(0.5);
    let markup = format!(
        r#"<ImageBrush xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" ImageSource="{}" Stretch="None" AlignmentX="{}" AlignmentY="{}"><ImageBrush.RelativeTransform><TransformGroup><ScaleTransform CenterX="{}" CenterY="{}" ScaleX="{shrink}" ScaleY="{shrink}"/><TranslateTransform Y="{drop}"/></TransformGroup></ImageBrush.RelativeTransform></ImageBrush>"#,
        file_uri(path),
        align.0,
        align.1,
        centre.0,
        centre.1,
    );
    unsafe { xaml::load_xaml(&markup) }
}

// ---- Tooltips ----

/// The tooltip on the element `owner` (ours to release), if it has one.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn tooltip_of(diagnostics: &xamlom::IXamlDiagnostics, owner: u64) -> Option<IInspectable> {
    use crate::winrt::{IDependencyObject, IToolTipServiceStatics, TOOLTIP_SERVICE};
    let service = xaml::factory::<IToolTipServiceStatics>(TOOLTIP_SERVICE)?;
    let object = xaml::element::<IDependencyObject>(diagnostics, owner)?;
    let mut raw: *mut core::ffi::c_void = core::ptr::null_mut();
    if service.GetToolTip(object.as_raw(), &mut raw).is_err() || raw.is_null() {
        return None;
    }
    Some(IInspectable::from_raw(raw))
}

/// Put `tip` on the element `owner` as its tooltip (`None`: none); `false` if it would not take.
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn set_tooltip(diagnostics: &xamlom::IXamlDiagnostics, owner: u64, tip: Option<&IInspectable>) -> bool {
    use crate::winrt::{IDependencyObject, IToolTipServiceStatics, TOOLTIP_SERVICE};
    let Some(service) = xaml::factory::<IToolTipServiceStatics>(TOOLTIP_SERVICE) else { return false };
    let Some(object) = xaml::element::<IDependencyObject>(diagnostics, owner) else { return false };
    service.SetToolTip(object.as_raw(), tip.map_or(core::ptr::null_mut(), |t| t.as_raw())).is_ok()
}

// ---- Markup ----

/// `text` as an XAML attribute's value (or element text): markup escaped, line breaks kept.
pub(crate) fn xml_attr(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\n' => out.push_str("&#10;"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// `C:\a b\c.png` as `file:///C:/a%20b/c.png`, escaped for an XAML attribute.
pub(crate) fn file_uri(path: &str) -> String {
    let mut uri = String::from("file:///");
    for c in path.chars() {
        match c {
            '\\' => uri.push('/'),
            ' ' => uri.push_str("%20"),
            '%' => uri.push_str("%25"),
            '#' => uri.push_str("%23"),
            c => uri.push(c),
        }
    }
    xml_attr(&uri)
}

/// Log the recorded tree under `root`, as `what` holds it.
pub(crate) fn describe(root: u64, what: &str) {
    logf!("{what} holds:");
    let mut queue = vec![(root, 0usize)];
    let mut listed = 0;
    while let Some((handle, depth)) = queue.pop() {
        if listed >= 80 {
            break;
        }
        listed += 1;
        let name = tree::name_of(handle).filter(|n| !n.is_empty()).map(|n| format!("#{n}")).unwrap_or_default();
        logf!("  {}{}{name}", "  ".repeat(depth), tree::type_of(handle).unwrap_or_default());
        queue.extend(tree::children_of(handle).into_iter().map(|c| (c, depth + 1)));
    }
}

#[cfg(test)]
mod tests {
    use super::{file_uri, xml_attr};

    #[test]
    fn markup_special_characters_are_escaped() {
        assert_eq!(xml_attr("Sturm & Drang"), "Sturm &amp; Drang");
        assert_eq!(xml_attr("<tag>"), "&lt;tag&gt;");
        assert_eq!(xml_attr("say \"hi\" 'there'"), "say &quot;hi&quot; &apos;there&apos;");
    }

    #[test]
    fn a_tooltip_keeps_its_lines_and_escapes_markup() {
        assert_eq!(xml_attr("PC: 43%\r\nG705 \"Mouse\" <BT> & co"), "PC: 43%&#10;G705 &quot;Mouse&quot; &lt;BT&gt; &amp; co");
    }

    #[test]
    fn badge_paths_become_file_uris_fit_for_an_attribute() {
        assert_eq!(file_uri(r"C:\Users\a b\x&y.png"), "file:///C:/Users/a%20b/x&amp;y.png");
        assert_eq!(file_uri(r#"C:\b%#\"q<.png"#), "file:///C:/b%25%23/&quot;q&lt;.png", "escaped for the address and the attribute");
    }
}

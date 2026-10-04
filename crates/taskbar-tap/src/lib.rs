//! The TAP (Test Access Point) DLL that runs inside `explorer.exe`.
//!
//! `InitializeXamlDiagnosticsEx` loads it, gets the object from `DllGetClassObject(CLSID_TAP)` and
//! calls `SetSite`; we subscribe with `AdviseVisualTreeChange`, which replays the existing tree as
//! `Add` mutations and then streams deltas. The callback only records the tree ([`tree`]); every
//! edit is made from the timer-driven [`sweep`], which holds the rules for when a XAML call is safe.

mod decorate;
mod interact;
mod ipc;
pub mod lifecycle;
mod log;
pub mod music;
mod reorder;
mod restore;
mod tree;
mod winrt;
pub mod xamlom;

use core::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, MutexGuard};
use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_POINTER, S_FALSE, S_OK};
use windows::Win32::System::Com::IClassFactory;
use windows::Win32::System::Com::IClassFactory_Impl;
use windows::Win32::System::Ole::{IObjectWithSite, IObjectWithSite_Impl};
use windows_core::{implement, Interface, IUnknownImpl, Ref, Result, GUID, HRESULT};

use crate::log::logf;
use xamlom::{
    bstr_to_string, IVisualTreeService3, IVisualTreeServiceCallback2,
    IVisualTreeServiceCallback_Impl, ParentChildRelation, VisualElement, VisualMutationType,
};

/// Our TAP's class id. Never registered anywhere — `InitializeXamlDiagnosticsEx`
/// passes it straight to our own `DllGetClassObject`.
pub const CLSID_TAP: GUID = GUID::from_u128(tap_proto::CLSID_TAP);

pub use tap_proto::ENDPOINT_NAME;

/// The DLL that exports `InitializeXamlDiagnosticsEx`.
pub const XAML_DLL: &str = "Windows.UI.Xaml.dll";

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Runs a COM method body, turning a panic into `E_UNEXPECTED`: nothing may unwind into Explorer.
fn guarded<T>(what: &str, body: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).unwrap_or_else(|_| {
        logf!("{what} panicked");
        Err(windows::Win32::Foundation::E_UNEXPECTED.into())
    })
}

/// Locks a mutex, taking the contents of a poisoned one rather than panicking (a panic would
/// abort Explorer; a poisoned lock holds at most a stale handle, which readers re-validate).
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------
// The TAP object
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Site {
    site: Option<windows_core::IUnknown>,
    service: Option<IVisualTreeService3>,
    diagnostics: Option<xamlom::IXamlDiagnostics>,
    /// Which [`GENERATION`] this instance was configured for.
    generation: u64,
}

/// Which TAP instance is allowed to act: re-injecting builds a second TAP rather than reusing
/// this one, and a reverted instance must go quiet.
///
/// Each `SetSite` takes a fresh number from [`GENERATION`] and makes it [`CURRENT`];
/// [`stand_down`] clears `CURRENT` to 0, and a handover ([`hand_over`]) makes the newest
/// instance current again. An instance whose generation is not `CURRENT` drops its callbacks.
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static CURRENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The `(icon, ContentPresenter)` we last decorated. Not a one-shot flag: the icon can be
/// rebuilt, and the shell data-binds `Content` and may overwrite our strip, so both are re-checked.
static DECORATED: Mutex<Option<(xamlom::InstanceHandle, xamlom::InstanceHandle)>> =
    Mutex::new(None);

fn decorated_pair() -> Option<(xamlom::InstanceHandle, xamlom::InstanceHandle)> {
    *lock(&DECORATED)
}

/// Whether our strip is actually on the taskbar. Gates every edit to the shell's own UI (hiding
/// its indicators, reordering): never take its controls away without ours in their place.
fn strip_placed() -> bool {
    decorated_pair().is_some()
}

/// # Safety
/// XAML UI thread only.
unsafe fn already_decorated(diagnostics: &xamlom::IXamlDiagnostics) -> bool {
    let Some((icon, presenter)) = decorated_pair() else {
        return false;
    };
    // Gone from the tree entirely → the icon was rebuilt, decorate the new one.
    if tree::type_of(icon).is_none() {
        return false;
    }
    // Still present, but the shell may have overwritten our content since.
    decorate::holds_our_strip(diagnostics, presenter)
}

/// Set once the tray sections have been reordered.
static REORDERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Segment elements that already have their pointer handlers, so a tray rebuild
/// wires the new ones without double-wiring the old.
static WIRED: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// One of Explorer's own indicators that our strip replaces and therefore hides (the volume
/// glyph, the "microphone in use" icon): found by glyph, collapsed with its slot, restorable.
struct Indicator {
    what: &'static str,
    /// Codepoints that identify it. Names are localised; glyphs are not.
    glyphs: &'static [char],
    /// Whether audio-tray asked for this one to be hidden.
    wanted: fn(&decorate::StripState) -> bool,
    /// The icon element and the container holding its slot, once found.
    slot: Mutex<Option<(xamlom::InstanceHandle, xamlom::InstanceHandle)>>,
    /// Collapse re-applications for the current slot. Bounded, because a collapsed element keeps
    /// reporting its old `ActualWidth`, so success cannot be read back.
    retries: std::sync::atomic::AtomicU32,
    /// Whether it has been logged as found.
    announced: std::sync::atomic::AtomicBool,
}

impl Indicator {
    const fn new(
        what: &'static str,
        glyphs: &'static [char],
        wanted: fn(&decorate::StripState) -> bool,
    ) -> Self {
        Self {
            what,
            glyphs,
            wanted,
            slot: Mutex::new(None),
            retries: std::sync::atomic::AtomicU32::new(0),
            announced: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn recorded(&self) -> Option<(xamlom::InstanceHandle, xamlom::InstanceHandle)> {
        *lock(&self.slot)
    }

    /// Record where it is. A *different* element than last time restarts the retry budget (the
    /// microphone indicator may be rebuilt per recording session).
    fn record(&self, pair: (xamlom::InstanceHandle, xamlom::InstanceHandle)) {
        let mut held = lock(&self.slot);
        if *held != Some(pair) {
            self.retries.store(0, Ordering::SeqCst);
        }
        *held = Some(pair);
    }

    fn forget(&self) {
        *lock(&self.slot) = None;
        self.retries.store(0, Ordering::SeqCst);
        self.announced.store(false, Ordering::SeqCst);
    }
}

/// Enough to cover the replay burst without re-applying for the process lifetime.
const HIDE_MAX_RETRIES: u32 = 24;

static SYSTEM_VOLUME: Indicator =
    Indicator::new("volume", decorate::VOLUME_GLYPHS, |s| s.hide_system_volume);
static SYSTEM_MIC: Indicator =
    Indicator::new("microphone", decorate::MIC_GLYPHS, |s| s.hide_system_mic);

fn indicators() -> [&'static Indicator; 2] {
    [&SYSTEM_VOLUME, &SYSTEM_MIC]
}

/// Set once the tray icons' automation names have been logged.
static PROBED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set once the section map has been logged (it only needs saying once).
static REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the strip is wanted at all. [`GENERATION`] says which instance may act; this says
/// whether anything may, for the timer-driven sweep, which has no instance to compare.
static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn tid() -> u32 {
    unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }
}

/// A new reference to the stored `IXamlDiagnostics`, for code (timers, event handlers) with no
/// borrow to thread down.
pub(crate) fn diagnostics() -> Option<xamlom::IXamlDiagnostics> {
    let raw = DIAGNOSTICS.load(Ordering::SeqCst);
    if raw == 0 {
        return None;
    }
    let stored = core::mem::ManuallyDrop::new(unsafe {
        core::mem::transmute::<*mut c_void, xamlom::IXamlDiagnostics>(raw as *mut c_void)
    });
    Some((*stored).clone())
}

/// The live `IXamlDiagnostics` as a raw pointer, so other threads can reach it. Stored on every
/// `SetSite`; the previous pointer is leaked on purpose (the TAP is pinned in Explorer anyway).
static DIAGNOSTICS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Last thread the callback arrived on, so a change of thread gets logged.
static CALLBACK_TID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The tooltip (substring) of the tray icon to decorate, from the init data; empty means the
/// first icon. A mutex, not a `OnceLock`: every injection re-reads the init data.
static TARGET_TOOLTIP: Mutex<String> = Mutex::new(String::new());

/// What the strip renders, from the init data (re-read on every injection).
static STRIP: Mutex<Option<decorate::StripState>> = Mutex::new(None);

fn target_tooltip() -> String {
    lock(&TARGET_TOOLTIP).clone()
}

fn strip_state() -> Option<decorate::StripState> {
    *lock(&STRIP)
}

/// Pulls one `key=value` out of the `key=value;` initialization payload.
fn value_from(data: &str, wanted: &str) -> Option<String> {
    data.split(';')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| key.trim() == wanted)
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Apply a `key=value;` init payload: what to decorate, what to draw, and whom to report to.
/// Shared by `SetSite` and [`hand_over`]; touches no XAML, so it is safe in any message handler.
fn configure(data: &str) {
    log::set_verbose(value_from(data, "debug").as_deref() == Some("1"));
    *lock(&TARGET_TOOLTIP) = value_from(data, "tooltip").unwrap_or_default();
    *lock(&STRIP) = Some(decorate::StripState::parse(data));
    // `tile=<app name>` names the taskbar button for the music tile; absent disables that half.
    music::tile::set_host(value_from(data, "tile"));
    if let Some(width) = value_from(data, "strip").and_then(|w| w.parse().ok()) {
        music::layout::set_content_width(width);
    }
    if let Some(hwnd) = value_from(data, "hwnd").and_then(|h| h.parse::<isize>().ok()) {
        ipc::set_receiver(hwnd);
    }
    // Whoever asked for the strip is also who we put it away for.
    lifecycle::watch_owner(value_from(data, "pid"));
}

/// The app version this DLL shipped with (stamped by build.rs from the root manifest).
pub const APP_VERSION: &str = env!("AUDIO_TRAY_VERSION");

pub use tap_proto::{HANDOVER_ACCEPTED, HANDOVER_DECLINED};

/// Whether a new audio-tray may adopt this already-loaded TAP: the same app version, and the
/// same DLL file it would have injected. Anything else needs a fresh Explorer.
fn compatible(data: &str, own_path: Option<&str>) -> core::result::Result<(), String> {
    match value_from(data, "ver") {
        Some(version) if version == APP_VERSION => {}
        other => return Err(format!("exe is v{}, TAP is v{APP_VERSION}", other.unwrap_or_default())),
    }
    if let (Some(wanted), Some(own)) = (value_from(data, "tap"), own_path) {
        if !wanted.eq_ignore_ascii_case(own) {
            return Err(format!("exe wants {wanted}, TAP is {own}"));
        }
    }
    Ok(())
}

/// A new audio-tray adopting this TAP instead of restarting Explorer: take its init data, make
/// the newest instance current again and let the sweep re-apply whatever is missing.
///
/// Runs inside a cross-process `SendMessage`, so it only flips state; all XAML work is left to
/// the sweep timer.
pub(crate) fn hand_over(data: &str) -> isize {
    if let Err(why) = compatible(data, lifecycle::own_module_path().as_deref()) {
        logf!("handover declined: {why}");
        return HANDOVER_DECLINED;
    }
    let before = strip_state();
    configure(data);
    // A revert still pending from the old owner is superseded: the new owner wants the strip.
    STAND_DOWN_PENDING.store(false, Ordering::SeqCst);
    let was_active = ACTIVE.swap(true, Ordering::SeqCst);
    CURRENT.store(GENERATION.load(Ordering::SeqCst), Ordering::SeqCst);
    if was_active && strip_state() != before {
        // The strip is up but shows the old owner's state: redraw it, as a restyle would.
        *lock(&DECORATED) = None;
        lock(&WIRED).clear();
    }
    logf!("handover accepted (strip was {}): init data = {data:?}", if was_active { "up" } else { "reverted" });
    unsafe { lifecycle::set_sweep_pace(false) };
    HANDOVER_ACCEPTED
}

/// A raw COM pointer handed to the advise thread, used only for the one `AdviseVisualTreeChange`
/// call (which marshals internally), as the known-good C++ TAPs do.
struct SendPtr(*mut c_void);
unsafe impl Send for SendPtr {}

/// Call `AdviseVisualTreeChange` off-thread, taking a reference on both objects
/// for the duration so neither can die under the call.
fn advise_on_new_thread(service: &IVisualTreeService3, callback: &IVisualTreeServiceCallback2) {
    let service_ref = SendPtr(service.clone().into_raw());
    let callback_ref = SendPtr(callback.clone().into_raw());
    std::thread::spawn(move || {
        logf!("advise thread {}", tid());
        let service = service_ref;
        let callback = callback_ref;
        let hr = unsafe {
            let svc = core::mem::transmute::<*mut c_void, IVisualTreeService3>(service.0);
            let hr = svc.AdviseVisualTreeChange(callback.0);
            drop(svc); // releases our reference
            hr
        };
        logf!("AdviseVisualTreeChange -> 0x{:08x}", hr.0);
        if hr.is_err() {
            // Drop the callback reference too; nothing will call us.
            drop(unsafe { core::mem::transmute::<*mut c_void, IVisualTreeServiceCallback2>(callback.0) });
        }
    });
}

// Only the v2 callback is declared: it contains v1's slot and answers QI for v1 too; declaring
// both would make QI ambiguous.
// `Agile = false` is load-bearing: it makes COM call us back on the XAML UI thread's apartment
// (agile, callbacks arrive on arbitrary threads and WinRT calls stall). See FINDINGS.md, "Threading, settled".
#[implement(IObjectWithSite, IVisualTreeServiceCallback2, Agile = false)]
struct Tap {
    site: Mutex<Site>,
}

impl Tap {
    fn new() -> Self {
        Self {
            site: Mutex::new(Site::default()),
        }
    }
}

impl Tap_Impl {
    fn state(&self) -> MutexGuard<'_, Site> {
        lock(&self.site)
    }
}

impl IObjectWithSite_Impl for Tap_Impl {
    fn SetSite(&self, punksite: Ref<'_, windows_core::IUnknown>) -> Result<()> {
        guarded("SetSite", || self.set_site(punksite))
    }

    fn GetSite(&self, riid: *const GUID, ppvsite: *mut *mut c_void) -> Result<()> {
        guarded("GetSite", || {
            if ppvsite.is_null() {
                return Err(E_POINTER.into());
            }
            unsafe { *ppvsite = core::ptr::null_mut() };
            match self.state().site.as_ref() {
                Some(site) => unsafe { site.query(riid, ppvsite).ok() },
                None => Err(E_POINTER.into()),
            }
        })
    }
}

impl Tap_Impl {
    fn set_site(&self, punksite: Ref<'_, windows_core::IUnknown>) -> Result<()> {
        logf!("SetSite on thread {}", tid());
        // Detach from any previous site first (SetSite(null) is also the teardown), so no stale
        // subscription is left behind.
        let previous = {
            let mut state = self.state();
            state.site = None;
            state.service.take()
        };
        if let Some(previous) = previous {
            let callback: IVisualTreeServiceCallback2 = self.to_interface();
            let _ = unsafe { previous.UnadviseVisualTreeChange(callback.as_raw()) };
            logf!("SetSite: detached from previous site");
        }

        let Ok(site) = punksite.ok() else {
            logf!("SetSite(null) — TAP detached");
            return Ok(());
        };

        // The site is the diagnostics object itself; both interfaces come off it.
        let service: IVisualTreeService3 = site.cast()?;
        let diagnostics = site.cast::<xamlom::IXamlDiagnostics>();
        match &diagnostics {
            Ok(diagnostics) => {
                let mut raw: *mut u16 = core::ptr::null_mut();
                let hr = unsafe { diagnostics.GetInitializationData(&mut raw) };
                let data = if hr == S_OK {
                    let text = unsafe { bstr_to_string(raw) };
                    // The out-BSTR is ours to free.
                    drop(unsafe { windows_core::BSTR::from_raw(raw) });
                    text
                } else {
                    String::new()
                };
                configure(&data);
                logf!("SetSite: IXamlDiagnostics ok, init data = {data:?}");
            }
            Err(err) => logf!("SetSite: no IXamlDiagnostics ({err}) — continuing"),
        }

        tree::start_watchdog();

        // Publish the site state BEFORE subscribing: the replay can start as soon as the advise
        // thread runs, and anything the callback needs must already be visible.
        {
            let mut state = self.state();
            // Claim the current generation, superseding whichever instance held it.
            state.generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
            CURRENT.store(state.generation, Ordering::SeqCst);
            state.site = Some(site.clone());
            state.service = Some(service.clone());
            state.diagnostics = diagnostics.ok();
            if let Some(diagnostics) = state.diagnostics.as_ref() {
                DIAGNOSTICS.store(diagnostics.clone().into_raw() as usize, Ordering::SeqCst);
            }
        }

        ACTIVE.store(true, Ordering::SeqCst);

        // Advise from a fresh thread: from the site's own thread it can hang in
        // `Advising::RunOnUIThread` and freeze the shell (per Windhawk's Taskbar Styler).
        let callback: IVisualTreeServiceCallback2 = self.to_interface();
        advise_on_new_thread(&service, &callback);
        Ok(())
    }
}

impl IVisualTreeServiceCallback_Impl for Tap_Impl {
    unsafe fn OnVisualTreeChange(
        &self,
        relation: ParentChildRelation,
        element: VisualElement,
        mutation_type: VisualMutationType,
    ) -> HRESULT {
        // Nothing may unwind into Explorer.
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let now = tid();
            if CALLBACK_TID.swap(now, Ordering::SeqCst) != now {
                logf!("OnVisualTreeChange on thread {now}");
            }
            let type_name = bstr_to_string(element.type_name);
            let name = bstr_to_string(element.name);
            let added = mutation_type == VisualMutationType::ADD;

            // Record first, so the parent links below are already populated.
            tree::record(
                relation.parent,
                relation.child,
                relation.child_index,
                type_name.clone(),
                name.clone(),
                added,
                element.handle,
                element.num_children,
            );

            // Explorer calls back on several islands' threads; track the one the latest tray
            // event came from (see `adopt_tray_thread`).
            if type_name.starts_with("SystemTray.") {
                lifecycle::adopt_tray_thread();
            }

            // The control window (revert channel + sweep timer) must be owned by the tray thread.
            // Created even for a superseded instance — the window outlives any one of them.
            if lifecycle::on_tray_thread() {
                lifecycle::ensure_window();
                // A revert whose owner died before the window existed.
                if let Some(pid) = lifecycle::take_pending_revert() {
                    // Posted, not run: no XAML from inside this callback.
                    logf!("posting the deferred revert");
                    lifecycle::request_revert(pid);
                }
            }

            // Only the edits below are gated by generation; recording above must stay ungated
            // (it is idempotent, and a gap between stand-down and re-injection would orphan nodes).
            if self.state().generation != CURRENT.load(Ordering::SeqCst) {
                return;
            }

            // A hover-preview transport button was just built: ask for it to be wired now (see
            // [`wire_transport`]). Above the tray-thread gate on purpose: the flyout is announced on
            // varying threads, and the post (not a XAML call) moves the work to the tray thread.
            if added && type_name == music::thumbbar::BUTTON_TYPE {
                lifecycle::nudge_transport();
            }

            // The shell rebuilt a tracked button's progress bar or running pill from its template:
            // ask for a re-pin now (posted — the write happens outside this callback).
            if added && (name == "ProgressIndicator" || name == "RunningIndicator") && music::is_tracked_part(element.handle) {
                lifecycle::nudge_repin();
            }

            // Below only handles the tray island's events, whose handles are that island's to use.
            // (Defensive: type/name triggers match in every island.)
            if !lifecycle::on_tray_thread() {
                return;
            }

            // **Nothing below touches XAML**: a WinRT call from inside the event stream can fail
            // to return and wedge the taskbar. Mutations live in `sweep`; this only queues the
            // handles that the recorded tree cannot give back later.

            // A possible system indicator glyph (matched by codepoint in the sweep, since reading
            // `Text` is a XAML call). Fast pace: the microphone icon appears only when recording
            // starts, and must be collapsed before it is seen.
            if added && name == "InnerTextBlock" {
                enqueue(&PENDING_GLYPHS, element.handle);
                if lifecycle::on_tray_thread() {
                    lifecycle::set_sweep_pace(false);
                }
            }

            // Our own segments announced back. Children are reported before parents, so the
            // segment's hover plate is already recorded.
            if added {
                if let Some(segment) = interact::Segment::from_name(&name) {
                    enqueue(&PENDING_SEGMENTS, (segment, name.clone(), element.handle));
                }
            }
        }));
        if caught.is_err() {
            logf!("OnVisualTreeChange panicked — event dropped");
        }
        S_OK
    }
}

/// Undo everything and go quiet (feature turned off, or audio-tray gone). The DLL stays loaded,
/// inert, on purpose: see FINDINGS.md, "Turning it off — revert in place, never unload".
///
/// # Safety
/// XAML UI thread only.
pub(crate) unsafe fn stand_down() {
    // First, so neither a callback nor the sweep re-applies behind the revert.
    ACTIVE.store(false, Ordering::SeqCst);
    CURRENT.store(0, Ordering::SeqCst);
    STAND_DOWN_PENDING.store(false, Ordering::SeqCst);

    match diagnostics() {
        Some(diagnostics) => {
            restore::revert(&diagnostics);
            // The music tile keeps its own record (widths and margins on the shell's elements).
            music::revert(&diagnostics);
        }
        None => logf!("stand down: no IXamlDiagnostics — cannot revert"),
    }

    // Reset the "done yet?" state so the feature can be re-enabled without restarting Explorer.
    *lock(&DECORATED) = None;
    for indicator in indicators() {
        indicator.forget();
    }
    lock(&WIRED).clear();
    REORDERED.store(false, Ordering::SeqCst);
    PROBED.store(false, Ordering::SeqCst);
    REPORTED.store(false, Ordering::SeqCst);
    logf!("stood down — the taskbar is as we found it");
}

/// Event handles the callback queued for the sweep (the callback may not touch XAML). Pushed only
/// from the tray island's thread.
static PENDING_GLYPHS: Mutex<Vec<xamlom::InstanceHandle>> = Mutex::new(Vec::new());
static PENDING_SEGMENTS: Mutex<Vec<(interact::Segment, String, xamlom::InstanceHandle)>> =
    Mutex::new(Vec::new());

/// Takes everything queued, leaving the queue empty.
fn drain<T>(queue: &Mutex<Vec<T>>) -> Vec<T> {
    std::mem::take(&mut *lock(queue))
}

fn enqueue<T>(queue: &Mutex<Vec<T>>, item: T) {
    lock(queue).push(item);
}

/// Set while we are inside a XAML call. An STA pumps messages during outgoing COM calls, so a
/// `WM_TIMER` or posted message can be dispatched inside `put_Content` on this same thread.
static XAML_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// RAII claim on [`XAML_BUSY`]. `None` means someone else already holds it.
struct BusyGuard;

impl BusyGuard {
    fn claim() -> Option<Self> {
        (!XAML_BUSY.swap(true, Ordering::SeqCst)).then_some(BusyGuard)
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        XAML_BUSY.store(false, Ordering::SeqCst);
    }
}

/// Re-applies whatever should be true and is not (the shell can overwrite our strip with no
/// further event). Every XAML mutation in the TAP happens from here.
///
/// # Safety
/// XAML UI thread only (the control window's timer).
pub(crate) unsafe fn sweep() {
    // Skipped if a XAML call is already in flight on this thread; the next tick retries.
    let Some(_busy) = BusyGuard::claim() else {
        return;
    };
    sweep_claimed();
    // A revert that arrived mid-sweep (see [`revert`]) runs now, still under the claim.
    if STAND_DOWN_PENDING.swap(false, Ordering::SeqCst) {
        stand_down();
    }
}

/// Whether the strip is still wanted. Checked between the sweep's phases: a revert pumped in
/// during one of its XAML calls clears it, and nothing may be written after that.
pub(crate) fn live() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

/// A revert that could not run because a XAML call was in flight; the sweep finishes it.
static STAND_DOWN_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Answer a revert request: stand down now, or, if a XAML call is in flight further up this
/// stack, stop all writing at once and leave the restore to the sweep.
///
/// # Safety
/// Tray thread only.
pub(crate) unsafe fn revert() {
    let Some(_busy) = BusyGuard::claim() else {
        ACTIVE.store(false, Ordering::SeqCst);
        CURRENT.store(0, Ordering::SeqCst);
        STAND_DOWN_PENDING.store(true, Ordering::SeqCst);
        unsafe { lifecycle::set_sweep_pace(false) };
        logf!("revert deferred — a XAML call is in flight");
        return;
    };
    stand_down();
}

/// The body of [`sweep`], under its busy claim.
unsafe fn sweep_claimed() {
    if !live() {
        return;
    }
    let Some(diagnostics) = diagnostics() else {
        return;
    };

    // Ahead of the mutation gate on purpose: attaching handlers writes nothing (see [`wire_transport`]).
    if tree::quiet_for(QUIET_BEFORE_WIRING) {
        wire_transport_now(&diagnostics);
    }

    // **Do not remove.** `put_Content` while the event stream is still running never returns and
    // wedges the whole taskbar; the timer is only the driver, this check is the guard.
    // See FINDINGS.md, "Resolved: never mutate from inside the event stream".
    if !tree::quiet_for(QUIET_BEFORE_MUTATING) {
        return;
    }

    // Order matters: record indicator slots (ungated), then decorate, then the edits gated on
    // `strip_placed()`.
    for text_block in drain(&PENDING_GLYPHS) {
        note_system_indicator(&diagnostics, text_block);
    }

    try_decorate(&diagnostics);
    if !live() {
        return;
    }

    enforce_hidden(&diagnostics);
    if !live() {
        return;
    }

    if strip_placed() && !REORDERED.load(Ordering::SeqCst) && reorder::sections_ready() {
        reorder_now(&diagnostics);
    }

    // After `try_decorate`: the segments only exist once the strip is placed.
    for (segment, name, element) in drain(&PENDING_SEGMENTS) {
        attach_segment(&diagnostics, segment, &name, element);
    }

    report_slot_metrics(&diagnostics);
    if !live() {
        return;
    }

    // The music tile: independent of the above, but relies on the same two guards.
    music::sweep(&diagnostics);
    if !live() {
        return;
    }

    // Drop to the idle pace once nothing is left to apply. Segments are wired on the tick after
    // the strip is drawn, hence `segments_wired`. With a music tile the sweep never settles: the
    // hover preview's buttons are rebuilt per hover and must be wired within it.
    let settled = strip_placed()
        && REORDERED.load(Ordering::SeqCst)
        && segments_wired()
        && music::tile::host().is_none();
    lifecycle::set_sweep_pace(settled);
}

/// Redraws the strip with a new device state: marks it stale and runs [`sweep`] at once (which
/// still applies the quiet-stream gate; the timer retries if it declines). The restore record is
/// safe: [`restore::remember_content`] never records our own strip.
///
/// # Safety
/// Tray thread only (the control window's procedure).
pub(crate) unsafe fn restyle(
    output: Option<char>,
    output_muted: bool,
    input: Option<char>,
    input_muted: bool,
    input_recording: bool,
) {
    {
        let mut strip = lock(&STRIP);
        let state = strip.get_or_insert_with(decorate::StripState::default);
        let wanted = decorate::StripState {
            output_glyph: output.unwrap_or(state.output_glyph),
            input_glyph: input.unwrap_or(state.input_glyph),
            output_muted,
            input_muted,
            input_recording,
            ..*state
        };
        // A restyle that changes nothing is dropped before it costs a rebuild.
        if wanted == *state {
            logf!("restyle: already showing that — nothing to do");
            return;
        }
        *state = wanted;
    }
    logf!(
        "restyle: out={:?} (muted {output_muted}) in={:?} (muted {input_muted}, recording {input_recording})",
        output,
        input
    );

    // Forces the redraw and re-wiring (the segments are replaced wholesale).
    *lock(&DECORATED) = None;
    lock(&WIRED).clear();
    // Fast pace first, so a declined sweep is retried soon.
    lifecycle::set_sweep_pace(false);
    unsafe { sweep() };
}

/// Whether the strip's segments have had their pointer handlers attached.
fn segments_wired() -> bool {
    !lock(&WIRED).is_empty()
}

/// Attach handlers to the hover preview's transport buttons behind only the short
/// [`QUIET_BEFORE_WIRING`] gate: it writes nothing, and the shell rebuilds these buttons per hover,
/// so the 400 ms mutation gate would leave the first press dead. Keep *some* gate: an unmeasured
/// risk whose failure mode is a wedged taskbar.
///
/// # Safety
/// XAML UI thread only (the tray island's).
pub(crate) unsafe fn wire_transport() {
    if !ACTIVE.load(Ordering::SeqCst) {
        return;
    }
    let Some(_busy) = BusyGuard::claim() else {
        return;
    };
    if !tree::quiet_for(QUIET_BEFORE_WIRING) {
        // The sweep (fast pace, same short gate) is the fallback.
        return;
    }
    let Some(diagnostics) = diagnostics() else {
        return;
    };
    wire_transport_now(&diagnostics);
}

/// The body of [`wire_transport`], for callers that already hold the diagnostics and the busy claim.
///
/// # Safety
/// XAML UI thread only.
unsafe fn wire_transport_now(diagnostics: &xamlom::IXamlDiagnostics) {
    let Some(host) = music::tile::host() else {
        return;
    };
    music::thumbbar::wire(diagnostics, &host);
}

/// Silence required before *attaching a handler* (two frames at 60 Hz: past a flyout's build burst).
const QUIET_BEFORE_WIRING: std::time::Duration = std::time::Duration::from_millis(32);

/// Silence required before the event-driven re-pin ([`repin`]). Short, unlike `put_Content`'s 400 ms:
/// a re-pin only sets layout properties on existing elements and creates nothing. See FINDINGS.md,
/// "Re-pinning the indicators on the event, not the sweep".
const QUIET_BEFORE_REPIN: std::time::Duration = std::time::Duration::from_millis(32);

/// Re-pin the music tile's indicators right after the shell rebuilt one, instead of waiting for the
/// sweep and its 400 ms gate (during which the template default — centred, natural width — shows).
/// Returns `false` to be retried shortly: the thread is mid-call, or the burst is still going.
///
/// # Safety
/// Tray thread only (the control window's procedure).
pub(crate) unsafe fn repin() -> bool {
    if !live() {
        return true;
    }
    let Some(_busy) = BusyGuard::claim() else {
        return false;
    };
    if !tree::quiet_for(QUIET_BEFORE_REPIN) {
        return false;
    }
    if let Some(diagnostics) = diagnostics() {
        music::repin(&diagnostics);
    }
    true
}

/// [`sweep`], timed: logs the average and worst cost once every [`SWEEP_REPORT_EVERY`] ticks.
///
/// # Safety
/// As for [`sweep`].
pub(crate) unsafe fn timed_sweep() {
    static STATS: Mutex<(u32, u128, u128)> = Mutex::new((0, 0, 0));
    static REPORTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let started = std::time::Instant::now();
    sweep();
    let micros = started.elapsed().as_micros();
    let mut stats = lock(&STATS);
    stats.0 += 1;
    stats.1 += micros;
    stats.2 = stats.2.max(micros);
    // The first report comes early (half a minute), so a fresh session shows the cost at once.
    let every = if REPORTS.load(Ordering::SeqCst) == 0 { SWEEP_REPORT_FIRST } else { SWEEP_REPORT_EVERY };
    if stats.0 >= every {
        REPORTS.fetch_add(1, Ordering::SeqCst);
        logf!("sweep cost: avg {} us, max {} us over {} ticks", stats.1 / u128::from(stats.0), stats.2, stats.0);
        *stats = (0, 0, 0);
    }
}

/// About ten minutes at the fast pace.
const SWEEP_REPORT_EVERY: u32 = 2400;
const SWEEP_REPORT_FIRST: u32 = 120;

/// Silence required before mutating XAML: longer than the gaps within a replay burst.
const QUIET_BEFORE_MUTATING: std::time::Duration = std::time::Duration::from_millis(400);

/// Logs, once, the shell's icon slot and hover plate sizes against our pill (diagnostic for how
/// evenly the shell's hover plate surrounds the strip). Retries each tick until layout has run.
///
/// # Safety
/// XAML UI thread only.
unsafe fn report_slot_metrics(diagnostics: &xamlom::IXamlDiagnostics) {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.load(Ordering::SeqCst) {
        return;
    }
    let Some((icon, presenter)) = decorated_pair() else {
        return;
    };
    let Some((slot_w, slot_h)) = decorate::actual_size(diagnostics, icon) else {
        return;
    };
    if slot_w <= 0.0 || slot_h <= 0.0 {
        return;
    }
    // The pill, not the presenter (which fills the slot).
    let Some((pill_w, pill_h)) = decorate::content_size(diagnostics, presenter) else {
        return;
    };
    if pill_w <= 0.0 || pill_h <= 0.0 {
        return;
    }
    logf!(
        "slot metrics: slot {slot_w}x{slot_h}, pill {pill_w}x{pill_h} \
         -> slot surround {} epx at the ends, {} top and bottom",
        (slot_w - pill_w) / 2.0,
        (slot_h - pill_h) / 2.0
    );

    // The visible hover highlight is a Border inside the icon (only its opacity depends on hover).
    // Breadth-first finds it before our pill, a level deeper.
    match decorate::descendant_of_class(diagnostics, icon, "Windows.UI.Xaml.Controls.Border")
        .and_then(|plate| decorate::actual_size(diagnostics, plate).map(|size| (plate, size)))
    {
        Some((plate, (plate_w, plate_h))) => logf!(
            "hover plate 0x{plate:x} {plate_w}x{plate_h} \
             -> plate surround {} epx at the ends, {} top and bottom",
            (plate_w - pill_w) / 2.0,
            (plate_h - pill_h) / 2.0
        ),
        None => logf!("hover plate: no Border found under the icon"),
    }
    DONE.store(true, Ordering::SeqCst);
}

/// Climbs the recorded tree looking for an ancestor of a given XAML type.
fn ancestor_of_type(start: xamlom::InstanceHandle, wanted: &str, max_up: usize) -> Option<u64> {
    let mut handle = start;
    for _ in 0..max_up {
        handle = tree::parent_of(handle)?;
        if tree::type_of(handle).as_deref() == Some(wanted) {
            return Some(handle);
        }
    }
    None
}

/// Move the notification area next to the wifi/battery button. Retried until it succeeds (the
/// sections trickle in).
///
/// # Safety
/// XAML UI thread only, and only once the event stream is quiet — see [`sweep`].
unsafe fn reorder_now(diagnostics: &xamlom::IXamlDiagnostics) {
    if !REPORTED.swap(true, Ordering::SeqCst) {
        reorder::report(diagnostics);
    }
    if reorder::move_after_language(diagnostics) {
        REORDERED.store(true, Ordering::SeqCst);
    }
}

/// Wires pointer handlers onto one of our segments, once per element handle (a tray rebuild
/// produces fresh segments).
///
/// # Safety
/// XAML UI thread only, and only once the event stream is quiet — see [`sweep`].
unsafe fn attach_segment(
    diagnostics: &xamlom::IXamlDiagnostics,
    segment: interact::Segment,
    name: &str,
    element: xamlom::InstanceHandle,
) {
    {
        let mut wired = lock(&WIRED);
        if wired.contains(&element) {
            return;
        }
        wired.push(element);
    }
    // The plate is named after the segment plus "Hover" (a contract with the markup).
    let plate_name = format!("{name}Hover");
    let Some(&plate) = tree::find_by_name(&plate_name).first() else {
        logf!("no hover plate {plate_name:?} recorded yet for 0x{element:x}");
        return;
    };
    interact::attach(diagnostics, segment, element, plate);
}

/// Note one of Explorer's own indicators if this text block is it.
///
/// Only *records* the slot, ungated; collapsing it is [`enforce_hidden`]'s job, gated on the
/// strip. (The glyph is announced during the replay, long before our icon is decorated.)
///
/// # Safety
/// XAML UI thread only, and only once the event stream is quiet — see [`sweep`].
unsafe fn note_system_indicator(
    diagnostics: &xamlom::IXamlDiagnostics,
    text_block: xamlom::InstanceHandle,
) {
    let Some(state) = strip_state() else {
        return;
    };
    let Some(text) = decorate::text_of(diagnostics, text_block) else {
        return;
    };
    let Some(glyph) = text.chars().next() else {
        return;
    };
    let found = indicators()
        .into_iter()
        .find(|i| (i.wanted)(&state) && i.glyphs.contains(&glyph));
    let Some(wanted) = found else {
        note_unknown_glyph(glyph);
        return;
    };

    // Only the shell's glyphs sit in a `SystemTray.IconView`; our own strip's fall out here.
    let Some(icon) = ancestor_of_type(text_block, "SystemTray.IconView", 8) else {
        return;
    };

    // The generated `ContentPresenter` around the icon keeps its layout box, so it must be
    // collapsed too or a hole remains.
    let slot = tree::parent_of(icon)
        .filter(|&parent| {
            tree::type_of(parent).as_deref() == Some("Windows.UI.Xaml.Controls.ContentPresenter")
        })
        .unwrap_or(icon);

    if !wanted.announced.swap(true, Ordering::SeqCst) {
        logf!(
            "system {} indicator found: glyph {:04X} in IconView 0x{icon:x}, slot 0x{slot:x} (hidden only once our strip is placed)",
            wanted.what,
            glyph as u32
        );
    }
    // Remembered so it can be re-applied: a collapse before layout has measured it frees nothing.
    wanted.record((icon, slot));
    enforce_hidden(diagnostics);
}

/// Logs a tray glyph we do not recognise, once per codepoint: the diagnostic for a Windows build
/// whose "microphone in use" glyph is missing from [`decorate::MIC_GLYPHS`].
fn note_unknown_glyph(glyph: char) {
    /// Cap on distinct glyphs logged.
    const MAX_SEEN: usize = 48;

    let mut seen = lock(&UNKNOWN_GLYPHS);
    if seen.contains(&glyph) || seen.len() >= MAX_SEEN {
        return;
    }
    seen.push(glyph);
    logf!("tray glyph {:04X} is not one we hide", glyph as u32);
}

/// Codepoints [`note_unknown_glyph`] has already reported.
static UNKNOWN_GLYPHS: Mutex<Vec<char>> = Mutex::new(Vec::new());

/// Re-applies each indicator's collapse (bounded by [`HIDE_MAX_RETRIES`]) once layout has measured it.
///
/// # Safety
/// XAML UI thread only, and only once the event stream is quiet — see [`sweep`].
unsafe fn enforce_hidden(diagnostics: &xamlom::IXamlDiagnostics) {
    if !strip_placed() {
        return;
    }
    for indicator in indicators() {
        if indicator.retries.load(Ordering::SeqCst) >= HIDE_MAX_RETRIES {
            continue;
        }
        let Some((icon, slot)) = indicator.recorded() else {
            continue;
        };
        // Zero means layout has not run yet, and a collapse now would do nothing.
        if decorate::actual_width(diagnostics, slot).is_none_or(|width| width <= 0.0) {
            continue;
        }
        // Before the collapse, never after (we would record our own zero width).
        restore::remember_layout(diagnostics, icon);
        decorate::collapse(diagnostics, icon);
        if slot != icon {
            restore::remember_layout(diagnostics, slot);
            decorate::collapse(diagnostics, slot);
        }
        if indicator.retries.fetch_add(1, Ordering::SeqCst) + 1 == HIDE_MAX_RETRIES {
            logf!(
                "system {} collapse re-applied {HIDE_MAX_RETRIES}x — stopping",
                indicator.what
            );
        }
    }
}

/// Find the tray icon we were asked to decorate and replace its content. Icons come from the
/// recorded tree; presenters must be looked up live (see [`decorate::descendant_presenter`]).
///
/// # Safety
/// XAML UI thread only.
unsafe fn try_decorate(diagnostics: &xamlom::IXamlDiagnostics) {
    // Logs why nothing was decorated, on change only (a count cap would spend itself on the replay).
    let why = |reason: &str| {
        static LAST: Mutex<String> = Mutex::new(String::new());
        let mut last = lock(&LAST);
        if *last != reason {
            logf!("try_decorate: {reason}");
            last.clear();
            last.push_str(reason);
        }
    };

    // Cheap check before the expensive live walk below.
    if already_decorated(diagnostics) {
        why("already decorated");
        report_slot_metrics(diagnostics);
        return;
    }

    let icons = tree::find_by_type("SystemTray.NotifyIconView");
    if icons.is_empty() {
        why("no SystemTray.NotifyIconView recorded yet");
        return;
    }
    let candidates: Vec<(xamlom::InstanceHandle, xamlom::InstanceHandle)> = icons
        .iter()
        .filter_map(|&icon| {
            decorate::descendant_presenter(diagnostics, icon).map(|presenter| (icon, presenter))
        })
        .collect();
    if candidates.is_empty() {
        why(&format!(
            "{} NotifyIconView(s) recorded, none with a live ContentPresenter",
            icons.len()
        ));
        return;
    }

    let target = target_tooltip();

    // Log once what each tray icon calls itself, so a tooltip mismatch is diagnosable.
    if !PROBED.swap(true, Ordering::SeqCst) {
        logf!("looking for tray icon named {target:?}; candidates:");
        for &(icon, _) in &candidates {
            for (handle, ty, name) in decorate::probe_names(diagnostics, icon, 6) {
                logf!("  icon 0x{icon:x}: {ty} [0x{handle:x}] = {name:?}");
            }
        }
    }

    for (icon, presenter) in candidates {
        let tooltip = decorate::automation_name(diagnostics, icon).unwrap_or_default();
        // Substring: the tooltip mostly names the current device; only the app's marker is stable.
        // An empty target takes the first icon.
        if !target.is_empty() && !tooltip.contains(&target) {
            why(&format!("icon 0x{icon:x} named {tooltip:?} is not {target:?}"));
            continue;
        }
        decorate_icon(diagnostics, icon, presenter, &tooltip);
        break;
    }
}

/// # Safety
/// XAML UI thread only.
unsafe fn decorate_icon(
    diagnostics: &xamlom::IXamlDiagnostics,
    icon: xamlom::InstanceHandle,
    presenter: xamlom::InstanceHandle,
    tooltip: &str,
) {
    logf!("decorating icon 0x{icon:x} (tooltip {tooltip:?}) via presenter 0x{presenter:x}");
    // The shell's own visual for this icon, kept alive so it can go back.
    restore::remember_content(diagnostics, presenter);
    let state = strip_state().unwrap_or_default();
    logf!(
        "strip state: accent={:?} hidevolume={} hidemic={} out={:04X} in={:04X}",
        state.accent,
        state.hide_system_volume,
        state.hide_system_mic,
        state.output_glyph as u32,
        state.input_glyph as u32
    );
    if decorate::set_chevron_content(diagnostics, presenter, state) {
        *lock(&DECORATED) = Some((icon, presenter));
    }
}

impl xamlom::IVisualTreeServiceCallback2_Impl for Tap_Impl {
    unsafe fn OnElementStateChanged(
        &self,
        element: xamlom::InstanceHandle,
        element_state: xamlom::VisualElementState,
        _context: *const u16,
    ) -> HRESULT {
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            logf!("OnElementStateChanged 0x{element:x} state={}", element_state.0);
        }));
        if caught.is_err() {
            logf!("OnElementStateChanged panicked");
        }
        S_OK
    }
}

// ---------------------------------------------------------------------------
// Class factory + DLL exports
// ---------------------------------------------------------------------------

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Ref<'_, windows_core::IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> Result<()> {
        guarded("CreateInstance", || {
            if ppvobject.is_null() {
                return Err(E_POINTER.into());
            }
            unsafe { *ppvobject = core::ptr::null_mut() };
            if !punkouter.is_null() {
                return Err(windows::Win32::Foundation::CLASS_E_NOAGGREGATION.into());
            }
            logf!("Factory::CreateInstance");
            let tap: IObjectWithSite = Tap::new().into();
            unsafe { tap.query(riid, ppvobject).ok() }
        })
    }

    fn LockServer(&self, _flock: windows_core::BOOL) -> Result<()> {
        Ok(())
    }
}

/// # Safety
/// COM entry point; the loader guarantees the pointers.
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    let caught = std::panic::catch_unwind(|| {
        if rclsid.is_null() || riid.is_null() || ppv.is_null() {
            return E_POINTER;
        }
        *ppv = core::ptr::null_mut();
        if *rclsid != CLSID_TAP {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        logf!("DllGetClassObject: handing out the TAP factory");
        let factory: IClassFactory = Factory.into();
        factory.query(riid, ppv)
    });
    caught.unwrap_or(E_POINTER)
}

/// Never unloadable: Explorer may still hold callbacks into our code.
#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handover_needs_the_same_version_and_dll() {
        let own = r"C:\App\audio_tray_tap.dll";
        let ok = format!(r"ver={APP_VERSION};tap=c:\app\AUDIO_TRAY_TAP.dll;pid=1");
        assert!(compatible(&ok, Some(own)).is_ok());
        // No path to compare (older payload, or the module path unreadable) still matches on version.
        assert!(compatible(&format!("ver={APP_VERSION}"), Some(own)).is_ok());
        assert!(compatible(&ok, None).is_ok());
        assert!(compatible("ver=0.0.1;pid=1", Some(own)).is_err());
        // An exe that predates the handover sends no version at all.
        assert!(compatible("pid=1", Some(own)).is_err());
        let other = format!(r"ver={APP_VERSION};tap=D:\dev\audio_tray_tap.dll");
        assert!(compatible(&other, Some(own)).is_err());
    }
}

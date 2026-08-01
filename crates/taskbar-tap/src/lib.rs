//! The TAP (Test Access Point) DLL — this is the half that runs inside
//! `explorer.exe`.
//!
//! Contract, as implemented by XAML diagnostics:
//!   1. `InitializeXamlDiagnosticsEx` (called from the injector) loads this DLL
//!      into the target and calls our `DllGetClassObject` for `CLSID_TAP`.
//!   2. It creates the object and hands it the diagnostics site via
//!      `IObjectWithSite::SetSite`.
//!   3. We QI the site for `IVisualTreeService3` and subscribe with
//!      `AdviseVisualTreeChange`, which replays the whole existing tree to us as
//!      `Add` mutations and then streams live deltas.
//!
//! M1 goal is only to prove that chain works from Rust and to see the tree, so
//! nothing here mutates anything.

mod decorate;
mod dispatch;
mod interact;
mod ipc;
pub mod lifecycle;
mod log;
mod reorder;
mod restore;
mod tree;
mod walk;
mod winrt;
pub mod xamlom;
mod xamltree;

use core::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
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
pub const CLSID_TAP: GUID = GUID::from_u128(0xb3e9_2816_117d_476f_936e_06ed_52b2_e55d);

/// The XAML diagnostics endpoint. This name is the reason the mechanism is
/// effectively single-consumer: TranslucentTB and Windhawk's Taskbar Styler use
/// the very same one.
pub const ENDPOINT_NAME: &str = "VisualDiagConnection1";

/// The DLL that exports `InitializeXamlDiagnosticsEx`.
pub const XAML_DLL: &str = "Windows.UI.Xaml.dll";

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
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

/// Which TAP instance is allowed to act.
///
/// Two things make a plain "am I set up?" flag insufficient. Injecting again
/// (audio-tray restarting, or the user turning the feature back on) does not
/// reuse the existing object — diagnostics builds a *second* TAP and advises it
/// too, so without this both would mutate the tray and both would record into the
/// one global visual tree. And after a revert the instance that made the changes
/// has to go quiet, or the next tray rebuild silently re-applies everything the
/// user just turned off.
///
/// Bumped on every `SetSite` and every [`stand_down`]. An instance whose stored
/// generation is not the current one drops its callbacks on the floor.
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The `(icon, ContentPresenter)` we last decorated.
///
/// Not a one-shot flag, for two reasons. The tray icon we care about usually
/// appears *after* the TAP has attached — audio-tray restarting, or Explorer
/// rebuilding the tray — so a one-shot would leave it undecorated forever. And
/// the shell data-binds the presenter's `Content`, so a set that lands mid-setup
/// gets overwritten; we have to notice and re-apply.
static DECORATED: Mutex<Option<(xamlom::InstanceHandle, xamlom::InstanceHandle)>> =
    Mutex::new(None);

fn decorated_pair() -> Option<(xamlom::InstanceHandle, xamlom::InstanceHandle)> {
    match DECORATED.lock() {
        Ok(guard) => *guard,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

/// Whether our strip is actually on the taskbar.
///
/// Gate for every edit to the *shell's own* UI. Hiding Windows' volume icon and
/// moving the notification area are only defensible as part of replacing them
/// with our strip — done on their own they take controls away and put nothing
/// back. That is not hypothetical: with the strip targeted at audio-tray's icon
/// by tooltip, an icon sitting in the overflow flyout is never decorated, and an
/// ungated build removed the volume icon and reordered the tray anyway.
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

/// One of Explorer's own indicators that our strip says instead, and therefore hides: the
/// volume glyph, and the "microphone in use" icon.
///
/// The two are the same problem — find a glyph `TextBlock` in the tray, collapse the icon and
/// the slot holding it, keep re-applying until layout actually gives the slot up, and
/// remember enough to put it back — so they are one type with two instances rather than two
/// copies of the logic.
struct Indicator {
    /// For the log.
    what: &'static str,
    /// Codepoints that identify it. Names are localised; glyphs are not.
    glyphs: &'static [char],
    /// Whether audio-tray asked for this one to be hidden.
    wanted: fn(&decorate::StripState) -> bool,
    /// The icon element and the container holding its slot, once found.
    slot: Mutex<Option<(xamlom::InstanceHandle, xamlom::InstanceHandle)>>,
    /// How many times the collapse has been re-applied for the *current* slot.
    ///
    /// There is no clean "it worked" signal to stop on: a collapsed element goes on
    /// reporting its last arranged `ActualWidth` (measured — still 24 long after the slot has
    /// visibly closed), so success cannot be read back off the element. A bounded retry is
    /// the honest way to cover the race instead.
    retries: std::sync::atomic::AtomicU32,
    /// Whether it has been found at all, so the log says so once rather than per sweep.
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
        match self.slot.lock() {
            Ok(guard) => *guard,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    /// Record where it is. A *different* element than last time restarts the retry budget:
    /// the microphone indicator comes and goes with each recording session, so an appearance
    /// can be a fresh element to hide, and a spent counter would leave it on screen.
    ///
    /// Measured on Win11 26200: the shell in fact *reuses* the same `IconView` for the next
    /// session, so the first collapse holds and no second announcement arrives — the log
    /// says "indicator found" once and the icon stays gone. This is for the case where it
    /// does not, which is not something to bet a visible regression on.
    fn record(&self, pair: (xamlom::InstanceHandle, xamlom::InstanceHandle)) {
        let mut held = match self.slot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if *held != Some(pair) {
            self.retries.store(0, Ordering::SeqCst);
        }
        *held = Some(pair);
    }

    fn forget(&self) {
        let mut held = match self.slot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *held = None;
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

/// Both, for the sweep and the revert.
fn indicators() -> [&'static Indicator; 2] {
    [&SYSTEM_VOLUME, &SYSTEM_MIC]
}

/// Set once the tray icons' automation names have been logged.
static PROBED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set once the section map has been logged (it only needs saying once).
static REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the strip is wanted at all.
///
/// [`GENERATION`] answers "which instance may act"; this answers "should anything
/// act". They are different questions, and the periodic sweep needs the second
/// one: it runs off a timer rather than a callback, so it has no instance to
/// compare generations against, and without this it would happily re-apply the
/// strip seconds after the user turned the feature off.
static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn tid() -> u32 {
    unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }
}

/// Borrows the stored `IXamlDiagnostics` without consuming the stored reference.
///
/// Event handlers reach it through here: they are invoked by XAML long after the
/// call that installed them has returned, so there is no borrow to thread down.
pub(crate) fn diagnostics() -> Option<xamlom::IXamlDiagnostics> {
    let raw = DIAGNOSTICS.load(Ordering::SeqCst);
    if raw == 0 {
        return None;
    }
    let stored = unsafe {
        core::mem::transmute::<*mut c_void, xamlom::IXamlDiagnostics>(raw as *mut c_void)
    };
    let borrowed = stored.clone();
    core::mem::forget(stored);
    Some(borrowed)
}

/// The live `IXamlDiagnostics`, as a raw pointer so other threads can reach it.
/// Set once in `SetSite` and intentionally never released — the TAP is pinned in
/// Explorer for the process lifetime anyway.
static DIAGNOSTICS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Last thread the callback arrived on, so a change is visible rather than
/// assumed. WinRT only works from the XAML UI thread, so which thread delivers
/// the replay decides whether any of our mutations can run at all.
static CALLBACK_TID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The tooltip of the tray icon to decorate, taken from the injector's
/// initialization data. Empty means "the first one", which is how the spike shows
/// itself off without audio-tray running.
///
/// Not a `OnceLock`: turning the feature off and on again injects a second time,
/// and a once-only cell would pin the first run's settings for the life of the
/// Explorer process — a changed accent colour would silently not take.
static TARGET_TOOLTIP: Mutex<String> = Mutex::new(String::new());

/// What the strip renders — also from the initialization data, and re-read on
/// every injection for the same reason as [`TARGET_TOOLTIP`].
static STRIP: Mutex<Option<decorate::StripState>> = Mutex::new(None);

fn target_tooltip() -> String {
    match TARGET_TOOLTIP.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

fn strip_state() -> Option<decorate::StripState> {
    match STRIP.lock() {
        Ok(guard) => *guard,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

/// Pulls one `key=value` out of the `key=value;` initialization payload.
fn value_from(data: &str, wanted: &str) -> Option<String> {
    data.split(';')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| key.trim() == wanted)
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// A raw COM pointer being handed to the advise thread. Both interfaces are
/// non-agile, but this mirrors what the known-good C++ TAPs do: the pointer is
/// only used for the one `AdviseVisualTreeChange` call, which marshals internally.
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

// Only the v2 callback is declared: its vtable already contains v1's slot, and
// the generated `matches` answers QueryInterface for the v1 IID too. Declaring
// both would build two vtables and make QI ambiguous.
//
// `Agile = false` is load-bearing. windows-core makes implementations agile by
// default, which lets COM invoke us on whatever thread happens to call — measured
// here as `OnVisualTreeChange` arriving on two different arbitrary threads, and
// every WinRT call from them stalling or failing. Non-agile forces COM to marshal
// back to the apartment the object was created on, which is the XAML UI thread.
// The known-good C++ TAPs declare `winrt::non_agile` for the same reason.
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
    fn state(&self) -> std::sync::MutexGuard<'_, Site> {
        match self.site.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl IObjectWithSite_Impl for Tap_Impl {
    fn SetSite(&self, punksite: Ref<'_, windows_core::IUnknown>) -> Result<()> {
        // A panic unwinding out of here would cross the COM boundary and abort
        // explorer.exe, so the whole body runs inside a catch.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.set_site(punksite)))
            .unwrap_or_else(|_| {
                logf!("SetSite panicked");
                Err(E_POINTER.into())
            })
    }

    fn GetSite(&self, riid: *const GUID, ppvsite: *mut *mut c_void) -> Result<()> {
        if ppvsite.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *ppvsite = core::ptr::null_mut() };
        match self.state().site.as_ref() {
            Some(site) => unsafe { site.query(riid, ppvsite).ok() },
            None => Err(E_POINTER.into()),
        }
    }
}

impl Tap_Impl {
    fn set_site(&self, punksite: Ref<'_, windows_core::IUnknown>) -> Result<()> {
        logf!("SetSite on thread {}", tid());
        // Detach from any previous site first — SetSite(null) is also how the
        // host tears us down, and leaving a stale subscription behind means
        // explorer calls back into a DLL that may be on its way out.
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
                    unsafe { bstr_to_string(raw) }
                } else {
                    String::new()
                };
                // Before any other logging, so the flag governs the whole session.
                log::set_verbose(value_from(&data, "debug").as_deref() == Some("1"));
                logf!("SetSite: IXamlDiagnostics ok, init data = {data:?}");
                // The injector passes both which icon to decorate and what to
                // draw in it, as a `key=value;` payload.
                if let Ok(mut tooltip) = TARGET_TOOLTIP.lock() {
                    *tooltip = value_from(&data, "tooltip").unwrap_or_default();
                }
                if let Ok(mut strip) = STRIP.lock() {
                    *strip = Some(decorate::StripState::parse(&data));
                }
                // Whoever asked for the strip is also who we put it away for.
                lifecycle::watch_owner(value_from(&data, "pid"));
            }
            Err(err) => logf!("SetSite: no IXamlDiagnostics ({err}) — continuing"),
        }

        tree::start_watchdog();

        // Publish the site state BEFORE subscribing. Advise runs on its own
        // thread (below) and the replay can begin immediately, so anything the
        // callback needs has to be visible first — otherwise the whole initial
        // burst is dropped on a `None` diagnostics, intermittently, depending on
        // how the two threads interleave.
        {
            let mut state = self.state();
            // Claim the current generation, standing down whichever instance
            // held it before.
            state.generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
            state.site = Some(site.clone());
            state.service = Some(service.clone());
            state.diagnostics = diagnostics.ok();
            // Also publish it where the watchdog thread can reach it.
            if let Some(diagnostics) = state.diagnostics.as_ref() {
                DIAGNOSTICS.store(diagnostics.clone().into_raw() as usize, Ordering::SeqCst);
            }
        }

        ACTIVE.store(true, Ordering::SeqCst);

        // Advise from a *fresh* thread. Windhawk's Taskbar Styler documents that
        // calling it from the site's own thread can hang in
        // `Advising::RunOnUIThread` — and a hang here freezes the shell.
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
        // A Rust panic unwinding through the COM boundary would abort the
        // process — i.e. take down the shell — so nothing is allowed to escape.
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let now = tid();
            if CALLBACK_TID.swap(now, Ordering::SeqCst) != now {
                // Ground truth for the question this all turns on.
                let access = self.state().diagnostics.clone().and_then(|d| {
                    dispatch::dispatcher(&d).and_then(|disp| dispatch::has_thread_access(&disp))
                });
                logf!("OnVisualTreeChange on thread {now}; HasThreadAccess = {access:?}");
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

            // Explorer runs several XAML islands and calls back on more than one
            // thread — measured, `OnVisualTreeChange` arriving on 9804 and 20008
            // in one session. Track whichever thread the most recent tray event
            // came in on; see `adopt_tray_thread` for why "most recent" rather
            // than "first", and why the test is this broad.
            if type_name.starts_with("SystemTray.") {
                lifecycle::adopt_tray_thread();
            }

            // The revert channel, and the sweep timer that rides on it. Both
            // deliver their messages to the thread that owns the window, so it
            // has to be this one. Created even for a superseded instance — the
            // window outlives any one of them.
            if lifecycle::on_tray_thread() {
                lifecycle::ensure_window();
                // A revert whose owner died before the window existed. Running it
                // here is the whole reason it was deferred: this is the thread
                // that may touch the tray.
                if lifecycle::take_pending_revert() {
                    logf!("running the deferred revert");
                    stand_down();
                }
            }

            // Recording happens above, unconditionally, and only the *edits*
            // below are gated. Gating the bookkeeping too was a bug: between a
            // stand-down and the next injection no instance holds the current
            // generation, so every event in that window — including our own tray
            // icon being destroyed — was dropped, leaving a childless orphan in
            // the tree that no later scan could ever complete. The symptom was a
            // re-enable that logged "1 NotifyIconView recorded" forever and never
            // drew. Recording is keyed by handle and idempotent, so two live
            // instances recording the same event costs nothing.
            if self.state().generation != GENERATION.load(Ordering::SeqCst) {
                return;
            }

            // Everything below touches the tray, so it may only run on the thread
            // that owns it. The triggers are element *types* and *names*, which
            // match in every island — "a ContentPresenter was added" fires on the
            // other island's thread too, and acting on it there wedges the call
            // forever. This is the single guard that made decoration reliable
            // instead of a coin flip.
            if !lifecycle::on_tray_thread() {
                return;
            }

            // **Nothing below touches XAML.** This callback is by definition inside
            // the visual-tree event stream, and any WinRT call against a tray
            // element from in there can fail to return — taking Explorer's UI
            // thread and the whole taskbar with it. Every mutation therefore lives
            // in `sweep`, which waits for the stream to fall quiet first.
            //
            // What is left here is bookkeeping: recording the tree, above, and
            // noting the two things that are identified by an event's *handle*
            // rather than by the recorded tree, since deferring those would
            // otherwise lose the handle.

            // Explorer's own volume and "microphone in use" indicators, which our strip
            // says instead. Each is a glyph *TextBlock* inside a SystemTray.IconView, so it
            // is matched on the codepoint rather than a (translated) name — done in the
            // sweep, because reading `Text` is itself a XAML call.
            //
            // Not one-shot: Explorer rebuilds the tray on DPI and monitor changes, and the
            // indicators come back. The microphone one is not even *born* until an app
            // starts recording, which is why the sweep is asked to go back to its fast pace
            // here — at the idle cadence the shell's icon would sit there for up to four
            // seconds of every call before being collapsed, which is exactly long enough
            // to be seen.
            if added && name == "InnerTextBlock" {
                enqueue(&PENDING_GLYPHS, element.handle);
                if lifecycle::on_tray_thread() {
                    lifecycle::set_sweep_pace(false);
                }
            }

            // Our own injected segments being announced back to us. XAML reports
            // children before parents, so by the time the segment `Grid` arrives
            // its hover plate is already recorded and findable by name.
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

/// Undo everything and go quiet.
///
/// The one exit path: the user turned the feature off, or audio-tray is gone.
/// Both mean the same thing here — put the shell back and stop touching it.
///
/// The DLL stays loaded, and that is deliberate. Unloading cannot do this work
/// (`DLL_PROCESS_DETACH` holds the loader lock, runs on the wrong thread for
/// XAML, and races the callbacks diagnostics still holds into our code) and does
/// not need to: once this has run there is nothing of ours left on screen. A
/// resident, inert DLL costs a page of memory; a wrong unload costs the shell.
///
/// # Safety
/// XAML UI thread only — it edits the tree.
pub(crate) unsafe fn stand_down() {
    // First, so nothing re-applies behind the revert — neither a live callback
    // nor the periodic sweep.
    ACTIVE.store(false, Ordering::SeqCst);
    GENERATION.fetch_add(1, Ordering::SeqCst);

    match diagnostics() {
        Some(diagnostics) => restore::revert(&diagnostics),
        // Without diagnostics no handle can be resolved, so there is no way to
        // put anything back. Say so rather than reporting a silent success.
        None => logf!("stand down: no IXamlDiagnostics — cannot revert"),
    }

    // Everything below is "have we done X yet?" state. Clearing it is what lets
    // the feature be switched back on without restarting Explorer.
    if let Ok(mut decorated) = DECORATED.lock() {
        *decorated = None;
    }
    for indicator in indicators() {
        indicator.forget();
    }
    if let Ok(mut wired) = WIRED.lock() {
        // Our segments died with the content they lived in; these handles are
        // stale, and keeping them would stop a fresh strip being wired up.
        wired.clear();
    }
    REORDERED.store(false, Ordering::SeqCst);
    PROBED.store(false, Ordering::SeqCst);
    REPORTED.store(false, Ordering::SeqCst);
    logf!("stood down — the taskbar is as we found it");
}

/// Element handles the callback has noticed but not yet acted on.
///
/// The callback must not touch XAML at all — see [`sweep`] — but two of the steps
/// are driven by the handle carried on an event rather than by the recorded tree.
/// Deferring those means remembering the handle, so they are queued here and
/// drained by the sweep.
///
/// Only ever pushed from the tray island's thread, so the handles are always that
/// island's to use.
static PENDING_GLYPHS: Mutex<Vec<xamlom::InstanceHandle>> = Mutex::new(Vec::new());
static PENDING_SEGMENTS: Mutex<Vec<(interact::Segment, String, xamlom::InstanceHandle)>> =
    Mutex::new(Vec::new());

/// Takes everything queued, leaving the queue empty.
fn drain<T>(queue: &Mutex<Vec<T>>) -> Vec<T> {
    let mut held = match queue.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    std::mem::take(&mut *held)
}

fn enqueue<T>(queue: &Mutex<Vec<T>>, item: T) {
    let mut held = match queue.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    held.push(item);
}

/// Set while we are inside a XAML call, so the sweep timer cannot re-enter.
///
/// This is not paranoia. An STA thread **pumps messages while an outgoing COM
/// call is in flight**, so a `WM_TIMER` posted to the control window is
/// dispatched *inside* `put_Content` — on the same thread, in the middle of a
/// decoration. Measured: the log stopped dead between "XamlReader.Load ok" and
/// the mutation result, and the strip never appeared.
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

/// Periodic "is what should be true still true?" check, run off a timer on the
/// XAML thread.
///
/// Everything else in the TAP is driven by visual-tree events, and that is not
/// enough on its own. The shell data-binds the presenter's `Content` and can
/// overwrite our strip with a freshly built visual of its own; the re-apply for
/// that only runs when another tray mutation happens to arrive, so if the tree
/// goes quiet the strip stays gone. Observed on screen: volume icon hidden, tray
/// reordered, and nothing drawn in their place — the exact state the
/// `strip_placed()` gate exists to prevent, reached from the other direction.
///
/// Cheap in the steady state: one handle resolve and a runtime-class read.
///
/// # Safety
/// XAML UI thread only — it is called from the control window's timer.
pub(crate) unsafe fn sweep() {
    if !ACTIVE.load(Ordering::SeqCst) {
        return;
    }
    // Dropped if a visual-tree callback — or another sweep — is already inside a
    // XAML call on this thread. Skipping is always safe: the next tick is 3s away.
    let Some(_busy) = BusyGuard::claim() else {
        return;
    };
    // **The fix for the shell freeze.** `put_Content` against a tray element
    // while `AdviseVisualTreeChange` is still streaming does not return: the UI
    // thread is inside a marshalled call, and it wedges there with the whole
    // taskbar — CPU flat, clock stopped, recoverable only by restarting Explorer.
    //
    // Decoration used to run inline from the callback, which is *always* inside
    // the stream. Whether it wedged then came down to timing: if the tray icon
    // happened to arrive after the replay finished it worked, and if it arrived
    // mid-replay it did not. That is exactly the coin-flip that made the same
    // binary behave differently on consecutive runs, and why injecting *after*
    // the icon already existed reproduced it every time.
    //
    // Waiting for silence is what makes the call safe. A `WM_TIMER` can itself be
    // dispatched mid-burst — an STA thread pumps while a call is outstanding — so
    // the timer is only the driver here; this check is the guard.
    if !tree::quiet_for(QUIET_BEFORE_MUTATING) {
        return;
    }
    let Some(diagnostics) = diagnostics() else {
        return;
    };

    // Order matters. Finding an indicator's slot only records it, and has to happen
    // whether or not the strip is up yet. Decoration comes next, because
    // collapsing those icons and reordering the tray are both gated on the
    // strip actually being on screen — that gate is what stops us taking Windows'
    // controls away and putting nothing back.
    for text_block in drain(&PENDING_GLYPHS) {
        note_system_indicator(&diagnostics, text_block);
    }

    // "Decorate unless it is already done", so this covers a strip the shell
    // overwrote as well as an icon that arrived with no further event to notice it.
    try_decorate(&diagnostics);

    enforce_hidden(&diagnostics);

    if strip_placed() && !REORDERED.load(Ordering::SeqCst) && reorder::sections_ready() {
        reorder_now(&diagnostics);
    }

    // Drained last, and deliberately after `try_decorate`: the segments only exist
    // once the strip has been placed, and their own announcements arrive
    // re-entrantly *during* that `put_Content`. By the time we get here they are
    // already queued, so hover is wired in the same tick that drew the strip.
    for (segment, name, element) in drain(&PENDING_SEGMENTS) {
        attach_segment(&diagnostics, segment, &name, element);
    }

    report_slot_metrics(&diagnostics);

    // Nothing left to apply — drop to the slow cadence until something comes
    // undone. `strip_placed` going false again (the shell re-binding the
    // presenter's content) is what brings it back.
    //
    // The wiring check earns its place: our own segments are announced *after*
    // `put_Content` returns rather than during it, so they are always queued for
    // the following tick. Without it the pace dropped first and hover took an idle
    // interval to arrive instead of a fast one.
    let settled = strip_placed() && REORDERED.load(Ordering::SeqCst) && segments_wired();
    lifecycle::set_sweep_pace(settled);
}

/// Redraws the strip with new glyphs, because the default devices changed.
///
/// Marks the strip as needing redoing and then attempts it at once. The attempt is
/// [`sweep`], not a `put_Content` from here: that is what carries the "the event
/// stream has been quiet" check which mutating a tray element safely depends on. If
/// the stream is mid-burst the sweep declines and the timer retries, so this is
/// still safe to call from a window procedure — it just no longer *waits* for the
/// timer in the common case, which was costing a redraw up to a full tick after the
/// click that asked for it.
///
/// The restore record is untouched: [`restore::remember_content`] refuses to record
/// our own strip, so redrawing cannot lose the shell's original visual.
///
/// # Safety
/// Called from the control window's procedure, on the tray thread.
pub(crate) unsafe fn restyle(
    output: Option<char>,
    output_muted: bool,
    input: Option<char>,
    input_muted: bool,
    input_recording: bool,
) {
    {
        let mut strip = match STRIP.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let state = strip.get_or_insert_with(decorate::StripState::default);
        let wanted = decorate::StripState {
            output_glyph: output.unwrap_or(state.output_glyph),
            input_glyph: input.unwrap_or(state.input_glyph),
            output_muted,
            input_muted,
            input_recording,
            ..*state
        };
        // A restyle that changes nothing is dropped here, before it costs a
        // rebuild. audio-tray suppresses its own duplicates, so one arriving means
        // a genuinely new state *or* an older audio-tray that does not: either way
        // this is what keeps a redundant message from tearing down a strip that is
        // already correct.
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

    // Clearing these is what makes the redraw happen and re-wire. The segment
    // elements are replaced wholesale, so their old handles are of no further use.
    {
        let mut decorated = match DECORATED.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *decorated = None;
    }
    if let Ok(mut wired) = WIRED.lock() {
        wired.clear();
    }
    // Fast pace first, so that if the sweep below declines the retry is the short
    // interval and not the idle one.
    lifecycle::set_sweep_pace(false);
    unsafe { sweep() };
}

/// Whether the strip's segments have had their pointer handlers attached.
fn segments_wired() -> bool {
    match WIRED.lock() {
        Ok(guard) => !guard.is_empty(),
        Err(poisoned) => !poisoned.into_inner().is_empty(),
    }
}

/// How long the visual-tree stream must be silent before we touch XAML.
///
/// Comfortably longer than the gaps *within* a replay burst and far shorter than
/// a user would notice the strip taking to appear.
const QUIET_BEFORE_MUTATING: std::time::Duration = std::time::Duration::from_millis(400);

/// Logs, once, how big the shell's icon slot is next to our pill.
///
/// Explorer draws its own hover plate on the notify-icon slot, behind whatever we
/// put in it — so how evenly that plate surrounds the pill is decided by the
/// difference between the two, and that difference has to be measured rather than
/// assumed.
///
/// Called from the visual-tree callback rather than the sweep timer. Layout has to
/// have run first (`ActualWidth` reads 0 until it has), which the timer would also
/// satisfy — but new WinRT calls are only introduced on the path that is already
/// known to be safe.
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
    // Zero means layout has not run yet; try again on the next tick.
    if slot_w <= 0.0 || slot_h <= 0.0 {
        return;
    }
    // The pill, not the presenter: the presenter fills the slot, so measuring it
    // could only ever report a zero surround. Retry next tick if layout has not
    // reached our content yet.
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

    // The slot is not what gets drawn. Explorer's hover highlight is a Border
    // inside the icon's ContainerGrid, and if its style insets it then the gap the
    // eye sees is this one, not the slot's. Its *size* does not depend on hover —
    // only its opacity does — so it can be measured cold, which beats trying to
    // sample a screenshot while someone holds the pointer still.
    //
    // Breadth-first finds the shell's Border before our pill, which is a level
    // deeper under the presenter.
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

/// Move the notification area next to the wifi/battery button.
///
/// Retried until it succeeds: the tray's sections trickle in, so an early attempt
/// can run before both of the sections it needs are recorded.
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

/// Wires pointer handlers onto one of our segments, once per element.
///
/// Keyed on the element handle rather than a plain "done" flag: Explorer rebuilds
/// the tray on DPI and monitor changes, which produces a fresh strip that needs
/// wiring again.
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
        let mut wired = match WIRED.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if wired.contains(&element) {
            return;
        }
        wired.push(element);
    }
    // The plate is a child of the segment and shares its name plus a suffix, which
    // is how the markup and this code stay in step.
    let plate_name = format!("{name}Hover");
    let Some(&plate) = tree::find_by_name(&plate_name).first() else {
        logf!("no hover plate {plate_name:?} recorded yet for 0x{element:x}");
        return;
    };
    interact::attach(diagnostics, segment, element, plate);
}

/// Note one of Explorer's own indicators if this text block is it.
///
/// Only *records* the slot — collapsing it is [`enforce_hidden`]'s job, gated on our strip
/// actually being placed. The two have to be separate because the volume glyph is announced
/// during the replay, well before our icon has been decorated; gating the search itself
/// would mean the slot was never found and the icon never hidden even when the strip does
/// appear.
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

    // What tells Explorer's microphone glyph from the one in our own input segment: only
    // the shell's sits in a `SystemTray.IconView`. Ours is inside the strip we injected, so
    // it falls out here — which is why matching on the codepoint alone is safe.
    let Some(icon) = ancestor_of_type(text_block, "SystemTray.IconView", 8) else {
        return;
    };

    // Collapsing the `IconView` hides the glyph but leaves a hole between
    // wifi and battery: inside the Quick Settings button each icon sits in
    // its own generated `ContentPresenter`, and that container keeps its
    // layout box no matter what happens to its content. Collapse the
    // container so the `StackPanel` closes the slot up.
    let slot = tree::parent_of(icon)
        .filter(|&parent| {
            tree::type_of(parent).as_deref() == Some("Windows.UI.Xaml.Controls.ContentPresenter")
        })
        .unwrap_or(icon);

    // Log only the first time, so a tray rebuild doesn't spam.
    if !wanted.announced.swap(true, Ordering::SeqCst) {
        logf!(
            "system {} indicator found: glyph {:04X} in IconView 0x{icon:x}, slot 0x{slot:x} (hidden only once our strip is placed)",
            wanted.what,
            glyph as u32
        );
    }
    // Remembered so it can be re-applied: a collapse that lands before the
    // shell has measured this item does not free the slot (observed —
    // `ActualWidth` still 0 at that point, and the gap survives).
    wanted.record((icon, slot));
    enforce_hidden(diagnostics);
}

/// Names a tray glyph we do not recognise, once per codepoint.
///
/// This is the diagnostic for the one thing [`decorate::MIC_GLYPHS`] cannot be sure of:
/// which codepoint *this* Windows build uses for "microphone in use". If the indicator is
/// not being hidden, its glyph is in the log — as long as it was announced, which is the
/// same condition the hiding itself needs.
fn note_unknown_glyph(glyph: char) {
    /// A tray holds a couple of dozen glyphs; past that something is announcing far more
    /// than we expected and the log is worth more than the completeness.
    const MAX_SEEN: usize = 48;

    let mut seen = match UNKNOWN_GLYPHS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if seen.contains(&glyph) || seen.len() >= MAX_SEEN {
        return;
    }
    seen.push(glyph);
    logf!("tray glyph {:04X} is not one we hide", glyph as u32);
}

/// Codepoints [`note_unknown_glyph`] has already reported.
static UNKNOWN_GLYPHS: Mutex<Vec<char>> = Mutex::new(Vec::new());

/// Re-applies each indicator's collapse until layout actually gives up its slot.
///
/// Called on every sweep, so it costs one `ActualWidth` read per tick per indicator until
/// they settle — then nothing.
///
/// # Safety
/// XAML UI thread only, and only once the event stream is quiet — see [`sweep`].
unsafe fn enforce_hidden(diagnostics: &xamlom::IXamlDiagnostics) {
    // Windows' own icons stay until ours is actually on the taskbar. This is the gate that
    // stops us taking the shell's controls away and putting nothing back.
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
        // Only worth repeating once layout has actually measured the item — a
        // zero here means it has not run yet, and collapsing then does nothing.
        if decorate::actual_width(diagnostics, slot).is_none_or(|width| width <= 0.0) {
            continue;
        }
        // Before, never after: from the second attempt onwards what we would read
        // back is our own zero width, not Explorer's original.
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

/// Find the tray icon we were asked to decorate and replace its content.
///
/// The icons come from the recorded tree — that part is reliable, they are always
/// announced. Their presenters do **not**: see
/// [`decorate::descendant_presenter`] for why they have to be looked up live.
///
/// A free function rather than a method, because [`sweep`] calls it from a timer
/// where there is no TAP instance in hand — only the process-wide diagnostics.
///
/// # Safety
/// XAML UI thread only.
unsafe fn try_decorate(diagnostics: &xamlom::IXamlDiagnostics) {
    // Every path out of here used to be silent, which made "the strip did not
    // appear" undiagnosable from the log — the interesting cases are all early
    // returns. Logged on *change* rather than capped at a count: this runs on
    // every tray mutation, so a plain limit spends itself on the replay burst and
    // then hides the reason that actually mattered.
    let why = |reason: &str| {
        static LAST: Mutex<String> = Mutex::new(String::new());
        let Ok(mut last) = LAST.lock() else { return };
        if *last != reason {
            logf!("try_decorate: {reason}");
            last.clear();
            last.push_str(reason);
        }
    };

    // Checked before the live walk below, which is the expensive part: once the
    // strip is up this is the path every remaining tray mutation takes.
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

    // One-shot: what each tray icon actually calls itself. Logged once so a
    // mismatch is diagnosable from the log instead of guessed at.
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
        // Substring, not equality: a tray icon's accessible name is its tooltip,
        // and audio-tray's is mostly the current device's name. Only the app's
        // marker within it is stable across device switches and locales.
        //
        // An empty target means "the first icon found", which is how the spike
        // demonstrates itself without audio-tray running.
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
        let mut decorated = match DECORATED.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *decorated = Some((icon, presenter));
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

/// Deliberately pins the DLL for the lifetime of the spike: returning `S_OK`
/// invites a free while explorer still holds callbacks into our code.
#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

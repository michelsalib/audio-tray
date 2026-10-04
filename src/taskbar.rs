//! Explorer integration: the pair of controls drawn inside the taskbar itself.
//!
//! `InitializeXamlDiagnosticsEx` loads `audio_tray_tap.dll` into `explorer.exe`, where it decorates
//! our `Shell_NotifyIcon` entry (registered unconditionally by `crate::tray`). Optional by contract:
//! every error here means "the plain icon carries on alone", never fatal. See
//! `crates/taskbar-tap/FINDINGS.md`.
//!
//! Taking the strip down is a revert in place, never an unload; the DLL stays loaded and inert.
//! Revert triggers: quit → [`revert`]; killed → the TAP's watch on the `pid=` owner; Explorer
//! restart → nothing to revert, [`apply_at_restart`] re-injects; `--taskbar-revert` → [`revert`].
//! A relaunch hands the loaded TAP the new owner ([`offer_handover`]); Explorer is restarted only
//! for a TAP from another build or one that does not answer. XAML Diagnostics is single-consumer
//! (TranslucentTB, Windhawk use the same endpoint).

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
use windows_core::{GUID, HRESULT, PCSTR, PCWSTR};

/// The TAP's class id, matched by the DLL's `DllGetClassObject`.
const CLSID_TAP: GUID = GUID::from_u128(tap_proto::CLSID_TAP);

use tap_proto::{ENDPOINT_NAME, TAP_DLL};

type InitializeXamlDiagnosticsEx = unsafe extern "system" fn(
    end_point_name: PCWSTR,
    pid: u32,
    wsz_dll_xaml_diagnostics: PCWSTR,
    wsz_tap_dll_name: PCWSTR,
    tap_clsid: GUID,
    wsz_initialization_data: PCWSTR,
) -> HRESULT;

/// Path to the TAP, if it shipped with this build.
fn tap_path() -> Result<PathBuf> {
    let dir = std::env::current_exe()?
        .parent()
        .context("exe has no parent directory")?
        .to_path_buf();
    let dll = dir.join(TAP_DLL);
    if !dll.is_file() {
        bail!("{TAP_DLL} not found next to the exe");
    }
    Ok(dll)
}

/// Shell process and time of the last successful injection, to suppress a duplicate.
static LAST_INJECTED: std::sync::Mutex<Option<(u32, std::time::Instant)>> =
    std::sync::Mutex::new(None);

/// Whether a strip of ours is currently up, as far as we know. See [`strip_is_up`].
static STRIP_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the strip is up and so owns left clicks on our tray icon (the shell also delivers a
/// segment click as a tray-icon click). Optimistic — it tracks our injections and reverts only —
/// so the icon's right click stays live regardless.
pub fn strip_is_up() -> bool {
    STRIP_UP.load(std::sync::atomic::Ordering::SeqCst)
}

/// Inject the TAP into the shell's Explorer. Best-effort by contract: every error
/// means "no strip this time", and the message says why.
fn enable(icons: StripIcons) -> Result<()> {
    // Cleared first so any failure leaves it false.
    STRIP_UP.store(false, std::sync::atomic::Ordering::SeqCst);
    let dll = tap_path()?;
    let pid = shell_pid()?;
    unsafe { inject(pid, &dll, icons)? };
    if let Ok(mut last) = LAST_INJECTED.lock() {
        *last = Some((pid, std::time::Instant::now()));
    }
    STRIP_UP.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// Whether we injected into this same Explorer a moment ago.
fn just_injected(pid: u32) -> bool {
    /// Covers "started as the shell came up"; short enough not to block a deliberate re-inject.
    const WINDOW: std::time::Duration = std::time::Duration::from_secs(10);

    LAST_INJECTED
        .lock()
        .ok()
        .and_then(|last| *last)
        .is_some_and(|(was, at)| was == pid && at.elapsed() < WINDOW)
}

use tap_proto::CONTROL_CLASS as TAP_CONTROL_CLASS;

use tap_proto::WM_TAP_REVERT;

/// Ask the injected TAP to undo its changes in place (it stays loaded). `owner_pid` 0 is
/// unconditional; otherwise the TAP ignores it once another process owns the strip. Best-effort.
pub fn revert(owner_pid: u32) {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

    // Unconditionally: the plain icon's clicks matter again from here on.
    STRIP_UP.store(false, std::sync::atomic::Ordering::SeqCst);
    // Posted, not sent: never block on Explorer's UI thread.
    for control in crate::win::windows_by_class(TAP_CONTROL_CLASS) {
        if let Err(e) = unsafe { PostMessageW(Some(control), WM_TAP_REVERT, WPARAM(owner_pid as usize), LPARAM(0)) } {
            eprintln!("taskbar: could not ask for a revert ({e})");
        }
    }
}

/// The injected TAP's control window, if there is one.
fn control_window() -> Option<windows::Win32::Foundation::HWND> {
    window_by_class(TAP_CONTROL_CLASS)
}

/// A top-level window with this exact class name, in any process. `EnumWindows`, because
/// `FindWindow` does not find these windows across processes.
fn window_by_class(class_name: &str) -> Option<windows::Win32::Foundation::HWND> {
    let mut found = None;
    crate::win::enum_windows(|hwnd| {
        if crate::win::class_name(hwnd) == class_name {
            found = Some(hwnd);
        }
        found.is_none()
    });
    found
}

/// The process owning the desktop window: the Explorer hosting the taskbar.
fn shell_pid() -> Result<u32> {
    let hwnd = unsafe { GetShellWindow() };
    if hwnd.0.is_null() {
        bail!("no shell window — Explorer is not running");
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        bail!("could not resolve the shell process id");
    }
    Ok(pid)
}

unsafe fn inject(pid: u32, dll: &std::path::Path, icons: StripIcons) -> Result<()> {
    // No import library for the system XAML runtime: resolve dynamically.
    let module = LoadLibraryW(PCWSTR(crate::win::wide("Windows.UI.Xaml.dll").as_ptr()))
        .context("load Windows.UI.Xaml.dll")?;
    let symbol = GetProcAddress(module, PCSTR(c"InitializeXamlDiagnosticsEx".as_ptr().cast()))
        .context("Windows.UI.Xaml.dll does not export InitializeXamlDiagnosticsEx")?;
    let initialize: InitializeXamlDiagnosticsEx = std::mem::transmute(symbol);

    // Both DLL parameters get the TAP's own path, matching the known-good C++ TAPs.
    let endpoint = crate::win::wide(ENDPOINT_NAME);
    let path = crate::win::wide(&dll.to_string_lossy());
    let init_data = crate::win::wide(&init_data(icons, std::process::id()));

    let hr = initialize(
        PCWSTR(endpoint.as_ptr()),
        pid,
        PCWSTR(path.as_ptr()),
        PCWSTR(path.as_ptr()),
        CLSID_TAP,
        PCWSTR(init_data.as_ptr()),
    );
    if hr.is_err() {
        // Name the likeliest cause: another consumer holds the endpoint.
        bail!(
            "InitializeXamlDiagnosticsEx failed: 0x{:08x} ({}). \
             The {ENDPOINT_NAME} endpoint takes one consumer at a time — if \
             TranslucentTB, Windhawk or another taskbar tool is running, that is \
             the first thing to rule out.",
            hr.0,
            windows_core::Error::from(hr).message()
        );
    }
    Ok(())
}

/// What the strip should draw right now: the current devices' icons, resolved through
/// [`crate::config::Config::icon_of`] like every other surface's (earbuds via [`strip_glyph`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StripIcons {
    pub output: char,
    pub input: char,
    pub output_muted: bool,
    pub input_muted: bool,
    /// An app holds the microphone open (red dot; see [`crate::audio::mic`]). Independent of
    /// `input_muted`, as Windows' own indicator is.
    pub input_recording: bool,
}

/// Fallbacks for when the devices cannot be resolved: Volume and Microphone.
impl Default for StripIcons {
    fn default() -> Self {
        Self {
            output: '\u{E767}',
            input: '\u{E720}',
            output_muted: false,
            input_muted: false,
            input_recording: false,
        }
    }
}

use tap_proto::{GLYPH_ROUND_EARBUDS, GLYPH_WIRELESS_EARBUDS};

/// The codepoint the strip should carry for an icon: `IconId::glyph`, except the hand-drawn earbuds
/// map to marker codepoints so the TAP draws the real shape instead of the headphone fallback.
pub fn strip_glyph(icon: crate::icons::IconId) -> char {
    use crate::icons::IconId;
    match icon {
        IconId::WirelessEarbuds => GLYPH_WIRELESS_EARBUDS,
        IconId::RoundEarbuds => GLYPH_ROUND_EARBUDS,
        other => other.glyph(),
    }
}

/// Alpha applied to the accent fill, as hex: half, to match the weight of the shell's own buttons.
const PILL_ALPHA: &str = "80";

/// The `key=value;` init payload for the TAP, read once in `SetSite` (or on a handover).
/// `tooltip` is matched as a substring of our icon's accessible name ([`crate::tray::TRAY_MARKER`]
/// is the stable part); `pid` is the owner the TAP watches and reverts on exit; `hidevolume`/`hidemic`
/// collapse Windows' own indicators; `tile` names the app button for the music tile (empty = off);
/// `ver`/`tap`/`hwnd` identify the build and receiver for [`offer_handover`].
fn init_data(icons: StripIcons, owner_pid: u32) -> String {
    let [r, g, b] = crate::flyout::theme::accent_rgb();
    let music = crate::config::Config::load().music;
    let tile = if music.enabled { music.tile } else { String::new() };
    let tap = tap_path().map(|path| path.display().to_string()).unwrap_or_default();
    format!(
        "tooltip={};out={:04X};in={:04X};\
         outmuted={};inmuted={};inrec={};accent={r:02X}{g:02X}{b:02X};alpha={PILL_ALPHA};\
         hidevolume=1;hidemic=1;tile={tile};pid={owner_pid};ver={};tap={tap};hwnd={}",
        crate::tray::TRAY_MARKER,
        icons.output as u32,
        icons.input as u32,
        u8::from(icons.output_muted),
        u8::from(icons.input_muted),
        u8::from(icons.input_recording),
        env!("CARGO_PKG_VERSION"),
        RECEIVER.load(std::sync::atomic::Ordering::SeqCst),
    )
}

/// Our receiver window, for the `hwnd=` key. Set by [`create_receiver`].
static RECEIVER: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

use tap_proto::{HANDOVER_ACCEPTED, HANDOVER_DECLINED, HANDOVER_MAGIC};

/// Hand every TAP loaded in Explorer a fresh init payload naming `owner_pid` as owner. A TAP from
/// this exact build adopts it (no Explorer restart needed); `Err` says why none did.
fn offer_handover(icons: StripIcons, owner_pid: u32) -> Result<()> {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_COPYDATA};

    let payload = crate::win::wide(&init_data(icons, owner_pid));
    let copy = COPYDATASTRUCT {
        dwData: HANDOVER_MAGIC,
        cbData: (payload.len() * 2) as u32,
        lpData: payload.as_ptr() as *mut core::ffi::c_void,
    };
    let mut why = Vec::new();
    for control in crate::win::windows_by_class(TAP_CONTROL_CLASS) {
        let mut result = 0usize;
        // `WM_COPYDATA` must be sent; bounded so a wedged shell cannot hang the tray.
        let sent = unsafe {
            SendMessageTimeoutW(
                control,
                WM_COPYDATA,
                WPARAM(RECEIVER.load(std::sync::atomic::Ordering::SeqCst) as usize),
                LPARAM(&copy as *const COPYDATASTRUCT as isize),
                SMTO_ABORTIFHUNG,
                3_000,
                Some(&mut result),
            )
        };
        match (sent.0, result as isize) {
            (0, _) => why.push("did not answer".to_string()),
            (_, HANDOVER_ACCEPTED) => return Ok(()),
            (_, HANDOVER_DECLINED) => why.push("is from another build".to_string()),
            _ => why.push("predates the handover".to_string()),
        }
    }
    if why.is_empty() {
        bail!("no TAP is loaded");
    }
    bail!("the loaded TAP {}", why.join(", "))
}

/// Make `child_pid` the owner of the strip before this process exits, so the TAP's owner watch
/// does not revert it when we go. For `tray::restart_app`; the child then offers its own handover.
pub fn transfer_owner(child_pid: u32, icons: StripIcons) {
    match offer_handover(icons, child_pid) {
        Ok(()) => println!("taskbar: strip handed to pid {child_pid}"),
        Err(e) => eprintln!("taskbar: could not hand the strip to pid {child_pid} ({e:#}); it will be redrawn"),
    }
}

use tap_proto::WM_TAP_RESTYLE;

/// Tell an injected TAP the devices changed (codepoint plus flag bits in each message param).
/// Returns whether it was actually posted: the control window appears only after the TAP's first
/// tree callback, and the caller must not remember an unsent restyle as sent.
#[must_use]
pub fn restyle(icons: StripIcons) -> bool {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

    let Some(control) = control_window() else {
        return false;
    };
    // The codepoint plus a flag bit each; recording is only ever set for the input.
    let pack = |glyph: char, muted: bool, recording: bool| {
        glyph as usize | if muted { tap_proto::RESTYLE_MUTED } else { 0 } | if recording { tap_proto::RESTYLE_RECORDING } else { 0 }
    };
    let posted = unsafe {
        PostMessageW(
            Some(control),
            WM_TAP_RESTYLE,
            WPARAM(pack(icons.output, icons.output_muted, false)),
            LPARAM(pack(icons.input, icons.input_muted, icons.input_recording) as isize),
        )
    };
    if let Err(e) = posted {
        eprintln!("taskbar: could not restyle the strip ({e})");
        return false;
    }
    true
}

/// What the user did on the injected strip. The TAP only reports; all decisions are made here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    CycleOutput,
    CycleInput,
    OpenPanel,
    /// Music transport. Codes 10+, deliberately far from the audio ones on the same window.
    MusicPrevious,
    MusicPlayPause,
    MusicNext,
}

impl Action {
    /// Decode an explicit wire code from `tap_proto`.
    pub fn from_code(code: usize) -> Option<Self> {
        match code {
            tap_proto::ACTION_CYCLE_OUTPUT => Some(Self::CycleOutput),
            tap_proto::ACTION_CYCLE_INPUT => Some(Self::CycleInput),
            tap_proto::ACTION_OPEN_PANEL => Some(Self::OpenPanel),
            tap_proto::ACTION_MUSIC_PREVIOUS => Some(Self::MusicPrevious),
            tap_proto::ACTION_MUSIC_PLAY_PAUSE => Some(Self::MusicPlayPause),
            tap_proto::ACTION_MUSIC_NEXT => Some(Self::MusicNext),
            _ => None,
        }
    }

    /// The same codes in the other direction, for [`post_action`].
    #[cfg(feature = "dev")]
    fn code(self) -> usize {
        match self {
            Self::CycleOutput => tap_proto::ACTION_CYCLE_OUTPUT,
            Self::CycleInput => tap_proto::ACTION_CYCLE_INPUT,
            Self::OpenPanel => tap_proto::ACTION_OPEN_PANEL,
            Self::MusicPrevious => tap_proto::ACTION_MUSIC_PREVIOUS,
            Self::MusicPlayPause => tap_proto::ACTION_MUSIC_PLAY_PAUSE,
            Self::MusicNext => tap_proto::ACTION_MUSIC_NEXT,
        }
    }
}

/// Dev: post the gesture the strip would have sent to a running tray (taskbar clicks cannot be
/// synthesised). Exercises everything from [`WM_TASKBAR_ACTION`] inward, not the TAP's half.
#[cfg(feature = "dev")]
pub fn post_action(action: Action) -> Result<()> {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

    let receiver = window_by_class(RECEIVER_CLASS_NAME)
        .context("no receiver window — audio-tray is not running")?;
    unsafe { PostMessageW(Some(receiver), WM_TASKBAR_ACTION, WPARAM(action.code()), LPARAM(0)) }
        .context("post the action to the tray")
}

/// Dev: post the scroll the TAP would send for `notches` (fractional allowed, as from a touchpad).
#[cfg(feature = "dev")]
pub fn post_scroll(flow: crate::audio::Flow, notches: f32) -> Result<()> {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WHEEL_DELTA};

    let receiver = window_by_class(RECEIVER_CLASS_NAME)
        .context("no receiver window — audio-tray is not running")?;
    let delta = (notches * WHEEL_DELTA as f32).round() as i32;
    unsafe {
        PostMessageW(
            Some(receiver),
            WM_TASKBAR_SCROLL,
            WPARAM(flow_code(flow)),
            LPARAM(delta as isize),
        )
    }
    .context("post the scroll to the tray")
}

/// Post a progress-bar value (`None` clears) to the tray thread, whose STA owns `ITaskbarList3`
/// (the music feed runs on an MTA).
pub fn post_progress(fraction: Option<f64>, playing: bool) -> Result<()> {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

    let receiver = receiver().context("no receiver window — the tray is not up")?;
    // `wParam`: the fraction in `PROGRESS_SCALE`ths, or `PROGRESS_NONE` to clear.
    let step = match fraction {
        Some(fraction) => (fraction.clamp(0.0, 1.0) * PROGRESS_SCALE as f64).round() as usize,
        None => PROGRESS_NONE,
    };
    unsafe {
        PostMessageW(
            Some(receiver),
            WM_MUSIC_PROGRESS,
            WPARAM(step),
            LPARAM(isize::from(playing)),
        )
    }
    .context("post the progress to the tray")
}

/// Apply a [`WM_MUSIC_PROGRESS`] payload on the tray's STA. Clears instead while the strip is down
/// (after `--taskbar-revert` the feed keeps polling).
pub fn apply_progress(step: usize, playing: bool) {
    let fraction = (step != PROGRESS_NONE).then(|| step as f64 / PROGRESS_SCALE as f64);
    if fraction.is_some() && !strip_is_up() {
        clear_player_progress();
        return;
    }
    if let Err(err) = crate::music::player::set_player_progress(fraction, playing) {
        // Routine when the player closes; logged only when there was something to draw.
        if fraction.is_some() {
            eprintln!("music: could not set the progress bar: {err:#}");
        }
    }
}

/// Take the progress bar off the player's window, ignoring the "no player" case.
pub fn clear_player_progress() {
    let _ = crate::music::player::set_player_progress(None, false);
}

/// Denominator for the progress fraction carried in [`WM_MUSIC_PROGRESS`]'s `wParam`.
const PROGRESS_SCALE: usize = 1000;

/// `wParam` value meaning "clear the bar" — outside the `0..=PROGRESS_SCALE` range a fraction uses.
const PROGRESS_NONE: usize = usize::MAX;

pub use tap_proto::WM_MUSIC_PROGRESS;

#[cfg(feature = "dev")]
use tap_proto::RECEIVER_CLASS as RECEIVER_CLASS_NAME;

pub use tap_proto::WM_TASKBAR_ACTION;

pub use tap_proto::WM_TASKBAR_SCROLL;

/// Wire code for a direction in [`WM_TASKBAR_SCROLL`]'s `wParam`.
pub fn flow_code(flow: crate::audio::Flow) -> usize {
    match flow {
        crate::audio::Flow::Output => tap_proto::FLOW_OUTPUT,
        crate::audio::Flow::Input => tap_proto::FLOW_INPUT,
    }
}

/// The other direction of [`flow_code`]; anything unrecognised reads as output.
pub fn flow_from_code(code: usize) -> crate::audio::Flow {
    match code {
        tap_proto::FLOW_INPUT => crate::audio::Flow::Input,
        _ => crate::audio::Flow::Output,
    }
}

pub use tap_proto::WM_TASKBAR_RESTARTED;

/// The shell's "the taskbar is back" broadcast, registered once.
pub fn taskbar_created_message() -> u32 {
    use std::sync::OnceLock;
    static ID: OnceLock<u32> = OnceLock::new();
    *ID.get_or_init(|| unsafe {
        windows::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW(windows::core::w!(
            "TaskbarCreated"
        ))
    })
}

/// Creates the hidden window the TAP posts to, with the tray's window procedure. Top-level, not
/// message-only: the TAP finds it with `EnumWindows`, which skips message-only windows.
pub fn create_receiver(proc: windows::Win32::UI::WindowsAndMessaging::WNDPROC) -> Result<windows::Win32::Foundation::HWND> {
    use windows::Win32::UI::WindowsAndMessaging::WS_EX_TOOLWINDOW;
    let class = windows_core::HSTRING::from(tap_proto::RECEIVER_CLASS);
    let class = PCWSTR(class.as_ptr());
    let hwnd = crate::win::create_popup(class, class, proc, WS_EX_TOOLWINDOW, (0, 0, 0, 0))
        .context("create the taskbar IPC receiver window")?;
    RECEIVER.store(hwnd.0 as isize, std::sync::atomic::Ordering::SeqCst);
    Ok(hwnd)
}

/// This process's receiver window, once [`create_receiver`] has run.
pub fn receiver() -> Option<windows::Win32::Foundation::HWND> {
    let raw = RECEIVER.load(std::sync::atomic::Ordering::SeqCst);
    (raw != 0).then_some(windows::Win32::Foundation::HWND(raw as *mut core::ffi::c_void))
}

/// Whether this process has already restarted Explorer to repair the strip. One restart per
/// process, shared by both triggers: the failures can be permanent, and a loop would be worse.
static HEALED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Restart Explorer for a clean injection, at most once per process; returns whether started. On a
/// worker thread: the tray thread must keep pumping to receive the *sent* `TaskbarCreated`.
fn heal_explorer(reason: &str) -> bool {
    if HEALED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        eprintln!("taskbar: {reason}, but Explorer has been restarted once already — leaving it");
        return false;
    }
    eprintln!("taskbar: {reason} — restarting Explorer for a clean injection");
    std::thread::spawn(|| {
        if let Err(e) = restart_explorer() {
            eprintln!("taskbar: could not restart Explorer ({e:#})");
        }
    });
    true
}

/// Whether a TAP from an earlier audio-tray is already loaded in this Explorer, detected by its
/// control window (owned by `explorer.exe`, so it outlives that audio-tray).
fn tap_already_present() -> bool {
    control_window().is_some()
}

/// Attempts, and the gap between them, before an injection failure is treated as real: at
/// sign-in audio-tray can beat Explorer's XAML runtime, and that must not trigger a restart.
const ENABLE_TRIES: u32 = 3;
const ENABLE_GAP: std::time::Duration = std::time::Duration::from_millis(750);

/// [`enable`], retried — see [`ENABLE_TRIES`].
fn enable_with_retries(icons: StripIcons) -> Result<()> {
    let mut last = None;
    for attempt in 1..=ENABLE_TRIES {
        match enable(icons) {
            Ok(()) => return Ok(()),
            Err(e) => {
                if attempt < ENABLE_TRIES {
                    eprintln!("taskbar: injection attempt {attempt} failed ({e:#}); retrying");
                    std::thread::sleep(ENABLE_GAP);
                }
                last = Some(e);
            }
        }
    }
    Err(last.expect("the loop runs at least once"))
}

/// Put the strip up at startup. Never fatal; restarts Explorer (see [`heal_explorer`]) only when a
/// loaded TAP refuses the handover or injection keeps failing.
pub fn apply_at_startup(icons: StripIcons) {
    // Never inject beside a loaded TAP (its owner watch can undo ours): hand it over, or restart
    // Explorer for another build's TAP (which also lets `place_staged_tap` finish an update).
    if tap_already_present() {
        match offer_handover(icons, std::process::id()) {
            Ok(()) => {
                STRIP_UP.store(true, std::sync::atomic::Ordering::SeqCst);
                eprintln!("taskbar: controls handed over to the TAP already in Explorer");
                return;
            }
            Err(e) => {
                if heal_explorer(&format!("{e:#}")) {
                    return;
                }
            }
        }
    }
    match enable_with_retries(icons) {
        Ok(()) => eprintln!("taskbar: controls enabled"),
        Err(e) => {
            eprintln!("taskbar: integration unavailable, using the plain tray icon ({e:#})");
            // Not from `apply_at_restart`: never restart Explorer right after the user did.
            heal_explorer("the injection failed");
        }
    }
}

/// Re-inject after Explorer restarted (the TAP died with it). Tray thread; failure leaves the plain icon.
pub fn apply_at_restart(icons: StripIcons) {
    // `TaskbarCreated` can land right after our own startup injection. Not "is a TAP loaded":
    // after a revert it still is, and that test would refuse a legitimate re-injection.
    if shell_pid().is_ok_and(just_injected) {
        return;
    }
    match enable(icons) {
        Ok(()) => eprintln!("taskbar: Explorer restarted — controls re-injected"),
        Err(e) => eprintln!("taskbar: Explorer restarted, re-injection failed ({e:#})"),
    }
}

/// The shell's private "Exit Explorer" command (Ctrl+Shift+right-click menu). Counts as deliberate,
/// so `AutoRestartShell` does not relaunch; [`restart_explorer`] does.
const WM_SHELL_EXIT: u32 = 0x5B4;

/// How long the polite request gets before termination. Short: with our TAP loaded Explorer
/// ignores it entirely, and without one it exits in under a second.
const SHELL_EXIT_WAIT_MS: u32 = 2_500;

/// Restart `explorer.exe`, placing a staged TAP in between. Blocks for seconds: keep it off a
/// thread that pumps messages. The strip comes back via `TaskbarCreated` → [`apply_at_restart`].
pub fn restart_explorer() -> Result<()> {
    use windows::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_TERMINATE};

    let pid = shell_pid()?;
    // A handle, opened before asking, so a recycled pid cannot fool the wait. `SYNCHRONIZE` is the
    // generic bit, re-wrapped as a process right.
    let access = PROCESS_ACCESS_RIGHTS(SYNCHRONIZE.0) | PROCESS_TERMINATE;
    let shell = unsafe { OpenProcess(access, false, pid) }.context("open the shell process")?;

    // Every path below ends with this Explorer (and the strip) gone.
    STRIP_UP.store(false, std::sync::atomic::Ordering::SeqCst);

    let outcome = exit_and_relaunch_shell(shell);
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(shell) };
    outcome
}

/// The body of [`restart_explorer`], split out so the process handle is closed on every path.
fn exit_and_relaunch_shell(shell: windows::Win32::Foundation::HANDLE) -> Result<()> {
    use windows::Win32::Foundation::{LPARAM, WAIT_OBJECT_0, WPARAM};
    use windows::Win32::System::Threading::{TerminateProcess, WaitForSingleObject};
    use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

    // Ask nicely first, so Explorer saves its state.
    let asked = match window_by_class("Shell_TrayWnd") {
        Some(tray) => unsafe { PostMessageW(Some(tray), WM_SHELL_EXIT, WPARAM(0), LPARAM(0)) }
            .inspect_err(|e| eprintln!("taskbar: could not ask Explorer to exit ({e})"))
            .is_ok(),
        None => {
            eprintln!("taskbar: no Shell_TrayWnd to ask for an exit");
            false
        }
    };

    let gone = |ms: u32| unsafe { WaitForSingleObject(shell, ms) } == WAIT_OBJECT_0;
    let mut terminated = false;
    if !asked || !gone(SHELL_EXIT_WAIT_MS) {
        // The usual path when our TAP is loaded (and `WM_SHELL_EXIT` is undocumented anyway).
        eprintln!("taskbar: Explorer did not exit on request — terminating it instead");
        unsafe { TerminateProcess(shell, 1) }.context("terminate the shell process")?;
        terminated = true;
        if !gone(SHELL_EXIT_WAIT_MS) {
            bail!("the shell process would not exit");
        }
    }

    // Must be here: the only moment no Explorer holds the DLL, before re-injection loads it again.
    crate::update::place_staged_tap();

    // Launching explorer.exe beside a live shell opens a file window, so wait briefly in case one
    // restarts itself (in practice none does, so keep this short).
    let budget = std::time::Duration::from_millis(if terminated { 1_500 } else { 500 });
    if wait_for_shell(budget) {
        eprintln!("taskbar: Explorer restarted itself");
        return Ok(());
    }
    // Detached from our streams: the shell outlives us and would hold a console or file open.
    std::process::Command::new("explorer.exe")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("launch explorer.exe")?;
    eprintln!("taskbar: Explorer restarted");
    Ok(())
}

/// Whether a shell is up, waiting up to `budget` for one to appear.
fn wait_for_shell(budget: std::time::Duration) -> bool {
    const POLL: std::time::Duration = std::time::Duration::from_millis(100);
    let deadline = std::time::Instant::now() + budget;
    loop {
        if !unsafe { GetShellWindow() }.0.is_null() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

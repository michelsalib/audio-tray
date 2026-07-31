//! Explorer integration: the pair of controls drawn inside the taskbar itself.
//!
//! This is how audio-tray presents itself in the notification area, and it is
//! attempted on every start. It is still not a *requirement*: the plain
//! `Shell_NotifyIcon` tray entry is registered unconditionally by `crate::tray`
//! and the strip decorates it, so an injection that is unavailable or fails leaves
//! the app behaving exactly as it did before the strip existed. Callers treat
//! every error here as "the plain icon carries on alone", never as fatal.
//!
//! Mechanism (see `crates/taskbar-tap/FINDINGS.md` for the measurements behind it):
//! `InitializeXamlDiagnosticsEx` loads `audio_tray_tap.dll` into `explorer.exe`,
//! where it implements `IVisualTreeServiceCallback2`, watches the XAML tree for
//! our `SystemTray.NotifyIconView`, and restyles it.
//!
//! Taking the strip back down is a revert, not an unload. The TAP pins itself in
//! `explorer.exe` and stays there, but everything it changed — the tray icon's
//! content, the tray sections' columns, Explorer's own volume slot — is an
//! ordinary property edit that it recorded before making and can undo in place.
//! [`revert`] asks for that; the DLL then sits inert until the next injection.
//! Unloading it instead would gain a page of memory and risk freeing code Explorer
//! still holds callbacks into.
//!
//! Four things have to lead to that revert, and each has its own trigger:
//!   * audio-tray quits                     → [`revert`], from the tray loop
//!   * audio-tray is killed or crashes      → the TAP waits on our process id,
//!     passed in [`init_data`], and reverts when it exits
//!   * Explorer restarts                    → nothing to revert; the DLL died
//!     with it, and [`apply_at_restart`] injects into the new one
//!   * the user asks for the taskbar back   → [`revert`], via
//!     `audio-tray --taskbar-revert`, which leaves the running tray untouched
//!
//! Remaining caveat: XAML Diagnostics is effectively single-consumer — TranslucentTB
//! and Windhawk's Taskbar Styler use the same `VisualDiagConnection1` endpoint.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
use windows_core::{GUID, HRESULT, PCSTR, PCWSTR};

/// The TAP's class id, matched by the DLL's `DllGetClassObject`.
const CLSID_TAP: GUID = GUID::from_u128(0xb3e9_2816_117d_476f_936e_06ed_52b2_e55d);

/// Shared XAML Diagnostics endpoint name — the single-consumer bottleneck.
const ENDPOINT_NAME: &str = "VisualDiagConnection1";

/// Ships next to `audio-tray.exe`.
const TAP_DLL: &str = "audio_tray_tap.dll";

type InitializeXamlDiagnosticsEx = unsafe extern "system" fn(
    end_point_name: PCWSTR,
    pid: u32,
    wsz_dll_xaml_diagnostics: PCWSTR,
    wsz_tap_dll_name: PCWSTR,
    tap_clsid: GUID,
    wsz_initialization_data: PCWSTR,
) -> HRESULT;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

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

/// Shell process and time of the last successful injection.
///
/// Only used to suppress an immediate duplicate; see [`apply_at_restart`].
static LAST_INJECTED: std::sync::Mutex<Option<(u32, std::time::Instant)>> =
    std::sync::Mutex::new(None);

/// Whether a strip of ours is currently up, as far as we know.
///
/// This decides what a click on the plain notification icon means, so it is not
/// merely informational — see [`strip_is_up`].
static STRIP_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the strip is up, and so owns the gestures over our tray icon.
///
/// The shell goes on invoking the notification icon *underneath* the strip, so a
/// click on a segment is also delivered as an ordinary tray-icon click. The two
/// deliveries are one gesture and must not both act on it: the strip's own
/// handlers are the ones that know which segment was hit.
///
/// Tracks our own injections and reverts, which is all it claims — a strip that
/// injected but never managed to draw still reads as up. That is why the icon's
/// *right* click stays live either way: it keeps the panel, and Quit, reachable
/// even if this is optimistic.
pub fn strip_is_up() -> bool {
    STRIP_UP.load(std::sync::atomic::Ordering::SeqCst)
}

/// Inject the TAP into the shell's Explorer. Best-effort by contract: every error
/// means "no strip this time", and the message says why.
fn enable(icons: StripIcons) -> Result<()> {
    // Cleared up front so that any failure below leaves it false; only the
    // success path at the end sets it.
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
    /// Long enough to cover "audio-tray started as the shell came up", short enough
    /// that a deliberate revert and re-inject is never mistaken for a duplicate.
    const WINDOW: std::time::Duration = std::time::Duration::from_secs(10);

    LAST_INJECTED
        .lock()
        .ok()
        .and_then(|last| *last)
        .is_some_and(|(was, at)| was == pid && at.elapsed() < WINDOW)
}

/// Window class of the TAP's control window, inside `explorer.exe`. Must match
/// `CONTROL_CLASS` in the TAP's `lifecycle` module.
const TAP_CONTROL_CLASS: &str = "AudioTrayTapControl";

/// "Put the taskbar back." Must match `WM_TAP_REVERT` in the TAP's `lifecycle`
/// module.
const WM_TAP_REVERT: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 21;

/// Ask the injected TAP to undo its changes.
///
/// The DLL stays loaded — it cannot do this work on the way out (`DllMain`'s
/// detach runs under the loader lock, on a thread that may not touch XAML) and it
/// does not need to, because the changes are ordinary property edits and are
/// reversible in place. What the TAP unloading would buy us is a page of memory;
/// what it would risk is freeing code Explorer still holds callbacks into.
///
/// Best-effort and quiet: a missing control window means nothing is injected, so
/// there is nothing to put back.
pub fn revert() {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

    // Before the post, and unconditionally: from here on the plain icon is the
    // whole of the UI again, so its clicks have to go back to being the ones that
    // matter. Nothing about a failed post would make a strip reappear.
    STRIP_UP.store(false, std::sync::atomic::Ordering::SeqCst);
    let Some(control) = control_window() else {
        return;
    };
    // Posted, not sent: this must never block on Explorer's UI thread, and there
    // is nothing to learn from the answer.
    if let Err(e) = unsafe { PostMessageW(Some(control), WM_TAP_REVERT, WPARAM(0), LPARAM(0)) } {
        eprintln!("taskbar: could not ask for a revert ({e})");
    }
}

/// The injected TAP's control window, if there is one.
fn control_window() -> Option<windows::Win32::Foundation::HWND> {
    window_by_class(TAP_CONTROL_CLASS)
}

/// A top-level window with this exact class name, in any process.
///
/// `EnumWindows` rather than `FindWindow`, which does not locate these windows
/// across processes — measured in both directions, for us finding the TAP's
/// control window and for the TAP finding our receiver.
fn window_by_class(class_name: &str) -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW};
    use windows_core::BOOL;

    struct Search<'a> {
        wanted: &'a str,
        found: HWND,
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        let mut class = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd, &mut class) };
        if len > 0 && String::from_utf16_lossy(&class[..len as usize]) == search.wanted {
            search.found = hwnd;
            return BOOL(0);
        }
        BOOL(1)
    }

    let mut search = Search { wanted: class_name, found: HWND(std::ptr::null_mut()) };
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize)) };
    (!search.found.0.is_null()).then_some(search.found)
}

/// The process owning the desktop window — the Explorer that hosts the taskbar,
/// not a file-browser window that happens to share the name.
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
    // `InitializeXamlDiagnosticsEx` is exported from the system XAML runtime, which
    // has no import library — resolve it dynamically.
    let module = LoadLibraryW(PCWSTR(wide("Windows.UI.Xaml.dll").as_ptr()))
        .context("load Windows.UI.Xaml.dll")?;
    let symbol = GetProcAddress(module, PCSTR(c"InitializeXamlDiagnosticsEx".as_ptr().cast()))
        .context("Windows.UI.Xaml.dll does not export InitializeXamlDiagnosticsEx")?;
    let initialize: InitializeXamlDiagnosticsEx = std::mem::transmute(symbol);

    // Both DLL parameters get the TAP's own path, matching the known-good C++ TAPs.
    let endpoint = wide(ENDPOINT_NAME);
    let path = wide(&dll.to_string_lossy());
    let init_data = wide(&init_data(icons));

    let hr = initialize(
        PCWSTR(endpoint.as_ptr()),
        pid,
        PCWSTR(path.as_ptr()),
        PCWSTR(path.as_ptr()),
        CLSID_TAP,
        PCWSTR(init_data.as_ptr()),
    );
    if hr.is_err() {
        // XAML Diagnostics is effectively single-consumer: TranslucentTB and
        // Windhawk's Taskbar Styler connect to this same `VisualDiagConnection1`
        // endpoint. A bare HRESULT sends people hunting for a bug in our code, so
        // name the likeliest cause alongside it.
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

/// What the strip should draw right now.
///
/// The glyphs are the *current devices'* icons, resolved exactly as the flyout and
/// the tray icon resolve theirs — a per-device override from the config if there is
/// one, otherwise the form-factor default. Sending fixed Volume/Microphone glyphs
/// instead was wrong in a visible way: the flyout showed a laptop, the taskbar
/// showed a speaker, for the same device.
///
/// One compromise carried over from [`crate::icons`]: the two earbud icons have no
/// glyph in Segoe Fluent and the tray hand-draws them. The strip can only render a
/// font glyph, so those fall back to the headphone glyph.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StripIcons {
    pub output: char,
    pub input: char,
    pub output_muted: bool,
    pub input_muted: bool,
}

/// Fallbacks for when the devices cannot be resolved: Volume and Microphone.
impl Default for StripIcons {
    fn default() -> Self {
        Self {
            output: '\u{E767}',
            input: '\u{E720}',
            output_muted: false,
            input_muted: false,
        }
    }
}

/// Codepoints standing in for the two icons Segoe Fluent has no glyph for.
///
/// **Plane 15 private use, not the BMP private-use area.** Segoe Fluent Icons
/// itself occupies much of the latter (roughly U+E700..U+F8B3), so a BMP PUA
/// codepoint could collide with a real glyph. These two are guaranteed not to.
///
/// The TAP recognises them and draws the shapes as XAML vectors rather than looking
/// for a glyph. Must match `EARBUDS_WIRELESS` / `EARBUDS_ROUND` in the TAP's
/// `decorate` module.
const GLYPH_WIRELESS_EARBUDS: char = '\u{F0001}';
const GLYPH_ROUND_EARBUDS: char = '\u{F0002}';

/// The codepoint the strip should carry for an icon.
///
/// Mostly `IconId::glyph`, but the two earbud variants are hand-drawn rather than
/// font glyphs — `glyph` returns the headphone codepoint for them as a deliberate
/// fallback, which in the strip would silently show the wrong icon. This maps them
/// to the markers above instead, so the TAP can draw the real shape.
pub fn strip_glyph(icon: crate::icons::IconId) -> char {
    use crate::icons::IconId;
    match icon {
        IconId::WirelessEarbuds => GLYPH_WIRELESS_EARBUDS,
        IconId::RoundEarbuds => GLYPH_ROUND_EARBUDS,
        other => other.glyph(),
    }
}

/// Alpha applied to the accent fill, as hex. A fully opaque accent block is
/// brighter than anything Windows puts in a taskbar; at half alpha the pill
/// carries the same visual weight as the Control Center button beside it.
const PILL_ALPHA: &str = "80";

/// The `key=value;` payload handed to the TAP as initialization data.
///
/// This is the only chance to configure the strip — it is read once in
/// `SetSite`. Passing an empty string (as this did before) is not neutral: the
/// TAP falls back to bare glyphs with no accent pill and leaves Explorer's own
/// volume icon in place, which is a visibly different control from the one the
/// design settles on.
///
/// `tooltip` targets *our* tray icon by its accessible name; without it the TAP
/// decorates whichever notify icon it happens to meet first. It is matched as a
/// **substring**, because the tooltip is mostly the current device's name — only
/// [`crate::tray::TRAY_MARKER`] is stable across device switches and locales.
///
/// `pid` is how the strip gets cleaned up when we are killed or crash rather than
/// quitting: the TAP waits on this process and reverts when it exits. Without it
/// a `taskkill` would leave a strip behind whose every click is posted to a
/// process that no longer exists.
fn init_data(icons: StripIcons) -> String {
    let [r, g, b] = crate::flyout::theme::accent_rgb();
    format!(
        "tooltip={};out={:04X};in={:04X};\
         outmuted={};inmuted={};accent={r:02X}{g:02X}{b:02X};alpha={PILL_ALPHA};hidevolume=1;pid={}",
        crate::tray::TRAY_MARKER,
        icons.output as u32,
        icons.input as u32,
        u8::from(icons.output_muted),
        u8::from(icons.input_muted),
        std::process::id()
    )
}

/// "Redraw the strip with these glyphs." Must match `WM_TAP_RESTYLE` in the TAP's
/// `lifecycle` module.
const WM_TAP_RESTYLE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 23;

/// Tell an injected TAP that the devices changed, so the strip follows them.
///
/// The init data is read once, in `SetSite`, so it cannot carry this — switching
/// devices after injection would otherwise leave the strip showing the icon of a
/// device that is no longer default.
///
/// Both glyphs fit in a message's parameters, so nothing has to be shared: the
/// codepoint goes in the low bits and the muted flag above it. Best-effort and
/// quiet, like [`revert`] — no control window means nothing is injected.
///
/// Returns whether the strip was actually told. The caller remembers what it has
/// posted so it can skip an identical restyle, and a post that never happened must
/// not be remembered as one that did: the control window does not exist until the
/// TAP's first visual-tree callback, so an early switch can land in that gap and
/// would otherwise leave the strip showing the injection's glyphs for good.
#[must_use]
pub fn restyle(icons: StripIcons) -> bool {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

    let Some(control) = control_window() else {
        return false;
    };
    let pack = |glyph: char, muted: bool| glyph as usize | (usize::from(muted) << 24);
    let posted = unsafe {
        PostMessageW(
            Some(control),
            WM_TAP_RESTYLE,
            WPARAM(pack(icons.output, icons.output_muted)),
            LPARAM(pack(icons.input, icons.input_muted) as isize),
        )
    };
    if let Err(e) = posted {
        eprintln!("taskbar: could not restyle the strip ({e})");
        return false;
    }
    true
}

/// What the user did on the injected strip.
///
/// The TAP deliberately decides nothing: it reports the gesture and the segment,
/// and every question about which device comes next is answered here, where the
/// audio state actually lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    CycleOutput,
    CycleInput,
    OpenPanel,
}

impl Action {
    /// Wire codes are explicit on both sides so the exe and the DLL can be built
    /// separately without silently disagreeing about enum ordering.
    pub fn from_code(code: usize) -> Option<Self> {
        match code {
            1 => Some(Self::CycleOutput),
            2 => Some(Self::CycleInput),
            3 => Some(Self::OpenPanel),
            _ => None,
        }
    }

    /// The same codes in the other direction, for [`post_action`].
    fn code(self) -> usize {
        match self {
            Self::CycleOutput => 1,
            Self::CycleInput => 2,
            Self::OpenPanel => 3,
        }
    }
}

/// Dev: hand a running tray the gesture the strip would have sent.
///
/// Clicks on the Win11 taskbar cannot be synthesised — `SendInput` moves the
/// pointer (the hover plate lights up) but produces no `Tapped`, and the same is
/// true of the plain tray icon, so it is the shell's input path rather than our
/// wiring. That leaves the cycling behaviour unreachable from a script, which is
/// what this exists for: it posts to the receiver window, so everything from
/// [`WM_TASKBAR_ACTION`] inward runs exactly as it does for a real click. What it
/// does *not* prove is the TAP's own half — the handlers, the segment routing and
/// the doubled-event coalescing still only get exercised by a finger.
pub fn post_action(action: Action) -> Result<()> {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

    let receiver = window_by_class(RECEIVER_CLASS_NAME)
        .context("no receiver window — audio-tray is not running")?;
    unsafe { PostMessageW(Some(receiver), WM_TASKBAR_ACTION, WPARAM(action.code()), LPARAM(0)) }
        .context("post the action to the tray")
}

/// Dev: hand a running tray the scroll the TAP would have sent for `notches` wheel notches
/// over one button.
///
/// The counterpart of [`post_action`], and it exists for the same reason plus one more: the
/// touchpad half of the gesture cannot be synthesised at all (its deltas arrive from XAML,
/// inside Explorer), so this is the only way to drive fractional notches — and the readout's
/// coalescing and fade — from a script.
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

/// Window class of the receiver. Must match `RECEIVER_CLASS` in the TAP's `ipc`
/// module — the TAP finds this window by class name.
const RECEIVER_CLASS_NAME: &str = "AudioTrayTaskbarIpc";

/// The same name, wide and NUL-terminated, for `RegisterClassW`. `w!` takes a
/// literal and there is no const way back from it to a `&str`, so the two are
/// spelled out separately — keep them identical.
const RECEIVER_CLASS: PCWSTR = windows::core::w!("AudioTrayTaskbarIpc");

/// Message the TAP posts; `wParam` carries the [`Action`] code.
pub const WM_TASKBAR_ACTION: u32 =
    windows::Win32::UI::WindowsAndMessaging::WM_APP + 20;

/// A scroll over one of the buttons: `wParam` is the direction ([`flow_code`]) and `lParam`
/// the signed wheel delta, in `WHEEL_DELTA` units.
///
/// Its own message rather than another [`Action`] code, because the tray *coalesces* these —
/// a precision touchpad produces a stream of sub-notch deltas, and one round of COM per
/// delta would fall behind the finger. Coalescing means draining every one that is queued,
/// and draining `WM_TASKBAR_ACTION` would swallow queued clicks with them.
///
/// Posted from two places, which is why the payload is this and not a pointer: the TAP, for
/// a scroll that XAML delivered over a button (the touchpad's only route in — see
/// [`crate::tray`]), and the tray's own mouse hook, as a thread message. Must match
/// `WM_TASKBAR_SCROLL` in the TAP's `ipc` module.
pub const WM_TASKBAR_SCROLL: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 24;

/// Wire code for a direction in [`WM_TASKBAR_SCROLL`]'s `wParam`. Explicit on both sides, so
/// the exe and the DLL can be built separately without agreeing by accident.
pub fn flow_code(flow: crate::audio::Flow) -> usize {
    match flow {
        crate::audio::Flow::Output => 0,
        crate::audio::Flow::Input => 1,
    }
}

/// The other direction of [`flow_code`]. Anything unrecognised reads as output — the
/// direction the wheel has always adjusted.
pub fn flow_from_code(code: usize) -> crate::audio::Flow {
    match code {
        1 => crate::audio::Flow::Input,
        _ => crate::audio::Flow::Output,
    }
}

/// Explorer restarted — re-inject. Posted to itself by the receiver's window
/// procedure; see [`create_receiver`] for why it cannot be observed directly.
pub const WM_TASKBAR_RESTARTED: u32 =
    windows::Win32::UI::WindowsAndMessaging::WM_APP + 22;

/// The shell's "the taskbar is back" broadcast, registered once.
fn taskbar_created_message() -> u32 {
    use std::sync::OnceLock;
    static ID: OnceLock<u32> = OnceLock::new();
    *ID.get_or_init(|| unsafe {
        windows::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW(windows::core::w!(
            "TaskbarCreated"
        ))
    })
}

/// Creates the hidden window the TAP posts to.
///
/// Deliberately a *top-level* window rather than a message-only one: message-only
/// windows are not reachable by `FindWindow`, and searching for them through
/// `FindWindowEx(HWND_MESSAGE, …)` did not find this window across processes
/// either. A never-shown, zero-sized tool window is findable by class name and
/// costs the same. `WS_EX_TOOLWINDOW` keeps it out of the taskbar and Alt-Tab,
/// and it is never given `SW_SHOW`, so nothing appears on screen.
///
/// Created on the tray thread so its messages arrive in the tray's own
/// `GetMessage` loop — no extra thread, and no locking around the audio state.
pub fn create_receiver() -> Result<windows::Win32::Foundation::HWND> {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, PostMessageW, RegisterClassW, WNDCLASSW, WS_EX_TOOLWINDOW,
        WS_POPUP,
    };

    // Mostly a pass-through: messages the TAP posts here are picked up by the
    // tray thread's own `GetMessage` loop, which is where the audio state lives.
    //
    // `TaskbarCreated` is the exception, and it has to be handled here. The shell
    // *sends* that broadcast rather than posting it, so it is delivered straight
    // to this procedure and never enters the message queue — a `GetMessage` loop
    // cannot see it at all. Measured: a hand-rolled `PostMessage(HWND_BROADCAST)`
    // showed up in the loop immediately, while three real Explorer restarts
    // produced nothing, even after 30 seconds. Re-posting it to ourselves is what
    // gets it into the queue, where the loop can act on it with the config in
    // scope.
    unsafe extern "system" fn proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == taskbar_created_message() {
            let _ = unsafe { PostMessageW(Some(hwnd), WM_TASKBAR_RESTARTED, WPARAM(0), LPARAM(0)) };
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    unsafe {
        let instance = GetModuleHandleW(None).context("GetModuleHandle")?;
        // Registering twice is harmless; the second attempt just fails.
        let class = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            lpszClassName: RECEIVER_CLASS,
            ..Default::default()
        };
        RegisterClassW(&class);

        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            RECEIVER_CLASS,
            RECEIVER_CLASS,
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .context("create the taskbar IPC receiver window")
    }
}

/// Whether this process has already restarted Explorer to repair the strip.
///
/// One restart is the budget for the life of the process, and both repair triggers draw on that
/// same one. The failures they answer can be permanent — an endpoint held by another
/// XAML-diagnostics consumer survives any number of restarts — and a self-healing loop that
/// rebuilds the shell over and over would be far worse than having no strip.
static HEALED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Rebuild Explorer to get a clean injection, at most once per process. Returns whether one
/// was started.
///
/// The waiting happens on a worker thread because the caller is the tray thread, and
/// `TaskbarCreated` is *sent* to our receiver window rather than posted (see
/// [`create_receiver`]) — only a thread sitting in `GetMessage` receives it, and that message is
/// what re-registers the notification icon and re-injects the strip. Blocking here would stall
/// the very restart we asked for.
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

/// Whether a TAP is *already* live in this Explorer, before we have injected — so one left
/// behind by an earlier audio-tray, which the shell keeps loaded for its own lifetime whether
/// or not it reverted (see the module docs).
///
/// Detected by its control window: every TAP instance creates its own (the class registration is
/// shared, the window is not), and the window belongs to `explorer.exe`, so it outlives the
/// audio-tray that put it there. Cheaper than reading Explorer's module list, and needs no
/// rights over another process.
fn tap_already_present() -> bool {
    control_window().is_some()
}

/// Attempts, and the gap between them, before an injection failure is treated as real.
///
/// What this absorbs is a shell that is not ready *yet* rather than one that never will be:
/// audio-tray autostarts at sign-in, where it can easily beat Explorer's XAML runtime to being
/// ready. Seconds spent retrying are cheap in a case that is already broken, and it keeps the
/// Explorer restart below for failures that are genuinely persistent — rebuilding the shell in
/// the middle of sign-in would be both disruptive and useless.
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

/// Put the strip up at startup. Never fatal: a failure here leaves the plain tray
/// icon as the whole of the UI, which is what the app looked like before the strip.
///
/// Rather than settle for that, this repairs the shell in the two situations where a fresh
/// Explorer is what is actually needed — each at most once, see [`heal_explorer`].
pub fn apply_at_startup(icons: StripIcons) {
    // A TAP already in this Explorer means an earlier audio-tray injected into it and the DLL is
    // still there. Injecting now would make ours the *second*, and two TAPs in one shell is not
    // benign: observed live, the older one's owner-watch fired its revert and undid the
    // decoration the newer one had just applied — leaving a bare notification icon, and
    // Explorer's own volume slot back, while every signal we have said the strip was up.
    //
    // So rebuild the shell and let `TaskbarCreated` inject into a clean one. This is also what
    // completes an update: taking one relaunches audio-tray, so the new process meets the old
    // process's TAP right here, and the restart that clears it is the same restart that frees
    // `audio_tray_tap.dll` for `crate::update::place_staged_tap`.
    //
    // Silent in the normal case — at sign-in Explorer is new and carries no TAP.
    if tap_already_present() && heal_explorer("another TAP is already loaded in Explorer") {
        return;
    }
    match enable_with_retries(icons) {
        Ok(()) => eprintln!("taskbar: controls enabled"),
        Err(e) => {
            eprintln!("taskbar: integration unavailable, using the plain tray icon ({e:#})");
            // Deliberately only from here and not from `apply_at_restart`: an injection failure
            // straight after an Explorer restart the *user* performed should not be answered by
            // restarting it again underneath them.
            heal_explorer("the injection failed");
        }
    }
}

/// Re-inject after Explorer restarted.
///
/// The TAP lives inside `explorer.exe` and dies with it, taking the strip along.
/// Nothing needs reverting in that case — the process that held our changes is
/// gone — but without this the strip would stay gone until audio-tray itself was
/// restarted.
///
/// Runs on the tray thread, where COM is already initialized, and inherits
/// [`enable`]'s contract: a failure means the plain tray icon carries on alone.
pub fn apply_at_restart(icons: StripIcons) {
    // A shell that has just restarted broadcasts `TaskbarCreated`, and audio-tray
    // starting up injects on its own account. Both can land within a second of each
    // other, which put two TAP instances in one Explorer — observed in the log as a
    // second `SetSite` for the same pid. The duplicate is harmless (the newer
    // generation supersedes the older) but it costs a COM object and an owner-watch
    // thread for nothing.
    //
    // Deliberately *not* a check for "is a TAP already loaded": after a revert the
    // DLL and its control window are still there, so that test would refuse the
    // re-injection it is meant to allow.
    if shell_pid().is_ok_and(just_injected) {
        return;
    }
    match enable(icons) {
        Ok(()) => eprintln!("taskbar: Explorer restarted — controls re-injected"),
        Err(e) => eprintln!("taskbar: Explorer restarted, re-injection failed ({e:#})"),
    }
}

/// The shell's private "exit Explorer" command — what the hidden Ctrl+Shift+right-click
/// "Exit Explorer" item on the taskbar posts. Explorer shuts down the orderly way (it saves
/// its state and closes its windows) *and* the exit counts as deliberate, so Winlogon's
/// `AutoRestartShell` does not bring it back. That is why [`restart_explorer`] launches the
/// replacement itself.
const WM_SHELL_EXIT: u32 = 0x5B4;

/// How long the polite request gets before the process is terminated instead.
///
/// Short on purpose. Measured on Win11 26200, both outcomes: with no TAP in Explorer the
/// graceful exit lands in well under a second, and with our TAP loaded — the normal state,
/// since the strip is injected on every start — it does not happen at all and the wait is pure
/// delay before the fallback does the work. So this only has to be long enough for the case
/// that succeeds quickly, not generous enough for one that never finishes.
const SHELL_EXIT_WAIT_MS: u32 = 2_500;

/// Restart `explorer.exe`, and with it the strip.
///
/// Offered by the flyout's footer in the two cases where only a fresh Explorer will do — no
/// strip this start, or a staged update whose new TAP is stuck behind the DLL Explorer holds
/// open (see `flyout::layout::footer_buttons`). Both are conditions the user can otherwise
/// only clear by signing out or rebooting.
///
/// **Blocks for as long as the shell takes to go and come back** (seconds), so callers keep
/// it off a thread that has to pump messages — see `crate::tray`.
///
/// Nothing here puts the strip back, deliberately: a restarted Explorer broadcasts
/// `TaskbarCreated`, and [`apply_at_restart`] is already wired to it for the Explorer restarts
/// we do not cause. This path is not special enough to need its own.
pub fn restart_explorer() -> Result<()> {
    use windows::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_TERMINATE};

    let pid = shell_pid()?;
    // Opened *before* Explorer is asked to leave, and waited on rather than polled for: a
    // handle goes on identifying this process after it exits, where a pid can be recycled
    // onto something else entirely in the gap. `SYNCHRONIZE` is bound as a *file* access
    // right, but the bit is the generic one every waitable handle uses, so it only needs
    // re-wrapping for a process.
    let access = PROCESS_ACCESS_RIGHTS(SYNCHRONIZE.0) | PROCESS_TERMINATE;
    let shell = unsafe { OpenProcess(access, false, pid) }.context("open the shell process")?;

    // The strip dies with the process hosting it, and every path below ends with this
    // Explorer gone. Cleared up front so a failure part-way cannot leave the tray icon
    // believing its clicks still belong to a strip that is not there.
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

    // Ask nicely first, so Explorer saves its state instead of being shot. `window_by_class`
    // rather than `FindWindow` for the reason given in its own docs.
    let asked = match window_by_class("Shell_TrayWnd") {
        Some(tray) => unsafe { PostMessageW(Some(tray), WM_SHELL_EXIT, WPARAM(0), LPARAM(0)) }
            .inspect_err(|e| eprintln!("taskbar: could not ask Explorer to exit ({e})"))
            .is_ok(),
        // No taskbar window at all — there is nothing to ask, so go straight to the fallback.
        None => {
            eprintln!("taskbar: no Shell_TrayWnd to ask for an exit");
            false
        }
    };

    let gone = |ms: u32| unsafe { WaitForSingleObject(shell, ms) } == WAIT_OBJECT_0;
    // Whether the shell had to be shot, which decides how long to wait for a replacement.
    let mut terminated = false;
    if !asked || !gone(SHELL_EXIT_WAIT_MS) {
        // Routine rather than exceptional: measured, an Explorer with our TAP loaded ignores
        // the request, and that is the state whenever the strip is up. `WM_SHELL_EXIT` is also
        // undocumented and could stop working on a future build. Either way a button that
        // silently does nothing would be the worse failure, so fall back to what
        // `taskkill /f /im explorer.exe` does — the shell is built to be killed, which is what
        // `AutoRestartShell` exists for.
        eprintln!("taskbar: Explorer did not exit on request — terminating it instead");
        unsafe { TerminateProcess(shell, 1) }.context("terminate the shell process")?;
        terminated = true;
        if !gone(SHELL_EXIT_WAIT_MS) {
            bail!("the shell process would not exit");
        }
    }

    // The one moment nothing holds `audio_tray_tap.dll`: the old Explorer has released it and
    // the new one is not started yet. A TAP update that was waiting for a reboot can land
    // now — and it has to be *here*, because the re-injection that follows `TaskbarCreated`
    // would otherwise pull the stale DLL straight back into the new process.
    crate::update::place_staged_tap();

    // Launching explorer.exe while a shell is already up opens a file-browser window rather
    // than a second shell, so look before leaping: a graceful exit is recorded as deliberate
    // and nothing restarts the shell for us, but a terminate is the kind of death
    // `AutoRestartShell` is meant to answer, so allow a little longer for one to appear on its
    // own. Only a *ceiling* — `wait_for_shell` returns the moment one shows up.
    //
    // Kept small because every millisecond of it is a taskbar the user does not have. Measured
    // on Win11 26200: nothing restarted the shell for us on either path, terminate included,
    // so in practice this budget is spent in full and then our own launch does the work.
    let budget = std::time::Duration::from_millis(if terminated { 1_500 } else { 500 });
    if wait_for_shell(budget) {
        eprintln!("taskbar: Explorer restarted itself");
        return Ok(());
    }
    // Detached from our streams on purpose. A child inherits them by default, and this child
    // outlives us by the whole session — so an explorer.exe launched from, say, a dev CLI run
    // would sit holding that console (or redirected file) open long after audio-tray is gone.
    // Nothing here wants to read explorer's output either way.
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

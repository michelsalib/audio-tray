//! Opt-in Explorer integration: the icon + chevron drawn inside the taskbar itself.
//!
//! **This is never on by default.** The plain `Shell_NotifyIcon` tray entry is
//! registered unconditionally by `crate::tray`, and everything here is additive.
//! If injection is disabled, unavailable, or fails, the app behaves exactly as it
//! always has — callers treat every error as "feature off", never as fatal.
//!
//! Mechanism (see `spikes/xaml-tap/FINDINGS.md` for the measurements behind it):
//! `InitializeXamlDiagnosticsEx` loads `audio_tray_tap.dll` into `explorer.exe`,
//! where it implements `IVisualTreeServiceCallback2`, watches the XAML tree for
//! our `SystemTray.NotifyIconView`, and restyles it.
//!
//! Turning it off is a revert, not an unload. The TAP pins itself in
//! `explorer.exe` and stays there, but everything it changed — the tray icon's
//! content, the tray sections' columns, Explorer's own volume slot — is an
//! ordinary property edit that it recorded before making and can undo in place.
//! [`revert`] asks for that; the DLL then sits inert until the feature is turned
//! back on. Unloading it instead would gain a page of memory and risk freeing
//! code Explorer still holds callbacks into.
//!
//! Four things have to lead to that revert, and each has its own trigger:
//!   * the user turns the feature off       → [`disable`]
//!   * audio-tray quits                     → [`revert`], from the tray loop
//!   * audio-tray is killed or crashes      → the TAP waits on our process id,
//!     passed in [`init_data`], and reverts when it exits
//!   * Explorer restarts                    → nothing to revert; the DLL died
//!     with it, and [`apply_at_restart`] injects into the new one
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

/// Inject the TAP into the shell's Explorer. Best-effort by contract: every error
/// means "feature stays off", and `enable`'s message says why.
pub fn enable() -> Result<()> {
    let dll = tap_path()?;
    let pid = shell_pid()?;
    unsafe { inject(pid, &dll) }
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
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, PostMessageW};
    use windows_core::BOOL;

    // `EnumWindows` rather than `FindWindow`, which does not locate this window
    // across processes — the same thing was measured in the other direction, for
    // the TAP finding our receiver.
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let mut class = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd, &mut class) };
        if len > 0 && String::from_utf16_lossy(&class[..len as usize]) == TAP_CONTROL_CLASS {
            unsafe { *(lparam.0 as *mut HWND) = hwnd };
            return BOOL(0);
        }
        BOOL(1)
    }

    let mut found = HWND(std::ptr::null_mut());
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut found as *mut HWND as isize)) };
    if found.0.is_null() {
        return;
    }
    // Posted, not sent: this must never block on Explorer's UI thread, and there
    // is nothing to learn from the answer.
    if let Err(e) = unsafe { PostMessageW(Some(found), WM_TAP_REVERT, WPARAM(0), LPARAM(0)) } {
        eprintln!("taskbar: could not ask for a revert ({e})");
    }
}

/// Turn the feature off: revert now, and report what the user will see.
pub fn disable() -> &'static str {
    revert();
    "Taskbar controls removed."
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

unsafe fn inject(pid: u32, dll: &std::path::Path) -> Result<()> {
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
    let init_data = wide(&init_data());

    let hr = initialize(
        PCWSTR(endpoint.as_ptr()),
        pid,
        PCWSTR(path.as_ptr()),
        PCWSTR(path.as_ptr()),
        CLSID_TAP,
        PCWSTR(init_data.as_ptr()),
    );
    if hr.is_err() {
        bail!(
            "InitializeXamlDiagnosticsEx failed: 0x{:08x} ({})",
            hr.0,
            windows_core::Error::from(hr).message()
        );
    }
    Ok(())
}

/// Segoe Fluent glyphs the strip draws by default: Volume and Microphone. They
/// match the flyout's own unmuted icons so the two never disagree.
const GLYPH_OUTPUT: u32 = 0xE767;
const GLYPH_INPUT: u32 = 0xE720;

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
fn init_data() -> String {
    let [r, g, b] = crate::flyout::theme::accent_rgb();
    format!(
        "tooltip={};out={GLYPH_OUTPUT:04X};in={GLYPH_INPUT:04X};\
         outmuted=0;inmuted=0;accent={r:02X}{g:02X}{b:02X};alpha={PILL_ALPHA};hidevolume=1;pid={}",
        crate::tray::TRAY_MARKER,
        std::process::id()
    )
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
}

/// Window class of the receiver. Must match `RECEIVER_CLASS` in the TAP's `ipc`
/// module — the TAP finds this window by class name.
const RECEIVER_CLASS: PCWSTR = windows::core::w!("AudioTrayTaskbarIpc");

/// Message the TAP posts; `wParam` carries the [`Action`] code.
pub const WM_TASKBAR_ACTION: u32 =
    windows::Win32::UI::WindowsAndMessaging::WM_APP + 20;

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

/// Apply the configured state at startup. Silent no-op when the opt-in is off,
/// which is the default.
pub fn apply_at_startup(enabled: bool) {
    if !enabled {
        return;
    }
    match enable() {
        Ok(()) => eprintln!("taskbar: controls enabled"),
        Err(e) => eprintln!("taskbar: integration unavailable, using the plain tray icon ({e:#})"),
    }
}

/// Re-inject after Explorer restarted.
///
/// The TAP lives inside `explorer.exe` and dies with it, taking the strip along.
/// Nothing needs reverting in that case — the process that held our changes is
/// gone — but without this the feature would stay silently off until audio-tray
/// itself was restarted.
///
/// Runs on the tray thread, where COM is already initialized, and inherits
/// [`enable`]'s contract: a failure means the plain tray icon carries on alone.
pub fn apply_at_restart(enabled: bool) {
    if !enabled {
        return;
    }
    match enable() {
        Ok(()) => eprintln!("taskbar: Explorer restarted — controls re-injected"),
        Err(e) => eprintln!("taskbar: Explorer restarted, re-injection failed ({e:#})"),
    }
}

//! The injector — runs as a normal user process and asks XAML diagnostics to
//! load `audio_tray_tap.dll` into `explorer.exe`.
//!
//! Usage:
//!   xaml-tap-inject [--pid N] [--dll PATH] [--diag-dll PATH] [--wait SECS]
//!                   [--debug]
//!   xaml-tap-inject --revert
//!
//! `--debug` turns on the exploratory logging — the raw event trace and the
//! periodic visual-tree dumps. Off by default: a single session with it on
//! measured 15 MB and 197k lines, 92% of that the dumps.
//!
//! The TAP is never unloaded — it pins itself (`DllCanUnloadNow` returns
//! `S_FALSE`), and the undo is a revert rather than an eject: `--revert` asks the
//! injected TAP to put the taskbar back, after which it sits inert. Restarting
//! Explorer is only needed to load a *rebuilt* DLL.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
use windows_core::{GUID, HRESULT, PCSTR, PCWSTR};
use audio_tray_tap::lifecycle::{CONTROL_CLASS, WM_TAP_REVERT};
use audio_tray_tap::{wide, CLSID_TAP, ENDPOINT_NAME, XAML_DLL};

type InitializeXamlDiagnosticsEx = unsafe extern "system" fn(
    end_point_name: PCWSTR,
    pid: u32,
    wsz_dll_xaml_diagnostics: PCWSTR,
    wsz_tap_dll_name: PCWSTR,
    tap_clsid: GUID,
    wsz_initialization_data: PCWSTR,
) -> HRESULT;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    // Asks an already-injected TAP to undo its changes, without injecting
    // anything. Stands in for audio-tray quitting or the user toggling the
    // feature off, so the revert path can be exercised on its own.
    if args.iter().any(|a| a == "--revert") {
        return post_revert();
    }

    let pid = match flag("--pid") {
        Some(value) => value.parse()?,
        None => shell_pid()?,
    };

    // Default to the DLL sitting next to this exe, which is where cargo puts it.
    let dll = match flag("--dll") {
        Some(value) => PathBuf::from(value),
        None => std::env::current_exe()?
            .parent()
            .ok_or("exe has no parent dir")?
            .join("audio_tray_tap.dll"),
    };
    if !dll.is_file() {
        return Err(format!("TAP dll not found: {}", dll.display()).into());
    }
    // Absolute, but *not* a `\\?\` verbatim path — that prefix survives
    // `canonicalize` and confuses the loader on the far side.
    let dll = strip_verbatim(dll.canonicalize()?);

    // Prior art (TranslucentTB's ExplorerTAP, Windhawk's Taskbar Styler) passes
    // the TAP's own path for both DLL parameters. Overridable because the
    // header documents the third parameter only as "the XAML diagnostics dll".
    let diag_dll = flag("--diag-dll").map_or_else(|| dll.clone(), PathBuf::from);
    let wait_secs: u64 = flag("--wait").map_or(Ok(5), |v| v.parse())?;

    // What the TAP should draw, passed through as initialization data. Glyphs are
    // Segoe Fluent codepoints in hex. `--tooltip` picks which tray icon to
    // decorate; empty means "the first one found".
    let has = |name: &str| args.iter().any(|a| a == name);
    // `--no-pill` drops the accent fill back to bare glyphs.
    let accent = if has("--no-pill") {
        String::new()
    } else {
        flag("--accent").unwrap_or_else(|| {
            accent_rgb().map_or(String::new(), |[r, g, b]| format!("{r:02X}{g:02X}{b:02X}"))
        })
    };
    // `--owner PID` is what audio-tray passes as its own process id: the TAP
    // waits on that process and reverts when it dies, so a killed or crashed
    // owner does not leave a dead strip on the taskbar.
    // `--debug` turns on the exploratory logging: the raw event trace and the
    // periodic visual-tree dumps. Off by default because it is measured in
    // megabytes per session, and nothing the strip does needs it.
    let init_data = format!(
        "tooltip={};out={};in={};outmuted={};inmuted={};accent={};alpha={};hidevolume={};pid={};debug={}",
        flag("--tooltip").unwrap_or_default(),
        flag("--out").unwrap_or_else(|| "E767".into()),
        flag("--in").unwrap_or_else(|| "E720".into()),
        u8::from(has("--muted-out")),
        u8::from(has("--muted-in")),
        accent,
        // Hex; "80" is the agreed 50% fill.
        flag("--alpha").unwrap_or_else(|| "80".into()),
        u8::from(has("--hide-system-volume")),
        flag("--owner").unwrap_or_default(),
        u8::from(has("--debug")),
    );

    println!("target pid  : {pid}");
    println!("tap dll     : {}", dll.display());
    println!("diag dll    : {}", diag_dll.display());
    println!("endpoint    : {ENDPOINT_NAME}");
    println!("clsid       : {CLSID_TAP:?}");
    println!("init data   : {init_data}");

    // Only new log output is interesting; the file accumulates across runs.
    let log_path = std::env::temp_dir().join("xaml-tap.log");
    let log_offset = std::fs::metadata(&log_path).map_or(0, |m| m.len());

    let hr = unsafe { inject(pid, &dll, &diag_dll, &init_data)? };
    println!("\nInitializeXamlDiagnosticsEx -> 0x{:08x}", hr.0);
    if hr.is_err() {
        println!("  {}", windows_core::Error::from(hr).message());
        return Err("injection failed".into());
    }

    println!("waiting {wait_secs}s for the tree dump…\n");
    std::thread::sleep(std::time::Duration::from_secs(wait_secs));
    print_log_since(&log_path, log_offset)?;
    println!("\nfull log: {}", log_path.display());
    Ok(())
}

/// Finds the TAP's control window and asks it to revert.
///
/// `EnumWindows` rather than `FindWindow` for the same reason the TAP uses it to
/// find audio-tray: `FindWindow` does not locate this window across processes,
/// while enumerating and matching the class name does.
fn post_revert() -> Result<(), Box<dyn std::error::Error>> {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, PostMessageW};
    use windows_core::BOOL;

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let mut class = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd, &mut class) };
        if len > 0 && String::from_utf16_lossy(&class[..len as usize]) == CONTROL_CLASS {
            unsafe { *(lparam.0 as *mut HWND) = hwnd };
            return BOOL(0);
        }
        BOOL(1)
    }

    let log_path = std::env::temp_dir().join("xaml-tap.log");
    let log_offset = std::fs::metadata(&log_path).map_or(0, |m| m.len());

    let mut found = HWND(std::ptr::null_mut());
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut found as *mut HWND as isize)) };
    if found.0.is_null() {
        return Err(format!("no {CONTROL_CLASS} window — is the TAP injected?").into());
    }
    println!("control window: 0x{:x}", found.0 as usize);
    unsafe { PostMessageW(Some(found), WM_TAP_REVERT, WPARAM(0), LPARAM(0)) }?;
    println!("posted WM_TAP_REVERT; waiting 2s\n");
    std::thread::sleep(std::time::Duration::from_secs(2));
    print_log_since(&log_path, log_offset)?;
    Ok(())
}

/// The user's accent colour, from the same place the flyout reads it: the
/// "Light2" shade of `Explorer\Accent\AccentPalette`, an 8-entry RGBA blob
/// ordered lightest→darkest. Keeping the source identical is what makes the
/// taskbar pill and the flyout agree.
fn accent_rgb() -> Option<[u8; 3]> {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_BINARY};
    let subkey = wide(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent");
    let value = wide("AccentPalette");
    let mut buf = [0u8; 32];
    let mut size = buf.len() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    // Light2 is the second entry (bytes 4..7 = R, G, B).
    (status.0 == 0 && size >= 8).then(|| [buf[4], buf[5], buf[6]])
}

fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy().into_owned();
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => path,
    }
}

/// The process owning the desktop window — i.e. the Explorer that hosts the
/// taskbar, not some file-browser window that happens to share the name.
fn shell_pid() -> Result<u32, Box<dyn std::error::Error>> {
    let hwnd = unsafe { GetShellWindow() };
    if hwnd.0.is_null() {
        return Err("GetShellWindow returned null — is Explorer running?".into());
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return Err("could not resolve the shell pid".into());
    }
    Ok(pid)
}

unsafe fn inject(
    pid: u32,
    dll: &std::path::Path,
    diag_dll: &std::path::Path,
    init_data: &str,
) -> Result<HRESULT, Box<dyn std::error::Error>> {
    let module = LoadLibraryW(PCWSTR(wide(XAML_DLL).as_ptr()))?;
    let symbol = GetProcAddress(module, PCSTR(c"InitializeXamlDiagnosticsEx".as_ptr().cast()))
        .ok_or("InitializeXamlDiagnosticsEx not exported by Windows.UI.Xaml.dll")?;
    let initialize: InitializeXamlDiagnosticsEx = std::mem::transmute(symbol);

    // Every wide string must outlive the call, hence the bindings.
    let endpoint = wide(ENDPOINT_NAME);
    let diag_path = wide(&diag_dll.to_string_lossy());
    let tap_path = wide(&dll.to_string_lossy());
    let init_data = wide(init_data);

    Ok(initialize(
        PCWSTR(endpoint.as_ptr()),
        pid,
        PCWSTR(diag_path.as_ptr()),
        PCWSTR(tap_path.as_ptr()),
        CLSID_TAP,
        PCWSTR(init_data.as_ptr()),
    ))
}

fn print_log_since(path: &std::path::Path, offset: u64) -> std::io::Result<()> {
    let Ok(mut file) = std::fs::File::open(path) else {
        println!("(no log file yet — the TAP never loaded)");
        return Ok(());
    };
    file.seek(SeekFrom::Start(offset))?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    if text.trim().is_empty() {
        println!("(log unchanged — the TAP never loaded)");
    } else {
        print!("{text}");
    }
    Ok(())
}

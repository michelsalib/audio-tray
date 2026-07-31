//! Logging that works from inside `explorer.exe`, where there is no console.
//!
//! Everything goes to `%TEMP%\xaml-tap.log` and is mirrored to the debugger
//! (DebugView / WinDbg) via `OutputDebugStringW`.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static SINK: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// Whether the spike's exploratory output is wanted.
///
/// Off unless the init data says `debug=1`, because it is *enormous*: measured at
/// 15 MB and 197k lines from a single session, 92% of it visual-tree dumps. That is
/// fine for a spike and unacceptable for something that runs inside the shell all
/// day. The lifecycle lines — inject, decorate, revert, stand down — stay on
/// unconditionally; they are low volume and they are exactly what a bug report
/// needs.
static VERBOSE: AtomicBool = AtomicBool::new(false);

/// Beyond this, the log is truncated on next open rather than growing forever.
const MAX_BYTES: u64 = 4 * 1024 * 1024;

pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::SeqCst);
}

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::SeqCst)
}

pub fn path() -> PathBuf {
    std::env::temp_dir().join("xaml-tap.log")
}

pub fn line(text: &str) {
    let mut wide: Vec<u16> = format!("[xaml-tap] {text}\r\n").encode_utf16().collect();
    wide.push(0);
    unsafe {
        windows::Win32::System::Diagnostics::Debug::OutputDebugStringW(
            windows_core::PCWSTR(wide.as_ptr()),
        );
    }

    // A poisoned mutex must not take explorer down with it — a spike's log is
    // never worth a shell crash.
    let mut guard = match SINK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.is_none() {
        // Start over rather than append if the previous session left a large file.
        // The TAP has no uninstall hook, so nothing else would ever reclaim it.
        let oversized = std::fs::metadata(path()).is_ok_and(|meta| meta.len() > MAX_BYTES);
        *guard = OpenOptions::new()
            .create(true)
            .append(!oversized)
            .truncate(oversized)
            .write(oversized)
            .open(path())
            .ok();
    }
    if let Some(file) = guard.as_mut() {
        let _ = writeln!(file, "{text}");
        let _ = file.flush();
    }
}

macro_rules! logf {
    ($($arg:tt)*) => { crate::log::line(&format!($($arg)*)) };
}
pub(crate) use logf;

//! Logging that works from inside `explorer.exe`, where there is no console.
//!
//! Everything goes to `%TEMP%\xaml-tap.log` and is mirrored to the debugger
//! (DebugView / WinDbg) via `OutputDebugStringW`.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

static SINK: Mutex<Option<std::fs::File>> = Mutex::new(None);

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
        *guard = OpenOptions::new()
            .create(true)
            .append(true)
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

//! Logging that works from inside `explorer.exe`, where there is no console.
//!
//! Everything goes to `%TEMP%\xaml-tap.log` and is mirrored to the debugger
//! (DebugView / WinDbg) via `OutputDebugStringW`.
//!
//! Callers never touch the file: a line is stamped (local time, thread id) and sent to a writer
//! thread, so a slow disk or a reader holding the file cannot stall Explorer's UI thread. The
//! writer collapses runs of identical lines and rotates the file to `xaml-tap.log.1` at
//! [`MAX_BYTES`], during the session as well as at start.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

/// Whether the high-volume exploratory output (tree dumps) is wanted: only with `debug=1` in the
/// init data. Lifecycle lines are always logged.
static VERBOSE: AtomicBool = AtomicBool::new(false);

/// Size at which the log is rotated to `.1` (the previous `.1` is dropped).
const MAX_BYTES: u64 = 4 * 1024 * 1024;

struct Entry {
    stamp: String,
    text: String,
}

/// The writer's inbox. The `Mutex` keeps the static `Sync` on any toolchain; it is held for
/// a channel send, never for I/O.
static INBOX: OnceLock<Mutex<Sender<Entry>>> = OnceLock::new();

pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::SeqCst);
}

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::SeqCst)
}

pub fn path() -> PathBuf {
    std::env::temp_dir().join("xaml-tap.log")
}

fn rotated_path() -> PathBuf {
    std::env::temp_dir().join("xaml-tap.log.1")
}

/// `YYYY-MM-DD HH:MM:SS.mmm tid` in local time: what lines the log up against the Event Log (AppHang times are
/// local) and tells the tray thread from the advise thread.
fn stamp() -> String {
    let now = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} {:>5}",
        now.wYear,
        now.wMonth,
        now.wDay,
        now.wHour,
        now.wMinute,
        now.wSecond,
        now.wMilliseconds,
        unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }
    )
}

pub fn line(text: &str) {
    let mut wide: Vec<u16> = format!("[xaml-tap] {text}\r\n").encode_utf16().collect();
    wide.push(0);
    unsafe {
        windows::Win32::System::Diagnostics::Debug::OutputDebugStringW(
            windows_core::PCWSTR(wide.as_ptr()),
        );
    }

    let inbox = INBOX.get_or_init(|| {
        let (sender, receiver) = channel();
        // The DLL is pinned for Explorer's lifetime, so the thread never outlives its code.
        let _ = std::thread::Builder::new()
            .name("xaml-tap log".into())
            .spawn(move || write_loop(receiver));
        Mutex::new(sender)
    });
    // A poisoned mutex must not take Explorer down with it.
    let sender = match inbox.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let _ = sender.send(Entry {
        stamp: stamp(),
        text: text.to_owned(),
    });
}

/// Collapses consecutive identical lines into one plus a count.
#[derive(Default)]
struct Collapser {
    last: Option<String>,
    repeats: u64,
}

impl Collapser {
    /// The lines to write for `text`: nothing while it repeats the previous one; otherwise the
    /// pending repeat count (if any) and the line itself.
    fn feed(&mut self, stamp: &str, text: &str) -> Vec<String> {
        if self.last.as_deref() == Some(text) {
            self.repeats += 1;
            return Vec::new();
        }
        let mut out = Vec::with_capacity(2);
        if let Some(flushed) = self.flush(stamp) {
            out.push(flushed);
        }
        out.push(format!("{stamp} {text}"));
        self.last = Some(text.to_owned());
        out
    }

    /// The pending "repeated" line, if the previous line has unreported repeats.
    fn flush(&mut self, stamp: &str) -> Option<String> {
        let repeats = std::mem::take(&mut self.repeats);
        (repeats > 0).then(|| format!("{stamp} … previous line repeated {repeats} more time(s)"))
    }
}

fn open(truncate: bool) -> Option<File> {
    OpenOptions::new()
        .create(true)
        .append(!truncate)
        .truncate(truncate)
        .write(truncate)
        .open(path())
        .ok()
}

/// Moves the log to `.1` and starts a new one; truncates in place if a reader blocks the rename.
fn rotate(file: &mut Option<File>) {
    *file = None;
    let _ = std::fs::remove_file(rotated_path());
    let renamed = std::fs::rename(path(), rotated_path()).is_ok();
    *file = open(!renamed);
}

/// How long the writer waits for more lines before writing out a pending repeat count.
const QUIET: std::time::Duration = std::time::Duration::from_millis(500);

fn write_loop(receiver: Receiver<Entry>) {
    use std::sync::mpsc::RecvTimeoutError;

    let mut file = None;
    if std::fs::metadata(path()).is_ok_and(|meta| meta.len() > MAX_BYTES) {
        rotate(&mut file);
    } else {
        file = open(false);
    }
    let mut written = std::fs::metadata(path()).map_or(0, |meta| meta.len());
    let mut collapser = Collapser::default();

    loop {
        // One line (or a quiet spell), then whatever queued behind it, then one flush.
        let lines = match receiver.recv_timeout(QUIET) {
            Ok(first) => {
                let mut lines = collapser.feed(&first.stamp, &first.text);
                for entry in receiver.try_iter() {
                    lines.extend(collapser.feed(&entry.stamp, &entry.text));
                }
                lines
            }
            Err(RecvTimeoutError::Timeout) => collapser.flush(&stamp()).into_iter().collect(),
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if lines.is_empty() {
            continue;
        }
        if written > MAX_BYTES {
            rotate(&mut file);
            written = 0;
        }
        if file.is_none() {
            file = open(false);
        }
        if let Some(handle) = file.as_mut() {
            for text in &lines {
                if writeln!(handle, "{text}").is_ok() {
                    written += text.len() as u64 + 1;
                }
            }
            let _ = handle.flush();
        }
    }
}

macro_rules! logf {
    ($($arg:tt)*) => { crate::log::line(&format!($($arg)*)) };
}
pub(crate) use logf;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_lines_collapse_into_a_count() {
        let mut c = Collapser::default();
        assert_eq!(c.feed("t1", "a"), vec!["t1 a"]);
        assert!(c.feed("t2", "a").is_empty());
        assert!(c.feed("t3", "a").is_empty());
        assert_eq!(
            c.feed("t4", "b"),
            vec!["t4 … previous line repeated 2 more time(s)", "t4 b"]
        );
    }

    #[test]
    fn a_flush_reports_pending_repeats_once() {
        let mut c = Collapser::default();
        c.feed("t1", "a");
        c.feed("t2", "a");
        assert!(c.flush("t3").is_some());
        assert!(c.flush("t4").is_none());
        // The next identical line after a flush still counts as a repeat, not a new line.
        assert!(c.feed("t5", "a").is_empty());
    }
}

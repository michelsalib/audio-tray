//! One tray per session: a named mutex in the `Local\` namespace.
//!
//! A second launch waits briefly and then exits quietly. The wait matters for `tray::restart_app`
//! (and a quick Quit-then-start), where the new process starts while the old one is still tearing
//! down; it passes [`RELAUNCHED`] to get the longer budget.

use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
use windows_core::HSTRING;

const NAME: &str = r"Local\AudioTray.SingleInstance";

/// Argument `tray::restart_app` passes to its replacement.
pub const RELAUNCHED: &str = "--relaunched";

/// How long to wait for a running instance to go away before giving up.
pub fn wait_budget(args: &[String]) -> Duration {
    if args.iter().any(|arg| arg == RELAUNCHED) {
        Duration::from_secs(15)
    } else {
        Duration::from_secs(3)
    }
}

/// Ownership of the instance mutex, released on drop. Must be dropped on the thread that took it.
pub struct Instance(HANDLE);

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

/// Become the session's tray, waiting up to `wait` for another one to exit. `None`: one is running.
pub fn acquire(wait: Duration) -> Option<Instance> {
    acquire_named(NAME, wait)
}

fn acquire_named(name: &str, wait: Duration) -> Option<Instance> {
    let handle = unsafe { CreateMutexW(None, false, &HSTRING::from(name)) }.ok()?;
    let waited = unsafe { WaitForSingleObject(handle, wait.as_millis().min(u32::MAX as u128) as u32) };
    // Abandoned = the previous owner died holding it, which leaves it ours.
    if waited == WAIT_OBJECT_0 || waited == WAIT_ABANDONED {
        return Some(Instance(handle));
    }
    let _ = unsafe { CloseHandle(handle) };
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn relaunch_waits_longer_than_a_second_launch() {
        assert!(wait_budget(&[RELAUNCHED.to_string()]) > wait_budget(&[]));
    }

    #[test]
    fn a_second_instance_is_refused_until_the_first_exits() {
        let name = format!(r"Local\AudioTray.Test.{}", std::process::id());
        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let first_name = name.clone();
        // Mutex ownership is per thread, so the first instance lives on a thread of its own.
        let first = std::thread::spawn(move || {
            let guard = acquire_named(&first_name, Duration::ZERO).expect("first acquires");
            held_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(guard);
        });
        held_rx.recv().unwrap();
        assert!(acquire_named(&name, Duration::from_millis(50)).is_none());
        release_tx.send(()).unwrap();
        // The waiting side picks it up as soon as the first lets go.
        assert!(acquire_named(&name, Duration::from_secs(5)).is_some());
        first.join().unwrap();
    }
}

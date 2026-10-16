//! Hang watch for the tray thread. Each step that calls into XAML runs inside a [`Phase`]; a
//! background thread logs when one has been running for longer than [`STUCK_AFTER`], and again
//! when it returns. Nothing is logged for phases that finish in time, so the cost is a mutex per
//! phase and the log stays quiet — but after an AppHang the last lines name what the tray thread
//! was doing.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::log::logf;

/// Long enough that a busy-but-healthy sweep never trips it (worst seen: tens of ms).
const STUCK_AFTER: Duration = Duration::from_secs(2);

const POLL: Duration = Duration::from_millis(500);

struct Current {
    name: &'static str,
    since: Instant,
    /// Bumped per phase entry, so the watcher can tell one long phase from two in a row.
    serial: u64,
}

static CURRENT: Mutex<Option<Current>> = Mutex::new(None);

/// A running phase; restores the enclosing phase (if any) when dropped.
pub struct Phase {
    outer: Option<(&'static str, Instant)>,
}

/// Enter a named phase for the lifetime of the returned guard.
pub fn enter(name: &'static str) -> Phase {
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut current = crate::lock(&CURRENT);
    let outer = current.as_ref().map(|c| (c.name, c.since));
    *current = Some(Current {
        name,
        since: Instant::now(),
        serial,
    });
    Phase { outer }
}

impl Drop for Phase {
    fn drop(&mut self) {
        let mut current = crate::lock(&CURRENT);
        *current = self.outer.map(|(name, since)| Current {
            name,
            since,
            serial: u64::MAX,
        });
    }
}

/// Starts the watcher thread once (the DLL is pinned, so it never outlives its code).
pub fn start() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("xaml-tap hang watch".into())
        .spawn(|| {
            // The phase reported as stuck: (serial, name, since).
            let mut stuck: Option<(u64, &'static str, Instant)> = None;
            loop {
                std::thread::sleep(POLL);
                let now = {
                    let current = crate::lock(&CURRENT);
                    current.as_ref().map(|c| (c.serial, c.name, c.since))
                };
                if let Some((serial, name, since)) = stuck {
                    if now.is_none_or(|(s, _, _)| s != serial) {
                        logf!("hang watch: {name} returned after {:.1}s", since.elapsed().as_secs_f32());
                        stuck = None;
                    }
                }
                if let Some((serial, name, since)) = now {
                    if stuck.is_none() && since.elapsed() >= STUCK_AFTER {
                        logf!("hang watch: tray thread inside {name} for {:.1}s", since.elapsed().as_secs_f32());
                        stuck = Some((serial, name, since));
                    }
                }
            }
        });
}

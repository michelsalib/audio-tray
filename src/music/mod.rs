//! YouTube Music in the taskbar: the feed, and the tile it draws into.
//!
//! It shares audio-tray's TAP because XAML Diagnostics takes one consumer per endpoint. All SMTC
//! work runs on its own MTA thread ([`spawn`]); the tray thread holds only a channel. Nothing here
//! draws the strip: [`publish`] hands state to the TAP as a file, while [`progress`] and
//! [`thumbbar`] drive the shell's own progress bar and thumbnail toolbar on the player's window.

pub mod feed;
pub mod player;
pub mod progress;
pub mod publish;
pub mod session;
pub mod smtc;
pub mod thumbbar;

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use anyhow::{Context, Result};

pub use feed::Ytm;
#[cfg(feature = "dev")]
use feed::State;

/// Run `body` on a fresh MTA thread and wait for it. SMTC calls block on `IAsyncOperation`, which
/// deadlocks on the main STA (FINDINGS.md, 'The app side: an MTA thread').
#[cfg(feature = "dev")]
fn on_mta_thread<T, F>(what: &'static str, body: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    std::thread::Builder::new()
        .name(what.to_string())
        .spawn(move || {
            enter_mta();
            body()
        })
        .with_context(|| format!("spawn the {what} thread"))?
        .join()
        .map_err(|_| anyhow::anyhow!("the {what} thread panicked"))?
}

fn enter_mta() {
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    // Only called on threads created for this, so no other apartment can already be set.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
}

/// What the tray thread can ask the music thread to do.
enum Request {
    Command(smtc::Command),
    /// Explorer restarted (tray saw `WM_TASKBAR_RESTARTED`): shell-side state is gone.
    TaskbarRestarted,
    /// Clear the published state and toolbar, then stop. Sent by [`Handle::drop`].
    ShutDown,
}

/// The tray thread's end of the music feature: just a channel, no WinRT, so safe in an STA.
pub struct Handle {
    requests: Sender<Request>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Disconnects when the thread has finished (its sender is dropped on the way out, panic included).
    finished: Receiver<()>,
}

impl Handle {
    /// A transport command from the strip. Best-effort if the music thread is dead.
    pub fn command(&self, command: smtc::Command) {
        let _ = self.requests.send(Request::Command(command));
    }

    /// Tell the feed Explorer restarted, so it re-asserts the progress bar and toolbar (their caches
    /// cannot see a restart: the player's window is unchanged).
    pub fn taskbar_restarted(&self) {
        let _ = self.requests.send(Request::TaskbarRestarted);
    }
}
// No `activate` on purpose: body clicks belong to the shell (activate/minimise, drag-to-reorder);
// only the no-session fallback in [`Music::command`] raises the player.

/// How long Quit waits for the feed thread to tidy up.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

impl Drop for Handle {
    /// Guarantees the teardown (state file, toolbar). Bounded: a thread stuck in a WinRT `.get()`
    /// is detached after [`SHUTDOWN_WAIT`] and the state file is removed from here instead.
    fn drop(&mut self) {
        let _ = self.requests.send(Request::ShutDown);
        match self.finished.recv_timeout(SHUTDOWN_WAIT) {
            Err(RecvTimeoutError::Timeout) => {
                eprintln!("music: the feed did not stop within {SHUTDOWN_WAIT:?}; leaving it behind");
                publish::remove_published();
            }
            _ => {
                if let Some(thread) = self.thread.take() {
                    let _ = thread.join();
                }
            }
        }
    }
}

/// Start following YouTube Music on its own MTA thread. `None` when switched off or the thread
/// cannot start; the audio half must come up either way.
pub fn spawn(settings: &crate::config::Music) -> Option<Handle> {
    if !settings.enabled {
        return None;
    }
    let pinned = settings.app_id.clone();
    let (requests, inbox) = std::sync::mpsc::channel();
    let (finished_tx, finished) = std::sync::mpsc::channel::<()>();
    let thread = std::thread::Builder::new()
        .name("music".to_string())
        .spawn(move || {
            let _finished = finished_tx;
            enter_mta();
            match Music::new(pinned) {
                Ok(mut music) => music.serve(inbox),
                Err(e) => eprintln!("music: unavailable ({e:#})"),
            }
        });
    match thread {
        Ok(thread) => Some(Handle {
            requests,
            thread: Some(thread),
            finished,
        }),
        Err(e) => {
            eprintln!("music: could not start the feed thread ({e:#})");
            None
        }
    }
}

/// The feed and its outputs, which a poll always updates together.
pub struct Music {
    feed: Ytm,
    publisher: publish::Publisher,
    progress: progress::Progress,
    /// The transport buttons on the shell's thumbnail toolbar under the player's preview.
    toolbar: thumbbar::Toolbar,
}

impl Music {
    /// Open the session feed. Fails only if SMTC itself is unavailable.
    pub fn new(pinned: Option<String>) -> Result<Self> {
        Ok(Self {
            feed: Ytm::new(pinned).context("opening the SMTC session manager")?,
            publisher: publish::Publisher::new(),
            progress: progress::Progress::new(),
            toolbar: thumbbar::Toolbar::new(),
        })
    }

    /// One poll: read the session, publish it, move the bar and toolbar. Errors are logged and the
    /// last good state kept (a session dying mid-enumeration is routine).
    pub fn poll(&mut self) {
        let (state, timeline) = match self.feed.read() {
            Ok(read) => read,
            Err(err) => {
                eprintln!("music: could not read the session: {err:#}");
                return;
            }
        };
        // Remember the player while visible, so a click after it closes launches the app.
        if let Some(app_id) = self.feed.current_app_id() {
            player::remember_player(app_id);
        }
        if let Err(err) = self.publisher.publish(&state) {
            eprintln!("music: could not publish the strip state: {err:#}");
        }

        let playing = state
            .snapshot()
            .is_some_and(|snapshot| snapshot.status.is_playing());
        self.progress.update(timeline, playing);
        self.toolbar.update(playing);
    }

    /// Send a transport command and republish immediately. With no session the player is raised
    /// instead; never synthesise a media key (it reaches whichever player owns them).
    pub fn command(&mut self, command: smtc::Command) {
        match self.feed.send(command) {
            Ok(true) => {}
            Ok(false) => match player::activate_player(self.feed.current_app_id()) {
                Ok(what) => println!("music: no session yet; {what:?}"),
                Err(err) => eprintln!("music: no session, and could not raise the player: {err:#}"),
            },
            Err(err) => eprintln!("music: {command:?} failed: {err:#}"),
        }
        self.poll();
    }

    /// Remove the state file and toolbar before exiting. The progress bar is not cleared here: the
    /// tray loop has already exited, so `tray::run` clears it directly.
    pub fn shut_down(&mut self) {
        self.publisher.clear();
        self.toolbar.clear();
    }

    /// The thread body: poll every second (SMTC raises no position events) and act on requests the
    /// moment they arrive.
    fn serve(&mut self, inbox: Receiver<Request>) {
        const POLL: Duration = Duration::from_secs(1);
        self.poll();
        loop {
            match inbox.recv_timeout(POLL) {
                Ok(Request::Command(command)) => self.command(command),
                Ok(Request::TaskbarRestarted) => {
                    self.progress.taskbar_restarted();
                    self.toolbar.taskbar_restarted();
                    // Now, alongside the strip's redraw, rather than flicker in a second later.
                    self.poll();
                }
                Ok(Request::ShutDown) => {
                    self.shut_down();
                    return;
                }
                Err(RecvTimeoutError::Timeout) => self.poll(),
                // Handle dropped without a shutdown: tear down anyway.
                Err(RecvTimeoutError::Disconnected) => {
                    self.shut_down();
                    return;
                }
            }
        }
    }
}

/// List every SMTC session with its YouTube Music verdict, and offer the `app_id` line to pin when
/// only a bare browser session is on offer.
#[cfg(feature = "dev")]
pub fn probe() -> Result<()> {
    on_mta_thread("music-probe", || {
        let mut feed = Ytm::new(None)?;
        report_sessions(&mut feed)
    })
}

#[cfg(feature = "dev")]
fn report_sessions(feed: &mut Ytm) -> Result<()> {
    let sessions = feed.all_sessions()?;
    if sessions.is_empty() {
        println!("no SMTC sessions at all — nothing on this machine is playing media.");
        println!("start YouTube Music, play a track, and run this again.");
        return Ok(());
    }

    println!("{} SMTC session(s):\n", sessions.len());
    for snapshot in &sessions {
        println!("  app id   : {}", snapshot.app_id);
        println!("  verdict  : {:?}", session::classify(&snapshot.app_id));
        println!("  title    : {}", show(&snapshot.title));
        println!("  artist   : {}", show(&snapshot.artist));
        println!("  status   : {:?}", snapshot.status);
        match &snapshot.cover {
            Some(bytes) => println!("  cover    : {} bytes", bytes.len()),
            None => println!("  cover    : <none published>"),
        }
        println!();
    }

    println!("--- what the strip would follow ---");
    match feed.read()?.0 {
        State::Track(snapshot) => println!("  {} — {}", snapshot.title, snapshot.artist),
        State::Absent => {
            println!("  nothing");
            for snapshot in &sessions {
                if session::classify(&snapshot.app_id) == session::Match::Browser {
                    println!(
                        "\n  {} is a bare browser id — it is whatever that browser is playing, so\n  \
                         it is not followed on its own. If YouTube Music runs as a plain tab there,\n  \
                         put this in config.toml to follow it anyway:\n\n      \
                         [music]\n      app_id = \"{}\"",
                        snapshot.app_id, snapshot.app_id
                    );
                    break;
                }
            }
        }
    }
    Ok(())
}

/// Re-ask "is this the player's window?" from an MTA for the given handles, to check it agrees
/// with the STA answer (toolbar and progress bar read the window identity in different apartments).
#[cfg(feature = "dev")]
pub fn player_verdicts_from_mta(windows: Vec<isize>) -> Result<Vec<bool>> {
    on_mta_thread("music-windows", move || {
        Ok(windows
            .into_iter()
            .map(|hwnd| {
                player::is_player_window(windows::Win32::Foundation::HWND(
                    hwnd as *mut core::ffi::c_void,
                ))
            })
            .collect())
    })
}

/// Trace what the feed reads every 100 ms for `seconds`, optionally sending `skip` two seconds in.
///
/// Shows the position checkpoint and its age, and the ~1 s session gap around a track change.
#[cfg(feature = "dev")]
pub fn report_timeline(seconds: u64, skip: Option<smtc::Command>) -> Result<()> {
    on_mta_thread("music-timeline", move || {
        let mut feed = Ytm::new(None)?;
        report_position(&mut feed, seconds, skip)
    })
}

#[cfg(feature = "dev")]
fn report_position(feed: &mut Ytm, seconds: u64, skip: Option<smtc::Command>) -> Result<()> {
    let started = std::time::Instant::now();
    let mut skipped = false;
    while started.elapsed().as_secs() < seconds {
        let at = started.elapsed().as_secs_f64();
        if !skipped && at >= 2.0_f64.min(seconds as f64 / 2.0) {
            if let Some(command) = skip {
                println!("{at:6.2}s  >>> {command:?}: {:?}", feed.send(command));
            }
            skipped = true;
        }
        let (state, timeline) = feed.read()?;
        let (status, title) = match state.snapshot() {
            Some(s) => (format!("{:?}", s.status), s.title.clone()),
            None => ("-".into(), "<absent>".into()),
        };
        let playing = state.snapshot().is_some_and(|s| s.status.is_playing());
        let line = match timeline {
            Some(t) => format!(
                "pos {:6.1}s / {:>7}  updated {:+7.2}s ago  -> {}",
                t.position_seconds(),
                t.duration_seconds().map(|d| format!("{d:.1}s")).unwrap_or_else(|| "?".into()),
                (smtc::now_ticks() - t.last_updated) as f64 / 1e7,
                t.fraction_at(smtc::now_ticks(), playing).map(|f| format!("{:5.1}%", f * 100.0)).unwrap_or_else(|| "none".into()),
            ),
            None => "no timeline".into(),
        };
        println!("{at:6.2}s  {:<8} {:<10} {line}  {title}", feed.current_app_id().map_or("-", |_| "app"), status);
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}

#[cfg(feature = "dev")]
fn show(value: &str) -> String {
    if value.trim().is_empty() {
        "<empty>".to_string()
    } else {
        value.to_string()
    }
}

//! YouTube Music in the taskbar: the feed, and the tile it draws into.
//!
//! **Why this lives in audio-tray at all.** Both features need the same scarce resource — XAML
//! Diagnostics takes *one* consumer per endpoint, so a separate app drawing into the taskbar cannot
//! run alongside this one. Sharing the TAP is not a convenience, it is the only arrangement in which
//! both can exist on the same machine. It was developed as its own project (media-tray) precisely to
//! avoid disturbing this one until it worked, and this is the merge.
//!
//! The split inside:
//!
//! ```text
//! feed      which SMTC session is YouTube Music, and what it is playing
//! smtc      the thin skin over Windows.Media.Control
//! session   the app-id matching that decides "this is YouTube Music"
//! publish   hands the state to the TAP, as a file it re-reads
//! player    the player's window: raising it, activating it, its progress bar
//! progress  the position, drawn as the shell's own taskbar progress bar
//! ```
//!
//! Nothing here draws the strip; that is the TAP's half. The one thing to know about the seam is
//! that it is a **file**, not a message: the cover art has to reach XAML as an image *source*, and
//! the only way to hand XAML a bitmap it did not create is a path — so a file is in play regardless,
//! and one mechanism beats two.

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

pub use feed::{State, Ytm};

/// **This feature cannot run on audio-tray's own thread, and that is not a style choice.**
///
/// Every SMTC call here blocks on the `IAsyncOperation` it returns, and audio-tray's main thread is
/// an **STA** because it owns windows — the tray icon, the flyout, the readout. Blocking on an
/// apartment-threaded call without pumping messages deadlocks, and it does: measured, the very first
/// `--music-probe` hung before printing a line, with no output and no error. media-tray never met
/// this because it had no UI and could take an MTA for the whole process.
///
/// So the feed lives on a thread of its own that initialises **MTA**, paces its own poll, and takes
/// requests by channel. The tray thread never touches WinRT media APIs at all, which also means a
/// slow or wedged session can never stall the audio half.
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
    // Already-initialised is not a failure; a *different* apartment on this thread would be, and
    // cannot happen — the thread is created here and does nothing else.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
}

/// What the tray thread can ask the music thread to do.
enum Request {
    Command(smtc::Command),
    /// Publish and put the progress bar back, then stop. Sent by [`Handle::drop`].
    ShutDown,
}

/// The tray thread's end of the music feature.
///
/// Holds no WinRT at all — just a channel — so it is safe to keep in an STA and cheap to poke from a
/// click handler.
pub struct Handle {
    requests: Sender<Request>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Handle {
    /// A transport command from the strip. Best-effort: a dead music thread means the feature is
    /// gone, which is not worth taking the tray down for.
    pub fn command(&self, command: smtc::Command) {
        let _ = self.requests.send(Request::Command(command));
    }

}
// There is deliberately no `activate` here. The strip *body* is left to the shell — clicking an app's
// own taskbar button already means "bring it forward or minimise it", and its press is the
// drag-to-reorder gesture — so the only place that raises the player is the cold-start fallback in
// [`Music::command`], where there is no session for a transport click to address.

impl Drop for Handle {
    /// **The teardown has to happen, and this is the only place that can guarantee it.** The state
    /// file and — more importantly — a progress bar on *another app's* taskbar button both outlive
    /// this process, so an exit that skips them leaves a strip with nothing driving it and a bar
    /// frozen mid-track that the user cannot attribute to anything.
    fn drop(&mut self) {
        let _ = self.requests.send(Request::ShutDown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Start following YouTube Music on a thread of its own.
///
/// Returns `None` when the feature is switched off. An SMTC that will not open is reported and also
/// yields `None`: the audio half must come up either way.
pub fn spawn(settings: &crate::config::Music) -> Option<Handle> {
    if !settings.enabled {
        return None;
    }
    let pinned = settings.app_id.clone();
    let (requests, inbox) = std::sync::mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("music".to_string())
        .spawn(move || {
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
        }),
        Err(e) => {
            eprintln!("music: could not start the feed thread ({e:#})");
            None
        }
    }
}

/// Everything the taskbar half of this feature needs, held together.
///
/// One struct rather than three locals in the message loop, because the three have to move
/// together: a poll reads the feed, writes the state file, and updates the progress bar, and any of
/// those happening without the others shows the user a strip that disagrees with itself.
pub struct Music {
    feed: Ytm,
    publisher: publish::Publisher,
    progress: progress::Progress,
    /// The transport buttons under the player's hover preview — the shell's own thumbnail toolbar.
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

    /// One poll: read the session, publish it for the TAP, and move the progress bar.
    ///
    /// Errors are reported and swallowed rather than returned. A session dying mid-enumeration is
    /// routine, and the right response to it is to keep showing the last good state — not to take
    /// the strip, or the audio half of the app, down with it.
    pub fn poll(&mut self) {
        let state = match self.feed.read() {
            Ok(state) => state,
            Err(err) => {
                eprintln!("music: could not read the session: {err:#}");
                return;
            }
        };
        // Remember who the player is while we can see it: the session disappears when YouTube Music
        // closes, and this is what lets a later click still launch the app rather than the website.
        if let Some(app_id) = self.feed.current_app_id() {
            player::remember_player(app_id);
        }
        if let Err(err) = self.publisher.publish(&state) {
            eprintln!("music: could not publish the strip state: {err:#}");
        }

        let timeline = match self.feed.current_app_id().map(str::to_string) {
            Some(app_id) => self.feed.timeline(&app_id).unwrap_or(None),
            None => None,
        };
        let playing = state
            .snapshot()
            .is_some_and(|snapshot| snapshot.status.is_playing());
        if let Err(err) = self.progress.update(timeline, playing) {
            eprintln!("music: could not set the progress bar: {err:#}");
        }
        // The transport buttons under the hover preview. Driven from the same poll as the bar
        // because they carry the same one bit of state — whether it is playing, which decides the
        // play/pause glyph — and an update that costs nothing when it has not changed.
        self.toolbar.update(playing);
    }

    /// Send a transport command, and republish immediately.
    ///
    /// Republished rather than waiting for the next poll: a play/pause that takes a second to change
    /// the glyph reads as a control that did not work.
    ///
    /// With no session to address — YouTube Music open but never played, or closed — the player is
    /// brought forward instead. **Not a synthesised media key**: the key is global, so with no
    /// session of our own it reaches YouTube Music only by winning a race against every other
    /// player, and it was measured pausing MPC-HC instead.
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

    /// Hand back everything this feature put somewhere else, before exiting.
    ///
    /// The state file goes so a strip left on screen has nothing to show, and the progress bar goes
    /// because it lives on **another app's** window and would otherwise sit there frozen mid-track
    /// with nobody left to attribute it to.
    pub fn shut_down(&mut self) {
        self.publisher.clear();
        self.progress.clear();
        self.toolbar.clear();
    }

    /// The thread body: poll on a timer of our own, and act on what the tray sends.
    ///
    /// `recv_timeout` rather than a sleep plus a `try_recv`, so a click is acted on the moment it
    /// arrives instead of waiting out the rest of the poll interval — a play/pause that takes up to a
    /// second to respond reads as a control that did not work.
    ///
    /// One second between polls is well inside "feels live" for a track change, and it is the
    /// position — which SMTC does *not* raise events for — that makes polling unavoidable at all.
    fn serve(&mut self, inbox: Receiver<Request>) {
        const POLL: Duration = Duration::from_secs(1);
        self.poll();
        loop {
            match inbox.recv_timeout(POLL) {
                Ok(Request::Command(command)) => self.command(command),
                Ok(Request::ShutDown) => {
                    self.shut_down();
                    return;
                }
                Err(RecvTimeoutError::Timeout) => self.poll(),
                // The tray dropped its handle without a shutdown — it is going away, so do the
                // teardown anyway rather than leaving state on other people's windows.
                Err(RecvTimeoutError::Disconnected) => {
                    self.shut_down();
                    return;
                }
            }
        }
    }
}

/// List every SMTC session on the machine, with the YouTube Music verdict on each.
///
/// The one thing the built-in matching cannot be sure of is the exact app id of *this* machine's
/// YouTube Music: it is a Chromium implementation detail, and an installed PWA reports something
/// quite different from a browser tab. Run this with the player going and the id to pin is the one
/// marked `Certain`.
pub fn probe() -> Result<()> {
    on_mta_thread("music-probe", || {
        let mut feed = Ytm::new(None)?;
        report_sessions(&mut feed)
    })
}

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
    match feed.read()? {
        State::Track(snapshot) => println!("  {} — {}", snapshot.title, snapshot.artist),
        State::Absent => println!("  nothing"),
    }
    Ok(())
}

/// Sample the followed session's position three times, three seconds apart.
///
/// **The measurement this exists for:** a player publishes its position as a *checkpoint with a
/// timestamp*, not as a running clock — measured, it moved 1.2 s over 6 s of playback — so a progress
/// bar has to interpolate from `last updated` while the status is playing. If a future Chromium stops
/// publishing a timeline at all, this is what says so.
pub fn report_timeline() -> Result<()> {
    on_mta_thread("music-timeline", || {
        let mut feed = Ytm::new(None)?;
        report_position(&mut feed)
    })
}

fn report_position(feed: &mut Ytm) -> Result<()> {
    let state = feed.read()?;
    let Some(app_id) = feed.current_app_id().map(str::to_string) else {
        println!("no YouTube Music session to ask");
        return Ok(());
    };
    println!(
        "following {app_id} — {}",
        if state.snapshot().is_some_and(|s| s.status.is_playing()) {
            "playing"
        } else {
            "not playing"
        }
    );
    for sample in 0..3 {
        if sample > 0 {
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
        match feed.timeline(&app_id)? {
            Some(timeline) => println!(
                "t+{}s  position {:.1}s / {}  published {}  last updated {}",
                sample * 3,
                timeline.position_seconds(),
                timeline
                    .duration_seconds()
                    .map(|d| format!("{d:.1}s"))
                    .unwrap_or_else(|| "unknown".into()),
                timeline.is_published(),
                timeline.last_updated,
            ),
            None => println!("t+{}s  the session went away", sample * 3),
        }
    }
    Ok(())
}

fn show(value: &str) -> String {
    if value.trim().is_empty() {
        "<empty>".to_string()
    } else {
        value.to_string()
    }
}

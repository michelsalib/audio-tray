//! YouTube Music in the taskbar: the feed, and the tile it draws into.
//!
//! It shares audio-tray's TAP because XAML Diagnostics takes one consumer per endpoint. All SMTC
//! work runs on its own MTA thread ([`spawn`]), woken only by SMTC events, requests, and the
//! progress bar's next step; the tray thread holds only a channel. Nothing here
//! draws the strip: [`publish`] hands state to the TAP as a file, while [`progress`] and
//! [`thumbbar`] drive the shell's own progress bar and thumbnail toolbar on the player's window.

pub mod feed;
pub mod player;
pub mod progress;
pub mod publish;
pub mod session;
pub mod smtc;
pub mod thumbbar;

use std::ops::ControlFlow;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
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

/// What the tray thread, or an SMTC event, can ask the music thread to do.
enum Request {
    /// Reported by an SMTC event handler.
    Changed(smtc::Change),
    Command(smtc::Command),
    /// A transport command for the session shown anywhere (the pill), whatever the app.
    CommandShown(smtc::Command),
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

    /// A transport command for the session the pill shows (any app).
    pub fn command_shown(&self, command: smtc::Command) {
        let _ = self.requests.send(Request::CommandShown(command));
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
pub fn spawn(settings: &crate::config::Music, taskbar: bool) -> Option<Handle> {
    if !settings.enabled {
        return None;
    }
    let pinned = settings.app_id.clone();
    let (requests, inbox) = std::sync::mpsc::channel();
    let (finished_tx, finished) = std::sync::mpsc::channel::<()>();
    let events = requests.clone();
    let thread = std::thread::Builder::new()
        .name("music".to_string())
        .spawn(move || {
            let _finished = finished_tx;
            enter_mta();
            let notify: smtc::Notify = Arc::new(move |change| {
                let _ = events.send(Request::Changed(change));
            });
            match Music::new(pinned, notify, taskbar) {
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

/// The feed and its outputs, which a reading always updates together.
pub struct Music {
    feed: Ytm,
    publisher: publish::Publisher,
    progress: progress::Progress,
    /// The transport buttons on the shell's thumbnail toolbar under the player's preview.
    toolbar: thumbbar::Toolbar,
    /// The strip is on: publish its state file and drive the taskbar progress bar.
    taskbar: bool,
    /// An SMTC change arrived since the last reading.
    dirty: bool,
    /// The media last reported to the hub (`Some(None)`: reported as nothing).
    reported: Option<Option<crate::hub::Media>>,
}

impl Music {
    /// Open the session feed and report its changes to `notify`. Fails only if SMTC itself is
    /// unavailable.
    pub fn new(pinned: Option<String>, notify: smtc::Notify, taskbar: bool) -> Result<Self> {
        let mut feed = Ytm::new(pinned).context("opening the SMTC session manager")?;
        feed.watch(notify).context("subscribing to SMTC events")?;
        Ok(Self {
            feed,
            publisher: publish::Publisher::new(),
            taskbar,
            progress: progress::Progress::new(),
            toolbar: thumbbar::Toolbar::new(),
            dirty: false,
            reported: None,
        })
    }

    /// Read the session, publish it, move the bar and toolbar. Errors are logged and the last good
    /// state kept (a session dying mid-enumeration is routine).
    pub fn poll(&mut self) {
        self.dirty = false;
        let (state, timeline, any) = match self.feed.read_with_any() {
            Ok(read) => read,
            Err(err) => {
                eprintln!("music: could not read the session: {err:#}");
                return;
            }
        };
        self.report(any);
        // Remember the player while visible, so a click after it closes launches the app.
        if let Some(app_id) = self.feed.current_app_id() {
            player::remember_player(app_id);
        }
        // The state file and the taskbar progress bar are the strip's; without it they cost wakes
        // for nothing.
        if self.taskbar {
            if let Err(err) = self.publisher.publish(&state) {
                eprintln!("music: could not publish the strip state: {err:#}");
            }
        }

        let playing = state
            .snapshot()
            .is_some_and(|snapshot| snapshot.status.is_playing());
        if self.taskbar {
            self.progress.update(timeline, playing);
        }
        self.toolbar.update(playing);
    }

    /// Tell the hub about the session to show anywhere, when it changed. A session with no title
    /// yet (a track change in flight) is held back rather than shown empty.
    fn report(&mut self, any: Option<smtc::Reading>) {
        let media = any.filter(|r| r.snapshot.has_track()).map(|r| {
            let cover = match (&self.reported, &r.snapshot.cover) {
                // Same bytes: share the old allocation rather than make another.
                (Some(Some(old)), Some(bytes)) if old.cover.as_deref().map(|c| c.as_slice()) == Some(bytes.as_slice()) => old.cover.clone(),
                (_, cover) => cover.clone().map(Arc::new),
            };
            crate::hub::Media {
                app: crate::hub::names::app_name(&r.snapshot.app_id),
                app_id: r.snapshot.app_id,
                title: r.snapshot.title,
                artist: r.snapshot.artist,
                playing: r.snapshot.status.is_playing(),
                cover,
                timeline: r.timeline,
            }
        });
        if self.reported.as_ref() != Some(&media) {
            crate::hub::emit(crate::hub::Event::Media(media.clone()));
            self.reported = Some(media);
        }
    }

    /// A transport command for the session shown anywhere (the pill), whatever the app.
    fn command_shown(&mut self, command: smtc::Command) {
        if let Err(err) = self.feed.send_to_shown(command) {
            eprintln!("music: {command:?} on the shown session failed: {err:#}");
        }
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

    /// The thread body: sleep until a request, an SMTC change, or the progress bar's next step.
    /// Nothing wakes it while no track plays.
    fn serve(&mut self, inbox: Receiver<Request>) {
        self.poll();
        loop {
            let request = match self.progress.due() {
                Some(wait) => match inbox.recv_timeout(wait) {
                    Ok(request) => request,
                    Err(RecvTimeoutError::Timeout) => {
                        self.progress.tick();
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match inbox.recv() {
                    Ok(request) => request,
                    Err(_) => break,
                },
            };
            if self.handle(request).is_break() {
                return;
            }
            // Events come in bursts (a track change raises several): read once for all of them.
            while let Ok(request) = inbox.try_recv() {
                if self.handle(request).is_break() {
                    return;
                }
            }
            if self.dirty {
                self.poll();
            }
        }
        // Every sender gone without a shutdown: tear down anyway.
        self.shut_down();
    }

    /// Act on one request. A change is only noted; [`Music::serve`] reads once the inbox is empty.
    fn handle(&mut self, request: Request) -> ControlFlow<()> {
        match request {
            Request::Changed(change) => {
                if let Err(err) = self.feed.changed(change) {
                    eprintln!("music: could not follow {change:?}: {err:#}");
                }
                self.dirty = true;
            }
            Request::Command(command) => self.command(command),
            Request::CommandShown(command) => self.command_shown(command),
            Request::TaskbarRestarted => {
                self.progress.taskbar_restarted();
                self.toolbar.taskbar_restarted();
                // Now, alongside the strip's redraw, rather than on the next change.
                self.poll();
            }
            Request::ShutDown => {
                self.shut_down();
                return ControlFlow::Break(());
            }
        }
        ControlFlow::Continue(())
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

/// Watch SMTC as the feed does for `seconds`, printing each event (with its delay from the
/// previous one) and the reading a burst of them leads to. Publishes nothing, so it can run beside
/// the installed tray; its thread should not wake between events.
#[cfg(feature = "dev")]
pub fn report_events(seconds: u64) -> Result<()> {
    on_mta_thread("music-events", move || {
        let (events, inbox) = std::sync::mpsc::channel();
        let mut feed = Ytm::new(None)?;
        feed.watch(Arc::new(move |change| {
            let _ = events.send((std::time::Instant::now(), change));
        }))?;
        let started = std::time::Instant::now();
        let deadline = started + Duration::from_secs(seconds);
        println!("watching SMTC for {seconds} s");
        print_reading(&mut feed, 0.0)?;
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            let Ok((at, change)) = inbox.recv_timeout(left) else {
                break;
            };
            let mut burst = vec![(at, change)];
            while let Ok(event) = inbox.try_recv() {
                burst.push(event);
            }
            for (at, change) in &burst {
                println!("{:8.3}s  event {change:?}", (*at - started).as_secs_f64());
                feed.changed(*change)?;
            }
            print_reading(&mut feed, (burst[0].0 - started).as_secs_f64())?;
        }
        Ok(())
    })
}

#[cfg(feature = "dev")]
fn print_reading(feed: &mut Ytm, since: f64) -> Result<()> {
    let read_at = std::time::Instant::now();
    let (state, timeline) = feed.read()?;
    let took = read_at.elapsed().as_secs_f64() * 1000.0;
    let what = match state.snapshot() {
        Some(s) => format!("{:?} {} — {} (cover: {})", s.status, s.title, s.artist, s.cover.as_ref().map_or(0, Vec::len)),
        None => "<absent>".into(),
    };
    let length = timeline
        .and_then(|t| t.duration_seconds())
        .map_or_else(|| "no length".into(), |d| format!("{d:.1}s long"));
    println!("{:>10} read in {took:.0} ms: {what}, {length} (burst began at {since:.3}s)", "->");
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

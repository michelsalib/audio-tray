//! The track position, drawn as the shell's own taskbar progress bar.
//!
//! **Nothing here draws anything, and nothing here calls COM.** The bar is the one Windows already
//! puts under a taskbar icon when an app reports progress — the line MPC-HC shows while a file plays
//! — and this module only decides what fraction to ask for and hands that to the tray thread.
//!
//! Two things had to be measured before this could exist, both recorded in FINDINGS.md:
//!
//! * **`ITaskbarList3` accepts progress for another process's window.** It is normally an app
//!   reporting its own, and nothing documents the cross-process case. It works.
//! * **The `ProgressIndicator` element has to be kept and sized.** The TAP used to collapse it,
//!   because at a widened button it stretches past the strip. `place_button_state` pins it to the
//!   plate instead — the full width, because the bar is about the track and the track is what the
//!   whole strip is showing.
//!
//! # Why the value is posted rather than applied here
//!
//! This module runs on the feed's **MTA** thread (see [`super::on_mta_thread`] for why that thread
//! has to be an MTA at all). `ITaskbarList3` is an apartment-threaded shell object, so creating it
//! from an MTA gets a proxy to a COM-spun host STA and every call is marshalled. It *worked* — the
//! bar has been watched moving for whole sessions — but it was never the arrangement that was
//! verified: `--music-progress`, the flag every measurement in FINDINGS was taken through, runs on
//! `main`'s STA. Shipping one apartment and testing another is the kind of difference that surfaces
//! as an intermittent failure on somebody else's machine.
//!
//! So the fraction is posted to the tray's message loop, which is a real STA that owns windows, and
//! the bar is set there. That also settles a second defect for free: the tray knows when the taskbar
//! controls have been reverted, so `--taskbar-revert` now stops the bar instead of leaving the feed
//! to put it straight back a second later.

use crate::music::smtc::{now_ticks, Timeline};

/// How finely the bar is stepped. 200 steps is half a percent — seven times finer than the 28 epx
/// it is drawn in, so the quantisation is invisible, and it cuts the posted messages to one every
/// few seconds on a normal track instead of one per poll.
const STEPS: f64 = 200.0;

/// Decides what the taskbar progress bar should show, and tells the tray thread.
pub struct Progress {
    /// The last thing actually posted, so an unchanged value costs nothing.
    last: Option<(u64, bool)>,
}

impl Progress {
    pub fn new() -> Self {
        Self { last: None }
    }

    /// Bring the bar in line with a timeline reading.
    ///
    /// `None` clears it, which is the right answer for "no session" and for "a session that
    /// publishes no timeline" alike: a bar stuck at some old fraction is worse than no bar.
    pub fn update(&mut self, timeline: Option<Timeline>, playing: bool) {
        let fraction = timeline.and_then(|timeline| timeline.fraction_at(now_ticks(), playing));
        let step = fraction.map(|fraction| (fraction * STEPS).round() as u64);
        let next = step.map(|step| (step, playing));
        if self.last == next {
            return;
        }
        // Logged on a change of *state*, not of value: a step is half a percent, so logging those
        // would be a line every second or two, while "playing at 71%" or "cleared" is the whole of
        // what one wants from the log when the bar looks wrong.
        if self.last.map(|(_, was)| was) != Some(playing) {
            match fraction {
                Some(fraction) => println!(
                    "progress bar -> {:.0}% ({})",
                    fraction * 100.0,
                    if playing { "playing" } else { "paused" }
                ),
                None => println!("progress bar -> cleared (no timeline)"),
            }
        }
        self.post(step, playing);
        self.last = next;
    }

    /// Explorer restarted: forget what was on screen, so the next poll posts it again.
    ///
    /// **The same defect the thumbnail toolbar had.** A progress bar is state the *shell* holds
    /// against a window, so a new Explorer starts with none — while `last` still says "already at
    /// 21 %, nothing to do" and suppresses every post from then on. The bar simply never comes back,
    /// with nothing logged, because from this side nothing changed.
    pub fn taskbar_restarted(&mut self) {
        self.last = None;
    }

    // There is no `clear` here any more. Taking the bar off is a *shutdown* action, and this side
    // cannot perform one: the tray thread has already left its message loop by then, so a posted
    // clear would sit in a queue nobody reads. `tray::run` does it directly — see the end of that
    // function, and `Music::shut_down`.

    /// Hand the value to the tray thread.
    ///
    /// Best-effort by design: a tray that has already gone is the shutdown path, where the bar is
    /// being cleared by [`crate::taskbar::clear_player_progress`] on the way out anyway.
    fn post(&self, step: Option<u64>, playing: bool) {
        if let Err(err) = crate::taskbar::post_progress(step.map(|step| step as f64 / STEPS), playing)
        {
            eprintln!("music: could not hand the progress bar to the tray: {err:#}");
        }
    }
}

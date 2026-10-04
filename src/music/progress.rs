//! The track position as the shell's taskbar progress bar on the player's window. Runs on the feed's
//! MTA and only decides the fraction; the tray's STA applies it (`ITaskbarList3` is apartment-bound).

use crate::music::smtc::{now_ticks, Timeline};

/// Bar quantisation: half a percent is invisible at 28 epx and cuts posts to one every few seconds.
const STEPS: f64 = 200.0;

/// Decides what the taskbar progress bar should show, and tells the tray thread.
pub struct Progress {
    /// The last value posted.
    last: Option<(u64, bool)>,
}

impl Progress {
    pub fn new() -> Self {
        Self { last: None }
    }

    /// Bring the bar in line with a timeline reading. Never clears: `TBPF_NOPROGRESS` rebuilds the
    /// shell's `ProgressIndicator`, which jumps (FINDINGS.md, 'The progress line jumped…').
    pub fn update(&mut self, timeline: Option<Timeline>, playing: bool) {
        // Gap between tracks: hold the last reading, colour included (`playing` reads false there).
        let Some(fraction) = timeline.and_then(|timeline| timeline.fraction_at(now_ticks(), playing)) else {
            return;
        };
        let step = (fraction * STEPS).round() as u64;
        let next = Some((step, playing));
        if self.last == next {
            return;
        }
        // Log play-state changes only, not every step.
        if self.last.map(|(_, was)| was) != Some(playing) {
            println!(
                "progress bar -> {:.0}% ({})",
                step as f64 / STEPS * 100.0,
                if playing { "playing" } else { "paused" }
            );
        }
        // Best-effort: a gone tray means shutdown, which clears the bar itself.
        if let Err(err) = crate::taskbar::post_progress(Some(step as f64 / STEPS), playing) {
            eprintln!("music: could not hand the progress bar to the tray: {err:#}");
        }
        self.last = next;
    }

    /// Explorer restarted: the new shell holds no bar, so forget `last` and repost on the next poll.
    pub fn taskbar_restarted(&mut self) {
        self.last = None;
    }

    // No `clear` on purpose: at shutdown the tray loop is gone, so `tray::run` clears it directly.
}

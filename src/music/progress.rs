//! The track position as the shell's taskbar progress bar on the player's window. Runs on the feed's
//! MTA and only decides the fraction; the tray's STA applies it (`ITaskbarList3` is apartment-bound).
//!
//! Between SMTC events the position is interpolated from the last timeline checkpoint, and the feed
//! wakes only when the bar is due to move a step ([`Progress::due`]).

use std::time::Duration;

use crate::music::smtc::{now_ticks, Timeline};

/// Bar quantisation: about one physical pixel of the ~150 epx bar on the widened tile. Each step is
/// a wake of this thread and a post to the tray.
const STEPS: f64 = 200.0;

/// Decides what the taskbar progress bar should show, and tells the tray thread.
pub struct Progress {
    /// The last value posted.
    last: Option<(u64, bool)>,
    /// The last reading, and whether it was playing; `None` between tracks.
    reading: Option<(Timeline, bool)>,
}

impl Progress {
    pub fn new() -> Self {
        Self { last: None, reading: None }
    }

    /// Take a new timeline reading and bring the bar in line with it.
    pub fn update(&mut self, timeline: Option<Timeline>, playing: bool) {
        self.reading = timeline.map(|timeline| (timeline, playing));
        self.tick();
    }

    /// Bring the bar in line with the last reading, interpolated to now. Never clears:
    /// `TBPF_NOPROGRESS` rebuilds the shell's `ProgressIndicator`, which jumps (FINDINGS.md, 'The
    /// progress line jumped…').
    pub fn tick(&mut self) {
        // Gap between tracks: hold the last post, colour included (`playing` reads false there).
        let Some((timeline, playing)) = self.reading else {
            return;
        };
        let Some(fraction) = timeline.fraction_at(now_ticks(), playing) else {
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

    /// How long until [`Progress::tick`] would move the bar, or `None` when it will not move by
    /// itself (paused, no track length, no checkpoint time, or at the end).
    pub fn due(&self) -> Option<Duration> {
        let (timeline, playing) = self.reading?;
        if !playing || timeline.last_updated <= 0 {
            return None;
        }
        time_to_next_step(timeline, now_ticks())
    }

    /// Explorer restarted: the new shell holds no bar, so forget `last` and repost on the next tick.
    pub fn taskbar_restarted(&mut self) {
        self.last = None;
    }

    // No `clear` on purpose: at shutdown the tray loop is gone, so `tray::run` clears it directly.
}

/// Time until a playing `timeline` crosses into the next rounded step, a millisecond late so the
/// wake lands on the new step rather than just before it.
fn time_to_next_step(timeline: Timeline, now: i64) -> Option<Duration> {
    let span = timeline.end - timeline.start;
    let fraction = timeline.fraction_at(now, true)?;
    // `round` changes step half-way between two steps.
    let boundary = ((fraction * STEPS + 0.5).floor() + 0.5) / STEPS;
    if boundary > 1.0 {
        return None;
    }
    let ticks = ((boundary - fraction) * span as f64).ceil() as u64;
    Some(Duration::from_nanos(ticks * 100) + Duration::from_millis(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: i64 = 10_000_000;
    const NOW: i64 = 134_304_952_968_951_060;

    /// A 200 s track at `position_s`, published now: one step is exactly one second.
    fn at(position_s: f64) -> Timeline {
        Timeline {
            start: 0,
            end: 200 * SECOND,
            position: (position_s * SECOND as f64) as i64,
            last_updated: NOW,
        }
    }

    #[test]
    fn the_next_step_is_half_a_step_past_the_current_one() {
        // At 10.0 s the bar shows step 10; step 11 starts at 10.5 s.
        let wait = time_to_next_step(at(10.0), NOW).unwrap();
        assert!((wait.as_secs_f64() - 0.501).abs() < 1e-3, "{wait:?}");
        let wait = time_to_next_step(at(10.6), NOW).unwrap();
        assert!((wait.as_secs_f64() - 0.901).abs() < 1e-3, "{wait:?}");
    }

    #[test]
    fn the_last_step_needs_no_wake() {
        assert_eq!(time_to_next_step(at(199.6), NOW), None);
    }

    #[test]
    fn a_paused_or_unpublished_track_is_never_due() {
        let mut progress = Progress::new();
        progress.reading = Some((at(10.0), false));
        assert_eq!(progress.due(), None);
        progress.reading = Some((Timeline { last_updated: 0, ..at(10.0) }, true));
        assert_eq!(progress.due(), None);
        progress.reading = None;
        assert_eq!(progress.due(), None);
    }
}

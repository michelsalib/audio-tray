//! The track position, drawn as the shell's own taskbar progress bar.
//!
//! **Nothing here draws anything.** The bar is the one Windows already puts under a taskbar icon
//! when an app reports progress — the line MPC-HC shows while a file plays — and this module only
//! tells the shell what fraction to fill. That is worth the indirection: the colour, the position,
//! the rounded ends and the animation all come from the shell, so the bar matches every other app's
//! and keeps matching when the theme changes.
//!
//! Two things had to be measured before this could exist, both recorded in FINDINGS.md:
//!
//! * **`ITaskbarList3` accepts progress for another process's window.** It is normally an app
//!   reporting its own, and nothing documents the cross-process case. It works.
//! * **The `ProgressIndicator` element has to be kept and sized.** The TAP used to collapse it,
//!   because at a 244-epx button it stretches the full width of the strip. `place_button_state`
//!   gives it the icon's width instead, which is where MPC-HC's sits.

use anyhow::Result;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::ITaskbarList3;

use crate::music::smtc::{now_ticks, Timeline};

/// How finely the bar is stepped. 200 steps is half a percent — seven times finer than the 28 epx
/// it is drawn in, so the quantisation is invisible, and it cuts the cross-process calls to one
/// every few seconds on a normal track instead of one per poll.
const STEPS: f64 = 200.0;

/// Drives the taskbar progress bar on the player's window.
pub struct Progress {
    /// Created once and kept: `CoCreateInstance` plus `HrInit` per update would be a broker call a
    /// second for a value that rarely changes.
    taskbar: Option<ITaskbarList3>,
    window: Option<HWND>,
    /// The last thing actually sent, so an unchanged value costs nothing.
    last: Option<(u64, bool)>,
}

impl Progress {
    pub fn new() -> Self {
        Self {
            taskbar: None,
            window: None,
            last: None,
        }
    }

    /// Bring the bar in line with a timeline reading.
    ///
    /// `None` clears it, which is the right answer for "no session" and for "a session that
    /// publishes no timeline" alike: a bar stuck at some old fraction is worse than no bar.
    pub fn update(&mut self, timeline: Option<Timeline>, playing: bool) -> Result<()> {
        let fraction = timeline.and_then(|timeline| timeline.fraction_at(now_ticks(), playing));
        let step = fraction.map(|fraction| (fraction * STEPS).round() as u64);
        if self.last == step.map(|step| (step, playing)) {
            return Ok(());
        }
        // Logged on a change of *state*, not of value: a step is half a percent, so logging those
        // would be a line every second or two, while "playing at 71%" or "cleared" is the whole of
        // what one wants from the log when the bar looks wrong.
        let state_changed = self.last.map(|(_, was)| was) != Some(playing);
        if state_changed {
            match fraction {
                Some(fraction) => println!(
                    "progress bar -> {:.0}% ({})",
                    fraction * 100.0,
                    if playing { "playing" } else { "paused" }
                ),
                None => println!("progress bar -> cleared (no timeline)"),
            }
        }
        self.apply(step.map(|step| step as f64 / STEPS), playing)?;
        self.last = step.map(|step| (step, playing));
        Ok(())
    }

    /// Take the bar off the button — on quit, and whenever the player goes away.
    ///
    /// Called from the quit path for the same reason the notify icon is removed there: state we put
    /// on somebody else's window outlives us, and a progress bar frozen mid-track on an app that is
    /// not being followed any more is a bug the user cannot even attribute to us.
    pub fn clear(&mut self) {
        if self.last.is_none() {
            return;
        }
        if let Err(err) = self.apply(None, false) {
            eprintln!("could not clear the progress bar: {err:#}");
        }
        self.last = None;
    }

    fn apply(&mut self, fraction: Option<f64>, playing: bool) -> Result<()> {
        // The window is cached but re-validated: it dies when the user closes the player, and
        // reporting progress against a dead handle would fail every poll from then on.
        if !self.window.is_some_and(|hwnd| unsafe {
            windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(hwnd)).as_bool()
        }) {
            self.window = crate::music::player::player_window();
        }
        let Some(hwnd) = self.window else {
            // No window is not an error: the strip runs perfectly well with the player closed.
            return Ok(());
        };
        if self.taskbar.is_none() {
            self.taskbar = Some(crate::music::player::taskbar_list()?);
        }
        let Some(taskbar) = self.taskbar.as_ref() else {
            return Ok(());
        };
        crate::music::player::set_progress(taskbar, hwnd, fraction, playing)
    }
}

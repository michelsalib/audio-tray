//! Transport buttons in the taskbar's **hover preview**, via the shell's own thumbnail toolbar.
//!
//! This is the Windows 7 thumbnail toolbar — the previous/play/next row iTunes and MPC-HC put under
//! their preview — and it is worth trying before drawing one ourselves for the same reason the
//! progress bar goes through `ITaskbarList3`: the shell supplies the chrome, the theme, the hover
//! and press states, the DPI scaling and the placement, and none of it is ours to keep in step.
//!
//! **The part that is not documented is the part this feature needs.** `ThumbBarAddButtons` is
//! normally an app decorating its *own* window, and we are calling it for somebody else's. That much
//! has a precedent here — `SetProgressValue` cross-process was measured working in this same PR — but
//! the buttons are only half of it:
//!
//! ```text
//! do the buttons draw?          ITaskbarList3 cross-process — precedent says probably
//! do the clicks reach us?       WM_COMMAND/THBN_CLICKED goes to the *owning* window's
//!                               wndproc, which is Chromium's. It cannot reach this process.
//! ```
//!
//! So the click half has a known answer and it is "no, not this way". The plan that makes it work is
//! the one this codebase is already built around: the buttons the shell draws are XAML elements in
//! Explorer's visual tree, and the TAP is already inside Explorer attaching `Tapped` handlers to
//! taskbar elements. Draw with the documented API, wire with the TAP.
//!
//! [`probe`] answers the first question on its own, which is why it exists as a dev command before
//! any of the wiring is built.

use anyhow::{Context, Result};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, GetDC, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
};
use windows::Win32::UI::Shell::{
    ITaskbarList3, THBF_ENABLED, THB_FLAGS, THB_ICON, THB_TOOLTIP, THUMBBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, DestroyIcon, HICON, ICONINFO};

/// The three glyphs, in the order they sit under the preview.
///
/// Segoe Fluent codepoints, the same ones the taskbar strip uses — `E892` previous, `E768` play,
/// `E769` pause, `E893` next — so the two surfaces cannot drift apart visually.
const PREVIOUS: char = '\u{E892}';
const PLAY: char = '\u{E768}';
const PAUSE: char = '\u{E769}';
const NEXT: char = '\u{E893}';

/// Button ids. Arbitrary, but they are what comes back in `THBN_CLICKED`'s `HIWORD`, so they are
/// deliberately the **same wire codes** the taskbar strip already uses (`taskbar::Action` 10/11/12).
/// If the click ever does become reachable, there is then one numbering for both surfaces rather
/// than a translation table nobody remembers to update.
const ID_PREVIOUS: u32 = 10;
const ID_PLAY_PAUSE: u32 = 11;
const ID_NEXT: u32 = 12;

/// The buttons currently installed on a window, with the icons the shell is still referencing.
///
/// **The icons have to outlive the call.** `ThumbBarAddButtons` does not copy them — the shell holds
/// the `HICON`s and draws from them until they are replaced or the window goes away — so destroying
/// them after the call leaves three blank buttons. They are owned here and destroyed only when a
/// newer set has replaced them.
pub struct ThumbBar {
    taskbar: ITaskbarList3,
    window: HWND,
    icons: Vec<HICON>,
    /// `ThumbBarAddButtons` is once-per-window; everything after it must be `ThumbBarUpdateButtons`,
    /// and calling `Add` twice returns a failure the shell does not explain.
    added: bool,
    /// What the play/pause button last showed, so an unchanged state costs no cross-process call.
    showing_pause: Option<bool>,
}

impl ThumbBar {
    pub fn new(taskbar: ITaskbarList3, window: HWND) -> Self {
        Self {
            taskbar,
            window,
            icons: Vec::new(),
            added: false,
            showing_pause: None,
        }
    }

    /// Put the three buttons up, or bring the play/pause glyph in line with `playing`.
    ///
    /// Idempotent: the first call adds, later ones update, and an update that would change nothing
    /// is dropped before it costs a cross-process call.
    pub fn apply(&mut self, playing: bool) -> Result<()> {
        if self.added && self.showing_pause == Some(playing) {
            return Ok(());
        }

        let size = crate::win::small_icon_size();
        let previous = icon_from_glyph(PREVIOUS, size)?;
        let toggle = icon_from_glyph(if playing { PAUSE } else { PLAY }, size)?;
        let next = icon_from_glyph(NEXT, size)?;

        let buttons = [
            button(ID_PREVIOUS, previous, "Previous"),
            button(
                ID_PLAY_PAUSE,
                toggle,
                if playing { "Pause" } else { "Play" },
            ),
            button(ID_NEXT, next, "Next"),
        ];

        let result = unsafe {
            if self.added {
                self.taskbar
                    .ThumbBarUpdateButtons(self.window, &buttons)
                    .context("ThumbBarUpdateButtons")
            } else {
                self.taskbar
                    .ThumbBarAddButtons(self.window, &buttons)
                    .context("ThumbBarAddButtons")
            }
        };
        let fresh = vec![previous, toggle, next];
        if let Err(err) = result {
            // The shell never took them, so nothing is referencing these — destroy them now rather
            // than holding them for a set that was refused.
            destroy(fresh);
            return Err(err);
        }

        // Only now: the shell has stopped drawing from the previous set.
        destroy(std::mem::replace(&mut self.icons, fresh));
        self.added = true;
        self.showing_pause = Some(playing);
        Ok(())
    }

    /// Forget that the toolbar was ever installed, without trying to take it down.
    ///
    /// For an Explorer restart: the registration lives in the *shell*, so a new Explorer has no
    /// record of it — but `added` still says otherwise, and every later call would take the
    /// `ThumbBarUpdateButtons` branch and update a toolbar that no longer exists. That is what left
    /// the preview with no buttons after a restart, silently, because updating a forgotten toolbar
    /// does not report an error.
    ///
    /// Clearing the flag also disarms [`ThumbBar::drop`]'s grey-them-out step, which is right: there
    /// is nothing on the new shell to grey.
    fn forget_registration(&mut self) {
        self.added = false;
        self.showing_pause = None;
    }
}

impl Drop for ThumbBar {
    /// **There is no `ThumbBarRemoveButtons`.** Once a window has a thumbnail toolbar it keeps it
    /// until the window is destroyed, so the honest teardown is to leave the buttons disabled rather
    /// than pretend they can be taken away — a live-looking button that no longer does anything is
    /// worse than a visibly greyed one.
    fn drop(&mut self) {
        if self.added {
            let size = crate::win::small_icon_size();
            let dim = |glyph| icon_from_glyph(glyph, size).ok();
            if let (Some(previous), Some(play), Some(next)) =
                (dim(PREVIOUS), dim(PLAY), dim(NEXT))
            {
                let buttons = [
                    disabled(ID_PREVIOUS, previous, "Previous"),
                    disabled(ID_PLAY_PAUSE, play, "Play"),
                    disabled(ID_NEXT, next, "Next"),
                ];
                let _ = unsafe { self.taskbar.ThumbBarUpdateButtons(self.window, &buttons) };
                destroy(vec![previous, play, next]);
            }
        }
        destroy(std::mem::take(&mut self.icons));
    }
}

fn button(id: u32, icon: HICON, tip: &str) -> THUMBBUTTON {
    THUMBBUTTON {
        dwMask: THB_ICON | THB_TOOLTIP | THB_FLAGS,
        iId: id,
        hIcon: icon,
        szTip: tooltip(tip),
        dwFlags: THBF_ENABLED,
        ..Default::default()
    }
}

fn disabled(id: u32, icon: HICON, tip: &str) -> THUMBBUTTON {
    use windows::Win32::UI::Shell::THBF_DISABLED;
    THUMBBUTTON {
        dwFlags: THBF_DISABLED,
        ..button(id, icon, tip)
    }
}

/// The button's tooltip — and **the identity the TAP matches on**.
///
/// These strings are a contract with `music::thumbbar` in the TAP, exactly like the 10/11/12 wire
/// codes: the shell exposes `szTip` as the button's accessible name, and that is how the TAP tells
/// the three buttons apart when it attaches the click handlers. It cannot use their position,
/// because updating the play glyph makes the shell rebuild that one button and announce it last.
/// Keep "Previous", "Play"/"Pause" and "Next" as substrings if these are ever reworded.
///
/// `szTip` is a fixed 260-wide buffer, not a pointer, so the text is copied into it and truncated
/// with room left for the terminator.
fn tooltip(text: &str) -> [u16; 260] {
    let mut buffer = [0u16; 260];
    for (slot, ch) in buffer.iter_mut().zip(text.encode_utf16()).take(259) {
        *slot = ch;
    }
    buffer
}

fn destroy(icons: Vec<HICON>) {
    for icon in icons {
        let _ = unsafe { DestroyIcon(icon) };
    }
}

/// Rasterise a Segoe Fluent glyph into an `HICON`.
///
/// White, because the thumbnail toolbar is drawn on the flyout's own backdrop, which follows the
/// system theme and is dark in both of Windows' default schemes. This is the one place the strip's
/// accent-derived `on_accent` contrast maths does not apply — there is no accent plate behind these.
fn icon_from_glyph(glyph: char, size: u32) -> Result<HICON> {
    let (rgba, width, height) = crate::icons::render_glyph(glyph, size, [255, 255, 255])
        .with_context(|| format!("rasterising U+{:04X} for the thumbnail toolbar", glyph as u32))?;
    unsafe { icon_from_rgba(&rgba, width, height) }
}

/// Build an `HICON` from straight (non-premultiplied) RGBA.
///
/// **Straight alpha is what an icon wants**, and it is what [`crate::icons::render_glyph`] produces —
/// full colour with coverage in the alpha channel. This is the opposite of the layered-window path in
/// `layered.rs`, which needs the same pixels premultiplied; handing either one the other's format is
/// a dark halo round every glyph.
///
/// # Safety
/// `rgba` must be `width * height * 4` bytes.
unsafe fn icon_from_rgba(rgba: &[u8], width: u32, height: u32) -> Result<HICON> {
    let header = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            // Negative: top-down, matching the buffer's row order. Bottom-up would draw every glyph
            // upside down, which is the classic tell that this sign was left positive.
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let screen = unsafe { GetDC(None) };
    let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
    let colour = unsafe {
        CreateDIBSection(
            Some(screen),
            &header,
            DIB_RGB_COLORS,
            &mut bits,
            None,
            0,
        )
    };
    if !screen.is_invalid() {
        unsafe { ReleaseDC(None, screen) };
    }
    let colour = colour.context("CreateDIBSection for the thumbnail button icon")?;
    if bits.is_null() {
        let _ = unsafe { DeleteObject(HGDIOBJ(colour.0)) };
        anyhow::bail!("CreateDIBSection returned no pixel buffer");
    }

    // RGBA in, BGRA out — the DIB is little-endian `0xAARRGGBB`, so the red and blue bytes swap.
    let pixels = (width * height) as usize;
    let out = unsafe { std::slice::from_raw_parts_mut(bits as *mut u8, pixels * 4) };
    for i in 0..pixels {
        out[i * 4] = rgba[i * 4 + 2];
        out[i * 4 + 1] = rgba[i * 4 + 1];
        out[i * 4 + 2] = rgba[i * 4];
        out[i * 4 + 3] = rgba[i * 4 + 3];
    }

    // An all-zero mask: with a 32-bit colour bitmap the alpha channel is what decides transparency,
    // and the mask only has to exist. A mask of ones would hide the icon entirely.
    let mask: HBITMAP = unsafe { CreateBitmap(width as i32, height as i32, 1, 1, None) };

    let info = ICONINFO {
        fIcon: true.into(),
        hbmMask: mask,
        hbmColor: colour,
        ..Default::default()
    };
    let icon = unsafe { CreateIconIndirect(&info) };

    // `CreateIconIndirect` copies both bitmaps, so ours go back now regardless of the outcome.
    let _ = unsafe { DeleteObject(HGDIOBJ(colour.0)) };
    let _ = unsafe { DeleteObject(HGDIOBJ(mask.0)) };

    icon.context("CreateIconIndirect for the thumbnail button icon")
}

/// Keeps the toolbar on whichever window the player currently has.
///
/// Separate from [`ThumbBar`] because the window is not a given: YouTube Music can be closed and
/// reopened under a running audio-tray, and the toolbar has to follow it onto the new window. The
/// same shape as [`crate::music::progress::Progress`], and for the same reason.
pub struct Toolbar {
    bar: Option<ThumbBar>,
    /// The window the current `bar` is attached to, re-validated because it dies with the player.
    window: Option<HWND>,
}

impl Toolbar {
    pub fn new() -> Self {
        Self {
            bar: None,
            window: None,
        }
    }

    /// Put the buttons up on the player's window, or bring their state in line with `playing`.
    ///
    /// Silent when there is no player window: the strip runs perfectly well with YouTube Music
    /// closed, and there is no taskbar button to decorate then.
    pub fn update(&mut self, playing: bool) {
        use windows::Win32::UI::WindowsAndMessaging::IsWindow;

        let live = self
            .window
            .is_some_and(|hwnd| unsafe { IsWindow(Some(hwnd)).as_bool() });
        if !live {
            // The player went away, or has not been found yet. Drop the old toolbar rather than
            // reporting against a dead handle for the rest of the session.
            self.bar = None;
            self.window = super::player::player_window();
        }
        let Some(hwnd) = self.window else {
            return;
        };

        if self.bar.is_none() {
            match super::player::taskbar_list() {
                Ok(taskbar) => self.bar = Some(ThumbBar::new(taskbar, hwnd)),
                Err(err) => {
                    eprintln!("music: no taskbar list for the thumbnail toolbar ({err:#})");
                    return;
                }
            }
        }
        if let Some(bar) = self.bar.as_mut() {
            if let Err(err) = bar.apply(playing) {
                eprintln!("music: could not set the thumbnail toolbar ({err:#})");
            }
        }
    }

    /// Grey the buttons out on the way down.
    ///
    /// There is no way to take a thumbnail toolbar off a window short of destroying the window, so
    /// this is the honest teardown — see [`ThumbBar::drop`].
    pub fn clear(&mut self) {
        self.bar = None;
    }

    /// Explorer restarted: the shell's record of our toolbar went with it, so put it back.
    ///
    /// The buttons are *shell* state on a window we do not own, which makes an Explorer restart
    /// exactly as destructive to them as it is to the strip — and unlike the strip, nothing about the
    /// window changes, so no amount of re-validating the handle notices. Re-adding is the only route,
    /// and it needs the "already added" flag cleared first.
    pub fn taskbar_restarted(&mut self) {
        if let Some(bar) = self.bar.as_mut() {
            // Before dropping it: this disarms the grey-them-out step in `ThumbBar::drop`, which
            // would otherwise call into the Explorer that has just gone.
            bar.forget_registration();
        }
        // Dropped rather than reused, because the `ITaskbarList3` inside it is a **proxy into
        // explorer.exe** — after a restart it addresses a dead process. `update` builds a fresh one.
        self.bar = None;
    }
}

/// Put the three buttons on the player's window and leave them there, for `--music-thumbbar`.
///
/// **The measurement this exists for.** Everything downstream — whether the buttons are worth wiring
/// through the TAP at all — depends on one unknown: does `ThumbBarAddButtons` accept a window this
/// process does not own? Nothing documents that case. Hover the player's taskbar button after running
/// this and the answer is on screen.
pub fn probe(playing: bool) -> Result<()> {
    let window = super::player::player_window()
        .context("no YouTube Music window to put a thumbnail toolbar on")?;
    let taskbar = super::player::taskbar_list()?;
    let mut bar = ThumbBar::new(taskbar, window);
    bar.apply(playing)?;
    println!(
        "music: thumbnail toolbar installed on {:?} ({} state)",
        window.0,
        if playing { "playing" } else { "paused" }
    );
    println!("hover the YouTube Music taskbar button to see whether the shell drew them.");
    // Deliberately leaked: `Drop` disables the buttons, which would undo the very thing being
    // measured before there is any chance to look at it.
    std::mem::forget(bar);
    Ok(())
}

//! Transport buttons in the taskbar's **hover preview**, via the shell's own thumbnail toolbar.
//!
//! `ThumbBarAddButtons` works on another process's window, but `THBN_CLICKED` goes to the player's
//! own wndproc, so this side only draws: the TAP finds the shell's `ThumbBarButton` elements and
//! handles their clicks. See FINDINGS.md, 'The transport controls moved to the hover preview'.

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

/// The Segoe Fluent glyphs (same codepoints the taskbar strip uses).
const PREVIOUS: char = '\u{E892}';
const PLAY: char = '\u{E768}';
const PAUSE: char = '\u{E769}';
const NEXT: char = '\u{E893}';

/// Button ids, deliberately the same as the strip's transport wire codes (10/11/12).
const ID_PREVIOUS: u32 = 10;
const ID_PLAY_PAUSE: u32 = 11;
const ID_NEXT: u32 = 12;

/// The buttons installed on a window. Owns the `HICON`s, which the shell keeps drawing from (not
/// copied): destroy them only once a newer set has replaced them.
pub struct ThumbBar {
    taskbar: ITaskbarList3,
    window: HWND,
    icons: Vec<HICON>,
    /// `ThumbBarAddButtons` is once per window; afterwards only `ThumbBarUpdateButtons` works.
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

    /// Put the three buttons up, or bring the play/pause glyph in line with `playing`. Idempotent;
    /// an unchanged state costs nothing.
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
            // Refused, so nothing references these.
            destroy(fresh);
            return Err(err);
        }

        // Only now: the shell has stopped drawing from the previous set.
        destroy(std::mem::replace(&mut self.icons, fresh));
        self.added = true;
        self.showing_pause = Some(playing);
        Ok(())
    }

    /// Forget the registration after an Explorer restart (the new shell has none, and updating a
    /// forgotten toolbar fails silently). Also disarms [`ThumbBar::drop`]'s grey-out.
    fn forget_registration(&mut self) {
        self.added = false;
        self.showing_pause = None;
    }
}

impl Drop for ThumbBar {
    /// There is no `ThumbBarRemoveButtons`, so teardown leaves the buttons visibly disabled.
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

/// The button's tooltip, which the TAP's `music::thumbbar` matches on as the accessible name. Keep
/// "Previous", "Play"/"Pause" and "Next" as substrings. Truncated to fit `szTip` with a terminator.
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

/// Rasterise a Segoe Fluent glyph into a white `HICON` (the preview flyout's backdrop is dark).
fn icon_from_glyph(glyph: char, size: u32) -> Result<HICON> {
    let (rgba, width, height) = crate::icons::render_glyph(glyph, size, [255, 255, 255])
        .with_context(|| format!("rasterising U+{:04X} for the thumbnail toolbar", glyph as u32))?;
    unsafe { icon_from_rgba(&rgba, width, height) }
}

/// Build an `HICON` from straight (non-premultiplied) RGBA, as [`crate::icons::render_glyph`]
/// produces; premultiplied input would give a dark halo.
///
/// # Safety
/// `rgba` must be `width * height * 4` bytes.
unsafe fn icon_from_rgba(rgba: &[u8], width: u32, height: u32) -> Result<HICON> {
    let header = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            // Negative: top-down, matching the buffer's row order.
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

    // All-zero mask: alpha decides transparency; the mask only has to exist.
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

/// Keeps the toolbar on whichever window the player currently has (it can close and reopen).
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
    /// Silent when there is no player window.
    pub fn update(&mut self, playing: bool) {
        use windows::Win32::UI::WindowsAndMessaging::IsWindow;

        let live = self
            .window
            .is_some_and(|hwnd| unsafe { IsWindow(Some(hwnd)).as_bool() });
        if !live {
            // Player gone or not yet found: drop the old toolbar and look again.
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

    /// Grey the buttons out on the way down (see [`ThumbBar::drop`]).
    pub fn clear(&mut self) {
        self.bar = None;
    }

    /// Explorer restarted and took the toolbar with it: the next `update` re-adds it.
    pub fn taskbar_restarted(&mut self) {
        if let Some(bar) = self.bar.as_mut() {
            // Before dropping: disarms the grey-out, which would call into the dead Explorer.
            bar.forget_registration();
        }
        // Not reused: its `ITaskbarList3` is a proxy into the dead Explorer.
        self.bar = None;
    }
}

/// Put the three buttons on the player's window and leave them there, for `--music-thumbbar`.
#[cfg(feature = "dev")]
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
    // Leaked on purpose: `Drop` would disable the buttons before anyone looks.
    std::mem::forget(bar);
    Ok(())
}

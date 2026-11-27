//! The taskbar's badges, drawn here and painted by the TAP: the bell's count, the focus timer's pie,
//! and the dot on Windows' battery icon. Each look is a PNG file of its own, drawn once.

use super::*;

/// A badge: its PNG, its width in DIPs, the DPI it was drawn at.
pub type Badge = (String, u32, u32);

/// The bell's badge: an accent pill with `count` (how many wait), drawn at the taskbar's scale into
/// a PNG the TAP paints beside the bell (beside the clock without one). Gives the file, its width in
/// DIPs and the taskbar's DPI. Each look (count, colour, scale) is its own file, drawn once: XAML
/// caches images by address.
pub fn badge(count: u32) -> Option<Badge> {
    // Asked at every tick: the same count within a while is the same badge (the accent and scale
    // seldom change, and are looked up again after).
    thread_local! {
        static LAST: RefCell<Option<(u32, Instant, Option<Badge>)>> = const { RefCell::new(None) };
    }
    if let Some(badge) = LAST.with(|l| l.borrow().as_ref().filter(|(n, at, _)| *n == count && at.elapsed() < BADGE_FRESH).map(|(.., b)| b.clone())) {
        return badge;
    }
    let badge = draw_badge(count);
    LAST.with(|l| *l.borrow_mut() = Some((count, Instant::now(), badge.clone())));
    badge
}

/// How long a badge looked up stands for its count.
const BADGE_FRESH: std::time::Duration = std::time::Duration::from_secs(60);

/// The focus timer on the bell, with nothing waiting: an accent ring (a timer at a glance, from
/// its first second) around an accent pie, `done` of it filled, over a faint disc. A badge file
/// as the count's ([`badge_file`]), in twelve steps, the first showing from the start.
pub fn focus_pie(done: f32) -> Option<Badge> {
    let step = ((done.clamp(0.0, 1.0) * 12.0).ceil() as u32).clamp(1, 12);
    badge_file(&format!("timer{step}"), BADGE_H, |pen, fill, _| {
        let disc = R::new(0.0, 0.0, BADGE_H, BADGE_H);
        pen.fill(disc, BADGE_H / 2.0, fill.alpha(0.18));
        pen.ring(disc, 1.5, fill);
        pen.pie(R::new(3.0, 3.0, BADGE_H - 6.0, BADGE_H - 6.0), step as f32 / 12.0, fill);
    })
}

/// A badge's height, in DIPs.
const BADGE_H: f32 = 14.0;

/// The badge file `name`, `w` DIPs wide, drawn by `draw` (given the pen, the accent fill and the
/// figures' ink) at the taskbar's scale, once per look: `<name>-b<fill>-<ink>-<dpi>.png`. A new
/// look (another colour or scale) clears the old looks' files.
fn badge_file(name: &str, w: f32, draw: impl FnOnce(&paint::Pen, Color, Color)) -> Option<Badge> {
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    let tray = unsafe { FindWindowW(w!("Shell_TrayWnd"), None) }.ok();
    let dpi = tray.map_or(96, |t| unsafe { GetDpiForWindow(t) }).max(96);
    let scale = dpi as f32 / 96.0;
    let (fill, ink) = badge_colours(Palette::current().accent);
    let hex = |c: Color| format!("{:02x}{:02x}{:02x}", (c.0 * 255.0) as u8, (c.1 * 255.0) as u8, (c.2 * 255.0) as u8);
    let dir = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("audio-tray").join("badges");
    let look = format!("-b{}-{}-{dpi}.png", hex(fill), hex(ink));
    let path = dir.join(format!("{name}{look}"));
    if !path.exists() {
        let _ = std::fs::create_dir_all(&dir);
        // Another colour or scale: the old looks' files go.
        for old in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if !old.file_name().to_string_lossy().ends_with(&look) {
                let _ = std::fs::remove_file(old.path());
            }
        }
        let size = ((w * scale).round() as u32, (BADGE_H * scale).round() as u32);
        let drawn = Gfx::new().and_then(|mut gfx| {
            gfx.frame(size, scale, |pen| {
                pen.clear(Color(0.0, 0.0, 0.0, 0.0));
                draw(pen, fill, ink);
            })?;
            gfx.save_png(&path)
        });
        if let Err(e) = drawn {
            eprintln!("calendar: could not draw the bell's badge {name} ({e:#})");
            return None;
        }
    }
    Some((path.to_string_lossy().into_owned(), w as u32, dpi))
}

/// A badge's fill and its figures' ink: the accent, dark figures on a light one (as the shell's).
fn badge_colours(accent: Color) -> (Color, Color) {
    let Color(r, g, b, _) = accent;
    let ink = if 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.55 { Color::hex(0x000000, 0.9) } else { Color::hex(0xFFFFFF, 1.0) };
    (accent, ink)
}

/// The dot on Windows' battery icon while a device runs low: a red disc at the picture's upper
/// right (the TAP sets the picture at the icon's upper right). A badge file as the bell's.
pub fn battery_dot() -> Option<Badge> {
    const SIZE: f32 = 7.0;
    badge_file("lowdot", SIZE + 1.0, |pen, _, _| {
        let [r, g, b] = scene::RED;
        pen.fill(R::new(0.5, 0.5, SIZE, SIZE), SIZE / 2.0, Color(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0));
    })
}

/// A small accent bubble with `count` on the bell's shoulder (the TAP keeps the bell itself a
/// plain outline meanwhile, so the two do not run together).
fn draw_badge(count: u32) -> Option<Badge> {
    let label = if count > 99 { "99+".to_string() } else { count.to_string() };
    let w = (BADGE_H + 6.0 * (label.chars().count().max(1) - 1) as f32).round();
    badge_file(&label.replace('+', "p"), w, |pen, fill, ink| {
        pen.fill(R::new(0.0, 0.0, w, BADGE_H), BADGE_H / 2.0, fill);
        pen.text(&label, paint::Font::TinyStrong, R::new(0.0, -0.5, w, BADGE_H), ink, paint::Align::Center);
    })
}

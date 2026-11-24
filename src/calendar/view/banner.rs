//! Banners: while Audio Tray shows every notification itself (Settings' take-over, with Windows'
//! own banners turned off by [`crate::calendar::takeover`]), each notification arriving in
//! Windows' list shows here, over the bell, on a card of its own drawn as the notification centre
//! draws it ([`scene::banner`]): its app, its text and picture, its buttons.
//!
//! As Windows' do: the newest nearest the corner, three at most, each for the time Windows'
//! accessibility setting gives notifications (reminders and calls until answered), held while
//! the pointer is over any of them. A click opens it, a button does its thing, its X sends it to
//! the list only. While Do not disturb is on, a full-screen app or a presentation runs, only calls,
//! alarms and reminders show, and a digest says what came meanwhile once it is over. None while
//! the notification panel is open (it lists them).
//!
//! Livelier than Windows': each banner wears its app's colour (from its logo), springs in, shows
//! its time left along its foot, and flies into the bell (which rings) when that runs out. A
//! conversation's messages share one banner, its faces stacked. Chats get one-tap reactions.
//! Calls pulse, their buttons green and red. Audio Tray's own notices are illustrated: a battery
//! draining, a speaker playing, a meeting counting down, confetti when focus is done.
//!
//! A glance (a keyboard layout switched) is news for a moment, never listed: Audio Tray's own solid
//! accent pill ([`scene::glance`]). It shows whenever the bell is Audio Tray's.
//!
//! Each banner is a layered window that never takes the foreground (`WS_EX_NOACTIVATE`) unless its
//! reply box is open: what the user was typing in keeps the keys. They share one drawing kit.
//! Tray thread.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, KillTimer, SetTimer, ShowWindow, MA_NOACTIVATE, SW_SHOWNA, WA_INACTIVE, WINDOW_EX_STYLE, WM_ACTIVATE, WM_CHAR,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_RBUTTONUP, WM_TIMER, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};

use super::paint::{Gfx, Palette};
use super::scene::{self, Art, Face, Hit, Scene};
use super::{arrived as when_words, client_xy, cursor, draw, edge_of, monitor, track_leave, Edge, MARGIN, WM_MOUSELEAVE};
use crate::calendar::notifications::{self, Toast};
use crate::calendar::rich::{self, Kind};
use crate::config::Config;

const CLASS: PCWSTR = w!("AudioTrayBanner");
/// Banners shown at once, at most: a fourth sends the oldest to the list.
const MAX: usize = 3;
/// Between two banners, in DIPs.
const GAP: f32 = 8.0;
const LIFE_TIMER: usize = 1;
const LIFE_MS: u32 = 200;
/// Its frames: every compositor tick or so (Windows timers step by 15.6 ms), so that its time line
/// glides rather than steps.
const FRAME_TIMER: usize = 2;
/// 10 ms: under one timer tick (15.6 ms), so it fires on every tick.
const FRAME_MS: u32 = 10;
/// A meeting's countdown says seconds.
const FRAME_SECOND_MS: u32 = 1000;
/// A call's rings, drawn whole each frame: smooth enough at half the pace.
const FRAME_CALL_MS: u32 = 33;
/// Nothing moves on it (a reminder that stays): looked at now and then.
const FRAME_STILL_MS: u32 = 250;
/// A layout switch's glyph (the edge pill's).
const GLYPH_KEYBOARD: char = '\u{E765}';
/// A layout switch: a glance.
const GLANCE: Duration = Duration::from_millis(1500);
/// A reply left half typed (the user went elsewhere): the banner waits this long for them.
const ABANDONED: Duration = Duration::from_secs(60);
/// A notification asking to stay long (`duration="long"`), as Windows keeps it.
const LONG: Duration = Duration::from_secs(25);
/// How long a sound waits for Windows' store to say whether it is wanted.
const CHIME_WAIT: Duration = Duration::from_millis(1500);
/// A reply or reaction sent: "Sent" shows this long, then the banner goes.
const SENT_FOR: Duration = Duration::from_millis(900);
/// How long an illustration moves (a battery draining, a speaker playing, confetti).
const ART_FOR: Duration = Duration::from_millis(2400);
/// A conversation's count, growing for a moment as it counts up.
const POP_FOR: Duration = Duration::from_millis(320);
/// A notification already this old when first read is not news (read late: after a sleep, or
/// Windows' store slow to say).
const FRESH_S: i64 = 120;
/// The quick reactions a chat's banner offers beside Reply.
const QUICK: [&str; 3] = ["👍", "❤️", "😂"];
/// Audio Tray's own colours for its own notices, and the digest's.
const AMBER: [u8; 3] = [0xF2, 0xA3, 0x3A];
const RED: [u8; 3] = [0xE5, 0x48, 0x4D];
const BLUE: [u8; 3] = [0x2F, 0x86, 0xE0];
const GREEN: [u8; 3] = [0x3F, 0xBF, 0x7F];

/// What a banner is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Is {
    /// A notification in Windows' list.
    Listed,
    /// A layout switch: a glance, never listed.
    Glance,
    /// What came during a quiet time, summed up; opens the list.
    Digest,
}

/// One of Audio Tray's own notices, from its tag (`audio-tray/<kind>/<detail>`): drawn with its
/// own picture.
#[derive(Clone, Copy, PartialEq)]
enum Own {
    No,
    Battery(u8),
    Audio,
    /// When the meeting starts (seconds since the epoch).
    Meeting(i64),
    Focus,
}

impl Own {
    fn of(t: &Toast) -> Own {
        if t.aumid != crate::toast::AUMID {
            return Own::No;
        }
        let Some(rest) = t.rich.tag.strip_prefix(crate::toast::TAG) else { return Own::No };
        let mut parts = rest.split('/');
        match parts.next() {
            Some("battery") => Own::Battery(parts.next().and_then(|l| l.parse().ok()).unwrap_or(10)),
            Some("audio") => Own::Audio,
            Some("meeting") => parts.next().and_then(|s| s.parse().ok()).map_or(Own::No, Own::Meeting),
            Some("focus") => Own::Focus,
            _ => Own::No,
        }
    }
}

struct Shown {
    hwnd: HWND,
    toast: Toast,
    is: Is,
    /// When it goes; `None`: when answered or dismissed (a reminder, a call).
    until: Option<Instant>,
    /// How long it was given (its countdown runs down this).
    life: Option<Duration>,
    /// When it came (its picture moves for a while from then).
    born: Instant,
    /// Held by the pointer since then: its time stands still (its line drawn as of then), and the
    /// whole hold is added to its end once the pointer leaves.
    held_since: Option<Instant>,
    hover: Option<Hit>,
    pressed: Option<Hit>,
    /// Its reply box is open, with what is typed: the banner has the keyboard meanwhile, and stays.
    draft: Option<String>,
    /// A reply or reaction went: what its buttons say for a moment ("Sent 👍").
    sent: Option<String>,
    /// Its sound, waiting for Windows' store to say whether it wants one (its payload not read
    /// yet), until then at the latest.
    chime_at: Option<Instant>,
    /// A conversation's banner: the faces of those who wrote before the newest, and how many
    /// more came, the last when.
    faces: Vec<Face>,
    more: usize,
    more_at: Option<Instant>,
    /// Its app's colour.
    tint: Option<[u8; 3]>,
    /// The pointer holds the banners: its time line stops, brighter.
    paused: bool,
    /// Drawn with its time line full and empty, for a frame that only moves the line: composed
    /// from the two (a few rows copied), not drawn again. Gone when anything else changes.
    line: Option<Line>,
    scene: Scene,
    /// Its own drawing kit: its text laid out and its pictures decoded once, never thrown away by
    /// another banner drawing in between (a kit keeps one surface and one generation of layouts).
    gfx: Gfx,
    canvas: crate::layered::Canvas,
    /// Its place in the stack on screen, in pixels.
    at: (i32, i32),
    /// Where its window is now: at its place, or on its way there.
    pos: (i32, i32),
    /// Gliding to its place: since its first frame (`None` before it: a glide planned while the
    /// tray was busy does not start half done), and from where.
    glide: Option<(Option<Instant>, (i32, i32))>,
    /// On its way in or out, since when.
    motion: Option<(Motion, Instant)>,
    /// Its frame timer's pace now (ms; 0: not running): set again only when it changes, since
    /// setting it restarts its count (16 ms set each frame came every 31 ms).
    frame_ms: u32,
    /// When it came, in words, and the minute those words are for.
    when: (i64, String),
}

/// A banner drawn twice, its time line full and empty: only the line's rows of each, kept to
/// compose a frame where only the line moves (its left part from the full one, the rest from the
/// empty one), written into the window's bitmap and sent alone.
struct Line {
    full: Vec<u8>,
    empty: Vec<u8>,
    size: (i32, i32),
    /// The line's rows and columns.
    rows: std::ops::Range<i32>,
    columns: std::ops::Range<i32>,
    /// Where the line was cut last (a frame where it has not moved a pixel sends nothing).
    cut: i32,
}

struct Banners {
    shown: Vec<Shown>,
    palette: Option<Palette>,
    scale: f32,
    work: RECT,
    edge: Edge,
    /// The notifications listed at the last read (`None` before the first: all of those are old).
    seen: Option<Vec<u32>>,
    /// The window that had the keyboard before a reply box took it: it gets it back.
    keys_from: Option<HWND>,
    /// The next synthetic banner's id (a glance, the digest), counted down from the top (Windows'
    /// ids count up from 1).
    next_own: u32,
    /// Notifications held back while quiet, for the digest once it is over.
    held: Vec<u32>,
    /// The thread timer looking for the quiet's end (0: none).
    quiet_timer: usize,
    /// The config, for the digest's button (it opens the list).
    config: Option<Config>,
    /// Each app's colour, from its logo, by its id (`None`: a grey logo, the accent then).
    tints: Vec<(String, Option<[u8; 3]>)>,
}

thread_local! {
    static BANNERS: RefCell<Banners> = RefCell::new(Banners {
        shown: Vec::new(),
        palette: None,
        scale: 1.0,
        work: RECT::default(),
        edge: Edge::Bottom,
        seen: None,
        keys_from: None,
        next_own: u32::MAX,
        held: Vec::new(),
        quiet_timer: 0,
        config: None,
        tints: Vec::new(),
    });
}

/// Work to do once the banners' state is released: it may pump messages.
type After = Box<dyn FnOnce()>;

fn with<R>(f: impl FnOnce(&mut Banners) -> R) -> Option<R> {
    BANNERS.with(|b| b.try_borrow_mut().ok().map(|mut b| f(&mut b)))
}

/// Windows' list was read again: what is new shows (when Audio Tray shows them), what is gone from
/// the list leaves, and what Windows' store has since told of a shown one (its picture, its
/// buttons) is drawn in.
pub fn list_changed(config: &Config) {
    if !notifications::read_yet() {
        return;
    }
    let list = notifications::list();
    let handed_over = crate::calendar::handed_over(config);
    let fresh: Vec<Toast> = with(|b| {
        b.config = Some(config.clone());
        let seen = b.seen.replace(list.iter().map(|t| t.id).collect());
        // Shown ones follow the list: what Windows' store told since (a picture, its buttons, that
        // it is a call) is drawn in, and its sound comes once it is known to want one.
        let (mut gone, mut redraw, mut sound) = (Vec::new(), Vec::new(), false);
        for s in b.shown.iter_mut().filter(|s| s.is == Is::Listed && s.sent.is_none() && !leaving(s)) {
            match list.iter().find(|t| t.id == s.toast.id) {
                Some(t) if *t != s.toast => {
                    s.toast = t.clone();
                    if s.toast.rich.sticky {
                        s.until = None;
                    }
                    if s.chime_at.is_some() && rich::read_for(s.toast.id) {
                        s.chime_at = None;
                        sound |= !s.toast.rich.silent;
                    }
                    redraw.push(s.hwnd);
                }
                Some(_) => {}
                // Gone from Windows' list (dismissed elsewhere; an app withdrawing it, as WhatsApp
                // does once it is read on the phone): the banner goes with it, but not while a reply
                // is being typed in it (the reply reaches the app all the same).
                None if s.draft.is_some() => println!("banners: {} \"{}\" left Windows' list; kept while replying", s.toast.app, s.toast.title),
                None => gone.push(s.hwnd),
            }
        }
        for hwnd in gone {
            leave(b, hwnd, Exit::Slide);
        }
        for &hwnd in &redraw {
            b.refresh(hwnd);
        }
        b.place();
        for hwnd in redraw {
            b.render(hwnd, 255, 0);
        }
        if sound {
            chime();
        }
        let Some(seen) = seen else { return Vec::new() };
        let now = crate::calendar::time::now();
        // Oldest first: the newest ends nearest the corner.
        list.iter().rev().filter(|t| !seen.contains(&t.id) && now - t.at <= FRESH_S).cloned().collect()
    })
    .unwrap_or_default();
    if !handed_over || fresh.is_empty() {
        return;
    }
    if super::OPEN.with(std::cell::Cell::get) != 0 {
        return; // the panel lists them
    }
    // Quiet (Do not disturb, a full-screen app, a presentation): only what Windows lets through
    // by default, calls, alarms and reminders; the rest summed up once it is over.
    let quiet = crate::calendar::focus::active() || busy();
    let (fresh, held): (Vec<Toast>, Vec<Toast>) = fresh.into_iter().partition(|t| !quiet || t.rich.sticky);
    if !held.is_empty() {
        println!("banners: {} quiet (Do not disturb, a full-screen app, a presentation, or the screen locked)", held.len());
        hold_for_digest(held.iter().map(|t| t.id));
    }
    use crate::calendar::takeover::{wants_banner, windows_shows};
    // Each one ours to show: not one whose banners were off before Audio Tray took them, nor the
    // first of an app (or site) Windows had no settings for, whose banner Windows shows itself.
    let ours: Vec<Toast> = fresh
        .into_iter()
        .filter(|t| {
            let windows = windows_shows(&t.aumid);
            if windows {
                println!("banners: {} \"{}\" left to Windows' own banner (its first; its next are ours)", t.app, t.title);
            }
            !windows && wants_banner(&t.aumid)
        })
        .collect();
    // A sound for those known to want one now; the others' once their payload is read.
    let sound = ours.iter().any(|t| rich::read_for(t.id) && !t.rich.silent);
    for toast in ours {
        println!("banners: {} \"{}\" ({})", toast.app, toast.title, toast.aumid);
        if join_conversation(&toast) {
            continue;
        }
        let life = if toast.rich.sticky { None } else if toast.rich.long { Some(LONG.max(lifetime())) } else { Some(lifetime()) };
        let chime_at = (!rich::read_for(toast.id)).then(|| Instant::now() + CHIME_WAIT);
        show(toast, Is::Listed, life, chime_at);
    }
    if sound {
        chime();
    }
}

/// A message in a conversation already showing: its banner takes it (the newest line, the face
/// of who wrote before stacked behind, "+1 more"), and stays for it. `false`: none was showing.
fn join_conversation(toast: &Toast) -> bool {
    if toast.rich.thread.is_empty() {
        return false;
    }
    with(|b| {
        let s = b.shown.iter_mut().find(|s| s.is == Is::Listed && !leaving(s) && s.sent.is_none() && s.draft.is_none() && s.toast.aumid == toast.aumid && s.toast.rich.thread == toast.rich.thread)?;
        let before = face_of(&s.toast);
        if !s.faces.contains(&before) {
            s.faces.insert(0, before);
        }
        s.toast = toast.clone();
        s.more += 1;
        s.more_at = Some(Instant::now());
        if let Some(life) = s.life {
            s.until = Some(deadline(s, life));
        }
        let hwnd = s.hwnd;
        b.refresh(hwnd);
        b.place();
        b.render(hwnd, 255, 0);
        b.pace(hwnd);
        Some(())
    })
    .flatten()
    .is_some()
}

/// Who wrote a notification, as a face: their photo, or their initials on a colour of their own.
fn face_of(t: &Toast) -> Face {
    if let Some(p) = &t.rich.avatar {
        return Face::Photo(p.key.clone(), p.bytes.clone());
    }
    let who = if t.title.is_empty() { &t.app } else { &t.title };
    let initials: String = who.split_whitespace().filter_map(|w| w.chars().find(|c| c.is_alphanumeric())).take(2).flat_map(char::to_uppercase).collect();
    // A colour of their own, the same each time.
    const SWATCH: [[u8; 3]; 6] = [[0xE0, 0x7B, 0x39], [0x5A, 0xA0, 0xE0], [0xB5, 0x5C, 0xC4], [0x3F, 0xBF, 0x7F], [0xE0, 0x5A, 0x7A], [0x8A, 0x7B, 0xE0]];
    let hash = who.bytes().fold(7u32, |h, c| h.wrapping_mul(31).wrapping_add(c as u32));
    Face::Initials(initials, SWATCH[hash as usize % SWATCH.len()])
}

/// Notifications held back by a quiet time: once it is over, a digest says what came.
fn hold_for_digest(ids: impl Iterator<Item = u32>) {
    with(|b| {
        b.held.extend(ids);
        if b.quiet_timer == 0 {
            b.quiet_timer = unsafe { SetTimer(None, 0, 2000, Some(quiet_over)) };
        }
    });
}

/// Every two seconds while notifications are held back: the quiet over, the digest shows.
unsafe extern "system" fn quiet_over(_: HWND, _: u32, timer: usize, _: u32) {
    let still = std::panic::catch_unwind(|| crate::calendar::focus::active() || busy()).unwrap_or(false);
    if still {
        return;
    }
    let _ = unsafe { KillTimer(None, timer) };
    let held = with(|b| {
        b.quiet_timer = 0;
        std::mem::take(&mut b.held)
    })
    .unwrap_or_default();
    let _ = std::panic::catch_unwind(|| digest(&held));
}

/// What came during a quiet time and is still waiting, in one banner: how many, from which apps,
/// the latest sender; its button opens the list.
fn digest(held: &[u32]) {
    let list = notifications::list();
    let waiting: Vec<&Toast> = list.iter().filter(|t| held.contains(&t.id)).collect();
    if waiting.is_empty() || super::OPEN.with(std::cell::Cell::get) != 0 {
        return;
    }
    let t = super::words::words();
    let mut apps: Vec<(String, usize)> = Vec::new();
    for toast in &waiting {
        match apps.iter_mut().find(|(a, _)| *a == toast.app) {
            Some((_, n)) => *n += 1,
            None => apps.push((toast.app.clone(), 1)),
        }
    }
    apps.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let by_app = apps.iter().take(3).map(|(a, n)| format!("{a} {n}")).collect::<Vec<_>>().join(", ");
    // Newest first in Windows' list.
    let latest = waiting.first().map(|w| if w.title.is_empty() { w.app.clone() } else { w.title.clone() }).unwrap_or_default();
    let body = format!("{} · {by_app}. {}", (t.away_count)(waiting.len()), (t.latest_from)(&latest));
    let Some(id) = with(|b| {
        b.next_own -= 1;
        b.next_own
    }) else {
        return;
    };
    let rich = rich::Rich { buttons: vec![rich::Action { label: t.show_them.to_string(), arguments: String::new(), kind: Kind::Foreground }], ..Default::default() };
    let toast = Toast { id, app: "Audio Tray".into(), aumid: crate::toast::AUMID.into(), title: t.away.to_string(), body, at: crate::calendar::time::now(), logo: None, rich };
    println!("banners: the digest, {} waiting", waiting.len());
    show(toast, Is::Digest, Some(lifetime() * 2), None);
    chime();
}

/// A glance of Audio Tray's own, never listed: a keyboard layout switched (the next replaces it).
pub fn glance(title: &str, body: &str) {
    // As quiet as a notification: not over Do not disturb, a full-screen app, or the open panel.
    if super::OPEN.with(std::cell::Cell::get) != 0 || crate::calendar::focus::active() || busy() {
        return;
    }
    // The one showing takes the new words.
    let replaced = with(|b| {
        let s = b.shown.iter_mut().find(|s| s.is == Is::Glance && !leaving(s))?;
        s.toast.title = title.to_string();
        s.toast.body = body.to_string();
        s.until = Some(deadline(s, GLANCE));
        s.life = Some(GLANCE);
        let hwnd = s.hwnd;
        b.refresh(hwnd);
        b.place();
        b.render(hwnd, 255, 0);
        Some(())
    })
    .flatten();
    if replaced.is_some() {
        return;
    }
    let Some(id) = with(|b| {
        b.next_own -= 1;
        b.next_own
    }) else {
        return;
    };
    let toast = Toast {
        id,
        app: "Audio Tray".into(),
        aumid: crate::toast::AUMID.into(),
        title: title.to_string(),
        body: body.to_string(),
        at: crate::calendar::time::now(),
        logo: None,
        rich: Default::default(),
    };
    show(toast, Is::Glance, Some(GLANCE), None);
}

/// Every banner goes at once (the panel opened, which lists them; Audio Tray no longer shows
/// them). A reply half typed goes to the panel's drafts: its box there opens with it.
pub fn clear() {
    let hwnds: Vec<HWND> = with(|b| {
        for s in b.shown.iter().filter(|s| s.draft.as_deref().is_some_and(|d| !d.is_empty())) {
            keep_draft(s.toast.id, s.draft.clone().unwrap_or_default());
        }
        b.shown.iter().map(|s| s.hwnd).collect()
    })
    .unwrap_or_default();
    for hwnd in hwnds {
        with(|b| leave(b, hwnd, Exit::Now));
    }
}

/// A reply typed and not sent, kept for the panel's box.
fn keep_draft(id: u32, text: String) {
    super::DRAFTS.with(|d| {
        let mut d = d.borrow_mut();
        d.retain(|(i, _)| *i != id);
        d.push((id, text));
    });
}

/// How long a banner stays: Windows' "Dismiss notifications after" (5 s unless changed).
fn lifetime() -> Duration {
    use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETMESSAGEDURATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS};
    let mut seconds = 0u32;
    let asked = unsafe { SystemParametersInfoW(SPI_GETMESSAGEDURATION, 0, Some(&mut seconds as *mut u32 as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) };
    Duration::from_secs(if asked.is_ok() { seconds.clamp(5, 300) } else { 5 } as u64)
}

/// Windows would not show a banner now: a full-screen game, a presentation, the screen locked.
fn busy() -> bool {
    use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_ACCEPTS_NOTIFICATIONS};
    unsafe { SHQueryUserNotificationState() }.is_ok_and(|state| state != QUNS_ACCEPTS_NOTIFICATIONS)
}

/// Windows' notification sound (its banners' own are off with them).
fn chime() {
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC, SND_NODEFAULT};
    let _ = unsafe { PlaySoundW(w!("Notification.Default"), None, SND_ALIAS | SND_ASYNC | SND_NODEFAULT) };
}

/// Windows' accent colour, read from the registry at most every two seconds (asked for each frame
/// and each listed notification).
fn accent() -> [u8; 3] {
    thread_local! {
        static ACCENT: std::cell::Cell<Option<(Instant, [u8; 3])>> = const { std::cell::Cell::new(None) };
    }
    ACCENT.with(|a| match a.get() {
        Some((at, rgb)) if at.elapsed() < Duration::from_secs(2) => rgb,
        _ => {
            let rgb = crate::flyout::theme::accent_rgb();
            a.set(Some((Instant::now(), rgb)));
            rgb
        }
    })
}

/// Banner `s`'s "when" words, worked out again only once a minute (they say minutes); `true`
/// when they changed (its drawing is then out of date).
fn update_when(s: &mut Shown) -> bool {
    let minute = crate::calendar::time::now() / 60;
    if s.when.0 == minute {
        return false;
    }
    let words = when_words(s.toast.at);
    let changed = words != s.when.1;
    s.when = (minute, words);
    changed
}

/// Motion is wanted (Windows' "Animation effects" is on).
fn animated() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS};
    let mut on = windows::core::BOOL(1);
    let asked = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) };
    asked.is_err() || on.as_bool()
}

/// An app's colour from its logo: the average of its saturated, bright pixels, weighted by how
/// saturated and bright they are; `None` for a grey logo. Lifted to read on the dark card.
fn tint_from_logo(bytes: &[u8]) -> Option<[u8; 3]> {
    let (rgba, _, _) = crate::image::fitted(bytes, 24, 24).ok()?;
    let (mut r, mut g, mut b, mut total) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for px in rgba.as_chunks::<4>().0 {
        if px[3] < 128 {
            continue;
        }
        let (pr, pg, pb) = (px[0] as f32 / 255.0, px[1] as f32 / 255.0, px[2] as f32 / 255.0);
        let max = pr.max(pg).max(pb);
        let min = pr.min(pg).min(pb);
        let saturation = if max > 0.0 { (max - min) / max } else { 0.0 };
        let weight = saturation * saturation * max;
        r += pr * weight;
        g += pg * weight;
        b += pb * weight;
        total += weight;
    }
    if total < 2.0 {
        return None;
    }
    let (r, g, b) = (r / total, g / total, b / total);
    // Bright enough to glow on the dark card.
    let max = r.max(g).max(b).max(0.01);
    let lift = (0.85 / max).max(1.0);
    let byte = |v: f32| ((v * lift).clamp(0.0, 1.0) * 255.0).round() as u8;
    Some([byte(r), byte(g), byte(b)])
}

/// How a banner leaves.
#[derive(Clone, Copy, PartialEq)]
enum Exit {
    /// Sideways, fading (dismissed, gone from the list, the oldest pushed out).
    Slide,
    /// Shrinking into the bell, which rings (its time ran out: it is in the list now).
    ToBell,
    /// At once (the panel opened over them).
    Now,
}

/// A banner on its way in or out, moved by its own frames (never in a loop: the others keep
/// moving meanwhile, and so does everything else on the tray thread).
#[derive(Clone, Copy, PartialEq)]
enum Motion {
    /// Springing in from the side.
    Enter,
    /// Sliding away, fading.
    Slide,
    /// Shrinking into the bell from its middle `from`: the bell, at `bell`, rings as it lands.
    ToBell { from: (i32, i32), bell: (i32, i32) },
}

impl Motion {
    fn length(self) -> Duration {
        Duration::from_millis(match self {
            Motion::Enter => 520,
            Motion::Slide => 180,
            Motion::ToBell { .. } => 380,
        })
    }
}

/// How long the others take to glide into the room a banner left (or out of a new one's way).
const GLIDE: Duration = Duration::from_millis(320);

/// Leaving: out of the stack already (the others close up), its window still moving.
fn leaving(s: &Shown) -> bool {
    matches!(s.motion, Some((Motion::Slide | Motion::ToBell { .. }, _)))
}

/// Springing in, `t` (0..1) of the way: how far from its place it is (pixels, toward the screen's
/// edge) and how faded.
fn enter_at(t: f32, scale: f32, edge: Edge) -> (i32, u8) {
    let toward = if edge == Edge::Left { -1.0 } else { 1.0 };
    let slide = crate::win::FLYOUT_SLIDE * scale * 1.6;
    let offset = (toward * slide * (1.0 - spring(t))).round() as i32;
    (offset, ((t * 4.0).min(1.0) * 255.0) as u8)
}

/// A spring settling at 1 from 0, past it a little first.
fn spring(t: f32) -> f32 {
    1.0 - (-7.0 * t).exp() * (10.0 * t).cos()
}

/// A banner's scene: as the notification centre draws its notification, in its app's colours, its
/// time and its picture moving; or for a glance Audio Tray's accent pill.
fn scene_of(s: &Shown) -> Scene {
    scene_at(s, time_left(s))
}

/// Banner `s`'s time left, 1 to 0 (`None`: it stays, or its line is not shown).
fn time_left(s: &Shown) -> Option<f32> {
    if s.sent.is_some() || s.draft.is_some() {
        return None;
    }
    left_at(s.until, s.life, s.held_since, Instant::now())
}

/// A banner's time left (1 to 0) at `now`, given when it goes, how long it was given, and since
/// when the pointer holds it: held, as of the moment the hold began (it stands exactly where it
/// stopped). `None`: it stays until answered (no end), or has no length.
fn left_at(until: Option<Instant>, life: Option<Duration>, held_since: Option<Instant>, now: Instant) -> Option<f32> {
    let (until, life) = (until?, life?);
    if life.is_zero() {
        return None;
    }
    let at = held_since.unwrap_or(now);
    Some(until.saturating_duration_since(at).as_secs_f32() / life.as_secs_f32())
}

/// When banner `s` goes, `after` from now: from the frozen clock while the pointer holds it (the
/// whole hold is added at its end, and must not be twice).
fn deadline(s: &Shown, after: Duration) -> Instant {
    s.held_since.unwrap_or_else(Instant::now) + after
}

/// When a banner held since `since` goes once the hold ends at `now`: its end moves on by the
/// whole hold, so its time resumes where it stopped.
fn released(until: Instant, since: Instant, now: Instant) -> Instant {
    until + now.saturating_duration_since(since)
}

/// [`scene_of`] with its time line at `left`.
fn scene_at(s: &Shown, left: Option<f32>) -> Scene {
    let t = &s.toast;
    let now = Instant::now();
    if s.is == Is::Glance {
        return scene::glance(GLYPH_KEYBOARD, &t.title, &t.body, left.unwrap_or(0.0), s.hover.is_some());
    }
    let words = super::words::words();
    let age = now.duration_since(s.born).as_secs_f32();
    let moving = (age / ART_FOR.as_secs_f32()).min(1.0);
    let own = Own::of(t);
    let mut body = t.body.clone();
    let (art, tint) = match (s.is, own) {
        (Is::Digest, _) => (Art::Glyph { glyph: '\u{E708}', rgb: s.tint.unwrap_or([0xB5, 0x5C, 0xC4]) }, s.tint),
        (_, Own::Battery(level)) => {
            // Draining from a little fuller down to its level.
            let to = level as f32 / 100.0;
            let from = (to + 0.35).min(1.0);
            let eased = 1.0 - (1.0 - moving).powi(3);
            (Art::Battery { level: from + (to - from) * eased, low: level <= 15 }, Some(if level <= 15 { RED } else { AMBER }))
        }
        (_, Own::Audio) => {
            let playing = 1.0 - moving;
            let bars = std::array::from_fn(|n| 0.35 + playing * 0.6 * ((age * 9.0 + n as f32 * 1.3).sin() * 0.5 + 0.5));
            // The speaker is in the accent: the banner with it.
            (Art::Speaker { bars }, Some(accent()))
        }
        (_, Own::Meeting(start)) => {
            // Counting down to it, then how long ago it began.
            let to = start - crate::calendar::time::now();
            body = if to > 0 { (words.starts_in)(to) } else { (words.started)((-to) / 60) };
            (Art::Glyph { glyph: '\u{E787}', rgb: BLUE }, Some(BLUE))
        }
        (_, Own::Focus) => (Art::Glyph { glyph: '\u{E73E}', rgb: GREEN }, Some(GREEN)),
        _ if s.more > 0 => {
            let mut faces = vec![face_of(t)];
            faces.extend(s.faces.iter().cloned());
            (Art::Faces(faces), s.tint)
        }
        // A call without the caller's photo: a phone on its colour, the rings around it.
        _ if t.rich.call && t.rich.avatar.is_none() => (Art::Glyph { glyph: '\u{E717}', rgb: s.tint.unwrap_or(GREEN) }, s.tint),
        _ => (Art::None, s.tint),
    };
    let more_pop = match s.more_at {
        Some(at) if now.duration_since(at) < POP_FOR => 1.0 + 0.3 * (1.0 - now.duration_since(at).as_secs_f32() / POP_FOR.as_secs_f32()),
        _ => 1.0,
    };
    // Confetti for a focus session done: falling and fading through the card's first seconds.
    let confetti = if own == Own::Focus && moving < 1.0 {
        (0..26u32)
            .map(|n| {
                let seed = n.wrapping_mul(2654435761) ^ t.id;
                let x = (seed % 360) as f32 + 10.0;
                let speed = 60.0 + (seed >> 9) as f32 % 90.0;
                let delay = (seed >> 3) as f32 % 0.5;
                let fall = (age - delay).max(0.0);
                let y = -10.0 + fall * speed;
                let colours = [[0xD8, 0x8D, 0xE0], [0x25, 0xD3, 0x66], AMBER, BLUE, RED];
                let alpha = ((1.0 - fall / 2.0).clamp(0.0, 1.0) * 255.0) as u8;
                (x, y, colours[n as usize % colours.len()], if fall > 0.0 { alpha } else { 0 })
            })
            .collect()
    } else {
        Vec::new()
    };
    let call = t.rich.call;
    let pulse = call.then(|| (age / 1.6) % 1.0);
    scene::banner(&scene::Banner {
        app: t.app.clone(),
        logo: t.logo.clone().map(|l| (t.aumid.clone(), l)),
        when: if s.when.1.is_empty() { when_words(t.at) } else { s.when.1.clone() },
        toast: scene::Toast {
            index: 0,
            title: if t.title.is_empty() { t.app.clone() } else { t.title.clone() },
            when: String::new(),
            body,
            avatar: t.rich.avatar.as_ref().map(|p| (p.key.clone(), p.bytes.clone())),
            picture: t.rich.picture.as_ref().map(|p| (p.key.clone(), p.bytes.clone())),
            buttons: t.rich.buttons.iter().map(|a| a.label.clone()).collect(),
            reply: t.rich.reply.as_ref().map(|r| if r.placeholder.is_empty() { words.reply.to_string() } else { r.placeholder.clone() }),
            // A banner has no room for a menu: the list has it.
            menu: Vec::new(),
            draft: s.draft.clone(),
            menu_open: false,
            collapse: 1.0,
            quick: if t.rich.reply.is_some() && !call { QUICK.iter().map(|q| q.to_string()).collect() } else { Vec::new() },
            sent: s.sent.clone(),
            call,
            art,
            // The card itself wears it.
            tint: None,
        },
        hover: s.hover,
        pressed: s.pressed,
        tint,
        left,
        paused: s.paused,
        more: s.more,
        more_pop,
        pulse,
        confetti,
    })
}

/// A notification's look in the panel's list, as its banner has it at rest: its picture (Audio
/// Tray's own illustrated, a call's phone) and its app's colour.
pub(super) fn list_look(t: &Toast) -> (Art, Option<[u8; 3]>) {
    let accent = accent();
    match Own::of(t) {
        Own::Battery(level) => {
            let low = level <= 15;
            (Art::Battery { level: level as f32 / 100.0, low }, Some(if low { RED } else { AMBER }))
        }
        Own::Audio => (Art::Speaker { bars: [0.5, 0.9, 0.65, 0.4] }, Some(accent)),
        Own::Meeting(_) => (Art::Glyph { glyph: '\u{E787}', rgb: BLUE }, Some(BLUE)),
        Own::Focus => (Art::Glyph { glyph: '\u{E73E}', rgb: GREEN }, Some(GREEN)),
        Own::No => {
            let tint = with(|b| b.tint_of(t)).flatten().or(Some(accent));
            if t.rich.call && t.rich.avatar.is_none() {
                (Art::Glyph { glyph: '\u{E717}', rgb: tint.unwrap_or(GREEN) }, tint)
            } else {
                (Art::None, tint)
            }
        }
    }
}

/// Something moves on banner `s` (its picture, a call's rings, its count growing): drawn smoothly.
fn lively(s: &Shown) -> bool {
    let now = Instant::now();
    s.is == Is::Glance
        || s.toast.rich.call
        || (Own::of(&s.toast) != Own::No && now.duration_since(s.born) < ART_FOR)
        || s.more_at.is_some_and(|at| now.duration_since(at) < POP_FOR)
}

/// A new banner, springing in at the corner.
fn show(toast: Toast, is: Is, life: Option<Duration>, chime_at: Option<Instant>) {
    let style = WINDOW_EX_STYLE(WS_EX_LAYERED.0 | WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0 | WS_EX_NOACTIVATE.0);
    // Its words as its window's name, for assistive tools reading the screen.
    let name = windows::core::HSTRING::from(format!("{}: {}. {}", toast.app, toast.title, toast.body));
    let hwnd = match crate::win::create_popup(CLASS, PCWSTR(name.as_ptr()), Some(wndproc), style, (0, 0, 1, 1)) {
        Ok(hwnd) => hwnd,
        Err(e) => {
            eprintln!("banners: could not make one ({e})");
            return;
        }
    };
    // A notification's card over Windows' acrylic; a glance draws its own pill, solid.
    if is != Is::Glance {
        unsafe { crate::layered::style_panel(hwnd, crate::layered::CORNER_ROUND) };
    }
    // Over the oldest's room, a fourth sends it to the list: the oldest that would go anyway, never
    // a call or a reminder for something less urgent (unless all of them are).
    let oldest = with(|b| {
        let staying: Vec<&Shown> = b.shown.iter().filter(|s| !leaving(s)).collect();
        (staying.len() >= MAX).then(|| staying.iter().find(|s| s.until.is_some() && s.draft.is_none()).unwrap_or(&staying[0]).hwnd)
    })
    .flatten();
    if let Some(old) = oldest {
        with(|b| leave(b, old, Exit::ToBell));
    }
    let made = with(|b| {
        // Where they go, and how they look, read afresh for each: the primary display's corner by
        // the taskbar, as Windows' own. A change (the scale, the taskbar moved, the theme) moves
        // and redraws those showing too.
        let (scale, rect, work) = monitor(POINT { x: 0, y: 0 });
        crate::flyout::theme::refresh_mode();
        let palette = Palette::current();
        let changed = (b.scale, b.work, b.edge) != (scale, work, edge_of(&rect, &work)) || b.palette.as_ref().is_none_or(|p| p.bg != palette.bg);
        b.scale = scale;
        b.work = work;
        b.edge = edge_of(&rect, &work);
        b.palette = Some(palette);
        let gfx = match Gfx::new() {
            Ok(gfx) => gfx,
            Err(e) => {
                eprintln!("banners: no drawing kit ({e:#})");
                return false;
            }
        };
        let tint = match is {
            Is::Glance => None,
            Is::Digest => Some(accent()),
            Is::Listed => b.tint_of(&toast),
        };
        let mut s = Shown {
            hwnd,
            toast,
            is,
            until: life.map(|l| Instant::now() + l),
            life,
            born: Instant::now(),
            held_since: None,
            hover: None,
            pressed: None,
            draft: None,
            sent: None,
            chime_at,
            faces: Vec::new(),
            more: 0,
            more_at: None,
            tint,
            paused: false,
            line: None,
            scene: scene::glance(' ', "", "", 0.0, false),
            gfx,
            canvas: crate::layered::Canvas::new(),
            at: (0, 0),
            pos: (0, 0),
            glide: None,
            motion: None,
            frame_ms: 0,
            when: (i64::MIN, String::new()),
        };
        s.scene = scene_of(&s);
        if changed {
            // Those staying put at their new places (a glide from the old screen's coordinates
            // would set off from nowhere); those leaving go on their way untouched.
            for s in b.shown.iter_mut().filter(|s| !leaving(s)) {
                s.at = (0, 0);
                s.glide = None;
            }
            let all: Vec<HWND> = b.shown.iter().filter(|s| !leaving(s)).map(|s| s.hwnd).collect();
            for other in all {
                b.refresh(other);
            }
        }
        b.shown.push(s);
        // The stack stays on screen: the oldest go to the list while it would run past the top.
        while b.shown.iter().filter(|s| !leaving(s)).count() > 1 && b.stack_height() > b.work.bottom - b.work.top - 2 * b.px(MARGIN) {
            let Some(old) = b.shown.iter().find(|s| !leaving(s)).map(|s| s.hwnd) else { break };
            leave(b, old, Exit::Now);
        }
        b.place();
        if changed {
            let others: Vec<HWND> = b.shown.iter().filter(|s| !leaving(s)).map(|s| s.hwnd).filter(|h| *h != hwnd).collect();
            for other in others {
                b.render(other, 255, 0);
            }
        }
        // Its first frame in the window, unseen, before it shows; then it springs in, past its
        // place a little and settled, moved by its own frames (drawn once: only moved).
        let enter = animated();
        b.render(hwnd, if enter { 0 } else { 255 }, 0);
        if let Some(s) = b.shown.iter_mut().find(|s| s.hwnd == hwnd) {
            s.motion = enter.then(|| (Motion::Enter, Instant::now()));
        }
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNA);
            SetTimer(Some(hwnd), LIFE_TIMER, LIFE_MS, None);
        }
        b.pace(hwnd);
        true
    });
    if made != Some(true) {
        // Never shown: its window goes with it.
        let _ = unsafe { DestroyWindow(hwnd) };
    }
}

/// Banner `hwnd` goes as `exit` says; the others close up. The keyboard goes back to where it was
/// if its reply box had it.
fn leave(b: &mut Banners, hwnd: HWND, exit: Exit) {
    let Some(at) = b.shown.iter().position(|s| s.hwnd == hwnd) else { return };
    if b.shown[at].draft.is_some() {
        // A reply half typed is kept for the panel's box, however the banner goes.
        if let Some(text) = b.shown[at].draft.clone().filter(|d| !d.is_empty()) {
            keep_draft(b.shown[at].toast.id, text);
        }
        give_keys_back(b, hwnd);
    }
    let exit = if animated() { exit } else { Exit::Now };
    // It goes from where it is: still springing in, from where the spring has it.
    let (scale, edge) = (b.scale, b.edge);
    if let Some((Motion::Enter, since)) = b.shown[at].motion {
        let (dx, _) = enter_at((since.elapsed().as_secs_f32() / Motion::Enter.length().as_secs_f32()).min(1.0), scale, edge);
        b.shown[at].pos.0 += dx;
    }
    let motion = match exit {
        Exit::Now => {
            finish(b, hwnd);
            return;
        }
        Exit::Slide => Motion::Slide,
        Exit::ToBell => {
            let s = &b.shown[at];
            let from = (s.pos.0 + b.px(s.scene.w) / 2, s.pos.1 + b.px(s.scene.h) / 2);
            // The bell where measured, else the taskbar's corner.
            let bell = crate::tray::bell_point().map_or((b.work.right - b.px(40.0), b.work.bottom + b.px(24.0)), |p| (p.x, p.y));
            Motion::ToBell { from, bell }
        }
    };
    // Shrinking within a window of its full size: the blur behind would stay that size.
    if matches!(motion, Motion::ToBell { .. }) {
        unsafe { crate::layered::set_acrylic(hwnd, false) };
    }
    // Going: the pointer passes through it to what is under it.
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_TRANSPARENT};
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_TRANSPARENT.0 as isize);
    }
    // Out of the stack at once (the others glide into its room) while it goes on its way.
    let s = &mut b.shown[at];
    s.motion = Some((motion, Instant::now()));
    s.glide = None;
    s.hover = None;
    unsafe {
        let _ = KillTimer(Some(hwnd), LIFE_TIMER);
    }
    b.place();
    b.pace(hwnd);
}

/// Banner `hwnd` gone for good: its window, and from the stack.
fn finish(b: &mut Banners, hwnd: HWND) {
    let Some(at) = b.shown.iter().position(|s| s.hwnd == hwnd) else { return };
    let gone = b.shown.remove(at);
    unsafe {
        let _ = KillTimer(Some(gone.hwnd), LIFE_TIMER);
        let _ = KillTimer(Some(gone.hwnd), FRAME_TIMER);
        let _ = DestroyWindow(gone.hwnd);
    }
    let landed = matches!(gone.motion, Some((Motion::ToBell { .. }, _)));
    drop(gone);
    b.place();
    if landed {
        crate::taskbar::ring_bell();
    }
}

/// One frame of banner `hwnd`'s way in or out (once over, leaving, it is gone).
fn step_motion(b: &mut Banners, hwnd: HWND) {
    let Some(s) = b.shown.iter_mut().find(|s| s.hwnd == hwnd) else { return };
    let Some((motion, since)) = s.motion else { return };
    let t = (since.elapsed().as_secs_f32() / motion.length().as_secs_f32()).min(1.0);
    let toward = if b.edge == Edge::Left { -1.0 } else { 1.0 };
    match motion {
        Motion::Enter => {
            // Still springing in when the stack moves (another arrived): it glides meanwhile too,
            // the spring on top, so it is never left to jump there once the spring ends.
            advance_glide(s);
            let (offset, alpha) = enter_at(t, b.scale, b.edge);
            crate::layered::Canvas::shift(hwnd, s.pos.0 + offset, s.pos.1, alpha);
            if t >= 1.0 {
                s.motion = None;
                crate::layered::Canvas::shift(hwnd, s.pos.0, s.pos.1, 255);
            }
        }
        Motion::Slide => {
            let slide = crate::win::FLYOUT_SLIDE * b.scale;
            let e = t * t;
            crate::layered::Canvas::shift(hwnd, s.pos.0 + (toward * slide * e).round() as i32, s.pos.1, ((1.0 - e) * 255.0) as u8);
            if t >= 1.0 {
                finish(b, hwnd);
            }
        }
        Motion::ToBell { from, bell } => {
            let k = t * t;
            let shrink = 1.0 - 0.85 * k;
            let centre = ((from.0 as f32 + (bell.0 - from.0) as f32 * k).round() as i32, (from.1 as f32 + (bell.1 - from.1) as f32 * k).round() as i32);
            b.render_zoomed(hwnd, shrink, centre, ((1.0 - k) * 255.0) as u8);
            if t >= 1.0 {
                finish(b, hwnd);
            }
        }
    }
}

/// One frame of banner `hwnd`'s glide to its place in the stack (eased out), only moved.
fn step_glide(s: &mut Shown) {
    if advance_glide(s) {
        crate::layered::Canvas::shift(s.hwnd, s.pos.0, s.pos.1, 255);
    }
}

/// Banner `s`'s place on its glide this frame (`pos`), not yet shown; `false`: not gliding. Eased
/// in and out (a glide that starts at full speed shows its first frames as a jump), its clock
/// started at its first frame.
fn advance_glide(s: &mut Shown) -> bool {
    let Some((since, from)) = s.glide else { return false };
    let since = since.unwrap_or_else(Instant::now);
    s.glide = Some((Some(since), from));
    let t = (since.elapsed().as_secs_f32() / GLIDE.as_secs_f32()).min(1.0);
    let e = glide_ease(t);
    s.pos = ((from.0 as f32 + (s.at.0 - from.0) as f32 * e).round() as i32, (from.1 as f32 + (s.at.1 - from.1) as f32 * e).round() as i32);
    if t >= 1.0 {
        s.pos = s.at;
        s.glide = None;
    }
    true
}

/// A glide's ease: slow out of its start, slow into its end, at most 1.5 times its mean speed
/// (smoothstep: a cubic in-and-out peaks at 3 times, a jump at a frame that comes late).
fn glide_ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Banners {
    fn px(&self, dip: f32) -> i32 {
        (dip * self.scale).round() as i32
    }

    /// App `toast`'s colour, from its logo, worked out once per app.
    fn tint_of(&mut self, toast: &Toast) -> Option<[u8; 3]> {
        if let Some((_, tint)) = self.tints.iter().find(|(a, _)| *a == toast.aumid) {
            return *tint;
        }
        let tint = toast.logo.as_ref().and_then(|l| tint_from_logo(l)).or(Some(accent()));
        self.tints.push((toast.aumid.clone(), tint));
        tint
    }

    /// Banner `hwnd`'s scene, built afresh (its state changed).
    fn refresh(&mut self, hwnd: HWND) {
        if let Some(s) = self.shown.iter_mut().find(|s| s.hwnd == hwnd) {
            update_when(s);
            s.scene = scene_of(s);
            s.line = None;
        }
    }

    /// Banner `hwnd`'s frames, as often as what is on it moves.
    fn pace(&mut self, hwnd: HWND) {
        let held = self.pointer_on_any();
        let Some(s) = self.shown.iter_mut().find(|s| s.hwnd == hwnd) else { return };
        let ms = if s.motion.is_some() || s.glide.is_some() {
            FRAME_MS
        } else if s.toast.rich.call {
            FRAME_CALL_MS
        } else if lively(s) {
            FRAME_MS
        } else if matches!(Own::of(&s.toast), Own::Meeting(_)) {
            // Its countdown says seconds.
            FRAME_SECOND_MS
        } else if time_left(s).is_some() && !held {
            FRAME_MS
        } else {
            FRAME_STILL_MS
        };
        // Set again only when it changes: setting it restarts its count.
        if ms != s.frame_ms {
            s.frame_ms = ms;
            unsafe { SetTimer(Some(hwnd), FRAME_TIMER, ms, None) };
        }
    }

    /// Each banner's place, the newest nearest the corner (those leaving have none): a new one is
    /// put there, a moved one glides there.
    fn place(&mut self) {
        let (m, gap) = (self.px(MARGIN), self.px(GAP));
        let mut offset = 0;
        let mut moved = Vec::new();
        for i in (0..self.shown.len()).rev() {
            if leaving(&self.shown[i]) {
                continue;
            }
            let (w, h) = (self.px(self.shown[i].scene.w), self.px(self.shown[i].scene.h));
            let work = self.work;
            let x = if self.edge == Edge::Left { work.left + m } else { work.right - w - m };
            let y = if self.edge == Edge::Top { work.top + m + offset } else { work.bottom - h - m - offset };
            offset += h + gap;
            let s = &mut self.shown[i];
            let was = s.at;
            s.at = (x, y);
            if was == (0, 0) {
                s.pos = s.at;
            } else if was != s.at {
                s.glide = Some((None, s.pos));
                moved.push(s.hwnd);
            }
        }
        for hwnd in moved {
            self.pace(hwnd);
        }
    }

    /// Draw banner `hwnd` and put it on screen where its window is, `dx` from there, `alpha` faded.
    fn render(&mut self, hwnd: HWND, alpha: u8, dx: i32) {
        let Some(s) = self.shown.iter().find(|s| s.hwnd == hwnd) else { return };
        // Leaving, it is drawn by its own motion only (never whole at its place mid-flight).
        if leaving(s) {
            return;
        }
        // Springing in, drawn where the spring has it and as faded (never popping in whole).
        let (dx, alpha) = match s.motion {
            Some((Motion::Enter, since)) => enter_at((since.elapsed().as_secs_f32() / Motion::Enter.length().as_secs_f32()).min(1.0), self.scale, self.edge),
            _ => (dx, alpha),
        };
        let at = (s.pos.0 + dx, s.pos.1);
        let scale = self.scale;
        self.draw_at(hwnd, scale, at, alpha);
    }

    /// Draw banner `hwnd` `zoom` times its size (smaller as it flies into the bell), centred on
    /// `centre`. Its frame keeps its full size (only what is drawn in it shrinks): its surface, its
    /// decoded pictures and its window's bitmap stay as they are.
    fn render_zoomed(&mut self, hwnd: HWND, zoom: f32, centre: (i32, i32), alpha: u8) {
        let scale = self.scale;
        let Some(palette) = self.palette.as_ref() else { return };
        let Some(s) = self.shown.iter_mut().find(|s| s.hwnd == hwnd) else { return };
        let Shown { gfx, scene, canvas, .. } = s;
        let size = (((scene.w * scale).round() as u32).max(1), ((scene.h * scale).round() as u32).max(1));
        if gfx.frame_zoomed(size, scale, zoom, None, |pen, area| draw(pen, scene, palette, area)).is_err() {
            return;
        }
        let at = (centre.0 - size.0 as i32 / 2, centre.1 - size.1 as i32 / 2);
        let pixels = &gfx.pixels;
        canvas.present(hwnd, (size.0 as i32, size.1 as i32), at, alpha, |dst| {
            let n = dst.len().min(pixels.len());
            dst[..n].copy_from_slice(&pixels[..n]);
        });
    }

    fn draw_at(&mut self, hwnd: HWND, scale: f32, at: (i32, i32), alpha: u8) {
        let Some(palette) = self.palette.as_ref() else { return };
        let Some(s) = self.shown.iter_mut().find(|s| s.hwnd == hwnd) else { return };
        let Shown { gfx, scene, canvas, .. } = s;
        let size = (((scene.w * scale).round() as u32).max(1), ((scene.h * scale).round() as u32).max(1));
        if let Err(e) = gfx.frame_in(size, scale, None, |pen, area| draw(pen, scene, palette, area)) {
            eprintln!("banners: could not draw ({e:#})");
            return;
        }
        let pixels = &gfx.pixels;
        canvas.present(hwnd, (size.0 as i32, size.1 as i32), at, alpha, |dst| {
            let n = dst.len().min(pixels.len());
            dst[..n].copy_from_slice(&pixels[..n]);
        });
    }

    /// A frame where only banner `hwnd`'s time line moves: its rows composed from its two drawings
    /// (drawn once, again only when something else on it changed), written into the window's
    /// bitmap and only they sent to Windows; nothing at all when the line has not moved a pixel.
    fn line_frame(&mut self, hwnd: HWND, left: f32) {
        let scale = self.scale;
        let Some(palette) = self.palette.as_ref() else { return };
        let Some(s) = self.shown.iter_mut().find(|s| s.hwnd == hwnd) else { return };
        if s.line.is_none() {
            let full = scene_at(s, Some(1.0));
            let empty = scene_at(s, Some(0.0));
            let size = (((full.w * scale).round() as u32).max(1), ((full.h * scale).round() as u32).max(1));
            let px = |dip: f32| (dip * scale).round() as i32;
            let (w, h) = (size.0 as i32, size.1 as i32);
            // The line as scene::banner lays it: 20 DIPs in from each side, 5 up from the foot,
            // 2 high; a pixel more each way for its rounded ends' blending.
            let rows = (h - px(5.0) - 1).max(0)..(h - px(3.0) + 1).min(h);
            let columns = (px(20.0) - 1).max(0)..(w - px(20.0) + 1).min(w);
            let row = w as usize * 4;
            let (from, to) = (rows.start as usize * row, rows.end as usize * row);
            // The empty one last: its whole frame primes the window's bitmap (all but the line is
            // the same in both), then only the line's rows are kept of each.
            if s.gfx.frame_in(size, scale, None, |pen, area| draw(pen, &full, palette, area)).is_err() {
                return;
            }
            let full_rows = s.gfx.pixels.get(from..to).map(<[u8]>::to_vec).unwrap_or_default();
            if s.gfx.frame_in(size, scale, None, |pen, area| draw(pen, &empty, palette, area)).is_err() {
                return;
            }
            let empty_rows = s.gfx.pixels.get(from..to).map(<[u8]>::to_vec).unwrap_or_default();
            // The first frame whole, its line already cut where it is now (sent empty first, the
            // line could flash out for a frame).
            let cut = columns.start + ((columns.end - columns.start) as f32 * left.clamp(0.0, 1.0)).round() as i32;
            let pixels = &s.gfx.pixels;
            let (rows_now, cols) = (rows.clone(), columns.clone());
            s.canvas.present(hwnd, (w, h), s.pos, 255, |dst| {
                let n = dst.len().min(pixels.len());
                dst[..n].copy_from_slice(&pixels[..n]);
                for r in rows_now {
                    let (from, at) = (r as usize * row + cols.start as usize * 4, r as usize * row + cut.max(cols.start) as usize * 4);
                    if at <= n && at - from <= full_rows.len() {
                        let base = rows.start as usize * row;
                        dst[from..at].copy_from_slice(&full_rows[from - base..at - base]);
                    }
                }
            });
            s.line = Some(Line { full: full_rows, empty: empty_rows, size: (w, h), rows, columns, cut });
            return;
        }
        let Some(line) = s.line.as_mut() else { return };
        let cut = line.columns.start + ((line.columns.end - line.columns.start) as f32 * left.clamp(0.0, 1.0)).round() as i32;
        if cut == line.cut {
            return;
        }
        line.cut = cut;
        let line = &*line;
        let (w, h) = line.size;
        let row = w as usize * 4;
        let base = line.rows.start as usize * row;
        let sent = s.canvas.present_rows(hwnd, (w, h), s.pos, line.rows.clone(), |dst| {
            for r in line.rows.clone() {
                let (from, to) = (r as usize * row + line.columns.start as usize * 4, r as usize * row + line.columns.end as usize * 4);
                let at = r as usize * row + cut.max(line.columns.start) as usize * 4;
                if to <= dst.len() && to - base <= line.full.len() {
                    dst[from..at].copy_from_slice(&line.full[from - base..at - base]);
                    dst[at..to].copy_from_slice(&line.empty[at - base..to - base]);
                }
            }
        });
        if !sent {
            // The window's bitmap is another size (drawn whole meanwhile): drawn again.
            s.line = None;
        }
    }

    /// The pointer holds the banners or not, all of them alike: a hold begins (each one's time
    /// stops where it is) or ends (each one's end moves on by the whole hold, so its line resumes
    /// exactly where it stopped). Returns whether they are held.
    fn hold(&mut self) -> bool {
        let held = self.pointer_on_any();
        let now = Instant::now();
        let mut flipped = Vec::new();
        for s in self.shown.iter_mut().filter(|s| !leaving(s)) {
            match (held, s.held_since) {
                (true, None) => {
                    s.held_since = Some(now);
                    s.paused = true;
                    s.line = None;
                    flipped.push(s.hwnd);
                }
                (false, Some(since)) => {
                    if let Some(until) = s.until.as_mut() {
                        *until = released(*until, since, now);
                    }
                    s.held_since = None;
                    s.paused = false;
                    s.line = None;
                    flipped.push(s.hwnd);
                }
                _ => {}
            }
        }
        // Each one's frames at their new pace at once (a line resuming waits for no slow tick).
        for hwnd in flipped {
            self.pace(hwnd);
        }
        held
    }

    /// The stack's height on screen, in pixels, gaps included.
    fn stack_height(&self) -> i32 {
        let gap = self.px(GAP);
        self.shown.iter().filter(|s| !leaving(s)).map(|s| self.px(s.scene.h) + gap).sum::<i32>() - gap
    }

    /// The pointer is over a banner: all of them stay.
    fn pointer_on_any(&self) -> bool {
        // Dev: the simulation says where the pointer is, never the real one.
        #[cfg(feature = "dev")]
        if let Some(held) = SIM_HOLD.with(std::cell::Cell::get) {
            return held;
        }
        let p = cursor();
        self.shown.iter().filter(|s| !leaving(s)).any(|s| {
            let (w, h) = (self.px(s.scene.w), self.px(s.scene.h));
            p.x >= s.pos.0 && p.x < s.pos.0 + w && p.y >= s.pos.1 && p.y < s.pos.1 + h
        })
    }
}

/// What a click on `hit` in banner `hwnd` does: the banner goes, and its notification is opened
/// or its button carried out as the list would.
fn click(b: &mut Banners, hwnd: HWND, hit: Hit) -> Option<After> {
    let s = b.shown.iter_mut().find(|s| s.hwnd == hwnd)?;
    // On its way out, or its reply sent: nothing more to do on it.
    if s.sent.is_some() || leaving(s) {
        return None;
    }
    let toast = s.toast.clone();
    let is = s.is;
    let after: Option<After> = match (is, hit) {
        // The digest opens the list it sums up.
        (Is::Digest, Hit::Toast(_) | Hit::Action(..)) => {
            let config = b.config.clone()?;
            Some(Box::new(move || crate::calendar::open(&config, crate::calendar::Panel::Notifications)))
        }
        // The reply box opens in place, and the banner takes the keyboard (the click on it lets
        // it): it stays until sent or dismissed.
        (Is::Listed, Hit::Reply(_)) if toast.rich.reply.is_some() => {
            s.draft = Some(String::new());
            s.until = None;
            b.refresh(hwnd);
            b.place();
            b.render(hwnd, 255, 0);
            return Some(Box::new(move || take_keys(hwnd)));
        }
        // Its field: the keyboard again (taken elsewhere meanwhile).
        (_, Hit::Draft(_)) => return Some(Box::new(move || take_keys(hwnd))),
        (Is::Listed, Hit::Send(_)) => {
            let text = s.draft.clone().unwrap_or_default();
            if text.trim().is_empty() {
                return None;
            }
            return send(b, hwnd, text, String::new());
        }
        // A reaction: sent as the reply, at once.
        (Is::Listed, Hit::Quick(_, n)) => {
            let emoji = QUICK.get(n)?.to_string();
            return send(b, hwnd, emoji.clone(), emoji);
        }
        (Is::Listed, Hit::Toast(_)) => {
            // Audio Tray's own without a link of their own have nothing to open: to the list.
            if toast.aumid == crate::toast::AUMID && toast.rich.launch.is_empty() {
                None
            } else {
                Some(Box::new(move || {
                    notifications::hold(toast.id);
                    let rich = &toast.rich;
                    if rich.launch.is_empty() && rich.launch_kind != Kind::Protocol {
                        notifications::open_app(&toast.aumid);
                    } else {
                        rich::invoke(toast.id, &toast.aumid, rich.activator, &rich.launch, rich.launch_kind, Vec::new());
                    }
                }))
            }
        }
        (Is::Listed, Hit::Action(_, n)) => {
            let action = toast.rich.buttons.get(n)?.clone();
            Some(Box::new(move || {
                notifications::hold(toast.id);
                rich::invoke(toast.id, &toast.aumid, toast.rich.activator, &action.arguments, action.kind, Vec::new());
            }))
        }
        // Its X: to the list only, as Windows' banners do.
        (_, Hit::Toast(_) | Hit::Dismiss(_)) => None,
        _ => return None,
    };
    leave(b, hwnd, Exit::Slide);
    after
}

/// Send `text` as banner `hwnd`'s reply: "Sent" (with `emoji` for a reaction) shows a moment where
/// its buttons were, then it goes. The keyboard goes back to where it was.
fn send(b: &mut Banners, hwnd: HWND, text: String, emoji: String) -> Option<After> {
    let s = b.shown.iter_mut().find(|s| s.hwnd == hwnd)?;
    let toast = s.toast.clone();
    let reply = toast.rich.reply.clone()?;
    let word = super::words::words().sent;
    s.sent = Some(if emoji.is_empty() { format!("✓ {word}") } else { format!("✓ {word} {emoji}") });
    let had_keys = s.draft.take().is_some();
    s.until = Some(deadline(s, SENT_FOR));
    s.life = None;
    b.refresh(hwnd);
    b.render(hwnd, 255, 0);
    if had_keys {
        give_keys_back(b, hwnd);
    }
    Some(Box::new(move || {
        notifications::hold(toast.id);
        rich::invoke(toast.id, &toast.aumid, toast.rich.activator, &reply.send.arguments, reply.send.kind, vec![(reply.input, text)]);
    }))
}

/// Banner `hwnd` takes the keyboard for its reply box: it may now be activated (a click on it
/// granted the foreground to this process).
fn take_keys(hwnd: HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE};
    // Where the keyboard was: it goes back there when the reply is sent or given up.
    let before = unsafe { GetForegroundWindow() };
    with(|b| {
        if before != hwnd && !before.is_invalid() && b.shown.iter().all(|s| s.hwnd != before) {
            b.keys_from = Some(before);
        }
    });
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style & !(WS_EX_NOACTIVATE.0 as isize));
    }
    let raised = crate::music::player::raise(hwnd);
    let _ = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(hwnd)) };
    println!("banners: replying ({raised:?})");
}

/// Banner `hwnd`'s reply box is done with (sent, given up, gone): it never takes the foreground
/// again, and the window that had the keyboard before gets it back.
fn give_keys_back(b: &mut Banners, hwnd: HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowLongPtrW, IsWindow, SetForegroundWindow, SetWindowLongPtrW, GWL_EXSTYLE};
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_NOACTIVATE.0 as isize);
    }
    // Only while the banner still has it: the user may have gone elsewhere meanwhile.
    if let Some(before) = b.keys_from.take() {
        if unsafe { GetForegroundWindow() } == hwnd && unsafe { IsWindow(Some(before)) }.as_bool() {
            let _ = unsafe { SetForegroundWindow(before) };
        }
    }
}

/// A key while banner `hwnd`'s reply box is open: typing, Enter sends, Escape closes the box.
fn key(b: &mut Banners, hwnd: HWND, c: char) -> Option<After> {
    let s = b.shown.iter_mut().find(|s| s.hwnd == hwnd)?;
    let draft = s.draft.as_mut()?;
    match c {
        '\r' | '\n' => return click(b, hwnd, Hit::Send(0)),
        '\u{1b}' => {
            s.draft = None;
            s.until = Some(deadline(s, lifetime()));
            b.refresh(hwnd);
            b.render(hwnd, 255, 0);
            give_keys_back(b, hwnd);
            return None;
        }
        '\u{8}' => {
            draft.pop();
        }
        // Ctrl+Backspace: the last word.
        '\u{7f}' => {
            let kept = draft.trim_end().rfind(' ').map_or(0, |i| i + 1);
            draft.truncate(kept);
        }
        // Ctrl+V.
        '\u{16}' => draft.push_str(&crate::win::clipboard_text().ok().flatten().unwrap_or_default().replace(['\r', '\n'], " ")),
        c if !c.is_control() => draft.push(c),
        _ => return None,
    }
    b.refresh(hwnd);
    b.render(hwnd, 255, 0);
    None
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // A panic must not unwind into user32.
    let handled = std::panic::catch_unwind(|| handle(hwnd, msg, wparam, lparam)).unwrap_or_else(|_| {
        eprintln!("banners: a message handler panicked");
        Some(LRESULT(0))
    });
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) })
}

fn handle(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let point = |b: &Banners| {
        let (x, y) = client_xy(lparam);
        (x as f32 / b.scale, y as f32 / b.scale)
    };
    // On its way in or out, a banner takes no pointer: a hover would draw it whole at its place in
    // the middle of its motion (a jump, a flash), a click would act on what is going.
    if matches!(msg, WM_MOUSEMOVE | WM_MOUSELEAVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONUP)
        && with(|b| b.shown.iter().any(|s| s.hwnd == hwnd && s.motion.is_some())).unwrap_or(true)
    {
        return Some(LRESULT(0));
    }
    match msg {
        // Never takes the foreground, but while its reply box is open (typing needs it).
        WM_MOUSEACTIVATE => {
            let replying = with(|b| b.shown.iter().any(|s| s.hwnd == hwnd && s.draft.is_some())).unwrap_or(false);
            Some(LRESULT(if replying { windows::Win32::UI::WindowsAndMessaging::MA_ACTIVATE as isize } else { MA_NOACTIVATE as isize }))
        }
        WM_MOUSEMOVE => {
            track_leave(hwnd);
            with(|b| {
                let (x, y) = point(b);
                let s = b.shown.iter_mut().find(|s| s.hwnd == hwnd)?;
                let hit = s.scene.hit(x, y);
                if hit != s.hover {
                    s.hover = hit;
                    b.refresh(hwnd);
                    b.render(hwnd, 255, 0);
                }
                Some(())
            });
            Some(LRESULT(0))
        }
        WM_MOUSELEAVE => {
            with(|b| {
                let s = b.shown.iter_mut().find(|s| s.hwnd == hwnd)?;
                if s.hover.is_some() || s.pressed.is_some() {
                    s.hover = None;
                    s.pressed = None;
                    b.refresh(hwnd);
                    b.render(hwnd, 255, 0);
                }
                Some(())
            });
            Some(LRESULT(0))
        }
        WM_LBUTTONDOWN => {
            with(|b| {
                let (x, y) = point(b);
                let s = b.shown.iter_mut().find(|s| s.hwnd == hwnd)?;
                s.pressed = s.scene.hit(x, y);
                b.refresh(hwnd);
                b.render(hwnd, 255, 0);
                Some(())
            });
            Some(LRESULT(0))
        }
        WM_LBUTTONUP => {
            let after = with(|b| {
                let (x, y) = point(b);
                let s = b.shown.iter_mut().find(|s| s.hwnd == hwnd)?;
                let (pressed, hit) = (s.pressed.take(), s.scene.hit(x, y));
                if pressed.is_none() || pressed != hit {
                    b.refresh(hwnd);
                    b.render(hwnd, 255, 0);
                    return None;
                }
                click(b, hwnd, hit?)
            })
            .flatten();
            if let Some(after) = after {
                after();
            }
            Some(LRESULT(0))
        }
        WM_CHAR => {
            let after = char::from_u32(wparam.0 as u32).and_then(|c| with(|b| key(b, hwnd, c)).flatten());
            if let Some(after) = after {
                after();
            }
            Some(LRESULT(0))
        }
        // Left for something else: the user chose where the keyboard went (it is not taken back).
        // With nothing typed the box closes and the banner goes on its way; with a reply half
        // typed it waits a while for them, then goes to the list (the panel keeps the draft).
        WM_ACTIVATE if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE => {
            with(|b| {
                let s = b.shown.iter_mut().find(|s| s.hwnd == hwnd)?;
                let draft = s.draft.clone()?;
                if draft.is_empty() {
                    s.draft = None;
                    s.until = Some(deadline(s, lifetime()));
                } else {
                    s.until = Some(deadline(s, ABANDONED));
                    keep_draft(s.toast.id, draft);
                }
                b.keys_from = None;
                give_keys_back(b, hwnd);
                b.refresh(hwnd);
                b.render(hwnd, 255, 0);
                Some(())
            });
            None
        }
        // A right click sends it to the list.
        WM_RBUTTONUP => {
            with(|b| leave(b, hwnd, Exit::Slide));
            Some(LRESULT(0))
        }
        // Its frames: what moves on it (its time left, its picture, a call's rings), redrawn; the
        // pace eases once nothing but its time moves.
        WM_TIMER if wparam.0 == FRAME_TIMER => {
            with(|b| {
                // On its way in or out: only that, this frame.
                if b.shown.iter().any(|s| s.hwnd == hwnd && s.motion.is_some()) {
                    let entering = b.shown.iter().any(|s| s.hwnd == hwnd && matches!(s.motion, Some((Motion::Enter, _))));
                    step_motion(b, hwnd);
                    if entering || b.shown.iter().any(|s| s.hwnd == hwnd) {
                        b.pace(hwnd);
                    }
                    return Some(());
                }
                // Held by the pointer (all banners alike): its time stands still, its line brighter.
                b.hold();
                let s = b.shown.iter_mut().find(|s| s.hwnd == hwnd)?;
                // Gliding to its place in the stack: moved (its pixels as they are).
                step_glide(s);
                // "Now" became "1 min": its drawings (whole, and its line's) are out of date.
                if update_when(s) {
                    s.scene = scene_of(s);
                    s.line = None;
                    b.render(hwnd, 255, 0);
                }
                let s = b.shown.iter_mut().find(|s| s.hwnd == hwnd)?;
                if lively(s) || matches!(Own::of(&s.toast), Own::Meeting(_)) {
                    // Something moves on it (its picture, a call's rings, a glance's line, a
                    // meeting's countdown): drawn when its scene changed.
                    let next = scene_of(s);
                    if next.items != s.scene.items {
                        s.scene = next;
                        b.render(hwnd, 255, 0);
                    }
                } else if let Some(left) = time_left(s) {
                    // Only its time line moves: composed, not drawn (held, it stands, brighter).
                    b.line_frame(hwnd, left);
                }
                b.pace(hwnd);
                Some(())
            });
            Some(LRESULT(0))
        }
        WM_TIMER if wparam.0 == LIFE_TIMER => {
            with(|b| {
                let now = Instant::now();
                // A sound that waited for Windows' store long enough: played as most want one.
                if let Some(s) = b.shown.iter_mut().find(|s| s.hwnd == hwnd && s.chime_at.is_some_and(|at| now >= at)) {
                    s.chime_at = None;
                    if !s.toast.rich.silent {
                        chime();
                    }
                }
                // Held by the pointer, its time stands still.
                if b.hold() {
                    return;
                }
                let Some(s) = b.shown.iter().find(|s| s.hwnd == hwnd && s.until.is_some_and(|u| now >= u)) else { return };
                // Its time ran out: a notification flies into the bell (it is in the list); a glance,
                // a sent reply and the digest simply go.
                let exit = if s.is == Is::Listed && s.sent.is_none() { Exit::ToBell } else { Exit::Slide };
                leave(b, hwnd, exit);
            });
            Some(LRESULT(0))
        }
        _ => None,
    }
}

/// Dev: the newest notifications listed (`count` of them), as banners that stay, then a glance,
/// for `seconds`. Windows' own banners are left alone.
#[cfg(feature = "dev")]
pub fn preview(count: usize, seconds: u64) {
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};
    let pump = |until: Instant| {
        let mut msg = MSG::default();
        while Instant::now() < until {
            while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    notifications::start();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !notifications::read_yet() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    // Pictures and buttons read from Windows' store before showing.
    std::thread::sleep(Duration::from_millis(600));
    let list = notifications::list();
    println!("banners: {} listed, showing {}", list.len(), count.min(list.len()));
    for toast in list.into_iter().take(count).rev() {
        let life = Some(Duration::from_secs(seconds + 5));
        show(toast, Is::Listed, life, None);
    }
    // A glance, held as long as the rest (a real one goes after 1.5 s).
    glance("Français (France)", "Logitech MX Keys");
    with(|b| {
        for s in b.shown.iter_mut().filter(|s| s.is == Is::Glance) {
            s.until = Some(Instant::now() + Duration::from_secs(seconds + 5));
            s.life = Some(Duration::from_secs(seconds + 10));
        }
    });
    pump(Instant::now() + Duration::from_secs(seconds));
}

/// Dev: the digest, as it would show after a quiet time over everything listed now.
#[cfg(feature = "dev")]
pub fn preview_digest(seconds: u64) {
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};
    notifications::start();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !notifications::read_yet() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let ids: Vec<u32> = notifications::list().iter().map(|t| t.id).collect();
    digest(&ids);
    let until = Instant::now() + Duration::from_secs(seconds);
    let mut msg = MSG::default();
    while Instant::now() < until {
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(feature = "dev")]
thread_local! {
    /// Dev: the simulation's pointer over the banners (`Some(held)`), in place of the real one.
    static SIM_HOLD: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Dev: the banners run through what the pointer and time do to them, with real windows but never
/// the real pointer (a hold is said, a hover is a message sent to the window), every banner's
/// place, time left and drawn line recorded meanwhile, and checked. Returns whether all passed.
#[cfg(feature = "dev")]
pub fn simulate() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetWindowRect, IsWindow, PeekMessageW, SendMessageW, TranslateMessage, MSG, PM_REMOVE};

    /// One look at every banner: its window, where it is, its size, its time left, its drawn cut,
    /// whether it is leaving.
    #[derive(Clone, Debug)]
    struct Look {
        hwnd: isize,
        rect: (i32, i32, i32, i32),
        left: Option<f32>,
        cut: Option<i32>,
        leaving: bool,
    }
    fn look() -> Vec<Look> {
        with(|b| {
            b.shown
                .iter()
                .map(|s| {
                    let mut r = RECT::default();
                    let _ = unsafe { GetWindowRect(s.hwnd, &mut r) };
                    Look { hwnd: s.hwnd.0 as isize, rect: (r.left, r.top, r.right - r.left, r.bottom - r.top), left: time_left(s), cut: s.line.as_ref().map(|l| l.cut), leaving: leaving(s) }
                })
                .collect()
        })
        .unwrap_or_default()
    }
    /// Run the banners for `ms`, looking every frame or so (and `each` called at each look).
    fn run(ms: u64, mut each: impl FnMut()) -> Vec<(u64, Vec<Look>)> {
        let start = Instant::now();
        let mut seen = Vec::new();
        let mut msg = MSG::default();
        while start.elapsed() < Duration::from_millis(ms) {
            while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            each();
            seen.push((start.elapsed().as_millis() as u64, look()));
            std::thread::sleep(Duration::from_millis(4));
        }
        seen
    }
    let mut passed = true;
    let mut check = |name: &str, ok: bool, detail: String| {
        println!("{} {name}{}", if ok { "PASS" } else { "FAIL" }, if ok { String::new() } else { format!(": {detail}") });
        passed &= ok;
    };
    let toast = |n: u32| Toast {
        id: u32::MAX - 1000 - n,
        app: format!("Simulation {n}"),
        aumid: format!("Sim.App{n}"),
        title: format!("Banner {n}"),
        body: "A banner the simulation drives.".into(),
        at: crate::calendar::time::now(),
        logo: None,
        rich: Default::default(),
    };
    SIM_HOLD.with(|h| h.set(Some(false)));

    // 1. Three arrive, one after another: each springs in and settles at its place; those already
    //    there glide up out of the newcomer's way, never jumping.
    let mut arrival = Vec::new();
    for n in 0..3 {
        show(toast(n), Is::Listed, Some(Duration::from_millis(4500)), None);
        arrival.extend(run(250, || {}));
    }
    arrival.extend(run(500, || {}));
    let settled = with(|b| b.shown.iter().all(|s| s.motion.is_none() && s.glide.is_none() && s.pos == s.at)).unwrap_or(false);
    check("arrivals settle at their places", settled, String::new());
    let jumps = |seen: &[(u64, Vec<Look>)], limit: i32| -> Vec<String> {
        let mut out = Vec::new();
        for pair in seen.windows(2) {
            // Those leaving move fast on purpose (to the bell, sideways): only the stack counts.
            for now in pair[1].1.iter().filter(|l| !l.leaving) {
                let Some(was) = pair[0].1.iter().find(|l| l.hwnd == now.hwnd) else { continue };
                let dy = (now.rect.1 - was.rect.1).abs();
                if dy > limit {
                    out.push(format!("{:x} moved {dy}px in one look at {} ms", now.hwnd, pair[1].0));
                }
            }
        }
        out
    };
    let j = jumps(&arrival, 60);
    check("no banner jumps while others arrive", j.is_empty(), j.join("; "));

    // 2. Running: every banner's drawn line only ever shortens.
    let running = run(400, || {});
    let mut grew = Vec::new();
    for pair in running.windows(2) {
        for now in &pair[1].1 {
            if let (Some(was), Some(c)) = (pair[0].1.iter().find(|l| l.hwnd == now.hwnd).and_then(|l| l.cut), now.cut) {
                if c > was {
                    grew.push(format!("{:x}: {was} to {c} at {} ms", now.hwnd, pair[1].0));
                }
            }
        }
    }
    let moved = running.first().zip(running.last()).is_some_and(|(a, z)| a.1.iter().zip(&z.1).any(|(x, y)| x.cut != y.cut));
    check("running lines only shorten, and do move", grew.is_empty() && moved, format!("grew: {}; moved: {moved}", grew.join("; ")));

    // 3. Held: every time left, and every drawn line, stands exactly still however long.
    let before_hold = look();
    SIM_HOLD.with(|h| h.set(Some(true)));
    let held = run(900, || {});
    let mut stirred = Vec::new();
    for (t, looks) in held.iter().skip(20) {
        for l in looks {
            let first = held[20].1.iter().find(|f| f.hwnd == l.hwnd);
            // Its time, and its drawn line once its drawing is cached (a hold redraws its cache, at
            // the same time).
            if first.is_some_and(|f| f.left != l.left || (f.cut.is_some() && l.cut.is_some() && f.cut != l.cut)) {
                stirred.push(format!("{:x} at {t} ms: {:?}/{:?} then {:?}/{:?}", l.hwnd, first.unwrap().left, first.unwrap().cut, l.left, l.cut));
            }
        }
    }
    check("held, every time and every line stands exactly still", stirred.is_empty(), stirred.into_iter().take(3).collect::<Vec<_>>().join("; "));
    let at_hold: Vec<(isize, Option<f32>)> = held.last().map(|(_, l)| l.iter().map(|l| (l.hwnd, l.left)).collect()).unwrap_or_default();
    let close = before_hold.iter().all(|b| at_hold.iter().find(|(h, _)| *h == b.hwnd).is_some_and(|(_, l)| match (b.left, *l) {
        (Some(x), Some(y)) => (x - y).abs() < 0.03,
        _ => true,
    }));
    check("a hold stops the time where it was", close, format!("{before_hold:?} / {at_hold:?}"));

    // 4. Released: each resumes from where it stood (no jump), then runs on.
    SIM_HOLD.with(|h| h.set(Some(false)));
    let released = run(300, || {});
    let resumed = at_hold.iter().all(|(hwnd, was)| {
        let first = released.iter().find_map(|(_, l)| l.iter().find(|l| l.hwnd == *hwnd).map(|l| l.left));
        match (was, first.flatten()) {
            (Some(x), Some(y)) => (x - y).abs() < 0.03 && y <= *x,
            _ => true,
        }
    });
    let ran_on = released.first().zip(released.last()).is_some_and(|(a, z)| a.1.iter().zip(&z.1).all(|(x, y)| x.left.zip(y.left).is_none_or(|(p, q)| q < p)));
    check("released, each resumes where it stood and runs on", resumed && ran_on, format!("resumed {resumed}, ran on {ran_on}"));

    // 5. The top one flies to the bell while hovered every frame (a mouse move sent to its window):
    //    it keeps its size, its middle only nears the bell, and the others glide into its room.
    let top = with(|b| b.shown.iter().filter(|s| !leaving(s)).min_by_key(|s| s.at.1).map(|s| s.hwnd)).flatten();
    let Some(top) = top else {
        check("a banner to fly", false, "none".into());
        return false;
    };
    with(|b| leave(b, top, Exit::ToBell));
    let bell = crate::tray::bell_point().map(|p| (p.x, p.y));
    let flight = run(600, || unsafe {
        let _ = SendMessageW(top, WM_MOUSEMOVE, Some(WPARAM(0)), Some(LPARAM((20 << 16) | 20)));
    });
    let mut path: Vec<(i32, i32, i32)> = Vec::new();
    for (_, looks) in &flight {
        if let Some(l) = looks.iter().find(|l| l.hwnd == top.0 as isize) {
            path.push((l.rect.0 + l.rect.2 / 2, l.rect.1 + l.rect.3 / 2, l.rect.2));
        }
    }
    let target = bell.unwrap_or_else(|| path.last().map_or((0, 0), |p| (p.0, p.1)));
    let distance = |p: &(i32, i32, i32)| (((p.0 - target.0).pow(2) + (p.1 - target.1).pow(2)) as f64).sqrt();
    let backwards: Vec<String> = path.windows(2).filter(|w| distance(&w[1]) > distance(&w[0]) + 2.0).map(|w| format!("{:?} then {:?}", w[0], w[1])).collect();
    let same_size = path.windows(2).all(|w| w[0].2 == w[1].2);
    check("hovered mid-flight, it only nears the bell, at one size", backwards.is_empty() && same_size && path.len() > 5, format!("{} frames; backwards: {}; same size {same_size}", path.len(), backwards.join("; ")));
    let gone = !unsafe { IsWindow(Some(top)) }.as_bool() && with(|b| b.shown.iter().all(|s| s.hwnd != top)).unwrap_or(false);
    check("it is gone once landed", gone, String::new());
    let j = jumps(&flight, 40);
    check("the others glide into its room, never jumping", j.is_empty(), j.join("; "));

    // 6. Held during another one's flight: the flight goes on, the others stand.
    let next = with(|b| b.shown.iter().filter(|s| !leaving(s)).min_by_key(|s| s.at.1).map(|s| s.hwnd)).flatten();
    if let Some(next) = next {
        with(|b| leave(b, next, Exit::Slide));
        SIM_HOLD.with(|h| h.set(Some(true)));
        let during = run(500, || {});
        SIM_HOLD.with(|h| h.set(Some(false)));
        let landed = !unsafe { IsWindow(Some(next)) }.as_bool();
        let still: bool = during.iter().skip(20).all(|(_, l)| l.iter().filter(|l| !l.leaving).all(|l| during[20].1.iter().find(|f| f.hwnd == l.hwnd).is_none_or(|f| f.left == l.left)));
        check("held while another leaves: it goes on, the rest stand", landed && still, format!("landed {landed}, still {still}"));
    }

    // 7. Time runs out for the last: it flies to the bell and is gone; nothing is left behind.
    let rest = run(5200, || {});
    let empty = with(|b| b.shown.is_empty()).unwrap_or(false) && rest.last().is_some_and(|(_, l)| l.is_empty());
    check("the last runs out and goes; nothing left", empty, format!("{} left", rest.last().map_or(0, |(_, l)| l.len())));

    // 8. Two arrive in the same instant (a burst): neither is left on the other, both settle
    //    one above the other.
    show(toast(10), Is::Listed, Some(Duration::from_millis(2500)), None);
    show(toast(11), Is::Listed, Some(Duration::from_millis(2500)), None);
    let burst = run(800, || {});
    let apart = burst.last().is_some_and(|(_, l)| l.len() == 2 && (l[0].rect.1 - l[1].rect.1).abs() >= l[0].rect.3.min(l[1].rect.3));
    check("two at once settle one above the other", apart, format!("{:?}", burst.last().map(|(_, l)| l.iter().map(|l| l.rect).collect::<Vec<_>>())));
    let j = jumps(&burst, 40);
    check("…never jumping", j.is_empty(), j.join("; "));

    // 9. A redraw while one springs in (its picture arrived): it stays where the spring has it,
    //    never popping to its place.
    show(toast(12), Is::Listed, Some(Duration::from_millis(2500)), None);
    let newest = with(|b| b.shown.last().map(|s| (s.hwnd, s.at))).flatten();
    run(90, || {});
    if let Some((h, at)) = newest {
        with(|b| {
            b.refresh(h);
            b.render(h, 255, 0);
        });
        let mut r = RECT::default();
        let _ = unsafe { GetWindowRect(h, &mut r) };
        check("a redraw mid-spring keeps the spring", (r.left - at.0).abs() > 2, format!("at {} (its place {})", r.left, at.0));
    }
    run(600, || {});

    // 10. "Sent" while held: once the pointer leaves, it goes after its moment, the hold not
    //     counted twice.
    if let Some(h) = with(|b| b.shown.last().map(|s| s.hwnd)).flatten() {
        SIM_HOLD.with(|x| x.set(Some(true)));
        run(700, || {});
        with(|b| {
            if let Some(s) = b.shown.iter_mut().find(|s| s.hwnd == h) {
                s.sent = Some("✓ Sent".into());
                s.until = Some(deadline(s, SENT_FOR));
                s.life = None;
            }
        });
        run(400, || {});
        SIM_HOLD.with(|x| x.set(Some(false)));
        let t0 = Instant::now();
        let mut gone_after = None;
        run(2500, || {
            if gone_after.is_none() && !unsafe { IsWindow(Some(h)) }.as_bool() {
                gone_after = Some(t0.elapsed().as_millis());
            }
        });
        check("sent while held: goes after its moment once let go", gone_after.is_some_and(|ms| ms < 1400), format!("gone after {gone_after:?} ms"));
    }
    run(3000, || {});

    // 11. A layout switched while the last glance leaves: a new glance shows the new words.
    glance("Layout A", "");
    run(GLANCE.as_millis() as u64 + 60, || {});
    glance("Layout B", "");
    run(120, || {});
    let shown_b = with(|b| b.shown.iter().any(|s| s.is == Is::Glance && !leaving(s) && s.toast.title == "Layout B")).unwrap_or(false);
    check("a switch while the last glance leaves shows the new one", shown_b, String::new());
    run(2200, || {});
    SIM_HOLD.with(|h| h.set(None));
    passed
}

/// Dev: listed notification `id` clicked as its banner would (its launch), then a few seconds for
/// the app to take it.
#[cfg(feature = "dev")]
pub fn preview_click(id: u32, dry: bool) {
    notifications::start();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !notifications::read_yet() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let Some(toast) = notifications::list().into_iter().find(|t| t.id == id) else {
        println!("banners: {id} is not listed");
        return;
    };
    let r = &toast.rich;
    println!(
        "banners: clicking {} \"{}\": launch {:?} ({:?}), activator {:?}, thread {:?}, tag {:?}, call {}",
        toast.app,
        toast.title,
        r.launch,
        r.launch_kind,
        r.activator.map(|c| format!("{c:032x}")),
        r.thread,
        r.tag,
        r.call
    );
    for b in &r.buttons {
        println!("banners:   button {:?}: {:?} ({:?})", b.label, b.arguments, b.kind);
    }
    if dry {
        return;
    }
    if r.launch.is_empty() && r.launch_kind != Kind::Protocol {
        println!("banners: no launch: the app only");
        notifications::open_app(&toast.aumid);
    } else {
        rich::invoke(toast.id, &toast.aumid, r.activator, &r.launch, r.launch_kind, Vec::new());
    }
    std::thread::sleep(Duration::from_secs(4));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_banner_s_time_stands_exactly_still_and_resumes_where_it_stopped() {
        let start = Instant::now();
        let ms = |n: u64| start + Duration::from_millis(n);
        let life = Some(Duration::from_secs(5));
        let mut until = Some(ms(5000));
        // Running: 1 at the start, half way at 2.5 s.
        assert_eq!(left_at(until, life, None, start), Some(1.0));
        assert!((left_at(until, life, None, ms(2500)).unwrap() - 0.5).abs() < 1e-6);
        // Held at 1 s: the same at every frame of the hold, however long, to the last bit.
        let held = Some(ms(1000));
        let at_hold = left_at(until, life, held, ms(1000)).unwrap();
        for t in [1010, 1200, 1800, 3999, 9000] {
            assert_eq!(left_at(until, life, held, ms(t)), Some(at_hold), "held at {t} ms");
        }
        // Released at 9 s: exactly where it stopped, then running again from there.
        until = Some(released(until.unwrap(), ms(1000), ms(9000)));
        assert_eq!(left_at(until, life, None, ms(9000)), Some(at_hold));
        assert!(left_at(until, life, None, ms(9100)).unwrap() < at_hold);
        // Two holds add up; a hold never makes it run backwards.
        let twice = released(released(ms(5000), ms(100), ms(600)), ms(1000), ms(1300));
        assert_eq!(twice, ms(5800));
        assert_eq!(released(ms(5000), ms(2000), ms(1000)), ms(5000), "a release before its hold is no hold");
    }

    #[test]
    fn a_banner_that_stays_or_has_no_length_shows_no_time() {
        let now = Instant::now();
        assert_eq!(left_at(None, Some(Duration::from_secs(5)), None, now), None, "a call, a reminder");
        assert_eq!(left_at(Some(now), Some(Duration::ZERO), None, now), None);
        assert_eq!(left_at(Some(now), None, None, now), None);
        // Past its end: none left, never negative.
        assert_eq!(left_at(Some(now), Some(Duration::from_secs(5)), None, now + Duration::from_secs(1)), Some(0.0));
    }

    #[test]
    fn a_glide_starts_and_ends_gently_never_jumping() {
        assert_eq!(glide_ease(0.0), 0.0);
        assert_eq!(glide_ease(1.0), 1.0);
        // A banner's height (192 px at 150%) over GLIDE, in 16 ms frames: no step past 20 px, and
        // the first and last steps small.
        let frames = (GLIDE.as_secs_f32() / 0.016).ceil() as usize;
        let at = |i: usize| 192.0 * glide_ease(i as f32 / frames as f32);
        let steps: Vec<f32> = (1..=frames).map(|i| at(i) - at(i - 1)).collect();
        assert!(steps.iter().all(|s| *s >= 0.0 && *s <= 20.0), "{steps:?}");
        assert!(steps[0] < 2.0 && steps[frames - 1] < 2.0, "{steps:?}");
    }

    #[test]
    fn a_spring_overshoots_then_settles() {
        assert!(spring(0.0).abs() < 1e-6);
        let peak = (0..100).map(|i| spring(i as f32 / 100.0)).fold(0.0f32, f32::max);
        assert!(peak > 1.0, "past its place a little first ({peak})");
        assert!((spring(1.0) - 1.0).abs() < 0.01, "settled by the end ({})", spring(1.0));
    }

    #[test]
    fn audio_tray_s_own_notices_say_what_they_are_by_their_tag() {
        let own = |tag: &str| {
            let mut t = Toast { id: 1, app: "Audio Tray".into(), aumid: crate::toast::AUMID.into(), title: String::new(), body: String::new(), at: 0, logo: None, rich: Default::default() };
            t.rich.tag = tag.to_string();
            Own::of(&t)
        };
        assert!(own("audio-tray/battery/12/3f2a") == Own::Battery(12));
        assert!(own("audio-tray/audio") == Own::Audio);
        assert!(own("audio-tray/meeting/1795512345") == Own::Meeting(1795512345));
        assert!(own("audio-tray/focus/25") == Own::Focus);
        assert!(own("something else") == Own::No);
        // Another app's tag is never taken for Audio Tray's.
        let mut other = Toast { id: 1, app: "X".into(), aumid: "X".into(), title: String::new(), body: String::new(), at: 0, logo: None, rich: Default::default() };
        other.rich.tag = "audio-tray/audio".into();
        assert!(Own::of(&other) == Own::No);
    }

    #[test]
    fn a_face_without_a_photo_is_its_initials_in_a_colour_of_its_own() {
        let t = Toast { id: 1, app: "WhatsApp".into(), aumid: "w".into(), title: "Iulia Dima".into(), body: String::new(), at: 0, logo: None, rich: Default::default() };
        let (Face::Initials(a, c1), Face::Initials(b, c2)) = (face_of(&t), face_of(&t)) else { panic!("initials") };
        assert_eq!(a, "ID");
        assert_eq!((a, c1), (b, c2), "the same each time");
    }
}

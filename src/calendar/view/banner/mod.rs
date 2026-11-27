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


mod input;
mod look;
mod motion;
#[cfg(feature = "dev")]
mod sim;
use input::*;
use look::*;
pub(super) use look::list_look;
use motion::*;
#[cfg(feature = "dev")]
pub use sim::{preview, preview_click, preview_digest, simulate};
#[cfg(feature = "dev")]
use sim::SIM_HOLD;

use std::cell::RefCell;
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, KillTimer, SetTimer, ShowWindow, MA_NOACTIVATE, SW_SHOWNA, WA_INACTIVE, WINDOW_EX_STYLE, WM_ACTIVATE, WM_CHAR, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_RBUTTONUP, WM_TIMER, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};

use super::paint::{Gfx, Palette};
use super::scene::{self, Art, Face, Hit, Scene};
use super::{arrived as when_words, client_xy, cursor, draw, drafts, edge_of, editor, monitor, track_leave, Edge, MARGIN, WM_MOUSELEAVE};
use crate::calendar::notifications::{self, Toast};
use crate::calendar::rich::{self, Kind};
use crate::config::Config;

const CLASS: PCWSTR = w!("AudioTrayBanner");
/// Banners shown at once, at most: a fourth sends the oldest to the list.
const MAX: usize = 3;
/// Between two banners, in DIPs.
const GAP: f32 = 8.0;
/// How often a banner checks whether its time is up.
const LIFE_TIMER: usize = 1;
const LIFE_MS: u32 = 200;
/// How often, while notifications are held back, the quiet is looked at for its end.
const QUIET_POLL_MS: u32 = 2000;
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
/// An app listening: the microphone (the edge pill's).
const GLYPH_MIC: char = crate::icons::glyph::MIC;
/// A song, under its cover (shown where the cover cannot be drawn).
const GLYPH_SONG: char = '\u{E8D6}';
/// A new song: the glance's time.
const SONG: Duration = Duration::from_secs(4);
/// Pictures of Audio Tray's own: a meeting's calendar, focus done's check, a call's phone, the
/// digest's moon (what came during a quiet time).
const GLYPH_MEETING: char = '\u{E787}';
const GLYPH_FOCUS: char = '\u{E73E}';
const GLYPH_PHONE: char = '\u{E717}';
const GLYPH_DIGEST: char = '\u{E708}';
/// A battery this low (or lower) is drawn red, else amber.
const LOW_LEVEL: u8 = 15;
/// A layout switch: a glance.
const GLANCE: Duration = Duration::from_millis(1500);
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
use super::scene::{AMBER, BLUE, GREEN, PURPLE, RED};

/// What a banner is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Is {
    /// A notification in Windows' list.
    Listed,
    /// A layout switch: a glance, never listed.
    Glance,
    /// What came during a quiet time, summed up; opens the list.
    Digest,
    /// An app listening outside a call (a privacy alert, the edge pill's red one): a red glance on
    /// a timer like the others (gone sooner when the recording stops); a click mutes the
    /// microphone, its × or a right click dismisses it.
    Listening,
    /// A new song: a glance with its cover, never listed (the next replaces it); a click brings the
    /// player to the front, its × or a right click dismisses it.
    Song,
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
    draft: Option<super::editor::Editor>,
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
    /// Drawing kits of banners gone, their pixels freed, for the next ones. Never dropped: this
    /// thread's storage goes at the process's end, after COM, and releasing Direct2D's objects
    /// then crashes it (a few kits left to the process's end cost nothing).
    spare: Vec<std::mem::ManuallyDrop<Gfx>>,
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
        spare: Vec::new(),
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
    if super::is_open() {
        return; // the panel lists them
    }
    use crate::calendar::takeover::{wants_banner, windows_shows};
    // Each one ours to show: not one whose banners were off before Audio Tray took them, nor the
    // first of an app (or site) Windows had no settings for, whose banner Windows shows itself.
    // Sorted out first: neither would be summed up in the digest either.
    let fresh: Vec<Toast> = fresh
        .into_iter()
        .filter(|t| {
            let windows = windows_shows(&t.aumid);
            if windows {
                println!("banners: {} \"{}\" left to Windows' own banner (its first; its next are ours)", t.app, t.title);
            }
            !windows && wants_banner(&t.aumid)
        })
        .collect();
    // Quiet (Do not disturb, a full-screen app, a presentation): only what Windows lets through
    // by default, calls, alarms and reminders; the rest summed up once it is over.
    let quiet = quiet();
    let (ours, held): (Vec<Toast>, Vec<Toast>) = fresh.into_iter().partition(|t| !quiet || t.rich.sticky);
    if !held.is_empty() {
        println!("banners: {} quiet (Do not disturb, a full-screen app, a presentation, or the screen locked)", held.len());
        hold_for_digest(held.iter().map(|t| t.id));
    }
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
        b.relaid(hwnd);
        b.pace(hwnd);
        Some(())
    })
    .flatten()
    .is_some()
}

/// Notifications held back by a quiet time: once it is over, a digest says what came.
fn hold_for_digest(ids: impl Iterator<Item = u32>) {
    with(|b| {
        b.held.extend(ids);
        if b.quiet_timer == 0 {
            b.quiet_timer = unsafe { SetTimer(None, 0, QUIET_POLL_MS, Some(quiet_over)) };
        }
    });
}

/// Every [`QUIET_POLL_MS`] while notifications are held back: the quiet over, the digest shows.
unsafe extern "system" fn quiet_over(_: HWND, _: u32, timer: usize, _: u32) {
    let still = std::panic::catch_unwind(quiet).unwrap_or(false);
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
    // Banners handed back to Windows meanwhile: Windows' centre has them, no digest of Audio Tray's.
    let handed_over = with(|b| b.config.as_ref().is_some_and(crate::calendar::handed_over)).unwrap_or(false);
    if !handed_over {
        return;
    }
    let list = notifications::list();
    let waiting: Vec<&Toast> = list.iter().filter(|t| held.contains(&t.id)).collect();
    if waiting.is_empty() || super::is_open() {
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
    let rich = rich::Rich { buttons: vec![rich::Action { label: t.show_them.to_string(), arguments: String::new(), kind: Kind::Foreground }], ..Default::default() };
    let Some(toast) = own_toast(t.away.to_string(), body, crate::toast::AUMID.into(), rich) else { return };
    println!("banners: the digest, {} waiting", waiting.len());
    show(toast, Is::Digest, Some(lifetime() * 2), None);
    chime();
}

/// A glance of Audio Tray's own, never listed: a keyboard layout switched (the next replaces it).
pub fn glance(title: &str, body: &str) {
    // As quiet as a notification: not over Do not disturb, a full-screen app, or the open panel.
    if quiet_now() {
        return;
    }
    // The one showing takes the new words.
    replace_or_show(
        Is::Glance,
        |s| {
            s.toast.title = title.to_string();
            s.toast.body = body.to_string();
            s.until = Some(deadline(s, GLANCE));
            s.life = Some(GLANCE);
        },
        || {
            let Some(toast) = own_toast(title.to_string(), body.to_string(), crate::toast::AUMID.into(), Default::default()) else { return };
            show(toast, Is::Glance, Some(GLANCE), None);
        },
    );
}

/// A new song (`title`, `body`: artist and player), a glance with its `cover`, never listed: the
/// next replaces it. A click brings the player (`app_id`) to the front.
pub fn now_playing(title: &str, body: &str, cover: Option<(String, std::sync::Arc<Vec<u8>>)>, app_id: &str) {
    // As quiet as a notification: not over Do not disturb, a full-screen app, or the open panel.
    if quiet_now() {
        return;
    }
    let avatar = cover.map(|(key, bytes)| rich::Picture { key, bytes });
    // The one showing takes the new song.
    replace_or_show(
        Is::Song,
        |s| {
            s.toast.title = title.to_string();
            s.toast.body = body.to_string();
            s.toast.aumid = app_id.to_string();
            s.toast.rich.avatar = avatar.clone();
            s.until = Some(deadline(s, SONG));
            s.life = Some(SONG);
        },
        || {
            let rich = rich::Rich { avatar: avatar.clone(), ..Default::default() };
            let Some(toast) = own_toast(title.to_string(), body.to_string(), app_id.to_string(), rich) else { return };
            show(toast, Is::Song, Some(SONG), None);
        },
    );
}

/// The banner of kind `is` showing (a glance, a song, an app listening: one of each at most) takes
/// what `update` gives it and is drawn again in place; with none showing, `show_new` shows one.
fn replace_or_show(is: Is, update: impl FnOnce(&mut Shown), show_new: impl FnOnce()) {
    let replaced = with(|b| {
        let s = b.shown.iter_mut().find(|s| s.is == is && !leaving(s))?;
        update(s);
        let hwnd = s.hwnd;
        b.relaid(hwnd);
        Some(())
    })
    .flatten();
    if replaced.is_none() {
        show_new();
    }
}

/// Windows keeps notifications quiet now: Do not disturb, a full-screen app, a presentation, the
/// screen locked.
fn quiet() -> bool {
    crate::calendar::focus::active() || busy()
}

/// Audio Tray's own glances keep quiet now: as notifications do ([`quiet`]), and under the open
/// panel.
fn quiet_now() -> bool {
    super::is_open() || quiet()
}

/// A banner's notification of Audio Tray's own, never in Windows' list (a glance, the digest, an
/// app listening), under `aumid`, with an id of its own (counted down from the top: Windows' count
/// up from 1). `None` while the banners are busy.
fn own_toast(title: String, body: String, aumid: String, rich: rich::Rich) -> Option<Toast> {
    let id = with(|b| {
        b.next_own -= 1;
        b.next_own
    })?;
    Some(Toast { id, app: "Audio Tray".into(), aumid, title, body, at: crate::calendar::time::now(), logo: None, rich })
}

/// An app listening outside a call (`Some((app, user))`, the newest), or none any more: its red
/// alert shows, the edge pill's no more, and goes on its timer (or when the recording stops, or
/// when closed). Shown whatever the quiet (a privacy alert, not news), never listed.
pub fn listening(who: Option<(String, String)>) {
    let words = |app: &str| {
        let t = super::words::words();
        ((t.listening)(app), t.click_to_mute.to_string())
    };
    let current = with(|b| b.shown.iter().find(|s| s.is == Is::Listening && !leaving(s)).map(|s| (s.hwnd, s.toast.aumid.clone()))).flatten();
    match (who, current) {
        // The same app still: nothing to do.
        (Some((_, user)), Some((_, shown))) if user == shown => {}
        // Another app now: the alert says so; or a new alert.
        (Some((app, user)), _) => replace_or_show(
            Is::Listening,
            |s| {
                (s.toast.title, s.toast.body) = words(&app);
                s.toast.aumid = user.clone();
            },
            || {
                let (title, body) = words(&app);
                // The recording app in the place of the notifying one (its id, to know it again).
                let Some(toast) = own_toast(title, body, user.clone(), Default::default()) else { return };
                println!("banners: {app} is listening");
                show(toast, Is::Listening, Some(lifetime()), None);
            },
        ),
        (None, Some((hwnd, _))) => {
            with(|b| leave(b, hwnd, Exit::Slide));
        }
        (None, None) => {}
    }
}

/// Every banner goes at once (the panel opened, which lists them; Audio Tray no longer shows
/// them). An app listening stays said (the panel does not say it). A reply half typed goes to the
/// panel's drafts: its box there opens with it. Those held for the digest are seen too: no digest.
pub fn clear() {
    let hwnds: Vec<HWND> = with(|b| {
        if b.quiet_timer != 0 {
            let _ = unsafe { KillTimer(None, b.quiet_timer) };
            b.quiet_timer = 0;
        }
        b.held.clear();
        for s in &b.shown {
            if let Some(draft) = &s.draft {
                drafts::keep(s.toast.id, draft.text());
            }
        }
        b.shown.iter().filter(|s| s.is != Is::Listening).map(|s| s.hwnd).collect()
    })
    .unwrap_or_default();
    for hwnd in hwnds {
        with(|b| leave(b, hwnd, Exit::Now));
    }
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

/// Windows' notification sound (its banners' own are off with them), unless Windows' "Allow
/// notifications to play sounds" is off.
fn chime() {
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC, SND_NODEFAULT};
    if !crate::calendar::takeover::sounds_allowed() {
        return;
    }
    let _ = unsafe { PlaySoundW(w!("Notification.Default"), None, SND_ALIAS | SND_ASYNC | SND_NODEFAULT) };
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
    if !matches!(is, Is::Glance | Is::Listening | Is::Song) {
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
        let changed = b.read_display();
        let gfx = match b.spare.pop().map_or_else(Gfx::new, |kept| Ok(std::mem::ManuallyDrop::into_inner(kept))) {
            Ok(gfx) => gfx,
            Err(e) => {
                eprintln!("banners: no drawing kit ({e:#})");
                return false;
            }
        };
        let tint = match is {
            Is::Glance | Is::Listening | Is::Song => None,
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
            line: None,
            // Built just below, once it is whole.
            scene: Scene::default(),
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
            b.replace_all();
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

/// The display changed (its resolution, its scale, the taskbar moved, the theme): those showing go
/// to their new places, redrawn. Nothing showing, nothing done (the next banner reads it anyway).
pub fn display_changed() {
    with(|b| {
        if b.shown.iter().all(leaving) || !b.read_display() {
            return;
        }
        b.replace_all();
        b.place();
        let all: Vec<HWND> = b.shown.iter().filter(|s| !leaving(s)).map(|s| s.hwnd).collect();
        for hwnd in all {
            b.render(hwnd, 255, 0);
        }
    });
}

impl Banners {
    fn px(&self, dip: f32) -> i32 {
        (dip * self.scale).round() as i32
    }

    /// Where they go, and how they look, read afresh: the primary display's corner by the taskbar,
    /// as Windows' own. Whether any of it changed (the scale, the taskbar moved, the theme).
    fn read_display(&mut self) -> bool {
        let (scale, rect, work) = monitor(POINT { x: 0, y: 0 });
        crate::theme::refresh_mode();
        let palette = Palette::current();
        let edge = edge_of(&rect, &work);
        let changed = (self.scale, self.work, self.edge) != (scale, work, edge) || self.palette.as_ref().is_none_or(|p| p.bg != palette.bg);
        self.scale = scale;
        self.work = work;
        self.edge = edge;
        self.palette = Some(palette);
        changed
    }

    /// Those staying put at their new places, rebuilt (a glide from the old screen's coordinates
    /// would set off from nowhere); those leaving go on their way untouched.
    fn replace_all(&mut self) {
        for s in self.shown.iter_mut().filter(|s| !leaving(s)) {
            s.at = (0, 0);
            s.glide = None;
        }
        let all: Vec<HWND> = self.shown.iter().filter(|s| !leaving(s)).map(|s| s.hwnd).collect();
        for other in all {
            self.refresh(other);
        }
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

    /// Banner `hwnd` built afresh and drawn where it is (its state changed; its size did not move
    /// the stack).
    fn redraw(&mut self, hwnd: HWND) {
        self.refresh(hwnd);
        self.render(hwnd, 255, 0);
    }

    /// Banner `hwnd` built afresh, the stack placed again (its size may have changed), and drawn.
    fn relaid(&mut self, hwnd: HWND) {
        self.refresh(hwnd);
        self.place();
        self.render(hwnd, 255, 0);
    }

    /// Banner `hwnd`'s frames, as often as what is on it moves.
    fn pace(&mut self, hwnd: HWND) {
        let held = self.pointer_on_any();
        let Some(s) = self.shown.iter_mut().find(|s| s.hwnd == hwnd) else { return };
        let ms = if s.motion.is_some() || s.glide.is_some() {
            FRAME_MS
        } else if s.toast.rich.call && animated() {
            // Its rings (none with animation effects off: it stays still until answered).
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
                    s.line = None;
                    flipped.push(s.hwnd);
                }
                (false, Some(since)) => {
                    if let Some(until) = s.until.as_mut() {
                        *until = released(*until, since, now);
                    }
                    s.held_since = None;
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
        let said = |tag: &str, body: &str| {
            let mut t = Toast { id: 1, app: "Audio Tray".into(), aumid: crate::toast::AUMID.into(), title: String::new(), body: body.into(), at: 0, logo: None, rich: Default::default() };
            t.rich.tag = tag.to_string();
            Own::of(&t)
        };
        let own = |tag: &str| said(tag, "");
        // A battery's level from its words, in either language (its tag is the device's alone).
        assert!(said("audio-tray/battery/3f2a", "12% left. Time to charge it.") == Own::Battery(12));
        assert!(said("audio-tray/battery/3f2a", "Il reste 7 %. Pensez à la recharger.") == Own::Battery(7));
        // As the tray words its warning (a device's name may hold digits: they are in the title).
        let sent = crate::hub::low_battery("MX Master 3S", 9);
        assert!(said(&sent.tag, &sent.body) == Own::Battery(9), "{:?}", sent.body);
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

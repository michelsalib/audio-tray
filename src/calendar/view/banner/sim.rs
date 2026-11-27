//! Dev: banners previewed from what is listed, the digest, a click, and the simulation that drives
//! them with real windows and checks what they do.

use super::*;

/// Dev: the newest notifications listed (`count` of them), as banners that stay, then a glance,
/// for `seconds`. Windows' own banners are left alone.
#[cfg(feature = "dev")]
pub fn preview(count: usize, seconds: u64) {
    wait_for_list();
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
    crate::win::pump_for(Duration::from_secs(seconds));
}

/// Dev: Windows' list read (started, and waited for up to 8 s).
#[cfg(feature = "dev")]
pub(super) fn wait_for_list() {
    notifications::start();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !notifications::read_yet() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Dev: the digest, as it would show after a quiet time over everything listed now.
#[cfg(feature = "dev")]
pub fn preview_digest(seconds: u64) {
    wait_for_list();
    let ids: Vec<u32> = notifications::list().iter().map(|t| t.id).collect();
    digest(&ids);
    crate::win::pump_for(Duration::from_secs(seconds));
}

#[cfg(feature = "dev")]
thread_local! {
    /// Dev: the simulation's pointer over the banners (`Some(held)`), in place of the real one.
    pub(super) static SIM_HOLD: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Dev: the banners run through what the pointer and time do to them, with real windows but never
/// the real pointer (a hold is said, a hover is a message sent to the window), every banner's
/// place, time left and drawn line recorded meanwhile, and checked. Returns whether all passed.
#[cfg(feature = "dev")]
pub fn simulate() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsWindow, SendMessageW};

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
        while start.elapsed() < Duration::from_millis(ms) {
            crate::win::pump_messages();
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
            b.redraw(h);
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
    wait_for_list();
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
    }
    rich::open(&toast);
    std::thread::sleep(Duration::from_secs(4));
}

//! What the pointer and the keys do to a banner: a click opens its notification or carries out its
//! button, its reply box takes the keyboard while open, and its window's messages.

use super::*;

/// What a click on `hit` in banner `hwnd` does: the banner goes, and its notification is opened
/// or its button carried out as the list would.
pub(super) fn click(b: &mut Banners, hwnd: HWND, hit: Hit) -> Option<After> {
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
            s.draft = Some(editor::Editor::default());
            editor::wake_caret(hwnd);
            s.until = None;
            b.relaid(hwnd);
            return Some(Box::new(move || take_keys(hwnd)));
        }
        // Its field: the keyboard again (taken elsewhere meanwhile).
        (_, Hit::Draft(_)) => return Some(Box::new(move || take_keys(hwnd))),
        (Is::Listed, Hit::Send(_)) => {
            let text = s.draft.as_ref().map(|d| d.text().to_string()).unwrap_or_default();
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
                Some(Box::new(move || rich::open(&toast)))
            }
        }
        (Is::Listed, Hit::Action(_, n)) => {
            let action = toast.rich.buttons.get(n)?.clone();
            Some(Box::new(move || rich::carry_out(&toast, &action, Vec::new())))
        }
        // An app listening: the microphone muted (as the edge pill did), the alert said.
        (Is::Listening, Hit::Toast(_)) => Some(Box::new(|| {
            crate::tray::post(crate::tray::WM_BANNER_LISTENING, 1, 0);
        })),
        // Its close button: the alert said, the microphone left as it is.
        (Is::Listening, Hit::Dismiss(_)) => Some(Box::new(|| {
            crate::tray::post(crate::tray::WM_BANNER_LISTENING, 0, 0);
        })),
        // A new song: its player to the front (its id where a notification's app id goes).
        (Is::Song, Hit::Toast(_)) => {
            let app_id = toast.aumid.clone();
            Some(Box::new(move || crate::tray::perform(crate::hub::Action::Focus(app_id))))
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
pub(super) fn send(b: &mut Banners, hwnd: HWND, text: String, emoji: String) -> Option<After> {
    let s = b.shown.iter_mut().find(|s| s.hwnd == hwnd)?;
    let toast = s.toast.clone();
    let reply = toast.rich.reply.clone()?;
    let word = super::super::words::words().sent;
    s.sent = Some(if emoji.is_empty() { format!("✓ {word}") } else { format!("✓ {word} {emoji}") });
    let had_keys = s.draft.take().is_some();
    s.until = Some(deadline(s, SENT_FOR));
    s.life = None;
    b.redraw(hwnd);
    if had_keys {
        give_keys_back(b, hwnd);
    }
    Some(Box::new(move || rich::carry_out(&toast, &reply.send, vec![(reply.input, text)])))
}

/// Banner `hwnd` takes the keyboard for its reply box: it may now be activated (a click on it
/// granted the foreground to this process).
pub(super) fn take_keys(hwnd: HWND) {
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
pub(super) fn give_keys_back(b: &mut Banners, hwnd: HWND) {
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

/// A key in banner `hwnd`'s open reply box, as `press` ([`editor::on_key`] for `WM_KEYDOWN`,
/// [`editor::on_char`] for `WM_CHAR`) takes it: typing and the caret's keys edit it, Enter sends,
/// Escape closes the box. `None`: no box open, or not its key.
pub(super) fn key(b: &mut Banners, hwnd: HWND, press: impl FnOnce(&mut editor::Editor) -> editor::Outcome) -> Option<Option<After>> {
    let s = b.shown.iter_mut().find(|s| s.hwnd == hwnd)?;
    let outcome = press(s.draft.as_mut()?);
    let after = match outcome {
        editor::Outcome::Edited => {
            b.redraw(hwnd);
            None
        }
        editor::Outcome::Send => click(b, hwnd, Hit::Send(0)),
        editor::Outcome::Cancel => {
            s.draft = None;
            s.until = Some(deadline(s, lifetime()));
            b.redraw(hwnd);
            give_keys_back(b, hwnd);
            None
        }
        editor::Outcome::NotMine => return None,
    };
    Some(after)
}

pub(super) unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // A panic must not unwind into user32.
    let handled = std::panic::catch_unwind(|| handle(hwnd, msg, wparam, lparam)).unwrap_or_else(|_| {
        eprintln!("banners: a message handler panicked");
        Some(LRESULT(0))
    });
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) })
}

pub(super) fn handle(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
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
                    b.redraw(hwnd);
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
                    b.redraw(hwnd);
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
                b.redraw(hwnd);
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
                    b.redraw(hwnd);
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
        // The reply box's keys (the rest are Windows'), then what is typed.
        WM_KEYDOWN => {
            let vk = wparam.0 as u16;
            let taken = with(|b| key(b, hwnd, |e| editor::on_key(e, hwnd, vk))).flatten();
            let handled = taken.is_some();
            if let Some(after) = taken.flatten() {
                after();
            }
            handled.then_some(LRESULT(0))
        }
        WM_CHAR => {
            let after = char::from_u32(wparam.0 as u32).and_then(|c| with(|b| key(b, hwnd, |e| editor::on_char(e, hwnd, c))).flatten()).flatten();
            if let Some(after) = after {
                after();
            }
            Some(LRESULT(0))
        }
        // Left for something else: the user chose where the keyboard went (it is not taken back).
        // With nothing typed the box closes and the banner goes on its way; with a reply half
        // typed it stays, with no time running, until it is sent or closed (its X, Escape back in
        // it); the panel's box has the draft meanwhile too.
        WM_ACTIVATE if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE => {
            with(|b| {
                let s = b.shown.iter_mut().find(|s| s.hwnd == hwnd)?;
                let draft = s.draft.as_ref()?.text().to_string();
                if draft.is_empty() {
                    s.draft = None;
                    s.until = Some(deadline(s, lifetime()));
                } else {
                    s.until = None;
                    drafts::keep(s.toast.id, &draft);
                }
                b.keys_from = None;
                give_keys_back(b, hwnd);
                b.redraw(hwnd);
                Some(())
            });
            None
        }
        // A right click sends it to the list.
        WM_RBUTTONUP => {
            // An app listening: the alert said, the microphone left as it is.
            if with(|b| b.shown.iter().any(|s| s.hwnd == hwnd && s.is == Is::Listening)).unwrap_or(false) {
                crate::tray::post(crate::tray::WM_BANNER_LISTENING, 0, 0);
            }
            with(|b| leave(b, hwnd, Exit::Slide));
            Some(LRESULT(0))
        }
        // The reply box's caret blinks while it is open (and it has the keyboard).
        WM_TIMER if wparam.0 == editor::CARET_TIMER => {
            let open = with(|b| b.shown.iter().any(|s| s.hwnd == hwnd && s.draft.is_some())).unwrap_or(false);
            if editor::caret_tick(hwnd, open) {
                with(|b| b.redraw(hwnd));
            }
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
                // An app listening, its time up: said (the alert does not come back for it).
                if s.is == Is::Listening {
                    crate::tray::post(crate::tray::WM_BANNER_LISTENING, 0, 0);
                }
                leave(b, hwnd, exit);
            });
            Some(LRESULT(0))
        }
        _ => None,
    }
}

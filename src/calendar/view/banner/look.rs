//! How a banner looks: its scene from its notification (Audio Tray's own notices illustrated, a
//! conversation's faces, a call's rings), its time left, and its frames drawn and sent (only its
//! time line's rows, when nothing else moved).

use super::*;

/// One of Audio Tray's own notices, from its tag (`audio-tray/<kind>/<detail>`): drawn with its
/// own picture.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Own {
    No,
    Battery(u8),
    Audio,
    /// When the meeting starts (seconds since the epoch).
    Meeting(i64),
    Focus,
}

impl Own {
    pub(super) fn of(t: &Toast) -> Own {
        if t.aumid != crate::toast::AUMID {
            return Own::No;
        }
        let Some(rest) = t.rich.tag.strip_prefix(crate::toast::TAG) else { return Own::No };
        let mut parts = rest.split('/');
        match parts.next() {
            // Its level is in its words ("12% left"), not its tag: a tag is what Windows replaces
            // by, and each device's warning replaces its last.
            Some("battery") => {
                let digits: String = t.body.chars().skip_while(|c| !c.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
                Own::Battery(digits.parse().unwrap_or(10))
            }
            Some("audio") => Own::Audio,
            Some("meeting") => parts.next().and_then(|s| s.parse().ok()).map_or(Own::No, Own::Meeting),
            Some("focus") => Own::Focus,
            _ => Own::No,
        }
    }
}

/// Who wrote a notification, as a face: their photo, or their initials on a colour of their own.
pub(super) fn face_of(t: &Toast) -> Face {
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

/// Windows' accent colour, read from the registry at most every two seconds (asked for each frame
/// and each listed notification).
pub(super) fn accent() -> [u8; 3] {
    thread_local! {
        static ACCENT: std::cell::Cell<Option<(Instant, [u8; 3])>> = const { std::cell::Cell::new(None) };
    }
    ACCENT.with(|a| match a.get() {
        Some((at, rgb)) if at.elapsed() < Duration::from_secs(2) => rgb,
        _ => {
            let rgb = crate::theme::accent_rgb();
            a.set(Some((Instant::now(), rgb)));
            rgb
        }
    })
}

/// Banner `s`'s "when" words, worked out again only once a minute (they say minutes); `true`
/// when they changed (its drawing is then out of date).
pub(super) fn update_when(s: &mut Shown) -> bool {
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
pub(super) fn animated() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS};
    let mut on = windows::core::BOOL(1);
    let asked = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) };
    asked.is_err() || on.as_bool()
}

/// An app's colour from its logo: the average of its saturated, bright pixels, weighted by how
/// saturated and bright they are; `None` for a grey logo. Lifted to read on the dark card.
pub(super) fn tint_from_logo(bytes: &[u8]) -> Option<[u8; 3]> {
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

/// A banner's scene: as the notification centre draws its notification, in its app's colours, its
/// time and its picture moving; or for a glance Audio Tray's accent pill.
pub(super) fn scene_of(s: &Shown) -> Scene {
    scene_at(s, time_left(s))
}

/// Banner `s`'s time left, 1 to 0 (`None`: it stays, or its line is not shown).
pub(super) fn time_left(s: &Shown) -> Option<f32> {
    if s.sent.is_some() || s.draft.is_some() {
        return None;
    }
    left_at(s.until, s.life, s.held_since, Instant::now())
}

/// A banner's time left (1 to 0) at `now`, given when it goes, how long it was given, and since
/// when the pointer holds it: held, as of the moment the hold began (it stands exactly where it
/// stopped). `None`: it stays until answered (no end), or has no length.
pub(super) fn left_at(until: Option<Instant>, life: Option<Duration>, held_since: Option<Instant>, now: Instant) -> Option<f32> {
    let (until, life) = (until?, life?);
    if life.is_zero() {
        return None;
    }
    let at = held_since.unwrap_or(now);
    Some(until.saturating_duration_since(at).as_secs_f32() / life.as_secs_f32())
}

/// When banner `s` goes, `after` from now: from the frozen clock while the pointer holds it (the
/// whole hold is added at its end, and must not be twice).
pub(super) fn deadline(s: &Shown, after: Duration) -> Instant {
    s.held_since.unwrap_or_else(Instant::now) + after
}

/// When a banner held since `since` goes once the hold ends at `now`: its end moves on by the
/// whole hold, so its time resumes where it stopped.
pub(super) fn released(until: Instant, since: Instant, now: Instant) -> Instant {
    until + now.saturating_duration_since(since)
}

/// [`scene_of`] with its time line at `left`.
pub(super) fn scene_at(s: &Shown, left: Option<f32>) -> Scene {
    let t = &s.toast;
    let now = Instant::now();
    // A glance: its glyph, its colour, and whether it closes (a song's cover over its glyph).
    let glance = match s.is {
        Is::Listening => Some((GLYPH_MIC, Some(RED), true)),
        Is::Glance => Some((GLYPH_KEYBOARD, None, false)),
        Is::Song => Some((GLYPH_SONG, None, true)),
        Is::Notice(note) => {
            let (glyph, tint) = note.look();
            Some((glyph, tint, true))
        }
        Is::Listed | Is::Digest => None,
    };
    if let Some((glyph, tint, closable)) = glance {
        let cover = t.rich.avatar.as_ref().filter(|_| s.is == Is::Song).map(|p| (p.key.clone(), p.bytes.clone()));
        return scene::glance(&scene::Glance { glyph, cover, title: &t.title, body: &t.body, left: left.unwrap_or(0.0), hover: s.hover, tint, closable });
    }
    // Windows' animation effects off: every picture at rest from the start (the battery at its
    // level, the speaker's bars still, no confetti, no rings, no pop).
    let still = !animated();
    let age = now.duration_since(s.born).as_secs_f32();
    let moving = if still { 1.0 } else { (age / ART_FOR.as_secs_f32()).min(1.0) };
    let own = Own::of(t);
    let mut body = t.body.clone();
    if let Own::Meeting(start) = own {
        // Counting down to it, then how long ago it began.
        let words = super::super::words::words();
        let to = start - crate::calendar::time::now();
        body = if to > 0 { (words.starts_in)(to) } else { (words.started)((-to) / 60) };
    }
    let (art, tint) = match (s.is, own) {
        (Is::Digest, _) => (Art::Glyph { glyph: GLYPH_DIGEST, rgb: s.tint.unwrap_or(PURPLE) }, s.tint),
        (_, Own::Battery(level)) if !still => {
            // Draining from a little fuller down to its level.
            let to = level as f32 / 100.0;
            let from = (to + 0.35).min(1.0);
            let eased = 1.0 - (1.0 - moving).powi(3);
            let (_, tint) = own_look(own);
            (Art::Battery { level: from + (to - from) * eased, low: level <= LOW_LEVEL }, tint)
        }
        (_, Own::Audio) if !still => {
            let playing = 1.0 - moving;
            let bars = std::array::from_fn(|n| 0.35 + playing * 0.6 * ((age * 9.0 + n as f32 * 1.3).sin() * 0.5 + 0.5));
            // The speaker is in the accent: the banner with it.
            (Art::Speaker { bars }, Some(accent()))
        }
        (_, Own::Battery(_) | Own::Audio | Own::Meeting(_) | Own::Focus) => own_look(own),
        _ if s.more > 0 => {
            let mut faces = vec![face_of(t)];
            faces.extend(s.faces.iter().cloned());
            (Art::Faces(faces), s.tint)
        }
        // A call without the caller's photo: a phone on its colour, the rings around it.
        _ if t.rich.call && t.rich.avatar.is_none() => (phone(s.tint), s.tint),
        _ => (Art::None, s.tint),
    };
    let more_pop = match s.more_at {
        Some(at) if !still && now.duration_since(at) < POP_FOR => 1.0 + 0.3 * (1.0 - now.duration_since(at).as_secs_f32() / POP_FOR.as_secs_f32()),
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
    let pulse = (call && !still).then(|| (age / 1.6) % 1.0);
    scene::banner(&scene::Banner {
        app: t.app.clone(),
        logo: t.logo.clone().map(|l| (t.aumid.clone(), l)),
        when: if s.when.1.is_empty() { when_words(t.at) } else { s.when.1.clone() },
        // Its colour: the card itself wears it.
        toast: scene::Toast {
            body,
            // A banner has no room for a menu: the list has it.
            menu: Vec::new(),
            draft: s.draft.clone(),
            quick: if t.rich.reply.is_some() && !call { QUICK.iter().map(|q| q.to_string()).collect() } else { Vec::new() },
            sent: s.sent.clone(),
            art,
            ..scene::Toast::of(t)
        },
        hover: s.hover,
        pressed: s.pressed,
        tint,
        left,
        // Held by the pointer: its time line stops, brighter.
        paused: s.held_since.is_some(),
        more: s.more,
        more_pop,
        pulse,
        confetti,
        caret: editor::caret_shown(),
    })
}

/// A notification's look in the panel's list, as its banner has it at rest: its picture (Audio
/// Tray's own illustrated, a call's phone) and its app's colour.
pub(in crate::calendar::view) fn list_look(t: &Toast) -> (Art, Option<[u8; 3]>) {
    match Own::of(t) {
        Own::No => {
            let tint = with(|b| b.tint_of(t)).flatten().or(Some(accent()));
            if t.rich.call && t.rich.avatar.is_none() {
                (phone(tint), tint)
            } else {
                (Art::None, tint)
            }
        }
        own => own_look(own),
    }
}

/// Audio Tray's own notice's picture and colour, at rest (`Own::No`: none).
pub(super) fn own_look(own: Own) -> (Art, Option<[u8; 3]>) {
    match own {
        Own::Battery(level) => {
            let low = level <= LOW_LEVEL;
            (Art::Battery { level: level as f32 / 100.0, low }, Some(if low { RED } else { AMBER }))
        }
        Own::Audio => (Art::Speaker { bars: SPEAKER_AT_REST }, Some(accent())),
        Own::Meeting(_) => (Art::Glyph { glyph: GLYPH_MEETING, rgb: BLUE }, Some(BLUE)),
        Own::Focus => (Art::Glyph { glyph: GLYPH_FOCUS, rgb: GREEN }, Some(GREEN)),
        Own::No => (Art::None, None),
    }
}

/// A call's phone, on the caller's app's colour (green without one).
pub(super) fn phone(tint: Option<[u8; 3]>) -> Art {
    Art::Glyph { glyph: GLYPH_PHONE, rgb: tint.unwrap_or(GREEN) }
}

/// The speaker's bars still (in the list, or with animation effects off).
pub(super) const SPEAKER_AT_REST: [f32; 4] = [0.5, 0.9, 0.65, 0.4];

/// Something moves on banner `s` (its picture, a call's rings, its count growing): drawn smoothly.
/// With Windows' animation effects off, nothing does.
pub(super) fn lively(s: &Shown) -> bool {
    let now = Instant::now();
    // Glances (the layout's, an app listening): pills, drawn whole (their line is not a card's).
    matches!(s.is, Is::Glance | Is::Listening | Is::Song | Is::Notice(_))
        || (animated()
            && (s.toast.rich.call
                || (Own::of(&s.toast) != Own::No && now.duration_since(s.born) < ART_FOR)
                || s.more_at.is_some_and(|at| now.duration_since(at) < POP_FOR)))
}

impl Banners {
    /// Draw banner `hwnd` and put it on screen where its window is, `dx` from there, `alpha` faded.
    pub(super) fn render(&mut self, hwnd: HWND, alpha: u8, dx: i32) {
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
        self.paint(hwnd, 1.0, |_| at, alpha);
    }

    /// Draw banner `hwnd` `zoom` times its size (smaller as it flies into the bell), centred on
    /// `centre`. Its frame keeps its full size (only what is drawn in it shrinks): its surface, its
    /// decoded pictures and its window's bitmap stay as they are.
    pub(super) fn render_zoomed(&mut self, hwnd: HWND, zoom: f32, centre: (i32, i32), alpha: u8) {
        self.paint(hwnd, zoom, |(w, h)| (centre.0 - w as i32 / 2, centre.1 - h as i32 / 2), alpha);
    }

    /// Draw banner `hwnd` whole, `zoom` times its size, and put its window at `at(size in pixels)`,
    /// `alpha` faded.
    pub(super) fn paint(&mut self, hwnd: HWND, zoom: f32, at: impl FnOnce((u32, u32)) -> (i32, i32), alpha: u8) {
        let scale = self.scale;
        let Some(palette) = self.palette.as_ref() else { return };
        let Some(s) = self.shown.iter_mut().find(|s| s.hwnd == hwnd) else { return };
        let Shown { gfx, scene, canvas, .. } = s;
        let size = (((scene.w * scale).round() as u32).max(1), ((scene.h * scale).round() as u32).max(1));
        if let Err(e) = gfx.frame_zoomed(size, scale, zoom, None, |pen, area| draw(pen, scene, palette, area)) {
            eprintln!("banners: could not draw ({e:#})");
            return;
        }
        let at = at(size);
        let pixels = &gfx.pixels;
        canvas.present(hwnd, (size.0 as i32, size.1 as i32), at, alpha, |dst| {
            let n = dst.len().min(pixels.len());
            dst[..n].copy_from_slice(&pixels[..n]);
        });
    }

    /// A frame where only banner `hwnd`'s time line moves: its rows composed from its two drawings
    /// (drawn once, again only when something else on it changed), written into the window's
    /// bitmap and only they sent to Windows; nothing at all when the line has not moved a pixel.
    pub(super) fn line_frame(&mut self, hwnd: HWND, left: f32) {
        let scale = self.scale;
        let Some(palette) = self.palette.as_ref() else { return };
        let Some(s) = self.shown.iter_mut().find(|s| s.hwnd == hwnd) else { return };
        if s.line.is_none() {
            let full = scene_at(s, Some(1.0));
            let empty = scene_at(s, Some(0.0));
            let size = (((full.w * scale).round() as u32).max(1), ((full.h * scale).round() as u32).max(1));
            let px = |dip: f32| (dip * scale).round() as i32;
            let (w, h) = (size.0 as i32, size.1 as i32);
            // The line as scene::banner lays it; a pixel more each way for its rounded ends'
            // blending.
            use super::scene::{LINE_H, LINE_INSET, LINE_UP};
            let rows = (h - px(LINE_UP) - 1).max(0)..(h - px(LINE_UP - LINE_H) + 1).min(h);
            let columns = (px(LINE_INSET) - 1).max(0)..(w - px(LINE_INSET) + 1).min(w);
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

}

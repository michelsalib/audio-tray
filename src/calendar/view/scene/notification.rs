//! The notification centre laid out: its header, each app's notifications (as a banner draws its
//! one too), and its height.

use super::*;

/// The notification centre's natural height (its header and every notification, as far as each is
/// collapsed), for `groups`.
pub(super) fn center_height(groups: &[Group]) -> f32 {
    let h = CENTER_HEAD_H;
    if groups.is_empty() {
        return h + EMPTY_H + 8.0;
    }
    h + groups.iter().map(|g| GROUP_H * g.collapse() + g.toasts.iter().map(Toast::room).sum::<f32>()).sum::<f32>() + 8.0
}

/// The notification centre's height: all it lists, up to the screen's; one line when none wait.
pub(super) fn center_card_height(input: &Input) -> f32 {
    let least = if input.groups.is_empty() { 0.0 } else { CENTER_MIN.min(input.room) };
    (center_height(&input.groups) + FOOTER_H).min(input.room).max(least)
}

/// One notification in `r`: the sender's photo, its text, its picture, then its buttons (or its
/// reply box, while open) and its menu.
pub(super) fn notification(ink: &mut Layout, r: R, toast: &Toast) {
    let i = toast.index;
    if !ink.inert {
        ink.places.push((i, r));
    }
    let lit = ink.hover.and_then(toast_of) == Some(i) || ink.focus.and_then(toast_of) == Some(i) || toast.draft.is_some();
    let fill = if ink.pressed == Some(Hit::Toast(i)) && ink.lit(Hit::Toast(i)) { Fill::Pressed } else if lit { Fill::Hover } else { Fill::Raised };
    ink.draw(r, Draw::Rect { fill, radius: 6.0 });
    if let Some(rgb) = toast.tint {
        // Its app's colour, from the left edge: the list as colourful as the banners.
        ink.draw(r, Draw::Wash { rgb, alpha: 64, reach: 0.55, radius: 6.0 });
    }
    ink.hit(r, Hit::Toast(i));
    let mut tx = r.x + 12.0;
    if toast.art != Art::None {
        // Its picture (a battery, a speaker, a conversation's faces) where a photo goes.
        art(ink, R::new(tx, r.y + 12.0, 36.0, 36.0), &toast.art);
        tx += 48.0;
    } else if let Some((key, bytes)) = &toast.avatar {
        ink.draw(R::new(tx, r.y + 12.0, 36.0, 36.0), Draw::Avatar { key: key.clone(), bytes: bytes.clone() });
        tx += 48.0;
    }
    ink.text(R::new(tx, r.y + 8.0, r.right() - tx - 76.0, 20.0), toast.title.clone(), Font::Strong, Ink::Text, Align::Left);
    ink.text(R::new(tx, r.y + 30.0, r.right() - tx - 12.0, 38.0), toast.body.clone(), Font::Wrap, Ink::Text2, Align::Left);
    if lit {
        // A full-size target, under the pointer or the keyboard.
        let x_r = R::new(r.right() - 38.0, r.y + 4.0, 32.0, 32.0);
        ink.plate(x_r, Hit::Dismiss(i), x_r.h / 2.0);
        ink.glyph(x_r, '\u{E711}', Font::IconSmall, Ink::Text2);
        if !toast.has_actions() && !toast.menu.is_empty() {
            // A menu with no buttons: its "…" beside the X, no row of its own.
            let more = R::new(x_r.x - 34.0, x_r.y, 32.0, 32.0);
            ink.plate(more, Hit::Menu(i), more.h / 2.0);
            ink.glyph(more, '\u{E712}', Font::IconSmall, Ink::Text2);
        }
    } else {
        // Its own time, in the X's place.
        ink.text(R::new(r.right() - 76.0, r.y + 8.0, 64.0, 20.0), toast.when.clone(), Font::Caption, Ink::Text3, Align::Right);
    }
    let mut y = r.y + TOAST_H - 4.0;
    if let Some((key, bytes)) = &toast.picture {
        ink.draw(R::new(r.x + 12.0, y, r.w - 24.0, PICTURE_H), Draw::Picture { key: key.clone(), bytes: bytes.clone() });
        y += PICTURE_H + 8.0;
    }
    if toast.has_actions() {
        let row = R::new(r.x + 12.0, y, r.w - 24.0, ACTIONS_H);
        match &toast.draft {
            // Sent: one word in the accent where its buttons were, for a moment.
            _ if toast.sent.is_some() => {
                let word = toast.sent.clone().unwrap_or_default();
                let pw = (text_w(&word, Font::CaptionStrong) + 32.0).min(row.w);
                let pill = R::new(row.x, row.y + 2.0, pw, row.h - 4.0);
                ink.draw(pill, Draw::Rect { fill: Fill::Accent, radius: pill.h / 2.0 });
                ink.text(pill, word, Font::CaptionStrong, Ink::OnAccent, Align::Center);
            }
            Some(draft) => {
                let field = R::new(row.x, row.y, row.w - ACTIONS_H - 6.0, row.h);
                // Windows 11's text box with the focus: a hairline round it, the accent underneath.
                ink.draw(field, Draw::Rect { fill: Fill::Raised, radius: 4.0 });
                ink.draw(field, Draw::Outline { ink: Ink::Text3, radius: 4.0 });
                ink.draw(R::new(field.x + 1.0, field.bottom() - 2.0, field.w - 2.0, 2.0), Draw::Rect { fill: Fill::Accent, radius: 1.0 });
                ink.hit(field, Hit::Draft(i));
                let inner = R::new(field.x + 10.0, field.y, field.w - 20.0, field.h);
                if draft.is_empty() {
                    // The hint stays put, the caret before it, as Windows' own boxes do.
                    let hint = toast.reply.clone().unwrap_or_default();
                    ink.text(R::new(inner.x + 1.0, inner.y, inner.w - 1.0, inner.h), hint, Font::Body, Ink::Text3, Align::Left);
                }
                let caret = ink.caret;
                ink.draw(inner, Draw::Field { editor: draft.clone(), font: Font::Body, caret });
                let send = R::new(row.right() - ACTIONS_H, row.y, ACTIONS_H, ACTIONS_H);
                let fill = ink.accent(Hit::Send(i));
                ink.draw(send, Draw::Rect { fill, radius: 4.0 });
                ink.glyph(send, '\u{E724}', Font::IconSmall, Ink::OnAccent);
                ink.hit(send, Hit::Send(i));
            }
            None => {
                let mut bx = row.x;
                let end = if toast.menu.is_empty() { row.right() } else { row.right() - ACTIONS_H - 6.0 };
                let mut pills: Vec<(String, Hit)> = toast.buttons.iter().enumerate().map(|(n, label)| (label.clone(), Hit::Action(i, n))).collect();
                if toast.reply.is_some() {
                    pills.insert(0, (words().reply.to_string(), Hit::Reply(i)));
                    // The reactions after Reply, before the app's own buttons.
                    for (n, emoji) in toast.quick.iter().enumerate() {
                        pills.insert(1 + n, (emoji.clone(), Hit::Quick(i, n)));
                    }
                }
                let buttons = toast.buttons.len();
                for (label, hit) in pills {
                    // What room is left, before sizing: a clamp with less than its minimum panics.
                    let room = end - bx;
                    if room < 32.0 {
                        break;
                    }
                    if matches!(hit, Hit::Quick(..)) {
                        // A reaction: a small round button, the emoji in its colours.
                        let q = R::new(bx, row.y + 2.0, (row.h - 4.0).min(room), row.h - 4.0);
                        let fill = ink.surface(hit, Fill::Raised);
                        ink.draw(q, Draw::Rect { fill, radius: q.h / 2.0 });
                        ink.draw(q, Draw::Outline { ink: Ink::Text3, radius: q.h / 2.0 });
                        ink.text(q, label, Font::Body, Ink::Text, Align::Center);
                        ink.hit(q, hit);
                        bx += q.w + 6.0;
                        continue;
                    }
                    let pw = (text_w(&label, Font::CaptionStrong) + 28.0).max(64.0).min(room);
                    let pill = R::new(bx, row.y + 2.0, pw, row.h - 4.0);
                    // A call answers in green and declines in red, as phones do.
                    let call = match hit {
                        Hit::Action(_, 0) if toast.call && buttons >= 2 => Some((GREEN, [0x08, 0x15, 0x0E])),
                        Hit::Action(_, n) if toast.call && buttons >= 2 && n + 1 == buttons => Some((RED, [0xFF, 0xFF, 0xFF])),
                        _ => None,
                    };
                    match call {
                        Some((ground, text)) => {
                            let lift = if ink.lit(hit) { 230 } else { 255 };
                            ink.draw(pill, Draw::Rect { fill: Fill::Rgba(ground, lift), radius: pill.h / 2.0 });
                            ink.text(pill, label, Font::CaptionStrong, Ink::Rgba(text, 255), Align::Center);
                            ink.hit(pill, hit);
                        }
                        None => ink.pill(pill, &label, hit, false),
                    }
                    bx += pw + 6.0;
                }
                if !toast.menu.is_empty() {
                    let more = R::new(row.right() - ACTIONS_H, row.y, ACTIONS_H, ACTIONS_H);
                    ink.plate(more, Hit::Menu(i), more.h / 2.0);
                    ink.glyph(more, '\u{E712}', Font::IconSmall, Ink::Text2);
                }
            }
        }
        y += ACTIONS_H + 8.0;
    }
    if toast.menu_open {
        for (n, label) in toast.menu.iter().enumerate() {
            let line = R::new(r.x + 8.0, y, r.w - 16.0, MENU_ROW_H);
            ink.plate(line, Hit::MenuItem(i, n), 4.0);
            ink.text(R::new(line.x + 12.0, line.y, line.w - 24.0, line.h), label.clone(), Font::Body, Ink::Text, Align::Left);
            y += MENU_ROW_H;
        }
    }
}

/// How many are waiting in all, in an outlined chip at `(x, y)`, as wide as its number (99+ at
/// most). Grey, not the accent: the clock's accent badge counts only the new ones.
pub(super) fn count_chip(ink: &mut Layout, x: f32, y: f32, unread: usize) -> R {
    let label = if unread > 99 { "99+".to_string() } else { unread.to_string() };
    let chip = R::new(x, y, (text_w(&label, Font::CaptionStrong) + 10.0).max(24.0), 18.0);
    ink.draw(chip, Draw::Outline { ink: Ink::Text3, radius: 9.0 });
    ink.text(chip, label, Font::CaptionStrong, Ink::Text2, Align::Center);
    chip
}

/// The notification centre: its header, each app's notifications, and the focus timer at its
/// foot. Returns its scrolling area.
pub(super) fn center(ink: &mut Layout, card: R, input: &Input) -> (R, f32) {
    let groups = &input.groups;
    let x = card.x + 8.0;
    let w = card.w - 16.0;
    let t = words();
    let head = R::new(x, card.y + 10.0, w, 32.0);
    let title_w = text_w(t.notifications, Font::Strong) + 4.0;
    ink.text(R::new(head.x + 8.0, head.y, title_w, head.h), t.notifications, Font::Strong, Ink::Text, Align::Left);
    if input.unread > 0 {
        count_chip(ink, head.x + 8.0 + title_w + 4.0, head.y + 7.0, input.unread);
    }
    // Round buttons from the right, as every panel's header: clear all (when there is anything),
    // Do not disturb, the focus timer (while it runs, a lit pill saying the minutes left; a click
    // stops it).
    let mut right = head.right();
    let mut button = |ink: &mut Layout, glyph: char, hit: Hit, on: bool| {
        let r = R::new(right - 32.0, head.y, 32.0, 32.0);
        ink.icon_button(r, glyph, hit, on);
        right = r.x - 4.0;
    };
    if input.unread > 0 {
        button(ink, '\u{E74D}', Hit::ClearAll, false);
    }
    button(ink, '\u{E708}', Hit::Quiet, input.quiet);
    match input.focus_left {
        Some(minutes) => {
            let label = format!("{minutes} min");
            let label_w = text_w(&label, Font::CaptionStrong);
            // The glyph where the round button has it, the minutes after it.
            let r = R::new(right - (32.0 + label_w + 12.0), head.y, 32.0 + label_w + 12.0, 32.0);
            let fill = ink.accent(Hit::Focus);
            ink.draw(r, Draw::Rect { fill, radius: 16.0 });
            ink.hit(r, Hit::Focus);
            ink.glyph(R::new(r.x, r.y, 32.0, 32.0), '\u{E916}', Font::Icon, Ink::OnAccent);
            ink.text(R::new(r.x + 30.0, r.y, label_w + 2.0, r.h), label, Font::CaptionStrong, Ink::OnAccent, Align::Left);
        }
        None => button(ink, '\u{E916}', Hit::Focus, false),
    }
    footer(ink, card, '\u{EA8F}', words().notification_settings);

    let top = head.bottom() + 6.0;
    let area = R::new(card.x, top, card.w, card.bottom() - FOOTER_H - 4.0 - top);
    if groups.is_empty() {
        // None waiting, as Windows' centre says it; or none readable, and where to allow it.
        let line = R::new(x + 8.0, area.y, w - 16.0, EMPTY_H - 8.0);
        if input.notifications_denied {
            ink.hit(line, Hit::Allow);
            let tint = if ink.lit(Hit::Allow) { Ink::Accent } else { Ink::Text2 };
            ink.text(line, t.not_allowed, Font::Body, tint, Align::Left);
        } else {
            // What was just done ("3 notifications cleared") for a moment, then the empty state.
            let said = input.flash.clone().unwrap_or_else(|| t.no_notifications.to_string());
            ink.text(line, said, Font::Body, Ink::Text3, Align::Left);
        }
        return (area, 0.0);
    }
    let content = center_height(groups) - CENTER_HEAD_H - 8.0;
    let max = (content - area.h).max(0.0);
    let scroll = input.center_scroll.clamp(0.0, max);
    ink.draw(area, Draw::Clip);
    let mut y = area.y - scroll;
    for (g, group) in groups.iter().enumerate() {
        // A group whose last notifications are going collapses with them, its line too.
        let shown = group.collapse();
        let line = R::new(x, y, w, GROUP_H);
        let folding = shown < 1.0;
        if folding {
            ink.inert = true;
            ink.draw(R::new(line.x, line.y, line.w, line.h * shown), Draw::Clip);
        }
        // No plate: one would run into the notification under it. The name brightens instead.
        ink.hit(line, Hit::Group(g));
        let lit = ink.lit(Hit::Group(g)) || ink.lit(Hit::DismissGroup(g));
        let mut tx = line.x + 8.0;
        if let Some((key, bytes)) = &group.logo {
            ink.draw(R::new(tx, line.y + (line.h - 16.0) / 2.0, 16.0, 16.0), Draw::Logo { key: key.clone(), bytes: bytes.clone() });
            tx += 24.0;
        }
        ink.text(R::new(tx, line.y, line.right() - tx - 80.0, line.h), group.name.clone(), Font::CaptionStrong, if lit { Ink::Text } else { Ink::Text2 }, Align::Left);
        if lit {
            // Under the pointer: clear all of this app's, in place of its time.
            let x_r = R::new(line.right() - 30.0, line.y + (line.h - 24.0) / 2.0, 24.0, 24.0);
            ink.plate(x_r, Hit::DismissGroup(g), x_r.h / 2.0);
            ink.glyph(x_r, '\u{E711}', Font::IconSmall, Ink::Text2);
        } else {
            ink.text(R::new(line.right() - 80.0, line.y, 72.0, line.h), group.when.clone(), Font::Caption, Ink::Text3, Align::Right);
        }
        if folding {
            ink.draw(line, Draw::Unclip);
            ink.inert = false;
        }
        y += line.h * shown;
        for toast in &group.toasts {
            let r = R::new(x, y, w, toast.height());
            if toast.collapse < 1.0 {
                // Going: drawn in what is left of it (its foot first), the ones under it coming
                // up, and no target in it.
                ink.inert = true;
                ink.draw(R::new(r.x, r.y, r.w, r.h * toast.collapse.max(0.0)), Draw::Clip);
                notification(ink, r, toast);
                ink.draw(r, Draw::Unclip);
                ink.inert = false;
            } else {
                notification(ink, r, toast);
            }
            y += toast.room();
        }
    }
    ink.draw(area, Draw::Unclip);
    scrollbar(ink, area, content, scroll);
    (area, max)
}

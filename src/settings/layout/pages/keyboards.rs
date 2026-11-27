//! The Keyboards page: a layout for each keyboard, and the keyboards added.

use crate::lang::tr;
use crate::tf;

use super::super::column::{Column, Control, Gate, RowButton};
use super::super::{plain, Button, Choice, Model, Target, Toggle, Tone};
use super::{cancel_label, remove_label};

pub(in crate::settings::layout) fn keyboards(col: &mut Column, model: &Model) {
    let c = &model.config;
    let k = &c.keyboards;
    col.toggle(
        Some('\u{E765}'),
        tr("A layout for each keyboard", "Une disposition par clavier"),
        plain(tr("Typing on a keyboard switches to its layout. Press Shift first: the first key still types in the old one", "Taper sur un clavier passe à sa disposition. Appuyez d’abord sur Maj : la première touche tape encore dans l’ancienne")),
        Gate::Open,
        Toggle::Keyboards,
    );
    let off = !k.enabled;
    // Hiding Windows' layout icon is with the taskbar's buttons.
    col.toggle(
        None,
        tr("Show each switch", "Afficher chaque changement"),
        plain(tr("The new layout, for a moment, in a small banner", "La nouvelle disposition, un instant, dans une petite bannière")),
        Gate::needs(off, tr("Needs a layout for each keyboard, above", "Nécessite une disposition par clavier, ci-dessus"), None),
        Toggle::KeyboardReadout,
    );

    col.section(tr("Your keyboards", "Vos claviers"));
    let labels: Vec<String> = model.layouts.iter().map(|(_, label)| label.clone()).collect();
    let remove = |button: Button| RowButton::new(remove_label(), button).glyph('\u{E74D}');
    for (i, keyboard) in k.assigned.iter().enumerate() {
        let mut group = col.group();
        let is_connected = |path: &str| model.connected.iter().any(|known| known.eq_ignore_ascii_case(path));
        let paths: Vec<&str> = keyboard.paths().collect();
        let now = paths.iter().copied().find(|path| is_connected(path));
        let connected = now.is_some();
        let kind = |path: &str| crate::keyboards::connection(path).label();
        let state = match now {
            Some(path) => tf!("{} · connected", "{} · connecté", kind(path)),
            None if paths.len() == 1 => tf!("{} · not connected", "{} · non connecté", kind(paths[0])),
            None => tr("Not connected", "Non connecté").to_string(),
        };
        match &model.renaming {
            Some((at, text)) if *at == i => col.field_row(&mut group, text, &[RowButton::new(tr("Save", "Enregistrer"), Button::SaveName).accent(true), RowButton::new(cancel_label(), Button::CancelRename)]),
            _ => col.row(
                &mut group,
                None,
                &keyboard.name,
                Some((state, if connected { Tone::Good } else { Tone::Plain })),
                off,
                Control::Buttons(vec![RowButton::new(tr("Rename", "Renommer"), Button::RenameKeyboard(i)).glyph('\u{E8AC}'), remove(Button::RemoveKeyboard(i))]),
            ),
        }
        let selected = keyboard.hkl().and_then(|hkl| model.layouts.iter().position(|(known, _)| *known == hkl));
        let desc = match selected {
            Some(_) => None,
            None => Some((tr("Its layout is no longer installed: pick another", "Sa disposition n’est plus installée : choisissez-en une autre").to_string(), Tone::Warn)),
        };
        // Every installed layout as a chip, wrapping: one click, however many there are.
        let picks: Vec<(String, bool, Target)> = labels.iter().enumerate().map(|(j, label)| (label.clone(), selected == Some(j), Target::Choice(Choice::KeyboardLayout(i), j))).collect();
        col.pick_row(&mut group, tr("Layout", "Disposition"), desc, &picks, off);
        // Its connections, once it has more than one: each can go on its own.
        if paths.len() > 1 {
            for (j, path) in paths.iter().enumerate() {
                let title = match crate::keyboards::vid_pid(path) {
                    Some(id) => format!("{} · {id}", kind(path)),
                    None => kind(path).to_string(),
                };
                let state = if is_connected(path) { (tr("Connected", "Connecté").to_string(), Tone::Good) } else { (tr("Not connected", "Non connecté").to_string(), Tone::Plain) };
                col.row(&mut group, None, &title, Some(state), off, Control::Button(remove(Button::RemoveConnection(i, j))));
            }
        }
        let capturing_here = model.capture.is_some() && model.capture_into == Some(i);
        let control = if off || model.capture.is_some() { Control::None } else { Control::Button(RowButton::new(tr("Add", "Ajouter"), Button::AddConnection(i)).glyph('\u{E710}')) };
        let desc = if capturing_here { tr("Press a key on it, through its new connection…", "Appuyez sur une de ses touches, par sa nouvelle connexion…") } else { tr("Its cable, receiver or Bluetooth: each is told apart", "Son câble, son récepteur ou le Bluetooth : chacun est distingué") };
        col.row(&mut group, None, tr("Another connection", "Autre connexion"), Some((desc.to_string(), if capturing_here { Tone::Warn } else { Tone::Plain })), off, control);
        col.end_group(group);
    }
    if off {
        col.card(Some('\u{E710}'), tr("Add a keyboard", "Ajouter un clavier"), plain(tr("Turn on a layout for each keyboard first", "Activez d’abord une disposition par clavier")), true, None);
    } else if let Some(hint) = &model.capture {
        let desc = match hint {
            Some(hint) => (hint.clone(), Tone::Warn),
            None => (tr("Only which keyboard it comes from is read, never the key", "Seul le clavier d’origine est lu, jamais la touche").to_string(), Tone::Plain),
        };
        let title = match model.capture_into.and_then(|i| k.assigned.get(i)) {
            Some(keyboard) => tf!("Press a key on \u{201C}{}\u{201D}, through its new connection…", "Appuyez sur une touche de « {} », par sa nouvelle connexion…", keyboard.name),
            None => tr("Press a key on the keyboard to add…", "Appuyez sur une touche du clavier à ajouter…").to_string(),
        };
        let card = col.card(Some('\u{E765}'), title, Some(desc), false, None);
        col.buttons_in(card, &[RowButton::new(cancel_label(), Button::CancelCapture)], false);
    } else {
        let desc = if k.assigned.is_empty() { tr("Then press a key on it. Keyboards not added switch nothing", "Puis appuyez sur une de ses touches. Les claviers non ajoutés ne changent rien") } else { tr("Then press a key on it", "Puis appuyez sur une de ses touches") };
        col.button_card(Some('\u{E710}'), tr("Add a keyboard", "Ajouter un clavier"), plain(desc), RowButton::new(tr("Add", "Ajouter"), Button::AddKeyboard).accent(true));
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::tests::{assert_inside, measure, model};
    use super::super::super::{page, Item, Kind, Page};
    use super::*;

    fn keyboards_model() -> Model {
        let mut m = model(Page::Keyboards);
        m.config.keyboards.enabled = true;
        let keyboard = |path: &str, name: &str, hkl: &str| crate::config::AssignedKeyboard { device_path: path.into(), name: name.into(), hkl: hkl.into(), ..Default::default() };
        m.config.keyboards.assigned = vec![keyboard(r"\\?\ACPI#HPQ8002#x#{guid}", "Laptop", "040C040C"), keyboard(r"\\?\HID#VID_19F5&PID_2620#x#{guid}", "External", "0409040C")];
        m.layouts = vec![(0x040C_040C, "AZERTY".into()), (0x0409_040C, "US".into())];
        m.connected = vec![r"\\?\ACPI#HPQ8002#x#{guid}".into()];
        m
    }

    #[test]
    fn each_keyboard_picks_its_layout_and_shows_whether_it_is_connected() {
        let m = keyboards_model();
        for width in [500.0, 900.0, 2000.0] {
            assert_inside(&page(&m, width, &measure), width, "Keyboards");
        }
        let laid = page(&m, 900.0, &measure);
        // The second keyboard is on US: its second chip is the selected one, and hits.
        let us = laid.items.iter().find(|i| matches!(i.kind, Kind::Pick { target: Target::Choice(Choice::KeyboardLayout(1), 1), .. })).unwrap();
        assert!(matches!(us.kind, Kind::Pick { selected: true, .. }));
        assert_eq!(laid.hit(us.r.x + 4.0, us.r.y + 4.0), Some(Target::Choice(Choice::KeyboardLayout(1), 1)));
        let states: Vec<String> = laid.items.iter().filter_map(|i| match &i.kind {
            Kind::Label { desc: Some((desc, _)), .. } if desc.contains("connected") => Some(desc.clone()),
            _ => None,
        }).collect();
        assert_eq!(states, ["Built-in · connected", "USB · not connected"]);
        assert!(laid.hits.iter().any(|(_, t)| *t == Target::Button(Button::AddKeyboard)));
    }

    #[test]
    fn a_keyboard_with_several_connections_lists_each_and_is_connected_by_any() {
        let mut m = keyboards_model();
        let cable = r"\\?\HID#VID_3151&PID_4015&MI_00#x#{guid}";
        let ble = r"\\?\HID#{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&023151_PID&4013#x#{guid}";
        m.config.keyboards.assigned[1].more_paths = vec![cable.into(), ble.into()];
        m.connected.push(cable.into());
        let laid = page(&m, 900.0, &measure);
        let titles: Vec<&str> = laid.items.iter().filter_map(|i| match &i.kind {
            Kind::Label { title, .. } => Some(title.as_str()),
            _ => None,
        }).collect();
        for title in ["USB · 19F5:2620", "USB · 3151:4015", "Bluetooth · 3151:4013"] {
            assert!(titles.contains(&title), "{title} in {titles:?}");
        }
        // The laptop has one connection: no list for it.
        assert!(!laid.hits.iter().any(|(_, t)| matches!(t, Target::Button(Button::RemoveConnection(0, _)))));
        assert!(laid.hits.iter().any(|(_, t)| *t == Target::Button(Button::RemoveConnection(1, 2))));
        assert!(laid.hits.iter().any(|(_, t)| *t == Target::Button(Button::AddConnection(0))));
        assert!(laid.items.iter().any(|i| matches!(&i.kind, Kind::Label { desc: Some((d, Tone::Good)), .. } if d == "USB · connected")));
        for width in [500.0, 2000.0] {
            assert_inside(&page(&m, width, &measure), width, "Keyboards");
        }
        // While a key is awaited, no other capture can start.
        m.capture = Some(None);
        m.capture_into = Some(1);
        let laid = page(&m, 900.0, &measure);
        assert!(!laid.hits.iter().any(|(_, t)| matches!(t, Target::Button(Button::AddConnection(_)))));
    }

    #[test]
    fn a_keyboard_forgets_one_connection_but_keeps_its_last() {
        let mut k = crate::config::AssignedKeyboard { device_path: "a".into(), more_paths: vec!["b".into(), "c".into()], ..Default::default() };
        k.remove_path(0);
        assert_eq!(k.paths().collect::<Vec<_>>(), ["b", "c"]);
        k.remove_path(1);
        assert_eq!(k.paths().collect::<Vec<_>>(), ["b"]);
        k.remove_path(0);
        assert_eq!(k.paths().collect::<Vec<_>>(), ["b"]);
        assert!(k.has("B"));
    }

    #[test]
    fn many_layouts_wrap_and_each_is_one_click() {
        let mut m = keyboards_model();
        m.layouts = (0..8).map(|i| (0x0409_0400 + i, format!("A rather long layout name {i}"))).collect();
        let laid = page(&m, 900.0, &measure);
        let chips: Vec<&Item> = laid.items.iter().filter(|i| matches!(i.kind, Kind::Pick { target: Target::Choice(Choice::KeyboardLayout(0), _), .. })).collect();
        assert_eq!(chips.len(), 8);
        assert!(chips.iter().any(|c| c.r.y > chips[0].r.y), "they wrap onto another line");
        for (j, chip) in chips.iter().enumerate() {
            assert_eq!(laid.hit(chip.r.x + 4.0, chip.r.y + 4.0), Some(Target::Choice(Choice::KeyboardLayout(0), j)));
        }
        // The second keyboard's layout is not installed any more: said so.
        assert!(laid.items.iter().any(|i| matches!(&i.kind, Kind::Label { desc: Some((d, Tone::Warn)), .. } if d.contains("no longer installed"))));
    }

    #[test]
    fn a_name_being_typed_is_a_field_beside_its_buttons() {
        let mut m = keyboards_model();
        m.renaming = Some((0, "Lap".into()));
        let laid = page(&m, 700.0, &measure);
        let field = laid.items.iter().find(|i| matches!(&i.kind, Kind::Field(t) if t == "Lap")).unwrap().r;
        let save = laid.items.iter().find(|i| matches!(i.kind, Kind::Button { target: Target::Button(Button::SaveName), .. })).unwrap().r;
        assert!(field.right() <= save.x, "the field stops before Save");
        assert!(!laid.hits.iter().any(|(_, t)| *t == Target::Button(Button::RenameKeyboard(0))));
    }

    #[test]
    fn a_capture_waits_with_a_way_out_and_nothing_is_added_while_off() {
        let mut m = keyboards_model();
        m.capture = Some(None);
        let laid = page(&m, 900.0, &measure);
        assert!(laid.hits.iter().any(|(_, t)| *t == Target::Button(Button::CancelCapture)));
        assert!(!laid.hits.iter().any(|(_, t)| *t == Target::Button(Button::AddKeyboard)));
        m.capture = None;
        m.config.keyboards.enabled = false;
        let laid = page(&m, 900.0, &measure);
        assert!(!laid.hits.iter().any(|(_, t)| matches!(t, Target::Button(Button::AddKeyboard) | Target::Choice(Choice::KeyboardLayout(_), _))));
    }
}

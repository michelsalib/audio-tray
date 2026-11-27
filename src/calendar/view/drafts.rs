//! Replies typed and not sent, by notification: one store for the panel's reply boxes and the
//! banners'. A draft outlives its box (the panel closes when it loses the foreground, a banner goes
//! on its time or when the panel opens over it) and comes back when that notification's box opens
//! in the panel again. Tray thread.

use std::cell::RefCell;

use super::editor::Editor;

thread_local! {
    static DRAFTS: RefCell<Vec<(u32, String)>> = const { RefCell::new(Vec::new()) };
}

/// Keep `text` as notification `id`'s draft, in place of any before. Nothing typed, nothing kept.
pub fn keep(id: u32, text: &str) {
    if text.is_empty() {
        return;
    }
    DRAFTS.with(|d| {
        let mut d = d.borrow_mut();
        d.retain(|(i, _)| *i != id);
        d.push((id, text.to_string()));
    });
}

/// Notification `id`'s draft, taken from the store into a box (empty without one).
pub fn take(id: u32) -> Editor {
    let kept = DRAFTS.with(|d| {
        let mut d = d.borrow_mut();
        d.iter().position(|(i, _)| *i == id).map(|at| d.remove(at).1)
    });
    Editor::new(kept.unwrap_or_default())
}

/// Only the drafts of notifications `listed` stay (the others were answered or dismissed).
pub fn retain_listed(listed: &[u32]) {
    DRAFTS.with(|d| d.borrow_mut().retain(|(id, _)| listed.contains(id)));
}

//! A one-line text box's model: what is typed, where the caret is, and what is selected. The reply
//! boxes (the panel's and a banner's) keep one each; [`super::scene::Draw::Field`] draws it with
//! the caret and the selection where DirectWrite measures them.
//!
//! The keys are Windows' own text box's: arrows (Ctrl by word, Shift selecting), Home and End,
//! Backspace and Delete (Ctrl by word), Ctrl+A, Ctrl+C, Ctrl+X, Ctrl+V.

/// What is typed, the caret and the selection's other end, as byte offsets on char boundaries.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Editor {
    text: String,
    caret: usize,
    /// Where the selection started; `None`, or the caret itself: nothing selected.
    anchor: Option<usize>,
}

/// A key a text box knows, from `WM_KEYDOWN`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Home,
    End,
    Delete,
}

/// A `WM_KEYDOWN` key the box knows, with Ctrl and Shift as held now.
fn from_vk(vk: u16) -> Option<(Key, bool, bool)> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VIRTUAL_KEY, VK_CONTROL, VK_DELETE, VK_END, VK_HOME, VK_LEFT, VK_RIGHT, VK_SHIFT};
    let key = match VIRTUAL_KEY(vk) {
        VK_LEFT => Key::Left,
        VK_RIGHT => Key::Right,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        VK_DELETE => Key::Delete,
        _ => return None,
    };
    let held = |k: VIRTUAL_KEY| unsafe { GetKeyState(k.0 as i32) } < 0;
    Some((key, held(VK_CONTROL), held(VK_SHIFT)))
}

impl Editor {
    /// `text`, the caret after it.
    pub fn new(text: String) -> Self {
        let caret = text.len();
        Self { text, caret, anchor: None }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The caret, in UTF-16 units (DirectWrite's positions).
    pub fn caret16(&self) -> usize {
        utf16_len(&self.text[..self.caret])
    }

    /// What is selected, start and end in UTF-16 units; `None` when nothing is.
    pub fn selection16(&self) -> Option<(usize, usize)> {
        let (a, b) = self.selection()?;
        Some((utf16_len(&self.text[..a]), utf16_len(&self.text[..b])))
    }

    fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor.filter(|a| *a != self.caret)?;
        Some((anchor.min(self.caret), anchor.max(self.caret)))
    }

    /// A character from `WM_CHAR`: typed text, or the control characters Windows sends for
    /// Backspace (with Ctrl: 0x7F), Ctrl+A, Ctrl+C, Ctrl+X and Ctrl+V. `false`: not the box's
    /// (Enter, Escape, Tab), left to the caller.
    pub fn char(&mut self, c: char) -> bool {
        match c {
            '\r' | '\n' | '\u{1b}' | '\t' => return false,
            '\u{1}' => {
                self.anchor = Some(0);
                self.caret = self.text.len();
            }
            '\u{3}' => self.copy(),
            '\u{18}' => {
                self.copy();
                self.delete_selection();
            }
            '\u{16}' => {
                let pasted = crate::win::clipboard_text().ok().flatten().unwrap_or_default();
                // One line: line breaks read as spaces.
                self.insert(&pasted.replace("\r\n", " ").replace(['\r', '\n'], " "));
            }
            '\u{8}' => self.erase(false, false),
            '\u{7f}' => self.erase(false, true),
            c if !c.is_control() => self.insert(c.encode_utf8(&mut [0; 4])),
            // Other control characters (Ctrl+letters) do nothing, but are the box's.
            _ => {}
        }
        true
    }

    /// A key from `WM_KEYDOWN`, with Ctrl and Shift as held.
    pub fn key(&mut self, key: Key, ctrl: bool, shift: bool) {
        if key == Key::Delete {
            self.erase(true, ctrl);
            return;
        }
        // Without Shift, an arrow ends a selection at its side.
        if !shift {
            if let Some((start, end)) = self.selection().filter(|_| matches!(key, Key::Left | Key::Right) && !ctrl) {
                self.caret = if key == Key::Left { start } else { end };
                self.anchor = None;
                return;
            }
        }
        let to = match key {
            Key::Left if ctrl => self.word_before(self.caret),
            Key::Left => self.prev(self.caret),
            Key::Right if ctrl => self.word_after(self.caret),
            Key::Right => self.next(self.caret),
            Key::Home => 0,
            Key::End => self.text.len(),
            Key::Delete => unreachable!(),
        };
        if shift {
            self.anchor.get_or_insert(self.caret);
        } else {
            self.anchor = None;
        }
        self.caret = to;
    }

    fn insert(&mut self, s: &str) {
        self.delete_selection();
        self.text.insert_str(self.caret, s);
        self.caret += s.len();
    }

    /// Backspace (`forward`: Delete), a character or (`word`) a word; a selection goes whole.
    fn erase(&mut self, forward: bool, word: bool) {
        if self.delete_selection() {
            return;
        }
        let (a, b) = match (forward, word) {
            (false, false) => (self.prev(self.caret), self.caret),
            (false, true) => (self.word_before(self.caret), self.caret),
            (true, false) => (self.caret, self.next(self.caret)),
            (true, true) => (self.caret, self.word_after(self.caret)),
        };
        self.text.replace_range(a..b, "");
        self.caret = a;
    }

    /// Remove what is selected; `false` when nothing was.
    fn delete_selection(&mut self) -> bool {
        let Some((a, b)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        self.text.replace_range(a..b, "");
        self.caret = a;
        self.anchor = None;
        true
    }

    fn copy(&self) {
        if let Some((a, b)) = self.selection() {
            let _ = crate::win::set_clipboard_text(&self.text[a..b]);
        }
    }

    fn prev(&self, at: usize) -> usize {
        self.text[..at].char_indices().next_back().map_or(0, |(i, _)| i)
    }

    fn next(&self, at: usize) -> usize {
        self.text[at..].chars().next().map_or(at, |c| at + c.len_utf8())
    }

    /// The start of the word before `at` (skipping the spaces just before it), as Ctrl+Left goes.
    fn word_before(&self, at: usize) -> usize {
        let head = self.text[..at].trim_end();
        head.char_indices().rev().find(|(_, c)| c.is_whitespace()).map_or(0, |(i, c)| i + c.len_utf8())
    }

    /// The start of the next word after `at`, as Ctrl+Right goes.
    fn word_after(&self, at: usize) -> usize {
        let rest = &self.text[at..];
        let word_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let spaces = rest[word_end..].find(|c: char| !c.is_whitespace()).unwrap_or(rest.len() - word_end);
        at + word_end + spaces
    }
}

/// What a key did to an open reply box, for the window holding it to follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Typed, moved or erased in it: drawn again.
    Edited,
    /// Enter: what is typed goes.
    Send,
    /// Escape: the box closes.
    Cancel,
    /// Not the box's (Tab, an arrow it has no use for): the window's own.
    NotMine,
}

/// A `WM_KEYDOWN` in window `hwnd` while its reply box `editor` is open: the caret's keys and
/// Delete edit it (the caret woken), Enter sends, Escape gives up. Typing comes as `WM_CHAR`
/// ([`on_char`]).
pub fn on_key(editor: &mut Editor, hwnd: windows::Win32::Foundation::HWND, vk: u16) -> Outcome {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_ESCAPE, VK_RETURN};
    if let Some((key, ctrl, shift)) = from_vk(vk) {
        editor.key(key, ctrl, shift);
        wake_caret(hwnd);
        return Outcome::Edited;
    }
    match VIRTUAL_KEY(vk) {
        VK_RETURN => Outcome::Send,
        VK_ESCAPE => Outcome::Cancel,
        _ => Outcome::NotMine,
    }
}

/// A `WM_CHAR` in window `hwnd` while its reply box `editor` is open: typed into it (the caret
/// woken). Enter and Escape are not: they came as keys ([`on_key`]).
pub fn on_char(editor: &mut Editor, hwnd: windows::Win32::Foundation::HWND, c: char) -> Outcome {
    if editor.char(c) {
        wake_caret(hwnd);
        Outcome::Edited
    } else {
        Outcome::NotMine
    }
}

/// [`CARET_TIMER`] in window `hwnd`: while its box is `open` the caret flips (`true`: draw it
/// again), else the timer stops.
pub fn caret_tick(hwnd: windows::Win32::Foundation::HWND, open: bool) -> bool {
    if open {
        blink();
    } else {
        let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(hwnd), CARET_TIMER) };
    }
    open
}

thread_local! {
    /// The caret's blink: drawn or not, for every box (one has the keyboard at a time).
    static CARET: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Whether the caret is drawn now.
pub fn caret_shown() -> bool {
    CARET.with(std::cell::Cell::get)
}

/// The timer that blinks the caret, on the window whose box has the keyboard.
pub const CARET_TIMER: usize = 0x4352;

/// The caret on again in `hwnd`'s box, a whole blink from now (after a key: it never blinks out
/// under typing). The window flips it on [`CARET_TIMER`] with [`blink`], and kills the timer
/// once its box is closed.
pub fn wake_caret(hwnd: windows::Win32::Foundation::HWND) {
    CARET.with(|c| c.set(true));
    if let Some(ms) = blink_ms() {
        unsafe { windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(hwnd), CARET_TIMER, ms, None) };
    }
}

/// A blink: the caret flips.
fn blink() {
    CARET.with(|c| c.set(!c.get()));
}

/// How often the caret blinks, as Windows is set (ms); `None`: it does not (set so for
/// accessibility).
pub fn blink_ms() -> Option<u32> {
    let ms = unsafe { windows::Win32::UI::WindowsAndMessaging::GetCaretBlinkTime() };
    (ms != 0 && ms != u32::MAX).then_some(ms.max(200))
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str) -> Editor {
        let mut e = Editor::default();
        for c in s.chars() {
            e.char(c);
        }
        e
    }

    #[test]
    fn typing_moves_the_caret_and_arrows_insert_in_the_middle() {
        let mut e = typed("helo");
        e.key(Key::Left, false, false);
        e.char('l');
        assert_eq!(e.text(), "hello");
        assert_eq!(e.caret16(), 4);
    }

    #[test]
    fn select_all_then_type_replaces_everything() {
        let mut e = typed("c'est fait aussi ");
        e.char('\u{1}');
        assert_eq!(e.selection16(), Some((0, 17)));
        e.char('o');
        e.char('k');
        assert_eq!(e.text(), "ok");
        assert_eq!(e.selection16(), None);
    }

    #[test]
    fn shift_arrows_select_and_backspace_removes_the_selection() {
        let mut e = typed("abc def");
        e.key(Key::Left, true, true);
        assert_eq!(e.selection16(), Some((4, 7)));
        e.char('\u{8}');
        assert_eq!(e.text(), "abc ");
    }

    #[test]
    fn words_by_ctrl() {
        let mut e = typed("one two  three");
        e.char('\u{7f}');
        assert_eq!(e.text(), "one two  ");
        e.key(Key::Home, false, false);
        e.key(Key::Right, true, false);
        assert_eq!(e.caret16(), 4);
        e.key(Key::Delete, true, false);
        assert_eq!(e.text(), "one ");
    }

    #[test]
    fn caret_counts_utf16_past_an_emoji() {
        let mut e = typed("a😀b");
        assert_eq!(e.caret16(), 4);
        e.key(Key::Left, false, false);
        e.key(Key::Left, false, false);
        assert_eq!(e.caret16(), 1);
        e.char('\u{8}');
        assert_eq!(e.text(), "😀b");
    }

    #[test]
    fn an_arrow_without_shift_collapses_the_selection_to_its_side() {
        let mut e = typed("abcd");
        e.char('\u{1}');
        e.key(Key::Left, false, false);
        assert_eq!((e.caret16(), e.selection16()), (0, None));
    }
}

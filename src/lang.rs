//! Audio Tray's words follow Windows' display language: French when it is French, English
//! otherwise. One rule for every panel, the menu and Settings (the calendar's own word tables
//! included); dates and times take Windows' formats whatever the language.

/// Whether Windows' display language is French (read once). Tests always run in English.
pub fn french() -> bool {
    static CHOSEN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // LANG_FRENCH, the primary language of the display language's id.
    let french = *CHOSEN.get_or_init(|| unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() } & 0x3ff == 0x0c);
    french && !cfg!(test)
}

/// `en`, or `fr` when Windows speaks French.
pub fn tr(en: &'static str, fr: &'static str) -> &'static str {
    if french() { fr } else { en }
}

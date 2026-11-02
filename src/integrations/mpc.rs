//! MPC-HC: it shares what it plays through Windows' media controls when its "Control via Windows
//! UI (SMTC)" option is on (Options > Player), and is then followed like any other player. The
//! option is off by default; Settings > Connectors turns it on in MPC-HC's own settings.
//!
//! Only the installed player's registry settings: a portable one keeps an ini next to its exe.

use windows::core::{w, PCWSTR};

const SETTINGS: PCWSTR = w!(r"Software\MPC-HC\MPC-HC\Settings");
const OPTION: PCWSTR = w!("UseSMTC");
/// The main window's class, to tell whether it runs.
const WINDOW_CLASS: &str = "MediaPlayerClassicW";

/// Whether MPC-HC's session is followed (`[mpc] enabled`); read on the music thread.
static FOLLOWED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_followed(on: bool) {
    FOLLOWED.store(on, std::sync::atomic::Ordering::SeqCst);
}

/// Whether the session of `app_id` is followed: any but MPC-HC's while it is switched off.
pub fn followed(app_id: &str) -> bool {
    FOLLOWED.load(std::sync::atomic::Ordering::SeqCst) || !is_mpc(app_id)
}

/// Whether `app_id` is an MPC player's (MPC-HC, or MPC-BE, which shares the same way).
pub fn is_mpc(app_id: &str) -> bool {
    let id = app_id.to_ascii_lowercase();
    id.contains("mpc-hc") || id.contains("mpc-be")
}

/// MPC-HC as found: `None` when it is not here (or too old to share), else whether it shares its media.
pub fn shares_media() -> Option<bool> {
    // Versions before the option (and players never started) have no value for it.
    crate::win::hkcu_dword(SETTINGS, OPTION).map(|on| on == 1)
}

/// Whether MPC-HC is open. It writes its settings back when it closes, over any made meanwhile.
pub fn running() -> bool {
    !crate::win::windows_by_class(WINDOW_CLASS).is_empty()
}

/// Turn MPC-HC's media controls on, for its next start. `Err` says what to do instead.
pub fn share_media() -> Result<(), &'static str> {
    use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD};
    if running() {
        return Err("Close MPC-HC first: it saves its own settings when it closes");
    }
    let on: u32 = 1;
    let set = unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, SETTINGS, OPTION, REG_DWORD.0, Some(&on as *const u32 as *const _), 4)
    };
    if set.is_ok() {
        Ok(())
    } else {
        Err("MPC-HC's settings could not be changed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The only test touching the switch, which is global.
    #[test]
    fn off_leaves_out_mpc_and_nothing_else() {
        set_followed(false);
        assert!(!followed("mpc-hc64.exe") && !followed("MPC-BE64.EXE"));
        assert!(followed("Spotify.exe"));
        set_followed(true);
        assert!(followed("mpc-hc64.exe"));
    }
}

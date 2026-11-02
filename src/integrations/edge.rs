//! Edge's sleeping tabs. A page Edge has put to sleep (paused and out of sight for a while: a
//! minimised YouTube Music window) still accepts play from Windows' media controls, and so from
//! the pill and the taskbar's buttons, but does nothing until its window is shown. Sites on Edge's
//! "Never put these sites to sleep" list are spared; Settings warns about the players that are not.
//!
//! Read from the last-used profile's `Preferences`, which Edge keeps while it runs: no API exposes
//! the list. A format change only hides the warning.

/// The players that matter, by a host they play on, and their names.
pub const PLAYERS: [(&str, &str); 3] = [("music.youtube.com", "YouTube Music"), ("www.youtube.com", "YouTube"), ("www.twitch.tv", "Twitch")];

/// The players Edge can put to sleep, by name. Empty when Edge is not here, sleeping tabs are off
/// (by policy), or every player is exempt.
pub fn players_at_risk() -> Vec<&'static str> {
    if sleeping_tabs_off_by_policy() {
        return Vec::new();
    }
    let Some(dir) = std::env::var_os("LOCALAPPDATA").map(|d| std::path::PathBuf::from(d).join(r"Microsoft\Edge\User Data")) else {
        return Vec::new();
    };
    let Ok(local_state) = std::fs::read_to_string(dir.join("Local State")) else {
        return Vec::new();
    };
    // A player is spared when any profile in use spares it: the exception is made in the profile it
    // plays in, and the others never see it.
    let mut risk: Option<Vec<&'static str>> = None;
    for profile in active_profiles(&local_state) {
        if let Ok(preferences) = std::fs::read_to_string(dir.join(profile).join("Preferences")) {
            let here = at_risk(&preferences);
            risk = Some(match risk {
                Some(so_far) => so_far.into_iter().filter(|name| here.contains(name)).collect(),
                None => here,
            });
        }
    }
    risk.unwrap_or_default()
}

/// Edge's `SleepingTabsEnabled` policy, set to 0.
fn sleeping_tabs_off_by_policy() -> bool {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].into_iter().any(|root| {
        let mut value = 1u32;
        let mut size = 4u32;
        let read = unsafe {
            RegGetValueW(root, w!(r"Software\Policies\Microsoft\Edge"), w!("SleepingTabsEnabled"), RRF_RT_REG_DWORD, None, Some(&mut value as *mut u32 as *mut _), Some(&mut size))
        };
        read.is_ok() && value == 0
    })
}

/// The profiles in use: the ones open last time, else the last used, else Edge's first.
fn active_profiles(local_state: &str) -> Vec<String> {
    let json: serde_json::Value = serde_json::from_str(local_state).unwrap_or_default();
    let mut profiles: Vec<String> = json
        .pointer("/profile/last_active_profiles")
        .and_then(|p| p.as_array())
        .map(|p| p.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    if profiles.is_empty() {
        profiles.push(json.pointer("/profile/last_used").and_then(|v| v.as_str()).unwrap_or("Default").to_string());
    }
    profiles
}

/// The players whose host no "never sleep" exception in `preferences` covers.
fn at_risk(preferences: &str) -> Vec<&'static str> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(preferences) else {
        return Vec::new();
    };
    let exempt: Vec<String> = json
        .pointer("/profile/content_settings/exceptions/sleeping_tabs")
        .and_then(|e| e.as_object())
        .map(|exceptions| {
            exceptions
                .iter()
                // 2: block, i.e. never put to sleep.
                .filter(|(_, v)| v.get("setting").and_then(|s| s.as_i64()) == Some(2))
                .map(|(pattern, _)| pattern.clone())
                .collect()
        })
        .unwrap_or_default();
    PLAYERS.iter().filter(|(host, _)| !exempt.iter().any(|p| covers(p, host))).map(|(_, name)| *name).collect()
}

/// Whether content-setting `pattern` (`twitch.tv,*`, `[*.]youtube.com,*`,
/// `https://music.youtube.com:443,*`) covers `host`. A bare domain covers its subdomains, as Edge's
/// list does.
fn covers(pattern: &str, host: &str) -> bool {
    let primary = pattern.split(',').next().unwrap_or("");
    let primary = primary.split("://").last().unwrap_or(primary);
    let primary = primary.trim_start_matches("[*.]").trim_start_matches("*.");
    let domain = primary.split(['/', ':']).next().unwrap_or("").to_ascii_lowercase();
    !domain.is_empty() && (host == domain || host.ends_with(&format!(".{domain}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exceptions_cover_their_domain_and_its_subdomains() {
        assert!(covers("twitch.tv,*", "www.twitch.tv"));
        assert!(covers("[*.]youtube.com,*", "music.youtube.com"));
        assert!(covers("https://music.youtube.com:443,*", "music.youtube.com"));
        assert!(!covers("https://music.youtube.com:443,*", "www.youtube.com"));
        assert!(!covers("notyoutube.com,*", "www.youtube.com"));
    }

    #[test]
    fn only_players_without_a_never_sleep_exception_are_at_risk() {
        let prefs = r#"{"profile":{"content_settings":{"exceptions":{"sleeping_tabs":{
            "twitch.tv,*":{"setting":2},
            "www.youtube.com,*":{"setting":1}
        }}}}}"#;
        assert_eq!(at_risk(prefs), vec!["YouTube Music", "YouTube"]);
        assert_eq!(at_risk(r#"{"profile":{}}"#), vec!["YouTube Music", "YouTube", "Twitch"]);
        assert!(at_risk("not json").is_empty(), "an unreadable file warns of nothing");
        assert_eq!(active_profiles(r#"{"profile":{"last_used":"Default","last_active_profiles":["Default","Profile 1"]}}"#), ["Default", "Profile 1"]);
        assert_eq!(active_profiles(r#"{"profile":{"last_used":"Profile 2"}}"#), ["Profile 2"]);
        assert_eq!(active_profiles("{}"), ["Default"]);
    }
}

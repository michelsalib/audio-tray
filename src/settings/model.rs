//! What the settings window shows: a copy of the config and what it is read against (the devices,
//! the displays, the connectors' state). Plain data, so the layout can be tested.

use crate::audio::wasapi::WasapiBackend;
use crate::audio::{DeviceId, Flow};
use crate::config::Config;
use crate::icons::IconId;
use crate::win::Monitor;

use super::layout::{Button, Page};

/// How a card's second line reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Tone {
    Plain,
    /// Working: connected, ready.
    Good,
    /// Something to do or fix.
    Warn,
}

pub(super) struct DeviceRow {
    pub id: DeviceId,
    pub name: String,
    pub icon: IconId,
    pub flow: Flow,
    pub default: bool,
}

/// What the hub knows about the connectors, and the taskbar's trouble.
#[derive(Clone, Default, PartialEq)]
pub(super) struct Status {
    pub integrations: Vec<(&'static str, bool)>,
    pub discord_problem: Option<&'static str>,
    pub teams_problem: Option<&'static str>,
    pub notice: Option<&'static str>,
    /// The connected devices that report a battery.
    pub batteries: Vec<crate::hub::Battery>,
}

impl Status {
    pub(super) fn of(state: &crate::hub::State) -> Self {
        Status {
            integrations: state.integrations.clone(),
            discord_problem: state.problem(crate::integrations::discord_rpc::NAME),
            teams_problem: state.problem(crate::integrations::teams::NAME),
            notice: crate::taskbar::current_notice(),
            batteries: state.batteries.clone(),
        }
    }
}

pub(super) struct Model {
    pub config: Config,
    pub page: Page,
    pub devices: Vec<DeviceRow>,
    /// The device whose icons are showing.
    pub expanded: Option<usize>,
    pub monitors: Vec<Monitor>,
    pub status: Status,
    /// A staged update's version.
    pub update: Option<String>,
    /// A one-off message in place of a button's value (a paste that found the wrong thing).
    pub hint: Option<(Button, &'static str, Tone)>,
    /// The players Edge can put to sleep ([`crate::integrations::edge`]).
    pub edge_sleep: Vec<&'static str>,
    /// The connector whose card is open.
    pub open: Option<super::layout::Connector>,
    /// MPC-HC: `None` when it is not here, else whether it shares what it plays.
    pub mpc: Option<bool>,
    /// The installed keyboard layouts, as `(handle, label)`.
    pub layouts: Vec<(u64, String)>,
    /// The interface paths of the keyboards connected now.
    pub connected: Vec<String>,
    /// The assigned keyboard being renamed, and its name so far.
    pub renaming: Option<(usize, String)>,
    /// Waiting for a key on the keyboard to add, with what to say about the last one pressed.
    pub capture: Option<Option<String>>,
    /// The keyboard a capture adds a connection to; `None` adds a new keyboard.
    pub capture_into: Option<usize>,
    /// The calendar's accounts and links, as last fetched or signed in.
    pub calendar: crate::calendar::Status,
}

impl Model {
    pub(super) fn new(config: Config) -> Self {
        Model {
            config,
            page: Page::Home,
            devices: Vec::new(),
            expanded: None,
            monitors: Vec::new(),
            status: Status::default(),
            update: None,
            hint: None,
            open: None,
            edge_sleep: crate::integrations::edge::players_at_risk(),
            mpc: crate::integrations::mpc::shares_media(),
            layouts: Vec::new(),
            connected: Vec::new(),
            renaming: None,
            capture: None,
            capture_into: None,
            calendar: crate::calendar::Status::default(),
        }
    }

    /// Read the installed layouts and the connected keyboards again.
    pub(super) fn read_keyboards(&mut self) {
        let installed = crate::keyboards::layouts();
        self.layouts = installed.iter().map(|layout| (layout.hkl, crate::keyboards::label(layout, &installed))).collect();
        self.connected = crate::keyboards::keyboards().into_iter().map(|keyboard| keyboard.path).collect();
    }

    /// The layout for a keyboard just added: the first one no other keyboard types in, else the first.
    pub(super) fn unused_layout(&self) -> Option<u64> {
        let used: Vec<u64> = self.config.keyboards.assigned.iter().filter_map(|k| k.hkl()).collect();
        self.layouts.iter().map(|(hkl, _)| *hkl).find(|hkl| !used.contains(hkl)).or_else(|| self.layouts.first().map(|(hkl, _)| *hkl))
    }

    /// The secret's last characters, when there is one: it is never shown whole.
    pub(super) fn secret_tail(&self) -> Option<String> {
        self.config.discord.client_secret.as_deref().map(|s| {
            let tail: String = s.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
            format!("••••{tail}")
        })
    }

    /// Read the output and input devices again, keeping the open one open.
    pub(super) fn read_devices(&mut self, backend: &WasapiBackend) {
        let open = self.expanded.and_then(|i| self.devices.get(i)).map(|d| d.id.clone());
        let mut rows = Vec::new();
        for flow in [Flow::Output, Flow::Input] {
            let default = backend.default_of(flow).ok().flatten();
            for d in backend.enumerate_flow(flow).unwrap_or_default() {
                rows.push(DeviceRow {
                    icon: self.config.icon_of(&d),
                    default: default.as_ref() == Some(&d.id),
                    id: d.id,
                    name: d.friendly_name,
                    flow,
                });
            }
        }
        self.devices = rows;
        self.expanded = open.and_then(|id| self.devices.iter().position(|d| d.id == id));
    }
}

/// Whether the clipboard's `text` is the Discord key `button` wants, else what to say instead.
pub(super) fn paste_key(button: Button, text: &str) -> Result<(), &'static str> {
    let id_like = (15..=21).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit());
    let secret_like = (24..=64).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    match button {
        Button::DiscordId if id_like => Ok(()),
        Button::DiscordId if secret_like => Err("That is the secret: copy the client ID"),
        Button::DiscordId => Err("Copy the client ID first, then paste"),
        _ if id_like => Err("That is the ID: copy the client secret"),
        _ if secret_like => Ok(()),
        _ => Err("Copy the client secret first, then paste"),
    }
}

/// What the clipboard should hold for one of the calendar's Paste buttons.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum CalendarKey {
    GoogleId,
    GoogleSecret,
    MicrosoftId,
    Link,
}

/// The clipboard's `text` as the calendar key `key` wants it, else what to say instead.
pub(super) fn paste_calendar(key: CalendarKey, text: &str) -> Result<String, &'static str> {
    let text = text.trim();
    let google_id = text.ends_with(".apps.googleusercontent.com") && !text.contains(char::is_whitespace);
    let google_secret = text.starts_with("GOCSPX-") && text.len() < 80;
    let guid = text.len() == 36
        && text.char_indices().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == '-' } else { c.is_ascii_hexdigit() });
    let link = {
        let lower = text.to_ascii_lowercase();
        (lower.starts_with("https://") || lower.starts_with("webcal://") || lower.starts_with("webcals://") || lower.starts_with("http://"))
            && !text.contains(char::is_whitespace)
    };
    match key {
        CalendarKey::GoogleId if google_id => Ok(text.to_string()),
        CalendarKey::GoogleId if google_secret => Err("That is the secret: copy the client ID"),
        CalendarKey::GoogleId => Err("Copy the client ID (…apps.googleusercontent.com) first"),
        CalendarKey::GoogleSecret if google_secret => Ok(text.to_string()),
        CalendarKey::GoogleSecret if google_id => Err("That is the ID: copy the client secret"),
        CalendarKey::GoogleSecret => Err("Copy the client secret (GOCSPX-…) first"),
        CalendarKey::MicrosoftId if guid => Ok(text.to_ascii_lowercase()),
        CalendarKey::MicrosoftId => Err("Copy the Application (client) ID from its Overview first"),
        // Google's and Outlook's calendar pages (embed, HTML) are not feeds.
        CalendarKey::Link if link && !looks_like_feed(text) => Err("That is a web page: copy the calendar's secret iCal (.ics) address"),
        CalendarKey::Link if link => Ok(text.to_string()),
        CalendarKey::Link => Err("Copy the calendar's address (https:// or webcal://) first"),
    }
}

/// Whether a calendar address is a feed rather than a page about one. Only Google's and Outlook's
/// own pages are recognised; any other address is taken at its word.
fn looks_like_feed(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let host = lower.split("://").nth(1).and_then(|rest| rest.split('/').next()).unwrap_or_default();
    if host.ends_with("calendar.google.com") {
        return lower.contains("/ical/");
    }
    if host.contains("outlook.") || host.contains("office365.com") || host.contains("office.com") {
        return lower.split('?').next().is_some_and(|path| path.ends_with(".ics"));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_pages_are_told_from_feeds() {
        assert!(paste_calendar(CalendarKey::Link, "https://calendar.google.com/calendar/embed?src=me%40gmail.com&ctz=Europe%2FParis").is_err());
        assert!(paste_calendar(CalendarKey::Link, "https://calendar.google.com/calendar/ical/me%40gmail.com/private-abc/basic.ics").is_ok());
        assert!(paste_calendar(CalendarKey::Link, "https://outlook.office365.com/owa/calendar/a@b.com/c/calendar.html").is_err());
        assert!(paste_calendar(CalendarKey::Link, "https://outlook.office365.com/owa/calendar/a@b.com/c/calendar.ics").is_ok());
        assert!(paste_calendar(CalendarKey::Link, "https://example.com/feed").is_ok());
    }

    #[test]
    fn calendar_keys_must_look_like_the_one_asked_for() {
        let id = "123-abc.apps.googleusercontent.com";
        assert_eq!(paste_calendar(CalendarKey::GoogleId, &format!(" {id}\n")).as_deref(), Ok(id));
        assert!(paste_calendar(CalendarKey::GoogleId, "GOCSPX-abc").is_err());
        assert!(paste_calendar(CalendarKey::GoogleSecret, "GOCSPX-abcDEF123").is_ok());
        assert!(paste_calendar(CalendarKey::GoogleSecret, id).is_err());
        assert_eq!(
            paste_calendar(CalendarKey::MicrosoftId, "0F1E2D3C-4B5A-6978-8796-A5B4C3D2E1F0").as_deref(),
            Ok("0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0")
        );
        assert!(paste_calendar(CalendarKey::MicrosoftId, "not-a-guid").is_err());
        assert!(paste_calendar(CalendarKey::Link, "webcal://example.com/cal.ics").is_ok());
        assert!(paste_calendar(CalendarKey::Link, "my calendar").is_err());
    }

    #[test]
    fn a_pasted_key_must_look_like_the_one_asked_for() {
        let (id, secret) = ("1234567890123456789", "abcdEFGH1234ijklMNOP5678qrstUVWX");
        assert_eq!(paste_key(Button::DiscordId, id), Ok(()));
        assert_eq!(paste_key(Button::DiscordSecret, secret), Ok(()));
        assert!(paste_key(Button::DiscordId, secret).is_err());
        assert!(paste_key(Button::DiscordSecret, id).is_err());
        assert!(paste_key(Button::DiscordId, "").is_err());
        assert!(paste_key(Button::DiscordSecret, "https://discord.com/developers").is_err());
    }

    #[test]
    fn the_secret_shows_only_its_end() {
        let mut m = Model::new(Config::default());
        assert_eq!(m.secret_tail(), None);
        m.config.discord.client_secret = Some("abcdefghWXYZ".into());
        assert_eq!(m.secret_tail().as_deref(), Some("••••WXYZ"));
    }
}

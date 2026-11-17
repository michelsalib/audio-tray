//! Where events come from: a Google account (Calendar API), a Microsoft work or personal account
//! (Graph), and calendar links (ICS feeds). Accounts are the user's own app registrations
//! (`[calendar]` in config.toml); each signed-in account's refresh token is kept in the Credential
//! Manager, its access token only in memory.

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::{Calendar, Details, Event, Source, When};
use super::oauth::{self, Client};
use super::time::{self, Zone};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Provider {
    Google,
    Microsoft,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::Google, Provider::Microsoft];

    pub fn name(self) -> &'static str {
        match self {
            Provider::Google => "Google",
            Provider::Microsoft => "Microsoft",
        }
    }

    pub fn source(self) -> Source {
        match self {
            Provider::Google => Source::Google,
            Provider::Microsoft => Source::Microsoft,
        }
    }

    fn secret_name(self) -> &'static str {
        match self {
            Provider::Google => crate::secrets::CALENDAR_GOOGLE,
            Provider::Microsoft => crate::secrets::CALENDAR_MICROSOFT,
        }
    }
}

/// A calendar link: a name of the user's and the feed's address (a secret: it reads the calendar).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub name: String,
    pub url: String,
}

/// What a fetch needs from the config.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Setup {
    pub google_id: Option<String>,
    pub google_secret: Option<String>,
    pub microsoft_id: Option<String>,
    /// `common`, `organizations`, `consumers` or a tenant id.
    pub tenant: String,
    pub links: Vec<Link>,
}

impl Setup {
    /// Whether `provider`'s app registration is filled in.
    pub fn registered(&self, provider: Provider) -> bool {
        match provider {
            Provider::Google => self.google_id.is_some() && self.google_secret.is_some(),
            Provider::Microsoft => self.microsoft_id.is_some(),
        }
    }
}

/// A signed-in account, as kept in the Credential Manager.
#[derive(Clone, Serialize, Deserialize)]
struct Grant {
    email: String,
    refresh: String,
}

fn load_grant(provider: Provider) -> Option<Grant> {
    serde_json::from_str(&crate::secrets::get(provider.secret_name())?).ok()
}

fn save_grant(provider: Provider, grant: Option<&Grant>) {
    let text = grant.and_then(|g| serde_json::to_string(g).ok());
    crate::secrets::set(provider.secret_name(), text.as_deref());
}

/// The address of the account signed in with `provider`, if any.
pub fn signed_in(provider: Provider) -> Option<String> {
    load_grant(provider).map(|g| g.email)
}

/// Access tokens in hand, with their expiry.
static ACCESS: Mutex<Vec<(Provider, String, i64)>> = Mutex::new(Vec::new());

fn cached_access(provider: Provider) -> Option<String> {
    let now = time::now();
    let access = ACCESS.lock().unwrap_or_else(|e| e.into_inner());
    access.iter().find(|(p, _, expires)| *p == provider && *expires > now + 60).map(|(_, token, _)| token.clone())
}

fn keep_access(provider: Provider, token: &str, expires: i64) {
    let mut access = ACCESS.lock().unwrap_or_else(|e| e.into_inner());
    access.retain(|(p, ..)| *p != provider);
    access.push((provider, token.to_string(), expires));
}

/// The endpoints and registration `provider` signs in with, owned (Microsoft's carry the tenant).
struct Registration {
    authorize: String,
    token: String,
    id: String,
    secret: Option<String>,
}

const GOOGLE_SCOPE: &str = "openid email https://www.googleapis.com/auth/calendar.readonly";
const MICROSOFT_SCOPE: &str = "offline_access User.Read Calendars.Read";

fn registration(provider: Provider, setup: &Setup) -> Result<Registration> {
    match provider {
        Provider::Google => Ok(Registration {
            authorize: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token: "https://oauth2.googleapis.com/token".into(),
            id: setup.google_id.clone().context("no Google client ID")?,
            secret: Some(setup.google_secret.clone().context("no Google client secret")?),
        }),
        Provider::Microsoft => {
            let tenant = if setup.tenant.is_empty() { "common" } else { setup.tenant.as_str() };
            Ok(Registration {
                authorize: format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize"),
                token: format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token"),
                id: setup.microsoft_id.clone().context("no Microsoft application ID")?,
                secret: None,
            })
        }
    }
}

fn client<'a>(provider: Provider, r: &'a Registration) -> Client<'a> {
    match provider {
        Provider::Google => Client {
            authorize: &r.authorize,
            token: &r.token,
            id: &r.id,
            secret: r.secret.as_deref(),
            scope: GOOGLE_SCOPE,
            host: "127.0.0.1",
            // Offline: a refresh token; consent: one every time, not only the first.
            extra: &[("access_type", "offline"), ("prompt", "consent")],
        },
        Provider::Microsoft => Client {
            authorize: &r.authorize,
            token: &r.token,
            id: &r.id,
            secret: None,
            scope: MICROSOFT_SCOPE,
            host: "localhost",
            extra: &[("prompt", "select_account")],
        },
    }
}

/// Sign in with `provider` in the browser; the account's address on success.
pub fn sign_in(provider: Provider, setup: &Setup, cancel: &AtomicBool) -> Result<String> {
    let registration = registration(provider, setup)?;
    let tokens = oauth::sign_in(&client(provider, &registration), cancel)?;
    let refresh = tokens.refresh.clone().context("the account gave no refresh token")?;
    let email = match provider {
        Provider::Google => tokens.id_token.as_deref().and_then(oauth::jwt_claims).and_then(|c| c["email"].as_str().map(str::to_string)),
        Provider::Microsoft => {
            let me = oauth::get_json("https://graph.microsoft.com/v1.0/me?$select=mail,userPrincipalName", &tokens.access, &[])?;
            me["mail"].as_str().or(me["userPrincipalName"].as_str()).map(str::to_string)
        }
    }
    .unwrap_or_else(|| provider.name().to_string());
    keep_access(provider, &tokens.access, tokens.expires);
    save_grant(provider, Some(&Grant { email: email.clone(), refresh }));
    Ok(email)
}

/// Forget `provider`'s account.
pub fn sign_out(provider: Provider) {
    save_grant(provider, None);
    ACCESS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(p, ..)| *p != provider);
}

/// Why an account needs the user: signed out on the provider's side.
#[derive(Debug)]
pub struct SignInAgain;

impl std::fmt::Display for SignInAgain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Signed out: sign in again")
    }
}

impl std::error::Error for SignInAgain {}

/// A valid access token for `provider`'s account, refreshed when needed.
fn access(provider: Provider, setup: &Setup) -> Result<String> {
    if let Some(token) = cached_access(provider) {
        return Ok(token);
    }
    let mut grant = load_grant(provider).context("not signed in")?;
    let registration = registration(provider, setup)?;
    let tokens = match oauth::refresh(&client(provider, &registration), &grant.refresh) {
        Ok(tokens) => tokens,
        Err(e) if oauth::needs_sign_in(&e) => return Err(e.context(SignInAgain)),
        Err(e) => return Err(e),
    };
    // Microsoft rotates its refresh tokens: keep the newest.
    if let Some(refresh) = tokens.refresh.filter(|r| *r != grant.refresh) {
        grant.refresh = refresh;
        save_grant(provider, Some(&grant));
    }
    keep_access(provider, &tokens.access, tokens.expires);
    Ok(tokens.access)
}

/// `provider`'s events between two instants, and its calendars.
pub fn fetch(provider: Provider, setup: &Setup, range: (i64, i64), zone: &dyn Zone) -> Result<(Vec<Event>, Vec<Calendar>)> {
    let run = |token: &str| match provider {
        Provider::Google => google_events(token, range),
        Provider::Microsoft => microsoft_events(token, range, zone),
    };
    let token = access(provider, setup)?;
    match run(&token) {
        // The token was revoked early: one fresh try.
        Err(e) if oauth::unauthorized(&e) => {
            ACCESS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(p, ..)| *p != provider);
            run(&access(provider, setup)?)
        }
        done => done,
    }
}

fn rfc3339(secs: i64) -> String {
    let (y, m, d) = time::civil_from_days(time::day_of(secs));
    let s = time::second_of_day(secs);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3600, s / 60 % 60, s % 60)
}

fn hex_color(text: &str) -> Option<[u8; 3]> {
    let hex = text.trim().trim_start_matches('#');
    let n = u32::from_str_radix(hex.get(..6)?, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

/// Google's own default event colour, for a calendar that names none.
const GOOGLE_BLUE: [u8; 3] = [0x7B, 0x86, 0xC6];
/// Outlook's calendar blue, lightened to read on a dark panel.
const OUTLOOK_BLUE: [u8; 3] = [0x47, 0x9E, 0xF5];
/// The links' colours, in turn.
pub const LINK_COLORS: [[u8; 3]; 4] = [[0xE8, 0x83, 0x6A], [0x6C, 0xCB, 0x5F], [0xC3, 0x9B, 0xF5], [0xF7, 0xB3, 0x2B]];

fn google_events(token: &str, (from, to): (i64, i64)) -> Result<(Vec<Event>, Vec<Calendar>)> {
    let list = oauth::get_json(
        "https://www.googleapis.com/calendar/v3/users/me/calendarList?minAccessRole=reader&fields=items(id,summary,summaryOverride,backgroundColor,selected,primary,hidden)",
        token,
        &[],
    )?;
    let mut events = Vec::new();
    let mut calendars = Vec::new();
    for calendar in list["items"].as_array().into_iter().flatten() {
        // The calendars shown in Google Calendar itself.
        let shown = calendar["selected"].as_bool().unwrap_or(false) || calendar["primary"].as_bool().unwrap_or(false);
        if !shown || calendar["hidden"].as_bool().unwrap_or(false) {
            continue;
        }
        let Some(id) = calendar["id"].as_str() else { continue };
        let color = calendar["backgroundColor"].as_str().and_then(hex_color).unwrap_or(GOOGLE_BLUE);
        let key = format!("google:{id}");
        let name = calendar["summaryOverride"].as_str().or(calendar["summary"].as_str()).unwrap_or(id).to_string();
        calendars.push(Calendar { key: key.clone(), name, color, source: Source::Google });
        let mut page: Option<String> = None;
        loop {
            let mut url = format!(
                "https://www.googleapis.com/calendar/v3/calendars/{}/events?singleEvents=true&orderBy=startTime&maxResults=2500&timeMin={}&timeMax={}\
                 &fields=nextPageToken,items(summary,description,start,end,location,htmlLink,hangoutLink,status,organizer(displayName,email),attendees(email,displayName,self,responseStatus),conferenceData/entryPoints)",
                oauth::encode(id),
                oauth::encode(&rfc3339(from)),
                oauth::encode(&rfc3339(to))
            );
            if let Some(token) = &page {
                url.push_str(&format!("&pageToken={}", oauth::encode(token)));
            }
            let answer = match oauth::get_json(&url, token, &[]) {
                Ok(answer) => answer,
                // A calendar shared and then withdrawn: the others still count.
                Err(e) if e.downcast_ref::<oauth::Refused>().is_some_and(|r| r.0 == 404 || r.0 == 403) => break,
                Err(e) => return Err(e),
            };
            events.extend(answer["items"].as_array().into_iter().flatten().filter_map(|item| google_event(item, color, &key)));
            match answer["nextPageToken"].as_str() {
                Some(next) => page = Some(next.to_string()),
                None => break,
            }
        }
    }
    Ok((events, calendars))
}

fn google_event(item: &Value, color: [u8; 3], key: &str) -> Option<Event> {
    if item["status"] == "cancelled" || declined(item["attendees"].as_array(), |a| a["self"].as_bool() == Some(true), |a| a["responseStatus"].as_str()) {
        return None;
    }
    let when = match (item["start"]["date"].as_str(), item["start"]["dateTime"].as_str()) {
        (Some(first), _) => {
            let first = time::day_of(time::parse_iso(first)?.0);
            let end = item["end"]["date"].as_str().and_then(time::parse_iso).map_or(first + 1, |(e, _)| time::day_of(e));
            When::AllDay { first, end: end.max(first + 1) }
        }
        (None, Some(start)) => {
            let start = time::parse_iso(start)?.0;
            let end = item["end"]["dateTime"].as_str().and_then(time::parse_iso).map_or(start, |(e, _)| e);
            When::At { start, end: end.max(start) }
        }
        _ => return None,
    };
    let video = item["conferenceData"]["entryPoints"]
        .as_array()
        .and_then(|points| points.iter().find(|p| p["entryPointType"] == "video"))
        .and_then(|p| p["uri"].as_str());
    Some(Event {
        when,
        title: item["summary"].as_str().unwrap_or("(No title)").to_string(),
        location: item["location"].as_str().unwrap_or_default().to_string(),
        link: item["htmlLink"].as_str().map(str::to_string),
        join: item["hangoutLink"].as_str().or(video).map(str::to_string),
        source: Source::Google,
        color,
        calendar: key.to_string(),
        details: Details {
            people: item["attendees"].as_array().into_iter().flatten().filter(|a| a["self"].as_bool() != Some(true)).filter_map(|a| a["displayName"].as_str().or(a["email"].as_str()).map(str::to_string)).collect(),
            organizer: item["organizer"]["displayName"].as_str().or(item["organizer"]["email"].as_str()).unwrap_or_default().to_string(),
            response: item["attendees"].as_array().into_iter().flatten().find(|a| a["self"].as_bool() == Some(true)).and_then(|a| a["responseStatus"].as_str()).map(response_word).unwrap_or_default(),
            notes: plain_text(item["description"].as_str().unwrap_or_default()),
        },
    })
}

fn declined(attendees: Option<&Vec<Value>>, me: impl Fn(&Value) -> bool, response: impl Fn(&Value) -> Option<&str>) -> bool {
    attendees.into_iter().flatten().any(|a| me(a) && response(a) == Some("declined"))
}

/// This PC's time zone as Windows names it (`Romance Standard Time`), which Graph accepts.
fn windows_zone_name() -> String {
    use windows::Win32::System::Time::{GetDynamicTimeZoneInformation, DYNAMIC_TIME_ZONE_INFORMATION};
    let mut info = DYNAMIC_TIME_ZONE_INFORMATION::default();
    unsafe { GetDynamicTimeZoneInformation(&mut info) };
    let name = String::from_utf16_lossy(&info.TimeZoneKeyName);
    let name = name.trim_end_matches('\0').to_string();
    if name.is_empty() { "UTC".into() } else { name }
}

fn microsoft_events(token: &str, (from, to): (i64, i64), zone: &dyn Zone) -> Result<(Vec<Event>, Vec<Calendar>)> {
    // In local time: an all-day event's dates are then the ones it shows on.
    let prefer = format!("outlook.timezone=\"{}\"", windows_zone_name());
    let mut url = format!(
        "https://graph.microsoft.com/v1.0/me/calendarView?startDateTime={}&endDateTime={}&$top=250\
         &$select=subject,start,end,isAllDay,location,webLink,onlineMeeting,isCancelled,responseStatus,organizer,attendees,bodyPreview&$orderby=start/dateTime",
        oauth::encode(&rfc3339(from)),
        oauth::encode(&rfc3339(to))
    );
    let mut events = Vec::new();
    for _ in 0..20 {
        let answer = oauth::get_json(&url, token, &[("Prefer", &prefer)])?;
        events.extend(answer["value"].as_array().into_iter().flatten().filter_map(|item| microsoft_event(item, zone)));
        match answer["@odata.nextLink"].as_str() {
            Some(next) => url = next.to_string(),
            None => break,
        }
    }
    let calendars = vec![Calendar { key: OUTLOOK_KEY.into(), name: "Outlook".into(), color: OUTLOOK_BLUE, source: Source::Microsoft }];
    Ok((events, calendars))
}

fn microsoft_event(item: &Value, zone: &dyn Zone) -> Option<Event> {
    if item["isCancelled"].as_bool() == Some(true) || item["responseStatus"]["response"] == "declined" {
        return None;
    }
    let start = time::parse_iso(item["start"]["dateTime"].as_str()?)?;
    let end = item["end"]["dateTime"].as_str().and_then(time::parse_iso).unwrap_or(start);
    // Wall-clock times on the zone asked for, unless one came with an offset.
    let instant = |(secs, utc): (i64, bool)| if utc { secs } else { zone.to_utc(secs) };
    let when = if item["isAllDay"].as_bool() == Some(true) {
        let first = time::day_of(start.0);
        When::AllDay { first, end: time::day_of(end.0).max(first + 1) }
    } else {
        let start = instant(start);
        When::At { start, end: instant(end).max(start) }
    };
    Some(Event {
        when,
        title: item["subject"].as_str().filter(|s| !s.is_empty()).unwrap_or("(No title)").to_string(),
        location: item["location"]["displayName"].as_str().unwrap_or_default().to_string(),
        link: item["webLink"].as_str().map(str::to_string),
        join: item["onlineMeeting"]["joinUrl"].as_str().map(str::to_string),
        source: Source::Microsoft,
        color: OUTLOOK_BLUE,
        calendar: OUTLOOK_KEY.into(),
        details: Details {
            people: item["attendees"].as_array().into_iter().flatten().filter_map(|a| a["emailAddress"]["name"].as_str().or(a["emailAddress"]["address"].as_str()).map(str::to_string)).collect(),
            organizer: item["organizer"]["emailAddress"]["name"].as_str().unwrap_or_default().to_string(),
            response: item["responseStatus"]["response"].as_str().map(response_word).unwrap_or_default(),
            notes: item["bodyPreview"].as_str().unwrap_or_default().trim().to_string(),
        },
    })
}

/// A link as fetched: `webcal://` is https.
pub fn feed_url(url: &str) -> String {
    let url = url.trim();
    match url.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("webcal") || scheme.eq_ignore_ascii_case("webcals") => format!("https://{rest}"),
        _ => url.to_string(),
    }
}

/// Calendar link `index`'s events on the local days `[from, to)`, and the calendar itself.
pub fn fetch_link(index: usize, link: &Link, days: (i32, i32), zone: &dyn Zone) -> Result<(Vec<Event>, Calendar)> {
    let text = oauth::get_text(&feed_url(&link.url))?;
    if !text.contains("BEGIN:VCALENDAR") {
        bail!("not a calendar");
    }
    let color = LINK_COLORS[index % LINK_COLORS.len()];
    let key = link_key(&link.url);
    let feed = feed_url(&link.url);
    let events = super::ics::parse(&text, days, zone)
        .into_iter()
        .map(|item| Event {
            link: item.link.or_else(|| web_link(&feed, &item.uid, &item.when, zone)),
            when: item.when,
            title: if item.title.is_empty() { "(No title)".into() } else { item.title },
            location: item.location,
            join: item.join,
            source: Source::Link(index),
            color,
            calendar: key.clone(),
            details: item.details,
        })
        .collect();
    Ok((events, Calendar { key, name: link.name.clone(), color, source: Source::Link(index) }))
}

/// Where a linked calendar's event opens on the web, when its feed gives no `URL`: the event itself
/// for Google's private and public addresses (its `eid` is the event id and the calendar id, in
/// base64), the event's day for Outlook's published calendars (their `UID`s are not item ids).
fn web_link(feed: &str, uid: &str, when: &When, zone: &dyn Zone) -> Option<String> {
    let lower = feed.to_ascii_lowercase();
    if let Some(at) = lower.find("calendar.google.com/calendar/ical/") {
        let rest = &feed[at + "calendar.google.com/calendar/ical/".len()..];
        let calendar = percent_decode(rest.split('/').next()?);
        let id = uid.strip_suffix("@google.com")?;
        let eid = super::oauth::base64url(format!("{id} {calendar}").as_bytes()).replace('-', "%2B").replace('_', "%2F");
        return Some(format!("https://calendar.google.com/calendar/event?eid={eid}"));
    }
    let day = match *when {
        When::AllDay { first, .. } => first,
        When::At { start, .. } => time::day_of(zone.to_local(start)),
    };
    let (y, m, d) = time::civil_from_days(day);
    if lower.contains("outlook.office365.com/") || lower.contains("outlook.office.com/") {
        return Some(format!("https://outlook.office.com/calendar/view/day/{y}/{m}/{d}"));
    }
    if lower.contains("outlook.live.com/") {
        return Some(format!("https://outlook.live.com/calendar/0/view/day/{y}/{m}/{d}"));
    }
    None
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%' && i + 2 < bytes.len()).then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())).flatten();
        match hex {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The Microsoft account's calendar's key.
pub const OUTLOOK_KEY: &str = "outlook";

/// A link's key: a hash of its address, which is a secret and stays out of config.toml.
pub fn link_key(url: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(feed_url(url).as_bytes());
    format!("link:{}", digest[..6].iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// An attendee's reply, as the card says it.
fn response_word(code: &str) -> String {
    match code {
        "accepted" => "Accepted",
        "tentative" | "tentativelyAccepted" => "Maybe",
        "declined" => "Declined",
        "needsAction" | "notResponded" | "none" => "Not answered",
        "organizer" => "Organiser",
        _ => "",
    }
    .to_string()
}

/// A description as plain text: Google sends HTML.
pub fn plain_text(html: &str) -> String {
    let text = html.replace("<br>", "\n").replace("<br/>", "\n").replace("</p>", "\n").replace("</li>", "\n");
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&nbsp;", " ");
    out.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

/// A pasted link's name: its host, or the calendar's when the feed says (filled in on fetch).
pub fn link_name(url: &str) -> String {
    let url = feed_url(url);
    let host = url.split("://").nth(1).and_then(|rest| rest.split('/').next()).unwrap_or("Calendar");
    match host {
        h if h.ends_with("google.com") => "Google calendar".into(),
        h if h.contains("outlook") || h.contains("office365") || h.contains("office.com") => "Outlook calendar".into(),
        h => h.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::time::{days_from_civil, Fixed, DAY};
    use serde_json::json;

    #[test]
    fn linked_events_open_on_the_web_where_the_feed_lives() {
        let when = When::At { start: days_from_civil(2026, 11, 20) as i64 * DAY + 18 * 3600, end: days_from_civil(2026, 11, 20) as i64 * DAY + 19 * 3600 };
        let google = "https://calendar.google.com/calendar/ical/me%40gmail.com/private-abc/basic.ics";
        let link = web_link(google, "7abc@google.com", &when, &Fixed(0)).expect("a Google link");
        let eid = link.strip_prefix("https://calendar.google.com/calendar/event?eid=").expect("an event link");
        let decoded = oauth::base64url_decode(&eid.replace("%2B", "-").replace("%2F", "_")).expect("base64");
        assert_eq!(String::from_utf8(decoded).unwrap(), "7abc me@gmail.com");
        // Not a Google event id: no guess.
        assert_eq!(web_link(google, "other-uid", &when, &Fixed(0)), None);
        let outlook = "https://outlook.office365.com/owa/calendar/x/y/calendar.ics";
        assert_eq!(web_link(outlook, "z", &when, &Fixed(0)).as_deref(), Some("https://outlook.office.com/calendar/view/day/2026/11/20"));
        assert_eq!(web_link("https://example.com/cal.ics", "z", &when, &Fixed(0)), None);
    }

    #[test]
    fn google_events_read_both_shapes_and_skip_declined_ones() {
        let timed = json!({"summary":"Call","start":{"dateTime":"2026-11-13T10:00:00+01:00"},"end":{"dateTime":"2026-11-13T11:00:00+01:00"},
            "hangoutLink":"https://meet.google.com/abc","htmlLink":"https://calendar.google.com/x"});
        let event = google_event(&timed, [1, 2, 3], "google:me").unwrap();
        let base = days_from_civil(2026, 11, 13) as i64 * DAY;
        assert_eq!(event.when, When::At { start: base + 9 * 3600, end: base + 10 * 3600 });
        assert_eq!(event.join.as_deref(), Some("https://meet.google.com/abc"));
        let day = json!({"summary":"Off","start":{"date":"2026-11-13"},"end":{"date":"2026-11-15"}});
        assert_eq!(google_event(&day, [0; 3], "google:me").unwrap().when, When::AllDay { first: days_from_civil(2026, 11, 13), end: days_from_civil(2026, 11, 15) });
        let declined = json!({"summary":"No","start":{"date":"2026-11-13"},"attendees":[{"self":true,"responseStatus":"declined"}]});
        assert!(google_event(&declined, [0; 3], "google:me").is_none());
    }

    #[test]
    fn event_details_are_read_and_descriptions_become_plain_text() {
        let item = json!({"summary":"Review","start":{"dateTime":"2026-11-13T10:00:00Z"},"end":{"dateTime":"2026-11-13T11:00:00Z"},
            "description":"Agenda:<br>1. Calendar &amp; clock<br><b>Notes</b>","organizer":{"displayName":"Ana"},
            "attendees":[{"email":"me@x.com","self":true,"responseStatus":"tentative"},{"displayName":"Jo","responseStatus":"accepted"}]});
        let event = google_event(&item, [0; 3], "google:me").unwrap();
        assert_eq!(event.details.people, vec!["Jo".to_string()]);
        assert_eq!(event.details.organizer, "Ana");
        assert_eq!(event.details.response, "Maybe");
        assert_eq!(event.details.notes, "Agenda:\n1. Calendar & clock\nNotes");
        assert_eq!(event.calendar, "google:me");
        assert!(link_key("webcal://a/b.ics") == link_key("https://a/b.ics") && link_key("https://a/b.ics").starts_with("link:"));
    }

    #[test]
    fn graph_events_are_read_on_the_local_clock() {
        let zone = Fixed(3600);
        let item = json!({"subject":"Standup","start":{"dateTime":"2026-11-13T09:00:00.0000000","timeZone":"Romance Standard Time"},
            "end":{"dateTime":"2026-11-13T09:15:00.0000000"},"isAllDay":false,"onlineMeeting":{"joinUrl":"https://teams.microsoft.com/l/x"},
            "location":{"displayName":"Teams"}});
        let event = microsoft_event(&item, &zone).unwrap();
        let base = days_from_civil(2026, 11, 13) as i64 * DAY;
        assert_eq!(event.when, When::At { start: base + 8 * 3600, end: base + 8 * 3600 + 900 });
        assert_eq!(event.location, "Teams");
        let all_day = json!({"subject":"Holiday","isAllDay":true,"start":{"dateTime":"2026-11-11T00:00:00.0000000"},"end":{"dateTime":"2026-11-12T00:00:00.0000000"}});
        assert_eq!(microsoft_event(&all_day, &zone).unwrap().when, When::AllDay { first: days_from_civil(2026, 11, 11), end: days_from_civil(2026, 11, 12) });
        assert!(microsoft_event(&json!({"isCancelled":true,"start":{"dateTime":"2026-11-11T00:00:00"}}), &zone).is_none());
    }

    #[test]
    fn links_are_fetched_over_https_and_named_by_host() {
        assert_eq!(feed_url("webcal://p01-calendars.icloud.com/x.ics"), "https://p01-calendars.icloud.com/x.ics");
        assert_eq!(link_name("https://calendar.google.com/calendar/ical/x/private-y/basic.ics"), "Google calendar");
        assert_eq!(link_name("https://outlook.office365.com/owa/calendar/a/b/calendar.ics"), "Outlook calendar");
        assert_eq!(rfc3339(days_from_civil(2026, 11, 1) as i64 * DAY + 3661), "2026-11-01T01:01:01Z");
    }
}

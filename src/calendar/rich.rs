//! What a notification offers beyond its text: its pictures, its reply box and its buttons.
//!
//! The listener gives an unpackaged app only a notification's text lines. Windows keeps each one
//! exactly as the app sent it (its toast XML) in its own store, `wpndatabase.db`, by the same id
//! the listener uses: that payload is read here, read-only, once per notification.
//!
//! **Pictures** come from wherever the payload points: the web (WhatsApp's contact photos), a file,
//! or the app's own package (`ms-appx:`, `ms-appdata:`).
//!
//! **Buttons and replies** go back to the app the way Windows' own centre sends them: through the
//! COM class the app registers to receive them (`ToastActivatorCLSID` in a package's manifest, or
//! `CustomActivator` under its AUMID for a desktop app), called with the button's arguments and the
//! text typed. An app without one can only be opened; a `protocol` button opens its address.

use std::sync::{Arc, Mutex};

use windows::core::{GUID, HSTRING, PCWSTR};

/// How a button is carried out, as the payload's `activationType` says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    /// The app comes up (or is told to bring something up).
    #[default]
    Foreground,
    /// The app is told quietly (a reply, a mute).
    Background,
    /// An address is opened.
    Protocol,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub label: String,
    pub arguments: String,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    /// The input's id, under which the text goes back.
    pub input: String,
    pub placeholder: String,
    pub send: Action,
}

/// A picture's address (also its cache key) and its file's bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub key: String,
    pub bytes: Arc<Vec<u8>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rich {
    /// What a click on the notification itself sends.
    pub launch: String,
    pub launch_kind: Kind,
    /// The sender's photo (`appLogoOverride`), shown round.
    pub avatar: Option<Picture>,
    /// A large picture (`hero`, or an inline image).
    pub picture: Option<Picture>,
    pub reply: Option<Reply>,
    /// Buttons under the notification, and those of its menu.
    pub buttons: Vec<Action>,
    pub menu: Vec<Action>,
    /// The app's activator, when it has one: buttons and replies can reach it.
    pub activator: Option<u128>,
}

/// The payload's parts, before pictures are fetched.
#[derive(Debug, Default, PartialEq)]
struct Parts {
    launch: String,
    launch_kind: Kind,
    avatar: Option<String>,
    picture: Option<String>,
    reply: Option<Reply>,
    buttons: Vec<Action>,
    menu: Vec<Action>,
}

/// A notification read: its extras without pictures, and where its pictures come from (they are
/// looked up at each read, so one downloaded meanwhile shows).
struct Known {
    rich: Rich,
    avatar: Option<String>,
    picture: Option<String>,
    base: Base,
}

/// What is known of each notification already read, by id.
static READ: Mutex<Vec<(u32, Known)>> = Mutex::new(Vec::new());
/// Notifications not found in Windows' store, and how many reads looked for them.
static MISSES: Mutex<Vec<(u32, u8)>> = Mutex::new(Vec::new());
/// Reads that look for a notification missing from the store before it stays plain.
const MISSES_MAX: u8 = 5;

/// Each notification's extras, read once (what is no longer listed is forgotten).
pub fn for_toasts(wanted: &[(u32, String, Option<windows::ApplicationModel::AppInfo>)]) -> Vec<Rich> {
    let mut read = READ.lock().unwrap_or_else(|e| e.into_inner());
    read.retain(|(id, _)| wanted.iter().any(|(w, ..)| w == id));
    let mut misses = MISSES.lock().unwrap_or_else(|e| e.into_inner());
    misses.retain(|(id, _)| wanted.iter().any(|(w, ..)| w == id));
    let missing: Vec<u32> = wanted.iter().map(|(id, ..)| *id).filter(|id| !read.iter().any(|(r, _)| r == id)).collect();
    let payloads = if missing.is_empty() { Vec::new() } else { payloads(&missing).unwrap_or_else(|e| {
        eprintln!("notifications: could not read Windows' store ({e:#})");
        Vec::new()
    }) };
    for (id, aumid, info) in wanted {
        if read.iter().any(|(r, _)| r == id) {
            continue;
        }
        let Some((_, xml)) = payloads.iter().find(|(p, _)| p == id) else {
            // Not in the store (yet): plain, and looked for again a few times.
            let tries = match misses.iter_mut().find(|(m, _)| m == id) {
                Some((_, n)) => {
                    *n += 1;
                    *n
                }
                None => {
                    misses.push((*id, 1));
                    1
                }
            };
            if tries >= MISSES_MAX {
                println!("notifications: {id} ({aumid}) never reached Windows' store; shown plain");
                read.push((*id, Known { rich: Rich::default(), avatar: None, picture: None, base: Base::default() }));
            }
            continue;
        };
        let parts = parse(xml);
        let base = Base::of(info.as_ref());
        let rich = Rich {
            launch: parts.launch,
            launch_kind: parts.launch_kind,
            avatar: None,
            picture: None,
            reply: parts.reply,
            buttons: parts.buttons,
            menu: parts.menu,
            activator: activator(aumid, &base),
        };
        read.push((*id, Known { rich, avatar: parts.avatar, picture: parts.picture, base }));
    }
    let out: Vec<Rich> = wanted
        .iter()
        .map(|(id, ..)| {
            let Some((_, known)) = read.iter().find(|(r, _)| r == id) else { return Rich::default() };
            Rich {
                avatar: known.avatar.as_deref().and_then(|src| picture(src, &known.base)),
                picture: known.picture.as_deref().and_then(|src| picture(src, &known.base)),
                ..known.rich.clone()
            }
        })
        .collect();
    // Pictures no listed notification shows are let go.
    let shown: Vec<&str> = read.iter().flat_map(|(_, k)| [k.avatar.as_deref(), k.picture.as_deref()]).flatten().collect();
    PICTURES.lock().unwrap_or_else(|e| e.into_inner()).retain(|(src, _)| shown.contains(&src.as_str()));
    out
}

/// The payloads of notifications `ids`, from Windows' store.
fn payloads(ids: &[u32]) -> anyhow::Result<Vec<(u32, String)>> {
    use rusqlite::{types::ValueRef, Connection, OpenFlags};
    let path = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or_else(|| anyhow::anyhow!("no LOCALAPPDATA"))?)
        .join(r"Microsoft\Windows\Notifications\wpndatabase.db");
    let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    db.busy_timeout(std::time::Duration::from_millis(500))?;
    let mut query = db.prepare("SELECT Payload FROM Notification WHERE Id = ?1")?;
    let mut out = Vec::new();
    for &id in ids {
        let payload = query.query_row([id], |row| {
            Ok(match row.get_ref(0)? {
                ValueRef::Text(b) | ValueRef::Blob(b) => String::from_utf8_lossy(b).into_owned(),
                _ => String::new(),
            })
        });
        if let Ok(xml) = payload {
            out.push((id, xml));
        }
    }
    Ok(out)
}

/// The tags of an XML document, in order, with their attributes (entities decoded). Enough for a
/// toast payload: no namespaces, text content or CDATA are needed.
fn tags(xml: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut out = Vec::new();
    let mut chars = xml.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != '<' {
            continue;
        }
        // To the closing '>', not counting one inside quotes.
        let mut quote = None;
        let mut end = None;
        for (j, d) in chars.by_ref() {
            match (quote, d) {
                (None, '"' | '\'') => quote = Some(d),
                (Some(q), d) if d == q => quote = None,
                (None, '>') => {
                    end = Some(j);
                    break;
                }
                _ => {}
            }
        }
        let Some(end) = end else { break };
        let inner = &xml[i + 1..end];
        if inner.starts_with(['/', '?', '!']) {
            continue;
        }
        let inner = inner.trim_end_matches('/').trim();
        let (name, mut rest) = inner.split_once(char::is_whitespace).unwrap_or((inner, ""));
        let mut attrs = Vec::new();
        while let Some(eq) = rest.find('=') {
            let key = rest[..eq].trim().to_string();
            let after = rest[eq + 1..].trim_start();
            let Some(q) = after.chars().next().filter(|q| *q == '"' || *q == '\'') else { break };
            let Some(close) = after[1..].find(q) else { break };
            attrs.push((key, unescape(&after[1..1 + close])));
            rest = &after[close + 2..];
        }
        out.push((name.to_string(), attrs));
    }
    out
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.find(';').filter(|s| *s <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn kind_of(value: Option<&str>) -> Kind {
    match value {
        Some("background") => Kind::Background,
        Some("protocol") => Kind::Protocol,
        _ => Kind::Foreground,
    }
}

/// A toast payload's launch, pictures, reply box and buttons.
fn parse(xml: &str) -> Parts {
    let mut parts = Parts::default();
    let mut inputs: Vec<(String, String)> = Vec::new();
    let mut sends: Vec<(String, Action)> = Vec::new();
    for (name, attrs) in tags(xml) {
        let get = |key: &str| attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        match name.as_str() {
            "toast" => {
                parts.launch = get("launch").unwrap_or_default().to_string();
                parts.launch_kind = kind_of(get("activationType"));
            }
            "image" => {
                let Some(src) = get("src").filter(|s| !s.is_empty()) else { continue };
                match get("placement") {
                    Some("appLogoOverride") => parts.avatar = Some(src.to_string()),
                    Some("hero") => parts.picture = Some(src.to_string()),
                    None | Some("inline") if parts.picture.is_none() => parts.picture = Some(src.to_string()),
                    _ => {}
                }
            }
            "input" if get("type").unwrap_or("text") == "text" => {
                if let Some(id) = get("id") {
                    inputs.push((id.to_string(), get("placeHolderContent").unwrap_or_default().to_string()));
                }
            }
            "action" => {
                // Snooze and dismiss belong to Windows' own centre.
                if get("activationType") == Some("system") {
                    continue;
                }
                let action = Action {
                    label: get("content").unwrap_or_default().to_string(),
                    arguments: get("arguments").unwrap_or_default().to_string(),
                    kind: kind_of(get("activationType")),
                };
                if let Some(input) = get("hint-inputId") {
                    sends.push((input.to_string(), action));
                } else if get("placement") == Some("contextMenu") {
                    parts.menu.push(action);
                } else {
                    parts.buttons.push(action);
                }
            }
            _ => {}
        }
    }
    // A reply box is a text input with a button sending it.
    if let Some((input, placeholder)) = inputs.into_iter().find(|(id, _)| sends.iter().any(|(s, _)| s == id)) {
        let send = sends.iter().find(|(s, _)| *s == input).map(|(_, a)| a.clone()).expect("just found");
        parts.reply = Some(Reply { input, placeholder, send });
    }
    parts
}

/// Where an app's own files are, for its pictures and its manifest.
#[derive(Clone, Default)]
struct Base {
    /// The package's install folder and family name, for a packaged app.
    installed: Option<String>,
    family: Option<String>,
}

impl Base {
    fn of(info: Option<&windows::ApplicationModel::AppInfo>) -> Self {
        let family = info.and_then(|i| i.PackageFamilyName().ok()).map(|s| s.to_string()).filter(|s| !s.is_empty());
        let installed = family
            .as_ref()
            .and_then(|_| info?.Package().ok())
            .and_then(|p| p.InstalledPath().ok())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());
        Base { installed, family }
    }
}

/// Pictures fetched so far, by address (`None`: there is none), for the listed notifications only.
static PICTURES: Mutex<Vec<(String, Option<Bytes>)>> = Mutex::new(Vec::new());
/// Web pictures being downloaded, on threads of their own.
static DOWNLOADING: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// A file's bytes, shared.
type Bytes = Arc<Vec<u8>>;
/// No picture bigger than this is kept.
const PICTURE_MAX: u64 = 4 * 1024 * 1024;
/// Nor more than this in all: the oldest go first.
const PICTURES_MAX: usize = 32 * 1024 * 1024;

fn picture(src: &str, base: &Base) -> Option<Picture> {
    if let Some((_, bytes)) = PICTURES.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(s, _)| s == src) {
        return bytes.clone().map(|bytes| Picture { key: src.to_string(), bytes });
    }
    if is_web(src) {
        // Downloaded aside, so a slow host never holds up the list; the list is read again when
        // it has come.
        let mut downloading = DOWNLOADING.lock().unwrap_or_else(|e| e.into_inner());
        if !downloading.iter().any(|s| s == src) {
            downloading.push(src.to_string());
            let src = src.to_string();
            let _ = std::thread::Builder::new().name("toast-picture".into()).spawn(move || {
                let bytes = download(&src);
                keep(&src, bytes);
                DOWNLOADING.lock().unwrap_or_else(|e| e.into_inner()).retain(|s| *s != src);
                super::notifications::refresh();
            });
        }
        return None;
    }
    let bytes = keep(src, fetch(src, base));
    bytes.map(|bytes| Picture { key: src.to_string(), bytes })
}

/// Cache a picture's bytes (or that there is none), within [`PICTURES_MAX`].
fn keep(src: &str, bytes: Option<Vec<u8>>) -> Option<Bytes> {
    let bytes = bytes.filter(|b| !b.is_empty()).map(Arc::new);
    let mut held = PICTURES.lock().unwrap_or_else(|e| e.into_inner());
    held.retain(|(s, _)| s != src);
    held.push((src.to_string(), bytes.clone()));
    let size = |held: &[(String, Option<Bytes>)]| held.iter().filter_map(|(_, b)| b.as_ref()).map(|b| b.len()).sum::<usize>();
    while held.len() > 1 && size(&held) > PICTURES_MAX {
        held.remove(0);
    }
    bytes
}

fn is_web(src: &str) -> bool {
    let lower = src.get(..8).unwrap_or(src).to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

fn download(src: &str) -> Option<Vec<u8>> {
    // Short: a slow picture host is given up on.
    let request = super::oauth::agent().get(src).config().timeout_global(Some(std::time::Duration::from_secs(10))).build();
    let mut response = request.call().ok()?;
    if response.status() != 200 {
        return None;
    }
    response.body_mut().with_config().limit(PICTURE_MAX).read_to_vec().ok()
}

/// A picture in a file: the app's own (`ms-appx`, `ms-appdata`) or one on a local drive.
fn fetch(src: &str, base: &Base) -> Option<Vec<u8>> {
    let path = if let Some(rest) = strip_scheme(src, "file:///") {
        percent_decoded(rest)
    } else if let Some(rest) = strip_scheme(src, "ms-appx:///") {
        format!(r"{}\{}", base.installed.as_ref()?, percent_decoded(rest))
    } else if let Some(rest) = strip_scheme(src, "ms-appdata:///") {
        let (store, rest) = rest.split_once('/')?;
        let folder = match store.to_ascii_lowercase().as_str() {
            "local" => "LocalState",
            "roaming" => "RoamingState",
            "temp" => "TempState",
            _ => return None,
        };
        let app_data = std::env::var("LOCALAPPDATA").ok()?;
        format!(r"{app_data}\Packages\{}\{folder}\{}", base.family.as_ref()?, percent_decoded(rest))
    } else {
        src.to_string()
    };
    let path = path.replace('/', r"\");
    if !on_a_local_drive(&path) {
        // `\\server\share`: looking would sign in to that server, as the user.
        return None;
    }
    let size = std::fs::metadata(&path).ok()?.len();
    (size <= PICTURE_MAX).then(|| std::fs::read(&path).ok()).flatten()
}

/// `C:\…`: a path on a drive letter, never a share (`\\server\…`, `\\?\UNC\…`) or a device.
fn on_a_local_drive(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}

fn strip_scheme<'a>(src: &'a str, scheme: &str) -> Option<&'a str> {
    src.get(..scheme.len()).filter(|s| s.eq_ignore_ascii_case(scheme)).map(|_| &src[scheme.len()..])
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Activators found so far, by app.
static ACTIVATORS: Mutex<Vec<(String, Option<u128>)>> = Mutex::new(Vec::new());

/// The COM class an app registers to receive its notifications' buttons.
fn activator(aumid: &str, base: &Base) -> Option<u128> {
    if let Some((_, clsid)) = ACTIVATORS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(a, _)| a == aumid) {
        return *clsid;
    }
    let from_manifest = || -> Option<u128> {
        let manifest = std::fs::read_to_string(format!(r"{}\AppxManifest.xml", base.installed.as_ref()?)).ok()?;
        let app = aumid.rsplit_once('!').map(|(_, app)| app);
        // The application's own entry first, then any.
        let mut current_app: Option<String> = None;
        let mut any = None;
        for (name, attrs) in tags(&manifest) {
            let get = |key: &str| attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
            if name == "Application" {
                current_app = get("Id");
            } else if name.ends_with("ToastNotificationActivation") {
                let clsid = get("ToastActivatorCLSID").and_then(|c| parse_guid(&c));
                if current_app.as_deref() == app && clsid.is_some() {
                    return clsid;
                }
                any = any.or(clsid);
            }
        }
        any
    };
    let clsid = from_manifest().or_else(|| registry_activator(aumid));
    ACTIVATORS.lock().unwrap_or_else(|e| e.into_inner()).push((aumid.to_string(), clsid));
    clsid
}

/// `HKCU\Software\Classes\AppUserModelId\<aumid>\CustomActivator`, where desktop apps register it.
fn registry_activator(aumid: &str) -> Option<u128> {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let key = crate::win::wide(&format!(r"Software\Classes\AppUserModelId\{aumid}"));
    let name = crate::win::wide("CustomActivator");
    let mut buf = [0u16; 64];
    let mut size = (buf.len() * 2) as u32;
    let read = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), PCWSTR(name.as_ptr()), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut size))
    };
    if read.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    parse_guid(&String::from_utf16_lossy(&buf[..len]))
}

/// `{bebd6a5e-1b14-4a57-a195-c772d28b6d15}` (braces optional).
fn parse_guid(text: &str) -> Option<u128> {
    let hex: String = text.trim().trim_matches(['{', '}']).chars().filter(|c| *c != '-').collect();
    (hex.len() == 32).then(|| u128::from_str_radix(&hex, 16).ok()).flatten()
}

/// Carry out a notification's button (or its click, or a reply with `inputs`), as Windows' centre
/// would. On a thread of its own: the app's activator may take a moment to start.
pub fn invoke(aumid: &str, activator: Option<u128>, arguments: &str, kind: Kind, inputs: Vec<(String, String)>) {
    let (aumid, arguments) = (aumid.to_string(), arguments.to_string());
    let _ = std::thread::Builder::new().name("notification action".into()).spawn(move || {
        if kind == Kind::Protocol {
            crate::win::shell_open(&arguments, None);
            return;
        }
        let Some(clsid) = activator else {
            if kind == Kind::Foreground {
                super::notifications::open_app(&aumid);
            } else {
                eprintln!("notifications: {aumid} takes no buttons from outside Windows' centre");
            }
            return;
        };
        if let Err(e) = activate(&aumid, clsid, &arguments, kind, &inputs) {
            eprintln!("notifications: {aumid} did not take the action ({e:#})");
            if kind == Kind::Foreground {
                super::notifications::open_app(&aumid);
            }
        }
    });
}

fn activate(aumid: &str, clsid: u128, arguments: &str, kind: Kind, inputs: &[(String, String)]) -> windows::core::Result<()> {
    use windows::Win32::System::Com::{CoAllowSetForegroundWindow, CoCreateInstance, CoInitializeEx, CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::UI::Notifications::{INotificationActivationCallback, NOTIFICATION_USER_INPUT_DATA};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let callback: INotificationActivationCallback = CoCreateInstance(&GUID::from_u128(clsid), None, CLSCTX_LOCAL_SERVER)?;
        if kind == Kind::Foreground {
            // As Windows' centre does: the app may come to the front.
            let _ = CoAllowSetForegroundWindow(&callback, None);
        }
        let held: Vec<(HSTRING, HSTRING)> = inputs.iter().map(|(k, v)| (HSTRING::from(k.as_str()), HSTRING::from(v.as_str()))).collect();
        let data: Vec<NOTIFICATION_USER_INPUT_DATA> = held.iter().map(|(k, v)| NOTIFICATION_USER_INPUT_DATA { Key: PCWSTR(k.as_ptr()), Value: PCWSTR(v.as_ptr()) }).collect();
        callback.Activate(&HSTRING::from(aumid), &HSTRING::from(arguments), &data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_are_read_from_local_drives_only() {
        assert!(on_a_local_drive(r"C:\Users\a\b.png"));
        assert!(!on_a_local_drive(r"\\server\share\b.png"));
        assert!(!on_a_local_drive(r"\\?\UNC\server\share\b.png"));
        assert!(!on_a_local_drive(r"\\.\pipe\x"));
        assert!(!on_a_local_drive("b.png"));
    }

    const WHATSAPP: &str = r#"<toast launch="action=Click&amp;key=1"><visual><binding template="ToastGeneric"><text>Pierre</text><text>Plouf</text><image src="https://x/a.jpg?s=96&amp;t=1" placement="appLogoOverride" hint-crop="circle" /></binding></visual><audio src="ms-appx:///Sounds/a.m4a" /><actions><input id="quickReplyInputBoxId" type="text" placeHolderContent="Saisir une réponse" /><action content="Envoyer" arguments="action=Reply&amp;k=1" activationType="background" hint-inputId="quickReplyInputBoxId" /><action content="Mode silencieux" arguments="action=Mute" activationType="background" placement="contextMenu" /><action content="Snooze" arguments="snooze" activationType="system" /></actions></toast>"#;

    #[test]
    fn a_whatsapp_payload_gives_its_photo_reply_box_and_menu() {
        let parts = parse(WHATSAPP);
        assert_eq!(parts.launch, "action=Click&key=1");
        assert_eq!(parts.avatar.as_deref(), Some("https://x/a.jpg?s=96&t=1"));
        let reply = parts.reply.expect("a reply box");
        assert_eq!(reply.input, "quickReplyInputBoxId");
        assert_eq!(reply.placeholder, "Saisir une réponse");
        assert_eq!(reply.send.arguments, "action=Reply&k=1");
        assert_eq!(reply.send.kind, Kind::Background);
        assert!(parts.buttons.is_empty());
        assert_eq!(parts.menu.len(), 1);
        assert_eq!(parts.menu[0].label, "Mode silencieux");
    }

    #[test]
    fn hero_pictures_and_protocol_buttons_are_read() {
        let parts = parse(r#"<toast launch="ms-screensketch:edit?a=1" activationType="protocol"><visual><binding template="ToastGeneric"><image placement="hero" src="file:///C%3A%5Cx.png" alt="a > b"></image></binding></visual><actions><action content="Share" arguments="ms-x:share" activationType="protocol"/></actions></toast>"#);
        assert_eq!(parts.launch_kind, Kind::Protocol);
        assert_eq!(parts.picture.as_deref(), Some("file:///C%3A%5Cx.png"));
        assert_eq!(parts.buttons, vec![Action { label: "Share".into(), arguments: "ms-x:share".into(), kind: Kind::Protocol }]);
        assert_eq!(percent_decoded("C%3A%5Cx%20y.png"), r"C:\x y.png");
    }

    #[test]
    fn entities_and_guids_decode() {
        assert_eq!(unescape("a &amp; b &#233; &#x41; &bogus c"), "a & b é A &bogus c");
        assert_eq!(parse_guid("{bebd6a5e-1b14-4a57-a195-c772d28b6d15}"), Some(0xbebd6a5e_1b14_4a57_a195_c772d28b6d15));
        assert_eq!(parse_guid("nope"), None);
    }
}

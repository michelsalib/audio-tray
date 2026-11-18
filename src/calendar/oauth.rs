//! OAuth 2.0 for an installed app: the authorization-code flow with PKCE, the code arriving on a
//! loopback redirect (a one-request HTTP listener on `127.0.0.1`), as Google and Microsoft both
//! document for desktop apps. Plus the JSON-over-HTTPS the calendars speak, on `ureq` with Windows'
//! own TLS.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

/// How long a sign-in waits for the browser.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

/// An app registration and the endpoints it signs in at.
pub struct Client<'a> {
    pub authorize: &'a str,
    pub token: &'a str,
    pub id: &'a str,
    /// Google's desktop clients have one (not confidential, but required); Microsoft's do not.
    pub secret: Option<&'a str>,
    pub scope: &'a str,
    /// The redirect's host: Google takes `127.0.0.1`, Microsoft registers `localhost`.
    pub host: &'a str,
    /// More query parameters for the authorization page.
    pub extra: &'a [(&'a str, &'a str)],
}

pub struct Tokens {
    pub access: String,
    pub refresh: Option<String>,
    /// When the access token expires, in seconds since the epoch.
    pub expires: i64,
    pub id_token: Option<String>,
}

/// Sign in through the user's browser: open the provider's page, wait for its redirect back to
/// the loopback listener, and exchange the code. `cancel` gives up early.
pub fn sign_in(client: &Client, cancel: &AtomicBool) -> Result<Tokens> {
    let listener = TcpListener::bind("127.0.0.1:0").context("could not listen for the browser")?;
    let port = listener.local_addr()?.port();
    // `localhost` may resolve to ::1 first: listen there too, on the same port, when possible.
    let listener6 = (client.host == "localhost").then(|| TcpListener::bind(("::1", port)).ok()).flatten();
    let redirect = format!("http://{}:{port}", client.host);
    let verifier = base64url(&random_bytes(32));
    let challenge = base64url(&sha256(verifier.as_bytes()));
    let state = base64url(&random_bytes(16));

    let mut url = format!("{}?", client.authorize);
    let mut params: Vec<(&str, &str)> = vec![
        ("client_id", client.id),
        ("response_type", "code"),
        ("redirect_uri", &redirect),
        ("scope", client.scope),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("state", &state),
    ];
    params.extend_from_slice(client.extra);
    url.push_str(&params.iter().map(|(k, v)| format!("{k}={}", encode(v))).collect::<Vec<_>>().join("&"));
    if !crate::win::shell_open(&url, None) {
        bail!("could not open the browser");
    }

    let code = wait_for_code(&listener, listener6.as_ref(), &state, cancel)?;
    let mut form: Vec<(&str, &str)> = vec![
        ("client_id", client.id),
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", &redirect),
        ("code_verifier", &verifier),
    ];
    if let Some(secret) = client.secret {
        form.push(("client_secret", secret));
    }
    token_request(client.token, &form)
}

/// A new access token from a refresh token. The answer may carry a new refresh token too.
pub fn refresh(client: &Client, refresh_token: &str) -> Result<Tokens> {
    let mut form: Vec<(&str, &str)> = vec![
        ("client_id", client.id),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("scope", client.scope),
    ];
    if let Some(secret) = client.secret {
        form.push(("client_secret", secret));
    }
    token_request(client.token, &form)
}

/// The refusal a token endpoint gave, when it means the grant is gone (revoked, expired): the
/// account has to sign in again.
pub fn needs_sign_in(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}");
    text.contains("invalid_grant") || text.contains("interaction_required") || text.contains("unauthorized_client")
}

fn token_request(url: &str, form: &[(&str, &str)]) -> Result<Tokens> {
    let mut response = agent()
        .post(url)
        .config()
        .http_status_as_error(false)
        .build()
        .send_form(form.iter().copied())
        .context("could not reach the sign-in service")?;
    let value: Value = serde_json::from_str(&response.body_mut().read_to_string()?).context("the sign-in service's answer")?;
    if let Some(error) = value["error"].as_str() {
        let why = value["error_description"].as_str().unwrap_or_default();
        // Microsoft's descriptions run on with trace and correlation ids.
        let why = why.split(['\r', '\n']).next().unwrap_or_default();
        bail!("{error}: {why}");
    }
    Ok(Tokens {
        access: value["access_token"].as_str().context("no access token")?.to_string(),
        refresh: value["refresh_token"].as_str().map(str::to_string),
        expires: super::time::now() + value["expires_in"].as_i64().unwrap_or(3600),
        id_token: value["id_token"].as_str().map(str::to_string),
    })
}

/// Accept connections until one brings the authorization code (a browser may ask for a favicon
/// first), answering each with a page the user sees in place of the provider's.
fn wait_for_code(listener: &TcpListener, listener6: Option<&TcpListener>, state: &str, cancel: &AtomicBool) -> Result<String> {
    listener.set_nonblocking(true)?;
    if let Some(l) = listener6 {
        l.set_nonblocking(true)?;
    }
    let deadline = Instant::now() + SIGN_IN_TIMEOUT;
    loop {
        if cancel.load(Ordering::SeqCst) {
            bail!("cancelled");
        }
        if Instant::now() > deadline {
            bail!("the browser did not come back in time");
        }
        let accepted = listener.accept().or_else(|e| listener6.map_or(Err(e), |l| l.accept()));
        let mut stream = match accepted {
            Ok((stream, _)) => stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => return Err(e).context("the browser's connection"),
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let Some(target) = read_request_target(&mut stream) else { continue };
        let (path, query) = target.split_once('?').unwrap_or((&target, ""));
        let params = parse_query(query);
        let get = |key: &str| params.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        if path != "/" || (get("code").is_none() && get("error").is_none()) {
            respond(&mut stream, "404 Not Found", "");
            continue;
        }
        // The state first: anything on this machine can call the port, and only the sign-in Audio
        // Tray started may end it, with a code or with an error.
        if get("state") != Some(state) {
            respond(&mut stream, "400 Bad Request", &page("Audio Tray could not connect", "That sign-in was not the one Audio Tray started."));
            continue;
        }
        if let Some(error) = get("error") {
            let why = get("error_description").unwrap_or(error).to_string();
            respond(&mut stream, "200 OK", &page("Audio Tray could not connect", &why));
            bail!("{error}: {why}");
        }
        respond(&mut stream, "200 OK", &page("Audio Tray is connected", "You can close this tab."));
        return get("code").map(str::to_string).ok_or_else(|| anyhow!("no code"));
    }
}

/// The request line's target (`/?code=…`), once the headers are in.
fn read_request_target(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 32 * 1024 {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&buf);
    let line = text.lines().next()?;
    let mut parts = line.split_whitespace();
    (parts.next()? == "GET").then(|| parts.next().map(str::to_string)).flatten()
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let reply = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.flush();
}

fn page(title: &str, text: &str) -> String {
    let escape = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        "<!doctype html><html><head><meta charset=utf-8><title>{t}</title><style>\
         body{{font-family:'Segoe UI Variable Text','Segoe UI',sans-serif;background:#202020;color:#fff;\
         display:grid;place-items:center;height:100vh;margin:0}}\
         @media(prefers-color-scheme:light){{body{{background:#f3f3f3;color:#000}}}}\
         h1{{font-weight:600;font-size:28px;margin:0 0 8px}}p{{opacity:.75;margin:0}}</style></head>\
         <body><div><h1>{t}</h1><p>{x}</p></div></body></html>",
        t = escape(title),
        x = escape(text)
    )
}

/// `a=1&b=x%20y` into pairs, decoded.
pub fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (decode(k), decode(v))
        })
        .collect()
}

/// A form value: `+` is a space, then [`percent_decode`].
fn decode(text: &str) -> String {
    percent_decode(&text.replace('+', " "))
}

/// `%XX` escapes decoded (a stray `%` stays); the bytes read as UTF-8.
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => match text.get(i + 1..i + 3).filter(|hex| hex.bytes().all(|b| b.is_ascii_hexdigit())).and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                Some(b) => {
                    out.push(b);
                    i += 2;
                }
                None => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encodes everything but RFC 3986's unreserved characters.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn base64url(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 2);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, &b)| acc | (b as u32) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

pub fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        } as u32)
    };
    let bytes: Vec<u8> = text.bytes().filter(|&b| b != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        if chunk.len() < 2 {
            return None;
        }
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= value(c)? << (18 - 6 * i);
        }
        for i in 0..chunk.len() - 1 {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    Some(out)
}

/// The claims of a JWT (unverified: it came straight from the token endpoint over TLS).
pub fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    serde_json::from_slice(&base64url_decode(payload)?).ok()
}

fn sha256(data: &[u8]) -> Vec<u8> {
    use sha2::Digest;
    sha2::Sha256::digest(data).to_vec()
}

/// Unpredictable bytes, from Windows' own generator (what `BCryptGenRandom` and std use).
fn random_bytes(n: usize) -> Vec<u8> {
    #[link(name = "bcryptprimitives", kind = "raw-dylib")]
    unsafe extern "system" {
        fn ProcessPrng(data: *mut u8, len: usize) -> i32;
    }
    let mut out = vec![0u8; n];
    // Documented never to fail.
    unsafe { ProcessPrng(out.as_mut_ptr(), out.len()) };
    out
}

/// One agent for the process: its connections (and TLS sessions) are reused.
pub fn agent() -> ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT
        .get_or_init(|| {
            ureq::Agent::config_builder()
                .tls_config(
                    ureq::tls::TlsConfig::builder()
                        .provider(ureq::tls::TlsProvider::NativeTls)
                        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                        .build(),
                )
                .timeout_global(Some(Duration::from_secs(20)))
                .http_status_as_error(false)
                .build()
                .into()
        })
        .clone()
}

/// A refusal from an API, by status.
#[derive(Debug)]
pub struct Refused(pub u16, pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {}: {}", self.0, self.1)
    }
}

impl std::error::Error for Refused {}

/// GET `url` with a bearer token (and more headers), as JSON. A refusal is a [`Refused`].
pub fn get_json(url: &str, token: &str, headers: &[(&str, &str)]) -> Result<Value> {
    let mut request = agent().get(url).header("Authorization", &format!("Bearer {token}")).header("Accept", "application/json");
    for (k, v) in headers {
        request = request.header(*k, *v);
    }
    let mut response = request.call().context("could not reach the calendar")?;
    let status = response.status().as_u16();
    let body = response.body_mut().with_config().limit(16 * 1024 * 1024).read_to_string()?;
    if !(200..300).contains(&status) {
        let value: Value = serde_json::from_str(&body).unwrap_or_default();
        let why = value["error"]["message"].as_str().or(value["error_description"].as_str()).unwrap_or("").to_string();
        return Err(Refused(status, why).into());
    }
    serde_json::from_str(&body).context("the calendar's answer")
}

/// Whether `error` is the API refusing the token (expired or revoked).
pub fn unauthorized(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Refused>().is_some_and(|r| r.0 == 401)
}

/// What a server said identifies a resource's version (`ETag`, `Last-Modified`), to ask again
/// only for a change.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Validators {
    pub etag: Option<String>,
    pub modified: Option<String>,
}

/// [`get_text`], unless it is unchanged since `known` (`None`: unchanged, as the server said).
pub fn get_text_since(url: &str, known: &Validators) -> Result<Option<(String, Validators)>> {
    let mut request = agent().get(url);
    if let Some(etag) = &known.etag {
        request = request.header("If-None-Match", etag);
    }
    if let Some(modified) = &known.modified {
        request = request.header("If-Modified-Since", modified);
    }
    let mut response = request.call().context("could not reach the link")?;
    let status = response.status().as_u16();
    if status == 304 && (known.etag.is_some() || known.modified.is_some()) {
        return Ok(None);
    }
    if !(200..300).contains(&status) {
        return Err(Refused(status, String::new()).into());
    }
    let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let validators = Validators { etag: header("etag"), modified: header("last-modified") };
    Ok(Some((response.body_mut().with_config().limit(32 * 1024 * 1024).read_to_string()?, validators)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_round_trips_without_padding() {
        for input in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar", &[0xfb, 0xff, 0xfe]] {
            let text = base64url(input);
            assert!(!text.contains(['=', '+', '/']));
            assert_eq!(base64url_decode(&text).as_deref(), Some(input));
        }
        assert_eq!(base64url(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn the_pkce_challenge_matches_the_rfc_example() {
        // RFC 7636, appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(base64url(&sha256(verifier.as_bytes())), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert_eq!(base64url(&random_bytes(32)).len(), 43);
        assert_ne!(random_bytes(16), random_bytes(16));
    }

    #[test]
    fn query_strings_decode_and_encode() {
        let pairs = parse_query("code=4%2F0Ab_x&state=a+b&scope=email%20openid");
        assert_eq!(pairs[0], ("code".to_string(), "4/0Ab_x".to_string()));
        assert_eq!(pairs[1].1, "a b");
        assert_eq!(pairs[2].1, "email openid");
        assert_eq!(encode("a b/c#d"), "a%20b%2Fc%23d");
    }

    #[test]
    fn jwt_claims_are_read() {
        let payload = base64url(br#"{"email":"me@example.com"}"#);
        let token = format!("eyJhbGciOiJub25lIn0.{payload}.sig");
        assert_eq!(jwt_claims(&token).unwrap()["email"], "me@example.com");
    }
}

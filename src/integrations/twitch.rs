//! Twitch went-live notices, off by default. Needs a Twitch application's client id and a user
//! access token with `user:read:follows` (`[twitch]` in config). Asks Helix's Get Followed Streams
//! every few minutes from a thread that sleeps in between, and reports channels that were not
//! live at the previous look; the first look only takes the baseline.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::hub::Event;

#[derive(Deserialize)]
struct Page<T> {
    data: Vec<T>,
}

#[derive(Deserialize)]
struct User {
    id: String,
}

#[derive(Deserialize)]
struct Stream {
    user_login: String,
    user_name: String,
    #[serde(default)]
    title: String,
}

/// Start following, when configured. The thread lives as long as the process.
pub fn spawn(settings: &crate::config::Twitch) {
    if !settings.enabled {
        return;
    }
    if settings.client_id.is_empty() || settings.token.is_empty() {
        eprintln!("twitch: enabled, but client_id or token is missing in config");
        return;
    }
    let settings = settings.clone();
    let every = Duration::from_secs(settings.every_minutes.max(1) * 60);
    let _ = std::thread::Builder::new().name("twitch".into()).spawn(move || {
        let agent = agent();
        let mut live: Option<HashSet<String>> = None;
        let mut user = None;
        loop {
            match follow(&agent, &settings, &mut user, &mut live) {
                Ok(()) => crate::hub::emit(Event::Integration { name: "Twitch", connected: true }),
                Err(e) => {
                    eprintln!("twitch: {e:#}");
                    crate::hub::emit(Event::Integration { name: "Twitch", connected: false });
                }
            }
            std::thread::sleep(every);
        }
    });
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).build())
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into()
}

fn get<T: for<'de> Deserialize<'de>>(agent: &ureq::Agent, settings: &crate::config::Twitch, url: &str) -> Result<Page<T>> {
    let mut response = agent
        .get(url)
        .header("Client-Id", &settings.client_id)
        .header("Authorization", &format!("Bearer {}", settings.token))
        .call()
        .with_context(|| format!("GET {url}"))?;
    let body = response.body_mut().read_to_string().context("reading the response")?;
    serde_json::from_str(&body).context("parsing the response")
}

fn follow(agent: &ureq::Agent, settings: &crate::config::Twitch, user: &mut Option<String>, live: &mut Option<HashSet<String>>) -> Result<()> {
    if user.is_none() {
        let page: Page<User> = get(agent, settings, "https://api.twitch.tv/helix/users")?;
        *user = Some(page.data.into_iter().next().context("the token names no user")?.id);
    }
    let id = user.as_deref().unwrap_or_default();
    let page: Page<Stream> = get(agent, settings, &format!("https://api.twitch.tv/helix/streams/followed?user_id={id}&first=100"))?;
    let now: HashSet<String> = page.data.iter().map(|s| s.user_login.clone()).collect();
    if let Some(before) = live.as_ref() {
        for stream in page.data.iter().filter(|s| !before.contains(&s.user_login)) {
            println!("twitch: {} went live", stream.user_name);
            crate::hub::emit(Event::WentLive {
                channel: stream.user_name.clone(),
                title: stream.title.clone(),
                url: format!("https://www.twitch.tv/{}", stream.user_login),
            });
        }
    }
    *live = Some(now);
    Ok(())
}

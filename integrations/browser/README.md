# Browser extension: every tab playing media

Windows' media controls get one session per browser, so with two Twitch streams (or a YouTube
video and a stream) open, only one reaches Audio Tray. This extension reports every tab and
frame playing `<video>` or `<audio>` itself: title, channel, artwork, playing or paused, position,
and whether it is live. The edge pill and the flyout then show each one, with play/pause.

## How it works

- `content.js` runs in every frame and watches its media elements (events only, no timer); the
  title, artist and artwork come from the page's Media Session metadata when it sets any.
- `background.js` collects the frames and talks native messaging to `com.audiotray.media`.
- That host is `audio-tray.exe` itself, launched by the browser; it relays to the running tray
  over `\\.\pipe\audio-tray-browser` (and commands back over `\\.\pipe\audio-tray-browser-cmd`).
  Audio Tray registers the host for Edge and Chrome, per user, each time it starts
  (`[browser] enabled = true`, the default).

The manifest's `key` pins the extension id (`eajikcpfpkjjbdfibhfnnacflhgklgll`), which is what
the host registration allows.

## Install (Edge or Chrome)

Audio Tray's **Settings > Connectors** does the first steps for you: **Open the browser's
extensions** opens your default browser's page, **Open the extension's folder** shows the folder
and copies its path. The installer puts the folder at `browser-extension` beside the exe.

1. Start Audio Tray once, so the host is registered.
2. Open `edge://extensions` (or `chrome://extensions`) and turn on **Developer mode**.
3. **Load unpacked**, and pick the folder (paste the copied path in the dialog's address bar).

Tabs already open are picked up when they next play, pause or seek; reloading them works too.

## What counts as playing

A tab counts while its media plays, and also while Chromium holds it paused for being muted in
a hidden tab (it resumes when the tab shows): a muted second stream is still being watched. A
paused live player keeps its row (Twitch resets the position on pause), and players that swap
`<video>` elements (Twitch after an ad) are followed through capture-phase media events.
Duplicates of what Windows' media controls already report (YouTube Music's app runs in Edge)
show once.

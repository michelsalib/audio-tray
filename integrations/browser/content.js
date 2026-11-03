// Audio Tray: reports this frame's playing <video>/<audio> to the extension's background, and
// plays or pauses it on command. Event-driven: media events (caught in the capture phase, so
// every element, wherever the page puts it), never a timer, and no watching of the page's DOM.

// In a function of its own: injected again into open tabs when the extension is reloaded, a second
// copy must not clash with the first one's declarations.
(() => {
const elements = new Set();
const EVENTS = ["play", "playing", "pause", "ended", "emptied", "loadedmetadata", "durationchange", "seeked", "volumechange"];
let timer = null;
let last = "";

function track(el) {
    if (elements.has(el)) return;
    elements.add(el);
    for (const name of EVENTS) el.addEventListener(name, report);
    report();
}

function scan(root) {
    if (root.nodeName === "VIDEO" || root.nodeName === "AUDIO") track(root);
    root.querySelectorAll?.("video, audio").forEach(track);
}

// Bursts of events (a seek, a track change) report once.
function report() {
    clearTimeout(timer);
    timer = setTimeout(send, 150);
}

/** The element that last played: kept, paused, while it is on the page. */
let lastPlayed = null;

/** Elements the browser paused, not the user: Chromium pauses a muted video in a hidden tab and
 * resumes it when the tab shows again. Those still count as playing. */
const suspended = new WeakSet();
document.addEventListener("pause", event => {
    const el = event.target;
    if (el instanceof HTMLMediaElement && document.hidden && (el.muted || el.volume === 0)) suspended.add(el);
}, true);
document.addEventListener("play", event => suspended.delete(event.target), true);
document.addEventListener("visibilitychange", () => report());

/** Live streams. Some say so (an infinite duration, Twitch); others publish a finite one that keeps
 * growing with the stream (YouTube), wherever in it the viewer is (a stream watched behind its live
 * point too), and in steps (YouTube adds half an hour at once). A video's duration does not gain
 * seconds once loaded: one that grows by more than 2 s while playing is live until the element
 * loads something else. */
const streams = new WeakSet();
const durations = new WeakMap();
document.addEventListener("durationchange", event => {
    const el = event.target;
    if (!(el instanceof HTMLMediaElement)) return;
    const before = durations.get(el);
    durations.set(el, el.duration);
    const grew = Number.isFinite(before) && Number.isFinite(el.duration) && el.duration > before + 2;
    if (grew && isPlaying(el)) streams.add(el);
}, true);
document.addEventListener("emptied", event => {
    streams.delete(event.target);
    durations.delete(event.target);
}, true);

/** YouTube itself: YouTube Music's pages carry the same player markup, marker included. */
const YOUTUBE = ["www.youtube.com", "youtube.com", "m.youtube.com"];

/** YouTube's player marks a live video in its time display, from the start (its duration only
 * grows every half hour): read that there. */
function youTubeLive() {
    return YOUTUBE.includes(location.hostname) && !!document.querySelector(".ytp-time-display.ytp-live");
}

function isLive(el) {
    return el.duration === Infinity || streams.has(el) || youTubeLive();
}

function isPlaying(el) {
    return (!el.paused && !el.ended) || suspended.has(el);
}

/** Elements that have played with sound. A muted video playing by itself (YouTube's preview of a
 * hovered thumbnail, a page's background clip) is not media anyone is listening to; one that was
 * heard and then muted still is. */
const heard = new WeakSet();
for (const name of ["playing", "volumechange"]) {
    document.addEventListener(name, event => {
        const el = event.target;
        if (el instanceof HTMLMediaElement && isPlaying(el) && !el.muted && el.volume > 0) heard.add(el);
    }, true);
}

/** YouTube's hover previews, whatever their sound: never the video being watched. */
const PREVIEWS = "#inline-preview-player, ytd-video-preview";

/** Whether an element is media worth reporting at all (see `heard`). */
function worth(el) {
    if (el.closest(PREVIEWS)) return false;
    return heard.has(el) || (!el.muted && el.volume > 0);
}

/** The element worth reporting: a playing one, else the one that last played (a live player can
 * reset its position on pause), else one that has started or holds a live stream (paused, a
 * Twitch player is at 0 when this script arrives late, injected after an update). */
function active() {
    const live = [...elements].filter(el => el.isConnected && worth(el));
    const playing = live.find(isPlaying);
    if (playing) lastPlayed = playing;
    if (lastPlayed && !lastPlayed.isConnected) lastPlayed = null;
    return playing ?? lastPlayed ?? live.find(el => el.currentTime > 0 || el.duration === Infinity) ?? null;
}

function artwork(metadata) {
    const art = metadata?.artwork;
    if (art?.length) return art[art.length - 1].src;
    return document.querySelector('meta[property="og:image"]')?.content ?? null;
}

function send() {
    // Players swap elements (Twitch after an ad): pick up any not seen yet.
    scan(document);
    const el = active();
    let state = null;
    if (el) {
        const metadata = navigator.mediaSession?.metadata;
        state = {
            title: metadata?.title || document.title,
            artist: metadata?.artist || "",
            artwork: artwork(metadata),
            playing: isPlaying(el),
            muted: el.muted || el.volume === 0,
            // A stream has no end: no duration, so no ring and a LIVE badge. Its position is
            // dropped too, or every growth of the window would be a new report.
            duration: !isLive(el) && Number.isFinite(el.duration) ? el.duration : 0,
            position: isLive(el) ? 0 : el.currentTime || 0,
        };
    }
    // Nothing new, nothing sent (position aside, which only matters on a seek or play).
    const key = JSON.stringify(state && { ...state, position: Math.round(state.position) });
    if (key === last) return;
    last = key;
    chrome.runtime.sendMessage({ type: "media", state }).catch(() => { });
}

chrome.runtime.onMessage.addListener(message => {
    if (message.ask) {
        last = "";
        send();
        return;
    }
    const el = active() ?? [...elements].find(e => e.isConnected);
    if (!el) return;
    if (message.command === "toggle") {
        if (isPlaying(el)) {
            suspended.delete(el);
            el.pause();
        } else {
            el.play();
        }
    }
});

// Media events do not bubble, but they pass the document in the capture phase: this catches any
// element, however the page made it (created off-document, moved, swapped after an ad).
for (const name of EVENTS) {
    document.addEventListener(name, event => {
        if (event.target instanceof HTMLMediaElement) {
            track(event.target);
            report();
        }
    }, true);
}


// A new title on the same element (YouTube's next video, a stream retitled).
const title = document.querySelector("title");
if (title) new MutationObserver(report).observe(title, { childList: true, characterData: true, subtree: true });

window.addEventListener("pagehide", () => chrome.runtime.sendMessage({ type: "media", state: null }).catch(() => { }));
scan(document);
})();

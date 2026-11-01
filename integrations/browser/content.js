// Audio Tray: reports this frame's playing <video>/<audio> to the extension's background, and
// plays or pauses it on command. Event-driven: media events (caught in the capture phase, so
// every element, wherever the page puts it), never a timer, and no watching of the page's DOM.

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

function isPlaying(el) {
    return (!el.paused && !el.ended) || suspended.has(el);
}

/** The element worth reporting: a playing one, else the one that last played (a live player can
 * reset its position on pause), else one that has started. */
function active() {
    const live = [...elements].filter(el => el.isConnected);
    const playing = live.find(isPlaying);
    if (playing) lastPlayed = playing;
    if (lastPlayed && !lastPlayed.isConnected) lastPlayed = null;
    return playing ?? lastPlayed ?? live.find(el => el.currentTime > 0) ?? null;
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
            duration: Number.isFinite(el.duration) ? el.duration : 0,
            position: el.currentTime || 0,
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

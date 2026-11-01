// Audio Tray: reports this frame's playing <video>/<audio> to the extension's background, and
// plays or pauses it on command. Event-driven: media events and DOM changes, never a timer.

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

/** The element worth reporting: a playing one, else one that has started. */
function active() {
    const live = [...elements].filter(el => el.isConnected);
    return live.find(el => !el.paused && !el.ended) ?? live.find(el => el.currentTime > 0) ?? null;
}

function artwork(metadata) {
    const art = metadata?.artwork;
    if (art?.length) return art[art.length - 1].src;
    return document.querySelector('meta[property="og:image"]')?.content ?? null;
}

function send() {
    const el = active();
    let state = null;
    if (el) {
        const metadata = navigator.mediaSession?.metadata;
        state = {
            title: metadata?.title || document.title,
            artist: metadata?.artist || "",
            artwork: artwork(metadata),
            playing: !el.paused && !el.ended,
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
    if (message.command === "toggle") el.paused ? el.play() : el.pause();
});

new MutationObserver(mutations => {
    for (const m of mutations) for (const node of m.addedNodes) scan(node);
}).observe(document, { childList: true, subtree: true });

// A new title on the same element (YouTube's next video, a stream retitled).
const title = document.querySelector("title");
if (title) new MutationObserver(report).observe(title, { childList: true, characterData: true, subtree: true });

window.addEventListener("pagehide", () => chrome.runtime.sendMessage({ type: "media", state: null }).catch(() => { }));
scan(document);

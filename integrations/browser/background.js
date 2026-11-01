// Audio Tray: gathers every frame's media and hands the list to Audio Tray through its native
// messaging host (`audio-tray.exe`, registered by Audio Tray as com.audiotray.media). Commands
// come back the same way. The open native port also keeps this service worker alive.

const HOST = "com.audiotray.media";
/** `${tabId}:${frameId}` → the frame's state, as content.js sends it. */
const frames = new Map();
/** Artwork URL → base64 bytes (or null), a few dozen at most. */
const art = new Map();
let port = null;
let pushTimer = null;

function connect() {
    port = chrome.runtime.connectNative(HOST);
    port.onMessage.addListener(onCommand);
    port.onDisconnect.addListener(() => {
        port = null;
        // Audio Tray not installed or the host refused: try again in a while, not in a loop.
        setTimeout(connect, 60000);
    });
    push();
}

async function artOf(url) {
    if (!url) return null;
    if (art.has(url)) return art.get(url);
    let value = null;
    try {
        const response = await fetch(url);
        if (response.ok) {
            const bytes = new Uint8Array(await response.arrayBuffer());
            let text = "";
            for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
            value = btoa(text);
        }
    } catch { }
    art.set(url, value);
    if (art.size > 40) art.delete(art.keys().next().value);
    return value;
}

function site(url) {
    try {
        return new URL(url).hostname;
    } catch {
        return "";
    }
}

function push() {
    clearTimeout(pushTimer);
    pushTimer = setTimeout(async () => {
        if (!port) return;
        const tabs = [];
        for (const [id, state] of frames) {
            const tabId = Number(id.split(":")[0]);
            const tab = await chrome.tabs.get(tabId).catch(() => null);
            if (!tab) {
                frames.delete(id);
                continue;
            }
            tabs.push({
                id,
                site: site(tab.url),
                title: state.title,
                artist: state.artist,
                art: await artOf(state.artwork),
                playing: state.playing,
                muted: state.muted,
                duration: state.duration,
                position: state.position,
            });
        }
        port?.postMessage({ type: "tabs", tabs });
    }, 100);
}

chrome.runtime.onMessage.addListener((message, sender) => {
    if (message.type !== "media" || !sender.tab) return;
    const id = `${sender.tab.id}:${sender.frameId}`;
    if (message.state) frames.set(id, message.state);
    else frames.delete(id);
    push();
});

chrome.tabs.onRemoved.addListener(tabId => {
    for (const id of [...frames.keys()]) if (id.startsWith(`${tabId}:`)) frames.delete(id);
    push();
});

function onCommand(message) {
    if (message.type !== "command") return;
    const [tabId, frameId] = message.id.split(":").map(Number);
    chrome.tabs.sendMessage(tabId, { command: message.command }, { frameId }).catch(() => { });
}

// A restarted worker has forgotten everything: ask every frame again.
chrome.tabs.query({}).then(tabs => {
    for (const tab of tabs) chrome.tabs.sendMessage(tab.id, { ask: true }).catch(() => { });
});
connect();

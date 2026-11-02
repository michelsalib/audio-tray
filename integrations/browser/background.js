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
/** When the last connection was tried, so a missing host is not retried on every media event. */
let lastTry = 0;

/** While disconnected, an alarm wakes the worker to try again: a sleeping worker forgets its timers,
 * and a tab playing a stream sends no event to wake it. */
const RETRY = "reconnect";

function connect() {
    lastTry = Date.now();
    port = chrome.runtime.connectNative(HOST);
    port.onMessage.addListener(onCommand);
    port.onDisconnect.addListener(() => {
        port = null;
        // A connection that was up went away (Audio Tray restarting for an update): back in a
        // moment. One that never came up (not installed): every half minute, never in a loop.
        if (Date.now() - lastTry > 5000) setTimeout(() => port || connect(), 2000);
        chrome.alarms.create(RETRY, { periodInMinutes: 0.5 });
    });
    push();
}

chrome.alarms.onAlarm.addListener(alarm => {
    if (alarm.name !== RETRY) return;
    if (port) chrome.alarms.clear(RETRY);
    else connect();
});

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
    // A sleeping worker loses the retry timer: a tab's media event is the next chance.
    if (!port && Date.now() - lastTry > 10000) connect();
    const id = `${sender.tab.id}:${sender.frameId}`;
    if (message.state) frames.set(id, message.state);
    else frames.delete(id);
    push();
});

chrome.tabs.onRemoved.addListener(tabId => {
    for (const id of [...frames.keys()]) if (id.startsWith(`${tabId}:`)) frames.delete(id);
    push();
});

async function onCommand(message) {
    // Audio Tray was updated and has written its copy of this extension where the browser loads it
    // from (the extension carries the app's version): reload onto it. Once per version, so files
    // that failed to be written cannot loop it.
    if (message.type === "host") {
        const mine = chrome.runtime.getManifest().version;
        if (!message.extension || message.extension === mine) return;
        const { reloadedFor } = await chrome.storage.local.get("reloadedFor");
        if (reloadedFor === message.extension) return;
        await chrome.storage.local.set({ reloadedFor: message.extension });
        chrome.runtime.reload();
        return;
    }
    if (message.type !== "command") return;
    const [tabId, frameId] = message.id.split(":").map(Number);
    if (message.command === "focus") {
        // The tab, then its window: Audio Tray allowed this browser to take the foreground.
        const tab = await chrome.tabs.update(tabId, { active: true }).catch(() => null);
        if (tab) await chrome.windows.update(tab.windowId, { focused: true }).catch(() => { });
        return;
    }
    chrome.tabs.sendMessage(tabId, { command: message.command }, { frameId }).catch(() => { });
}

// Installed, updated or reloaded: tabs already open still run the old copy of content.js, cut off
// from this one, and the browser only injects into pages loaded from now on. Inject it into them.
chrome.runtime.onInstalled.addListener(async () => {
    const tabs = await chrome.tabs.query({ url: ["http://*/*", "https://*/*"] });
    for (const tab of tabs) {
        chrome.scripting.executeScript({ target: { tabId: tab.id, allFrames: true }, files: ["content.js"] }).catch(() => { });
    }
});

// A restarted worker has forgotten everything: ask every frame again.
chrome.tabs.query({}).then(tabs => {
    for (const tab of tabs) chrome.tabs.sendMessage(tab.id, { ask: true }).catch(() => { });
});
connect();

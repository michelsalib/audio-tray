/*
 * Audio Tray, the native half: runs in Discord's main process, where Node can open
 * Audio Tray's named pipe and fetch avatars without CORS.
 */

import { IpcMainInvokeEvent } from "electron";
import { connect, Socket } from "net";

const PIPE = "\\\\.\\pipe\\audio-tray";

let socket: Socket | null = null;
let buffer = "";
/** The last call message, re-sent whenever the pipe (re)connects. */
let lastCall: string | null = null;
const lines: string[] = [];
const waiters: ((line: string) => void)[] = [];

function ensure() {
    if (socket) return;
    const s = connect(PIPE);
    socket = s;
    s.setEncoding("utf8");
    s.on("connect", () => {
        if (lastCall) s.write(lastCall + "\n");
    });
    s.on("data", (chunk: string) => {
        buffer += chunk;
        let end: number;
        while ((end = buffer.indexOf("\n")) >= 0) {
            const line = buffer.slice(0, end);
            buffer = buffer.slice(end + 1);
            const waiter = waiters.shift();
            if (waiter) waiter(line);
            else lines.push(line);
        }
    });
    // Audio Tray not running: stay quiet, the next send or ping tries again.
    s.on("error", () => { });
    s.on("close", () => {
        if (socket === s) socket = null;
    });
}

/** One JSON message to Audio Tray. */
export function send(_: IpcMainInvokeEvent, line: string) {
    if (line.startsWith('{"type":"call"')) lastCall = line;
    ensure();
    // While connecting, the connect handler sends the last call, which carries everything.
    if (socket && !socket.connecting) socket.write(line + "\n");
}

/** Reconnect if Audio Tray started after Discord (called while in a call). */
export function ping(_: IpcMainInvokeEvent) {
    ensure();
}

/** The next command from Audio Tray; resolves when one arrives. */
export function nextCommand(_: IpcMainInvokeEvent): Promise<string> {
    const line = lines.shift();
    if (line !== undefined) return Promise.resolve(line);
    return new Promise(resolve => waiters.push(resolve));
}

/** An avatar as base64 PNG, or null. */
export async function avatar(_: IpcMainInvokeEvent, url: string): Promise<string | null> {
    try {
        const png = url.replace(/\.(webp|gif)(\?|$)/, ".png$2");
        const response = await fetch(png);
        if (!response.ok) return null;
        return Buffer.from(await response.arrayBuffer()).toString("base64");
    } catch {
        return null;
    }
}

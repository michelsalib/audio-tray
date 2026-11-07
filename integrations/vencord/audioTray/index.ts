/*
 * Audio Tray: tells Audio Tray (https://github.com/michelsalib/audio-tray) who is in your voice
 * channel, who is talking and whether you are muted, and lets its edge pill toggle your mute or leave the channel.
 */

import definePlugin, { PluginNative } from "@utils/types";
import { findByPropsLazy, findStoreLazy } from "@webpack";
import { ChannelStore, IconUtils, SelectedChannelStore, UserStore } from "@webpack/common";

const Native = VencordNative.pluginHelpers.AudioTray as PluginNative<typeof import("./native")>;

const VoiceStateStore = findStoreLazy("VoiceStateStore");
const MediaEngineStore = findStoreLazy("MediaEngineStore");
const VoiceActions = findByPropsLazy("toggleSelfMute");
const ChannelActions = findByPropsLazy("disconnect", "selectVoiceChannel");

/** Who is talking in the current channel. */
const speaking = new Set<string>();
/** Avatars already fetched, by user id and avatar hash. */
const avatars = new Map<string, string | null>();
let running = false;
let ping: ReturnType<typeof setInterval> | null = null;

async function avatarOf(user: any): Promise<string | null> {
    const key = `${user.id}:${user.avatar}`;
    if (!avatars.has(key)) avatars.set(key, await Native.avatar(IconUtils.getUserAvatarURL(user, false, 64)));
    return avatars.get(key) ?? null;
}

async function sendCall() {
    const channelId = SelectedChannelStore.getVoiceChannelId();
    if (!channelId) {
        speaking.clear();
        if (ping) clearInterval(ping);
        ping = null;
        Native.send(JSON.stringify({ type: "call", channel: null }));
        return;
    }
    // Retry the pipe while in a call, in case Audio Tray starts later.
    ping ??= setInterval(() => Native.ping(), 15000);
    const me = UserStore.getCurrentUser();
    const states = Object.values(VoiceStateStore.getVoiceStatesForChannel(channelId) ?? {}) as any[];
    const members = await Promise.all(states.map(async state => {
        const user = UserStore.getUser(state.userId);
        return {
            id: state.userId,
            name: user?.globalName ?? user?.username ?? "Someone",
            avatar: user ? await avatarOf(user) : null,
            speaking: speaking.has(state.userId),
            muted: Boolean(state.selfMute || state.mute),
            me: state.userId === me?.id,
        };
    }));
    Native.send(JSON.stringify({
        type: "call",
        channel: ChannelStore.getChannel(channelId)?.name ?? null,
        selfMute: MediaEngineStore.isSelfMute(),
        selfDeaf: MediaEngineStore.isSelfDeaf(),
        members,
    }));
}

async function commands() {
    while (running) {
        const line = await Native.nextCommand();
        if (!running) return;
        try {
            const command = JSON.parse(line);
            if (command.type === "toggleMute") VoiceActions.toggleSelfMute();
            else if (command.type === "setMute" && MediaEngineStore.isSelfMute() !== command.muted) VoiceActions.toggleSelfMute();
            else if (command.type === "disconnect") ChannelActions.disconnect();
        } catch { }
    }
}

export default definePlugin({
    name: "AudioTray",
    description: "Shows your voice channel, who is talking and your mute on Audio Tray's edge pill",
    authors: [{ name: "Michel Salib", id: 0n }],

    flux: {
        VOICE_CHANNEL_SELECT() {
            sendCall();
        },
        VOICE_STATE_UPDATES({ voiceStates }: { voiceStates: any[]; }) {
            const channelId = SelectedChannelStore.getVoiceChannelId();
            if (voiceStates.some(s => s.channelId === channelId || s.oldChannelId === channelId)) sendCall();
        },
        SPEAKING({ userId, speakingFlags }: { userId: string; speakingFlags: number; }) {
            const now = speakingFlags !== 0;
            if (now === speaking.has(userId)) return;
            if (now) speaking.add(userId);
            else speaking.delete(userId);
            if (SelectedChannelStore.getVoiceChannelId()) Native.send(JSON.stringify({ type: "speaking", id: userId, speaking: now }));
        },
        AUDIO_TOGGLE_SELF_MUTE() {
            // The store updates after the action; read it on the next turn.
            setTimeout(() => Native.send(JSON.stringify({ type: "selfMute", muted: MediaEngineStore.isSelfMute() })), 0);
        },
    },

    start() {
        running = true;
        commands();
        sendCall();
    },

    stop() {
        running = false;
        if (ping) clearInterval(ping);
        ping = null;
        Native.send(JSON.stringify({ type: "call", channel: null }));
    },
});

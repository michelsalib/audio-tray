# Discord integration (Vencord user plugin)

Audio Tray's edge pill can show your Discord voice call: who is in it, who is talking, and
whether you are muted, and clicking the pill toggles your mute. Discord has no API for this that
an app may use, so a [Vencord](https://vencord.dev) user plugin reports it.

> Client mods are against Discord's terms of service. The plugin only reads your own voice
> state and only talks to Audio Tray on this machine, but use it knowing that.

## How it works

Audio Tray hosts the named pipe `\\.\pipe\audio-tray` (local clients only, `[discord] enabled`
in its config). The plugin's native half (`native.ts`, in Discord's main process) connects and
writes one JSON object per line; Audio Tray writes commands back. See
`src/integrations/discord.rs` for the wire format.

## Install

User plugins need Vencord built from source:

1. Clone Vencord and install its dependencies: `git clone https://github.com/Vendicated/Vencord`,
   then `pnpm install` in it.
2. Copy the `audioTray` folder here to `Vencord/src/userplugins/audioTray`.
3. `pnpm build`, then `pnpm inject` (or rebuild if Vencord is already injected) and restart Discord.
4. In Discord, Settings > Vencord > Plugins, turn on **AudioTray**.

Join a voice channel: the pill shows the call while your microphone is live or someone talks.
If Audio Tray starts after Discord, the plugin reconnects within 15 seconds while you are in a call.

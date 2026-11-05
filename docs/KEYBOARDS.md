# Plan: a keyboard layout per physical keyboard, and hiding Windows' layout indicator

Written 2026-11-04 for the agent implementing it. Read `CLAUDE.md` first (build commands,
`cargo` PATH, killing `audio-tray.exe` before building, no `rustfmt`, the `windows` crate is
pinned to 0.61). The decisions below were taken with the user. Don't reopen them; raise
anything that contradicts them instead.

## The user's need

- The laptop's built-in keyboard is French AZERTY. The user often types on Bluetooth and USB
  keyboards that are ANSI US QWERTY.
- Windows has one current layout per user (or per window), not per device. Today the user
  switches by hand from the taskbar's layout indicator.
- Wanted:
  1. audio-tray switches the layout automatically, based on which keyboard is being typed on;
  2. Windows' layout indicator is hidden from the taskbar, since it is no longer needed.

### This machine (checked 2026-11-04)

- **One language, two layouts.** `Get-WinUserLanguageList` shows only `fr-FR`, with
  `InputMethodTips` `040C:0000040C` (French AZERTY) and `040C:00000409` (US under French).
  `HKCU\Keyboard Layout\Preload` holds `0000040c` and `d001040c`; the latter is a substitute
  entry, resolved through `HKCU\Keyboard Layout\Substitutes`.
- **Don't assume the handle values.** The handles to work with are the ones
  `GetKeyboardLayoutList` returns, probably `0x040C040C` and `0x0409040C`. Log them in the spike.
- **Keyboards (`Get-PnpDevice -Class Keyboard`):**
  - the laptop's own PS/2 keyboard (`ACPI\HPQ8002`) and an HP hotkey device
    (`HID\CONVERTEDDEVICE&COL01`);
  - an Intel device (`HID\INTC816&COL01`);
  - a Logitech receiver (`VID_046D&PID_C539`), receivers `VID_19F5&PID_102C` and `PID_2620`;
  - Bluetooth LE keyboards (`{00001812-…}_DEV_VID&023151_PID&4013…` and `…VID&02046D_PID&B02E…`).
  - Several of these expose more than one keyboard interface each.

## Decisions already taken

1. **Built into audio-tray, not delegated to an existing app.** No existing app handles two
   layouts under the same language (references below).
2. **Raw Input, header only.**
   - Register `RegisterRawInputDevices` (usage page 1, usage 6) with `RIDEV_INPUTSINK |
     RIDEV_DEVNOTIFY`.
   - For each `WM_INPUT`, call `GetRawInputData(..., RID_HEADER, ...)` and read only `hDevice`.
   - **Never read key codes, never store or log keys.** The user explicitly asked whether all
     keys are listened to; the promise is "device identity only".
   - No keyboard hook, no DLL loaded into other processes, no `SendInput`.
3. **Switch on any key from a different keyboard.** The first key typed after changing keyboards
   still arrives in the old layout. Raw Input reports a key only after it has gone to the app
   (see [boppreh/keyboard#21](https://github.com/boppreh/keyboard/issues/21)).
   - The user accepts this. Their habit is to press Ctrl, Alt or Shift first.
   - **Rejected:** the "Backspace and retype" correction (AutoRightKeyboard's "Instant Switch").
     Don't implement it.
   - Don't gate the switch on modifiers either: that would require reading key codes.
4. **Switch only when the keyboard changes.** Act when `hDevice` differs from the last one seen.
   A manual Win+Space on the same keyboard then sticks until another keyboard is used.
5. **Identify a layout by its full handle, never by its 16-bit language id.**
   - Offer and store values from `GetKeyboardLayoutList`.
   - Never call `LoadKeyboardLayout("00000409", …)`, and never use `KLF_REPLACELANG`. That
     installs or replaces the *English* language. It is the bug in every existing app (below).
6. **Confirm each switch on the edge pill,** where it announces a new song or a moved audio
   device (changed by the user on 2026-11-05; it was the scroll readout, `src/osd.rs`, before).
   - Done as a hub notice (`Event::KeyboardSwitched` → `Notice::KeyboardSwitched`, 1.5 s), shown
     by the pill as a `Level::Normal` banner: below alerts and "audio moved", hidden by a compact
     pill or a fullscreen app like the other quiet states.
   - **Manual changes too** (asked by the user, 2026-11-05): a layout changed by Win+Space,
     Alt+Shift or the language bar shows the same banner, named after the keyboard that typed
     last (layout alone when it has no name). No process is told of another's layout change, so
     the switcher thread polls the focus window's layout every 150 ms; a change counts once the
     focus has stayed on one thread for 400 ms (a newly focused thread may still hold a stale
     layout, and Win+Space's picker takes the focus while open). Its own switches are not
     announced twice. With "a different input method for each app window" on, moving between
     windows of different layouts is announced too.
   - Content: the name the user gave the keyboard, plus the layout and language, e.g.
     "⌨ K380 · US — Français".
   - **Not** a toast notification: one toast per keyboard change would fill the notification
     centre.
7. **Assigning a keyboard in Settings works by pressing a key on it.**
   - Windows names nearly every keyboard "Périphérique clavier PIH", and one device exposes
     several interfaces. Capturing the next `WM_INPUT` picks the interface that really types.
   - Store the device by its Raw Input interface path (`GetRawInputDeviceInfoW(RIDI_DEVICENAME)`),
     which is stable across reconnects (the Bluetooth path carries the MAC). Keep a name the user
     can edit.
8. **Keyboards with no assignment don't switch anything** (agreed with the user, 2026-11-04).
9. **The lock screen is out of scope.** The app gets no input there and cannot change the lock
   screen's layout.
   - The lock screen has its own Win+Space and layout indicator.
   - An unverified idea (switch to the last-used keyboard's layout on `WTS_SESSION_LOCK`) is a
     possible later experiment, not part of this work.
10. **Hiding the indicator is done by the TAP, with the section-scoped matching** described in
    `crates/taskbar-tap/FINDINGS.md`, "Telling the shell's tray icons apart" (details in step 1).

## References (read before writing code)

### Existing apps, and what to take or avoid

| Project | Take | Avoid |
| --- | --- | --- |
| [agabor/RightKeyboard](https://github.com/agabor/RightKeyboard) (C#, 75★), the original, from a [CodeProject article](http://www.codeproject.com/Articles/20994/Using-multiple-keyboards-with-different-layouts-on) | Raw Input with `RIDEV_INPUTSINK`; `BroadcastSystemMessage(WM_INPUTLANGCHANGEREQUEST)` + `SPI_SETDEFAULTINPUTLANG` | Keyed by language id: issues [#1](https://github.com/agabor/RightKeyboard/issues/1), [#2](https://github.com/agabor/RightKeyboard/issues/2) and [#7](https://github.com/agabor/RightKeyboard/issues/7) (same-language layouts, open since 2016). [#3](https://github.com/agabor/RightKeyboard/issues/3) is the first-key limit (decision 3) |
| [andresapitt/AutoRightKeyboard](https://github.com/andresapitt/AutoRightKeyboard) (C#, Feb 2026), `MainForm.cs` | Ignores `hDevice == 0` (injected keys); ignores the "Microsoft Keyboard RID" virtual device, which fires on every key and overrode real switches; `WM_DEVICECHANGE` for late keyboards; caches the last device | `FindSystemHKL(ushort languageId)` returns the first layout of that language, so it has the same bug. Blocking `SendMessage` to the foreground window (use `SendMessageTimeout`). "Instant Switch" (rejected) |
| [olshtieg-dev/HardwireGraft](https://github.com/olshtieg-dev/HardwireGraft) (C#, early), `Services/KeyboardLayoutSwitcher.cs`, `Services/DeviceIdentity.cs` | Device identity by hardware id; a debug monitor window | `LoadKeyboardLayout(klid, KLF_ACTIVATE \| KLF_REPLACELANG)`; its README admits switching between same-language variants is unreliable. That's the cause |
| [flostellbrink/cleverkeyboard](https://github.com/flostellbrink/cleverkeyboard) | — | Same design as RightKeyboard, abandoned in 2020 |

### Hiding the indicator

- [Windhawk "Taskbar tray system icon tweaks"](https://github.com/ramensoftware/windhawk-mods/blob/main/mods/taskbar-tray-system-icon-tweaks.wh.cpp)
  (v1.3.1). See `ApplyNonActivatableStackIconViewStyle` and the `hideLanguageBar` /
  `hideLanguageSupplementaryIcons` options.
- Its method is written up in our `crates/taskbar-tap/FINDINGS.md`, section "Telling the shell's
  tray icons apart — scope by section, then identify" (element paths, glyph table, why the
  match must be re-checked).

### Win32 APIs

All need the `windows` 0.61 crate. `Win32_UI_Input` (Raw Input) is not in the root `Cargo.toml`
feature list yet. Add it there; it's the app, not the TAP. The `HKL` type may also need
`Win32_UI_TextServices`: let the compiler tell you. Either addition costs one cold rebuild of
`windows` (~1–2 min); that is expected.

- **Raw Input:** `RegisterRawInputDevices`, `GetRawInputData` (`RID_HEADER`),
  `GetRawInputDeviceInfoW` (`RIDI_DEVICENAME`), `GetRawInputDeviceList`, and `WM_INPUT_DEVICE_CHANGE`
  (from `RIDEV_DEVNOTIFY`).
- **Layouts:** `GetKeyboardLayoutList`, `GetKeyboardLayout(thread of the foreground window)`.
  - Names come from `HKLM\SYSTEM\CurrentControlSet\Control\Keyboard Layouts\<KLID>\Layout Text`
    (or `Layout Display Name` via `SHLoadIndirectString`).
  - The language name comes from `LCIDToLocaleName` + `GetLocaleInfoEx(LOCALE_SLOCALIZEDDISPLAYNAME)`.
  - Getting from a handle to a layout id: the high word, unless it is `0xFxxx` (a substitute
    or IME). Then read `GetKeyboardLayoutNameW` while it is active, or the `Layout Id` registry
    value. The spike should log what this machine returns.
- **Switching:**
  - Primary: `PostMessageW(target, WM_INPUTLANGCHANGEREQUEST, 0, hkl)`, where `target` is the
    focus window of the foreground thread (`GetGUIThreadInfo(...).hwndFocus`, falling back to
    `GetForegroundWindow()`).
  - Never use blocking `SendMessage`; use `SendMessageTimeout` (`SMTO_ABORTIFHUNG`) if a
    synchronous answer is needed.
  - Compare against the foreground thread's current layout first.
  - Windows' default ("Let me use a different input method for each app window" off) makes the
    switch apply everywhere.
  - Known gap: windows running as administrator ignore messages from a normal process. There the
    layout may not follow until another window is focused. Report it, don't work around it.

## Where it fits in the code

- **Tray message window:** `src/tray/mod.rs` (`Tray`), which owns `osd: Osd`. `WM_INPUT` can
  be received there.
  - If the tray thread's slow COM work makes the switch feel late, give Raw Input its own thread
    and hidden window, like `src/tray/hook.rs` does for the wheel hook.
  - Measure before choosing.
- **Free `WM_APP` ids:** the app's own are `WM_APP + 1..=19` and `+ 30..=39`; free right now are
  `+2`, `+4..=9` and `+12..=19`. `crates/tap-proto/src/lib.rs` explains the split. Don't renumber
  anything.
- **Config:** `src/config.rs`, `config.toml` in `%APPDATA%\AudioTray`, serde, `#[serde(default)]`
  on every struct.
  - Add a `[keyboards]` table: `enabled`, `show_readout`, and a list of
    `{ device_path, name, hkl }`.
  - Store `hkl` as a hex string or `u64`; a config file must survive a layout being uninstalled.
    Ignore entries whose layout is no longer installed.
- **Settings window:** `src/settings/` (`layout.rs` is pure and tested, plus `model.rs`,
  `paint.rs`, `mod.rs`).
  - Pages are in `Page::ALL`. Add the keyboard section to "Devices", or add a "Keyboards" page.
  - The window keeps its own `Config` copy and hands changes over with `WM_SETTINGS`; the tray's
    `adopt_settings` saves first, then reconfigures what changed.
  - Calls that pump messages must run after the `RefCell` borrow is released (`After` closures).
  - Preview with `--settings-preview [page]`.
- **Readout:** `src/osd.rs` today draws only a volume level (`show(flow, level, muted, anchor)`).
  - Generalise it, or add a sibling with the same window recipe, for a text line with a keyboard
    glyph (Segoe Fluent `E765`, "Keyboard").
  - Keep `--osd` working and add a dev preview.
- **Dev CLI:** `src/dev.rs` (`--features dev`). Add the spike there (step 2).
- **Hiding the indicator (TAP side):**
  - Settings: `hide_windows_volume` / `hide_windows_mic` in `src/config.rs` (`Strip`).
  - Setting toggles in `src/settings/layout.rs` (`Toggle::HideVolume`, `Toggle::HideMic`) and
    `src/settings/mod.rs`.
  - Init data sent to the TAP: `src/taskbar.rs` (`("hidevolume", …)`, `("hidemic", …)`).
  - Parsed by the TAP into `StripState` (`crates/taskbar-tap/src/decorate.rs`).
  - Changes that need a TAP re-apply are compared in `src/tray/mod.rs` (the tuple around line
    622) and in `crates/taskbar-tap/src/lib.rs` (`hide_system_volume, hide_system_mic`
    comparison near line 351).
  - Hiding itself: `Indicator`, `note_system_indicator`, `recheck_indicators`, `enforce_hidden`
    in `crates/taskbar-tap/src/lib.rs`, and `restore::put_back_layout`.

## Steps

### 1. Hide Windows' keyboard-layout indicator (TAP), as its own setting

This is independent of the rest and small. It uses the section-scoped indicator code finished on
2026-11-04.

- **Extend `Indicator` to match by content class** as well as by glyph. The layout indicator has
  no glyph: its text is a localised abbreviation ("FRA").
  - Section: `NonActivatableStack`.
  - Under `SystemTray.IconView > ContainerGrid > ContentPresenter > ContentGrid`, the child is a
    `SystemTray.LanguageTextIconContent` or `SystemTray.LanguageImageIconContent`.
  - Collapse the `IconView` and its `ContentPresenter` slot, exactly like volume and microphone
    (`enforce_hidden`; the "two traps" in FINDINGS still apply).
- **Leave the IME mode icons alone** (`SystemTray.TextIconContent` / `ImageIconContent` in the
  same stack). The user has no IME, and Windhawk treats them as a separate option.
- **Find the indicator without XAML calls in the tree callback.** Queue `IconView`s whose
  recorded children include those class names, as `InnerTextBlock` is queued today
  (`PENDING_GLYPHS`). Type names come from the recorded tree, so the match itself costs no XAML
  call.
  - Re-check each sweep that the content class is still a language one, and put the icon back
    if it is not (mirror `recheck_indicators`).
- **Plumbing:** `hidelanguage` init flag, `Strip::hide_windows_language` (default **false**), a
  toggle labelled "Hide Windows' keyboard layout icon" with a note that Win+Space still switches.
  Add it to the re-apply comparisons in `src/tray/mod.rs` and in the TAP.
- **Gating:** the volume and microphone hides are gated on `strip_placed()` ("Only touch the
  shell's UI once ours is up" in FINDINGS), because they are part of *replacing* those icons.
  - The layout indicator is not replaced by the strip, so its hide is gated on the TAP being
    active (`ACTIVE`) only, not on the strip (agreed with the user, 2026-11-04). It must also
    work with `buttons=0` (music tile only). Record this exception in FINDINGS.
- **26300 reorder:** "next to wifi" uses margins computed from live widths of the sections it
  steps over, including `NonActivatableStack` (`reorder.rs`, FINDINGS "26300: the frame is a
  `StackPanel` now"). Check on screen that the strip still lands right after the indicator is
  collapsed, and after it comes back.
- **Verify:**
  - `cargo clippy --workspace --all-targets --features audio-tray/dev`.
  - `cargo build -p taskbar-tap` needs Explorer to have released the TAP, so ask the user before
    restarting Explorer.
  - Then check `%TEMP%\xaml-tap.log` for the "indicator found" line naming `NonActivatableStack`.
  - Turn the toggle off and on (handover), and confirm the icon comes back and goes away again
    without an Explorer restart.

### 2. Spike: `--keyboards [secs]` dev command, no settings yet

The goal is to measure before building UI. It prints:

- every keyboard from `GetRawInputDeviceList`, with its interface path and the matching PnP
  friendly name if one can be found;
- the installed layouts from `GetKeyboardLayoutList`, with handle, layout id, layout name and
  language name;
- then, live, for each `WM_INPUT` from a *different* device than the last: time, device path,
  the foreground window's process and current layout. **Device and time only, never the key**
  (decision 2).

With `--keyboards switch <device-substring>=<hkl> …`, it also switches as in decision 4, and
prints the layout read back ~50 ms later.

The user types on each keyboard. Things to confirm:

- that each physical keyboard has one interface that actually types;
- what the "Microsoft Keyboard RID" / `hDevice == 0` cases look like here;
- that switching AZERTY ⇄ US-under-French works (the bug the other apps have);
- that a modifier pressed first (decision 3) means no character is mistyped;
- the behaviour in Windows Terminal, a browser, an Office app, and an elevated window.

Write the findings into this file (a "Spike results" section) before step 3.

#### Spike results (2026-11-05)

Run with `--keyboards [secs] switch <path-substring>=<hkl> …`; it also writes its transcript to
`%TEMP%\audio-tray-keyboards.txt` (a GUI-subsystem exe started without a console prints nowhere).

- **Layouts.** `GetKeyboardLayoutList` returns `0x040C040C` ("Français (Traditionnel, AZERTY)")
  and `0x0409040C` ("Anglais (États-Unis)"), both under "Français (France)". The high word is the
  plain KLID (`0000040C` / `00000409`); no `0xFxxx` substitute handle here, despite the
  `d001040c` Preload entry. Names come from `Layout Display Name` + `SHLoadIndirectString`.
- **Keyboards.** Six keyboard interfaces are listed. Every HID one is named "Périphérique clavier
  PIH"; the *parent* devnodes are what tell them apart (`… < G502 LIGHTSPEED` is the mouse's
  receiver). The laptop keyboard is `ACPI\HPQ8002` ("Standard 101/102-Key … for HP Hotkey
  Support"). The external keyboard (on USB at the time) is `VID_19F5&PID_2620`, which exposes
  `MI_00` and `MI_01`: **only `MI_01` types**. Capture-by-keypress (decision 7) is needed.
- **Handles are not stable.** The same external keyboard was `0x505a8`, then `0x4703dd`, then
  `0x16a0a43` across runs; its path never changed. Store the path; rebuild `hDevice → path`.
- **`RIDEV_DEVNOTIFY`** sends `WM_INPUT_DEVICE_CHANGE` (arrival) for every keyboard already
  present, right after registering: the runtime can build its map from those alone.
- **No `hDevice == 0` and no "Microsoft Keyboard RID" events** were seen in these runs (no
  remote session, no injected input). Keep ignoring them anyway.
- **Switching AZERTY ⇄ US-under-French works** (the bug the other apps have): `PostMessageW
  (focus, WM_INPUTLANGCHANGEREQUEST, 0, hkl)` with the full handle. Read back ~60 ms later as
  switched every time in **VS Code, Edge and Windows Terminal** (about 40 switches).
- **Teams (WebView2): read the layout from the focus window's thread.** The first version read
  `GetKeyboardLayout` on the *foreground window's* thread. In Teams the focus is a WebView2 child
  owned by another process, so that read never changed: every switch read back as "NOT switched",
  and twice the spike decided "already US" and skipped a switch that was needed. `foreground()`
  now reads the layout of `GetWindowThreadProcessId(hwndFocus)`. **Not re-verified in Teams yet.**
- **Speed.** The spike's own window answers `WM_INPUT` in well under the ~60 ms read-back delay;
  the tray thread's COM work was not measured. Decide the thread in step 3.
- **First key / modifier first.** The user reports "it seems to work" in practice with the
  Shift-first habit; no mistyped character was reported.
- **Not tested yet:** an elevated window, Office, the Bluetooth connection of the keyboard.

### 3. Runtime switcher

- **Placement:** a `keyboards` module in the app (`src/keyboards/`), owned by the tray. It
  registers Raw Input on start when `[keyboards] enabled` is on, and unregisters on disable
  (`RIDEV_REMOVE`).
- **Device map:** keep `hDevice → device_path` (refreshed on `WM_INPUT_DEVICE_CHANGE`) and
  `device_path → hkl` from config.
- **On a key from a new `hDevice`:** look up its layout. If it has one and the foreground layout
  differs, switch it and show the readout.
- **Ignore:** `hDevice == 0` and virtual devices (decision references above).
- **No logging of keys,** not even in debug builds.
- **Done 2026-11-05** (`src/keyboards/switcher.rs`): on its own thread and hidden window, not the
  tray's. The tray thread's latency was not measured; a thread of its own costs nothing and keeps
  the switch independent of COM stalls, and it stays the one owner of the registration (step 4's
  capture must go through it). The tray spawns it, reconfigures it in place on a settings change,
  and drops it (unregistering) when turned off.

### 4. Settings UI

- **Section:** a "Keyboards" section with:
  - the feature toggle;
  - "Show a confirmation when the keyboard changes";
  - the list of assigned keyboards (name editable, layout picker, remove);
  - an "Add a keyboard" button.
- **Adding a keyboard:**
  - "Add" arms a capture: "Press a key on the keyboard to add…". The next `WM_INPUT` device that
    is not yet assigned is taken.
  - The name defaults to the PnP friendly name, or "Keyboard N" when that is the generic
    "Périphérique clavier PIH".
  - The layout picker lists `GetKeyboardLayoutList` entries as "Layout — Language".
- **Capture plumbing:** the settings window runs on its own thread with its own `Config` copy. Do
  the capture in the tray (which owns Raw Input) and hand the result to the window by message, or
  register Raw Input in the settings window while the capture is armed. Pick whichever keeps a
  single owner of the registration: a process has one registration per usage, and a second
  `RegisterRawInputDevices` for the same usage replaces the first.
- **Layout tests:** add them in `src/settings/layout.rs` like the existing ones.
- **Done 2026-11-05:** a "Keyboards" page (between Devices and Connectors) with the feature switch,
  the pill switch and the hide-icon switch (moved here from Taskbar), a card per keyboard (name,
  "USB/Bluetooth/Built-in · connected", Rename, Remove, and a segmented layout picker that turns
  into a "next layout" button when the labels do not fit), and "Add a keyboard".
  - **Capture** goes through the switcher thread, the one Raw Input owner: `keyboards::capture`
    arms it with the settings window and a message (`WM_APP + 2` in that window's own space); the
    next key from a keyboard not yet added ends it; an added one only says "already here, as …".
    Add is greyed out while the feature is off (no switcher thread to capture with).
  - **Defaults:** the name is the first non-generic name up the devnode chain (a Bluetooth
    keyboard's own, a receiver's), "Built-in keyboard" for an ACPI one, else "Keyboard N"; the
    layout is the first one no other keyboard uses.
  - **Rename** is a minimal text field (the window had none): `WM_CHAR`, Backspace, Enter saves,
    Esc cancels, any other click saves, 40 characters, no surrogate pairs.

### 5. Readout

- **Content:** keyboard glyph, the user's name for the keyboard, the layout name (with " —
  Language" only when two installed layouts share a name: `keyboards::label`).
- **Placement and behaviour:** on the edge pill, like a new song (decision 6). Wired in step 3;
  what is left here is checking it on screen in both themes and a dev preview.
- **When it shows:** only on an actual switch, never when the layout already matched.
- **Previews:** a dev preview (`--osd keyboard`), and screenshots in both themes.

### 6. Docs

- **`docs/NOTES.md` / `FINDINGS.md`:** what the spike measured, the switch mechanism that won,
  and the elevated-window gap.
- **`README.md`:** the feature, the first-key behaviour ("press Shift first"), the privacy
  statement (device identity only, no keys read), and that the lock screen is not covered.

## Risks to keep in mind

- **Raw Input is one registration per process and usage.** If any other part of the app ever
  registers keyboard Raw Input, they will fight; keep one owner.
- **Speed:** the tray thread does slow COM work at times (audio, music). A switch that lands
  after the user's second key defeats the purpose. Measure in the spike (step 2) and move to a
  dedicated thread if needed.
- **Settings take the config path:** changing it must not restart the TAP; the keyboard feature
  has nothing to do with Explorer, except for the hide toggle in step 1.

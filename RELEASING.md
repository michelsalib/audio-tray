# Releasing audio-tray

This repo ships three automated pieces:

| Task | Mechanism | File |
|------|-----------|------|
| Build + installer | GitHub Actions on a `v*` tag → Inno Setup `Setup.exe` | [`.github/workflows/release.yml`](.github/workflows/release.yml), [`packaging/audio-tray.iss`](packaging/audio-tray.iss) |
| Auto-update | In-app updater checks GitHub Releases on launch | [`src/update.rs`](src/update.rs) |
| winget | winget-releaser opens a manifest PR on each release (opt-in) | [`.github/workflows/winget.yml`](.github/workflows/winget.yml), [`packaging/winget/`](packaging/winget/) |

CI ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)) runs clippy/build/test on every push and PR.
The repo must stay **public** so the self-updater and winget can download release assets anonymously.

---

## 1. Cut a release

The tag is the source of truth for the version. It **must** match `version` in
`Cargo.toml` (the release workflow fails otherwise).

### Recommended: `cargo release`

[`release.toml`](release.toml) drives [`cargo release`](https://github.com/crate-ci/cargo-release)
(`cargo install cargo-release`). One command bumps the version, refreshes
`Cargo.lock`, commits `Release v<version>`, tags `v<version>`, and pushes the
commit + tag — which triggers `release.yml`. The tag can't drift from the
Cargo.toml version because both come from the same bump.

```sh
cargo release patch            # dry-run preview (0.4.1 -> 0.4.2); also: minor / major
cargo release patch --execute  # actually do it
```

Publishing to crates.io is disabled (`publish = false` in both `Cargo.toml` and
`release.toml`) — this is an app, not a library.

### Manual (equivalent)

```sh
# 1. Bump the version in Cargo.toml, e.g. 0.1.0 -> 0.1.1
# 2. Update Cargo.lock + commit
cargo build          # refreshes Cargo.lock with the new version
git add Cargo.toml Cargo.lock
git commit -m "Release v0.1.1"
git push

# 3. Tag and push the tag -> triggers .github/workflows/release.yml
git tag v0.1.1
git push origin v0.1.1
```

The workflow builds `--release --workspace` — the workspace part matters, because it
is what produces `audio_tray_tap.dll` from `crates/taskbar-tap` alongside the exe.
It then publishes a GitHub Release with three assets:

- **`AudioTray-0.1.1-Setup.exe`** — the Inno installer (humans + winget). Ships the
  exe **and** the TAP.
- **`audio-tray-x86_64-pc-windows-msvc.zip`** — zipped `audio-tray.exe` plus the
  TAP, consumed by the in-app updater.
- **`SHA256SUMS`** — one `<sha256>  <name>` line for each of the two above. The
  in-app updater refuses a release without it (see §2).

All three names are load-bearing — see the regex in `winget.yml`, and `TARGET` and
`SUMS_ASSET` in `src/update.rs`. Do not publish per-asset `<name>.sha256` files
instead: `audio-tray-x86_64-pc-windows-msvc.zip.sha256` contains `TARGET`, and
`self_update` picks the first asset whose name contains it as the archive.

### If you changed the TAP

`self_update` replaces exactly one file — the one named by `bin_name` — so the DLL
gets its own pass in `update::update_tap`, which runs only when an update was
actually applied. Where it lands depends on whether Explorer has the DLL open:

| DLL state | what happens |
|---|---|
| not loaded (the injection never landed) | copied over the old one, alongside the exe |
| loaded — the normal case | the held file is **renamed** to `audio_tray_tap.dll.old<n>` and the new one copied in. Explorer keeps running the old image until it restarts |
| loaded and cannot be renamed (not expected) | **staged** in `%TEMP%\audio-tray-tap-<version>\` for `update::place_staged_tap` |

The second row is the usual one, since the strip is injected on every start. A loaded
DLL cannot be overwritten or deleted, but it can be renamed, and that needs no
elevation. `update::install_tap` does it, and the installer does the same thing
(`SetAsideLoadedTap` in `packaging/audio-tray.iss`). The `.old<n>` files are deleted
whenever nothing holds them any more (on the next update check, `place_staged_tap`, or
uninstall).

There is deliberately **no boot-time fallback**. `MoveFileExW(MOVEFILE_DELAY_UNTIL_REBOOT)`
and Inno's `restartreplace` both record the rename under
`HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\PendingFileRenameOperations`,
which needs administrator rights. A per-user install in `%LocalAppData%\Programs\AudioTray`
never has them, so the call failed with access denied and the DLL was never scheduled.
`--taskbar-revert` does not free the file either: the revert deliberately leaves the DLL
pinned in `explorer.exe` (see `src/taskbar.rs`). Restarting Explorer is what frees it.

**Users do not have to restart Explorer themselves.** Taking the update gets the new DLL
running without anyone asking for it:

1. `restart_app` relaunches audio-tray into the new exe (with `--relaunched`, so it waits
   for the single-instance mutex instead of exiting), and hands the strip's ownership to the
   child so the TAP's owner watch ignores the old process exiting.
2. The new process starts, and `taskbar::apply_at_startup` finds the *old* TAP still loaded
   in Explorer (the shell keeps it for its own lifetime). It first offers it a **handover**:
   its init data (`ver=`, `tap=`, `hwnd=`, `pid=` …) over `WM_COPYDATA` to the TAP's control
   window. A TAP from the same version and the same DLL file accepts, re-binds owner watch and
   receiver, and re-applies the strip — no Explorer restart. That is what happens on Quit then
   start, after a kill, and on a relaunch that did not change the version.
3. After an update the versions differ, so the TAP declines (an older TAP that predates the
   handover, or one that does not answer within 3 s, counts the same) and audio-tray calls
   `taskbar::restart_explorer`. That waits for the old shell to exit and, in the gap where
   nothing holds the DLL, calls `update::place_staged_tap`. Normally the new DLL is already in
   place (set aside above) and that only deletes the `.old<n>` files. A *staged* DLL it finds
   by the staging directory for its *own* `CARGO_PKG_VERSION`, so it does not matter that a
   different process downloaded it.
4. The fresh Explorer is injected into off `TaskbarCreated`, with the new DLL.

If none of that happens, a DLL that was set aside loads the next time Explorer starts
(sign-in, reboot). A *staged* one does not: that Explorer gets the old file, and the new
build's `repair_stale_tap` then places the staged copy (setting the loaded one aside), so it
lands one Explorer start later. The restart in step 3 is also the general repair path — it is
not update-specific — and it is budgeted to one restart per run.

Failing to place the DLL is logged and otherwise ignored: the exe has already been
replaced by then, and turning a good update into a bad one over the DLL would be the
wrong trade.

**Still keep the init-data protocol backwards compatible.** A stale DLL is always
possible — Explorer may not have restarted yet — and it degrades rather than breaks
because unknown `key=value` pairs are ignored and missing ones fall back to defaults.
Do not add a key the DLL must have.

### When the two halves drift apart

That tolerance cuts both ways: because a stale TAP degrades quietly, a *permanently*
stale one is invisible. It happened. From v0.9.0 to v0.10.1 `update_tap` asked GitHub
for the release asset without an `Accept: application/octet-stream` header — and
`self_update`'s `download_url` is the **api.github.com** url, not the browser one, so
the endpoint answered with the asset's JSON metadata. 1.7 KB landed on disk named
`.zip`, extraction failed, and the failure is deliberately non-fatal. Users ran a
v0.10.0 exe beside a v0.8.0 DLL: no YouTube Music tile, and thumbnail-toolbar transport
buttons that drew and did nothing.

Two things now guard against a repeat.

**The DLL carries its version.** `crates/taskbar-tap/build.rs` stamps a Windows version
resource, reading the number from the **root** `Cargo.toml` — the crate's own version is
`0.0.0`, because `cargo release` skips it (`publish = false`). A resource travels inside
the file, so the installer and the self-updater both get it right by construction and
there is no marker file that can land out of step with what it describes.

**The new build repairs what the old one left.** `update::repair_stale_tap` compares
that stamp against `CARGO_PKG_VERSION` and re-fetches the DLL if they disagree. This is
the half that matters, because `update_tap` runs *in the process being replaced* — a bug
in it can only be fixed one release later, by the build that comes after. It runs from
the background check and from `--update`, and only on `VersionStatus::UpToDate`: right after an
update the new DLL is already on disk against this old process's version, so comparing
there would "repair" a downgrade.

Debug builds report and change nothing, or `--update` in a dev tree would overwrite the
TAP you are working on with a release one.

```powershell
audio-tray --tap-version   # what is actually on disk, next to the exe
audio-tray --update        # checks, then repairs the DLL if it is out of step
```

A repair whose copy is blocked (Explorer has the file) sets the old file aside like an
update does. Only if that fails too is the DLL staged and reported, and the next
launch retries only the *copy* — it does not download 2 MB again per start. The one
outcome that logs an error is a release whose own asset is unstamped, which would put the
check back where it started; `build.rs` panics rather than emit an unstamped DLL, so that
should not be reachable from CI.

---

## 2. How auto-update behaves

On tray launch (release builds only — debug builds skip it) a background thread asks GitHub
for the latest release. If it's newer than the running `CARGO_PKG_VERSION`, it downloads the
`.zip` asset and replaces `audio-tray.exe` in place. Because the app installs **per-user** to
`%LocalAppData%\Programs\AudioTray`, no admin rights are needed.

The update is applied **on next launch** (i.e. next sign-in for an autostart install) — the
running tray is never killed out from under you.

- Force a check now: `audio-tray --update`
- The updater is silent and failure-tolerant: no network / GitHub down = no-op, tray unaffected.

### Integrity check

Before anything on disk is replaced, the updater downloads the release's `SHA256SUMS` and
checks the `.zip` against its line, for the exe (`self_update`'s `checksum_from_asset`) and
for the TAP (`update::update_tap`, which downloads the same `.zip` again). A release without
`SHA256SUMS`, without a line for the `.zip`, or whose `.zip` does not match is **refused**:
the error is logged (or printed by `--update`) and the current install stays as it is. So
never publish a release by hand without it. `self_update` also checks GitHub's own per-asset
`digest` when the API reports one.

This protects against a corrupted or truncated download, and against an asset swapped on
its way to the user. It does **not** protect against someone who can write to the GitHub
release itself (a leaked token or a compromised workflow): they can replace `SHA256SUMS`
along with the `.zip`. That needs a signature, below.

A change here takes effect one release later, like any `update_tap` fix: the update is run
by the *old* build. Builds before the checksum check download unverified, including the
update *to* the first build that has it.

### Optional next step: signatures

Real authenticity needs a private key that never leaves the maintainer's control (a GitHub
secret). There is none yet, so none of this is implemented. Two complementary options:

1. **Sign the update archive** — `self_update`'s `signatures` feature (zipsign, ed25519).
   - Create a key pair once, locally: `cargo install zipsign`, then
     `zipsign gen-key priv.key pub.key`. Store `priv.key` (base64) as the repo secret
     `ZIPSIGN_PRIVATE_KEY`, and keep an offline backup. Losing it means a new key and
     a release that ships the new public key.
   - In `release.yml`, after packaging and before `SHA256SUMS`, decode the secret to a
     file and sign the `.zip` with `zipsign sign zip …` (see `zipsign sign --help` for the
     output flag). The signature is embedded in the zip, so the asset name does not change.
   - In the app, enable the `signatures` feature on `self_update`, embed `pub.key` with
     `include_bytes!`, pass it to `.verifying_keys([..])` in `check_and_apply`, and verify
     the TAP's archive in `update_tap` with `self_update::verify_signature` before extracting.
   - Roll out in two releases: first one whose CI signs (the app does not check yet), then
     the one that requires it, or the first signed build would refuse every older release.
   - [minisign](https://jedisct1.github.io/minisign/) over `SHA256SUMS`
     (`SHA256SUMS.minisig`) works the same way and also covers `Setup.exe`, but the app would
     need its own verifier crate (e.g. `minisign-verify`).
2. **Authenticode** — sign `audio-tray.exe`, `audio_tray_tap.dll` and `Setup.exe` with a
   code-signing certificate (§4). The updater could then also check
   `WinVerifyTrust` on the extracted files before installing them. This is what removes the
   SmartScreen prompt; option 1 is free and is what protects the updater.

---

## 3. winget (free, optional)

Publishing to the Windows Package Manager Community Repository is free. It requires a **one-time
manual submission**, after which the workflow keeps it updated automatically.

### 3a. One-time initial submission

Easiest path is Microsoft's `wingetcreate` (it computes the SHA256 and picks the schema):

```powershell
winget install wingetcreate
# Point it at the installer asset URL from your first release:
wingetcreate new https://github.com/michelsalib/audio-tray/releases/download/v0.1.0/AudioTray-0.1.0-Setup.exe
# Fill in the prompts (identifier MichelSalib.AudioTray, publisher MichelSalib, etc.),
# then let it submit the PR to microsoft/winget-pkgs.
```

Or submit by hand using the templates in [`packaging/winget/`](packaging/winget/): fill in the
real `InstallerSha256`, validate, and open a PR against `microsoft/winget-pkgs` under
`manifests/m/MichelSalib/AudioTray/0.1.0/`:

```powershell
(Get-FileHash .\AudioTray-0.1.0-Setup.exe -Algorithm SHA256).Hash   # -> InstallerSha256
winget validate --manifest .\packaging\winget
```

**winget requirements to know:** installer must be publicly downloadable (GitHub release URL is
fine); a `License` is required (add a `LICENSE` file to the repo); unsigned installers are
accepted but Microsoft's automated scan + SmartScreen may flag an unknown publisher. Review by
maintainers can take a day or two.

### 3b. Automatic updates thereafter

Once the package exists in winget-pkgs, enable the workflow:

1. Create a **classic** PAT (fine-grained tokens are *not* supported) with the **`public_repo`**
   scope: <https://github.com/settings/tokens>.
2. Repo → Settings → Secrets and variables → Actions:
   - Add secret **`WINGET_TOKEN`** = that PAT.
   - Add variable **`PUBLISH_WINGET`** = `true`.

Now every stable release auto-opens an update PR to winget-pkgs. Users update with
`winget upgrade MichelSalib.AudioTray` (or `winget upgrade --all`). Note winget upgrades are
**not** automatic — that's what the in-app updater in §2 is for.

---

## 4. Code signing (future)

The installer and exe are currently **unsigned**, so users see a SmartScreen "unknown publisher"
prompt on first run. To remove it, sign `audio-tray.exe` (and the `Setup.exe`) with a code-signing
certificate. Realistic options: **Azure Trusted Signing** (~$10/mo, needs a verified org/individual
identity) or an OV/EV cert from a CA. Free signing is effectively unavailable. When you have a cert,
add a signing step to `release.yml` (after the build, before packaging) using GitHub secrets.

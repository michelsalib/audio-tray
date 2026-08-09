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
It then publishes a GitHub Release with two assets:

- **`AudioTray-0.1.1-Setup.exe`** — the Inno installer (humans + winget). Ships the
  exe **and** the TAP.
- **`audio-tray-x86_64-pc-windows-msvc.zip`** — zipped `audio-tray.exe` plus the
  TAP, consumed by the in-app updater.

Both asset names are load-bearing — see the regex in `winget.yml` and `TARGET` in `src/update.rs`.

### If you changed the TAP

`self_update` replaces exactly one file — the one named by `bin_name` — so the DLL
gets its own pass in `update::update_tap`, which runs only when an update was
actually applied. Where it lands depends on whether Explorer has the DLL open:

| DLL state | when the new DLL takes effect |
|---|---|
| not loaded (the injection never landed) | immediately, alongside the exe |
| loaded — the normal case | **next reboot** — handed to `MoveFileExW` with `MOVEFILE_DELAY_UNTIL_REBOOT` |

That second row is the same mechanism as the installer's `restartreplace`, and it is
now the usual outcome: the strip is injected on every start. `--taskbar-revert` does
not help — the revert deliberately leaves the DLL pinned in `explorer.exe` (see
`src/taskbar.rs`), so the file stays locked. Restarting Explorer is what frees it.

**Users do not usually wait for that reboot.** Taking the update applies the DLL too,
without anyone asking for it:

1. `restart_app` relaunches audio-tray into the new exe.
2. The new process starts, and `taskbar::apply_at_startup` finds the *old* process's TAP
   still loaded in Explorer (the shell keeps it for its own lifetime). Rather than inject
   alongside it, it calls `taskbar::restart_explorer`.
3. That waits for the old shell to exit and, in the gap where nothing holds the DLL,
   calls `update::place_staged_tap` — which finds the staging directory for its *own*
   `CARGO_PKG_VERSION`, so it does not matter that a different process downloaded it.
4. The fresh Explorer is injected into off `TaskbarCreated`, with the new DLL.

The pending boot rename is left scheduled throughout, so a reboot remains the fallback if
any of that does not happen. Note step 2 is also the general repair path — it is not
update-specific — and it is budgeted to one restart per run.

Failing to place the DLL is logged and otherwise ignored: the exe has already been
replaced by then, and turning a good update into a bad one over the DLL would be the
wrong trade.

**Still keep the init-data protocol backwards compatible.** A stale DLL is always
possible — the reboot may not have happened yet — and it degrades rather than breaks
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
the background check and from `--update`, and only on `Status::UpToDate`: right after an
update the new DLL is already on disk against this old process's version, so comparing
there would "repair" a downgrade.

Debug builds report and change nothing, or `--update` in a dev tree would overwrite the
TAP you are working on with a release one.

```powershell
audio-tray --tap-version   # what is actually on disk, next to the exe
audio-tray --update        # checks, then repairs the DLL if it is out of step
```

A repair that cannot copy (Explorer has the file) is staged and reported, and the next
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

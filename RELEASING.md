# Releasing audio-tray

This repo ships three automated pieces:

| Task | Mechanism | File |
|------|-----------|------|
| Build + installer | GitHub Actions on a `v*` tag → Inno Setup `Setup.exe` | [`.github/workflows/release.yml`](.github/workflows/release.yml), [`packaging/audio-tray.iss`](packaging/audio-tray.iss) |
| Auto-update | In-app updater checks GitHub Releases on launch | [`src/update.rs`](src/update.rs) |
| winget | winget-releaser opens a manifest PR on each release (opt-in) | [`.github/workflows/winget.yml`](.github/workflows/winget.yml), [`packaging/winget/`](packaging/winget/) |

CI ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)) runs fmt/clippy/build/test on every push and PR.

---

## 0. One-time: create the public GitHub repo

`git init` and the first commit are already done locally. Create the remote and push
(no `gh` CLI installed, so do it in the browser + git):

1. Create an **empty public** repo at <https://github.com/new> named `audio-tray`
   (owner `michelsalib`). Do **not** add a README/license/`.gitignore` — the repo already has them.
2. Push:
   ```sh
   git remote add origin https://github.com/michelsalib/audio-tray.git
   git branch -M main
   git push -u origin main
   ```

The repo **must be public** for the self-updater and winget to download release assets anonymously.

---

## 1. Cut a release

The tag is the source of truth for the version. It **must** match `version` in
`Cargo.toml` (the release workflow fails otherwise).

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

The workflow builds `--release`, then publishes a GitHub Release with two assets:

- **`AudioTray-0.1.1-Setup.exe`** — the Inno installer (humans + winget).
- **`audio-tray-x86_64-pc-windows-msvc.zip`** — zipped `audio-tray.exe`, consumed by the in-app updater.

Both asset names are load-bearing — see the regex in `winget.yml` and `TARGET` in `src/update.rs`.

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

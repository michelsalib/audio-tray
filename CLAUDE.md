# audio-tray

Windows system-tray app (Rust, MSVC toolchain) for viewing and switching the default audio
device. Workspace members: `crates/taskbar-tap` (the TAP DLL loaded into explorer.exe) and
`crates/tap-proto` (constants shared by both sides).

## Dev loop: use only these commands

Every distinct cargo variant (`build`/`check`/`clippy`, root vs `--workspace`, debug vs
`--release`) has its own copy of the dependencies, and its first run recompiles the huge
`windows` crate cold (~1–2 min). Once warm, an edit costs ~2–4 s. So stick to this set and
don't invent new combinations:

| Purpose                     | Command                                                        |
|-----------------------------|----------------------------------------------------------------|
| Check + lint (no binary)    | `cargo clippy --workspace --all-targets --features audio-tray/dev` |
| Build the app to run it     | `cargo build --features dev`                                   |
| Tests                       | `cargo test --features dev` (root only: a workspace test relinks the TAP DLL) |
| Build the TAP DLL           | `cargo build -p taskbar-tap` (only when the TAP changed)       |

- `clippy` doesn't produce a binary. Don't run `cargo check` as well as clippy. Run
  `cargo build` only when the app needs launching.
- Avoid `--release` locally unless measuring release behaviour. CI covers it.
- Don't change the linker, `RUSTFLAGS` or profiles ad hoc. Any of these invalidates every
  cached artifact (switching to `rust-lld` was measured: full rebuild, no speedup).
- The root `windows` feature list is kept a superset of `taskbar-tap`'s, so root and
  `--workspace` builds share one `windows` build. When adding a `windows` feature to the
  TAP, add it to the root `Cargo.toml` too.
- rust-analyzer checks into `target/rust-analyzer` (`.vscode/settings.json`), so it
  never holds the build lock.

## Gotchas

- `cargo` isn't on the Bash PATH. Run it from PowerShell with
  `$env:PATH="$env:USERPROFILE\.cargo\bin;$env:PATH"`.
- Kill a running `audio-tray.exe` before `cargo build`, or linking fails with
  "Accès refusé (os error 5)": `Get-Process audio-tray | Stop-Process -Force`.
  `cargo build -p taskbar-tap` fails the same way while Explorer has the TAP loaded.
- Don't run `rustfmt` / `cargo fmt` on the code. It's hand-formatted.
- `windows` is pinned to 0.61 to match `com-policy-config` 0.6. Don't bump it alone.
- Tests that switch the live default audio device must restore the original device after.

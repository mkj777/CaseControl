# Brief: 0.1.0 release pipeline and auto-update (ADR 0001 #39)

Read `docs/briefs/phase2/00-common-rules.md`, ADR 0001 #3, #11, #13, #32, #39, `.github/workflows/ci.yml`, `deny.toml`,
`apps/desktop/src-tauri/{Cargo.toml,tauri.conf.json,capabilities,build.rs,src/lib.rs}`, the workspace `Cargo.toml`.
You own those plus new workflow files, the updater module and `docs/architecture.md` / `docs/releasing.md` sections
on updates and releases. Another worker edits `crates/**` and other `src-tauri/src` files in parallel; do not touch
theirs. Do not launch, tauri-build or kill the app. Do not push, tag, create releases or touch GitHub settings or
secrets: the orchestrator does that.

## Owner decisions

- Releases on GitHub in `mkj777/CaseControl` (the repo will be public). Version 0.1.0.
- Tauri updater plugin (`tauri-plugin-updater`, plus `tauri-plugin-process` if needed for restart). It is the only
  network access in the app. Allow `reqwest` in `deny.toml` only as the updater's dependency (scoped `wrappers` /
  comment), keep the ban for any other crate.
- Behaviour: check on start (after the engine is up, off the main thread) and every 6 hours; download a newer signed
  update silently in the background; install it on the next quit (tray Quit, `--quit`, logoff/shutdown handler) and
  on the next start if a downloaded update is pending; never interrupt the user, no dialog. Devices are released
  before the installer runs (reuse the normal shutdown path). Log every step. Failures (offline, GitHub down, bad
  signature) are logged and retried next interval, never shown as errors. NSIS install mode passive/quiet, per-user
  (ADR #32).
- Settings and UI: none in this release (no toggle). Debug builds never check for updates.

## Pipeline

- Updater endpoint: `https://github.com/mkj777/CaseControl/releases/latest/download/latest.json`.
- Updater public key (put in `tauri.conf.json` `plugins.updater.pubkey`):
  `dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDQxNTc3NkJCNjUxMTYzRjIKUldU` + the rest of the file
  `C:\Users\maxik\.tauri\casecontrol-updater.key.pub` (read the full value from that file; never read or copy the
  private key file next to it).
- `bundle.createUpdaterArtifacts: true`.
- New workflow `.github/workflows/release.yml`: on push of tags `v*.*.*`; job 1 runs the same checks as `ci.yml`
  (reuse via `workflow_call` or a shared job); job 2 on `windows-latest` builds with `tauri-apps/tauri-action`
  (pinned SHA) using secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, creates the GitHub
  release for the tag with the NSIS installer, its `.sig` and `latest.json` (`includeUpdaterJson`), not as draft,
  release notes from the tag message. Fail if the tag version differs from `tauri.conf.json` / Cargo versions.
  Minimal permissions (`contents: write` only on the release job), pinned action SHAs, `persist-credentials: false`.
- Versions: keep 0.1.0 everywhere; add a short `docs/releasing.md` (bump versions, tag, push tag, what CI does, key
  location and backup, rollback = publish a newer version).

## Tests and checks

Unit tests for the updater state machine (pending update install on quit/start, no check in debug, interval) with the
plugin behind a small trait. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo nextest run --workspace`, `cargo deny check` (if installed; else say so), `pnpm typecheck`, `pnpm build`. No
commit. One final report including exactly what the orchestrator must do on GitHub.

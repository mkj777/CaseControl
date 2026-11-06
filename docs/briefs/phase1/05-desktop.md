# Brief: `apps/desktop` + CI (Phase 1)

You own `apps/desktop/**`, `.github/**`, and the repo-root files `deny.toml` and `pnpm-workspace.yaml` (only if you
need the latter). The Tauri crate `apps/desktop/src-tauri` already exists (created with `cargo tauri init`, then edited):
package `casecontrol-desktop`, lib `casecontrol_desktop_lib`, `tauri` with `tray-icon`, `tauri-plugin-single-instance`,
`tauri-plugin-autostart`, `tracing*`, `directories`, `anyhow` as workspace deps; `tauri.conf.json` points at `../dist`,
dev URL `http://localhost:5173`, `pnpm dev` / `pnpm build`, NSIS bundle only, strict CSP. `apps/desktop/dist/index.html`
is a placeholder that your frontend build replaces (`dist/` is git-ignored).

## Goal

The desktop shell from `docs/architecture.md` section "Desktop app", without device features (those come in Phase 2):
a Tauri 2 process that starts, shows a window with four English pages, minimizes to the tray on close, enforces a
single instance, logs to files, and exposes a first IPC command. Plus the CI pipeline and `cargo-deny` configuration.
Decisions that bind you: ADR 0001 #3 (one process, tray), #7 (Vue 3 + TS, Vite, pnpm), #13 (no network, no telemetry),
#14 (English; pages Devices, Profiles, Conflicts, Settings; tray menu Show, Apply profile, Release devices, Quit),
#15 (dark and spacious; the owner settles details later, so keep the first pass neutral: no UI component library,
CSS custom properties for colors/spacing, system font stack), #16 (name CaseControl).

## Frontend (`apps/desktop`, Vite root)

- Scaffold Vue 3 + TypeScript with Vite and vue-router (pinia is fine if you want a store now; keep it small).
  `package.json` name `casecontrol-desktop`, `private: true`, scripts: `dev` (vite), `build` (`vue-tsc --noEmit && vite build`),
  `typecheck` (`vue-tsc --noEmit`), `preview`, `tauri` (`tauri`). Dev deps: `@tauri-apps/cli` ^2 (so `pnpm tauri dev`
  works without the cargo binary), `vue-tsc`, `typescript`, `@vitejs/plugin-vue`. Deps: `vue`, `vue-router`, `@tauri-apps/api` ^2.
  Commit `pnpm-lock.yaml`. Use pnpm 11 (installed).
- `vite.config.ts` per Tauri docs: `clearScreen: false`, `server: { port: 5173, strictPort: true, watch: { ignored: ['**/src-tauri/**'] } }`,
  `envPrefix: ['VITE_', 'TAURI_ENV_*']`, `build.target` `chrome105` on Windows, `minify` off for debug builds,
  `sourcemap` when `TAURI_ENV_DEBUG`.
- Layout: left sidebar with the four routes, content area; each page a placeholder with its title and a one-sentence
  description of what it will hold. Settings page shows the result of `invoke('app_info')` (name, version, build profile)
  and a "Hide to tray" button calling `invoke('hide_to_tray')`. Dark theme only for now, generous spacing.
- No remote URLs, no fonts from the network, no analytics.

## Rust side (`apps/desktop/src-tauri`)

- `lib.rs::run()`: Builder with `tauri_plugin_single_instance` (on second launch: show and focus the `main` window),
  `tauri_plugin_autostart` registered (`MacosLauncher::LaunchAgent` placeholder arg is irrelevant; pass `Some(vec!["--hidden"])`),
  not enabled by default (Settings page wires it in Phase 3). Tray: `TrayIconBuilder` with the app icon, tooltip
  "CaseControl", menu Show / Apply profile (disabled, placeholder) / Release devices (disabled, placeholder) / separator / Quit;
  left click shows and focuses the window. Window `CloseRequested` -> `api.prevent_close()` + `hide()`. Quit from the tray
  exits the process (later phases hook device release here; leave a clearly named function `shutdown(app)`).
  Honor a `--hidden` CLI argument: start with the window hidden.
- Logging: `tracing-subscriber` with `EnvFilter` (`CASECONTROL_LOG`, default `info`), writing to stderr in debug and to a
  daily rolling file under `%LOCALAPPDATA%\CaseControl\logs`, computed as
  `directories::BaseDirs::new().data_local_dir().join("CaseControl").join("logs")` (not `ProjectDirs`, which would add
  vendor folders; Phase 2's `cc-config` will own one `paths` definition that must match this). Log startup, tray actions,
  close-to-tray, quit.
- Commands: `app_info() -> AppInfo { name, version, profile: "debug" | "release" }`, `show_window(app)`, `hide_to_tray(app)`,
  `quit(app)`. Register them; capabilities in `capabilities/default.json` limited to what the pages use
  (`core:default` plus the window permissions needed; no `shell`, no `fs`, no `http`).
- Keep `tauri.conf.json` as is unless something is wrong; you may add `app.trayIcon` if you prefer config over code,
  but the menu must come from code.

## CI and supply chain (`.github/workflows/ci.yml`, `deny.toml`)

- Workflow `ci` on `pull_request` and `push` to `main`, `permissions: contents: read`, `concurrency` cancel-in-progress,
  runner `windows-latest`. Steps: checkout; `pnpm/action-setup` (reads the pnpm version from the `packageManager`
  field you set in `package.json`, so pass no `version` input); `actions/setup-node` 24 with `cache: pnpm` and
  `cache-dependency-path: apps/desktop/pnpm-lock.yaml` (the lockfile is not at the repo root);
  `dtolnay/rust-toolchain` stable with `rustfmt, clippy`; `Swatinem/rust-cache`; `taiki-e/install-action` for `cargo-nextest`
  and `cargo-deny`; `pnpm install --frozen-lockfile` and `pnpm typecheck` and `pnpm build` in `apps/desktop`;
  `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo nextest run --workspace`;
  `cargo deny check`. Second job `installer` (needs `check`, only on push to `main`): install `tauri-cli` via
  `taiki-e/install-action`, `pnpm install --frozen-lockfile`, `cargo tauri build`, upload `target/release/bundle/nsis/*.exe`
  as artifact `CaseControl-nsis`. Pin every action to a full commit SHA with a `# vX.Y.Z` comment (resolve with
  `gh api repos/<owner>/<repo>/git/ref/tags/<tag>`, dereferencing annotated tags). Add `.github/dependabot.yml` for
  `cargo`, `npm` (directory `/apps/desktop`) and `github-actions`, weekly.
- `deny.toml` at the repo root: `[advisories]` deny vulnerabilities, warn unmaintained; `[licenses]` allow list derived
  from `cargo deny list` / `cargo tree` (expect MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2/3, ISC, Zlib,
  Unicode-3.0, MPL-2.0, GPL-2.0-or-later for our own crates, CC0-1.0, 0BSD, bsl-1.0 if present); `[bans]` deny
  `reqwest`, `ureq`, `curl`, `isahc`, `attohttpc`, `surf` (ADR 0001 #13); run `cargo deny check` locally and iterate until
  it passes without blanket `skip`s. If Tauri pulls `hyper` or `reqwest` transitively, do not ban it blindly: report the
  dependency path so the orchestrator can decide (Tauri's optional `http` features should be off).

## Verify before reporting

`pnpm install`, `pnpm typecheck`, `pnpm build`; `cargo check -p casecontrol-desktop`; `cargo clippy -p casecontrol-desktop --all-targets -- -D warnings`;
`cargo fmt -p casecontrol-desktop --check`; `cargo deny check`; `cargo tauri build --no-bundle` from `apps/desktop`
(full release compile, several minutes). You cannot verify the tray or window behavior unattended; describe exactly what
the orchestrator should click to verify (`cargo tauri dev` run is the orchestrator's). Do not run `cargo tauri dev` yourself.

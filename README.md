# CaseControl

Windows tool that detects RGB-capable PC hardware and sets its colors, shows which other software competes for the
same devices, and coexists with FanControl. Rust workspace plus a Tauri 2 desktop app (Vue 3). No ads, no
telemetry, no kernel driver, no admin rights; the only network access is the signed auto-update from GitHub releases.

Start with `docs/architecture.md`, then `docs/decisions/0001-project-decisions.md` and `docs/build-plan.md`.
Research behind the design is in `docs/research/`. Worker briefs for each build phase are in `docs/briefs/`.

## Layout

| Path | Purpose |
|---|---|
| `crates/cc-core` | device model, controller and backend traits (no I/O) |
| `crates/cc-hid` | HID transport (hidapi), scripted fake for tests, Windows hot-plug, named-mutex guard |
| `crates/cc-nvapi` | dynamic NvAPI binding for GPU illumination |
| `crates/cc-drivers` | device drivers (Phase 2/3) |
| `crates/cc-conflicts` | conflicting-software catalog, scanners, opt-in actions |
| `crates/cc-config` | settings and profiles (Phase 2) |
| `crates/cc-engine` | runtime: detection, device threads, frame scheduler (Phase 2) |
| `apps/desktop` | Tauri 2 shell (`src-tauri`) and Vue 3 frontend (`src`) |
| `reference/openrgb` | GPLv2 sources copied for protocol reference |

## Build and check

```
cd apps/desktop && pnpm install --frozen-lockfile && pnpm typecheck && pnpm build
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo deny check
cd apps/desktop && pnpm tauri dev        # run the app
```

Read-only hardware probes (never run in CI):

```
cargo run -p cc-hid --example list_hid
cargo run -p cc-conflicts --example scan
cargo run -p cc-nvapi --example dump_illum
```

## Rules that every change keeps

- Devices are opened shared, never exclusively; every driver restores the firmware mode on `release()`.
- `unsafe` lives only in `cc-hid`, `cc-nvapi` and the `sys` module of `cc-conflicts`, each block with a `// SAFETY:` comment.
- No test opens a real device, loads NvAPI, terminates a process, stops a service or writes the registry.
- Nothing reaches `main` without the CI workflow passing.

## Licence and credits

CaseControl is copyright Maximilian Kielholz and licensed under the GNU General Public License, version 2 or (at your
option) any later version (`GPL-2.0-or-later`); the full text is in `LICENSE`. The device drivers are modified ports
of [OpenRGB](https://gitlab.com/CalcProgrammer1/OpenRGB) (Adam Honse and contributors, GPL-2.0-or-later); the
eyedropper icon follows Lucide's "pipette" (ISC). Details are in `NOTICE.md`.

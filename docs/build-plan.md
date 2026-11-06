# Build plan (awaiting owner's go)

Orchestration: the orchestrator writes briefs, creates the workspace skeleton, reviews results and runs all hardware
tests. Code is written by worker agents (Opus 5.5, effort medium), one crate per worker, strict crate ownership,
same working tree. Each phase lands as one PR against `main` once the repo exists.

## Phase 0: repository (orchestrator)

- `git init`, `.gitignore` (target/, node_modules/, dist/, logs), first commit with docs and `reference/`.
- Private GitHub repo via `gh repo create` (account mkj777), push, branch protection on `main` once CI exists.
- Workspace `Cargo.toml` with all members, pinned `[workspace.dependencies]`, stub `lib.rs` per crate, `cargo generate-lockfile`.

## Phase 1: foundations (5 workers in parallel, no hardware)

| Worker | Crate | Deliverable | Tests |
|---|---|---|---|
| core | `cc-core` | model types, traits, `DeviceId` rules, errors, events | invariants, serde round-trips |
| hid | `cc-hid` | `HidTransport`, hidapi backend, `FakeTransport`, enumerate/open, hot-plug via `CM_Register_Notification`, `NamedMutexGuard` | FakeTransport scripts, match logic |
| nvapi | `cc-nvapi` | dynamic `nvapi64.dll` binding, Illumination GetControl/SetControl, byte-exact structs from `nvapi.h` | struct size/layout asserts, no device calls in tests |
| conflicts | `cc-conflicts` | `catalog.toml` from research 03, scanners (processes, services + driver services, uninstall keys, Dynamic Lighting registry), opt-in actions | scanners behind traits with injected fakes |
| desktop | `apps/desktop` | Tauri 2 scaffold, tray, hide-on-close, single instance, Vue 3 + TS + pnpm, placeholder pages, CI workflow | `pnpm vue-tsc`, `cargo tauri build` |

Gate: `cargo fmt`, `clippy -D warnings`, `nextest` green; orchestrator review of every crate against `docs/architecture.md`.

## Phase 2: first light (vertical slice)

- `cc-drivers::msi_mystic_light` ported from `reference/openrgb/MSIMotherboardController` (worker).
- Minimal `cc-engine` (detection, one worker thread per device, release on quit) and `cc-config` (settings only) (worker).
- Tauri commands `list_devices`, `set_direct`, `set_zone`, `release_device` plus a Devices page (worker, after UI direction is known).
- Orchestrator: hardware test on this machine with FanControl running; case lighting changes color, quit restores firmware mode.

## Phase 3: remaining devices and features

- Drivers in parallel: `wooting` (V3), `logitech` (HID++ 2.0 G560, Lightspeed PRO Wireless), `nvidia_illumination` (RTX 5080 FE).
- Profiles (TOML), Conflicts page with actions, Settings page (autostart, start minimized, frame rate, detector toggles).
- `cargo-deny` configuration from `cargo tree`, hardware checklist `docs/testing-hardware.md`, release job producing the installer.

## Owner decisions

All answered, see ADR 0001 #14 to #17. The remaining gate is the owner's go.

## Status

- Phase 0: done 2026-11-06 (repo `mkj777/CaseControl`, workspace skeleton, Tauri crate).
- Phase 1: built 2026-11-06 by five Opus workers (workflow run `wf_c805e6ed-a57`), reviewed per crate, minor findings fixed
  in two follow-up passes, cc-conflicts re-reviewed in depth. Gate: 130 tests, fmt, clippy `-D warnings`, rustdoc
  `-D warnings`, cargo-deny all clean. Read-only hardware probes run on the development machine: `list_hid` finds
  Mystic Light, Wooting and both Logitech devices; `scan` reports 9 findings; `dump_illum` reads the RTX 5080 FE
  (3 single-color zones, brightness only). Manual window/tray check still pending (a full-screen game was running).
- Phase 2: waits for the owner's go.

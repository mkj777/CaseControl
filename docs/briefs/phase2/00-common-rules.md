# Phase 2 worker rules (apply to every brief in this folder)

Read first, in this order: `docs/architecture.md`, `docs/decisions/0001-project-decisions.md`, `docs/build-plan.md`,
then your own brief. The research reports in `docs/research/` hold the facts the architecture is built on; consult them
when your brief points at one. `reference/openrgb/` contains GPLv2 C++ sources copied for porting. The project is
private and the owner allowed porting from them (ADR 0001 #4), so port freely but write idiomatic Rust, not transliterated C++.

## Decisions are taken; do not ask, do not delegate

The owner's global instructions ask for decisions to be handed back before acting. For this phase that has already
happened: every architecture, library, naming and scope decision is recorded in ADR 0001, `docs/architecture.md` and your
brief. You cannot ask anyone mid-run, so where a brief leaves a detail open, make the conservative choice that follows
the architecture, record it under "deviations" in your report, and keep going. Never stop to wait for an answer.
Do the work yourself: do not spawn agents or workflows.

## Ownership and concurrency

- Six workers run at the same time in the same working tree. You own exactly the paths your brief names. Do not create,
  edit or delete anything outside them. In particular never edit the root `Cargo.toml`, `Cargo.lock` by hand, other
  crates, or `docs/` (except a file your brief explicitly assigns to you).
- Dependencies: prefer what the root `[workspace.dependencies]` already declares (`dep = { workspace = true }`).
  If you need something else, add it to your own crate's `Cargo.toml` with an explicit version (not `workspace = true`),
  and list it under "deps added" in your report so the orchestrator can lift it to the workspace. Keep additions minimal.
- The `windows` crate feature list in your crate's `Cargo.toml` is yours to extend.
- Use `cargo <cmd> -p <your-crate>` only, never `--workspace`. Builds share one target directory and serialize on the
  Cargo lock; if a command waits for the lock, wait with it, do not kill it.
- If a cargo command fails with a manifest or parse error that names a crate you do not own, another worker is mid-edit:
  wait 30 seconds and retry, up to a few times. It is not your bug and not yours to fix.
- Never run `cargo clean` (shared target directory) or `cargo update` (shared lockfile).
- Do not run git commands that change state (no add, commit, stash, checkout, reset). The orchestrator commits.
  Read-only git (`git status`, `git diff`) is fine.

## Code standards

- Edition 2024, `rust-version` 1.90. Workspace lints: `unsafe_code = "deny"`; only `cc-hid` and `cc-nvapi` opt out
  crate-wide via `#![allow(unsafe_code)]`, and `cc-conflicts` opts out for exactly one module (`#[allow(unsafe_code)] mod sys;`).
  `clippy::undocumented_unsafe_blocks = "deny"` (every `unsafe` block carries a `// SAFETY:` comment that states the
  invariant), `clippy::unwrap_used = "warn"` (use `expect` with a reason, or propagate).
- Errors via `thiserror` enums in libraries. Logging via `tracing` (no `println!`, no `log`).
- `cargo fmt -p <crate>` clean; `cargo clippy -p <crate> --all-targets -- -D warnings` clean; `cargo test -p <crate>` green.
  `cargo nextest run -p <crate>` is also available.
- Public items documented with `///`; the crate root has a `//!` overview that states what the crate does and what it
  deliberately does not do.
- No network access anywhere (ADR 0001 #13). No telemetry. No `std::process::Command` unless your brief says so.

## Tests

- Tests must pass on a CI runner without any RGB hardware, without a GPU driver, and without admin rights.
  Therefore no test may open a real HID device, load or call `nvapi64.dll`, terminate a process, stop a service or
  write to the registry. Reading registry keys and enumerating devices/processes/services is allowed in `#[cfg(windows)]`
  smoke tests that assert only "returns Ok".
- Hardware-touching code paths get an `examples/` program the orchestrator runs by hand; examples are not run in CI.
- Pure functions (packet builders, parsers, matchers) get byte-exact unit tests.

## Report (your final message, structured)

1. Summary of what exists now (modules, public API in one screen).
2. Files created or changed.
3. Commands run and their results (fmt, clippy, tests, counts).
4. Deviations from `docs/architecture.md` and why.
5. Deps added (name, version, reason).
6. Open issues, risks, and anything the orchestrator must do by hand (hardware runs, secrets, decisions for the owner).

## Phase 2 additions

- Phase 1 crates (`cc-core`, `cc-hid`, `cc-nvapi`, `cc-conflicts`) are done and reviewed. Read their public API and
  rustdoc before using them; do not edit them. If one has a bug or a gap that blocks you, work around it locally and
  report it under "open issues" with the exact change you need.
- New since Phase 1 (orchestrator): `DetectContext::skip` / `is_skipped` in `cc-core`, the `cc-drivers` module skeleton
  (`src/lib.rs` with `all_backends()` and one placeholder `src/<family>/mod.rs` per family), and the shared profile
  types in `crates/cc-config/src/profile.rs` (`DeviceSetting`, `Profile`; their shape is fixed).

## Driver contract (applies to the four driver workers)

You own `crates/cc-drivers/src/<your_family>/**` and `crates/cc-drivers/examples/<your_family>_*.rs`. Do not edit
`crates/cc-drivers/src/lib.rs`. If you need a dependency, add exactly one line under `[dependencies]` of
`crates/cc-drivers/Cargo.toml` with the Edit tool (never rewrite the file; three other workers edit it too) and report it.

- `pub fn backend() -> Box<dyn Backend>` in `mod.rs`, plus `pub` packet builders and parsers that are pure functions.
- Detection: skip entirely when `!ctx.is_enabled(family)`. Compute each candidate's `DeviceId` from enumeration data
  only (vid, pid, serial; `DeviceId::hid`, ordinals for identical serial-less devices) and do not open or probe a
  candidate whose id `ctx.is_skipped`. `DeviceInfo.location` is the HID path (or PCI location for NvAPI); the engine
  matches hot-plug removals against it case-insensitively.
- `subscribe_hotplug` returns `None`. The engine owns a single `cc_hid::hotplug::watch` and re-runs `detect` with the
  open devices in `ctx.skip` when something arrives.
- The `Controller` is called from one engine thread only. No threads, no timers, no sleeps longer than the protocol
  requires (state the reference's delay when you keep one). The engine calls `set_direct` up to 30 times per second;
  make it cheap and send only what the reference sends for a frame.
- Before taking control, read whatever the device lets you read about its current hardware mode and keep it.
  `release()` restores that hardware mode (or the firmware default the reference uses when nothing can be read).
  `release()` is not terminal: a later `set_direct` or `set_mode` takes control again (re-send any init the reference
  does). `release()` twice in a row is harmless.
- Never write to flash/EEPROM except in `save_to_hardware()`. The engine never calls it unless the user asks.
- `DeviceDescription` must pass `validate()`. Provide a direct mode (`direct_mode`) whenever the device has one, plus the
  hardware modes the reference exposes. Names for zones and LEDs come from the reference.
- Tests: packet builders byte-exact against bytes you derive by reading the reference C++ (write the derivation in a
  comment next to each expected array), controller call sequences against `cc_hid::FakeTransport` scripts (init,
  set_direct, set_mode, release, release-then-set_direct). No test touches real hardware.
- Examples (run only by the orchestrator, never in CI):
  `examples/<family>_probe.rs`: detect, print the description and any state read from the device; read-only.
  `examples/<family>_demo.rs`: detect, set all LEDs red, wait 3 s, green, 3 s, blue, 3 s, then `release()` and print
  what was restored. It must also release on every error path. Hardware writes happen only here.

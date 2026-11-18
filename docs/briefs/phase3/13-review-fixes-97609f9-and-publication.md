# Brief: review fixes for 97609f9 (ADR #38), publication fixes, autostart path

Read `docs/briefs/phase2/00-common-rules.md`, ADR 0001 #3, #4, #24, #34, #38, #39, brief
`docs/briefs/phase3/12-msi-last-good-state-and-release-hardening.md`, then `crates/cc-core`, `crates/cc-config`,
`crates/cc-drivers/src/msi_mystic_light`, `crates/cc-engine`, `apps/desktop/src-tauri/src` (not `lib.rs` updater
wiring, `tauri.conf.json`, `Cargo.toml`, `deny.toml`, `.github/`: a release worker owns those in parallel). No new
dependencies. Do not launch, tauri-build or kill the app; nothing that writes to HID or NvAPI. If a check fails only in
files the release worker owns, re-run once later and otherwise report it.

## A. MSI own lighting: "CaseControl left it" marker (review 1, 3, 5)

Problem: any non-per-LED readback is stored as "last good own lighting", including states CaseControl wrote (Static 0
from brightness 0, a mode picked in the app) after a crash/sleep/re-enumeration; and with a stored state, release now
writes to a board CaseControl never took (another program may be driving it per-LED); and the UI shows the stored mode
while the board still shows a stale per-LED frame.

Fix with one persisted marker per device id in `hardware-state.toml`: "CaseControl has written to this device and not
yet released it" (set before the first write after a takeover, cleared after a successful release). At open:
- marker set (CaseControl left the board in its own state): do not record the readback as own lighting; if a stored
  good state exists, restore it immediately at open (so the board and the UI agree), then clear the marker.
- marker not set and readback per-LED: another program drives it; do not store, do not restore, do not write on
  release of an untouched controller (as before 97609f9).
- marker not set and readback not per-LED: record it as last good own lighting (as now).

## B. Other review findings (fix each, verify first)

1. Faulted release timeout race (`engine.rs` ~608): never let an old worker write after a new controller opened the
   device: if the faulted release is not acknowledged in time, skip re-opening that device in this pass (retry next
   pass) instead of opening alongside a still-running worker.
2. Off output skips zones with len 0 (`controller.rs` ~837): Static 0 must darken every header, including size-0
   ones (set their records to Static brightness 0 too).
3. `is_direct()` vs `has_direct_effect()` (`controller.rs` ~496): use the all-zones check consistently for "per-LED".
4. `record_own_lighting` updates memory before the write (`hardware_state.rs` ~84): update memory only after a
   successful write (or keep a dirty flag and retry the write on the next record).
5. Unreadable state file (`hardware_state.rs` ~56): an I/O error other than NotFound or non-UTF-8 content makes the
   state read-only for the session (never overwrite a file that could not be read), like settings.
6. `EngineHandle::hardware_state()` returns a clone with a file path (`engine.rs` ~1778): return a read-only snapshot
   type or the map, so nobody can write the file from a stale copy.
7. `from_hex` accepts '+' (`hardware_state.rs` ~161): accept only `[0-9a-fA-F]`.
8. `release_faulted` serial waits and wrong scope (`engine.rs` ~607): send all Stops first, then wait on all with one
   deadline; only for devices of the backends in this pass.
9. `FAULTED_RELEASE_TIMEOUT` into `Timing` (`engine.rs` ~39) so tests shorten it; add a test for item B1.
10. Drop the redundant `own_lighting` field (`controller.rs` ~346): derive it from `original` and `restore_pending`.
11. `engine_config()` placeholder `HardwareState::in_memory()` (`prefs.rs` ~198): pass the state in explicitly; iterate
    the TOML table directly in `parse()`.

## C. Autostart entry and console window (owner: "beim Start öffnet sich ein Terminal und bleibt offen")

Cause: the HKCU Run entry points at the debug exe (`target\debug`, console subsystem). The release build has no
console. Fix: on start, if autostart is enabled and this is a release build, rewrite the Run value to the current exe
path when it differs (so installing the release fixes the path). Debug builds never write the Run value (toggling
autostart in a debug build changes only the setting and logs that the entry is not written). Tests for the decision.

## D. Publication fixes (pre-publication audit)

- Add `LICENSE` (GPL-2.0 text, copy `reference/openrgb/core/LICENSE`) and a `NOTICE.md`: CaseControl copyright
  (Maximilian Kielholz), GPL-2.0-or-later; OpenRGB credit (Adam Honse and contributors, https://gitlab.com/CalcProgrammer1/OpenRGB,
  GPL-2.0-or-later) listing the ported files as modified ports; Lucide ISC notice for the pipette icon
  (`apps/desktop/src/components/ColorPicker.vue`). Add a one-line credit header to each ported source file that lacks
  one. README: short licence/credits section.
- Replace stale "private" wording: `README.md` line 3, `reference/openrgb/README.md`, `docs/architecture.md` ~339
  (ADR file is the orchestrator's; leave it).
- `reference/openrgb/NVFC/`: delete it (no licence header; upstream is public), and note in `reference/openrgb/README.md`
  that NVFC was removed.
- `apps/desktop/package.json`: `"license": "GPL-2.0-or-later"`.

## Checks

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace`,
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`, `pnpm typecheck`, `pnpm build`. Tests for A (each case),
B1, B2, B4, B5, B7, C. No commit. One final report.

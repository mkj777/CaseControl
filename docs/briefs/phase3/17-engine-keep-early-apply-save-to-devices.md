# Brief: engine keep-on-exit, early apply, save to devices (ADR 0001 #42, part 1 of 3)

Read `docs/briefs/phase2/00-common-rules.md`, ADR 0001 #3, #25, #33, #34, #36, #38, #40, #42, `docs/architecture.md`,
`docs/research/hardware-persistence-and-service.md` (sections 1, 2, 6), then the scout reports
`C:\Users\maxik\AppData\Local\Temp\claude\390fc6e9cd42ba40\ba8ab383-1ff6-47cc-843f-8afd412f1190\scratchpad\scout\engine.md`
and `...\scout\drivers.md` (file:line maps of exactly the code you change). You own `crates/cc-core`, `crates/cc-drivers`,
`crates/cc-engine`, `crates/cc-config` and the matching sections of `docs/architecture.md`. Another worker builds a new
crate `crates/cc-system` and changes `crates/cc-conflicts` at the same time; do not touch those or `apps/desktop`.
No new dependencies. Do not launch, tauri-build or kill the app; do not run driver examples or anything that writes to
HID or NvAPI. No commit. While the other worker is active, run checks per package (`-p cc-core -p cc-drivers -p cc-engine
-p cc-config`); if a check fails only in files you do not own, re-run once and report it.

## 1. Stop without hand-back (A)

- `EngineHandle::shutdown_with(mode, timeout)` with `ShutdownMode::{Release, Keep}`; `shutdown(timeout)` stays
  `Release`. `Keep` stops every worker without `release()` (`stop_slot(.., false, ..)`), but first flushes a pending or
  paced frame once so the device shows the latest output. `Guard::drop` keeps releasing.
- Bookkeeping stays honest: the unreleased marker stays set in `Keep` (devices still show our output), own lighting is
  untouched. Tests with fakes: no release call in `Keep`, last frame flushed, marker kept, budget honoured.

## 2. Each device gets its setting the moment it is found (B)

- `EngineConfig` gets the initial settings (the visible active profile's device settings) and the start brightness map;
  the engine seeds `pending` and the brightness levels from them before detection.
- Add devices per backend as each backend finishes (MSI first), not after all backends, so the MSI board is driven about
  1.5 s after start instead of 3.6 s. Keep detection order and fault handling as today.
- MSI `open_with_stored` (opened with the unreleased marker and a valid stored own lighting): do not write the stored
  state at open when a setting is pending for that device (it is about to be driven); still use it as `original` for
  release. Without a pending setting keep today's behaviour. This removes the flash of the old look at every start
  after a kept shutdown.
- Tests: pending applied on add, per-backend add order, no stored-state write when a setting is pending.

## 3. Save to devices (D, implements #34 for every capable driver)

Owner decisions (do not change): one device-agnostic setting; mixed colours of one zone are saved as their **average
in linear RGB**; brightness (device x master) is **baked into the colour**; hardware effects saved exactly with their
brightness level scaled; factor 0 saves the device's off state; never on live edits; the device's original saved state
is backed up before the first write and written back when the setting is turned off; a device whose original state
cannot be read completely is never written.

Driver API in `cc-core` (defaults = unsupported, no I/O in capability queries). Start from the proposal in
`scout\drivers.md` section 5 and keep it minimal: a capability query, `persist_look(look, last_fingerprint)` returning
written-with-fingerprint or unchanged, a backup accessor that returns the device's original saved state as opaque bytes
only when it is known to be complete, and `restore_saved_state(bytes)` that validates before sending. Add sRGB <-> linear
helpers and a linear average in `cc-core::color` with tests.

Engine API: persist all visible, capable, present devices from the effective output (`slot.output(levels).out`, hidden
devices skipped, hidden/zero-size zones left as stored), with a per-call time budget so it fits Windows' 5 s session-end
limit together with the stop; restore all backed-up devices; report per device (written, unchanged, skipped with
reason, failed). The engine refuses to persist a device without a stored backup. A daily cap per device (20 writes)
guards flash wear; log every write.

`cc-config` hardware state: new per-device records for the backup (written once before the first save, never
overwritten while it exists, removed only after a successful restore), the last saved fingerprint, the daily write
count, and pending restores (devices to restore when they are next connected). Unknown keys must survive a rewrite (an
older build must not drop a backup): keep them like `Settings.extra`. Writes stay atomic; a failed backup write blocks
the device write.

MSI (`msi_mystic_light`): follow `scout\drivers.md` section 1 exactly: base on `self.data`, never on a direct-mode
readback (the enable packet must never reach flash); when not driven require a fresh readback equal to `data`; build the
packet with `apply_mode`/`apply_zone_color` per header (average colour as Static at level 10, hardware modes exact, off
as Static brightness 0); enforce every gate from research section 1.4 (185 bytes, report 0x52, no effect 0 or 0x25 in
any record, LED counts unchanged and within max, diff limited to the supplied records plus the documented on-board
sync/mirror bytes and the save byte, firmware/class gates); send order: leave direct with save 0 and verify, then the
two-report save sequence, then verify by readback; afterwards re-enter direct and resend the last frame when the
device is driven. After a save, `release()` hands back the saved packet (not the pre-save original), and the own-lighting
record is updated to the saved state. Backup = the stored own lighting captured in a non-direct state before
CaseControl's first write of the session; restore validation adds effect 1..=39 (not 0x25), brightness <= 10, counts
<= max.

G560 (`logitech`): persist direct looks as Static per zone with persistence (brightness baked), effects as they are,
off as persisted Off/Static black. The persisted state cannot be read directly: the backup is the per-zone open-time
readback taken before any write of the session, complete only when every zone mapped; otherwise the device reports
"original state unknown" and is not written. Look at why zone 1 reads 16 zero bytes on the owner's device (log
2026-11-17) and fix it only if the cause is clear from code and protocol docs.

NVIDIA, Wooting, Lightspeed: unsupported (no change).

Tests with fakes/mocks for every gate (packet diff limits, refused effects, no 0x25 to flash, verify mismatch aborts,
unchanged fingerprint skips, cap reached, missing backup refuses, restore validates and drops backup only on success,
pending restore kept for absent devices, release after save hands back the saved packet, G560 incomplete backup
refuses).

## Checks and report

`cargo fmt --all --check`, `cargo clippy` and `cargo nextest run` for your packages, `RUSTDOCFLAGS="-D warnings" cargo
doc` for your packages; at the end once for the workspace. Update `docs/architecture.md` (engine shutdown modes, early
apply, save to devices, hardware-state schema). One final report: public API added (signatures), files changed,
behaviour, check results, and every decision this brief did not cover.

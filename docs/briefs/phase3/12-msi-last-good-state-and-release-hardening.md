# Brief: MSI remembers its last good own lighting; release hardening (ADR 0001 #38)

Read `docs/briefs/phase2/00-common-rules.md`, ADR 0001 #3, #10, #24, #33-#38, `docs/architecture.md`,
`docs/research/hardware-persistence-and-service.md` (MSI), the analysis in brief
`docs/briefs/phase3/11-nvidia-white-msi-perled.md` part B, then `crates/cc-core`, `crates/cc-drivers/src/msi_mystic_light`,
`crates/cc-engine`, `crates/cc-config`, `apps/desktop/src-tauri`. You own those. No new dependencies. Do not launch,
tauri-build or kill the app; do not run driver examples or anything that writes to HID or NvAPI.

## Background

The owner's board (MS-7D75) was found in per-LED mode at start (run 8) after a clean quit of run 7 with no fault,
removal or release error in the log. Since then every start captures per-LED as the board's own lighting and release
restores static colours; the real effect (Rainbow wave) is only in flash. Owner decision: option b.

## Do

1. **Last good own lighting (owner choice b).** When a Mystic Light board is opened in a non-direct state, persist its
   185-byte (or 162-byte class) config readback per device id as the "last good own lighting". When it is opened in
   per-LED mode and a stored state exists for that id (and the packet class/length matches), use the stored state as
   `original` for release and for the hardware readback (`hardware_mode`), and log that at info. Without a stored
   state keep today's behaviour (warning).
   - Storage: a small machine-state file `%APPDATA%\CaseControl\hardware-state.toml` (not `config.toml`), owned by
     cc-config, hex-encoded blobs keyed by device id, written only when the value changes, atomic write like the
     settings file, unknown/garbled content ignored with a warning. Generic shape (driver-opaque bytes per device id) so
     other drivers can use it later.
   - API: the smallest honest seam. E.g. the backend/driver gets the stored blob at open through `DetectContext` and
     reports a new blob to persist through a `Controller` method or the description; the engine or app writes the
     file. Decide, keep it minimal, document it in `docs/architecture.md`.
   - Validate the stored packet before ever sending it (length, report id, class, no direct-mode flag); never send
     anything that does not pass. Never set the save-to-flash flag (ADR #34 stays unbuilt).
2. **Release hardening:**
   - Release on stop also for a faulted device, best effort with the existing short timeouts (`worker.rs` ~494).
   - On re-detect of a faulted or removed device, release the old controller (best effort) before opening the new one,
     or carry the old controller's original over; pick the simpler safe one.
   - A freshly opened controller that never wrote (`touched == false`) but found the board per-LED with a stored good
     state: release restores the stored state.
3. **MSI brightness 0 without per-LED:** for the off output on Mystic Light use Static at brightness 0 (a hardware
   mode) instead of direct black, so brightness 0 does not switch the board to per-LED. Keep other devices as they
   are. Moving back up from 0 returns to the previous output without a visible flash where possible.
4. **Investigate (report, fix only if clear):** in run 7 the log shows `connect all taken=4` three times within four
   seconds (12:33:47, :49, :51), each preceded by a Logitech `open-time effect readback not mappable` line, i.e. the
   same devices were taken again each click. Find why taken devices did not stay driven (re-open of a device on
   takeover? hidden PRO Wireless? status not updating?). Logs are in the brief 11 report's run; reproduce with fakes.

## Tests

Fakes for: stored state used when opened per-LED, stored state written on non-direct open and not rewritten when
unchanged, invalid stored blobs rejected, release on faulted stop, re-detect releases old controller, MSI brightness 0
uses Static 0 and no per-LED packet, state file round trip and garbage tolerance.

## Checks

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace`,
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`, `pnpm typecheck`, `pnpm build`. No commit. One final
report: design of the seam, file format, behaviour changes, item 4 findings, check results.

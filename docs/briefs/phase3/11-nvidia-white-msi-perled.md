# Brief: RTX 5080 FE is white only; why the MSI board stays in per-LED mode after quit

Read `docs/briefs/phase2/00-common-rules.md`, ADR 0001 #24-#26, #33-#37, `docs/testing-hardware.md`,
`docs/research/hardware-persistence-and-service.md` (MSI sections), then `crates/cc-core`, `crates/cc-drivers`
(`nvidia_illumination`, `msi_mystic_light`), `crates/cc-engine`. You own `crates/cc-core`, `crates/cc-drivers`,
`crates/cc-engine`, `apps/desktop/src/pages/DevicePage.vue`, `apps/desktop/src/components/DevicePreview.vue` and the
matching sections of `docs/architecture.md`. Other workers edit `apps/desktop` (other files) and the Conflicts page at
the same time; do not touch their files. No new dependencies. Do not launch, tauri-build or kill the app; do not run
driver examples or anything that writes to HID or NvAPI. If a check fails only in files you do not own, re-run once
later and otherwise report it.

## A. NVIDIA white only (build it)

Owner: the RTX 5080 FE light can only show white; colour does not change, only brightness. NvAPI reports its zones as
`SINGLE COLOR` (probe: 3 zones, brightness 50/60/60).

- Model single-colour zones honestly in the core description (e.g. a zone/LED capability "brightness only, fixed
  colour"), set by the NVIDIA driver for `SingleColor` (and `ColorFixed`) zones. Pick the smallest model change.
- Engine: a colour written to such LEDs is reduced to an intensity (use the max channel, so white 255 = full and the
  fallback colour white keeps full intensity); device brightness and master brightness scale it as for any device.
  Takeover with the fallback colour on such a device = full intensity, i.e. only brightness matters.
- Device view: for a device whose LEDs are all brightness-only, hide the colour picker, swatches and LED selection
  tools; show the brightness slider (and modes if any) only; the preview shows white scaled by the output intensity.
  Mixed devices: colour controls apply only to colour LEDs (no such device today; just do not break).
- Tests: description from `SingleColor` zones, intensity mapping, takeover, view flags.

## B. MSI per-LED after quit (analyse, do not change behaviour yet)

Facts: through app run 7 (all-off removal build, commit 72bd865) every start read the board in its own effect
(Rainbow wave). Run 7 was quit with `--quit` (log: "engine shut down devices=5 elapsed=73 ms"). The next start
(run 8) logged `board was left in per-LED mode by another program; release() will restore that state`, and since then
each start captures per-LED as the board's own lighting and release restores static colours. The owner does not know
what was set during run 7 (maybe brightness 0, a hidden zone, Connect all). Find every code path in driver and engine
that can leave the board in per-LED mode after release or shutdown (e.g. release ordering vs a late frame, the
removed all-off/fade path, brightness-0 Off fallback to direct black, reassert, hidden-zone masking, a re-take after
release during shutdown, the captured `original` being overwritten), with the evidence in code. Then propose fixes,
including how to recover the board's own effect when the captured state is per-LED (what the 185-byte config packet
still holds in that case, per the OpenRGB reference in `reference/openrgb/` and our research doc). Do not implement
the recovery; a fix for a clear bug in ordering may be implemented with a test if it is unambiguous.

## Checks

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace`,
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`, `pnpm typecheck`, `pnpm build`. No commit. One final
report: A changes and strings, B findings with file:line evidence and fix options.

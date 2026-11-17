# Brief: remove All off (ADR 0001 #35, owner 2026-11-17)

Read `docs/briefs/phase2/00-common-rules.md`, `docs/design-system.md`, ADR 0001 #33-#35, then `crates/cc-engine`,
`crates/cc-config`, `apps/desktop`. You own them plus `docs/architecture.md` and `docs/design-system.md` (not the ADR
file). No new dependencies. Do not launch, tauri-build or kill the app; nothing that writes to HID.

## Decision

Owner: "All off sollte equivalent mit Brightness 0 sein, es kann weg." All off is removed completely. Brightness 0
(device or All brightness) is the way to turn lights off.

## Do

- Engine: remove the all-off state, all-off/on commands, the fade machinery that exists only for it (`cc-fade`
  thread etc.), "hold newly detected devices dark while all off", and all-off paths in takeover rules. If the fade is
  also used by something else, keep only that use and say so in the report. Brightness 0 must still take over a
  released, non-hidden device (brief 05 rule) and output black; a device at 0 stays Active.
- Config: drop `Settings::all_off`. Old files that contain `all_off = ...` must still load (ignored, not an error) and
  the key disappears on the next save. Test with a file containing `all_off = true`.
- Desktop: remove the all-off commands, event fields, start-up handling, and every UI trace (device view "All off"
  state, "Turn on" button, any all-off notice or wording, tray entries if any). Connect all just connects.
- Docs: remove All off from `docs/architecture.md` and `docs/design-system.md`; update the outdated
  `device-state-changed` payload in `docs/architecture.md` to the current shape while you are there.
- Tests: delete the all-off tests, keep/adjust the rest; add the config compatibility test above.

## Checks

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace`,
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`, `pnpm typecheck`, `pnpm build`. No commit. One final
report: what was removed, whether any fade use remains, UI strings removed, check results.

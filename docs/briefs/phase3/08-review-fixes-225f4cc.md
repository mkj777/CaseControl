# Brief: fixes from the code review of 225f4cc (ADR #36)

Read `docs/briefs/phase2/00-common-rules.md`, `docs/design-system.md`, ADR 0001 #26 and #33-#36, brief
`docs/briefs/phase3/07-default-colour-toasts-conflict-list.md`, then `crates/cc-engine`, `apps/desktop`. You own them
plus `crates/cc-config` and the two docs. No new dependencies. Do not launch, tauri-build or kill the app; nothing that
writes to HID. Verify each finding against the code first; if one is wrong, say so and skip it.

## Fix

1. **Hidden zones leak on takeover** (`engine.rs` `Slot::take_over` ~164, `queue_setting`): dark zones are only
   applied in `default_want`. Apply them in the engine to every Direct output from takeover (stored setting) and to
   queued/pending Direct settings, so a hidden zone is black whatever path drives the device. Keep one masking
   implementation: move `hidden::mask_hidden_zones` into the engine (or cc-core) and use it from the app too.
2. **Race in `set_zone_hidden`** (`commands.rs` ~472): update the engine's dark zones before saving/deciding, and let
   the engine re-mask any driven device itself (not based on a view read before the change).
3. **`default_want` 0..=0 colour range** (`lighting.rs` ~195): require `range.end() >= 1`. Remove the dead
   `direct_mode` filter and dead inverted-range min/max handling (~189).
4. **Frontend duplicates the takeover choice** (`lib/device.ts` `defaultColorMode` ~118): have the engine report in
   `DeviceView` whether the device can be taken over and which mode a takeover would use (e.g.
   `takeover: { mode_id } | null` or similar); remove the TS copy and use that for `brightnessMode`, `canTakeOver`,
   `canConnect`.
5. **Brightness slider on a device that cannot be taken over** (`DevicePage.vue` ~396): disabled when the engine says
   it cannot be taken over, with the short reason the slider had before ("Set a colour or mode first.").
6. **All brightness during Release all** (`DevicesPage.vue` ~157): disable it (like Connect all) while the release runs.
7. **Default colour drag writes config every frame** (`SettingsPage.vue` ~72): live value without persisting while
   dragging, persist on release (same pattern as the brightness sliders' `persist` flag).
8. **Stale settings responses** (`SettingsPage.vue` ~72): the colour sender must not overwrite other fields with an
   older response; apply only the field it sent, or order responses so an older one never wins.
9. **`onDefaultColor` clears every error** (`SettingsPage.vue` ~86): clear only its own error.
10. **Duplicate fallback construction** (`engine.rs` ~244): one `State::fallback()` helper; carry the colour buffer
    inside the Direct variant instead of `(Want, Option<Vec<Rgb>>)`.
11. **Two default colours**: `EngineConfig::default` uses `cc_config::DEFAULT_COLOR` if the dependency direction
    allows, else one shared constant in cc-core used by both.
12. **ListRow wrapping changed Profiles** (`ListRow.vue` ~59): restore `flex: none` as the default; make wrapping
    opt-in via a prop used only by Conflicts.
13. **Double details wrapper** (`ConflictsPage.vue` ~208): drop the page's own `div.details`, put the `aria-controls`
    id on ListRow's wrapper (prop or attribute pass-through).

Do not change UI copy (the "Lighting" heading and the Default colour hint are with the owner).

## Tests and checks

Tests for 1-4 at least (hidden zone black after takeover of a stored Direct setting and after a queued setting, the
0..=0 mode skipped, DeviceView takeover info). `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --
-D warnings`, `cargo nextest run --workspace`, `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`,
`pnpm typecheck`, `pnpm build`. No commit. One final report: per finding fixed/skipped and why.

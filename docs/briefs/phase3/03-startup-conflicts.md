# Brief: conflicts at startup and autostart control (ADR 0001 #31)

Read `docs/briefs/phase2/00-common-rules.md`, ADR 0001 #12 and #26-#31, `docs/architecture.md` (conflicts section),
the existing `crates/cc-conflicts` and the desktop commands for conflicts. Part A now; part B only after the
orchestrator says the design-system migration (brief 02) is merged, because the UI must use its primitives.

## Part A: backend (you own `crates/cc-conflicts/**` and `apps/desktop/src-tauri/**`; not `apps/desktop/src`)

- Catalogue: every entry gets a class `suite` (full RGB suites: OpenRGB, SignalRGB, iCUE, Armoury Crate, Aura,
  Polychromatic-like, ...) or `vendor_tool` (G HUB, Wootility, MSI Center / Dragon Center, LampArray service, ...).
  Keep the existing device-family mapping; a finding "affects" our devices only through it.
- Autostart detection (read-only scan): Run / RunOnce keys in HKCU, HKLM and HKLM WOW6432Node, the
  `Explorer\StartupApproved\{Run,Run32,StartupFolder}` state (enabled/disabled), the per-user and common Startup
  folders, and service start type (already read). Match entries to catalogue entries by executable path/name globs.
  Real examples on the owner's machine: `HKCU\...\Run\SignalRgb = "...\VortxEngine\SignalRgbLauncher.exe" --silent`,
  `HKCU\...\Run\LGHUB`, services `OpenRGB` (Auto, runs OpenRGB.exe) and `SignalRgb.Service` (Auto).
  Findings carry their autostart items with an enabled flag.
- Startup check: a function that, given a scan report, the detected non-hidden devices and the "don't ask again" set,
  returns the dialog model: suites that affect at least one visible device and are running or have autostart
  enabled, each with its close targets (processes, services) and autostart items. Pure and unit-tested.
- Actions (only ever triggered by a user click):
  - Turn off autostart: app entries by writing the StartupApproved value to "disabled" (byte 0 = 0x03, as Task
    Manager does; never delete the Run value); services: start type Manual. Turn on (Re-enable): StartupApproved
    "enabled" (0x02), service back to Auto (remember the previous start type in settings so Re-enable restores
    exactly it).
  - Close now: terminate processes (existing checked terminate), stop services.
  - One Proceed = at most one UAC prompt: everything needing admin (service stop/config, HKLM StartupApproved) runs in
    one elevated invocation of CaseControl's own executable in a helper mode (e.g. `--elevated-actions <file>`),
    handled in `main` before Tauri/single-instance start. The elevated helper re-reads the request, accepts only
    action kinds from a fixed enum on names/paths that a fresh scan matches to a catalogue entry, and never runs a
    shell. HKCU-only and same-user actions run in-process without UAC. Report per item success/failure back to the app
    (e.g. a result file the app waits for with a timeout).
  - Never terminate or change anything not in the dialog model / catalogue; no automatic proceed anywhere.
- Settings: `conflicts.dont_ask: BTreeSet<String>` (catalogue ids), remembered previous service start types.
- Commands: `startup_conflicts()` (dialog model), `resolve_conflicts(request)` (close + autostart per item, returns
  per-item results), `set_autostart(entry, enabled)` for the Conflicts page, `set_dont_ask(id, bool)`. Events:
  `conflicts-updated` after any change. On start (also with `--hidden`): if the dialog model is non-empty, show the
  window and emit `startup-conflicts` with the model. Run the scan off the main thread; do not delay device control.
- Tests for matching, StartupApproved parsing/encoding, dialog model, request validation in helper mode (fakes for the
  registry/services). Do not perform any real termination, service change or registry write in tests or by hand.

Report the TS shapes of every new command/event payload for part B.

## Part B: UI (after the design-system migration; you then own the conflict UI files in `apps/desktop/src`)

- Start dialog (design-system Dialog): title and copy saying CaseControl will close these programs; per program its
  name, why (which of our devices), "Close now" and "Turn off autostart" (preselected), "Don't ask again"; buttons
  Proceed (primary) and Not now. Note that a Windows permission prompt may appear. After Proceed show per-item results.
- Persistent hint (warning Notice in the shell) while a dialog-eligible program still has autostart on and is not in
  "don't ask again", with a button that opens the dialog.
- Conflicts page: autostart state per finding with Turn off / Re-enable, "Don't ask again" toggle, vendor tools listed
  as before.
- List every UI string you add.

## Checks

`cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace`,
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`, and for part B `pnpm typecheck`, `pnpm build`. Do not
launch the app or `tauri dev/build`.

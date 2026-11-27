# Brief: fixes from /code-review max and the security review of ADR 0001 #42

Read `docs/briefs/phase2/00-common-rules.md`, ADR 0001 #32, #34, #38, #42, briefs 17-19 in this folder,
`docs/architecture.md`, `docs/research/hardware-persistence-and-service.md` §1.2-1.4. The uncommitted working tree is the
ADR #42 change; `git diff HEAD` shows it. Every finding below was verified against the code (file:line refer to the tree
at review time). Fix the cause, add a regression test for each (fakes, pure functions, temp dirs), and keep all existing
tests green. Owner decision already taken: the per-user install risk of #32 (H0) is accepted and documented in ADR #42;
do not change the install mode.

Three workers: part E and part S run in parallel, part D runs after both. Ownership:
- **E**: `crates/cc-core`, `crates/cc-drivers`, `crates/cc-engine`, `crates/cc-config`, root `Cargo.toml` (only to drop
  the unused `tauri-plugin-autostart` workspace dependency).
- **S**: `crates/cc-system`, `crates/cc-conflicts`.
- **D**: `apps/desktop` (Rust, NSIS hook, Vue), plus `docs/architecture.md`, `docs/testing-hardware.md`.
Same hard rules as before: no new third-party dependencies (enabling `windows` features is fine), do not launch,
tauri-build or kill the app, no driver examples, nothing that writes to HID, NvAPI, the registry or the Task Scheduler,
never read `%USERPROFILE%\.tauri`, no commit. Run checks per package while another worker is active; at the end of part D
the full workspace checks plus `pnpm typecheck` and `pnpm build`.

## Part E: engine, drivers, config

E1. **MSI persist must never build on per-LED leftovers** (controller.rs:840, 517-552, 634-637; protocol.rs:385-388,
449-450, 510-535). `persist_packet` starts from `self.data`, which can be a raw direct-mode readback (board left
per-LED by another program, opened unmarked) or an unvalidated stored blob; the 0x25 gate checks only the finished packet
and the JRGB/JPIPE mirror can overwrite the only 0x25 record. Fix: the base for any flash write is a trusted packet only:
the validated backup or a fresh non-direct readback of this session; refuse persist when `original`/`data` came from a
direct-mode readback or `left_by_us` and no validated base exists. Accept stored own lighting only if it passes
`validate_backup`. In `check_persist_packet` run the per-record checks (effect 1..=39, not 0x25; brightness <= 10;
documented flag bits) on the whole `next` packet, and require every byte outside the change mask to equal the trusted
base. Zones passed as `None` keep the base bytes, not this session's `set_mode` records (controller.rs:862).

E2. **LED counts are never written from configured zone sizes** (controller.rs:946, 1141-1142; protocol.rs:541-590):
take every header's count byte from the trusted base (research §1.4: unchanged). Test with a board whose stored count
differs from the configured size and with a `DirectSupport::Disabled` board.

E3. **Manual "Save to device" goes through the same gates** (msi controller.rs:796-800, 1146-1153; worker.rs:538-545):
route `Command::Save` through the persist machinery: backup first, gates, verify, daily cap, fingerprint updated;
refuse when the device is not driven by a hardware mode (never send a 0x25 readback with save 1). G560 the same way.
No UI change.

E4. **G560 and NVIDIA remember their own lighting like MSI** (g560.rs:579, engine.rs:715): while opened with the
unreleased marker they report `shows_own_lighting() == false`, keep the marker until a release, and use the stored own
lighting (recorded only from unmarked opens) for release and for the G560 backup; without one, release replays what
they read and the G560 reports "original state unknown". Generalise the ADR #38 seam rather than copying MSI code.
G560 release after a save replays every zone, so a hidden zone does not stay black (g560.rs:750).

E5. **Durable, separate, machine-bound backups** (hardware_state.rs:197-225, 320-328, 388-394, 540-546; paths.rs:31-58):
- Move machine state from Roaming to `%LOCALAPPDATA%\CaseControl` (migrate the old file once, leave the old one so a
  downgrade still finds its markers) and store the device backups, fingerprints, write counts and pending restores in a
  separate file (e.g. `device-memory.toml`) that older builds never touch.
- Bind both files to the machine (store `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid`, read only; records from
  another machine are ignored and logged, never written to a device).
- `record_saved_backup`, `record_saved_write` and pending-restore writes fail unless the file was written durably; a
  read-only or in-memory state refuses persist and restore.
- A file that cannot be parsed is never overwritten: keep it (rename aside with a timestamp, log), start empty and
  read-only for device-memory writes until a fresh file exists; unparsable entries of known tables are kept verbatim.

E6. **Restore safety and retries** (engine.rs:868, 878-902, 1758; worker.rs:611-702; msi controller.rs:1240-1255):
- `EngineConfig` flag that forbids every device-memory write (persist, restore, manual save, pending restores); the
  boot mode and the uninstall clean-up's helper-less paths set it as part D decides (boot: always off).
- `restore_saved` marks every backed-up device pending before running the jobs (success clears it).
- MSI restore: build from a fresh non-direct readback with only the backed-up records replaced, gated by
  `check_save_diff` with LED counts from the readback; skip the write when the readback already equals the backup.
- Restores are capped too (e.g. 3 attempts per device per day); after the cap the pending flag stays but nothing is
  written until the next day, and the report says so.
- Clear the stored fingerprint whenever a write may have reached the device (failed persist, every restore,
  manual save) (worker.rs:649, 680, 693).
- Gate refusals before any write do not count toward the cap (worker.rs:647).
- Refresh `slot.hardware` after persist and restore.

E7. **Deadlines travel with the command** (engine.rs:1636): `Command::Persist`/`Restore` carry a deadline; a worker
that dequeues one late skips it (`TimeBudget`); a reply timeout is reported as "may still complete", not as not written.

E8. **Small engine/driver items**: MSI `write_flash` error re-enters direct mode when the device was driven
(controller.rs:956); an MSI board opened with `setting_pending` whose pending setting fails falls back to restoring the
stored own lighting (engine.rs:869).

E9. **Settings are real fields** (cc-config `Settings`): `keep_colors_on_sign_out` (default true), `save_to_devices`,
`apply_before_sign_in` (default false), with migration from the `extra` keys part D wrote earlier; fix the doc comment
of `Settings::autostart` (no plugin any more); drop `tauri-plugin-autostart` from the root `Cargo.toml`.

## Part S: cc-system, cc-conflicts

S1. **Principal read-back by SID** (sys.rs:647): `IPrincipal::UserId` returns a bare name; read the principal and the
logon trigger's user SID from `IRegisteredTask::Xml` (or compare via `LookupAccountSidW` of the expected SID), so
domain and Entra users are not refused and ownership is judged by SID. Report `Enabled` in `TaskInfo`; a disabled task
is its own state (`TaskState::Disabled`), not `Matches`.

S2. **No delete-then-register gap** (task.rs:618): replacing our own task uses `TASK_CREATE_OR_UPDATE` after the
ownership check, then verify; on verify failure delete.

S3. **Exe path**: refuse paths containing `%` (Task Scheduler expands it, task.rs:273); `check_exe_path` requires
`GetDriveType == DRIVE_FIXED` (acl.rs:166-230).

S4. **Session end** (session_end.rs:95): a stale `WM_QUERYENDSESSION` (no `WM_ENDSESSION(TRUE)` followed, e.g. another
app cancelled) must not classify a later normal quit as a session end: decide "ending" only from `WM_ENDSESSION(TRUE)`
or `SM_SHUTTINGDOWN`; use QUERY flags only for the kind; expire them after a short time.

S5. **Device lock** (sys.rs:396-417): check owner and DACL of an opened existing mutex (owner must be the user SID or
SYSTEM, else `HeldBySomeoneElse`, and the app proceeds without waiting); add a per-user named event the signed-in
instance sets to ask the start-up instance to finish now (`request_yield` / `yield_requested`), same naming and DACL
rules.

S6. **Elevated helper hardening**:
- the helper's scan resolves the common Startup folder with `SHGetKnownFolderPath(FOLDERID_CommonStartup)`, never from
  environment variables (platform.rs:131, 201-226, 298-312);
- after creating the result file, check with `GetFinalPathNameByHandleW` that it lies in the validated directory, else
  delete it through the handle and fail (elevation.rs:179-188);
- a refusal because the helper's token user differs from the session user (other admin credentials, Windows
  "Administrator protection") or because the request file is owned by Administrators returns a clear, specific error
  text the app can show (boot_task.rs:94, 104);
- foreign task under our name (`NotOurs`) returns a clear error text.

## Part D: desktop app (after E and S; read their final reports, given to you)

D1. **Uninstall clean-up quit path** (cleanup.rs:118, 110): the clean-up must stop the running instance without
installing a pending update and without saving to devices: use a dedicated single-instance argument handled by the
callback (release, no update install, no persist), and delete nothing else of the updater. No double restore of a
pending device (cleanup.rs:187); include devices of disabled families that have backups (cleanup.rs:175).

D2. **Upgrade is not uninstall** (hooks.nsh:11): skip the clean-up when the uninstaller runs for a reinstall (the
template passes `_?=$INSTDIR`; verify against the template in `C:\Users\maxik\.cargo\registry\src\...\tauri-bundler-*\src\bundle\windows\nsis\installer.nsi`)
and do not show UAC in silent (`/S`) uninstalls. At start, when the start-up task can be read, align
`apply_before_sign_in` with reality (missing or not ours: setting off, logged); after an `unknown` helper outcome or a
failed settings save, re-read the task and set the flag from it (commands.rs:822).

D3. **Lock before state** (state.rs:102, 115): take the device lock (signal the start-up instance to yield, S5) before
loading machine state; reload after a wait. With the yield event the signed-in instance may wait up to ~10 s.

D4. **Unreadable settings never delete autostart** (lib.rs:294): skip `sync_at_start` when `settings_error()` is Some;
serialise the start-up autostart sync with the autostart toggle (no race with `set_settings`). A disabled logon task
(S1) is treated like the Task-Manager-disabled Run entry: autostart off.

D5. **Exit path never blocks long** (state.rs:439, 498-506): at session end wait for `persist_lock` at most ~1 s (a
running switch-off restore has already marked its devices pending, E6); at Quit wait for it bounded by its own budget.

D6. **Boot mode** (`boot.rs`, `main.rs`): device-memory writes off (E6); fail closed when an argv contains
`--apply-at-boot` or `--uninstall-cleanup` but is not the exact form (exit with an error code, no GUI, like
`--elevated-actions`); refuse to start the GUI in session 0; log `TokenElevationType` and integrity level in the boot log;
bounded reads (1 MiB) of config, profiles and state; do not wait for devices of disabled families (boot.rs:151); stop
promptly when the signed-in instance asks to yield (S5).

D7. **Settings page**: each switch is busy only while its own command runs (SettingsPage.vue:67); the note includes
faulted devices (power.rs:175); errors from S6 are shown as given; use the real settings fields (E9).

## Report

One final report per part: what changed per finding id (E1..E9, S1..S6, D1..D7), tests added, check results, and any
decision this brief did not cover. Part D also updates `docs/architecture.md` and the hardware checklist in
`docs/testing-hardware.md` (add: upgrade with the old setup keeps the start-up task; uninstall with a pending update).

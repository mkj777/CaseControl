# Brief: Windows tasks, session-end detection, elevated start-up task (ADR 0001 #42, part 2 of 3)

Read `docs/briefs/phase2/00-common-rules.md`, ADR 0001 #3, #31, #32, #42, `docs/architecture.md` (security, conflicts,
elevated helper), `docs/research/hardware-persistence-and-service.md` section 7, then the scout reports
`C:\Users\maxik\AppData\Local\Temp\claude\390fc6e9cd42ba40\ba8ab383-1ff6-47cc-843f-8afd412f1190\scratchpad\scout\elevation.md`,
`...\scout\windows-research.md` and `...\scout\desktop.md` (sections 1-3). You own a new crate `crates/cc-system`,
`crates/cc-conflicts`, the workspace `Cargo.toml` (members, `windows` features) and `Cargo.lock`. Another worker changes
`cc-core`, `cc-drivers`, `cc-engine`, `cc-config` at the same time; do not touch those or `apps/desktop` (a third worker
wires your APIs into the app afterwards). No new third-party dependencies (enabling `windows` crate features is fine).
Do not launch, tauri-build or kill the app; do not create, change or delete real scheduled tasks, registry values or
windows; tests use fakes and pure functions. No commit. Run checks per package while the other worker is active.

## 1. Crate `cc-system` (Windows system glue, the only new home for this `unsafe`)

Same lint policy as `cc-conflicts`: `unsafe` only in one `sys` module with RAII handles and SAFETY comments; pure,
testable logic outside it; non-Windows stubs so the workspace still builds and tests elsewhere.

- **Task Scheduler** through COM `ITaskService` / `ITaskFolder::RegisterTask` with XML (no `schtasks.exe`, no shell).
  Pure XML builders with unit tests, escaping every interpolated value:
  - Logon task (B, created by the user, no admin): LogonTrigger for the user's SID, no delay; principal = that SID,
    InteractiveToken, LeastPrivilege; Priority 4; DisallowStartIfOnBatteries and StopIfGoingOnBatteries false;
    ExecutionTimeLimit PT0S; MultipleInstancesPolicy IgnoreNew; action = the exe with `--hidden`, working directory the
    exe's folder.
  - Start-up task (C, created only by the elevated helper): BootTrigger; principal = the user's SID, logon type S4U,
    RunLevel LeastPrivilege (LUA); ExecutionTimeLimit PT2M; battery settings false; Priority 4; IgnoreNew; action = the
    exe with exactly `--apply-at-boot`.
  - Fixed task names derived from the user's SID so two users never collide and removal can never hit another user's
    task (pick names and folder; the start-up task must not live in a folder the user created or controls).
  - Operations: register, delete, exists, read back (principal, logon type, run level, trigger kind, action path and
    arguments) and verify against what was intended; delete and fail on any difference. Only delete a task after reading
    it back and confirming it is ours (name, principal, our exe path pattern).
- **Session end**: a safe API the app installs on its main thread given a top-level `HWND` (as `isize`): subclass the
  window (`SetWindowSubclass`), record the `lParam` flags of `WM_QUERYENDSESSION` / `WM_ENDSESSION` in atomics, always
  pass on to `DefSubclassProc`. Also subclass tao's thread event-target window (class "Tao Thread Event Target", found
  with `EnumThreadWindows` on the calling thread) so the flags are seen before tao calls `process::exit`. A query
  function returns `SessionEnd::{NotEnding, SignOut, ShutdownOrRestart, CloseApp (Restart Manager), Unknown}`, combining
  the flags with `GetSystemMetrics(SM_SHUTTINGDOWN)` (ending but no flags seen = `Unknown`). Unit-test the pure decoding.
- **Cross-session lock**: a named `Global\` mutex per user SID with an explicit DACL for that SID (and SYSTEM), used so
  the start-up instance (session 0) and the signed-in instance never drive devices at the same time: `try_acquire`,
  `acquire_with_timeout`, and "held by someone else" when opening fails with access denied.
- **User identity**: current user SID (string), the interactive user of the current session
  (`ProcessIdToSessionId` + `WTSQuerySessionInformation` + `LookupAccountName`), and the profile path from HKLM
  `ProfileList\<SID>\ProfileImagePath` (read only).
- **Path and ACL checks** for the start-up task's exe: absolute, regular file, no reparse point in any ancestor, and the
  DACL of the exe and every ancestor grants write/delete/WRITE_DAC/WRITE_OWNER only to the principal SID, SYSTEM,
  Administrators or TrustedInstaller, with the owner among them (reason in `scout\elevation.md` section 2). Pure
  evaluation of a parsed DACL is unit-tested.

## 2. Elevated helper actions for the start-up task (C)

In `cc-conflicts` (follow the existing protocol exactly; read `scout\elevation.md` sections 1-2):

- Two unit variants `register_boot_task` / `remove_boot_task` (no fields; add `deny_unknown_fields` so any extra field
  such as an exe or user is refused), `needs_admin` true, a reserved non-catalogue entry id accepted only for these two
  actions, phase and `helper_timeout` cases, `SystemOps`/`FakeOps`/`WindowsOps`/non-Windows stub.
- The helper derives every value itself, nothing from the request: exe = its own `current_exe()` passing the path/ACL
  check; arguments fixed; principal = the interactive user of the helper's own session; refuse when the helper's token
  user differs from that user (over-the-shoulder elevation with other admin credentials is not supported) or when the
  request file's owner or `ProfileList` disagree; never SYSTEM, a service account, HIGHEST, a password or
  InteractiveToken for this task. After registering, read back and verify; on mismatch delete and fail. Remove deletes
  only our task for that SID.
- Close two gaps the scout found: the helper compares the request's `session_id` with its own session and refuses a
  mismatch; reduce the result-file TOCTOU (write the result relative to an already-opened, validated directory handle,
  or re-validate the directory right before writing) if it can be done simply; otherwise report it.
- Tests in the existing style: wire round trip, extra fields refused, wrong entry id refused, principal derivation
  (fake identity provider) including the mismatch cases, XML assertions (S4U, LUA, BootTrigger, never SYSTEM/HIGHEST,
  fixed args), verify-after-register mismatch deletes, remove refuses a task that is not ours.

## Checks and report

`cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo nextest run` and `cargo doc` with
`RUSTDOCFLAGS="-D warnings"` for `cc-system` and `cc-conflicts`; at the end once for the workspace; `cargo deny check`
if available. Add a short `cc-system` section to `docs/architecture.md` and update the elevated-helper section. One
final report: public API (signatures and intended call sites for the app worker), files changed, check results, and
every decision this brief did not cover.

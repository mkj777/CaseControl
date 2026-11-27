# Brief: second round of ADR 0001 #42 review fixes

Context: brief 20 was carried out; independent verifiers confirmed most items and found the gaps below (each verified,
some reproduced with `FakeTransport`). Same rules as brief 20 (no new dependencies, no launch/tauri-build/kill, no
driver examples, nothing that writes to HID/NvAPI/registry/Task Scheduler, never read `%USERPROFILE%\.tauri`, no
commit). Fix the cause, add a regression test per item, keep everything green. Three workers in parallel, disjoint
ownership; run checks per package.

## Part E2 (owner: `cc-core`, `cc-drivers`, `cc-engine`, `cc-config`)

1. **MSI restore on a marked board** (`msi_mystic_light/controller.rs` `restore_target` ~1041, `may_skip` ~1349):
   the open-time readback is treated as flash content whenever this controller instance has sent nothing. For a board
   opened with the unreleased marker (stored own lighting + pending setting, or nothing stored) that readback is the
   volatile state a previous instance left. Result: configured LED counts written to flash, a wrong `Unchanged` that
   drops the backup, or a per-LED leftover that refuses the restore forever. Fix: a marked open counts like "sent since
   open": always do the volatile leave step with the trusted base's counts first, and allow `may_skip` only for an
   unmarked board that nothing has written to. Test with the scenario from the verifier: flash/stored count 3,
   configured 10, marked open with a pending setting, pending restore queued first -> flash keeps 3.
2. **MSI save in a hardware mode leaves the base's LED counts on screen** (controller.rs ~990, `after_flash`;
   worker.rs `shown_mode` ~360): after a flash write the board shows `next` (base counts) until the output changes.
   After the write, resend the current output volatile with the configured sizes (or make the worker resend it).
3. **Fingerprint after a failed record** (worker.rs ~782 `count_write`): when the device write succeeded but recording
   the new fingerprint failed, drop the stored fingerprint in memory too, so a later persist of the previous look is
   not `Unchanged`.
4. **Backup entries that cannot be parsed** (hardware_state.rs ~324-328 `with_invalid`): an unparsable `saved_backup`
   entry for a device must block a new backup for that device (report "original state unreadable", never write) and
   survive every rewrite.
5. **Migration order** (hardware_state.rs ~792-817 `migrate`): write the durable device-memory file first and the
   state file last, and migrate again on a later start while the new memory file is missing and the legacy file still
   has backups (e.g. machine id unknown at the first attempt).
6. **Flaky test** `cc-engine tests::shutdown_honours_budget_while_detection_is_stuck` (tests.rs ~1400): fails in about 40%
   of runs on this tree, never on `HEAD`. Find what in the ADR #42 engine changes (per-backend add, Keep, early apply)
   lets the release race the assertion, fix the cause (not the timeout), and show 30 green runs in a row.

## Part S2 (owner: `cc-system`, `cc-conflicts`)

1. **Logon trigger user** (cc-system `sys.rs` ~895-906, task.rs ~382-390): Windows stores a logon trigger's `<UserId>`
   as `DOMAIN\user`, never a SID; the code still turns that name into a SID with `LookupAccountNameW`, which fails for
   Entra users (`AzureAD\Name`) and offline domains, and then `register_verified` deletes the task it just registered.
   Compare by translating the expected SID with `LookupAccountSidW` and comparing `DOMAIN\user` case-insensitively
   (accept either match). Replace the test that feeds a SID in the trigger XML with realistic XML.
2. **Common Startup owner check only in the helper** (cc-conflicts `platform.rs` ~221): the owner check on the common
   Startup folder must only apply in the elevated helper; the normal scan lists those items as before.
3. **Action-neutral helper errors**: `other_account_message` and `ADMIN_OWNED_REQUEST_MESSAGE` say "was not set up"
   also for removal; make them name the right action (set up / remove) or be neutral.

## Part D2 (owner: `apps/desktop`)

1. **Bounded reads in boot mode**: profiles and machine state are size-checked and then read unbounded (TOCTOU). Read
   every boot-mode input through a bounded reader (`take`), as `config.toml` already is; if a store API in cc-config is
   needed, add a bounded variant in the desktop crate or report it.
2. **Error text** (commands.rs ~856): with S2 item 3, show the helper's sentence for both directions correctly; check the
   switch-off path shows "not removed" semantics.

## Report

Per part: what changed per item, tests added, check results (part E2: the flaky test 30x). The orchestrator runs the
full workspace checks afterwards.

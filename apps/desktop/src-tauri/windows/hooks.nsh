; CaseControl NSIS installer hooks (bundle > windows > nsis > installerHooks in tauri.conf.json).
;
; Before the uninstaller removes any file (ADR 0001 #42), the installed executable cleans up after itself with
; `--uninstall-cleanup` (handled in `main` before Tauri starts, see src/cleanup.rs): it asks a running instance to
; quit (`--quit-for-uninstall`: devices released, nothing saved to them, no update installed), deletes the per-user
; logon task, writes the original saved state back to devices CaseControl saved colours to, and removes the start-up
; task through the elevated helper when it exists (one UAC prompt). It is best effort, bounded in time and always
; exits with 0.
;
; Skipped when the uninstall is not a real removal:
; - updates run the uninstaller with /UPDATE and keep everything;
; - a reinstall (an upgrade, downgrade or repair where the user chose to uninstall first) runs the uninstaller in
;   place from the install folder (the tauri-bundler 2.10 template's PageLeaveReinstall: `ExecWait '$R1 _?=$4'`). A
;   real uninstall runs from a copy in %TEMP% (NSIS copies itself there and runs the copy with `_?=`), so the
;   uninstaller's own folder ($EXEDIR) is the install folder only for a reinstall. The new version installs right
;   after and keeps the tasks and the saved state.
; A silent uninstall (/S) passes `--no-prompt`: no UAC prompt, the start-up task is left (logged).

!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
  ${AndIf} $EXEDIR != $INSTDIR
  ${AndIf} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    DetailPrint "Cleaning up CaseControl's tasks and saved device colours..."
    ${If} ${Silent}
      ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --uninstall-cleanup --no-prompt'
    ${Else}
      ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --uninstall-cleanup'
    ${EndIf}
  ${EndIf}
!macroend

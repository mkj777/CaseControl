# Hardware persistence (and why there is no pre-login service)

Research only, no decision. Context: ADR 0001 #3 (one process, no service; **confirmed by the owner on 2026-11-16**:
stay lightweight, user-only, as few rights as possible), #24 (last profile applied on start), #33 (brightness, all
off, everything persists). Target experience: **the PC turns on and immediately shows the wanted profile, before
login, without CaseControl running**, using state saved on the devices themselves. Nothing in this research wrote to
hardware, services or the registry. Statements marked **UNVERIFIED** need a hardware test on this machine.

## Summary

| Device | What the hardware can remember | How close to a CaseControl profile | Pre-login look after a save |
|---|---|---|---|
| MSI B650 TOMAHAWK (MS-7D75), JRAINBOW1 = 20 LEDs | Per header: one effect, two colours, speed, brightness (off, 10 %..100 %), colour flags, LED count | Hardware-effect profiles and single-colour profiles: exact. Per-LED profiles: one colour for all 20 LEDs (approximation). Off: exact (Static, brightness 0). | Saved state from power-on, through POST, until software writes |
| Logitech G560 | Per zone (4 zones): static colour, effect, off (0x8070 persistence byte) | Exact: CaseControl's direct mode on the G560 already is Static per zone | Saved effect |
| Wooting 60HE | Only the keyboard's onboard profiles, written by Wootility | Nothing CaseControl can write; the owner mirrors the look once in Wootility | Active Wootility profile |
| Logitech PRO Wireless | 0x8070 persistence byte exists (not implemented in our driver); onboard profiles may override | Per zone static / effect / off, if the mouse honours it (**UNVERIFIED**) | Firmware effect or onboard profile |
| RTX 5080 FE | Nothing known; OpenRGB marks it not savable | None | Card default until CaseControl runs after logon |

The decisive point for MSI: per-LED (direct) colours travel in a report that has no save byte, so they are never
stored; only the zone-level configuration packet is.

## 1. MSI Mystic Light, 185-byte class, MS-7D75 (`1462:7D75`)

### 1.1 What the saved packet contains

`FeaturePacket_185` (report 0x52, 185 bytes; `reference/openrgb/MSIMotherboardController/MSIMotherboardCommon.h`,
ported in `crates/cc-drivers/src/msi_mystic_light/protocol.rs`) is the board's whole lighting configuration. Byte 184
is `save_data`: "This byte is stored as a zero in the data flash, but is set to 1 at the protocol level to tell the
firmware to save the active configuration to flash" (`MSIMotherboard185Controller.cpp` ~line 400).

Per zone (`ZoneData`, 10 bytes; `RainbowZoneData` 11 bytes):

| Offset in zone | Field | Meaning |
|---|---|---|
| +0 | `effect` | 1 Static, 2 Breathing, ... 26 Rainbow wave ... (`enum MSI_MODE`); 0x25 = per-LED (direct) |
| +1..+3 | `color` | RGB, primary colour |
| +4 | `speedAndBrightnessFlags` | `brightness << 2 \| speed`; brightness 0 = off, 1..10 = 10 %..100 %; speed 0..2 |
| +5..+7 | `color2` | RGB, secondary colour (two-colour effects) |
| +8 | `colorFlags` | 0x80 = use custom colour (else random), plus sync bits on the on-board zone |
| +9 | padding | |
| +10 (JRAINBOW only) | `cycle_or_led_num` | LED count of the ARGB header |

JRAINBOW1 starts at byte 31 (effect 31, colour 32-34, speed/brightness 35, colour2 36-38, flags 39, padding 40, count
41); JRAINBOW2 at 42. JRGB1 at 1, JRGB2 at 174, on-board records at 74..164 (MS-7D75 has no on-board LEDs; the board
table entry is `{0x7D75, 0, 0, 0, 2, &zones_set13, DIRECT_MODE_PER_LED, 0}`). The probe on 2026-11-16 read stored
JRAINBOW counts 20/20, so the LED count already survives power cycles.

So a save persists per header: effect, two colours, speed, brightness, random/custom flag and LED count. Nothing
per-LED.

### 1.2 Per-LED vs zone, effects, brightness, off

- **Per-LED colours: not savable.** Direct mode = an "enable per-LED" 0x52 packet (effects set to 0x25) followed by
  frames on report 0x53 (`FeaturePacket_PerLED_185`, 5 header bytes + 240 RGB triples, no save byte). OpenRGB's
  `Update(save)` ignores `save` in direct mode; "Direct" is registered without a save flag while Static and all
  effects carry `MODE_FLAG_MANUAL_SAVE` (`RGBController_MSIMotherboard185.cpp` 346-383). Our `save_to_hardware()`
  returns `Unsupported` in direct mode. Saving the enable-direct packet itself (effect 0x25, save 1) would make the
  board boot into per-LED mode with no frame data; behaviour unknown, nobody upstream does it: **never**.
- **Static colour per header: savable.** One colour for all 20 LEDs of JRAINBOW1.
- **Hardware effects (ADR #25): savable exactly** (effect, colours, speed, brightness).
- **Brightness: savable** in 10 % steps (levels 1..10) for Static and the effects that have brightness.
- **Off: savable** as Static with brightness 0 (`MSI_BRIGHTNESS_OFF`; Static's brightness range is 0..10 upstream, a
  shipped path) or Static with colour black. `MSI_MODE_DISABLE` (effect 0) exists in the enum but
  `SetupMode("Off", MSI_MODE_DISABLE, 0)` is commented out for this class in OpenRGB without a stated reason:
  untested, avoid.

### 1.3 What the board shows before Windows

The RGB MCU loads the stored packet at power-up; the ARGB headers get 5 V when the PSU turns on, so the saved state is
visible from power-on through POST, BIOS setup, Windows boot and the login screen, until software writes something.
Evidence: the owner's demo on 2026-11-16 ("previous lighting (JRGB effect 25, Rainbow wave 26) returned" after
release, a state saved earlier by other software) and reports that MSI lighting survives uninstalling MSI Center
([SignalRGB MSI troubleshooting](https://signalrgb.developerhub.io/troubleshooting/msi),
[Microcenter community](https://community.microcenter.com/discussion/comment/58243/)). The BIOS "RGB Light Control" /
"EZ LED Control" option turns the LEDs off regardless of the stored config ([MSI FAQ 3532](https://www.msi.com/faq/3532));
it must be enabled. Because it is stored on the board, this works with Fast Startup, after sleep, after a crash and
without CaseControl installed.

### 1.4 Flash wear and safety

- The MCU is not identified. Its firmware reports "AP/LD 0.6/1.5" (APROM/LDROM, Nuvoton NuMicro terminology) and the
  reference speaks of "data flash" (**UNVERIFIED** that it is Nuvoton). Embedded MCU data flash is typically rated
  10k-100k erase cycles (generic, **UNVERIFIED** for this part); whether the firmware skips writes of unchanged data or
  wear-levels is unknown.
- The reference save sends **two** reports with the save byte set (current state, then new state; "Windows Mystic
  Light sends two consecutive reports"), so assume 2 flash writes per save.
- Budget at a pessimistic 10k cycles: 5,000 saves. At 20 saves/day that is ~8 months; at 3/day ~4.5 years; with
  explicit saves only, effectively unlimited. Saving on live edits (30 Hz, ADR #21) would exhaust it within minutes:
  **never save on live edits.**
- How others do it: OpenRGB never auto-saves MSI (all modes are manual-save, `Update(flags & AUTOMATIC_SAVE)` is
  false); the user clicks "Save To Device". MSI Center's save policy is undocumented (**UNVERIFIED**).
- Brick history: OpenRGB's MSI code "reportedly bricked the RGB on certain MSI boards" (old wiki, issue
  [#389](https://gitlab.com/CalcProgrammer1/OpenRGB/-/work_items/389)). A saved bad packet survives reboots. Rules:
  only save a packet derived from a fresh read-back with the minimal changed bytes; never a constructed packet; never
  effect 0x25 or 0; keep the LED count byte unchanged; check firmware/length gates as detection does.

### 1.5 Safe hardware test plan (MS-7D75)

Needs the owner's explicit go. Requires a small new example (e.g. `msi_mystic_light_save`) that reads, shows a byte
diff and only writes after a typed confirmation; the existing `msi_mystic_light_probe` is read-only and covers step 0.

**Step 0: capture the current saved config (read-only).**
1. Turn off CaseControl's autostart for one boot (it applies the last profile at start, ADR #24) and make sure
   SignalRGB / MSI Center are not running. BIOS RGB Light Control enabled.
2. Full cold boot: Shut down with Fast Startup disabled (or `shutdown /s /t 0`), power on.
3. Run `cargo run -p cc-drivers --example msi_mystic_light_probe`. Right after a cold boot with no software writing,
   the 0x52 read-back is what the MCU loaded from flash. Store the 185 bytes as hex in a file kept in the repo or
   `%LOCALAPPDATA%\CaseControl` (e.g. `msi-7d75-saved-<date>.hex`). Note what the fans look like now (this is the
   reference look to restore).
4. Run the probe a second time and confirm both reads are identical.

**Step 1: Static red on JRAINBOW1, saved.**
- Packet: the captured packet with only JRAINBOW1 changed: byte 31 = 0x01 (Static), 32-34 = FF 00 00, 35 = 0x28
  (brightness 10, speed 0), 36-38 = FF 00 00, byte 39 |= 0x80 (custom colour), 40 = 0, 41 unchanged (0x14 = 20).
  The driver's `apply_mode` also clears the on-board zone sync bits (bit 7 of byte 78, 0x30 of byte 82) for effects
  up to Lightning; the tool should use `apply_mode`/`apply_zone_color` so the bytes match the tested path, and print
  every changed byte. Byte 184 = 1.
- Send as the reference does: read current, send it with save 1, send the new packet with save 1.
- Expected immediately: the 20 fan LEDs turn solid red.
- Cold boot (as in step 0). Expected: red from power-on, through POST/BIOS and the login screen. After login (CaseControl
  autostart still off) the probe reads the same bytes as sent, byte 184 = 0.

**Step 2: Off, saved.** Same as step 1 with byte 35 = 0x00 (Static, brightness 0), colour kept red. Expected after a
cold boot: JRAINBOW1 dark from power-on. Optional variant: brightness 10 with colour 00 00 00.

**Step 3 (optional): a hardware effect**, e.g. Rainbow wave (26) at brightness 5 (byte 35 = 0x14 | speed). Expected
after a cold boot: the effect at half brightness. The owner's earlier observation already suggests this works.

**Step 4: restore.** Send the step 0 bytes with byte 184 = 1 (same two-report sequence), cold boot, verify the probe
read equals the step 0 capture and the look matches the note. Re-enable CaseControl autostart.

Abort rules for the tool: any read failure, a read length other than 185, a firmware gate failure, a diff touching
anything other than the JRAINBOW1 record and the two documented sync bytes, or an effect of 0 or 0x25 in the new
packet. Total flash writes for the whole plan: about 8.

### 1.6 Mapping a CaseControl profile to the closest savable MSI state

Per non-hidden header with LEDs (on this machine only JRAINBOW1), from the profile's state:

| Profile state | Saved hardware state |
|---|---|
| Hardware effect (ADR #25) | Exactly that effect, colours, speed, brightness |
| Direct, all LEDs one colour | Static, that colour: exact |
| Direct, mixed colours | Static with one representative colour (choice below): approximation |
| Device brightness × master brightness (ADR #33) | Either MSI level `round(b × 10)` (10 % steps, colour unchanged) or colour premultiplied at level 10 (finer steps, hue drifts at low values); choice for the owner |
| All off, or brightness 0 | Static, brightness 0 |
| Hidden zone | Bytes left as read; hidden device: not saved at all (ADR #26) |

Representative colour for mixed LEDs, options:
- **First LED** (what OpenRGB and our `set_mode` do today): predictable, arbitrary.
- **Average** (mean in linear RGB): good for gradients of similar hues, turns a rainbow into grey/white.
- **Dominant** (most frequent colour, or the largest hue cluster): keeps a saturated colour, ignores minorities.
- **Owner picks a "hardware colour"** per profile, preview shown in the UI.

## 2. Logitech G560 (`046D:0A78`)

- 0x8070 `SetZoneEffect` has a persistence byte. OpenRGB: "On 0x8070 devices every effect write is ephemeral by
  default ... Add a Save button on firmware-effect modes so users can explicitly commit the active mode to NVM. Direct
  is excluded ..." (`reference/openrgb/LogitechController/LogitechHIDPP20Controller/RGBController_LogitechHIDPP20.cpp`
  ~1326).
- Our `save_to_hardware()` replays the active firmware mode with persistence 1, Off included (tests in
  `crates/cc-drivers/src/logitech/g560.rs`). Direct on the G560 is Static per zone (4 LEDs = 4 zones), so the mapping
  is exact: save Static per zone with the profile colours (brightness premultiplied), effects as they are, all off =
  Off. Only the driver path from direct to a saved Static per zone needs adding.
- The G560 shows its firmware effect when it powers up (own PSU, USB). Wear: same policy as MSI (NVM write per save,
  endurance unknown).

## 3. Wooting 60HE (ARM) (`31E3:1310`)

- Direct (SDK protocol) colours are volatile; `release()` sends `WOOTING_RESET_ALL_COMMAND` and the keyboard shows its
  active profile again (`crates/cc-drivers/src/wooting/controller.rs`). OpenRGB: `@save :x:`.
- Persistent lighting is part of the onboard profiles, which Wootility writes; the protocol is not public and the
  driver deliberately never touches profiles ([Wootility changelog](https://wooting.io/wootility/changelogs/5.1.0)).
- Closest result: the owner sets the active profile's lighting in Wootility once (static colour, effect or off). It
  shows from power-on. CaseControl could only remind; it cannot sync.

## 4. Logitech PRO Wireless (Lightspeed `046D:C539`)

- Our Lightspeed controller returns `Unsupported` for save; upstream's legacy Lightspeed controller offers save
  through the same 0x8070 persistence byte (`RGBController_LogitechLightspeed.cpp`, `@save :white_check_mark:`).
- Whether the mouse shows that at power-on or its onboard-profile lighting (feature 0x8100, G HUB "onboard memory"
  mode) is **UNVERIFIED**. The mouse sleeps when idle and is under the hand, so the value is low; adding save is a small
  driver change if wanted.

## 5. RTX 5080 Founders Edition (NvAPI Illumination)

- We write the active values (`bDefault = 0`). The params struct also has a `bDefault = 1` "default values" set
  (`reference/openrgb/NVFC/nvapi.h` ~840); what writing it does and whether it is stored on the card is undocumented.
  Not to be tried without a separate decision.
- OpenRGB marks the device `@save :x:`; third-party FE tools re-apply at startup instead
  ([nvidia_fe_lighting](https://awesome.ecosyste.ms/projects/github.com%2Fintel00000%2Fnvidia_fe_lighting)); board
  partner threads describe cards booting to their default
  ([EVGA forum](https://forums.evga.com/Is-this-normal-LEDs-default-upon-shutdownreboot-m2536412.aspx)).
- Result: before logon the logo shows the card default. Cheap read-only check: set a colour, cold boot, look at POST
  (expected: default). Without a service there is no way to change this pre-login.

## 6. When CaseControl writes flash: options for the owner

1. **Explicit only.** A "Save to device" action (per device and/or "save all"), shows what the hardware will look like
   (the mapped preview). Fewest writes, the owner controls drift between profile and hardware.
2. **Automatic on profile apply and on all off / on, rate-limited.** Debounce (e.g. write 30 s after the last
   apply/toggle), skip when the mapped packet equals the last saved one (remember a hash, or compare with a read-back),
   daily cap (e.g. 20 per device). Hardware follows the profile without thinking about it.
3. **Automatic once at session end.** On logoff/shutdown (`WM_QUERYENDSESSION`/`WM_ENDSESSION`) and on Quit, save the
   mapped current state if it differs. One write per session at most; nothing saved after a crash or power loss;
   shutdown has a short time budget.

Any automatic option changes the meaning of `release()`: today it restores the open-time configuration (which, after
a save, would be the previously saved one). With saves, release should hand back the last state CaseControl saved, so
quitting does not flash an old look. Live edits never write flash under any option.

## 7. Rejected: pre-login Windows service (short)

Researched and rejected by the owner (ADR #3 stays). Reasons on record:
- A LocalSystem service whose binary lives in the per-user install folder (`%LOCALAPPDATA%`) is a local privilege
  escalation: any process running as the user can replace the exe or plant a DLL and get SYSTEM at the next service
  start without a UAC prompt ([MITRE T1574.010](https://attack.mitre.org/techniques/T1574/010/)). A service therefore
  forces a per-machine install (admin for install, every update and uninstall), reversing ADR #32.
- It splits the app into a session 0 engine plus a per-user UI with named-pipe IPC; OpenRGB's service shows the
  typical costs (separate config location [#5108](https://gitlab.com/CalcProgrammer1/OpenRGB/-/issues/5108), server
  state confusion [#5140](https://gitlab.com/CalcProgrammer1/OpenRGB/-/issues/5140), detection races at service start
  [#5289](https://gitlab.com/CalcProgrammer1/OpenRGB/-/issues/5289), power events left as a TODO in
  [`main_Windows.cpp`](https://gitlab.com/CalcProgrammer1/OpenRGB/-/raw/master/startup/main_Windows.cpp)).
- Windows' default shutdown with Fast Startup hibernates session 0, so a service is not restarted at power-on and must
  handle resume itself ([Microsoft](https://docs.microsoft.com/en-us/windows-hardware/test/weg/delivering-a-great-startup-and-shutdown-experience));
  hardware-saved state has no such gap.
- Feasibility was not the blocker: HID and NvAPI GPU calls from session 0 are used by other services.

Related gap that stays relevant without a service: the app has no sleep/resume handling (no `WM_POWERBROADCAST` /
`RegisterSuspendResumeNotification` in the repo). After wake, devices show their saved or firmware state until the app
re-applies; with hardware saves that state is at least close to the profile.

## Sources

- Repo: `reference/openrgb/MSIMotherboardController/MSIMotherboardCommon.h`, `MSIMotherboard185Controller/*.cpp`;
  `reference/openrgb/LogitechController/LogitechHIDPP20Controller/*`, `LogitechLightspeedController/*`;
  `reference/openrgb/NVFC/nvapi.h`; `reference/openrgb/WootingKeyboardController/*`; `crates/cc-drivers/src/*`,
  `crates/cc-drivers/examples/msi_mystic_light_probe.rs`; `docs/testing-hardware.md`.
- MSI: https://www.msi.com/faq/3532 ; https://signalrgb.developerhub.io/troubleshooting/msi ;
  https://community.microcenter.com/discussion/comment/58243/ ; https://gitlab.com/CalcProgrammer1/OpenRGB/-/work_items/389
- Wooting: https://wooting.io/wootility/changelogs/5.1.0
- NVIDIA: https://awesome.ecosyste.ms/projects/github.com%2Fintel00000%2Fnvidia_fe_lighting ;
  https://forums.evga.com/Is-this-normal-LEDs-default-upon-shutdownreboot-m2536412.aspx
- Service (rejected): https://gitlab.com/CalcProgrammer1/OpenRGB/-/raw/master/startup/main_Windows.cpp ;
  OpenRGB issues 5108, 5140, 5289 ; https://attack.mitre.org/techniques/T1574/010/ ;
  https://docs.microsoft.com/en-us/windows-hardware/test/weg/delivering-a-great-startup-and-shutdown-experience ;
  https://v2.tauri.app/distribute/windows-installer/

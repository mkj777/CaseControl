# Research overview, local inventory and reconciled contradictions (2026-11-06)

Four research reports were produced by parallel agents from primary sources (OpenRGB source,
LibreHardwareMonitor source, PawnIO docs, Microsoft Learn, vendor docs). This file ties them
together, records what was found on the development machine, and resolves contradictions.

Reports:
- `01-device-access-paths-openrgb.md`: which RGB devices need which access path (HID / SMBus / Super I/O / GPU I2C), protocols, OpenRGB data model, licensing.
- `02-fancontrol-coexistence.md`: how FanControl/LibreHardwareMonitor touch hardware (PawnIO), named-mutex convention, what actually breaks, options for a new app.
- `03-conflicting-software-catalog.md`: catalog of RGB/hardware-monitor software, process/service/driver names, detection APIs that work without elevation.
- `04-rust-windows-ecosystem.md`: HID crates, Win32 crates, GUI frameworks with tray support, IPC, config, packaging, signing, CI.

## Local inventory (development machine, verified with PowerShell on 2026-11-06)

| Item | Finding | Access path |
|---|---|---|
| Mainboard | MSI MAG B650 TOMAHAWK WIFI (MS-7D75) | Mystic Light controller is USB HID `1462:7D75`, bus-reported name "MYSTIC LIGHT". OpenRGB lists `MS_7D75` in its tested detector table (not behind `ENABLE_UNTESTED_MYSTIC_LIGHT`). Case fans/strips on the board's ARGB headers go through this device. No kernel driver needed. |
| GPU | NVIDIA GeForce RTX 5080 | GPU RGB (if any) is NvAPI I2C via driver DLL, no kernel driver. |
| RAM | 2x Kingston `KF564C32-16` (part number truncated by SMBIOS) | Owner confirmed on 2026-11-06 that the sticks have no RGB, so nothing on this machine needs SMBus. (The RGB variants would use DDR5 SPD-hub SMBus, the riskiest path in the research, OpenRGB issue #4934.) |
| Keyboard | Wooting 60HE (ARM), `31E3:1310` | USB HID, OpenRGB has a Wooting controller; Wooting also publishes an RGB SDK. |
| Logitech G560 speakers, PRO Wireless mouse via LIGHTSPEED receiver | `046D:0A78`, `046D:C539`; both exposed as Windows **LampArray** devices (virtual HID via Logitech LampArray service) | Either Logitech HID protocol directly (fights G HUB) or Windows Dynamic Lighting. |
| HyperX Cloud III S Wireless `03F0:06BE`, SteelSeries Arctis 7+ `1038:220E` | Headsets with vendor-defined HID interfaces | HID; RGB presence unverified. |
| No Corsair / NZXT / Lian Li / Aquacomputer hub found | | The "case colors" on this machine are the Mystic Light headers. |

Software and drivers present:
- FanControl V285 running (`FanControl.exe`), `FanControl.Service` installed but stopped.
- **PawnIO 2.0.1** installed, `PawnIO.sys` running (Microsoft WHCP-signed). FanControl uses it for Super I/O / SMBus sensors.
- **SignalRGB 2.5.84** installed per-user (`%LOCALAPPDATA%\VortxEngine`), `SignalRgb.Service` stopped, but **`SignalRgbDriver.sys` kernel driver is running (auto-start)** even with the app closed. Sectigo EV + WHCP signed. CVE-2026-8049/8050 were in this driver family (fixed in 1.3.6/1.3.7).
- **`inpoutx64.sys` running (auto-start)**, "Kernel level port access driver" signed by Red Fox UK Limited. This is the InpOut32 family that OpenRGB documentation says to remove. Owner unknown (likely installed by SignalRGB or a hardware tool). Concrete conflict-view example.
- SignalRGB registered itself as a **Windows Dynamic Lighting provider** (`HKCU\Software\Microsoft\Lighting\Providers`: `SignalRGB_4n6rtbnm9e9y2`, `WindowsLighting`), i.e. it uses the MSIX-package-identity route for background LampArray control.
- Logitech G HUB (updater service running, `logi_lamparray_service` running), SteelSeries GG, CPU-Z MSI edition.
- Dynamic Lighting registry: `AmbientLightingEnabled=0`, `ControlledByForegroundApp=0` on all three virtual LampArray devices.

## Contradictions between reports, resolved

| Topic | Catalog (03) said | Source-verified (01, 02) and local evidence | Use |
|---|---|---|---|
| OpenRGB kernel access | WinRing0, SMBus mutex adoption "disputed" | OpenRGB 1.0 uses PawnIO (MR !2833, 2025-05), honours `Global\Access_SMBUS.HTP.Method` in `i2c_smbus_pawnio.cpp` (`shared_smbus_access` default true); does NOT honour `Access_ISABUS` for Super I/O | PawnIO + SMBus mutex |
| SignalRGB kernel driver | "none found" | `SignalRgbDriver.sys` running on this machine; CERT VU#380058 CVEs | Has own driver |
| FanControl/LHM driver | WinRing0 | PawnIO since FanControl V238 / LHM 0.9.5 (Sep 2025); PawnIO 2.0.1 running here | PawnIO |
| SMBus mutex name | UNVERIFIED | Verified in OpenRGB source and RAMSPDToolkit constants: `Access_SMBUS.HTP.Method` (`Global\` prefix in OpenRGB) | Verified |
| MSI Mystic Light brick risk | family-level warning | Board PID 7D75 is in the tested list; the historic brick was a different, older code path | Tested, still treat writes conservatively |

## Facts that constrain architecture (no decisions taken here)

1. Everything RGB on this machine is reachable from user mode (HID, LampArray, NvAPI). SMBus (DDR5 RAM, older boards) is the only path that needs PawnIO, admin rights or a service, and the mutex discipline.
2. Coexistence with FanControl at the HID level is about firmware state and interleaved multi-packet commands, not file handles (HidSharp and hidapi both open shared). At the SMBus/Super I/O level it is about the `Global\Access_*` mutexes and sharing PawnIO instead of loading a second driver.
3. Background control of LampArray devices via Dynamic Lighting requires MSIX package identity and the `com.microsoft.windows.lighting` AppExtension. Unpackaged Win32 exes can only be foreground controllers.
4. Loaded-driver detection without elevation must use `EnumServicesStatusEx(SERVICE_DRIVER)`; `EnumDeviceDrivers` returns NULL image bases on Windows 11 24H2+ without SeDebugPrivilege.
5. OpenRGB is GPLv2 (some files GPL-2.0-only), liquidctl GPLv3. Porting code requires a compatible project license; using protocol facts only is the clean-room alternative.
6. Rust: `nusb` cannot do HID on Windows; `hidapi` opens shared but has no hot-plug and fixed share flags; `async-hid` has hot-plug; direct `windows` crate is the only route to custom share mode. `iced` has no tray support; Tauri 2 and Dioxus have it built in; eframe works with the standalone `tray-icon` crate.

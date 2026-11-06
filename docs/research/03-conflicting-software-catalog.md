# Windows RGB / low-level hardware software: conflict-detection catalog

Research only, no recommendations. Names marked **UNVERIFIED** come from third-party file catalogs, forum posts or memory and were not confirmed by a vendor/primary source; confirm on a real install (Task Manager Details tab, `sc query type= all`, `driverquery`). Citations are `[n]` into the reference list. Service names drift between product versions; match on prefixes/paths, not one exact string.

## 1. Catalog

Access path legend: **HID** = opens the USB HID interface with a vendor protocol (in practice one process owns the feature-report channel), **SMBus/I2C** = motherboard/RAM/GPU RGB ICs reached through the PCI SMBus controller (needs a kernel driver), **SIO** = Super I/O / port I/O (needs a kernel driver), **LampArray** = Windows HID LampArray path.

| Software | Process names | Windows services | Kernel driver | Access path | Fights over | Detection | Src |
|---|---|---|---|---|---|---|---|
| Corsair iCUE | `iCUE.exe`, `Corsair.Service.exe`, `CueLLAccessService.exe` | `CorsairLLAService` (display "Corsair LLA Service"; Corsair's own fix uses `sc delete CorsairLLAService`), `CorsairService` / "Corsair Service" / "Corsair Device Control Service" (name varies by version) | `CorsairLLAccess64.sys` / `CorsairLLAccess32.sys` (CVE-2020-8808) | HID + SMBus (RAM) + SIO | Corsair peripherals, Corsair RAM, Commander/iCUE Link hubs, the SMBus itself | process; SCM; Uninstall key "CORSAIR iCUE 5 Software"; path `%ProgramFiles%\Corsair\CORSAIR iCUE 5 Software` (UNVERIFIED) | [1][2][3] |
| ASUS Armoury Crate / Aura Sync | `LightingService.exe`, `ArmourySocketServer.exe`, `asus_framework.exe`, `ArmourySwAgent.exe`, `AsusCertService.exe`, `AsSysCtrlService.exe`, `Aac*Hal_x86.exe` (per-device Aura HALs); `ArmouryCrate.Service.exe` UNVERIFIED | `LightingService` (display "AURA Service (Lighting Service)" / "ASUS AURA SYNC"), `ArmouryCrateControlInterface`, `AsusCertService`, `AsusROGLSLService` (ROG Live), `asus`, `asusm`, `AsusAppService`, `GameSDK Service`, `AsusFanControlService`, `AsSysCtrlService`, `asComSvc`; driver services `asusgio2`, `asusgio3`, `IOMap` | `AsIO2.sys`, `AsIO3.sys` (CVE-2025-9336/9337), `IOMap64.sys` | SMBus/I2C + SIO + HID | ASUS boards, Aura RAM, ASUS GPUs, ROG peripherals, any SMBus RGB IC | process; SCM; paths `%ProgramFiles(x86)%\LightingService`, `%ProgramFiles%\ASUS\Armoury Crate Service`; UI is a Store/MSIX package | [4][5][6][7] |
| MSI Center / Mystic Light / Dragon Center | `MSI_Central_Service.exe`, `LEDKeeper2.exe`, `LightKeeperService.exe`, `Mystic_Light_Service.exe` (UNVERIFIED, file catalogs only) | `MSI_Central_Service` (UNVERIFIED); "Mystic Light SDK service" (SignalRGB docs, unnamed) | `NTIOLib_X64.sys` (CVE-2024-12227, CVE-2026-6102, CVE-2026-82908) | SMBus/I2C + SIO + HID | MSI boards, MSI GPUs, Mystic Light-synced RAM/peripherals | process; SCM; path `%ProgramFiles(x86)%\MSI\MSI Center\Mystic Light\` | [8][9][10] |
| Gigabyte RGB Fusion 2.0 / Gigabyte Control Center (GCC) | `RGBFusion.exe`, `GvLedServices.exe`, `RunLed.exe` (RGB Fusion); GCC process/service names UNVERIFIED | UNVERIFIED | `gdrv.sys` (CVE-2018-19320, Gigabyte APP Center / AORUS Engine family; RGB Fusion linkage UNVERIFIED) | SMBus/I2C + SIO | Gigabyte boards/GPUs via SMBus, RGB RAM (documented fight with iCUE) | process; path `%ProgramFiles(x86)%\GIGABYTE\RGBFusion`; MSI product code `{FFA8F1FA-3C2C-4A94-AC0B-0DF47272C25F}` (RGB Fusion) | [11][12][13] |
| ASRock Polychrome Sync / RGB Sync | `ASRRGBLED.exe` (shortcut name; as process UNVERIFIED) | unnamed background service (SignalRGB docs) | UNVERIFIED | SMBus/I2C + SIO | ASRock headers and SMBus devices | process; Uninstall key "ASRock Polychrome" / "ASRRGBLED" (UNVERIFIED) | [1][14] |
| NZXT CAM | `NZXT CAM.exe`, `cam_helper.exe` (vendor-confirmed) | UNVERIFIED | `WinRing0x64.sys` shipped in CAM 4.8.0 (CVE-2020-13512..13514); current status UNVERIFIED | HID (vendor: CAM "needs exclusive access") + SIO/MSR via WinRing0 | Kraken, Smart Device, Hue, RGB and Fan Controller; sensor polling | process; Uninstall key "NZXT CAM"; path `%LocalAppData%\NZXT\CAM` (UNVERIFIED) | [15][16] |
| Lian Li L-Connect 3 | UNVERIFIED; has its own background service and an in-app "Conflicting Process Running" prompt; beta integrates OpenRGB | UNVERIFIED | UNVERIFIED | HID (Uni Fan / Strimer controllers) | Lian Li hubs; vendor tells users to stop ASUS `LightingService` for OpenRGB mode | process; Uninstall key "L-Connect 3" | [17][18] |
| Razer Synapse 3 / 4 | S3: `Razer Synapse 3.exe`, `Razer Synapse Service.exe`, `Razer Synapse Service Process.exe`, `RazerCentralService.exe`, `Razer Central.exe`, `GameManagerService.exe`; S4: `RazerAppEngine.exe`, `GameManagerService3.exe`, `RzEngineMon.exe` | "Razer Central Service", "Razer Synapse Service" (vendor), "Razer Game Manager Service", `RzActionSvc` (forum); `RzSDKService` / "Razer Chroma SDK Service" UNVERIFIED | none found | HID; Chroma SDK as localhost REST/WebSocket server UNVERIFIED | all Razer devices; "Chroma Connect" handing devices to third-party apps UNVERIFIED | process; SCM; paths `%ProgramFiles(x86)%\Razer\`, `%ProgramFiles%\Razer\RazerAppEngine` | [19][20] |
| Logitech G HUB / LGS | `lghub.exe`, `lghub_agent.exe`, `lghub_updater.exe`, `logi_crashpad_handler.exe`; `logi_lamparray_service.exe` (DriverStore, community) | `LGHUBUpdaterService` (display "LGHUB Updater Service"); `Logitech_Lamparray_Service` (Dynamic Lighting bridge, community) | none (user-mode HID) | HID; LampArray via Logi driver | Logitech G peripherals; LampArray-exposed devices | process; SCM; path `%ProgramFiles%\LGHUB` | [21][22] |
| SteelSeries GG / Engine | `SteelSeriesGG.exe`, `SteelSeriesGGClient.exe`, `SteelSeriesGGUpdateService.exe`, `SteelSeriesSvcLauncher.exe`, older `SteelSeriesEngine.exe` | background service "may need manual stop" (SignalRGB); name UNVERIFIED | none found | HID | SteelSeries peripherals | process; Uninstall key "SteelSeries GG" | [1][23] |
| HyperX NGENUITY | Microsoft Store app (`9P1TBXR6QDCX`) plus "NGENUITY Legacy"; process name UNVERIFIED | none found | none found | HID | HyperX peripherals (RAM via SMBus UNVERIFIED) | enumerate AppX packages, not Uninstall key | [24] |
| Cooler Master MasterPlus+ | legacy `MasterPlus(PER. Only).exe`, `cm-blackhawk.exe`, `CMService.exe`; current MasterPlus+ exe UNVERIFIED | UNVERIFIED | UNVERIFIED | HID | CM peripherals, ARGB/fan controllers | process; Uninstall key "MasterPlus" | [1][25] |
| Thermaltake TT RGB PLUS | `TT RGB Plus.exe`, `ThermaltakeUpdate.exe` | UNVERIFIED | UNVERIFIED | HID | TT controllers/fans/AIOs | process; path `%ProgramFiles%\Tt\TT RGB Plus\` | [26] |
| EVGA Precision X1 | `PrecisionX_x64.exe` (UNVERIFIED) | UNVERIFIED | WinRing0 (CVE-2020-14979) | GPU I2C + MSR | EVGA GPU RGB, GPU I2C bus | process; Uninstall key | [27] |
| MSI Afterburner | `MSIAfterburner.exe`, `RTSS.exe` (both UNVERIFIED) | UNVERIFIED (driver believed loaded on demand) | `RTCore64.sys` / `RTCore32.sys` (CVE-2019-16098) | GPU I2C + MSR/port I/O | GPU I2C (fan/RGB), monitoring | process; Uninstall key | [28] |
| Zotac Firestorm, Sapphire TriXX, PowerColor, Gigabyte AORUS Engine | UNVERIFIED | UNVERIFIED | AORUS Engine: `gdrv.sys` family (CVE-2018-19320) | GPU I2C | vendor GPU RGB | process; Uninstall key | [13] |
| OpenRGB | `OpenRGB.exe` | none; optional SDK server on TCP 6742 (`--server`) | historically `WinRing0x64.sys`, Defender-flagged; admin per launch for SMBus per MR 2778; PawnIO replacement UNVERIFIED | SMBus/I2C + SIO + HID; shared SMBus mutex adoption disputed (issue 2785 says not implemented; HWiNFO forum reportedly confirms a later MR adopted it; UNVERIFIED) | everything it detects | process; TCP 6742 listener; Uninstall key or portable | [29][30][31][32] |
| SignalRGB | `SignalRgb.exe`, `SignalRgbLauncher.exe`, `Update.exe` under `%LocalAppData%\VortxEngine\` | none found | none found | HID + SMBus (own driver UNVERIFIED); has a **Conflicts** setting that force-closes other RGB apps (exception list in Settings) | all supported devices; may kill competing processes | process; per-user Uninstall key (HKCU); path `%LocalAppData%\VortxEngine` | [33][34] |
| HWiNFO64 | `HWiNFO64.exe` | "HWiNFO32/64 Kernel Driver" (demand-start driver service, forum) | `HWiNFO64A.SYS` | SMBus + SIO + MSR; honors shared bus mutexes | SMBus RGB/RAM ICs (collision corrupts RAM RGB until reboot) | process; SCM type SERVICE_DRIVER | [35][36] |
| AIDA64 | `aida64.exe` | none: driver loaded dynamically, not a registered service | `kerneld.x64` (new driver in v7.60) | SMBus + SIO + MSR; honors mutexes | SMBus | process; path | [37] |
| CPU-Z / HWMonitor | `cpuz.exe`, `HWMonitor.exe` | per-version driver service (UNVERIFIED) | `cpuz1xx_x64.sys` (e.g. `cpuz149_x64.sys`) | SMBus + SIO + MSR | SMBus (SPD reads) | process; SCM driver type | [38] |
| LibreHardwareMonitor / FanControl / OpenHardwareMonitor | `LibreHardwareMonitor.exe`, `FanControl.exe` (uses LibreHardwareMonitorLib) | temporary driver service created at start (UNVERIFIED) | `WinRing0x64.sys` (Defender-flagged) | SMBus + SIO + MSR; uses `Global\Access_ISABUS.HTP.Method`, `Global\Access_PCI`, `Global\Access_EC`, `Global\RazerReadWriteGuardMutex`, `Global\Access_USB_Sensors` | SIO fan control, SMBus | process; SCM driver type | [39][40] |
| Argus Monitor | `ArgusMonitor.exe` (UNVERIFIED) | UNVERIFIED | UNVERIFIED | SIO + SMBus | SIO fan control | process | |
| SpeedFan | `speedfan.exe` | driver service | `speedfan.sys` | SIO + SMBus | SIO | process; SCM driver type | [41] |
| Windows Dynamic Lighting | system component (`AmbientLightingServer` named in MS doc); host process UNVERIFIED; no user process | built-in | none | LampArray | every HID LampArray device (Logitech G LIGHTSYNC, Razer, HyperX, SteelSeries, ASUS ROG partner models) | registry HKCU (section 4); Windows build check | [42][43] |

## 2. SignalRGB's published "Conflicting Programs" list

URL: https://docs.signalrgb.com/troubleshooting/software-conflicts/conflicting-programs/ [1]. Headings verbatim: "ASUS — Armoury Crate / AURA Sync"; "ASRock — Polychrome RGB"; "Corsair — iCUE"; "Cooler Master — MasterPlus+"; "Gigabyte — RGB Fusion 2.0"; "Logitech — G Hub / Logitech Gaming Software"; "MSI — MSI Center / Mystic Light"; "NZXT — CAM"; "Razer — Synapse 3 / Synapse 2"; "SteelSeries — GG / SteelSeries Engine"; "Windows — Dynamic Lighting"; "Avast / AVG Antivirus". The page gives no process names. It states that AURA Sync, Polychrome, iCUE, RGB Fusion, G Hub/LGS, the Mystic Light SDK and SteelSeries keep background services alive after the window closes, and that Dynamic Lighting "can override SignalRGB even when no other RGB software is installed" (fix: set SignalRGB as priority app in Windows Settings; do not uninstall).

## 3. Corsair's list (cross-reference)

https://www.corsair.com/us/en/explorer/gamer/accessories/list-of-programs-known-to-conflict-with-corsair-icue/ [2]: MSI (Mystic Light, MSI Center), Gigabyte (RGB Fusion, GCC), ASUS (Aura Sync, Armoury Crate, AniMe Matrix), ASRock (RGB Sync, Polychrome), Razer (Synapse 3, Cortex, Chroma RGB), Dell Alienware Command Center, NZXT CAM, SteelSeries GG/Engine, Logitech G HUB/LGS, Cooler Master MasterPlus+, SignalRGB, OpenRGB, JackNet RGB Sync; plus VM software (VMware, VirtualBox, Hyper-V, Citrix, Red Hat).

## 4. Windows Dynamic Lighting (Windows 11)

- Native control of HID LampArray devices; foreground control since Win10 1809, background ("ambient") control since Windows 11 build 23466 [42][43].
- Policy: a foreground app always gets the device unless the user disables "Compatible apps in the foreground always control lighting"; the "Background light control" priority list arbitrates between ambient apps; a device not assigned to any app falls to **Autonomous mode** (firmware default effect) [42].
- Ambient/background controllers must have package identity (MSIX or sparse package) and declare the `com.microsoft.windows.lighting` AppExtension; `AmbientLightingServer` refuses unpackaged clients. An unpackaged Win32 exe cannot appear in the priority list [42].
- Settings > Personalization > Dynamic lighting. Registry (community-sourced; not in Microsoft docs): `HKCU\Software\Microsoft\Lighting`, DWORD `AmbientLightingEnabled` (1 on / 0 off) and `ControlledByForegroundApp` (1 = foreground wins) [44][45]. Microsoft only states that ambient settings are tied to the device and the USB port it is connected on; the HKCU location is community-reported only [42].
- Vendor bridge: Logitech installs `Logitech_Lamparray_Service` (`logi_lamparray_service.exe` in DriverStore); disabling it removes the device from the Dynamic Lighting page [22].

## 5. Shared-bus mutexes

HWiNFO, AIDA64, CPU-Z and LibreHardwareMonitor serialize SMBus/ISA/PCI access through named global mutexes; a program that skips them or creates them under `Local\` breaks the scheme, and a collision leaves RAM RGB unresponsive until reboot [36][32]. Verified names (LibreHardwareMonitor source): `Global\Access_ISABUS.HTP.Method`, `Global\Access_PCI`, `Global\Access_EC`, `Global\RazerReadWriteGuardMutex`, `Global\Access_USB_Sensors` [40]. The SMBus mutex is widely reported as `Global\Access_SMBUS.HTP.Method` but **UNVERIFIED** here (HWiNFO forum and rh-software.com unreachable). OpenRGB issue 2785 asks OpenRGB to adopt it; outcome unknown [32]. (Untested: `OpenMutex` on these names from an unelevated process would indicate a cooperating monitor is active.)

## 6. Windows APIs for unelevated detection

| Need | API | Elevation |
|---|---|---|
| Running processes (name, PID, parent) | `CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS)` + `Process32First/Next`; full path via `QueryFullProcessImageName` after `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` (fails for protected / other-session processes without SeDebugPrivilege) | none for enumeration [46] |
| Same, faster | `NtQuerySystemInformation(SystemProcessInformation)`; `SystemBasicProcessInformation` on 26100.4770+. Microsoft: internal, may change, link dynamically | none [47] |
| Services and driver services | `OpenSCManager(SC_MANAGER_CONNECT \| SC_MANAGER_ENUMERATE_SERVICE)` + `EnumServicesStatusEx(SC_ENUM_PROCESS_INFO, SERVICE_WIN32 \| SERVICE_DRIVER, SERVICE_STATE_ALL)`; `OpenService(SERVICE_QUERY_CONFIG)` + `QueryServiceConfig` for the binary path. Local authenticated users get CONNECT, ENUMERATE_SERVICE, QUERY_LOCK_STATUS, STANDARD_RIGHTS_READ on the SCM and QUERY_STATUS/QUERY_CONFIG on default service DACLs; services with tightened DACLs are silently omitted | none [48][49] |
| Loaded kernel drivers | `EnumDeviceDrivers` + `GetDeviceDriverBaseName`. **Windows 11 24H2+: all image bases are NULL without SeDebugPrivilege**, so unusable unelevated; use the running state from `EnumServicesStatusEx(SERVICE_DRIVER)` instead | admin on 24H2 [50] |
| Installed apps | `HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\*` (values `DisplayName`, `DisplayVersion`, `Publisher`, `InstallLocation`, `UninstallString`); 32-bit installers land in `Wow6432Node` (read with `KEY_WOW64_32KEY` from a 64-bit process); per-user installs (SignalRGB) under `HKCU\...\Uninstall` | none [51][52] |
| Store/MSIX apps (NGENUITY, Armoury Crate UI) | `Windows.Management.Deployment.PackageManager.FindPackagesForUser("")` or `GetPackagesByPackageFamily` | none (no cite retrieved) |
| OpenRGB SDK server | TCP connect to 127.0.0.1:6742 | none [30] |
| Dynamic Lighting state | `RegOpenKeyEx(HKCU\Software\Microsoft\Lighting)` (community key) | none [44] |

## 7. Game SDK integrations

Games use Razer Chroma SDK, Logitech LED Illumination SDK, Corsair iCUE SDK, SteelSeries GameSense and Windows LampArray; they drive the vendor service, not the device, so detecting the vendor service covers them.

## References

1. https://docs.signalrgb.com/troubleshooting/software-conflicts/conflicting-programs/
2. https://www.corsair.com/us/en/explorer/gamer/accessories/list-of-programs-known-to-conflict-with-corsair-icue/
3. https://help.corsair.com/hc/en-us/articles/30167929440781-iCUE-RAM-RGB-Detection-in-iCUE-5-17-and-5-18 ; https://nvd.nist.gov/vuln/detail/CVE-2020-8808 ; https://www.file.net/process/cuellaccessservice.exe.html
4. https://openrgb-wiki.readthedocs.io/en/latest/OpenRGB-Windows-Setup-and-Usage/
5. https://gist.github.com/allenk/fcbee909fbf8fb9a54d4484297a1eeba (ASUS removal script: services, drivers, paths)
6. https://rog-forum.asus.com/t5/release-notes/release-notes-armoury-crate-v5-6-3-0-aura-creator-3-6-5-0/ba-p/929268
7. https://www.kodemsecurity.com/cve-database/cve-2025-9336 (AsIO3.sys)
8. https://www.file.net/process/ledkeeper2.exe.html ; https://advanceduninstaller.com/MSI-NBFoundation-Service-19861502bc3f5fd97af0e9b7f7099eef-application.htm
9. https://forum.signalrgb.com/t/mystic-light-ledkeeper2-mystic-light/1612
10. https://nvd.nist.gov/vuln/detail/CVE-2026-6102 ; https://nvd.nist.gov/vuln/detail/CVE-2024-12227
11. https://www.advanceduninstaller.com/RGB-Fusion-298e7f468d27a5046b082b79d6bc0f8b-application.htm
12. https://forum.corsair.com/forums/topic/180785-ram-conflict-with-gigabyte-rgb-fusion ; https://forum.signalrgb.com/t/conflict-signalrgb-with-gigabyte-control-center/8953
13. https://www.cve.org/CVERecord?id=CVE-2018-19320 (gdrv.sys; record text not retrieved)
14. https://www.microcenter.com/tech_center/article/11170/how-to-adjust-rgb-led-settings-for-powerspec-desktops-with-asrock-motherboards
15. https://nzxt.com/blogs/news/nzxt-cam-not-detecting-device
16. https://talosintelligence.com/vulnerability_reports/TALOS-2020-1111
17. https://lian-li.com/fq-2/
18. https://lian-li.com/l-connect-3-x-openrgb/
19. https://mysupport.razer.com/app/answers/detail/a_id/1917
20. https://github.com/nxroot/synapsekiller
21. https://support.logi.com/hc/articles/360036179173 ; https://www.file.net/process/lghub_updater.exe.html
22. https://www.thewindowsclub.com/dynamic-lighting-not-working-in-windows ; https://redlib.ssps.io/r/logitech/comments/1afa2sn/what_is_logitech_lamparray_service
23. https://www.advanceduninstaller.com/SteelSeries-GG-94_0_0-ee9d6ec27c23b9c4060509e83b864eb5-application.htm
24. https://apps.microsoft.com/detail/9P1TBXR6QDCX ; https://www.hp.com/gb-en/tech-takes/gaming/explainer/hyperx-ngenuity-software.html
25. https://www.file.net/process/cm-blackhawk.exe.html
26. https://www.freefixer.com/library/file/TT%20RGB%20Plus.exe-302267
27. https://vulmon.com/vulnerabilitydetails?qid=CVE-2020-14979
28. https://nvd.nist.gov/vuln/detail/CVE-2019-16098
29. https://gitlab.com/CalcProgrammer1/OpenRGB/-/merge_requests/2778
30. https://dyn.manpages.debian.org/testing/openrgb/openrgb.1
31. https://www.neowin.net/amp/windows-1110-is-flagging-winring0-on-your-pc-monitoring-fan-control-apps-heres-why/
32. https://gitlab.com/CalcProgrammer1/OpenRGB/-/issues/2785
33. https://www.file.net/process/signalrgb.exe.html ; https://www.advanceduninstaller.com/SignalRgb-c3987bee4b9c33aea4a14e8090b1cdee-application.htm
34. https://forum.signalrgb.com/t/nzxt-cam-cant-run-while-signalrgb-is/9773
35. https://www.hwinfo.com/forum/threads/hwinfo32-64-kernel-driver-just-appears.2848
36. https://hwinfo.com/forum/threads/ram-detection-lost-occasionally.8657/post-38336 (fetch blocked; search summary only)
37. https://www.aida64.com/news/kernel-driver-update-aida64-v760 ; https://forums.aida64.com/topic/17205-kernel-driver-update-in-aida64-v760
38. https://www.file.net/process/cpuz149_x64.sys.html
39. https://www.file.net/process/winring0x64.sys.html
40. https://raw.githubusercontent.com/LibreHardwareMonitor/LibreHardwareMonitor/master/LibreHardwareMonitorLib/Hardware/Mutexes.cs
41. https://www.freefixer.com/library/file/speedfan.sys-22035/
42. https://learn.microsoft.com/en-us/windows/uwp/devices-sensors/lighting-dynamic-lamparray
43. https://learn.microsoft.com/en-us/windows-hardware/design/component-guidelines/dynamic-lighting-devices
44. https://geekrewind.com/how-to-turn-on-or-off-compatible-apps-always-control-lighting-in-windows-11/
45. https://geekrewind.com/how-to-turn-on-dynamic-lighting-in-windows-11/
46. https://learn.microsoft.com/en-us/windows/win32/api/tlhelp32/nf-tlhelp32-createtoolhelp32snapshot
47. https://learn.microsoft.com/en-us/windows/win32/api/winternl/nf-winternl-ntquerysysteminformation
48. https://learn.microsoft.com/en-us/windows/win32/api/winsvc/nf-winsvc-enumservicesstatusexa
49. https://learn.microsoft.com/en-us/windows/win32/services/service-security-and-access-rights
50. https://learn.microsoft.com/en-us/windows/win32/api/psapi/nf-psapi-enumdevicedrivers
51. https://learn.microsoft.com/en-us/windows/win32/msi/uninstall-registry-key
52. https://learn.microsoft.com/en-us/windows/win32/winprog64/registry-redirector

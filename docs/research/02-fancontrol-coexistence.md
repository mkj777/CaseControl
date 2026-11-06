# Coexisting with FanControl: hardware-access research (Windows, Nov 2026)

## 1. FanControl hardware access
- FanControl V238 (18 Sep 2025): migrated to PawnIO version of LibreHardwareMonitor (LHM); WinRing0 no longer shipped. Without PawnIO: no motherboard sensors. https://github.com/Rem0o/FanControl.Releases/releases/tag/V238
- LHM PR #1857 "Swap WinRing0 to PawnIO" merged 16 Sep 2025; first release 0.9.5; 0.9.6 updates PawnIO modules to 2.2.
- LHM does NOT use PawnIOLib.dll: opens \?\GLOBALROOT\Device\PawnIO directly, IOCTL_PIO_LOAD_BINARY / IOCTL_PIO_EXECUTE_FN; detects install via HKLM\...\Uninstall\PawnIO\DisplayVersion.
- Embedded modules: AMDFamily0F/10/17, IntelMSR, LpcACPIEC, LpcIO, RyzenSMU, SmbusI801, SmbusNCT6793, SmbusPIIX4, LpcCrOSEC.
- PawnIO: kernel driver PawnIO.sys runs Pawn-script modules. Driver GPL-2.0 with exception for IOCTL-only clients; PawnIOLib LGPL-2.1; modules must be LGPL and merged upstream (namazso/PawnIO.Modules) to run on the signed driver. Third-party use documented: read InstallLocation, LoadLibrary("PawnIOLib"), pawnio_open/load/execute/close (+ pawnio_version). Installer redistribution allowed. Local PawnIO.sys 2.0.1.0 is Microsoft Windows Hardware Compatibility Publisher-signed.
- No cross-process arbitration in PawnIO; named-mutex convention is the only interlock.
- Hardware touched by LHM: Super I/O via LpcIO (0x2E/0x4E), EC via LpcACPIEC, SMBus (RAMSPDToolkit for SPD/DIMM temps), CPU MSR/SMU, GPU vendor APIs, HID. LHM built-in HID controllers: AeroCool, AquaComputer, Arctic, Heatmaster, MSI, Nzxt, Razer, TBalancer -> FanControl touches NZXT and Aquacomputer HID even without plugins.

### Shared fan+RGB devices (plugins)
| Device family | Plugin | Holds device how | Interop |
|---|---|---|---|
| Corsair Commander Pro/Core/Core XT/Duo, iCUE LINK | FanControl.CorsairLink | Switches device to Software Mode; implements Global\CorsairLinkReadWriteGuardMutex | iCUE incompatible; SignalRGB OK from certain versions; OpenRGB 1.0+ OK; "hold the mutex as short as possible"; HWiNFO stutter (issues #242) |
| NZXT Kraken / Smart Device | LHM built-in + FanControl.NzxtKraken (HidSharp) | share mode not documented | - |
| Lian Li Uni Fan SL/AL/TL | FanControl.LianLi | L-Connect and plugin can't run simultaneously; separate builds Lighting / ARGB / Standard | - |
| Aquacomputer Quadro/Octo/D5 Next | LHM built-in + FanControl.AquacomputerDevices | direct HID | close AquaSuite |
- HidSharp opens with EFileShare.All -> handles shared; conflicts are firmware-state/protocol-level, not handle-level.

## 2. Conflict mechanisms
### Named mutexes
| Mutex | LHM Mutexes.cs | RAMSPDToolkit | OpenRGB 1.0 | CorsairLink |
|---|---|---|---|---|
| Global\Access_ISABUS.HTP.Method | yes | - | NO (super_io_pawnio.cpp has no mutex) | - |
| Global\Access_SMBUS.HTP.Method | no | yes (Global\ prefix UNVERIFIED) | yes (i2c_smbus_pawnio, setting Drivers.shared_smbus_access default true, WaitForSingleObject INFINITE per transfer) | - |
| Global\Access_PCI | yes | yes | - | - |
| Global\Access_EC | yes | - | - | - |
| Global\RazerReadWriteGuardMutex, Global\Access_USB_Sensors | yes (usage UNVERIFIED) | - | - | - |
| Global\CorsairLinkReadWriteGuardMutex | - | - | added later | yes |
- LHM creates mutexes with Everyone FullControl, falls back to OpenExisting, treats AbandonedMutex as acquired.
- HWiNFO/SIV/AIDA64/CPU-Z honouring the SMBus mutex: forum-sourced only. iCUE/ASUS/SignalRGB: UNVERIFIED.

### What goes wrong
- Verified: OpenRGB #1171 HyperX DRAM on Gigabyte Z390/Z490: SPD corruption when OpenRGB and CPU-Z/HWiNFO used SMBus concurrently; fixed via mutex + transactional writes. Mechanism: index/data register sequences interleaved by a second writer.
- MSI Mystic Light and Gigabyte Z390 bricks were single-tool misprobing (dumping SMBus 0x68), not two-driver collisions.
- Corsair: HWiNFO 7.42+ puts Commander Core back into hardware mode after each update, breaking SignalRGB RGB and fan control. SignalRGB "Fan Control Enabled" flag overrides third-party RPM targets.
- HID: Windows opens keyboard/mouse/touch collections exclusively (RIM); vendor-defined collections shared. dwShareMode=0 -> ERROR_SHARING_VIOLATION for others.

## 3. SignalRGB
- Ships SignalRgbDriver.sys (service SignalRgbDriver, auto start). Sectigo EV + Microsoft WHCP signatures.
- CERT VU#380058 (17 Jun 2026): CVE-2026-8049 (\.\SignalIo device without security descriptor -> any local user could do port I/O and PCI config IOCTLs), CVE-2026-8050 (NULL deref DoS). Fixed 1.3.6/1.3.7 via image-name allowlist + EV thumbprint check.
- Replaced WinRing0 around v2.2.32 (2023). Mutex behaviour UNVERIFIED.
- Complaints: ads/RAM in free tier; privacy policy lists analytics, device identifiers, ad partners Nitro/Playwire.

## 4. Options for a new app
| Option | Devices reachable | Conflict exposure w/ FanControl | Security | Effort/Cost |
|---|---|---|---|---|
| a) Own kernel driver | everything | second driver on same ports; must implement all Access_* mutexes | own attack surface (cf. SignalRGB CVEs), HVCI/blocklist | EV cert + Partner Center; attestation "testing only", retail needs WHCP/HLK |
| b) PawnIO | same as (a) via SmbusI801/PIIX4/NCT6793/IMC + LpcIO; new chips need LGPL module merged upstream | same bus contention but shares FanControl's driver; mutexes still required | MS-signed, sandboxed interpreter | low: LGPL lib, redistributable installer |
| c) WinRing0/InpOut32 | same | same + Defender quarantines WinRing0 | CVE-2020-14979 | blocked for most users |
| d) HID + WMI/ACPI only | most USB devices: ASUS Aura USB, MSI Mystic Light USB, Gigabyte RGB Fusion 2 USB, ASRock Polychrome USB, peripherals, AIOs, hubs; NOT SMBus DRAM/older boards | HID-level only: Corsair/NZXT/Lian Li mode flips, CorsairLink mutex, Access_USB_Sensors | no kernel code | low |
| e) OpenRGB SDK backend (TCP 6742, protocol 6, Rust crate openrgb2) | whatever OpenRGB 1.0 supports incl. SMBus | OpenRGB honours SMBus mutex but not Access_ISABUS; user runs OpenRGB elevated | delegated | low, extra dependency |

## 5. Microsoft requirements
- Vulnerable Driver Blocklist on by default since Win11 22H2, enforced under HVCI/Smart App Control. Defender detection VulnerableDriver:WinNT/Winring0 confirmed valid by Microsoft (names CapFrameX, FanCtrl, HWiNFO, LHM, MSI Afterburner, OpenRGB, SteelSeries Engine).
- Attestation signing: "for testing purposes only", retail needs WHCP/HLK. HVCI compatibility of PawnIO/SignalRgbDriver UNVERIFIED.

## UNVERIFIED / open
Latest PawnIO version; SignalRGB mutex behaviour; HWiNFO/iCUE/ASUS mutex code; plugin polling intervals; NZXT/Lian Li handle modes; Global\ prefix in RAMSPDToolkit; OpenRGB Corsair mutex in source.

# ADR 0001: Project decisions (taken by the owner on 2026-11-06)

Status: accepted. Source: question rounds in the planning session; research in `docs/research/`.

| # | Decision | Choice | Consequence |
|---|---|---|---|
| 1 | Language | Rust (owner's request) | Cargo workspace, MSVC toolchain. |
| 2 | Hardware access tier for v1 | User-mode only (USB HID, NvAPI). SMBus/PawnIO is a later tier. | A `Backend` boundary must exist so an SMBus backend can be added without touching drivers or UI. No kernel code, no admin requirement in v1. |
| 3 | Process shape | One Tauri 2 process: Rust backend holds devices, WebView2 UI, minimizes to tray. No Windows service. The device engine never runs without the process. | Autostart via HKCU Run key. Single instance via named mutex. |
| 4 | License / code reuse | Private, no publication planned. OpenRGB (GPLv2) and liquidctl (GPLv3) code may be ported freely. | If the project is ever published it must be GPL. `reference/openrgb/` holds copied sources. |
| 5 | Logitech / LampArray | Direct Logitech HID (HID++ 2.0, Lightspeed). Not a Dynamic Lighting provider. | Conflict view must list G HUB, `logi_lamparray_service` and Windows Dynamic Lighting as competitors. No MSIX requirement. |
| 6 | HID layer | `hidapi` crate for I/O plus own hot-plug via `CM_Register_Notification` (windows crate). | Shared opens (hidapi default). Device enumeration and hot-plug live in one crate. |
| 7 | Frontend | Vue 3 + TypeScript, Vite, pnpm. | Tauri 2 app with `tray-icon` feature. |
| 8 | Core data model | Designed by the orchestrator (owner delegated: "mach du alles"). Mirrors OpenRGB's RGBController semantics in idiomatic Rust. | See `docs/architecture.md`. |
| 9 | v1 devices | MSI Mystic Light `1462:7D75` (board + ARGB headers), Wooting 60HE ARM `31E3:1310`, Logitech G560 `046D:0A78`, Logitech PRO Wireless via Lightspeed receiver `046D:C539`, RTX 5080 Founders Edition (`10DE:2C02`, subsys `10DE:2095`) via NvAPI Illumination. | Each is a driver module in `cc-drivers`. |
| 10 | Config format | TOML under `%APPDATA%\CaseControl`. | `toml` + `serde`; comments preserved where edited in place. |
| 11 | Repository / CI | Private GitHub repo, GitHub Actions on `windows-latest`: fmt, clippy, nextest, cargo-deny, pnpm typecheck + build, Tauri build artifact. Pinned action SHAs, minimal permissions, branch protection on main. | Release = installer artifact from CI. |
| 12 | Conflict view powers | Show findings and, on explicit click, terminate a process or stop a service (service stop triggers UAC). No automatic intervention, no autostart changes. | Actions are opt-in per click and logged. |
| 13 | Non-goals | No community features, no ads, no telemetry, no network calls, no account, no game integrations, no effects marketplace. | `cargo-deny` bans HTTP client crates; the app never opens a socket. |
| 14 | UI language and names | English. Pages: Devices, Profiles, Conflicts, Settings. Tray menu: Show, Apply profile, Release devices, Quit. | No translation layer in v1. |
| 15 | Visual direction | Dark and spacious: few large elements with generous spacing, the device preview is the focus. | More scrolling, less information at a glance. Details are settled with the owner when the Devices page is built. |
| 16 | Product name | CaseControl. | Window title, tray, installer and `%APPDATA%\CaseControl`. Crate prefix `cc-` stays. |
| 17 | RAM | The Kingston DDR5 sticks have no RGB (owner confirmed). | Nothing on this machine needs SMBus. The PawnIO tier stays a design boundary only and has no planned work. |
| 18 | Devices page layout | Overview grid of device tiles; clicking a tile opens that device full-size in its own view with a back arrow. (2026-11-16) | Two views: overview and device detail. |
| 19 | Colour granularity | Down to single LEDs: whole device, zones, and LEDs selected in the preview. (2026-11-16) | Engine keeps a per-LED buffer; profiles store full buffers. |
| 20 | Colour picker | Swatches for quick clicks, a free picker (saturation/value area + hue bar + hex) that folds out below. (2026-11-16) | Custom component, no UI library. |
| 21 | When changes reach hardware | Live while dragging, throttled to the frame rate. No apply button. (2026-11-16) | Engine coalescing is the throttle. |
| 22 | LED selection | Click selects one LED, dragging sweeps several, Ctrl-click adds. The chosen colour applies to the selection. (2026-11-16) | |
| 23 | Accent colour | Neutral: whites and greys only; device colours in the previews are the only colour in the UI. (2026-11-16) | |
| 24 | Startup behaviour | The last applied profile is applied again on start. Without one, devices stay in hardware mode until the user acts. (2026-11-16) | `last_profile` in settings is set on every apply. |
| 25 | Hardware effects in v1 | Yes: per device a mode menu with speed, brightness, direction and colours as far as the device supports them. (2026-11-16) | |
| 26 | Hiding devices and zones | Right-click hides a device (overview tile) or a zone (device view). Hidden means not controlled: a hidden device is released and keeps its own lighting, a hidden zone gets black in direct colour. (2026-11-16, after the first hardware test: JRGB1/2 and JRAINBOW2 are empty headers) | Stored in settings per device id and zone name. Hardware modes act on the whole board and cannot skip a zone. |
| 27 | Bringing hidden things back | A "Show hidden" toggle in the overview and in the device view shows hidden items greyed out; right-click there unhides. (2026-11-16) | |
| 28 | Layout | Overview tiles fill the window width; the device preview scales with the window instead of a fixed size. Select all / none sits above the preview card. (2026-11-16) | |
| 30 | Design system | `docs/design-system.md` + `apps/desktop/src/tokens.css`, structure modelled on redpanda (role tokens, one primitives folder). Owner choices: neutral greys without a cool tint; one muted red for Faulted / errors / danger and a muted amber for warnings (amends #23: these are the only non-grey UI colours); radius 6/10/18; type 13/15/18/28; controls 36 px; text pages max 760 px, device pages fluid to 1440 px; fields with a subtle border plus a lighter fill (accepted below the WCAG 3:1 control-edge target); all other open points as the draft's option A. (2026-11-16) | Adopted after the phase 3 hide/layout/frame work is merged, in the draft's migration order. |
| 31 | Conflicts at startup and autostart (amends #12) | On every start CaseControl scans for conflicts. RGB suites (OpenRGB, SignalRGB and similar full suites) that affect a detected, non-hidden device are listed in a start dialog saying CaseControl will close them: per program "Close now" (running processes and services) and "Turn off autostart", both preselected; buttons Proceed and Not now; per program "Don't ask again" (it then only shows on the Conflicts page). Vendor tools (G HUB, Wootility, MSI Center, LampArray service ...) never appear in the dialog, only on the Conflicts page. Autostart is turned off reversibly: app entries through the same StartupApproved switch Task Manager uses, services set to Manual; the Conflicts page offers Re-enable. While a dialog-eligible program still has autostart on, the app shows a persistent hint. Nothing is closed or changed without the user's click; there is no automatic proceed. (2026-11-16) | One Proceed causes at most one UAC prompt. Software that affects no detected device is ignored. |
| 29 | Window frame | No Windows title bar. App name at the top of the sidebar, minimise / maximise / close at the top right over the content, the top edge drags the window. Close hides to tray as before. (2026-11-16) | |

No owner decisions are open. Implementation starts only after the owner's explicit go.

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

No owner decisions are open. Implementation starts only after the owner's explicit go.

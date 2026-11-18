# Notices

## CaseControl

Copyright (C) 2026 Maximilian Kielholz

CaseControl is free software: you can redistribute it and/or modify it under the terms of the GNU General Public
License as published by the Free Software Foundation, either version 2 of the License, or (at your option) any later
version (`GPL-2.0-or-later`). It is distributed WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See `LICENSE` for the full text of the GNU General Public
License, version 2.

## OpenRGB

The device drivers are modified ports of OpenRGB, Copyright (C) Adam Honse and contributors,
https://gitlab.com/CalcProgrammer1/OpenRGB, licensed under GPL-2.0-or-later. The ported code was rewritten in Rust;
protocol constants, packet layouts, board tables and LED layouts were taken from the OpenRGB sources (upstream commit
`4f2d8920d8e187e8e47b36ebe3c12301e23eda37`, a reference copy is in `reference/openrgb/`). Each ported file starts with
a line naming OpenRGB as its origin.

| CaseControl file(s) (modified ports) | OpenRGB source |
| --- | --- |
| `crates/cc-drivers/src/msi_mystic_light/{mod,controller,protocol,boards}.rs` | `Controllers/MSIMotherboardController/` (`MSIMotherboardControllerDetect.cpp`, `MSIMotherboard185Controller/*`, `MSIMotherboardCommon.h`) |
| `crates/cc-drivers/src/wooting/{mod,controller,protocol,layout}.rs` | `Controllers/WootingKeyboardController/` (`WootingKeyboardController*`, `WootingV3KeyboardController/*`, `RGBController_WootingKeyboard.*`) |
| `crates/cc-drivers/src/logitech/{mod,hidpp,lightspeed,g560}.rs` | `Controllers/LogitechController/` (`LogitechControllerDetect.cpp`, `LogitechProtocolCommon.*`, `LogitechHIDPP20Controller/*`, `LogitechLightspeedController/*`) |
| `crates/cc-drivers/src/nvidia_illumination/{mod,controller,table,mapping}.rs` | `Controllers/NVIDIAIlluminationController/` (`NVIDIAIlluminationV1Controller_*`, `RGBController_NVIDIAIllumination_*`, `NVIDIAIlluminationControllerDetect_*`, `nvapi_accessor_*`), `pci_ids/pci_ids.h` |
| `crates/cc-nvapi/src/{raw,model,win}.rs` | `dependencies/NVFC/nvapi.h`, `nvapi.cpp` (NvAPI illumination ABI as used by OpenRGB) |

The device model in `crates/cc-core` follows the semantics of OpenRGB's `RGBController` (zones, LEDs, modes) without
copying its code.

## Lucide

The eyedropper icon in `apps/desktop/src/components/ColorPicker.vue` follows the "pipette" icon of Lucide
(https://lucide.dev), licensed under the ISC License:

> ISC License
>
> Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part of Feather (MIT). All other copyright
> (c) for Lucide are held by Lucide Contributors 2022.
>
> Permission to use, copy, modify, and/or distribute this software for any purpose with or without fee is hereby
> granted, provided that the above copyright notice and this permission notice appear in all copies.
>
> THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS SOFTWARE INCLUDING ALL
> IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
> INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN
> AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR
> PERFORMANCE OF THIS SOFTWARE.

Third-party Rust crates and npm packages keep their own licences; `deny.toml` lists the allowed ones.

# OpenRGB reference sources (GPL-2.0, upstream commit 4f2d8920d8e187e8e47b36ebe3c12301e23eda37, 2026-11-04)

Copied for protocol reference while porting to Rust. CaseControl is published under GPL-2.0-or-later and its driver
modules are modified ports of these sources; see `NOTICE.md` at the repository root and ADR 0001 #4, #39.
Upstream: https://gitlab.com/CalcProgrammer1/OpenRGB (mirrored at https://github.com/CalcProgrammer1/OpenRGB).

`dependencies/NVFC/` (`nvapi.h`, `nvapi.cpp`) was removed from this copy: the files carry no licence header and are
available upstream. `crates/cc-nvapi` names the upstream files it was ported from.

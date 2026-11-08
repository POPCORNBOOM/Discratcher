<p align="center">
  <img src="docs/banner.webp" alt="Discratcher" width="100%">
</p>

<p align="center">
  <strong>Fully automatic calibration with a single disc, no photos needed | Use and contribute community calibration data | An improved algorithm for more successful burns</strong>
</p>

<p align="center">
  <a href="README.md">简体中文</a> | English
</p>

<p align="center">
  <a href="https://github.com/POPCORNBOOM/Discratcher/actions/workflows/ci.yml"><img src="https://github.com/POPCORNBOOM/Discratcher/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0--or--later-9045f9" alt="GPL-3.0-or-later"></a>
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-555" alt="Platform">
</p>

<p align="center">
  <img src="docs/carve.webp" alt="Preview in the app" height="300">
  &nbsp;
  <img src="docs/print.webp" alt="The burned disc" height="300">
</p>
<p align="center">
  <sub>Left: the preview in the app. Right: the result after a single automatic calibration.</sub>
</p>

> [!NOTE]
> Discratcher is at an early stage and **supports DVD discs only** for now; CD and Blu-ray are not supported yet. The combinations tested so far are under [Platform support](#platform-support), and [issues](https://github.com/POPCORNBOOM/Discratcher/issues) are welcome. To draw on a CD, try [DiscImageStudio](https://github.com/JiaFeiMiao-K-Cat/DiscImageStudio) for now.

## Features

<table>
  <tr>
    <td width="40%" valign="top">
      <h3>Automatic calibration</h3>
      <p>No manual measuring. The drive's own read timing reveals the disc's spiral geometry.</p>
    </td>
    <td width="60%"><img src="docs/feature-calibrate.en.webp" alt="Calibration page" width="100%"></td>
  </tr>
  <tr>
    <td width="40%" valign="top">
      <h3>Community calibration library</h3>
      <p>Enter the code printed on the disc's inner ring to download someone else's measurements, or share your own with people using the same disc.</p>
    </td>
    <td width="60%"><img src="docs/feature-library.en.webp" alt="Library page" width="100%"></td>
  </tr>
  <tr>
    <td width="40%" valign="top">
      <h3>Drive protection</h3>
      <p>Caps the share of dark bytes and breaks up runs of them, so the drive does not lose tracking mid-burn.</p>
    </td>
    <td width="60%"><img src="docs/feature-protect.en.webp" alt="Advanced settings on the carve page" width="100%"></td>
  </tr>
</table>

The core logic does not depend on a window framework, and drive access is implemented per platform, so Discratcher is cross-platform.

## Quick start

1. Find your disc: enter the code printed on its inner ring and download the calibration.
2. Pick an image and press burn.
3. If your disc is not listed, calibrate it once (this uses up a disc), then share the result.

## Updates

The app looks for a new version at startup, or you can use Help → Check for updates. Updates are signature-checked and the app restarts after installing. On Linux, only the AppImage can update itself.

## Platform support

| Platform | Status |
| --- | --- |
| macOS + LG GUD1N | Tested |
| Other drives on macOS | Unverified |
| Windows 11 + LG GUD1N | Tested |
| Linux | Implemented, not tested on hardware |

| Disc | Status |
| --- | --- |
| DVD-R | Tested (CMC MAG. AM3) |
| DVD+R, DVD-RW, DVD+RW | Should work, unverified |
| CD-R, CD-RW | Not supported yet: calibration and carving follow the DVD format |
| Blu-ray, DVD-RAM, dual-layer DVD | Not supported |

## Build from source

```bash
pnpm install
pnpm tauri dev    # desktop app
pnpm dev          # UI only, with drive commands mocked
pnpm tauri build
cargo test -p disc-core --release
```

The core logic lives in `crates/disc-core` and does not depend on Tauri; `src-tauri` is only a command shell.

## Calibration library

Calibration data comes from issues labelled `calibration` in this repository. When they change, a workflow publishes them to the `catalog` branch, and the app reads that file with no account needed.

- **Contribute**: you must burn a real disc with your calibration before sharing it. The app opens a pre-filled issue form, you confirm the submission on GitHub, and the app stores no token. You can drop the source image and a photo of the result into the form.
- **Rate**: after burning with someone else's calibration, you can give it 1–5 stars, also through a pre-filled issue (label `rating`). Only an account's latest vote on each calibration counts.
- **Review**: adding the `verified` label makes a calibration searchable within minutes; closing the issue withdraws it.

## Translation

UI text lives in `src/i18n/locales/`, one JSON file per language. Simplified Chinese and English are available today.

To add a language, copy `zh-CN.json`, rename it to the language code (for example `ja.json`), set `_name` to the language's own name, and translate the other values. The file then appears under Help → Language, and missing entries fall back to Simplified Chinese. Code reads text with `t('key')`; keep placeholders such as `{name}` as they are.

## Technical notes

Discratcher exists to make calibration simpler for DVD-R image burning, with the same graphical app on macOS, Windows and Linux. The measuring method comes from the published approaches of [disc-probe](https://github.com/Ashley-QwQ/disc-probe) and [discprobe-macos](https://github.com/NaiveTomcat/discprobe-macos).

<details>
<summary><strong>Problems hit while measuring discs</strong></summary>

- On macOS, some USB drives (such as the LG GUD1N) reject the MMC interface and cannot be opened. The app now falls back to the SCSITask user client.
- The default range covered only 3%–85% of the disc, leaving about 340,000 outer sectors unmeasured. The range is now 2%–96% (25.29–56.91 mm), and the constant-pitch residual is about 2.9° peak to peak beyond 44 mm, with about 2.0° of variation at 25–31 mm too (one 56-rung run on one drive and one disc).
- Preset export fitted ideal constant-pitch points rather than measurements, so the cubic coefficient always stayed near 0. The fit now uses the measured unwrapped angles directly, lowering the whole-disc residual RMS from 0.715° to 0.402°.
- The default total sector count was 2,298,496, which is 3,392 more than a DVD-R's actual 2,295,104 and makes the outer diameter about 0.035 mm too large. It now holds the actual value.
- Burned discs with no file system could not be opened because the exclusive lock was unavailable (0xE00002D5). Reads now go through `/dev/rdiskN` with `F_NOCACHE`, which needs no exclusive access.

</details>

<details>
<summary><strong>Problems hit while burning images</strong></summary>

- Dark pixels were filled with continuous `0xA5`, which drives cannot sustain. With large black areas the laser stays at full duty cycle for too long, trips the write power calibration protection, and reports a write error at around 15%, ruining the disc.
- Inverting the background is not enough either. With thick black lines, 99.7% of a sector is continuous `0xA5`, so the drive loses the pre-groove tracking, writes bad blocks, and jumps 179 sectors ahead along the spiral. Sectors span different fractions of a turn on the inner and outer tracks, so after the jump the whole image twists by about 2.4 extra turns.
- The cap is 65% dark bytes by default with runs broken up; tests require runs of at most 3.

</details>

## Prior work and acknowledgements

Measuring sector angles from read timing did not start here. The acknowledgements of [disc-probe](https://github.com/Ashley-QwQ/disc-probe) collect the earlier work:

- DPM (Data Position Measurement) in disc copy protection;
- a 2025 redump forum discussion on timing a full turn by re-reading one sector with FUA;
- Philips patent US7538787B2, a method of writing a label on an optical disc.

This project builds on these open source works:

- [disc-probe](https://github.com/Ashley-QwQ/disc-probe) (GPL-3.0-or-later): the open source implementation of read-timing calibration. Discratcher's step-by-step measurement, spindle settling and some constants draw on it.
- [discprobe-macos](https://github.com/NaiveTomcat/discprobe-macos) (GPLv3): the macOS port of disc-probe. Its public data serves as a test reference here.
- [DiscImageStudio](https://github.com/JiaFeiMiao-K-Cat/DiscImageStudio) (GPL-3.0-or-later): the spiral model and encoding for data-side images draw on it.

Thanks to the authors of these projects, and to everyone who reported measurements to them.

## License

Copyright (C) 2026 Yigu Wang

This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. It is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY, without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See [LICENSE](LICENSE) and [NOTICE](NOTICE). SPDX: `GPL-3.0-or-later`.

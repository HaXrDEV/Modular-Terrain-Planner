<p align="center">
  <img src="docs/banner-light.svg#gh-light-mode-only" alt="Modular Terrain Planner" width="100%">
  <img src="docs/banner-dark.svg#gh-dark-mode-only" alt="Modular Terrain Planner" width="100%">
</p>

<p align="center">
  A desktop app for laying out dungeons from modular terrain STL files,<br>
  then printing exactly the tiles the layout needs.
</p>

<p align="center">
  <a href="https://github.com/HaXrDEV/modular-terrain-planner/releases/latest"><img src="https://img.shields.io/github/v/release/HaXrDEV/modular-terrain-planner?include_prereleases&style=flat-square&label=version&color=c97d1f" alt="Version"></a>
  <a href="https://github.com/HaXrDEV/modular-terrain-planner/actions/workflows/checks.yml"><img src="https://img.shields.io/github/actions/workflow/status/HaXrDEV/modular-terrain-planner/checks.yml?branch=main&style=flat-square&label=checks" alt="Checks"></a>
  <img src="https://img.shields.io/badge/platform-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux-555?style=flat-square" alt="Windows, macOS and Linux">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-lightgrey?style=flat-square" alt="MIT license"></a>
</p>

<p align="center">
  <a href="#features">Features</a> ·
  <a href="#install">Install</a> ·
  <a href="#usage">Usage</a> ·
  <a href="#development">Development</a>
</p>

<p align="center">
  <img src="docs/screenshot-light.png#gh-light-mode-only" alt="A two-level dungeon of flagstone floors, brick walls, doors and stairs in Modular Terrain Planner" width="900">
  <img src="docs/screenshot-dark.png#gh-dark-mode-only" alt="A two-level dungeon of flagstone floors, brick walls, doors and stairs in Modular Terrain Planner" width="900">
</p>

## Features

| Feature | What it does |
| :--- | :--- |
| **Tile library** | Opens folders of STL tiles, such as OpenForge or Dragonlock, sized from each file |
| **Layout** | Paint, fill, turn and stack tiles in 2D or 3D, across levels and over a battle map |
| **Smart tools** | Stamps, tile variants, walls around a room and a measuring tool |
| **Print list** | What to print after the tiles you own, with filament, cost and build plate estimates |
| **Build plan** | A PDF with a numbered map and every placement, plus a PNG map, a CSV and the STL files |

- **Fast.** Thousands of tiles stay smooth, and a library opens instantly the second time.
- **Safe.** Saves are atomic, unsaved work survives a crash, and tiles with a missing STL are kept.
- **Portable.** A project carries its STL files and battle map, and opens complete on any computer. No account, no telemetry.

The grid is 12.5 mm, so a 1" battle map square is 2 × 2 cells.

## Install

Download the latest version from [Releases](https://github.com/HaXrDEV/modular-terrain-planner/releases/latest).

| Platform | Download |
| :--- | :--- |
| Windows 10 and 11 | Installer (`.exe`) or portable `.zip` |
| macOS 11 and later | Universal app (`.zip`) |
| Linux (glibc 2.35 and later) | `.AppImage` or `.tar.gz` |

> [!NOTE]
> The downloads aren't code-signed. On first launch, choose **More info › Run anyway** in Windows SmartScreen, or **Open Anyway** under **System Settings › Privacy & Security** on macOS.

Needs a graphics card with Vulkan, DirectX 12, Metal or OpenGL 3.3. If the view stays blank, choose OpenGL in **View › Settings**. Projects from version 0.4 and earlier open unchanged.

## Usage

1. Add a folder of STL tiles with **File › Add tile folder**, or try the sample tiles.
2. Pick a tile in the library, then click or drag on the grid to place it.
3. Export the build plan with <kbd>Ctrl</kbd>+<kbd>E</kbd>.

<kbd>Ctrl</kbd>+<kbd>K</kbd> finds any command or tile, and <kbd>F1</kbd> lists every shortcut. On macOS, use <kbd>Cmd</kbd> for <kbd>Ctrl</kbd>.

<details>
<summary><b>Common shortcuts</b></summary>
<br>

| Action | Shortcut |
| :--- | :--- |
| Select, erase, measure | <kbd>V</kbd> <kbd>X</kbd> <kbd>M</kbd> |
| Turn | <kbd>Q</kbd> <kbd>E</kbd> |
| Fill a rectangle, draw a line | <kbd>Shift</kbd>+drag, <kbd>Shift</kbd>+click |
| Recent tiles | <kbd>1</kbd> to <kbd>9</kbd> |
| Next variant of a tile | <kbd>C</kbd> |
| Place off the grid | Hold <kbd>Ctrl</kbd> |
| Pick up a placed tile | <kbd>Alt</kbd>+click |
| Level up or down | <kbd>Page Up</kbd> <kbd>Page Down</kbd> |
| Switch 2D and 3D | <kbd>T</kbd> |
| Orbit, pan, zoom | Right drag, middle drag, wheel |

</details>

## Development

Install [Rust](https://rustup.rs), then:

```bash
cargo run --release
cargo test --workspace
```

[How it works](docs/how-it-works.md) covers loading, the placement rules and drawing. [The project format](docs/project-format.md) describes `.mtp` files, and [Contributing](CONTRIBUTING.md) the branch and pull request routine.

## License

[MIT](LICENSE). The build plan font, Cantarell, is under the [SIL Open Font License](assets/fonts/OFL.txt).

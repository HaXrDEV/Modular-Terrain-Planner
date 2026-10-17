<p align="center">
  <img src="docs/banner-light.svg#gh-light-mode-only" alt="Modular Terrain Planner" width="100%">
  <img src="docs/banner-dark.svg#gh-dark-mode-only" alt="Modular Terrain Planner" width="100%">
</p>

<p align="center">
  Lay out a dungeon from your modular terrain STL files,<br>
  then print exactly the tiles it needs and build it from the plan.
</p>

<p align="center">
  <a href="https://github.com/HaXrDEV/modular-terrain-planner/releases/latest"><img src="https://img.shields.io/github/v/release/HaXrDEV/modular-terrain-planner?include_prereleases&style=flat-square&label=version&color=c87020" alt="Version"></a>
  <a href="https://github.com/HaXrDEV/modular-terrain-planner/actions/workflows/checks.yml"><img src="https://img.shields.io/github/actions/workflow/status/HaXrDEV/modular-terrain-planner/checks.yml?branch=main&style=flat-square&label=checks" alt="Checks"></a>
  <img src="https://img.shields.io/badge/Windows%20%C2%B7%20macOS%20%C2%B7%20Linux-555?style=flat-square" alt="Windows, macOS and Linux">
  <a href="https://www.rust-lang.org"><img src="https://img.shields.io/badge/Rust-1.95%2B-dea584?style=flat-square&logo=rust&logoColor=white" alt="Rust 1.95 or newer"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-lightgrey?style=flat-square" alt="MIT license"></a>
</p>

<p align="center">
  <a href="#features">Features</a> ·
  <a href="#install">Install</a> ·
  <a href="#using-it">Using it</a> ·
  <a href="#files-and-privacy">Files and privacy</a> ·
  <a href="#development">Development</a>
</p>

<p align="center">
  <img src="docs/screenshot-light.png#gh-light-mode-only" alt="A dungeon of stone floors and brick walls in Modular Terrain Planner" width="900">
  <img src="docs/screenshot-dark.png#gh-dark-mode-only" alt="A dungeon of stone floors and brick walls in Modular Terrain Planner" width="900">
</p>

## Features

| Part | What it does |
| :--- | :--- |
| **Library** | Opens folders of STL files (OpenForge, Dragonlock and the like), with pictures, search and footprint filters. Each tile's size in grid cells comes from the file, inches included. |
| **Layout** | Place, paint, move, turn, copy and stack tiles on a grid in 2D or 3D. Tiles stack only on a level surface, and a red ghost says why a spot won't work. |
| **Battle map** | Lays an image under the grid to build on. |
| **Build plan** | A PDF with a numbered map that shows which way each tile faces, a print list per STL file and a table of every placement. Also a PNG map and a CSV print list. |

- **Fast.** Thousands of tiles stay smooth, far-away tiles are drawn with less detail, and a library opens instantly the second time.
- **Nothing is lost.** Tiles whose STL is missing are kept and saved as they are, saves never leave a half-written file, and unsaved work is recovered after a crash.
- **Projects travel.** Paths are saved relative to the project, so a project and its tiles can move together, also between Windows, macOS and Linux.

1 grid cell is 12.5 mm, so a 1" battle-map square is 2 × 2 cells.

## Install

Download the file for your system from the [latest release](https://github.com/HaXrDEV/modular-terrain-planner/releases/latest):

| System | File |
| :--- | :--- |
| Windows 10 or 11 | `…-windows-x64-setup.exe` installs it for you (no administrator rights), or unzip `…-windows-x64.zip` anywhere |
| macOS 11 or newer | Unzip `…-macos-universal.zip` and move the app to Applications |
| Linux (glibc 2.35 or newer) | `chmod +x` the `…-linux-x86_64.AppImage` and run it, or unpack `…-linux-x64.tar.gz` and run `./install.sh` |

The downloads aren't code-signed, so the first start asks once:

- **Windows:** SmartScreen protected your PC. Choose *More info*, then *Run anyway*.
- **macOS:** Apple can't check the app. Open *System Settings > Privacy & Security* and choose *Open Anyway*.

The app draws with Vulkan, DirectX 12, Metal or OpenGL 3.3, so most graphics cards from the last ten years work. If the view stays blank, update the graphics driver, or pick *OpenGL* under *View > Settings > Graphics*.

Projects from the Python version (0.4 and earlier) open as they are. That version is tagged [`python-final`](https://github.com/HaXrDEV/modular-terrain-planner/tree/python-final).

## Using it

Add a folder of STL files with **File > Add tile folder** (or drop it onto the window), or try the sample tiles. Click a tile in the library, then click or drag on the grid to place it.

| To | Do this |
| :--- | :--- |
| Place, select, erase | Click a tile in the library, press **V** for Select, **X** for Erase |
| Turn | **Q** and **E** |
| Move the selection | Drag it, or use the arrow keys |
| Place freely, off the grid | Hold **Ctrl** |
| Pick up a placed tile | **Alt**+click it |
| Look around | Right drag to orbit, middle drag to pan, wheel to zoom, **T** for 2D or 3D |
| Export the build plan | **Ctrl+E** |

**Help > Keyboard and mouse** lists everything else.

## Files and privacy

The app never uses the network. Projects are `.mtp` files (JSON) wherever you save them. Settings, logs, autosaves and the mesh cache stay in the app's own folders, which *Help > About* shows:

- Windows: `%APPDATA%` and `%LOCALAPPDATA%\Modular Terrain Planner`
- macOS: `~/Library/Application Support` and `~/Library/Caches`
- Linux: `~/.config`, `~/.local/share` and `~/.cache`

## Development

Install [Rust](https://rustup.rs), then:

```bash
cargo run
```

```bash
cargo test --workspace
```

[How it works](docs/how-it-works.md) explains loading, the placement rules and drawing. [The project format](docs/project-format.md) describes `.mtp` files, and [Contributing](CONTRIBUTING.md) the branch and pull request routine.

## License

[MIT](LICENSE). The build plan font, Cantarell, is under the [SIL Open Font License](assets/fonts/OFL.txt).

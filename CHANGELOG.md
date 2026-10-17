# Changelog

Release notes on GitHub list every pull request. This is the short version.

## 0.5.0 (unreleased)

The app is rebuilt in Rust. Projects from earlier versions open as they are;
saving writes format version 5, which earlier versions can't open.

### New

- A tile library that stays open through New and Open, with pictures,
  search, footprint filters, subfolders and sample tiles. It loads in the
  background and opens instantly the second time.
- Tools: Select, Place (click or drag to paint, Shift+drag to fill a
  rectangle, Shift+click for a line), Erase and Paste. Keys 1 to 9 pick a
  recently placed tile, Alt+click picks up a placed one, Q and E turn
  either way and arrows nudge. Right click opens a menu.
- A 2D top-down view (the default) and a 3D view, zooming toward the
  pointer.
- A red, hatched ghost and a reason when a tile can't go somewhere.
- Build plan PDF with a vector map, detail pages for big layouts, a print
  list per STL file with a filament estimate and a table of every
  placement, on A4, A3, Letter, Legal or Tabloid.
- Autosave and recovery after a crash, `.bak` copies, crash reports.
- Installers for Windows, macOS (universal) and Linux (AppImage and tar.gz).

### Fixed

- Opening and saving no longer drops tiles: overlapping, off-grid and
  missing-STL tiles, and missing battle maps, are kept.
- One broken STL file no longer hangs loading.
- Stacking checks the whole footprint, collisions compare heights, and
  moving, pasting and turning follow the same rules as placing.
- Print lists no longer merge same-named files from two packs, arrows on
  the map point the right way, and every placement is listed.
- Tile colours stay the same between launches.
- The macOS build renders.

## 0.4.1 and earlier

The Python version. See the [GitHub releases](https://github.com/HaXrDEV/modular-terrain-planner/releases)
and the `python-final` tag.

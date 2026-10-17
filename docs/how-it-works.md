# How it works

The app is a Cargo workspace. Each crate owns one part, and the crates below
the app know nothing about the UI.

| Crate | Owns |
| :--- | :--- |
| `crates/mtp-core` | Units, footprints and turning, the cell index, the placement rule, undo history, the document, `.mtp` files and atomic saves. No GPU or UI code. |
| `crates/mtp-stl` | Reading STL files, sizing tiles, simplified levels of detail, the mesh cache and folder scanning. |
| `crates/mtp-render` | The wgpu renderer, the camera, picking and thumbnails. |
| `crates/mtp-export` | The build plan PDF, the PNG map and the CSV print list. |
| `app` | The window: panels, tools, menus, settings, file dialogs and logging. |
| `tools/icons` | Renders the app icon from its SVG. |

## Loading a tile library

A folder is scanned without glob patterns, so folders with brackets in their
name and files ending in `.Stl` are found. Files load in parallel on
background threads. Each file reports one result, and a broken file becomes a
line in the Problems panel while the rest of the folder loads.

For one file:

1. **Read once.** Binary files are recognised by their size (84 bytes plus 50
   per triangle), because many begin with `solid` like ASCII files do.
   Truncated, empty and non-STL files are refused with a reason.
2. **Size.** The bounding box gives the footprint: whole cells of 12.5 mm,
   rounded half to even like the Python version did. If the larger side is
   under 5 units, the file is taken to be in inches and scaled by 25.4.
   Heights are kept in fractions of a cell.
3. **Normalise and weld.** Every axis is scaled to 0..1, and corners that
   share a position are merged into an indexed mesh. The printed volume is
   measured for filament estimates.
4. **Simplify.** [meshoptimizer](https://github.com/zeux/meshoptimizer) builds up
   to five coarser tiers, each from the one before, stopping when a tier
   would barely shrink.
5. **Cache.** The result is stored in the mesh cache, keyed by path, size,
   modification time and loader version, so the next start skips 1 to 4.

## Placing tiles

Every edit (placing, painting, moving, pasting, turning, replacing) asks one
function, `rules::arrange`, where its tiles land:

- The whole footprint must be on the grid.
- A tile's base sits on the highest surface under its footprint, and that
  surface must be level. Half on a floor, half beside it, is refused.
- Tiles may not overlap: two footprints sharing a cell must not share any
  height.

Holding Ctrl (free placement) keeps only the grid-edge check. Groups settle
bottom first, so a wall moved together with the floor under it stays on it.
A refused edit changes nothing and leaves no undo step.

Turning a group happens about a fixed whole-cell pivot, kept while the same
selection is turned again, so whole cells stay whole and four turns bring the
group back.

## Undo

The history keeps whole document states (shared, so a step costs one copy of
the tile list) with a mark for the saved state, so undoing back to it clears
the unsaved star. When a tile's STL loads after a project was opened, its real
size is written into every undo step, so undo can't bring back the guess.

## Drawing

The renderer draws into offscreen textures that the UI shows:

1. Ground, battle map and grid, in that order with depth writes off.
2. Placed tiles, instanced. Each tile gets the tier whose triangle count is
   closest to a quarter of the pixels it covers, and only tiers that are drawn
   are uploaded. A coarser tier on the GPU stands in until a finer one
   arrives. Tiles whose STL isn't loaded are drawn as hatched boxes.
3. Ghosts for placements and moves, red and hatched when refused.
4. The selection, into a mask that a last pass turns into an outline.

Only features every backend has are used, so the OpenGL fallback works: per
instance data comes from a vertex buffer bound at an offset, with no storage
buffers or compute.

Picking tests every tile's box (a few microseconds for thousands), then the
exact triangles of the candidates, nearest first, so clicks pass through the
empty parts of a tile's box.

## Build plans

`mtp-export` turns the document into one map layout that both the PDF and
the PNG draw, through one drawing routine, so they can't disagree. IDs follow
reading order on the printed map. Print lists have one row per STL file.

## One window

The first instance listens on a local socket (a named pipe on Windows, an
abstract socket on Linux, a socket file on macOS), named per user. A later
launch connects, sends its file paths one per line and exits, and the open
window opens them and comes to the front. Double-clicking a project on
macOS reaches the app as an Apple event instead of a launch argument, which
it doesn't handle yet.

## Tests

- Golden files: `tools/golden.py` ran the Python version (tag `python-final`)
  on the fixtures in `tests/fixtures` and recorded STL sizes, project reading
  and model matrices in `tests/golden/python.json`. The Rust tests compare
  against them.
- Rendering: `crates/mtp-render/tests/headless.rs` draws real frames. CI runs
  it on lavapipe and llvmpipe (Linux), WARP (Windows) and Metal (macOS).
- `modular-terrain-planner --self-test` checks a built app end to end without
  a window. The release workflow runs it on every download before
  publishing.

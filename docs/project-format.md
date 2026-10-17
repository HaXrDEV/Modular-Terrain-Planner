# The `.mtp` project format

A project is a UTF-8 JSON file. The app writes version 5 and reads every
version since 1.

## Version 5

```json
{
  "format": "Modular Terrain Planner project",
  "version": 5,
  "min_reader_version": 5,
  "app_version": "0.5.0",
  "grid": { "cols": 80, "rows": 80 },
  "folders": [
    { "path": "../Tiles/OpenForge", "absolute": "C:/Tiles/OpenForge", "recursive": true }
  ],
  "tiles": [
    {
      "folder": 0,
      "file": "floors/floor_2x2.stl",
      "x": 4, "y": 6, "z": 0,
      "rotation": 90,
      "level": 1,
      "size": { "w": 2, "h": 2, "gz": 0.48 }
    }
  ],
  "ground_image": { "path": "maps/cave.png", "absolute": "C:/Maps/cave.png", "rect": [0, 0, 80, 60] }
}
```

| Field | Meaning |
| :--- | :--- |
| `version` | The format of this file. |
| `min_reader_version` | The oldest reader that can open it. A newer file opens with a warning while this is 5 or lower. |
| `grid` | Grid size in cells (1 to 2000 each way). One cell is 12.5 mm. |
| `folders` | Tile folders. `path` is relative to the project's folder when it can be (otherwise absolute) and `absolute` is where the folder was when saved. Forward slashes on every system. |
| `tiles[].folder`, `file` | Which STL file: an index into `folders` and a path inside that folder. |
| `x`, `y` | The tile's corner nearest the grid's origin, in cells. Whole numbers unless placed freely. |
| `z` | Height of the tile's base, in cells. |
| `rotation` | 0, 90, 180 or 270: counter-clockwise, seen from above. |
| `level` | The floor of a multi-storey layout. Left out when 0. |
| `size` | Footprint and height in cells, as last read from the STL. Lets a tile whose STL is missing keep its place and size. |
| `ground_image` | The battle map: `rect` is `[x, y, width, depth]` in cells. |

When opening, each folder is looked for relative to the project first, then
at `absolute`. If neither exists, the app asks where it is now. Tiles from a
folder that isn't found are kept, drawn as grey boxes and written back
unchanged.

A damaged record (a missing field, a position that isn't a number) is left
out with a warning in the Problems panel. The rest of the file still opens.

## Older versions

The Python app wrote versions 1 to 4, with absolute paths only.

| Version | Change |
| :--- | :--- |
| 1 | `folders` (paths) and `tiles` with `stl_path`, `grid_x`, `grid_y`, `rotation` |
| 2 | Adds `grid_cols` and `grid_rows` (40 × 40 before) |
| 3 | Adds `z_offset` per tile (0 before) |
| 4 | Adds `ground_image` with `path` and `rect` |

These are read exactly as the Python app read them. The golden tests in
`crates/mtp-core/tests/projects.rs` check this against its output. Saving
writes version 5, which the Python app can't open.

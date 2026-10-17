"""Writes the test fixtures in tests/fixtures: a small corpus of STL files
(good, unusual and broken) and project files from every format version.

Run with any Python 3.11+, no packages needed:

    python tools/make_fixtures.py

The output is committed; rerun only when adding a fixture.
"""
import json
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent / "tests" / "fixtures"


def box(x0, y0, z0, x1, y1, z1):
    """12 triangles of an axis-aligned box, wound counter-clockwise from outside."""
    p = [(x, y, z) for z in (z0, z1) for y in (y0, y1) for x in (x0, x1)]
    quads = [(0, 2, 3, 1), (4, 5, 7, 6), (0, 1, 5, 4), (2, 6, 7, 3), (0, 4, 6, 2), (1, 3, 7, 5)]
    tris = []
    for a, b, c, d in quads:
        tris.append((p[a], p[b], p[c]))
        tris.append((p[a], p[c], p[d]))
    return tris


def normal(t):
    (ax, ay, az), (bx, by, bz), (cx, cy, cz) = t
    ux, uy, uz = bx - ax, by - ay, bz - az
    vx, vy, vz = cx - ax, cy - ay, cz - az
    n = (uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx)
    length = sum(c * c for c in n) ** 0.5 or 1.0
    return tuple(c / length for c in n)


def binary(tris, header=b"binary STL written by make_fixtures.py"):
    out = bytearray(header.ljust(80, b" ")[:80])
    out += struct.pack("<I", len(tris))
    for t in tris:
        out += struct.pack("<3f", *normal(t))
        for v in t:
            out += struct.pack("<3f", *v)
        out += b"\0\0"
    return bytes(out)


def ascii_stl(tris, name="fixture"):
    lines = [f"solid {name}"]
    for t in tris:
        lines.append("  facet normal {:e} {:e} {:e}".format(*normal(t)))
        lines.append("    outer loop")
        for v in t:
            lines.append("      vertex {:e} {:e} {:e}".format(*v))
        lines.append("    endloop")
        lines.append("  endfacet")
    lines.append(f"endsolid {name}")
    return ("\n".join(lines) + "\n").encode()


def write(rel, data):
    path = ROOT / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def stl_corpus():
    # Readable files: sizing must match the Python version (tests/golden)
    write("stl/good/floor_2x2_mm.stl", binary(box(0, 0, 0, 25, 25, 6)))
    write("stl/good/wall_inch.stl", binary(box(0, 0, 0, 2, 0.5, 2)))
    write("stl/good/ascii_corner.stl", ascii_stl(box(0, 0, 0, 25, 25, 50), "corner"))
    write("stl/good/solid_header_binary.stl", binary(box(0, 0, 0, 37.5, 12.5, 25), header=b"solid exported by a CAD tool"))
    write("stl/good/offset_origin.stl", binary(box(-10, 40, 3, 15, 90, 9)))
    write("stl/good/tiny_mm_part.stl", binary(box(0, 0, 0, 4, 3, 2)))
    write("stl/good/flat_plate.stl", binary([((0, 0, 0), (25, 0, 0), (25, 25, 0)), ((0, 0, 0), (25, 25, 0), (0, 25, 0))]))
    write("stl/good/Mixed Case.Stl", binary(box(0, 0, 0, 50, 25, 12.5)))
    two = box(0, 0, 0, 25, 25, 6) + box(25, 0, 0, 50, 25, 6)
    write("stl/good/two_boxes_zero_normals.stl", zero_normals(binary(two)))
    write("stl/good/OpenForge [Stone]/stone_floor_1x1.stl", binary(box(0, 0, 0, 12.5, 12.5, 6)))
    write("stl/good/OpenForge [Stone]/notes.txt", b"Not an STL, the scanner skips it.\n")

    # Broken files: each is skipped with its own message while the rest load
    write("stl/bad/empty.stl", b"")
    good = binary(box(0, 0, 0, 25, 25, 6))
    write("stl/bad/truncated.stl", good[:84 - 4] + struct.pack("<I", 100) + good[84:84 + 50 * 3])
    write("stl/bad/not_an_stl.stl", b"<!DOCTYPE html><html><body>404 Not Found</body></html>\n")
    write("stl/bad/ascii_garbage.stl", b"solid broken\n  facet normal 0 0 1\n    outer loop\n      vertex 1 2\n")
    write("stl/bad/zero_triangles.stl", binary([]))


def zero_normals(data):
    """Same file with every stored normal set to zero, as many exporters write it."""
    out = bytearray(data)
    count = struct.unpack_from("<I", out, 80)[0]
    for i in range(count):
        struct.pack_into("<3f", out, 84 + i * 50, 0.0, 0.0, 0.0)
    return bytes(out)


def projects():
    lib = "C:/Tiles/OpenForge"
    write("projects/v1.mtp", json.dumps({
        "folders": [lib],
        "tiles": [
            {"stl_path": f"{lib}/floor.stl", "grid_x": 0, "grid_y": 0, "rotation": 0},
            {"stl_path": f"{lib}/wall.stl", "grid_x": 2, "grid_y": 0, "rotation": 90},
        ],
    }, indent=2).encode())
    write("projects/v2.mtp", json.dumps({
        "version": 2, "grid_cols": 60, "grid_rows": 30,
        "folders": [lib],
        "tiles": [{"stl_path": f"{lib}/floor.stl", "grid_x": 4, "grid_y": 6, "rotation": 180}],
    }, indent=2).encode())
    write("projects/v3.mtp", json.dumps({
        "version": 3, "grid_cols": 80, "grid_rows": 80,
        "folders": [lib, "D:/More Tiles"],
        "tiles": [
            {"stl_path": f"{lib}/floor.stl", "grid_x": 0, "grid_y": 0, "rotation": 0, "z_offset": 0.0},
            {"stl_path": f"{lib}/floor.stl", "grid_x": 0, "grid_y": 0, "rotation": 270, "z_offset": 0.48},
            {"stl_path": "D:/More Tiles/pillar.stl", "grid_x": 10.25, "grid_y": 3.5, "rotation": 90, "z_offset": 0.0},
        ],
    }, indent=2).encode())
    write("projects/v4.mtp", json.dumps({
        "version": 4, "grid_cols": 120, "grid_rows": 90,
        "folders": [lib],
        "tiles": [
            {"stl_path": f"{lib}/floor.stl", "grid_x": 2, "grid_y": 2, "rotation": 0, "z_offset": 0.0},
            {"stl_path": f"{lib}/floor.stl", "grid_x": 2, "grid_y": 2, "rotation": 0, "z_offset": 0.0},
        ],
        "ground_image": {"path": "C:/Maps/cave.png", "rect": [0.0, 0.0, 120.0, 90.0]},
    }, indent=2).encode())
    write("projects/v4_no_image.mtp", json.dumps({
        "version": 4, "grid_cols": 40, "grid_rows": 40, "folders": [], "tiles": [], "ground_image": None,
    }, indent=2).encode())
    write("projects/v4_windows_paths.mtp", json.dumps({
        "version": 4, "grid_cols": 40, "grid_rows": 40,
        "folders": [r"C:\Users\someone\Tiles\Dungeon"],
        "tiles": [{"stl_path": r"C:\Users\someone\Tiles\Dungeon\door.stl", "grid_x": 1, "grid_y": 1, "rotation": 90, "z_offset": 0.0}],
    }, indent=2).encode())
    # Files the Python version could not read: the Rust version must refuse
    # or repair them without crashing (asserted in the Rust tests, not golden)
    write("projects/bad/newer_version.mtp", json.dumps({"version": 99, "tiles": []}).encode())
    write("projects/bad/missing_field.mtp", json.dumps({"version": 4, "tiles": [{"grid_x": 1}]}).encode())
    write("projects/bad/wrong_types.mtp", json.dumps({"version": "4", "folders": "C:/x", "tiles": [3]}).encode())
    write("projects/bad/not_json.mtp", b"this is not json")
    write("projects/bad/odd_values.mtp", json.dumps({
        "version": 4, "grid_cols": 40, "grid_rows": 40, "folders": [],
        "tiles": [
            {"stl_path": "a.stl", "grid_x": 1, "grid_y": 1, "rotation": 45, "z_offset": 0},
            {"stl_path": "b.stl", "grid_x": "NaN", "grid_y": 1, "rotation": -90, "z_offset": 0},
            {"stl_path": "c.stl", "grid_x": 3, "grid_y": 1, "rotation": 450, "z_offset": 0},
        ],
        "ground_image": {"path": "x.png", "rect": [1]},
    }).encode())


if __name__ == "__main__":
    stl_corpus()
    projects()
    print(f"wrote fixtures under {ROOT}")

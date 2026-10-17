"""Records what the Python version (tag python-final) does with the test
fixtures, so the Rust tests can prove the rewrite reads and sizes things the
same way. Writes tests/golden/python.json.

Run it against a checkout of python-final, with that version's packages
(numpy-stl, numpy, PySide6) installed:

    git worktree add ../mtp-python python-final
    python tools/golden.py --python-root ../mtp-python

Only behaviour that should stay identical is recorded: STL sizing, project
migration and reading, and tile footprints and model matrices. Behaviour the
rewrite changes on purpose (collisions, print-list keys, placement IDs, tile
colours) is tested directly in Rust instead.
"""
import argparse
import json
import math
import os
import shutil
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
FIXTURES = REPO / "tests" / "fixtures"
OUT = REPO / "tests" / "golden" / "python.json"


def stl_sizes(load_stl_folder):
    """Sizes each readable fixture on its own, from a plain temporary folder
    (the Python scanner can't see folders with [ ] in their name)."""
    sizes = {}
    good = FIXTURES / "stl" / "good"
    for path in sorted(p for p in good.rglob("*") if p.suffix.lower() == ".stl"):
        rel = path.relative_to(good).as_posix()
        with tempfile.TemporaryDirectory() as tmp:
            # The Python scanner only matches *.stl and *.STL
            shutil.copy(path, Path(tmp) / "tile.stl")
            errors = []
            defs = load_stl_folder(tmp, errors)
        if errors or not defs:
            sizes[rel] = {"error": errors[0] if errors else "no definitions"}
            continue
        d = defs[0]
        tris = d.view_triangles
        sizes[rel] = {
            "grid_w": d.grid_w,
            "grid_h": d.grid_h,
            "grid_z": round(float(d.grid_z), 6),
            "triangles": int(len(tris)),
            "normalised_min": [round(float(v), 6) for v in tris.reshape(-1, 3).min(axis=0)],
            "normalised_max": [round(float(v), 6) for v in tris.reshape(-1, 3).max(axis=0)],
        }
    return sizes


def bad_stl_outcomes():
    """What numpy-stl does with each broken file (for the record only)."""
    from stl import mesh as stl_mesh

    out = {}
    for path in sorted((FIXTURES / "stl" / "bad").glob("*.stl")):
        try:
            m = stl_mesh.Mesh.from_file(str(path))
            out[path.name] = f"read {len(m.vectors)} triangles"
        except BaseException as exc:  # noqa: BLE001 - recording every outcome
            out[path.name] = f"{type(exc).__name__}"
    return out


def projects(load_project):
    out = {}
    for path in sorted((FIXTURES / "projects").glob("*.mtp")):
        folders, tiles, cols, rows, image = load_project(str(path))
        out[path.name] = {
            "folders": folders,
            "grid_cols": cols,
            "grid_rows": rows,
            "tiles": [
                {"stl_path": t.stl_path, "grid_x": t.grid_x, "grid_y": t.grid_y,
                 "rotation": t.rotation, "z_offset": t.z_offset}
                for t in tiles
            ],
            "ground_image": None if image is None else {"path": image.path, "rect": image.rect},
        }
    return out


def footprints(TileDefinition, PlacedTile):
    out = []
    for w, h, gz in [(1, 1, 0.5), (2, 1, 4.0), (3, 2, 1.25)]:
        d = TileDefinition(name="t", stl_path="t.stl", grid_w=w, grid_h=h, grid_z=gz, color=None)
        for rotation in (0, 90, 180, 270):
            for x, y, z in [(0, 0, 0.0), (3, 5, 0.5), (2.5, 1.25, 2.0)]:
                pt = PlacedTile(definition=d, grid_x=x, grid_y=y, rotation=rotation, z_offset=z)
                m = pt.model_matrix()
                # QMatrix4x4.data() is column-major, like glam's to_cols_array()
                out.append({
                    "size": [w, h, gz], "rotation": rotation, "pos": [x, y, z],
                    "effective": [pt.effective_w, pt.effective_h],
                    "cells": [list(c) for c in pt.occupies()],
                    "model_cols": [round(float(v), 6) + 0.0 for v in m.data()],
                })
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--python-root", required=True, help="checkout of the python-final tag")
    args = parser.parse_args()
    root = Path(args.python_root).resolve()
    sys.path.insert(0, str(root))
    os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

    from models.placed_tile import PlacedTile
    from models.tile_definition import TileDefinition
    from persistence.project import load_project
    from stl_loader.loader import load_stl_folder

    golden = {
        "source": "python-final",
        "stl": stl_sizes(load_stl_folder),
        "stl_bad": bad_stl_outcomes(),
        "projects": projects(load_project),
        "footprints": footprints(TileDefinition, PlacedTile),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(golden, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {OUT.relative_to(REPO)}")


if __name__ == "__main__":
    main()

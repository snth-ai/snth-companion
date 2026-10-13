#!/usr/bin/env python3
"""Build the cursor looks SNTH Connect ships: one compiled theme per colour.

Cua Driver has no colour setting for its agent cursor. Its built-in theme
(`cua.default`) is recoloured per session by the driver itself, and an
installed theme keeps the colours it was authored with. So a fixed colour is
a theme of its own: this script takes the driver's own theme source
(`cua.default.lottie`, MIT, Cua AI, Inc.), swaps its one palette colour for
ours, gives it our id, and compiles it with the driver's own compiler
(`cua-cursor-theme`, which ships next to `cua-driver`).

The results are committed (`crates/connect-core/assets/cursor-themes/`) and
compiled into the app, so nothing is built on the owner's computer. Run this
again when the pinned driver version changes or a colour is added:

    python3 scripts/build-cursor-themes.py --compiler <dir>/cua-cursor-theme

`connectd selftest cua` checks every committed theme against the real pinned
driver (inspect, install into a private folder, select, read back).

Standard library only. Output is deterministic: same input, same bytes.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
import tempfile
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

REPO = Path(__file__).resolve().parent.parent
ASSETS = REPO / "crates" / "connect-core" / "assets" / "cursor-themes"
SOURCE = ASSETS / "cua.default.lottie"

# The palette key of the driver's default theme (Cua blue).
KEY = [94 / 255, 192 / 255, 232 / 255, 1]

# (look id, display name, colour). The look id is what settings.json stores
# and what `LOOKS` in `crates/connect-core/src/cua_look.rs` lists.
LOOKS = [
    ("sky", "SNTH Sky", "#5EC0E8"),
    ("violet", "SNTH Violet", "#B284FF"),
    ("rose", "SNTH Rose", "#F0569A"),
    ("green", "SNTH Green", "#3FCF8E"),
    ("amber", "SNTH Amber", "#F4B242"),
    ("red", "SNTH Red", "#E85262"),
    ("graphite", "SNTH Graphite", "#4B5160"),
]

THEME_VERSION = "1.0.0"
AUTHOR = "SNTH.AI, from Cua Default by Cua AI, Inc."


def theme_id(look: str) -> str:
    return f"ai.snth.connect.{look}"


def rgba(colour: str) -> list[float]:
    value = colour.lstrip("#")
    return [int(value[i : i + 2], 16) / 255 for i in (0, 2, 4)] + [1]


def is_key(value: object) -> bool:
    return (
        isinstance(value, list)
        and len(value) == 4
        and all(isinstance(v, (int, float)) for v in value)
        and all(abs(a - b) < 1e-6 for a, b in zip(value, KEY))
    )


def recolour(node: object, colour: list[float]) -> int:
    """Replace every static fill or stroke of the key colour. Returns how many."""
    changed = 0
    if isinstance(node, dict):
        paint = node.get("c")
        if node.get("ty") in ("fl", "st") and isinstance(paint, dict) and is_key(paint.get("k")):
            paint["k"] = list(colour)
            changed += 1
        for value in node.values():
            changed += recolour(value, colour)
    elif isinstance(node, list):
        for value in node:
            changed += recolour(value, colour)
    return changed


def build_source(source: Path, look: str, name: str, colour: str, out: Path) -> None:
    """Write the recoloured dotLottie archive, entries in the source's order."""
    with ZipFile(source) as archive, ZipFile(out, "w") as result:
        for entry in archive.infolist():
            data = archive.read(entry.filename)
            if entry.filename == "cua/theme.json":
                theme = json.loads(data)
                if theme.get("license") != "MIT":
                    sys.exit(f"the source theme is not MIT licensed: {theme.get('license')!r}")
                theme["id"] = theme_id(look)
                theme["name"] = name
                theme["author"] = AUTHOR
                theme["version"] = THEME_VERSION
                data = json.dumps(theme, sort_keys=True, separators=(",", ":")).encode()
            elif entry.filename.startswith("a/") and entry.filename.endswith(".json"):
                animation = json.loads(data)
                if recolour(animation, rgba(colour)) == 0:
                    sys.exit(f"{entry.filename}: the key colour was not found")
                data = json.dumps(animation, separators=(",", ":")).encode()
            info = ZipInfo(entry.filename, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            result.writestr(info, data)


def run(compiler: Path, *args: str) -> str:
    done = subprocess.run([str(compiler), *args], capture_output=True, text=True)
    if done.returncode != 0:
        sys.exit(f"{compiler.name} {' '.join(args)} failed:\n{done.stdout}{done.stderr}")
    return done.stdout


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--compiler", required=True, type=Path, help="path to cua-cursor-theme")
    parser.add_argument("--source", type=Path, default=SOURCE, help="cua.default.lottie")
    parser.add_argument("--out", type=Path, default=ASSETS)
    args = parser.parse_args()

    args.out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as work:
        for look, name, colour in LOOKS:
            lottie = Path(work) / f"{theme_id(look)}.lottie"
            target = args.out / f"{theme_id(look)}.cua-theme"
            build_source(args.source, look, name, colour, lottie)
            run(args.compiler, "validate", str(lottie))
            run(args.compiler, "build", str(lottie), "--output", str(target))
            seen = json.loads(run(args.compiler, "inspect", str(target), "--json"))
            if seen.get("id") != theme_id(look):
                sys.exit(f"{target.name}: compiled with id {seen.get('id')!r}")
            digest = hashlib.sha256(target.read_bytes()).hexdigest()
            print(f"{target.name}  {colour}  {target.stat().st_size} bytes  sha256 {digest}")


if __name__ == "__main__":
    main()

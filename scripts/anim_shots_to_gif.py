#!/usr/bin/env python3
"""Render emulator animation screenshots to PNG + animated GIF.

Consumes the PGM frames dumped by `anim_shots` (src-tauri/src/bin/anim_shots.rs,
validation §3 visual check) under a folder like tmp/anim-shots/<effect>/phase_XX.pgm
and writes, per effect folder:

- phase_XX.png  (1:1) plus a scaled 4x PNG for quick eyeballing
- <effect>.gif  — animated, ~8 fps-ish (120 ms/frame), infinite loop, of the
  scaled frames — the 64x128 1bpp screen at native size is unreadable.

Stdlib + Pillow only. Run: python3 scripts/anim_shots_to_gif.py [shots_root]
(default shots_root = tmp/anim-shots under the repo root).
"""

import sys
from pathlib import Path

from PIL import Image

SCALE = 4
FRAME_MS = 120


def load_pgm(path: Path) -> Image.Image:
    with path.open("rb") as f:
        magic = f.readline().strip()
        if magic != b"P5":
            raise ValueError(f"{path}: not a binary PGM")
        # header: width height, then maxval, then raster (whitespace-separated,
        # possibly with comments) — Pillow cannot read PGM with unusual
        # whitespace, so parse manually.
        nums = []
        while len(nums) < 3:
            tok = f.readline().strip()
            if tok.startswith(b"#") or not tok:
                continue
            nums.extend(int(t) for t in tok.split())
        w, h, _maxval = nums[:3]
        return Image.frombytes("L", (w, h), f.read())


def main() -> None:
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent.parent / "tmp/anim-shots"
    if not root.is_dir():
        sys.exit(f"no shots folder at {root} — run anim_shots first")

    for effect_dir in sorted(p for p in root.iterdir() if p.is_dir()):
        pgms = sorted(effect_dir.glob("phase_*.pgm"))
        if not pgms:
            continue
        frames = [load_pgm(p) for p in pgms]
        for p, im in zip(pgms, frames):
            im.save(p.with_suffix(".png"))
        scaled = [im.resize((im.width * SCALE, im.height * SCALE), Image.NEAREST) for im in frames]
        for p, im in zip(pgms, scaled):
            im.save(p.with_name(p.stem + f"_x{SCALE}.png"))
        gif = effect_dir / f"{effect_dir.name}.gif"
        scaled[0].save(
            gif,
            save_all=True,
            append_images=scaled[1:],
            duration=FRAME_MS,
            loop=0,
            optimize=False,
        )
        on = [sum(1 for px in f.getdata() if px) for f in frames]
        print(f"{effect_dir.name}: {len(frames)} frames ({pgms[0].name}..{pgms[-1].name}), "
              f"on-pixels {on[0]} -> {on[-1]}")
        print(f"  gif: {gif}")


if __name__ == "__main__":
    main()

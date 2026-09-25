#!/usr/bin/env python3
"""
wpolymap.py - where a compiled map draws the most world polygons.

Why this exists: in GoldSrc the frame rate follows wpoly, the world faces the
engine draws, and those come from the PVS: standing in a leaf, every face listed
by any leaf that leaf can see is sent to the renderer before frustum and
backface culling. This script counts, for every leaf, the world faces its PVS
brings in, so the places that cost the most show up with a number and a spot.
Fixing them is map work: HINT brushes at the choke points, walls or corners
that break long sightlines, NULL on faces nobody sees.

Output: the distribution (mean, p50, p95, max), the worst areas merged by
proximity, and optionally a pointfile marking them. J.A.C.K. and Hammer load
it with "Load pointfile" like a leak trail; each hotspot is a star, bigger for
worse ones.

The counts are upper bounds: the engine also skips faces outside the view
frustum and faces pointing away, so in game you see roughly half of it.
Brush entities are not counted; they add their own faces when visible.

Usage:
    wpolymap.py map.bsp [--top N] [--pts out.pts] [--radius R]
"""

import argparse
import math
import struct
import sys

LUMP_VISIBILITY, LUMP_LEAFS, LUMP_MARKSURFACES, LUMP_MODELS = 4, 10, 11, 14
CONTENTS_SOLID = -2


def load(path):
    data = open(path, "rb").read()
    version = struct.unpack_from("<i", data, 0)[0]
    if version != 30:
        sys.exit(f"{path}: BSP version {version}, expected 30")
    lumps = [struct.unpack_from("<ii", data, 4 + 8 * i) for i in range(15)]

    def arr(index, fmt):
        off, size = lumps[index]
        step = struct.calcsize(fmt)
        return [struct.unpack_from(fmt, data, off + i * step) for i in range(size // step)]

    off, size = lumps[LUMP_VISIBILITY]
    vis = data[off:off + size]
    return vis, arr(LUMP_LEAFS, "<ii3h3hHH4B"), arr(LUMP_MARKSURFACES, "<H"), arr(LUMP_MODELS, "<9f4iiii")[0]


def decompress(vis, ofs, numleafs):
    """Leaf indices (1-based) set in a leaf's compressed PVS row."""
    out = []
    if ofs < 0 or not vis:
        return list(range(1, numleafs + 1))  # no vis data: everything is visible
    i, leaf = ofs, 1
    while leaf <= numleafs and i < len(vis):
        b = vis[i]
        i += 1
        if b == 0:
            leaf += 8 * vis[i]
            i += 1
            continue
        for bit in range(8):
            if b & (1 << bit) and leaf + bit <= numleafs:
                out.append(leaf + bit)
        leaf += 8
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("bsp")
    ap.add_argument("--top", type=int, default=10, help="hotspots to list (default 10)")
    ap.add_argument("--pts", help="write a pointfile marking the hotspots")
    ap.add_argument("--radius", type=float, default=256.0,
                    help="leaves closer than this merge into one hotspot (default 256)")
    args = ap.parse_args()

    vis, leafs, marks, model = load(args.bsp)
    numleafs = model[13]
    worldfaces = range(model[14], model[14] + model[15])
    worldset = set(worldfaces)
    if not vis:
        print("warning: the map has no vis data, every leaf sees everything; run VIS first")

    leafmarks = []
    for leaf in leafs[:numleafs + 1]:
        leafmarks.append([m[0] for m in marks[leaf[8]:leaf[8] + leaf[9]] if m[0] in worldset])

    rows = []
    for n in range(1, numleafs + 1):
        leaf = leafs[n]
        if leaf[0] == CONTENTS_SOLID:
            continue
        faces = set(leafmarks[n])
        for v in decompress(vis, leaf[1], numleafs):
            faces.update(leafmarks[v])
        center = tuple((leaf[2 + k] + leaf[5 + k]) / 2.0 for k in range(3))
        size = max(leaf[5 + k] - leaf[2 + k] for k in range(3))
        rows.append((len(faces), center, size, n))

    if not rows:
        sys.exit("no empty leaves in the world model")
    counts = sorted(r[0] for r in rows)
    pick = lambda q: counts[min(len(counts) - 1, int(q * len(counts)))]
    print(f"{args.bsp}: {len(worldfaces)} world faces, {len(rows)} empty leaves")
    print(f"  faces in PVS per leaf: mean {sum(counts) / len(counts):.0f}  p50 {pick(0.5)}  "
          f"p95 {pick(0.95)}  max {counts[-1]}")

    # greedy merge of the worst leaves into hotspots
    hotspots = []
    for count, center, size, n in sorted(rows, reverse=True):
        if any(math.dist(center, h[1]) < args.radius for h in hotspots):
            continue
        hotspots.append((count, center, size, n))
        if len(hotspots) >= args.top:
            break
    print(f"  worst areas (faces in PVS, center of the leaf):")
    for count, center, size, n in hotspots:
        print(f"    {count:6d}  ({center[0]:.0f} {center[1]:.0f} {center[2]:.0f})  leaf {n}")

    if args.pts:
        worst = hotspots[0][0] if hotspots else 1
        dirs = [(1, 0, 0), (0, 1, 0), (0, 0, 1), (1, 1, 0), (1, -1, 0), (1, 0, 1), (1, 0, -1),
                (0, 1, 1), (0, 1, -1), (1, 1, 1), (1, 1, -1), (1, -1, 1), (1, -1, -1)]
        with open(args.pts, "w") as f:
            for count, center, size, n in hotspots:
                radius = 16 + 48 * count / worst
                for d in dirs:
                    length = math.sqrt(sum(c * c for c in d))
                    steps = int(radius)
                    for s in range(-steps, steps + 1, 2):
                        f.write("%f %f %f\n" % tuple(center[k] + d[k] / length * s for k in range(3)))
        print(f"  pointfile: {args.pts} (load it in the editor like a leak trail)")


if __name__ == "__main__":
    main()

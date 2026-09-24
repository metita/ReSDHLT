#!/usr/bin/env python3
"""
holecheck.py - find see-through holes in a compiled BSP.

Why this exists: a face can be present in the .map and in CSG output and still
be missing from the BSP, so the player sees through the wall even though the
editor shows a closed surface. Vertex manipulation with off-grid points does
this: neighbouring faces end up on planes that differ by more than CSG merges
yet stay within ON_EPSILON of each other, and sdHLBSP used to lose the face.
bspcheck.py cannot see it, because every face that IS in the BSP is valid.

How: rays are cast through hull 0 of the world model. Where a ray first enters
solid or sky, a face the engine draws (one listed by a non-solid leaf) must face
the viewer and cross the ray there, on that node plane or a hair behind it: two
nearly coplanar faces can leave the solid boundary on one plane and the drawn
face on the other, which looks fine in game. If none does, that spot is a hole.

With a .map, rays start just in front of random points on the visible
worldspawn and func_detail brush faces and aim back at them, so every face gets tested. Without one, rays
start at random empty points. Faces the compiler removes on purpose (NULL,
SKIP, CLIP, BEVEL, sky) show up as holes in the second mode; the first one
does not aim at them, though a ray can still graze one near an edge.

Usage:
    holecheck.py map.bsp [--map map.map] [--rays N] [--seed S]
"""

import argparse
import math
import random
import re
import struct
import sys
from collections import defaultdict

CONTENTS_EMPTY, CONTENTS_SOLID, CONTENTS_SKY = -1, -2, -6
LUMP_PLANES, LUMP_TEXTURES, LUMP_VERTEXES, LUMP_NODES = 1, 2, 3, 5
LUMP_TEXINFO, LUMP_FACES, LUMP_LEAFS, LUMP_EDGES = 6, 7, 10, 12
LUMP_MARKSURFACES, LUMP_SURFEDGES, LUMP_MODELS = 11, 13, 14
TOOL_TEXTURES = ("NULL", "SKIP", "CLIP", "BEVEL", "ORIGIN", "HINT", "SOLIDHINT",
                 "BEVELHINT", "AAATRIGGER", "BOUNDINGBOX", "CONTENT", "SKY")


def dot(a, b):
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


def sub(a, b):
    return (a[0] - b[0], a[1] - b[1], a[2] - b[2])


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0])


def length(a):
    return math.sqrt(dot(a, a))


class Bsp:
    def __init__(self, path):
        data = open(path, "rb").read()
        version = struct.unpack_from("<i", data, 0)[0]
        if version != 30:
            sys.exit(f"{path}: BSP version {version}, expected 30")
        lumps = [struct.unpack_from("<ii", data, 4 + 8 * i) for i in range(15)]

        def arr(index, fmt):
            off, size = lumps[index]
            step = struct.calcsize(fmt)
            return [struct.unpack_from(fmt, data, off + i * step) for i in range(size // step)]

        self.planes = arr(LUMP_PLANES, "<4fi")
        vertexes = arr(LUMP_VERTEXES, "<3f")
        self.nodes = arr(LUMP_NODES, "<i2h3h3hHH")
        texinfo = arr(LUMP_TEXINFO, "<8fii")
        faces = arr(LUMP_FACES, "<Hhihh4Bi")
        self.leafs = arr(LUMP_LEAFS, "<ii3h3hHH4B")
        edges = arr(LUMP_EDGES, "<HH")
        surfedges = arr(LUMP_SURFEDGES, "<i")
        model = arr(LUMP_MODELS, "<9f4iiii")[0]
        off = lumps[LUMP_TEXTURES][0]
        names = []
        for i in range(struct.unpack_from("<i", data, off)[0]):
            o = struct.unpack_from("<i", data, off + 4 + 4 * i)[0]
            names.append(data[off + o: off + o + 16].split(b"\0")[0].decode("latin-1"))

        self.head = model[9]
        self.mins, self.maxs = model[0:3], model[3:6]
        # planenum -> [(side, points, texture)] for the world model's faces
        self.faces = defaultdict(list)
        for fi in range(model[14], model[14] + model[15]):
            planenum, side, firstedge, numedges, ti = faces[fi][:5]
            pts = []
            for k in range(numedges):
                se = surfedges[firstedge + k][0]
                e = edges[abs(se)]
                pts.append(vertexes[e[0] if se >= 0 else e[1]])
            self.faces[planenum].append((side, pts, names[texinfo[ti][8]]))
        # faces the engine can draw: listed by a non-solid leaf's marksurfaces
        marks = arr(LUMP_MARKSURFACES, "<H")
        drawn = set()
        for lf in self.leafs:
            if lf[0] != CONTENTS_SOLID:
                for k in range(lf[8], lf[8] + lf[9]):
                    drawn.add(marks[k][0])
        self.drawn = []
        for fi in range(model[14], model[14] + model[15]):
            if fi not in drawn:
                continue
            planenum, side, firstedge, numedges = faces[fi][:4]
            pts = []
            for k in range(numedges):
                se = surfedges[firstedge + k][0]
                e = edges[abs(se)]
                pts.append(vertexes[e[0] if se >= 0 else e[1]])
            pl = self.planes[planenum]
            n = tuple(-c for c in pl[:3]) if side else pl[:3]
            d = -pl[3] if side else pl[3]
            lo = tuple(min(q[i] for q in pts) for i in range(3))
            hi = tuple(max(q[i] for q in pts) for i in range(3))
            self.drawn.append((n, d, pts, lo, hi))

    def covered_render(self, p, direction, reach=0.5, tol=0.1):
        """Some drawable face, facing the viewer, crosses the ray within reach of p."""
        for n, d, pts, lo, hi in self.drawn:
            if any(p[i] < lo[i] - reach or p[i] > hi[i] + reach for i in range(3)):
                continue
            den = dot(direction, n)
            if den >= 0:
                continue
            t = (d - dot(p, n)) / den
            if abs(t) > reach:
                continue
            q = tuple(p[i] + direction[i] * t for i in range(3))
            if inside(q, pts, tol):
                return True
        return False

    def contents(self, p):
        n = self.head
        while n >= 0:
            node = self.nodes[n]
            pl = self.planes[node[0]]
            n = node[1] if dot(p, pl) - pl[3] >= 0 else node[2]
        return self.leafs[-n - 1][0]

    def trace(self, start, end):
        """First point where the segment enters solid or sky: (point, planenum, side)."""
        def walk(num, a, b):
            if num < 0:
                c = self.leafs[-num - 1][0]
                return "start" if c in (CONTENTS_SOLID, CONTENTS_SKY) else None
            node = self.nodes[num]
            pl = self.planes[node[0]]
            t1 = dot(a, pl) - pl[3]
            t2 = dot(b, pl) - pl[3]
            if t1 >= 0 and t2 >= 0:
                return walk(node[1], a, b)
            if t1 < 0 and t2 < 0:
                return walk(node[2], a, b)
            side = 0 if t1 >= 0 else 1
            frac = t1 / (t1 - t2)
            mid = tuple(a[i] + frac * (b[i] - a[i]) for i in range(3))
            hit = walk(node[1 + side], a, mid)
            if hit is not None:
                return hit
            hit = walk(node[2 - side], mid, b)
            return (mid, node[0], side) if hit == "start" else hit

        hit = walk(self.head, start, end)
        return None if hit in (None, "start") else hit

    def covered(self, p, planenum, viewside, tol=0.1):
        facing_away = False
        for side, pts, _ in self.faces.get(planenum, []):
            if inside(p, pts, tol):
                if side == viewside:
                    return "ok"
                facing_away = True
        return "backface" if facing_away else "hole"


def inside(p, pts, tol):
    """p within tol of the inside of the convex polygon pts (p assumed on its plane)."""
    normal = [0.0, 0.0, 0.0]
    for i, a in enumerate(pts):
        b = pts[(i + 1) % len(pts)]
        normal[0] += (a[1] - b[1]) * (a[2] + b[2])
        normal[1] += (a[2] - b[2]) * (a[0] + b[0])
        normal[2] += (a[0] - b[0]) * (a[1] + b[1])
    for i, a in enumerate(pts):
        inward = cross(normal, sub(pts[(i + 1) % len(pts)], a))
        size = length(inward)
        if size and dot(sub(p, a), inward) / size < -tol:
            return False
    return True


POINT = re.compile(r"\(\s*([-\d.eE+]+)\s+([-\d.eE+]+)\s+([-\d.eE+]+)\s*\)")


def map_world_faces(path):
    """Visible brush faces of worldspawn and func_detail (both end up in the world
    model) as (polygon, outward normal)."""
    brushes, sides, depth, entity = [], None, 0, -1
    ent_brushes, classname = [], ""
    for line in open(path, encoding="latin-1"):
        s = line.strip()
        if s == "{":
            depth += 1
            if depth == 1:
                entity += 1
                ent_brushes, classname = [], ""
            elif depth == 2:
                sides = []
            continue
        if s == "}":
            if depth == 2:
                ent_brushes.append(sides)
            elif depth == 1 and classname in ("worldspawn", "func_detail"):
                brushes.extend(ent_brushes)
            depth -= 1
            continue
        if depth == 1 and s.startswith('"classname"'):
            classname = s.split('"')[3]
        if depth == 2 and s.startswith("("):
            pts = [tuple(float(c) for c in m) for m in POINT.findall(s)[:3]]
            rest = s[s.rfind(")") + 1:].split()
            sides.append((pts, rest[0].upper() if rest else ""))

    faces = []
    for sides in brushes:
        planes = []
        for pts, _ in sides:
            # same orientation as sdHLCSG's PlaneFromPoints
            n = cross(sub(pts[0], pts[1]), sub(pts[2], pts[1]))
            size = length(n)
            if not size:
                break
            n = tuple(c / size for c in n)
            planes.append((n, dot(n, pts[0])))
        else:
            for i, (n, d) in enumerate(planes):
                if sides[i][1].startswith(TOOL_TEXTURES):
                    continue
                w = base_winding(n, d)
                for j, (n2, d2) in enumerate(planes):
                    if i != j:
                        w = chop(w, n2, d2)
                        if not w:
                            break
                if len(w) >= 3:
                    faces.append((w, n))
    return faces


def base_winding(n, d):
    axis = max(range(3), key=lambda i: abs(n[i]))
    up = (0.0, 0.0, 1.0) if axis != 2 else (1.0, 0.0, 0.0)
    k = dot(up, n)
    up = tuple(up[i] - n[i] * k for i in range(3))
    size = length(up)
    up = tuple(c / size for c in up)
    right = cross(up, n)
    o = tuple(c * d for c in n)
    big = 65536.0
    return [tuple(o[i] + (sr * right[i] + su * up[i]) * big for i in range(3))
            for sr, su in ((-1, 1), (1, 1), (1, -1), (-1, -1))]


def chop(w, n, d, eps=1e-5):
    """Keep the part of w behind the plane (n, d)."""
    out = []
    for i, a in enumerate(w):
        b = w[(i + 1) % len(w)]
        da, db = dot(a, n) - d, dot(b, n) - d
        if da <= eps:
            out.append(a)
        if (da > eps and db < -eps) or (da < -eps and db > eps):
            t = da / (da - db)
            out.append(tuple(a[k] + (b[k] - a[k]) * t for k in range(3)))
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("bsp")
    ap.add_argument("--map", help="source .map: aim rays at its visible world faces")
    ap.add_argument("--rays", type=int, default=20000)
    ap.add_argument("--seed", type=int, default=1)
    args = ap.parse_args()

    rnd = random.Random(args.seed)
    bsp = Bsp(args.bsp)
    targets = map_world_faces(args.map) if args.map else None
    holes, backfaces, hits, tries = [], [], 0, 0
    while hits < args.rays and tries < args.rays * 50:
        tries += 1
        if targets:
            w, n = rnd.choice(targets)
            weights = [rnd.random() ** 3 for _ in w]
            total = sum(weights)
            q = tuple(sum(w[k][i] * weights[k] for k in range(len(w))) / total for i in range(3))
            back = rnd.uniform(0.5, 48)
            origin = tuple(q[i] + n[i] * back for i in range(3))
            d = tuple(-n[i] + rnd.uniform(-0.6, 0.6) for i in range(3))
        else:
            origin = tuple(rnd.uniform(bsp.mins[i], bsp.maxs[i]) for i in range(3))
            d = tuple(rnd.gauss(0, 1) for _ in range(3))
        size = length(d)
        if not size or bsp.contents(origin) != CONTENTS_EMPTY:
            continue
        end = tuple(origin[i] + d[i] / size * 16384 for i in range(3))
        hit = bsp.trace(origin, end)
        if not hit:
            continue
        hits += 1
        p, planenum, side = hit
        result = bsp.covered(p, planenum, side)
        if result != "ok" and bsp.covered_render(p, tuple(c / size for c in d)):
            result = "ok"
        if result == "hole":
            holes.append((p, planenum, origin))
        elif result == "backface":
            backfaces.append((p, planenum, origin))

    print(f"{args.bsp}: rays={hits} holes={len(holes)} backfaces={len(backfaces)}")
    seen = set()
    for kind, found in (("HOLE", holes), ("BACKFACE", backfaces)):
        for p, planenum, origin in found:
            key = (kind, planenum, tuple(round(c / 16) for c in p))
            if key in seen:
                continue
            seen.add(key)
            pl = bsp.planes[planenum]
            print(f"  {kind} at ({p[0]:.1f} {p[1]:.1f} {p[2]:.1f}) plane {planenum} "
                  f"normal ({pl[0]:.3f} {pl[1]:.3f} {pl[2]:.3f}) seen from "
                  f"({origin[0]:.0f} {origin[1]:.0f} {origin[2]:.0f})")
    return 1 if holes else 0


if __name__ == "__main__":
    sys.exit(main())

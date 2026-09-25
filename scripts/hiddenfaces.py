#!/usr/bin/env python3
"""
hiddenfaces.py - faces a compiled map keeps but nobody can see.

Why this exists: brush entities (func_wall, func_illusionary, func_breakable,
doors...) do not cut the world in CSG, so a world floor under a func_wall prop
or a wall behind a func_wall panel is still drawn every time its leaf is
visible, although the entity covers it completely. The same goes the other way
for entity faces buried in world brushes. Both cost wpoly for nothing; NULL on
the brush face, or making the prop func_detail when it does not need to be an
entity, removes them.

Reported:
  covered world faces    drawn world faces that an opaque brush entity covers
                         everywhere (every sample point just in front of the
                         face is inside the entity)
  buried entity faces    faces of opaque brush entities whose front is inside
                         world solid everywhere
  never drawn faces      world faces no empty leaf lists; the engine skips them
                         already, but they still take lightmap space and count
                         against the face limits

Only entities that are always there and drawn fully opaque count: func_wall
and func_illusionary with rendermode 0 and no zhlt_invisible. Triggers and
ladders are not drawn; doors, trains and breakables move or break and uncover
what is behind them, so those faces are not wasted. Textures the compiler
already hides (sky, NULL, tool textures) are skipped.
With --pts the faces are marked in a pointfile the editor loads like a leak.

Usage:
    hiddenfaces.py map.bsp [--spacing N] [--list N] [--pts out.pts]
"""

import argparse
import math
import re
import struct
import sys
from collections import Counter

CONTENTS_SOLID = -2
TEX_SPECIAL = 1
SKIP_TEXTURES = ("sky", "null", "clip", "origin", "hint", "skip", "aaatrigger", "bevel")
STATIC_VISIBLE_CLASSES = ("func_wall", "func_illusionary")


def dot(a, b):
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


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

        off, size = lumps[0]
        self.entities = data[off:off + size].decode("latin-1", "replace")
        self.planes = arr(1, "<4fi")
        self.verts = arr(3, "<3f")
        self.nodes = arr(5, "<i2h3h3hHH")
        self.texinfo = arr(6, "<8fii")
        self.faces = arr(7, "<Hhihh4Bi")
        self.leafs = arr(10, "<ii3h3hHH4B")
        self.marks = arr(11, "<H")
        self.edges = arr(12, "<HH")
        self.surfedges = arr(13, "<i")
        self.models = arr(14, "<9f4iiii")
        off = lumps[2][0]
        names = []
        for i in range(struct.unpack_from("<i", data, off)[0]):
            o = struct.unpack_from("<i", data, off + 4 + 4 * i)[0]
            names.append(data[off + o: off + o + 16].split(b"\0")[0].decode("latin-1").lower() if o >= 0 else "")
        self.texnames = names

    def face_points(self, fi):
        f = self.faces[fi]
        pts = []
        for k in range(f[3]):
            se = self.surfedges[f[2] + k][0]
            e = self.edges[abs(se)]
            pts.append(self.verts[e[0] if se >= 0 else e[1]])
        return pts

    def face_normal(self, fi):
        f = self.faces[fi]
        pl = self.planes[f[0]]
        return tuple(-c for c in pl[:3]) if f[1] else tuple(pl[:3])

    def texname(self, fi):
        return self.texnames[self.texinfo[self.faces[fi][4]][8]]

    def contents(self, headnode, p):
        n = headnode
        while n >= 0:
            node = self.nodes[n]
            pl = self.planes[node[0]]
            n = node[1] if dot(p, pl) - pl[3] >= 0 else node[2]
        return self.leafs[-n - 1][0]


def parse_entities(text):
    ents = []
    for block in re.findall(r"\{([^{}]*)\}", text):
        ents.append(dict(re.findall(r'"([^"]*)"\s*"([^"]*)"', block)))
    return ents


def area(pts):
    s = [0.0, 0.0, 0.0]
    for i in range(1, len(pts) - 1):
        a = [pts[i][k] - pts[0][k] for k in range(3)]
        b = [pts[i + 1][k] - pts[0][k] for k in range(3)]
        c = (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])
        for k in range(3):
            s[k] += c[k]
    return math.sqrt(dot(s, s)) / 2


def sample_points(pts, spacing, margin=1.0, maxpoints=1024):
    """A grid over the whole convex polygon, `margin` units in from its edges, so a
    face only counts as covered when its corners and borders are covered too."""
    c = tuple(sum(p[k] for p in pts) / len(pts) for k in range(3))
    # in-plane basis from the first long edge
    e = None
    for i in range(len(pts)):
        d = [pts[(i + 1) % len(pts)][k] - pts[i][k] for k in range(3)]
        if dot(d, d) > 1e-6:
            e = d
            break
    if e is None:
        return [c]
    # Newell normal: faces often have collinear vertices, so three of them are not enough
    n = [0.0, 0.0, 0.0]
    for i in range(len(pts)):
        p, q = pts[i], pts[(i + 1) % len(pts)]
        n[0] += (p[1] - q[1]) * (p[2] + q[2])
        n[1] += (p[2] - q[2]) * (p[0] + q[0])
        n[2] += (p[0] - q[0]) * (p[1] + q[1])
    ln = math.sqrt(dot(n, n))
    if ln < 1e-6:
        return [c]
    n = tuple(x / ln for x in n)
    le = math.sqrt(dot(e, e))
    u = tuple(x / le for x in e)
    v = (n[1] * u[2] - n[2] * u[1], n[2] * u[0] - n[0] * u[2], n[0] * u[1] - n[1] * u[0])
    poly = [(dot(p, u), dot(p, v)) for p in pts]
    smin, smax = min(p[0] for p in poly), max(p[0] for p in poly)
    tmin, tmax = min(p[1] for p in poly), max(p[1] for p in poly)
    step = max(spacing, math.sqrt((smax - smin) * (tmax - tmin) / maxpoints) if maxpoints else spacing)

    def inside(s, t):
        sign = 0
        for i in range(len(poly)):
            (s0, t0), (s1, t1) = poly[i], poly[(i + 1) % len(poly)]
            el = math.hypot(s1 - s0, t1 - t0)
            if el < 1e-6:
                continue
            cr = ((s1 - s0) * (t - t0) - (t1 - t0) * (s - s0)) / el  # signed distance to the edge
            if abs(cr) < margin:
                return False
            if sign == 0:
                sign = 1 if cr > 0 else -1
            elif (cr > 0) != (sign > 0):
                return False
        return True

    def spread(lo, hi):
        """values from lo to hi, both ends included, at most `step` apart"""
        lo, hi = lo + margin, hi - margin
        if hi <= lo:
            return [(lo + hi) / 2]
        count = int(math.ceil((hi - lo) / step)) + 1
        return [lo + (hi - lo) * i / (count - 1) for i in range(count)]

    off = dot(c, n)
    out = [tuple(u[k] * s + v[k] * t + n[k] * off for k in range(3))
           for s in spread(smin, smax) for t in spread(tmin, tmax) if inside(s, t)]
    return out or [c]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("bsp")
    ap.add_argument("--spacing", type=float, default=8.0, help="grid step of the test points on each face (default 8)")
    ap.add_argument("--list", type=int, default=15, help="faces to list per group (default 15)")
    ap.add_argument("--pts", help="write a pointfile marking the faces found")
    args = ap.parse_args()
    bsp = Bsp(args.bsp)

    def skip(fi):
        name = bsp.texname(fi)
        return (bsp.texinfo[bsp.faces[fi][4]][9] & TEX_SPECIAL) or name.startswith(SKIP_TEXTURES)

    # opaque brush entities and their world-space offset
    covers = []
    for e in parse_entities(bsp.entities):
        model = e.get("model", "")
        if not model.startswith("*"):
            continue
        # only entities that are always there and always drawn hide what is behind
        # them: triggers and ladders are invisible, doors, trains and breakables
        # move or go away and uncover it
        if e.get("classname", "") not in STATIC_VISIBLE_CLASSES:
            continue
        if int(e.get("rendermode", "0") or 0) != 0 or e.get("zhlt_invisible", "0") not in ("", "0"):
            continue
        mi = int(model[1:])
        if mi <= 0 or mi >= len(bsp.models):
            continue
        m = bsp.models[mi]
        if not any(not skip(fi) for fi in range(m[14], m[14] + m[15])):
            continue
        origin = tuple(float(v) for v in (e.get("origin", "0 0 0").split() + ["0", "0", "0"])[:3])
        lo = tuple(m[k] + origin[k] for k in range(3))
        hi = tuple(m[3 + k] + origin[k] for k in range(3))
        covers.append((mi, origin, lo, hi, e.get("classname", "?")))

    world = bsp.models[0]
    worldfaces = range(world[14], world[14] + world[15])
    drawn = set()
    for leaf in bsp.leafs[1:world[13] + 1]:
        if leaf[0] != CONTENTS_SOLID:
            drawn.update(m[0] for m in bsp.marks[leaf[8]:leaf[8] + leaf[9]])

    covered, never = [], []
    for fi in worldfaces:
        if skip(fi):
            continue
        pts = bsp.face_points(fi)
        if fi not in drawn:
            never.append((area(pts), fi, None))
            continue
        n = bsp.face_normal(fi)
        tests = [tuple(p[k] + n[k] * 0.5 for k in range(3)) for p in sample_points(pts, args.spacing)]
        lo = tuple(min(p[k] for p in tests) for k in range(3))
        hi = tuple(max(p[k] for p in tests) for k in range(3))
        for mi, origin, clo, chi, cls in covers:
            if any(hi[k] < clo[k] or lo[k] > chi[k] for k in range(3)):
                continue
            head = bsp.models[mi][9]
            if all(bsp.contents(head, tuple(p[k] - origin[k] for k in range(3))) == CONTENTS_SOLID for p in tests):
                covered.append((area(pts), fi, cls))
                break

    buried = []
    for mi, origin, clo, chi, cls in covers:
        m = bsp.models[mi]
        for fi in range(m[14], m[14] + m[15]):
            if skip(fi):
                continue
            pts = [tuple(p[k] + origin[k] for k in range(3)) for p in bsp.face_points(fi)]
            n = bsp.face_normal(fi)
            tests = [tuple(p[k] + n[k] * 0.5 for k in range(3)) for p in sample_points(pts, args.spacing)]
            if all(bsp.contents(world[9], p) == CONTENTS_SOLID for p in tests):
                buried.append((area(pts), fi, cls))

    def report(title, rows, note):
        rows.sort(reverse=True)
        total = sum(r[0] for r in rows)
        print(f"  {title}: {len(rows)} faces, {total:.0f} square units{note}")
        tex = Counter(bsp.texname(r[1]) for r in rows)
        if rows:
            print("    textures: " + ", ".join(f"{t} {c}" for t, c in tex.most_common(6)))
        for a, fi, cls in rows[:args.list]:
            pts = bsp.face_points(fi)
            c = tuple(sum(p[k] for p in pts) / len(pts) for k in range(3))
            extra = f"  under {cls}" if cls and title.startswith("covered") else (f"  of {cls}" if cls else "")
            print(f"    face {fi:6d}  area {a:8.0f}  at ({c[0]:.0f} {c[1]:.0f} {c[2]:.0f})  {bsp.texname(fi)}{extra}")

    print(f"{args.bsp}: {len(worldfaces)} world faces, {len(covers)} opaque brush entities")
    report("covered world faces", covered, "  (drawn for nothing: NULL them or make the entity func_detail)")
    report("buried entity faces", buried, "  (drawn for nothing: NULL them)")
    report("never drawn faces", never, "  (not drawn already; only lightmap space)")

    if args.pts:
        with open(args.pts, "w") as f:
            for rows in (covered, buried):
                for a, fi, cls in rows:
                    pts = bsp.face_points(fi)
                    c = tuple(sum(p[k] for p in pts) / len(pts) for k in range(3))
                    for d in ((1, 0, 0), (0, 1, 0), (0, 0, 1)):
                        for s in range(-12, 13, 2):
                            f.write("%f %f %f\n" % tuple(c[k] + d[k] * s for k in range(3)))
        print(f"  pointfile: {args.pts}")


if __name__ == "__main__":
    main()

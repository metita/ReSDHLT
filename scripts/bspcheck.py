#!/usr/bin/env python3
"""
bspcheck.py - geometric validation of a compiled BSP.

Why this exists: changes to CSG, BSP or RAD can corrupt geometry in ways a
byte-diff cannot judge (a diff tells you the output changed, not whether it got
better or worse). Loading the map in the game is the real test, but most damage
is machine-detectable, and this catches it without leaving the terminal.

Checks performed
----------------
  planarity     every face point lies on the face's plane
  convexity     every face is a convex polygon, wound consistently
  degeneracy    no zero-area faces, no duplicate or colinear-only points
  bounds        point counts and coordinates within engine limits
  edges         surfedge/edge indices in range, faces reference valid texinfo
  area          total surface area, reported per plane, so two compiles can be
                compared: merging faces must preserve area exactly

  residual      how many face pairs could STILL be merged but were not. A pair
  merges        qualifies when it shares a plane and texinfo, shares an edge in
                reverse, would form a convex polygon, AND the union would still
                fit inside the subdivision limit. That last condition matters:
                sdHLBSP merges first and subdivides afterwards, so most
                adjacent coplanar faces are deliberately split to keep the
                lightmap extent legal, and merging them back would break the
                software renderer.

Usage:
    bspcheck.py map.bsp [--verbose] [--json out.json]
    bspcheck.py --compare a.json b.json
"""

import argparse
import json
import math
import struct
import sys
from collections import defaultdict

ON_EPSILON = 0.01
PLANE_EPSILON = 0.05          # generous: BSP vertices are snapped
MAXEDGES = 48                 # sdHLBSP bsp5.h
ENGINE_COORD_LIMIT = 65536.0
TEX_SPECIAL = 1               # no lightmap, exempt from subdivision
SUBDIVIDE_SIZE = 240.0        # (MAX_SURFACE_EXTENT-1)*TEXTURE_STEP, bsp5.h
ANGULAR_EPSILON = 1e-4        # sin of the turn angle, scale-independent

LUMP_PLANES, LUMP_VERTEXES, LUMP_FACES = 1, 3, 7
LUMP_TEXINFO, LUMP_EDGES, LUMP_SURFEDGES = 6, 12, 13


def sub(a, b):
    return (a[0] - b[0], a[1] - b[1], a[2] - b[2])


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0])


def dot(a, b):
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


def length(a):
    return math.sqrt(dot(a, a))


def normalize(a):
    l = length(a)
    return (a[0] / l, a[1] / l, a[2] / l) if l > 1e-12 else (0.0, 0.0, 0.0)


class Bsp:
    def __init__(self, path):
        self.data = open(path, "rb").read()
        self.version = struct.unpack_from("<i", self.data, 0)[0]

        self.planes = [struct.unpack_from("<4fi", self.lump(LUMP_PLANES), i * 20)
                       for i in range(len(self.lump(LUMP_PLANES)) // 20)]
        vx = self.lump(LUMP_VERTEXES)
        self.verts = [struct.unpack_from("<3f", vx, i * 12)
                      for i in range(len(vx) // 12)]
        ed = self.lump(LUMP_EDGES)
        self.edges = [struct.unpack_from("<2H", ed, i * 4)
                      for i in range(len(ed) // 4)]
        se = self.lump(LUMP_SURFEDGES)
        self.surfedges = [struct.unpack_from("<i", se, i * 4)[0]
                          for i in range(len(se) // 4)]
        tl = self.lump(LUMP_TEXINFO)
        self.numtexinfo = len(tl) // 40
        self.texinfo = []
        for i in range(self.numtexinfo):
            v = struct.unpack_from("<8f", tl, i * 40)
            miptex, flags = struct.unpack_from("<2i", tl, i * 40 + 32)
            self.texinfo.append({
                "s": v[0:3], "t": v[4:7], "flags": flags,
            })

        fc = self.lump(LUMP_FACES)
        self.faces = []
        for i in range(len(fc) // 20):
            planenum, side, firstedge, numedges, texinfo = \
                struct.unpack_from("<HhihH", fc, i * 20)
            self.faces.append({
                "index": i, "planenum": planenum, "side": side,
                "firstedge": firstedge, "numedges": numedges,
                "texinfo": texinfo,
            })

    def lump(self, i):
        o, l = struct.unpack_from("<ii", self.data, 4 + i * 8)
        return self.data[o:o + l]

    def face_points(self, f):
        """Walk surfedges to recover the face's polygon, in winding order."""
        pts = []
        for k in range(f["numedges"]):
            si = f["firstedge"] + k
            if not (0 <= si < len(self.surfedges)):
                return None
            e = self.surfedges[si]
            ei = abs(e)
            if not (0 <= ei < len(self.edges)):
                return None
            v = self.edges[ei][0] if e >= 0 else self.edges[ei][1]
            if not (0 <= v < len(self.verts)):
                return None
            pts.append(self.verts[v])
        return pts

    def face_normal(self, f):
        n = self.planes[f["planenum"]][:3]
        return (-n[0], -n[1], -n[2]) if f["side"] else n


def polygon_area(pts, normal):
    """Area via the projected shoelace / cross product sum."""
    if len(pts) < 3:
        return 0.0
    total = (0.0, 0.0, 0.0)
    for i in range(1, len(pts) - 1):
        c = cross(sub(pts[i], pts[0]), sub(pts[i + 1], pts[0]))
        total = (total[0] + c[0], total[1] + c[1], total[2] + c[2])
    return abs(dot(total, normal)) / 2.0


def check_face(bsp, f, issues):
    pts = bsp.face_points(f)
    tag = "face %d" % f["index"]

    if pts is None:
        issues["bad_indices"].append(tag)
        return 0.0
    if len(pts) < 3:
        issues["degenerate"].append("%s: %d points" % (tag, len(pts)))
        return 0.0
    if len(pts) > MAXEDGES:
        issues["too_many_points"].append("%s: %d points" % (tag, len(pts)))
    if not (0 <= f["texinfo"] < bsp.numtexinfo):
        issues["bad_texinfo"].append("%s: texinfo %d" % (tag, f["texinfo"]))

    plane = bsp.planes[f["planenum"]]
    pn, pd = plane[:3], plane[3]

    # planarity
    for p in pts:
        d = dot(p, pn) - pd
        if abs(d) > PLANE_EPSILON:
            issues["not_planar"].append("%s: point off plane by %.4f" % (tag, d))
            break

    # coordinate sanity
    for p in pts:
        if max(abs(c) for c in p) > ENGINE_COORD_LIMIT:
            issues["out_of_bounds"].append("%s: %s" % (tag, str(p)))
            break

    # duplicate points
    for i in range(len(pts)):
        if length(sub(pts[i], pts[(i + 1) % len(pts)])) < ON_EPSILON:
            issues["duplicate_points"].append("%s: point %d" % (tag, i))
            break

    normal = bsp.face_normal(f)

    # convexity: every turn must have the same sign relative to the normal
    n = len(pts)
    sign = 0
    for i in range(n):
        a, b, c = pts[i], pts[(i + 1) % n], pts[(i + 2) % n]
        e1, e2 = sub(b, a), sub(c, b)
        l1, l2 = length(e1), length(e2)
        if l1 < ON_EPSILON or l2 < ON_EPSILON:
            continue
        # Normalise by the edge lengths: the raw cross product scales with
        # them, so a fixed absolute threshold would flag long edges with a
        # negligible angle as non-convex.
        turn = dot(cross(e1, e2), normal) / (l1 * l2)
        if abs(turn) < ANGULAR_EPSILON:
            continue                       # colinear, harmless
        s = 1 if turn > 0 else -1
        if sign == 0:
            sign = s
        elif s != sign:
            issues["not_convex"].append(tag)
            break

    area = polygon_area(pts, normal)
    if area < 1e-4:
        issues["zero_area"].append(tag)
    return area


def shares_reversed_edge(pa, pb):
    """True if the polygons share an edge traversed in opposite directions."""
    na, nb = len(pa), len(pb)
    for i in range(na):
        a1, a2 = pa[i], pa[(i + 1) % na]
        for j in range(nb):
            b1, b2 = pb[j], pb[(j + 1) % nb]
            if (length(sub(a1, b2)) < ON_EPSILON and
                    length(sub(a2, b1)) < ON_EPSILON):
                return (i, j)
    return None


def fits_subdivision(bsp, texinfo_index, pts):
    """
    True if a face covering these points would survive SubdivideFace intact.

    sdHLBSP merges faces first and subdivides afterwards, so adjacent coplanar
    faces in a finished BSP are usually the *deliberate* result of subdivision
    keeping the lightmap extent within MAX_SURFACE_EXTENT. Reporting those as
    missed merges would be wrong: merging them back would break the software
    renderer and the HLDS. Only a pair whose union still fits counts.
    """
    tex = bsp.texinfo[texinfo_index]
    if tex["flags"] & TEX_SPECIAL:
        return True                        # not subdivided at all
    for axis in ("s", "t"):
        vals = [dot(p, tex[axis]) for p in pts]
        if (max(vals) - min(vals)) > SUBDIVIDE_SIZE:
            return False
    return True


def would_be_convex(pa, pb, hit, normal):
    """Mirror of TryMerge's two convexity tests at the shared edge's ends."""
    i, j = hit
    na, nb = len(pa), len(pb)
    p1, p2 = pa[i], pa[(i + 1) % na]

    back = pa[(i + na - 1) % na]
    n1 = normalize(cross(normal, sub(p1, back)))
    if dot(sub(pb[(j + 2) % nb], p1), n1) > ON_EPSILON:
        return False

    back = pa[(i + 2) % na]
    n2 = normalize(cross(normal, sub(back, p2)))
    if dot(sub(pb[(j + nb - 1) % nb], p2), n2) > ON_EPSILON:
        return False

    return na + nb <= MAXEDGES


def find_residual_merges(bsp, faces_pts):
    """Face pairs that share plane+texinfo and could still legally merge."""
    groups = defaultdict(list)
    for f in bsp.faces:
        groups[(f["planenum"], f["side"], f["texinfo"])].append(f)

    residual = []
    for key, group in groups.items():
        if len(group) < 2:
            continue
        for x in range(len(group)):
            for y in range(x + 1, len(group)):
                fa, fb = group[x], group[y]
                pa, pb = faces_pts.get(fa["index"]), faces_pts.get(fb["index"])
                if not pa or not pb:
                    continue
                hit = shares_reversed_edge(pa, pb)
                if not hit or not would_be_convex(pa, pb, hit, bsp.face_normal(fa)):
                    continue
                if not fits_subdivision(bsp, fa["texinfo"], pa + pb):
                    continue               # would exceed the lightmap extent
                residual.append((fa["index"], fb["index"]))
    return residual


def run(path, verbose=False):
    bsp = Bsp(path)
    issues = defaultdict(list)
    area_by_plane = defaultdict(float)
    faces_pts = {}

    for f in bsp.faces:
        pts = bsp.face_points(f)
        if pts:
            faces_pts[f["index"]] = pts
        area_by_plane[f["planenum"]] += check_face(bsp, f, issues)

    residual = find_residual_merges(bsp, faces_pts)

    report = {
        "file": path,
        "version": bsp.version,
        "faces": len(bsp.faces),
        "planes": len(bsp.planes),
        "vertexes": len(bsp.verts),
        "total_area": round(sum(area_by_plane.values()), 3),
        "residual_merges": len(residual),
        "issues": {k: len(v) for k, v in issues.items()},
    }

    print("%s  (BSP v%d)" % (path, bsp.version))
    print("  faces=%d planes=%d vertexes=%d" % (
        report["faces"], report["planes"], report["vertexes"]))
    print("  total surface area = %.3f" % report["total_area"])

    if issues:
        print("\n  PROBLEMS:")
        for k, v in sorted(issues.items()):
            print("    %-18s %d" % (k, len(v)))
            if verbose:
                for item in v[:5]:
                    print("        " + item)
    else:
        print("  geometry OK: planar, convex, no degenerate faces")

    print("\n  face pairs that could still be merged: %d" % len(residual))
    if residual:
        print("    (face merging left work on the table)")
        if verbose:
            for a, b in residual[:10]:
                print("      faces %d + %d" % (a, b))
    else:
        print("    (0 = face merging reached a fixed point)")

    return report, bool(issues)


def compare(a_path, b_path):
    a, b = json.load(open(a_path)), json.load(open(b_path))
    print("%-22s %14s %14s %12s" % ("metric", "before", "after", "delta"))
    for k in ("faces", "planes", "vertexes", "residual_merges"):
        print("%-22s %14d %14d %+12d" % (k, a[k], b[k], b[k] - a[k]))

    da = b["total_area"] - a["total_area"]
    rel = (da / a["total_area"] * 100) if a["total_area"] else 0
    print("%-22s %14.3f %14.3f %+12.4f" % (
        "total_area", a["total_area"], b["total_area"], da))

    print()
    if abs(rel) < 0.01:
        print("Surface area preserved (%.4f%%): no geometry lost or duplicated." % rel)
    else:
        print("WARNING: surface area changed by %.4f%%. Merging must preserve "
              "area exactly;\n         a change means geometry was lost or "
              "double-covered." % rel)

    if b["faces"] < a["faces"]:
        print("Face count dropped by %d (%.1f%%): less wpoly for the engine." % (
            a["faces"] - b["faces"],
            (a["faces"] - b["faces"]) / a["faces"] * 100))
    elif b["faces"] > a["faces"]:
        print("Face count ROSE by %d: worse for FPS." % (b["faces"] - a["faces"]))

    bad = {k: v for k, v in b["issues"].items() if v}
    print("New geometry problems: %s" % (bad if bad else "none"))
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("bsp", nargs="?")
    ap.add_argument("-v", "--verbose", action="store_true")
    ap.add_argument("--json")
    ap.add_argument("--compare", nargs=2, metavar=("A", "B"))
    args = ap.parse_args()

    if args.compare:
        return compare(*args.compare)
    if not args.bsp:
        ap.error("need a .bsp (or --compare)")

    report, failed = run(args.bsp, args.verbose)
    if args.json:
        json.dump(report, open(args.json, "w"), indent=2, sort_keys=True)
        print("\nwrote %s" % args.json)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())

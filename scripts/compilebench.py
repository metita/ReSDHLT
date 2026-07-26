#!/usr/bin/env python3
"""
compilebench.py - time a full CSG/BSP/VIS/RAD compile and dump BSP statistics.

Two jobs:

  1. Performance:  how long each stage takes, so optimisations can be judged on
     measurements instead of assumptions.
  2. Regression:   a fingerprint of the resulting BSP (face/node/leaf counts,
     lightmap size, content hashes of each lump). Changes to BSP, CSG or RAD
     must not alter these unless that is the explicit intent of the change, so
     comparing two runs catches accidental geometry or lighting damage.

IMPORTANT - always pass --threads 1 when comparing for regressions.

The tools are not deterministic when run multi-threaded: work units are
handed out in whatever order threads reach them, and some stages append to
shared output in that order. Measured on koth_sandy, two consecutive runs of
the *same unmodified binary* with autodetected threads differed by +2
clipnodes, +1 marksurface and -4 bytes of visibility data. That noise makes
multi-threaded output useless as a regression baseline.

With --threads 1 the same binary reproduces byte-identical output, so any
difference is genuinely attributable to a code change. Use multi-threaded
runs for timing, single-threaded runs for correctness.

Usage:
    compilebench.py --tools DIR --map FILE.map [--runs N] [--json OUT.json]
    compilebench.py --compare before.json after.json
"""

import argparse
import hashlib
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import time

# BSP v30 lump layout: (name, index, struct size in bytes or None if variable)
LUMPS = [
    ("entities",     0, None),
    ("planes",       1, 20),
    ("textures",     2, None),
    ("vertexes",     3, 12),
    ("visibility",   4, None),
    ("nodes",        5, 24),
    ("texinfo",      6, 40),
    ("faces",        7, 20),
    ("lighting",     8, None),
    ("clipnodes",    9, 8),
    ("leaves",      10, 28),
    ("marksurfaces",11, 2),
    ("edges",       12, 4),
    ("surfedges",   13, 4),
    ("models",      14, 64),
]

STAGES = ["sdHLCSG", "sdHLBSP", "sdHLVIS", "sdHLRAD"]


def tool_path(tools_dir, name):
    for candidate in (name, name + ".exe"):
        p = os.path.join(tools_dir, candidate)
        if os.path.isfile(p):
            return p
    raise SystemExit("tool not found in %s: %s" % (tools_dir, name))


def read_bsp_stats(path):
    """Parse the BSP header and summarise every lump."""
    with open(path, "rb") as fh:
        data = fh.read()

    version = struct.unpack_from("<i", data, 0)[0]
    stats = {"__version": version, "__filesize": len(data)}

    for name, index, entsize in LUMPS:
        offset, length = struct.unpack_from("<ii", data, 4 + index * 8)
        blob = data[offset:offset + length]
        entry = {
            "bytes": length,
            # Hash the lump so a regression run can prove the content is
            # byte-identical, not merely the same size.
            "sha1": hashlib.sha1(blob).hexdigest()[:16],
        }
        if entsize:
            entry["count"] = length // entsize
        stats[name] = entry

    return stats


def run_stage(exe, target, extra_args, log_lines):
    args = [exe] + extra_args + [target]
    t0 = time.perf_counter()
    proc = subprocess.run(args, capture_output=True, text=True,
                          errors="replace")
    elapsed = time.perf_counter() - t0
    out = (proc.stdout or "") + (proc.stderr or "")
    log_lines.append("$ %s\n%s" % (" ".join(args), out))

    errors = [l.strip() for l in out.splitlines()
              if re.match(r"\s*Error", l) or "Fatal" in l]
    return elapsed, proc.returncode, errors, out


def compile_map(tools_dir, map_path, threads=None, stage_args=None):
    """Compile in a scratch directory so the source tree is never touched."""
    stage_args = stage_args or {}
    workdir = tempfile.mkdtemp(prefix="compilebench_")
    base = os.path.splitext(os.path.basename(map_path))[0]
    local_map = os.path.join(workdir, base + ".map")
    shutil.copyfile(map_path, local_map)

    result = {"map": base, "stages": {}, "ok": True, "errors": []}
    log_lines = []

    for stage in STAGES:
        exe = tool_path(tools_dir, stage)
        extra = list(stage_args.get(stage, []))
        if threads:
            extra += ["-threads", str(threads)]
        # CSG takes the .map, the later stages take the bare name.
        target = local_map if stage == "sdHLCSG" else os.path.join(workdir, base)

        elapsed, rc, errors, _ = run_stage(exe, target, extra, log_lines)
        result["stages"][stage] = round(elapsed, 3)
        if errors:
            result["ok"] = False
            result["errors"] += ["%s: %s" % (stage, e) for e in errors[:3]]
            break

    result["total"] = round(sum(result["stages"].values()), 3)

    bsp = os.path.join(workdir, base + ".bsp")
    if result["ok"] and os.path.isfile(bsp):
        result["bsp"] = read_bsp_stats(bsp)
    else:
        result["ok"] = False

    result["_log"] = "\n".join(log_lines)
    shutil.rmtree(workdir, ignore_errors=True)
    return result


def cmd_run(args):
    runs = []
    for i in range(args.runs):
        r = compile_map(args.tools, args.map, args.threads)
        if not r["ok"]:
            print("COMPILE FAILED:")
            for e in r["errors"]:
                print("  " + e)
            if args.verbose:
                print(r["_log"][-3000:])
            return 1
        runs.append(r)
        print("run %d/%d  %s" % (i + 1, args.runs, fmt_stages(r)))

    # Report the fastest run: least contaminated by noise from other processes.
    best = min(runs, key=lambda r: r["total"])
    best.pop("_log", None)
    best["runs"] = args.runs
    best["threads"] = args.threads
    best["all_totals"] = [r["total"] for r in runs]

    print("\nbest of %d: %s" % (args.runs, fmt_stages(best)))
    b = best["bsp"]
    print("faces=%d nodes=%d leaves=%d marksurfaces=%d planes=%d lightdata=%dKB" % (
        b["faces"]["count"], b["nodes"]["count"], b["leaves"]["count"],
        b["marksurfaces"]["count"], b["planes"]["count"],
        b["lighting"]["bytes"] // 1024))

    if args.json:
        with open(args.json, "w") as fh:
            json.dump(best, fh, indent=2, sort_keys=True)
        print("wrote %s" % args.json)
    return 0


def fmt_stages(r):
    return "  ".join("%s=%.2fs" % (k.replace("sdHL", ""), v)
                     for k, v in r["stages"].items()) + \
           "  total=%.2fs" % r["total"]


def cmd_compare(args):
    a = json.load(open(args.compare[0]))
    b = json.load(open(args.compare[1]))

    print("%-14s %12s %12s %10s" % ("stage", "before", "after", "delta"))
    for stage in STAGES + ["total"]:
        va = a["stages"].get(stage) if stage != "total" else a["total"]
        vb = b["stages"].get(stage) if stage != "total" else b["total"]
        if va is None or vb is None:
            continue
        pct = ((vb - va) / va * 100) if va else 0
        print("%-14s %11.2fs %11.2fs %+9.1f%%" % (
            stage.replace("sdHL", ""), va, vb, pct))

    print("\n%-14s %12s %12s %10s" % ("bsp lump", "before", "after", "delta"))
    changed = False
    for name, _, entsize in LUMPS:
        ea, eb = a["bsp"].get(name, {}), b["bsp"].get(name, {})
        key = "count" if entsize else "bytes"
        va, vb = ea.get(key), eb.get(key)
        if va is None or vb is None:
            continue
        same_hash = ea.get("sha1") == eb.get("sha1")
        mark = "" if same_hash else "  <-- CONTENT CHANGED"
        if not same_hash:
            changed = True
        print("%-14s %12d %12d %+10d%s" % (name, va, vb, vb - va, mark))

    if changed:
        print("\nBSP OUTPUT DIFFERS - review intentionality")
        if not (a.get("threads") == 1 and b.get("threads") == 1):
            print("NOTE: at least one run was not --threads 1. Multi-threaded "
                  "output is nondeterministic;\n      re-compare with "
                  "--threads 1 before treating this as a real regression.")
    else:
        print("\nBSP output byte-identical - no regression")
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--tools", help="directory holding the compiled tools")
    ap.add_argument("--map", help="path to a .map file")
    ap.add_argument("--runs", type=int, default=1)
    ap.add_argument("--threads", type=int)
    ap.add_argument("--json", help="write the fastest run's stats here")
    ap.add_argument("--compare", nargs=2, metavar=("BEFORE", "AFTER"))
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args()

    if args.compare:
        return cmd_compare(args)
    if not args.tools or not args.map:
        ap.error("--tools and --map are required unless --compare is used")
    return cmd_run(args)


if __name__ == "__main__":
    sys.exit(main())

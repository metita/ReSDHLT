#!/usr/bin/env python3
"""Exercise the tools exactly as users receive them, next to settings.txt."""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path


TOOLS = ("sdHLCSG", "sdHLBSP", "sdHLVIS", "sdHLRAD", "sdRIPENT")
PROGRAM_SELECTORS = tuple(f"<{name}>" for name in TOOLS)

# Every CSG branch which consumes argv[++i] now goes through the same guard.
# -console is parsed earlier by the shared console bootstrap, so it has its own
# clean usage failure but cannot emit the guard's diagnostic.
CSG_VALUE_OPTIONS = (
    "-threads",
    "-worldextent",
    "-dev",
    "-cliptype",
    "-nullfile",
    "-wadinclude",
    "-texdata",
    "-lightdata",
    "-brushunion",
    "-tiny",
    "-hullfile",
    "-wadcfgfile",
    "-wadconfig",
    "-scale",
    "-lang",
    "-mergesize",
)

# Missing-value coverage for every always-built parser. A return code of 1 is
# the tools' normal Usage()/Error() path; Windows fast-fail codes catch the
# out-of-bounds argv reads which this test was added to prevent.
VALUE_OPTIONS = {
    "sdHLCSG": CSG_VALUE_OPTIONS,
    "sdHLBSP": (
        "-threads", "-dev", "-subdivide", "-maxnodesize", "-texdata",
        "-lightdata", "-lang",
    ),
    "sdHLVIS": (
        "-threads", "-dev", "-texdata", "-lightdata", "-maxdistance", "-lang",
    ),
    "sdHLRAD": (
        "-bounce", "-dev", "-threads", "-chop", "-texchop", "-scale",
        "-fade", "-ambient", "-limiter", "-lights", "-gamma", "-dlight",
        "-sky", "-smooth", "-smooth2", "-coring", "-texdata", "-lightdata",
        "-vismatrix", "-dscale", "-colourgamma", "-colourscale",
        "-colourjitter", "-jitter", "-minlight", "-skylevel", "-softsky",
        "-gpuadapter", "-drawsample", "-compress", "-rgbcompress", "-depth",
        "-blockopaque", "-waddir", "-texreflectgamma", "-texreflectscale",
        "-blur", "-texlightgap", "-lang",
    ),
    "sdRIPENT": ("-texdata", "-lightdata", "-lang"),
}


def fail(message: str, output: str = "") -> None:
    print(f"FAIL: {message}", file=sys.stderr)
    if output:
        print(output[-4000:], file=sys.stderr)
    raise SystemExit(1)


def find_tool(tools_dir: Path, name: str) -> Path:
    suffixes = (".exe", "") if os.name == "nt" else ("", ".exe")
    for suffix in suffixes:
        candidate = tools_dir / f"{name}{suffix}"
        if candidate.is_file():
            return candidate
    fail(f"missing packaged tool: {name} in {tools_dir}")
    raise AssertionError("unreachable")


def run(tool: Path, *args: str) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            [str(tool), *args],
            cwd=tool.parent,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            errors="replace",
            timeout=20,
            check=False,
        )
    except subprocess.TimeoutExpired as exc:
        fail(f"{tool.name} timed out", (exc.stdout or "") + (exc.stderr or ""))
        raise AssertionError("unreachable")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--tools-dir",
        required=True,
        type=Path,
        help="directory containing all five tools and settings.txt",
    )
    args = parser.parse_args()
    tools_dir = args.tools_dir.resolve()

    settings_path = tools_dir / "settings.txt"
    if not settings_path.is_file():
        fail(f"settings.txt is not beside the packaged tools in {tools_dir}")

    settings = settings_path.read_text(encoding="utf-8")
    missing_selectors = [value for value in PROGRAM_SELECTORS if value not in settings]
    if missing_selectors:
        fail(f"settings.txt has no selector for: {', '.join(missing_selectors)}")
    if any(line.strip() == "#define -low" for line in settings.splitlines()):
        fail("settings.txt silently forces low process priority")

    tools = {name: find_tool(tools_dir, name) for name in TOOLS}

    for name, tool in tools.items():
        result = run(tool)
        if result.returncode != 1:
            fail(f"{name} usage returned {result.returncode}, expected 1", result.stdout)
        if "Half-Life Compilation Tools" not in result.stdout:
            fail(f"{name} did not print its banner", result.stdout)
        if "Unknown option" in result.stdout:
            fail(f"{name} rejected an option injected by settings.txt", result.stdout)
        print(f"ok: packaged {name} starts with settings.txt")

    for name, options in VALUE_OPTIONS.items():
        tool = tools[name]
        for option in options:
            result = run(tool, option)
            if result.returncode != 1:
                fail(
                    f"{name} {option} returned {result.returncode}, expected clean usage exit 1",
                    result.stdout,
                )
            if name == "sdHLCSG":
                expected = f"Error: option '{option}' requires a value"
                if expected not in result.stdout:
                    fail(f"{name} {option} did not diagnose the missing value", result.stdout)
        print(f"ok: {name} rejects {len(options)} missing-value cases cleanly")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())

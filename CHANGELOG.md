# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] - ReSDHLT
Fork of seedee/SDHLT focused on compile performance and map FPS for Counter-Strike 1.6.

### Changed
- RAD: `GatherSampleLight()` no longer computes the `-texlightgap` texture basis
  on every call (two cross products and two divides, ~1M calls per map) when the
  option is off, which is the default; it is built on first use
- RAD: `GatherSampleLight()` clears and scans only the light styles a sample
  actually receives, instead of memsetting 768 bytes and branching over all 64
  on every call
- RAD: `CalcSightArea()`, `CalcSightArea_SpotLight()` and
  `snap_to_winding_noedge()` take their scratch buffers from the stack instead of
  a `malloc`/`free` pair per call
- All three verified byte-identical on `ba_dust_island`. Measured gain is small
  (~1.5-2% of RAD CPU time, at the edge of this machine's noise): the compile is
  dominated by ray casting, not by these overheads
- RAD: `TestLine_r()` walks the BSP in a loop instead of recursing on its three
  tail positions. It is entered ~1.85 billion times per map, so the call
  overhead was worth removing; genuine two-way splits still recurse
- RAD: `TestSegmentAgainstOpaqueList()` first tests the ray against one box
  covering every opaque entity, instead of walking the list per ray
- RAD: samples whose PVS reaches no sky-touching leaf skip the sky loop
  entirely. An indoor luxel was firing 4098 occluded rays at `-skylevel 6`;
  the PVS is conservative, so a negative answer cannot darken anything that was
  lit before. Switches itself off if no sky face is found, rather than risk a
  black map
- `SDHLT_ARCH` (default `avx2`, applied to **RAD only**): 4-5% off RAD's CPU
  time with byte-identical output. RAD only because building CSG/BSP/VIS with
  `/arch:AVX2` changed their floating-point output and produced a `koth_sandy`
  .bsp whose vis data made RAD abort with "DecompressVis Overflow" -
  reproducible, and gone as soon as those three are built portably.
  `-DSDHLT_ARCH_ALL=ON` opts into the risky version; `-DSDHLT_ARCH=` builds a
  fully portable set. **An AVX2 build will not start on a CPU older than 2013**
- Combined, all of the above: 5-13% off RAD's CPU time depending on map and run
  (measured on `ba_dust_island` and `koth_sandy`, interleaved pairs, process CPU
  time), with every .bsp byte-identical to the unoptimised build
- `zhlt_embedlightmap` produces 24-34% less texture data. It bakes one texture
  per face, so a map using it grows enormously (measured: 673 KB to 12.5 MB on
  ba_dust_island with 190 faces). Two changes, neither touching the lighting:
  baked textures are no longer rounded up to a power of two (GoldSrc only needs
  multiples of 16, and the rounding wasted up to 4x the pixels;
  `zhlt_embedlightmappoweroftwo` restores it), and faces that bake byte-identical
  textures now share one. `zhlt_embedlightmapresolution` remains the setting that
  actually decides the size: each doubling divides it by four. See
  docs/BENCHMARKS.md
- CSG's `MAX_WADPATHS` raised from 128 to 512 (and `MAX_TEXFILES` with it). The
  old ceiling is easy to hit with a large texture library, and CSG aborted with
  "too many wad files" instead of ignoring the excess. Each slot is a pointer
  plus an open `FILE*`, so CSG now also raises the CRT stream limit
  (`_setmaxstdio`) on Windows to keep the handles available. Verified with a
  201-WAD compile

### Tried and rejected
- Sampling the sky once per lightmap pixel instead of once per `-extra`
  subsample (`-fastsky`): measured **slower** than not doing it (12.0% vs 13.6%
  on `ba_dust_island`, 7.1% vs 9.2% on `koth_sandy`) because the luxel centre
  then walks the light list twice, and it changed the lighting on top of that.
  Removed

### Fixed
- `zhlt_embedlightmap` on a `func_water` turned the water into an ordinary
  surface: no waves, no fog. The baked texture was renamed from `!leanwater_w5`
  to `__rad...`, and GoldSrc decides a surface is water by that leading `!`. The
  requantised palette also destroyed entries 3 and 4, where the engine keeps the
  water's fog colour and density. Both paths existed in the upstream source but
  were commented out; they are enabled and verified against a real map
- `SDHLT_ARCH=avx2` produced `/arch:avx2`, which MSVC ignores with "command line
  warning D9002": every release so far was built without the vectorisation it
  claimed. The flag is upper-cased now
- `gui/`: the executable's version resource was hardcoded in `assets/icon.rc`
  and kept reporting 0.1.0 in the file properties no matter what was released.
  `build.rs` now generates the whole resource script from `CARGO_PKG_VERSION`
- `gui/`: toolbar buttons pinned to the right edge of a row were drawn on top of
  the ones next to them once the panel got narrow ("Limpiar intermedios" over
  "Actualizar", the delete button over the project actions, the status line
  under the footer buttons). A label that truncates claims the whole row, so
  everything now shares one left-to-right flow that wraps instead
- Linux builds no longer compile single-threaded by default. `DEFAULT_NUMTHREADS`
  was `1` on POSIX, which made the autodetection branch in `ThreadSetDefault()`
  dead code
- Windows machines with more than 32 logical processors no longer fall back to a
  single thread
- Stack buffer overflow when `-threads` is given a value above `MAX_THREADS`
  (e.g. `-threads 5000` crashed with SIGSEGV); the count is now clamped with a
  warning
- CMake Release builds are no longer silently downgraded from `-O3` to `-O2` by a
  hardcoded `add_compile_options(-O2)`
- CMake no longer produces unoptimised binaries when `CMAKE_BUILD_TYPE` is unset
  with single-config generators
- CI ran `ctest` with no registered tests and always failed that step

### Added
- `gui/`: updates from GitHub Releases. Checks once a day, offers the update in
  a window with the release notes, and swaps the executable and `tools/` with a
  helper script once the GUI has exited. Only releases count, so pushing to
  master does not nag anybody. No new crates: `curl.exe` plus the `serde_json`
  that was already there, and downloads are only accepted from `github.com`
- `.github/workflows/release.yml`: builds the tools and the GUI on a Windows
  runner when a `v*` tag is pushed, refuses to publish a package missing any of
  the executables or `sdhlt.wad`, and checks the tag against `gui/Cargo.toml`
- `gui/`: dark-theme map compiler front-end in Rust/egui, with per-option
  explanations and Draft/Recommended/Release presets built from the measured
  results. Not compiled or tested - see `gui/README.md`
- RAD `-skylevel N` (4-8): controls how finely the sky hemisphere is sampled.
  Measured, the skylight loop is ~96% of all rays RAD casts
- RAD `-profile`: reports where RAD spends its time, with no external profiler.
  Inner ray-casting counters behind `-DSDHLT_PROFILE=ON` (off by default because
  `TestLine_r` is entered ~1.8 billion times per map)
- `docs/PERFILAR_RAD.md`: how to profile RAD on real hardware
- Reproducible compiles: CSG's parallel phases now run single-threaded by
  default, because `FindIntPlane` numbered planes and `WriteFace` ordered faces
  by thread timing. Costs ~0.1% of total compile time. `-nodeterministic` opts out
- `scripts/bspcheck.py`: geometric BSP validation (planarity, convexity,
  degenerate faces, surface area, residual merge opportunities)
- `scripts/compilebench.py`: times a full compile and fingerprints every BSP lump
  for regression checking
- `install` target and CPack packaging (ZIP on Windows, TGZ elsewhere)
- Opt-in `SDHLT_LTO` CMake option
- `docs/FPS_Y_TOOL_TEXTURES.md` and `docs/BENCHMARKS.md`

### Changed
- **RAD defaults to `-skylevel 6` instead of 7: ~1.65x faster RAD** (ba_dust_island
  10.09s to 6.21s) for a maximum per-luxel difference of 1/255. This is the one
  deliberate change to lighting output in this fork; `-skylevel 7` reproduces
  upstream's lighting byte for byte
- Threading unified on `std::thread`, replacing the separate Win32 and pthread
  backends (723 lines to 448). Worker handles are now a `std::vector` sized by
  the real thread count instead of stack arrays sized by `MAX_THREADS`
- `MAX_THREADS` raised from 64 to 256
- Work units are claimed with an atomic instead of the global thread lock
- Added opt-in `SDHLT_LTO` CMake option
- CI now smoke tests all five tools instead of running an empty test suite

### Verified
- Upstream and ReSDHLT produce a byte-identical BSP on koth_sandy and
  ba_coliseum with `-threads 1`, lump for lump: all changes preserve output
- Compiling koth_sandy went from 2.88s to 1.71s by actually using both cores

### Known limitations
- The tools are nondeterministic when multi-threaded: two runs of the same
  binary can differ by a few clipnodes/marksurfaces. Use `-threads 1` for
  reproducible output
- Sky visibility culling and the iterative `TestLine_r` were both measured as
  no-gain earlier in this fork's life and are now in the tree anyway (see
  Changed). They were re-measured together with the other RAD work as a 5-13%
  win with byte-identical output; on their own, on outdoor maps, the earlier
  reading of "no measurable difference" may well still hold. Neither has been
  measured on an indoor-heavy map, which is where the sky cull should pay
- The cost of RAD is the number of rays cast, not the price of each. Anything
  much better than a few percent has to be algorithmic
- Face merging was investigated and left unchanged: residual merge candidates
  are 0.7% of faces, and `MAXEDGES` is never the binding constraint

## [1.2.0] - Jul 11 2024
### Changed
- Add studiomodel shadows with 3 shadow modes and `-nostudioshadow`
- Add *info_portal* and *info_leaf*
- Add *info_minlights* and `%` texture flag
- Add `-pre25`, increase `-limiter` default to `255`
- Increase `-bounce` to min `12` if using `-expert`
- Enable `-wadautodetect` by default
- Reformatted texture-related logging to look like resgen
- Add CMake config and Makefile

### Fixed
- Potential buffer overrun in `PushWadPath`

## [1.1.2] - Sep 09 2022
### Changed
- Reasons for skipping portal file optimisation process are more detailed

### Fixed
- Fatal errors replaced with generic log messages when skipping optimisation of portal file

## [1.1.1] - Aug 27 2022
### Changed
- Portal file optimisation process more streamlined. Separate .prt file is no longer created, instead the same one is optimised after VIS compilation
- Automatic embedding of tool texture WAD file is hard-coded again due to lazy mappers
- -chart parameter now enabled by default

### Fixed
- Bug with -worldextent CSG parameter, where the default map size was +/-2048

## [1.1.0] - Jul 04 2020
### Added
- -worldextent CSG parameter. Extends map geometry limits beyond +/-32768
- Optimised portal file workflow for J.A.C.K, allowing import of .prt file into the editor directly after BSP compilation
- Higher resolution image textures to tool texture WAD file

## 1.0.0 - Mar 09 2020
### Added
- BEVELHINT tool texture, which acts like SOLIDHINT and BEVEL. Eliminates unnecessary face subdivision and bevels clipnodes at the same time
- SPLITFACE tool texture. Brushes with this texture will subdivide faces they touch along their edges
- !cur_ tool textures, which act like CONTENTWATER and func_pushable with a speed of 2048 units/s
### Changed
- Automatic embedding of tool texture WAD file can now be controlled in settings.txt

[1.1.2]: https://github.com/seedee/SDHLT/compare/v1.1.1...v1.1.2
[1.1.1]: https://github.com/seedee/SDHLT/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/seedee/SDHLT/releases/tag/v1.1.0

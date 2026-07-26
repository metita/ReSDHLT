# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] - ReSDHLT
Fork of seedee/SDHLT focused on compile performance and map FPS for Counter-Strike 1.6.

### Fixed
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
- No RAD optimisation was attempted: profiling was inconclusive in the test
  environment (see `docs/BENCHMARKS.md`)
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

# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- RAD: ray-traced ambient occlusion ported from seedee/SDHLT (`-ao`,
  `-aoscale`, `-aogain`, `-aolevel`, `-aominweight`, `-aoopacity`,
  `-aocolor`), including upstream's texlight exemption and color fixes. Not
  ported: `-aostats` and `-aostudiomode`, which need the newer studio trace
  controls this RAD does not have. With AO the direct-light gather runs on the
  CPU even under `-gpu`; transfers still use the GPU.
- RAD: `-aoall`, a ReSDHLT extension. Upstream AO only darkens light gathered
  per sample, so maps lit by texlights (fast texlights and bounces are added at
  patch level) got no visible AO. `-aoall` records the occlusion of each point,
  blends it with the same weights as the light and darkens the final light of
  every sample. On ze_elysium: 27% of faces darker, +4% RAD time.
- CSG: `-convexgap #` sets how far the editor solid and the plane solid must
  differ before a non-planar brush is rebuilt.

### Changed
- CSG: non-planar brushes are only rebuilt when the mismatch is at least 0.2
  units (was `ON_EPSILON`). Rebuilding the 1081 candidates of ze_elysium, most
  off by a few hundredths, added 17% to the faces a leaf sees; 48 rebuilds cost
  2% and still close the zpa_house seams.

Without `-ao` or `-aoall`, RAD writes the same file as 0.10.3. Maps with no
non-planar brushes (ar_pokemon, zm_azteca, zm_eichen_v2) compile byte-identical
end to end.

## [0.10.3] - 2026-09-24

### Fixed
- CSG: a map near `MAX_MAP_CLIPNODES` that compiled with 0.10.0 could exceed it
  with 0.10.2. Rebuilt non-planar brushes split bent faces into triangles, and
  every extra side became extra clipnodes (ze_elysium, 1081 rebuilt brushes,
  went from 32083 to over 32767). The clip hulls now expand from the brush as
  written; only what is drawn uses the rebuilt one. ze_elysium is back to 32038
  with `-nohull2`, and the seams stay closed.

## [0.10.2] - 2026-09-24

### Fixed
- BSP: maps that compiled with 0.10.0 could leak with 0.10.1, so no portal file
  and no VIS. The finer portal cut of 0.10.1 turned slits between brushes the
  editor meant to touch (2079.98 against 2080) into real portals, and the
  outside fill went through them. Portals narrower than `ON_EPSILON` are closed
  again for the outside fill, the leak trail and `FillInside`.
- CSG: brushes with non-planar faces left by vertex manipulation are rebuilt as
  the solid the editor shows. A `.map` face is three points, so CSG used to
  build a different solid, up to a unit off, and see-through seams opened
  between neighbouring brushes. For J.A.C.K. and Hammer maps the brush becomes
  the convex hull of the points it writes, plus any corner it never writes,
  snapped to the vertex a neighbouring brush writes when the plane intersection
  put it a fraction of a unit away. Brushes whose points fit their planes are
  untouched. `-noconvexfix` turns it off.

### Changed
- `scripts/holecheck.py` counts a spot as covered when any face the engine
  draws crosses the ray there, not only faces on the node plane, and aims at
  `func_detail` faces too. The 28 spots reported for 0.10.1 were such cases.

## [0.10.1] - 2026-09-24

### Fixed
- BSP: faces built with vertex manipulation no longer vanish in game while the
  editor shows a closed solid. Neighbouring faces left on planes that differ by
  more than CSG merges, yet stay within `ON_EPSILON` of each other, made three
  things go wrong: a node whose portal was clipped away was collapsed together
  with the visible face on it; the portal between the room and the thin empty
  leaf in front of such a face went to the wrong child, so `FillInside` filled
  that leaf; and an ambiguous leaf always became SOLID because of a CSG sliver.
  Nodes that still own faces are kept, node portals are split with a 0.001
  tolerance, and an ambiguous leaf takes the contents covering most of its
  boundary. On 32 generated vertex-manipulated maps the see-through spots went
  from 469 (13 maps) to 28 (2 maps, both with sub-unit deformation); three
  real maps compile to the same geometry as before.

### Added
- `scripts/holecheck.py`: ray-casts a compiled BSP and reports see-through
  holes, optionally aiming at every visible face of the source `.map`.

## [0.10.0] - 2026-09-24

### Fixed
- VIS: `-full` output no longer depends on thread timing. A portal pruned with
  the visbits of whichever portals other threads had finished, so two identical
  multithreaded compiles could write different `.bsp` files. Every thread count
  now writes the same `.bsp` as `-threads 1` (verified on 13 maps, Windows and
  Linux).

### Changed
- VIS: portals flow in a fixed order and a waiting thread helps finish the
  portal it waits for by running subtrees of its recursion. Near-linear scaling
  (5.98x on 12 threads for a 6334-portal map).
- VIS: separator candidates are decided without normalizing the plane when the
  result is certain within twice the rounding error bound, the degenerate-normal
  test no longer needs a `sqrt`, `ChopWinding` counts sides without branches and
  the bitset loops work 64 bits at a time. About 20% less work per flow on one
  thread; output byte-identical.
- CSG: plane lookups use a hash on the quantized normal instead of scanning
  every plane (same plane numbers), and `CSGBrush` runs on all threads with
  per-brush output buffers written in brush order (same `.p`/`.b` files).
  CSG is 1.3x to 2.4x faster on the maps measured.

## [0.9.0] - 2026-08-15
Fork of seedee/SDHLT focused on compile performance and map FPS for Counter-Strike 1.6.

### Changed
- Release security: rotated the Ed25519 signing key, updated the pinned public
  key in the GUI and signer, and stored the matching private seed only in the
  GitHub Actions secret.
- Portability: removed obsolete C++ `register` specifiers so the RAD target
  builds cleanly with Clang in C++17 mode.
- RAD/Vulkan: added a generated style-0 bounce kernel with device-local
  accumulation and a safe CPU fallback. It is enabled for one-bounce runs after
  validating opaque/style-0 inputs; RGB transfers, transparency, custom shadows,
  style remapping, GPU errors, and multi-bounce runs remain on the deterministic
  CPU path. `scripts/gen_spirv.py --only bounce` regenerates only this shader.
- Build hygiene: cleaned the first `/W4` warning tranche in the shared file,
  logging, BSP and CSG paths (assignment-in-condition, signed/unsigned loop
  bounds, shadowed locals and proven initialization cases). The opt-in
  `SDHLT_WARNINGS_AS_ERRORS=ON` build now passes all targets while keeping
  `/W4` visible in normal builds: a documented allowlist isolates the remaining
  legacy RAD/VIS/RIPENT warning debt; warnings outside that allowlist still
  fail the strict build.
- RAD/Vulkan: immutable gather and form-factor scene buffers now use host-visible
  staging plus device-local storage. Sparse form-factor dispatches have two
  independent command-buffer/fence slots, so the next pair batch is submitted
  while the previous result is collected and packed. Transfer counts remain
  identical; the known bounce fixture still has its pre-existing one-byte
  lighting delta (`191` versus `192`) outside the transfer pipeline.
- Releases/updater: every runtime and symbol ZIP now gets a detached Ed25519
  signature (`.sig`) in addition to SHA-256. The updater verifies the exact
  portable ZIP against the pinned public key before unpacking; CI refuses to
  publish if `RESDHLT_ED25519_PRIVATE_KEY_B64` is missing or mismatched.
- Build hygiene: the release signer is a small audited Rust binary and the
  signature key material is never stored in the repository; `--generate-key`
  prints a new seed/public-key pair for rotating the documented CI secret.
- Build/release: the portable build is now the default. The `portable` and
  `avx2` CMake presets use isolated output trees, RelWithDebInfo, optional
  compiler caching, separated symbols, package smoke tests, and SHA-256
  manifests. The traditional `cmake -B build -S .` layout remains available
  for map-editor integrations.
- RAD: `-gpuauto` selects CPU or Vulkan independently for direct-light gather
  and Sparse transfer-factor generation. `-gpu-gather`, `-gpu-transfers`,
  `-nogpu-gather`, and `-nogpu-transfers` expose the phases explicitly. Vulkan
  uploads the immutable form-factor scene once and reuses pair/result buffers
  across bounded dispatches. A CPU/GPU run with six transfer dispatches
  produced the same SHA-256 BSP as the CPU reference.
- GUI: preview and execution now consume the same immutable `CompilePlan`;
  preflight validates stage dependencies and missing tools, cancellation is
  reported separately, destructive controls are locked during jobs, and a
  workspace lock prevents concurrent compiles from corrupting intermediates.
- GUI updater: checks and downloads remain asynchronous, but installation now
  requires an explicit click, the exact portable Windows asset, a matching
  SHA-256 manifest, and a rollback-capable swap. The GUI also refuses a second
  instance so project state cannot be overwritten by two windows.
- RAD: `-gpu` now also computes Sparse patch transfer factors on Vulkan. The
  old path tested the full patch-pair space on the CPU even though the Sparse
  visibility matrix already named the useful pairs. The GPU path walks those
  stored pairs directly, dispatches them in bounded batches, and feeds the
  existing packed transfer lists used by every bounce. RGB transfers,
  translucent patches, custom bounce shadows, and GPU errors fall back to the
  original CPU `MakeScales`. A sealed-room parity test produced the same 95,742
  transfers and differed by one lighting byte (`191` vs `192`) out of 15,102.
- `gui/`: the launch check runs asynchronously and reports available releases,
  but installation always requires the explicit `Actualizar ahora` action.
  The menu checkbox is `Buscar actualizaciones al abrir`; updates are blocked
  while a compile is running.
- `gui/`: when idle the app now asks for a repaint every 30s while update
  checks are enabled. egui sleeps until an input event, and the reply from the
  check thread is not one, so an app left open for days would sit there having
  finished a check nobody ever drained
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
- `SDHLT_ARCH` (portable by default, with optional `avx2` applied to **RAD
  only**): 4-5% off RAD's CPU time with byte-identical output. RAD only because
  building CSG/BSP/VIS with
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
- RAD: `InterpolateSampleLight()` works out the sample's phong normal once
  instead of once per candidate patch. `CalcAdaptedSpot()` was calling
  `GetPhongNormal()` with the same (surface, position) for every patch it
  considered: 8.4 million calls for 105k samples on `ba_dust_island`, ~80
  identical recomputations per sample. `AddPatchLights` is 15.8% faster on that
  map (2.79s to 2.35s, one thread, interleaved pairs, best of five) and it is a
  quarter of RAD, so ~4% off a compile. Maps with little lerp work see less.
  .bsp byte-identical on `ba_dust_island` and `ar_pokemon`
- RAD: the sample interpolation reuses its scratch buffers per thread instead of
  allocating them per sample and per candidate patch. `CalcWeight()` and
  `CalcInterpolation()` each built a `std::vector` on every call, and every
  accepted candidate got a `new interpolation_t` carrying another one: ~1.2
  million allocations per map on `ba_dust_island`. `AddPatchLights` is 21.5%
  faster (2.37s to 1.86s) and RAD as a whole 6.4% (10.01s to 9.37s), one thread,
  interleaved pairs, best of five. Combined with the phong normal change above,
  that phase went from 2.79s to 1.86s, a third off. .bsp byte-identical on
  `ba_dust_island` and `ar_pokemon`, at one thread and at six
- CSG's `MAX_WADPATHS` raised from 128 to 512 (and `MAX_TEXFILES` with it). The
  old ceiling is easy to hit with a large texture library, and CSG aborted with
  "too many wad files" instead of ignoring the excess. Each slot is a pointer
  plus an open `FILE*`, so CSG now also raises the CRT stream limit
  (`_setmaxstdio`) on Windows to keep the handles available. Verified with a
  201-WAD compile

### Added
- CLI/package smoke coverage now exercises all five shipped tools beside
  `settings.txt`, including 71 missing-value cases across CSG, BSP, VIS, RAD,
  and RIPENT. CI runs the same checks on every push and pull request, together
  with GUI formatting, Clippy, tests, and release compilation.
- GUI RAD controls now expose automatic GPU selection, gather and Sparse
  transfer phases, GPU adapter selection, BSP `-lmoptimize`, and `-allleaks`.
- Project persistence now lives under `%LOCALAPPDATA%/ReSDHLT`, migrates the
  old executable-local files, flushes atomically, keeps a backup, and surfaces
  corrupt JSON instead of silently resetting it.
- RAD: `-raybench` captures a bounded sample of the real sky rays generated by
  the map and reports the BSP tracer's raw throughput. An optional
  `SDHLT_EMBREE` build compares those same rays against Embree without changing
  normal compilation behavior
- RAD: `-workbench` records per-face timings for `BuildFacelights` and
  `AddPatchLights`, then reports natural, sorted, and ideal scheduling bounds
  so load imbalance can be measured instead of guessed
- RAD: the lightmap atlas budget is checked before any lighting work. GoldSrc
  packs every lit face into 64 pages of 128x128 luxels and aborts the map load
  with "AllocBlock: full" if they do not fit, which used to be discovered after
  a full compile - or in game. RAD now runs the engine's own allocator up front,
  and past 95% of the budget it prints a breakdown by texture ranked by lightmap
  footprint, so the textures worth rescaling are named. Over the limit it is an
  error; `-noallocblockcheck` compiles anyway
- BSP: `-lmoptimize` reorders faces to waste fewer atlas pages. The engine packs
  in face order, first fit, and never backtracks, so feeding it the big
  rectangles first leaves less unusable space behind the small ones. Three legal
  orders are measured against the engine's allocator and the best one is kept,
  so it can never come out worse. Faces stay inside the node that owns them;
  `node->firstface` and the marksurface table are remapped to follow. Geometry,
  texture scale and lightmap resolution are untouched. Off by default: the
  default build still writes byte-identical .bsp files
- BSP: leak diagnostics point at the hole. The pointfile used to be whatever
  path the outside flood fill unwound through - it wanders, doubles back, and
  never says where the map actually opens. The trail is now built after the
  leak is proved, as a Dijkstra over the portal graph: the geometrically
  shortest way from the leaked entity to the void, simplified with
  Douglas-Peucker so the `.lin` file is a few clean segments. The hole itself -
  the first portal on that path leading into the void - gets its coordinates
  printed and a dense marker star written into the `.pts`, so it is
  unmistakable in the editor. Every hull that leaks is consolidated into one
  report instead of one warning per hull
- BSP: `-allleaks` surveys the map and reports every hole, not just the first
  one found, so a leaky map can be sealed in one pass through the editor
  instead of one hole per compile

- RAD: `-gpu` gathers direct lighting with Vulkan compute. Ported from
  speedrun-16/hltools. BuildFacelights runs twice: a collect pass records every
  `GatherSampleLight()` call as a work item, the kernel evaluates all of them,
  and the real pass replays the same code path reading the stored results.
  Everything outside the gather is the untouched CPU code, and the parts the
  kernel cannot do exactly - the texlight near branch, which needs the
  emitter's winding and a sight-area integration - come back to the CPU and are
  resolved with the reference functions. **The .bsp comes out byte-identical**
  on `ba_dust_island` and `ar_pokemon` with and without `-extra`, and on four
  of five light-count fixtures; the fifth differs by one byte in 95,982, by one
  step out of 255, because the kernel normalizes in float where the CPU does
  not. `SDHLT_GPU_FP64_NORMALIZE=1` selects the double-precision parity variant
  for comparison. It declines
  and leaves RAD on the CPU path, with a reason on the console, for opaque
  entities, studio shadows, more distinct light styles than the kernel has
  slots, a BSP deeper than its traversal stack, or no Vulkan driver.
  `-gpuadapter #` picks the device by index.

  BuildFacelights is split in two halves that each run once - the first does
  sample placement, phong normals, PVS and records the gather calls; the second
  drops the device's answers where the CPU gather would have written them and
  carries on with the blur, the patches and the lightmap. The state between
  them is carried in a `facebuild_t`, and faces are processed in memory-bounded
  batches because the lmcache is megabytes per face with `-extra`. Without
  `-gpu` the two halves run back to back and it is the same function it was

  **What decides whether it wins is how many direct lights the map has.** Same
  room, same settings, only the light count changing, GTX 1060 against 6
  threads with `-extra`: 1 light 0.66s CPU vs 1.32s GPU (half the speed), 256
  lights 1.59s vs 1.29s, 512 lights 2.27s vs **1.31s**, 1024 lights 3.91s vs
  **1.72s** - 2.27x. CPU time grows linearly with the light count because every
  sample walks the lights its PVS can see one at a time; device time barely
  moves (0.28s to 0.56s across a 1024x increase), which is the thing a GPU is
  for. The crossover is around 150-200 lights.

  That is why `ba_dust_island` looks bad and always will: it marshals to a
  single light in a single leaf, the worst possible case. It is also why
  hltools measures 2.96x on a map whose CPU RAD takes 115s - same curve,
  different point on it. Off by default because the answer depends on the map;
  `-gpu` prints the light count and a collect/device/finish breakdown so the
  call can be made from numbers. docs/BENCHMARKS.md §4.8
- The Vulkan headers and the compiled SPIR-V kernels are in the tree, so
  building needs no Vulkan SDK; `scripts/gen_spirv.py` regenerates them after a
  shader edit. `-DSDHLT_GPU=OFF` leaves the backend out entirely

### Tried and rejected
- Sampling the sky once per lightmap pixel instead of once per `-extra`
  subsample (`-fastsky`): measured **slower** than not doing it (12.0% vs 13.6%
  on `ba_dust_island`, 7.1% vs 9.2% on `koth_sandy`) because the luxel centre
  then walks the light list twice, and it changed the lighting on top of that.
  Removed

### Fixed
- Packaging: `settings.txt` now matches the actual `sdHL*` selectors, never
  injects RIPENT's unsupported `-low`, and no longer adds the nonexistent
  `-wadautodetect` option. CSG value options and `-worldextent` reject missing
  values cleanly instead of reading past `argv`; BSP `-threads` now updates the
  global thread count instead of a shadowing local.
- GUI: project renames preserve the active identity, MAP warnings refer to the
  selected project, renamed maps update `map_path`, stale checks refresh, paths
  are trimmed consistently, and status/cancel/update states are reported with
  the correct outcome. The custom toggle has keyboard and accessibility
  semantics, and the faint command text now meets the 4.5:1 AA contrast target.
- CSG: the `BEVELHINT` tool texture never did anything. `ParseBrush()` tested
  for `BEVEL` first, comparing only the first five characters, so `BEVELHINT`
  matched it, had its texture name overwritten with `NULL`, and never reached
  the test right below meant for it. The name was already gone by the time it
  was stored on the side, so all eight later `BEVELHINT` checks in CSG and BSP
  were unreachable and a mapper using it silently got a plain `BEVEL`. The test
  now runs before `BEVEL`, the way `BEVELBRUSH` already does - that one only
  survived the collision because it is ten characters long. Measured on a face
  buried inside a solid brush, which is the case the texture exists for: the
  surface file went from 361 lines (identical to `NULL`, i.e. the face was
  discarded) to 373 (identical to `SOLIDHINT`), with `NULL` unchanged
- `tools/sdhlt.fgd`: documented `SOLIDHINT` and `BEVELHINT`, including the trap
  that they turn into `NULL` on a face that is not solid on both sides, which
  leaves a hole in the wall with no warning
- `gui/`: updating the app left the compile running old binaries whenever the
  tools folder pointed somewhere else - a mapper using their editor's copy
  (`...\JACK\tools_sdhlt\`) got a new GUI over July's compilers, and the first
  sign of it was a compile dying on `Unknown option "-mergeentities"`. The
  updater only replaces the `tools` beside the executable, and it should not go
  writing into folders the user chose, so the fix is to notice instead: the
  folder in use is compared against the shipped one and, when the shipped one is
  newer, a warning offers to switch to it or to copy the new binaries over
- `zhlt_embedlightmap` on a `func_water` turned the water into an ordinary
  surface: no waves, no fog. The baked texture was renamed from `!leanwater_w5`
  to `__rad...`, and GoldSrc decides a surface is water by that leading `!`. The
  requantised palette also destroyed entries 3 and 4, where the engine keeps the
  water's fog colour and density. Both paths existed in the upstream source but
  were commented out; they are enabled and verified against a real map
- `zhlt_embedlightmap` broke every other surface GoldSrc recognises by texture
  name, not just water. The engine reads the name at load time, so renaming the
  baked texture turns the surface into an ordinary one: a `func_conveyor`
  (`scroll*`) stopped scrolling, and `water*`, `laser*` and `*` stopped being
  water. All four water spellings now bake as `!`, which is the same flag in one
  character. `scroll` needs six, so baked names have a second layout,
  `scroll_radTTTcc`, with the texinfo in base 62; the character after `_rad`
  tells them apart (digit = the old layout, letter = the new one), so a .bsp
  from an earlier version still reads. `{scroll` is seven characters and cannot
  keep both halves of its meaning, so those faces are left unbaked with a
  warning instead of silently losing one. Verified on a test map with all five
  cases; maps without them, maps baking an ordinary texture, and `func_water`
  with the key all compile byte-identical. See docs/BENCHMARKS.md
- `SDHLT_ARCH=avx2` produced `/arch:avx2`, which MSVC ignores with "command line
  warning D9002": every release so far was built without the vectorisation it
  claimed. The flag is upper-cased now
- `gui/`: "Carpeta por proyecto" did nothing when no output folder was set, and
  the hint explaining the layout was hidden in that case, so the toggle looked
  active while every intermediate still landed next to the .map. With no output
  folder the layout now applies where the map lives: no per-project subfolder
  (the .bsp stays where it is expected, beside the source), but the compile runs
  in `intermedios/` there and the .bsp is moved back up when it succeeds
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
- `gui/`: `-texchart` has a toggle under CSG, "Informe de coste de texturas",
  with the explanation aimed at the decision rather than the mechanism: what
  the `oversampled` column means and what to do at each value (4x or more,
  halve it; around 1x, leave it; below 1x, it tiles and halving will show).
  It also carries the reason the column exists at all, since reading the size
  column alone suggests savings that are not there
- CSG: `-texchart` reports what each texture costs the bsp. On a map compiled
  with `-nowadtextures` the texture lump is most of the file - measured at
  82.8% on `zm_azteca` and 69.9% on `zm_eichen`, against 8.7% and 15.4% for
  lighting - and `-chart` stops at a single `texdata` total, which is where the
  question starts. The report lists textures by bytes, flags any whose pixels
  are byte for byte identical, and crosses each one against the surface area it
  actually paints: the texture axes carry the texels-per-unit scale, so the
  pixels a texture ever displays is a number, not a guess. Anything with 4x
  more pixels than it displays can lose half its resolution in each axis and
  still have a pixel per pixel on screen. That distinction matters - a naive
  "halve everything 256px and up" projection claimed 70% off `zm_azteca`, and
  the real answer there is that nothing is oversampled at all, because those
  textures tile across very large surfaces. On `zm_eichen` it finds 3 textures
  worth halving (6.1%) and one duplicated pair. Off by default and it only
  reads back the lump CSG has already written
  field of 200 identical `func_illusionary` bushes costs one BSP model instead
  of 200. Models are the scarce resource here: the engine precache table they
  live in is shared with studio models and sprites. Entities are only folded
  when they carry the exact same keyvalues, have no name/target/origin of any
  kind, belong to a whitelist of purely static classes (`func_illusionary`,
  `func_wall`) and use a render mode the engine does not depth sort. Groups are
  clustered by proximity and capped at `-mergesize` units (1024 by default) so
  the merged model does not end up with a map-sized bounding box that defeats
  culling; `-mergeblend` lifts the render mode restriction, and `zhlt_nomerge`
  `1` opts a single entity out. Off by default, ignored under `-onlyents`.
  See `docs/MERGE_DE_ENTIDADES.md`
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

# Measurements

Everything here was measured with `scripts/compilebench.py` on real CS 1.6 maps.
I also record the **negative results**, because knowing that something does *not* work saves wasted time
later.

## Environment

- Linux, GCC 11.4, **2 cores** (sandbox)
- Maps: `ba_dust_island`, `ba_coliseum`, `koth_sandy`
- Measurement noise in this environment is **±8%**, which limits which conclusions can be
  drawn (see §3)

> With 2 cores, everything about thread scalability is **underestimated**. On an 8- or 16-core
> CPU the threading gains are considerably larger.

## 1. Where the compile time goes

| Map | CSG | BSP | VIS | RAD | Total |
|---|---|---|---|---|---|
| ba_dust_island | 0.03s | 0.04s | 0.01s | **8.57s** | 8.64s |
| ba_coliseum | 0.05s | 0.13s | 0.00s | **8.56s** | 8.75s |
| koth_sandy | 0.03s | 0.09s | 0.06s | **1.44s** | 1.62s |

**RAD is more than 95% of the time.** Optimizing CSG, BSP or VIS is irrelevant in practice;
any performance work has to go into RAD.

## 2. Impact of the threading fix (the most important result)

`koth_sandy`, best of 3:

| | CSG | BSP | VIS | RAD | Total |
|---|---|---|---|---|---|
| `-threads 1` | 0.03s | 0.06s | 0.10s | 2.69s | **2.88s** |
| `-threads 2` | 0.04s | 0.07s | 0.06s | 1.53s | **1.71s** |

**1.68× faster** from using both cores. This is exactly what every Linux build was losing,
because `DEFAULT_NUMTHREADS` was `1` on POSIX and the autodetection branch of `ThreadSetDefault()` was
dead code. On an 8-core machine the difference is much larger.

## 3. `-O2` vs `-O3` vs LTO: no measurable difference ❌

The first run (a single pass, `ba_dust_island`) suggested a clear improvement from LTO:

| | RAD |
|---|---|
| `-O2` | 9.21s |
| `-O3` | 8.75s |
| LTO | 8.10s |

But when repeated, the order **flipped**:

| | RAD (best of 2) |
|---|---|
| `-O2` | 8.51s |
| `-O3` | 8.26s |
| LTO | 8.68s |

A cleaner measurement (`-threads 1`, 4 runs, `koth_sandy`), showing every run:

| | totals | min |
|---|---|---|
| `-O2` | 2.75 / 3.22 / 2.81 / 2.81 | 2.75s |
| `-O3` | 2.74 / 2.76 / 2.82 / 2.89 | 2.74s |
| LTO | 2.73 / 3.52 / 3.10 / 2.65 | 2.65s |

**The variance within each variant (up to 0.87s) is larger than the difference between variants
(0.10s).** There is no evidence that `-O3` or LTO improve anything here.

**Decision:** LTO stays **opt-in** (`-DSDHLT_LTO=ON`), not the default. Turning LTO on by default
would lengthen builds with no demonstrated benefit. `-O3` stays simply because it is the
`Release` default in CMake.

Size of the `sdHLRAD` binary: `-O2` 442KB, `-O3` 482KB, LTO 438KB.

> If you want to decide for *your* machine, run this and see whether the difference exceeds your own variance:
> ```
> python3 scripts/compilebench.py --tools <dir> --map <map> --threads 1 --runs 5
> ```

## 4. Profiling RAD: solved, with one failed optimization attempt

RAD is >95% of the time, so it was the obvious target. The first attempt with external tools
failed and the second, instrumenting the tools, worked.

### 4.1 Why gprof was no use

- **`perf` is not available** in the environment (`perf_event_paranoid` blocks it), and it is Linux-only anyway.
- **`gprof` gave data that does not survive verification:** it reported `MakeTnode()` as 49.83% of the time with
  **93,301,204 calls**. I added a counter: it is called **78 times**. With symbols from another build, it even
  attributed 85 million calls to `_fini`.

`MakeTnode` is the only `static` function in `trace.cpp`, so it was absorbing the samples of the neighboring
code.

### 4.2 The built-in profiler (`-profile`)

It lives inside the tools and works on Windows without installing anything. Two levels, because the hot code
has very different frequencies:

- `PROF_SCOPE` **times** the phase functions; it is enabled at runtime with `-profile`.
- `PROF_CALL` **counts** the internal ray casting functions; it is enabled **at compile time** with
  `-DSDHLT_PROFILE=ON`. `TestLine_r` is entered ~1.8 billion times per map, so even a
  disabled check would cost almost a second per compile for someone who never profiles.

**Profile of `ba_dust_island`, 1 thread:**

| site | calls | share |
|---|---|---|
| `BuildFacelights` | 653 | 51.6% |
| `GatherSampleLight` | 146,152 | 47.9% (nested in the one above, ~94% of its time) |
| `TestLine` | 62,245,301 | (count only) |
| **`TestLine_r`** | **1,849,616,672** | (count only) |
| `TestSegmentAgainstOpaqueList` | 38,965,499 | (count only) |

This confirms the suspicion about gprof: the real hot spot is **`TestLine_r`**. And the counts match
the credible gprof entries (`TestSegmentAgainstOpaqueList` = 38,965,499 in both), which validates the
two measurements against each other.

I verified that the disabled instrumentation is free, measuring A/B **interleaved** to cancel the machine's
load: **−1.1%**, in other words noise. An earlier non-interleaved measurement suggested a 20% regression, which
was drift from other builds running on the same machine. It serves as a warning about measuring in a
shared environment.

### 4.3 The optimization attempt, and why it was reverted ❌

Three of the four recursive paths of `TestLine_r` are **tail calls**, so I rewrote it as a loop.
The lighting came out **byte-identical** on all three maps, so the transformation was correct. But it was
slower, twice:

| version | RAD (min of 4, `koth_sandy`, 1 thread) |
|---|---|
| original recursive | **2.59s** |
| iterative, copying endpoints | 2.69s |
| iterative, pointers to endpoints | 2.76s |

Reasons: GCC already emits fewer recursive call sites than the code suggests (10 in the body),
calls are cheap on modern CPUs thanks to the *return stack buffer*, and both of my versions spent more
on copies or pointer indirection than they saved.

**Reverted.** Only the instrumentation stayed.

### 4.4 Where the rays come from, and the real gain ✅

Counting rays by origin turned up the figure that mattered. `koth_sandy`, out of 17,068,356 `TestLine` calls:

| origin | rays | % |
|---|---|---|
| **skylight normals** | **16,358,753** | **95.8%** (67% of them occluded) |
| sun spread normals | 26,628 | 0.2% |
| everything else | 682,975 | 4.0% |

So **96% of the ray casting is the sky light loop**, which casts one ray for every normal of a
subdivided hemisphere. `g_numskynormals` is `{0, 6, 18, 66, 258, 1026, 4098, 16386, 65538}` and `-softsky`
chooses level 7 when on or level 4 when off: **16,386 rays or 258**, a 63× jump with nothing in between.

**Measured curve** (`koth_sandy`, 2 threads, compared against level 7):

| level | rays | RAD | vs level 7 | mean diff | max diff |
|---|---|---|---|---|---|
| 4 | 258 | 0.67s | **2.29×** | 0.43/255 | 5/255 |
| 5 | 1,026 | 0.70s | **2.19×** | 0.14/255 | 2/255 |
| **6** | **4,098** | **0.83s** | **1.84×** | **0.05/255** | **1/255** |
| 7 | 16,386 | 1.43s | 1.00× | 0 | 0 |

Level 6 is where the curve breaks: 4× fewer rays for a maximum difference of **1/255 per luxel**,
which cannot be seen. Confirmed on the heavy map `ba_dust_island`: 10.07s → 6.05s, maximum 1/255, and only
3.4% of luxels change.

**Implemented:** `-skylevel N` (4 to 8) and **default changed to 6**.

| map | new default (6) | `-skylevel 7` | gain |
|---|---|---|---|
| koth_sandy | 0.99s | 1.64s | **1.65×** |
| ba_dust_island | 6.21s | 10.09s | **1.63×** |

This is **the only deliberate deviation** from equivalence with upstream in the whole fork. I verified that
`-skylevel 7` reproduces upstream's `lighting` lump **byte for byte** (sha1 `40c7dc1ac1a3da50` in both),
so the old behavior is one flag away.

### 4.5 Per-leaf sky culling: measured and ruled out ❌

The next idea looked like the best one: 67% of sky rays end up occluded, so for an
indoor sample thousands of rays are cast just to discover that the sky is not visible. A per-leaf
visibility test would skip the whole loop **without changing the lighting**.

**Measure the ceiling before writing a line of code.** I counted how many entries to the loop end with
**zero** hits, in other words the completely wasted loop:

| map | loop entries | with zero hits | culling ceiling |
|---|---|---|---|
| koth_sandy | 2,028 | 466 | **23%** |
| ba_dust_island | 6,623 | 137 | **2.1%** |

**On the heavy map, 98% of samples get at least one sky hit.** There is no wasted
loop to skip. And it makes sense in hindsight: the expensive maps are expensive *because* they are
outdoors and see sky. Indoor samples, where culling would help, are a minority in exactly the
maps where it would matter.

The 67% of occluded rays is waste **inside** each loop, not whole loops wasted: a
sample sees some sky but most of its 4,098 directions are blocked. Per-sample culling does not fix that;
it would take per-direction culling, which is the same problem as the ray test.

**Decision: do not implement it.** A 2.1% ceiling on the map that matters does not justify the risk.

### 4.6 What would remain

One route is still open but it **changes the lighting**, so it can no longer be validated with a hash: the
sky normal levels are a **subdivision hierarchy** (258 → 1,026 → 4,098 → 16,386). A
coarse-to-fine approach could test a coarse level first and, in the fully occluded regions, skip their
fine subdivisions. It is real but complex, and it approximates, so you have to decide how much deviation is
acceptable. Compared with that, `-skylevel` gives 1.65× with one line of configuration.

### 4.7 External tracer (Embree) and GPU: measured and ruled out ❌

The question was whether it is worth replacing the BSP descent (`TestLine`) with a production BVH, or
moving the tracing to the GPU. Both cost weeks, so first the ceiling was measured with
`-raybench` (see §"How to reproduce"): it captures the **real** sky rays from the compile and traces them again in isolation,
against the same BSP, on one thread.

`ba_dust_island`, MSVC 19.44, Release, AVX2, one thread, 13,339,167 sky rays (208,832 sampled in
contiguous runs of 64, so that neighboring rays share an origin the way the real compile sees them):

| tracer | Mrays/s | ns/ray | vs `TestLine` |
|---|---|---|---|
| `TestLine` (BSP walk) | 7.47 | 134 | — |
| Embree `rtcIntersect1` | 7.70 | 130 | **1.03×** |
| Embree `rtcIntersect8` (packets) | 7.91 | 126 | **1.06×** |
| Embree `rtcOccluded1` | 13.58 | 74 | 1.8× (no use: it cannot tell sky from solid) |

**Embree gains nothing.** The reason is the size of the geometry: the whole map is **1,876 triangles**.
A production BVH is built for scenes of millions of primitives; with 1,876 the per-ray cost
is fixed overhead (building the ray, crossing the library boundary, walking a tree a few levels deep),
not scene complexity. 8-wide packets do not help either for the same reason: there is no work to amortize.

And the ceiling of the whole project is smaller than it looked. A complete RAD on one thread on that map
takes **9.31 s**; the 13.3M sky rays at 7.47 Mrays/s are **1.79 s**, which is **19% of RAD**. With
`-extra` (14.09 s, same number of rays) it drops to **12.7%**.

> Even if tracing were **free**, RAD would drop 19% at normal quality and 13% with `-extra`.
> Embree, at 1.06×, leaves that at ~1%.

**Decision: implement neither.** GPU is ruled out by the same calculation and with extra
drawbacks: the same 19% ceiling, plus transfers, more driver dependency, a mandatory CPU fallback for
life, and goodbye to the byte-identical `.bsp` that is how everything in this fork is validated.

> **This decision was reversed.** The GPU was implemented later and measured: see §4.8. On a map with many
> direct lights it clearly wins (2.27× with 1024 lights), and the `.bsp` still comes out byte-identical. What
> this analysis got wrong was the scope: `-raybench` measured **sky tracing**, which really is
> 19%, but the full gather is considerably more than that: it walks the light list per sample, and that
> grows linearly with the number of lights in the map. `ba_dust_island` has a single light, so it was
> precisely the map where it did not show.

What the number does say is where to look: the **remaining 80% of RAD** is not ray tracing. It is the rest
of `GatherSampleLight` (walking the light list, styles, opaques), the transfers and the bounce.

Methodological note: the Embree tracer that was measured builds a triangle soup of all faces
and agrees with `TestLine` on sky/not-sky for **78%** of rays. It is not a correct port and does not pretend
to be: `TestLine` resolves `contents` per leaf (including water transitions) and descends on both
sides of a nearly coplanar plane via `ON_EPSILON`, things a BVH does not have. For measuring **speed**
it works anyway: the traversal work is the same order. Had the result been 10×, the next
step would have been to solve that part; at 1.06× it is not needed.

### 4.8 GPU: implemented anyway, and measured ⚠️

§4.7 ruled out the GPU by calculation. It was implemented afterwards regardless, porting the Vulkan backend of
[speedrun-16/hltools](https://github.com/speedrun-16/hltools) (GPL-2.0), to have the real number instead
of the estimate. It is in the tree as `-gpu`, off by default.

**How it works.** `BuildFacelights` runs twice. The first pass (*collect*) intercepts every call to
`GatherSampleLight`, records it as a work item and throws away the rest of the face's work. Then the compute
kernel evaluates all the items together. The second pass (*consume*) walks exactly the same code path
and each call reads its stored result instead of tracing. Everything that is not the gather (sample
positioning, lmcache blur, bounces, final blend) is the CPU code, untouched.

**Correctness: byte-identical `.bsp`.** That was the requirement, and it is met. `ba_dust_island` and `ar_pokemon`,
with and without `-extra`, give the same SHA-256 on CPU and on GPU. The kernel replicates the CPU's accumulation
order (ascending leaf, then each leaf's list) and the branches it cannot do correctly (the texlights'
*near branch*, which needs the emitter's winding and an area integration) go back to the CPU
and are resolved with the reference code.

**The duplicated work was eliminated.** The first version ran `BuildFacelights` **twice**: once to
record the calls and once to consume them. `BuildFacelights` is split into two halves that each execute
only once:

- `BuildFacelights_Begin`: `CalcPoints` and the expensive part of `CalcLightmap`: the position of each sample,
  phong normal, PVS, and the gather call **recorded** instead of traced. It also stores the two
  per-point booleans (`blocked`, `nudged`) that the tail needs.
- `BuildFacelights_End`: `CalcLightmap` in *apply* mode, which jumps straight to the tail and leaves each
  result where the CPU would have written it, followed by the blur, the patches and the lightmap.

The state between the two halves (the `lightinfo_t` with its `lmcache`) is stored in a `facebuild_t`. Since the
`lmcache` is megabytes per face with `-extra`, faces are processed in **batches** bounded by a memory
budget: `Begin` runs over the batch, the whole batch is sent in one dispatch, and `End` runs. Without
`-gpu` the two halves are called one after the other and it is exactly the function from before.

**What decides whether it wins: how many direct lights the map has.** This is the important measurement, and it explains
why the first tests showed a loss. Same sealed room, same size, same `-extra -threads 6`,
GTX 1060 3GB against a 6-core Ryzen; the only thing that changes is the number of `light` entities:

| lights | CPU | `-gpu` | | device time | `.bsp` |
|---:|---:|---:|---|---:|---|
| 1 | 0.66 s | 1.32 s | **0.5×** (loses) | 0.28 s | identical |
| 64 | 1.17 s | 1.39 s | 0.84× | 0.30 s | identical |
| 256 | 1.59 s | 1.29 s | 1.23× | 0.35 s | 1 byte of 95,982, ±1 |
| 512 | 2.27 s | 1.31 s | **1.73×** | 0.41 s | identical |
| 1024 | 3.91 s | 1.72 s | **2.27×** | 0.56 s | identical |

CPU time grows **linearly** with the number of lights: each sample walks the list of lights that its
PVS sees, one by one. Device time barely moves (0.28 s to 0.56 s with 1024× more lights), because that
is exactly what a GPU does well. The crossover is around **150-200 lights**, and from there
upward the advantage keeps growing.

That is why `ba_dust_island` showed a loss and will keep doing so: the marshaled map has **1 single light in 1 leaf**.
It is the worst possible case: there is nothing to parallelize per sample, and all that is left is the fixed cost
of the BSP descent, which in this fork is already very well handled on the CPU (sky PVS shortcut §4.5,
single box against the opaques, AVX2). With `-extra`:

| | RAD total | `BuildFacelights` phase |
|---|---|---|
| CPU | 3.33 s | 0.96 s (everything, including tracing) |
| `-gpu` | 4.93 s | collect 0.62 s + device 0.93 s + finish 0.51 s |

In other words: the hltools number (2.96× on `speedrun_celerior`, a map whose CPU RAD takes 115 s) and the
number here do not contradict each other. They are the same behavior measured at two different points on the same
curve.

**About the identical `.bsp`.** The kernel normalizes in `float` while the CPU uses the `VectorNormalize`
path; on four of the five maps measured that gives the same file byte for byte, and on the
remaining one it gives **one byte out of 95,982 with a difference of 1** out of 255: one channel of one luxel, one step.
`SDHLT_GPU_FP64_NORMALIZE=1` selects the parity variant in `double` for comparison, at the cost of
speed (on GeForce `fp64` runs at 1/32).

**It stays optional (`-gpu`, off by default)** because the result depends on the map, and on one with few
lights it costs dearly. It falls back to the CPU path with a clear warning when the map has opaque entities, model
shadows, more light styles than the kernel handles, a BSP tree deeper than its traversal stack,
or when there is no Vulkan driver. And it always prints the breakdown (`collect`, `device`, `finish`) plus
the number of marshaled lights, which is the number used to decide whether it is worthwhile.

Note on the traversal stack: it is seven arrays per thread, so its depth decides occupancy.
The kernel is compiled in 16/24/32/48/64 variants and the host picks the smallest one that covers the map's actual
tree. On `ba_dust_island` it did not move the device time (with a single light the bottleneck is not there), but
it is free and on large maps is where it may be noticeable.

## 5. Face merging: no real headroom (investigated thoroughly)

This was the candidate route to lowering `wpoly`. The answer, with evidence, is that there is no headroom.

**What merging already achieves** (`MergeAll` / `TryMerge`, measured with `-verbose`):

| Map | Final faces | Freed by merging | Reduction |
|---|---|---|---|
| ba_coliseum | 1144 | 975 | **46%** |
| koth_sandy | 1259 | 303 | **19%** |
| ba_dust_island | 583 | 149 | **20%** |

**`TryMerge` rejections, instrumented** (a counter for each of the 9 reasons):

| Map | merges | maxedges | tex | convex | plane |
|---|---|---|---|---|---|
| ba_coliseum | 2131 | **0** | 7522 | 934+934 | 96 |
| koth_sandy | 474 | **0** | 1884 | 10+10 | 177 |
| ba_dust_island | 1040 | **0** | 286 | 47+47 | 0 |

`maxedges = 0` on all three maps: **raising `MAXEDGES` would gain nothing**. The `tex` and
`contents` counts are high but misleading, because they are checked *before* the shared-edge test, and
most of those pairs do not share an edge.

**Duplicate texinfos: 0.** If CSG emitted redundant texinfos, visually identical faces would fail
the `texturenum` check and merges would be lost. I parsed the lump: 34 entries, 34 unique, 0 faces using
a duplicate texinfo. Hypothesis ruled out.

**Residual merges in the final BSP** (`scripts/bspcheck.py`). This is where the important finding was.
A first naive count gave **697** "mergeable" pairs in ba_coliseum, which would have been huge.
But `sdHLBSP` does `MergePlaneFaces()` and **then** `SubdivideFace()`: the adjacent coplanar faces
in the final BSP are, for the most part, **deliberate** subdivisions to keep the lightmap extent
within `MAX_SURFACE_EXTENT`. Merging them would break the software renderer and the HLDS.

Excluding the pairs whose union would exceed the 240-unit limit: **32**.
And mapping each face to its BSP node:

| | ba_coliseum |
|---|---|
| residual pairs | 32 |
| in **different nodes** (impossible: the face belongs to its node) | 26 |
| in the **same node** (the only real candidates) | 6 |

**6 of 815 faces = 0.7%.** And those 6 are probably blocked by `contents` / `detaillevel` /
`facestyle`, which are compile attributes that are not recorded in the BSP.

**Decision: do not touch `TryMerge`.** Not out of caution, but because it is measured that there is nothing to gain.
Added to the fact that `MergeFaceToList()` already recurses after each successful merge, the final list per plane is
*pairwise* non-mergeable: a true fixed point.

> Methodological note: my first convexity check reported 84 non-convex faces in ba_coliseum.
> It was a bug of **mine**: it compared a raw cross product against a fixed epsilon, and that magnitude scales
> with edge length, so long edges with negligible angles gave false positives.
> Normalizing by the lengths gives a scale-independent angular test, and all three maps come out with
> clean geometry. It serves as a reminder to suspect the tool before the compiler.

## 6. Non-determinism: found and fixed ✅

Discovered while building the regression baseline. Two runs of the **same unmodified binary** on
`koth_sandy`, with autodetected threads:

| lump | run 1 | run 2 | delta |
|---|---|---|---|
| clipnodes | 596 | 598 | +2 |
| marksurfaces | 613 | 614 | +1 |
| visibility | 1509 bytes | 1505 bytes | −4 |

**Isolation by stage.** Fixing CSG at 1 thread but leaving VIS multithreaded, the output is
byte-identical. On these three maps all of the variation came from CSG (note: the conclusion at the time,
"VIS is already order-independent", turned out to be false for `-full` on medium-sized maps; see §8):

- `FindIntPlane()` appends to the global plane array under a lock: the thread that wins defines the plane's
  index. The original code even carries the comment *"BUG: there might be some multithread issue
  --vluzacn"* at that spot.
- `WriteFace()` does a direct `fprintf` to the `.p0`–`.p3` files under a lock: the completion order
  defines the face order.

Both indices reach BSP (which is single-threaded), and that is where the differing clipnode counts came from.

**Fix:** the three parallel phases of CSG (`CreateBrush`, `CSGBrush`, `CalculateBrushUnions`) run
on one thread when `g_deterministic` is active, which is now **the default**.

**Cost, measured:** CSG alone 0.046s → 0.059s. CSG+BSP+VIS end-to-end 0.170s → 0.180s. Against ~8.5s of
RAD on the same map, that is **0.1%**. `-nodeterministic` restores the old behavior.

**Verified:**

| Map | two multithreaded compiles |
|---|---|
| ba_coliseum | identical |
| koth_sandy | identical |
| ba_dust_island | identical |

And the deterministic output is **byte-identical to the previous single-threaded baseline**, in other words the order
was fixed, no new result was adopted. With `-nodeterministic` it still varies, which confirms the attribution.

Practical consequence: **`-threads 1` is no longer needed to compare regressions.** The recommendation
is kept the same, because it is the strongest guarantee and costs nothing on small maps.

## 7. Regression from the threading refactor

Replacing the Win32/pthread backends with `std::thread` (723 → 448 lines), verified with
`--threads 1`:

| Map | Result |
|---|---|
| koth_sandy | BSP byte-identical, lump by lump |
| ba_coliseum | BSP byte-identical, lump by lump |

Plus the unit tests: autodetection = `nproc`, `-threads 5000` clamped without a crash, and 200,000
work units dispatched exactly once with 1/2/4/16/64 threads.

## 8. CSG and VIS: full determinism and speed (September 2026)

Environment: Windows 11, MSVC 19.44, **Ryzen 5 5600G (6 cores / 12 threads)**, desktop with ~12% of
background load. Corpus: 13 real CS 1.6 maps (the three above plus `ar_azteca`,
`ar_old_azteca`, `ar_crazyjump`, `ar_namekusein_zg`, `ar_pokemon`, `fp_icy_island`,
`fp_squidgame_thno`, `zm_azteca`, `zm_eichen_v2`, `zm_mini_dust`). Times: minimum of
**interleaved** A/B runs.

### 8.1 VIS `-full` was not deterministic ❌ → fixed ✅

Two identical runs of `zm_azteca` with 12 threads gave different `.bsp` files. The cause was in
`RecursiveLeafFlow`: to prune, a portal used the final `visbits` of the portals that *had already
finished* and `mightsee` of the others, so the result depended on which thread got there first. With
`-fast` or without `-full` it is not noticeable; with `-full` on medium-sized maps, it is.

**Fix:** portals are processed in a fixed order (least `mightsee` first, the same order the original scan
chose) and each portal prunes only with lower-ranked portals, waiting for them if
needed. It is exactly what a single thread does, so **any number of threads produces the same
`.bsp` as `-threads 1`**. Verified on all 13 maps with 2, 5 and 12 threads, and on Linux (GCC 15).

Waiting had a cost: on `fp_squidgame_thno` (6334 portals) half the thread time sat
idle and VIS went from ~36 s to ~50 s. It was solved with a scheduler: the thread waiting for a portal
runs subtrees of that portal's recursion, which its owner publishes only when there are hungry
threads. Splitting a flow like that does not change the result: a subtree only marks leafs from its own
`mightsee` and is only pruned when all of them are already marked.

Resulting scaling (squidgame, `-full`): 2 threads 1.95×, 5 threads 3.84×, 12 threads 5.98×.

A sweep that ruled out relaxing the rule (using `visbits` only from portals W ranks back):

| W | time | CPU | output |
|---|---|---|---|
| 0 (exact) | 48 s (no scheduler) | 283 s | = 1 thread |
| 48 | 43.5 s | 355 s | different |
| 200 | 40.7 s | 396 s | different |
| ∞ (`mightsee` only) | 97.1 s | 820 s | different |

Pruning with `visbits` saves half the work; changing the output is not worth what little is gained.

### 8.2 VIS: less work per flow ✅

Profiled with a custom sampler (Windows, nothing to install): `ClipToSeperators` ~55% of the compute and
the normalization of the candidate plane alone ~22%; `ChopWinding` ~35%.

| Change | `zm_azteca -full`, 1 thread |
|---|---|
| original | 3.90 s |
| 64-bit bitsets in one pass (`long` is 32-bit on Windows) | 3.76 s |
| `ChopWinding` without jumps (73% of calls leave the winding whole) | 3.62 s |
| `SeparatorVerdict`: deciding candidates without normalizing | 3.20 s |
| `sqrt` only for candidates that get normalized | 3.14 s |

`SeparatorVerdict` classifies the points in `double` on the raw cross product and only decides when the
point is farther from the `ON_EPSILON` thresholds than twice the combined rounding error
bound (17u times the largest L1 norm). A verification build recomputed the exact test on every
candidate that the filter decided: **zero discrepancies** on all 13 maps (over a billion
candidates). The undecided ones (0.4 to 1.7%) use the original test.

Ruled out by measurement: a prefilter that *added* a "pass on both sides" test before the original
caught only 23% of candidates and made VIS 13% slower.

With all 12 threads busy, SMT hides a good part of the latency of `sqrt` and divisions, and the
gain shrinks: squidgame with 6 threads went from 44.0 s to 42.5 s (median 49.4 → 45.4 s), and with 12 threads
it is on par with the non-deterministic original. What was gained there is that the output is now reproducible.

### 8.3 CSG: indexed plane table and parallel `CSGBrush` ✅

- `FindIntPlane` compared each lookup against all planes (O(N²)). It now uses buckets by normal
  quantized to 1/2048; the match test is the same and the lowest index wins, so the
  numbering does not change.
- `CSGBrush` ran on one thread because faces were written in completion order. Now each brush
  formats its faces into its own buffer (the `fprintf` was the expensive part) and they are written in brush order.

Full CSG, 12 threads, output identical to the previous one on all 13 maps:

| Map | before | now |
|---|---|---|
| fp_squidgame_thno | 0.48 s | 0.20 s |
| zm_eichen_v2 | 0.31 s | 0.15 s |
| zm_azteca | 0.21 s | 0.09 s |
| ar_azteca | 0.13 s | 0.06 s |

`CreateBrush` is still on one thread: there the creation order decides which plane ends up as the representative of
each group, and changing it would change the `.bsp`.

## 9. RAD on a texlights map (September 2026)

`ze_elysium_b1`: 15,002 faces, 43,376 patches, **1,561 direct lights** (non-fast texlights) and 4,282
fast ones, no `light_environment`. GUI arguments (`-extra -bounce 12 -skylevel 6 -softsky 1
-pre25`), Ryzen 5 5600G, 12 threads, GTX 1060 3GB.

### 9.1 Where the time goes: measure with sampling, not with clocks

The profile with `-profile` and `rdtsc` clocks around the hot code said that half of
`GatherSampleLight` was walking the PVS leaves. It was a lie: two clock reads for each of
the 1.58 billion sample-light pairs distorted exactly what they were measuring (with `-profile`, RAD took 92 s;
without it, 83 s). A sampler (every millisecond it suspends the threads and records where they are, then resolves
with the PDB) does not touch the code and gave the real picture, in CPU time spent working:

| function | % |
|---|---|
| `TestLine_r` (shadow rays) | **58%** |
| `IsVisbitInArray` (lookup in the sparse vismatrix) | 11% |
| `GatherSampleLight` (the light computation) | 7% |
| `CalcSightArea` | 4% |
| `VectorNormalize` | 4% |

With `-extra` that is 6.6 million samples and 512 million shadow rays, at ~460 ns each.

### 9.2 What went in (same output byte for byte) ✅

- **`-vismatrix auto`, the GUI default.** The normal matrix answers each pair of patches with a bit; the
  sparse one, with a lookup. It weighs (n+1)²/16 bytes: 112 MB with 43,376 patches. `auto` takes it up to 512 MB and
  otherwise uses sparse. MakeScales: 14.0 s → 3.4 s.
- **The normal matrix is built twice as fast.** Each leaf walked all the patches in the map looking for
  its own (240 million iterations), and each visible bit took a global lock shared by all the threads.
  Now there is a patch list per leaf and an atomic OR: 9.6 s → 6.2 s.
- **Light list per PVS.** `GatherSampleLight` walked all the leaves in the map for every sample. Each
  thread keeps the flattened list of the last few PVSs; same lights, same order. On the order of 5%, within the
  noise of a single run.

| map | v0.14.0 | now | |
|---|---|---|---|
| ze_elysium_b1 | 88.5 s | 70.5 s | identical |
| zpa_house | 5.5 s | 5.1 s | identical |
| zm_eichen_v2 | 4.1 s | 3.7 s | identical |
| zm_azteca | 1.9 s | 1.8 s | identical |
| ar_pokemon | 0.4 s | 0.4 s | identical |

With the GPU (`-gpuauto`, which already existed) the same map drops to **49 s**: 0.6% of the light bytes change,
by 3/255 at most. The GUI has it enabled from now on; without Vulkan, RAD goes back to the CPU.

### 9.3 What was tried and did not go in ❌

- **Reusing shadow rays with `-extra`.** The points of the luxel grid trace and record each ray;
  the intermediate ones skip the rays where the four corners agree. Over whole faces it gave 2.2×
  in `BuildFacelights` (49 → 22.5 s), but 110 of the 114 bytes with an error of 8/255 or more (up to 49/255)
  were on the edge of the face, where the blur margin points fall on another surface. Limited to
  points on the same surface as their corners, the error drops to 3/255 and the gain to 10-20%: the map
  has many small faces, and on a small face almost everything is edge.
- **Interpolating the light** where the four corners agree on all rays and look alike: more error and
  no faster; only 0.45 of 6.6 million points qualified.
- **Cutting dim lights with bounded error.** Trace from largest to smallest contribution and stop when what remains
  cannot add more than 0.5% of what has been seen. Validated (threshold 0 = identical), but sorting each sample's
  lights costs more than the rays it saves.
- **Copying contiguous lights in memory** instead of pointers: no change; they were not cache misses.

### 9.4 Second pass: GPU and CPU in parallel ✅

With the GUI at its default (GPU + `-vismatrix auto`), the timeline of the same map was: 19 s of
`BuildFacelights`, 6.7 s of `BuildVisLeafs`, 3.7 s of `MakeScales`, 4 s of bounces. Within the 19 s the
GPU worked 14.6 s and the CPU 7.4 s (5.1 s preparing samples, 2.3 s finishing faces), **taking turns**.

- **Pipeline.** While the GPU computes batch N, the CPU prepares N+1 and finishes N-1. Two live batches,
  with a 256 MB budget each instead of 512 MB for one.
- **Nearby texlight pairs** (the kernel hands back to the CPU the sample-texlight pairs closer than the
  emitter's range, to resolve them with `CalcSightArea`) were 3.2 million and were resolved on the single thread
  that fed the GPU: they were hidden inside the "device time". They now go to the thread pool,
  grouped by sample so that the summation order does not change. 47.5 s → 36.1 s in one stroke.
- **`auto` picks sparse with GPU.** The GPU only computes transfers for the sparse matrix; with a GPU, sparse
  beats the normal matrix on CPU by 3.2 s. Without a GPU it still picks the normal one.
- **Candidate faces per leaf** in `BuildVisLeafs`: from the leaves the PVS lists, instead of walking all 15,000
  faces of the map for each leaf. For both matrices. 6.7 → 4.1 s.
- Larger dispatch batches (262k items) did not help; the kernel is what costs, not the dispatch.

| ze_elysium_b1, GUI default | time | .bsp |
|---|---|---|
| 0.15.0 | 52.4 s | — |
| now | **33.7 s** (31.5 s the best) | identical to 0.15.0's with GPU |
| now, CPU only, normal matrix | 70.9 s | identical to 0.14.0's |
| now, CPU only, sparse | 88.3 s | identical to 0.14.0's |

What remains: the gather kernel, 16 s of GPU over 6.6 million samples.

## 10. Bounces, BSP and cell subdivision (October 2026)

Environment: Windows 11, MSVC 19.44, Ryzen 5 5600G (12 threads), GTX 1060 3GB. Maps: `fp_squidgame_thno`
(7,742 faces, 20,623 patches, 81.6 million transfers), `zm_eichen_v2`, `zm_azteca`,
`ar_pokemon`, `ba_dust_island`, `ba_coliseum`. GUI arguments. Profile with an external sampler
(suspends the threads and records the program counter, resolves with the PDB), without touching the code.

### 10.1 Where the time was going

`fp_squidgame_thno`, v0.17.2: CSG 0.3 s, BSP 0.9 s, VIS 29 to 32 s, RAD 16.7 s with GPU and 19.0 s on CPU.

Inside RAD the 12 bounces cost 0.44 s each: 5.3 s, more than the gather on the GPU. Sampling
showed `GatherLight` with almost no samples inside the executable and the threads "outside": they were not
idle, they were inside `ucrtbase.dll`. `isPointFinite` called `_finite` for each component, six
times per transfer. `std::isfinite` does not fix it: on MSVC it ends up in `_fdtest`, another CRT call.

### 10.2 What went in (same output byte for byte) ✅

| change | measurement |
|---|---|
| `isPointFinite` reading the exponent bits | bounce 0.44 s → 0.26 s |
| emitters flattened once per bounce, with the destination style already resolved | bounce 0.26 s → 0.18 s |
| discarding the light by the sign of the raw vector, before normalizing | BuildFacelights on CPU −7% |
| `GetEdge` with a map per vertex pair instead of walking all the edges | BSP 0.88 s → 0.73 s |
| progress bar redrawn every 50 ms (affects `-estimate`, the default on Linux) | one write per patch and per bounce fewer |

A first shortcut only for style-0 patches did not work on this map: it has styled lights, and
after the first bounce 680 of 20,623 patches were left "simple", and 11 by the ninth. The one that went in covers
all styles.

Full RAD: 16.7 s → 14.6 s with GPU, 19.0 s → 16.8 s on CPU. The `.bsp` is identical to v0.17.2's on
all six maps, with and without GPU, and with `-aoall -pcf 3 -blurclamp 0.5`, `-ao -rgbtransfers`,
`-vismatrix sparse -bounce 3` and `-gpu`.

### 10.3 What was tried and did not go in ❌

- **`/Ob2` and non-incremental linking.** The published binaries are built as RelWithDebInfo, which on
  MSVC brings `/O2 /Ob1` and `/INCREMENTAL`. With `/Ob2 /INCREMENTAL:NO /OPT:REF /OPT:ICF`: VIS 32.9 and 33.8 s
  against 32.3 and 34.1 s; RAD 17.7 s against 18.0 s. Within the noise. The hot code is already written
  with macros and `inline` functions.
- **Idle time in VIS.** Sampling suggested a third of the threads outside the executable. Process time
  says otherwise: 336 s of CPU in 29.3 s of wall clock with 12 threads, 96% usage. There is no waiting
  to recover; `ClipToSeperators` and `ChopWinding` are 87% and were already worked on in §8.2.
- **Normal VIS instead of `-full`.** On this map it takes the same time (32.9 s against 31.6 s) and leaves the PVS
  nearly the same (4,570 faces on average per leaf against 4,561). `-fast` does change it: 6,881.
- **`-maxnodesize` as a wpoly lever.** It moves the average number of faces in the PVS by ±5%, but the best
  value changes with the map:

  | `-maxnodesize` | zm_eichen_v2 | zm_azteca |
  |---|---|---|
  | 512 | 524 | 656 |
  | 1024 (default) | 542 | 630 |
  | 2048 | 577 | 672 |
  | 4096 | 549 | 680 |

  Choosing it well requires compiling BSP and VIS several times per map. It stays as something the mapper can
  try with the Analysis tab, not as a default.
- **`-vismatrix auto` with GPU.** On this map the normal matrix on CPU wins (BuildVisLeafs 2.3 s +
  MakeScales 1.0 s) over sparse with GPU (2.8 s + 3.1 s); on `ze_elysium_b1` the opposite was measured (§9.4).
  Without that map at hand there is nothing to pick a threshold with, so `auto` stays as it is.

### 10.4 `-gridsubdivide`: cutting by lightmap cells ✅ (optional)

The engine rejects a face when `ceil(max / 16) - floor(min / 16)` exceeds 16 on a texture axis
(`extents > 256`, verified in ReHLDS's `CalcSurfaceExtents`). The classic rule does not count cells: it lets
up to 240 units through and cuts the rest every 224, which is safe at any grid position but
wastes up to two cells per piece. A 256 wall that starts on a cell edge fits exactly, and
came out as 224 + 32; a 256 × 512 one, as 6 faces instead of 2.

With `-gridsubdivide` BSP counts the cells with the engine's arithmetic (`CalculatePointVecsProduct`) and
cuts only when passing 16. Each end of the face carries a margin, unless its position is safe: the
texture axis follows a world axis and the vertex is at an integer coordinate. The cut goes on a cell edge
when the new vertices also land on integers, and otherwise through the middle of a cell.

Two checks close off the "Bad surface extents" risk:

1. When emitting edges, the cells are counted using the positions that `GetVertex` is going to weld. A face
   that would overflow keeps its own vertices. Tested with a synthetic map: 256 slabs with neighbors with a
   slanted side whose edge falls 0.012 from the slab; 7 faces kept their vertices.
2. Before writing, all faces are walked with `GetFaceExtents`. A single one over the limit stops
   the compile.

| map | faces | with `-gridsubdivide` | |
|---|---|---|---|
| ba_coliseum | 815 | 655 | −19.6% |
| ba_dust_island | 653 | 576 | −11.8% |
| ar_pokemon | 363 | 326 | −10.2% |
| zm_eichen_v2 | 2,786 | 2,635 | −5.4% |
| zm_azteca | 1,789 | 1,730 | −3.3% |
| ar_azteca | 1,203 | 1,163 | −3.3% |
| fp_squidgame_thno | 7,742 | 7,494 | −3.2% |

World faces the PVS sends to the renderer (`wpolymap.py`), average and worst leaf: `zm_eichen_v2` 542 →
504 and 1,090 → 999; `zm_azteca` 630 → 608 and 1,330 → 1,278. `bspcheck.py` gives the same total area and
valid geometry, and the face pairs still mergeable on `zm_eichen_v2` drop from 159 to 91. RAD
gains a bit too, because there are fewer faces to light: `ba_coliseum` 2.7 s → 2.3 s.

Without the option, BSP writes the same file as before on the 10 corpus maps that compile here.

**What is missing:** opening a map compiled this way in CS 1.6. The cell count matches the engine's and
RAD's, but it was not tested in game.

## 11. RAD by blocks: light lists and ray bundles (October 2026)

Environment: the one from §10 (Windows 11, MSVC 19.44, 12-thread Ryzen 5 5600G, GTX 1060 3GB), GUI
arguments (`-extra -bounce 12 -skylevel 6 -softsky 1 -vismatrix auto -pre25`, plus `-gpuauto` in the
GPU column). In addition to the maps from §10, RAD was run on already compiled BSPs: `de_inferno` and `de_aztec` from the
game, and `ze_labs_eeas_zg`, `ze_rats_lg` and `ze_fanb_area51_fix1`. RAD relights a vis'd BSP without touching
anything else, so they are useful for measuring even without the `.map`.

### 11.1 Where the time was going

Counters inside `GatherSampleLight` on `fp_squidgame_thno` (687 direct lights, 2.79 million
samples):

| | amount | |
|---|---|---|
| sample-light iterations | 1,469 million | 526 per sample |
| discarded by side (light behind the plane or the surface) | 740 million | 50% |
| discarded by `-lightskip` | 391 million | 27% |
| with a shadow ray | 168 million | 11% |
| rays that reach the light | 123 million | 73% of the rays |

Half of the time of `BuildFacelights` was not tracing: it was walking lights that the sample was going to
discard, and most of the rays that were traced found nothing. On `ze_sanctorum` (212 lights)
it was 214 million iterations and 99 million rays, 40% free.

### 11.2 What went in (same `.bsp` byte for byte) ✅

**Samples by blocks.** `CalcLightmap` first places all of the face's samples and then gathers the
light, in the same order. The samples are grouped into blocks of 8×8 lmcache points (8/3 of a luxel with
`-extra`), with a box on the face's axes and the spread of their normals.

**Light list per block.** The block discards once each light that all of its samples would discard
one by one: behind the texlight's plane or the surface, outside a spot's cone, or under
`-lightskip` with the distance and the two cosines bounded over the box. The list keeps the order of the
PVS list, so the sums do not change. The bounds are loose on purpose: `vec_t` is `float` in
RAD, and a tight bound off by a rounding error would discard a light that the sample keeps.

| `fp_squidgame_thno` | before | now |
|---|---|---|
| sample-light iterations | 1,469 million | 190 million |

**Proven ray bundles.** The first ray from a block toward a light is traced as usual. Depending on what it
finds, one of two proofs is attempted, and the other only if a later ray does not match:

- *All clear* (`TestBeamClear`): the set of segments between the box and the light goes down the tree
  the way `TestLine_r` takes one segment, splitting it at each plane. The distance to a plane along
  `a + t (b - a)` is linear in `t`, so each child receives the range of `t` where some corner is on
  its side by more than the margin. If all the leaves reached are of one and the same contents, neither solid nor
  sky, `TestLine` would return `CONTENTS_EMPTY` for each one.
- *All blocked* (`TestBeamBlocked`): the blocked ray is repeated, recording the path to the solid
  leaf. If the corner segments cross the interior of that leaf, they all cross it: with one
  endpoint fixed, the other endpoints whose segment cuts a convex cell form a convex set.

The margin is `ON_EPSILON` plus a fraction of the size of the coordinates: `TestLine_r` sends a span to
one side only when it exceeds `ON_EPSILON/2`, and its midpoints drift off the segment through rounding.

| light shadow rays | traced before | traced now |
|---|---|---|
| fp_squidgame_thno | 168 million | 25 million |
| ze_sanctorum | 99 million | 34 million |

Lesson from the first version: classifying the **whole** bundle at each node, without clipping it, gave 0.13% of
successful proofs on `ze_sanctorum`, even with single-sample blocks. In a deep tree the unclipped bundle
"crosses" planes that the ray never touches inside that cell and lands in solid leaves that are not
on its path. Clipping the `t` range took it to 36%.

**Patch pairs in `BuildVisLeafs`.** The same proof between the patches of a face in a leaf and those
of a destination face, for both matrix variants. It is only attempted when there are at least 8 pairs in
play: with fewer, the proof costs more than the rays. On `fp_squidgame_thno`, 41 of 76
million pairs are traced.

**`TestSegmentAgainstOpaqueList` inlined.** It was called for every ray that reaches its light even if the map
has no opaque entities or models with shadows. Now, in that case, the answer comes out without the call.

**`-vismatrix auto` with GPU.** It chose the sparse matrix so that the GPU could compute the transfers. That
won on `ze_elysium_b1` (43,376 patches, §9.4) and loses on smaller maps: `fp_squidgame_thno` 3.5 s
against 1.2 s for the normal matrix on CPU, `guard` 2.1 s against 0.6 s. Now the GPU computes transfers
from 32,768 patches; below that, the GPU gathers the direct light and the normal matrix stays on the CPU. Incidentally, the
result with GPU ends up within a few bytes (±1) of the CPU one: 3 on `fp_squidgame_thno`, 4 on `ze_sanctorum`;
before, even the lump size changed.

### 11.3 What was tried and did not go in ❌

- **Bundles for the sky dome.** With `-softsky` the sky is gathered per patch: on `de_inferno` that is 153
  million rays and 59% of RAD's time. The directions were grouped by cell of a cube
  (4×4 per face) and each group's fan was tested, with the probe ray's cell and a point inside
  it for each corner direction. It is correct (same `.bsp`), but the blocked rays it skips, 33
  million, are short rays that hit a nearby wall, and the ones that reach the sky are almost never
  proven: the fan grazes walls or ends up in different leaves of the sky brush. With 8×8 groups
  more are skipped but the proofs cost more than they save. No net gain; it did not go in.
- **Bounces.** Accumulating style 0 in registers and packing the emitter into a cache line with
  prefetch: −13% with one thread, nothing with all 12. With all threads the bounce is memory-bound.
- **4×4 blocks.** Clear-ray proofs go from 77 to 78 million on `fp_squidgame_thno`: the
  free ones that are missing are stuck to the geometry, it is not a block size problem.
- **Pair proofs for any group.** With 2.7 patches per face on `fp_squidgame_thno`,
  `BuildVisLeafs` got worse (4.7 to 6.1 s against 3.6 to 3.7 s). Hence the minimum of 8 pairs.

### 11.4 Result

Full RAD, minimum of 3 runs interleaved with 0.20.0 (the machine had other background load; the individual
runs are in the measurement log). "Same" is the light lump byte for byte.

| map | CPU 0.20.0 | CPU now | | GPU 0.20.0 | GPU now | |
|---|---:|---:|---|---:|---:|---|
| fp_squidgame_thno | 17.31 s | 12.47 s | **1.39×**, same | 17.87 s | 13.74 s | **1.30×** |
| ze_sanctorum | 10.53 s | 8.69 s | 1.21×, same | 9.85 s | 8.48 s | 1.16× |
| ze_labs_eeas_zg | 5.34 s | 4.73 s | 1.13×, same | 6.38 s | 5.37 s | 1.19× |
| ze_fanb_area51_fix1 | 23.30 s | 21.97 s | 1.06×, same | 24.43 s | 22.80 s | 1.07×, same |
| de_inferno | 11.93 s | 11.67 s | 1.02×, same | 12.65 s | 11.30 s | 1.12× |
| zm_eichen_v2 | 2.63 s | 2.37 s | 1.11×, same | 3.09 s | 2.62 s | 1.18× |
| guard | 3.84 s | 3.61 s | 1.06×, same | 6.13 s | 3.93 s | **1.56×** |

The phase that was attacked drops much more than the total: `BuildFacelights` on CPU goes from 12.7 to 5.5–6 s on
`fp_squidgame_thno` and from 8.9 to 5.9 s on `ze_sanctorum`. What remains (`BuildVisLeafs`, `MakeScales`, the
bounces and `AddPatchLights`) does not depend on how many lights the map has, and on maps with few lights and
a lot of sky, like `de_inferno`, 59% of the time is the per-patch sky dome (§11.3).

With GPU the difference against 0.20.0 comes from the `-vismatrix auto` policy: the result is now
the CPU one, except on `fp_squidgame_thno` (3 bytes, ±1) and where the GPU gathers the direct light. On
`ze_fanb_area51_fix1` (45,590 patches) the transfers are still done on the GPU, with the same result
as before. The portable build gives the same `.bsp` as the AVX2 one.

## How to reproduce

```sh
cmake -B build -S . && cmake --build build -j

# performance (multithreaded)
python3 scripts/compilebench.py --tools tools --map map.map --runs 3

# regression (ALWAYS with 1 thread)
python3 scripts/compilebench.py --tools tools_before --map map.map --threads 1 --json before.json
python3 scripts/compilebench.py --tools tools_after --map map.map --threads 1 --json after.json
python3 scripts/compilebench.py --compare before.json after.json
```

Note: the `.map` files carry absolute WAD paths from the machine where they were made. You have to rewrite the
`"wad"` key of worldspawn to local paths before compiling.

### Measuring ray tracing alone (`-raybench`)

To find out the ceiling of any plan to speed up tracing (§4.7), without the rest of the
compile masking it:

```sh
sdHLRAD map -raybench
```

It captures the real sky rays while running and, when `BuildFacelights` finishes, traces them again on
one thread, reporting Mrays/s. Comparing against the total RAD time **with `-threads 1`** directly gives the
percentage that tracing represents, which is the project's Amdahl ceiling.

To compare against Embree you have to build separately (none of this goes into a normal build):

```sh
cmake -B build-embree -S . -DCMAKE_BUILD_TYPE=Release \
      -DSDHLT_EMBREE=ON -DEMBREE_ROOT=<embree sdk>
cmake --build build-embree --target RAD
```

The resulting binary depends on `embree4.dll`; it is for research only.

## zhlt_embedlightmap: why it weighs so much

`zhlt_embedlightmap` exists because the engine does not apply lightmaps to entities
with a special render mode (additive, textured, etc.). RAD solves that by
**baking the light into the texture**, and to do that it generates **a new texture
per face**, not per entity or per original texture.

Measured on `ba_dust_island`, putting the key on its 6 `func_breakable`
(190 faces), with `-fast -bounce 1 -skylevel 4`:

| `zhlt_embedlightmapresolution` | .bsp | textures | texture bytes |
|---|---|---|---|
| without the key | 673 KB | 0 | 0 |
| 1 (default) | 12,540 KB | 190 | 11,859 KB |
| 2 | 3,759 KB | 190 | 3,078 KB |
| 4 | 1,570 KB | 190 | 889 KB |
| 8 | 1,255 KB | 190 | 573 KB |
| 16 | 994 KB | 190 | 312 KB |

**The map went from 673 KB to 12.5 MB: 18 times larger.** With `resolution 1`
each face is stored at its texture's native resolution (16 pixels per luxel),
plus 4 mipmap levels and its own 256-color palette (768 bytes) per
texture.

The resolution is the only thing that really moves the needle: each doubling divides
by four. It lowers the detail of the base texture, not that of the light, which by itself
only has one sample every 16 units.

### What this fork changed

Two things, neither affects light quality:

| | res 1 | res 2 | res 4 | res 8 |
|---|---|---|---|---|
| before | 11,859 KB | 3,078 KB | 889 KB | 573 KB |
| now | 9,048 KB | 2,210 KB | 687 KB | 380 KB |
| | **-24%** | **-28%** | **-23%** | **-34%** |

1. **No rounding to a power of two.** Each texture was rounded up to the next
   power of two, and a 272x144 face ended up taking 512x256: 2.4 times the pixels
   needed. GoldSrc only requires multiples of 16, which the code already guaranteed.
   `"zhlt_embedlightmappoweroftwo" "1"` brings back the previous behavior if needed.
2. **Identical textures shared.** Repeated faces (drawers, columns,
   grilles) baked the same bytes over and over. Now they are compared and the same texture
   is reused: 12 shared faces on this map at resolution 1,
   more at higher resolutions. It is only shared between faces that come from the same original
   texinfo, because the generated texture's name encodes which one it was and
   the tools read it back.

Verified with `scripts/bspcheck.py` (sound geometry) and by checking the textures
lump: dimensions multiples of 16, mips inside the lump, all
`texinfo->miptex` in range. A map without the key compiles byte for byte the same as
before.

### What other surfaces it broke, and why

Water was not a special case: it was **the first one that got noticed**. GoldSrc decides what a surface is
solely by the **texture name**, when loading the map (`Mod_LoadSurfaces`):

| name | chars | result |
|---|---|---|
| `sky` | 3 | `SURF_DRAWSKY` |
| `!` or `*` | 1 | `SURF_DRAWTURB` (water: wave, fog, culling from below) |
| `water` | 5 | `SURF_DRAWTURB` |
| `laser` | 5 | `SURF_DRAWTURB` |
| `scroll` | 6 | `SURF_CONVEYOR` (`func_conveyor` movement) |
| `{scroll` | 7 | `SURF_CONVEYOR` + `SURF_TRANSPARENT` |
| `{` | 1 | `SURF_TRANSPARENT` |

Since baking renames the texture, **each one of these that is not preserved turns the surface into an
ordinary one**. The water fix covered `!` and `{`; `*`, `water`, `laser` and `scroll` were missing.

`sky` never gets there: `EmbedLightmapInTextures` already skips sky and `TEX_SPECIAL`.

**All four ways of writing water produce the same flag**, so all four are baked with `!`, which
is what fits in one character.

`scroll` does not fit: the baked name had a single character of prefix (`?_rad` + 5 digits of texinfo +
3 of hash + 2 of counter = exactly the 15 characters). That is why there is a **second layout**:

```
<c>_radDDDDDhhhcc     the usual one: 1 char of prefix, texinfo in 5 decimals
scroll_radTTTcc       new: 6 of prefix, texinfo in 3 base-62 characters
```

What tells one from the other when reading them is the character following `_rad`: **digit** = old layout,
**letter** = new (the first base-62 character is forced to a letter when written). That way a `.bsp` made with
an earlier version is still read the same.

`{scroll` (7 characters) is **not baked**: no characters are left for the texinfo plus a unique suffix, and
keeping half the name would break either the movement or the transparency. Those faces keep their original
texture and their normal lightmap, with a warning in the log.

Verified on a test map with the five textures (`scroll*`, `water*`, `laser*`, `{scroll*` and
an ordinary one) and `zhlt_embedlightmap` on five entities:

| | before | after |
|---|---|---|
| `func_conveyor` (`scrolltest`) | `__rad*` (stops moving) | **`scroll_rad*`** |
| `watertest`, `lasertest` | `__rad*` (stops being water) | **`!_rad*`** |
| `{scrolltst` | `{_rad*` (stops moving) | not baked, with a warning |
| ordinary texture | `__rad*` | `__rad*` |

And with no regressions, all byte for byte: a map without the key, a map with the key on an ordinary texture, and
`func_water` with the key give `.bsp` files **identical** to those from before the change. Recompiling RAD twice
on the same `.bsp` also gives the same result, which is what proves that
`DeleteEmbeddedLightmaps()` recognizes the new name and restores the texinfo correctly.

### Water: why it stopped moving

`zhlt_embedlightmap` on a `func_water` broke the water, and the cause was in
two commented-out blocks in `loadtextures.cpp`.

GoldSrc decides that a surface is water **by the `!` at the start of the texture
name**, and reads the fog color and density from entries 3 and 4
of the palette. RAD got both wrong:

```
before: !leanwater_w5   palette[3],[4] = [25,69,155, 31,65,132]
        __rad00026MLe00 palette[3],[4] = [9,71,117, 14,70,117]   <- no longer water

now:    !_rad00026oI400 palette[3],[4] = [25,69,155, 31,65,132]  <- still water
```

Without the `!` the engine treats it as an ordinary surface: the waves, the fog and
the water behavior when looking at it from below are gone. And the requantized palette
took the fog with it even if the name had been fixed.

Now the name keeps the `!` and the first 16 palette entries are
copied as they are from the original texture; the baked colors use the remaining
240.

**Still to be checked in game:** the engine draws water with a sinusoidal
distortion of up to 8 texels, and the baked texture does not tile like the original,
so some bleeding may show at the edges of each face. The baking
leaves a margin of `(texturesize * resolution - extent) / 2` pixels on each side,
which in most cases is enough. If you see seams at the edge of the water,
raise `zhlt_embedlightmapresolution` or remove the key from that entity.

A size warning: a single large water brush is subdivided into many faces
(23 in the test map), and each one bakes its own texture. Water is one of the
most expensive places to put this key.

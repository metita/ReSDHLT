# Profiling RAD on your machine

RAD is **more than 95%** of compile time, so it is the only place where real time is left to
gain. This guide is for you to measure on your own CPU, which has more cores and less noise than the
environment where I did the analysis (2 cores, ±8% variance).

You do not need to install anything: the profiler is built into RAD.

---

## 1. Quick profile (no rebuild)

With the regular binaries you already get the phase-level view:

```powershell
sdHLRAD -profile -threads 1 mymap
```

You will get something like this:

```
---- profile ----
site                              calls       Mticks    share   ticks/call
BuildFacelights                     653      45282.5    51.6%   69345405.7
GatherSampleLight                146152      42399.9    47.9%     290108.3
FinalLightFace                      653         15.8     0.0%      24239.2
```

`GatherSampleLight` is nested inside `BuildFacelights`, so that 47.9% is ~94% of the time of
`BuildFacelights`. In plain terms: **almost all of RAD's time goes into gathering light per sample**.

> Use `-threads 1` when profiling. With several threads the timings overlap and the numbers become
> hard to interpret.

## 2. Full profile (with internal counters)

The counters for the internal ray casting functions are compiled out on purpose:
`TestLine_r` is entered ~1.8 billion times per map, so even a disabled check would cost
almost a second per compile. To enable them:

```powershell
cmake -B build-profile -S . -DSDHLT_PROFILE=ON
cmake --build build-profile -j
```

And then:

```powershell
tools\sdHLRAD -profile -threads 1 mymap
```

It now adds:

```
TestLine                       62245301 (count only)
TestLine_r                   1849616672 (count only)
TestSegmentAgainstOpaqueList   38965499 (count only)
CheckVisBit                    43864129 (count only)
```

**Note:** this build is slower because of the instrumentation itself. It is useful for *understanding* where the work goes,
not for measuring times. To measure times, use the regular build.

## 3. What has already been ruled out

So you do not spend time twice:

- **Lowering the skylight level further.** Levels 5 and 4 give only a little more speed than 6 (2.19× and 2.29× vs
  1.84×), but the visible difference grows quickly (2/255 and 5/255). Level 6 is the breaking point.
- **Turning the recursion in `TestLine_r` into a loop.** Three of its four paths are tail calls, so
  it looks like the obvious optimization. I did it: the lighting came out byte-identical (correct) but **slower**,
  both when copying endpoints (2.69s vs 2.59s) and when using pointers (2.76s). GCC already emits fewer recursive
  calls than the code suggests, and the calls are cheap thanks to the *return stack buffer*.
  Reverted.
- **`-O3` and LTO.** No measurable improvement, since the internal variance is larger than the difference. Details in
  `BENCHMARKS.md` §3.
- **Face merging / `TryMerge`.** 0.7% of headroom. Details in `BENCHMARKS.md` §5.

## 4. What has ALREADY been achieved: `-skylevel`

Counting rays per origin gave the big finding: **95.8% of all rays** come from the skylight
loop. `-softsky` only offered level 7 (16,386 rays per sample) or level 4 (258). Now there is
`-skylevel N`:

| level | rays | vs level 7 | max diff |
|---|---|---|---|
| 4 | 258 | 2.29× | 5/255 |
| 5 | 1,026 | 2.19× | 2/255 |
| **6 (default)** | **4,098** | **1.84×** | **1/255** |
| 7 | 16,386 | 1.00× | 0 |

`-skylevel 7` reproduces upstream's lighting byte for byte if you need it.

## 5. Where there is still room to look

The important finding from the profile:

| | ba_dust_island | koth_sandy |
|---|---|---|
| `TestLine` rays per `GatherSampleLight` | **426** | 374 |
| tree descents per ray | 30 | 20 |

The cost is in **how many rays are cast**, not in what each one costs. So the lines worth pursuing
are algorithmic:

1. **Cast fewer rays.** Can light sources that cannot reach a sample be discarded earlier?
   A PVS or distance test before `TestLine` would avoid the whole ray.
2. **Cache visibility.** Neighboring samples test the same lights against the same geometry. If there is
   exploitable correlation, whole rays are saved.
3. **More aggressive early-out.** If a light's contribution is going to fall below the `-limiter`
   threshold, the ray is not needed.

> All three were done later **without changing the lighting**: discarding by blocks of samples with
> conservative bounds and testing whole bundles of rays that are either free or blocked. See `BENCHMARKS.md` §11.

Any of those **changes the lighting**, so you have to explicitly decide how much deviation is
acceptable. And that changes the validation: requiring a byte-identical `lighting` lump no longer works.

### Per-leaf sky culling: already measured, does not pay off

It looked like the best idea: 67% of sky rays are occluded, so skipping the loop for samples that
do not see the sky would cost nothing in quality. Measure the ceiling before implementing it, by counting entries to the loop with
zero hits:

| map | entries | zero hits | ceiling |
|---|---|---|---|
| koth_sandy | 2,028 | 466 | 23% |
| ba_dust_island | 6,623 | 137 | **2.1%** |

On the heavy map, 98% of the samples see some sky. The expensive maps are expensive *because* they are
outdoors. The waste is **inside** each loop, not in whole loops, so per-sample culling
does not touch it.

### What would remain (changes the lighting)

The sky levels are a subdivision hierarchy (258 → 1,026 → 4,098 → 16,386). A
coarse-to-fine scheme could test coarsely first and skip the subdivisions of the fully
occluded regions. It is real but complex and approximate, so it can no longer be validated with a byte-identical hash.

## 6. How to measure a change without fooling yourself

Three things I learned measuring here that will save you mistakes:

**Interleave A/B.** A non-interleaved measurement gave me a "20% regression" that was another compile
running on the same machine. Alternate the old binary and the new binary in the same batch:

```powershell
python scripts\compilebench.py --tools tools_old --map m.map --threads 1 --runs 4 --json a.json
python scripts\compilebench.py --tools tools_new --map m.map --threads 1 --runs 4 --json b.json
python scripts\compilebench.py --compare a.json b.json
```

**Look at every run, not the average.** `compilebench.py` prints `all_totals`. If the spread
within one variant is larger than the difference between variants, you measured nothing. That is exactly what
happened with `-O3` and LTO.

**Verify correctness separately from time.** A ray casting optimization that does not intend to change the
result must give an identical `lighting` lump:

```powershell
python scripts\compilebench.py --compare a.json b.json   # includes per-lump hash
python scripts\bspcheck.py mymap.bsp                     # geometry
```

If the `lighting` hash changes and that was not the intent, the optimization is wrong, no matter how much
it sped things up.

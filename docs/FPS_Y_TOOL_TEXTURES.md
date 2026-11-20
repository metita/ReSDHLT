# Lowering `wpoly` and raising FPS in CS 1.6

This is the practical guide for the person making the map. It is kept apart from the rest of the documentation
because it is, measured against anything else, **the biggest lever there is over FPS**,
and it is not in the compiler, it is in how the map is built.

## Why this guide exists

The compile tools can do little for FPS. VIS already computes a practically exact PVS,
and the BSP's face merging already eliminates between **19% and 46%** of faces depending on the map
(measured on three real maps from this repo, see §6). There is no magic flag missing.

What does move the needle is reducing **what the engine has to draw every frame**. That is decided
by geometry and visibility blocking, in other words, by the mapper.

## 1. Measure before optimizing

In the console:

```
developer 1
r_speeds 1
```

You will see `wpoly` (world polygons) and `epoly` (entities/models).

Practical references for CS 1.6:

| `wpoly` | Situation |
|---|---|
| < 600 | Comfortable |
| 600 – 800 | Acceptable |
| 800 – 1000 | Starts to hurt on modest machines |
| > 1000 | Problem zone |

Rerun the map, noting `wpoly` at the same points before and after each change. Without that you are
guessing. The `scripts/compilebench.py` script in this repo gives you the BSP's total face count,
which is useful as a global number, but `r_speeds` is the one that tells you what happens **in each place**.

## 2. NULL, the most cost-effective tool

`NULL` **deletes the face** instead of drawing it. Every surface the player cannot see should be
NULL: back faces of walls, the underside of the floor, the sides of blocks against other blocks,
the tops of boxes that cannot be climbed.

It is the most direct FPS improvement and zero risk: if the player does not see it, it should cost nothing.

> Note: NULL removes the face but **not** the visibility blocking or the collision. The brush is still
> solid and still blocks VIS. That is exactly what you want.

## 3. HINT and SKIP: controlling the BSP cuts

- **HINT**: forces the BSP to cut at that plane, creating a leaf boundary wherever you want.
- **SKIP**: the face is neither drawn nor taken into account; it is used on the other faces of the hint brush.

A typical hint brush: the face that defines the cut gets `HINT`, the other five get `SKIP`.

**Where to put them:** in the passages between large zones: doors, corridors, bends. The idea is that
when you are in zone A, the PVS does not include all of zone B. A well-placed hint in a doorway can
remove hundreds of `wpoly`.

**Where NOT to put them:** everywhere "just in case". Extra hints fragment the BSP, raise the
face count and make VIS slower, making worse the two things you wanted to fix.

## 4. SOLIDHINT and BEVELHINT: avoiding useless subdivision

This is the ceiling worth understanding. The BSP subdivides faces because the engine cannot have
lightmaps larger than a certain extent:

```c
#define TEXTURE_STEP        16   // luxels per lightmap unit
#define MAX_SURFACE_EXTENT  16   // limit of the software renderer and the HLDS
#define DEFAULT_SUBDIVIDE_SIZE ((MAX_SURFACE_EXTENT-1)*TEXTURE_STEP)  // = 240
```

**`-subdivide` cannot be raised above 240**: the map stops loading in the software renderer and in
the HLDS. So "I compile with a large subdivide and gain FPS" is not a real option.

What can be done is avoiding unnecessary cuts:

- **`-gridsubdivide`** (BSP, or "Subdivide by light cells" in the GUI): the engine's real limit
  is not 240 units but 16 lightmap cells per axis. The classic rule cuts every 224 units without
  looking at where the face falls on the grid, and an aligned 256 wall comes out split into 224 + 32. With this
  option BSP counts cells like the engine does and cuts only when needed: 3% to 20% fewer
  faces on the test maps, without touching the map. It pays off more the more the geometry is aligned to the 16 grid
  and the textures are at scale 1 with no rotation. Details in `docs/BENCHMARKS.md` §10.4.

- **SOLIDHINT**: eliminates the face subdivision that this brush causes on the faces it touches.
  A classic for terrain, ramps, stairs and complex shapes, where the BSP generates a mess of
  tiny faces for no reason.
- **BEVELHINT** (specific to SDHLT): acts as `SOLIDHINT` **and** `BEVEL` at once. It avoids
  subdivision and bevels the clipnodes in the same pass. It is what you want on terrain and on clipping of
  spiral stairs.

These two usually give large reductions in face count on irregular geometry, which is exactly where
`wpoly` shoots up.

## 5. The rest of the kit

| Texture | What it is for |
|---|---|
| `CLIP` | Collision with no visible face. For smoothing stairs and edges |
| `CLIPBEVEL` / `CLIPBEVELBRUSH` | Beveled collision, avoids getting stuck on corners |
| `NOCLIP` | Visible but with no collision |
| `BEVEL` | Bevels clipnodes, against getting stuck on protrusions |
| `SPLITFACE` | Subdivides the faces it touches along its edges (equivalent to `zhlt_chopdown`) |
| `ORIGIN` | Defines the rotation point of brush entities |
| `CONTENTWATER` | Water contents |
| `BOUNDINGBOX` | Defines an entity's bbox |
| `%texname` | Per-texture minlight. `%texname` = `_minlight 1.0`; `%#texname` with `#` from 0 to 255 |

## 6. What the compiler does on its own

Measured with this repo on three real maps, the BSP's coplanar face merging
(`MergeAll` / `TryMerge`) already eliminates:

| Map | Final faces | Freed by merging | Reduction |
|---|---|---|---|
| ba_coliseum | 1144 | 975 | 46% |
| koth_sandy | 1259 | 303 | 19% |
| ba_dust_island | 583 | 149 | 20% |

In other words: the compiler is already doing considerable work, and it reaches a fixed point per plane
(each merge retries against the whole list). Do not expect big gains from touching that algorithm;
the margin is in not generating the input faces.

## 7. VIS: `-full` and visibility blocking

- Compile the final release with **`-full`** in VIS. It takes longer, gives the tightest PVS, and that means less
  `wpoly` in game.
- What really lowers the PVS is **blocking the line of sight**: corridors with bends, level changes,
  walls that cut long sightlines. An open map where everything is visible from everywhere will have
  high `wpoly` and no compiler can fix that.
- `info_portal` (specific to SDHLT, it targets any entity with a targetname) forces visibility between two specific leafs. They are for
  one-off cases, not for lowering `wpoly` in general.

## 8. About lighting

RAD **does not affect `wpoly`**. Lightmaps do not change the geometry that gets drawn. Raising light quality
costs you compile time and some BSP size, but **not FPS in game**. It is the area where
you can be generous without fear:

- `-extra` (which in SDHLT raises `-bounce` to a minimum of 12)
- lower `-chop` / `-texchop` for more detail, at the cost of time and size

## 9. Suggested order of work

1. `r_speeds 1` and find the points where `wpoly` is high.
2. `NULL` on everything invisible. It is the cheapest and breaks nothing.
3. `SOLIDHINT` / `BEVELHINT` on terrain and complex shapes.
4. `HINT` in the passages between large zones, **one at a time and measuring**.
5. If it is still high: it is design. Cut sightlines.
6. Final release with `-full` in VIS.

Steps 2 and 3 almost always give more than any compiler flag.

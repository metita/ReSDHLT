# Static entity merging (`-mergeentities`)

A guide for the person making the map. It explains what problem the option solves, when to
use it, and in which cases the compiler will refuse to merge.

## The problem

Every brush entity in the map costs **one BSP model**. The format limit is
512 models (`MAX_MAP_MODELS`), but the ceiling that really hurts is in the
engine: the model precache table is a single one, shared by the BSP's `*N` models
with player models, weapons, props and sprites. A map that spends 300
slots on decoration leaves the server with no room for everything else.

That is paid per entity, not per brush. A 1-brush `func_illusionary` and another
one with 80 brushes cost exactly the same: one slot.

The typical case is a field of bushes. They are made with `func_illusionary` because
it is the only way to get transparency: `rendermode` is a runtime entity
property, so a `worldspawn` brush (or a `func_detail`, which ends up merged into
`worldspawn` in CSG) cannot have it. If there are 200 bushes, that is 200 entities and 200 slots.

## What the option does

In CSG, before anything starts handing out models, it looks for brush entities
that are **interchangeable** and merges them into one. The brushes are not
touched: they stay the same, with the same geometry, the same textures and
the same lighting. The only thing that changes is which entity they hang from.

200 identical bushes spread around the map become a handful of
entities, one per area.

## Usage

```
sdHLCSG map.map -mergeentities
```

Options:

| Option | Effect |
|---|---|
| `-mergeentities` | enables merging (off by default) |
| `-mergesize #` | maximum size of the merged group, in units. Default 1024. `0` = no limit |
| `-mergeblend` | also allows merging the render modes that the engine blends (see below) |

`-mergesize` and `-mergeblend` enable merging by themselves, there is no need to also pass
`-mergeentities`.

When it finishes, CSG reports what it did:

```
Merged 23 static brush entities into 4 (19 models freed)
```

With `-verbose` it also lists each group, with its class and the size of the box that
resulted:

```
merge: 8 func_illusionary entities -> entity 1, box 368 x 32 x 96
```

## Why `-mergesize` exists

A brush entity is culled using its model's bounding box: if the box is
not in the player's PVS, the engine neither sends nor draws it.

If all the bushes in the map were merged into a single entity, that box would be
the size of the map and would be in the PVS from almost anywhere. The result
would be worse than not merging at all: slots are saved but decoration that
cannot be seen gets drawn.

That is why candidates are grouped by proximity, and a group stops growing when
its box would exceed `-mergesize` units on any axis. The default of 1024
keeps groups within something like a room or a sector. If the
map is very open it can be raised; if it is made of corridors, lower it.

## What gets merged and what does not

Only entities that are provably equivalent are merged. To qualify,
an entity must meet **all** of the following:

- Class on the allowlist: `func_illusionary` or `func_wall`. These are the two classes
  that do not move, do not think and cannot be triggered.
- **All** keyvalues identical to those of the others in the group. Two bushes
  with a different `renderamt` go to different groups.
- No `targetname`, `target`, `killtarget`, `globalname`, `parentname`,
  `master`, `netname` or `message`. Any of those keys ties the entity to the
  rest of the map.
- No origin brush (`origin` at 0 0 0).
- No `zhlt_usemodel` or `zhlt_minsmaxs`.
- No `zhlt_nomerge` `1`.
- `rendermode` **Normal (0)** or **Solid (4)**, unless `-mergeblend` is passed.

Everything else stays as it was.

### About `rendermode`

Transparent entities that the engine **blends** (Texture, Additive, Glow,
Color) are sorted by depth using **a single point per entity**. If several are
merged into one, they all end up sharing that point and may be drawn in the
wrong order relative to each other.

The two modes that do not have that problem are:

- **Normal (0)**: opaque, not sorted.
- **Solid (4)**: the alpha test used with `{` textures. It cuts out the pixel,
  it does not blend it, so it is not sorted either.

And **Solid is precisely the one bushes use**, so the case that motivates
all this is accepted without restrictions. The other modes are left out unless
`-mergeblend` is requested on purpose.

### Manual opt-out

To leave a specific entity out of merging, add this to it:

```
zhlt_nomerge  1
```

It is useful when you want to keep an entity separate for some reason that the
compiler cannot see.

## Recommendations

- Bushes and vegetation: `func_illusionary`, `{` texture, **rendermode
  Solid, renderamt 255**, no name. They merge on their own.
- There is no need to group by hand in the editor. It makes no difference, and doing it by hand makes it easier to
  slip up and put something with a `targetname` in the group.
- If the map is very open, try `-mergesize 2048` and watch `r_speeds`. If
  decoration shows up being drawn from far away, lower it.
- With `-onlyents` the option is ignored: there CSG only rewrites the entity
  lump of a BSP that already has its models built, and changing the entity list
  would misalign them.

## What this is not

It does not give transparency to `func_detail` or to world brushes. That cannot be done:
`rendermode` only exists for entities, and `func_detail` stops being an
entity in CSG. What this option does is take away almost all of the cost of
`func_illusionary`, which is the real reason one would want to avoid it.

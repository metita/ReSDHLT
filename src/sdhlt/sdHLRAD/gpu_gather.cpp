#include "qrad.h"
#include "gpu_gather.h"

bool            g_gpu = false;
bool            g_gpu_auto = false;
bool            g_gpu_gather = false;
bool            g_gpu_transfers = false;
int             g_gpu_adapter = -1;
int             g_gpu_phase = 0;

#ifdef SDHLT_GPU

#include <algorithm>
#include <chrono>
#include <cstdlib>
#include <cstring>
#include <map>
#include <mutex>
#include <string>
#include <utility>
#include <vector>

#include "gpu/gpu.h"

namespace
{
    // The upper clamp, and the size the resident chunk buffers are cut to; the
    // dispatch loop steers the live chunk somewhere below this.
    const size_t GATHER_CHUNK_ITEMS = 65536;

    // A work item's cost scales with how many emitters the kernel walks for
    // it, so a fixed item count says nothing about how long a dispatch runs: a
    // map carrying 16k texlight emitters can spend many seconds in one, and
    // Windows resets the device after about two. Measure each dispatch and
    // steer the next one at a target instead. The target is deliberately far
    // under that window - the spread around the average is wide and only the
    // peak matters. An extra dispatch costs microseconds of submit overhead,
    // while one overshoot loses the whole gather to the CPU fallback.
    const double GATHER_TARGET_SECONDS = 0.20;
    const double GATHER_CEILING_SECONDS = 0.40;
    const size_t GATHER_CHUNK_MIN = 256;
    const size_t GATHER_CHUNK_START = 2048;
    const size_t GPU_AUTO_GATHER_LIGHTS = 150;

    struct gpu_gather_data
    {
        // collect
        std::vector< std::vector< rad::gpu::work_item_gpu > > face_items;
        std::vector< std::vector< byte > > pvs_rows;
        std::map< std::string, int > row_lookup;
        std::mutex row_mutex;
        size_t rowbytes;

        // consume
        std::vector< rad::gpu::work_item_gpu > items;       // flat, face major
        std::vector< int > face_first;
        std::vector< int > face_cursor;
        std::vector< rad::gpu::gather_result_gpu > results;
        int style_to_compact[ALLSTYLES];
        std::vector< int > compact_to_style;

        gpu_gather_data () : rowbytes (0)
        {
            for (int s = 0; s < ALLSTYLES; s++)
            {
                style_to_compact[s] = -1;
            }
        }
    };

    gpu_gather_data *g_data = NULL;
    std::vector< float > g_face_gap;                        // 6 floats per face, kept for the CPU resolver
    std::vector< directlight_t * > g_flatlights;            // same order as scene.lights

    thread_local int t_current_face = -1;

    // Consecutive lmcache points almost always share a PVS row - CalcLightmap
    // only decompresses one when the leaf changes - so remembering the last
    // one per thread turns a mutex and a std::string per gather call, a million
    // of them per map, into a memcmp that nearly always hits.
    thread_local std::vector< byte > t_last_pvs;
    thread_local int t_last_row = -1;
    thread_local int t_last_generation = -1;
    // The row table is rebuilt for every chunk, so a remembered index is only
    // good for the chunk it came from.
    int g_row_generation = 0;

    size_t CountDirectLights ()
    {
        size_t count = 0;
        const int numleafs = 1 + g_dmodels[0].visleafs;
        for (int leaf = 0; leaf < numleafs; leaf++)
        {
            for (directlight_t *light = RadGpuDirectLights (leaf); light; light = light->next)
            {
                count++;
            }
        }
        return count;
    }

    bool MarshalScene (rad::gpu::gather_scene &scene)
    {
        int tnodecount = ExportTnodes (NULL, 0);
        if (tnodecount <= 0)
        {
            return false;
        }
        // Same 32 byte layout, declared twice so that trace.cpp does not have
        // to know about the GPU backend and gpu.h does not have to know about
        // RAD. The assert is what keeps that honest.
        static_assert (sizeof (gputnode_t) == sizeof (rad::gpu::tnode_gpu),
                       "gputnode_t must match the kernel's Tnode layout");
        scene.tnodes.resize ((size_t)tnodecount);
        if (ExportTnodes ((gputnode_t *)&scene.tnodes[0], tnodecount) != tnodecount)
        {
            return false;
        }

        // The kernel walks the lights in the CPU's order: leaf index ascending,
        // then the leaf's list in order. Flatten in exactly that order and the
        // per-sample accumulation order - and so the result - is preserved.
        const int numleafs = 1 + g_dmodels[0].visleafs;
        for (int i = 0; i < numleafs; i++)
        {
            directlight_t *l = RadGpuDirectLights (i);
            if (!l)
            {
                continue;
            }
            rad::gpu::lightleaf_gpu leaf;
            leaf.leafnum = i;
            leaf.firstlight = (int)scene.lights.size ();
            leaf.numlights = 0;
            leaf.pad = 0;
            for (; l; l = l->next)
            {
                if (l->style < 0 || l->style >= ALLSTYLES)
                {
                    return false;
                }
                if (g_data->style_to_compact[l->style] < 0)
                {
                    if ((int)g_data->compact_to_style.size () >= rad::gpu::max_compact_styles)
                    {
                        return false;                       // more distinct styles than the kernel handles
                    }
                    g_data->style_to_compact[l->style] = (int)g_data->compact_to_style.size ();
                    g_data->compact_to_style.push_back (l->style);
                }

                rad::gpu::light_gpu out;
                memset (&out, 0, sizeof (out));
                out.type = (int)l->type;
                out.compact_style = g_data->style_to_compact[l->style];
                out.topatch = l->topatch? 1: 0;
                out.sun_ofs = (int)(scene.sun_normals.size () / 4);
                out.sun_count = 0;
                if (l->type == emit_skylight && l->sunnormals && l->sunnormalweights)
                {
                    out.sun_count = l->numsunnormals;
                    for (int j = 0; j < l->numsunnormals; j++)
                    {
                        scene.sun_normals.push_back ((float)l->sunnormals[j][0]);
                        scene.sun_normals.push_back ((float)l->sunnormals[j][1]);
                        scene.sun_normals.push_back ((float)l->sunnormals[j][2]);
                        scene.sun_normals.push_back ((float)l->sunnormalweights[j]);
                    }
                }
                for (int x = 0; x < 3; x++)
                {
                    out.origin[x] = (float)l->origin[x];
                    out.intensity[x] = (float)l->intensity[x];
                    out.normal[x] = (float)l->normal[x];
                    out.diffuse[x] = (float)l->diffuse_intensity[x];
                    out.diffuse2[x] = (float)l->diffuse_intensity2[x];
                }
                out.stopdot = (float)l->stopdot;
                out.stopdot2 = (float)l->stopdot2;
                out.fade = (float)l->fade;
                out.texlightgap = (float)l->texlightgap;
                out.patch_area = (float)l->patch_area;
                out.patch_emitter_range = (float)l->patch_emitter_range;
                scene.lights.push_back (out);
                g_flatlights.push_back (l);
                leaf.numlights++;
            }
            scene.lightleafs.push_back (leaf);
        }
        if (scene.lights.empty ())
        {
            return false;
        }

        const int skylevel = g_softsky? g_skylevel: SKYLEVEL_SOFTSKYOFF;
        for (int j = 0; j < g_numskynormals[skylevel]; j++)
        {
            scene.sky_normals.push_back ((float)g_skynormals[skylevel][j][0]);
            scene.sky_normals.push_back ((float)g_skynormals[skylevel][j][1]);
            scene.sky_normals.push_back ((float)g_skynormals[skylevel][j][2]);
            scene.sky_normals.push_back ((float)g_skynormalsizes[skylevel][j]);
        }
        scene.sky_lighting_fix = g_sky_lighting_fix? 1: 0;
        scene.sky_step_match = (g_softsky || g_fastmode)? 1: 0;
        scene.indirect_sun = (float)g_indirect_sun;

        // The texlightgap basis, per face: the kernel needs it as data, the CPU
        // resolver reuses the same table.
        scene.face_gap.assign ((size_t)g_numfaces * 6, 0.0f);
        for (int f = 0; f < g_numfaces; f++)
        {
            vec3_t textoworld[2];
            RadGpuTexToWorld (f, textoworld);
            for (int x = 0; x < 2; x++)
            {
                scene.face_gap[(size_t)f * 6 + x * 3 + 0] = (float)textoworld[x][0];
                scene.face_gap[(size_t)f * 6 + x * 3 + 1] = (float)textoworld[x][1];
                scene.face_gap[(size_t)f * 6 + x * 3 + 2] = (float)textoworld[x][2];
            }
        }
        return true;
    }

    // The CPU resolver for near pairs: the emit_surface case of
    // GatherSampleLight with the near branch taken. The kernel routes those
    // back here because they need the emitter's winding, an alternate origin
    // and a sight-area integration - reference code, so the result is the
    // reference result.
    bool ResolveNearPair (const vec3_t pos, const vec3_t normal, const directlight_t *l,
                          vec_t lighting_power, vec_t lighting_scale, const float *gap6,
                          vec3_t add_out)
    {
        const bool lighting_diversify = (lighting_power != 1.0 || lighting_scale != 1.0);

        vec3_t testline_origin;
        VectorCopy (l->origin, testline_origin);
        vec3_t delta;
        VectorSubtract (l->origin, pos, delta);
        VectorMA (delta, -PATCH_HUNT_OFFSET, l->normal, delta);
        vec_t dist = VectorNormalize (delta);
        vec_t dot = DotProduct (delta, normal);
        if (dist < 1.0)
        {
            dist = 1.0;
        }

        bool light_behind_surface = false;
        if (dot <= NORMAL_EPSILON)
        {
            light_behind_surface = true;
        }
        if (lighting_diversify && !light_behind_surface)
        {
            dot = lighting_scale * pow (dot, lighting_power);
        }
        vec_t dot2 = -DotProduct (delta, l->normal);
        if (l->texlightgap > 0)
        {
            vec3_t g0, g1;
            VectorCopy (gap6, g0);
            VectorCopy (gap6 + 3, g1);
            vec_t test = dot2 * dist;
            test -= l->texlightgap * fabs (DotProduct (l->normal, g0));
            test -= l->texlightgap * fabs (DotProduct (l->normal, g1));
            if (test < -ON_EPSILON)
            {
                return false;
            }
        }
        if (dot2 * dist <= MINIMUM_PATCH_DISTANCE)
        {
            return false;
        }

        vec_t range = l->patch_emitter_range;
        vec_t ratio;
        if (l->stopdot > 0.0)
        {
            vec_t range_scale = 1 - l->stopdot2 * l->stopdot2;
            range_scale = 1 / sqrt (qmax (NORMAL_EPSILON, range_scale));
            range_scale = qmin (range_scale, 2);
            range *= range_scale;
            if (dot2 <= l->stopdot2 + NORMAL_EPSILON)
            {
                if (dist >= range)
                {
                    return false;
                }
                ratio = 0.0;
            }
            else if (dot2 <= l->stopdot)
            {
                ratio = dot * dot2 * (dot2 - l->stopdot2) / (dist * dist * (l->stopdot - l->stopdot2));
            }
            else
            {
                ratio = dot * dot2 / (dist * dist);
            }
        }
        else
        {
            ratio = dot * dot2 / (dist * dist);
        }
        if (ratio * l->patch_area > 0.4f)
        {
            ratio = 0.4f / l->patch_area;
        }
        if (!(dist < range - ON_EPSILON))
        {
            return false;                                   // the kernel handles the far branch
        }

        if (light_behind_surface)
        {
            dot = 0.0;
            ratio = 0.0;
        }
        GetAlternateOrigin (pos, normal, l->patch, testline_origin);
        vec_t sightarea;
        int skylevel = l->patch->emitter_skylevel;
        if (l->stopdot > 0.0)
        {
            const vec_t *emitnormal = getPlaneFromFaceNumber (l->patch->faceNumber)->normal;
            if (l->stopdot2 >= 0.8)
            {
                skylevel += 1;
            }
            sightarea = CalcSightArea_SpotLight (pos, normal, l->patch->winding, emitnormal,
                                                 l->stopdot, l->stopdot2, skylevel,
                                                 lighting_power, lighting_scale);
        }
        else
        {
            sightarea = CalcSightArea (pos, normal, l->patch->winding, skylevel,
                                       lighting_power, lighting_scale);
        }
        vec_t frac = dist / range;
        frac = (frac - 0.5) * 2;
        frac = qmax (0, qmin (frac, 1));
        vec_t ratio2 = (sightarea / l->patch_area);
        ratio = frac * ratio + (1 - frac) * ratio2;

        if (TestLine (pos, testline_origin) != CONTENTS_EMPTY)
        {
            return false;
        }
        VectorScale (l->intensity, ratio, add_out);
        return true;
    }
}

void GpuGatherBeginFace (int facenum)
{
    t_current_face = facenum;
}

void GpuGatherIntercept (const vec3_t pos, const byte* const pvs, const vec3_t normal,
                         vec3_t *sample, byte *styles, int step, int miptex,
                         int texlightgap_surfacenum)
{
    (void)sample;
    (void)styles;
    gpu_gather_data &d = *g_data;
    // Group by the face whose BuildFacelights is running, not by the sample's
    // own surface: near an edge, texlightgap_surfacenum names a neighbouring
    // face another thread owns.
    const int facenum = t_current_face;
    if (facenum < 0)
    {
        Error ("GPU gather: intercept outside BuildFacelights");
    }

    {
        rad::gpu::work_item_gpu item;
        memset (&item, 0, sizeof (item));
        for (int x = 0; x < 3; x++)
        {
            item.pos[x] = (float)pos[x];
            item.normal[x] = (float)normal[x];
        }
        item.cone_power = (float)g_lightingconeinfo[miptex][0];
        item.cone_scale = (float)g_lightingconeinfo[miptex][1];
        item.step = step;
        item.face = texlightgap_surfacenum;
        item.sky_reach = RadGpuSampleMayReachSky (pvs)? 1: 0;
        if (t_last_row >= 0 && t_last_generation == g_row_generation
            && t_last_pvs.size () == d.rowbytes
            && !memcmp (&t_last_pvs[0], pvs, d.rowbytes))
        {
            item.pvs_row = t_last_row;
        }
        else
        {
            std::lock_guard< std::mutex > guard (d.row_mutex);
            std::string key ((const char *)pvs, d.rowbytes);
            std::map< std::string, int >::iterator it = d.row_lookup.find (key);
            if (it == d.row_lookup.end ())
            {
                it = d.row_lookup.insert (std::make_pair (key, (int)d.pvs_rows.size ())).first;
                d.pvs_rows.push_back (std::vector< byte > (pvs, pvs + d.rowbytes));
            }
            item.pvs_row = it->second;
            t_last_pvs.assign (pvs, pvs + d.rowbytes);
            t_last_row = it->second;
            t_last_generation = g_row_generation;
        }
        d.face_items[facenum].push_back (item);
    }
}

// The apply half. CalcLightmap re-walks its lmcache points in the same order
// under the same conditions, so the nth call for a face is the nth item that
// face recorded - no key, no lookup, just a cursor.
void GpuGatherApply (vec3_t *sample, byte *styles)
{
    gpu_gather_data &d = *g_data;
    const int facenum = t_current_face;
    if (facenum < 0)
    {
        Error ("GPU gather: apply outside BuildFacelights");
    }
    const int cursor = d.face_cursor[facenum]++;
    if (cursor >= (int)d.face_items[facenum].size ())
    {
        Error ("GPU gather: face %d asked for result %d of %d", facenum, cursor,
               (int)d.face_items[facenum].size ());
    }
    const int flat = d.face_first[facenum] + cursor;
    const rad::gpu::gather_result_gpu &r = d.results[flat];
    const vec_t *pos = NULL;
    vec3_t itempos;
    {
        const rad::gpu::work_item_gpu &item = d.items[flat];
        VectorCopy (item.pos, itempos);
        pos = itempos;
    }

    // The same tail the CPU gather ends with: ascending style order, coring
    // threshold, style slot assignment, discarded light bookkeeping.
    for (int style = 0; style < ALLSTYLES; style++)
    {
        const int compact = d.style_to_compact[style];
        if (compact < 0 || !(r.touched & (1u << compact)))
        {
            continue;
        }
        vec3_t adds;
        adds[0] = r.adds[compact * 3 + 0];
        adds[1] = r.adds[compact * 3 + 1];
        adds[2] = r.adds[compact * 3 + 2];

        if (VectorMaximum (adds) > g_corings[style] * 0.1)
        {
            int style_index;
            for (style_index = 0; style_index < ALLSTYLES; style_index++)
            {
                if (styles[style_index] == style || styles[style_index] == 255)
                {
                    break;
                }
            }
            if (style_index == ALLSTYLES)                   // shouldn't happen
            {
                return;
            }
            if (styles[style_index] == 255)
            {
                styles[style_index] = (byte)style;
            }
            VectorAdd (sample[style_index], adds, sample[style_index]);
        }
        else
        {
            if (VectorMaximum (adds) > g_maxdiscardedlight + NORMAL_EPSILON)
            {
                ThreadLock ();
                if (VectorMaximum (adds) > g_maxdiscardedlight + NORMAL_EPSILON)
                {
                    g_maxdiscardedlight = VectorMaximum (adds);
                    VectorCopy (pos, g_maxdiscardedpos);
                }
                ThreadUnlock ();
            }
        }
    }
}

const char *GpuDeviceDescription ()
{
    static std::string desc;
    rad::gpu::set_adapter_override (g_gpu_adapter);
    if (rad::gpu::available ())
    {
        desc = rad::gpu::device_name ();
    }
    else
    {
        desc = "none found (will use the CPU)";
    }
    return desc.c_str ();
}

void GpuGatherFinish ()
{
    g_gpu_phase = 0;
    delete g_data;
    g_data = NULL;
    g_face_gap.clear ();
    g_flatlights.clear ();
}

namespace
{
    // How much of a chunk's face state we are willing to hold at once. The
    // lmcache dominates it - one vec3_t per light style per lmcache point,
    // megabytes on a large face with -extra - and it is the price of not
    // recomputing everything a second time.
    const size_t CHUNK_BYTE_BUDGET = 512u * 1024u * 1024u;
    const int CHUNK_FACES_START = 64;
    const int CHUNK_FACES_MAX = 4096;

    // Set for the duration of a chunk so the worker callbacks know their range.
    int g_chunk_base = 0;
    std::vector< char > g_chunk_state;
    std::vector< unsigned char > g_chunk_live;

    void ChunkCollect (int i)
    {
        void *state = &g_chunk_state[(size_t)i * RadGpuFaceStateSize ()];
        g_chunk_live[i] = RadGpuFaceBegin (g_chunk_base + i, state)? 1: 0;
    }

    void ChunkFinish (int i)
    {
        if (!g_chunk_live[i])
        {
            return;
        }
        void *state = &g_chunk_state[(size_t)i * RadGpuFaceStateSize ()];
        RadGpuFaceEnd (g_chunk_base + i, state);
    }

    // Dispatches the items collected for the current chunk. Returns false if
    // the device failed, in which case the caller has to abandon the GPU path.
    bool DispatchChunk (size_t first_item, size_t &chunk_hint,
                        std::vector< rad::gpu::near_pair > &near_pairs,
                        size_t &dispatches, double &spent, double &slowest,
                        size_t &chunk_ceiling)
    {
        for (size_t base = first_item; base < g_data->items.size ();)
        {
            const size_t count = qmin (g_data->items.size () - base, chunk_hint);
            std::vector< rad::gpu::gather_result_gpu > chunk_results;
            std::vector< rad::gpu::near_pair > chunk_near;
            const std::chrono::steady_clock::time_point started = std::chrono::steady_clock::now ();
            const bool ok = rad::gpu::gather_batch (&g_data->items[base], count, chunk_results, chunk_near);
            const double elapsed = std::chrono::duration< double > (std::chrono::steady_clock::now () - started).count ();
            if (!ok)
            {
                Warning ("-gpu: %s (dispatch %d, item %d of %d, chunk %d, %.3fs)",
                         rad::gpu::last_error ().c_str (), (int)dispatches + 1, (int)base,
                         (int)g_data->items.size (), (int)count, elapsed);
                return false;
            }
            dispatches++;
            spent += elapsed;
            if (elapsed > slowest)
            {
                slowest = elapsed;
            }
            memcpy (&g_data->results[base], &chunk_results[0],
                    chunk_results.size () * sizeof (rad::gpu::gather_result_gpu));
            for (size_t p = 0; p < chunk_near.size (); p++)
            {
                rad::gpu::near_pair pair = chunk_near[p];
                pair.item += (uint32_t)base;
                near_pairs.push_back (pair);
            }
            base += count;

            // A dispatch that ran long ratchets the ceiling down for good: the
            // average says little here, so the run is paced by the worst chunk.
            if (elapsed > GATHER_CEILING_SECONDS)
            {
                const double scaled = (double)count * (GATHER_CEILING_SECONDS / elapsed);
                const size_t capped = scaled < (double)GATHER_CHUNK_MIN? GATHER_CHUNK_MIN: (size_t)scaled;
                if (capped < chunk_ceiling)
                {
                    chunk_ceiling = capped;
                }
            }
            if (elapsed > 1e-6)
            {
                double factor = GATHER_TARGET_SECONDS / elapsed;
                factor = qmax (0.25, qmin (factor, 1.5));  // creep up, shrink at once
                const double next = (double)count * factor;
                const size_t ceiling = qmin (chunk_ceiling, GATHER_CHUNK_ITEMS);
                chunk_hint = next < (double)GATHER_CHUNK_MIN? GATHER_CHUNK_MIN
                           : (next > (double)ceiling? ceiling: (size_t)next);
            }
        }
        return true;
    }

    // The texlight near branch, resolved with the reference CPU functions for
    // the pairs the kernel routed back. Sorted by item then light so the merge
    // order matches the CPU's ascending light loop.
    void ResolveNearPairs (std::vector< rad::gpu::near_pair > &near_pairs)
    {
        if (near_pairs.empty ())
        {
            return;
        }
        std::sort (near_pairs.begin (), near_pairs.end (),
            [](const rad::gpu::near_pair &a, const rad::gpu::near_pair &b)
            {
                return a.item != b.item? a.item < b.item: a.light < b.light;
            });
        for (size_t p = 0; p < near_pairs.size (); p++)
        {
            const rad::gpu::near_pair &pair = near_pairs[p];
            if (pair.light >= g_flatlights.size ())
            {
                continue;
            }
            const rad::gpu::work_item_gpu &item = g_data->items[pair.item];
            directlight_t *l = g_flatlights[pair.light];
            vec3_t pos, normal, add;
            VectorCopy (item.pos, pos);
            VectorCopy (item.normal, normal);
            if (ResolveNearPair (pos, normal, l, item.cone_power, item.cone_scale,
                                 &g_face_gap[(size_t)item.face * 6], add))
            {
                rad::gpu::gather_result_gpu &out = g_data->results[pair.item];
                const int compact = g_data->style_to_compact[l->style];
                out.adds[compact * 3 + 0] += (float)add[0];
                out.adds[compact * 3 + 1] += (float)add[1];
                out.adds[compact * 3 + 2] += (float)add[2];
                out.touched |= 1u << compact;
            }
        }
        near_pairs.clear ();
    }
}

bool GpuBuildFacelights ()
{
    // The kernel implements the gather, not the whole lighting model. Anything
    // it cannot see has to keep the CPU path, and saying so plainly beats a
    // subtly wrong lightmap.
    if (g_opaque_face_count != 0)
    {
        Warning ("-gpu: the map has opaque entities; using the CPU path");
        return false;
    }
    {
        extern int num_models;                              // studio.cpp
        if (num_models != 0)
        {
            Warning ("-gpu: the map has studio model shadows; using the CPU path");
            return false;
        }
    }
    if (g_numfaces <= 0)
    {
        return false;
    }
    if (g_gpu_auto)
    {
        const size_t light_count = CountDirectLights ();
        if (light_count < GPU_AUTO_GATHER_LIGHTS)
        {
            Log ("BuildFacelights: GPU auto chose CPU (%d lights; crossover starts near %d)\n",
                 (int)light_count, (int)GPU_AUTO_GATHER_LIGHTS);
            return false;
        }
    }
    rad::gpu::set_adapter_override (g_gpu_adapter);

    g_data = new gpu_gather_data ();
    g_data->rowbytes = (size_t)((g_dmodels[0].visleafs + 7) / 8);
    g_data->face_items.resize (g_numfaces);
    g_data->face_first.assign (g_numfaces, 0);
    g_data->face_cursor.assign (g_numfaces, 0);

    rad::gpu::gather_scene scene;
    if (!MarshalScene (scene))
    {
        Warning ("-gpu: the map exceeds the kernel's light or style limits; using the CPU path");
        GpuGatherFinish ();
        return false;
    }

    if (!rad::gpu::available ())
    {
        Warning ("-gpu gather: %s; using the CPU path", rad::gpu::last_error ().c_str ());
        GpuGatherFinish ();
        return false;
    }

    // The face_gap table is needed by the CPU near-pair resolver as well as by
    // the kernel, so keep a copy; gather_begin only reads the scene.
    g_face_gap = scene.face_gap;

    Log ("BuildFacelights (GPU: %s, %d lights in %d lit leaves):\n",
         rad::gpu::device_name ().c_str (), (int)scene.lights.size (), (int)scene.lightleafs.size ());
    g_gpu_phase = 1;

    const size_t statesize = RadGpuFaceStateSize ();
    int chunkfaces = CHUNK_FACES_START;
    size_t dispatches = 0;
    double spent = 0.0;
    double slowest = 0.0;
    size_t chunk_hint = GATHER_CHUNK_START;
    size_t chunk_ceiling = GATHER_CHUNK_ITEMS;
    std::vector< rad::gpu::near_pair > near_pairs;
    size_t totalsamples = 0;
    double t_collect = 0, t_finish = 0;
    bool ok = true;

    // Advance by the count this iteration actually used, not by chunkfaces:
    // the loop retunes chunkfaces at the bottom, and stepping by the new value
    // would skip everything between.
    for (int base = 0; base < g_numfaces && ok;)
    {
        const int count = qmin (chunkfaces, g_numfaces - base);
        g_chunk_base = base;
        g_chunk_state.assign ((size_t)count * statesize, 0);
        g_chunk_live.assign (count, 0);

        // Everything below is per chunk and consumed before the next one, so
        // the buffers are reused rather than grown to the whole map. A million
        // samples would otherwise mean 200 MB of results alone.
        g_data->items.clear ();
        g_data->results.clear ();
        g_data->pvs_rows.clear ();
        g_data->row_lookup.clear ();
        g_row_generation++;                                 // retires the per-thread row memo

        // First half: sample placement, phong normals, PVS, and the gather
        // calls recorded rather than traced. Threaded, like the CPU pass.
        double t0 = I_FloatTime ();
        RunThreadsOnIndividual (count, false, ChunkCollect);
        t_collect += I_FloatTime () - t0;

        const size_t first_item = 0;
        size_t chunkbytes = 0;
        for (int i = 0; i < count; i++)
        {
            const int f = base + i;
            g_data->face_first[f] = (int)g_data->items.size ();
            g_data->items.insert (g_data->items.end (),
                                  g_data->face_items[f].begin (), g_data->face_items[f].end ());
            if (g_chunk_live[i])
            {
                chunkbytes += RadGpuFaceStateBytes (&g_chunk_state[(size_t)i * statesize]);
            }
        }
        g_data->results.resize (g_data->items.size ());
        totalsamples += g_data->items.size ();

        // PVS rows accumulate across chunks, so the session is reopened per
        // chunk with the rows known so far. The scene upload is the same
        // buffers every time; only the row table grows.
        const uint32_t stride_words = (uint32_t)((g_data->rowbytes + 3) / 4 + 1);
        std::vector< uint32_t > pvs_words (g_data->pvs_rows.size () * stride_words, 0);
        for (size_t r = 0; r < g_data->pvs_rows.size (); r++)
        {
            memcpy (&pvs_words[r * stride_words], &g_data->pvs_rows[r][0], g_data->rowbytes);
        }

        if (g_data->items.size () > first_item)
        {
            const size_t chunkitems = g_data->items.size () - first_item;
            const size_t cap = qmin (chunkitems, GATHER_CHUNK_ITEMS);
            if (!rad::gpu::gather_begin (scene, pvs_words, stride_words, (uint32_t)cap))
            {
                Warning ("-gpu: %s; using the CPU path", rad::gpu::last_error ().c_str ());
                ok = false;
            }
            else
            {
                ok = DispatchChunk (first_item, chunk_hint, near_pairs,
                                    dispatches, spent, slowest, chunk_ceiling);
                rad::gpu::gather_end ();
            }
            if (ok)
            {
                ResolveNearPairs (near_pairs);
            }
        }

        if (!ok)
        {
            // Nothing has been written to the map yet for this chunk, and the
            // chunks before it were finished properly - but their lightmaps
            // would be half a compile. Give up on the whole GPU path and let
            // RAD redo BuildFacelights from scratch on the CPU.
            for (int i = 0; i < count; i++)
            {
                if (g_chunk_live[i])
                {
                    RadGpuFaceAbandon (&g_chunk_state[(size_t)i * statesize]);
                }
            }
            break;
        }

        // Second half: the blur, the patch accumulation and the lightmap
        // write, each face reading the results the device just produced.
        g_gpu_phase = 0;
        double t1 = I_FloatTime ();
        RunThreadsOnIndividual (count, false, ChunkFinish);
        t_finish += I_FloatTime () - t1;
        g_gpu_phase = 1;

        // Done with this chunk's items; the memory is worth reclaiming on a
        // large map, and the results are already in the lightmaps.
        for (int i = 0; i < count; i++)
        {
            std::vector< rad::gpu::work_item_gpu > ().swap (g_data->face_items[base + i]);
        }

        // Aim the next chunk at the byte budget, so a map of small faces gets
        // big batches and a map of huge ones does not run the machine out of
        // memory.
        if (chunkbytes > 0)
        {
            const double perface = (double)chunkbytes / count;
            double next = (double)CHUNK_BYTE_BUDGET / perface;
            if (next < CHUNK_FACES_START) next = CHUNK_FACES_START;
            if (next > CHUNK_FACES_MAX) next = CHUNK_FACES_MAX;
            chunkfaces = (int)next;
        }
        base += count;
    }

    if (!ok)
    {
        GpuGatherFinish ();
        for (int f = 0; f < g_numfaces; f++)
        {
            RadGpuResetFace (f);
        }
        return false;
    }

    Log ("  %d samples, %d dispatches, %.2fs on the device (slowest %.3fs)\n",
         (int)totalsamples, (int)dispatches, spent, slowest);
    Log ("  collect %.2fs, finish %.2fs\n", t_collect, t_finish);
    GpuGatherFinish ();
    return true;
}

#else // !SDHLT_GPU

bool GpuBuildFacelights ()
{
    Warning ("-gpu: this build has no GPU backend; using the CPU path");
    return false;
}

void GpuGatherBeginFace (int)
{
}

void GpuGatherIntercept (const vec3_t, const byte* const, const vec3_t, vec3_t *, byte *, int, int, int)
{
    Error ("GpuGatherIntercept called in a build without the GPU backend");
}

void GpuGatherApply (vec3_t *, byte *)
{
    Error ("GpuGatherApply called in a build without the GPU backend");
}

const char *GpuDeviceDescription ()
{
    return "not built in (will use the CPU)";
}

#endif

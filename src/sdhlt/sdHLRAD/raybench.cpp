#include <algorithm>
#include <atomic>
#include <chrono>
#include <functional>
#include <vector>

#ifdef SDHLT_EMBREE
#include <embree4/rtcore.h>
#endif

#include "cmdlib.h"
#include "mathlib.h"
#include "bspfile.h"
#include "log.h"
#include "winding.h"
#include "qrad.h"
#include "raybench.h"

bool            g_raybench = false;

namespace
{
    struct ray_t
    {
        vec3_t          start;
        vec3_t          stop;
    };

    // 1M rays is ~24 MB and takes a couple of seconds to re-trace, which is
    // long enough to swamp timer noise without making the flag annoying.
    const size_t    RAYBENCH_MAX = 1u << 20;

    // Keeping the first N rays would sample whatever faces happen to be
    // scheduled first, so the sample is spread across the whole compile.
    //
    // But it is taken in contiguous runs, not every Nth ray, and that detail
    // decides whether the measurement means anything: consecutive sky rays
    // share an origin (one luxel against the sky dome) and are what any packet
    // or SIMD tracer would actually be handed. Sampling every Nth ray hands it
    // eight unrelated rays from eight parts of the map instead, which is a
    // workload that never occurs and which no vector tracer can do anything
    // with. The run counter is thread local for the same reason: the global one
    // interleaves threads, and "consecutive" would again mean unrelated.
    const long long RAYBENCH_RUN = 64;                     // rays kept in a row
    const long long RAYBENCH_STRIDE = 64;                  // one run in every N

    // Wall clock, not rdtsc: this is a rate in rays per second, so it has to be
    // a real clock, and the region timed is seconds long.
    double Now()
    {
        using namespace std::chrono;
        return duration_cast<duration<double> >(
                   steady_clock::now().time_since_epoch()).count();
    }

    std::vector<ray_t>      s_rays;
    std::atomic<long long>  s_offered(0);
    std::atomic<size_t>     s_stored(0);

#ifdef SDHLT_EMBREE
    RTCDevice       s_device = NULL;
    RTCScene        s_scene = NULL;

    // Per triangle: does it belong to a sky face? TestLine answers SKY / not
    // SKY, so a hit alone is not comparable - the winner has to say which.
    std::vector<unsigned char> s_trisky;

    //
    // Turns the BSP's faces into a triangle soup for Embree.
    //
    // This is where the two tracers stop being the same algorithm, and the
    // measurement has to be read with that in mind:
    //
    //   * TestLine walks leaf contents. Embree hits triangles. A brush face
    //     exists for anything visible, so solid blockers line up well, but a
    //     leaf boundary with no face on it (and CONTENTS_WATER / SLIME
    //     transitions, which TestLine folds into `linecontent`) has no
    //     equivalent here.
    //   * TestLine's ON_EPSILON descends both sides of a near-coplanar plane.
    //     A BVH has no such tolerance.
    //
    // So the rate is the honest part of this comparison and the agreement
    // figure is a sanity check, not a correctness proof. A real port has to
    // solve both of the above; the point of the benchmark is to find out
    // whether it is worth the trouble first.
    //
    bool EmbreeBuild()
    {
        s_device = rtcNewDevice(NULL);
        if (s_device == NULL)
        {
            return false;
        }
        s_scene = rtcNewScene(s_device);
        rtcSetSceneBuildQuality(s_scene, RTC_BUILD_QUALITY_HIGH);

        // Fan triangulation of every face, which is what the winding of a BSP
        // face allows: they are convex by construction.
        std::vector<float>          verts;
        std::vector<unsigned>       indices;

        for (int f = 0; f < g_numfaces; f++)
        {
            const dface_t*  face = &g_dfaces[f];
            if (face->numedges < 3)
            {
                continue;
            }

            const bool issky = !strncasecmp(GetTextureByNumber(face->texinfo), "sky", 3);
            const unsigned  first = (unsigned)(verts.size() / 3);

            for (int e = 0; e < face->numedges; e++)
            {
                const int se = g_dsurfedges[face->firstedge + e];
                const int v = se >= 0 ? g_dedges[se].v[0] : g_dedges[-se].v[1];
                for (int k = 0; k < 3; k++)
                {
                    verts.push_back(g_dvertexes[v].point[k]);
                }
            }

            for (int e = 1; e + 1 < face->numedges; e++)
            {
                indices.push_back(first);
                indices.push_back(first + e);
                indices.push_back(first + e + 1);
                s_trisky.push_back(issky ? 1 : 0);
            }
        }

        if (indices.empty())
        {
            return false;
        }

        RTCGeometry geom = rtcNewGeometry(s_device, RTC_GEOMETRY_TYPE_TRIANGLE);
        float* vb = (float*)rtcSetNewGeometryBuffer(geom, RTC_BUFFER_TYPE_VERTEX, 0,
                                                    RTC_FORMAT_FLOAT3,
                                                    3 * sizeof(float),
                                                    verts.size() / 3);
        unsigned* ib = (unsigned*)rtcSetNewGeometryBuffer(geom, RTC_BUFFER_TYPE_INDEX, 0,
                                                          RTC_FORMAT_UINT3,
                                                          3 * sizeof(unsigned),
                                                          indices.size() / 3);
        if (vb == NULL || ib == NULL)
        {
            return false;
        }
        memcpy(vb, &verts[0], verts.size() * sizeof(float));
        memcpy(ib, &indices[0], indices.size() * sizeof(unsigned));

        rtcCommitGeometry(geom);
        rtcAttachGeometry(s_scene, geom);
        rtcReleaseGeometry(geom);
        rtcCommitScene(s_scene);

        Log("-raybench: embree scene built, %zu triangles from %d faces\n",
            indices.size() / 3, g_numfaces);
        return true;
    }

    // Closest hit, because the caller needs to know whether what stopped the
    // ray was sky or anything else - an occlusion test cannot answer that.
    int EmbreeTestLine(const vec3_t start, const vec3_t stop)
    {
        RTCRayHit rh;
        rh.ray.org_x = (float)start[0];
        rh.ray.org_y = (float)start[1];
        rh.ray.org_z = (float)start[2];
        rh.ray.dir_x = (float)(stop[0] - start[0]);
        rh.ray.dir_y = (float)(stop[1] - start[1]);
        rh.ray.dir_z = (float)(stop[2] - start[2]);
        rh.ray.tnear = 0.0f;
        rh.ray.tfar = 1.0f;                        // direction is the segment
        rh.ray.mask = 0xFFFFFFFF;
        rh.ray.flags = 0;
        rh.ray.time = 0.0f;
        rh.hit.geomID = RTC_INVALID_GEOMETRY_ID;
        rh.hit.primID = RTC_INVALID_GEOMETRY_ID;

        rtcIntersect1(s_scene, &rh, NULL);

        if (rh.hit.geomID == RTC_INVALID_GEOMETRY_ID)
        {
            return CONTENTS_EMPTY;
        }
        return s_trisky[rh.hit.primID] ? CONTENTS_SKY : CONTENTS_SOLID;
    }

    // Occlusion only. Cannot answer sky-or-solid, so it is not a candidate
    // implementation - it is here to separate "the traversal costs this much"
    // from "returning hit data costs this much".
    int EmbreeOccluded(const vec3_t start, const vec3_t stop)
    {
        RTCRay r;
        r.org_x = (float)start[0];
        r.org_y = (float)start[1];
        r.org_z = (float)start[2];
        r.dir_x = (float)(stop[0] - start[0]);
        r.dir_y = (float)(stop[1] - start[1]);
        r.dir_z = (float)(stop[2] - start[2]);
        r.tnear = 0.0f;
        r.tfar = 1.0f;
        r.mask = 0xFFFFFFFF;
        r.flags = 0;
        r.time = 0.0f;

        rtcOccluded1(s_scene, &r, NULL);
        return r.tfar < 0 ? CONTENTS_SOLID : CONTENTS_EMPTY;
    }

    // Eight rays at a time. This is the shape the sky loop actually has - one
    // origin, a fixed set of directions - so it is the fair test of what a
    // vector tracer can do here, and the single-ray number alone would sell it
    // short.
    double MeasureEmbreePackets(size_t count, std::vector<int>& answers)
    {
        answers.assign(count, CONTENTS_EMPTY);

        const double    started = Now();
        long long       traced = 0;
        double          elapsed = 0;
        int             passes = 0;

        while (elapsed < 2.0 && passes < 10000)
        {
            for (size_t i = 0; i < count; i += 8)
            {
                RTCRayHit8  p;
                int         valid[8];
                const size_t n = qmin((size_t)8, count - i);

                for (size_t k = 0; k < 8; k++)
                {
                    valid[k] = k < n ? -1 : 0;
                    const ray_t& ray = s_rays[k < n ? i + k : i];
                    p.ray.org_x[k] = (float)ray.start[0];
                    p.ray.org_y[k] = (float)ray.start[1];
                    p.ray.org_z[k] = (float)ray.start[2];
                    p.ray.dir_x[k] = (float)(ray.stop[0] - ray.start[0]);
                    p.ray.dir_y[k] = (float)(ray.stop[1] - ray.start[1]);
                    p.ray.dir_z[k] = (float)(ray.stop[2] - ray.start[2]);
                    p.ray.tnear[k] = 0.0f;
                    p.ray.tfar[k] = 1.0f;
                    p.ray.mask[k] = 0xFFFFFFFF;
                    p.ray.flags[k] = 0;
                    p.ray.time[k] = 0.0f;
                    p.hit.geomID[k] = RTC_INVALID_GEOMETRY_ID;
                    p.hit.primID[k] = RTC_INVALID_GEOMETRY_ID;
                }

                rtcIntersect8(valid, s_scene, &p, NULL);

                for (size_t k = 0; k < n; k++)
                {
                    answers[i + k] = p.hit.geomID[k] == RTC_INVALID_GEOMETRY_ID
                        ? CONTENTS_EMPTY
                        : (s_trisky[p.hit.primID[k]] ? CONTENTS_SKY : CONTENTS_SOLID);
                }
            }
            traced += (long long)count;
            passes++;
            elapsed = Now() - started;
        }

        return elapsed > 0 ? traced / elapsed : 0;
    }
#endif // SDHLT_EMBREE
}

void            RayBenchInit()
{
    s_rays.resize(RAYBENCH_MAX);
    s_offered.store(0);
    s_stored.store(0);
}

void            RayBenchCapture(const vec3_t start, const vec3_t stop)
{
    // Every thread lands here for every sky ray it casts, so the common path
    // has to be cheap: a thread local increment, and one atomic only for the
    // rays that are actually kept.
    static thread_local long long t_seen = 0;
    const long long seen = t_seen++;
    s_offered.fetch_add(1, std::memory_order_relaxed);

    if ((seen / RAYBENCH_RUN) % RAYBENCH_STRIDE != 0)
    {
        return;
    }

    const size_t slot = s_stored.fetch_add(1, std::memory_order_relaxed);
    if (slot >= RAYBENCH_MAX)
    {
        return;
    }

    VectorCopy(start, s_rays[slot].start);
    VectorCopy(stop, s_rays[slot].stop);
}


namespace
{
    // Runs one tracer over the whole sample often enough to be timed, and
    // records what it answered for every ray so two tracers can be compared
    // ray for ray afterwards.
    //
    // `answers` doubles as the anti-optimiser guard: the results are stored, so
    // nothing here can be folded away.
    template <typename Tracer>
    double MeasureTracer(size_t count, Tracer trace, std::vector<int>& answers)
    {
        answers.resize(count);

        // One untimed pass: the first sweep pays for page faults on the ray
        // buffer and for pulling the acceleration structure into cache, neither
        // of which is ray casting.
        for (size_t i = 0; i < count; i++)
        {
            answers[i] = trace(s_rays[i].start, s_rays[i].stop);
        }

        // At least 2 seconds of work, so a scheduling hiccup cannot pass for a
        // result. Small maps just get more passes over the same rays.
        const double    started = Now();
        long long       traced = 0;
        double          elapsed = 0;
        int             passes = 0;

        while (elapsed < 2.0 && passes < 10000)
        {
            for (size_t i = 0; i < count; i++)
            {
                answers[i] = trace(s_rays[i].start, s_rays[i].stop);
            }
            traced += (long long)count;
            passes++;
            elapsed = Now() - started;
        }

        return elapsed > 0 ? traced / elapsed : 0;
    }

    void ReportMix(const char* who, const std::vector<int>& answers)
    {
        size_t sky = 0, solid = 0, empty = 0;
        for (size_t i = 0; i < answers.size(); i++)
        {
            if (answers[i] == CONTENTS_SKY)      sky++;
            else if (answers[i] == CONTENTS_EMPTY) empty++;
            else                                 solid++;
        }
        const double n = (double)answers.size();
        Log("-raybench: %-8s sky %.1f%%, blocked %.1f%%, nothing hit %.1f%%\n",
            who, 100.0 * sky / n, 100.0 * solid / n, 100.0 * empty / n);
    }
}

void            RayBenchRun()
{
    const size_t    count = qmin(s_stored.load(), RAYBENCH_MAX);
    const long long offered = s_offered.load();

    Log("\n");
    Log("-raybench: %lld sky rays cast, %zu sampled (runs of %lld, one in %lld)\n",
        offered, count, RAYBENCH_RUN, RAYBENCH_STRIDE);

    if (count == 0)
    {
        Log("-raybench: nothing to measure. The map has no sky, or no sample "
            "could reach it.\n");
        return;
    }

    // Single threaded on purpose. Threading is already measured elsewhere, and
    // a per-core rate is what a replacement tracer has to be compared against.
    std::vector<int>    base;
    const double        baserate = MeasureTracer(count, [](const vec3_t a, const vec3_t b)
    {
        vec3_t skyhit;
        VectorCopy(b, skyhit);
        return TestLine(a, b, skyhit);
    }, base);

    Log("-raybench: %-8s %.2f Mrays/s single threaded (%.0f ns/ray)\n",
        "TestLine", baserate / 1e6, baserate > 0 ? 1e9 / baserate : 0);
    ReportMix("TestLine", base);

#ifdef SDHLT_EMBREE
    if (!EmbreeBuild())
    {
        Log("-raybench: embree scene could not be built; skipping\n");
        return;
    }

    std::vector<int>    emb;
    const double        embrate = MeasureTracer(count, [](const vec3_t a, const vec3_t b)
    {
        return EmbreeTestLine(a, b);
    }, emb);

    Log("-raybench: %-8s %.2f Mrays/s single threaded (%.0f ns/ray)\n",
        "embree", embrate / 1e6, embrate > 0 ? 1e9 / embrate : 0);
    ReportMix("embree", emb);

    std::vector<int>    occ;
    const double        occrate = MeasureTracer(count, [](const vec3_t a, const vec3_t b)
    {
        return EmbreeOccluded(a, b);
    }, occ);
    Log("-raybench: %-8s %.2f Mrays/s (occlusion only, cannot report sky)\n",
        "embree1o", occrate / 1e6);

    std::vector<int>    pkt;
    const double        pktrate = MeasureEmbreePackets(count, pkt);
    Log("-raybench: %-8s %.2f Mrays/s (8-wide packets, %.2fx over single ray)\n",
        "embree8", pktrate / 1e6, embrate > 0 ? pktrate / embrate : 0);

    size_t agree = 0;
    size_t sky_disagree = 0;
    for (size_t i = 0; i < count; i++)
    {
        const bool basesky = base[i] == CONTENTS_SKY;
        const bool embsky = emb[i] == CONTENTS_SKY;
        if (basesky == embsky)
        {
            agree++;
        }
        else
        {
            sky_disagree++;
        }
    }

    Log("-raybench: sky/no-sky agreement %.2f%% (%zu of %zu rays differ)\n",
        100.0 * agree / (double)count, sky_disagree, count);
    Log("-raybench: speedup %.2fx single ray, %.2fx 8-wide\n",
        baserate > 0 ? embrate / baserate : 0,
        baserate > 0 ? pktrate / baserate : 0);

    rtcReleaseScene(s_scene);
    rtcReleaseDevice(s_device);
    s_scene = NULL;
    s_device = NULL;
#endif
    Log("\n");
}

//
// Work balance
//

bool            g_workbench = false;

namespace
{
    struct unitcost_t
    {
        unitcost_t() : seconds(0.0) {}
        double          seconds;
    };

    std::vector<unitcost_t> s_units;
    std::vector<double>     s_unitstart;

    // Simulates handing these units, in this order, to `threads` workers that
    // each take the next one the moment they are free. That is exactly what
    // GetThreadWork() does, so the result is the makespan RAD would get.
    double SimulateMakespan(const std::vector<double>& costs, int threads)
    {
        std::vector<double> busy(threads, 0.0);
        for (size_t i = 0; i < costs.size(); i++)
        {
            // The worker that frees up first takes the next unit.
            size_t next = 0;
            for (int t = 1; t < threads; t++)
            {
                if (busy[t] < busy[next])
                {
                    next = t;
                }
            }
            busy[next] += costs[i];
        }

        double makespan = 0;
        for (int t = 0; t < threads; t++)
        {
            makespan = qmax(makespan, busy[t]);
        }
        return makespan;
    }
}

namespace
{
    std::atomic<long long>  s_scanned(0);
    std::atomic<long long>  s_used(0);
}

void            WorkBenchInit(int units)
{
    s_units.assign(units, unitcost_t());
    s_unitstart.assign(units, 0.0);
    s_scanned.store(0);
    s_used.store(0);
}

void            WorkBenchTally(long long scanned, long long used)
{
    s_scanned.fetch_add(scanned, std::memory_order_relaxed);
    s_used.fetch_add(used, std::memory_order_relaxed);
}

void            WorkBenchBegin(int unit)
{
    if ((size_t)unit < s_unitstart.size())
    {
        s_unitstart[unit] = I_FloatTime();
    }
}

void            WorkBenchEnd(int unit)
{
    if ((size_t)unit < s_units.size())
    {
        s_units[unit].seconds = I_FloatTime() - s_unitstart[unit];
    }
}

void            WorkBenchReport(const char* phase)
{
    if (s_units.empty())
    {
        return;
    }

    std::vector<double> costs;
    double total = 0;
    double worst = 0;
    for (size_t i = 0; i < s_units.size(); i++)
    {
        costs.push_back(s_units[i].seconds);
        total += s_units[i].seconds;
        worst = qmax(worst, s_units[i].seconds);
    }

    // Descending cost: the classic longest-processing-time-first schedule. It
    // is the cheap fix if the numbers say the tail is the problem, because the
    // big units go out while there is still small work left to fill the gaps.
    std::vector<double> sorted(costs);
    std::sort(sorted.begin(), sorted.end(), std::greater<double>());

    Log("\n-workbench: %s, %zu units, %.3fs of work, worst unit %.3fs\n",
        phase, costs.size(), total, worst);

    const long long scanned = s_scanned.load();
    if (scanned > 0)
    {
        const long long used = s_used.load();
        Log("-workbench: %lld items scanned, %lld used (%.2f%% - the rest is the "
            "loop rejecting them)\n",
            scanned, used, 100.0 * used / (double)scanned);
    }

    for (int t = 2; t <= 12; t += (t < 6 ? 2 : 6))
    {
        const double natural = SimulateMakespan(costs, t);
        const double lpt = SimulateMakespan(sorted, t);
        const double ideal = total / t;

        Log("-workbench: %2d threads: natural %.3fs (%.2fx), sorted %.3fs (%.2fx), "
            "perfect %.3fs (%.2fx)\n",
            t, natural, total / natural, lpt, total / lpt, ideal, (double)t);
    }

    s_units.clear();
    s_unitstart.clear();
    Log("\n");
}

#ifndef RAYBENCH_H__
#define RAYBENCH_H__

#if _MSC_VER >= 1000
#pragma once
#endif

//
// Isolated measurement of the ray casting itself.
//
// RAD is >95% of a compile, GatherSampleLight is ~94% of that, and the sky dome
// loop casts ~96% of all rays. Any plan to make RAD faster (a packet tracer, an
// external BVH like Embree, a GPU kernel) lives or dies on how many rays per
// second that one loop can do, and a whole-compile timing cannot tell you: it
// mixes in the light list walk, the opaque list, the transfers and the disk.
//
// So: capture a representative sample of the real sky rays as they are cast,
// then re-trace exactly those rays with everything else switched off and report
// rays/second. That number is the ceiling any replacement has to beat, and the
// checksum is what proves a replacement still answers the same thing.
//
// The rays are the real ones, on real geometry, in the real BSP: this is not a
// synthetic scene, which is the whole reason it lives inside RAD instead of in
// a standalone tool.
//
// Enable with:  sdHLRAD -raybench <map>
//

#include "cmdlib.h"
#include "mathtypes.h"

// -raybench. Off by default; the capture hook costs one predictable branch,
// which is nothing next to the TestLine call it sits in front of.
extern bool     g_raybench;

// Reserves the sample buffer. Called once when the flag is parsed.
extern void     RayBenchInit();

// Offers one sky ray to the sample. Thread safe, and deliberately cheap: most
// calls do an atomic increment and return.
extern void     RayBenchCapture(const vec3_t start, const vec3_t stop);

// Re-traces the captured rays single-threaded and reports the rate. Call after
// BuildFacelights, while the tnodes are still alive.
extern void     RayBenchRun();

//
// Work balance measurement, a separate question from the tracing rate.
//
// RAD hands out one work unit per face and the units are wildly uneven: a big
// wall is worth hundreds of small trims. With a dynamic dispatcher that is
// still fine *until* the end of a phase, where a face that takes a second can
// leave five cores idle waiting for one. Measured on a 6 core machine the
// phases scale ~4x, not 6x, and this is the suspect.
//
// So: time every unit, then work out what the same units would have cost with
// perfect scheduling. The gap is what a smarter dispatch order could win, and
// it is worth knowing before writing one.
//
extern bool     g_workbench;

// Sizes the table before the workers start. The per-unit calls below then only
// ever touch their own slot, which is what keeps them thread safe without a
// lock in the middle of a phase being measured.
extern void     WorkBenchInit(int units);

// Counts work looked at versus work actually done, for loops that scan a set
// and keep a subset. Reported by WorkBenchReport along with the timings.
extern void     WorkBenchTally(long long scanned, long long used);
extern void     WorkBenchBegin(int unit);
extern void     WorkBenchEnd(int unit);
extern void     WorkBenchReport(const char* phase);

class WorkBenchScope
{
public:
    WorkBenchScope(int unit)
        : m_unit(unit), m_on(g_workbench)
    {
        if (m_on)
        {
            WorkBenchBegin(m_unit);
        }
    }

    ~WorkBenchScope()
    {
        if (m_on)
        {
            WorkBenchEnd(m_unit);
        }
    }

private:
    int             m_unit;
    bool            m_on;

    WorkBenchScope(const WorkBenchScope&);
    WorkBenchScope& operator=(const WorkBenchScope&);
};

#endif // RAYBENCH_H__

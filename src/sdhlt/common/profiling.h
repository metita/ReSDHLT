#ifndef PROFILING_H__
#define PROFILING_H__

#if _MSC_VER >= 1000
#pragma once
#endif

//
// Lightweight in-tool profiler.
//
// Exists because external profiling of these tools turned out to be
// impractical: perf is unavailable in some environments, is Linux-only anyway,
// and gprof produced attribution that did not survive checking (it reported
// MakeTnode() as ~50% of RAD's runtime with 93 million calls, where a counter
// showed 78). This gives numbers that can be verified against each other
// instead, on any platform, with no external tooling.
//
// Design constraints, driven by the shape of the hot code:
//
//   * The innermost ray-casting functions are called tens of millions of times,
//     so per-call work must be nearly free. Counters are plain thread_local
//     integers: no atomics, no locks, no contention. Each thread registers its
//     own block once, and the blocks are summed when reporting.
//   * Reading a cycle counter costs tens of cycles, which would dominate the
//     very functions we care about. So there are two levels: PROF_CALL for
//     counting only (safe anywhere), and PROF_SCOPE for timing (use on
//     functions entered thousands, not millions, of times).
//   * When profiling is off the cost is one predictable branch on a global
//     bool, so the instrumentation can stay in release builds.
//

#include "cmdlib.h"

#ifdef _MSC_VER
#include <intrin.h>
#endif

// Profiled sites. Keep in sync with g_prof_names in profiling.cpp.
enum
{
    PROF_BUILDFACELIGHTS = 0,
    PROF_GATHERSAMPLELIGHT,
    PROF_FINALLIGHTFACE,
    PROF_CREATETRIANGULATIONS,
    PROF_ADDPATCHLIGHTS,
    PROF_TESTLINE,
    PROF_TESTLINE_R,
    PROF_TESTSEGMENTOPAQUE,
    PROF_CHECKVISBIT,
    PROF_GETPHONGNORMAL,

    PROF_SLOTS
};

struct prof_block_t;
extern void     ProfFlushBlock(prof_block_t* block);

struct prof_block_t
{
    long long       calls[PROF_SLOTS];
    long long       ticks[PROF_SLOTS];
    bool            registered;

    // RunThreadsOn() spawns fresh threads for each phase and joins them, so a
    // block dies with its thread. Flush on the way out or the phase's numbers
    // are lost.
    ~prof_block_t() { ProfFlushBlock(this); }
};

extern bool                 g_profile;
extern thread_local prof_block_t g_prof;

extern void     ProfRegisterBlock(prof_block_t* block);
extern void     ProfReset();
extern void     ProfReport(double wallseconds);

// Monotonic tick source. rdtsc is not a wall clock and is not comparable
// across machines, but it is monotonic per core and cheap, which is all that
// relative attribution needs.
static inline unsigned long long ProfTicks()
{
#if defined(_MSC_VER)
    return __rdtsc();
#elif defined(__i386__) || defined(__x86_64__)
    return __builtin_ia32_rdtsc();
#else
    return 0;                                              // counting still works
#endif
}

static inline void ProfTouch()
{
    if (!g_prof.registered)
    {
        g_prof.registered = true;
        ProfRegisterBlock(&g_prof);
    }
}

// Count an entry. Cheap enough for the innermost functions.
static inline void ProfCall(int slot)
{
    if (!g_profile)
    {
        return;
    }
    ProfTouch();
    g_prof.calls[slot]++;
}

// Count and time an entry. Do not use on functions called millions of times:
// the tick reads would distort the measurement.
class ProfScope
{
public:
    ProfScope(int slot)
        : m_slot(slot), m_start(0)
    {
        if (g_profile)
        {
            ProfTouch();
            g_prof.calls[m_slot]++;
            m_start = ProfTicks();
        }
    }

    ~ProfScope()
    {
        if (g_profile)
        {
            g_prof.ticks[m_slot] += (long long)(ProfTicks() - m_start);
        }
    }

private:
    int                 m_slot;
    unsigned long long  m_start;

    ProfScope(const ProfScope&);
    ProfScope& operator=(const ProfScope&);
};

//
// PROF_SCOPE is runtime-gated: it only wraps phase-level functions entered a
// few hundred thousand times at most, where one branch on a global bool is
// free. It stays available in normal builds.
//
#define PROF_SCOPE(slot)  ProfScope prof_scope_##__LINE__(slot)

//
// PROF_CALL is compile-time gated, and deliberately so. It is placed in the
// innermost ray-casting functions, and TestLine_r alone is entered about 1.8
// billion times for a mid-size map. Even the disabled path (load a global bool,
// branch) would cost roughly a second of every compile for users who never
// profile, so it must vanish entirely unless asked for.
//
// Build a profiling binary with:  cmake -B build -S . -DSDHLT_PROFILE=ON
//
#ifdef SDHLT_PROFILE
#define PROF_CALL(slot)   ProfCall(slot)
#else
#define PROF_CALL(slot)   ((void)0)
#endif

#endif // PROFILING_H__

#include "cmdlib.h"
#include "log.h"
#include "profiling.h"

#include <atomic>

bool                        g_profile = false;

// thread_local objects have static initialisation, so the counters start zeroed
// on every thread without any per-thread setup cost.
thread_local prof_block_t   g_prof;

//
// Totals survive the threads that produced them.
//
// The first version of this kept a registry of pointers to each thread's
// thread_local block and summed them when reporting. That silently lost almost
// everything: RunThreadsOn() creates fresh threads for every phase and joins
// them, so each block was destroyed when its thread exited and the registry
// was left holding dangling pointers. The report showed only the final phase.
//
// Instead each block flushes itself into these atomics when its thread exits,
// so the hot path stays lock-free and contention-free while the totals
// accumulate safely.
//
static std::atomic<long long>   g_total_calls[PROF_SLOTS];
static std::atomic<long long>   g_total_ticks[PROF_SLOTS];
static std::atomic<int>         g_total_threads(0);

// Names must line up with the enum in profiling.h.
static const char* const g_prof_names[PROF_SLOTS] =
{
    "BuildFacelights",
    "GatherSampleLight",
    "FinalLightFace",
    "CreateTriangulations",
    "AddPatchLights",
    "TestLine",
    "TestLine_r",
    "TestSegmentAgainstOpaqueList",
    "CheckVisBit",
    "GetPhongNormal",
};

void            ProfFlushBlock(prof_block_t* block)
{
    if (!block->registered)
    {
        return;
    }

    for (int s = 0; s < PROF_SLOTS; s++)
    {
        if (block->calls[s])
        {
            g_total_calls[s].fetch_add(block->calls[s], std::memory_order_relaxed);
            block->calls[s] = 0;
        }
        if (block->ticks[s])
        {
            g_total_ticks[s].fetch_add(block->ticks[s], std::memory_order_relaxed);
            block->ticks[s] = 0;
        }
    }
    block->registered = false;
}

void            ProfRegisterBlock(prof_block_t*)
{
    g_total_threads.fetch_add(1, std::memory_order_relaxed);
}

void            ProfReset()
{
    for (int s = 0; s < PROF_SLOTS; s++)
    {
        g_total_calls[s].store(0, std::memory_order_relaxed);
        g_total_ticks[s].store(0, std::memory_order_relaxed);
    }
    g_total_threads.store(0, std::memory_order_relaxed);
}

void            ProfReport(double wallseconds)
{
    if (!g_profile)
    {
        return;
    }

    // The reporting thread's own block is still alive; fold it in.
    ProfFlushBlock(&g_prof);

    long long       calls[PROF_SLOTS];
    long long       ticks[PROF_SLOTS];
    long long       total = 0;

    for (int s = 0; s < PROF_SLOTS; s++)
    {
        calls[s] = g_total_calls[s].load(std::memory_order_relaxed);
        ticks[s] = g_total_ticks[s].load(std::memory_order_relaxed);
        total += ticks[s];
    }

    Log("\n---- profile ----\n");
    Log("worker threads seen: %d, wall time %.2fs\n\n",
        g_total_threads.load(std::memory_order_relaxed), wallseconds);
    Log("%-30s %16s %12s %8s %12s\n",
        "site", "calls", "Mticks", "share", "ticks/call");

    for (int s = 0; s < PROF_SLOTS; s++)
    {
        if (calls[s] == 0)
        {
            continue;
        }

        if (ticks[s] > 0)
        {
            Log("%-30s %16lld %12.1f %7.1f%% %12.1f\n",
                g_prof_names[s], calls[s], ticks[s] / 1e6,
                total ? (100.0 * ticks[s] / total) : 0.0,
                (double)ticks[s] / (double)calls[s]);
        }
        else
        {
            // Counted but not timed: reading a cycle counter around these
            // would cost more than the work being measured.
            Log("%-30s %16lld %12s %8s %12s\n",
                g_prof_names[s], calls[s], "(count only)", "-", "-");
        }
    }

    Log("\nNotes:\n");
    Log("  Timed sites are top-level phases; 'share' is relative to their sum.\n");
    Log("  Count-only sites are the inner ray-casting functions, called too\n");
    Log("  often to time without distorting the result. Divide a phase's ticks\n");
    Log("  by the counts underneath it to see where its work goes.\n");
    Log("  Ticks are rdtsc units: comparable within a run, not across machines.\n");
    Log("-----------------\n\n");
}

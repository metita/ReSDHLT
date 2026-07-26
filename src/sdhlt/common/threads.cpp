#ifdef SYSTEM_WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <malloc.h>
#endif

#ifdef HAVE_CONFIG_H
#include "config.h"
#endif

#include "cmdlib.h"
#include "messages.h"
#include "log.h"
#include "threads.h"
#include "blockmem.h"

#ifdef SYSTEM_POSIX
#ifdef HAVE_SYS_TIME_H
#include <sys/time.h>
#endif
#ifdef HAVE_SYS_RESOURCE_H
#include <sys/resource.h>
#endif
#include <unistd.h>
#endif

#include "hlassert.h"

#include <atomic>
#include <mutex>
#include <thread>
#include <vector>

//
// One portable worker pool built on <thread>, replacing the two near-identical
// Win32 (CreateThread/CRITICAL_SECTION) and POSIX (pthread) implementations
// this file used to carry. Only thread priority is still platform specific,
// because there is no standard equivalent.
//

q_threadpriority g_threadpriority = DEFAULT_THREAD_PRIORITY;
int             g_numthreads = DEFAULT_NUMTHREADS;

#define THREADTIMES_SIZE 100
#define THREADTIMES_SIZEf (float)(THREADTIMES_SIZE)

static std::atomic<int> dispatch(0);
static int      workcount = 0;
static int      oldf = 0;
static bool     pacifier = false;
static bool     threaded = false;
static double   threadstart = 0;
static double   threadtimes[THREADTIMES_SIZE];

static std::mutex       g_threadmutex;
static int              g_lockdepth = 0;

// Returns the number of logical processors available to this process, or 0 if
// it cannot be determined.
static int      GetLogicalProcessorCount()
{
    unsigned int    hc = std::thread::hardware_concurrency();

    if (hc == 0)
    {
        return 0;
    }
    if (hc > (unsigned int)MAX_THREADS)
    {
        return MAX_THREADS;
    }
    return (int)hc;
}

// Keeps g_numthreads sane. No tool bounds-checks its "-threads" argument, and
// an absurd value would otherwise mean an absurd number of OS threads.
static void     ClampNumThreads()
{
    if (g_numthreads < 1)
    {
        Warning("Invalid thread count %d, using 1 thread\n", g_numthreads);
        g_numthreads = 1;
    }
    else if (g_numthreads > MAX_THREADS)
    {
        Warning("Thread count %d exceeds the maximum of %d, using %d threads\n",
                g_numthreads, MAX_THREADS, MAX_THREADS);
        g_numthreads = MAX_THREADS;
    }
}

void            ThreadSetDefault()
{
    if (g_numthreads == -1)                                // not set manually
    {
        g_numthreads = GetLogicalProcessorCount();

        if (g_numthreads < 1)                              // detection failed
        {
#ifdef SYSTEM_WIN32
            SYSTEM_INFO     info;

            GetSystemInfo(&info);
            g_numthreads = (int)info.dwNumberOfProcessors;
#elif defined(_SC_NPROCESSORS_ONLN)
            long            n = sysconf(_SC_NPROCESSORS_ONLN);

            g_numthreads = (n > 0) ? (int)n : 1;
#else
            g_numthreads = 1;
#endif
            if (g_numthreads < 1)
            {
                g_numthreads = 1;
            }
            else if (g_numthreads > MAX_THREADS)
            {
                g_numthreads = MAX_THREADS;
            }
        }
    }
}

void            ThreadSetPriority(q_threadpriority type)
{
    g_threadpriority = type;

    // std::thread exposes no portable priority control, so this stays
    // platform specific.
#ifdef SYSTEM_WIN32
    int             val;

    switch (g_threadpriority)
    {
    case eThreadPriorityLow:
        val = IDLE_PRIORITY_CLASS;
        break;

    case eThreadPriorityHigh:
        val = HIGH_PRIORITY_CLASS;
        break;

    case eThreadPriorityNormal:
    default:
        val = NORMAL_PRIORITY_CLASS;
        break;
    }

    SetPriorityClass(GetCurrentProcess(), val);
#endif

#ifdef SYSTEM_POSIX
    int             val;

    // Unprivileged processes cannot raise their priority, so -high is
    // effectively a no-op unless running as root.
    switch (g_threadpriority)
    {
    case eThreadPriorityLow:
        val = PRIO_MAX;
        break;

    case eThreadPriorityHigh:
        val = PRIO_MIN;
        break;

    case eThreadPriorityNormal:
    default:
        val = 0;
        break;
    }

    setpriority(PRIO_PROCESS, 0, val);
#endif
}

void            threads_InitCrit()
{
    threaded = true;
}

void            threads_UninitCrit()
{
    threaded = false;
}

void            ThreadLock()
{
    if (!threaded)
    {
        return;
    }
    g_threadmutex.lock();
    if (g_lockdepth)
    {
        Warning("Recursive ThreadLock\n");
    }
    g_lockdepth++;
}

void            ThreadUnlock()
{
    if (!threaded)
    {
        return;
    }
    if (!g_lockdepth)
    {
        Error("ThreadUnlock without lock\n");
    }
    g_lockdepth--;
    g_threadmutex.unlock();
}

int             GetThreadWork()
{
    int             r, f, i;
    double          ct, finish, finish2, finish3;
	static const char *s1 = NULL; // avoid frequent call of Localize() in PrintConsole
	static const char *s2 = NULL;

    //
    // Claim a work unit without holding the global lock. Previously every
    // single work unit funnelled all workers through ThreadLock() merely to
    // execute "dispatch++", so with many threads and millions of units (RAD
    // and VIS in particular) the dispatcher itself became the bottleneck and
    // the tools stopped scaling with core count.
    //
    r = dispatch.fetch_add(1, std::memory_order_relaxed);

    if (r >= workcount)
    {
        // No work left. Give the slot back so dispatch does not drift far past
        // workcount while the remaining workers drain.
        dispatch.fetch_sub(1, std::memory_order_relaxed);

        if (r == workcount)
        {
            Developer(DEVELOPER_LEVEL_MESSAGE, "dispatch == workcount, work is complete\n");
        }
        return -1;
    }

    if (r < 0)
    {
        Developer(DEVELOPER_LEVEL_ERROR, "negative dispatch!!!\n");
        return -1;
    }

    if (!pacifier)
    {
        return r;
    }

    //
    // Progress reporting still needs the lock: it touches the shared
    // threadtimes/oldf state and writes to the console.
    //
    ThreadLock();

	if (s1 == NULL)
		s1 = Localize ("  (%d%%: est. time to completion %ld/%ld/%ld secs)   ");
	if (s2 == NULL)
		s2 = Localize ("  (%d%%: est. time to completion <1 sec)   ");

    if (r == 0)
    {
        oldf = 0;
    }

	PrintConsole
		("\r%6d /%6d", r + 1, workcount);

    f = THREADTIMES_SIZE * r / workcount;

    if (f != oldf)
    {
        ct = I_FloatTime();
        /* Fill in current time for threadtimes record */
        for (i = oldf; i <= f; i++)
        {
            if (threadtimes[i] < 1)
            {
                threadtimes[i] = ct;
            }
        }
        oldf = f;

        if (f > 10)
        {
            finish = (ct - threadtimes[0]) * (THREADTIMES_SIZEf - f) / f;
            finish2 = 10.0 * (ct - threadtimes[f - 10]) * (THREADTIMES_SIZEf - f) / THREADTIMES_SIZEf;
            finish3 = THREADTIMES_SIZEf * (ct - threadtimes[f - 1]) * (THREADTIMES_SIZEf - f) / THREADTIMES_SIZEf;

            if (finish > 1.0)
            {
				PrintConsole
					(s1, f, (long)(finish), (long)(finish2),
                       (long)(finish3));
            }
            else
            {
				PrintConsole
					(s2, f);
            }
        }
    }

    ThreadUnlock();

    return r;
}

q_threadfunction workfunction;

static void     ThreadWorkerFunction(int)
{
    int             work;

    while ((work = GetThreadWork()) != -1)
    {
        workfunction(work);
    }
}

void            RunThreadsOnIndividual(int workcnt, bool showpacifier, q_threadfunction func)
{
    workfunction = func;
    RunThreadsOn(workcnt, showpacifier, ThreadWorkerFunction);
}

#ifndef SINGLE_THREADED

void            RunThreadsOn(int workcnt, bool showpacifier, q_threadfunction func)
{
    double          start, end;
    int             i;

    ClampNumThreads();

    threadstart = I_FloatTime();
    start = threadstart;
    for (i = 0; i < THREADTIMES_SIZE; i++)
    {
        threadtimes[i] = 0;
    }

    dispatch = 0;
    workcount = workcnt;
    oldf = -1;
    pacifier = showpacifier;

    if (workcount < dispatch.load())
    {
        Developer(DEVELOPER_LEVEL_ERROR, "RunThreadsOn: Workcount(%i) < dispatch(%i)\n",
                  workcount, dispatch.load());
    }
    hlassume(workcount >= dispatch.load(), assume_BadWorkcount);

    if (pacifier)
    {
        setbuf(stdout, NULL);
    }

    threads_InitCrit();

    //
    // A vector rather than a fixed MAX_THREADS array: the old code sized two
    // stack arrays by MAX_THREADS and indexed them with an unvalidated thread
    // count.
    //
    std::vector<std::thread> workers;
    workers.reserve((size_t)g_numthreads);

    for (i = 0; i < g_numthreads; i++)
    {
        try
        {
            workers.emplace_back(func, i);
        }
        catch (const std::system_error& e)
        {
            // Run with however many threads did start rather than aborting a
            // long compile outright.
            Warning("Could not create thread #%d (%s), continuing with %d\n",
                    i, e.what(), (int)workers.size());
            break;
        }
    }

    if (workers.empty())
    {
        // Nothing could be spawned; do the work on this thread so the compile
        // still completes.
        Warning("No worker threads could be created, running single-threaded\n");
        threads_UninitCrit();
        func(0);
    }
    else
    {
        for (std::thread& t : workers)
        {
            t.join();
        }
        threads_UninitCrit();
    }

    end = I_FloatTime();
    if (pacifier)
    {
        PrintConsole("\r%60s\r", "");
    }
    Log(" (%.2f seconds)\n", end - start);
}

#else /*SINGLE_THREADED*/

void            RunThreadsOn(int workcnt, bool showpacifier, q_threadfunction func)
{
    double          start, end;
    int             i;

    g_numthreads = 1;

    threadstart = I_FloatTime();
    start = threadstart;
    for (i = 0; i < THREADTIMES_SIZE; i++)
    {
        threadtimes[i] = 0;
    }

    dispatch = 0;
    workcount = workcnt;
    oldf = -1;
    pacifier = showpacifier;
    threaded = false;

    func(0);

    end = I_FloatTime();
    if (pacifier)
    {
        PrintConsole("\r%60s\r", "");
    }
    Log(" (%.2f seconds)\n", end - start);
}

#endif /*SINGLE_THREADED*/

#ifndef THREADS_H__
#define THREADS_H__
#include "cmdlib.h" //--vluzacn

#if _MSC_VER >= 1000
#pragma once
#endif

// Upper bound on worker threads. Modern desktop CPUs can exceed the historic
// 64-thread assumption, so this is deliberately generous; the actual thread
// count is autodetected at runtime (see ThreadSetDefault).
#define	MAX_THREADS	256

typedef enum
{
    eThreadPriorityLow = -1,
    eThreadPriorityNormal,
    eThreadPriorityHigh
}
q_threadpriority;

typedef void    (*q_threadfunction) (int);

// -1 means "not set manually": ThreadSetDefault() will autodetect the number of
// logical processors. Previously POSIX defaulted to 1, which made the
// autodetection branch in ThreadSetDefault() dead code and forced every Linux
// build to compile single-threaded unless -threads was passed explicitly.
#define DEFAULT_NUMTHREADS -1

#define DEFAULT_THREAD_PRIORITY eThreadPriorityNormal

extern int      g_numthreads;
extern q_threadpriority g_threadpriority;

extern void     ThreadSetPriority(q_threadpriority type);
extern void     ThreadSetDefault();
extern int      GetThreadWork();
extern void     ThreadLock();
extern void     ThreadUnlock();

extern void     RunThreadsOnIndividual(int workcnt, bool showpacifier, q_threadfunction);
extern void     RunThreadsOn(int workcnt, bool showpacifier, q_threadfunction);

#ifdef ZHLT_NETVIS
extern void     threads_InitCrit();
extern void     threads_UninitCrit();
#endif

#define NamedRunThreadsOn(n,p,f) { Log("%s\n", Localize(#f ":")); RunThreadsOn(n,p,f); }
#define NamedRunThreadsOnIndividual(n,p,f) { Log("%s\n", Localize(#f ":")); RunThreadsOnIndividual(n,p,f); }

#endif //**/ THREADS_H__

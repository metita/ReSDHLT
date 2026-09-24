#include "vis.h"
#include <algorithm>
#include <climits>
#include <condition_variable>
#include <mutex>
#include <set>

// =====================================================================================
//  Flow scheduling
//
//  Portals are flowed in rank order and a portal prunes with the final visbits of
//  every lower ranked portal, so it may have to wait for one another thread is still
//  flowing. Measured on a 6334 portal map that left half of all thread time idle.
//
//  A waiting thread therefore helps: the thread flowing a portal publishes subtrees of
//  its recursion as tasks while someone is hungry, and waiters run them. Splitting a
//  flow like this cannot change its result: a subtree only ever marks leafs in its own
//  mightsee, and it is only skipped when all of those are already marked, so the order
//  the subtrees run in does not matter.
//
//  Deadlock freedom: while waiting for portal P a thread only runs tasks of portals
//  ranked at most P. Those never wait on anything the waiting thread holds.
// =====================================================================================
typedef struct flowtask_s
{
    int             rank;                                  // rank of the portal being flowed
    int             leafnum;
    threaddata_t*   thread;
    pstack_t        frame;                                 // private copy of the parent frame
} flowtask_t;

// Nested help grows the stack of the helping thread; stop helping past this depth.
#define MAX_FLOW_NESTING 3

static std::mutex g_flowmutex;                             // guards everything below
static std::condition_variable g_flowcv;                   // task queued, portal done, flow joined
static std::vector<flowtask_t*> g_flowtasks;               // heap, lowest rank on top
static std::multiset<int> g_hungry;                        // rank limits of idle threads
static int      g_portalsleft;
static std::atomic<int> g_hungrylimit(-1);                 // highest rank an idle thread accepts
static std::atomic<int> g_numhungry(0);
static std::atomic<int> g_numflowtasks(0);
static thread_local int t_nesting;

static void     RunFlowTask(flowtask_t* t);

static bool     FlowTaskAfter(const flowtask_t* a, const flowtask_t* b)
{
    return a->rank > b->rank;
}

static void     UpdateHungry()
{
    g_numhungry.store((int)g_hungry.size(), std::memory_order_relaxed);
    g_hungrylimit.store(g_hungry.empty() ? -1 : *g_hungry.rbegin(), std::memory_order_relaxed);
}

static flowtask_t* PopFlowTask()
{
    std::pop_heap(g_flowtasks.begin(), g_flowtasks.end(), FlowTaskAfter);
    flowtask_t*     t = g_flowtasks.back();
    g_flowtasks.pop_back();
    g_numflowtasks.fetch_sub(1, std::memory_order_relaxed);
    return t;
}

// Called with g_flowmutex held. Runs queued tasks ranked at most `limit` until `done`.
template <typename Done>
static void     HelpUntil(std::unique_lock<std::mutex>& lock, const int limit, Done done)
{
    while (!done())
    {
        if (!g_flowtasks.empty() && g_flowtasks.front()->rank <= limit && t_nesting < MAX_FLOW_NESTING)
        {
            flowtask_t* t = PopFlowTask();
            lock.unlock();
            RunFlowTask(t);
            lock.lock();
            continue;
        }
        std::multiset<int>::iterator it = g_hungry.insert(limit);
        UpdateHungry();
        g_flowcv.wait(lock);
        g_hungry.erase(it);
        UpdateHungry();
    }
}

// =====================================================================================
//  TrySpawnFlow
//      Queues the recursion into `leafnum` instead of running it, if an idle thread
//      would pick it up. `stack` is the frame the recursion reads as prevstack.
// =====================================================================================
static bool     TrySpawnFlow(const int leafnum, threaddata_t* const thread, const pstack_t* const stack)
{
    const int       rank = thread->base->rank;

    if (g_hungrylimit.load(std::memory_order_relaxed) < rank
        || g_numflowtasks.load(std::memory_order_relaxed) >= g_numhungry.load(std::memory_order_relaxed))
    {
        return false;
    }
    // The copy uses fixed windings; an unusually large original portal stays inline.
    if (stack->source->numpoints > MAX_POINTS_ON_FIXED_WINDING
        || (stack->pass && stack->pass->numpoints > MAX_POINTS_ON_FIXED_WINDING))
    {
        return false;
    }

    flowtask_t*     t = (flowtask_t*)malloc(sizeof(flowtask_t));
    hlassume(t != NULL, assume_NoMemory);
    t->rank = rank;
    t->leafnum = leafnum;
    t->thread = thread;
    // Only the fields the recursion reads from its prevstack. source and pass may
    // point into frames that are gone by the time the task runs, so copy them.
    t->frame.head = stack->head;
    t->frame.portalplane = stack->portalplane;
    memcpy(t->frame.mightsee, stack->mightsee, g_bitbytes);
    t->frame.windings[0].numpoints = stack->source->numpoints;
    memcpy(t->frame.windings[0].points, stack->source->points, stack->source->numpoints * sizeof(vec3_t));
    t->frame.source = &t->frame.windings[0];
    t->frame.pass = NULL;
    if (stack->pass)
    {
        t->frame.windings[1].numpoints = stack->pass->numpoints;
        memcpy(t->frame.windings[1].points, stack->pass->points, stack->pass->numpoints * sizeof(vec3_t));
        t->frame.pass = &t->frame.windings[1];
    }

    {
        std::lock_guard<std::mutex> lock(g_flowmutex);
        thread->pending++;
        g_flowtasks.push_back(t);
        std::push_heap(g_flowtasks.begin(), g_flowtasks.end(), FlowTaskAfter);
        g_numflowtasks.fetch_add(1, std::memory_order_relaxed);
    }
    g_flowcv.notify_all();
    return true;
}

// =====================================================================================
//  WaitForPortal
//      Blocks until a lower ranked portal has final visbits, helping with it meanwhile.
// =====================================================================================
static void     WaitForPortal(const portal_t* const p)
{
    const std::atomic<bool>& done = g_portaldone[p - g_portals];

    if (done.load(std::memory_order_acquire))
    {
        return;
    }
    std::unique_lock<std::mutex> lock(g_flowmutex);
    HelpUntil(lock, p->rank, [&done] { return done.load(std::memory_order_acquire); });
}

// =====================================================================================
//  MarkPortalDone
// =====================================================================================
static void     MarkPortalDone(const portal_t* const p)
{
    {
        // Under the mutex so a waiter cannot test the flag, miss this store and then
        // sleep through the notification.
        std::lock_guard<std::mutex> lock(g_flowmutex);
        g_portaldone[p - g_portals].store(true, std::memory_order_release);
        g_portalsleft--;
    }
    g_flowcv.notify_all();
}

// =====================================================================================
//  InitPortalFlow / RunQueuedFlowTask / HelpUntilAllPortalsDone
//      Used by LeafThread.
// =====================================================================================
void            InitPortalFlow(const int numportals)
{
    g_portalsleft = numportals;
}

bool            RunQueuedFlowTask()
{
    std::unique_lock<std::mutex> lock(g_flowmutex);

    if (g_flowtasks.empty())
    {
        return false;
    }
    flowtask_t*     t = PopFlowTask();
    lock.unlock();
    RunFlowTask(t);
    return true;
}

void            HelpUntilAllPortalsDone()
{
    std::unique_lock<std::mutex> lock(g_flowmutex);
    HelpUntil(lock, INT_MAX, [] { return g_portalsleft == 0; });
}

// Leafs are marked by every thread working on the same portal.
static inline void AtomicOrByte(byte* const dst, const byte bits)
{
#ifdef _MSC_VER
    _InterlockedOr8((volatile char*)dst, (char)bits);
#else
    __atomic_fetch_or(dst, bits, __ATOMIC_RELAXED);
#endif
}

// =====================================================================================
//  CheckStack
// =====================================================================================
#ifdef USE_CHECK_STACK
static void     CheckStack(const leaf_t* const leaf, const threaddata_t* const thread)
{
    pstack_t*       p;

    for (p = thread->pstack_head.next; p; p = p->next)
    {
        if (p->leaf == leaf)
            Error("CheckStack: leaf recursion");
    }
}
#endif

// =====================================================================================
//  AllocStackWinding
// =====================================================================================
inline static winding_t* AllocStackWinding(pstack_t* const stack)
{
    int             i;

    for (i = 0; i < 3; i++)
    {
        if (stack->freewindings[i])
        {
            stack->freewindings[i] = 0;
            return &stack->windings[i];
        }
    }

    Error("AllocStackWinding: failed");

    return NULL;
}

// =====================================================================================
//  FreeStackWinding
// =====================================================================================
inline static void     FreeStackWinding(const winding_t* const w, pstack_t* const stack)
{
    int             i;

    i = w - stack->windings;

    if (i < 0 || i > 2)
        return;                                            // not from local

    if (stack->freewindings[i])
        Error("FreeStackWinding: allready free");
    stack->freewindings[i] = 1;
}

// =====================================================================================
//  ChopWinding
// =====================================================================================
inline winding_t*      ChopWinding(winding_t* const in, pstack_t* const stack, const plane_t* const split)
{
    vec_t           dists[128];
    int             sides[128];
    int             front, back;
    vec_t           dot;
    int             i;
    vec3_t          mid;
    winding_t*      neww;

    if (in->numpoints > (sizeof(sides) / sizeof(*sides)))
    {
        Error("Winding with too many sides!");
    }

    // Count the sides first and only classify each point once the winding
    // turns out to need splitting: three out of four calls keep it whole.
    // Counting with comparisons instead of counts[sides[i]]++ avoids a
    // mispredicted branch per point and a store/load chain on counts.
    front = back = 0;
    for (i = 0; i < in->numpoints; i++)
    {
        dot = DotProduct(in->points[i], split->normal);
        dot -= split->dist;
        dists[i] = dot;
        front += dot > ON_EPSILON;
        back += dot < -ON_EPSILON;
    }

    if (!back)
    {
        return in;                                         // completely on front side
    }

    if (!front)
    {
        FreeStackWinding(in, stack);
        return NULL;
    }

    for (i = 0; i < in->numpoints; i++)
    {
        sides[i] = dists[i] > ON_EPSILON ? SIDE_FRONT : dists[i] < -ON_EPSILON ? SIDE_BACK : SIDE_ON;
    }
    sides[i] = sides[0];
    dists[i] = dists[0];

    neww = AllocStackWinding(stack);

    neww->numpoints = 0;

    for (i = 0; i < in->numpoints; i++)
    {
        vec_t* p1 = in->points[i];

        if (neww->numpoints == MAX_POINTS_ON_FIXED_WINDING)
        {
            Warning("ChopWinding : rejected(1) due to too many points\n");
            FreeStackWinding(neww, stack);
            return in;                                     // can't chop -- fall back to original
        }

        if (sides[i] == SIDE_ON)
        {
            VectorCopy(p1, neww->points[neww->numpoints]);
            neww->numpoints++;
            continue;
        }
        else if (sides[i] == SIDE_FRONT)
        {
            VectorCopy(p1, neww->points[neww->numpoints]);
            neww->numpoints++;
        }

        if ((sides[i + 1] == SIDE_ON) | (sides[i + 1] == sides[i])) // | instead of || for branch optimization
        {
            continue;
        }

        if (neww->numpoints == MAX_POINTS_ON_FIXED_WINDING)
        {
            Warning("ChopWinding : rejected(2) due to too many points\n");
            FreeStackWinding(neww, stack);
            return in;                                     // can't chop -- fall back to original
        }

        // generate a split point
        {
            unsigned tmp = i + 1;
            if (tmp >= in->numpoints)
            {
                tmp = 0;
            }
            const vec_t* p2 = in->points[tmp];

            dot = dists[i] / (dists[i] - dists[i + 1]);

            const vec_t* normal = split->normal;
            const vec_t dist = split->dist;
            unsigned int j;
            for (j = 0; j < 3; j++)
            {                                                  // avoid round off error when possible
                if (normal[j] < (1.0 - NORMAL_EPSILON))
                {
                    if (normal[j] > (-1.0 + NORMAL_EPSILON))
                    {
                        mid[j] = p1[j] + dot * (p2[j] - p1[j]);
                    }
                    else
                    {
                        mid[j] = -dist;
                    }
                }
                else
                {
                    mid[j] = dist;
                }
            }
        }

        VectorCopy(mid, neww->points[neww->numpoints]);
        neww->numpoints++;
    }

    // free the original winding
    FreeStackWinding(in, stack);

    return neww;
}

// =====================================================================================
//  AddPlane
// =====================================================================================
#ifdef RVIS_LEVEL_2
inline static void AddPlane(pstack_t* const stack, const plane_t* const split)
{
    int     j;
    
    if (stack->clipPlaneCount)
    {
        for (j = 0; j < stack->clipPlaneCount; j++)
        {
            if (fabs((stack->clipPlane[j]).dist - split->dist) <= EQUAL_EPSILON &&
                VectorCompare((stack->clipPlane[j]).normal, split->normal))
            {
                return;
            }
        }
    }
    stack->clipPlane[stack->clipPlaneCount] = *split;
    stack->clipPlaneCount++;
}
#endif

// =====================================================================================
//  SeparatorVerdict
//      Decides a ClipToSeperators candidate without normalizing its plane.
//
//      The exact test normalizes the cross product n and classifies points by their
//      float distance d to the plane. Here each point is classified in double on the raw
//      n instead, as e = (point - pass[j]) . n, which is d * length up to rounding. Summing
//      the float and the double rounding errors gives |d - e / length| <= 17u * maxnorm,
//      where u = 2^-24 and maxnorm is the largest L1 norm among the points; `slack` is
//      twice that. A point is only classified here when e / length is further than slack
//      from the ON_EPSILON boundaries, so the exact test would classify it the same way.
//
//      Returns -1 when the exact test certainly rejects the candidate, 1 when it certainly
//      accepts it (with *fliptest set), 0 when some point is too close to call.
// =====================================================================================
static inline int SeparatorVerdict(
    const double (*src)[3], const int ns, const int i, const int l,
    const double (*pas)[3], const int np, const int j,
    const vec3_t n, const double length, const double slack, bool* const fliptest)
{
    const double    nx = n[0], ny = n[1], nz = n[2];
    const double    clear = (ON_EPSILON + slack) * length; // |e| beyond this: |d| > ON_EPSILON
    const double    on = (ON_EPSILON - slack) * length;    // |e| below this: |d| < ON_EPSILON
    const double    base = pas[j][0] * nx + pas[j][1] * ny + pas[j][2] * nz;
    int             k;

    // which side of the plane is the source on
    for (k = 0; k < ns; k++)
    {
        if ((k == i) | (k == l))
        {
            continue;
        }
        const double e = src[k][0] * nx + src[k][1] * ny + src[k][2] * nz - base;
        if (e < -clear)
        {
            *fliptest = false;
            break;
        }
        if (e > clear)
        {
            *fliptest = true;
            break;
        }
        if (!(fabs(e) < on))
        {
            return 0;
        }
    }
    if (k == ns)
    {
        return -1;                                         // planar with source portal
    }

    // pass must be entirely on the other side
    const double    sign = *fliptest ? -1.0 : 1.0;
    bool            unsure = false;
    int             front = 0;
    for (k = 0; k < np; k++)
    {
        if (k == j)
        {
            continue;
        }
        const double e = sign * (pas[k][0] * nx + pas[k][1] * ny + pas[k][2] * nz - base);
        if (e < -clear)
        {
            return -1;                                     // points on negative side
        }
        if (e > clear)
        {
            front++;
        }
        else if (!(fabs(e) < on))
        {
            unsure = true;                                 // could still be on the negative side
        }
    }
    if (unsure)
    {
        return 0;
    }
    return front ? 1 : -1;                                 // no front point: planar with the plane
}

// =====================================================================================
//  ClipToSeperators
//      Source, pass, and target are an ordering of portals.
//      Generates seperating planes canidates by taking two points from source and one
//      point from pass, and clips target by them.
//      If the target argument is NULL, then a list of clipping planes is built in
//      stack instead.
//      If target is totally clipped away, that portal can not be seen through.
//      Normal clip keeps target on the same side as pass, which is correct if the
//      order goes source, pass, target.  If the order goes pass, source, target then
//      flipclip should be set.
// =====================================================================================
inline static winding_t* ClipToSeperators(
    const winding_t* const source,
    const winding_t* const pass,
    winding_t* const a_target,
    const bool flipclip,
    pstack_t* const stack)
{
    int             i, j, k, l;
    plane_t         plane;
    vec3_t          v1, v2;
    float           d;
    int             counts[3];
    bool            fliptest;
    winding_t*      target = a_target;

    const unsigned int numpoints = source->numpoints;

    // Double copies of both windings for SeparatorVerdict, which settles about 70% of
    // the candidates without the sqrt-and-three-divides normalization. Only the
    // candidates it accepts or cannot decide are normalized.
    double          srcd[MAX_POINTS_ON_FIXED_WINDING][3];
    double          pasd[MAX_POINTS_ON_FIXED_WINDING][3];
    double          slack = 0;
    const bool      filter = numpoints <= MAX_POINTS_ON_FIXED_WINDING && pass->numpoints <= MAX_POINTS_ON_FIXED_WINDING;
    if (filter)
    {
        double      maxnorm = 0;
        for (k = 0; k < (int)numpoints; k++)
        {
            VectorCopy(source->points[k], srcd[k]);
            maxnorm = qmax(maxnorm, fabs(srcd[k][0]) + fabs(srcd[k][1]) + fabs(srcd[k][2]));
        }
        for (k = 0; k < pass->numpoints; k++)
        {
            VectorCopy(pass->points[k], pasd[k]);
            maxnorm = qmax(maxnorm, fabs(pasd[k][0]) + fabs(pasd[k][1]) + fabs(pasd[k][2]));
        }
        slack = 34.0 * maxnorm / 16777216.0 + 1e-6;
    }

    // check all combinations
    for (i=0, l=1; i < numpoints; i++, l++)
    {
        if (l == numpoints)
        {
            l = 0;
        }

        VectorSubtract(source->points[l], source->points[i], v1);

        // fing a vertex of pass that makes a plane that puts all of the
        // vertexes of pass on the front side and all of the vertexes of
        // source on the back side
        for (j = 0; j < pass->numpoints; j++)
        {
            VectorSubtract(pass->points[j], source->points[i], v2);
            CrossProduct(v1, v2, plane.normal);

            // What VectorNormalize computes, split so the divides can be skipped.
            const double length = sqrt((double)DotProduct(plane.normal, plane.normal));
            if (length < ON_EPSILON)
            {
                continue;
            }
            const int verdict = filter
                ? SeparatorVerdict(srcd, numpoints, i, l, pasd, pass->numpoints, j, plane.normal, length, slack, &fliptest)
                : 0;
            if (verdict < 0)
            {
                continue;
            }
            plane.normal[0] /= length;
            plane.normal[1] /= length;
            plane.normal[2] /= length;
            plane.dist = DotProduct(pass->points[j], plane.normal);

            if (verdict == 0)
            {
            // find out which side of the generated seperating plane has the
            // source portal
            fliptest = false;
            for (k = 0; k < numpoints; k++)
            {
                if ((k == i) | (k == l)) // | instead of || for branch optimization
                {
                    continue;
                }
                d = DotProduct(source->points[k], plane.normal) - plane.dist;
                if (d < -ON_EPSILON)
                {                                          // source is on the negative side, so we want all
                    // pass and target on the positive side
                    fliptest = false;
                    break;
                }
                else if (d > ON_EPSILON)
                {                                          // source is on the positive side, so we want all
                    // pass and target on the negative side
                    fliptest = true;
                    break;
                }
            }
            if (k == numpoints)
            {
                continue;                                  // planar with source portal
            }

            // flip the normal if the source portal is backwards
            if (fliptest)
            {
                VectorSubtract(vec3_origin, plane.normal, plane.normal);
                plane.dist = -plane.dist;
            }

            // if all of the pass portal points are now on the positive side,
            // this is the seperating plane
            counts[0] = counts[1] = counts[2] = 0;
            for (k = 0; k < pass->numpoints; k++)
            {
                if (k == j)
                {
                    continue;
                }
                d = DotProduct(pass->points[k], plane.normal) - plane.dist;
                if (d < -ON_EPSILON)
                {
                    break;
                }
                else if (d > ON_EPSILON)
                {
                    counts[0]++;
                }
                else
                {
                    counts[2]++;
                }
            }
            if (k != pass->numpoints)
            {
                continue;                                  // points on negative side, not a seperating plane
            }

            if (!counts[0])
            {
                continue;                                  // planar with seperating plane
            }
            }
            else if (fliptest)
            {
                VectorSubtract(vec3_origin, plane.normal, plane.normal);
                plane.dist = -plane.dist;
            }

            // flip the normal if we want the back side
            if (flipclip)
            {
                VectorSubtract(vec3_origin, plane.normal, plane.normal);
                plane.dist = -plane.dist;
            }

	    if (target != NULL)
	    {
            // clip target by the seperating plane
            target = ChopWinding(target, stack, &plane);
            if (!target)
            {
                return NULL;                               // target is not visible
            }
	    }
	    else
	    {
        	AddPlane(stack, &plane);
	    }

#ifdef RVIS_LEVEL_1
            break; /* Antony was here */
#endif
        }
    }

    return target;
}

// =====================================================================================
//  RecursiveLeafFlow
//      Flood fill through the leafs
//      If src_portal is NULL, this is the originating leaf
// =====================================================================================
static void     RecursiveLeafFlow(const int leafnum, threaddata_t* const thread, const pstack_t* const prevstack)
{
    pstack_t        stack;
    leaf_t*         leaf;

    leaf = &g_leafs[leafnum];
#ifdef USE_CHECK_STACK
    CheckStack(leaf, thread);
#endif

    {
        const unsigned offset = leafnum >> 3;
        const unsigned bit = (1 << (leafnum & 7));

        // mark the leaf as visible
        if (!(thread->leafvis[offset] & bit))
        {
            AtomicOrByte(&thread->leafvis[offset], (byte)bit);
        }
    }

#ifdef USE_CHECK_STACK
    prevstack->next = &stack;
    stack.next = NULL;
#endif
    stack.head = prevstack->head;
    stack.leaf = leaf;
    stack.portal = NULL;
#ifdef RVIS_LEVEL_2
    stack.clipPlaneCount = -1;
    stack.clipPlane = NULL;
#endif

    // check all portals for flowing into other leafs       
    unsigned i;
    portal_t** plist = leaf->portals;

    for (i = 0; i < leaf->numportals; i++, plist++)
    {
        portal_t* p = *plist;

#if ZHLT_ZONES
        portal_t * head_p = stack.head->portal;
        if (g_Zones->check(head_p->zone, p->zone))
        {
            continue;
        }
#endif

        {
            const unsigned offset = p->leaf >> 3;
            const unsigned bit = 1 << (p->leaf & 7);

            if (!(stack.head->mightsee[offset] & bit))
            {
                continue;                                      // can't possibly see it
            }
            if (!(prevstack->mightsee[offset] & bit))
            {
                continue;                                      // can't possibly see it
            }
        }

        // if the portal can't see anything we haven't allready seen, skip it
        {
            const uint64_t* test;

            // Use the final visbits of every portal flowed earlier in the
            // sorted order and mightsee for the rest. That is exactly what a
            // single thread sees; racing on "whichever portal happens to be
            // done by now" made -full output depend on thread timing.
            if (p->rank < thread->base->rank)
            {
                WaitForPortal(p);
                test = (const uint64_t*)p->visbits;
            }
            else
            {
                test = (const uint64_t*)p->mightsee;
            }

            // 64 bits at a time (long is 32 bits on Windows), and one pass
            // instead of building the whole set before testing it. g_bitbytes
            // is a multiple of 8. leafvis may be growing under other threads
            // flowing the same portal; a stale read only skips less.
            const uint64_t* prevmight = (const uint64_t*)prevstack->mightsee;
            const uint64_t* vis = (const uint64_t*)thread->leafvis;
            uint64_t*       might = (uint64_t*)stack.mightsee;
            uint64_t        unseen = 0;
            const unsigned  bitquads = g_bitbytes / 8;

            for (unsigned j = 0; j < bitquads; j++)
            {
                const uint64_t m = prevmight[j] & test[j];
                might[j] = m;
                unseen |= m & ~vis[j];
            }
            if (!unseen)
            {
                continue;                                  // can't see anything new
            }
        }

        // get plane of portal, point normal into the neighbor leaf
        stack.portalplane = &p->plane;
        plane_t             backplane;
        VectorSubtract(vec3_origin, p->plane.normal, backplane.normal);
        backplane.dist = -p->plane.dist;

        if (VectorCompare(prevstack->portalplane->normal, backplane.normal))
        {
            continue;                                      // can't go out a coplanar face
        }

        stack.portal = p;
#ifdef USE_CHECK_STACK
        stack.next = NULL;
#endif
        stack.freewindings[0] = 1;
        stack.freewindings[1] = 1;
        stack.freewindings[2] = 1;

        stack.pass = ChopWinding(p->winding, &stack, thread->pstack_head.portalplane);
        if (!stack.pass)
        {
            continue;
        }

        stack.source = ChopWinding(prevstack->source, &stack, &backplane);
        if (!stack.source)
        {
            continue;
        }

        if (!prevstack->pass)
        {                                                  // the second leaf can only be blocked if coplanar
            if (!TrySpawnFlow(p->leaf, thread, &stack))
            {
                RecursiveLeafFlow(p->leaf, thread, &stack);
            }
            continue;
        }

        stack.pass = ChopWinding(stack.pass, &stack, prevstack->portalplane);
        if (!stack.pass)
        {
            continue;
        }

#ifdef RVIS_LEVEL_2
        if (stack.clipPlaneCount == -1)
        {
            stack.clipPlaneCount = 0;
            stack.clipPlane = (plane_t*)alloca(sizeof(plane_t) * prevstack->source->numpoints * prevstack->pass->numpoints);

            ClipToSeperators(prevstack->source, prevstack->pass, NULL, false, &stack);
            ClipToSeperators(prevstack->pass, prevstack->source, NULL, true, &stack);
        }

        if (stack.clipPlaneCount > 0)
        {
            unsigned j;
            for (j = 0; j < stack.clipPlaneCount && stack.pass != NULL; j++)
            {
                stack.pass = ChopWinding(stack.pass, &stack, &(stack.clipPlane[j]));
            }

            if (stack.pass == NULL)
            continue;
        }
#else

        stack.pass = ClipToSeperators(stack.source, prevstack->pass, stack.pass, false, &stack);
        if (!stack.pass)
        {
            continue;
        }

        stack.pass = ClipToSeperators(prevstack->pass, stack.source, stack.pass, true, &stack);
        if (!stack.pass)
        {
            continue;
        }
#endif

        if (g_fullvis)
        {
            stack.source = ClipToSeperators(stack.pass, prevstack->pass, stack.source, false, &stack);
            if (!stack.source)
            {
                continue;
            }

            stack.source = ClipToSeperators(prevstack->pass, stack.pass, stack.source, true, &stack);
            if (!stack.source)
            {
                continue;
            }
        }

        // flow through it for real
        if (!TrySpawnFlow(p->leaf, thread, &stack))
        {
            RecursiveLeafFlow(p->leaf, thread, &stack);
        }
    }

#ifdef RVIS_LEVEL_2
#if 0
    if (stack.clipPlane != NULL)
    {
        free(stack.clipPlane);
    }
#endif
#endif
}

// =====================================================================================
//  RunFlowTask
// =====================================================================================
static void     RunFlowTask(flowtask_t* t)
{
    threaddata_t*   thread = t->thread;

    t_nesting++;
    RecursiveLeafFlow(t->leafnum, thread, &t->frame);
    t_nesting--;
    free(t);
    {
        // The owner may return and release `thread` as soon as this reaches zero.
        std::lock_guard<std::mutex> lock(g_flowmutex);
        thread->pending--;
    }
    g_flowcv.notify_all();
}

// =====================================================================================
//  PortalFlow
// =====================================================================================
void            PortalFlow(portal_t* p)
{
    threaddata_t    data;
    unsigned        i;

    if (p->status != stat_working)
        Error("PortalFlow: reflowed");

    p->visbits = (byte*)calloc(1, g_bitbytes);

    memset(&data, 0, sizeof(data));
    data.leafvis = p->visbits;
    data.base = p;

    data.pstack_head.head = &data.pstack_head;
    data.pstack_head.portal = p;
    data.pstack_head.source = p->winding;
    data.pstack_head.portalplane = &p->plane;
    memcpy(data.pstack_head.mightsee, p->mightsee, g_bitbytes);
    RecursiveLeafFlow(p->leaf, &data, &data.pstack_head);

    {
        // Subtrees handed to other threads must finish before the visbits are final.
        std::unique_lock<std::mutex> lock(g_flowmutex);
        HelpUntil(lock, p->rank, [&data] { return data.pending == 0; });
    }

    p->numcansee = 0;
    for (i = 0; i < g_portalleafs; i++)
    {
        if (p->visbits[i >> 3] & (1 << (i & 7)))
        {
            p->numcansee++;
        }
    }

#ifdef ZHLT_NETVIS
    p->fromclient = g_clientid;
#endif
    p->status = stat_done;
    MarkPortalDone(p);
#ifdef ZHLT_NETVIS
    Flag_VIS_DONE_PORTAL(g_visportalindex);
#endif
}

// =====================================================================================
//  SimpleFlood
//      This is a rough first-order aproximation that is used to trivially reject some
//      of the final calculations.
// =====================================================================================
static void     SimpleFlood(byte* const srcmightsee, const int leafnum, byte* const portalsee, unsigned int* const c_leafsee)
{
    unsigned        i;
    leaf_t*         leaf;
    portal_t*       p;

    {
        const unsigned  offset = leafnum >> 3;
        const unsigned  bit = (1 << (leafnum & 7));
    
        if (srcmightsee[offset] & bit)
        {
            return;
        }
        else
        {
            srcmightsee[offset] |= bit;
        }
    }

    (*c_leafsee)++;
    leaf = &g_leafs[leafnum];

    for (i = 0; i < leaf->numportals; i++)
    {
        p = leaf->portals[i];
        if (!portalsee[p - g_portals])
        {
            continue;
        }
        SimpleFlood(srcmightsee, p->leaf, portalsee, c_leafsee);
    }
}

#define PORTALSEE_SIZE (MAX_PORTALS*2)
#ifdef SYSTEM_WIN32
#pragma warning(push)
#pragma warning(disable: 4100)                             // unreferenced formal parameter
#endif


// =====================================================================================
//  BasePortalVis
// =====================================================================================
void            BasePortalVis(int unused)
{
    int             i, j, k;
    portal_t*       tp;
    portal_t*       p;
    float           d;
    winding_t*      w;
    byte            portalsee[PORTALSEE_SIZE];
    const int       portalsize = (g_numportals * 2);

#ifdef ZHLT_NETVIS
    {
        i = unused;
#else
    while (1)
    {
        i = GetThreadWork();
        if (i == -1)
            break;
#endif
        p = g_portals + i;

        p->mightsee = (byte*)calloc(1, g_bitbytes);

        memset(portalsee, 0, portalsize);

#if ZHLT_ZONES
        UINT32 zone = p->zone;
#endif

        for (j = 0, tp = g_portals; j < portalsize; j++, tp++)
        {
            if (j == i)
            {
                continue;
            }
#if ZHLT_ZONES
            if (g_Zones->check(zone, tp->zone))
            {
                continue;
            }
#endif

            w = tp->winding;
            for (k = 0; k < w->numpoints; k++)
            {
                d = DotProduct(w->points[k], p->plane.normal) - p->plane.dist;
                if (d > ON_EPSILON)
                {
                    break;
                }
            }
            if (k == w->numpoints)
            {
                continue;                                  // no points on front
            }


            w = p->winding;
            for (k = 0; k < w->numpoints; k++)
            {
                d = DotProduct(w->points[k], tp->plane.normal) - tp->plane.dist;
                if (d < -ON_EPSILON)
                {
                    break;
                }
            }
            if (k == w->numpoints)
            {
                continue;                                  // no points on front
            }


            portalsee[j] = 1;
        }

        SimpleFlood(p->mightsee, p->leaf, portalsee, &p->nummightsee);
        Verbose("portal:%4i  nummightsee:%4i \n", i, p->nummightsee);
    }
}

bool BestNormalFromWinding (const vec3_t *points, int numpoints, vec3_t &normal_out)
{
	const vec3_t *pt1, *pt2, *pt3;
	int k;
	vec3_t d, normal, edge;
	vec_t dist, maxdist;
	if (numpoints < 3)
	{
		return false;
	}
	pt1 = &points[0];
	maxdist = -1;
	for (k = 0; k < numpoints; k++)
	{
		if (&points[k] == pt1)
		{
			continue;
		}
		VectorSubtract (points[k], *pt1, edge);
		dist = DotProduct (edge, edge);
		if (dist > maxdist)
		{
			maxdist = dist;
			pt2 = &points[k];
		}
	}
	if (maxdist <= ON_EPSILON * ON_EPSILON)
	{
		return false;
	}
	maxdist = -1;
	VectorSubtract (*pt2, *pt1, edge);
	VectorNormalize (edge);
	for (k = 0; k < numpoints; k++)
	{
		if (&points[k] == pt1 || &points[k] == pt2)
		{
			continue;
		}
		VectorSubtract (points[k], *pt1, d);
		CrossProduct (edge, d, normal);
		dist = DotProduct (normal, normal);
		if (dist > maxdist)
		{
			maxdist = dist;
			pt3 = &points[k];
		}
	}
	if (maxdist <= ON_EPSILON * ON_EPSILON)
	{
		return false;
	}
	VectorSubtract (*pt3, *pt1, d);
	CrossProduct (edge, d, normal);
	VectorNormalize (normal);
	if (pt3 < pt2)
	{
		VectorScale (normal, -1, normal);
	}
	VectorCopy (normal, normal_out);
	return true;
}

vec_t WindingDist (const winding_t *w[2])
{
	vec_t minsqrdist = 99999999.0 * 99999999.0;
	vec_t sqrdist;
	int a, b;
	// point to point
	for (a = 0; a < w[0]->numpoints; a++)
	{
		for (b = 0; b < w[1]->numpoints; b++)
		{
			vec3_t v;
			VectorSubtract (w[0]->points[a], w[1]->points[b], v);
			sqrdist = DotProduct (v, v);
			if (sqrdist < minsqrdist)
			{
				minsqrdist = sqrdist;
			}
		}
	}
	// point to edge
	for (int side = 0; side < 2; side++)
	{
		for (a = 0; a < w[side]->numpoints; a++)
		{
			for (b = 0; b < w[!side]->numpoints; b++)
			{
				const vec3_t &p = w[side]->points[a];
				const vec3_t &p1 = w[!side]->points[b];
				const vec3_t &p2 = w[!side]->points[(b + 1) % w[!side]->numpoints];
				vec3_t delta;
				vec_t frac;
				vec3_t v;
				VectorSubtract (p2, p1, delta);
				if (VectorNormalize (delta) <= ON_EPSILON)
				{
					continue;
				}
				frac = DotProduct (p, delta) - DotProduct (p1, delta);
				if (frac <= ON_EPSILON || frac >= (DotProduct (p2, delta) - DotProduct (p1, delta)) - ON_EPSILON)
				{
					// p1 or p2 is closest to p
					continue;
				}
				VectorMA (p1, frac, delta, v);
				VectorSubtract (p, v, v);
				sqrdist = DotProduct (v, v);
				if (sqrdist < minsqrdist)
				{
					minsqrdist = sqrdist;
				}
			}
		}
	}
	// edge to edge
	for (a = 0; a < w[0]->numpoints; a++)
	{
		for (b = 0; b < w[1]->numpoints; b++)
		{
			const vec3_t &p1 = w[0]->points[a];
			const vec3_t &p2 = w[0]->points[(a + 1) % w[0]->numpoints];
			const vec3_t &p3 = w[1]->points[b];
			const vec3_t &p4 = w[1]->points[(b + 1) % w[1]->numpoints];
			vec3_t delta1;
			vec3_t delta2;
			vec3_t normal;
			vec3_t normal1;
			vec3_t normal2;
			VectorSubtract (p2, p1, delta1);
			VectorSubtract (p4, p3, delta2);
			CrossProduct (delta1, delta2, normal);
			if (!VectorNormalize (normal))
			{
				continue;
			}
			CrossProduct (normal, delta1, normal1); // same direction as delta2
			CrossProduct (delta2, normal, normal2); // same direction as delta1
			if (VectorNormalize (normal1) <= ON_EPSILON || VectorNormalize (normal2) <= ON_EPSILON)
			{
				continue;
			}
			if (DotProduct (p3, normal1) >= DotProduct (p1, normal1) - ON_EPSILON ||
				DotProduct (p4, normal1) <= DotProduct (p1, normal1) + ON_EPSILON ||
				DotProduct (p1, normal2) >= DotProduct (p3, normal2) - ON_EPSILON ||
				DotProduct (p2, normal2) <= DotProduct (p3, normal2) + ON_EPSILON )
			{
				// the edges are not crossing when viewed along normal
				continue;
			}
			sqrdist = DotProduct (p3, normal) - DotProduct (p1, normal);
			sqrdist = sqrdist * sqrdist;
			if (sqrdist < minsqrdist)
			{
				minsqrdist = sqrdist;
			}
		}
	}
	// point to face and edge to face
	for (int side = 0; side < 2; side++)
	{
		vec3_t planenormal;
		vec_t planedist;
		vec3_t *boundnormals;
		vec_t *bounddists;
		if (!BestNormalFromWinding (w[!side]->points, w[!side]->numpoints, planenormal))
		{
			continue;
		}
		planedist = DotProduct (planenormal, w[!side]->points[0]);
		hlassume (boundnormals = (vec3_t *)malloc (w[!side]->numpoints * sizeof (vec3_t)), assume_NoMemory);
		hlassume (bounddists = (vec_t *)malloc (w[!side]->numpoints * sizeof (vec_t)), assume_NoMemory);
		// build boundaries
		for (b = 0; b < w[!side]->numpoints; b++)
		{
			vec3_t v;
			const vec3_t &p1 = w[!side]->points[b];
			const vec3_t &p2 = w[!side]->points[(b + 1) % w[!side]->numpoints];
			VectorSubtract (p2, p1, v);
			CrossProduct (v, planenormal, boundnormals[b]);
			if (!VectorNormalize (boundnormals[b]))
			{
				bounddists[b] = 1.0;
			}
			else
			{
				bounddists[b] = DotProduct (p1, boundnormals[b]);
			}
		}
		for (a = 0; a < w[side]->numpoints; a++)
		{
			const vec3_t &p = w[side]->points[a];
			for (b = 0; b < w[!side]->numpoints; b++)
			{
				if (DotProduct (p, boundnormals[b]) - bounddists[b] >= -ON_EPSILON)
				{
					break;
				}
			}
			if (b < w[!side]->numpoints)
			{
				continue;
			}
			sqrdist = DotProduct (p, planenormal) - planedist;
			sqrdist = sqrdist * sqrdist;
			if (sqrdist < minsqrdist)
			{
				minsqrdist = sqrdist;
			}
		}
		for (a = 0; a < w[side]->numpoints; a++)
		{
			const vec3_t &p1 = w[side]->points[a];
			const vec3_t &p2 = w[side]->points[(a + 1) % w[side]->numpoints];
			vec_t dist1 = DotProduct (p1, planenormal) - planedist;
			vec_t dist2 = DotProduct (p2, planenormal) - planedist;
			vec3_t delta;
			vec_t frac;
			vec3_t v;
			if (dist1 > ON_EPSILON && dist2 < -ON_EPSILON || dist1 < -ON_EPSILON && dist2 > ON_EPSILON)
			{
				frac = dist1 / (dist1 - dist2);
				VectorSubtract (p2, p1, delta);
				VectorMA (p1, frac, delta, v);
				for (b = 0; b < w[!side]->numpoints; b++)
				{
					if (DotProduct (v, boundnormals[b]) - bounddists[b] >= -ON_EPSILON)
					{
						break;
					}
				}
				if (b < w[!side]->numpoints)
				{
					continue;
				}
				minsqrdist = 0;
			}
		}
		free (boundnormals);
		free (bounddists);
	}
	return (sqrt (minsqrdist));
}
// AJM: MVD
// =====================================================================================
//  MaxDistVis
// =====================================================================================
void	MaxDistVis(int unused)
{
	int i, j, k, m;
	int a, b, c, d;
	leaf_t	*l;
	leaf_t	*tl;
	plane_t	*boundary = NULL;
	vec3_t delta;

	float new_dist;

	unsigned offset_l;
	unsigned bit_l;

	unsigned offset_tl;
	unsigned bit_tl;
	
	while(1)
	{
		i = GetThreadWork();
		if (i == -1)
			break;

		l = &g_leafs[i];

		for(j = i + 1, tl = g_leafs + j; j < g_portalleafs; j++, tl++)
		{

			offset_l = i >> 3;
			bit_l = (1 << (i & 7));

			offset_tl = j >> 3;
			bit_tl = (1 << (j & 7));

			{
				bool visible = false;
				for (k = 0; k < l->numportals; k++)
				{
					if (l->portals[k]->visbits[offset_tl] & bit_tl)
					{
						visible = true;
					}
				}
				for (m = 0; m < tl->numportals; m++)
				{	
					if (tl->portals[m]->visbits[offset_l] & bit_l)
					{
						visible = true;
					}
				}
				if (!visible)
				{
					goto NoWork;
				}
			}
			
			// rough check
			{
				vec3_t v;
				vec_t dist;
				const winding_t *w;
				const leaf_t *leaf[2] = {l, tl};
				vec3_t center[2];
				vec_t radius[2];
				int count[2];
				for (int side = 0; side < 2; side++)
				{
					count[side] = 0;
					VectorClear (center[side]);
					for (a = 0; a < leaf[side]->numportals; a++)
					{
						w = leaf[side]->portals[a]->winding;
						for (b = 0; b < w->numpoints; b++)
						{
							VectorAdd (w->points[b], center[side], center[side]);
							count[side]++;
						}
					}
				}
				if (!count[0] && !count[1])
				{
					goto Work;
				}
				for (int side = 0; side < 2; side++)
				{
					VectorScale (center[side], 1.0 / (vec_t)count[side], center[side]);
					radius[side] = 0;
					for (a = 0; a < leaf[side]->numportals; a++)
					{
						w = leaf[side]->portals[a]->winding;
						for (b = 0; b < w->numpoints; b++)
						{
							VectorSubtract (w->points[b], center[side], v);
							dist = DotProduct (v, v);
							radius[side] = qmax (radius[side], dist);
						}
					}
					radius[side] = sqrt (radius[side]);
				}
				VectorSubtract (center[0], center[1], v);
				dist = VectorLength (v);
				if (qmax (dist - radius[0] - radius[1], 0) >= g_maxdistance - ON_EPSILON)
				{
					goto Work;
				}
				if (dist + radius[0] + radius[1] < g_maxdistance - ON_EPSILON)
				{
					goto NoWork;
				}
			}

			// exact check
			{
				vec_t mindist = 9999999999;
				vec_t dist;
				for (k = 0; k < l->numportals; k++)
				{
					for (m = 0; m < tl->numportals; m++)
					{
						const winding_t *w[2];
						w[0] = l->portals[k]->winding;
						w[1] = tl->portals[m]->winding;
						dist = WindingDist (w);
						mindist = qmin (dist, mindist);
					}
				}
				if (mindist >= g_maxdistance - ON_EPSILON)
				{
					goto Work;
				}
				else
				{
					goto NoWork;
				}
			}

Work:
			ThreadLock ();
			for (k = 0; k < l->numportals; k++)
			{
				l->portals[k]->visbits[offset_tl] &= ~bit_tl;
			}
			for (m = 0; m < tl->numportals; m++)
			{
				tl->portals[m]->visbits[offset_l] &= ~bit_l;
			}
			ThreadUnlock ();
			
NoWork:
			continue;	// Hack to keep label from causing compile error
		}
	}

	// Release potential memory
	if(boundary)
		delete [] boundary;
}

#ifdef SYSTEM_WIN32
#pragma warning(pop)
#endif

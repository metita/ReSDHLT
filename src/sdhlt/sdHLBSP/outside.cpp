#pragma warning(disable: 4267) // 'size_t' to 'unsigned int', possible loss of data

#include "bsp5.h"

#include <algorithm>
#include <queue>
#include <unordered_map>
#include <utility>
#include <vector>

//  PointInLeaf
//  PlaceOccupant
//  BuildShortestLeakTrail
//  RecursiveFillOutside
//  ClearOutFaces_r
//  isClassnameAllowableOutside
//  FreeAllowableOutsideList
//  LoadAllowableOutsideList
//  FillOutside
//  PrintLeakSummary

static int      outleafs;
static int      valid;
static int      c_falsenodes;
static int      c_free_faces;
static int      c_keep_faces;

// =====================================================================================
//  PointInLeaf
// =====================================================================================
static node_t*  PointInLeaf(node_t* node, const vec3_t point)
{
    vec_t           d;

	if (node->isportalleaf)
    {
        //Log("PointInLeaf::node->contents == %i\n", node->contents);
        return node;
    }

    d = DotProduct(g_dplanes[node->planenum].normal, point) - g_dplanes[node->planenum].dist;

    if (d > 0)
        return PointInLeaf(node->children[0], point);

    return PointInLeaf(node->children[1], point);
}

// =====================================================================================
//  PlaceOccupant
// =====================================================================================
static bool     PlaceOccupant(const int num, const vec3_t point, node_t* headnode)
{
    node_t*         n;

    n = PointInLeaf(headnode, point);
    if (n->contents == CONTENTS_SOLID)
    {
        return false;
    }
    //Log("PlaceOccupant::n->contents == %i\n", n->contents);

    n->occupied = num;
    return true;
}

// =====================================================================================
//  Leak diagnostics
//      The trail the classic pointfile drew was whatever order the outside
//      flood fill happened to unwind in: it wanders through the map, doubles
//      back, and the point where it actually leaves the map is nowhere marked.
//      What a mapper needs is the hole.
//
//      So the trail is built separately, after the fill has proved there is a
//      leak: a Dijkstra over the portal graph, weighted by the distance
//      between consecutive portal centres, giving the geometrically shortest
//      way from the leaked entity to the void. Its far end is the shell portal
//      the leak escapes through, and the point just before it is the
//      constriction - the hole - which gets its coordinates printed and a
//      dense marker star written into the pointfile.
// =====================================================================================
typedef struct
{
	vec3_t			p;
	vec_t			area;    // of the portal this point centres; unset for the entity
	bool			outside; // does this portal lead into the void?
}
trailpoint_t;

typedef struct
{
	char			classname[64];
	vec3_t			origin;
	vec3_t			hole;
}
leaksite_t;

static std::vector< trailpoint_t > g_leaktrail;
static node_t*  g_leakleaf;                                // leaf of the entity that leaked
static bool     g_haveleaktrail;
static vec3_t   g_leakexit;

// Consolidated across every hull, printed once by PrintLeakSummary.
static bool     g_leak_any = false;
static int      g_leak_hulls = 0;                          // bitmask
static char     g_leak_entclass[64] = "";
static vec3_t   g_leak_entorigin;
static vec3_t   g_leak_hole;
static bool     g_leak_hashole = false;
static std::vector< leaksite_t > g_leaksites;
static std::vector< node_t * > g_leakfrontier;

static void     WindingCenter (const portal_t *p, trailpoint_t &out)
{
	p->winding->getCenter (out.p);
	out.area = p->winding->getArea ();
}

// =====================================================================================
//  MarkOutsideLeafs
//      Floods the void with entities treated as walls rather than as a
//      stopping condition. What it marks is exactly the region that is
//      unambiguously outside the map, and where it stops is one frontier leaf
//      per hole - which is both what tells the trail builder where the map
//      ends, and what -allleaks needs to find every hole instead of the first.
// =====================================================================================
static int      g_outsidemark = 0;

static void     MarkOutsideLeafs_r (node_t *l)
{
	if (l->contents == CONTENTS_SOLID || l->contents == CONTENTS_SKY)
	{
		return;
	}
	if (l->valid == valid)
	{
		return;
	}
	l->valid = valid;

	if (l->occupied)
	{
		g_leakfrontier.push_back (l);
		return; // a barrier, so the flood stays outside
	}

	portal_t *p;
	for (p = l->portals; p;)
	{
		int s = (p->nodes[0] == l);
		MarkOutsideLeafs_r (p->nodes[s]);
		p = p->next[!s];
	}
}

static void     MarkOutsideLeafs ()
{
	g_leakfrontier.clear ();
	valid++;
	g_outsidemark = valid;
	int s = !(g_outside_node.portals->nodes[1] == &g_outside_node);
	MarkOutsideLeafs_r (g_outside_node.portals->nodes[s]);
}

static bool     LeafIsOutside (const node_t *l)
{
	return l != &g_outside_node && l->valid == g_outsidemark && !l->occupied;
}

// The hole is where the escape route leaves the map: the first portal on the
// path whose far side is a leaf the outside flood reached. Everything past it
// is void, and the void's own portals - the BSP splits it as finely as it
// splits anything - are why "the point before the last one" lands in odd
// places. Falls back to the narrowest portal on the path if nothing was
// marked, which is the same idea by a weaker measure.
static int      FindHoleIndex ()
{
	int n = (int)g_leaktrail.size ();
	if (n < 2)
	{
		return 0;
	}
	int i;
	for (i = 1; i < n; i++)
	{
		if (g_leaktrail[i].outside)
		{
			return i;
		}
	}
	int best = 1;
	for (i = 1; i < n - 1; i++)
	{
		if (g_leaktrail[i].area < g_leaktrail[best].area)
		{
			best = i;
		}
	}
	return best;
}

// perpendicular distance from p to the segment a-b
static vec_t    PointSegDistance (const vec3_t p, const vec3_t a, const vec3_t b)
{
	vec3_t ab, ap, proj;
	VectorSubtract (b, a, ab);
	VectorSubtract (p, a, ap);
	vec_t ab2 = DotProduct (ab, ab);
	vec_t t = ab2 > 0? DotProduct (ap, ab) / ab2: 0;
	if (t < 0) t = 0;
	if (t > 1) t = 1;
	VectorMA (a, t, ab, proj);
	VectorSubtract (p, proj, proj);
	return VectorLength (proj);
}

// Douglas-Peucker: keep the vertices the simplified polyline needs to stay
// within tol of the original, so the .lin file is a handful of clean segments
// instead of one per portal crossed.
static void     SimplifyTrail_r (int i0, int i1, vec_t tol, std::vector< char > &keep)
{
	vec_t dmax = 0;
	int idx = -1;
	int i;
	for (i = i0 + 1; i < i1; i++)
	{
		vec_t d = PointSegDistance (g_leaktrail[i].p, g_leaktrail[i0].p, g_leaktrail[i1].p);
		if (d > dmax)
		{
			dmax = d;
			idx = i;
		}
	}
	if (dmax > tol && idx > 0)
	{
		keep[idx] = 1;
		SimplifyTrail_r (i0, idx, tol, keep);
		SimplifyTrail_r (idx, i1, tol, keep);
	}
}

// A dense 3D star, so the hole is an unmistakable blob among the trail dots.
static void     WriteLeakMarker (FILE *f, const vec3_t pos)
{
	const vec_t radius = 48.0;
	static const int dirs[13][3] = {
		{1,0,0}, {0,1,0}, {0,0,1},
		{1,1,0}, {1,-1,0}, {1,0,1}, {1,0,-1}, {0,1,1}, {0,1,-1},
		{1,1,1}, {1,1,-1}, {1,-1,1}, {1,-1,-1},
	};
	int k;
	for (k = 0; k < 13; k++)
	{
		vec_t d;
		for (d = -radius; d <= radius; d += 2.0)
		{
			fprintf (f, "%f %f %f\n", pos[0] + dirs[k][0] * d, pos[1] + dirs[k][1] * d, pos[2] + dirs[k][2] * d);
		}
	}
}

static bool     BuildShortestLeakTrail (node_t *occupied, const vec3_t startpos)
{
	g_leaktrail.clear ();
	if (!occupied)
	{
		return false;
	}

	std::unordered_map< node_t *, portal_t * > parentportal;
	std::unordered_map< node_t *, node_t * > parentnode;
	std::unordered_map< node_t *, double > dist;
	std::unordered_map< node_t *, trailpoint_t > arrival; // portal centre used to reach a leaf
	typedef std::pair< double, node_t * > queueentry_t;
	std::priority_queue< queueentry_t, std::vector< queueentry_t >, std::greater< queueentry_t > > pq;

	trailpoint_t start;
	VectorCopy (startpos, start.p);
	start.area = 0; // never a candidate hole: FindHoleIndex skips both ends
	dist[occupied] = 0;
	arrival[occupied] = start;
	parentportal[occupied] = NULL;
	parentnode[occupied] = NULL;
	pq.push (queueentry_t (0.0, occupied));

	node_t *exitleaf = NULL;
	portal_t *exitportal = NULL;
	double exitcost = 0;

	while (!pq.empty ())
	{
		double d = pq.top ().first;
		node_t *l = pq.top ().second;
		pq.pop ();
		if (d > dist[l])
		{
			continue; // stale queue entry
		}
		if (exitleaf && d >= exitcost)
		{
			break; // the best exit is already settled
		}

		portal_t *p;
		for (p = l->portals; p;)
		{
			int side = (p->nodes[0] == l);
			node_t *nb = p->nodes[side];
			trailpoint_t cp;
			WindingCenter (p, cp);
			vec3_t seg;
			VectorSubtract (cp.p, arrival[l].p, seg);
			double nd = d + VectorLength (seg);

			if (nb == &g_outside_node)
			{
				if (!exitleaf || nd < exitcost)
				{
					exitleaf = l;
					exitportal = p;
					exitcost = nd;
				}
			}
			else if (nb->contents != CONTENTS_SOLID && nb->contents != CONTENTS_SKY)
			{
				std::unordered_map< node_t *, double >::iterator it = dist.find (nb);
				if (it == dist.end () || nd < it->second)
				{
					dist[nb] = nd;
					arrival[nb] = cp;
					parentportal[nb] = p;
					parentnode[nb] = l;
					pq.push (queueentry_t (nd, nb));
				}
			}
			p = p->next[!side];
		}
	}
	if (!exitleaf)
	{
		return false;
	}

	// backtrace exit -> entity, then reverse
	std::vector< trailpoint_t > path;
	node_t *cur;
	for (cur = exitleaf; cur && parentportal[cur]; cur = parentnode[cur])
	{
		trailpoint_t tp;
		WindingCenter (parentportal[cur], tp);
		tp.outside = LeafIsOutside (cur); // this portal leads into cur
		path.push_back (tp);
	}
	std::reverse (path.begin (), path.end ());
	// start the trail at the entity itself, which is what the summary names
	start.outside = false;
	g_leaktrail.push_back (start);
	g_leaktrail.insert (g_leaktrail.end (), path.begin (), path.end ());
	// and end it at the shell portal: the way out
	trailpoint_t exitcentre;
	WindingCenter (exitportal, exitcentre);
	exitcentre.outside = true;
	g_leaktrail.push_back (exitcentre);
	return true;
}

// Emits the trail that has already been built into open pts/lin files and
// reports the hole it pierces. Shared by the single leak and the -allleaks
// survey, which concatenates several trails into the same pair of files.
static bool     EmitLeakTrail (FILE *pts, FILE *lin, vec3_t hole_out)
{
	int n = (int)g_leaktrail.size ();
	if (n < 1)
	{
		return false;
	}

	VectorCopy (g_leaktrail[FindHoleIndex ()].p, hole_out);

	std::vector< char > keep ((size_t)n, 0);
	keep[0] = keep[n - 1] = 1;
	if (n >= 2)
	{
		SimplifyTrail_r (0, n - 1, 16.0, keep);
	}
	std::vector< int > idx;
	int i;
	for (i = 0; i < n; i++)
	{
		if (keep[i])
		{
			idx.push_back (i);
		}
	}

	size_t s;
	for (s = 0; s + 1 < idx.size (); s++)
	{
		const vec_t *a = g_leaktrail[idx[s]].p;
		const vec_t *b = g_leaktrail[idx[s + 1]].p;
		// lin: one clean segment per simplified edge
		fprintf (lin, "%f %f %f - %f %f %f\n", a[0], a[1], a[2], b[0], b[1], b[2]);
		// pts: densified, for editors that only render dots
		vec3_t p, dir;
		VectorCopy (a, p);
		VectorSubtract (b, a, dir);
		vec_t len = VectorNormalize (dir);
		while (len > 8)
		{
			fprintf (pts, "%f %f %f\n", p[0], p[1], p[2]);
			VectorMA (p, 8, dir, p);
			len -= 8;
		}
	}
	const vec_t *last = g_leaktrail[idx.back ()].p;
	fprintf (pts, "%f %f %f\n", last[0], last[1], last[2]);
	WriteLeakMarker (pts, hole_out);
	return true;
}

static void     OpenLeakFiles (FILE *&pts, FILE *&lin)
{
	pts = fopen (g_pointfilename, "w");
	lin = fopen (g_linefilename, "w");
	if (!pts || !lin)
	{
		if (pts) fclose (pts);
		if (lin) fclose (lin);
		Error ("Couldn't open leak file %s / %s\n", g_pointfilename, g_linefilename);
	}
}

static bool     WriteLeakFiles ()
{
	if (g_leaktrail.empty ())
	{
		return false;
	}
	FILE *pts = NULL;
	FILE *lin = NULL;
	OpenLeakFiles (pts, lin);
	vec3_t hole;
	bool ok = EmitLeakTrail (pts, lin, hole);
	fclose (lin);
	fclose (pts);
	if (ok)
	{
		VectorCopy (hole, g_leakexit);
		g_haveleaktrail = true;
	}
	return ok;
}

// =====================================================================================
//  RecursiveFillOutside
//      Returns true if an occupied leaf is reached
//      If fill is false, just check, don't fill 
// =====================================================================================
static void FreeDetailNode_r (node_t *n)
{
	int i;
	if (n->planenum == -1)
	{
		if (!(n->isportalleaf && n->contents == CONTENTS_SOLID))
		{
			free (n->markfaces);
			n->markfaces = NULL;
		}
		return;
	}
	for (i = 0; i < 2; i++)
	{
		FreeDetailNode_r (n->children[i]);
		free (n->children[i]);
		n->children[i] = NULL;
	}
	face_t *f, *next;
	for (f = n->faces; f; f = next)
	{
		next = f->next;
		FreeFace (f);
	}
	n->faces = NULL;
}
static void FillLeaf (node_t *l)
{
	if (!l->isportalleaf)
	{
		Warning ("FillLeaf: not leaf");
		return;
	}
	if (l->contents == CONTENTS_SOLID)
	{
		Warning ("FillLeaf: fill solid");
		return;
	}
	FreeDetailNode_r (l);
	l->contents = CONTENTS_SOLID;
	l->planenum = -1;
}
static int      hit_occupied;
static bool     RecursiveFillOutside(node_t* l, const bool fill)
{
    portal_t*       p;
    int             s;

    if ((l->contents == CONTENTS_SOLID) || (l->contents == CONTENTS_SKY) 
        )
	{
        /*if (l->contents != CONTENTS_SOLID)
            Log("RecursiveFillOutside::l->contents == %i \n", l->contents);*/

        return false;
    }

    if (l->valid == valid)
    {
        return false;
    }

    if (l->occupied)
    {
        hit_occupied = l->occupied;
        g_leakleaf = l;                                    // where the trail starts
        return true;
    }

    l->valid = valid;

    // fill it and it's neighbors
    if (fill)
    {
		FillLeaf (l);
    }
    outleafs++;

    for (p = l->portals; p;)
    {
        s = (p->nodes[0] == l);

        if (RecursiveFillOutside(p->nodes[s], fill))
        {                                                  // leaked, so stop filling
            return true;
        }
        p = p->next[!s];
    }

    return false;
}

// =====================================================================================
//  ClearOutFaces_r
//      Removes unused nodes
// =====================================================================================
static void MarkFacesInside_r (node_t *node)
{
	if (node->planenum == -1)
	{
		face_t **fp;
		for (fp = node->markfaces; *fp; fp++)
		{
	        (*fp)->outputnumber = 0;
		}
	}
	else
	{
		MarkFacesInside_r (node->children[0]);
		MarkFacesInside_r (node->children[1]);
	}
}
static node_t*  ClearOutFaces_r(node_t* node)
{
    face_t*         f;
    face_t*         fnext;
    portal_t*       p;

    // mark the node and all it's faces, so they
    // can be removed if no children use them

    node->valid = 0;                                       // will be set if any children touch it
    for (f = node->faces; f; f = f->next)
    {
        f->outputnumber = -1;
    }

    // go down the children
	if (!node->isportalleaf)
    {
        //
        // decision node
        //
        node->children[0] = ClearOutFaces_r(node->children[0]);
        node->children[1] = ClearOutFaces_r(node->children[1]);

        // free any faces not in open child leafs
        f = node->faces;
        node->faces = NULL;

        for (; f; f = fnext)
        {
            fnext = f->next;
            if (f->outputnumber == -1)
            {                                              // never referenced, so free it
                c_free_faces++;
                FreeFace(f);
            }
            else
            {
                c_keep_faces++;
                f->next = node->faces;
                node->faces = f;
            }
        }

        // A node whose portal was clipped away still owns faces that open leafs list
        // in their marksurfaces. That happens when its plane lies within ON_EPSILON of
        // an ancestor's plane over the whole node, typically two neighbouring faces
        // left almost coplanar by vertex manipulation. Collapsing it dropped those
        // faces from the tree, and the player saw through the wall.
        if (!node->valid && node->faces)
        {
            c_falsenodes++;
            return node;
        }

        if (!node->valid)
        {
			// Here leaks memory. --vluzacn
            // this node does not touch any interior leafs

            // if both children are solid, just make this node solid
            if (node->children[0]->contents == CONTENTS_SOLID && node->children[1]->contents == CONTENTS_SOLID)
            {
                node->contents = CONTENTS_SOLID;
                node->planenum = -1;
				node->isportalleaf = true;
                return node;
            }

            // if one child is solid, shortcut down the other side
            if (node->children[0]->contents == CONTENTS_SOLID)
            {
                return node->children[1];
            }
            if (node->children[1]->contents == CONTENTS_SOLID)
            {
                return node->children[0];
            }

            c_falsenodes++;
        }
        return node;
    }

    //
    // leaf node
    //
    if (node->contents != CONTENTS_SOLID)
    {
        // this node is still inside

        // mark all the nodes used as portals
        for (p = node->portals; p;)
        {
            if (p->onnode)
            {
                p->onnode->valid = 1;
            }
            if (p->nodes[0] == node)                       // only write out from first leaf
            {
                p = p->next[0];
            }
            else
            {
                p = p->next[1];
            }
        }

		MarkFacesInside_r (node);

        return node;
    }


    return node;
}

// =====================================================================================
//  isClassnameAllowableOutside
// =====================================================================================
#define  MAX_ALLOWABLE_OUTSIDE_GROWTH_SIZE 64

unsigned        g_nAllowableOutside = 0;
unsigned        g_maxAllowableOutside = 0;
char**          g_strAllowableOutsideList;

bool            isClassnameAllowableOutside(const char* const classname)
{
    if (g_strAllowableOutsideList)
    {
        unsigned        x;
        char**          list = g_strAllowableOutsideList;

        for (x = 0; x < g_nAllowableOutside; x++, list++)
        {
            if (list)
            {
                if (!strcasecmp(classname, *list))
                {
                    return true;
                }
            }
        }
    }

    return false;
}

// =====================================================================================
//  FreeAllowableOutsideList
// =====================================================================================
void            FreeAllowableOutsideList()
{
    if (g_strAllowableOutsideList)
    {
        free(g_strAllowableOutsideList);
        g_strAllowableOutsideList = NULL;
    }
}

// =====================================================================================
//  LoadAllowableOutsideList
// =====================================================================================
void            LoadAllowableOutsideList(const char* const filename)
{
    char*           fname;
    int             i, x, y;
    char*           pData;
    char*           pszData;

    if (!filename)
    {
        return;
    }
    else
    {
        unsigned        len = strlen(filename) + 5;

        fname = (char*)Alloc(len);
        safe_snprintf(fname, len, "%s", filename);
    }

    if (q_exists(fname))
    {
        if ((i = LoadFile(fname, &pData)))
        {
            Log("Reading allowable void entities from file '%s'\n", fname);
            g_nAllowableOutside = 0;
            for (pszData = pData, y = 0, x = 0; x < i; x++)
            {
                if ((pData[x] == '\n') || (pData[x] == '\r'))
                {
                    pData[x] = 0;
                    if (strlen(pszData))
                    {
                        if (g_nAllowableOutside == g_maxAllowableOutside)
                        {
                            g_maxAllowableOutside += MAX_ALLOWABLE_OUTSIDE_GROWTH_SIZE;
                            g_strAllowableOutsideList =

                                (char**)realloc(g_strAllowableOutsideList, sizeof(char*) * g_maxAllowableOutside);
                        }

                        g_strAllowableOutsideList[y++] = pszData;
                        g_nAllowableOutside++;

                        Verbose("Adding entity '%s' to the allowable void list\n", pszData);
                    }
                    pszData = pData + x + 1;
                }
            }
        }
    }
}

// =====================================================================================
//  FillOutside
// =====================================================================================
// =====================================================================================
//  SurveyLeaks
//      The normal flood stops at the first entity it can see, so one compile
//      reports one hole no matter how many there are. MarkOutsideLeafs has
//      already left one frontier leaf at every hole; each of those gets its
//      own shortest path, and paths that come out of the same gap are merged.
// =====================================================================================
static void     SurveyLeaks ()
{
	const vec_t samehole = 128.0;                          // holes closer than this are one
	const size_t maxsites = 256;                           // bound the per-frontier Dijkstra

	if (g_leakfrontier.empty ())
	{
		return;
	}

	// one representative leaf per entity: an entity can own several leafs
	// along the same opening
	std::vector< int > seen;
	std::vector< node_t * > reps;
	size_t k;
	for (k = 0; k < g_leakfrontier.size (); k++)
	{
		node_t *l = g_leakfrontier[k];
		if (std::find (seen.begin (), seen.end (), l->occupied) != seen.end ())
		{
			continue;
		}
		seen.push_back (l->occupied);
		reps.push_back (l);
	}

	FILE *pts = NULL;
	FILE *lin = NULL;
	OpenLeakFiles (pts, lin);

	for (k = 0; k < reps.size () && g_leaksites.size () < maxsites; k++)
	{
		node_t *l = reps[k];
		vec3_t startpos;
		GetVectorForKey (&g_entities[l->occupied], "origin", startpos);
		if (!BuildShortestLeakTrail (l, startpos))
		{
			continue;
		}

		const vec_t *hole = g_leaktrail[FindHoleIndex ()].p;
		bool duplicate = false;
		size_t j;
		for (j = 0; j < g_leaksites.size (); j++)
		{
			vec3_t d;
			VectorSubtract (g_leaksites[j].hole, hole, d);
			if (VectorLength (d) < samehole)
			{
				duplicate = true;
				break;
			}
		}
		if (duplicate)
		{
			continue;
		}

		leaksite_t site;
		if (!EmitLeakTrail (pts, lin, site.hole))
		{
			continue;
		}
		safe_strncpy (site.classname, ValueForKey (&g_entities[l->occupied], "classname"), sizeof (site.classname));
		VectorCopy (startpos, site.origin);
		g_leaksites.push_back (site);
	}

	fclose (lin);
	fclose (pts);
}

// =====================================================================================
//  PrintLeakSummary
//      One block after every hull has been filled, instead of one warning per
//      leaking hull saying the same thing four times.
// =====================================================================================
void            PrintLeakSummary ()
{
	if (!g_leak_any)
	{
		return;
	}

	char hulls[64];
	hulls[0] = '\0';
	int count = 0;
	int h;
	for (h = 0; h < NUM_HULLS; h++)
	{
		if (g_leak_hulls & (1 << h))
		{
			char tmp[16];
			safe_snprintf (tmp, sizeof (tmp), "%s%d", count? ", ": "", h);
			safe_strncpy (hulls + strlen (hulls), tmp, sizeof (hulls) - strlen (hulls));
			count++;
		}
	}

	Log ("\n  !!! LEAK - map is not sealed (hull%s %s)\n", count == 1? "": "s", hulls);

	if (g_leaksites.size () > 1)
	{
		Log ("    found   %d holes; every one of their paths is in the pointfile\n", (int)g_leaksites.size ());
		size_t i;
		for (i = 0; i < g_leaksites.size (); i++)
		{
			const leaksite_t &site = g_leaksites[i];
			Log ("\n    hole %-3d (%.0f, %.0f, %.0f)\n", (int)(i + 1), site.hole[0], site.hole[1], site.hole[2]);
			Log ("      from  %s @ (%.0f, %.0f, %.0f)\n", site.classname, site.origin[0], site.origin[1], site.origin[2]);
		}
		Log ("\n    action  load the pointfile and seal every marked hole\n");
	}
	else
	{
		Log ("    entity  %s @ (%.0f, %.0f, %.0f)\n", g_leak_entclass, g_leak_entorigin[0], g_leak_entorigin[1], g_leak_entorigin[2]);
		if (g_leak_hashole)
		{
			Log ("    hole    (%.0f, %.0f, %.0f)   marked in the pointfile\n", g_leak_hole[0], g_leak_hole[1], g_leak_hole[2]);
		}
		Log ("    action  load the pointfile in your editor and seal the marked hole\n");
		if (!g_leaksites.empty ())
		{
			Log ("    note    -allleaks surveyed the map and found only this one\n");
		}
	}

	Log ("\n  A LEAK is a hole in the map where the inside is exposed to the outside void.\n"
		 "  The listed entity is where the leak trace starts; follow the pointfile from it to\n"
		 "  the marked hole and seal the gap. Unless the entity is accidentally outside the map,\n"
		 "  do not delete it. Some rotating-object entities need their origin outside the map;\n"
		 "  enclose such an origin brush in a solid world brush.\n\n");
}

node_t*         FillOutside(node_t* node, const bool leakfile, const unsigned hullnum)
{
    int             s;
    int             i;
    bool            inside;
    bool            ret;
    vec3_t          origin;
    const char*     cl;

    Verbose("----- FillOutside ----\n");

    if (g_nofill)
    {
        Log("skipped\n");
        return node;
    }
	if (hullnum == 2 && g_nohull2)
		return node;

    //
    // place markers for all entities so
    // we know if we leak inside
    //
    inside = false;
    for (i = 1; i < g_numentities; i++)
    {
        GetVectorForKey(&g_entities[i], "origin", origin);
        cl = ValueForKey(&g_entities[i], "classname");
        if (!isClassnameAllowableOutside(cl))
        {
            /*if (!VectorCompare(origin, vec3_origin))
			*/ if (*ValueForKey(&g_entities[i], "origin")) //--vluzacn
            {
                origin[2] += 1;                            // so objects on floor are ok

                // nudge playerstart around if needed so clipping hulls allways
                // have a vlaid point
                if (!strcmp(cl, "info_player_start"))
                {
                    int             x, y;

                    for (x = -16; x <= 16; x += 16)
                    {
                        for (y = -16; y <= 16; y += 16)
                        {
                            origin[0] += x;
                            origin[1] += y;
                            if (PlaceOccupant(i, origin, node))
                            {
                                inside = true;
                                goto gotit;
                            }
                            origin[0] -= x;
                            origin[1] -= y;
                        }
                    }
                  gotit:;
                }
                else
                {
                    if (PlaceOccupant(i, origin, node))
                        inside = true;
                }
            }
        }
    }

    if (!inside)
    {
        Warning("No entities exist in hull %i, no filling performed for this hull", hullnum);
        return node;
    }

	if(!g_outside_node.portals)
	{
		Warning("No outside node portal found in hull %i, no filling performed for this hull",hullnum);
		return node;
	}

    s = !(g_outside_node.portals->nodes[1] == &g_outside_node);

    // first check to see if an occupied leaf is hit
    outleafs = 0;
    valid++;

    g_leakleaf = NULL;
    g_haveleaktrail = false;
    g_leaktrail.clear();

    ret = RecursiveFillOutside(g_outside_node.portals->nodes[s], false);

    if (leakfile && ret)
    {
        // Only now, with the leak proved, is the trail worth building - and it
        // is built as the shortest way out rather than the flood's own path.
        GetVectorForKey(&g_entities[hit_occupied], "origin", origin);
        // Learn where the map ends first: the trail needs it to tell the hole
        // apart from the BSP's own subdivision of the void beyond it.
        MarkOutsideLeafs();
        BuildShortestLeakTrail(g_leakleaf, origin);
        WriteLeakFiles();
        // The survey rewrites the same two files with a path for every hole,
        // so it runs after the single trail and simply supersedes it.
        if (g_allleaks)
        {
            SurveyLeaks();
        }
    }

    if (ret)
    {
        GetVectorForKey(&g_entities[hit_occupied], "origin", origin);

        // Collect; PrintLeakSummary shows one block once every hull is done.
        // The first hull to leak owns the pointfile, so it owns the details.
        if (!g_leak_any)
        {
            safe_strncpy(g_leak_entclass, ValueForKey(&g_entities[hit_occupied], "classname"), sizeof(g_leak_entclass));
            VectorCopy(origin, g_leak_entorigin);
            if (g_haveleaktrail)
            {
                VectorCopy(g_leakexit, g_leak_hole);
                g_leak_hashole = true;
            }
        }
        g_leak_any = true;
        if (hullnum < NUM_HULLS)
        {
            g_leak_hulls |= (1 << hullnum);
        }

        if (g_bLeakOnly)
        {
            PrintLeakSummary();
            Error("Stopped by leak.");
        }

        g_bLeaked = true;

        return node;
    }
	if (leakfile && !ret)
	{
		unlink(g_linefilename);
		unlink(g_pointfilename);
	}

    // now go back and fill things in
    valid++;
    RecursiveFillOutside(g_outside_node.portals->nodes[s], true);

    // remove faces and nodes from filled in leafs  
    c_falsenodes = 0;
    c_free_faces = 0;
    c_keep_faces = 0;
    node = ClearOutFaces_r(node);

    Verbose("%5i outleafs\n", outleafs);
    Verbose("%5i freed faces\n", c_free_faces);
    Verbose("%5i keep faces\n", c_keep_faces);
    Verbose("%5i falsenodes\n", c_falsenodes);

    // save portal file for vis tracing
    if ((hullnum == 0) && leakfile)
    {
        WritePortalfile(node);
    }

    return node;
}

void			ResetMark_r (node_t* node)
{
	if (node->isportalleaf)
	{
		if (node->contents == CONTENTS_SOLID || node->contents == CONTENTS_SKY)
		{
			node->empty = 0;
		}
		else
		{
			node->empty = 1;
		}
	}
	else
	{
		ResetMark_r (node->children[0]);
		ResetMark_r (node->children[1]);
	}
}
void			MarkOccupied_r (node_t* node)
{
	if (node->empty == 1)
	{
		node->empty = 0;
		portal_t*       p;
		int             s;
		for (p = node->portals; p; p = p->next[!s])
		{
			s = (p->nodes[0] == node);
			MarkOccupied_r (p->nodes[s]);
		}
	}
}
void			RemoveUnused_r (node_t* node)
{
	if (node->isportalleaf)
	{
		if (node->empty == 1)
		{
			FillLeaf (node);
		}
	}
	else
	{
		RemoveUnused_r (node->children[0]);
		RemoveUnused_r (node->children[1]);
	}
}
void			FillInside (node_t* node)
{
	int i;
	g_outside_node.empty = 0;
	ResetMark_r (node);
    for (i = 1; i < g_numentities; i++)
    {
		if (*ValueForKey(&g_entities[i], "origin"))
		{
			vec3_t origin;
			node_t* innode;
			GetVectorForKey(&g_entities[i], "origin", origin);
			origin[2] += 1;
			innode = PointInLeaf (node, origin);
			MarkOccupied_r (innode);
			origin[2] -= 2;
			innode = PointInLeaf (node, origin);
			MarkOccupied_r (innode);
		}
	}
	RemoveUnused_r (node);
}

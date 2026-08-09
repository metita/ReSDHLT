#include "bsp5.h"

//  WriteClipNodes_r
//  WriteClipNodes
//  WriteDrawLeaf
//  WriteFace
//  WriteDrawNodes_r
//  FreeDrawNodes_r
//  WriteDrawNodes
//  BeginBSPFile
//  FinishBSPFile

#include <algorithm>
#include <map>
#include <vector>

typedef std::map< int, int > PlaneMap;
static PlaneMap gPlaneMap;
static int gNumMappedPlanes;
static dplane_t gMappedPlanes[MAX_MAP_PLANES];
extern bool g_noopt;

typedef std::map< int, int > texinfomap_t;
static int g_nummappedtexinfo;
static texinfo_t g_mappedtexinfo[MAX_MAP_TEXINFO];
static texinfomap_t g_texinfomap;

int count_mergedclipnodes;
typedef std::map< std::pair< int, std::pair< int, int > >, int > clipnodemap_t;
inline clipnodemap_t::key_type MakeKey (const dclipnode_t &c)
{
	return std::make_pair (c.planenum, std::make_pair (c.children[0], c.children[1]));
}

// =====================================================================================
//  WritePlane
//  hook for plane optimization
// =====================================================================================
static int WritePlane(int planenum)
{
	planenum = planenum & (~1);

	if(g_noopt)
	{
		return planenum;
	}

	PlaneMap::iterator item = gPlaneMap.find(planenum);
	if(item != gPlaneMap.end())
	{
		return item->second;
	}
	//add plane to BSP
	hlassume(gNumMappedPlanes < MAX_MAP_PLANES, assume_MAX_MAP_PLANES);
	gMappedPlanes[gNumMappedPlanes] = g_dplanes[planenum];
	gPlaneMap.insert(PlaneMap::value_type(planenum,gNumMappedPlanes));

	return gNumMappedPlanes++;
}

// =====================================================================================
//  WriteTexinfo
// =====================================================================================
static int WriteTexinfo (int texinfo)
{
	if (texinfo < 0 || texinfo >= g_numtexinfo)
	{
		Error ("Bad texinfo number %d.\n", texinfo);
	}

	if (g_noopt)
	{
		return texinfo;
	}

	texinfomap_t::iterator it;
	it = g_texinfomap.find (texinfo);
	if (it != g_texinfomap.end ())
	{
		return it->second;
	}

	int c;
	hlassume (g_nummappedtexinfo < MAX_MAP_TEXINFO, assume_MAX_MAP_TEXINFO);
	c = g_nummappedtexinfo;
	g_mappedtexinfo[g_nummappedtexinfo] = g_texinfo[texinfo];
	g_texinfomap.insert (texinfomap_t::value_type (texinfo, g_nummappedtexinfo));
	g_nummappedtexinfo++;
	return c;
}

// =====================================================================================
//  WriteClipNodes_r
// =====================================================================================
static int      WriteClipNodes_r(node_t* node
								 , const node_t *portalleaf
								 , clipnodemap_t *outputmap
								 )
{
    int             i, c;
    dclipnode_t*    cn;
    int             num;

	if (node->isportalleaf)
	{
		if (node->contents == CONTENTS_SOLID)
		{
			free (node);
			return CONTENTS_SOLID;
		}
		else
		{
			portalleaf = node;
		}
	}
	if (node->planenum == -1)
	{
		if (node->iscontentsdetail)
		{
			num = CONTENTS_SOLID;
		}
		else
		{
			num = portalleaf->contents;
		}
		free (node->markfaces);
		free (node);
		return num;
	}

	dclipnode_t tmpclipnode; // this clipnode will be inserted into g_dclipnodes[c] if it can't be merged
	cn = &tmpclipnode;
	c = g_numclipnodes;
	g_numclipnodes++;
    if (node->planenum & 1)
    {
        Error("WriteClipNodes_r: odd planenum");
    }
    cn->planenum = WritePlane(node->planenum);
    for (i = 0; i < 2; i++)
    {
        cn->children[i] = WriteClipNodes_r(node->children[i]
			, portalleaf
			, outputmap
			);
    }
	clipnodemap_t::iterator output;
	output = outputmap->find (MakeKey (*cn));
	if (g_noclipnodemerge || output == outputmap->end ())
	{
		hlassume (c < MAX_MAP_CLIPNODES, assume_MAX_MAP_CLIPNODES);
		g_dclipnodes[c] = *cn;
		(*outputmap)[MakeKey (*cn)] = c;
	}
	else
	{
		count_mergedclipnodes++;
		if (g_numclipnodes != c + 1)
		{
			Error ("Merge clipnodes: internal error");
		}
		g_numclipnodes = c;
		c = output->second; // use existing clipnode
	}

    free(node);
    return c;
}

// =====================================================================================
//  WriteClipNodes
//      Called after the clipping hull is completed.  Generates a disk format
//      representation and frees the original memory.
// =====================================================================================
void            WriteClipNodes(node_t* nodes)
{
	// we only merge among the clipnodes of the same hull of the same model
	clipnodemap_t outputmap;
    WriteClipNodes_r(nodes
		, NULL
		, &outputmap
		);
}

// =====================================================================================
//  WriteDrawLeaf
// =====================================================================================
static int		WriteDrawLeaf (node_t *node, const node_t *portalleaf)
{
    face_t**        fp;
    face_t*         f;
    dleaf_t*        leaf_p;
	int				leafnum = g_numleafs;

    // emit a leaf
	hlassume (g_numleafs < MAX_MAP_LEAFS, assume_MAX_MAP_LEAFS);
    leaf_p = &g_dleafs[g_numleafs];
    g_numleafs++;

	leaf_p->contents = portalleaf->contents;

    //
    // write bounding box info
    //
	vec3_t mins, maxs;
#if 0
	printf ("leaf isdetail = %d loosebound = (%f,%f,%f)-(%f,%f,%f) portalleaf = (%f,%f,%f)-(%f,%f,%f)\n", node->isdetail,
		node->loosemins[0], node->loosemins[1], node->loosemins[2], node->loosemaxs[0], node->loosemaxs[1], node->loosemaxs[2],
		portalleaf->mins[0], portalleaf->mins[1], portalleaf->mins[2], portalleaf->maxs[0], portalleaf->maxs[1], portalleaf->maxs[2]);
#endif
	if (node->isdetail)
	{
		// intersect its loose bounds with the strict bounds of its parent portalleaf
		VectorCompareMaximum (portalleaf->mins, node->loosemins, mins);
		VectorCompareMinimum (portalleaf->maxs, node->loosemaxs, maxs);
	}
	else
	{
		VectorCopy (node->mins, mins);
		VectorCopy (node->maxs, maxs);
	}
	for (int k = 0; k < 3; k++)
	{
		leaf_p->mins[k] = (short)qmax (-32767, qmin ((int)mins[k], 32767));
		leaf_p->maxs[k] = (short)qmax (-32767, qmin ((int)maxs[k], 32767));
	}

    leaf_p->visofs = -1;                                   // no vis info yet

    //
    // write the marksurfaces
    //
    leaf_p->firstmarksurface = g_nummarksurfaces;

    hlassume(node->markfaces != NULL, assume_EmptySolid);

    for (fp = node->markfaces; *fp; fp++)
    {
        // emit a marksurface
        f = *fp;
        do
        {
			// fix face 0 being seen everywhere
			if (f->outputnumber == -1)
			{
				f = f->original;
				continue;
			}
			bool ishidden = false;
			{
				const char *name = GetTextureByNumber (f->texturenum);
				if (strlen (name) >= 7 && !strcasecmp (&name[strlen (name) - 7], "_HIDDEN"))
				{
					ishidden = true;
				}
			}
			if (ishidden)
			{
				f = f->original;
				continue;
			}
            g_dmarksurfaces[g_nummarksurfaces] = f->outputnumber;
            hlassume(g_nummarksurfaces < MAX_MAP_MARKSURFACES, assume_MAX_MAP_MARKSURFACES);
            g_nummarksurfaces++;
            f = f->original;                               // grab tjunction split faces
        }
        while (f);
    }
    free(node->markfaces);

    leaf_p->nummarksurfaces = g_nummarksurfaces - leaf_p->firstmarksurface;
	return leafnum;
}

// =====================================================================================
//  WriteFace
// =====================================================================================
static void     WriteFace(face_t* f)
{
    dface_t*        df;
    int             i;
    int             e;

    if (    CheckFaceForHint(f)
        ||  CheckFaceForSkip(f)
        ||  CheckFaceForNull(f)  // AJM
		|| CheckFaceForDiscardable (f)
		|| f->texturenum == -1
		|| f->referenced == 0 // this face is not referenced by any nonsolid leaf because it is completely covered by func_details

// =====================================================================================
//Cpt_Andrew - Env_Sky Check
// =====================================================================================
       ||  CheckFaceForEnv_Sky(f)
// =====================================================================================

       )
    {
		f->outputnumber = -1;
        return;
    }

    f->outputnumber = g_numfaces;

    df = &g_dfaces[g_numfaces];
    hlassume(g_numfaces < MAX_MAP_FACES, assume_MAX_MAP_FACES);
    g_numfaces++;

	df->planenum = WritePlane(f->planenum);
	df->side = f->planenum & 1;
    df->firstedge = g_numsurfedges;
    df->numedges = f->numpoints;

	df->texinfo = WriteTexinfo (f->texturenum);

    for (i = 0; i < f->numpoints; i++)
    {
		e = f->outputedges[i];
        hlassume(g_numsurfedges < MAX_MAP_SURFEDGES, assume_MAX_MAP_SURFEDGES);
        g_dsurfedges[g_numsurfedges] = e;
        g_numsurfedges++;
    }
	free (f->outputedges);
	f->outputedges = NULL;
}

// =====================================================================================
//  WriteDrawNodes_r
// =====================================================================================
static int WriteDrawNodes_r (node_t *node, const node_t *portalleaf)
{
	if (node->isportalleaf)
	{
		if (node->contents == CONTENTS_SOLID)
		{
			return -1;
		}
		else
		{
			portalleaf = node;
			// Warning: make sure parent data have not been freed when writing children.
		}
	}
	if (node->planenum == -1)
	{
		if (node->iscontentsdetail)
		{
			free(node->markfaces);
			return -1;
		}
		else
		{
			int leafnum = WriteDrawLeaf (node, portalleaf);
			return -1 - leafnum;
		}
	}
    dnode_t*        n;
    int             i;
    face_t*         f;
	int nodenum = g_numnodes;

    // emit a node
    hlassume(g_numnodes < MAX_MAP_NODES, assume_MAX_MAP_NODES);
    n = &g_dnodes[g_numnodes];
    g_numnodes++;

	vec3_t mins, maxs;
#if 0
	if (node->isdetail || node->isportalleaf)
		printf ("node isdetail = %d loosebound = (%f,%f,%f)-(%f,%f,%f) portalleaf = (%f,%f,%f)-(%f,%f,%f)\n", node->isdetail,
			node->loosemins[0], node->loosemins[1], node->loosemins[2], node->loosemaxs[0], node->loosemaxs[1], node->loosemaxs[2],
			portalleaf->mins[0], portalleaf->mins[1], portalleaf->mins[2], portalleaf->maxs[0], portalleaf->maxs[1], portalleaf->maxs[2]);
#endif
	if (node->isdetail)
	{
		// intersect its loose bounds with the strict bounds of its parent portalleaf
		VectorCompareMaximum (portalleaf->mins, node->loosemins, mins);
		VectorCompareMinimum (portalleaf->maxs, node->loosemaxs, maxs);
	}
	else
	{
		VectorCopy (node->mins, mins);
		VectorCopy (node->maxs, maxs);
	}
	for (int k = 0; k < 3; k++)
	{
		n->mins[k] = (short)qmax (-32767, qmin ((int)mins[k], 32767));
		n->maxs[k] = (short)qmax (-32767, qmin ((int)maxs[k], 32767));
	}

    if (node->planenum & 1)
    {
        Error("WriteDrawNodes_r: odd planenum");
    }
    n->planenum = WritePlane(node->planenum);
    n->firstface = g_numfaces;

    for (f = node->faces; f; f = f->next)
    {
        WriteFace(f);
    }

    n->numfaces = g_numfaces - n->firstface;

    //
    // recursively output the other nodes
    //
    for (i = 0; i < 2; i++)
    {
		n->children[i] = WriteDrawNodes_r (node->children[i], portalleaf);
    }
	return nodenum;
}

// =====================================================================================
//  FreeDrawNodes_r
// =====================================================================================
static void     FreeDrawNodes_r(node_t* node)
{
    int             i;
    face_t*         f;
    face_t*         next;

    for (i = 0; i < 2; i++)
    {
        if (node->children[i]->planenum != -1)
        {
            FreeDrawNodes_r(node->children[i]);
        }
    }

    //
    // free the faces on the node
    //
    for (f = node->faces; f; f = next)
    {
        next = f->next;
        FreeFace(f);
    }

    free(node);
}

// =====================================================================================
//  WriteDrawNodes
//      Called after a drawing hull is completed
//      Frees all nodes and faces
// =====================================================================================
void OutputEdges_face (face_t *f)
{
	if (CheckFaceForHint(f)
		|| CheckFaceForSkip(f)
        || CheckFaceForNull(f)  // AJM
		|| CheckFaceForDiscardable (f)
		|| f->texturenum == -1
		|| f->referenced == 0
		|| CheckFaceForEnv_Sky(f)//Cpt_Andrew - Env_Sky Check
		)
	{
		return;
	}
	f->outputedges = (int *)malloc (f->numpoints * sizeof (int));
	hlassume (f->outputedges != NULL, assume_NoMemory);
	int i;
	for (i = 0; i < f->numpoints; i++)
	{
		int e = GetEdge (f->pts[i], f->pts[(i + 1) % f->numpoints], f);
		f->outputedges[i] = e;
	}
}
int OutputEdges_r (node_t *node, int detaillevel)
{
	int next = -1;
	if (node->planenum == -1)
	{
		return next;
	}
	face_t *f;
	for (f = node->faces; f; f = f->next)
	{
		if (f->detaillevel > detaillevel)
		{
			if (next == -1? true: f->detaillevel < next)
			{
				next = f->detaillevel;
			}
		}
		if (f->detaillevel == detaillevel)
		{
			OutputEdges_face (f);
		}
	}
	int i;
	for (i = 0; i < 2; i++)
	{
		int r = OutputEdges_r (node->children[i], detaillevel);
		if (r == -1? false: next == -1? true: r < next)
		{
			next = r;
		}
	}
	return next;
}
static void RemoveCoveredFaces_r (node_t *node)
{
	if (node->isportalleaf)
	{
		if (node->contents == CONTENTS_SOLID)
		{
			return; // stop here, don't go deeper into children
		}
	}
	if (node->planenum == -1)
	{
		// this is a leaf
		if (node->iscontentsdetail)
		{
			return;
		}
		else
		{
			face_t **fp;
			for (fp = node->markfaces; *fp; fp++)
			{
				for (face_t *f = *fp; f; f = f->original) // for each tjunc subface
				{
					f->referenced++; // mark the face as referenced
				}
			}
		}
		return;
	}
	
	// this is a node
	for (face_t *f = node->faces; f; f = f->next)
	{
		f->referenced = 0; // clear the mark
	}

	RemoveCoveredFaces_r (node->children[0]);
	RemoveCoveredFaces_r (node->children[1]);
}
void            WriteDrawNodes(node_t* headnode)
{
	RemoveCoveredFaces_r (headnode); // fill "referenced" value
	// higher detail level should not compete for edge pairing with lower detail level.
	int detaillevel, nextdetaillevel;
	for (detaillevel = 0; detaillevel != -1; detaillevel = nextdetaillevel)
	{
		nextdetaillevel = OutputEdges_r (headnode, detaillevel);
	}
	WriteDrawNodes_r (headnode, NULL);
}


// =====================================================================================
//  BeginBSPFile
// =====================================================================================
void            BeginBSPFile()
{
    // these values may actually be initialized
    // if the file existed when loaded, so clear them explicitly
	gNumMappedPlanes = 0;
	gPlaneMap.clear();

	g_nummappedtexinfo = 0;
	g_texinfomap.clear ();

	count_mergedclipnodes = 0;
    g_nummodels = 0;
    g_numfaces = 0;
    g_numnodes = 0;
    g_numclipnodes = 0;
    g_numvertexes = 0;
    g_nummarksurfaces = 0;
    g_numsurfedges = 0;

    // edge 0 is not used, because 0 can't be negated
    g_numedges = 1;

    // leaf 0 is common solid with no faces
    g_numleafs = 1;
    g_dleafs[0].contents = CONTENTS_SOLID;
}

// =====================================================================================
//  OptimizeFaceOrder
//      The engine packs face lightmaps into its 64 atlas pages in face order,
//      first fit, and never backtracks. That makes the order the faces happen
//      to be written in worth a few pages on a full map: feeding the allocator
//      the big rectangles first leaves less unusable space behind the small
//      ones.
//
//      Only the order changes. Geometry, texture scale and lightmap resolution
//      are untouched, and faces stay inside the node range that owns them, so
//      the tree is still valid; node->firstface and the marksurface table are
//      remapped to follow. Three legal orders are tried against the engine's
//      own allocator and the one needing the fewest pages wins, so this can
//      never make a map worse than it already was.
//
//      Off by default (-lmoptimize): the default build stays byte-identical to
//      what previous versions produced.
// =====================================================================================
typedef struct
{
	bool			lightmapped;
	long long		area;
	int				longest;
	int				shortest;
	int				width;
	int				height;
}
faceatlassize_t;

static int CountAtlasPages (const std::vector< int > &order, const std::vector< faceatlassize_t > &sizes)
{
	// The engine's allocator, exactly: a skyline per page, first fit, lowest
	// resulting height wins. The "< BLOCK_WIDTH - width" bound is Quake's own
	// off-by-one and is deliberately reproduced.
	const int BLOCK_WIDTH = 128;
	const int BLOCK_HEIGHT = 128;
	std::vector< std::vector< int > > pages;
	size_t k;
	for (k = 0; k < order.size (); k++)
	{
		const faceatlassize_t &size = sizes[order[k]];
		if (!size.lightmapped)
		{
			continue;
		}
		if (size.width < 1 || size.height < 1 || size.width >= BLOCK_WIDTH || size.height > BLOCK_HEIGHT)
		{
			continue; // CountBlocks warns about these; they never reach the atlas
		}
		bool placed = false;
		size_t p;
		for (p = 0; p < pages.size () && !placed; p++)
		{
			std::vector< int > &page = pages[p];
			int best = BLOCK_HEIGHT;
			int bestx = 0;
			int i, j;
			for (i = 0; i < BLOCK_WIDTH - size.width; i++)
			{
				int height = 0;
				for (j = 0; j < size.width; j++)
				{
					if (page[i + j] >= best)
						break;
					if (page[i + j] > height)
						height = page[i + j];
				}
				if (j == size.width)
				{
					bestx = i;
					best = height;
				}
			}
			if (best + size.height > BLOCK_HEIGHT)
			{
				continue;
			}
			for (i = 0; i < size.width; i++)
			{
				page[bestx + i] = best + size.height;
			}
			placed = true;
		}
		if (!placed)
		{
			pages.push_back (std::vector< int > (BLOCK_WIDTH, 0));
			std::vector< int > &page = pages.back ();
			int i;
			for (i = 0; i < size.width; i++)
			{
				page[i] = size.height;
			}
		}
	}
	return (int)pages.size ();
}

static void OptimizeFaceOrder ()
{
	if (g_numfaces < 2)
	{
		return;
	}
	if (g_numfaces > 65535)
	{
		// node->firstface is an unsigned short; rewriting it would truncate.
		return;
	}

	std::vector< faceatlassize_t > sizes ((size_t)g_numfaces);
	int i;
	for (i = 0; i < g_numfaces; i++)
	{
		dface_t *f = &g_dfaces[i];
		int texinfo = ParseTexinfoForFace (f);
		if (texinfo < 0 || texinfo >= g_numtexinfo)
		{
			return; // not something we understand; leave the order alone
		}
		const char *name = GetTextureByNumber (texinfo);
		faceatlassize_t &size = sizes[i];
		size.lightmapped = strncmp (name, "sky", 3)
			&& name[0] != '!'
			&& strncasecmp (name, "water", 5)
			&& strncasecmp (name, "laser", 5)
			&& !(g_texinfo[texinfo].flags & TEX_SPECIAL);
		int mins[2];
		int maxs[2];
		GetFaceExtents (i, mins, maxs);
		size.width = maxs[0] - mins[0] + 1;
		size.height = maxs[1] - mins[1] + 1;
		size.area = (long long)size.width * size.height;
		size.longest = qmax (size.width, size.height);
		size.shortest = qmin (size.width, size.height);
	}

	// Candidate 1: what we would have written anyway.
	std::vector< int > original ((size_t)g_numfaces);
	for (i = 0; i < g_numfaces; i++)
	{
		original[i] = i;
	}

	// Candidate 2: biggest first inside each node's own face range.
	std::vector< int > sorted = original;
	for (i = 0; i < g_numnodes; i++)
	{
		size_t first = g_dnodes[i].firstface;
		size_t count = g_dnodes[i].numfaces;
		if (count < 2)
		{
			continue;
		}
		if (first > (size_t)g_numfaces || count > (size_t)g_numfaces - first)
		{
			return;
		}
		std::stable_sort (sorted.begin () + first, sorted.begin () + first + count,
			[&sizes](int a, int b)
			{
				const faceatlassize_t &l = sizes[a];
				const faceatlassize_t &r = sizes[b];
				if (l.lightmapped != r.lightmapped) return l.lightmapped > r.lightmapped;
				if (l.area != r.area) return l.area > r.area;
				if (l.longest != r.longest) return l.longest > r.longest;
				return l.shortest > r.shortest;
			});
	}

	// Candidate 3: candidate 2, plus the node blocks themselves reordered
	// biggest-first inside each model. This one moves node->firstface, so the
	// original values are kept to put back if it loses.
	std::vector< int > grouped = sorted;
	std::vector< unsigned short > firstfaces ((size_t)g_numnodes);
	for (i = 0; i < g_numnodes; i++)
	{
		firstfaces[i] = g_dnodes[i].firstface;
	}
	bool groupedvalid = true;
	{
		typedef struct { int node; size_t first; size_t count; long long area; } nodegroup_t;
		int m;
		for (m = 0; m < g_nummodels && groupedvalid; m++)
		{
			const dmodel_t *model = &g_dmodels[m];
			if (model->firstface < 0 || model->numfaces < 1)
			{
				continue;
			}
			size_t modelfirst = (size_t)model->firstface;
			size_t modelcount = (size_t)model->numfaces;
			if (modelfirst > (size_t)g_numfaces || modelcount > (size_t)g_numfaces - modelfirst)
			{
				groupedvalid = false;
				break;
			}
			if (modelcount < 2)
			{
				continue;
			}
			std::vector< unsigned char > covered (modelcount, 0);
			std::vector< nodegroup_t > groups;
			for (i = 0; i < g_numnodes; i++)
			{
				size_t first = g_dnodes[i].firstface;
				size_t count = g_dnodes[i].numfaces;
				if (!count || first < modelfirst || first >= modelfirst + modelcount)
				{
					continue;
				}
				if (count > modelfirst + modelcount - first)
				{
					groupedvalid = false; // a node straddling two models
					break;
				}
				nodegroup_t group;
				group.node = i;
				group.first = first;
				group.count = count;
				group.area = 0;
				size_t k;
				for (k = first; k < first + count; k++)
				{
					if (covered[k - modelfirst])
					{
						groupedvalid = false; // two nodes claim the same face
						break;
					}
					covered[k - modelfirst] = 1;
					group.area += sizes[grouped[k]].area;
				}
				if (!groupedvalid)
				{
					break;
				}
				groups.push_back (group);
			}
			if (!groupedvalid)
			{
				break;
			}
			size_t k;
			for (k = 0; k < modelcount; k++)
			{
				if (!covered[k])
				{
					groupedvalid = false; // a face no node owns: not ours to move
					break;
				}
			}
			if (!groupedvalid)
			{
				break;
			}
			std::stable_sort (groups.begin (), groups.end (),
				[](const nodegroup_t &l, const nodegroup_t &r) { return l.area > r.area; });
			std::vector< int > reordered;
			reordered.reserve (modelcount);
			size_t next = modelfirst;
			for (k = 0; k < groups.size (); k++)
			{
				g_dnodes[groups[k].node].firstface = (unsigned short)next;
				reordered.insert (reordered.end (), grouped.begin () + groups[k].first,
					grouped.begin () + groups[k].first + groups[k].count);
				next += groups[k].count;
			}
			if (reordered.size () != modelcount)
			{
				groupedvalid = false;
				break;
			}
			std::copy (reordered.begin (), reordered.end (), grouped.begin () + modelfirst);
		}
	}

	const std::vector< int > *best = &original;
	int bestpages = CountAtlasPages (original, sizes);
	const int originalpages = bestpages;
	int pages = CountAtlasPages (sorted, sizes);
	if (pages < bestpages)
	{
		bestpages = pages;
		best = &sorted;
	}
	if (groupedvalid)
	{
		pages = CountAtlasPages (grouped, sizes);
		if (pages < bestpages)
		{
			bestpages = pages;
			best = &grouped;
		}
	}
	if (best != &grouped)
	{
		for (i = 0; i < g_numnodes; i++) // undo candidate 3's firstface writes
		{
			g_dnodes[i].firstface = firstfaces[i];
		}
	}
	if (best == &original)
	{
		Log ("Lightmap atlas order: %d pages, already the best of the orders tried\n", originalpages);
		return;
	}

	for (i = 0; i < g_nummarksurfaces; i++)
	{
		if (g_dmarksurfaces[i] >= (unsigned short)g_numfaces)
		{
			return; // bogus reference: check before anything is rewritten
		}
	}
	std::vector< int > newforold ((size_t)g_numfaces);
	std::vector< dface_t > reordered ((size_t)g_numfaces);
	for (i = 0; i < g_numfaces; i++)
	{
		int old = (*best)[i];
		reordered[i] = g_dfaces[old];
		newforold[old] = i;
	}
	for (i = 0; i < g_nummarksurfaces; i++)
	{
		g_dmarksurfaces[i] = (unsigned short)newforold[g_dmarksurfaces[i]];
	}
	for (i = 0; i < g_numfaces; i++)
	{
		g_dfaces[i] = reordered[i];
	}
	Log ("Lightmap atlas order: %d pages, down from %d\n", bestpages, originalpages);
}

// =====================================================================================
//  FinishBSPFile
// =====================================================================================
void            FinishBSPFile()
{
    Verbose("--- FinishBSPFile ---\n");

	if (g_dmodels[0].visleafs > MAX_MAP_LEAFS_ENGINE)
	{
		Warning ("Number of world leaves(%d) exceeded MAX_MAP_LEAFS(%d)\nIf you encounter problems when running your map, consider this the most likely cause.\n", g_dmodels[0].visleafs, MAX_MAP_LEAFS_ENGINE);
	}
	if (g_dmodels[0].numfaces > MAX_MAP_WORLDFACES)
	{
		Warning ("Number of world faces(%d) exceeded %d. Some faces will disappear in game.\nTo reduce world faces, change some world brushes (including func_details) to func_walls.\n", g_dmodels[0].numfaces, MAX_MAP_WORLDFACES);
	}
	Developer (DEVELOPER_LEVEL_MESSAGE, "count_mergedclipnodes = %d\n", count_mergedclipnodes);
	if (!g_noclipnodemerge)
	{
		Log ("Reduced %d clipnodes to %d\n", g_numclipnodes + count_mergedclipnodes, g_numclipnodes);
	}
	if(!g_noopt)
	{
		{
			Log ("Reduced %d texinfos to %d\n", g_numtexinfo, g_nummappedtexinfo);
			for (int i = 0; i < g_nummappedtexinfo; i++)
			{
				g_texinfo[i] = g_mappedtexinfo[i];
			}
			g_numtexinfo = g_nummappedtexinfo;
		}
		{
			dmiptexlump_t *l = (dmiptexlump_t *)g_dtexdata;
			int &g_nummiptex = l->nummiptex;
			bool *Used = (bool *)calloc (g_nummiptex, sizeof(bool));
			int Num = 0, Size = 0;
			int *Map = (int *)malloc (g_nummiptex * sizeof(int));
			int i;
			hlassume (Used != NULL && Map != NULL, assume_NoMemory);
			int *lumpsizes = (int *)malloc (g_nummiptex * sizeof (int));
			const int newdatasizemax = g_texdatasize - ((byte *)&l->dataofs[g_nummiptex] - (byte *)l);
			byte *newdata = (byte *)malloc (newdatasizemax);
			int newdatasize = 0;
			hlassume (lumpsizes != NULL && newdata != NULL, assume_NoMemory);
			int total = 0;
			for (i = 0; i < g_nummiptex; i++)
			{
				if (l->dataofs[i] == -1)
				{
					lumpsizes[i] = -1;
					continue;
				}
				lumpsizes[i] = g_texdatasize - l->dataofs[i];
				for (int j = 0; j < g_nummiptex; j++)
				{
					int lumpsize = l->dataofs[j] - l->dataofs[i];
					if (l->dataofs[j] == -1 || lumpsize < 0 || lumpsize == 0 && j <= i)
						continue;
					if (lumpsize < lumpsizes[i])
						lumpsizes[i] = lumpsize;
				}
				total += lumpsizes[i];
			}
			if (total != newdatasizemax)
			{
				Warning ("Bad texdata structure.\n");
				goto skipReduceTexdata;
			}
			for (i = 0; i < g_numtexinfo; i++)
			{
				texinfo_t *t = &g_texinfo[i];
				if (t->miptex < 0 || t->miptex >= g_nummiptex)
				{
					Warning ("Bad miptex number %d.\n", t->miptex);
					goto skipReduceTexdata;
				}
				Used[t->miptex] = true;
			}
			for (i = 0; i < g_nummiptex; i++)
			{
				const int MAXWADNAME = 16;
				char name[MAXWADNAME];
				int j, k;
				if (l->dataofs[i] < 0)
					continue;
				if (Used[i] == true)
				{
					miptex_t *m = (miptex_t *)((byte *)l + l->dataofs[i]);
					if (m->name[0] != '+' && m->name[0] != '-')
						continue;
					safe_strncpy (name, m->name, MAXWADNAME);
					if (name[1] == '\0')
						continue;
					for (j = 0; j < 20; j++)
					{
						if (j < 10)
							name[1] = '0' + j;
						else
							name[1] = 'A' + j - 10;
						for (k = 0; k < g_nummiptex; k++)
						{
							if (l->dataofs[k] < 0)
								continue;
							miptex_t *m2 = (miptex_t *)((byte *)l + l->dataofs[k]);
							if (!strcasecmp (name, m2->name))
								Used[k] = true;
						}
					}
				}
			}
			for (i = 0; i < g_nummiptex; i++)
			{
				if (Used[i])
				{
					Map[i] = Num;
					Num++;
				}
				else
				{
					Map[i] = -1;
				}
			}
			for (i = 0; i < g_numtexinfo; i++)
			{
				texinfo_t *t = &g_texinfo[i];
				t->miptex = Map[t->miptex];
			}
			Size += (byte *)&l->dataofs[Num] - (byte *)l;
			for (i = 0; i < g_nummiptex; i++)
			{
				if (Used[i])
				{
					if (lumpsizes[i] == -1)
					{
						l->dataofs[Map[i]] = -1;
					}
					else
					{
						memcpy ((byte *)newdata + newdatasize, (byte *)l + l->dataofs[i], lumpsizes[i]);
						l->dataofs[Map[i]] = Size;
						newdatasize += lumpsizes[i];
						Size += lumpsizes[i];
					}
				}
			}
			memcpy (&l->dataofs[Num], newdata, newdatasize);
			Log ("Reduced %d texdatas to %d (%d bytes to %d)\n", g_nummiptex, Num, g_texdatasize, Size);
			g_nummiptex = Num;
			g_texdatasize = Size;
			skipReduceTexdata:;
			free (lumpsizes);
			free (newdata);
			free (Used);
			free (Map);
		}
		Log ("Reduced %d planes to %d\n", g_numplanes, gNumMappedPlanes);

		for(int counter = 0; counter < gNumMappedPlanes; counter++)
		{
			g_dplanes[counter] = gMappedPlanes[counter];
		}
		g_numplanes = gNumMappedPlanes;
	}
	else
	{
		hlassume (g_numtexinfo < MAX_MAP_TEXINFO, assume_MAX_MAP_TEXINFO);
		hlassume (g_numplanes < MAX_MAP_PLANES, assume_MAX_MAP_PLANES);
	}

	if (!g_nobrink)
	{
		Log ("FixBrinks:\n");
		dclipnode_t *clipnodes; //[MAX_MAP_CLIPNODES]
		int numclipnodes;
		clipnodes = (dclipnode_t *)malloc (MAX_MAP_CLIPNODES * sizeof (dclipnode_t));
		hlassume (clipnodes != NULL, assume_NoMemory);
		void *(*brinkinfo)[NUM_HULLS]; //[MAX_MAP_MODELS]
		int (*headnode)[NUM_HULLS]; //[MAX_MAP_MODELS]
		brinkinfo = (void *(*)[NUM_HULLS])malloc (MAX_MAP_MODELS * sizeof (void *[NUM_HULLS]));
		hlassume (brinkinfo != NULL, assume_NoMemory);
		headnode = (int (*)[NUM_HULLS])malloc (MAX_MAP_MODELS * sizeof (int [NUM_HULLS]));
		hlassume (headnode != NULL, assume_NoMemory);

		int i, j, level;
		for (i = 0; i < g_nummodels; i++)
		{
			dmodel_t *m = &g_dmodels[i];
			Developer (DEVELOPER_LEVEL_MESSAGE, " model %d\n", i);
			for (j = 1; j < NUM_HULLS; j++)
			{
				brinkinfo[i][j] = CreateBrinkinfo (g_dclipnodes, m->headnode[j]);
			}
		}
		for (level = BrinkAny; level > BrinkNone; level--)
		{
			numclipnodes = 0;
			count_mergedclipnodes = 0;
			for (i = 0; i < g_nummodels; i++)
			{
				for (j = 1; j < NUM_HULLS; j++)
				{
					if (!FixBrinks (brinkinfo[i][j], (bbrinklevel_e) level, headnode[i][j], clipnodes, MAX_MAP_CLIPNODES, numclipnodes, numclipnodes))
					{
						break;
					}
				}
				if (j < NUM_HULLS)
				{
					break;
				}
			}
			if (i == g_nummodels)
			{
				break;
			}
		}
		for (i = 0; i < g_nummodels; i++)
		{
			for (j = 1; j < NUM_HULLS; j++)
			{
				DeleteBrinkinfo (brinkinfo[i][j]);
			}
		}
		if (level == BrinkNone)
		{
			Warning ("No brinks have been fixed because clipnode data is almost full.");
		}
		else
		{
			if (level != BrinkAny)
			{
				Warning ("Not all brinks have been fixed because clipnode data is almost full.");
			}
			Developer (DEVELOPER_LEVEL_MESSAGE, "count_mergedclipnodes = %d\n", count_mergedclipnodes);
			Log ("Increased %d clipnodes to %d.\n", g_numclipnodes, numclipnodes);
			g_numclipnodes = numclipnodes;
			memcpy (g_dclipnodes, clipnodes, numclipnodes * sizeof (dclipnode_t));
			for (i = 0; i < g_nummodels; i++)
			{
				dmodel_t *m = &g_dmodels[i];
				for (j = 1; j < NUM_HULLS; j++)
				{
					m->headnode[j] = headnode[i][j];
				}
			}
		}
		free (brinkinfo);
		free (headnode);
		free (clipnodes);
	}
	
	if (g_lmoptimize)
	{
		OptimizeFaceOrder ();
	}

#ifdef PLATFORM_CAN_CALC_EXTENT
	WriteExtentFile (g_extentfilename);
#else
	Warning ("The " PLATFORM_VERSIONSTRING " version of hlbsp couldn't create extent file. The lack of extent file may cause hlrad error.");
#endif
	if (g_chart)
    {
        PrintBSPFileSizes();
    }

#undef dplane_t // this allow us to temporarily access the raw data directly without the layer of indirection
#undef g_dplanes
	for (int i = 0; i < g_numplanes; i++)
	{
		plane_t *mp = &g_mapplanes[i];
		dplane_t *dp = &g_dplanes[i];
		VectorCopy (mp->normal, dp->normal);
		dp->dist = mp->dist;
		dp->type = mp->type;
	}
#define dplane_t plane_t
#define g_dplanes g_mapplanes
    WriteBSPFile(g_bspfilename);
}

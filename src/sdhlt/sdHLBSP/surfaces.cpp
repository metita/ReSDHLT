#include "bsp5.h"
#include <climits>
#include <unordered_map>
#include <vector>

//  SubdivideFace

//  InitHash
//  HashVec

//  GetVertex
//  GetEdge
//  MakeFaceEdges

static int      subdivides;

/* a surface has all of the faces that could be drawn on a given plane
   the outside filling stage can remove some of them so a better bsp can be generated */

// =====================================================================================
//  GridSubdivideCut
//      -gridsubdivide: decides whether a face still needs a cut along one texture axis,
//      and where.
//
//      The engine sizes a lightmap from the luxel cells the face touches,
//      ceil(max / 16) - floor(min / 16), and refuses the map past 16 of them
//      ("Bad surface extents"). The classic rule never looks at the cells: it cuts
//      every 224 units and keeps anything up to 240, which is safe wherever the face
//      sits on the grid but wastes up to two cells per piece. A 256 unit wall that
//      starts on a cell boundary fits exactly, and still came out as 224 + 32.
//
//      Here the cells are counted the way the engine will count them, and the face is
//      only cut when they exceed 16. Each end of the face gets a safety margin unless
//      its position is certain: the texture axis follows one world axis and the vertex
//      sits on an integer coordinate, which no later stage moves (GetVertex snaps to
//      integers, splits along such an edge keep the coordinate, t-junction fixes add
//      points on the same line). The cut goes on a cell boundary when the new vertices
//      land on integers too, so no cell is shared by two pieces; otherwise through the
//      middle of a cell, which costs one cell and is safe against any rounding.
//
//      Returns false when the face fits. *cut is in texture units, offset included.
// =====================================================================================
static bool     GridSubdivideCut(const face_t* const f, const texinfo_t* const tex, const int axis, vec_t* const cut)
{
    const float*    vecs = tex->vecs[axis];
    int             k = -1;                                // world axis the texture axis follows
    int             nonzero = 0;
    int             i, j;

    for (j = 0; j < 3; j++)
    {
        if (vecs[j] != 0)
        {
            nonzero++;
            k = j;
        }
    }
    const bool      axial = nonzero == 1;
    const double    l1 = fabs((double)vecs[0]) + fabs((double)vecs[1]) + fabs((double)vecs[2]);
    // Welding can move a vertex 0.02 on each axis and a t-junction point can sit a
    // little off its edge. Capped under half a cell so a mid-cell cut stays valid.
    const double    loose = qmin(4.0, 0.1 + 0.1 * l1);

    double          vmin = 0, vmax = 0;
    int             lo = INT_MAX, hi = INT_MIN;

    for (i = 0; i < f->numpoints; i++)
    {
        float           p[3];
        bool            integer = true;

        for (j = 0; j < 3; j++)
        {
            // what GetVertex will store
            const double rounded = floor(f->pts[i][j] + 0.5);
            const bool snaps = fabs(f->pts[i][j] - rounded) < 0.001;
            p[j] = (float)(snaps ? rounded : f->pts[i][j]);
            if (j == k && !snaps)
            {
                integer = false;
            }
        }
        // the engine's arithmetic
        const double    val = CalculatePointVecsProduct(p, vecs);
        const double    margin = (axial && integer) ? 0.0 : loose;
        const int       cellmin = (int)floor((val - margin) / TEXTURE_STEP);
        const int       cellmax = (int)ceil((val + margin) / TEXTURE_STEP);

        if (i == 0 || val < vmin)
        {
            vmin = val;
        }
        if (i == 0 || val > vmax)
        {
            vmax = val;
        }
        lo = qmin(lo, cellmin);
        hi = qmax(hi, cellmax);
    }

    if (hi - lo <= MAX_SURFACE_EXTENT)
    {
        return false;
    }

    // First choice: the boundary 16 cells in, if the vertices it creates are certain.
    if (axial)
    {
        const double    boundary = (double)(lo + MAX_SURFACE_EXTENT) * TEXTURE_STEP;
        const double    world = (boundary - (double)vecs[3]) / (double)vecs[k];
        const double    rounded = floor(world + 0.5);

        if (fabs(world - rounded) < 1e-9 && fabs(rounded) < 65536
            && (double)(float)((double)(float)rounded * (double)vecs[k] + (double)vecs[3]) == boundary
            && boundary > vmin + 1 && boundary < vmax - 1)
        {
            *cut = boundary;
            return true;
        }
    }
    // Otherwise the middle of the 16th cell: both pieces count that cell.
    *cut = (double)(lo + MAX_SURFACE_EXTENT - 1) * TEXTURE_STEP + TEXTURE_STEP / 2.0;
    if (*cut <= vmin + 1 || *cut >= vmax - 1)
    {
        // Margins alone pushed it over; nothing sensible to cut.
        return false;
    }
    return true;
}

// =====================================================================================
//  SubdivideFace
//      If the face is >256 in either texture direction, carve a valid sized
//      piece off and insert the remainder in the next link
// =====================================================================================
void            SubdivideFace(face_t* f, face_t** prevptr)
{
    vec_t           mins, maxs;
    vec_t           v;
    int             axis;
    int             i;
    dplane_t        plane;
    face_t*         front;
    face_t*         back;
    face_t*         next;
    texinfo_t*      tex;
    vec3_t          temp;

    // special (non-surface cached) faces don't need subdivision

	if (f->texturenum == -1)
	{
		return;
	}
    tex = &g_texinfo[f->texturenum];

    if (tex->flags & TEX_SPECIAL) 
    {
        return;
    }

    if (f->facestyle == face_hint)
    {
        return;
    }
    if (f->facestyle == face_skip)
    {
        return;
    }

    if (f->facestyle == face_null)
        return; // ideally these should have their tex_special flag set, so its here jic
	if (f->facestyle == face_discardable)
		return;
	
    for (axis = 0; axis < 2; axis++)
    {
        while (g_gridsubdivide)
        {
            vec_t           cut;

            if (!GridSubdivideCut(f, tex, axis, &cut))
            {
                break;
            }
            subdivides++;

            VectorCopy(tex->vecs[axis], temp);
            v = VectorNormalize(temp);

            VectorCopy(temp, plane.normal);
            plane.dist = (cut - tex->vecs[axis][3]) / v;
            next = f->next;
            SplitFace(f, &plane, &front, &back);
            if (!front || !back)
            {
                // Not cut after all; leave the rest of this axis to the classic rule.
                f = next;
                if (front)
                {
                    front->next = f;
                    f = front;
                }
                if (back)
                {
                    back->next = f;
                    f = back;
                }
                *prevptr = f;
                break;
            }
            front->next = next;
            back->next = front;
            f = back;
            *prevptr = f;
        }
        while (!g_gridsubdivide)
        {
			mins = 99999999;
			maxs = -99999999;

            for (i = 0; i < f->numpoints; i++)
            {
                v = DotProduct(f->pts[i], tex->vecs[axis]);
                if (v < mins)
                {
                    mins = v;
                }
                if (v > maxs)
                {
                    maxs = v;
                }
            }

            if ((maxs - mins) <= g_subdivide_size)
            {
                break;
            }
                
            // split it
            subdivides++;

            VectorCopy(tex->vecs[axis], temp);
            v = VectorNormalize(temp);

            VectorCopy(temp, plane.normal);
            plane.dist = (mins + g_subdivide_size - TEXTURE_STEP) / v; //plane.dist = (mins + g_subdivide_size - 16) / v; //--vluzacn
            next = f->next;
            SplitFace(f, &plane, &front, &back);
            if (!front || !back)
            {
                Developer(DEVELOPER_LEVEL_SPAM, "SubdivideFace: didn't split the %d-sided polygon @(%.0f,%.0f,%.0f)",
                        f->numpoints, f->pts[0][0], f->pts[0][1], f->pts[0][2]);
            }
			f = next;
			if (front)
			{
				front->next = f;
				f = front;
			}
			if (back)
			{
				back->next = f;
				f = back;
			}
			*prevptr = f;
        }
    }
}

//===========================================================================

typedef struct hashvert_s
{
    struct hashvert_s* next;
    vec3_t          point;
    int             num;
    int             numplanes;                             // for corner determination
    int             planenums[2];
    int             numedges;
}
hashvert_t;

// #define      POINT_EPSILON   0.01
#define POINT_EPSILON	(ON_EPSILON / 2) //#define POINT_EPSILON	ON_EPSILON //--vluzacn

static hashvert_t hvertex[MAX_MAP_VERTS];
static hashvert_t* hvert_p;

static face_t*  edgefaces[MAX_MAP_EDGES][2];
static int      firstmodeledge = 1;
static int      firstmodelface;

//============================================================================

#define	NUM_HASH	4096

static hashvert_t* hashverts[NUM_HASH];

static vec3_t   hash_min;
static vec3_t   hash_scale;
// It's okay if the coordinates go under hash_min, because they are hashed in a cyclic way (modulus by hash_numslots)
// So please don't change the hardcoded hash_min and scale
static int		hash_numslots[3];
#define MAX_HASH_NEIGHBORS	4

// =====================================================================================
//  InitHash
// =====================================================================================
static void     InitHash()
{
    vec3_t          size;
    vec_t           volume;
    vec_t           scale;
    int             i;

    memset(hashverts, 0, sizeof(hashverts));

    for (i = 0; i < 3; i++)
    {
        hash_min[i] = -8000;
        size[i] = 16000;
    }

    volume = size[0] * size[1];

    scale = sqrt(volume / NUM_HASH);

	hash_numslots[0] = (int)floor (size[0] / scale);
	hash_numslots[1] = (int)floor (size[1] / scale);
	while (hash_numslots[0] * hash_numslots[1] > NUM_HASH)
	{
		Developer (DEVELOPER_LEVEL_WARNING, "hash_numslots[0] * hash_numslots[1] > NUM_HASH");
		hash_numslots[0]--;
		hash_numslots[1]--;
	}

	hash_scale[0] = hash_numslots[0] / size[0];
	hash_scale[1] = hash_numslots[1] / size[1];

    hvert_p = hvertex;
}

// =====================================================================================
//  HashVec
// =====================================================================================
static int HashVec (const vec3_t vec, int *num_hashneighbors, int *hashneighbors)
	// returned value: the one bucket that a new vertex may "write" into
	// returned hashneighbors: the buckets that we should "read" to check for an existing vertex
{
	int h;
	int i;
	int x;
	int y;
	int slot[2];
	vec_t normalized[2];
	vec_t slotdiff[2];

	for (i = 0; i < 2; i++)
	{
		normalized[i] = hash_scale[i] * (vec[i] - hash_min[i]);
		slot[i] = (int)floor (normalized[i]);
		slotdiff[i] = normalized[i] - (vec_t)slot[i];

		slot[i] = (slot[i] + hash_numslots[i]) % hash_numslots[i];
		slot[i] = (slot[i] + hash_numslots[i]) % hash_numslots[i]; // do it twice to handle negative values
	}

	h = slot[0] * hash_numslots[1] + slot[1];

	*num_hashneighbors = 0;
	for (x = -1; x <= 1; x++)
	{
		if (x == -1 && slotdiff[0] > hash_scale[0] * (2 * POINT_EPSILON) ||
			x == 1 && slotdiff[0] < 1 - hash_scale[0] * (2 * POINT_EPSILON))
		{
			continue;
		}
		for (y = -1; y <= 1; y++)
		{
			if (y == -1 && slotdiff[1] > hash_scale[1] * (2 * POINT_EPSILON) ||
				y == 1 && slotdiff[1] < 1 - hash_scale[1] * (2 * POINT_EPSILON))
			{
				continue;
			}
			if (*num_hashneighbors >= MAX_HASH_NEIGHBORS)
			{
				Error ("HashVec: internal error.");
			}
			hashneighbors[*num_hashneighbors] =
				((slot[0] + x + hash_numslots[0]) % hash_numslots[0]) * hash_numslots[1] +
				(slot[1] + y + hash_numslots[1]) % hash_numslots[1];
			(*num_hashneighbors)++;
		}
	}

	return h;
}

// =====================================================================================
//  GetVertex
// =====================================================================================
static bool     s_exactvertex = false;                     // GetVertex: match equal points only
static int      GetVertex(const vec3_t in, const int planenum)
{
    int             h;
    int             i;
    hashvert_t*     hv;
    vec3_t          vert;
	int				num_hashneighbors;
	int				hashneighbors[MAX_HASH_NEIGHBORS];

    for (i = 0; i < 3; i++)
    {
        if (fabs(in[i] - VectorRound(in[i])) < 0.001)
        {
            vert[i] = VectorRound(in[i]);
        }
        else
        {
            vert[i] = in[i];
        }
    }

	h = HashVec(vert, &num_hashneighbors, hashneighbors);

  for (i = 0; i < num_hashneighbors; i++)
	for (hv = hashverts[hashneighbors[i]]; hv; hv = hv->next)
    {
        if (s_exactvertex
            ? (hv->point[0] == vert[0] && hv->point[1] == vert[1] && hv->point[2] == vert[2])
            : (fabs(hv->point[0] - vert[0]) < POINT_EPSILON
            && fabs(hv->point[1] - vert[1]) < POINT_EPSILON && fabs(hv->point[2] - vert[2]) < POINT_EPSILON))
        {
            hv->numedges++;
            if (hv->numplanes == 3)
            {
                return hv->num;                            // allready known to be a corner
            }
            for (i = 0; i < hv->numplanes; i++)
            {
                if (hv->planenums[i] == planenum)
                {
                    return hv->num;                        // allready know this plane
                }
            }
            if (hv->numplanes != 2)
            {
                hv->planenums[hv->numplanes] = planenum;
            }
            hv->numplanes++;
            return hv->num;
        }
    }

    hv = hvert_p;
    hv->numedges = 1;
    hv->numplanes = 1;
    hv->planenums[0] = planenum;
    hv->next = hashverts[h];
    hashverts[h] = hv;
    VectorCopy(vert, hv->point);
    hv->num = g_numvertexes;
    hlassume(hv->num != MAX_MAP_VERTS, assume_MAX_MAP_VERTS);
    hvert_p++;

    // emit a vertex
    hlassume(g_numvertexes < MAX_MAP_VERTS, assume_MAX_MAP_VERTS);

    g_dvertexes[g_numvertexes].point[0] = vert[0];
    g_dvertexes[g_numvertexes].point[1] = vert[1];
    g_dvertexes[g_numvertexes].point[2] = vert[2];
    g_numvertexes++;

    return hv->num;
}

// =====================================================================================
//  -gridsubdivide: holding the lightmap size when the vertices are emitted
//
//  GridSubdivideCut counts luxel cells from the face's own points. GetVertex then welds
//  each point to any vertex within POINT_EPSILON, and a neighbour that sits 0.01 past a
//  cell boundary would hand the face a 17th cell: "Bad surface extents" when the map
//  loads. The classic rule never got there because it wastes a cell on each side.
//
//  So the cells are counted again with the positions GetVertex is about to give. A face
//  that would go over keeps vertices of its own, at its own points, instead of the
//  welded ones; a point that is itself past the 16 cells (a t-junction point slightly
//  off its edge) is pulled back inside along the texture axis, by less than a unit or
//  the compile stops. The gap this leaves against the neighbour is the few hundredths
//  of a unit the two already were apart.
// =====================================================================================
static int      s_heldfaces = 0;

static void     SnapVertex(const vec3_t in, vec3_t out)
{
    for (int i = 0; i < 3; i++)
    {
        out[i] = fabs(in[i] - VectorRound(in[i])) < 0.001 ? VectorRound(in[i]) : in[i];
    }
}

// The position GetVertex would give this point, without registering anything.
static void     PeekVertex(const vec3_t in, vec3_t out)
{
    vec3_t          vert;
    int             num_hashneighbors;
    int             hashneighbors[MAX_HASH_NEIGHBORS];

    SnapVertex(in, vert);
    HashVec(vert, &num_hashneighbors, hashneighbors);
    for (int i = 0; i < num_hashneighbors; i++)
    {
        for (const hashvert_t* hv = hashverts[hashneighbors[i]]; hv; hv = hv->next)
        {
            if (fabs(hv->point[0] - vert[0]) < POINT_EPSILON
                && fabs(hv->point[1] - vert[1]) < POINT_EPSILON && fabs(hv->point[2] - vert[2]) < POINT_EPSILON)
            {
                VectorCopy(hv->point, out);
                return;
            }
        }
    }
    VectorCopy(vert, out);
}

// Luxel cells along one texture axis, with the engine's arithmetic.
static int      CountCells(const vec3_t* const points, const int numpoints, const float* const vecs,
                           int* const lo, int* const hi, float* const vals)
{
    float           vmin = 0, vmax = 0;

    for (int i = 0; i < numpoints; i++)
    {
        float           p[3];

        VectorCopy(points[i], p);
        vals[i] = CalculatePointVecsProduct(p, vecs);
        if (i == 0 || vals[i] < vmin)
        {
            vmin = vals[i];
        }
        if (i == 0 || vals[i] > vmax)
        {
            vmax = vals[i];
        }
    }
    *lo = (int)floor(vmin / TEXTURE_STEP);
    *hi = (int)ceil(vmax / TEXTURE_STEP);
    return *hi - *lo;
}

void            HoldFaceExtents(face_t* const f, bool* const exact)
{
    vec3_t          welded[MAXEDGES];
    vec3_t          own[MAXEDGES];
    float           vals[MAXEDGES];
    int             lo, hi;
    int             i, axis;

    for (i = 0; i < f->numpoints; i++)
    {
        exact[i] = false;
    }
    if (!g_gridsubdivide || f->texturenum == -1)
    {
        return;
    }
    const texinfo_t* const tex = &g_texinfo[f->texturenum];
    if (tex->flags & TEX_SPECIAL)
    {
        return;
    }

    for (i = 0; i < f->numpoints; i++)
    {
        PeekVertex(f->pts[i], welded[i]);
    }
    if (CountCells(welded, f->numpoints, tex->vecs[0], &lo, &hi, vals) <= MAX_SURFACE_EXTENT
        && CountCells(welded, f->numpoints, tex->vecs[1], &lo, &hi, vals) <= MAX_SURFACE_EXTENT)
    {
        return;
    }

    for (i = 0; i < f->numpoints; i++)
    {
        SnapVertex(f->pts[i], own[i]);
    }
    for (axis = 0; axis < 2; axis++)
    {
        if (CountCells(own, f->numpoints, tex->vecs[axis], &lo, &hi, vals) <= MAX_SURFACE_EXTENT)
        {
            continue;
        }
        // Past 16 cells on its own points: keep the 16 that lose the least.
        float           vmin = vals[0], vmax = vals[0];
        for (i = 1; i < f->numpoints; i++)
        {
            vmin = qmin(vmin, vals[i]);
            vmax = qmax(vmax, vals[i]);
        }
        const double    overtop = vmax - (double)(lo + MAX_SURFACE_EXTENT) * TEXTURE_STEP;
        const double    overbottom = (double)(hi - MAX_SURFACE_EXTENT) * TEXTURE_STEP - vmin;
        const double    low = (overtop <= overbottom ? lo : hi - MAX_SURFACE_EXTENT) * (double)TEXTURE_STEP;
        const double    high = low + MAX_SURFACE_EXTENT * (double)TEXTURE_STEP;

        // Move along the texture axis, inside the plane of the face.
        const dplane_t* const plane = &g_dplanes[f->planenum];
        vec3_t          dir;
        VectorCopy(tex->vecs[axis], dir);
        const vec_t     away = DotProduct(dir, plane->normal);
        VectorMA(dir, -away, plane->normal, dir);
        const vec_t     rate = DotProduct(dir, tex->vecs[axis]); // texture units per unit along dir

        if (qmin(overtop, overbottom) > 1.0 || rate < NORMAL_EPSILON)
        {
            Error("-gridsubdivide: a face at (%.0f %.0f %.0f) ends %.2f units past its 16 lightmap cells. "
                  "Compile without -gridsubdivide and please report this map.",
                  f->pts[0][0], f->pts[0][1], f->pts[0][2], qmin(overtop, overbottom));
        }
        for (i = 0; i < f->numpoints; i++)
        {
            // 0.01 inside, so the float the engine computes cannot land outside again
            double          target = vals[i];
            if (vals[i] < low)
            {
                target = low + 0.01;
            }
            else if (vals[i] > high)
            {
                target = high - 0.01;
            }
            if (target != vals[i])
            {
                VectorMA(own[i], (target - vals[i]) / rate, dir, own[i]);
            }
        }
    }

    for (axis = 0; axis < 2; axis++)
    {
        if (CountCells(own, f->numpoints, tex->vecs[axis], &lo, &hi, vals) > MAX_SURFACE_EXTENT)
        {
            Error("-gridsubdivide: could not keep a face at (%.0f %.0f %.0f) inside 16 lightmap cells. "
                  "Compile without -gridsubdivide and please report this map.",
                  f->pts[0][0], f->pts[0][1], f->pts[0][2]);
        }
    }
    for (i = 0; i < f->numpoints; i++)
    {
        if (welded[i][0] != own[i][0] || welded[i][1] != own[i][1] || welded[i][2] != own[i][2])
        {
            exact[i] = true;
        }
        VectorCopy(own[i], f->pts[i]);
    }
    s_heldfaces++;
}

int             NumHeldFaces()
{
    return s_heldfaces;
}

//===========================================================================

static std::unordered_map<unsigned long long, std::vector<int> > s_edgesbyverts; // edges of the current model

static inline unsigned long long EdgeKey(const int v0, const int v1)
{
    return ((unsigned long long)(unsigned int)v0 << 32) | (unsigned int)v1;
}

// =====================================================================================
//  GetEdge
//      Don't allow four way edges
// =====================================================================================
int             GetEdge(const vec3_t p1, const vec3_t p2, face_t* f, const bool exact1, const bool exact2)
{
    int             v1;
    int             v2;
    dedge_t*        edge;
    int             i;

    hlassert(f->contents);

    s_exactvertex = exact1;
    v1 = GetVertex(p1, f->planenum);
    s_exactvertex = exact2;
    v2 = GetVertex(p2, f->planenum);
    s_exactvertex = false;

    // The edges of this model that run v2 -> v1, oldest first. This used to be a
    // scan over every edge of the model for every edge of every face; the first
    // match in edge order is still the one taken.
    std::unordered_map<unsigned long long, std::vector<int> >::iterator it = s_edgesbyverts.find(EdgeKey(v2, v1));
    if (it != s_edgesbyverts.end())
    {
        for (size_t n = 0; n < it->second.size(); n++)
        {
            i = it->second[n];
            if (!edgefaces[i][1] && edgefaces[i][0]->contents == f->contents
                && edgefaces[i][0]->planenum != (f->planenum ^ 1))
            {
                edgefaces[i][1] = f;
                return -i;
            }
        }
    }

    // emit an edge
    hlassume(g_numedges < MAX_MAP_EDGES, assume_MAX_MAP_EDGES);
    i = g_numedges;
    edge = &g_dedges[g_numedges];
    g_numedges++;
    edge->v[0] = v1;
    edge->v[1] = v2;
    edgefaces[i][0] = f;
    s_edgesbyverts[EdgeKey(v1, v2)].push_back(i);

    return i;
}

// =====================================================================================
//  MakeFaceEdges
// =====================================================================================
void            MakeFaceEdges()
{
    InitHash();
    s_edgesbyverts.clear();
    firstmodeledge = g_numedges;
    firstmodelface = g_numfaces;
}

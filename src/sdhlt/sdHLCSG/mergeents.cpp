// Merging of equivalent static brush entities.
//
// Every brush entity costs one BSP model (MAX_MAP_MODELS) and one slot in the
// engine's model precache table, which is shared with studio models and
// sprites. Decorative geometry (bushes made of func_illusionary, static
// func_wall trims, ...) usually consists of many small entities that carry the
// exact same keyvalues and differ only in their position, so they can be folded
// into a single entity without any change in behaviour.
//
// Two entities are only folded together when they are provably
// interchangeable: same classname (from a whitelist of purely static classes),
// identical keyvalue sets, no name/target/origin of any kind, and a render mode
// that is not affected by the engine's per-entity transparency sorting.
//
// Because a brush entity is culled through the bounding box of its model,
// blindly merging everything would produce one map-sized box that is drawn from
// almost every leaf. Candidates are therefore clustered by proximity and a
// cluster is only grown while its bounding box stays below a size limit.

#include "csg.h"

#include <map>
#include <vector>

bool  g_merge_entities = DEFAULT_MERGE_ENTITIES;
vec_t g_merge_maxsize  = DEFAULT_MERGE_MAXSIZE;
bool  g_merge_blend    = DEFAULT_MERGE_BLEND;

// Classes that hold nothing but static geometry. Anything that moves, thinks,
// blocks a trigger or is toggled at runtime must stay out of this list.
static const char* const s_mergeable_classes[] =
{
	"func_illusionary",
	"func_wall",
};

// Keys that tie an entity to the rest of the map. Their presence alone rules
// the entity out, even when two entities happen to share the same value.
static const char* const s_linking_keys[] =
{
	"targetname",
	"target",
	"killtarget",
	"globalname",
	"parentname",
	"master",
	"netname",
	"message",
	"zhlt_usemodel",	// refers to another model, and can be referred to
	"zhlt_minsmaxs",	// pins the model bounds, meaningless once merged
	"zhlt_nomerge",		// explicit opt-out
};

// Render modes whose faces the engine blends. Blended entities are depth sorted
// against each other using a single point per entity, so folding several of
// them into one can visibly reorder them. kRenderNormal and kRenderTransAlpha
// (the alpha tested "Solid" mode used by '{' textures) are not sorted and are
// always safe.
#define kRenderNormal     0
#define kRenderTransAlpha 4

typedef struct
{
	int    entnum;
	vec3_t mins;
	vec3_t maxs;
	bool   used;
} mergeent_t;

// =====================================================================================
//  EntityBounds
//      bounds of every plane point of every brush of the entity. Brush windings
//      do not exist yet at this point of the compile, but the three points that
//      define each side come straight from the map file and enclose the brush.
// =====================================================================================
static bool     EntityBounds(const entity_t* const ent, vec3_t mins, vec3_t maxs)
{
	bool found = false;

	for (int b = 0; b < ent->numbrushes; b++)
	{
		const brush_t* brush = &g_mapbrushes[ent->firstbrush + b];

		for (int s = 0; s < brush->numsides; s++)
		{
			const side_t* side = &g_brushsides[brush->firstside + s];

			for (int p = 0; p < 3; p++)
			{
				if (!found)
				{
					VectorCopy(side->planepts[p], mins);
					VectorCopy(side->planepts[p], maxs);
					found = true;
					continue;
				}

				for (int axis = 0; axis < 3; axis++)
				{
					vec_t value = side->planepts[p][axis];

					mins[axis] = qmin(mins[axis], value);
					maxs[axis] = qmax(maxs[axis], value);
				}
			}
		}
	}

	return found;
}

// =====================================================================================
//  BoxLongestSide
// =====================================================================================
static vec_t    BoxLongestSide(const vec3_t mins, const vec3_t maxs)
{
	vec_t longest = 0;

	for (int axis = 0; axis < 3; axis++)
	{
		longest = qmax(longest, maxs[axis] - mins[axis]);
	}

	return longest;
}

// =====================================================================================
//  IsMergeableEntity
// =====================================================================================
static bool     IsMergeableEntity(const entity_t* const ent)
{
	unsigned int i;

	if (ent->numbrushes <= 0)
	{
		return false;
	}

	const char* classname = ValueForKey(ent, "classname");
	bool        known = false;

	for (i = 0; i < sizeof(s_mergeable_classes) / sizeof(s_mergeable_classes[0]); i++)
	{
		if (!strcmp(classname, s_mergeable_classes[i]))
		{
			known = true;
			break;
		}
	}
	if (!known)
	{
		return false;
	}

	for (i = 0; i < sizeof(s_linking_keys) / sizeof(s_linking_keys[0]); i++)
	{
		if (*ValueForKey(ent, s_linking_keys[i]))
		{
			return false;
		}
	}

	// An origin brush means the entity is placed relative to a point, which no
	// longer holds once its brushes share a model with someone else's.
	if (!VectorCompare(ent->origin, vec3_origin))
	{
		return false;
	}

	if (!g_merge_blend)
	{
		int rendermode = IntForKey(ent, "rendermode");

		if (rendermode != kRenderNormal && rendermode != kRenderTransAlpha)
		{
			return false;
		}
	}

	return true;
}

// =====================================================================================
//  StringHash
// =====================================================================================
static unsigned int StringHash(const char* s)
{
	unsigned int hash = 2166136261u;

	for (; *s; s++)
	{
		hash = (hash ^ (unsigned char)*s) * 16777619u;
	}

	return hash;
}

// =====================================================================================
//  KeyvalueHash
//      order independent digest of the whole keyvalue set, used to bucket
//      entities before comparing them pair by pair
// =====================================================================================
static unsigned int KeyvalueHash(const entity_t* const ent)
{
	unsigned int hash = 0;

	for (const epair_t* ep = ent->epairs; ep; ep = ep->next)
	{
		// summed so that the order the keys were parsed in does not matter
		hash += StringHash(ep->key) * 31u + StringHash(ep->value);
	}

	return hash;
}

// =====================================================================================
//  SameKeyvalues
// =====================================================================================
static bool     SameKeyvalues(const entity_t* const a, const entity_t* const b)
{
	const epair_t* ep;
	int            counta = 0;
	int            countb = 0;

	for (ep = a->epairs; ep; ep = ep->next)
	{
		if (strcmp(ValueForKey(b, ep->key), ep->value))
		{
			return false;
		}
		counta++;
	}
	for (ep = b->epairs; ep; ep = ep->next)
	{
		countb++;
	}

	// a duplicated key would make the counts disagree even though every lookup
	// matched, so bail out in that case as well
	return counta == countb;
}

// =====================================================================================
//  ClusterGroup
//      greedily grows clusters of nearby entities, never letting a cluster grow
//      past g_merge_maxsize on any axis. Clusters are returned as lists of
//      indices into 'ents'.
// =====================================================================================
static void     ClusterGroup(std::vector< mergeent_t >& ents, const std::vector< int >& group,
                             std::vector< std::vector< int > >& clusters)
{
	for (unsigned int i = 0; i < group.size(); i++)
	{
		if (ents[group[i]].used)
		{
			continue;
		}

		std::vector< int > cluster;
		vec3_t             mins;
		vec3_t             maxs;

		ents[group[i]].used = true;
		cluster.push_back(group[i]);
		VectorCopy(ents[group[i]].mins, mins);
		VectorCopy(ents[group[i]].maxs, maxs);

		// keep absorbing the nearest entity that still fits
		while (true)
		{
			int    best = -1;
			vec_t  bestdist = 0;
			vec3_t bestmins;
			vec3_t bestmaxs;

			for (unsigned int j = 0; j < group.size(); j++)
			{
				const mergeent_t& candidate = ents[group[j]];

				if (candidate.used)
				{
					continue;
				}

				vec3_t newmins;
				vec3_t newmaxs;

				VectorCompareMinimum(mins, candidate.mins, newmins);
				VectorCompareMaximum(maxs, candidate.maxs, newmaxs);

				if (g_merge_maxsize > 0 && BoxLongestSide(newmins, newmaxs) > g_merge_maxsize)
				{
					continue;
				}

				// squared distance between the two box centers
				vec_t dist = 0;

				for (int axis = 0; axis < 3; axis++)
				{
					vec_t delta = (candidate.mins[axis] + candidate.maxs[axis]) * 0.5
					            - (mins[axis] + maxs[axis]) * 0.5;

					dist += delta * delta;
				}

				if (best < 0 || dist < bestdist)
				{
					best = (int)j;
					bestdist = dist;
					VectorCopy(newmins, bestmins);
					VectorCopy(newmaxs, bestmaxs);
				}
			}

			if (best < 0)
			{
				break;
			}

			ents[group[best]].used = true;
			cluster.push_back(group[best]);
			VectorCopy(bestmins, mins);
			VectorCopy(bestmaxs, maxs);
		}

		if (cluster.size() > 1)
		{
			clusters.push_back(cluster);
		}
	}
}

// =====================================================================================
//  RebuildEntities
//      rewrites the entity and brush arrays so that every cluster becomes a
//      single entity whose brushes are contiguous. Relative order is preserved,
//      so model numbers of untouched entities shift only by the entities that
//      actually went away.
// =====================================================================================
static void     RebuildEntities(const std::vector< int >& leaderof,
                                const std::vector< std::vector< int > >& membersof)
{
	std::vector< brush_t > newbrushes;
	int                    numentities = 0;

	newbrushes.reserve(g_nummapbrushes);

	for (int i = 0; i < g_numentities; i++)
	{
		if (leaderof[i] != i)
		{
			continue;	// absorbed by an earlier entity
		}

		entity_t ent = g_entities[i];

		ent.firstbrush = (int)newbrushes.size();
		ent.numbrushes = 0;

		for (unsigned int m = 0; m < membersof[i].size(); m++)
		{
			// every member is still at its original index: a member is always
			// greater than its leader, and we only ever write at 'numentities',
			// which never runs ahead of 'i'
			const entity_t* src = &g_entities[membersof[i][m]];

			for (int b = 0; b < src->numbrushes; b++)
			{
				brush_t brush = g_mapbrushes[src->firstbrush + b];

				brush.entitynum = numentities;
				brush.brushnum = ent.numbrushes;
				// originalentitynum / originalbrushnum are left alone so that
				// warnings keep pointing at the brush the mapper wrote

				newbrushes.push_back(brush);
				ent.numbrushes++;
			}
		}

		g_entities[numentities++] = ent;
	}

	// every brush belongs to a live entity, so the rebuild has to see all of
	// them. Bail out loudly rather than silently dropping geometry.
	if ((int)newbrushes.size() != g_nummapbrushes)
	{
		Error("MergeStaticEntities: %d of %d brushes accounted for.",
		      (int)newbrushes.size(), g_nummapbrushes);
	}

	for (int b = 0; b < g_nummapbrushes; b++)
	{
		g_mapbrushes[b] = newbrushes[b];
	}

	g_numentities = numentities;
}

// =====================================================================================
//  MergeStaticEntities
// =====================================================================================
void            MergeStaticEntities()
{
	if (!g_merge_entities)
	{
		return;
	}
	if (g_onlyents)
	{
		// -onlyents only rewrites the entity lump of an existing bsp, whose
		// models are already baked. Changing the entity list would shift them.
		return;
	}

	std::vector< mergeent_t > ents;
	int                       i;

	for (i = 1; i < g_numentities; i++)	// never worldspawn
	{
		if (!IsMergeableEntity(&g_entities[i]))
		{
			continue;
		}

		mergeent_t candidate;

		candidate.entnum = i;
		candidate.used = false;

		if (!EntityBounds(&g_entities[i], candidate.mins, candidate.maxs))
		{
			continue;
		}

		ents.push_back(candidate);
	}

	if (ents.size() < 2)
	{
		return;
	}

	// bucket by keyvalue digest, then split each bucket into groups that really
	// do share every keyvalue
	std::map< unsigned int, std::vector< int > > buckets;

	for (i = 0; i < (int)ents.size(); i++)
	{
		buckets[KeyvalueHash(&g_entities[ents[i].entnum])].push_back(i);
	}

	std::vector< std::vector< int > > clusters;

	for (std::map< unsigned int, std::vector< int > >::iterator bucket = buckets.begin();
	     bucket != buckets.end(); ++bucket)
	{
		std::vector< int >& candidates = bucket->second;
		std::vector< bool > taken(candidates.size(), false);

		for (unsigned int a = 0; a < candidates.size(); a++)
		{
			if (taken[a])
			{
				continue;
			}

			std::vector< int > group;

			taken[a] = true;
			group.push_back(candidates[a]);

			for (unsigned int b = a + 1; b < candidates.size(); b++)
			{
				if (taken[b])
				{
					continue;
				}
				if (SameKeyvalues(&g_entities[ents[candidates[a]].entnum],
				                  &g_entities[ents[candidates[b]].entnum]))
				{
					taken[b] = true;
					group.push_back(candidates[b]);
				}
			}

			if (group.size() > 1)
			{
				ClusterGroup(ents, group, clusters);
			}
		}
	}

	if (clusters.empty())
	{
		return;
	}

	// leaderof[i] == i for every entity that survives
	std::vector< int >                leaderof(g_numentities);
	std::vector< std::vector< int > > membersof(g_numentities);
	int                               absorbed = 0;

	for (i = 0; i < g_numentities; i++)
	{
		leaderof[i] = i;
	}

	for (unsigned int c = 0; c < clusters.size(); c++)
	{
		const std::vector< int >& cluster = clusters[c];
		int                       leader = g_numentities;
		vec3_t                    mins;
		vec3_t                    maxs;
		unsigned int              m;

		VectorCopy(ents[cluster[0]].mins, mins);
		VectorCopy(ents[cluster[0]].maxs, maxs);

		for (m = 0; m < cluster.size(); m++)
		{
			leader = qmin(leader, ents[cluster[m]].entnum);
			VectorCompareMinimum(mins, ents[cluster[m]].mins, mins);
			VectorCompareMaximum(maxs, ents[cluster[m]].maxs, maxs);
		}

		for (m = 0; m < cluster.size(); m++)
		{
			int entnum = ents[cluster[m]].entnum;

			if (entnum != leader)
			{
				leaderof[entnum] = leader;
				absorbed++;
			}
		}

		Verbose("merge: %d %s entities -> entity %d, box %.0f x %.0f x %.0f\n",
		        (int)cluster.size(), ValueForKey(&g_entities[leader], "classname"), leader,
		        maxs[0] - mins[0], maxs[1] - mins[1], maxs[2] - mins[2]);
	}

	for (i = 0; i < g_numentities; i++)
	{
		membersof[leaderof[i]].push_back(i);
	}

	RebuildEntities(leaderof, membersof);

	Log("Merged %d static brush entities into %d (%d models freed)\n",
	    absorbed + (int)clusters.size(), (int)clusters.size(), absorbed);
}

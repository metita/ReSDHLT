#include "qrad.h"
#include "profiling.h"
#include "gpu_gather.h"

#include <algorithm>
#include <atomic>
#include <vector>

#ifdef SDHLT_GPU
#include <cmath>
#include <cstdint>
#include <cstdlib>

#include "gpu/gpu.h"
#endif



typedef struct
{
    unsigned        offset:24;
    unsigned        values:8;
}
sparse_row_t;

typedef struct
{
    sparse_row_t*   row;
    int             count;
}
sparse_column_t;

sparse_column_t* s_vismatrix;
static std::vector< std::vector<unsigned> > s_patches_by_leaf;
static std::atomic<uint64_t> s_sparse_source_patches(0);
static std::atomic<uint64_t> s_sparse_candidate_faces(0);
static std::atomic<uint64_t> s_sparse_visible_pairs(0);

// Vismatrix protected
static unsigned IsVisbitInArray(const unsigned x, const unsigned y)
{
    int             first, last, current;
    int             y_byte = y / 8;
    sparse_row_t*  row;
    sparse_column_t* column = s_vismatrix + x;

    if (!column->count)
    {
        return -1;
    }

    first = 0;
    last = column->count - 1;

    //    Warning("Searching . . .");
    // binary search to find visbit
    while (1)
    {
        current = (first + last) / 2;
        row = column->row + current;
        //        Warning("first %u, last %u, current %u, row %p, row->offset %u", first, last, current, row, row->offset);
        if ((row->offset) < y_byte)
        {
            first = current + 1;
        }
        else if ((row->offset) > y_byte)
        {
            last = current - 1;
        }
        else
        {
            return current;
        }
        if (first > last)
        {
            return -1;
        }
    }
}

static size_t SetVisColumn(int patchnum, std::vector<unsigned>& visible_patches)
{
	sparse_column_t* column = &s_vismatrix[patchnum];
	if (column->count || column->row)
	{
		Error ("SetVisColumn: column has been set");
	}

	if (visible_patches.empty())
	{
		return 0;
	}

	// TestPatchToFace normally emits indices in face/patch order, but sorting
	// here makes the compact row byte-for-byte equivalent to the old full-array
	// scan even if patch allocation order changes in the future.
	std::sort(visible_patches.begin(), visible_patches.end());
	visible_patches.erase(std::unique(visible_patches.begin(), visible_patches.end()),
	                      visible_patches.end());

	unsigned previous_byte = UINT_MAX;
	for (size_t i = 0; i < visible_patches.size(); ++i)
	{
		const unsigned patch = visible_patches[i];
		if (patch < (unsigned)patchnum || patch >= g_num_patches)
			Error("SetVisColumn: invalid patch index");
		const unsigned byte_offset = patch >> 3;
		if (byte_offset != previous_byte)
		{
			column->count++;
			previous_byte = byte_offset;
		}
	}

	column->row = (sparse_row_t*)malloc(column->count * sizeof(sparse_row_t));
	hlassume (column->row != NULL, assume_NoMemory);

	int row = -1;
	previous_byte = UINT_MAX;
	for (size_t i = 0; i < visible_patches.size(); ++i)
	{
		const unsigned patch = visible_patches[i];
		const unsigned byte_offset = patch >> 3;
		if (byte_offset != previous_byte)
		{
			++row;
			column->row[row].offset = byte_offset;
			column->row[row].values = 0;
			previous_byte = byte_offset;
		}
		column->row[row].values |= 1u << (patch & 7u);
	}
	if (row + 1 != column->count)
	{
		Error ("SetVisColumn: internal error");
	}
	return visible_patches.size();
}

// Vismatrix public
static bool     CheckVisBitSparse(unsigned x, unsigned y
								  , vec3_t &transparency_out
								  , unsigned int &next_index
								  )
{
    PROF_CALL(PROF_CHECKVISBIT);
    int                offset;

    	VectorFill(transparency_out, 1.0);

    if (x == y)
    {
        return 1;
    }

    const unsigned a = x;
    const unsigned b = y;

    if (x > y)
    {
        x = b;
        y = a;
    }

    if (x > g_num_patches)
    {
        Warning("in CheckVisBit(), x > num_patches");
    }
    if (y > g_num_patches)
    {
        Warning("in CheckVisBit(), y > num_patches");
    }

    if ((offset = IsVisbitInArray(x, y)) != -1)
    {
    	if(g_customshadow_with_bouncelight)
    	{
    	     GetTransparency(a, b, transparency_out, next_index);
    	}
        return s_vismatrix[x].row[offset].values & (1 << (y & 7));
    }

	return false;
}

/*
 * ==============
 * TestPatchToFace
 * 
 * Sets vis bits for all patches in the face
 * ==============
 */
static void     TestPatchToFace(const unsigned patchnum, const int facenum, const int head
								, byte *pvs
								, uint32_t* visible_generation
								, uint32_t generation
								, std::vector<unsigned>& visible_patches
								)
{
    patch_t*        patch = &g_patches[patchnum];
    patch_t*        patch2 = g_face_patches[facenum];

    // if emitter is behind that face plane, skip all patches

    if (patch2)
    {
        const dplane_t* plane2 = getPlaneFromFaceNumber(facenum);

		if (DotProduct (patch->origin, plane2->normal) > PatchPlaneDist (patch2) + ON_EPSILON - patch->emitter_range)
        {
            // we need to do a real test
            const dplane_t* plane = getPlaneFromFaceNumber(patch->faceNumber);

            for (; patch2; patch2 = patch2->next)
            {
                unsigned        m = patch2 - g_patches;

                vec3_t		transparency = {1.0,1.0,1.0};
				int opaquestyle = -1;

                // check vis between patch and patch2
                // if bit has not already been set
                //  && v2 is not behind light plane
                //  && v2 is visible from v1
                if (m > patchnum)
				{
					if (patch2->leafnum == 0 || !(pvs[(patch2->leafnum - 1) >> 3] & (1 << ((patch2->leafnum - 1) & 7))))
					{
						continue;
					}
					vec3_t origin1, origin2;
					vec3_t delta;
					vec_t dist;
					VectorSubtract (patch->origin, patch2->origin, delta);
					dist = VectorLength (delta);
					if (dist < patch2->emitter_range - ON_EPSILON)
					{
						GetAlternateOrigin (patch->origin, plane->normal, patch2, origin2);
					}
					else
					{
						VectorCopy (patch2->origin, origin2);
					}
					if (DotProduct (origin2, plane->normal) <= PatchPlaneDist (patch) + MINIMUM_PATCH_DISTANCE)
					{
						continue;
					}
					if (dist < patch->emitter_range - ON_EPSILON)
					{
						GetAlternateOrigin (patch2->origin, plane2->normal, patch, origin1);
					}
					else
					{
						VectorCopy (patch->origin, origin1);
					}
					if (DotProduct (origin1, plane2->normal) <= PatchPlaneDist (patch2) + MINIMUM_PATCH_DISTANCE)
					{
						continue;
					}
                    if (TestLine(
						origin1, origin2
						) != CONTENTS_EMPTY)
					{
						continue;
					}
                    if (TestSegmentAgainstOpaqueList(
						origin1, origin2
						, transparency
						, opaquestyle
					))
					{
						continue;
					}

					if (opaquestyle != -1)
					{
						AddStyleToStyleArray (m, patchnum, opaquestyle);
						AddStyleToStyleArray (patchnum, m, opaquestyle);
					}
                                        
                    if(g_customshadow_with_bouncelight && !VectorCompare(transparency, vec3_one) )
                    {
                    	AddTransparencyToRawArray(patchnum, m, transparency);
                    }
					if (visible_generation[m] != generation)
					{
						visible_generation[m] = generation;
						visible_patches.push_back(m);
					}
                }
            }
        }
    }
}


/*
 * ===========
 * BuildVisLeafs
 * 
 * This is run by multiple threads
 * ===========
 */
#ifdef SYSTEM_WIN32
#pragma warning(push)
#pragma warning(disable: 4100)                             // unreferenced formal parameter
#endif
static void     BuildVisLeafs(int threadnum)
{
    int             i;
    byte            pvs[(MAX_MAP_LEAFS + 7) / 8];
    dleaf_t*        srcleaf;
    int             head;
	std::vector<uint32_t> visible_generation(g_num_patches, 0);
	std::vector<unsigned> visible_patches;
	std::vector<int> candidate_faces;
	uint32_t generation = 0;
	uint64_t local_source_patches = 0;
	uint64_t local_candidate_faces = 0;
	uint64_t local_visible_pairs = 0;

    while (1)
    {
        //
        // build a minimal BSP tree that only
        // covers areas relevent to the PVS
        //
        i = GetThreadWork();
        if (i == -1)
        {
            break;
        }
        i++;                                               // skip leaf 0
        srcleaf = &g_dleafs[i];
        if (!g_visdatasize)
		{
			memset (pvs, 255, (g_dmodels[0].visleafs + 7) / 8);
		}
		else
		{
		if (srcleaf->visofs == -1)
		{
			Developer (DEVELOPER_LEVEL_ERROR, "Error: No visdata for leaf %d\n", i);
			continue;
		}
        DecompressVis(&g_dvisdata[srcleaf->visofs], pvs, sizeof(pvs));
		}
        head = 0;

		// Build the PVS-compatible target face list once per source leaf. The old
		// code rediscovered the same empty faces for every patch in this leaf.
		candidate_faces.clear();
		for (int facenum = 0; facenum < g_numfaces; ++facenum)
		{
			for (patch_t* target = g_face_patches[facenum]; target; target = target->next)
			{
				if (target->leafnum != 0
					&& (pvs[(target->leafnum - 1) >> 3]
						& (1 << ((target->leafnum - 1) & 7))))
				{
					candidate_faces.push_back(facenum);
					break;
				}
			}
		}

		const std::vector<unsigned>& source_patches = s_patches_by_leaf[i];
		for (size_t source = 0; source < source_patches.size(); ++source)
		{
			const unsigned patchnum = source_patches[source];
			const int source_face = g_patches[patchnum].faceNumber;
			if (++generation == 0)
			{
				std::fill(visible_generation.begin(), visible_generation.end(), 0);
				generation = 1;
			}
			visible_patches.clear();
			std::vector<int>::const_iterator target = std::upper_bound(
				candidate_faces.begin(), candidate_faces.end(), source_face);
			local_source_patches++;
			local_candidate_faces += candidate_faces.end() - target;
			for (; target != candidate_faces.end(); ++target)
			{
				TestPatchToFace(patchnum, *target, head, pvs,
				                visible_generation.data(), generation, visible_patches);
			}
			local_visible_pairs += SetVisColumn((int)patchnum, visible_patches);
		}

    }
	s_sparse_source_patches.fetch_add(local_source_patches, std::memory_order_relaxed);
	s_sparse_candidate_faces.fetch_add(local_candidate_faces, std::memory_order_relaxed);
	s_sparse_visible_pairs.fetch_add(local_visible_pairs, std::memory_order_relaxed);
}

#ifdef SYSTEM_WIN32
#pragma warning(pop)
#endif

/*
 * ==============
 * BuildVisMatrix
 * ==============
 */
static void     BuildVisMatrix()
{
    s_vismatrix = (sparse_column_t*)AllocBlock(g_num_patches * sizeof(sparse_column_t));

    if (!s_vismatrix)
    {
        Log("Failed to allocate vismatrix");
        hlassume(s_vismatrix != NULL, assume_NoMemory);
    }

	s_patches_by_leaf.clear();
	s_patches_by_leaf.resize((size_t)g_dmodels[0].visleafs + 1);
	for (int facenum = 0; facenum < g_numfaces; ++facenum)
	{
		for (patch_t* patch = g_face_patches[facenum]; patch; patch = patch->next)
		{
			if (patch->leafnum > 0 && patch->leafnum <= g_dmodels[0].visleafs)
				s_patches_by_leaf[patch->leafnum].push_back((unsigned)(patch - g_patches));
		}
	}
	s_sparse_source_patches.store(0, std::memory_order_relaxed);
	s_sparse_candidate_faces.store(0, std::memory_order_relaxed);
	s_sparse_visible_pairs.store(0, std::memory_order_relaxed);
	const double started = I_FloatTime();
    NamedRunThreadsOn(g_dmodels[0].visleafs, g_estimate, BuildVisLeafs);
	Log("Sparse visibility: %llu source patches, %.2fM candidate faces, %.2fM visible pairs (%.2f seconds)\n",
		(unsigned long long)s_sparse_source_patches.load(std::memory_order_relaxed),
		s_sparse_candidate_faces.load(std::memory_order_relaxed) / 1000000.0,
		s_sparse_visible_pairs.load(std::memory_order_relaxed) / 1000000.0,
		I_FloatTime() - started);
	std::vector< std::vector<unsigned> >().swap(s_patches_by_leaf);
}

static void     FreeVisMatrix()
{
    if (s_vismatrix)
    {
        unsigned        x;
        sparse_column_t* item;

        for (x = 0, item = s_vismatrix; x < g_num_patches; x++, item++)
        {
            if (item->row)
            {
                free(item->row);
            }
        }
        if (FreeBlock(s_vismatrix))
        {
            s_vismatrix = NULL;
        }
        else
        {
            Warning("Unable to free vismatrix");
        }
    }


}

static void     DumpVismatrixInfo()
{
    unsigned        totals[8];
    size_t          total_vismatrix_memory;
	total_vismatrix_memory = sizeof(sparse_column_t) * g_num_patches;

    sparse_column_t* column_end = s_vismatrix + g_num_patches;
    sparse_column_t* column = s_vismatrix;

    memset(totals, 0, sizeof(totals));

    while (column < column_end)
    {
        total_vismatrix_memory += column->count * sizeof(sparse_row_t);
        column++;
    }

    Log("%-20s: %5.1f megs\n", "visibility matrix", total_vismatrix_memory / (1024 * 1024.0));
}

#ifdef SDHLT_GPU
namespace
{
    // Four million pairs use 32 MB for the pair buffer and 16 MB for results,
    // below Vulkan's guaranteed 128 MB storage-buffer range. It also dispatches
    // 62,500 workgroups, below the guaranteed per-axis limit of 65,535.
    const size_t GPU_TRANSFER_BATCH_PAIRS = 4000000;
    const size_t GPU_AUTO_TRANSFER_PAIRS = 1000000;

    size_t GpuTransferBatchPairs(size_t largest_row)
    {
        size_t requested = GPU_TRANSFER_BATCH_PAIRS;
        const char* value = std::getenv("SDHLT_GPU_TRANSFER_BATCH_PAIRS");
        if (value && *value)
        {
            char* end = NULL;
            const unsigned long long parsed = std::strtoull(value, &end, 10);
            if (end != value && *end == '\0' && parsed > 0)
            {
                requested = (size_t)std::min(parsed,
                    (unsigned long long)GPU_TRANSFER_BATCH_PAIRS);
            }
        }
        return std::max(requested, largest_row);
    }

    template <typename Visitor>
    void VisitSparsePairs(Visitor visit)
    {
        for (unsigned x = 0; x < g_num_patches; x++)
        {
            const sparse_column_t& column = s_vismatrix[x];
            for (int r = 0; r < column.count; r++)
            {
                const sparse_row_t& row = column.row[r];
                const unsigned base = row.offset * 8u;
                unsigned bits = row.values;
                while (bits)
                {
                    unsigned bit = 0;
                    while ((bits & (1u << bit)) == 0)
                    {
                        bit++;
                    }
                    bits &= ~(1u << bit);
                    const unsigned y = base + bit;
                    if (y < g_num_patches && y > x)
                    {
                        visit(x, y);
                    }
                }
            }
        }
    }

    bool MarshalFormFactorScene(rad::gpu::formfactor_scene& scene)
    {
        scene.patches.resize(g_num_patches);
        scene.sky_levels.resize((SKYLEVELMAX + 1) * 2);

        for (int level = 0; level <= SKYLEVELMAX; level++)
        {
            scene.sky_levels[level * 2] = (int32_t)(scene.sky_normals.size() / 4);
            scene.sky_levels[level * 2 + 1] = g_numskynormals[level];
            for (int i = 0; i < g_numskynormals[level]; i++)
            {
                scene.sky_normals.push_back(g_skynormals[level][i][0]);
                scene.sky_normals.push_back(g_skynormals[level][i][1]);
                scene.sky_normals.push_back(g_skynormals[level][i][2]);
                scene.sky_normals.push_back(g_skynormalsizes[level][i]);
            }
        }

        for (unsigned i = 0; i < g_num_patches; i++)
        {
            const patch_t& patch = g_patches[i];
            if (patch.translucent_b || !patch.winding || patch.winding->m_NumPoints > 32)
            {
                return false;
            }

            rad::gpu::patch_gpu& out = scene.patches[i];
            const vec_t* normal = getPlaneFromFaceNumber(patch.faceNumber)->normal;
            for (int axis = 0; axis < 3; axis++)
            {
                out.origin[axis] = patch.origin[axis];
                out.normal[axis] = normal[axis];
            }
            out.area = patch.area;
            out.emitter_range = patch.emitter_range;
            out.exposure = patch.exposure;
            const int miptex = g_texinfo[g_dfaces[patch.faceNumber].texinfo].miptex;
            out.cone_power = g_lightingconeinfo[miptex][0];
            out.cone_scale = g_lightingconeinfo[miptex][1];
            out.skylevel = patch.emitter_skylevel;
            out.wind_ofs = (int32_t)(scene.windings.size() / 3);
            out.wind_count = (int32_t)patch.winding->m_NumPoints;
            for (unsigned p = 0; p < patch.winding->m_NumPoints; p++)
            {
                scene.windings.push_back(patch.winding->m_Points[p][0]);
                scene.windings.push_back(patch.winding->m_Points[p][1]);
                scene.windings.push_back(patch.winding->m_Points[p][2]);
            }
        }
        return true;
    }

    void RollbackGpuTransfers(size_t old_total, size_t old_index_bytes,
                              size_t old_data_bytes)
    {
        for (unsigned i = 0; i < g_num_patches; i++)
        {
            patch_t& patch = g_patches[i];
            if (patch.tIndex)
            {
                FreeBlock(patch.tIndex);
            }
            if (patch.tData)
            {
                FreeBlock(patch.tData);
            }
            patch.tIndex = NULL;
            patch.tData = NULL;
            patch.iIndex = 0;
            patch.iData = 0;
        }
        g_total_transfer = old_total;
        g_transfer_index_bytes = old_index_bytes;
        g_transfer_data_bytes = old_data_bytes;
    }
}
#endif

bool MakeScalesSparseGpu()
{
#ifndef SDHLT_GPU
    return false;
#else
    if (!g_gpu || !g_gpu_transfers || g_rgb_transfers
        || g_customshadow_with_bouncelight)
    {
        return false;
    }

    std::vector<uint32_t> degree(g_num_patches, 0);
    size_t directed_pairs = 0;
    VisitSparsePairs([&](unsigned x, unsigned y)
    {
        degree[x]++;
        degree[y]++;
        directed_pairs += 2;
    });
    if (!directed_pairs)
    {
        return true;
    }
    if (g_gpu_auto && directed_pairs < GPU_AUTO_TRANSFER_PAIRS)
    {
        Log("MakeScales: GPU auto chose CPU (%.2fM visible pairs; crossover starts near %.2fM)\n",
            directed_pairs / 1000000.0, GPU_AUTO_TRANSFER_PAIRS / 1000000.0);
        return false;
    }

    rad::gpu::set_adapter_override(g_gpu_adapter);
    // Only initialize Vulkan after automatic selection has decided the phase
    // is large enough to benefit. The ordinary CPU path remains the fallback.
    if (!rad::gpu::available())
    {
        Warning("-gpu transfers: %s; using the CPU path",
                rad::gpu::last_error().c_str());
        return false;
    }

    rad::gpu::formfactor_scene scene;
    if (!MarshalFormFactorScene(scene))
    {
        Warning("-gpu transfers: translucent or oversized patch winding; using the CPU path");
        return false;
    }

    const size_t largest_row = *std::max_element(degree.begin(), degree.end());
    const size_t batch_pairs = GpuTransferBatchPairs(largest_row);

    std::vector<size_t> offsets(g_num_patches + 1, 0);
    for (unsigned i = 0; i < g_num_patches; i++)
    {
        offsets[i + 1] = offsets[i] + degree[i];
    }
    std::vector<uint32_t> emitters(directed_pairs);
    std::vector<size_t> cursor(offsets.begin(), offsets.end() - 1);
    VisitSparsePairs([&](unsigned x, unsigned y)
    {
        emitters[cursor[x]++] = y;
        emitters[cursor[y]++] = x;
    });

    Log("MakeScales (GPU: %s, %.2fM visible pairs):\n",
        rad::gpu::device_name().c_str(), directed_pairs / 1000000.0);
    const double started = I_FloatTime();
    const size_t old_total = g_total_transfer;
    const size_t old_index_bytes = g_transfer_index_bytes;
    const size_t old_data_bytes = g_transfer_data_bytes;

    std::vector<rad::gpu::transfer_pair> pairs;
    std::vector<float> results;
    std::vector<transfer_raw_index_t> row_indices;
    std::vector<float> row_values;
    pairs.reserve(batch_pairs);

    if (!rad::gpu::formfactor_begin(scene, (uint32_t)batch_pairs))
    {
        Warning("-gpu transfers: %s; using the CPU path",
                rad::gpu::last_error().c_str());
        return false;
    }
    struct formfactor_session_guard_t
    {
        ~formfactor_session_guard_t()
        {
            rad::gpu::formfactor_end();
        }
    } formfactor_session_guard;

    unsigned receiver = 0;
    size_t dispatches = 0;
    while (receiver < g_num_patches)
    {
        const unsigned batch_first = receiver;
        pairs.clear();
        while (receiver < g_num_patches)
        {
            const size_t row_count = offsets[receiver + 1] - offsets[receiver];
            if (!pairs.empty() && pairs.size() + row_count > batch_pairs)
            {
                break;
            }
            for (size_t p = offsets[receiver]; p < offsets[receiver + 1]; p++)
            {
                rad::gpu::transfer_pair pair;
                pair.receiver = (int32_t)receiver;
                pair.emitter = (int32_t)emitters[p];
                pairs.push_back(pair);
            }
            receiver++;
        }

        if (!rad::gpu::formfactor_batch(pairs.data(), pairs.size(), results)
            || results.size() != pairs.size())
        {
            Warning("-gpu transfers: %s; using the CPU path",
                    rad::gpu::last_error().c_str());
            RollbackGpuTransfers(old_total, old_index_bytes, old_data_bytes);
            return false;
        }
        dispatches++;

        size_t result_pos = 0;
        for (unsigned row = batch_first; row < receiver; row++)
        {
            row_indices.clear();
            row_values.clear();
            const size_t row_count = offsets[row + 1] - offsets[row];
            row_indices.reserve(row_count);
            row_values.reserve(row_count);
            for (size_t p = offsets[row]; p < offsets[row + 1]; p++, result_pos++)
            {
                const float value = results[result_pos];
                if (!std::isfinite(value))
                {
                    Warning("-gpu transfers: non-finite result; using the CPU path");
                    RollbackGpuTransfers(old_total, old_index_bytes, old_data_bytes);
                    return false;
                }
                if (value > 0.0f)
                {
                    row_indices.push_back(emitters[p]);
                    row_values.push_back(value);
                }
            }
            StoreTransferScales(&g_patches[row], row_indices.data(), row_values.data(),
                                (unsigned)row_indices.size());
            g_total_transfer += row_indices.size();
        }
    }

    Log("  %.2fM candidates, %zu dispatches (%.2f seconds)\n",
        directed_pairs / 1000000.0, dispatches, I_FloatTime() - started);
    return true;
#endif
}

//
// end old vismat.c
////////////////////////////

void            MakeScalesSparseVismatrix()
{
    char            transferfile[_MAX_PATH];

    hlassume(g_num_patches < MAX_SPARSE_VISMATRIX_PATCHES, assume_MAX_PATCHES);

	safe_snprintf(transferfile, _MAX_PATH, "%s.inc", g_Mapname);

    if (!g_incremental || !readtransfers(transferfile, g_num_patches))
    {
        // determine visibility between g_patches
        BuildVisMatrix();
        DumpVismatrixInfo();
        g_CheckVisBit = CheckVisBitSparse;

        CreateFinalTransparencyArrays("custom shadow array");
        
	if(g_rgb_transfers)
		{NamedRunThreadsOn(g_num_patches, g_estimate, MakeRGBScales);}
	else if (!MakeScalesSparseGpu())
		{NamedRunThreadsOn(g_num_patches, g_estimate, MakeScales);}
        FreeVisMatrix();
        FreeTransparencyArrays();

        if (g_incremental)
        {
            writetransfers(transferfile, g_num_patches);
        }
        else
        {
            _unlink(transferfile);
        }
        // release visibility matrix
        DumpTransfersMemoryUsage();
		CreateFinalStyleArrays ("dynamic shadow array");
    }
}

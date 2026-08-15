#pragma once

#include <cstdint>
#include <string>
#include <vector>

// the vulkan compute lighting backend everything here is host side plumbing
// with no vulkan types exposed,
// so the rest of rad can compile without the backend (hltools_gpu=off)

namespace rad
{
    namespace gpu
    {
        // lazily creates the d3d12 device on first call prefers the
        // high performance adapter (discrete over integrated); index overrides
        // via adapter_override (-1 = automatic)
        bool available();
        void set_adapter_override(int index);
        std::string device_name();
        // last initialization or dispatch error, for the fallback warning
        std::string last_error();

        // ===== batched bsp tracing =====

        // mirrors rad::tnode in an explicitly padded 32 byte layout
        struct tnode_gpu
        {
            float normal[3];
            float dist;
            int32_t type;
            int32_t children[2];
            int32_t pad = 0;
        };
        static_assert(sizeof(tnode_gpu) == 32, "must match the hlsl Tnode layout");

        struct trace_segment
        {
            float start[3];
            float stop[3];
        };
        static_assert(sizeof(trace_segment) == 24, "must match the hlsl Segment layout");

        // contents result per segment plus the sky entry point when the result
        // is contents_sky; flags bit 0 marks segments that took the rare
        // in plane fallback path (its sky vs solid ordering is approximated,
        // see trace_bsphlsl), bit 1 a traversal stack overflow
        struct trace_result
        {
            int32_t contents;
            int32_t flags;
            float skyhit[3];
            int32_t pad = 0;
        };
        static_assert(sizeof(trace_result) == 24, "must match the hlsl Result layout");

        bool trace_batch(const std::vector<tnode_gpu> &tnodes,
                         const std::vector<trace_segment> &segments,
                         std::vector<trace_result> &results);

        // ===== direct lighting gather =====
        // mirrors of the std430 structs in shaders/gathercomp; all layouts
        // are scalar only on purpose (std430 vec3/ivec2 alignment)

        constexpr int max_compact_styles = 16;

        struct lightleaf_gpu
        {
            int32_t leafnum;
            int32_t firstlight;
            int32_t numlights;
            int32_t pad = 0;
        };
        static_assert(sizeof(lightleaf_gpu) == 16, "must match the glsl LightLeaf layout");

        struct light_gpu
        {
            int32_t type;         // emit_type
            int32_t compact_style; // host assigned compact style slot
            int32_t topatch;
            int32_t sun_ofs;      // into the sun normal array, 4 floats per entry
            int32_t sun_count;
            int32_t pad0 = 0;
            float origin[3];
            float intensity[3];
            float normal[3];
            float stopdot;
            float stopdot2;
            float fade;
            float texlightgap;
            float patch_area;
            float patch_emitter_range;
            float diffuse[3];
            float diffuse2[3];
            float pad1 = 0;
        };
        static_assert(sizeof(light_gpu) == 112, "must match the glsl Light layout");

        struct work_item_gpu
        {
            float pos[3];
            float normal[3];
            float cone_power; // lightingconeinfo[miptex]
            float cone_scale;
            int32_t pvs_row;
            int32_t step;
            int32_t face; // index into the per face texlightgap axis table
            // Whether this sample's PVS reaches any sky-touching leaf. The CPU
            // gather skips both sky loops when it does not - thousands of rays
            // per sample that can only come back occluded - and the kernel has
            // to do the same or it is not measuring the same work.
            int32_t sky_reach = 1;
        };
        static_assert(sizeof(work_item_gpu) == 48, "must match the glsl WorkItem layout");

        struct gather_result_gpu
        {
            float adds[3 * max_compact_styles];
            uint32_t touched;
            uint32_t pad0 = 0, pad1 = 0, pad2 = 0;
        };
        static_assert(sizeof(gather_result_gpu) == 208, "must match the glsl GatherResult layout");

        // the per map immutable inputs, uploaded once per gather session
        struct gather_scene
        {
            std::vector<tnode_gpu> tnodes;
            std::vector<lightleaf_gpu> lightleafs;
            std::vector<light_gpu> lights;
            std::vector<float> sun_normals;  // 4 floats per entry (xyz + weight)
            std::vector<float> sky_normals;  // 4 floats per entry, active sky level only
            std::vector<float> face_gap;     // 6 floats per face (texlightgap axes)
            uint32_t sky_lighting_fix = 0;
            int32_t sky_step_match = 0;
            float indirect_sun = 0;
        };

        // a (work item, light) pair the kernel routed to the cpu (the surface
        // near branch: sight area blend + alternate origin)
        struct near_pair
        {
            uint32_t item;
            uint32_t light;
        };

        // gather_begin uploads the scene once and keeps
        // it resident across the chunked dispatches (it also builds the
        // per pvs row visible lit leaf lists host side, and refuses maps
        // whose bsp tree is deeper than the kernel's trace stack); each
        // gather_batch call then only uploads count <= max_chunk_items work
        // items and reads its results back gather_end releases everything
        // (idempotent, safe on failure paths) requires available() the
        // default kernel normalizes in plain float and matches the validation
        // results from the fp64 parity version while needing no shaderfloat64
        // hltools_gpu_fp64_normalize=1 selects the fp64 vectornormalize parity
        // variant for debugging (requires shaderfloat64)
        bool gather_begin(const gather_scene &scene,
                          const std::vector<uint32_t> &pvs_words,
                          uint32_t pvs_stride_words,
                          uint32_t max_chunk_items);
        bool gather_batch(const work_item_gpu *items, size_t count,
                          std::vector<gather_result_gpu> &results,
                          std::vector<near_pair> &near_pairs);
        void gather_end();

        // one call convenience for cpu and gpu comparison tests
        bool gather_batch(const gather_scene &scene,
                          const std::vector<work_item_gpu> &items,
                          const std::vector<uint32_t> &pvs_words,
                          uint32_t pvs_stride_words,
                          std::vector<gather_result_gpu> &results,
                          std::vector<near_pair> &near_pairs);

        // ===== patch to patch transfer factors =====

        struct patch_gpu
        {
            float origin[3];
            float normal[3];
            float area;
            float emitter_range;
            float exposure;
            float cone_power;
            float cone_scale;
            float pad0 = 0;
            int32_t skylevel;
            int32_t wind_ofs;
            int32_t wind_count;
            int32_t pad1 = 0;
        };
        static_assert(sizeof(patch_gpu) == 64, "must match the glsl PatchData layout");

        struct transfer_pair
        {
            int32_t receiver;
            int32_t emitter;
        };
        static_assert(sizeof(transfer_pair) == 8, "must match the glsl Pair layout");

        struct formfactor_scene
        {
            std::vector<patch_gpu> patches;
            std::vector<float> windings;      // 3 floats per vertex
            std::vector<float> sky_normals;   // 4 floats per normal, all levels
            std::vector<int32_t> sky_levels;  // (offset, count) per level
        };

        // Uploads immutable patch/winding/sky data once and reuses the pair,
        // result and readback buffers across all transfer batches.
        bool formfactor_begin(const formfactor_scene &scene, uint32_t max_pairs);
        // Submit/collect are split so the CPU can construct the next pair batch
        // while the previous dispatch is running. Slots 0 and 1 are independent
        // and may both be in flight at once.
        bool formfactor_submit(const transfer_pair *pairs, size_t count,
                               uint32_t slot);
        bool formfactor_collect(uint32_t slot, std::vector<float> &trans);
        bool formfactor_batch(const transfer_pair *pairs, size_t count,
                              std::vector<float> &trans);
        void formfactor_end();

        // One-shot convenience used by parity tests. NaN marks pairs the host
        // must recompute (emitter winding too large for the kernel).
        bool formfactor_batch(const formfactor_scene &scene,
                              const std::vector<transfer_pair> &pairs,
                              std::vector<float> &trans);

        // ===== style-0 radiosity bounce accumulation =====
        // The first GPU bounce kernel deliberately models only the stable
        // style-0 path: RGB/custom-shadow/transparency/style remapping stay on
        // the reference CPU path.  The host decides whether a map qualifies,
        // so unsupported maps fall back without changing their output.
        struct bounce_patch
        {
            float direct[3];
            float total[3];
            float reflectivity[3];
        };
        static_assert(sizeof(bounce_patch) == 36, "must match the GLSL BouncePatch layout");

        struct bounce_result
        {
            float total[3];
            float pad = 0.0f;
        };
        static_assert(sizeof(bounce_result) == 16, "must match the GLSL BounceResult layout");

        bool bounce_batch(const std::vector<bounce_patch> &patches,
                          const std::vector<uint32_t> &row_offsets,
                          const std::vector<uint32_t> &emitters,
                          const std::vector<float> &factors,
                          std::vector<bounce_result> &results);
    }
}

#ifndef GPU_GATHER_H__
#define GPU_GATHER_H__

#if _MSC_VER >= 1000
#pragma once
#endif

#include "cmdlib.h"
#include "mathlib.h"

// -gpu: run the direct lighting gather as a Vulkan compute dispatch instead of
// on the CPU. Everything outside the gather itself - sample placement, the
// lmcache blur, bounces, the final blend - is the untouched CPU code.
//
// BuildFacelights runs twice. The collect pass records every GatherSampleLight
// call it would make as a work item and throws the face's side effects away;
// the GPU then evaluates all of them at once; the consume pass replays the
// same code path and feeds each call its stored result through the same
// coring/style-slot tail the CPU gather ends with.
//
// Anything the kernel does not implement - opaque entities, studio shadows,
// more distinct light styles than it has slots, a BSP deeper than its trace
// stack - makes GpuGatherRun() decline, and RAD runs the CPU path unchanged.

extern bool     g_gpu;                                     // -gpu was given
extern bool     g_gpu_auto;                                // choose CPU for small workloads
extern bool     g_gpu_gather;                              // direct-light phase enabled
extern bool     g_gpu_transfers;                           // sparse form-factor phase enabled
extern int      g_gpu_adapter;                             // -gpuadapter, -1 = automatic

// 0 = the GPU path is not running, 1 = collect pass, 2 = consume pass.
extern int      g_gpu_phase;

// Runs the whole BuildFacelights phase on the GPU path. Returns false without
// having touched anything if the map or the machine is outside what the kernel
// covers, in which case RAD runs its ordinary CPU pass.
extern bool     GpuBuildFacelights();
extern void     GpuGatherBeginFace(int facenum);
extern void     GpuGatherIntercept(const vec3_t pos, const byte* const pvs, const vec3_t normal,
                                   vec3_t* sample, byte* styles, int step, int miptex,
                                   int texlightgap_surfacenum);
// Drops the next stored result for the current face into a sample/styles pair,
// through the same coring and style-slot tail the CPU gather ends with.
extern void     GpuGatherApply(vec3_t* sample, byte* styles);
extern const char* GpuDeviceDescription();

// Provided by lightmap.cpp, which owns the state the marshaller needs.
struct directlight_s;
extern struct directlight_s* RadGpuDirectLights(int leafnum);
extern void     RadGpuResetFace(int facenum);
extern void     RadGpuTexToWorld(int surfacenum, vec3_t textoworld[2]);
extern bool     RadGpuSampleMayReachSky(const byte* const pvs);

// The two halves of BuildFacelights, so the expensive first one runs once.
extern bool     RadGpuFaceBegin(int facenum, void* state);
extern void     RadGpuFaceEnd(int facenum, void* state);
extern void     RadGpuFaceAbandon(void* state);
extern size_t   RadGpuFaceStateSize();
extern size_t   RadGpuFaceStateBytes(void* state);

// Provided by trace.cpp: the tnodes, in the flat layout the kernel expects.
typedef struct
{
    float           normal[3];
    float           dist;
    int             type;
    int             children[2];
    int             pad;
}
gputnode_t;

extern int      ExportTnodes(gputnode_t* out, int max);

#endif // GPU_GATHER_H__

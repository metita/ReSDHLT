# Third-party code in `src/sdhlt/sdHLRAD/gpu/`

## Vulkan headers — `vulkan/`, `vk_video/`

Unmodified copies of `vulkan_core.h`, `vk_platform.h` and the `vk_video/`
headers from [KhronosGroup/Vulkan-Headers](https://github.com/KhronosGroup/Vulkan-Headers),
tag `v1.4.309`. Licensed **Apache-2.0 OR MIT**.

`vulkan/vulkan.h` is not from that repository: it is a three-line stand-in that
includes the two core headers and nothing else, because the real one pulls in
every windowing-system extension header and a headless compute backend can use
none of them.

They are vendored so that building ReSDHLT needs no Vulkan SDK. Nothing links
against a Vulkan library either — `gpu.cpp` defines `VK_NO_PROTOTYPES` and
resolves every entry point through `vkGetInstanceProcAddr` from the loader that
ships with the graphics driver, so a machine with no driver gets a clean error
string and the CPU path.

## Compute backend — `gpu.cpp`, `gpu.h`, `shaders/`

Ported from [speedrun-16/hltools](https://github.com/speedrun-16/hltools)
(`src/rad/gpu/`), which is **GPL-2.0**, the same licence as this project.

Changes made here: the environment variables are `SDHLT_GPU_*` instead of
`HLTOOLS_GPU_*`, the generated SPIR-V headers live under `spirv/` and are
committed rather than produced at build time, `tnode_tree_depth()` was rewritten
without structured bindings so the backend builds at the same C++ standard as
the rest of the tree, and the gather kernel's `WorkItem` carries a `skyReach`
flag so it takes the same PVS shortcut as this fork's CPU gather.

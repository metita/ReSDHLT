#ifndef VULKAN_H_
#define VULKAN_H_ 1

/*
 * A minimal stand-in for the Khronos <vulkan/vulkan.h>.
 *
 * The real header pulls in every windowing-system extension header, none of
 * which a headless compute backend can use. gpu.cpp defines VK_NO_PROTOTYPES
 * and resolves everything through vkGetInstanceProcAddr, so the core header
 * plus the platform typedefs is the entire dependency - which is why they are
 * vendored here rather than requiring the Vulkan SDK to build the compile
 * tools.
 *
 * vulkan_core.h and vk_platform.h are unmodified copies from
 * KhronosGroup/Vulkan-Headers v1.4.309, Apache-2.0 OR MIT.
 */

#include "vk_platform.h"
#include "vulkan_core.h"

#endif // VULKAN_H_

#include "gpu.h"

#include <cstdlib>
#include <cstring>
#include <mutex>
#include <utility>
#include <vector>

#define VK_NO_PROTOTYPES
#include <vulkan/vulkan.h>

#ifdef _WIN32
#include <windows.h>
#else
#include <dlfcn.h>
#endif

#include "spirv/trace_bsp_spirv.h" // generated: g_trace_bsp_spirv[]
#include "spirv/gather_spirv.h"    // generated: g_gather_spirv[]
// One float-normalize gather kernel per traversal-stack depth. The stack is
// seven per-thread arrays, so the shallowest variant that still covers the
// map's real BSP depth is the one that keeps the most threads resident.
#include "spirv/gather_f32_d16_spirv.h"
#include "spirv/gather_f32_d24_spirv.h"
#include "spirv/gather_f32_d32_spirv.h"
#include "spirv/gather_f32_d48_spirv.h"
#include "spirv/gather_f32_d64_spirv.h"
#include "spirv/formfactor_spirv.h" // generated: g_formfactor_spirv[]

// vulkan host plumbing for the gpu lighting backend the loader is opened
// dynamically from the vulkan loader libraries that ship with the gpu
// driver), so building needs only the vendored khronos headers and running
// without a vulkan driver degrades to a clean error string that lets the
// rad stage fall back to the cpu path with a warning

namespace rad
{
    namespace gpu
    {
        namespace
        {
            // the narrow slice of the api we call, resolved at runtime
            struct vk_functions
            {
                PFN_vkGetInstanceProcAddr GetInstanceProcAddr = nullptr;
                PFN_vkCreateInstance CreateInstance = nullptr;
                PFN_vkEnumeratePhysicalDevices EnumeratePhysicalDevices = nullptr;
                PFN_vkGetPhysicalDeviceProperties GetPhysicalDeviceProperties = nullptr;
                PFN_vkGetPhysicalDeviceFeatures GetPhysicalDeviceFeatures = nullptr;
                PFN_vkGetPhysicalDeviceQueueFamilyProperties GetPhysicalDeviceQueueFamilyProperties = nullptr;
                PFN_vkGetPhysicalDeviceMemoryProperties GetPhysicalDeviceMemoryProperties = nullptr;
                PFN_vkCreateDevice CreateDevice = nullptr;
                PFN_vkGetDeviceProcAddr GetDeviceProcAddr = nullptr;

                PFN_vkGetDeviceQueue GetDeviceQueue = nullptr;
                PFN_vkCreateBuffer CreateBuffer = nullptr;
                PFN_vkDestroyBuffer DestroyBuffer = nullptr;
                PFN_vkGetBufferMemoryRequirements GetBufferMemoryRequirements = nullptr;
                PFN_vkAllocateMemory AllocateMemory = nullptr;
                PFN_vkFreeMemory FreeMemory = nullptr;
                PFN_vkBindBufferMemory BindBufferMemory = nullptr;
                PFN_vkMapMemory MapMemory = nullptr;
                PFN_vkUnmapMemory UnmapMemory = nullptr;
                PFN_vkCreateShaderModule CreateShaderModule = nullptr;
                PFN_vkCreateDescriptorSetLayout CreateDescriptorSetLayout = nullptr;
                PFN_vkCreatePipelineLayout CreatePipelineLayout = nullptr;
                PFN_vkCreateComputePipelines CreateComputePipelines = nullptr;
                PFN_vkCreateDescriptorPool CreateDescriptorPool = nullptr;
                PFN_vkDestroyDescriptorPool DestroyDescriptorPool = nullptr;
                PFN_vkAllocateDescriptorSets AllocateDescriptorSets = nullptr;
                PFN_vkUpdateDescriptorSets UpdateDescriptorSets = nullptr;
                PFN_vkResetDescriptorPool ResetDescriptorPool = nullptr;
                PFN_vkCreateCommandPool CreateCommandPool = nullptr;
                PFN_vkResetCommandPool ResetCommandPool = nullptr;
                PFN_vkAllocateCommandBuffers AllocateCommandBuffers = nullptr;
                PFN_vkBeginCommandBuffer BeginCommandBuffer = nullptr;
                PFN_vkEndCommandBuffer EndCommandBuffer = nullptr;
                PFN_vkCmdBindPipeline CmdBindPipeline = nullptr;
                PFN_vkCmdBindDescriptorSets CmdBindDescriptorSets = nullptr;
                PFN_vkCmdPushConstants CmdPushConstants = nullptr;
                PFN_vkCmdDispatch CmdDispatch = nullptr;
                PFN_vkCmdCopyBuffer CmdCopyBuffer = nullptr;
                PFN_vkCmdPipelineBarrier CmdPipelineBarrier = nullptr;
                PFN_vkCreateFence CreateFence = nullptr;
                PFN_vkResetFences ResetFences = nullptr;
                PFN_vkWaitForFences WaitForFences = nullptr;
                PFN_vkQueueSubmit QueueSubmit = nullptr;
            };

            struct device_state
            {
                void *library = nullptr;
                vk_functions fn;
                VkInstance instance = VK_NULL_HANDLE;
                VkPhysicalDevice physical = VK_NULL_HANDLE;
                VkPhysicalDeviceMemoryProperties memory_properties = {};
                uint32_t queue_family = 0;
                VkDevice device = VK_NULL_HANDLE;
                VkQueue queue = VK_NULL_HANDLE;
                VkCommandPool command_pool = VK_NULL_HANDLE;
                VkCommandBuffer command_buffer = VK_NULL_HANDLE;
                VkFence fence = VK_NULL_HANDLE;
                VkDescriptorPool descriptor_pool = VK_NULL_HANDLE;

                // cached trace pipeline
                VkDescriptorSetLayout trace_set_layout = VK_NULL_HANDLE;
                VkPipelineLayout trace_pipeline_layout = VK_NULL_HANDLE;
                VkPipeline trace_pipeline = VK_NULL_HANDLE;

                // cached gather pipeline
                VkDescriptorSetLayout gather_set_layout = VK_NULL_HANDLE;
                VkPipelineLayout gather_pipeline_layout = VK_NULL_HANDLE;
                VkPipeline gather_pipeline = VK_NULL_HANDLE;

                // cached gather pipeline for float normalization without shaderfloat64
                VkDescriptorSetLayout gather_f32_set_layout = VK_NULL_HANDLE;
                VkPipelineLayout gather_f32_pipeline_layout = VK_NULL_HANDLE;
                VkPipeline gather_f32_pipeline = VK_NULL_HANDLE;

                // cached form factor pipeline
                VkDescriptorSetLayout ff_set_layout = VK_NULL_HANDLE;
                VkPipelineLayout ff_pipeline_layout = VK_NULL_HANDLE;
                VkPipeline ff_pipeline = VK_NULL_HANDLE;

                bool has_float64 = false;
                int gather_f32_depth = 0;   // stack depth the cached kernel was built for

                std::string name;
                std::string error;
                int adapter_override = -1;
                bool initialized = false;
                bool ok = false;
            };

            device_state g;
            std::mutex g_mutex;

            bool env_flag(const char *name)
            {
                const char *v = std::getenv(name);
                return v && v[0] && v[0] != '0';
            }

            void *load_loader_symbol(const char *name)
            {
#ifdef _WIN32
                return (void *)GetProcAddress((HMODULE)g.library, name);
#else
                return dlsym(g.library, name);
#endif
            }

            bool open_loader()
            {
#ifdef _WIN32
                g.library = (void *)LoadLibraryW(L"vulkan-1.dll");
#else
                g.library = dlopen("libvulkan.so.1", RTLD_NOW | RTLD_LOCAL);
                if (!g.library)
                    g.library = dlopen("libvulkan.so", RTLD_NOW | RTLD_LOCAL);
#endif
                if (!g.library)
                {
                    g.error = "vulkan loader not found (no vulkan-capable driver installed)";
                    return false;
                }
                g.fn.GetInstanceProcAddr =
                    (PFN_vkGetInstanceProcAddr)load_loader_symbol("vkGetInstanceProcAddr");
                if (!g.fn.GetInstanceProcAddr)
                {
                    g.error = "vkGetInstanceProcAddr not found in the vulkan loader";
                    return false;
                }
                return true;
            }

            bool init_device_locked()
            {
                if (!open_loader())
                    return false;

                g.fn.CreateInstance =
                    (PFN_vkCreateInstance)g.fn.GetInstanceProcAddr(VK_NULL_HANDLE, "vkCreateInstance");
                if (!g.fn.CreateInstance)
                {
                    g.error = "vkCreateInstance not resolvable";
                    return false;
                }

                VkApplicationInfo app = {VK_STRUCTURE_TYPE_APPLICATION_INFO};
                app.pApplicationName = "ReSDHLT RAD";
                app.apiVersion = VK_API_VERSION_1_0;

                VkInstanceCreateInfo ici = {VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO};
                ici.pApplicationInfo = &app;
                const char *validation = "VK_LAYER_KHRONOS_validation";
                if (env_flag("SDHLT_GPU_DEBUG"))
                {
                    ici.enabledLayerCount = 1;
                    ici.ppEnabledLayerNames = &validation;
                }
                VkResult res = g.fn.CreateInstance(&ici, nullptr, &g.instance);
                if (res != VK_SUCCESS && ici.enabledLayerCount)
                {
                    // validation layer not installed; retry without it
                    ici.enabledLayerCount = 0;
                    res = g.fn.CreateInstance(&ici, nullptr, &g.instance);
                }
                if (res != VK_SUCCESS)
                {
                    g.error = "vkCreateInstance failed";
                    return false;
                }

                auto instance_fn = [&](const char *name)
                {
                    return g.fn.GetInstanceProcAddr(g.instance, name);
                };
                g.fn.EnumeratePhysicalDevices =
                    (PFN_vkEnumeratePhysicalDevices)instance_fn("vkEnumeratePhysicalDevices");
                g.fn.GetPhysicalDeviceProperties =
                    (PFN_vkGetPhysicalDeviceProperties)instance_fn("vkGetPhysicalDeviceProperties");
                g.fn.GetPhysicalDeviceFeatures =
                    (PFN_vkGetPhysicalDeviceFeatures)instance_fn("vkGetPhysicalDeviceFeatures");
                g.fn.GetPhysicalDeviceQueueFamilyProperties =
                    (PFN_vkGetPhysicalDeviceQueueFamilyProperties)instance_fn("vkGetPhysicalDeviceQueueFamilyProperties");
                g.fn.GetPhysicalDeviceMemoryProperties =
                    (PFN_vkGetPhysicalDeviceMemoryProperties)instance_fn("vkGetPhysicalDeviceMemoryProperties");
                g.fn.CreateDevice = (PFN_vkCreateDevice)instance_fn("vkCreateDevice");
                g.fn.GetDeviceProcAddr = (PFN_vkGetDeviceProcAddr)instance_fn("vkGetDeviceProcAddr");
                if (!g.fn.EnumeratePhysicalDevices || !g.fn.GetPhysicalDeviceProperties
                    || !g.fn.GetPhysicalDeviceQueueFamilyProperties
                    || !g.fn.GetPhysicalDeviceMemoryProperties
                    || !g.fn.CreateDevice || !g.fn.GetDeviceProcAddr)
                {
                    g.error = "vulkan instance functions not resolvable";
                    return false;
                }

                uint32_t count = 0;
                g.fn.EnumeratePhysicalDevices(g.instance, &count, nullptr);
                if (!count)
                {
                    g.error = "no vulkan physical devices";
                    return false;
                }
                std::vector<VkPhysicalDevice> devices(count);
                g.fn.EnumeratePhysicalDevices(g.instance, &count, devices.data());

                // pick the best usable device: discrete > integrated > other;
                // cpu implementations (llvmpipe, swiftshader) only when asked
                const bool allow_cpu = env_flag("SDHLT_GPU_WARP");
                int best_score = -1;
                VkPhysicalDeviceProperties best_props = {};
                for (uint32_t i = 0; i < count; i++)
                {
                    if (g.adapter_override >= 0 && (int)i != g.adapter_override)
                        continue;
                    VkPhysicalDeviceProperties props = {};
                    g.fn.GetPhysicalDeviceProperties(devices[i], &props);
                    if (props.deviceType == VK_PHYSICAL_DEVICE_TYPE_CPU && !allow_cpu)
                        continue;

                    uint32_t qcount = 0;
                    g.fn.GetPhysicalDeviceQueueFamilyProperties(devices[i], &qcount, nullptr);
                    std::vector<VkQueueFamilyProperties> families(qcount);
                    g.fn.GetPhysicalDeviceQueueFamilyProperties(devices[i], &qcount, families.data());
                    int family = -1;
                    for (uint32_t q = 0; q < qcount; q++)
                    {
                        if (families[q].queueFlags & VK_QUEUE_COMPUTE_BIT)
                        {
                            family = (int)q;
                            break;
                        }
                    }
                    if (family < 0)
                        continue;

                    int score = 0;
                    switch (props.deviceType)
                    {
                    case VK_PHYSICAL_DEVICE_TYPE_DISCRETE_GPU: score = 3; break;
                    case VK_PHYSICAL_DEVICE_TYPE_INTEGRATED_GPU: score = 2; break;
                    case VK_PHYSICAL_DEVICE_TYPE_VIRTUAL_GPU: score = 1; break;
                    default: score = 0; break;
                    }
                    if (score > best_score)
                    {
                        best_score = score;
                        g.physical = devices[i];
                        g.queue_family = (uint32_t)family;
                        best_props = props;
                    }
                }
                if (best_score < 0)
                {
                    g.error = g.adapter_override >= 0
                        ? "requested gpu adapter not usable (needs a vulkan compute queue)"
                        : "no usable vulkan device with a compute queue";
                    return false;
                }
                g.name = best_props.deviceName;
                g.fn.GetPhysicalDeviceMemoryProperties(g.physical, &g.memory_properties);

                float priority = 1.0f;
                VkDeviceQueueCreateInfo qci = {VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO};
                qci.queueFamilyIndex = g.queue_family;
                qci.queueCount = 1;
                qci.pQueuePriorities = &priority;

                // the gather kernel replicates vectornormalize's double
                // sqrt/divide, so enable fp64 when the device has it
                VkPhysicalDeviceFeatures supported = {};
                if (g.fn.GetPhysicalDeviceFeatures)
                    g.fn.GetPhysicalDeviceFeatures(g.physical, &supported);
                VkPhysicalDeviceFeatures enabled = {};
                enabled.shaderFloat64 = supported.shaderFloat64;
                g.has_float64 = supported.shaderFloat64 == VK_TRUE;

                VkDeviceCreateInfo dci = {VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO};
                dci.queueCreateInfoCount = 1;
                dci.pQueueCreateInfos = &qci;
                dci.pEnabledFeatures = &enabled;
                if (g.fn.CreateDevice(g.physical, &dci, nullptr, &g.device) != VK_SUCCESS)
                {
                    g.error = "vkCreateDevice failed";
                    return false;
                }

                auto device_fn = [&](const char *name)
                {
                    return g.fn.GetDeviceProcAddr(g.device, name);
                };
                g.fn.GetDeviceQueue = (PFN_vkGetDeviceQueue)device_fn("vkGetDeviceQueue");
                g.fn.CreateBuffer = (PFN_vkCreateBuffer)device_fn("vkCreateBuffer");
                g.fn.DestroyBuffer = (PFN_vkDestroyBuffer)device_fn("vkDestroyBuffer");
                g.fn.GetBufferMemoryRequirements =
                    (PFN_vkGetBufferMemoryRequirements)device_fn("vkGetBufferMemoryRequirements");
                g.fn.AllocateMemory = (PFN_vkAllocateMemory)device_fn("vkAllocateMemory");
                g.fn.FreeMemory = (PFN_vkFreeMemory)device_fn("vkFreeMemory");
                g.fn.BindBufferMemory = (PFN_vkBindBufferMemory)device_fn("vkBindBufferMemory");
                g.fn.MapMemory = (PFN_vkMapMemory)device_fn("vkMapMemory");
                g.fn.UnmapMemory = (PFN_vkUnmapMemory)device_fn("vkUnmapMemory");
                g.fn.CreateShaderModule = (PFN_vkCreateShaderModule)device_fn("vkCreateShaderModule");
                g.fn.CreateDescriptorSetLayout =
                    (PFN_vkCreateDescriptorSetLayout)device_fn("vkCreateDescriptorSetLayout");
                g.fn.CreatePipelineLayout = (PFN_vkCreatePipelineLayout)device_fn("vkCreatePipelineLayout");
                g.fn.CreateComputePipelines =
                    (PFN_vkCreateComputePipelines)device_fn("vkCreateComputePipelines");
                g.fn.CreateDescriptorPool = (PFN_vkCreateDescriptorPool)device_fn("vkCreateDescriptorPool");
                g.fn.DestroyDescriptorPool =
                    (PFN_vkDestroyDescriptorPool)device_fn("vkDestroyDescriptorPool");
                g.fn.AllocateDescriptorSets =
                    (PFN_vkAllocateDescriptorSets)device_fn("vkAllocateDescriptorSets");
                g.fn.UpdateDescriptorSets = (PFN_vkUpdateDescriptorSets)device_fn("vkUpdateDescriptorSets");
                g.fn.ResetDescriptorPool = (PFN_vkResetDescriptorPool)device_fn("vkResetDescriptorPool");
                g.fn.CreateCommandPool = (PFN_vkCreateCommandPool)device_fn("vkCreateCommandPool");
                g.fn.ResetCommandPool = (PFN_vkResetCommandPool)device_fn("vkResetCommandPool");
                g.fn.AllocateCommandBuffers =
                    (PFN_vkAllocateCommandBuffers)device_fn("vkAllocateCommandBuffers");
                g.fn.BeginCommandBuffer = (PFN_vkBeginCommandBuffer)device_fn("vkBeginCommandBuffer");
                g.fn.EndCommandBuffer = (PFN_vkEndCommandBuffer)device_fn("vkEndCommandBuffer");
                g.fn.CmdBindPipeline = (PFN_vkCmdBindPipeline)device_fn("vkCmdBindPipeline");
                g.fn.CmdBindDescriptorSets =
                    (PFN_vkCmdBindDescriptorSets)device_fn("vkCmdBindDescriptorSets");
                g.fn.CmdPushConstants = (PFN_vkCmdPushConstants)device_fn("vkCmdPushConstants");
                g.fn.CmdDispatch = (PFN_vkCmdDispatch)device_fn("vkCmdDispatch");
                g.fn.CmdCopyBuffer = (PFN_vkCmdCopyBuffer)device_fn("vkCmdCopyBuffer");
                g.fn.CmdPipelineBarrier = (PFN_vkCmdPipelineBarrier)device_fn("vkCmdPipelineBarrier");
                g.fn.CreateFence = (PFN_vkCreateFence)device_fn("vkCreateFence");
                g.fn.ResetFences = (PFN_vkResetFences)device_fn("vkResetFences");
                g.fn.WaitForFences = (PFN_vkWaitForFences)device_fn("vkWaitForFences");
                g.fn.QueueSubmit = (PFN_vkQueueSubmit)device_fn("vkQueueSubmit");

                g.fn.GetDeviceQueue(g.device, g.queue_family, 0, &g.queue);

                VkCommandPoolCreateInfo cpci = {VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO};
                cpci.queueFamilyIndex = g.queue_family;
                if (g.fn.CreateCommandPool(g.device, &cpci, nullptr, &g.command_pool) != VK_SUCCESS)
                {
                    g.error = "vkCreateCommandPool failed";
                    return false;
                }
                VkCommandBufferAllocateInfo cbai = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO};
                cbai.commandPool = g.command_pool;
                cbai.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
                cbai.commandBufferCount = 1;
                if (g.fn.AllocateCommandBuffers(g.device, &cbai, &g.command_buffer) != VK_SUCCESS)
                {
                    g.error = "vkAllocateCommandBuffers failed";
                    return false;
                }
                VkFenceCreateInfo fci = {VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
                if (g.fn.CreateFence(g.device, &fci, nullptr, &g.fence) != VK_SUCCESS)
                {
                    g.error = "vkCreateFence failed";
                    return false;
                }
                return true;
            }

            bool ensure_device_locked()
            {
                if (!g.initialized)
                {
                    g.initialized = true;
                    g.ok = init_device_locked();
                }
                return g.ok;
            }

            struct gpu_buffer
            {
                VkBuffer buffer = VK_NULL_HANDLE;
                VkDeviceMemory memory = VK_NULL_HANDLE;
                void *mapped = nullptr; // only for host visible buffers

                void destroy()
                {
                    if (buffer)
                        g.fn.DestroyBuffer(g.device, buffer, nullptr);
                    if (memory)
                    {
                        if (mapped)
                            g.fn.UnmapMemory(g.device, memory);
                        g.fn.FreeMemory(g.device, memory, nullptr);
                    }
                    buffer = VK_NULL_HANDLE;
                    memory = VK_NULL_HANDLE;
                    mapped = nullptr;
                }
            };

            int find_memory_type(uint32_t type_bits, VkMemoryPropertyFlags wanted)
            {
                for (uint32_t i = 0; i < g.memory_properties.memoryTypeCount; i++)
                {
                    if ((type_bits & (1u << i))
                        && (g.memory_properties.memoryTypes[i].propertyFlags & wanted) == wanted)
                        return (int)i;
                }
                return -1;
            }

            bool create_buffer(gpu_buffer &out, VkDeviceSize size, VkBufferUsageFlags usage,
                               VkMemoryPropertyFlags properties, bool map)
            {
                VkBufferCreateInfo bci = {VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO};
                bci.size = size;
                bci.usage = usage;
                bci.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
                if (g.fn.CreateBuffer(g.device, &bci, nullptr, &out.buffer) != VK_SUCCESS)
                    return false;

                VkMemoryRequirements req = {};
                g.fn.GetBufferMemoryRequirements(g.device, out.buffer, &req);
                int type = find_memory_type(req.memoryTypeBits, properties);
                if (type < 0 && (properties & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT))
                {
                    // no device local heap for this buffer (software devices);
                    // any host visible type still works, only slower
                    properties = VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT
                        | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT;
                    type = find_memory_type(req.memoryTypeBits, properties);
                }
                if (type < 0)
                    return false;

                VkMemoryAllocateInfo mai = {VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO};
                mai.allocationSize = req.size;
                mai.memoryTypeIndex = (uint32_t)type;
                if (g.fn.AllocateMemory(g.device, &mai, nullptr, &out.memory) != VK_SUCCESS)
                    return false;
                if (g.fn.BindBufferMemory(g.device, out.buffer, out.memory, 0) != VK_SUCCESS)
                    return false;
                if (!map)
                    return true;
                return g.fn.MapMemory(g.device, out.memory, 0, VK_WHOLE_SIZE, 0, &out.mapped) == VK_SUCCESS;
            }

            bool create_host_buffer(gpu_buffer &out, VkDeviceSize size)
            {
                return create_buffer(out, size, VK_BUFFER_USAGE_STORAGE_BUFFER_BIT,
                                     VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT
                                     | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT, true);
            }

            bool ensure_descriptor_pool_locked()
            {
                if (g.descriptor_pool)
                    return true;
                VkDescriptorPoolSize pool_size = {};
                pool_size.type = VK_DESCRIPTOR_TYPE_STORAGE_BUFFER;
                pool_size.descriptorCount = 64;
                VkDescriptorPoolCreateInfo dpci = {VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO};
                dpci.maxSets = 8;
                dpci.poolSizeCount = 1;
                dpci.pPoolSizes = &pool_size;
                if (g.fn.CreateDescriptorPool(g.device, &dpci, nullptr, &g.descriptor_pool) != VK_SUCCESS)
                {
                    g.error = "vkCreateDescriptorPool failed";
                    return false;
                }
                return true;
            }

            // builds a compute pipeline with `bindings` storage buffers and a
            // push constant range of push_size bytes
            bool create_compute_pipeline(const void *spirv, size_t spirv_size, uint32_t bindings,
                                         uint32_t push_size, const char *what,
                                         VkDescriptorSetLayout &set_layout,
                                         VkPipelineLayout &pipeline_layout, VkPipeline &pipeline)
            {
                std::vector<VkDescriptorSetLayoutBinding> binding_desc(bindings);
                for (uint32_t i = 0; i < bindings; i++)
                {
                    binding_desc[i].binding = i;
                    binding_desc[i].descriptorType = VK_DESCRIPTOR_TYPE_STORAGE_BUFFER;
                    binding_desc[i].descriptorCount = 1;
                    binding_desc[i].stageFlags = VK_SHADER_STAGE_COMPUTE_BIT;
                }
                VkDescriptorSetLayoutCreateInfo dslci = {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO};
                dslci.bindingCount = bindings;
                dslci.pBindings = binding_desc.data();
                if (g.fn.CreateDescriptorSetLayout(g.device, &dslci, nullptr, &set_layout) != VK_SUCCESS)
                {
                    g.error = std::string("vkCreateDescriptorSetLayout failed (") + what + ")";
                    return false;
                }

                VkPushConstantRange push = {};
                push.stageFlags = VK_SHADER_STAGE_COMPUTE_BIT;
                push.size = push_size;

                VkPipelineLayoutCreateInfo plci = {VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO};
                plci.setLayoutCount = 1;
                plci.pSetLayouts = &set_layout;
                plci.pushConstantRangeCount = 1;
                plci.pPushConstantRanges = &push;
                if (g.fn.CreatePipelineLayout(g.device, &plci, nullptr, &pipeline_layout) != VK_SUCCESS)
                {
                    g.error = std::string("vkCreatePipelineLayout failed (") + what + ")";
                    return false;
                }

                VkShaderModuleCreateInfo smci = {VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO};
                smci.codeSize = spirv_size;
                smci.pCode = (const uint32_t *)spirv;
                VkShaderModule module = VK_NULL_HANDLE;
                if (g.fn.CreateShaderModule(g.device, &smci, nullptr, &module) != VK_SUCCESS)
                {
                    g.error = std::string("vkCreateShaderModule failed (") + what + ")";
                    return false;
                }

                VkComputePipelineCreateInfo cpci = {VK_STRUCTURE_TYPE_COMPUTE_PIPELINE_CREATE_INFO};
                cpci.stage.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
                cpci.stage.stage = VK_SHADER_STAGE_COMPUTE_BIT;
                cpci.stage.module = module;
                cpci.stage.pName = "main";
                cpci.layout = pipeline_layout;
                if (g.fn.CreateComputePipelines(g.device, VK_NULL_HANDLE, 1, &cpci, nullptr,
                                                &pipeline) != VK_SUCCESS)
                {
                    g.error = std::string("vkCreateComputePipelines failed (") + what + ")";
                    return false;
                }
                return true;
            }

            bool ensure_trace_pipeline_locked()
            {
                if (g.trace_pipeline)
                    return true;
                return ensure_descriptor_pool_locked()
                    && create_compute_pipeline(g_trace_bsp_spirv, g_trace_bsp_spirv_size, 3, 4,
                                               "trace_bsp", g.trace_set_layout,
                                               g.trace_pipeline_layout, g.trace_pipeline);
            }

            struct gather_variant
            {
                int depth;
                const unsigned char *spirv;
                unsigned int size;
                const char *name;
            };

            const gather_variant *pick_gather_variant(int tree_depth)
            {
                static const gather_variant variants[] = {
                    {16, g_gather_f32_d16_spirv, g_gather_f32_d16_spirv_size, "gather_f32_d16"},
                    {24, g_gather_f32_d24_spirv, g_gather_f32_d24_spirv_size, "gather_f32_d24"},
                    {32, g_gather_f32_d32_spirv, g_gather_f32_d32_spirv_size, "gather_f32_d32"},
                    {48, g_gather_f32_d48_spirv, g_gather_f32_d48_spirv_size, "gather_f32_d48"},
                    {64, g_gather_f32_d64_spirv, g_gather_f32_d64_spirv_size, "gather_f32_d64"},
                };
                for (size_t i = 0; i < sizeof(variants) / sizeof(variants[0]); i++)
                {
                    if (tree_depth <= variants[i].depth)
                        return &variants[i];
                }
                return nullptr;
            }

            bool ensure_gather_pipeline_locked(bool float_normalize, int tree_depth)
            {
                if (float_normalize)
                {
                    const gather_variant *v = pick_gather_variant(tree_depth);
                    if (!v)
                    {
                        g.error = "gather_begin: bsp tree depth " + std::to_string(tree_depth)
                            + " exceeds the deepest kernel variant (64)";
                        return false;
                    }
                    if (g.gather_f32_pipeline && g.gather_f32_depth == v->depth)
                        return true;
                    // A different depth than the cached one: the pipeline is
                    // kept for the life of the process, so build the new one.
                    g.gather_f32_pipeline = VK_NULL_HANDLE;
                    g.gather_f32_depth = v->depth;
                    return ensure_descriptor_pool_locked()
                        && create_compute_pipeline(v->spirv, v->size,
                                                   11, 32, v->name, g.gather_f32_set_layout,
                                                   g.gather_f32_pipeline_layout,
                                                   g.gather_f32_pipeline);
                }
                if (g.gather_pipeline)
                    return true;
                if (!g.has_float64)
                {
                    g.error = "the fp64-parity gather kernel (SDHLT_GPU_FP64_NORMALIZE=1) "
                              "needs the shaderFloat64 device feature; this device lacks it";
                    return false;
                }
                return ensure_descriptor_pool_locked()
                    && create_compute_pipeline(g_gather_spirv, g_gather_spirv_size, 11, 32,
                                               "gather", g.gather_set_layout,
                                               g.gather_pipeline_layout, g.gather_pipeline);
            }

            bool ensure_ff_pipeline_locked()
            {
                if (g.ff_pipeline)
                    return true;
                if (!g.has_float64)
                {
                    g.error = "form-factor kernel needs the shaderFloat64 device feature; "
                              "this device lacks it";
                    return false;
                }
                return ensure_descriptor_pool_locked()
                    && create_compute_pipeline(g_formfactor_spirv, g_formfactor_spirv_size, 6, 4,
                                               "formfactor", g.ff_set_layout,
                                               g.ff_pipeline_layout, g.ff_pipeline);
            }
        }

        bool available()
        {
            std::lock_guard<std::mutex> lock(g_mutex);
            return ensure_device_locked();
        }

        void set_adapter_override(int index)
        {
            std::lock_guard<std::mutex> lock(g_mutex);
            g.adapter_override = index;
        }

        std::string device_name()
        {
            std::lock_guard<std::mutex> lock(g_mutex);
            return g.name;
        }

        std::string last_error()
        {
            std::lock_guard<std::mutex> lock(g_mutex);
            return g.error;
        }

        bool trace_batch(const std::vector<tnode_gpu> &tnodes,
                         const std::vector<trace_segment> &segments,
                         std::vector<trace_result> &results)
        {
            std::lock_guard<std::mutex> lock(g_mutex);
            results.clear();
            if (!ensure_device_locked() || !ensure_trace_pipeline_locked())
                return false;
            if (tnodes.empty() || segments.empty())
            {
                g.error = "trace_batch: empty input";
                return false;
            }

            const VkDeviceSize results_size = segments.size() * sizeof(trace_result);
            gpu_buffer tnode_buf, segment_buf, result_buf, readback_buf;
            auto destroy_all = [&]()
            {
                tnode_buf.destroy();
                segment_buf.destroy();
                result_buf.destroy();
                readback_buf.destroy();
            };
            // inputs are read once each, host visible is fine; the results are
            // written by every thread, so they stage in device local memory
            // and cross the bus once in a copy
            bool ok = create_host_buffer(tnode_buf, tnodes.size() * sizeof(tnode_gpu))
                && create_host_buffer(segment_buf, segments.size() * sizeof(trace_segment))
                && create_buffer(result_buf, results_size,
                                 VK_BUFFER_USAGE_STORAGE_BUFFER_BIT | VK_BUFFER_USAGE_TRANSFER_SRC_BIT,
                                 VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT, false)
                && create_buffer(readback_buf, results_size,
                                 VK_BUFFER_USAGE_TRANSFER_DST_BIT,
                                 VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT
                                 | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT, true);
            if (!ok)
            {
                g.error = "trace_batch: buffer creation failed";
                destroy_all();
                return false;
            }
            std::memcpy(tnode_buf.mapped, tnodes.data(), tnodes.size() * sizeof(tnode_gpu));
            std::memcpy(segment_buf.mapped, segments.data(), segments.size() * sizeof(trace_segment));

            g.fn.ResetDescriptorPool(g.device, g.descriptor_pool, 0);
            VkDescriptorSetAllocateInfo dsai = {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO};
            dsai.descriptorPool = g.descriptor_pool;
            dsai.descriptorSetCount = 1;
            dsai.pSetLayouts = &g.trace_set_layout;
            VkDescriptorSet set = VK_NULL_HANDLE;
            if (g.fn.AllocateDescriptorSets(g.device, &dsai, &set) != VK_SUCCESS)
            {
                g.error = "trace_batch: descriptor set allocation failed";
                destroy_all();
                return false;
            }

            VkDescriptorBufferInfo infos[3] = {
                {tnode_buf.buffer, 0, VK_WHOLE_SIZE},
                {segment_buf.buffer, 0, VK_WHOLE_SIZE},
                {result_buf.buffer, 0, VK_WHOLE_SIZE},
            };
            VkWriteDescriptorSet writes[3] = {};
            for (uint32_t i = 0; i < 3; i++)
            {
                writes[i].sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
                writes[i].dstSet = set;
                writes[i].dstBinding = i;
                writes[i].descriptorCount = 1;
                writes[i].descriptorType = VK_DESCRIPTOR_TYPE_STORAGE_BUFFER;
                writes[i].pBufferInfo = &infos[i];
            }
            g.fn.UpdateDescriptorSets(g.device, 3, writes, 0, nullptr);

            g.fn.ResetCommandPool(g.device, g.command_pool, 0);
            VkCommandBufferBeginInfo cbbi = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
            cbbi.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
            g.fn.BeginCommandBuffer(g.command_buffer, &cbbi);
            g.fn.CmdBindPipeline(g.command_buffer, VK_PIPELINE_BIND_POINT_COMPUTE, g.trace_pipeline);
            g.fn.CmdBindDescriptorSets(g.command_buffer, VK_PIPELINE_BIND_POINT_COMPUTE,
                                       g.trace_pipeline_layout, 0, 1, &set, 0, nullptr);
            const uint32_t count = (uint32_t)segments.size();
            g.fn.CmdPushConstants(g.command_buffer, g.trace_pipeline_layout,
                                  VK_SHADER_STAGE_COMPUTE_BIT, 0, 4, &count);
            g.fn.CmdDispatch(g.command_buffer, (count + 63) / 64, 1, 1);

            VkMemoryBarrier to_copy = {VK_STRUCTURE_TYPE_MEMORY_BARRIER};
            to_copy.srcAccessMask = VK_ACCESS_SHADER_WRITE_BIT;
            to_copy.dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT;
            g.fn.CmdPipelineBarrier(g.command_buffer, VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT,
                                    VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 1, &to_copy, 0, nullptr, 0, nullptr);
            VkBufferCopy region = {0, 0, results_size};
            g.fn.CmdCopyBuffer(g.command_buffer, result_buf.buffer, readback_buf.buffer, 1, &region);
            VkMemoryBarrier to_host = {VK_STRUCTURE_TYPE_MEMORY_BARRIER};
            to_host.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
            to_host.dstAccessMask = VK_ACCESS_HOST_READ_BIT;
            g.fn.CmdPipelineBarrier(g.command_buffer, VK_PIPELINE_STAGE_TRANSFER_BIT,
                                    VK_PIPELINE_STAGE_HOST_BIT, 0, 1, &to_host, 0, nullptr, 0, nullptr);
            g.fn.EndCommandBuffer(g.command_buffer);

            g.fn.ResetFences(g.device, 1, &g.fence);
            VkSubmitInfo submit = {VK_STRUCTURE_TYPE_SUBMIT_INFO};
            submit.commandBufferCount = 1;
            submit.pCommandBuffers = &g.command_buffer;
            if (g.fn.QueueSubmit(g.queue, 1, &submit, g.fence) != VK_SUCCESS)
            {
                g.error = "trace_batch: vkQueueSubmit failed";
                destroy_all();
                return false;
            }
            VkResult wait = g.fn.WaitForFences(g.device, 1, &g.fence, VK_TRUE, ~0ull);
            if (wait != VK_SUCCESS)
            {
                g.error = "trace_batch: vkWaitForFences failed (device lost?)";
                destroy_all();
                return false;
            }

            results.resize(segments.size());
            std::memcpy(results.data(), readback_buf.mapped, (size_t)results_size);

            destroy_all();
            return true;
        }

        // ===== resident gather session =====

        namespace
        {
            // must cover the deepest root to leaf path of the tnode tree;
            // keep in sync with trace_stack_size in shaders/trace_libglsl
            constexpr int kernel_trace_stack = 64;

            struct gather_session
            {
                bool active = false;
                VkDescriptorPool pool = VK_NULL_HANDLE; // owned; the shared
                // pool is reset wholesale by the one shot batches
                VkDescriptorSet set = VK_NULL_HANDLE;
                gpu_buffer scene_bufs[8]; // tnodes, lightleafs, lights, sun,
                                          // sky, vis list, face gap, vis range
                gpu_buffer item_buf, near_buf, result_buf, readback_buf;
                VkPipeline pipeline = VK_NULL_HANDLE;              // fp64 or f32 variant
                VkPipelineLayout pipeline_layout = VK_NULL_HANDLE; // (unowned)
                uint32_t max_items = 0;
                uint32_t max_near = 0;
                uint32_t num_lightleafs = 0;
                uint32_t sky_lighting_fix = 0;
                int32_t sky_step_match = 0;
                float indirect_sun = 0;
                uint32_t num_sky_normals = 0;
            };
            gather_session gsess;

            int tnode_tree_depth(const std::vector<tnode_gpu> &tnodes)
            {
                int max_depth = 0;
                std::vector<std::pair<int, int> > walk; // (node, depth)
                walk.push_back(std::make_pair(0, 1));
                while (!walk.empty())
                {
                    const int n = walk.back().first;
                    const int d = walk.back().second;
                    walk.pop_back();
                    if (d > max_depth)
                        max_depth = d;
                    const tnode_gpu &tn = tnodes[(size_t)n];
                    if (tn.children[0] >= 0)
                        walk.push_back(std::make_pair(tn.children[0], d + 1));
                    if (tn.children[1] >= 0)
                        walk.push_back(std::make_pair(tn.children[1], d + 1));
                }
                return max_depth;
            }

            void gather_end_locked()
            {
                if (!gsess.active && !gsess.pool)
                    return;
                for (gpu_buffer &b : gsess.scene_bufs)
                    b.destroy();
                gsess.item_buf.destroy();
                gsess.near_buf.destroy();
                gsess.result_buf.destroy();
                gsess.readback_buf.destroy();
                if (gsess.pool)
                    g.fn.DestroyDescriptorPool(g.device, gsess.pool, nullptr);
                gsess = gather_session();
            }
        }

        bool gather_begin(const gather_scene &scene,
                          const std::vector<uint32_t> &pvs_words,
                          uint32_t pvs_stride_words,
                          uint32_t max_chunk_items)
        {
            std::lock_guard<std::mutex> lock(g_mutex);
            gather_end_locked();
            // float normalization is the default and matches the fp64 validation
            // results the environment variable enables fp64 comparison
            const char *fp64_env = std::getenv("SDHLT_GPU_FP64_NORMALIZE");
            const bool float_normalize = !(fp64_env && *fp64_env && *fp64_env != '0');
            if (max_chunk_items == 0 || scene.tnodes.empty() || scene.lightleafs.empty()
                || scene.lights.empty() || pvs_words.empty())
            {
                g.error = "gather_begin: empty input";
                return false;
            }

            // The kernel is chosen by the map's real tree depth, so this has to
            // come before the pipeline is built.
            const int depth = tnode_tree_depth(scene.tnodes);
            if (depth > kernel_trace_stack)
            {
                g.error = "gather_begin: bsp tree depth " + std::to_string(depth)
                    + " exceeds the kernel trace stack (" + std::to_string(kernel_trace_stack) + ")";
                return false;
            }
            if (!ensure_device_locked() || !ensure_gather_pipeline_locked(float_normalize, depth))
                return false;

            // per pvs row, the ascending list of lit leaf indices the row can
            // see (the leaf 0 entry obeys sky_lighting_fix); the kernel walks
            // these instead of bit testing every lit leaf per sample
            const size_t rows = pvs_words.size() / pvs_stride_words;
            std::vector<uint32_t> vis_list;
            std::vector<uint32_t> vis_range(rows * 2);
            for (size_t r = 0; r < rows; r++)
            {
                const uint32_t *row_words = &pvs_words[r * pvs_stride_words];
                const uint32_t first = (uint32_t)vis_list.size();
                for (size_t i = 0; i < scene.lightleafs.size(); i++)
                {
                    const int leafnum = scene.lightleafs[i].leafnum;
                    bool visible;
                    if (leafnum == 0)
                        visible = scene.sky_lighting_fix != 0;
                    else
                    {
                        const int bit = leafnum - 1;
                        visible = (row_words[bit >> 5] & (1u << (bit & 31))) != 0;
                    }
                    if (visible)
                        vis_list.push_back((uint32_t)i);
                }
                vis_range[r * 2] = first;
                vis_range[r * 2 + 1] = (uint32_t)vis_list.size() - first;
            }

            // vulkan buffers cannot be zero sized; optional arrays get a
            // dummy allocation the kernel never indexes
            auto upload = [&](gpu_buffer &buf, const void *data, size_t size)
            {
                if (!create_host_buffer(buf, size ? size : 16))
                    return false;
                if (size)
                    std::memcpy(buf.mapped, data, size);
                return true;
            };

            gsess.max_items = max_chunk_items;
            gsess.max_near = max_chunk_items * 64u + 4096u;
            gsess.num_lightleafs = (uint32_t)scene.lightleafs.size();
            gsess.sky_lighting_fix = scene.sky_lighting_fix;
            gsess.sky_step_match = scene.sky_step_match;
            gsess.indirect_sun = scene.indirect_sun;
            gsess.num_sky_normals = (uint32_t)(scene.sky_normals.size() / 4);

            const VkDeviceSize near_size = 16 + (VkDeviceSize)gsess.max_near * 8;
            const VkDeviceSize results_size =
                (VkDeviceSize)max_chunk_items * sizeof(gather_result_gpu);

            bool ok = upload(gsess.scene_bufs[0], scene.tnodes.data(),
                             scene.tnodes.size() * sizeof(tnode_gpu))
                && upload(gsess.scene_bufs[1], scene.lightleafs.data(),
                          scene.lightleafs.size() * sizeof(lightleaf_gpu))
                && upload(gsess.scene_bufs[2], scene.lights.data(),
                          scene.lights.size() * sizeof(light_gpu))
                && upload(gsess.scene_bufs[3], scene.sun_normals.data(),
                          scene.sun_normals.size() * sizeof(float))
                && upload(gsess.scene_bufs[4], scene.sky_normals.data(),
                          scene.sky_normals.size() * sizeof(float))
                && upload(gsess.scene_bufs[5], vis_list.data(),
                          vis_list.size() * sizeof(uint32_t))
                && upload(gsess.scene_bufs[6], scene.face_gap.data(),
                          scene.face_gap.size() * sizeof(float))
                && upload(gsess.scene_bufs[7], vis_range.data(),
                          vis_range.size() * sizeof(uint32_t))
                && create_host_buffer(gsess.item_buf,
                                      (VkDeviceSize)max_chunk_items * sizeof(work_item_gpu))
                && create_buffer(gsess.near_buf, near_size, VK_BUFFER_USAGE_STORAGE_BUFFER_BIT,
                                 VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT
                                 | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT, true)
                && create_buffer(gsess.result_buf, results_size,
                                 VK_BUFFER_USAGE_STORAGE_BUFFER_BIT | VK_BUFFER_USAGE_TRANSFER_SRC_BIT,
                                 VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT, false)
                && create_buffer(gsess.readback_buf, results_size,
                                 VK_BUFFER_USAGE_TRANSFER_DST_BIT,
                                 VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT
                                 | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT, true);
            if (!ok)
            {
                g.error = "gather_begin: buffer creation failed";
                gather_end_locked();
                return false;
            }

            VkDescriptorPoolSize pool_size = {};
            pool_size.type = VK_DESCRIPTOR_TYPE_STORAGE_BUFFER;
            pool_size.descriptorCount = 11;
            VkDescriptorPoolCreateInfo dpci = {VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO};
            dpci.maxSets = 1;
            dpci.poolSizeCount = 1;
            dpci.pPoolSizes = &pool_size;
            if (g.fn.CreateDescriptorPool(g.device, &dpci, nullptr, &gsess.pool) != VK_SUCCESS)
            {
                g.error = "gather_begin: descriptor pool creation failed";
                gather_end_locked();
                return false;
            }
            gsess.pipeline = float_normalize ? g.gather_f32_pipeline : g.gather_pipeline;
            gsess.pipeline_layout = float_normalize ? g.gather_f32_pipeline_layout
                                                    : g.gather_pipeline_layout;
            VkDescriptorSetAllocateInfo dsai = {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO};
            dsai.descriptorPool = gsess.pool;
            dsai.descriptorSetCount = 1;
            dsai.pSetLayouts = float_normalize ? &g.gather_f32_set_layout : &g.gather_set_layout;
            if (g.fn.AllocateDescriptorSets(g.device, &dsai, &gsess.set) != VK_SUCCESS)
            {
                g.error = "gather_begin: descriptor set allocation failed";
                gather_end_locked();
                return false;
            }

            VkDescriptorBufferInfo infos[11];
            VkBuffer bindings[11] = {
                gsess.scene_bufs[0].buffer, gsess.scene_bufs[1].buffer,
                gsess.scene_bufs[2].buffer, gsess.scene_bufs[3].buffer,
                gsess.scene_bufs[4].buffer, gsess.scene_bufs[5].buffer,
                gsess.scene_bufs[6].buffer, gsess.item_buf.buffer,
                gsess.result_buf.buffer, gsess.near_buf.buffer,
                gsess.scene_bufs[7].buffer,
            };
            VkWriteDescriptorSet writes[11] = {};
            for (uint32_t i = 0; i < 11; i++)
            {
                infos[i] = {bindings[i], 0, VK_WHOLE_SIZE};
                writes[i].sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
                writes[i].dstSet = gsess.set;
                writes[i].dstBinding = i;
                writes[i].descriptorCount = 1;
                writes[i].descriptorType = VK_DESCRIPTOR_TYPE_STORAGE_BUFFER;
                writes[i].pBufferInfo = &infos[i];
            }
            g.fn.UpdateDescriptorSets(g.device, 11, writes, 0, nullptr);

            gsess.active = true;
            return true;
        }

        bool gather_batch(const work_item_gpu *items, size_t count,
                          std::vector<gather_result_gpu> &results,
                          std::vector<near_pair> &near_pairs)
        {
            std::lock_guard<std::mutex> lock(g_mutex);
            results.clear();
            near_pairs.clear();
            if (!gsess.active)
            {
                g.error = "gather_batch: no active gather session";
                return false;
            }
            if (count == 0 || count > gsess.max_items)
            {
                g.error = "gather_batch: bad item count";
                return false;
            }

            std::memcpy(gsess.item_buf.mapped, items, count * sizeof(work_item_gpu));
            std::memset(gsess.near_buf.mapped, 0, 16); // near pair counter + padding

            struct push_params
            {
                uint32_t num_items;
                uint32_t num_lightleafs;
                uint32_t sky_lighting_fix;
                int32_t sky_step_match;
                float indirect_sun;
                uint32_t max_near_pairs;
                uint32_t pvs_stride_words;
                uint32_t num_sky_normals;
            } push = {
                (uint32_t)count,
                gsess.num_lightleafs,
                gsess.sky_lighting_fix,
                gsess.sky_step_match,
                gsess.indirect_sun,
                gsess.max_near,
                0, // pvs stride is unused with visible leaf lists
                gsess.num_sky_normals,
            };
            static_assert(sizeof(push_params) == 32, "must match the glsl Params block");

            const VkDeviceSize results_size = (VkDeviceSize)count * sizeof(gather_result_gpu);

            g.fn.ResetCommandPool(g.device, g.command_pool, 0);
            VkCommandBufferBeginInfo cbbi = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
            cbbi.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
            g.fn.BeginCommandBuffer(g.command_buffer, &cbbi);
            g.fn.CmdBindPipeline(g.command_buffer, VK_PIPELINE_BIND_POINT_COMPUTE, gsess.pipeline);
            g.fn.CmdBindDescriptorSets(g.command_buffer, VK_PIPELINE_BIND_POINT_COMPUTE,
                                       gsess.pipeline_layout, 0, 1, &gsess.set, 0, nullptr);
            g.fn.CmdPushConstants(g.command_buffer, gsess.pipeline_layout,
                                  VK_SHADER_STAGE_COMPUTE_BIT, 0, sizeof(push), &push);
            g.fn.CmdDispatch(g.command_buffer, ((uint32_t)count + 63) / 64, 1, 1);

            VkMemoryBarrier to_copy = {VK_STRUCTURE_TYPE_MEMORY_BARRIER};
            to_copy.srcAccessMask = VK_ACCESS_SHADER_WRITE_BIT;
            to_copy.dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT;
            g.fn.CmdPipelineBarrier(g.command_buffer, VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT,
                                    VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 1, &to_copy, 0, nullptr, 0, nullptr);
            VkBufferCopy region = {0, 0, results_size};
            g.fn.CmdCopyBuffer(g.command_buffer, gsess.result_buf.buffer,
                               gsess.readback_buf.buffer, 1, &region);
            VkMemoryBarrier to_host = {VK_STRUCTURE_TYPE_MEMORY_BARRIER};
            to_host.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT | VK_ACCESS_SHADER_WRITE_BIT;
            to_host.dstAccessMask = VK_ACCESS_HOST_READ_BIT;
            g.fn.CmdPipelineBarrier(g.command_buffer,
                                    VK_PIPELINE_STAGE_TRANSFER_BIT | VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT,
                                    VK_PIPELINE_STAGE_HOST_BIT, 0, 1, &to_host, 0, nullptr, 0, nullptr);
            g.fn.EndCommandBuffer(g.command_buffer);

            g.fn.ResetFences(g.device, 1, &g.fence);
            VkSubmitInfo submit = {VK_STRUCTURE_TYPE_SUBMIT_INFO};
            submit.commandBufferCount = 1;
            submit.pCommandBuffers = &g.command_buffer;
            if (g.fn.QueueSubmit(g.queue, 1, &submit, g.fence) != VK_SUCCESS)
            {
                g.error = "gather_batch: vkQueueSubmit failed";
                return false;
            }
            if (g.fn.WaitForFences(g.device, 1, &g.fence, VK_TRUE, ~0ull) != VK_SUCCESS)
            {
                g.error = "gather_batch: vkWaitForFences failed (device lost?)";
                return false;
            }

            const uint32_t near_count = *(const uint32_t *)gsess.near_buf.mapped;
            if (near_count > gsess.max_near)
            {
                g.error = "gather_batch: near pair buffer overflow";
                return false;
            }
            near_pairs.resize(near_count);
            std::memcpy(near_pairs.data(), (const unsigned char *)gsess.near_buf.mapped + 16,
                        (size_t)near_count * sizeof(near_pair));

            results.resize(count);
            std::memcpy(results.data(), gsess.readback_buf.mapped, (size_t)results_size);
            return true;
        }

        void gather_end()
        {
            std::lock_guard<std::mutex> lock(g_mutex);
            gather_end_locked();
        }

        bool gather_batch(const gather_scene &scene,
                          const std::vector<work_item_gpu> &items,
                          const std::vector<uint32_t> &pvs_words,
                          uint32_t pvs_stride_words,
                          std::vector<gather_result_gpu> &results,
                          std::vector<near_pair> &near_pairs)
        {
            if (!gather_begin(scene, pvs_words, pvs_stride_words, (uint32_t)items.size()))
                return false;
            const bool ok = gather_batch(items.data(), items.size(), results, near_pairs);
            gather_end();
            return ok;
        }

        bool formfactor_batch(const formfactor_scene &scene,
                              const std::vector<transfer_pair> &pairs,
                              std::vector<float> &trans)
        {
            std::lock_guard<std::mutex> lock(g_mutex);
            trans.clear();
            if (!ensure_device_locked() || !ensure_ff_pipeline_locked())
                return false;
            if (scene.patches.empty() || pairs.empty())
            {
                g.error = "formfactor_batch: empty input";
                return false;
            }

            auto upload = [&](gpu_buffer &buf, const void *data, size_t size)
            {
                if (!create_host_buffer(buf, size ? size : 16))
                    return false;
                if (size)
                    std::memcpy(buf.mapped, data, size);
                return true;
            };

            const VkDeviceSize out_size = pairs.size() * sizeof(float);
            gpu_buffer bufs[5];
            gpu_buffer result_buf, readback_buf;
            auto destroy_all = [&]()
            {
                for (gpu_buffer &b : bufs)
                    b.destroy();
                result_buf.destroy();
                readback_buf.destroy();
            };

            bool ok = upload(bufs[0], scene.patches.data(), scene.patches.size() * sizeof(patch_gpu))
                && upload(bufs[1], scene.windings.data(), scene.windings.size() * sizeof(float))
                && upload(bufs[2], scene.sky_normals.data(), scene.sky_normals.size() * sizeof(float))
                && upload(bufs[3], scene.sky_levels.data(), scene.sky_levels.size() * sizeof(int32_t))
                && upload(bufs[4], pairs.data(), pairs.size() * sizeof(transfer_pair))
                && create_buffer(result_buf, out_size,
                                 VK_BUFFER_USAGE_STORAGE_BUFFER_BIT | VK_BUFFER_USAGE_TRANSFER_SRC_BIT,
                                 VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT, false)
                && create_buffer(readback_buf, out_size,
                                 VK_BUFFER_USAGE_TRANSFER_DST_BIT,
                                 VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT
                                 | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT, true);
            if (!ok)
            {
                g.error = "formfactor_batch: buffer creation failed";
                destroy_all();
                return false;
            }

            g.fn.ResetDescriptorPool(g.device, g.descriptor_pool, 0);
            VkDescriptorSetAllocateInfo dsai = {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO};
            dsai.descriptorPool = g.descriptor_pool;
            dsai.descriptorSetCount = 1;
            dsai.pSetLayouts = &g.ff_set_layout;
            VkDescriptorSet set = VK_NULL_HANDLE;
            if (g.fn.AllocateDescriptorSets(g.device, &dsai, &set) != VK_SUCCESS)
            {
                g.error = "formfactor_batch: descriptor set allocation failed";
                destroy_all();
                return false;
            }

            VkDescriptorBufferInfo infos[6];
            VkBuffer bindings[6] = {
                bufs[0].buffer, bufs[1].buffer, bufs[2].buffer,
                bufs[3].buffer, bufs[4].buffer, result_buf.buffer,
            };
            VkWriteDescriptorSet writes[6] = {};
            for (uint32_t i = 0; i < 6; i++)
            {
                infos[i] = {bindings[i], 0, VK_WHOLE_SIZE};
                writes[i].sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
                writes[i].dstSet = set;
                writes[i].dstBinding = i;
                writes[i].descriptorCount = 1;
                writes[i].descriptorType = VK_DESCRIPTOR_TYPE_STORAGE_BUFFER;
                writes[i].pBufferInfo = &infos[i];
            }
            g.fn.UpdateDescriptorSets(g.device, 6, writes, 0, nullptr);

            g.fn.ResetCommandPool(g.device, g.command_pool, 0);
            VkCommandBufferBeginInfo cbbi = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
            cbbi.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
            g.fn.BeginCommandBuffer(g.command_buffer, &cbbi);
            g.fn.CmdBindPipeline(g.command_buffer, VK_PIPELINE_BIND_POINT_COMPUTE, g.ff_pipeline);
            g.fn.CmdBindDescriptorSets(g.command_buffer, VK_PIPELINE_BIND_POINT_COMPUTE,
                                       g.ff_pipeline_layout, 0, 1, &set, 0, nullptr);
            const uint32_t count = (uint32_t)pairs.size();
            g.fn.CmdPushConstants(g.command_buffer, g.ff_pipeline_layout,
                                  VK_SHADER_STAGE_COMPUTE_BIT, 0, 4, &count);
            g.fn.CmdDispatch(g.command_buffer, (count + 63) / 64, 1, 1);

            VkMemoryBarrier to_copy = {VK_STRUCTURE_TYPE_MEMORY_BARRIER};
            to_copy.srcAccessMask = VK_ACCESS_SHADER_WRITE_BIT;
            to_copy.dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT;
            g.fn.CmdPipelineBarrier(g.command_buffer, VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT,
                                    VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 1, &to_copy, 0, nullptr, 0, nullptr);
            VkBufferCopy region = {0, 0, out_size};
            g.fn.CmdCopyBuffer(g.command_buffer, result_buf.buffer, readback_buf.buffer, 1, &region);
            VkMemoryBarrier to_host = {VK_STRUCTURE_TYPE_MEMORY_BARRIER};
            to_host.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
            to_host.dstAccessMask = VK_ACCESS_HOST_READ_BIT;
            g.fn.CmdPipelineBarrier(g.command_buffer, VK_PIPELINE_STAGE_TRANSFER_BIT,
                                    VK_PIPELINE_STAGE_HOST_BIT, 0, 1, &to_host, 0, nullptr, 0, nullptr);
            g.fn.EndCommandBuffer(g.command_buffer);

            g.fn.ResetFences(g.device, 1, &g.fence);
            VkSubmitInfo submit = {VK_STRUCTURE_TYPE_SUBMIT_INFO};
            submit.commandBufferCount = 1;
            submit.pCommandBuffers = &g.command_buffer;
            if (g.fn.QueueSubmit(g.queue, 1, &submit, g.fence) != VK_SUCCESS)
            {
                g.error = "formfactor_batch: vkQueueSubmit failed";
                destroy_all();
                return false;
            }
            if (g.fn.WaitForFences(g.device, 1, &g.fence, VK_TRUE, ~0ull) != VK_SUCCESS)
            {
                g.error = "formfactor_batch: vkWaitForFences failed (device lost?)";
                destroy_all();
                return false;
            }

            trans.resize(pairs.size());
            std::memcpy(trans.data(), readback_buf.mapped, (size_t)out_size);
            destroy_all();
            return true;
        }
    }
}

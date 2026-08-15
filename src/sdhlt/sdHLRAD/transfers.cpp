#include "qrad.h"

#include <limits.h>
#include <stdint.h>

#ifdef SYSTEM_WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <io.h>
#include <sys/stat.h>
#include <fcntl.h>
#include "win32fix.h"
#endif

#ifdef HAVE_SYS_STAT_H
#include <sys/stat.h>
#endif

namespace
{
    const char TRANSFER_CACHE_MAGIC[8] = {'R', 'S', 'D', 'H', 'L', 'T', 'I', '2'};
    const uint32_t TRANSFER_CACHE_SCHEMA = 2;

    enum transfer_cache_flags_t
    {
        TRANSFER_CACHE_RGB = 1u << 0,
        TRANSFER_CACHE_CUSTOM_SHADOW = 1u << 1
    };

    uint32_t TransferCacheFlags()
    {
        return (g_rgb_transfers ? (uint32_t)TRANSFER_CACHE_RGB : 0u)
            | (g_customshadow_with_bouncelight
                ? (uint32_t)TRANSFER_CACHE_CUSTOM_SHADOW : 0u);
    }

    bool TransferCacheSupportsCurrentMap()
    {
        // Dynamic opaque styles are held in transparency.cpp's separate style
        // table. Until that table has its own versioned serialization, loading
        // transfers alone would silently lose style remapping during bounces.
        for (unsigned i = 0; i < g_opaque_face_count; ++i)
        {
            if (g_opaque_face_list[i].style != -1)
                return false;
        }
        return true;
    }

#pragma pack(push, 1)
    struct transfer_cache_header_t
    {
        char magic[8];
        uint32_t schema;
        uint32_t header_size;
        uint64_t fingerprint;
        uint64_t payload_size;
        uint32_t payload_checksum;
        uint32_t patch_count;
        uint32_t index_element_size;
        uint32_t data_element_size;
        uint32_t flags;
    };
#pragma pack(pop)

    static_assert(sizeof(transfer_cache_header_t) == 52,
                  "transfer cache header layout changed");

    void HashBytes(uint64_t& hash, const void* data, size_t size)
    {
        const byte* bytes = (const byte*)data;
        for (size_t i = 0; i < size; ++i)
        {
            hash ^= bytes[i];
            hash *= UINT64_C(1099511628211);
        }
    }

    template <typename T>
    void HashValue(uint64_t& hash, const T& value)
    {
        HashBytes(hash, &value, sizeof(value));
    }

    // Deliberately excludes the BSP lighting lump and dface lightofs/styles:
    // RAD rewrites those on every successful run, but they do not affect the
    // transfer graph. Everything that can affect BSP tracing, PVS selection,
    // patch geometry, opaque models, and form factors is included.
    uint64_t TransferFingerprint(long total_patches)
    {
        uint64_t hash = UINT64_C(1469598103934665603);
        HashValue(hash, TRANSFER_CACHE_SCHEMA);
        HashValue(hash, total_patches);
        HashValue(hash, g_nummodels);
        HashBytes(hash, g_dmodels, (size_t)g_nummodels * sizeof(g_dmodels[0]));
        HashValue(hash, g_numplanes);
        HashBytes(hash, g_dplanes, (size_t)g_numplanes * sizeof(g_dplanes[0]));
        HashValue(hash, g_numvertexes);
        HashBytes(hash, g_dvertexes, (size_t)g_numvertexes * sizeof(g_dvertexes[0]));
        HashValue(hash, g_numnodes);
        HashBytes(hash, g_dnodes, (size_t)g_numnodes * sizeof(g_dnodes[0]));
        HashValue(hash, g_numclipnodes);
        HashBytes(hash, g_dclipnodes, (size_t)g_numclipnodes * sizeof(g_dclipnodes[0]));
        HashValue(hash, g_numtexinfo);
        HashBytes(hash, g_texinfo, (size_t)g_numtexinfo * sizeof(g_texinfo[0]));
        HashValue(hash, g_numedges);
        HashBytes(hash, g_dedges, (size_t)g_numedges * sizeof(g_dedges[0]));
        HashValue(hash, g_nummarksurfaces);
        HashBytes(hash, g_dmarksurfaces,
                  (size_t)g_nummarksurfaces * sizeof(g_dmarksurfaces[0]));
        HashValue(hash, g_numsurfedges);
        HashBytes(hash, g_dsurfedges, (size_t)g_numsurfedges * sizeof(g_dsurfedges[0]));

        HashValue(hash, g_numfaces);
        for (int i = 0; i < g_numfaces; ++i)
        {
            const dface_t& face = g_dfaces[i];
            HashValue(hash, face.planenum);
            HashValue(hash, face.side);
            HashValue(hash, face.firstedge);
            HashValue(hash, face.numedges);
            HashValue(hash, face.texinfo);
        }

        HashValue(hash, g_numleafs);
        for (int i = 0; i < g_numleafs; ++i)
        {
            const dleaf_t& leaf = g_dleafs[i];
            HashValue(hash, leaf.contents);
            HashValue(hash, leaf.visofs);
            HashBytes(hash, leaf.mins, sizeof(leaf.mins));
            HashBytes(hash, leaf.maxs, sizeof(leaf.maxs));
            HashValue(hash, leaf.firstmarksurface);
            HashValue(hash, leaf.nummarksurfaces);
        }

        HashValue(hash, g_visdatasize);
        HashBytes(hash, g_dvisdata, (size_t)g_visdatasize);
        HashValue(hash, g_entdatasize);
        HashBytes(hash, g_dentdata, (size_t)g_entdatasize);
        HashValue(hash, g_texdatasize);
        HashBytes(hash, g_dtexdata, (size_t)g_texdatasize);

        HashValue(hash, g_rgb_transfers);
        HashValue(hash, g_customshadow_with_bouncelight);
        const int vismatrix_method = RadVisMatrixMethodId();
        HashValue(hash, vismatrix_method);
        HashValue(hash, g_transfer_compress_type);
        HashValue(hash, g_rgbtransfer_compress_type);
        HashValue(hash, g_translucentdepth);
        HashValue(hash, g_noemitterrange);
        HashValue(hash, g_blockopaque);
        HashValue(hash, g_studioshadow);
        HashValue(hash, g_numtextures);
        if (g_lightingconeinfo && g_numtextures > 0)
        {
            // The third vec3 component is intentionally unused and is not
            // initialized by ReadLightingCone(). Do not let allocator junk
            // make an otherwise identical cache fingerprint unstable.
            for (int i = 0; i < g_numtextures; ++i)
            {
                HashValue(hash, g_lightingconeinfo[i][0]);
                HashValue(hash, g_lightingconeinfo[i][1]);
            }
        }

        HashValue(hash, g_opaque_face_count);
        for (unsigned i = 0; i < g_opaque_face_count; ++i)
        {
            const opaqueList_t& opaque = g_opaque_face_list[i];
            HashValue(hash, opaque.entitynum);
            HashValue(hash, opaque.modelnum);
            HashBytes(hash, opaque.origin, sizeof(opaque.origin));
            HashBytes(hash, opaque.transparency_scale,
                      sizeof(opaque.transparency_scale));
            HashValue(hash, opaque.transparency);
            HashValue(hash, opaque.style);
            HashValue(hash, opaque.block);
        }

        const uint64_t studio_fingerprint = StudioModelFingerprint();
        HashValue(hash, studio_fingerprint);

        for (long i = 0; i < total_patches; ++i)
        {
            const patch_t& patch = g_patches[i];
            HashBytes(hash, patch.origin, sizeof(patch.origin));
            HashValue(hash, patch.area);
            HashValue(hash, patch.exposure);
            HashValue(hash, patch.emitter_range);
            HashValue(hash, patch.emitter_skylevel);
            HashValue(hash, patch.scale);
            HashValue(hash, patch.chop);
            HashValue(hash, patch.faceNumber);
            HashValue(hash, patch.flags);
            HashValue(hash, patch.translucent_b);
            if (patch.translucent_b)
                HashBytes(hash, patch.translucent_v, sizeof(patch.translucent_v));
            if (patch.winding)
            {
                HashValue(hash, patch.winding->m_NumPoints);
                HashBytes(hash, patch.winding->m_Points,
                          (size_t)patch.winding->m_NumPoints * sizeof(patch.winding->m_Points[0]));
            }
            else
            {
                const unsigned no_points = 0;
                HashValue(hash, no_points);
            }
        }
        return hash;
    }

    uint32_t Crc32Update(uint32_t crc, const void* data, size_t size)
    {
        static uint32_t table[256];
        static bool table_ready = false;
        if (!table_ready)
        {
            for (uint32_t value = 0; value < 256; ++value)
            {
                uint32_t entry = value;
                for (int bit = 0; bit < 8; ++bit)
                {
                    const uint32_t mask = (uint32_t)-(int32_t)(entry & 1u);
                    entry = (entry >> 1) ^ (UINT32_C(0xEDB88320) & mask);
                }
                table[value] = entry;
            }
            table_ready = true;
        }

        const byte* bytes = (const byte*)data;
        for (size_t i = 0; i < size; ++i)
        {
            crc = table[(crc ^ bytes[i]) & 0xFFu] ^ (crc >> 8);
        }
        return crc;
    }

    bool WritePayload(FILE* file, const void* data, size_t element_size,
                      size_t count, uint32_t& crc, uint64_t& payload_size)
    {
        if (count && element_size > SIZE_MAX / count)
            return false;
        const size_t bytes = element_size * count;
        if (bytes > UINT64_MAX - payload_size)
            return false;
        if (bytes && (!data || fwrite(data, 1, bytes, file) != bytes))
            return false;
        crc = Crc32Update(crc, data, bytes);
        payload_size += bytes;
        return true;
    }

    bool ReadPayload(FILE* file, void* data, size_t bytes, uint64_t& remaining)
    {
        if ((uint64_t)bytes > remaining)
            return false;
        if (bytes && fread(data, 1, bytes, file) != bytes)
            return false;
        remaining -= bytes;
        return true;
    }

    void ClearTransferData()
    {
        for (unsigned i = 0; i < g_num_patches; ++i)
        {
            patch_t& patch = g_patches[i];
            if (patch.tIndex)
                FreeBlock(patch.tIndex);
            if (patch.tData)
                FreeBlock(patch.tData);
            if (patch.tRGBData)
                FreeBlock(patch.tRGBData);
            patch.tIndex = NULL;
            patch.tData = NULL;
            patch.tRGBData = NULL;
            patch.iIndex = 0;
            patch.iData = 0;
        }
    }

    bool FlushFile(FILE* file)
    {
        if (fflush(file) != 0)
            return false;
#ifdef SYSTEM_WIN32
        return _commit(_fileno(file)) == 0;
#else
        return fsync(fileno(file)) == 0;
#endif
    }

    bool ReplaceFileAtomically(const char* temporary, const char* destination)
    {
#ifdef SYSTEM_WIN32
        return MoveFileExA(temporary, destination,
                           MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) != 0;
#else
        return rename(temporary, destination) == 0;
#endif
    }

    bool ValidateCompressedIndices(const patch_t& patch, unsigned numpatches)
    {
        uint64_t expanded = 0;
        uint64_t previous_end = 0;
        for (unsigned i = 0; i < patch.iIndex; ++i)
        {
            const uint64_t first = patch.tIndex[i].index;
            const uint64_t last = first + patch.tIndex[i].size;
            if (last >= numpatches || (i && first <= previous_end))
                return false;
            previous_end = last;
            expanded += patch.tIndex[i].size + 1u;
        }
        return expanded == patch.iData;
    }
}

void writetransfers(const char* const transferfile, const long total_patches)
{
    if (!TransferCacheSupportsCurrentMap())
    {
        Warning("Incremental cache disabled: dynamic opaque styles are not cacheable yet\n");
        return;
    }
    if (total_patches < 0 || (uint64_t)total_patches > UINT32_MAX)
    {
        Warning("Incremental cache patch count is out of range\n");
        return;
    }

    char temporary[_MAX_PATH + 64];
#ifdef SYSTEM_WIN32
    safe_snprintf(temporary, sizeof(temporary), "%s.tmp.%lu", transferfile,
                  (unsigned long)GetCurrentProcessId());
#else
    safe_snprintf(temporary, sizeof(temporary), "%s.tmp.%lu", transferfile,
                  (unsigned long)getpid());
#endif
    _unlink(temporary);

    FILE* file = fopen(temporary, "w+b");
    if (!file)
    {
        Warning("Failed to open incremental cache [%s] for writing\n", temporary);
        return;
    }

    transfer_cache_header_t header = {};
    memcpy(header.magic, TRANSFER_CACHE_MAGIC, sizeof(header.magic));
    header.schema = TRANSFER_CACHE_SCHEMA;
    header.header_size = sizeof(header);
    header.fingerprint = TransferFingerprint(total_patches);
    header.patch_count = (uint32_t)total_patches;
    header.index_element_size = sizeof(transfer_index_t);
    header.data_element_size = (uint32_t)(g_rgb_transfers
        ? vector_size[g_rgbtransfer_compress_type]
        : float_size[g_transfer_compress_type]);
    header.flags = TransferCacheFlags();

    bool success = fwrite(&header, sizeof(header), 1, file) == 1;
    uint32_t crc = UINT32_C(0xFFFFFFFF);
    uint64_t payload_size = 0;
    for (long i = 0; success && i < total_patches; ++i)
    {
        const patch_t& patch = g_patches[i];
        const uint32_t index_count = patch.iIndex;
        const uint32_t data_count = patch.iData;
        success = WritePayload(file, &index_count, sizeof(index_count), 1,
                               crc, payload_size)
            && WritePayload(file, patch.tIndex, sizeof(transfer_index_t),
                            patch.iIndex, crc, payload_size)
            && WritePayload(file, &data_count, sizeof(data_count), 1,
                            crc, payload_size);
        if (success && patch.iData)
        {
            const void* data = g_rgb_transfers
                ? (const void*)patch.tRGBData : (const void*)patch.tData;
            success = WritePayload(file, data, header.data_element_size,
                                   patch.iData, crc, payload_size);
        }
    }

    if (success)
    {
        header.payload_size = payload_size;
        header.payload_checksum = crc ^ UINT32_C(0xFFFFFFFF);
        success = fseek(file, 0, SEEK_SET) == 0
            && fwrite(&header, sizeof(header), 1, file) == 1
            && FlushFile(file);
    }

    if (fclose(file) != 0)
        success = false;
    if (success)
        success = ReplaceFileAtomically(temporary, transferfile);

    if (!success)
    {
        _unlink(temporary);
        Warning("Failed to generate incremental cache [%s]\n", transferfile);
        return;
    }

    Log("Wrote incremental cache [%s]: %.2f MB, fingerprint %016llx\n",
        transferfile, (double)(sizeof(header) + payload_size) / (1024.0 * 1024.0),
        (unsigned long long)header.fingerprint);
}

bool readtransfers(const char* const transferfile, const long numpatches)
{
    if (!TransferCacheSupportsCurrentMap())
    {
        Warning("Incremental cache disabled: dynamic opaque styles are not cacheable yet\n");
        return false;
    }
    FILE* file = fopen(transferfile, "rb");
    if (!file)
    {
        Log("No incremental cache found [%s]\n", transferfile);
        return false;
    }

    transfer_cache_header_t header = {};
    bool valid = numpatches >= 0 && (uint64_t)numpatches <= UINT32_MAX
        && fread(&header, sizeof(header), 1, file) == 1
        && !memcmp(header.magic, TRANSFER_CACHE_MAGIC, sizeof(header.magic))
        && header.schema == TRANSFER_CACHE_SCHEMA
        && header.header_size == sizeof(header)
        && header.patch_count == (uint32_t)numpatches
        && header.index_element_size == sizeof(transfer_index_t)
        && header.data_element_size == (uint32_t)(g_rgb_transfers
            ? vector_size[g_rgbtransfer_compress_type]
            : float_size[g_transfer_compress_type])
        && header.flags == TransferCacheFlags();

    if (!valid)
    {
        fclose(file);
        Warning("Ignoring incompatible or legacy incremental cache [%s]\n", transferfile);
        return false;
    }

    const uint64_t expected_fingerprint = TransferFingerprint(numpatches);
    if (header.fingerprint != expected_fingerprint)
    {
        fclose(file);
        Log("Incremental cache is stale [%s] (map/options fingerprint changed)\n",
            transferfile);
        return false;
    }

    // Validate checksum and exact length before allocating anything. A corrupt
    // file therefore cannot leave a partly populated transfer graph behind.
    uint32_t crc = UINT32_C(0xFFFFFFFF);
    uint64_t remaining = header.payload_size;
    byte buffer[64 * 1024];
    while (remaining)
    {
        const size_t chunk = remaining < sizeof(buffer)
            ? (size_t)remaining : sizeof(buffer);
        if (fread(buffer, 1, chunk, file) != chunk)
        {
            valid = false;
            break;
        }
        crc = Crc32Update(crc, buffer, chunk);
        remaining -= chunk;
    }
    if (valid && fgetc(file) != EOF)
        valid = false;
    if ((crc ^ UINT32_C(0xFFFFFFFF)) != header.payload_checksum)
        valid = false;
    if (!valid || fseek(file, (long)sizeof(header), SEEK_SET) != 0)
    {
        fclose(file);
        Warning("Ignoring corrupt incremental cache [%s]\n", transferfile);
        return false;
    }

    const size_t old_total = g_total_transfer;
    const size_t old_index_bytes = g_transfer_index_bytes;
    const size_t old_data_bytes = g_transfer_data_bytes;
    remaining = header.payload_size;
    for (long i = 0; valid && i < numpatches; ++i)
    {
        patch_t& patch = g_patches[i];
        uint32_t index_count = 0;
        uint32_t data_count = 0;
        valid = ReadPayload(file, &index_count, sizeof(index_count), remaining)
            && index_count <= (uint32_t)numpatches;
        if (!valid)
            break;

        patch.iIndex = index_count;
        if (patch.iIndex)
        {
            patch.tIndex = (transfer_index_t*)AllocBlock(
                (size_t)patch.iIndex * sizeof(transfer_index_t));
            hlassume(patch.tIndex != NULL, assume_NoMemory);
            valid = ReadPayload(file, patch.tIndex,
                (size_t)patch.iIndex * sizeof(transfer_index_t), remaining);
        }

        valid = valid
            && ReadPayload(file, &data_count, sizeof(data_count), remaining)
            && data_count <= (uint32_t)numpatches
            && ((index_count == 0) == (data_count == 0));
        if (!valid)
            break;

        patch.iData = data_count;
        if (patch.iData)
        {
            const size_t bytes = (size_t)patch.iData * header.data_element_size;
            if (bytes > ULONG_MAX - unused_size)
            {
                valid = false;
                break;
            }
            if (g_rgb_transfers)
            {
                patch.tRGBData = (rgb_transfer_data_t*)AllocBlock(
                    (unsigned long)(bytes + unused_size));
                hlassume(patch.tRGBData != NULL, assume_NoMemory);
                valid = ReadPayload(file, patch.tRGBData, bytes, remaining);
            }
            else
            {
                patch.tData = (transfer_data_t*)AllocBlock(
                    (unsigned long)(bytes + unused_size));
                hlassume(patch.tData != NULL, assume_NoMemory);
                valid = ReadPayload(file, patch.tData, bytes, remaining);
            }
        }

        valid = valid && ValidateCompressedIndices(patch, (unsigned)numpatches);
        if (valid)
        {
            g_total_transfer += patch.iData;
            g_transfer_index_bytes += (size_t)patch.iIndex * sizeof(transfer_index_t);
            g_transfer_data_bytes += (size_t)patch.iData * header.data_element_size
                + (patch.iData ? unused_size : 0);
        }
    }

    valid = valid && remaining == 0;
    fclose(file);
    if (!valid)
    {
        ClearTransferData();
        g_total_transfer = old_total;
        g_transfer_index_bytes = old_index_bytes;
        g_transfer_data_bytes = old_data_bytes;
        Warning("Ignoring malformed incremental cache [%s]\n", transferfile);
        return false;
    }

    Log("Loaded incremental cache [%s]: %.2f MB, %zu transfers, fingerprint %016llx\n",
        transferfile, (double)(sizeof(header) + header.payload_size) / (1024.0 * 1024.0),
        g_total_transfer - old_total, (unsigned long long)header.fingerprint);
    return true;
}

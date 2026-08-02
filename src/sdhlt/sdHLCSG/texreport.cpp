// Texture cost report.
//
// "-chart" tells you texdata is 2.2 MB and stops there, which is where the
// question actually starts: on a map that embeds its textures that lump is
// most of the .bsp, and nothing says which textures it is made of or whether
// the resolution is doing any work.
//
// This walks the texture lump CSG has just written and cross-references it
// with the surface area each texture ends up painting, so an oversized texture
// on a surface nobody gets close to shows up as a number instead of a hunch.

#include "csg.h"

#include <algorithm>
#include <string>
#include <vector>

bool            g_texreport = DEFAULT_TEXREPORT;

// Surface area, in world units squared, written out per texinfo. Filled by
// WriteFace, which already runs under ThreadLock, so no locking of our own.
static std::vector< double >    s_area_by_texinfo;

void            AccumulateTextureArea(int texinfo, vec_t area)
{
    if (!g_texreport || texinfo < 0)
    {
        return;
    }
    if ((int)s_area_by_texinfo.size() <= texinfo)
    {
        s_area_by_texinfo.resize(texinfo + 1, 0.0);
    }
    s_area_by_texinfo[texinfo] += (double)area;
}

typedef struct
{
    std::string     name;
    int             width;
    int             height;
    int             bytes;                                 // 0 when the texture is not embedded
    double          area;                                  // world units squared painted with it
    double          texels;                                // texture pixels actually displayed
    unsigned int    hash;                                  // of the first mip, 0 when not embedded
    int             dupgroup;                              // -1 when unique
}
texcost_t;

// FNV-1a. Only ever compared against other hashes from this same run.
static unsigned int HashPixels(const byte* data, int count)
{
    unsigned int    h = 2166136261u;

    for (int i = 0; i < count; i++)
    {
        h ^= data[i];
        h *= 16777619u;
    }
    return h;
}

static double   AxisLength(const float* vec)
{
    return sqrt((double)vec[0] * vec[0] + (double)vec[1] * vec[1] + (double)vec[2] * vec[2]);
}

// The size a texture would take at half the resolution in each axis, i.e. a
// quarter of the pixels. Mirrors the WAD3 layout: header, four mips, the
// palette size word and the palette itself.
static int      HalvedSize(int width, int height)
{
    int             px = (width / 2) * (height / 2);

    return (int)sizeof(miptex_t) + px + px / 4 + px / 16 + px / 64 + 2 + 256 * 3;
}

// =====================================================================================
//  CollectTextureCosts
//      Reads back the lump CSG just built. Sizes come from the gaps between
//      consecutive entries rather than from recomputing the WAD3 layout, so
//      whatever padding the writer left is accounted for.
// =====================================================================================
static void     CollectTextureCosts(std::vector< texcost_t >& out)
{
    dmiptexlump_t*  lump = (dmiptexlump_t*)g_dtexdata;

    if (g_texdatasize < (int)sizeof(int) || lump->nummiptex <= 0)
    {
        return;
    }

    const int       count = lump->nummiptex;

    // Entries are not stored in offset order, so sort a copy to work out where
    // each one ends.
    std::vector< std::pair< int, int > > byoffset;         // offset, index
    for (int i = 0; i < count; i++)
    {
        if (lump->dataofs[i] >= 0)
        {
            byoffset.push_back(std::make_pair(lump->dataofs[i], i));
        }
    }
    std::sort(byoffset.begin(), byoffset.end());

    out.assign(count, texcost_t());
    for (int i = 0; i < count; i++)
    {
        out[i].bytes = 0;
        out[i].area = 0.0;
        out[i].texels = 0.0;
        out[i].hash = 0;
        out[i].dupgroup = -1;
        out[i].width = 0;
        out[i].height = 0;
    }

    for (unsigned int k = 0; k < byoffset.size(); k++)
    {
        const int       ofs = byoffset[k].first;
        const int       index = byoffset[k].second;
        const int       end = (k + 1 < byoffset.size()) ? byoffset[k + 1].first : g_texdatasize;
        const miptex_t* mt = (const miptex_t*)(g_dtexdata + ofs);

        char            name[16 + 1];
        memcpy(name, mt->name, 16);
        name[16] = '\0';

        out[index].name = name;
        out[index].width = (int)mt->width;
        out[index].height = (int)mt->height;

        // offsets[0] == 0 means the texture is only referenced, its pixels
        // live in the wad and cost the .bsp nothing.
        if (mt->offsets[0] != 0)
        {
            out[index].bytes = end - ofs;

            const int       px = (int)mt->width * (int)mt->height;
            const int       pixofs = ofs + (int)mt->offsets[0];

            if (px > 0 && pixofs >= 0 && pixofs + px <= g_texdatasize)
            {
                out[index].hash = HashPixels(g_dtexdata + pixofs, px);
            }
        }
    }

    // Fold in the painted area. After WriteMiptex, texinfo.miptex is the real
    // index into this lump.
    for (int i = 0; i < (int)s_area_by_texinfo.size() && i < g_numtexinfo; i++)
    {
        const double    area = s_area_by_texinfo[i];

        if (area <= 0.0)
        {
            continue;
        }

        const int       index = g_texinfo[i].miptex;

        if (index < 0 || index >= count)
        {
            continue;
        }

        // The texture axes are scaled by texels-per-unit, so their lengths turn
        // world area into the number of texture pixels that area displays.
        const double    density = AxisLength(g_texinfo[i].vecs[0]) * AxisLength(g_texinfo[i].vecs[1]);

        out[index].area += area;
        out[index].texels += area * density;
    }
}

// =====================================================================================
//  MarkDuplicates
//      Groups textures whose first mip is byte for byte identical.
// =====================================================================================
static int      MarkDuplicates(std::vector< texcost_t >& tex)
{
    int             groups = 0;

    for (unsigned int i = 0; i < tex.size(); i++)
    {
        if (tex[i].hash == 0 || tex[i].dupgroup != -1)
        {
            continue;
        }

        int             found = -1;

        for (unsigned int j = i + 1; j < tex.size(); j++)
        {
            if (tex[j].dupgroup != -1 || tex[j].hash != tex[i].hash)
            {
                continue;
            }
            if (tex[j].width != tex[i].width || tex[j].height != tex[i].height)
            {
                continue;
            }
            if (found == -1)
            {
                found = groups++;
                tex[i].dupgroup = found;
            }
            tex[j].dupgroup = found;
        }
    }
    return groups;
}

static bool     MoreExpensive(const texcost_t& a, const texcost_t& b)
{
    if (a.bytes != b.bytes)
    {
        return a.bytes > b.bytes;
    }
    return a.name < b.name;
}

// =====================================================================================
//  TextureCostReport
// =====================================================================================
void            TextureCostReport()
{
    if (!g_texreport)
    {
        return;
    }

    std::vector< texcost_t > tex;

    CollectTextureCosts(tex);
    if (tex.empty())
    {
        return;
    }

    int             total = 0;
    for (unsigned int i = 0; i < tex.size(); i++)
    {
        total += tex[i].bytes;
    }

    Log("\n");
    Log("Texture cost report\n");
    Log("-------------------\n");

    if (total == 0)
    {
        Log("No texture is embedded in the bsp, so textures cost it nothing.\n");
        Log("The sizes below are what they would cost with -nowadtextures.\n\n");
    }

    const int       dupgroups = MarkDuplicates(tex);

    std::vector< texcost_t > sorted(tex);
    std::sort(sorted.begin(), sorted.end(), MoreExpensive);

    Log("     bytes  share   size       painted    oversampled  texture\n");

    int             shown = 0;
    for (unsigned int i = 0; i < sorted.size() && shown < 25; i++)
    {
        const texcost_t& t = sorted[i];

        if (t.bytes == 0 && t.area == 0.0)
        {
            continue;
        }
        shown++;

        char            over[32];
        if (t.texels > 0.0)
        {
            snprintf(over, sizeof(over), "%9.1fx",
                     (double)((double)t.width * t.height) / t.texels);
        }
        else
        {
            snprintf(over, sizeof(over), "        --");
        }

        Log("%10d  %4.1f%%  %4dx%-4d  %10.0f  %s  %s%s\n",
            t.bytes,
            total ? 100.0 * t.bytes / total : 0.0,
            t.width, t.height,
            t.area,
            over,
            t.name.c_str(),
            t.dupgroup >= 0 ? "  (duplicate)" : "");
    }

    if (shown < (int)sorted.size())
    {
        Log("           ... and %d more\n", (int)sorted.size() - shown);
    }

    Log("\n");
    Log("%d textures, %d bytes of texture data\n", (int)tex.size(), total);

    // Duplicates: every copy after the first is dead weight.
    if (dupgroups > 0)
    {
        std::vector< int > firstseen(dupgroups, -1);
        int             wasted = 0;

        for (unsigned int i = 0; i < tex.size(); i++)
        {
            const int       g = tex[i].dupgroup;

            if (g < 0)
            {
                continue;
            }
            if (firstseen[g] == -1)
            {
                firstseen[g] = (int)i;
            }
            else
            {
                wasted += tex[i].bytes;
            }
        }
        Log("%d group%s of identical textures, %d bytes paid twice (%.1f%%)\n",
            dupgroups, dupgroups == 1 ? "" : "s", wasted,
            total ? 100.0 * wasted / total : 0.0);
    }

    // A texture with four times more pixels than it ever displays can lose half
    // its resolution in each axis and still have a pixel per pixel on screen.
    int             safe = 0;
    int             safecount = 0;
    for (unsigned int i = 0; i < tex.size(); i++)
    {
        const texcost_t& t = tex[i];

        if (t.bytes == 0 || t.texels <= 0.0 || t.width < 2 || t.height < 2)
        {
            continue;
        }
        if ((double)t.width * t.height >= 4.0 * t.texels)
        {
            safe += t.bytes - HalvedSize(t.width, t.height);
            safecount++;
        }
    }

    if (safecount > 0)
    {
        Log("%d texture%s have at least 4x more pixels than they ever display;\n"
            "halving those would save %d bytes (%.1f%%) with a pixel still to spare\n",
            safecount, safecount == 1 ? "" : "s", safe,
            total ? 100.0 * safe / total : 0.0);
    }
    else
    {
        Log("No texture is carrying 4x more pixels than it displays\n");
    }

    Log("\n");
}

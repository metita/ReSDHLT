// AJM: added file in
#ifndef WADPATH_H__
#define WADPATH_H__
#include "cmdlib.h" //--vluzacn

// Was 128, which a mapper with a big texture library reaches by accident: the
// tools abort with "too many wad files" instead of ignoring the excess. Each
// slot is a pointer plus an open FILE*, so the only real cost of raising it is
// file handles, and CSG raises the CRT limit at startup to match.
// Keep in step with MAX_TEXFILES in textures.cpp.
#define MAX_WADPATHS 512

typedef struct    
{
    char            path[_MAX_PATH];
    bool            usedbymap;        // does this map requrie this wad to be included in the bsp?
    int             usedtextures;     // number of textures in this wad the map actually uses
	int             totaltextures;    // total textures in this wad
} wadpath_t;                          // !!! the above two are VERY DIFFERENT. ie (usedtextures == 0) != (usedbymap == false)

extern wadpath_t*  g_pWadPaths[MAX_WADPATHS];
extern int         g_iNumWadPaths;    


extern void        PushWadPath(const char* const path, bool inuse);
extern void        FreeWadPaths();
extern void        GetUsedWads();

#endif

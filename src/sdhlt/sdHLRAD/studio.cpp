#include "qrad.h"
#include "meshtrace.h"
#include "filelib.h"
#include "stringlib.h"

#include <algorithm>
#include <cstdint>
#include <cstring>
#include <map>
#include <string>
#include <vector>
#include <sys/stat.h>

#ifdef SYSTEM_WIN32
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#endif

#define MAX_MODELS		1024

model_t models[MAX_MODELS];
int num_models;

static vec3_t g_studio_mins[MAX_MODELS];
static vec3_t g_studio_maxs[MAX_MODELS];
static vec3_t g_studio_global_mins;
static vec3_t g_studio_global_maxs;
static bool   g_studio_bounds_valid = false;

static void BuildStudioBounds( void )
{
	g_studio_bounds_valid = false;
	VectorFill( g_studio_global_mins,  99999999.0f );
	VectorFill( g_studio_global_maxs, -99999999.0f );

	for( int i = 0; i < num_models; i++ )
	{
		mmesh_t *pMesh = models[i].mesh.GetMesh();
		if( !pMesh )
		{
			VectorFill( g_studio_mins[i],  99999999.0f );
			VectorFill( g_studio_maxs[i], -99999999.0f );
			continue;
		}
		for( int k = 0; k < 3; k++ )
		{
			g_studio_mins[i][k] = pMesh->mins[k] - 1.0f;
			g_studio_maxs[i][k] = pMesh->maxs[k] + 1.0f;
			if( g_studio_mins[i][k] < g_studio_global_mins[k] ) g_studio_global_mins[k] = g_studio_mins[i][k];
			if( g_studio_maxs[i][k] > g_studio_global_maxs[k] ) g_studio_global_maxs[k] = g_studio_maxs[i][k];
		}
		g_studio_bounds_valid = true;
	}
}

static inline bool SegmentIntersectsAABB( const vec_t* p1, const vec_t* p2, const vec3_t mins, const vec3_t maxs )
{
	float tmin = 0.0f;
	float tmax = 1.0f;
	for( int k = 0; k < 3; k++ )
	{
		const float d = (float)( p2[k] - p1[k] );
		if( fabs( d ) < 1e-8f )
		{
			if( p1[k] < mins[k] || p1[k] > maxs[k] )
				return false;
		}
		else
		{
			const float inv = 1.0f / d;
			float t0 = ( (float)mins[k] - (float)p1[k] ) * inv;
			float t1 = ( (float)maxs[k] - (float)p1[k] ) * inv;
			if( t0 > t1 )
			{
				const float tmp = t0; t0 = t1; t1 = tmp;
			}
			if( t0 > tmin ) tmin = t0;
			if( t1 < tmax ) tmax = t1;
			if( tmin > tmax )
				return false;
		}
	}
	return true;
}

int ExportStudioBoxes( float *out6, int max_boxes )
{
	if( !g_studio_bounds_valid || num_models <= 0 )
		return 0;
	int count = 0;
	// Slot 0 = global union AABB for fast 1-test early-out in the shader.
	if( out6 && max_boxes > 0 )
	{
		out6[0] = (float)g_studio_global_mins[0];
		out6[1] = (float)g_studio_global_mins[1];
		out6[2] = (float)g_studio_global_mins[2];
		out6[3] = (float)g_studio_global_maxs[0];
		out6[4] = (float)g_studio_global_maxs[1];
		out6[5] = (float)g_studio_global_maxs[2];
	}
	count = 1;
	for( int i = 0; i < num_models; i++ )
	{
		if( !models[i].mesh.GetMesh() )
			continue;
		if( out6 && count < max_boxes )
		{
			float *dst = out6 + count * 6;
			dst[0] = (float)g_studio_mins[i][0];
			dst[1] = (float)g_studio_mins[i][1];
			dst[2] = (float)g_studio_mins[i][2];
			dst[3] = (float)g_studio_maxs[i][0];
			dst[4] = (float)g_studio_maxs[i][1];
			dst[5] = (float)g_studio_maxs[i][2];
		}
		count++;
	}
	return count;
}

namespace
{
	struct StudioTriBuild
	{
		float v0[3], e1[3], e2[3];
		float mins[3], maxs[3], centroid[3];
		float st[6];
		int alpha_tex; // -1 if opaque
	};

	struct AlphaTexPacked
	{
		uint32_t width;
		uint32_t height;
		uint32_t word_ofs; // into alpha_words
	};

	struct StudioBvhNode
	{
		float mins[3], maxs[3];
		uint32_t left_or_first; // bit 31 = leaf
		uint32_t right_or_count;
	};

	static inline float UintAsFloat( uint32_t u )
	{
		float f;
		memcpy( &f, &u, sizeof( float ));
		return f;
	}

	static uint32_t BuildStudioBVHRecursive(
		std::vector<StudioTriBuild> &tris,
		std::vector<StudioBvhNode> &nodes,
		uint32_t start, uint32_t end, int depth )
	{
		const uint32_t nodeIdx = (uint32_t)nodes.size();
		nodes.push_back( StudioBvhNode{} );

		float bmin[3] = {  1e30f,  1e30f,  1e30f };
		float bmax[3] = { -1e30f, -1e30f, -1e30f };
		float cmin[3] = {  1e30f,  1e30f,  1e30f };
		float cmax[3] = { -1e30f, -1e30f, -1e30f };

		for( uint32_t i = start; i < end; i++ )
		{
			for( int k = 0; k < 3; k++ )
			{
				if( tris[i].mins[k] < bmin[k] ) bmin[k] = tris[i].mins[k];
				if( tris[i].maxs[k] > bmax[k] ) bmax[k] = tris[i].maxs[k];
				if( tris[i].centroid[k] < cmin[k] ) cmin[k] = tris[i].centroid[k];
				if( tris[i].centroid[k] > cmax[k] ) cmax[k] = tris[i].centroid[k];
			}
		}

		for( int k = 0; k < 3; k++ )
		{
			nodes[nodeIdx].mins[k] = bmin[k] - 0.01f;
			nodes[nodeIdx].maxs[k] = bmax[k] + 0.01f;
		}

		const uint32_t count = end - start;
		if( count <= 4 || depth >= 18 )
		{
			nodes[nodeIdx].left_or_first = 0x80000000u | start;
			nodes[nodeIdx].right_or_count = count;
			return nodeIdx;
		}

		int axis = 0;
		float extent = cmax[0] - cmin[0];
		if( cmax[1] - cmin[1] > extent ) { axis = 1; extent = cmax[1] - cmin[1]; }
		if( cmax[2] - cmin[2] > extent ) { axis = 2; extent = cmax[2] - cmin[2]; }

		if( extent < 1e-4f )
		{
			nodes[nodeIdx].left_or_first = 0x80000000u | start;
			nodes[nodeIdx].right_or_count = count;
			return nodeIdx;
		}

		const uint32_t mid = start + count / 2;
		std::nth_element( tris.begin() + start, tris.begin() + mid, tris.begin() + end,
			[axis]( const StudioTriBuild &a, const StudioTriBuild &b ) {
				return a.centroid[axis] < b.centroid[axis];
			});

		const uint32_t left = BuildStudioBVHRecursive( tris, nodes, start, mid, depth + 1 );
		const uint32_t right = BuildStudioBVHRecursive( tris, nodes, mid, end, depth + 1 );
		nodes[nodeIdx].left_or_first = left;
		nodes[nodeIdx].right_or_count = right;
		return nodeIdx;
	}
}

bool ExportStudioBVH( std::vector<float> &out )
{
	if( !g_studio_bounds_valid || num_models <= 0 )
		return false;

	std::vector<int> valid_models;
	valid_models.reserve( num_models );
	for( int i = 0; i < num_models; i++ )
	{
		if( models[i].mesh.GetMesh() )
			valid_models.push_back( i );
	}
	if( valid_models.empty() )
		return false;

	std::vector<StudioTriBuild> all_tris;
	std::vector<StudioBvhNode> all_nodes;
	std::vector<uint32_t> model_roots( valid_models.size(), 0xFFFFFFFEu );
	std::vector<AlphaTexPacked> alpha_texs;
	std::vector<uint32_t> alpha_words;
	std::map<std::pair<const void*, int>, int> alpha_tex_map;

	for( size_t mi = 0; mi < valid_models.size(); mi++ )
	{
		const int idx = valid_models[mi];
		mmesh_t *pMesh = models[idx].mesh.GetMesh();
		const void *extradata = models[idx].extradata;
		const uint32_t tri_start = (uint32_t)all_tris.size();

		for( uint f = 0; f < pMesh->numfacets; f++ )
		{
			const mfacet_t &fac = pMesh->facets[f];
			int alpha_tex = -1;
			if( fac.texture && fac.texture->index )
			{
				if( fac.texture->flags & STUDIO_NF_ADDITIVE )
					continue;
				if( fac.texture->flags & STUDIO_NF_TRANSPARENT )
				{
					const auto key = std::make_pair( extradata, (int)fac.texture->index );
					auto it = alpha_tex_map.find( key );
					if( it == alpha_tex_map.end() )
					{
						const uint32_t w = (uint32_t)fac.texture->width;
						const uint32_t h = (uint32_t)fac.texture->height;
						const uint32_t npix = w * h;
						const uint32_t nwords = ( npix + 31u ) >> 5u;
						const uint32_t wofs = (uint32_t)alpha_words.size();
						alpha_words.resize( wofs + nwords, 0u );
						const byte *pixels = (const byte *)extradata + fac.texture->index;
						for( uint32_t p = 0; p < npix; p++ )
						{
							if( pixels[p] != 0xFF )
								alpha_words[wofs + ( p >> 5u )] |= ( 1u << ( p & 31u ));
						}
						alpha_tex = (int)alpha_texs.size();
						alpha_texs.push_back( AlphaTexPacked{ w, h, wofs } );
						alpha_tex_map[key] = alpha_tex;
					}
					else
					{
						alpha_tex = it->second;
					}
				}
			}
			StudioTriBuild tb;
			tb.alpha_tex = alpha_tex;
			for( int k = 0; k < 3; k++ )
			{
				const float v0 = (float)fac.triangle[0].point[k];
				const float v1 = (float)fac.triangle[1].point[k];
				const float v2 = (float)fac.triangle[2].point[k];
				tb.v0[k] = v0;
				tb.e1[k] = (float)fac.edge1[k];
				tb.e2[k] = (float)fac.edge2[k];
				tb.mins[k] = std::min( v0, std::min( v1, v2 ));
				tb.maxs[k] = std::max( v0, std::max( v1, v2 ));
				tb.centroid[k] = ( v0 + v1 + v2 ) * ( 1.0f / 3.0f );
			}
			tb.st[0] = fac.triangle[0].st[0]; tb.st[1] = fac.triangle[0].st[1];
			tb.st[2] = fac.triangle[1].st[0]; tb.st[3] = fac.triangle[1].st[1];
			tb.st[4] = fac.triangle[2].st[0]; tb.st[5] = fac.triangle[2].st[1];
			all_tris.push_back( tb );
		}

		if( all_tris.size() > tri_start )
		{
			model_roots[mi] = BuildStudioBVHRecursive( all_tris, all_nodes, tri_start, (uint32_t)all_tris.size(), 0 );
		}
	}

	uint32_t num_alpha_tris = 0;
	for( const StudioTriBuild &t : all_tris )
	{
		if( t.alpha_tex >= 0 ) num_alpha_tris++;
	}

	const uint32_t base_ofs = (uint32_t)out.size();
	const uint32_t nmodels = (uint32_t)valid_models.size();
	const uint32_t nodes_ofs = base_ofs + 9u + nmodels * 12u;
	const uint32_t tris_ofs = nodes_ofs + (uint32_t)all_nodes.size() * 8u;
	const uint32_t alpha_meta_ofs = tris_ofs + (uint32_t)all_tris.size() * 10u;
	const uint32_t alpha_bits_ofs = alpha_meta_ofs + num_alpha_tris * 9u;
	const size_t total_floats = (size_t)alpha_bits_ofs - base_ofs + alpha_words.size();
	out.reserve( out.size() + total_floats );

	// Global header (9 floats)
	out.push_back( UintAsFloat( nmodels ));
	out.push_back( UintAsFloat( nodes_ofs ));
	out.push_back( UintAsFloat( tris_ofs ));
	out.push_back( (float)g_studio_global_mins[0] );
	out.push_back( (float)g_studio_global_mins[1] );
	out.push_back( (float)g_studio_global_mins[2] );
	out.push_back( (float)g_studio_global_maxs[0] );
	out.push_back( (float)g_studio_global_maxs[1] );
	out.push_back( (float)g_studio_global_maxs[2] );

	// Per-model headers (12 floats each)
	for( size_t mi = 0; mi < valid_models.size(); mi++ )
	{
		const int idx = valid_models[mi];
		const model_t &m = models[idx];
		out.push_back( (float)g_studio_mins[idx][0] );
		out.push_back( (float)g_studio_mins[idx][1] );
		out.push_back( (float)g_studio_mins[idx][2] );
		out.push_back( (float)g_studio_maxs[idx][0] );
		out.push_back( (float)g_studio_maxs[idx][1] );
		out.push_back( (float)g_studio_maxs[idx][2] );
		out.push_back( m.has_lightspot ? (float)m.lightspot[0] : 0.0f );
		out.push_back( m.has_lightspot ? (float)m.lightspot[1] : 0.0f );
		out.push_back( m.has_lightspot ? (float)m.lightspot[2] : 0.0f );
		out.push_back( m.has_lightspot ? (float)( m.lightspot_radius * m.lightspot_radius ) : 0.0f );
		out.push_back( UintAsFloat( model_roots[mi] ));
		out.push_back( 0.0f );
	}

	// BVH nodes (8 floats each)
	for( const StudioBvhNode &n : all_nodes )
	{
		out.push_back( n.mins[0] );
		out.push_back( n.mins[1] );
		out.push_back( n.mins[2] );
		out.push_back( n.maxs[0] );
		out.push_back( n.maxs[1] );
		out.push_back( n.maxs[2] );
		out.push_back( UintAsFloat( n.left_or_first ));
		out.push_back( UintAsFloat( n.right_or_count ));
	}

	// Triangles (10 floats each: v0, e1, e2, alphaMetaOfs)
	uint32_t cur_alpha_meta = alpha_meta_ofs;
	for( const StudioTriBuild &t : all_tris )
	{
		out.push_back( t.v0[0] ); out.push_back( t.v0[1] ); out.push_back( t.v0[2] );
		out.push_back( t.e1[0] ); out.push_back( t.e1[1] ); out.push_back( t.e1[2] );
		out.push_back( t.e2[0] ); out.push_back( t.e2[1] ); out.push_back( t.e2[2] );
		if( t.alpha_tex >= 0 )
		{
			out.push_back( UintAsFloat( cur_alpha_meta ));
			cur_alpha_meta += 9u;
		}
		else
		{
			out.push_back( UintAsFloat( 0u ));
		}
	}

	// Alpha triangle metadata (9 floats each: st0, st1, st2, width, height, bitsOfs)
	for( const StudioTriBuild &t : all_tris )
	{
		if( t.alpha_tex < 0 ) continue;
		const AlphaTexPacked &at = alpha_texs[t.alpha_tex];
		out.push_back( t.st[0] ); out.push_back( t.st[1] );
		out.push_back( t.st[2] ); out.push_back( t.st[3] );
		out.push_back( t.st[4] ); out.push_back( t.st[5] );
		out.push_back( UintAsFloat( at.width ));
		out.push_back( UintAsFloat( at.height ));
		out.push_back( UintAsFloat( alpha_bits_ofs + at.word_ofs ));
	}

	// 1-bit alpha bitmaps
	for( uint32_t w : alpha_words )
	{
		out.push_back( UintAsFloat( w ));
	}

	return true;
}

bool StudioSphereNearAny( const vec3_t center, vec_t radius )
{
	if( !g_studio_bounds_valid )
		return false;
	for( int k = 0; k < 3; k++ )
	{
		if( center[k] + radius < g_studio_global_mins[k] || center[k] - radius > g_studio_global_maxs[k] )
			return false;
	}
	for( int i = 0; i < num_models; i++ )
	{
		if( !models[i].mesh.GetMesh() )
			continue;
		if( center[0] + radius >= g_studio_mins[i][0] && center[0] - radius <= g_studio_maxs[i][0]
			&& center[1] + radius >= g_studio_mins[i][1] && center[1] - radius <= g_studio_maxs[i][1]
			&& center[2] + radius >= g_studio_mins[i][2] && center[2] - radius <= g_studio_maxs[i][2] )
		{
			return true;
		}
	}
	return false;
}

char g_moddir[_MAX_PATH] = "";	// -moddir

// =====================================================================================
//  Where the .mdl files are
//      First the folder above the map's (a map compiled from <mod>/maps finds
//      them there), then the game: the -moddir folder, or Counter-Strike in a
//      Steam library when none is given. In the game the same folders the
//      engine reads: <mod>_addon, <mod>, <mod>_downloads and valve.
// =====================================================================================
static std::vector<std::string> g_modelpaths;
static std::map<std::string, int> g_missingmodels;	// name -> entities using it

static bool IsDir( const std::string &path )
{
	struct stat st;
	return !stat( path.c_str(), &st ) && ( st.st_mode & S_IFDIR );
}

static std::string TrimSlashes( std::string path )
{
	while( !path.empty() && ( path.back() == '/' || path.back() == '\\' ))
		path.pop_back();
	return path;
}

static void AddModelPath( const std::string &dir )
{
	// stat fails on Windows with a trailing slash
	std::string path = TrimSlashes( dir );
	if( path.empty() || !IsDir( path )) return;
	path += "/";
	FlipSlashes( &path[0] );
	for( const std::string &known : g_modelpaths )
	{
		if( !Q_stricmp( known.c_str(), path.c_str() )) return;
	}
	g_modelpaths.push_back( path );
}

static void AddGamePaths( const std::string &moddir )
{
	const std::string mod = TrimSlashes( moddir );
	const size_t slash = mod.find_last_of( "/\\" );
	const std::string root = slash == std::string::npos ? "." : mod.substr( 0, slash );
	AddModelPath( mod + "_addon" );
	AddModelPath( mod );
	AddModelPath( mod + "_downloads" );
	AddModelPath( root + "/valve" );
}

// Steam folders that may hold steamapps: the install itself, and every
// library listed in its libraryfolders.vdf
static std::vector<std::string> SteamLibraries( void )
{
	std::vector<std::string> roots, libraries;
#ifdef SYSTEM_WIN32
	char steam[MAX_PATH];
	DWORD size = sizeof( steam );
	if( RegGetValueA( HKEY_CURRENT_USER, "Software\\Valve\\Steam", "SteamPath", RRF_RT_REG_SZ, NULL, steam, &size ) == ERROR_SUCCESS )
		roots.push_back( steam );
#else
	const char *home = getenv( "HOME" );
	if( home )
	{
		roots.push_back( std::string( home ) + "/.steam/steam" );
		roots.push_back( std::string( home ) + "/.local/share/Steam" );
	}
#endif
	for( const std::string &root : roots )
	{
		libraries.push_back( root );

		FILE *f = fopen(( root + "/steamapps/libraryfolders.vdf" ).c_str(), "rb" );
		if( !f ) continue;
		char line[1024];
		while( fgets( line, sizeof( line ), f ))
		{
			// "path"		"D:\\SteamLibrary"
			const char *key = strstr( line, "\"path\"" );
			if( !key ) continue;
			const char *open = strchr( key + 6, '"' );
			if( !open ) continue;
			std::string path;
			for( const char *c = open + 1; *c && *c != '"'; c++ )
			{
				if( *c == '\\' && c[1] ) c++;
				path += *c;
			}
			libraries.push_back( path );
		}
		fclose( f );
	}
	return libraries;
}

static void FindModelPaths( void )
{
	g_modelpaths.clear();
	// empty when the map was given without a folder: the current one
	AddModelPath( *g_Wadpath ? g_Wadpath : "." );

	if( *g_moddir )
	{
		if( !IsDir( g_moddir ))
			Warning( "-moddir: %s is not a folder", g_moddir );
		AddGamePaths( g_moddir );
		return;
	}

	for( const std::string &library : SteamLibraries() )
	{
		const std::string cstrike = library + "/steamapps/common/Half-Life/cstrike";
		if( IsDir( cstrike ))
		{
			AddGamePaths( cstrike );
			return;
		}
	}
}

// Full path of a model in the first folder that has it
static bool FindModelFile( const char *modelname, char *out, size_t outsize )
{
	for( const std::string &dir : g_modelpaths )
	{
		snprintf( out, outsize, "%s%s", dir.c_str(), modelname );
		FlipSlashes( out );
		if( q_exists( out )) return true;
	}
	return false;
}

static void StudioFingerprintBytes(uint64_t& hash, const void* data, size_t size)
{
	const byte* bytes = (const byte*)data;
	for (size_t i = 0; i < size; ++i)
	{
		hash ^= bytes[i];
		hash *= UINT64_C(1099511628211);
	}
}

uint64_t StudioModelFingerprint(void)
{
	uint64_t hash = UINT64_C(1469598103934665603);
	StudioFingerprintBytes(hash, &num_models, sizeof(num_models));
	for (int i = 0; i < num_models; ++i)
	{
		const model_t* model = &models[i];
		StudioFingerprintBytes(hash, model->name, strlen(model->name) + 1);
		StudioFingerprintBytes(hash, model->origin, sizeof(model->origin));
		StudioFingerprintBytes(hash, model->angles, sizeof(model->angles));
		StudioFingerprintBytes(hash, model->scale, sizeof(model->scale));
		StudioFingerprintBytes(hash, &model->trace_mode, sizeof(model->trace_mode));
		StudioFingerprintBytes(hash, &model->body, sizeof(model->body));
		StudioFingerprintBytes(hash, &model->skin, sizeof(model->skin));
		StudioFingerprintBytes(hash, &model->has_lightspot, sizeof(model->has_lightspot));
		StudioFingerprintBytes(hash, model->lightspot, sizeof(model->lightspot));
		StudioFingerprintBytes(hash, &model->lightspot_radius, sizeof(model->lightspot_radius));

		const studiohdr_t* header = (const studiohdr_t*)model->extradata;
		if (header && header->length > 0)
		{
			StudioFingerprintBytes(hash, &header->length, sizeof(header->length));
			StudioFingerprintBytes(hash, header, (size_t)header->length);
		}
	}
	return hash;
}

model_t *LoadStudioModel( const char *modelname, const vec3_t origin, const vec3_t angles, const vec3_t scale, int body, int skin, int trace_mode )
{
	if( num_models >= MAX_MODELS )
	{
		Developer( DEVELOPER_LEVEL_ERROR, "LoadStudioModel: MAX_MODELS exceeded\n" );
		return NULL;
	}
	char path[_MAX_PATH];
	if( !FindModelFile( modelname, path, sizeof( path )))
	{
		std::string key = modelname;
		for( char &c : key )
			c = c == '\\' ? '/' : (char)tolower( (unsigned char)c );
		g_missingmodels[key]++;
		return NULL;
	}

	// the name, not the full path: it has to fit name[64]
	model_t *m = &models[num_models];
	safe_strncpy(m->name, modelname, sizeof(m->name));
	FlipSlashes(m->name);
	LoadFile(path, (char**)&m->extradata);

	studiohdr_t *phdr = (studiohdr_t *)m->extradata;

	// well the textures place in separate file (very stupid case)
	if( phdr->numtextures == 0 )
	{
		char texpath[_MAX_PATH];
		byte *texdata, *moddata;
		studiohdr_t *thdr, *newhdr;

		// <name>T.mdl, next to the model
		safe_strncpy(texpath, path, sizeof(texpath));
		StripExtension(texpath);
		safe_strncat(texpath, "T.mdl", sizeof(texpath));

		LoadFile(texpath, (char**)&texdata);
		moddata = (byte *)m->extradata;
		phdr = (studiohdr_t *)moddata;

		thdr = (studiohdr_t *)texdata;

		// merge textures with main model buffer
		m->extradata = malloc( phdr->length + thdr->length - sizeof( studiohdr_t ));	// we don't need two headers
		memcpy( m->extradata, moddata, phdr->length );
		memcpy( (byte *)m->extradata + phdr->length, texdata + sizeof( studiohdr_t ), thdr->length - sizeof( studiohdr_t ));

		// merge header
		newhdr = (studiohdr_t *)m->extradata;

		newhdr->numskinfamilies = thdr->numskinfamilies;
		newhdr->numtextures = thdr->numtextures;
		newhdr->numskinref = thdr->numskinref;
		newhdr->textureindex = phdr->length;
		newhdr->skinindex = newhdr->textureindex + ( newhdr->numtextures * sizeof( mstudiotexture_t ));
		newhdr->texturedataindex = newhdr->skinindex + (newhdr->numskinfamilies * newhdr->numskinref * sizeof( short ));
		newhdr->length = phdr->length + thdr->length - sizeof( studiohdr_t );

		// and finally merge datapointers for textures
		for( int i = 0; i < newhdr->numtextures; i++ )
		{
			mstudiotexture_t *ptexture = (mstudiotexture_t *)(((byte *)newhdr) + newhdr->textureindex);
			ptexture[i].index += ( phdr->length - sizeof( studiohdr_t ));
//			printf( "Texture %i [%s]\n", i, ptexture[i].name );
			// now we can replace offsets with real pointers
//			ptexture[i].pixels = (byte *)newhdr + ptexture[i].index;
		}

		free( moddata );
		free( texdata );
	}
	else
	{
#if 0
		for( int i = 0; i < phdr->numtextures; i++ )
		{
			mstudiotexture_t *ptexture = (mstudiotexture_t *)(((byte *)phdr) + phdr->textureindex);
			// now we can replace offsets with real pointers
			ptexture[i].pixels = (byte *)phdr + ptexture[i].index;
		}
#endif
	}

	VectorCopy( origin, m->origin );
	VectorCopy( angles, m->angles );
	VectorCopy( scale, m->scale );

	m->trace_mode = trace_mode;

	m->body = body;
	m->skin = skin;

	m->mesh.StudioConstructMesh( m );

	num_models++;
	return m;
}

// =====================================================================================
//  Where the game lights a studio model from
//      The engine (R_StudioDynamicLight) first looks from the model toward the
//      sun (sv_skyvec). If that line reaches the sky, the model takes
//      sv_skycolor and no lightmap is read. Otherwise R_LightPoint follows
//      sv_skyvec (straight down without a light_environment) for 2048 units and
//      reads the single lightmap texel of the first face it crosses. The whole
//      model gets that one color, so a model shadowing that texel goes black.
// =====================================================================================
#define GAME_LIGHTPOINT_RANGE	2048.0

// rendermode values of the game (const.h)
#define RENDER_NORMAL		0
#define RENDER_TRANSADD		5

static void SetVec( vec3_t v, vec_t x, vec_t y, vec_t z )
{
	v[0] = x;
	v[1] = y;
	v[2] = z;
}

static vec3_t g_game_skyvec;	// sv_skyvec: the direction sunlight travels in

static void FindGameSkyVec( void )
{
	VectorClear( g_game_skyvec );

	// light_environment sets the cvars when it spawns, so the last one wins;
	// info_sunlight exists to say which one it is
	const entity_t *sun = NULL;
	for( int i = 0; i < g_numentities; i++ )
	{
		const char *classname = ValueForKey( &g_entities[i], "classname" );
		if( !strcmp( classname, "info_sunlight" ))
		{
			sun = &g_entities[i];
			break;
		}
		if( !strcmp( classname, "light_environment" ))
			sun = &g_entities[i];
	}
	if( !sun ) return;

	// same angle rules as the sun in CreateDirectLights
	vec3_t angles;
	GetVectorForKey( sun, "angles", angles );
	vec_t angle = FloatForKey( sun, "angle" );
	if( angle == ANGLE_UP )
	{
		SetVec( g_game_skyvec, 0, 0, 1 );
		return;
	}
	if( angle == ANGLE_DOWN )
	{
		SetVec( g_game_skyvec, 0, 0, -1 );
		return;
	}
	if( !angle ) angle = angles[1];
	vec_t pitch = FloatForKey( sun, "pitch" );
	if( !pitch ) pitch = angles[0];

	g_game_skyvec[0] = (vec_t)( cos( angle / 180 * Q_PI ) * cos( pitch / 180 * Q_PI ));
	g_game_skyvec[1] = (vec_t)( sin( angle / 180 * Q_PI ) * cos( pitch / 180 * Q_PI ));
	g_game_skyvec[2] = (vec_t)sin( pitch / 180 * Q_PI );
}

// Port of the engine's RecursiveLightPoint over the world BSP. It does not stop
// at solid leaves: it takes the first face on a crossed node plane whose
// lightmap extents hold the crossing point.
static bool GameLightPoint_r( int nodenum, const vec3_t start, const vec3_t end, vec3_t texel_out, vec_t &radius_out )
{
	if( nodenum < 0 ) return false;

	const dnode_t *node = &g_dnodes[nodenum];
	const dplane_t *plane = &g_dplanes[node->planenum];
	const vec_t front = DotProduct( start, plane->normal ) - plane->dist;
	const vec_t back = DotProduct( end, plane->normal ) - plane->dist;
	const int side = front < 0;

	if(( back < 0 ) == side )
		return GameLightPoint_r( node->children[side], start, end, texel_out, radius_out );

	vec3_t mid;
	const vec_t frac = front / ( front - back );
	for( int k = 0; k < 3; k++ )
		mid[k] = start[k] + ( end[k] - start[k] ) * frac;

	if( GameLightPoint_r( node->children[side], start, mid, texel_out, radius_out ))
		return true;

	for( int i = 0; i < node->numfaces; i++ )
	{
		const int facenum = node->firstface + i;
		const texinfo_t *tex = &g_texinfo[ParseTexinfoForFace( &g_dfaces[facenum] )];
		if( tex->flags & TEX_SPECIAL ) continue; // no lightmap

		int mins[2], maxs[2];
		GetFaceExtents( facenum, mins, maxs );

		int texel[2];
		bool inside = true;
		for( int j = 0; j < 2 && inside; j++ )
		{
			const int st = (int)( DotProduct( mid, tex->vecs[j] ) + tex->vecs[j][3] );
			const int d = st - mins[j] * TEXTURE_STEP;
			if( d < 0 || d > ( maxs[j] - mins[j] ) * TEXTURE_STEP )
				inside = false;
			texel[j] = mins[j] * TEXTURE_STEP + ( d >> 4 ) * TEXTURE_STEP;
		}
		if( !inside ) continue;

		// the texel sits on the lightmap grid, up to one texel away from mid:
		// solve s, t and the plane for its position
		const vec_t *a = tex->vecs[0], *b = tex->vecs[1], *n = plane->normal;
		const vec_t rs = texel[0] - a[3], rt = texel[1] - b[3], rn = plane->dist;
		const vec_t det = a[0] * ( b[1] * n[2] - b[2] * n[1] ) - a[1] * ( b[0] * n[2] - b[2] * n[0] ) + a[2] * ( b[0] * n[1] - b[1] * n[0] );
		if( fabs( det ) > NORMAL_EPSILON )
		{
			texel_out[0] = ( rs * ( b[1] * n[2] - b[2] * n[1] ) - a[1] * ( rt * n[2] - b[2] * rn ) + a[2] * ( rt * n[1] - b[1] * rn )) / det;
			texel_out[1] = ( a[0] * ( rt * n[2] - b[2] * rn ) - rs * ( b[0] * n[2] - b[2] * n[0] ) + a[2] * ( b[0] * rn - rt * n[0] )) / det;
			texel_out[2] = ( a[0] * ( b[1] * rn - rt * n[1] ) - a[1] * ( b[0] * rn - rt * n[0] ) + rs * ( b[0] * n[1] - b[1] * n[0] )) / det;
		}
		else
		{
			VectorCopy( mid, texel_out );
		}

		// the texel blurs the light samples around it (lmcache_side in
		// lightmap.cpp): reach the farthest one, diagonally
		const vec_t scale = qmin( VectorLength( a ), VectorLength( b ));
		const vec_t texel_size = scale > NORMAL_EPSILON ? TEXTURE_STEP / scale : TEXTURE_STEP;
		const int density = g_extra && !g_fastmode ? 3 : 1;
		const int blur_side = (int)ceil(( 0.5 * g_blur * density - 0.5 ) * ( 1 - NORMAL_EPSILON ));
		const vec_t spacing = texel_size / density;
		radius_out = blur_side * spacing * (vec_t)1.41421356 + 0.5f * spacing;
		return true;
	}

	return GameLightPoint_r( node->children[!side], mid, end, texel_out, radius_out );
}

enum gamelight_t
{
	GAMELIGHT_SKY,		// lit by sv_skycolor, the lightmap is not read
	GAMELIGHT_TEXEL,	// lit by one lightmap texel
	GAMELIGHT_NONE		// no lightmap along the line: drawn black
};

static gamelight_t FindGameLightSpot( const vec3_t origin, vec3_t texel_out, vec_t &radius_out )
{
	vec3_t dir, src, end;
	VectorCopy( g_game_skyvec, dir );
	if( VectorCompare( dir, vec3_origin ))
		SetVec( dir, 0, 0, -1 );

	VectorCopy( origin, src );
	src[2] -= dir[2] * 8.0f;

	if( !VectorCompare( g_game_skyvec, vec3_origin ))
	{
		VectorMA( src, -GAME_LIGHTPOINT_RANGE, dir, end );
		if( TestLine( src, end ) == CONTENTS_SKY )
			return GAMELIGHT_SKY;
	}

	VectorMA( src, GAME_LIGHTPOINT_RANGE, dir, end );
	if( GameLightPoint_r( g_dmodels[0].headnode[0], src, end, texel_out, radius_out ))
		return GAMELIGHT_TEXEL;
	return GAMELIGHT_NONE;
}

static bool IsStudioModelName( const char *model )
{
	const size_t len = strlen( model );
	return model[0] != '*' && len > 4 && !Q_stricmp( model + len - 4, ".mdl" );
}

// -studioshadowall leaves out models the game draws see-through
static bool IsOpaqueRenderMode( const entity_t *e )
{
	// renderamt defaults to 0, which hides the model in any other mode
	const int rendermode = IntForKey( e, "rendermode" );
	if( rendermode == RENDER_NORMAL )
		return true;
	if( rendermode == RENDER_TRANSADD )
		return false;
	return IntForKey( e, "renderamt" ) >= 255;
}

// Point entities whose model key only tells the editor what to draw. The game
// never sets that model on them, so they are invisible: the player spawns
// carry models/player/gsg9/gsg9.mdl in the CS FGD.
static bool GameDrawsModel( const char *classname )
{
	return Q_strnicmp( classname, "info_", 5 )
		&& Q_strnicmp( classname, "light", 5 )
		&& Q_strnicmp( classname, "path_", 5 );
}

// =====================================================================================
//  LoadStudioModels
// =====================================================================================
void LoadStudioModels( void )
{
	memset( models, 0, sizeof( models ));
	num_models = 0;

	FindGameSkyVec();
	FindModelPaths();
	g_missingmodels.clear();

	int count_sky = 0, count_texel = 0, count_dark = 0, count_spots = 0;

	for( int i = 0; i < g_numentities; i++ )
	{
		const char *name, *model;
		vec3_t origin, angles;

		entity_t* e = &g_entities[i];
		name = ValueForKey( e, "classname" );
		model = ValueForKey( e, "model" );
		GetVectorForKey( e, "origin", origin );

		// what the game will light this model with, for every .mdl on the map
		gamelight_t gamelight = GAMELIGHT_NONE;
		vec3_t lightspot;
		vec_t lightspot_radius = 0;
		const bool studio = IsStudioModelName( model ) && GameDrawsModel( name );
		if( studio )
		{
			gamelight = FindGameLightSpot( origin, lightspot, lightspot_radius );
			if( gamelight == GAMELIGHT_SKY )
				count_sky++;
			else if( gamelight == GAMELIGHT_TEXEL )
				count_texel++;
			else
			{
				count_dark++;
				const dleaf_t *leaf = PointInLeaf( origin );
				Warning( "%s (%s) at (%.0f %.0f %.0f) has no lightmap below it: the game will draw it black%s",
					name, model, origin[0], origin[1], origin[2],
					leaf->contents == CONTENTS_SOLID ? ". Its origin is inside a solid brush" : "" );
			}
		}

		if( !g_studioshadow ) continue;

		if( !Q_stricmp( name, "env_static" ))
		{
			int spawnflags = IntForKey( e, "spawnflags" );
			if( spawnflags & 4 ) continue; // shadow disabled
		
			model = ValueForKey( e, "model" );

			if( !model || !*model )
			{
				Developer( DEVELOPER_LEVEL_WARNING, "env_static has empty model field\n" );
				continue;
			}
		}
		else if( *ValueForKey( e, "zhlt_studioshadow" ))
		{
			if( !IntForKey( e, "zhlt_studioshadow" ) || !*model )
				continue;
		}
		else if( !g_studioshadowall || !studio || !IsOpaqueRenderMode( e ))
		{
			continue;
		}

		GetVectorForKey( e, "angles", angles );

		angles[0] = -angles[0]; // Stupid quake bug workaround
		int trace_mode = SHADOW_NORMAL;	// default mode

		// make sure what field is present
		if( strcmp( ValueForKey( e, "zhlt_shadowmode" ), "" ))
			trace_mode = IntForKey( e, "zhlt_shadowmode" );

		int body = IntForKey( e, "body" );
		int skin = IntForKey( e, "skin" );

		float scale = FloatForKey( e, "scale" );
		vec3_t xform;

		GetVectorForKey( e, "xform", xform );

		if( VectorCompare( xform, vec3_origin ))
			VectorFill( xform, scale );

		// check xform values
		if( xform[0] < 0.01f ) xform[0] = 1.0f;
		if( xform[1] < 0.01f ) xform[1] = 1.0f;
		if( xform[2] < 0.01f ) xform[2] = 1.0f;
		if( xform[0] > 16.0f ) xform[0] = 16.0f;
		if( xform[1] > 16.0f ) xform[1] = 16.0f;
		if( xform[2] > 16.0f ) xform[2] = 16.0f;

		model_t *m = LoadStudioModel( model, origin, angles, xform, body, skin, trace_mode );

		// keep the texel the game reads out of this model's own shadow, or the
		// model darkens itself; with the sun on it the game never reads it
		if( m && g_studiolightspot && gamelight == GAMELIGHT_TEXEL )
		{
			m->has_lightspot = true;
			VectorCopy( lightspot, m->lightspot );
			m->lightspot_radius = lightspot_radius;
			count_spots++;
		}
	}

	BuildStudioBounds();
	Log( "%i opaque studio models\n", num_models );
	if( !g_missingmodels.empty() )
	{
		for( const auto &missing : g_missingmodels )
		{
			Warning( "LoadStudioModel: couldn't find %s (%i %s), it casts no shadow",
				missing.first.c_str(), missing.second, missing.second == 1 ? "entity" : "entities" );
		}
		Log( "Models were looked for in:\n" );
		for( const std::string &dir : g_modelpaths )
			Log( "    %s\n", dir.c_str() );
		if( !*g_moddir )
			Log( "Use -moddir <game>/cstrike (or the mod's folder) to read them from the game\n" );
	}
	if( count_sky + count_texel + count_dark )
	{
		Log( "studio models lit in game: %i by the sun, %i by a lightmap texel, %i black\n", count_sky, count_texel, count_dark );
	}
	if( count_spots )
	{
		Log( "%i shadowing models keep their own light texel unshadowed\n", count_spots );
	}
}

void FreeStudioModels( void )
{
	for( int i = 0; i < num_models; i++ )
	{
		model_t *m = &models[i];

		// first, delete the mesh
		m->mesh.FreeMesh();

		// unload the model
		Free( m->extradata );
	}

	memset( models, 0, sizeof( models ));
	num_models = 0;
	g_studio_bounds_valid = false;
}

void MoveBounds( const vec3_t start, const vec3_t mins, const vec3_t maxs, const vec3_t end, vec3_t outmins, vec3_t outmaxs )
{
	for( int i = 0; i < 3; i++ )
	{
		if( end[i] > start[i] )
		{
			outmins[i] = start[i] + mins[i] - 1.0f;
			outmaxs[i] = end[i] + maxs[i] + 1.0f;
		}
		else
		{
			outmins[i] = end[i] + mins[i] - 1.0f;
			outmaxs[i] = start[i] + maxs[i] + 1.0f;
		}
	}
}

bool TestSegmentAgainstStudioList( const vec_t* p1, const vec_t* p2 )
{
	if( !num_models || !g_studio_bounds_valid ) return false; // easy out
	if( !SegmentIntersectsAABB( p1, p2, g_studio_global_mins, g_studio_global_maxs ))
		return false;

	for( int i = 0; i < num_models; i++ )
	{
		if( !SegmentIntersectsAABB( p1, p2, g_studio_mins[i], g_studio_maxs[i] ))
			continue;

		model_t *m = &models[i];

		if( m->has_lightspot )
		{
			const vec_t r2 = m->lightspot_radius * m->lightspot_radius;
			vec3_t d1, d2;
			VectorSubtract( p1, m->lightspot, d1 );
			VectorSubtract( p2, m->lightspot, d2 );
			if( DotProduct( d1, d1 ) < r2 || DotProduct( d2, d2 ) < r2 )
				continue; // the texel the game lights this model from
		}

		mmesh_t *pMesh = m->mesh.GetMesh();
		areanode_t *pHeadNode = m->mesh.GetHeadNode();

		if( !pMesh )
			continue;

		TraceMesh	trm;	// a name like Doom3 :-)

		trm.SetTraceModExtradata( m->extradata );
		trm.SetTraceMesh( pMesh, pHeadNode );
		trm.SetupTrace( p1, vec3_origin, vec3_origin, p2 );

		if( trm.DoTrace())
			return true; // we hit studio model
	}

	return false;
}

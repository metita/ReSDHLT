#include "qrad.h"
#include "meshtrace.h"
#include "filelib.h"
#include "stringlib.h"

#include <map>
#include <string>
#include <sys/stat.h>

#ifdef SYSTEM_WIN32
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#endif

#define MAX_MODELS		1024

model_t models[MAX_MODELS];
int num_models;

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
	if( !num_models ) return false; // easy out

	vec3_t	trace_mins, trace_maxs;

	MoveBounds( p1, vec3_origin, vec3_origin, p2, trace_mins, trace_maxs );

	for( int i = 0; i < num_models; i++ )
	{
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

		if( !pMesh || !m->mesh.Intersect( trace_mins, trace_maxs ))
			continue; // bad model or not intersect with trace

		TraceMesh	trm;	// a name like Doom3 :-)

		trm.SetTraceModExtradata( m->extradata );
		trm.SetTraceMesh( pMesh, pHeadNode );
		trm.SetupTrace( p1, vec3_origin, vec3_origin, p2 );

		if( trm.DoTrace())
			return true; // we hit studio model
	}

	return false;
}

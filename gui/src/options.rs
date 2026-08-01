//! Compile options, what each one does, and the presets.
//!
//! The advice here is not folklore. Where a number is quoted it was measured on
//! real CS 1.6 maps with the tools in this repository; see docs/BENCHMARKS.md
//! for the raw figures and docs/FPS_Y_TOOL_TEXTURES.md for the FPS side.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Subfolder that holds everything a compile produces except the .bsp: logs,
/// the portal file, the .p0-.p3 intermediates and the copied .map.
pub const WORK_SUBDIR: &str = "intermedios";

/// Radiosity visibility matrix method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VisMatrix {
    Normal,
    Sparse,
    Off,
}

impl VisMatrix {
    pub fn flag(self) -> &'static str {
        match self {
            VisMatrix::Normal => "normal",
            VisMatrix::Sparse => "sparse",
            VisMatrix::Off => "off",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VisMatrix::Normal => "normal (rápido, mucha RAM)",
            VisMatrix::Sparse => "sparse (equilibrado)",
            VisMatrix::Off => "off (mapas enormes)",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            VisMatrix::Normal => {
                "Guarda en memoria, sin comprimir, qué parche de luz ve a qué otro. \
                 Es el más rápido de los tres, pero el consumo de RAM crece con el \
                 cuadrado de la cantidad de parches: en un mapa grande con -chop bajo \
                 puede pedir varios GB y fallar por falta de memoria.\n\n\
                 NO cambia la iluminación resultante, solo cómo se calcula."
            }
            VisMatrix::Sparse => {
                "Guarda esa misma información comprimida: solo los pares que \
                 realmente se ven. Usa mucha menos RAM a cambio de algo de CPU. Es el \
                 default y el equilibrio correcto para prácticamente cualquier mapa.\n\n\
                 NO cambia la iluminación resultante."
            }
            VisMatrix::Off => {
                "No construye ninguna matriz: recalcula la visibilidad cada vez que la \
                 necesita. La RAM deja de ser un problema, pero es bastante más lento. \
                 Solo tiene sentido cuando los otros dos se quedan sin memoria.\n\n\
                 NO cambia la iluminación resultante."
            }
        }
    }
}

/// How thorough the VIS stage is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VisQuality {
    Fast,
    Normal,
    Full,
}

impl VisQuality {
    pub fn label(self) -> &'static str {
        match self {
            VisQuality::Fast => "fast (borrador)",
            VisQuality::Normal => "normal",
            VisQuality::Full => "full (para publicar)",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            VisQuality::Fast => {
                "Cálculo aproximado de visibilidad. Compila en segundos, pero el PVS \
                 queda de más: el motor dibuja cosas que en realidad no se ven y los \
                 FPS bajan. Solo para comprobar que el mapa carga."
            }
            VisQuality::Normal => {
                "Cálculo completo estándar. Suficiente mientras construyes el mapa."
            }
            VisQuality::Full => {
                "El PVS más ajustado que las herramientas saben calcular. Tarda más en \
                 compilar y produce MENOS wpoly en juego, es decir más FPS. Úsalo \
                 siempre en la versión que publicas."
            }
        }
    }
}

/// Everything the UI can set. Serialised as the saved profile.
///
/// `PartialEq` is what the project auto-save watches: the options are compared
/// against the stored copy each frame, which is cheaper and more reliable than
/// threading a "changed" flag through every widget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    // ---- paths ----
    pub map_path: String,
    pub tools_dir: String,
    /// Empty = compile next to the .map.
    pub output_dir: String,
    /// Folder holding the .wad files this map uses.
    pub wad_dir: String,
    /// Resolve the map's WAD list and hand CSG a generated wad.cfg instead of
    /// letting it read the (often broken) paths stored in the map.
    #[serde(alias = "rewrite_wad_key")]
    pub auto_wads: bool,
    /// Give the compile its own folder inside the output folder, named after
    /// the project, with the scratch files in a subfolder of that.
    pub organize_output: bool,
    /// Name of the project this belongs to. Empty means "use the map's name".
    pub project_name: String,

    // ---- shared ----
    pub threads: u32, // 0 = autodetect
    pub chart: bool,
    pub verbose: bool,
    pub low_priority: bool,

    // ---- CSG ----
    pub run_csg: bool,
    pub nowadtextures: bool,
    pub nodeterministic: bool,
    pub worldextent: u32, // 0 = leave default
    pub mergeentities: bool,
    pub mergesize: u32, // longest side a merged group may reach, 0 = no limit
    pub mergeblend: bool,
    pub csg_extra: String,

    // ---- BSP ----
    pub run_bsp: bool,
    pub subdivide: u32,
    pub maxnodesize: u32,
    pub leakonly: bool,
    pub notjunc: bool,
    pub noclip: bool,
    pub bsp_extra: String,

    // ---- VIS ----
    pub run_vis: bool,
    pub vis_quality: VisQuality,
    pub maxdistance: u32, // 0 = default
    pub vis_extra: String,

    // ---- RAD ----
    pub run_rad: bool,
    pub rad_fast: bool,
    pub extra_sampling: bool,
    pub bounce: u32,
    pub skylevel: u32,
    pub softsky: bool,
    pub chop: f32,
    pub texchop: f32,
    pub smooth: f32,
    pub vismatrix: VisMatrix,
    pub pre25: bool,
    pub nostudioshadow: bool,
    pub profile: bool,
    pub rad_extra: String,

    // ---- interface ----
    /// Zoom applied to the whole UI. Saved so a 4K user does not have to
    /// re-scale on every launch.
    pub ui_scale: f32,
}

impl Default for Options {
    fn default() -> Self {
        // These defaults ARE the recommended preset.
        Self {
            map_path: String::new(),
            tools_dir: String::new(),
            output_dir: String::new(),
            wad_dir: String::new(),
            auto_wads: true,
            organize_output: true,
            project_name: String::new(),

            threads: 0,
            chart: true,
            verbose: false,
            low_priority: false,

            run_csg: true,
            // On by default: a map whose textures are embedded works for everyone,
            // with no "missing WAD" on join. See the tooltip for the trade-off.
            nowadtextures: true,
            nodeterministic: false,
            worldextent: 0,
            mergeentities: false,
            mergesize: 1024,
            mergeblend: false,
            csg_extra: String::new(),

            run_bsp: true,
            subdivide: 240,
            maxnodesize: 1024,
            leakonly: false,
            notjunc: false,
            noclip: false,
            bsp_extra: String::new(),

            run_vis: true,
            vis_quality: VisQuality::Full,
            maxdistance: 0,
            vis_extra: String::new(),

            run_rad: true,
            rad_fast: false,
            extra_sampling: true,
            bounce: 12,
            skylevel: 6,
            softsky: true,
            chop: 64.0,
            texchop: 32.0,
            smooth: 50.0,
            vismatrix: VisMatrix::Sparse,
            // On by default: almost nobody runs the 25th anniversary build, and
            // compiling for it breaks bright areas on older clients. The reverse
            // is merely a little dimmer. See the tooltip.
            pre25: true,
            nostudioshadow: false,
            profile: false,
            rad_extra: String::new(),

            ui_scale: 1.0,
        }
    }
}

/// Named starting points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Draft,
    Recommended,
    Release,
}

impl Preset {
    pub fn label(self) -> &'static str {
        match self {
            Preset::Draft => "Borrador",
            Preset::Recommended => "Recomendado",
            Preset::Release => "Publicar",
        }
    }

    pub fn summary(self) -> &'static str {
        match self {
            Preset::Draft => {
                "Lo más rápido posible, para comprobar que el mapa carga y no tiene \
                 leaks. La iluminación queda fea y los FPS peores que los reales: no \
                 juzgues el mapa con esto."
            }
            Preset::Recommended => {
                "El que conviene usar casi siempre. Calidad completa con las mejoras \
                 medidas de este fork ya aplicadas. Es también el default al abrir."
            }
            Preset::Release => {
                "Para la versión que publicas. Igual que Recomendado pero con el \
                 muestreo de cielo al máximo (-skylevel 7). Cuesta ~1.65x más tiempo \
                 en RAD por una diferencia que no se ve: solo tiene sentido si \
                 necesitas reproducir exactamente la salida de SDHLT original."
            }
        }
    }

    /// Whether these options are exactly what this preset produces.
    ///
    /// Rather than tracking "which preset was clicked", which goes stale the
    /// moment a slider moves, the preset is re-applied to a copy and compared:
    /// what `apply` preserves (paths, threads, layout, UI scale) is ignored for
    /// free.
    pub fn is_active(self, o: &Options) -> bool {
        let mut expected = o.clone();
        self.apply(&mut expected);
        expected == *o
    }

    /// Applies the preset, preserving paths and thread count.
    pub fn apply(self, o: &mut Options) {
        let map = o.map_path.clone();
        let tools = o.tools_dir.clone();
        let out = o.output_dir.clone();
        let wad = o.wad_dir.clone();
        let rewrite = o.auto_wads;
        let threads = o.threads;
        let scale = o.ui_scale;
        let organize = o.organize_output;
        let project = o.project_name.clone();

        *o = Options::default();
        o.ui_scale = scale;
        o.organize_output = organize;
        o.project_name = project;
        o.map_path = map;
        o.tools_dir = tools;
        o.output_dir = out;
        o.wad_dir = wad;
        o.auto_wads = rewrite;
        o.threads = threads;

        match self {
            Preset::Draft => {
                o.vis_quality = VisQuality::Fast;
                o.extra_sampling = false;
                o.rad_fast = true;
                o.bounce = 1;
                o.skylevel = 4;
                o.chart = false;
            }
            Preset::Recommended => { /* defaults already are this */ }
            Preset::Release => {
                o.skylevel = 7;
            }
        }
    }
}

/// Project names are free text but end up as a folder, so the characters
/// Windows refuses are replaced rather than failing the compile.
pub fn sanitize_folder(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 32 => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim_end_matches([' ', '.']).to_string();
    if cleaned.is_empty() {
        "mapa".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_layout_puts_the_bsp_alone_and_the_rest_below() {
        let mut o = Options::default();
        o.map_path = r"E:\Mapping\ba_dust_island.map".to_string();
        o.output_dir = r"C:\Users\marce\Desktop\Mapas".to_string();
        o.project_name = "zm_hola".to_string();

        let base = o.output_base().unwrap();
        let work = o.work_dir().unwrap();
        assert!(base.ends_with(r"Mapas\zm_hola"), "{}", base.display());
        assert_eq!(work, base.join(WORK_SUBDIR));

        // No project name: the map names the folder.
        o.project_name.clear();
        assert!(o
            .output_base()
            .unwrap()
            .ends_with(r"Mapas\ba_dust_island"));

        // Turned off, everything lands in the output folder as before.
        o.organize_output = false;
        assert_eq!(o.output_base().unwrap(), PathBuf::from(&o.output_dir));
        assert_eq!(o.work_dir().unwrap(), PathBuf::from(&o.output_dir));

        // Without an output folder and without the layout, everything is
        // compiled in place, next to the .map.
        o.output_dir.clear();
        assert!(o.output_base().is_none());
        assert!(o.work_dir().is_none());
    }

    #[test]
    fn the_layout_still_applies_without_an_output_folder() {
        let mut o = Options::default();
        o.map_path = r"E:\Mapping\ba_dust_island.map".to_string();
        o.project_name = "zm_hola".to_string();

        // No project subfolder here: the .bsp stays where it has always been,
        // beside the source map. Only the scratch gets moved out of the way.
        let base = o.output_base().unwrap();
        assert_eq!(base, PathBuf::from(r"E:\Mapping"));
        assert_eq!(o.work_dir().unwrap(), PathBuf::from(r"E:\Mapping\intermedios"));

        // A map path with no folder gives nothing to hang the layout on.
        o.map_path = "ba_dust_island.map".to_string();
        assert!(o.output_base().is_none());
        assert!(o.work_dir().is_none());
    }

    #[test]
    fn merging_only_reaches_the_command_line_when_it_is_asked_for() {
        let mut o = Options::default();

        // Off by default, and the size alone means nothing without it.
        o.mergesize = 512;
        assert!(!o.csg_args().iter().any(|a| a.starts_with("-merge")));

        // The default size is left implicit, so the command line stays short.
        o.mergesize = 1024;
        o.mergeentities = true;
        let a = o.csg_args();
        assert!(a.contains(&"-mergeentities".to_string()));
        assert!(!a.iter().any(|a| a == "-mergesize"));

        // A size that is not the default is passed through, 0 included.
        o.mergesize = 0;
        let a = o.csg_args();
        let i = a.iter().position(|a| a == "-mergesize").unwrap();
        assert_eq!(a[i + 1], "0");

        o.mergeblend = true;
        assert!(o.csg_args().contains(&"-mergeblend".to_string()));

        // Blending is a modifier of the merge, never a switch of its own.
        o.mergeentities = false;
        assert!(!o.csg_args().iter().any(|a| a.starts_with("-merge")));
    }

    #[test]
    fn project_names_survive_becoming_folders() {
        assert_eq!(sanitize_folder("zm_hola"), "zm_hola");
        assert_eq!(sanitize_folder(r"de_dust: beta/3"), "de_dust_ beta_3");
        assert_eq!(sanitize_folder("  raro.  "), "raro");
        assert_eq!(sanitize_folder("   "), "mapa");
    }
}

/// One row of advice shown in the "always do this" panel.
pub struct Rule {
    pub title: &'static str,
    pub body: &'static str,
}

pub fn always_rules() -> Vec<Rule> {
    vec![
        Rule {
            title: "Deja los hilos en automático",
            body: "Este fork arregló dos bugs por los que las herramientas usaban un \
                   solo hilo: en Linux siempre, y en Windows con CPUs de más de 32 \
                   hilos lógicos. Medido, usar todos los núcleos dio 1.68x en una \
                   máquina de 2; con 8 la diferencia es mucho mayor. No pongas un \
                   número a mano salvo que quieras dejar CPU libre para otra cosa.",
        },
        Rule {
            title: "Deja -pre25 activado",
            body: "Salvo que sepas que todos tus jugadores usan el cliente del 25 \
                   aniversario, que en la práctica casi nadie usa. Compilar sin -pre25 \
                   y jugar en un cliente antiguo produce zonas brillantes rotas; al \
                   revés solo se ve un poco menos brillante. El error es asimétrico, \
                   así que -pre25 es la opción segura.",
        },
        Rule {
            title: "VIS en 'full' para lo que publicas",
            body: "VIS es lo único que decide cuánto le pide el mapa al motor en cada \
                   frame. 'full' tarda más en compilar y da menos wpoly en juego. \
                   Compila con 'fast' mientras construyes si quieres, pero nunca \
                   publiques un mapa con VIS rápido.",
        },
        Rule {
            title: "No toques -subdivide",
            body: "El límite de 240 no es un número conservador: es el techo que impone \
                   MAX_SURFACE_EXTENT. Si lo subes, el mapa deja de cargar en el \
                   software renderer y en el HLDS. Bajarlo genera más caras y baja los \
                   FPS. Déjalo en 240.",
        },
        Rule {
            title: "Los FPS los baja el mapper, no el compilador",
            body: "La fusión de caras del BSP ya elimina entre 19% y 46% de las caras \
                   (medido en tres mapas) y el PVS de VIS ya es casi exacto. Lo que \
                   mueve la aguja es NULL en todo lo que no se ve, y \
                   SOLIDHINT/BEVELHINT en terreno y escaleras. Mira \
                   docs/FPS_Y_TOOL_TEXTURES.md.",
        },
        Rule {
            title: "Más calidad de luz no cuesta FPS",
            body: "RAD no cambia la geometría que dibuja el motor, así que -extra y más \
                   bounces cuestan tiempo de compilación y algo de tamaño de BSP, pero \
                   cero FPS en juego. Es el lugar donde puedes ser generoso.",
        },
        Rule {
            title: "Mira el chart cuando compiles en serio",
            body: "Con 'chart' activado las herramientas imprimen cuánto de cada límite \
                   del motor estás usando. Es la forma de enterarte de que vas camino a \
                   un límite antes de chocarlo.",
        },
    ]
}

fn push_num(args: &mut Vec<String>, flag: &str, value: impl std::fmt::Display) {
    args.push(flag.to_string());
    args.push(value.to_string());
}

fn push_extra(args: &mut Vec<String>, extra: &str) {
    for token in extra.split_whitespace() {
        args.push(token.to_string());
    }
}

impl Options {
    fn shared(&self, args: &mut Vec<String>) {
        if self.threads > 0 {
            push_num(args, "-threads", self.threads);
        }
        if self.chart {
            args.push("-chart".to_string());
        }
        if self.verbose {
            args.push("-verbose".to_string());
        }
        if self.low_priority {
            args.push("-low".to_string());
        }
    }

    pub fn csg_args(&self) -> Vec<String> {
        let mut a = Vec::new();
        self.shared(&mut a);
        if self.nowadtextures {
            a.push("-nowadtextures".to_string());
        }
        if self.nodeterministic {
            a.push("-nodeterministic".to_string());
        }
        if self.worldextent > 0 {
            push_num(&mut a, "-worldextent", self.worldextent);
        }
        if self.mergeentities {
            a.push("-mergeentities".to_string());
            if self.mergesize != 1024 {
                push_num(&mut a, "-mergesize", self.mergesize);
            }
            if self.mergeblend {
                a.push("-mergeblend".to_string());
            }
        }
        push_extra(&mut a, &self.csg_extra);
        a
    }

    pub fn bsp_args(&self) -> Vec<String> {
        let mut a = Vec::new();
        self.shared(&mut a);
        if self.leakonly {
            a.push("-leakonly".to_string());
        }
        if self.subdivide != 240 {
            push_num(&mut a, "-subdivide", self.subdivide);
        }
        if self.maxnodesize != 1024 {
            push_num(&mut a, "-maxnodesize", self.maxnodesize);
        }
        if self.notjunc {
            a.push("-notjunc".to_string());
        }
        if self.noclip {
            a.push("-noclip".to_string());
        }
        push_extra(&mut a, &self.bsp_extra);
        a
    }

    pub fn vis_args(&self) -> Vec<String> {
        let mut a = Vec::new();
        self.shared(&mut a);
        match self.vis_quality {
            VisQuality::Fast => a.push("-fast".to_string()),
            VisQuality::Normal => {}
            VisQuality::Full => a.push("-full".to_string()),
        }
        if self.maxdistance > 0 {
            push_num(&mut a, "-maxdistance", self.maxdistance);
        }
        push_extra(&mut a, &self.vis_extra);
        a
    }

    pub fn rad_args(&self) -> Vec<String> {
        let mut a = Vec::new();
        self.shared(&mut a);
        // Only RAD understands -waddir; CSG reads its WAD list from the map's
        // worldspawn key, which is why the map gets rewritten instead.
        if !self.wad_dir.trim().is_empty() {
            a.push("-waddir".to_string());
            a.push(self.wad_dir.clone());
        }
        if self.rad_fast {
            a.push("-fast".to_string());
        }
        if self.extra_sampling {
            a.push("-extra".to_string());
        }
        push_num(&mut a, "-bounce", self.bounce);
        push_num(&mut a, "-skylevel", self.skylevel);
        push_num(&mut a, "-softsky", if self.softsky { 1 } else { 0 });
        if (self.chop - 64.0).abs() > 0.01 {
            push_num(&mut a, "-chop", self.chop);
        }
        if (self.texchop - 32.0).abs() > 0.01 {
            push_num(&mut a, "-texchop", self.texchop);
        }
        if (self.smooth - 50.0).abs() > 0.01 {
            push_num(&mut a, "-smooth", self.smooth);
        }
        a.push("-vismatrix".to_string());
        a.push(self.vismatrix.flag().to_string());
        if self.pre25 {
            a.push("-pre25".to_string());
        }
        if self.nostudioshadow {
            a.push("-nostudioshadow".to_string());
        }
        if self.profile {
            a.push("-profile".to_string());
        }
        push_extra(&mut a, &self.rad_extra);
        a
    }

    /// Folder name for this compile: the project's name, or the map's if there
    /// is no project.
    pub fn effective_name(&self) -> String {
        let name = self.project_name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
        Path::new(self.map_path.trim())
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("mapa")
            .to_string()
    }

    /// Where the finished .bsp ends up.
    ///
    /// With `organize_output` the output folder gets one subfolder per project,
    /// so pointing several maps at the same "Mapas" folder keeps them apart
    /// instead of piling everything together.
    ///
    /// With no output folder the .bsp keeps landing next to the source map,
    /// which is what anyone who leaves the field empty expects. The layout
    /// still applies there: no per-project subfolder (that would move the .bsp
    /// out from under them), but the scratch files do get their own.
    pub fn output_base(&self) -> Option<PathBuf> {
        if !self.uses_output_dir() {
            if !self.organize_output {
                return None;
            }
            return Path::new(self.map_path.trim())
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| p.to_path_buf());
        }
        let root = PathBuf::from(self.output_dir.trim());
        if self.organize_output {
            Some(root.join(sanitize_folder(&self.effective_name())))
        } else {
            Some(root)
        }
    }

    /// Where the tools actually run. Everything they scatter around lands here.
    pub fn work_dir(&self) -> Option<PathBuf> {
        let base = self.output_base()?;
        if self.organize_output {
            Some(base.join(WORK_SUBDIR))
        } else {
            Some(base)
        }
    }

    /// True when the compile happens on a copy in the output folder rather than
    /// beside the source map.
    pub fn uses_output_dir(&self) -> bool {
        !self.output_dir.trim().is_empty()
    }

    /// Whether CSG will get a generated WAD list. Only needs the switch: the
    /// map's own entries are resolved even without a WAD folder.
    pub fn will_resolve_wads(&self) -> bool {
        self.auto_wads && self.run_csg
    }

    /// Problems worth warning about before a compile starts.
    pub fn warnings(&self) -> Vec<String> {
        let mut w = Vec::new();

        if self.subdivide > 240 {
            w.push(format!(
                "-subdivide {} pasa el techo de 240. El mapa no cargará en el software \
                 renderer ni en el HLDS.",
                self.subdivide
            ));
        }
        if self.vis_quality == VisQuality::Fast {
            w.push(
                "VIS en 'fast': el PVS queda de más y los FPS serán peores que en la \
                 versión real. No publiques así."
                    .to_string(),
            );
        }
        if self.rad_fast {
            w.push(
                "RAD en 'fast': la iluminación es un borrador. No juzgues la luz del \
                 mapa con esto."
                    .to_string(),
            );
        }
        if !self.pre25 {
            w.push(
                "-pre25 desactivado: las zonas más brillantes se verán rotas en \
                 clientes anteriores al 25 aniversario, que son la mayoría."
                    .to_string(),
            );
        }
        if self.leakonly {
            w.push(
                "'leakonly' corta BSP en cuanto termina de buscar leaks: no obtendrás \
                 un mapa jugable."
                    .to_string(),
            );
        }
        if self.notjunc || self.noclip {
            w.push(
                "'notjunc' y 'noclip' están pensadas para pruebas rápidas, no para un \
                 compilado final."
                    .to_string(),
            );
        }
        if self.nodeterministic {
            w.push(
                "Sin determinismo, dos compilados del mismo mapa pueden dar BSPs \
                 distintos. Ahorra ~0.1% de tiempo; casi nunca compensa."
                    .to_string(),
            );
        }
        if self.skylevel >= 8 {
            w.push(
                "-skylevel 8 son 65.538 rayos de cielo por muestra. Es enormemente más \
                 lento y la diferencia con 6 no se ve."
                    .to_string(),
            );
        }
        if !self.auto_wads {
            w.push(
                "WADs automáticos desactivado: CSG va a usar las rutas guardadas en el \
                 .map. Si el mapa viene de otra PC o de otro disco, va a fallar con \
                 'Could not open wad file'."
                    .to_string(),
            );
        }
        w
    }
}

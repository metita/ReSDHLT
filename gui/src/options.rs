//! Compile options, what each one does, and the presets.
//!
//! The advice here is not folklore. Where a number is quoted it was measured on
//! real CS 1.6 maps with the tools in this repository; see docs/BENCHMARKS.md
//! for the raw figures and docs/FPS_Y_TOOL_TEXTURES.md for the FPS side.

use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    // ---- paths ----
    pub map_path: String,
    pub tools_dir: String,
    /// Empty = compile next to the .map.
    pub output_dir: String,
    /// Folder holding the .wad files this map uses.
    pub wad_dir: String,
    /// Rewrite the worldspawn "wad" key of the copied map to point at wad_dir.
    pub rewrite_wad_key: bool,

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
}

impl Default for Options {
    fn default() -> Self {
        // These defaults ARE the recommended preset.
        Self {
            map_path: String::new(),
            tools_dir: String::new(),
            output_dir: String::new(),
            wad_dir: String::new(),
            rewrite_wad_key: true,

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

    /// Applies the preset, preserving paths and thread count.
    pub fn apply(self, o: &mut Options) {
        let map = o.map_path.clone();
        let tools = o.tools_dir.clone();
        let out = o.output_dir.clone();
        let wad = o.wad_dir.clone();
        let rewrite = o.rewrite_wad_key;
        let threads = o.threads;

        *o = Options::default();
        o.map_path = map;
        o.tools_dir = tools;
        o.output_dir = out;
        o.wad_dir = wad;
        o.rewrite_wad_key = rewrite;
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

    /// True when the compile happens on a copy in the output folder rather than
    /// beside the source map.
    pub fn uses_output_dir(&self) -> bool {
        !self.output_dir.trim().is_empty()
    }

    /// Whether the map's WAD list will actually be rewritten.
    pub fn will_rewrite_wads(&self) -> bool {
        self.rewrite_wad_key
            && !self.wad_dir.trim().is_empty()
            && self.uses_output_dir()
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
        if self.rewrite_wad_key
            && !self.wad_dir.trim().is_empty()
            && !self.uses_output_dir()
        {
            w.push(
                "Para reescribir la lista de WADs hace falta una carpeta de salida: el \
                 .map original nunca se modifica, se compila una copia."
                    .to_string(),
            );
        }
        w
    }
}

// Hide the console window on Windows release builds. Debug builds keep it so
// panics are visible.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod options;
mod runner;

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui;
use egui::{Color32, RichText};

use options::{always_rules, Options, Preset, VisMatrix, VisQuality};
use runner::{Job, LineKind, Msg, Stage, STAGES};

// ---------------------------------------------------------------- palette

const BG: Color32 = Color32::from_rgb(0x16, 0x18, 0x1D);
const PANEL: Color32 = Color32::from_rgb(0x1D, 0x20, 0x26);
const CARD: Color32 = Color32::from_rgb(0x24, 0x28, 0x30);
const TEXT: Color32 = Color32::from_rgb(0xE2, 0xE5, 0xEA);
const MUTED: Color32 = Color32::from_rgb(0x95, 0x9C, 0xA8);
const ACCENT: Color32 = Color32::from_rgb(0x62, 0xA8, 0xFF);
const OK: Color32 = Color32::from_rgb(0x63, 0xC9, 0x8A);
const WARN: Color32 = Color32::from_rgb(0xE8, 0xB3, 0x39);
const ERR: Color32 = Color32::from_rgb(0xE8, 0x6B, 0x6B);

fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = PANEL;
    visuals.window_fill = BG;
    visuals.extreme_bg_color = BG;
    visuals.faint_bg_color = CARD;
    visuals.override_text_color = Some(TEXT);
    visuals.widgets.noninteractive.bg_fill = CARD;
    visuals.widgets.inactive.bg_fill = CARD;
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(0x2E, 0x34, 0x3E);
    visuals.widgets.active.bg_fill = Color32::from_rgb(0x35, 0x3C, 0x48);
    visuals.selection.bg_fill = ACCENT.linear_multiply(0.35);
    visuals.hyperlink_color = ACCENT;
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    ctx.set_style(style);
}

// ---------------------------------------------------------------- tabs

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab {
    Compile,
    Csg,
    Bsp,
    Vis,
    Rad,
    Advice,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Compile => "Compilar",
            Tab::Csg => "CSG",
            Tab::Bsp => "BSP",
            Tab::Vis => "VIS",
            Tab::Rad => "RAD",
            Tab::Advice => "Qué usar siempre",
        }
    }
}

// ---------------------------------------------------------------- app

struct StageState {
    secs: f64,
    ok: bool,
}

struct App {
    opts: Options,
    tab: Tab,
    job: Option<Job>,
    log: Vec<(LineKind, String)>,
    running_stage: Option<Stage>,
    done: HashMap<&'static str, StageState>,
    total_secs: Option<f64>,
    last_ok: Option<bool>,
    status: String,
}

impl Default for App {
    fn default() -> Self {
        Self {
            opts: load_profile().unwrap_or_default(),
            tab: Tab::Compile,
            job: None,
            log: Vec::new(),
            running_stage: None,
            done: HashMap::new(),
            total_secs: None,
            last_ok: None,
            status: String::from("Elige un .map y la carpeta de herramientas."),
        }
    }
}

fn profile_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("resdhlt-gui.json")))
}

fn load_profile() -> Option<Options> {
    let path = profile_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_profile(opts: &Options) -> Result<(), String> {
    let path = profile_path().ok_or("no pude determinar dónde guardar")?;
    let text = serde_json::to_string_pretty(opts).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- widgets

/// A labelled row with a help tooltip on both the label and the control.
fn row<R>(
    ui: &mut egui::Ui,
    label: &str,
    help: &str,
    recommended: Option<&str>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let mut result = None;
    ui.horizontal(|ui| {
        ui.set_min_height(22.0);
        let text = ui.add_sized(
            [190.0, 20.0],
            egui::Label::new(RichText::new(label).color(TEXT)),
        );
        let _ = text.on_hover_text(help);
        result = Some(add(ui));
        if let Some(rec) = recommended {
            ui.label(RichText::new(rec).color(MUTED).small())
                .on_hover_text(help);
        }
    });
    result.unwrap()
}

fn section(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).color(ACCENT).strong().size(15.0));
    if !subtitle.is_empty() {
        ui.label(RichText::new(subtitle).color(MUTED).small());
    }
    ui.add_space(2.0);
    ui.separator();
}

fn pick_file(filter_name: &str, ext: &str) -> Option<String> {
    rfd::FileDialog::new()
        .add_filter(filter_name, &[ext])
        .pick_file()
        .map(|p| p.display().to_string())
}

fn pick_dir() -> Option<String> {
    rfd::FileDialog::new()
        .pick_folder()
        .map(|p| p.display().to_string())
}

// ---------------------------------------------------------------- impl

impl App {
    fn drain_messages(&mut self) {
        let mut finished = false;
        if let Some(job) = &self.job {
            while let Ok(msg) = job.rx.try_recv() {
                match msg {
                    Msg::Line(kind, text) => {
                        self.log.push((kind, text));
                        // Keep the log bounded; RAD can emit a lot.
                        if self.log.len() > 6000 {
                            self.log.drain(0..2000);
                        }
                    }
                    Msg::StageStart(s) => {
                        self.running_stage = Some(s);
                        self.status = format!("{}...", s.name());
                    }
                    Msg::StageDone(s, secs, ok) => {
                        self.done.insert(s.name(), StageState { secs, ok });
                        self.running_stage = None;
                    }
                    Msg::Finished(total, ok) => {
                        self.total_secs = Some(total);
                        self.last_ok = Some(ok);
                        self.running_stage = None;
                        self.status = if ok {
                            format!("Listo en {:.1}s", total)
                        } else {
                            "Terminó con errores".to_string()
                        };
                        finished = true;
                    }
                }
            }
        }
        if finished {
            self.job = None;
        }
    }

    fn can_run(&self) -> bool {
        self.job.is_none()
            && !self.opts.map_path.trim().is_empty()
            && !self.opts.tools_dir.trim().is_empty()
    }

    fn start(&mut self) {
        // Persist before every run, so a crash mid-compile cannot lose settings.
        let _ = save_profile(&self.opts);
        self.log.clear();
        self.done.clear();
        self.total_secs = None;
        self.last_ok = None;
        self.job = Some(runner::start(self.opts.clone()));
    }

    // ---------------- tabs ----------------

    fn ui_compile(&mut self, ui: &mut egui::Ui) {
        section(ui, "Archivos", "Qué compilar y con qué herramientas");

        row(
            ui,
            "Mapa (.map)",
            "El archivo fuente que exporta tu editor (J.A.C.K., Hammer). \
             Los .bsp, .prt y logs se generan al lado de este archivo.",
            None,
            |ui| {
                ui.text_edit_singleline(&mut self.opts.map_path);
                if ui.button("Buscar").clicked() {
                    if let Some(p) = pick_file("Mapa", "map") {
                        self.opts.map_path = p;
                    }
                }
            },
        );

        row(
            ui,
            "Herramientas",
            "La carpeta con sdHLCSG, sdHLBSP, sdHLVIS y sdHLRAD. Si compilaste \
             este repo, es la carpeta 'tools'.",
            None,
            |ui| {
                ui.text_edit_singleline(&mut self.opts.tools_dir);
                if ui.button("Buscar").clicked() {
                    if let Some(p) = pick_dir() {
                        self.opts.tools_dir = p;
                    }
                }
            },
        );

        row(
            ui,
            "Carpeta de salida",
            "Dónde queda el .bsp. Si la dejas vacía, todo se genera junto al .map, \
             mezclado con tus archivos fuente.\n\n\
             Si la indicas, se copia el .map ahí y se compila esa copia: el .bsp, el \
             .prt, los logs y los intermedios (.p0 a .p3) quedan todos en esa carpeta \
             y tu carpeta de trabajo no se ensucia. El .map original nunca se toca.",
            Some("recomendado"),
            |ui| {
                ui.text_edit_singleline(&mut self.opts.output_dir);
                if ui.button("Buscar").clicked() {
                    if let Some(p) = pick_dir() {
                        self.opts.output_dir = p;
                    }
                }
                if ui.button("Limpiar").clicked() {
                    self.opts.output_dir.clear();
                }
            },
        );

        row(
            ui,
            "Carpeta de WADs",
            "Dónde tienes tus .wad. Sirve para dos cosas:\n\n\
             1) Se le pasa a RAD como -waddir para que encuentre las texturas al \
             calcular la luz.\n\
             2) Si activas la casilla de abajo, la lista de WADs del mapa se reescribe \
             para apuntar a esta carpeta.",
            None,
            |ui| {
                ui.text_edit_singleline(&mut self.opts.wad_dir);
                if ui.button("Buscar").clicked() {
                    if let Some(p) = pick_dir() {
                        self.opts.wad_dir = p;
                    }
                }
            },
        );

        row(
            ui,
            "Reescribir lista de WADs",
            "Un .map guarda las rutas absolutas de los WADs de la máquina donde se \
             hizo. Si el mapa es de otra persona, esas rutas no existen aquí y CSG no \
             encuentra las texturas.\n\n\
             Con esto activado se reescribe esa lista en la COPIA, apuntando a todos \
             los .wad de tu carpeta de WADs más sdhlt.wad. Tu .map original queda \
             intacto.\n\n\
             Necesita una carpeta de salida, porque no se modifica el archivo fuente. \
             CSG no tiene un equivalente a -waddir: solo lee esta clave del mapa.",
            Some("si el mapa no es tuyo"),
            |ui| ui.checkbox(&mut self.opts.rewrite_wad_key, ""),
        );

        ui.add_space(8.0);
        section(
            ui,
            "Preset",
            "Un punto de partida. Después puedes ajustar cualquier pestaña.",
        );
        ui.horizontal(|ui| {
            for p in [Preset::Draft, Preset::Recommended, Preset::Release] {
                if ui
                    .button(p.label())
                    .on_hover_text(p.summary())
                    .clicked()
                {
                    p.apply(&mut self.opts);
                    self.status = format!("Preset '{}' aplicado.", p.label());
                }
            }
        });
        ui.label(
            RichText::new(Preset::Recommended.summary())
                .color(MUTED)
                .small(),
        );

        ui.add_space(8.0);
        section(ui, "Etapas", "Puedes saltar etapas si ya las ejecutaste");
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.opts.run_csg, "CSG")
                .on_hover_text(Stage::Csg.purpose());
            ui.checkbox(&mut self.opts.run_bsp, "BSP")
                .on_hover_text(Stage::Bsp.purpose());
            ui.checkbox(&mut self.opts.run_vis, "VIS")
                .on_hover_text(Stage::Vis.purpose());
            ui.checkbox(&mut self.opts.run_rad, "RAD")
                .on_hover_text(Stage::Rad.purpose());
        });

        row(
            ui,
            "Hilos",
            "0 = usar todos los núcleos, que es lo que quieres. Este fork arregló \
             dos bugs por los que las tools corrían en un solo hilo. Pon un número \
             solo si quieres dejar CPU libre para otra cosa.",
            Some("0 = automático"),
            |ui| {
                ui.add(egui::Slider::new(&mut self.opts.threads, 0..=64));
            },
        );

        row(
            ui,
            "Prioridad baja",
            "Corre las herramientas con prioridad baja para que la PC siga usable \
             mientras compila. Alarga un poco la compilación.",
            None,
            |ui| ui.checkbox(&mut self.opts.low_priority, ""),
        );

        row(
            ui,
            "Chart de límites",
            "Al final, cada herramienta imprime cuánto de cada límite del motor \
             estás usando. Es como te enteras de que vas camino a un límite antes \
             de chocarlo. Déjalo activado.",
            Some("recomendado"),
            |ui| ui.checkbox(&mut self.opts.chart, ""),
        );

        row(
            ui,
            "Salida detallada",
            "Las herramientas imprimen mucho más sobre lo que hacen. Útil cuando \
             algo falla y no sabes por qué; para uso normal solo ensucia el log.",
            None,
            |ui| ui.checkbox(&mut self.opts.verbose, ""),
        );

        let warnings = self.opts.warnings();
        if !warnings.is_empty() {
            ui.add_space(8.0);
            section(ui, "Avisos", "Cosas que probablemente no quieras");
            for w in &warnings {
                ui.label(RichText::new(format!("• {w}")).color(WARN));
            }
        }
    }

    fn ui_csg(&mut self, ui: &mut egui::Ui) {
        section(
            ui,
            "CSG",
            "Recorta los brushes entre sí y resuelve texturas y WADs. Rápido: \
             centésimas de segundo.",
        );

        row(
            ui,
            "Embeder texturas",
            "Copia dentro del .bsp las texturas que el mapa usa realmente (no el WAD \
             entero, solo las que aparecen).\n\n\
             QUÉ CAMBIA: el jugador ya no necesita tener el WAD. Se acabó el 'missing \
             texture' al entrar al servidor y no hay que distribuir archivos aparte. \
             A cambio el .bsp crece, típicamente entre unos cientos de KB y unos MB \
             según cuántas texturas propias uses.\n\n\
             Activado por defecto porque es lo que evita problemas a los jugadores. \
             Desactívalo solo si distribuyes el WAD por tu cuenta y te importa el \
             tamaño del .bsp.",
            Some("activado"),
            |ui| ui.checkbox(&mut self.opts.nowadtextures, ""),
        );

        row(
            ui,
            "Sin determinismo",
            "Por defecto este fork compila de forma reproducible: el mismo .map da \
             siempre el mismo .bsp, byte por byte.\n\n\
             QUÉ CAMBIA: activarlo devuelve el comportamiento original, donde dos \
             compilaciones del mismo mapa pueden diferir en unos pocos clipnodes y \
             marksurfaces, porque CSG numera los planos según qué hilo gana la \
             carrera. Ahorra ~0.1% del tiempo total.\n\n\
             El determinismo importa cuando persigues un bug: si el mapa sale con una \
             grieta rara, conviene que vuelva a salir igual en la siguiente \
             compilación en vez de desaparecer sola.",
            Some("déjalo apagado"),
            |ui| ui.checkbox(&mut self.opts.nodeterministic, ""),
        );

        row(
            ui,
            "Extender el mundo",
            "Sube el límite de geometría más allí de +/-32768 unidades. Solo si tu \
             mapa realmente excede el tamaño estándar; 0 deja el valor por defecto.",
            Some("0 = normal"),
            |ui| {
                ui.add(egui::Slider::new(&mut self.opts.worldextent, 0..=65536).step_by(1024.0));
            },
        );

        self.ui_extra(ui, "CSG");
    }

    fn ui_bsp(&mut self, ui: &mut egui::Ui) {
        section(
            ui,
            "BSP",
            "Construye el árbol, fusiona caras coplanares y subdivide las grandes.",
        );

        row(
            ui,
            "Subdivide",
            "Tamaño máximo de una cara antes de partirla. 240 NO es un número \
             conservador: es el techo que impone MAX_SURFACE_EXTENT. Si lo subes, el \
             mapa deja de cargar en el software renderer y en el HLDS. Bajarlo genera \
             más caras y baja los FPS. Déjalo en 240.",
            Some("240, no lo toques"),
            |ui| {
                ui.add(egui::Slider::new(&mut self.opts.subdivide, 64..=512));
            },
        );

        row(
            ui,
            "Max node size",
            "Tamaño máximo, en unidades, de un nodo del árbol BSP antes de partirlo.\n\n\
             QUÉ CAMBIA: más chico produce más nodos y hojas, lo que da un PVS más \
             fino (VIS puede descartar mejor) pero también más caras, un BSP más \
             grande y VIS más lento. Más grande hace lo contrario: compila antes y el \
             PVS queda más grueso.\n\n\
             1024 es el equilibrio probado. Bajarlo a 512 a veces ayuda en mapas muy \
             abiertos, pero mídelo con r_speeds antes de darlo por bueno: es igual de \
             fácil empeorar los FPS.",
            Some("1024"),
            |ui| {
                ui.add(egui::Slider::new(&mut self.opts.maxnodesize, 64..=4096));
            },
        );

        ui.add_space(6.0);
        section(ui, "Atajos de prueba", "No los uses para un compilado final");

        row(
            ui,
            "Solo buscar leaks",
            "Corta BSP en cuanto termina de buscar leaks. Sirve para saber en segundos \
             si el mapa está sellado, pero no produce un mapa jugable.",
            None,
            |ui| ui.checkbox(&mut self.opts.leakonly, ""),
        );

        row(
            ui,
            "Sin t-junctions",
            "Saltea el arreglo de t-junctions. Más rápido, pero vas a ver grietas de \
             luz entre caras. Solo para pruebas.",
            None,
            |ui| ui.checkbox(&mut self.opts.notjunc, ""),
        );

        row(
            ui,
            "Sin clipping hull",
            "No genera la geometría de colisión. El mapa carga pero el jugador \
             atraviesa las paredes. Solo para mirar geometría.",
            None,
            |ui| ui.checkbox(&mut self.opts.noclip, ""),
        );

        self.ui_extra(ui, "BSP");
    }

    fn ui_vis(&mut self, ui: &mut egui::Ui) {
        section(
            ui,
            "VIS",
            "Calcula qué se ve desde dónde. Es la etapa que define los FPS del mapa.",
        );

        row(
            ui,
            "Calidad",
            "El PVS que calcula VIS decide cuánto le pide el mapa al motor en cada \
             frame. Cuanto más ajustado, menos wpoly y más FPS.",
            Some("full para publicar"),
            |ui| {
                egui::ComboBox::from_id_source("vis_q")
                    .selected_text(self.opts.vis_quality.label())
                    .show_ui(ui, |ui| {
                        for q in [VisQuality::Fast, VisQuality::Normal, VisQuality::Full] {
                            ui.selectable_value(&mut self.opts.vis_quality, q, q.label())
                                .on_hover_text(q.help());
                        }
                    });
            },
        );
        ui.label(
            RichText::new(self.opts.vis_quality.help())
                .color(MUTED)
                .small(),
        );

        row(
            ui,
            "Distancia máxima",
            "Recorta la visibilidad más allí de N unidades. Puede ayudar en mapas \
             enormes y abiertos, pero mal usado produce geometría que aparece de \
             golpe. 0 lo desactiva.",
            Some("0 = sin límite"),
            |ui| {
                ui.add(egui::Slider::new(&mut self.opts.maxdistance, 0..=8192).step_by(64.0));
            },
        );

        self.ui_extra(ui, "VIS");
    }

    fn ui_rad(&mut self, ui: &mut egui::Ui) {
        section(
            ui,
            "RAD",
            "Calcula la iluminación. Es ~95% del tiempo de compilación, así que casi \
             todo lo que ajustes aquí se nota en el reloj.",
        );

        ui.label(
            RichText::new(
                "RAD no cambia la geometría que dibuja el motor: subir la calidad de \
                 luz te cuesta tiempo de compilación, no FPS en juego.",
            )
            .color(OK)
            .small(),
        );
        ui.add_space(4.0);

        row(
            ui,
            "Nivel de cielo",
            "Cuántos rayos lanza cada muestra hacia el cielo: nivel 4 = 258 rayos, \
             5 = 1026, 6 = 4098, 7 = 16386. Medido, este loop es el 96% de todos los \
             rayos de RAD. El nivel 6 usa 4x menos rayos que el 7 con una diferencia \
             máxima de 1/255 por luxel, o sea invisible. Por eso el default de este \
             fork es 6 y no 7.",
            Some("6"),
            |ui| {
                ui.add(egui::Slider::new(&mut self.opts.skylevel, 4..=8));
            },
        );

        row(
            ui,
            "Cielo suave",
            "Ilumina cada punto con todo el domo del cielo en lugar de una sola \
             dirección.\n\n\
             QUÉ CAMBIA: con esto los bordes de sombra en exteriores son graduales, \
             como en la realidad, y las zonas en sombra reciben algo de luz azulada \
             del cielo. Sin esto el sol es una sola dirección y las sombras quedan \
             duras y de borde nítido, con aspecto de mapa antiguo.\n\n\
             Apagarlo fuerza el nivel de cielo a 4 (258 rayos) y acelera bastante, \
             pero la diferencia visual en exteriores es evidente.",
            Some("activado"),
            |ui| ui.checkbox(&mut self.opts.softsky, ""),
        );

        row(
            ui,
            "Oversampling (-extra)",
            "Toma 9 muestras de luz por cada luxel en vez de 1, y promedia.\n\n\
             QUÉ CAMBIA: es la mejora de calidad más visible de RAD. Sin esto los \
             bordes de sombra salen escalonados, con el típico dentado de lightmap de \
             16 unidades; con esto quedan suaves. También desaparecen los puntos de \
             luz mal calculados en esquinas y superficies inclinadas.\n\n\
             Cuesta tiempo de compilación y CERO FPS en juego: el lightmap resultante \
             tiene exactamente el mismo tamaño.",
            Some("activado"),
            |ui| ui.checkbox(&mut self.opts.extra_sampling, ""),
        );

        row(
            ui,
            "Bounces",
            "Cuántas veces se le permite rebotar a la luz antes de darla por agotada.\n\n\
             QUÉ CAMBIA: con 0 solo hay luz directa, así que todo lo que no ve una \
             lámpara queda negro. Cada rebote reparte luz desde las superficies \
             iluminadas hacia las que no lo están, y además les tiñe el color: un piso \
             rojo iluminado va a teñir de rojo la pared de al lado. Los primeros \
             rebotes cambian mucho; a partir de ~12 el aporte es tan pequeño que \
             prácticamente no se distingue.\n\n\
             Cuesta tiempo, no FPS. 12 es lo que aplica -extra en este fork.",
            Some("12"),
            |ui| {
                ui.add(egui::Slider::new(&mut self.opts.bounce, 0..=32));
            },
        );

        row(
            ui,
            "RAD rápido",
            "Modo borrador de la iluminación.\n\n\
             QUÉ CAMBIA: baja la calidad del muestreo y simplifica el cálculo de \
             rebotes. El resultado sirve para ver que las luces están donde \
             corresponde y que no quedó nada a oscuras, pero los degradados salen \
             sucios y las sombras imprecisas.\n\n\
             Útil mientras construyes; nunca para la versión que publicas.",
            None,
            |ui| ui.checkbox(&mut self.opts.rad_fast, ""),
        );

        ui.add_space(6.0);
        section(ui, "Detalle y suavizado", "");

        row(
            ui,
            "Chop",
            "Tamaño, en unidades, de los parches en que RAD divide las superficies \
             para repartir la luz rebotada.\n\n\
             QUÉ CAMBIA: cada parche emite y recibe luz como una unidad. Más chico da \
             luz indirecta con más detalle (los degradados en paredes grandes dejan de \
             verse por bloques), pero el coste crece rápido: el trabajo va con el \
             cuadrado de la cantidad de parches, así que bajarlo a la mitad puede \
             cuadruplicar el tiempo y la RAM.\n\n\
             64 es el default. Si un muro grande se ve 'por manchones', prueba 48 o 32 \
             antes de tocar nada más.",
            Some("64"),
            |ui| {
                ui.add(egui::Slider::new(&mut self.opts.chop, 16.0..=128.0));
            },
        );

        row(
            ui,
            "Texchop",
            "Lo mismo que chop, pero para las superficies que EMITEN luz: las \
             texlights definidas en lights.rad.\n\n\
             QUÉ CAMBIA: controla con cuánto detalle se reparte la luz que sale de un \
             cartel luminoso o un tubo fluorescente. Más chico hace que la forma de la \
             fuente se note en la luz que proyecta, en vez de comportarse como una \
             mancha difusa.\n\n\
             Es más bajo que chop (32 contra 64) porque las texlights suelen ser \
             pequeñas y su forma sí importa.",
            Some("32"),
            |ui| {
                ui.add(egui::Slider::new(&mut self.opts.texchop, 8.0..=128.0));
            },
        );

        row(
            ui,
            "Smooth",
            "Ángulo máximo, en grados, para que dos caras vecinas se iluminen como si \
             fueran una superficie curva continua.\n\n\
             QUÉ CAMBIA: si el ángulo entre dos caras es menor que este valor, RAD \
             mezcla sus normales y la luz cruza de una a otra sin corte. Por encima, \
             deja un borde marcado.\n\n\
             Subirlo suaviza terreno y arcos hechos con muchas caras. Pasarse hace que \
             esquinas que deberían tener una arista clara se vean redondeadas y \
             blandas. 50 grados funciona para casi todo; 70 para terreno muy \
             facetado.",
            Some("50"),
            |ui| {
                ui.add(egui::Slider::new(&mut self.opts.smooth, 0.0..=180.0));
            },
        );

        ui.add_space(6.0);
        section(ui, "Avanzado", "");

        row(
            ui,
            "Vismatrix",
            "Cómo guarda RAD la visibilidad entre parches. Es una decisión de \
             memoria, no de calidad.",
            Some("sparse"),
            |ui| {
                egui::ComboBox::from_id_source("vismat")
                    .selected_text(self.opts.vismatrix.label())
                    .show_ui(ui, |ui| {
                        for m in [VisMatrix::Normal, VisMatrix::Sparse, VisMatrix::Off] {
                            ui.selectable_value(&mut self.opts.vismatrix, m, m.label())
                                .on_hover_text(m.help());
                        }
                    });
            },
        );

        row(
            ui,
            "Motor pre-25 aniversario",
            "Baja el umbral de recorte de luz de 255 a 188.\n\n\
             QUÉ CAMBIA: el motor anterior a la actualización del 25 aniversario no \
             maneja valores de luz por encima de ~188. Si compilas sin esto y alguien \
             juega en un cliente antiguo, las zonas más brillantes se ven rotas: \
             quemadas o con el color dado vuelta.\n\n\
             Al revés el error es mucho menor: un mapa compilado con -pre25 visto en \
             el cliente nuevo solo se ve un poco menos brillante en los puntos más \
             claros.\n\n\
             Como en la práctica casi nadie usa el cliente del 25 aniversario, y el \
             error es asimétrico, viene ACTIVADO por defecto. Desactívalo solo si \
             sabes que todos tus jugadores están actualizados.",
            Some("activado, casi siempre"),
            |ui| ui.checkbox(&mut self.opts.pre25, ""),
        );

        row(
            ui,
            "Ignorar sombras de modelos",
            "Hace que RAD ignore la clave zhlt_studioshadow de las entidades.\n\n\
             QUÉ CAMBIA: esa clave permite que un modelo (un árbol, una reja, una \
             estatua) proyecte sombra real sobre el mapa, trazando su malla \
             triángulo a triángulo. Con esta opción los modelos dejan de proyectar \
             sombra: la luz los atraviesa como si no existieran.\n\n\
             Solo importa si tu mapa usa esa clave. Cuando la usa, trazar la malla es \
             caro, así que activarlo acelera bastante los compilados de prueba. Para \
             la versión final déjalo apagado.",
            None,
            |ui| ui.checkbox(&mut self.opts.nostudioshadow, ""),
        );

        row(
            ui,
            "Perfilar RAD",
            "Imprime al final en qué gasta RAD el tiempo, con conteos de llamadas. \
             Sirve para entender un compilado lento, no para uso normal.",
            None,
            |ui| ui.checkbox(&mut self.opts.profile, ""),
        );

        self.ui_extra(ui, "RAD");
    }

    fn ui_extra(&mut self, ui: &mut egui::Ui, which: &str) {
        ui.add_space(6.0);
        let field = match which {
            "CSG" => &mut self.opts.csg_extra,
            "BSP" => &mut self.opts.bsp_extra,
            "VIS" => &mut self.opts.vis_extra,
            _ => &mut self.opts.rad_extra,
        };
        ui.label(
            RichText::new(format!("Parámetros extra para {which}"))
                .color(MUTED)
                .small(),
        );
        ui.add(
            egui::TextEdit::singleline(field)
                .hint_text("se pasan tal cual, separados por espacios")
                .desired_width(f32::INFINITY),
        )
        .on_hover_text(
            "Para flags que no están en la interfaz. Se agregan al final de la línea \
             de comandos sin validar.",
        );
    }

    fn ui_advice(&mut self, ui: &mut egui::Ui) {
        section(
            ui,
            "Lo que deberías hacer siempre",
            "Cada punto está respaldado por mediciones sobre mapas reales; los números \
             están en docs/BENCHMARKS.md.",
        );
        for rule in always_rules() {
            ui.add_space(6.0);
            egui::Frame::none()
                .fill(CARD)
                .inner_margin(egui::Margin::same(10.0))
                .rounding(6.0)
                .show(ui, |ui| {
                    ui.label(RichText::new(rule.title).color(ACCENT).strong());
                    ui.label(RichText::new(rule.body).color(TEXT).small());
                });
        }
    }

    // ---------------- log panel ----------------

    fn ui_log(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Salida").color(ACCENT).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Limpiar").clicked() {
                    self.log.clear();
                }
                if ui.button("Copiar").clicked() {
                    let all: String = self
                        .log
                        .iter()
                        .map(|(_, t)| t.as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    ui.output_mut(|o| o.copied_text = all);
                }
            });
        });
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if self.log.is_empty() {
                    ui.label(
                        RichText::new("La salida de las herramientas aparece aquí.")
                            .color(MUTED)
                            .small(),
                    );
                }
                for (kind, text) in &self.log {
                    let color = match kind {
                        LineKind::Normal => TEXT,
                        LineKind::Command => MUTED,
                        LineKind::Warning => WARN,
                        LineKind::Error => ERR,
                        LineKind::Success => OK,
                    };
                    ui.label(RichText::new(text).color(color).monospace().size(11.5));
                }
            });
    }

    fn ui_progress(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for stage in STAGES.iter() {
                let name = stage.name();
                let (txt, color) = if let Some(st) = self.done.get(name) {
                    (
                        format!("{name} {:.2}s", st.secs),
                        if st.ok { OK } else { ERR },
                    )
                } else if self.running_stage == Some(*stage) {
                    (format!("{name} ..."), ACCENT)
                } else {
                    (name.to_string(), MUTED)
                };
                ui.label(RichText::new(txt).color(color).monospace())
                    .on_hover_text(stage.purpose());
                ui.add_space(6.0);
            }
            if let Some(total) = self.total_secs {
                ui.label(
                    RichText::new(format!("total {:.1}s", total))
                        .color(TEXT)
                        .strong(),
                );
            }
        });
    }
}

impl eframe::App for App {
    /// eframe calls this when the window closes. Preferences are saved here as
    /// well as on every compile, so the button is only a manual extra.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = save_profile(&self.opts);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_messages();
        if self.job.is_some() {
            // Keep painting while output streams in.
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("ReSDHLT")
                        .color(ACCENT)
                        .strong()
                        .size(19.0),
                );
                ui.label(
                    RichText::new("compilador de mapas para CS 1.6")
                        .color(MUTED)
                        .small(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.job.is_some() {
                        if ui.button("Cancelar").clicked() {
                            if let Some(j) = &self.job {
                                j.cancel();
                            }
                            self.status = "Cancelando...".to_string();
                        }
                        ui.spinner();
                    } else {
                        let enabled = self.can_run();
                        let btn = ui.add_enabled(
                            enabled,
                            egui::Button::new(RichText::new("Compilar").strong())
                                .fill(ACCENT.linear_multiply(0.55)),
                        );
                        if btn.clicked() {
                            self.start();
                        }
                        if !enabled {
                            btn.on_hover_text(
                                "Falta el .map o la carpeta de herramientas.",
                            );
                        }
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                for t in [
                    Tab::Compile,
                    Tab::Csg,
                    Tab::Bsp,
                    Tab::Vis,
                    Tab::Rad,
                    Tab::Advice,
                ] {
                    let selected = self.tab == t;
                    let text = if selected {
                        RichText::new(t.label()).color(ACCENT).strong()
                    } else {
                        RichText::new(t.label()).color(MUTED)
                    };
                    if ui.selectable_label(selected, text).clicked() {
                        self.tab = t;
                    }
                }
            });
            ui.add_space(6.0);
        });

        egui::TopBottomPanel::bottom("bottom").show(ctx, |ui| {
            ui.add_space(4.0);
            self.ui_progress(ui);
            ui.horizontal(|ui| {
                let color = match self.last_ok {
                    Some(true) => OK,
                    Some(false) => ERR,
                    None => MUTED,
                };
                ui.label(RichText::new(&self.status).color(color).small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let btn = ui.small_button("Guardar ahora");
                    if btn.clicked() {
                        self.status = match save_profile(&self.opts) {
                            Ok(()) => "Preferencias guardadas.".to_string(),
                            Err(e) => format!("No pude guardar: {e}"),
                        };
                    }
                    let _ = btn.on_hover_text(
                        "Las preferencias se guardan solas al cerrar la ventana y al \
                         empezar una compilación. Este botón solo fuerza el guardado \
                         ahora.",
                    );
                });
            });
            ui.add_space(4.0);
        });

        egui::SidePanel::right("log")
            .resizable(true)
            .default_width(520.0)
            .min_width(300.0)
            .show(ctx, |ui| {
                self.ui_log(ui);
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match self.tab {
                    Tab::Compile => self.ui_compile(ui),
                    Tab::Csg => self.ui_csg(ui),
                    Tab::Bsp => self.ui_bsp(ui),
                    Tab::Vis => self.ui_vis(ui),
                    Tab::Rad => self.ui_rad(ui),
                    Tab::Advice => self.ui_advice(ui),
                });
        });
    }
}

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([900.0, 560.0])
            .with_title("ReSDHLT - compilador de mapas"),
        ..Default::default()
    };

    eframe::run_native(
        "ReSDHLT",
        native_options,
        // eframe 0.28's app creator returns a Result, so the boxed App has to be
        // wrapped in Ok. Older releases returned the Box directly.
        Box::new(|cc| {
            apply_theme(&cc.egui_ctx);
            Ok(Box::new(App::default()))
        }),
    )
}

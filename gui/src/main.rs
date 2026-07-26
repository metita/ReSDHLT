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
            status: String::from("Elegí un .map y la carpeta de herramientas."),
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

        ui.add_space(8.0);
        section(
            ui,
            "Preset",
            "Un punto de partida. Después podés ajustar cualquier pestaña.",
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
        section(ui, "Etapas", "Podés saltear etapas si ya las corriste");
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
            "0 = usar todos los núcleos, que es lo que querés. Este fork arregló \
             dos bugs por los que las tools corrían en un solo hilo. Poné un número \
             solo si querés dejar CPU libre para otra cosa.",
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
             estás usando. Es como te enterás de que vas camino a un límite antes \
             de chocarlo. Dejalo activado.",
            Some("recomendado"),
            |ui| ui.checkbox(&mut self.opts.chart, ""),
        );

        row(
            ui,
            "Salida detallada",
            "Las herramientas imprimen mucho más sobre lo que hacen. Útil cuando \
             algo falla y no sabés por qué; para uso normal solo ensucia el log.",
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
            "Copia dentro del .bsp todas las texturas que usa el mapa. El mapa \
             funciona sin que el jugador tenga el WAD, a cambio de un .bsp mucho más \
             grande. Para mapas de servidor suele ser lo que querés; para un mapa \
             con WAD propio distribuido aparte, no.",
            None,
            |ui| ui.checkbox(&mut self.opts.nowadtextures, ""),
        );

        row(
            ui,
            "Sin determinismo",
            "Por defecto este fork compila de forma reproducible: el mismo .map da \
             siempre el mismo .bsp. Activar esto recupera el comportamiento viejo y \
             ahorra ~0.1% de tiempo, a cambio de que dos compilados difieran. \
             Sirve de poco.",
            Some("dejalo apagado"),
            |ui| ui.checkbox(&mut self.opts.nodeterministic, ""),
        );

        row(
            ui,
            "Extender el mundo",
            "Sube el límite de geometría más allá de +/-32768 unidades. Solo si tu \
             mapa realmente excede el tamaño estándar; 0 deja el valor por defecto.",
            Some("0 = normal"),
            |ui| {
                ui.add(egui::Slider::new(&mut self.opts.worldextent, 0..=65536).step_by(1024.0));
            },
        );

        ui.add_space(6.0);
        section(ui, "WADs a embeder", "-wadinclude, uno por línea de texto");
        ui.horizontal(|ui| {
            if ui.button("Agregar WAD").clicked() {
                if let Some(p) = pick_file("WAD", "wad") {
                    self.opts.wad_dirs.push(p);
                }
            }
            if ui.button("Limpiar").clicked() {
                self.opts.wad_dirs.clear();
            }
        });
        let mut remove = None;
        for (i, w) in self.opts.wad_dirs.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(w).color(MUTED).small());
                if ui.small_button("quitar").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.opts.wad_dirs.remove(i);
        }

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
             conservador: es el techo que impone MAX_SURFACE_EXTENT. Si lo subís, el \
             mapa deja de cargar en el software renderer y en el HLDS. Bajarlo genera \
             más caras y baja los FPS. Dejalo en 240.",
            Some("240, no lo toques"),
            |ui| {
                ui.add(egui::Slider::new(&mut self.opts.subdivide, 64..=512));
            },
        );

        row(
            ui,
            "Max node size",
            "Tamaño máximo de un nodo del árbol. Valores más chicos dan más nodos y \
             cortes más finos; más grandes, lo contrario. El default anda bien casi \
             siempre.",
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
            "Recorta la visibilidad más allá de N unidades. Puede ayudar en mapas \
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
             todo lo que ajustes acá se nota en el reloj.",
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
            "Ilumina con todo el domo del cielo en vez de una sola dirección. Da \
             sombras exteriores mucho más creíbles. Apagarlo baja el nivel efectivo a \
             4 y acelera, pero se nota.",
            Some("activado"),
            |ui| ui.checkbox(&mut self.opts.softsky, ""),
        );

        row(
            ui,
            "Oversampling (-extra)",
            "Toma 9 muestras por luxel en vez de 1. Es la mejora de calidad más \
             visible que tiene RAD: bordes de sombra suaves en lugar de escalonados. \
             Cuesta tiempo, no FPS.",
            Some("activado"),
            |ui| ui.checkbox(&mut self.opts.extra_sampling, ""),
        );

        row(
            ui,
            "Bounces",
            "Cuántas veces rebota la luz. Más rebotes = interiores menos planos y \
             menos negros. 12 es lo que usa -extra en este fork.",
            Some("12"),
            |ui| {
                ui.add(egui::Slider::new(&mut self.opts.bounce, 0..=32));
            },
        );

        row(
            ui,
            "RAD rápido",
            "Modo borrador: sacrifica calidad por velocidad. Útil mientras construís, \
             nunca para publicar.",
            None,
            |ui| ui.checkbox(&mut self.opts.rad_fast, ""),
        );

        ui.add_space(6.0);
        section(ui, "Detalle y suavizado", "");

        row(
            ui,
            "Chop",
            "Tamaño de los parches de radiosidad en texturas normales. Más chico = \
             más detalle en la luz indirecta y bastante más tiempo.",
            Some("64"),
            |ui| {
                ui.add(egui::Slider::new(&mut self.opts.chop, 16.0..=128.0));
            },
        );

        row(
            ui,
            "Texchop",
            "Igual que chop pero para superficies que emiten luz (texlights). Más \
             chico da texlights más definidas.",
            Some("32"),
            |ui| {
                ui.add(egui::Slider::new(&mut self.opts.texchop, 8.0..=128.0));
            },
        );

        row(
            ui,
            "Smooth",
            "Ángulo en grados por debajo del cual dos caras se iluminan como una \
             superficie curva. Alto suaviza terreno; demasiado alto suaviza esquinas \
             que deberían ser filosas.",
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
            "Ajusta el umbral de recorte de luz para el motor viejo de Half-Life. \
             Actívalo solo si tu mapa apunta a clientes anteriores a la actualización \
             del 25 aniversario.",
            None,
            |ui| ui.checkbox(&mut self.opts.pre25, ""),
        );

        row(
            ui,
            "Ignorar sombras de modelos",
            "Ignora zhlt_studioshadow. Los modelos dejan de proyectar sombra y RAD \
             va más rápido.",
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
                        RichText::new("La salida de las herramientas aparece acá.")
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
                    if ui.small_button("Guardar preferencias").clicked() {
                        self.status = match save_profile(&self.opts) {
                            Ok(()) => "Preferencias guardadas.".to_string(),
                            Err(e) => format!("No pude guardar: {e}"),
                        };
                    }
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
        Box::new(|cc| {
            apply_theme(&cc.egui_ctx);
            Box::new(App::default())
        }),
    )
}

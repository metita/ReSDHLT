// Hide the console window on Windows release builds. Debug builds keep it so
// panics are visible.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod analysis;
mod options;
mod projects;
mod runner;
mod theme;
mod update;
mod widgets;

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui;
use egui::{Align, Layout, RichText};
use fs2::FileExt;

use options::{always_rules, AoMode, Options, Preset, VisMatrix, VisQuality};
use projects::{FileEntry, FileKind, Library, Project};
use runner::{CompilePlan, Job, LineKind, Msg, RunOutcome, Stage, STAGES};
use theme::*;
use update::Release;
use widgets::*;

// ---------------------------------------------------------------- tabs

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab {
    Projects,
    Compile,
    Csg,
    Bsp,
    Vis,
    Rad,
    Analysis,
    Advice,
}

impl Tab {
    fn label(self) -> &'static str {
        TABS.iter()
            .find(|(t, _)| *t == self)
            .map(|(_, l)| *l)
            .unwrap_or("")
    }
}

const TABS: [(Tab, &str); 8] = [
    (Tab::Projects, "Proyectos"),
    (Tab::Compile, "Compilar"),
    (Tab::Csg, "CSG"),
    (Tab::Bsp, "BSP"),
    (Tab::Vis, "VIS"),
    (Tab::Rad, "RAD"),
    (Tab::Analysis, "Análisis"),
    (Tab::Advice, "Guía"),
];

// ---------------------------------------------------------------- app

struct StageState {
    secs: f64,
    ok: bool,
}

/// Cached filesystem checks. Doing them per frame would stat four files on
/// every repaint; they only change when the user edits a path.
#[derive(Default)]
struct Checks {
    map_key: String,
    map_ok: bool,
    tools_key: String,
    tools_ok: bool,
    plan_key: String,
    plan_errors: Vec<String>,
    checked_at: Option<Instant>,
    /// The tools folder in use is not the one shipped beside this executable,
    /// and the shipped one is newer. Updating the app does not touch a folder
    /// the user pointed somewhere else, so the compile would silently keep
    /// running the old binaries.
    tools_stale: Option<PathBuf>,
}

struct App {
    opts: Options,
    tab: Tab,
    job: Option<Job>,
    log: Vec<(LineKind, String)>,
    running_stage: Option<Stage>,
    stage_since: Option<Instant>,
    run_since: Option<Instant>,
    done: HashMap<&'static str, StageState>,
    total_secs: Option<f64>,
    last_outcome: Option<RunOutcome>,
    status: String,
    checks: Checks,
    log_filter: String,
    only_problems: bool,
    show_command: bool,
    applied_scale: f32,

    // ---- projects ----
    lib: Library,
    active: Option<usize>,
    selected: Option<usize>,
    new_name: String,
    rename_buf: Option<(usize, String)>,
    confirm_delete: Option<usize>,
    confirm_clean: bool,
    /// The project file is written a moment after the last edit rather than on
    /// every keystroke, so dragging a slider is not a burst of disk writes.
    dirty_since: Option<Instant>,
    project_filter: String,
    files: Vec<FileEntry>,
    files_dir: Option<PathBuf>,
    files_at: Option<Instant>,
    /// Selected row in the folder view, and the two in-place edits it offers.
    file_sel: Option<PathBuf>,
    file_rename: Option<(PathBuf, String)>,
    file_delete: Option<PathBuf>,

    // ---- updates ----
    update_check: Option<update::Check>,
    update_found: Option<Release>,
    update_status: String,
    update_window: bool,
    /// The launch check has already gone out. Until it does, the daily throttle
    /// is ignored: updating on startup is pointless if yesterday's check still
    /// counts.
    startup_check_done: bool,
    update_install: Option<update::Install>,
    installing: bool,

    // ---- analysis ----
    analysis_job: Option<analysis::Job>,
    analysis_report: Option<analysis::Report>,
    analysis_status: String,
    /// .bsp picked by hand. Empty means the last compile's result.
    analysis_bsp: String,
}

impl Default for App {
    fn default() -> Self {
        let mut startup_errors = Vec::new();
        let mut opts = match load_profile() {
            Ok(Some(opts)) => opts,
            Ok(None) => Options::default(),
            Err(error) => {
                startup_errors.push(error);
                Options::default()
            }
        };
        if opts.tools_dir.trim().is_empty() {
            if let Some(d) = detect_tools_dir() {
                opts.tools_dir = d;
            }
        }
        if !(0.7..=2.0).contains(&opts.ui_scale) {
            opts.ui_scale = 1.0;
        }

        // Come back to whatever project was open, with its options, so the
        // window opens ready to compile.
        let lib = match load_library() {
            Ok(lib) => lib,
            Err(error) => {
                startup_errors.push(error);
                Library::default()
            }
        };
        let active = lib.active.as_deref().and_then(|name| lib.index_of(name));
        if let Some(i) = active {
            opts = lib.projects[i].opts.clone();
            if opts.tools_dir.trim().is_empty() {
                if let Some(d) = detect_tools_dir() {
                    opts.tools_dir = d;
                }
            }
        }
        let status = if startup_errors.is_empty() {
            match active {
                Some(i) => format!("Proyecto '{}' cargado.", lib.projects[i].name),
                None => String::from("Elige un .map y la carpeta de herramientas."),
            }
        } else {
            startup_errors.join(" | ")
        };

        Self {
            opts,
            tab: Tab::Compile,
            job: None,
            log: Vec::new(),
            running_stage: None,
            stage_since: None,
            run_since: None,
            done: HashMap::new(),
            total_secs: None,
            last_outcome: None,
            status,
            checks: Checks::default(),
            log_filter: String::new(),
            only_problems: false,
            show_command: false,
            applied_scale: 0.0,

            selected: active,
            lib,
            active,
            new_name: String::new(),
            rename_buf: None,
            confirm_delete: None,
            confirm_clean: false,
            dirty_since: None,
            project_filter: String::new(),
            files: Vec::new(),
            files_dir: None,
            files_at: None,
            file_sel: None,
            file_rename: None,
            file_delete: None,

            update_check: None,
            update_found: None,
            update_status: String::new(),
            update_window: false,
            startup_check_done: false,
            update_install: None,
            installing: false,

            analysis_job: None,
            analysis_report: None,
            analysis_status: String::new(),
            analysis_bsp: String::new(),
        }
    }
}

// ---------------------------------------------------------------- profile

fn legacy_state_path(name: &str) -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join(name)))
}

fn state_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|dir| dir.join("ReSDHLT").join(name))
        .or_else(|| legacy_state_path(name))
}

fn migrate_legacy_state(name: &str, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Ok(());
    }
    let Some(legacy) = legacy_state_path(name) else {
        return Ok(());
    };
    if legacy == destination || !legacy.is_file() {
        return Ok(());
    }
    let text = std::fs::read_to_string(&legacy)
        .map_err(|error| format!("no pude migrar {}: {error}", legacy.display()))?;
    projects::atomic_write(destination, &text)
        .map_err(|error| format!("no pude migrar a {}: {error}", destination.display()))
}

fn profile_path() -> Option<PathBuf> {
    state_path("resdhlt-gui.json")
}

fn load_profile() -> Result<Option<Options>, String> {
    let path = profile_path().ok_or("no pude determinar dónde leer las preferencias")?;
    migrate_legacy_state("resdhlt-gui.json", &path)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| format!("{} está corrupto: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("no pude leer {}: {error}", path.display())),
    }
}

fn library_path() -> Option<PathBuf> {
    state_path("resdhlt-projects.json")
}

struct AppInstanceLock {
    file: File,
}

impl Drop for AppInstanceLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn acquire_app_instance_lock(path: &Path) -> Result<AppInstanceLock, String> {
    let parent = path.parent().ok_or("ruta de lock sin carpeta")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("no pude abrir {}: {error}", path.display()))?;
    file.try_lock_exclusive().map_err(|_| {
        "ReSDHLT ya está abierto. Usa la ventana existente para evitar perder proyectos."
            .to_string()
    })?;
    Ok(AppInstanceLock { file })
}

fn load_library() -> Result<Library, String> {
    let path = library_path().ok_or("no pude determinar dónde leer los proyectos")?;
    migrate_legacy_state("resdhlt-projects.json", &path)?;
    Library::load_checked(&path)
}

fn save_profile(opts: &Options) -> Result<(), String> {
    let path = profile_path().ok_or("no pude determinar dónde guardar")?;
    let text = serde_json::to_string_pretty(opts).map_err(|e| e.to_string())?;
    projects::atomic_write(&path, &text)
}

// ---------------------------------------------------------------- helpers

fn has_tools(dir: &Path) -> bool {
    dir.join("sdHLCSG.exe").is_file() || dir.join("sdHLCSG").is_file()
}

fn dir_has_wads(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries.flatten().any(|e| {
                e.path()
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case("wad"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Looks for the tools next to the GUI, which is where they end up in a normal
/// build of this repository. Saves the first-run user from browsing.
/// The `tools` folder that ships beside this executable, which is the one the
/// updater replaces.
fn bundled_tools_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [dir.join("tools"), dir.to_path_buf()]
        .into_iter()
        .find(|d| has_tools(d))
}

/// Compares the tools folder in use against the one shipped with the app and
/// returns the shipped one when it is newer.
///
/// The updater swaps the app and the `tools` beside it, but somebody who
/// pointed this at their editor's own copy (JACK, Hammer, an old install) keeps
/// compiling with those binaries after every update, with no sign that the new
/// options are missing until a compile dies on "Unknown option".
fn stale_against_bundled(in_use: &Path) -> Option<PathBuf> {
    let bundled = bundled_tools_dir()?;
    let same = std::fs::canonicalize(&bundled).ok()? == std::fs::canonicalize(in_use).ok()?;
    if same {
        return None;
    }

    let built = |dir: &Path| {
        std::fs::metadata(dir.join("sdHLCSG.exe"))
            .or_else(|_| std::fs::metadata(dir.join("sdHLCSG")))
            .and_then(|md| md.modified())
            .ok()
    };
    (built(&bundled)? > built(in_use)?).then_some(bundled)
}

/// Copies the shipped tools over the folder in use, for the mapper whose editor
/// launches the compilers from its own folder and needs them current there.
fn copy_bundled_tools(from: &Path, to: &Path) -> Result<usize, String> {
    let mut copied = 0;
    for entry in std::fs::read_dir(from).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        std::fs::copy(entry.path(), to.join(entry.file_name())).map_err(|e| {
            format!(
                "no pude copiar {}: {e}",
                entry.file_name().to_string_lossy()
            )
        })?;
        copied += 1;
    }
    Ok(copied)
}

fn detect_tools_dir() -> Option<String> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            roots.push(d.to_path_buf());
            roots.push(d.join("tools"));
            if let Some(up) = d.parent() {
                roots.push(up.join("tools"));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd.join("tools"));
        roots.push(cwd);
    }
    roots
        .into_iter()
        .find(|d| has_tools(d))
        .map(|d| d.display().to_string())
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

/// Opens the containing folder with the file highlighted, which is what people
/// mean by "show me where this is".
fn reveal_in_explorer(path: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn();
    }
    #[cfg(not(windows))]
    {
        if let Some(dir) = path.parent() {
            open_in_explorer(dir);
        }
    }
}

fn open_in_explorer(path: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

/// Whether two paths point at the same file on disk, so the delete prompt can
/// warn when it is about to remove the project's own source map.
fn same_file(a: &Path, b: &Path) -> bool {
    if a.as_os_str().is_empty() || b.as_os_str().is_empty() {
        return false;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

fn fmt_unix_age(secs: u64) -> String {
    let now = projects::now_secs();
    if secs == 0 || secs > now {
        return String::new();
    }
    projects::fmt_age(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
}

fn fmt_secs(s: f64) -> String {
    if s >= 60.0 {
        format!("{}m {:02.0}s", (s / 60.0).floor(), s % 60.0)
    } else {
        format!("{s:.1}s")
    }
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
                        self.stage_since = Some(Instant::now());
                        self.status = format!("{} en curso...", s.name());
                    }
                    Msg::StageDone(s, secs, ok) => {
                        self.done.insert(s.name(), StageState { secs, ok });
                        self.running_stage = None;
                        self.stage_since = None;
                    }
                    Msg::Finished(total, outcome) => {
                        self.total_secs = Some(total);
                        self.last_outcome = Some(outcome);
                        self.running_stage = None;
                        self.stage_since = None;
                        self.run_since = None;
                        self.status = match outcome {
                            RunOutcome::Succeeded => format!("Listo en {}", fmt_secs(total)),
                            RunOutcome::Failed => "Terminó con errores".to_string(),
                            RunOutcome::Cancelled => {
                                format!("Cancelado después de {}", fmt_secs(total))
                            }
                        };
                        finished = true;
                    }
                }
            }
        }
        if finished {
            self.job = None;
            if self.last_outcome == Some(RunOutcome::Succeeded)
                && self.lib.analysis.after_compile
                && self.opts.run_bsp | self.opts.run_vis | self.opts.run_rad
            {
                self.analysis_bsp.clear();
                self.start_analysis();
            }
        }
    }

    // ---------------- analysis ----------------

    /// The .bsp the last compile left: beside the project folder when the
    /// output is organised, in the scratch folder, or next to the source map.
    fn compiled_bsp(&self) -> Option<PathBuf> {
        let map = Path::new(self.opts.map_path.trim());
        let name = map.file_stem()?.to_os_string();
        let mut file = PathBuf::from(name);
        file.set_extension("bsp");
        [
            self.opts.output_base(),
            self.opts.work_dir(),
            map.parent().map(|p| p.to_path_buf()),
        ]
        .into_iter()
        .flatten()
        .map(|dir| dir.join(&file))
        .find(|p| p.is_file())
    }

    fn analysis_target(&self) -> Option<PathBuf> {
        let manual = self.analysis_bsp.trim();
        if manual.is_empty() {
            self.compiled_bsp()
        } else {
            Some(PathBuf::from(manual))
        }
    }

    fn start_analysis(&mut self) {
        if self.analysis_job.is_some() {
            return;
        }
        let Some(bsp) = self.analysis_target().filter(|p| p.is_file()) else {
            self.analysis_status = "No hay .bsp para analizar: compila el mapa o elige uno.".into();
            return;
        };
        // The source map only helps if it is the one this .bsp came from.
        let map = PathBuf::from(self.opts.map_path.trim());
        let map = (map.file_stem() == bsp.file_stem() && map.is_file()).then_some(map);
        self.analysis_status = format!("Analizando {}", bsp.display());
        self.analysis_job = Some(analysis::start(bsp, map, self.lib.analysis.clone()));
    }

    fn drain_analysis(&mut self) {
        let Some(job) = &self.analysis_job else {
            return;
        };
        let mut done = false;
        while let Ok(msg) = job.rx.try_recv() {
            match msg {
                analysis::Msg::Progress(text) => self.analysis_status = text,
                analysis::Msg::Done(Ok(report)) => {
                    let problems = report
                        .sections
                        .iter()
                        .filter(|s| {
                            matches!(s.level, analysis::Level::Warn | analysis::Level::Error)
                        })
                        .count();
                    self.analysis_status = format!(
                        "Análisis listo en {}: {}",
                        fmt_secs(report.secs),
                        if problems == 0 {
                            "sin problemas".to_string()
                        } else {
                            format!("{problems} apartados para revisar")
                        }
                    );
                    self.analysis_report = Some(report);
                    done = true;
                }
                analysis::Msg::Done(Err(error)) => {
                    self.analysis_status = error;
                    done = true;
                }
            }
        }
        if done {
            self.analysis_job = None;
        }
    }

    // ---------------- updates ----------------

    /// Once on launch, and once a day after that while the app stays open.
    ///
    /// The launch check ignores the daily throttle, but installation always
    /// requires an explicit click in the release window.
    fn maybe_check_updates(&mut self) {
        const DAY: u64 = 24 * 60 * 60;
        if !self.lib.check_updates || self.update_check.is_some() || self.update_found.is_some() {
            return;
        }
        if !self.startup_check_done {
            self.startup_check_done = true;
            self.start_update_check(false);
            return;
        }
        if projects::now_secs().saturating_sub(self.lib.last_update_check) < DAY {
            return;
        }
        self.start_update_check(false);
    }

    fn start_update_check(&mut self, manual: bool) {
        if self.update_check.is_some() {
            return;
        }
        self.lib.last_update_check = projects::now_secs();
        self.save_library();
        self.update_status = "Buscando actualizaciones...".to_string();
        if manual {
            self.status = self.update_status.clone();
        }
        self.update_check = Some(update::check());
    }

    fn drain_update_check(&mut self) {
        let Some(check) = &self.update_check else {
            return;
        };
        let Ok(msg) = check.rx.try_recv() else {
            return;
        };
        self.update_check = None;
        match msg {
            update::Msg::Available(release) => {
                self.update_status = format!("Hay una versión nueva: {}", release.tag);
                self.status = self.update_status.clone();
                self.update_window = true;
                self.update_found = Some(release);
            }
            update::Msg::UpToDate => {
                self.update_status = format!(
                    "Estás en la última versión ({}).",
                    update::Version::current()
                );
            }
            update::Msg::Failed(e) => {
                self.update_status = format!("No pude comprobar actualizaciones: {e}");
            }
        }
    }

    /// The confirmation window: what is going to be installed, and from where.
    fn ui_update_window(&mut self, ctx: &egui::Context) {
        if !self.update_window {
            return;
        }
        let Some(release) = self.update_found.clone() else {
            self.update_window = false;
            return;
        };

        let mut open = true;
        let mut install = false;
        egui::Window::new("Actualización disponible")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(560.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} -> {}",
                            update::Version::current(),
                            release.version
                        ))
                        .color(TEXT)
                        .strong()
                        .size(15.0),
                    );
                    chip(ui, &release.tag, ACCENT);
                });
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "{} · {} · github.com/{}",
                        release.asset_name,
                        projects::fmt_size(release.asset_size),
                        update::REPO
                    ))
                    .color(MUTED)
                    .small(),
                );
                ui.add_space(10.0);

                if !release.notes.trim().is_empty() {
                    egui::ScrollArea::vertical()
                        .id_source("release_notes")
                        .max_height(260.0)
                        .show(ui, |ui| {
                            ui.label(RichText::new(release.notes.trim()).color(TEXT).small());
                        });
                    ui.add_space(10.0);
                }

                ui.label(
                    RichText::new(
                        "Se descarga el .zip de la release, se reemplazan el ejecutable y \
                         la carpeta tools, y la GUI se reinicia sola. Tus proyectos y \
                         preferencias no se tocan.",
                    )
                    .color(MUTED)
                    .small(),
                );
                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    let btn = egui::Button::new(
                        RichText::new(if self.installing {
                            "Instalando..."
                        } else {
                            "Actualizar ahora"
                        })
                        .strong(),
                    )
                    .min_size(egui::vec2(170.0, 30.0))
                    .fill(ACCENT_DEEP)
                    .stroke(egui::Stroke::new(1.0_f32, ACCENT));
                    if ui
                        .add_enabled(!self.installing && self.job.is_none(), btn)
                        .on_hover_text(if self.job.is_some() {
                            "Cancela o espera la compilación antes de actualizar."
                        } else {
                            "Descargar e instalar esta versión."
                        })
                        .clicked()
                    {
                        install = true;
                    }
                    if ui.button("Ahora no").clicked() {
                        self.update_window = false;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.hyperlink_to(
                            "Ver en GitHub",
                            format!("https://github.com/{}/releases/latest", update::REPO),
                        );
                    });
                });

                if !self.update_status.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new(&self.update_status).color(MUTED).small());
                }
            });

        if !open {
            self.update_window = false;
        }
        if install {
            self.do_install(&release);
        }
    }

    /// Downloads and stages in a worker. `drain_install` closes the app only
    /// after the swap helper is ready.
    fn do_install(&mut self, release: &Release) {
        if self.job.is_some() || self.installing {
            self.update_status = "Cancela o espera la compilación antes de actualizar.".to_string();
            return;
        }
        self.installing = true;
        self.update_status = "Descargando...".to_string();
        self.update_install = Some(update::install_async(release.clone()));
    }

    fn drain_install(&mut self, ctx: &egui::Context) {
        let Some(install) = &self.update_install else {
            return;
        };
        let Ok(result) = install.rx.try_recv() else {
            return;
        };
        self.update_install = None;
        match result {
            Ok(()) => {
                // The helper is waiting for this process to exit before it
                // can replace the files.
                let _ = save_profile(&self.opts);
                self.sync_active_project();
                self.save_library();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => {
                self.installing = false;
                self.update_status = format!("Falló la actualización: {e}");
                self.status = self.update_status.clone();
                // Leave the window up with the button in it rather than a dead
                // status line, so a failed automatic update can be retried.
                self.update_window = true;
            }
        }
    }

    // ---------------- projects ----------------

    fn save_library(&mut self) {
        self.lib.active = self.active.map(|i| self.lib.projects[i].name.clone());
        if let Some(path) = library_path() {
            if let Err(e) = self.lib.save(&path) {
                self.status = format!("No pude guardar los proyectos: {e}");
                return;
            }
        }
        self.dirty_since = None;
    }

    /// Keeps the open project in step with whatever the user is editing, then
    /// writes it out once the edits stop. Comparing the whole options struct is
    /// simpler than tracking every widget, and cheap at this size.
    fn sync_active_project(&mut self) {
        if let Some(i) = self.active {
            if self.lib.projects[i].opts != self.opts {
                self.lib.projects[i].opts = self.opts.clone();
                self.lib.projects[i].last_used = projects::now_secs();
                self.dirty_since = Some(Instant::now());
            }
        }
        // A short delay so dragging a slider is one write, not hundreds.
        if let Some(since) = self.dirty_since {
            if since.elapsed().as_millis() > 800 {
                self.save_library();
            }
        }
    }

    fn load_project(&mut self, index: usize) {
        // Whatever is on screen belongs to the project being left.
        self.sync_active_project();
        if self.dirty_since.is_some() {
            self.save_library();
        }

        let project = &mut self.lib.projects[index];
        project.last_used = projects::now_secs();
        project.opts.project_name = project.name.clone();
        self.opts = project.opts.clone();
        // A project saved on another machine may point at tools that are not
        // there any more; the local ones are the ones that work.
        if !has_tools(Path::new(self.opts.tools_dir.trim())) {
            if let Some(d) = detect_tools_dir() {
                self.opts.tools_dir = d;
            }
        }
        self.active = Some(index);
        self.selected = Some(index);
        self.status = format!("Proyecto '{}' cargado.", self.lib.projects[index].name);
        self.files_at = None; // force a folder rescan
        self.save_library();
        let _ = save_profile(&self.opts);
    }

    fn create_project(&mut self, name: &str, opts: Options) {
        let name = self.lib.unique_name(name);
        let mut opts = opts;
        opts.project_name = name.clone();
        self.opts.project_name = name.clone();
        self.lib.projects.push(Project::new(&name, opts));
        self.lib.sort_by_name();
        let index = self.lib.index_of(&name).unwrap_or(0);
        self.active = Some(index);
        self.selected = Some(index);
        self.status = format!("Proyecto '{name}' creado.");
        self.new_name.clear();
        self.files_at = None;
        self.save_library();
    }

    fn delete_project(&mut self, index: usize) {
        let name = self.lib.projects.remove(index).name;
        // Indices shift; whatever was pointing past the hole moves with it.
        let fix = |cur: Option<usize>| match cur {
            Some(i) if i == index => None,
            Some(i) if i > index => Some(i - 1),
            other => other,
        };
        self.active = fix(self.active);
        self.selected = fix(self.selected);
        self.confirm_delete = None;
        self.status = format!("Proyecto '{name}' borrado. Los archivos del mapa no se tocaron.");
        self.save_library();
    }

    /// Files in the open project's folder, rescanned when stale.
    fn refresh_files(&mut self, force: bool) {
        let dir = self
            .selected
            .and_then(|i| self.lib.projects[i].folder())
            .or_else(|| {
                Path::new(self.opts.output_dir.trim())
                    .is_dir()
                    .then(|| PathBuf::from(self.opts.output_dir.trim()))
            })
            .or_else(|| {
                Path::new(self.opts.map_path.trim())
                    .parent()
                    .filter(|p| p.is_dir())
                    .map(|p| p.to_path_buf())
            });

        let stale = self
            .files_at
            .map(|t| t.elapsed().as_secs() >= 3)
            .unwrap_or(true);
        if !force && !stale && dir == self.files_dir {
            return;
        }
        self.files = dir
            .as_deref()
            .map(projects::scan_folder)
            .unwrap_or_default();

        self.files_dir = dir;
        self.files_at = Some(Instant::now());
    }

    fn refresh_checks(&mut self) {
        let periodic = self
            .checks
            .checked_at
            .map(|at| at.elapsed().as_secs() >= 2)
            .unwrap_or(true);
        let tools_key = format!(
            "{}|{}{}{}{}",
            self.opts.tools_dir,
            self.opts.run_csg as u8,
            self.opts.run_bsp as u8,
            self.opts.run_vis as u8,
            self.opts.run_rad as u8
        );
        let plan_key = format!(
            "{}|{}|{}|{}|{}|{}{}{}{}|{}|{}|{}|{}",
            self.opts.map_path,
            self.opts.tools_dir,
            self.opts.output_dir,
            self.opts.project_name,
            self.opts.organize_output as u8,
            self.opts.run_csg as u8,
            self.opts.run_bsp as u8,
            self.opts.run_vis as u8,
            self.opts.run_rad as u8,
            self.opts.csg_extra,
            self.opts.bsp_extra,
            self.opts.vis_extra,
            self.opts.rad_extra
        );
        if periodic || self.checks.map_key != self.opts.map_path {
            self.checks.map_key = self.opts.map_path.clone();
            self.checks.map_ok = !self.opts.map_path.trim().is_empty()
                && Path::new(self.opts.map_path.trim()).is_file();
        }
        if periodic || self.checks.tools_key != tools_key {
            self.checks.tools_key = tools_key;
            let dir = Path::new(self.opts.tools_dir.trim());
            self.checks.tools_ok = !self.opts.tools_dir.trim().is_empty()
                && STAGES.iter().all(|stage| {
                    let enabled = match stage {
                        Stage::Csg => self.opts.run_csg,
                        Stage::Bsp => self.opts.run_bsp,
                        Stage::Vis => self.opts.run_vis,
                        Stage::Rad => self.opts.run_rad,
                    };
                    !enabled
                        || dir.join(format!("{}.exe", stage.exe())).is_file()
                        || dir.join(stage.exe()).is_file()
                });
            self.checks.tools_stale = self
                .checks
                .tools_ok
                .then(|| stale_against_bundled(Path::new(self.opts.tools_dir.trim())))
                .flatten();
        }
        if periodic || self.checks.plan_key != plan_key {
            self.checks.plan_key = plan_key;
            self.checks.plan_errors = runner::validation_errors(&self.opts);
        }
        if periodic {
            self.checks.checked_at = Some(Instant::now());
        }
    }

    fn can_run(&self) -> bool {
        self.job.is_none() && !self.installing && self.checks.plan_errors.is_empty()
    }

    fn enabled_stages(&self) -> usize {
        [
            self.opts.run_csg,
            self.opts.run_bsp,
            self.opts.run_vis,
            self.opts.run_rad,
        ]
        .iter()
        .filter(|b| **b)
        .count()
    }

    /// Where the .bsp ends up: the project's folder inside the output folder if
    /// there is one, else next to the source map.
    fn result_dir(&self) -> Option<PathBuf> {
        if let Some(base) = self.opts.output_base() {
            if base.is_dir() {
                return Some(base);
            }
        }
        if self.opts.uses_output_dir() {
            let p = PathBuf::from(self.opts.output_dir.trim());
            return p.is_dir().then_some(p);
        }
        Path::new(self.opts.map_path.trim())
            .parent()
            .map(|p| p.to_path_buf())
            .filter(|p| p.is_dir())
    }

    fn start(&mut self) {
        self.opts.normalize_paths();
        let plan = match CompilePlan::new(self.opts.clone()) {
            Ok(plan) => plan,
            Err(errors) => {
                self.status = errors
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "No se puede iniciar la compilación.".to_string());
                self.log
                    .extend(errors.into_iter().map(|error| (LineKind::Error, error)));
                self.last_outcome = Some(RunOutcome::Failed);
                return;
            }
        };
        // Persist before every run, so a crash mid-compile cannot lose settings.
        let _ = save_profile(&self.opts);
        self.log.clear();
        self.done.clear();
        self.total_secs = None;
        self.last_outcome = None;
        self.run_since = Some(Instant::now());
        self.job = Some(runner::start(plan));
        // The project should hold what was actually compiled, even if the
        // window never gets closed cleanly afterwards.
        self.sync_active_project();
        self.save_library();
        self.files_at = None;
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        for p in dropped {
            if p.is_dir() {
                // A tools folder is recognised by its executables; anything else
                // dropped is taken as the output folder.
                if has_tools(&p) {
                    self.opts.tools_dir = p.display().to_string();
                    self.status = "Carpeta de herramientas actualizada.".into();
                } else if dir_has_wads(&p) {
                    self.opts.wad_dir = p.display().to_string();
                    self.status = "Carpeta de WADs actualizada.".into();
                } else {
                    self.opts.output_dir = p.display().to_string();
                    self.status = "Carpeta de salida actualizada.".into();
                }
            } else if p
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x.eq_ignore_ascii_case("map"))
                .unwrap_or(false)
            {
                self.opts.map_path = p.display().to_string();
                self.status = "Mapa cargado.".into();
                self.tab = Tab::Compile;
            }
        }
    }

    // ---------------- tabs ----------------

    fn ui_projects(&mut self, ui: &mut egui::Ui, m: &Metrics) {
        card(
            ui,
            "Proyectos",
            "cada uno guarda su mapa, sus carpetas y todas las opciones",
            |ui| {
                ui.horizontal(|ui| {
                    let btn_w = 200.0;
                    let field_w = (ui.available_width() - btn_w - 16.0).max(120.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_name)
                            .desired_width(field_w)
                            .hint_text("nombre del proyecto (vacío = el del mapa)"),
                    );
                    let btn = egui::Button::new(RichText::new("Guardar como proyecto").strong())
                        .min_size(egui::vec2(btn_w, 26.0))
                        .fill(ACCENT_DEEP)
                        .stroke(egui::Stroke::new(1.0_f32, ACCENT));
                    if ui
                        .add(btn)
                        .on_hover_text(
                            "Crea un proyecto con lo que tienes cargado ahora: mapa, \
                             carpetas y todas las opciones de las demás pestañas.",
                        )
                        .clicked()
                    {
                        let name = if self.new_name.trim().is_empty() {
                            projects::name_from_map(&self.opts.map_path)
                        } else {
                            projects::sanitize_name(&self.new_name)
                        };
                        let opts = self.opts.clone();
                        self.create_project(&name, opts);
                    }
                });

                ui.add_space(8.0);

                if self.lib.projects.is_empty() {
                    ui.label(
                        RichText::new(
                            "Todavía no hay ninguno. Carga un .map en la pestaña Compilar, \
                             ajusta lo que quieras y guárdalo aquí con un nombre: zm_hola, \
                             de_dust_beta3, lo que sea.",
                        )
                        .color(MUTED)
                        .small(),
                    );
                    return;
                }

                // A filter earns its place once the list stops fitting at a
                // glance; below that it is one more thing in the way.
                if self.lib.projects.len() > 8 {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.project_filter)
                                .desired_width(ui.available_width() - 90.0)
                                .hint_text("filtrar por nombre o mapa..."),
                        );
                        if ui.button("Limpiar").clicked() {
                            self.project_filter.clear();
                        }
                    });
                    ui.add_space(6.0);
                }

                let needle = self.project_filter.to_lowercase();
                let shown: Vec<(usize, String, String, u64)> = self
                    .lib
                    .projects
                    .iter()
                    .enumerate()
                    .map(|(i, p)| (i, p.name.clone(), p.map_name(), p.last_used))
                    .filter(|(_, name, map, _)| {
                        needle.is_empty()
                            || name.to_lowercase().contains(&needle)
                            || map.to_lowercase().contains(&needle)
                    })
                    .collect();

                if shown.is_empty() {
                    ui.label(
                        RichText::new("Ningún proyecto coincide con el filtro.")
                            .color(MUTED)
                            .small(),
                    );
                    return;
                }

                // No inner scroll area: the page scrolls, so every project is
                // reachable the same way as everything else on the tab. Columns
                // as soon as there is room, because a list of short names down
                // one side wastes most of the width.
                let avail = ui.available_width();
                let cols = if avail >= 1000.0 {
                    3
                } else if avail >= 620.0 {
                    2
                } else {
                    1
                };
                // Cards need visible air between them; at the default spacing
                // they read as one block.
                let gap = 18.0;
                let cell_w = ((avail - gap * (cols as f32 - 1.0)) / cols as f32).max(200.0);
                let cell_h = 38.0;

                let mut load: Option<usize> = None;
                for chunk in shown.chunks(cols) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for (i, name, map, used) in chunk {
                            let i = *i;
                            let is_active = self.active == Some(i);
                            let is_selected = self.selected == Some(i);

                            // The row is allocated as one clickable rectangle
                            // first, then painted and filled in. Doing it this
                            // way means the whole card answers the click, not
                            // just whatever label happens to be under the
                            // cursor, and the hover highlight makes it obvious
                            // that the thing is clickable at all.
                            let resp = ui.allocate_response(
                                egui::vec2(cell_w, cell_h),
                                egui::Sense::click(),
                            );
                            let hovered = resp.hovered();
                            let fill = if is_selected {
                                ACCENT_DEEP
                            } else if hovered {
                                CARD_HI
                            } else {
                                CARD
                            };
                            let border = if is_active {
                                ACCENT
                            } else if hovered || is_selected {
                                ACCENT.linear_multiply(0.6)
                            } else {
                                LINE
                            };
                            ui.painter().rect(
                                resp.rect,
                                ROUND,
                                fill,
                                egui::Stroke::new(1.0_f32, border),
                            );

                            let inner = resp.rect.shrink2(egui::vec2(12.0, 6.0));
                            ui.allocate_ui_at_rect(inner, |ui| {
                                ui.horizontal(|ui| {
                                    ui.set_min_height(inner.height());
                                    // Button and map name first, from the right;
                                    // the project name takes what is left. A
                                    // truncating label claims the whole row, so
                                    // the other way round draws on top of it.
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        // A real button, so opening a project is
                                        // one click and never a guess.
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new(if is_active {
                                                        "abierto"
                                                    } else {
                                                        "Abrir"
                                                    })
                                                    .small(),
                                                )
                                                .small()
                                                .fill(if is_active {
                                                    ACCENT.linear_multiply(0.25)
                                                } else {
                                                    CARD_HI
                                                })
                                                .stroke(egui::Stroke::new(1.0_f32, LINE)),
                                            )
                                            .clicked()
                                        {
                                            load = Some(i);
                                        }
                                        // The map name is secondary, so it never
                                        // takes more than a third of the card:
                                        // the project's own name is what the
                                        // user is looking for and must not end
                                        // up as an ellipsis.
                                        let map_w =
                                            ((inner.width() * 0.34) - 8.0).clamp(0.0, 130.0);
                                        if map_w > 50.0 && ui.available_width() > map_w + 60.0 {
                                            ui.allocate_ui_with_layout(
                                                egui::vec2(map_w, inner.height()),
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    ui.add(
                                                        egui::Label::new(
                                                            RichText::new(map).color(MUTED).small(),
                                                        )
                                                        .truncate()
                                                        .selectable(false),
                                                    );
                                                },
                                            );
                                        }

                                        ui.with_layout(
                                            Layout::left_to_right(Align::Center),
                                            |ui| {
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(name)
                                                            .color(if is_selected || is_active {
                                                                egui::Color32::WHITE
                                                            } else {
                                                                TEXT
                                                            })
                                                            .strong(),
                                                    )
                                                    .truncate()
                                                    .selectable(false),
                                                );
                                            },
                                        );
                                    });
                                });
                            });

                            if resp.clicked() {
                                self.selected = Some(i);
                            }
                            if resp.double_clicked() {
                                load = Some(i);
                            }
                            let age = fmt_unix_age(*used);
                            let _ = resp.on_hover_text(if age.is_empty() {
                                "Click para elegirlo · doble click para abrirlo".to_string()
                            } else {
                                format!(
                                    "Usado {age}. Click para elegirlo, doble click para abrirlo."
                                )
                            });
                        }
                    });
                    ui.add_space(gap * 0.7);
                }
                if let Some(i) = load {
                    self.load_project(i);
                }

                ui.add_space(6.0);

                let Some(sel) = self.selected else {
                    ui.label(
                        RichText::new("Elige un proyecto de la lista.")
                            .color(MUTED)
                            .small(),
                    );
                    return;
                };

                // Rename happens in place, right above the buttons.
                let mut commit_rename: Option<String> = None;
                let mut cancel_rename = false;
                if let Some((idx, buf)) = &mut self.rename_buf {
                    if *idx == sel {
                        ui.horizontal(|ui| {
                            let w = (ui.available_width() - 170.0).max(120.0);
                            let r = ui.add(
                                egui::TextEdit::singleline(buf)
                                    .desired_width(w)
                                    .hint_text("nuevo nombre"),
                            );
                            let entered =
                                r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if entered || ui.button("Aceptar").clicked() {
                                commit_rename = Some(buf.clone());
                            }
                            if ui.button("Cancelar").clicked() {
                                cancel_rename = true;
                            }
                        });
                        ui.add_space(6.0);
                    }
                }
                if cancel_rename {
                    self.rename_buf = None;
                }
                if let Some(wanted) = commit_rename {
                    let wanted = projects::sanitize_name(&wanted);
                    if self.lib.name_taken(&wanted, Some(sel)) {
                        self.status = format!("Ya hay un proyecto llamado '{wanted}'.");
                    } else {
                        // Sorting invalidates every numeric index, not just the
                        // one being renamed. Preserve both identities by name
                        // and resolve them again after the sort.
                        let active_name = self.active.map(|index| {
                            if index == sel {
                                wanted.clone()
                            } else {
                                self.lib.projects[index].name.clone()
                            }
                        });
                        self.lib.projects[sel].name = wanted.clone();
                        self.lib.projects[sel].opts.project_name = wanted.clone();
                        if self.active == Some(sel) {
                            self.opts.project_name = wanted.clone();
                        }
                        self.lib.sort_by_name();
                        self.active = active_name
                            .as_deref()
                            .and_then(|name| self.lib.index_of(name));
                        self.selected = self.lib.index_of(&wanted);
                        self.rename_buf = None;
                        self.status = format!("Renombrado a '{wanted}'.");
                        self.save_library();
                    }
                }

                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button(RichText::new("Abrir").strong())
                        .on_hover_text("Carga su mapa y todas sus opciones")
                        .clicked()
                    {
                        self.load_project(sel);
                    }
                    if ui.button("Renombrar").clicked() {
                        self.rename_buf = Some((sel, self.lib.projects[sel].name.clone()));
                    }
                    if ui.button("Duplicar").clicked() {
                        let src = self.lib.projects[sel].clone();
                        self.create_project(&src.name, src.opts);
                    }
                    if ui
                        .button("Actualizar con lo actual")
                        .on_hover_text(
                            "Pisa las opciones guardadas del proyecto con las que tienes \
                             en pantalla ahora.",
                        )
                        .clicked()
                    {
                        self.lib.projects[sel].opts = self.opts.clone();
                        self.lib.projects[sel].last_used = projects::now_secs();
                        self.status = format!("'{}' actualizado.", self.lib.projects[sel].name);
                        self.save_library();
                    }

                    if self.confirm_delete == Some(sel) {
                        let del = egui::Button::new(RichText::new("Confirmar borrado").strong())
                            .fill(ERR.linear_multiply(0.35))
                            .stroke(egui::Stroke::new(1.0_f32, ERR));
                        if ui.add(del).clicked() {
                            self.delete_project(sel);
                        }
                        if ui.button("No").clicked() {
                            self.confirm_delete = None;
                        }
                    } else if ui
                        .button(RichText::new("Borrar").color(ERR))
                        .on_hover_text(
                            "Borra el proyecto de la lista. No toca ningún archivo del mapa.",
                        )
                        .clicked()
                    {
                        self.confirm_delete = Some(sel);
                    }
                });
            },
        );

        let Some(sel) = self.selected else { return };
        let (name, opts) = {
            let p = &self.lib.projects[sel];
            (p.name.clone(), p.opts.clone())
        };

        card(ui, &name, "lo que guarda este proyecto", |ui| {
            let rows = [
                ("Mapa", opts.map_path.clone()),
                ("Herramientas", opts.tools_dir.clone()),
                ("Carpeta de salida", opts.output_dir.clone()),
                ("Carpeta de WADs", opts.wad_dir.clone()),
            ];
            for (label, value) in rows {
                row(ui, m, label, "", None, |ui| {
                    let shown = if value.trim().is_empty() {
                        RichText::new("(sin definir)").color(FAINT).small()
                    } else {
                        RichText::new(value.clone()).color(TEXT).small()
                    };
                    ui.add(egui::Label::new(shown).truncate())
                        .on_hover_text(value);
                });
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "VIS {} · {} · bounce {} · skylevel {} · {}",
                    opts.vis_quality.label(),
                    if opts.rad_fast {
                        "RAD rápido"
                    } else if opts.extra_sampling {
                        "RAD con -extra"
                    } else {
                        "RAD sin -extra"
                    },
                    opts.bounce,
                    opts.skylevel,
                    if opts.pre25 { "-pre25" } else { "sin -pre25" },
                ))
                .color(MUTED)
                .small(),
            );
        });
    }

    /// What is sitting in the project's folder right now: the compile's output,
    /// its logs, and the scratch files worth deleting.
    fn ui_project_files(&mut self, ui: &mut egui::Ui) {
        self.refresh_files(false);

        let Some(dir) = self.files_dir.clone() else {
            card(ui, "Carpeta del proyecto", "", |ui| {
                ui.label(
                    RichText::new("Todavía no hay una carpeta válida que mirar.")
                        .color(MUTED)
                        .small(),
                );
            });
            return;
        };

        let junk: Vec<PathBuf> = self
            .files
            .iter()
            .filter(|f| f.kind == FileKind::Intermediate)
            .map(|f| f.path.clone())
            .collect();
        let junk_bytes: u64 = self
            .files
            .iter()
            .filter(|f| f.kind == FileKind::Intermediate)
            .map(|f| f.size)
            .sum();
        let total: u64 = self.files.iter().map(|f| f.size).sum();
        let subtitle = format!(
            "{} archivos · {}",
            self.files.len(),
            projects::fmt_size(total)
        );

        card(ui, "Carpeta del proyecto", &subtitle, |ui| {
            let path_text = dir.display().to_string();
            ui.add(egui::Label::new(RichText::new(&path_text).color(MUTED).small()).truncate())
                .on_hover_text(&path_text);
            ui.add_space(6.0);

            let mut do_refresh = false;
            let mut do_clean = false;
            ui.horizontal_wrapped(|ui| {
                if ui.button("Abrir carpeta").clicked() {
                    open_in_explorer(&dir);
                }
                if ui.button("Actualizar").clicked() {
                    do_refresh = true;
                }
                // In the same left-to-right flow as the other two: pinning this
                // one to the right edge made it sit on top of "Actualizar" as
                // soon as the panel was narrow.
                if !junk.is_empty() {
                    if self.confirm_clean {
                        let btn = egui::Button::new(RichText::new("Confirmar").strong())
                            .fill(ERR.linear_multiply(0.35))
                            .stroke(egui::Stroke::new(1.0_f32, ERR));
                        if ui.add(btn).clicked() {
                            do_clean = true;
                        }
                        if ui.button("No").clicked() {
                            self.confirm_clean = false;
                        }
                    } else if ui
                        .button(format!(
                            "Limpiar intermedios ({}, {})",
                            junk.len(),
                            projects::fmt_size(junk_bytes)
                        ))
                        .on_hover_text(
                            "Borra .p0 a .p3, .lin, .pts, .wa_, .ext y .max. El .map, \
                             el .bsp y los logs no se tocan.",
                        )
                        .clicked()
                    {
                        self.confirm_clean = true;
                    }
                }
            });

            if do_clean {
                let mut removed = 0;
                for p in &junk {
                    if std::fs::remove_file(p).is_ok() {
                        removed += 1;
                    }
                }
                self.status = format!("Borrados {removed} archivos intermedios.");
                self.confirm_clean = false;
                do_refresh = true;
            }
            if do_refresh {
                self.refresh_files(true);
            }

            ui.add_space(8.0);

            if self.files.is_empty() {
                ui.label(RichText::new("La carpeta está vacía.").color(MUTED).small());
                return;
            }

            // In-place rename of the selected file.
            let mut commit_rename: Option<(PathBuf, String)> = None;
            let mut cancel_rename = false;
            if let Some((path, buf)) = &mut self.file_rename {
                let old_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("Renombrar {old_name}:"))
                            .color(TEXT)
                            .small(),
                    );
                    let w = (ui.available_width() - 170.0).max(120.0);
                    let r = ui.add(
                        egui::TextEdit::singleline(buf)
                            .desired_width(w)
                            .hint_text("nuevo nombre con extensión"),
                    );
                    let entered = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if entered || ui.button("Aceptar").clicked() {
                        commit_rename = Some((path.clone(), buf.clone()));
                    }
                    if ui.button("Cancelar").clicked() {
                        cancel_rename = true;
                    }
                });
                ui.add_space(6.0);
            }
            if cancel_rename {
                self.file_rename = None;
            }
            if let Some((from, wanted)) = commit_rename {
                self.rename_file(&from, &wanted);
                do_refresh = true;
            }

            // Deleting a real file gets its own confirmation bar, not a menu
            // item that acts on the first click.
            let mut confirmed_delete: Option<PathBuf> = None;
            if let Some(path) = self.file_delete.clone() {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string();
                let source_map = self
                    .selected
                    .and_then(|index| self.lib.projects.get(index))
                    .map(|project| project.opts.map_path.as_str())
                    .unwrap_or(self.opts.map_path.as_str());
                let is_source = same_file(&path, Path::new(source_map.trim()));
                egui::Frame::none()
                    .fill(ERR.linear_multiply(0.18))
                    .stroke(egui::Stroke::new(1.0_f32, ERR))
                    .rounding(ROUND)
                    .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new(if is_source {
                                format!("¿Borrar {name}? ES EL .MAP FUENTE DE ESTE PROYECTO.")
                            } else {
                                format!("¿Borrar {name}? No va a la papelera.")
                            })
                            .color(if is_source { ERR } else { TEXT })
                            .strong(),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let btn = egui::Button::new(RichText::new("Sí, borrar").strong())
                                .fill(ERR.linear_multiply(0.35))
                                .stroke(egui::Stroke::new(1.0_f32, ERR));
                            if ui.add(btn).clicked() {
                                confirmed_delete = Some(path.clone());
                            }
                            if ui.button("Cancelar").clicked() {
                                self.file_delete = None;
                            }
                        });
                    });
                ui.add_space(6.0);
            }
            if let Some(path) = confirmed_delete {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string();
                self.status = match std::fs::remove_file(&path) {
                    Ok(()) => format!("{name} borrado."),
                    Err(e) => format!("No pude borrar {name}: {e}"),
                };
                if self.file_sel.as_deref() == Some(path.as_path()) {
                    self.file_sel = None;
                }
                self.file_delete = None;
                do_refresh = true;
            }

            // The panel is as tall as the window, so the list simply fills it
            // and scrolls when there is more. auto_shrink on the vertical axis
            // keeps a short list from being stretched into a scrollbox.
            let row_h = 24.0;
            let scroll = egui::ScrollArea::vertical()
                .id_source("project_files")
                .auto_shrink([false, true]);

            let mut open_file: Option<PathBuf> = None;
            let mut reveal_file: Option<PathBuf> = None;
            let mut select_file: Option<PathBuf> = None;
            let mut rename_file: Option<PathBuf> = None;
            let mut delete_file: Option<PathBuf> = None;
            let mut copy_path: Option<String> = None;

            // Buttons for whatever is marked, so the actions are visible
            // instead of hiding behind a right click.
            if let Some(sel) = self.file_sel.clone() {
                if self.files.iter().any(|f| f.path == sel) {
                    let name = sel
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("?")
                        .to_string();
                    // One quiet strip, not a card inside a card: the file on the
                    // left, its actions on the right, everything on one line.
                    egui::Frame::none()
                        .fill(CARD_HI)
                        .rounding(ROUND)
                        .inner_margin(egui::Margin::symmetric(10.0, 6.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            // Name on its own line, buttons underneath. Five
                            // buttons and a file name do not fit across a side
                            // panel: sharing the line either overlapped the
                            // text or squeezed it down to an ellipsis.
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&name).color(TEXT).monospace().size(11.5),
                                )
                                .truncate()
                                .selectable(false),
                            )
                            .on_hover_text(sel.display().to_string());
                            ui.add_space(6.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
                                if ui.small_button("Abrir").clicked() {
                                    open_file = Some(sel.clone());
                                }
                                if ui
                                    .small_button("Mostrar")
                                    .on_hover_text("Abrir la carpeta con el archivo marcado")
                                    .clicked()
                                {
                                    reveal_file = Some(sel.clone());
                                }
                                if ui
                                    .small_button("Ruta")
                                    .on_hover_text("Copiar la ruta completa")
                                    .clicked()
                                {
                                    copy_path = Some(sel.display().to_string());
                                }
                                if ui.small_button("Renombrar").clicked() {
                                    rename_file = Some(sel.clone());
                                }
                                if ui
                                    .add(
                                        egui::Button::new(RichText::new("Borrar").color(ERR))
                                            .small(),
                                    )
                                    .clicked()
                                {
                                    delete_file = Some(sel.clone());
                                }
                            });
                        });
                    ui.add_space(8.0);
                }
            }

            scroll.show(ui, |ui| {
                for f in &self.files {
                    let selected = self.file_sel.as_deref() == Some(f.path.as_path());
                    let color = match f.kind {
                        FileKind::Bsp => OK,
                        FileKind::Map => ACCENT,
                        FileKind::Log => WARN,
                        FileKind::Intermediate => FAINT,
                        _ => MUTED,
                    };

                    // Same shape as the project cards: reserve the row as one
                    // clickable rectangle, then paint into it. Wrapping labels
                    // in a frame and asking the frame for clicks does not work
                    // - the click lands on whatever label is under the cursor
                    // and the row never answers.
                    let resp = ui.allocate_response(
                        egui::vec2(ui.available_width(), row_h),
                        egui::Sense::click(),
                    );
                    let hovered = resp.hovered();
                    if selected || hovered {
                        ui.painter().rect_filled(
                            resp.rect,
                            4.0,
                            if selected { ACCENT_DEEP } else { CARD_HI },
                        );
                    }

                    // Fixed columns, all vertically centred on the row: a dot
                    // for the file type, the name, then size and age in
                    // right-aligned cells of their own so they line up down the
                    // list instead of drifting with the text length.
                    let mid = resp.rect.center().y;
                    ui.painter().circle_filled(
                        egui::pos2(resp.rect.left() + 12.0, mid),
                        3.5,
                        color,
                    );

                    let size_w = 66.0;
                    let age_w = 86.0;
                    let name_rect = egui::Rect::from_min_max(
                        egui::pos2(resp.rect.left() + 24.0, resp.rect.top()),
                        egui::pos2(
                            (resp.rect.right() - size_w - age_w - 14.0)
                                .max(resp.rect.left() + 60.0),
                            resp.rect.bottom(),
                        ),
                    );
                    let size_rect = egui::Rect::from_min_max(
                        egui::pos2(resp.rect.right() - size_w - age_w - 8.0, resp.rect.top()),
                        egui::pos2(resp.rect.right() - age_w - 8.0, resp.rect.bottom()),
                    );
                    let age_rect = egui::Rect::from_min_max(
                        egui::pos2(resp.rect.right() - age_w - 6.0, resp.rect.top()),
                        egui::pos2(resp.rect.right() - 6.0, resp.rect.bottom()),
                    );

                    let mut cell = |rect: egui::Rect, align: Align, text: RichText| {
                        ui.allocate_ui_at_rect(rect, |ui| {
                            ui.with_layout(
                                if align == Align::Min {
                                    Layout::left_to_right(Align::Center)
                                } else {
                                    Layout::right_to_left(Align::Center)
                                },
                                |ui| {
                                    ui.set_min_height(rect.height());
                                    ui.add(egui::Label::new(text).truncate().selectable(false));
                                },
                            );
                        });
                    };

                    cell(
                        name_rect,
                        Align::Min,
                        RichText::new(&f.name)
                            .color(if selected { egui::Color32::WHITE } else { TEXT })
                            .monospace()
                            .size(11.5),
                    );
                    cell(
                        size_rect,
                        Align::Max,
                        RichText::new(projects::fmt_size(f.size))
                            .color(MUTED)
                            .small(),
                    );
                    if let Some(t) = f.modified {
                        cell(
                            age_rect,
                            Align::Max,
                            RichText::new(projects::fmt_age(t)).color(FAINT).small(),
                        );
                    }

                    if resp.clicked() {
                        select_file = Some(f.path.clone());
                    }
                    if resp.double_clicked() {
                        open_file = Some(f.path.clone());
                    }
                    // The name column truncates on a narrow panel, so the whole
                    // thing lives in the tooltip.
                    let kind = f.kind.label();
                    let hover = resp.on_hover_text(format!(
                        "{}{}\nClick marca · doble click abre · click derecho para más",
                        f.name,
                        if kind.is_empty() {
                            String::new()
                        } else {
                            format!("  ({kind})")
                        }
                    ));

                    hover.context_menu(|ui| {
                        // Right-clicking a row acts on that row, whatever was
                        // selected before.
                        select_file = Some(f.path.clone());
                        ui.set_min_width(190.0);
                        if ui.button("Abrir").clicked() {
                            open_file = Some(f.path.clone());
                            ui.close_menu();
                        }
                        if ui.button("Mostrar en la carpeta").clicked() {
                            reveal_file = Some(f.path.clone());
                            ui.close_menu();
                        }
                        if ui.button("Copiar ruta").clicked() {
                            copy_path = Some(f.path.display().to_string());
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Renombrar").clicked() {
                            rename_file = Some(f.path.clone());
                            ui.close_menu();
                        }
                        if ui
                            .button(RichText::new("Borrar").color(ERR))
                            .on_hover_text("Pide confirmación antes de borrar")
                            .clicked()
                        {
                            delete_file = Some(f.path.clone());
                            ui.close_menu();
                        }
                    });
                }
            });

            if let Some(p) = select_file {
                self.file_sel = Some(p);
            }
            if let Some(p) = open_file {
                open_in_explorer(&p);
            }
            if let Some(p) = reveal_file {
                reveal_in_explorer(&p);
            }
            if let Some(text) = copy_path {
                ui.output_mut(|o| o.copied_text = text);
                self.status = "Ruta copiada.".to_string();
            }
            if let Some(p) = rename_file {
                let current = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                self.file_sel = Some(p.clone());
                self.file_rename = Some((p, current));
                self.file_delete = None;
            }
            if let Some(p) = delete_file {
                self.file_sel = Some(p.clone());
                self.file_delete = Some(p);
                self.file_rename = None;
            }
            if do_refresh {
                self.refresh_files(true);
            }
        });
    }

    /// Renames a file inside its own folder. Anything that looks like a path
    /// is refused: this box is for a name, not for moving files around.
    fn rename_file(&mut self, from: &Path, wanted: &str) {
        let wanted = wanted.trim();
        if wanted.is_empty() {
            self.status = "El nombre no puede quedar vacío.".to_string();
            return;
        }
        if wanted.contains(['/', '\\', ':']) {
            self.status = "Solo el nombre, sin carpetas.".to_string();
            return;
        }
        let Some(dir) = from.parent() else {
            self.status = "No pude determinar la carpeta.".to_string();
            return;
        };
        let to = dir.join(wanted);
        if to == from {
            self.file_rename = None;
            return;
        }
        if to.exists() {
            self.status = format!("Ya existe {wanted} en esa carpeta.");
            return;
        }
        let selected_source = self.selected.filter(|index| {
            self.lib
                .projects
                .get(*index)
                .map(|project| same_file(from, Path::new(project.opts.map_path.trim())))
                .unwrap_or(false)
        });
        match std::fs::rename(from, &to) {
            Ok(()) => {
                if let Some(index) = selected_source {
                    let new_path = to.display().to_string();
                    self.lib.projects[index].opts.map_path = new_path.clone();
                    if self.active == Some(index) {
                        self.opts.map_path = new_path;
                    }
                    self.dirty_since = Some(Instant::now());
                    self.checks.checked_at = None;
                }
                self.status = format!(
                    "{} renombrado a {wanted}.",
                    from.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                );
                self.file_sel = Some(to);
                self.file_rename = None;
            }
            Err(e) => self.status = format!("No pude renombrar: {e}"),
        }
    }

    fn ui_compile(&mut self, ui: &mut egui::Ui, m: &Metrics) {
        card(ui, "Archivos", "qué compilar y con qué", |ui| {
            row(
                ui,
                m,
                "Mapa (.map)",
                "El archivo fuente que exporta tu editor (J.A.C.K., Hammer). También \
                 puedes arrastrarlo sobre la ventana.\n\n\
                 Los .bsp, .prt y logs se generan al lado de este archivo, salvo que \
                 definas una carpeta de salida.",
                None,
                |ui| {
                    let ok = self.checks.map_ok;
                    path_row(
                        ui,
                        m,
                        &mut self.opts.map_path,
                        Some(ok),
                        "Buscar",
                        || pick_file("Mapa", "map"),
                        false,
                    );
                },
            );

            row(
                ui,
                m,
                "Herramientas",
                "La carpeta con sdHLCSG, sdHLBSP, sdHLVIS y sdHLRAD. Si compilaste \
                 este repo, es la carpeta 'tools'. La GUI la busca sola al arrancar.\n\n\
                 OJO: si la apuntas a la carpeta de otro programa (JACK, Hammer, una \
                 instalación vieja), actualizar la app NO actualiza esos binarios. La app \
                 solo reemplaza el 'tools' que tiene al lado. Si eso pasa te avisamos aquí \
                 abajo.",
                None,
                |ui| {
                    let ok = self.checks.tools_ok;
                    path_row(
                        ui,
                        m,
                        &mut self.opts.tools_dir,
                        Some(ok),
                        "Buscar",
                        pick_dir,
                        false,
                    );
                },
            );

            if let Some(bundled) = self.checks.tools_stale.clone() {
                hint(
                    ui,
                    m,
                    "Estas herramientas son más viejas que las que trae esta versión de la \
                     app. Actualizar la app no toca una carpeta que apunta a otro lado, así \
                     que compilarías con los binarios viejos y sin las opciones nuevas",
                    WARN,
                );
                ui.horizontal(|ui| {
                    ui.add_space(m.label_w + 10.0);
                    if ui.button("Usar las que trae la app").clicked() {
                        self.opts.tools_dir = bundled.display().to_string();
                        self.status = "Ahora se usan las herramientas de la app".to_string();
                    }
                    if ui.button("Copiar las nuevas ahí").clicked() {
                        let to = PathBuf::from(self.opts.tools_dir.trim());
                        self.status = match copy_bundled_tools(&bundled, &to) {
                            Ok(n) => {
                                // Force the check to run again against the files
                                // that are now there.
                                self.checks.tools_key.clear();
                                format!("{n} archivos copiados a {}", to.display())
                            }
                            Err(e) => format!("No pude copiar: {e}"),
                        };
                    }
                });
                ui.add_space(2.0);
            }

            row(
                ui,
                m,
                "Carpeta de salida",
                "Dónde queda el .bsp. Si la dejas vacía, el .bsp se genera junto al .map \
                 (y si 'Carpeta por proyecto' está activada, la basura del compilado va a \
                 una subcarpeta 'intermedios' ahí mismo).\n\n\
                 Si la indicas, se copia el .map ahí y se compila esa copia: el .bsp, el \
                 .prt, los logs y los intermedios (.p0 a .p3) quedan todos en esa carpeta \
                 y tu carpeta de trabajo no se ensucia. El .map original nunca se toca.",
                Some("recomendado"),
                |ui| {
                    let ok = (!self.opts.output_dir.trim().is_empty())
                        .then(|| Path::new(self.opts.output_dir.trim()).is_dir());
                    path_row(
                        ui,
                        m,
                        &mut self.opts.output_dir,
                        ok,
                        "Buscar",
                        pick_dir,
                        true,
                    );
                },
            );

            row(
                ui,
                m,
                "Carpeta de WADs",
                "Dónde tienes tus .wad. Sirve para dos cosas:\n\n\
                 1) Se le pasa a RAD como -waddir para que encuentre las texturas al \
                 calcular la luz.\n\
                 2) Si activas la opción de abajo, la lista de WADs del mapa se reescribe \
                 para apuntar a esta carpeta.",
                None,
                |ui| {
                    let ok = (!self.opts.wad_dir.trim().is_empty())
                        .then(|| Path::new(self.opts.wad_dir.trim()).is_dir());
                    path_row(ui, m, &mut self.opts.wad_dir, ok, "Buscar", pick_dir, true);
                },
            );

            toggle_row(
                ui,
                m,
                "Carpeta por proyecto",
                "Con esto activado, la carpeta de salida no se llena de archivos sueltos: \
                 se crea dentro una carpeta con el nombre del proyecto (o el del mapa si no \
                 hay proyecto), y dentro de esa, una subcarpeta 'intermedios'.\n\n\
                 El compilado corre en 'intermedios', así que TODO lo que ensucian las \
                 herramientas queda ahí: la copia del .map, los logs, el .prt, los .p0 a \
                 .p3, el .lin y el .pts. Al terminar bien, el .bsp se mueve solo un nivel \
                 arriba.\n\n\
                 Resultado: en la carpeta del proyecto ves el .bsp y nada más. Podés \
                 apuntar diez mapas a la misma carpeta de salida sin que se mezclen.\n\n\
                 Sin carpeta de salida también sirve: no se crea carpeta de proyecto (el \
                 .bsp sigue quedando junto al .map, donde lo esperás), pero sí la \
                 subcarpeta 'intermedios' al lado, así tu carpeta de fuentes no se ensucia.",
                Some("recomendado"),
                &mut self.opts.organize_output,
            );

            // Shown with and without an output folder: the layout applies in
            // both cases, and leaving the hint hidden made the toggle look like
            // it did nothing.
            let text = match (self.opts.uses_output_dir(), self.opts.organize_output) {
                (true, true) => self.opts.output_base().map(|base| {
                    let folder = base
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_string();
                    format!(
                        "{folder}\\ para el .bsp, {folder}\\{}\\ para logs e intermedios",
                        options::WORK_SUBDIR
                    )
                }),
                (true, false) => Some("Todo suelto en la carpeta de salida".to_string()),
                (false, true) => Some(format!(
                    "Sin carpeta de salida: el .bsp junto al .map, {}\\ al lado para logs e \
                     intermedios",
                    options::WORK_SUBDIR
                )),
                (false, false) => {
                    Some("Todo suelto junto al .map, mezclado con tus fuentes".to_string())
                }
            };
            if let Some(text) = text {
                hint(
                    ui,
                    m,
                    &text,
                    if self.opts.organize_output { OK } else { MUTED },
                );
            }

            toggle_row(
                ui,
                m,
                "WADs automáticos",
                "Un .map guarda las rutas absolutas de los WADs de la máquina donde se \
                 hizo: 'C:/Users/Otro/...', o peor, rutas sin letra de unidad como \
                 '/Users/Admin/...', que Windows resuelve contra la unidad actual. Si el \
                 mapa vino de otra PC, de otro disco, o simplemente moviste tus WADs, CSG \
                 muere con 'Could not open wad file'.\n\n\
                 Con esto activado se revisa cada entrada de la lista antes de compilar: \
                 las que existen se dejan, las rotas se buscan por nombre de archivo en tu \
                 carpeta de WADs (incluidas subcarpetas), junto al .map y en la carpeta de \
                 herramientas. Siempre se agrega sdhlt.wad.\n\n\
                 Si aun así faltan texturas, se leen las que el mapa usa de verdad y se abre \
                 el índice de cada .wad de tus carpetas para ver cuál las tiene: se cargan \
                 sólo los que aportan algo. Podés tener mil WADs en la carpeta; entran los \
                 que hacen falta y nada más. Tope de 500, dejando margen bajo el límite \
                 de 512 de este fork.\n\n\
                 La lista resultante se le pasa a CSG como -wadcfgfile, que hace que ignore \
                 la clave del mapa. Tu .map no se modifica y no hace falta carpeta de \
                 salida. Lo que no aparezca se avisa en el log en vez de matar el compilado.",
                Some("recomendado"),
                &mut self.opts.auto_wads,
            );

            if self.opts.will_resolve_wads() {
                let msg = if self.opts.wad_dir.trim().is_empty() {
                    "Indica una carpeta de WADs para poder reubicar los que falten"
                } else {
                    "Las rutas rotas se buscan por nombre en esa carpeta"
                };
                hint(ui, m, msg, OK);
            } else {
                hint(
                    ui,
                    m,
                    "CSG usará las rutas del .map tal cual: riesgo de 'Could not open wad file'",
                    WARN,
                );
            }
        });

        card(
            ui,
            "Preset",
            "un punto de partida; después ajusta lo que quieras",
            |ui| {
                ui.horizontal(|ui| {
                    let n = 3.0;
                    let w = ((ui.available_width() - ui.spacing().item_spacing.x * (n - 1.0)) / n)
                        .max(90.0);
                    for p in [Preset::Draft, Preset::Recommended, Preset::Release] {
                        let active = p.is_active(&self.opts);
                        let btn = egui::Button::new(
                            RichText::new(p.label())
                                .color(if active { egui::Color32::WHITE } else { TEXT })
                                .strong(),
                        )
                        .min_size(egui::vec2(w, 30.0))
                        .fill(if active { ACCENT_DEEP } else { CARD_HI })
                        .stroke(egui::Stroke::new(
                            1.0_f32,
                            if active { ACCENT } else { LINE },
                        ));
                        if ui.add(btn).on_hover_text(p.summary()).clicked() {
                            p.apply(&mut self.opts);
                            self.status = format!("Preset '{}' aplicado.", p.label());
                        }
                    }
                });
                ui.add_space(6.0);
                let active_summary = [Preset::Draft, Preset::Recommended, Preset::Release]
                    .into_iter()
                    .find(|preset| preset.is_active(&self.opts))
                    .map(Preset::summary)
                    .unwrap_or(
                        "Configuración personalizada: no coincide exactamente con ningún preset.",
                    );
                ui.label(RichText::new(active_summary).color(MUTED).small());
            },
        );

        card(ui, "Etapas", "puedes saltar las que ya ejecutaste", |ui| {
            ui.horizontal(|ui| {
                let n = 4.0;
                let w = ((ui.available_width() - ui.spacing().item_spacing.x * (n - 1.0)) / n)
                    .max(70.0);
                let stages = [
                    (Stage::Csg, &mut self.opts.run_csg),
                    (Stage::Bsp, &mut self.opts.run_bsp),
                    (Stage::Vis, &mut self.opts.run_vis),
                    (Stage::Rad, &mut self.opts.run_rad),
                ];
                for (stage, flag) in stages {
                    let on = *flag;
                    let btn = egui::Button::new(
                        RichText::new(stage.name())
                            .color(if on { TEXT } else { FAINT })
                            .strong(),
                    )
                    .min_size(egui::vec2(w, 30.0))
                    .fill(if on { ACCENT_DEEP } else { CARD_HI })
                    .stroke(egui::Stroke::new(1.0_f32, if on { ACCENT } else { LINE }));
                    if ui.add(btn).on_hover_text(stage.purpose()).clicked() {
                        *flag = !on;
                    }
                }
            });
            if self.enabled_stages() == 0 {
                hint(
                    ui,
                    m,
                    "Activa al menos una etapa. Una ejecución vacía no produce nada.",
                    ERR,
                );
            }
        });

        card(ui, "Rendimiento y salida", "", |ui| {
            row(
                ui,
                m,
                "Hilos",
                "0 = usar todos los núcleos, que es lo que quieres. Este fork arregló \
                 dos bugs por los que las tools corrían en un solo hilo. Pon un número \
                 solo si quieres dejar CPU libre para otra cosa.",
                Some("0 = automático"),
                |ui| slider_u32(ui, m, &mut self.opts.threads, 0..=64, 0.0),
            );
            toggle_row(
                ui,
                m,
                "Prioridad baja",
                "Corre las herramientas con prioridad baja para que la PC siga usable \
                 mientras compila. Alarga un poco la compilación.",
                None,
                &mut self.opts.low_priority,
            );
            toggle_row(
                ui,
                m,
                "Chart de límites",
                "Al final, cada herramienta imprime cuánto de cada límite del motor \
                 estás usando. Es como te enteras de que vas camino a un límite antes \
                 de chocarlo. Déjalo activado.",
                Some("recomendado"),
                &mut self.opts.chart,
            );
            toggle_row(
                ui,
                m,
                "Salida detallada",
                "Las herramientas imprimen mucho más sobre lo que hacen. Útil cuando \
                 algo falla y no sabes por qué; para uso normal solo ensucia el log.",
                None,
                &mut self.opts.verbose,
            );
        });

        let warnings = self.opts.warnings();
        if !warnings.is_empty() {
            card(ui, "Avisos", "cosas que probablemente no quieras", |ui| {
                for w in &warnings {
                    ui.horizontal_top(|ui| {
                        ui.label(RichText::new("•").color(WARN));
                        ui.label(RichText::new(w).color(WARN).small());
                    });
                }
            });
        }

        // The exact command lines. Useful for scripting a compile, and for
        // understanding what a preset actually changed.
        card(ui, "Línea de comandos", "lo que se va a ejecutar", |ui| {
            ui.checkbox(&mut self.show_command, "Mostrar");
            if self.show_command {
                ui.add_space(4.0);
                let mut text = CompilePlan::preview(&self.opts);
                if text.is_empty() {
                    text = "(ninguna etapa seleccionada)".to_string();
                }
                ui.label(RichText::new(text.trim_end()).monospace().color(MUTED));
                if ui.button("Copiar").clicked() {
                    ui.output_mut(|o| o.copied_text = text);
                }
            }
        });
    }

    fn ui_csg(&mut self, ui: &mut egui::Ui, m: &Metrics) {
        card(
            ui,
            "CSG",
            "recorta brushes y resuelve texturas; centésimas de segundo",
            |ui| {
                toggle_row(
                    ui,
                    m,
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
                    Some("recomendado"),
                    &mut self.opts.nowadtextures,
                );
                toggle_row(
                    ui,
                    m,
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
                    &mut self.opts.nodeterministic,
                );
                toggle_row(
                    ui,
                    m,
                    "Fusionar entidades estáticas",
                    "Junta en una sola entidad las entidades brush que son intercambiables \
                     entre sí. Los brushes no se tocan: misma geometría, mismas texturas, \
                     misma iluminación, mismas colisiones. Lo único que cambia es de qué \
                     entidad cuelgan.\n\n\
                     QUÉ CAMBIA: cada entidad brush cuesta un modelo BSP, y esos modelos \
                     salen de la misma tabla de precache que comparten los modelos de \
                     jugadores, armas y sprites. Un campo de 200 arbustos hechos con \
                     func_illusionary pasa de gastar 200 slots a gastar unos pocos, uno por \
                     zona. Eso se paga por entidad, no por brush: un func_illusionary de 1 \
                     brush y otro de 80 cuestan lo mismo.\n\n\
                     PARA QUE TUS ARBUSTOS SE FUSIONEN, en el editor:\n\
                     · classname func_illusionary (o func_wall)\n\
                     · textura con '{' y rendermode Solid, renderamt 255\n\
                     · SIN targetname, sin target, sin brush de origin\n\
                     · todos con exactamente los mismos keyvalues\n\n\
                     NO tienes que agruparlos a mano en el editor. El compilador los agrupa \
                     por cercanía él solo, y a mano es más fácil colar en el grupo algo que \
                     tenga nombre.\n\n\
                     Basta con que UNA clave difiera (un renderamt distinto, zhlt_noclip \
                     puesto en unos sí y otros no) para que vayan a grupos separados. No es \
                     un error: es la garantía de que la fusión no cambia el comportamiento.\n\n\
                     Para dejar una entidad concreta fuera, ponle zhlt_nomerge 1.\n\n\
                     CÓMO SABER SI FUNCIONÓ: al terminar CSG el log dice 'Merged N static \
                     brush entities into M'. Si dice 0, o no aparece, es que algo las \
                     descalifica; activa 'Verbose' y el log lista cada grupo que sí armó.\n\n\
                     CUÁNDO NO USARLO: si tu mapa tiene pocas entidades brush no ganas nada. \
                     Esto es para mapas con mucha decoración repetida.\n\n\
                     Ver docs/MERGE_DE_ENTIDADES.md.",
                    Some("si usas mucha decoración"),
                    &mut self.opts.mergeentities,
                );
                if self.opts.mergeentities {
                    hint(
                        ui,
                        m,
                        "En el editor: func_illusionary + textura '{' + rendermode Solid + \
                         renderamt 255, sin targetname. Se agrupan solos, no los juntes a mano",
                        OK,
                    );
                    row(
                        ui,
                        m,
                        "Tamaño de grupo",
                        "Hasta cuántas unidades puede medir, en cualquier eje, la caja de un \
                         grupo fusionado. Es el único valor que quizá quieras tocar, y solo \
                         si sabes cómo es tu mapa.\n\n\
                         QUÉ CAMBIA: una entidad brush se descarta con el bounding box de su \
                         modelo: si la caja no está en el PVS del jugador, ni se manda ni se \
                         dibuja. Si se fusionara el mapa entero en una entidad, esa caja \
                         sería del tamaño del mapa y estaría en el PVS desde casi cualquier \
                         lado, así que se dibujaría decoración que no se ve. El límite \
                         mantiene cada grupo dentro de algo parecido a un sector.\n\n\
                         VALORES:\n\
                         · 512 — mapas de pasillos y habitaciones chicas, culling más fino\n\
                         · 1024 — el default, sirve para casi todo\n\
                         · 2048 — mapas abiertos, donde el PVS es malo igual y conviene \
                         ahorrar más slots\n\
                         · 0 — sin límite. No lo uses salvo que te estés quedando sin \
                         modelos y no te importe el rendimiento\n\n\
                         Si tras subirlo ves en r_speeds decoración dibujándose desde lejos, \
                         bájalo.",
                        Some("1024"),
                        |ui| slider_u32(ui, m, &mut self.opts.mergesize, 0..=8192, 256.0),
                    );
                    if self.opts.mergesize == 0 {
                        hint(
                            ui,
                            m,
                            "Sin límite: la caja de un grupo puede abarcar el mapa entero y \
                             quedar siempre en el PVS. Ahorras modelos y pierdes culling",
                            WARN,
                        );
                    } else if self.opts.mergesize >= 2048 {
                        hint(
                            ui,
                            m,
                            "Grupos grandes: solo para mapas abiertos. Mira r_speeds antes de \
                             dejarlo así",
                            WARN,
                        );
                    }
                    toggle_row(
                        ui,
                        m,
                        "Fusionar también los modos mezclados",
                        "Déjalo apagado salvo que sepas exactamente por qué lo enciendes.\n\n\
                         Por defecto solo se fusionan los render modes que el motor NO ordena \
                         por profundidad: Normal y Solid. Solid es el alpha test de las \
                         texturas '{' — el de la vegetación — y recorta el píxel en vez de \
                         mezclarlo, así que no hay nada que ordenar. Por eso el caso de los \
                         arbustos entra sin restricciones y no necesitas esta opción.\n\n\
                         QUÉ CAMBIA: suma los modos que sí se mezclan (Texture, Additive, \
                         Glow, Color). El motor los ordena usando un solo punto por entidad; \
                         si fusionas varias en una, todas pasan a compartir ese punto y se \
                         pueden dibujar en el orden equivocado entre ellas. Un cristal se ve \
                         delante de otro que en realidad está más cerca.\n\n\
                         Solo tiene sentido si tienes muchas entidades mezcladas juntas en \
                         una zona donde no se superponen visualmente entre sí. Y hay que \
                         verlo en el juego, porque el compilador no puede avisarte de esto.",
                        Some("déjalo apagado"),
                        &mut self.opts.mergeblend,
                    );
                    if self.opts.mergeblend {
                        hint(
                            ui,
                            m,
                            "Los cristales y efectos aditivos fusionados pueden dibujarse en \
                             el orden equivocado entre sí. Compruébalo en el juego",
                            WARN,
                        );
                    }
                }
                toggle_row(
                    ui,
                    m,
                    "Informe de coste de texturas",
                    "No cambia el .bsp. Solo añade un informe al final de CSG diciendo qué \
                     texturas lo están engordando.\n\n\
                     PARA QUÉ: si compilas con 'Meter las texturas en el BSP', el lump de \
                     texturas suele ser la mayor parte del archivo. Medido en mapas reales: \
                     82.8% y 69.9% del .bsp, contra 8.7% y 15.4% de la iluminación. El chart \
                     normal te da un único total y ahí se acaba; este te dice de qué está \
                     hecho.\n\n\
                     QUÉ MIRAR, la columna 'oversampled': cuántos píxeles tiene la textura \
                     por cada píxel que llega a mostrarse. Se calcula con la superficie real \
                     que pinta en el mapa y la escala de textura, no es una estimación.\n\
                     · 4x o más — le sobra resolución. Puedes bajarla a la mitad en cada eje \
                     y seguir teniendo un píxel por píxel en pantalla\n\
                     · alrededor de 1x — está usada a su resolución nativa, no la toques\n\
                     · por debajo de 1x — se repite sobre una superficie grande. Bajarla se \
                     va a notar\n\n\
                     Esa columna es el motivo de la opción. Mirando solo el tamaño parece que \
                     bajar todas las texturas de 256px ahorraría muchísimo, y en un mapa real \
                     medido resultó que no sobraba resolución en ninguna: eran texturas que \
                     se repiten sobre paredes enormes y bajarlas solo se habría visto peor.\n\n\
                     También avisa si dos texturas tienen los píxeles idénticos, que es peso \
                     pagado dos veces.\n\n\
                     CUÁNDO USARLO: cuando el .bsp pese más de lo que te gustaría. No hace \
                     falta dejarlo puesto en cada compilación.",
                    Some("cuando quieras adelgazar el mapa"),
                    &mut self.opts.texchart,
                );
                if self.opts.texchart {
                    hint(
                        ui,
                        m,
                        "Mira la columna 'oversampled': 4x o más significa que puedes bajar \
                         esa textura a la mitad sin que se note. Cerca de 1x, déjala",
                        OK,
                    );
                }
                row(
                    ui,
                    m,
                    "Extender el mundo",
                    "Sube el límite de geometría más allá de +/-32768 unidades. Solo si tu \
                     mapa realmente excede el tamaño estándar; 0 deja el valor por defecto.",
                    Some("0 = normal"),
                    |ui| slider_u32(ui, m, &mut self.opts.worldextent, 0..=65536, 1024.0),
                );
            },
        );
        card(
            ui,
            "Brushes deformados",
            "vertex manipulation de J.A.C.K. y Hammer",
            |ui| {
                toggle_row(
                    ui,
                    m,
                    "Reconstruir brushes no planos",
                    "Cuando mueves vértices en el editor, una cara puede dejar de ser plana: \
                     el editor la dibuja con sus vértices, pero el compilador solo guarda 3 \
                     puntos por cara y arma el plano con ellos. El sólido que compila no es \
                     el que ves, y en el juego aparecen caras invisibles, rendijas o caras \
                     con NULL.\n\n\
                     QUÉ CAMBIA: CSG detecta esos brushes y los rehace como el sólido convexo \
                     de sus vértices, que es lo que el editor muestra. No tienes que tocar \
                     nada en el editor. Las colisiones se siguen armando con los planos \
                     originales, así que no suben los clipnodes.\n\n\
                     Déjalo activado. Apagarlo solo sirve para comparar con el \
                     comportamiento de antes.",
                    Some("recomendado"),
                    &mut self.opts.convexfix,
                );
                if self.opts.convexfix {
                    row(
                        ui,
                        m,
                        "Diferencia mínima",
                        "Cuántas unidades tiene que separarse el sólido del editor del sólido \
                         de los planos para que el brush se reconstruya.\n\n\
                         QUÉ CAMBIA: la mayoría de los brushes deformados difieren en unas \
                         centésimas, lo que no abre ninguna rendija visible. Reconstruirlos \
                         igual agrega caras: en ze_elysium, rehacer los 1081 candidatos subió \
                         un 17% las caras que ve cada hoja; con 0.2 se rehacen 48, cuesta un \
                         2% y las rendijas de zpa_house siguen cerradas.\n\n\
                         Bájalo solo si todavía ves una rendija fina en el juego.",
                        Some("0.2"),
                        |ui| {
                            ui.spacing_mut().slider_width = (m.ctrl_w - 78.0).max(90.0);
                            ui.add(
                                egui::Slider::new(&mut self.opts.convexgap, 0.01..=2.0)
                                    .max_decimals(2),
                            );
                        },
                    );
                } else {
                    hint(
                        ui,
                        m,
                        "Los sólidos deformados pueden dejar caras invisibles o rendijas en el \
                         juego",
                        WARN,
                    );
                }
            },
        );
        self.ui_extra(ui, "CSG");
    }

    fn ui_bsp(&mut self, ui: &mut egui::Ui, m: &Metrics) {
        card(
            ui,
            "BSP",
            "construye el árbol, fusiona coplanares y subdivide caras",
            |ui| {
                row(
                    ui,
                    m,
                    "Subdivide",
                    "Tamaño máximo de una cara antes de partirla. 240 NO es un número \
                     conservador: es el techo que impone MAX_SURFACE_EXTENT. Si lo subes, el \
                     mapa deja de cargar en el software renderer y en el HLDS. Bajarlo genera \
                     más caras y baja los FPS. Déjalo en 240.",
                    Some("240, no tocar"),
                    |ui| slider_u32(ui, m, &mut self.opts.subdivide, 64..=512, 0.0),
                );
                row(
                    ui,
                    m,
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
                    |ui| slider_u32(ui, m, &mut self.opts.maxnodesize, 64..=4096, 0.0),
                );
                toggle_row(
                    ui,
                    m,
                    "Optimizar atlas de luz",
                    "Activa -lmoptimize. Reordena las caras para desperdiciar menos espacio \
                     en las páginas de lightmap. Puede reducir páginas ocupadas sin cambiar \
                     la geometría ni la calidad de iluminación.",
                    Some("-lmoptimize"),
                    &mut self.opts.lmoptimize,
                );
            },
        );

        card(
            ui,
            "Atajos de prueba",
            "no los uses para un compilado final",
            |ui| {
                toggle_row(
                    ui,
                    m,
                    "Solo buscar leaks",
                    "Corta BSP en cuanto termina de buscar leaks. Sirve para saber en segundos \
                 si el mapa está sellado, pero no produce un mapa jugable.",
                    None,
                    &mut self.opts.leakonly,
                );
                toggle_row(
                    ui,
                    m,
                    "Encontrar todos los leaks",
                    "Activa -allleaks. Si el mapa está abierto, BSP continúa el relevamiento y \
                 marca todos los agujeros que encuentre en vez de detenerse en el primero. \
                 Útil para corregir varias fugas en una sola pasada.",
                    None,
                    &mut self.opts.allleaks,
                );
                toggle_row(
                    ui,
                    m,
                    "Sin t-junctions",
                    "Saltea el arreglo de t-junctions. Más rápido, pero vas a ver grietas de \
                 luz entre caras. Solo para pruebas.",
                    None,
                    &mut self.opts.notjunc,
                );
                toggle_row(
                    ui,
                    m,
                    "Sin clipping hull",
                    "No genera la geometría de colisión. El mapa carga pero el jugador \
                 atraviesa las paredes. Solo para mirar geometría.",
                    None,
                    &mut self.opts.noclip,
                );
            },
        );

        self.ui_extra(ui, "BSP");
    }

    fn ui_vis(&mut self, ui: &mut egui::Ui, m: &Metrics) {
        card(
            ui,
            "VIS",
            "calcula qué se ve desde dónde: define los FPS del mapa",
            |ui| {
                row(
                    ui,
                    m,
                    "Calidad",
                    "El PVS que calcula VIS decide cuánto le pide el mapa al motor en cada \
                     frame. Cuanto más ajustado, menos wpoly y más FPS.",
                    Some("full para publicar"),
                    |ui| {
                        egui::ComboBox::from_id_source("vis_q")
                            .width(m.ctrl_w)
                            .selected_text(self.opts.vis_quality.label())
                            .show_ui(ui, |ui| {
                                for q in [VisQuality::Fast, VisQuality::Normal, VisQuality::Full] {
                                    ui.selectable_value(&mut self.opts.vis_quality, q, q.label())
                                        .on_hover_text(q.help());
                                }
                            });
                    },
                );
                ui.add_space(2.0);
                ui.label(
                    RichText::new(self.opts.vis_quality.help())
                        .color(MUTED)
                        .small(),
                );
                ui.add_space(6.0);
                row(
                    ui,
                    m,
                    "Distancia máxima",
                    "Recorta la visibilidad más allá de N unidades. Puede ayudar en mapas \
                     enormes y abiertos, pero mal usado produce geometría que aparece de \
                     golpe. 0 lo desactiva.",
                    Some("0 = sin límite"),
                    |ui| slider_u32(ui, m, &mut self.opts.maxdistance, 0..=8192, 64.0),
                );
            },
        );
        self.ui_extra(ui, "VIS");
    }

    fn ui_rad(&mut self, ui: &mut egui::Ui, m: &Metrics) {
        card(
            ui,
            "RAD",
            "la iluminación: ~95% del tiempo de compilación",
            |ui| {
                ui.label(
                    RichText::new(
                        "RAD no cambia la geometría que dibuja el motor: subir la calidad \
                         de luz cuesta tiempo de compilación, no FPS en juego.",
                    )
                    .color(OK)
                    .small(),
                );
                ui.add_space(8.0);

                row(
                    ui,
                    m,
                    "Nivel de cielo",
                    "Cuántos rayos lanza cada muestra hacia el cielo: nivel 4 = 258 rayos, \
                     5 = 1026, 6 = 4098, 7 = 16386. Medido, este loop es el 96% de todos los \
                     rayos de RAD. El nivel 6 usa 4x menos rayos que el 7 con una diferencia \
                     máxima de 1/255 por luxel, o sea invisible. Por eso el default de este \
                     fork es 6 y no 7.",
                    Some("6"),
                    |ui| slider_u32(ui, m, &mut self.opts.skylevel, 4..=8, 0.0),
                );
                toggle_row(
                    ui,
                    m,
                    "Cielo suave",
                    "Ilumina cada punto con todo el domo del cielo en lugar de una sola \
                     dirección.\n\n\
                     QUÉ CAMBIA: con esto los bordes de sombra en exteriores son graduales, \
                     como en la realidad, y las zonas en sombra reciben algo de luz azulada \
                     del cielo. Sin esto el sol es una sola dirección y las sombras quedan \
                     duras y de borde nítido, con aspecto de mapa antiguo.\n\n\
                     Apagarlo fuerza el nivel de cielo a 4 (258 rayos) y acelera bastante, \
                     pero la diferencia visual en exteriores es evidente.",
                    Some("recomendado"),
                    &mut self.opts.softsky,
                );
                toggle_row(
                    ui,
                    m,
                    "Oversampling (-extra)",
                    "Toma 9 muestras de luz por cada luxel en vez de 1, y promedia.\n\n\
                     QUÉ CAMBIA: es la mejora de calidad más visible de RAD. Sin esto los \
                     bordes de sombra salen escalonados, con el típico dentado de lightmap de \
                     16 unidades; con esto quedan suaves. También desaparecen los puntos de \
                     luz mal calculados en esquinas y superficies inclinadas.\n\n\
                     Cuesta tiempo de compilación y CERO FPS en juego: el lightmap resultante \
                     tiene exactamente el mismo tamaño.",
                    Some("recomendado"),
                    &mut self.opts.extra_sampling,
                );
                row(
                    ui,
                    m,
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
                    |ui| slider_u32(ui, m, &mut self.opts.bounce, 0..=32, 0.0),
                );
                toggle_row(
                    ui,
                    m,
                    "RAD rápido",
                    "Modo borrador de la iluminación.\n\n\
                     QUÉ CAMBIA: baja la calidad del muestreo y simplifica el cálculo de \
                     rebotes. El resultado sirve para ver que las luces están donde \
                     corresponde y que no quedó nada a oscuras, pero los degradados salen \
                     sucios y las sombras imprecisas.\n\n\
                     Útil mientras construyes; nunca para la versión que publicas.",
                    None,
                    &mut self.opts.rad_fast,
                );
            },
        );

        card(ui, "Detalle y suavizado", "", |ui| {
            row(
                ui,
                m,
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
                |ui| slider_f32(ui, m, &mut self.opts.chop, 16.0..=128.0),
            );
            row(
                ui,
                m,
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
                |ui| slider_f32(ui, m, &mut self.opts.texchop, 8.0..=128.0),
            );
            row(
                ui,
                m,
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
                |ui| slider_f32(ui, m, &mut self.opts.smooth, 0.0..=180.0),
            );
        });

        self.ui_rad_shading(ui, m);

        card(ui, "Avanzado", "", |ui| {
            toggle_row(
                ui,
                m,
                "Aceleración GPU",
                "Activa el backend Vulkan de RAD para el gather directo y las transferencias \
                 compatibles. Si una fase o entidad no está soportada, RAD informa el motivo \
                 y vuelve a CPU. En mapas con pocas luces la GPU puede no ser más rápida; \
                 compara el perfil antes de dejarla fija.",
                Some("Vulkan"),
                &mut self.opts.gpu,
            );
            if self.opts.gpu {
                toggle_row(
                    ui,
                    m,
                    "Selección automática",
                    "RAD mide cada fase antes de abrir Vulkan. Usa CPU en trabajos pequeños, \
                     donde copiar datos cuesta más que calcularlos, y GPU al superar el umbral.",
                    Some("recomendado"),
                    &mut self.opts.gpu_auto,
                );
                toggle_row(
                    ui,
                    m,
                    "Gather directo",
                    "Permite que Vulkan calcule las luces directas compatibles. Entidades o \
                     modelos no soportados vuelven a CPU sin cambiar el resultado.",
                    Some("-gpu-gather"),
                    &mut self.opts.gpu_gather,
                );
                toggle_row(
                    ui,
                    m,
                    "Transferencias Sparse",
                    "Permite que Vulkan calcule los factores entre parches de la matriz Sparse. \
                     Los rebotes consumen luego esos mismos factores con la ruta de referencia.",
                    Some("-gpu-transfers"),
                    &mut self.opts.gpu_transfers,
                );
                row(
                    ui,
                    m,
                    "Adaptador GPU",
                    "Índice del dispositivo Vulkan que RAD debe usar. -1 elige \
                     automáticamente; usa un número solo si tienes varias GPU y sabes el \
                     índice que imprime RAD.",
                    Some("-1 = automático"),
                    |ui| {
                        ui.spacing_mut().slider_width = (m.ctrl_w - 78.0).max(90.0);
                        ui.add(egui::Slider::new(&mut self.opts.gpu_adapter, -1..=15));
                    },
                );
            }
            row(
                ui,
                m,
                "Vismatrix",
                "Cómo guarda RAD la visibilidad entre parches. Es una decisión de \
                 memoria, no de calidad.",
                Some("sparse"),
                |ui| {
                    egui::ComboBox::from_id_source("vismat")
                        .width(m.ctrl_w)
                        .selected_text(self.opts.vismatrix.label())
                        .show_ui(ui, |ui| {
                            for mm in [VisMatrix::Normal, VisMatrix::Sparse, VisMatrix::Off] {
                                ui.selectable_value(&mut self.opts.vismatrix, mm, mm.label())
                                    .on_hover_text(mm.help());
                            }
                        });
                },
            );
            toggle_row(
                ui,
                m,
                "Motor pre-25 aniv.",
                "Baja el umbral de recorte de luz de 255 a 188.\n\n\
                 QUÉ CAMBIA: limita antes las zonas muy brillantes para conservar el \
                 comportamiento esperado por motores anteriores al aniversario 25. Viene \
                 activado para mantener el default histórico de ReSDHLT.\n\n\
                 No todos los clientes y forks renderizan el límite igual. Antes de \
                 publicar, compara una zona brillante con el cliente y servidor reales de \
                 tu comunidad; desactívalo si tu objetivo confirmado es el motor nuevo.",
                Some("compatibilidad"),
                &mut self.opts.pre25,
            );
            toggle_row(
                ui,
                m,
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
                &mut self.opts.nostudioshadow,
            );
            toggle_row(
                ui,
                m,
                "Perfilar RAD",
                "Imprime al final en qué gasta RAD el tiempo, con conteos de llamadas. \
                 Sirve para entender un compilado lento, no para uso normal.",
                None,
                &mut self.opts.profile,
            );
        });

        self.ui_extra(ui, "RAD");
    }

    fn ui_rad_shading(&mut self, ui: &mut egui::Ui, m: &Metrics) {
        card(
            ui,
            "Sombras y oclusión",
            "todo apagado por defecto; sin tocarlo el mapa sale igual",
            |ui| {
                row(
                    ui,
                    m,
                    "Oclusión ambiental",
                    "Oscurece la luz en rincones, bajo repisas y junto a objetos, donde en la \
                     realidad llega menos luz de alrededor. Cada muestra de luz lanza rayos \
                     en media esfera y se oscurece según cuántos chocan con geometría cerca.\n\n\
                     QUÉ MODO USAR: si tu mapa se ilumina con light, light_spot y \
                     light_environment, 'solo luz directa'. Si se ilumina con texlights, \
                     'toda la luz': el otro modo no toca la luz de las texlights y casi no se \
                     notaría.\n\n\
                     Cuesta tiempo de compilación y cero FPS.",
                    Some("apagada"),
                    |ui| {
                        egui::ComboBox::from_id_source("ao_mode")
                            .width(m.ctrl_w)
                            .selected_text(self.opts.ao.label())
                            .show_ui(ui, |ui| {
                                for mode in [AoMode::Off, AoMode::Direct, AoMode::All] {
                                    ui.selectable_value(&mut self.opts.ao, mode, mode.label())
                                        .on_hover_text(mode.help());
                                }
                            });
                    },
                );
                if self.opts.ao != AoMode::Off {
                    hint(ui, m, self.opts.ao.help(), MUTED);
                    row(
                        ui,
                        m,
                        "Alcance",
                        "Hasta cuántas unidades busca geometría cada rayo. Más alcance \
                         oscurece zonas más amplias alrededor de cada objeto; poco alcance \
                         marca solo las esquinas.",
                        Some("32"),
                        |ui| slider_f32(ui, m, &mut self.opts.ao_scale, 1.0..=1024.0),
                    );
                    row(
                        ui,
                        m,
                        "Intensidad",
                        "Cuánto oscurece la oclusión: 1 es el efecto completo, 0 no hace \
                         nada. Bájalo si los rincones quedan demasiado negros.",
                        Some("1"),
                        |ui| slider_f32(ui, m, &mut self.opts.ao_opacity, 0.0..=1.0),
                    );
                    row(
                        ui,
                        m,
                        "Caída",
                        "Cómo pesa la distancia del choque. 1 es lineal. Más alto deja \
                         solo el oscurecido pegado a la geometría; más bajo lo extiende.",
                        Some("1"),
                        |ui| slider_f32(ui, m, &mut self.opts.ao_gain, 0.125..=8.0),
                    );
                    row(
                        ui,
                        m,
                        "Rayos por muestra",
                        "1 = 6 rayos, 2 = 18, 3 = 66, 4 = 258. Más rayos dan una oclusión \
                         más suave y cuestan más tiempo. 3 alcanza para casi todo.",
                        Some("3"),
                        |ui| slider_u32(ui, m, &mut self.opts.ao_level, 1..=6, 0.0),
                    );
                    row(
                        ui,
                        m,
                        "Descartar rayos débiles",
                        "Salta los rayos que casi no aportan (los muy rasantes). Ahorra \
                         tiempo sin cambio visible. 0 los traza todos.",
                        Some("0.045"),
                        |ui| {
                            ui.spacing_mut().slider_width = (m.ctrl_w - 78.0).max(90.0);
                            ui.add(
                                egui::Slider::new(&mut self.opts.ao_minweight, 0.0..=0.1)
                                    .max_decimals(3),
                            );
                        },
                    );
                    row(
                        ui,
                        m,
                        "Color",
                        "Hacia qué color tiñe la oclusión. Negro es lo normal; un tono \
                         oscuro frío o cálido puede darle carácter a un mapa.",
                        Some("negro"),
                        |ui| {
                            egui::color_picker::color_edit_button_srgb(ui, &mut self.opts.ao_color);
                            let [r, g, b] = self.opts.ao_color;
                            ui.label(RichText::new(format!("{r} {g} {b}")).color(MUTED).small());
                        },
                    );
                }

                row(
                    ui,
                    m,
                    "Sombras suaves",
                    "Cuántos rayos de sombra por eje traza cada muestra hacia cada luz: 1 es \
                     un rayo (sombra dura, lo de siempre), 3 son 9 rayos repartidos en el \
                     tamaño de un texel de lightmap.\n\n\
                     QUÉ CAMBIA: el borde de las sombras pasa de una escalera de texels a un \
                     degradado corto. Afecta a light, light_spot y light_environment; las \
                     texlights ya dan sombras blandas.\n\n\
                     COSTE: en zm_eichen_v2, 3 llevó RAD de 2.5 a 3.6 s. Calcula la luz \
                     directa en la CPU aunque uses la GPU.",
                    Some("1 = apagado"),
                    |ui| slider_u32(ui, m, &mut self.opts.pcf, 1..=8, 0.0),
                );
                if self.opts.pcf > 1 {
                    hint(
                        ui,
                        m,
                        &format!(
                            "{} rayos de sombra por muestra y luz. 3 es el valor útil; más \
                             casi no se distingue y cuesta más",
                            self.opts.pcf * self.opts.pcf
                        ),
                        OK,
                    );
                }
                row(
                    ui,
                    m,
                    "Frenar la luz que se filtra",
                    "RAD suaviza cada luxel promediándolo con sus vecinos. En el borde de \
                     una sombra fina o al pie de una pared, esos vecinos iluminados meten luz \
                     donde no debería haber.\n\n\
                     QUÉ CAMBIA: un vecino más brillante que el luxel pesa menos en el \
                     promedio, hasta 1 - N de su peso. 0 lo apaga, 0.5 es un buen punto de \
                     partida, 1 no deja entrar a ningún vecino más brillante.\n\n\
                     Actúa sobre la luz directa de light, light_spot y light_environment. \
                     Un mapa iluminado casi solo con texlights no cambia.",
                    Some("0 = apagado"),
                    |ui| slider_f32(ui, m, &mut self.opts.blurclamp, 0.0..=1.0),
                );
            },
        );
    }

    fn ui_analysis(&mut self, ui: &mut egui::Ui, m: &Metrics) {
        let running = self.analysis_job.is_some();
        let prefs_before = self.lib.analysis.clone();
        card(
            ui,
            "Analizar el mapa compilado",
            "busca agujeros, zonas caras en FPS y caras que se dibujan de más",
            |ui| {
                let target = self.analysis_target();
                row(
                    ui,
                    m,
                    "Archivo .bsp",
                    "Vacío analiza el .bsp que dejó la última compilación de este proyecto. \
                     Elige otro para revisar cualquier mapa, aunque no lo hayas compilado tú.",
                    None,
                    |ui| {
                        let state = target.as_ref().map(|p| p.is_file());
                        path_row(
                            ui,
                            m,
                            &mut self.analysis_bsp,
                            state,
                            "Buscar",
                            || pick_file("Mapa compilado", "bsp"),
                            true,
                        );
                    },
                );
                if self.analysis_bsp.trim().is_empty() {
                    let text = match &target {
                        Some(p) => format!("Último compilado: {}", p.display()),
                        None => "Todavía no hay un .bsp compilado para este proyecto".to_string(),
                    };
                    hint(ui, m, &text, MUTED);
                }

                let prefs = &mut self.lib.analysis;
                toggle_row(
                    ui,
                    m,
                    "Agujeros",
                    "Lanza rayos por el mapa compilado. Donde uno entra en una pared y ninguna \
                     cara dibujada lo tapa, el jugador vería a través: es el problema de las \
                     caras invisibles de los brushes deformados.\n\n\
                     Si el .map del proyecto es el de este .bsp, los rayos apuntan a cada cara \
                     visible del .map, así que no se escapa ninguna.",
                    Some("recomendado"),
                    &mut prefs.holes,
                );
                if prefs.holes {
                    row(
                        ui,
                        m,
                        "Rayos",
                        "Más rayos revisan más superficie y tardan más. 20000 alcanza para \
                         un mapa normal; súbelo en mapas enormes.",
                        Some("20000"),
                        |ui| slider_u32(ui, m, &mut prefs.rays, 1000..=200000, 1000.0),
                    );
                }
                toggle_row(
                    ui,
                    m,
                    "wpoly por zona",
                    "Cuenta, para cada hoja del mapa, cuántas caras del mundo le manda el PVS \
                     al motor. Eso es lo que sigue el wpoly y lo que baja los FPS. Lista las \
                     zonas peores para que sepas dónde poner HINT o cortar una visual.",
                    Some("FPS"),
                    &mut prefs.wpoly,
                );
                if prefs.wpoly {
                    row(
                        ui,
                        m,
                        "Radio de zona",
                        "Hojas más cerca que esto se juntan en una sola zona de la lista.",
                        Some("256"),
                        |ui| slider_f32(ui, m, &mut prefs.radius, 64.0..=2048.0),
                    );
                }
                toggle_row(
                    ui,
                    m,
                    "Caras ocultas",
                    "Caras que se dibujan y se iluminan sin que nadie pueda verlas: paredes \
                     del mundo tapadas del todo por un func_wall o func_illusionary, y caras \
                     de entidades metidas dentro de paredes. Con NULL o func_detail se van.",
                    Some("FPS"),
                    &mut prefs.hidden,
                );
                toggle_row(
                    ui,
                    m,
                    "Geometría",
                    "Revisa que cada cara sea plana, convexa y sin vértices repetidos, y \
                     cuenta los pares de caras que todavía se podrían fusionar. Un error acá \
                     es del compilador, no tuyo.",
                    None,
                    &mut prefs.geometry,
                );
                toggle_row(
                    ui,
                    m,
                    "Analizar al compilar",
                    "Corre el análisis solo cada vez que una compilación termina bien. Tarda \
                     unos segundos y avisa en esta pestaña.",
                    Some("recomendado"),
                    &mut prefs.after_compile,
                );

                let any = prefs.holes || prefs.wpoly || prefs.hidden || prefs.geometry;
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if running {
                        if ui.button("Cancelar").clicked() {
                            if let Some(job) = &self.analysis_job {
                                job.cancel();
                            }
                        }
                        ui.spinner();
                    } else if ui
                        .add_enabled(
                            any && target.as_ref().is_some_and(|p| p.is_file()),
                            egui::Button::new(RichText::new("Analizar").strong()),
                        )
                        .clicked()
                    {
                        self.start_analysis();
                    }
                    ui.label(RichText::new(&self.analysis_status).color(MUTED).small());
                });
            },
        );
        // The prefs are global; persist them right away like the update switch.
        if self.lib.analysis != prefs_before {
            self.save_library();
        }

        let Some(report) = self.analysis_report.clone() else {
            return;
        };
        card(ui, "Resultado", "", |ui| {
            ui.label(RichText::new(report.bsp.display().to_string()).color(TEXT));
            ui.label(RichText::new(&report.stats).color(MUTED).small());
        });
        for section in &report.sections {
            self.ui_analysis_section(ui, &report, section);
        }
    }

    fn ui_analysis_section(
        &mut self,
        ui: &mut egui::Ui,
        report: &analysis::Report,
        section: &analysis::Section,
    ) {
        let (tag, color) = match section.level {
            analysis::Level::Ok => ("bien", OK),
            analysis::Level::Info => ("info", ACCENT),
            analysis::Level::Warn => ("revisar", WARN),
            analysis::Level::Error => ("problema", ERR),
        };
        card(ui, section.title, "", |ui| {
            ui.horizontal(|ui| {
                chip(ui, tag, color);
                ui.add(egui::Label::new(RichText::new(&section.summary).color(TEXT)).wrap());
            });
            if !section.advice.is_empty() && section.level != analysis::Level::Ok {
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(section.advice).color(MUTED).small()).wrap());
            }
            if section.findings.is_empty() {
                return;
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let dir = report.bsp.parent().map(|p| p.to_path_buf());
                let stem = report
                    .bsp
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "mapa".into());
                if ui
                    .button("Guardar pointfile")
                    .on_hover_text(format!(
                        "Escribe {stem}_{}.pts junto al .bsp. En J.A.C.K. o Hammer: Map > \
                         Load Pointfile y elige ese archivo. Cada estrella marca un punto de \
                         la lista.",
                        section.key
                    ))
                    .clicked()
                {
                    if let Some(dir) = &dir {
                        let path = dir.join(format!("{stem}_{}.pts", section.key));
                        self.analysis_status =
                            match analysis::write_pointfile(&path, &section.findings) {
                                Ok(()) => {
                                    reveal_in_explorer(&path);
                                    format!("Pointfile guardado: {}", path.display())
                                }
                                Err(e) => format!("No pude escribir {}: {e}", path.display()),
                            };
                    }
                }
                let map = PathBuf::from(self.opts.map_path.trim());
                let map_pts = map.with_extension("pts");
                if map.file_stem() == report.bsp.file_stem()
                    && map.parent().is_some_and(|p| p.is_dir())
                    && ui
                        .button(format!(
                            "Como {}",
                            map_pts.file_name().unwrap_or_default().to_string_lossy()
                        ))
                        .on_hover_text(
                            "Lo guarda con el nombre que el editor abre directo con Load \
                             Pointfile, junto al .map. Reemplaza el rastro del último leak.",
                        )
                        .clicked()
                {
                    self.analysis_status =
                        match analysis::write_pointfile(&map_pts, &section.findings) {
                            Ok(()) => format!("Pointfile guardado: {}", map_pts.display()),
                            Err(e) => format!("No pude escribir {}: {e}", map_pts.display()),
                        };
                }
                ui.label(
                    RichText::new(format!("{} puntos", section.findings.len()))
                        .color(MUTED)
                        .small(),
                );
            });
            ui.add_space(4.0);
            const SHOWN: usize = 300;
            egui::ScrollArea::vertical()
                .id_source(("analysis", section.key))
                .max_height(260.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    egui::Grid::new(("analysis_grid", section.key))
                        .num_columns(3)
                        .striped(true)
                        .spacing([12.0, 4.0])
                        .show(ui, |ui| {
                            for f in section.findings.iter().take(SHOWN) {
                                ui.label(RichText::new(&f.text).color(TEXT).small());
                                let coords =
                                    format!("{:.0} {:.0} {:.0}", f.pos[0], f.pos[1], f.pos[2]);
                                ui.label(RichText::new(&coords).color(MUTED).small().monospace());
                                if ui
                                    .small_button("Copiar")
                                    .on_hover_text(
                                        "Copia la coordenada. En el juego, con sv_cheats 1: \
                                         setpos seguido de la coordenada.",
                                    )
                                    .clicked()
                                {
                                    ui.output_mut(|o| o.copied_text = coords.clone());
                                    self.analysis_status = format!("Copiado: {coords}");
                                }
                                ui.end_row();
                            }
                        });
                    if section.findings.len() > SHOWN {
                        ui.label(
                            RichText::new(format!(
                                "y {} más; el pointfile los incluye todos",
                                section.findings.len() - SHOWN
                            ))
                            .color(MUTED)
                            .small(),
                        );
                    }
                });
        });
    }

    fn ui_extra(&mut self, ui: &mut egui::Ui, which: &str) {
        let field = match which {
            "CSG" => &mut self.opts.csg_extra,
            "BSP" => &mut self.opts.bsp_extra,
            "VIS" => &mut self.opts.vis_extra,
            _ => &mut self.opts.rad_extra,
        };
        card(ui, &format!("Parámetros extra para {which}"), "", |ui| {
            ui.add(
                egui::TextEdit::singleline(field)
                    .hint_text("admite comillas: -lights \"C:\\Mis mapas\\x.rad\"")
                    .desired_width(f32::INFINITY),
            )
            .on_hover_text(
                "Para flags que no están en la interfaz. Se agregan al final de la línea \
                 de comandos. Las comillas agrupan rutas con espacios; una comilla sin \
                 cerrar bloquea la compilación y muestra el error.",
            );
            if let Some(error) = options::parse_extra_args(field).err() {
                ui.label(RichText::new(error).color(ERR).small());
            }
        });
    }

    fn ui_advice(&mut self, ui: &mut egui::Ui) {
        card(
            ui,
            "Lo que deberías hacer siempre",
            "medido sobre mapas reales; números en docs/BENCHMARKS.md",
            |ui| {
                for rule in always_rules() {
                    egui::Frame::none()
                        .fill(CARD_HI)
                        .rounding(ROUND)
                        .stroke(egui::Stroke::new(1.0_f32, LINE))
                        .inner_margin(egui::Margin::same(11.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new(rule.title).color(ACCENT).strong());
                            ui.add_space(3.0);
                            ui.label(RichText::new(rule.body).color(TEXT).small());
                        });
                    ui.add_space(8.0);
                }
            },
        );
    }

    // ---------------- log panel ----------------

    fn ui_log(&mut self, ui: &mut egui::Ui) {
        let errors = self
            .log
            .iter()
            .filter(|(k, _)| *k == LineKind::Error)
            .count();
        let warns = self
            .log
            .iter()
            .filter(|(k, _)| *k == LineKind::Warning)
            .count();

        ui.horizontal(|ui| {
            ui.label(RichText::new("Salida").color(TEXT).strong().size(14.5));
            if errors > 0 {
                chip(ui, &format!("{errors} errores"), ERR);
            }
            if warns > 0 {
                chip(ui, &format!("{warns} avisos"), WARN);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Limpiar").clicked() {
                    self.log.clear();
                }
                if ui.small_button("Copiar").clicked() {
                    let all: String = self
                        .log
                        .iter()
                        .map(|(_, t)| t.as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    ui.output_mut(|o| o.copied_text = all);
                }
                if ui.small_button("Guardar").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_file_name("compile.log")
                        .save_file()
                    {
                        let all: String = self
                            .log
                            .iter()
                            .map(|(_, t)| t.as_str())
                            .collect::<Vec<_>>()
                            .join("\n");
                        self.status = match std::fs::write(&path, all) {
                            Ok(()) => format!("Log guardado en {}", path.display()),
                            Err(e) => format!("No pude guardar el log: {e}"),
                        };
                    }
                }
            });
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 130.0).max(80.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.log_filter)
                    .desired_width(w)
                    .hint_text("filtrar..."),
            );
            ui.checkbox(&mut self.only_problems, "Solo problemas")
                .on_hover_text("Muestra únicamente errores y avisos.");
        });
        ui.add_space(6.0);

        let needle = self.log_filter.to_ascii_lowercase();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                let mut shown = 0usize;
                for (kind, text) in &self.log {
                    if self.only_problems && !matches!(kind, LineKind::Error | LineKind::Warning) {
                        continue;
                    }
                    if !needle.is_empty() && !text.to_ascii_lowercase().contains(&needle) {
                        continue;
                    }
                    let color = match kind {
                        LineKind::Normal => TEXT,
                        LineKind::Command => FAINT,
                        LineKind::Warning => WARN,
                        LineKind::Error => ERR,
                        LineKind::Success => OK,
                    };
                    ui.label(RichText::new(text).color(color).monospace().size(11.5));
                    shown += 1;
                }
                if shown == 0 {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(if self.log.is_empty() {
                            "La salida de las herramientas aparece aquí.\n\nArrastra un .map \
                             sobre la ventana y pulsa Compilar (F5)."
                        } else {
                            "Ninguna línea coincide con el filtro."
                        })
                        .color(MUTED)
                        .small(),
                    );
                }
            });
    }

    // ---------------- header / footer ----------------

    fn ui_progress(&mut self, ui: &mut egui::Ui) {
        let total_stages = self.enabled_stages().max(1);
        let done = self.done.len();
        let frac = (done as f32 / total_stages as f32).clamp(0.0, 1.0);

        ui.horizontal(|ui| {
            for stage in STAGES.iter() {
                let name = stage.name();
                let enabled = match stage {
                    Stage::Csg => self.opts.run_csg,
                    Stage::Bsp => self.opts.run_bsp,
                    Stage::Vis => self.opts.run_vis,
                    Stage::Rad => self.opts.run_rad,
                };
                let (txt, color) = if let Some(st) = self.done.get(name) {
                    (
                        format!("{name} {}", fmt_secs(st.secs)),
                        if st.ok { OK } else { ERR },
                    )
                } else if self.running_stage == Some(*stage) {
                    let live = self
                        .stage_since
                        .map(|t| t.elapsed().as_secs_f64())
                        .unwrap_or(0.0);
                    (format!("{name} {}", fmt_secs(live)), ACCENT)
                } else if enabled {
                    (name.to_string(), MUTED)
                } else {
                    (format!("{name} (saltado)"), FAINT)
                };
                chip(ui, &txt, color).on_hover_text(stage.purpose());
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(total) = self.total_secs {
                    ui.label(
                        RichText::new(format!("total {}", fmt_secs(total)))
                            .color(TEXT)
                            .strong(),
                    );
                } else if let Some(t) = self.run_since {
                    ui.label(
                        RichText::new(format!("total {}", fmt_secs(t.elapsed().as_secs_f64())))
                            .color(MUTED),
                    );
                }
            });
        });

        ui.add_space(6.0);
        let bar = egui::ProgressBar::new(frac)
            .desired_height(6.0)
            .rounding(3.0)
            .fill(match self.last_outcome {
                Some(RunOutcome::Failed) => ERR,
                Some(RunOutcome::Cancelled) => WARN,
                _ => ACCENT,
            });
        ui.add(bar);
    }

    fn ui_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // Which project is open, and one click to go manage them.
            if let Some(i) = self.active {
                let name = self.lib.projects[i].name.clone();
                let saving = self.dirty_since.is_some();
                if chip(ui, &name, ACCENT)
                    .interact(egui::Sense::click())
                    .on_hover_text("Proyecto abierto. Click para ir a Proyectos.")
                    .clicked()
                {
                    self.tab = Tab::Projects;
                }
                ui.label(
                    RichText::new(if saving { "guardando..." } else { "guardado" })
                        .color(if saving { MUTED } else { OK })
                        .small(),
                );
            } else if ui
                .add(egui::Button::new(RichText::new("sin proyecto").color(MUTED).small()).small())
                .on_hover_text(
                    "Guarda lo que tienes cargado como proyecto para volver a él después.",
                )
                .clicked()
            {
                self.tab = Tab::Projects;
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.job.is_some() {
                    let btn = egui::Button::new(RichText::new("Cancelar").strong())
                        .min_size(egui::vec2(140.0, 32.0))
                        .fill(ERR.linear_multiply(0.35))
                        .stroke(egui::Stroke::new(1.0_f32, ERR));
                    if ui.add(btn).on_hover_text("Esc").clicked() {
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
                        egui::Button::new(RichText::new("Compilar").strong().size(14.5))
                            .min_size(egui::vec2(140.0, 32.0))
                            .fill(if enabled { ACCENT_DEEP } else { CARD })
                            .stroke(egui::Stroke::new(
                                1.0_f32,
                                if enabled { ACCENT } else { LINE },
                            )),
                    );
                    if btn.clicked() {
                        self.start();
                    }
                    let _ = btn.on_hover_text(if enabled {
                        "Compilar ahora (F5)"
                    } else if self.enabled_stages() == 0 {
                        "Activa al menos una etapa."
                    } else if !self.checks.map_ok {
                        "Falta un .map válido."
                    } else if !self.opts.extra_args_errors().is_empty() {
                        "Corrige las comillas de los parámetros extra."
                    } else {
                        self.checks
                            .plan_errors
                            .first()
                            .map(String::as_str)
                            .unwrap_or("Revisa la configuración de la compilación.")
                    });
                }

                if self.update_found.is_some() {
                    let tag = self
                        .update_found
                        .as_ref()
                        .map(|r| r.tag.clone())
                        .unwrap_or_default();
                    if chip(ui, &format!("actualizar a {tag}"), OK)
                        .interact(egui::Sense::click())
                        .on_hover_text("Hay una release nueva. Click para verla.")
                        .clicked()
                    {
                        self.update_window = true;
                    }
                }

                // Zoom: the whole point of this pair is a 4K screen, where the
                // default egui scale is unreadably small.
                ui.add_space(4.0);
                if ui
                    .small_button("A+")
                    .on_hover_text("Agrandar la interfaz")
                    .clicked()
                {
                    self.opts.ui_scale = (self.opts.ui_scale + 0.1).min(2.0);
                }
                if ui
                    .small_button("A-")
                    .on_hover_text("Achicar la interfaz")
                    .clicked()
                {
                    self.opts.ui_scale = (self.opts.ui_scale - 0.1).max(0.7);
                }
            });
        });
    }

    fn ui_footer(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let lower = self.status.to_ascii_lowercase();
            let color = if lower.starts_with("listo") || lower.contains("guardad") {
                OK
            } else if lower.contains("cancel") {
                WARN
            } else if lower.contains("error")
                || lower.contains("falló")
                || lower.contains("no pude")
                || lower.starts_with("falta")
            {
                ERR
            } else {
                MUTED
            };
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mut check_now = false;
                ui.menu_button("Actualizaciones", |ui| {
                    ui.set_min_width(230.0);
                    ui.label(
                        RichText::new(format!("Versión {}", update::Version::current()))
                            .color(TEXT)
                            .strong(),
                    );
                    if !self.update_status.is_empty() {
                        ui.label(RichText::new(&self.update_status).color(MUTED).small());
                    }
                    ui.separator();
                    if self.update_found.is_some() && ui.button("Ver la actualización").clicked() {
                        self.update_window = true;
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            self.update_check.is_none(),
                            egui::Button::new("Buscar ahora"),
                        )
                        .clicked()
                    {
                        check_now = true;
                        ui.close_menu();
                    }
                    let mut auto = self.lib.check_updates;
                    if ui
                        .checkbox(&mut auto, "Buscar actualizaciones al abrir")
                        .on_hover_text(
                            "Al abrir la app se busca una versión nueva. Si existe, se \
                             muestra la release y tú decides si instalarla. Nunca se \
                             actualiza durante un compilado ni sin confirmación.",
                        )
                        .changed()
                    {
                        self.lib.check_updates = auto;
                        self.save_library();
                    }
                    ui.separator();
                    ui.hyperlink_to(
                        "Releases en GitHub",
                        format!("https://github.com/{}/releases", update::REPO),
                    );
                });
                if check_now {
                    self.start_update_check(true);
                }

                let btn = ui.small_button("Guardar ajustes");
                if btn.clicked() {
                    self.status = match save_profile(&self.opts) {
                        Ok(()) => "Preferencias guardadas.".to_string(),
                        Err(e) => format!("No pude guardar: {e}"),
                    };
                }
                let _ = btn.on_hover_text(
                    "Las preferencias se guardan solas al cerrar la ventana y al \
                     empezar una compilación. Este botón solo fuerza el guardado ahora.",
                );

                let dir = self.result_dir();
                if ui
                    .add_enabled(dir.is_some(), egui::Button::new("Abrir carpeta").small())
                    .on_hover_text("Abre dónde queda el .bsp")
                    .clicked()
                {
                    if let Some(d) = dir {
                        open_in_explorer(&d);
                    }
                }

                // Whatever room the buttons left over, and not a pixel more.
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(&self.status).color(color).small())
                            .truncate(),
                    )
                    .on_hover_text(&self.status);
                });
            });
        });
    }
}

impl eframe::App for App {
    /// eframe calls this when the window closes. Preferences are saved here as
    /// well as on every compile, so the button is only a manual extra.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(mut job) = self.job.take() {
            job.cancel_and_wait();
        }
        let _ = save_profile(&self.opts);
        self.sync_active_project();
        self.save_library();
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_messages();
        self.drain_analysis();
        self.refresh_checks();
        self.sync_active_project();
        self.maybe_check_updates();
        self.drain_update_check();
        self.ui_update_window(ctx);
        self.drain_install(ctx);
        if self.job.is_none() && !self.installing {
            self.handle_drops(ctx);
        }

        if (self.opts.ui_scale - self.applied_scale).abs() > 0.001 {
            self.applied_scale = self.opts.ui_scale;
            ctx.set_zoom_factor(self.opts.ui_scale);
        }

        if self.job.is_some() || self.installing || self.analysis_job.is_some() {
            // Keep painting while output streams in and the timers run.
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        } else {
            // Files can disappear or tools can be replaced outside the app.
            // Wake often enough for the cached preflight to notice without
            // stat'ing every file on every frame.
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }

        // Keyboard: F5 compiles, Esc cancels.
        if ctx.input(|input| input.key_pressed(egui::Key::F5)) && self.can_run() {
            self.start();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if let Some(j) = &self.job {
                j.cancel();
                self.status = "Cancelando...".to_string();
            }
        }

        egui::TopBottomPanel::top("top")
            .frame(
                egui::Frame::none()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(14.0, 10.0)),
            )
            .show(ctx, |ui| {
                self.ui_header(ui);
                ui.add_space(10.0);
                tab_strip(ui, &mut self.tab, &TABS);
            });

        egui::TopBottomPanel::bottom("bottom")
            .frame(
                egui::Frame::none()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(14.0, 10.0)),
            )
            .show(ctx, |ui| {
                self.ui_progress(ui);
                ui.add_space(8.0);
                self.ui_footer(ui);
            });

        // Responsive: on a wide window the log lives beside the options; on a
        // narrow one it moves underneath, where it still gets useful height.
        let wide = ctx.screen_rect().width() >= 1120.0;
        let log_frame = egui::Frame::none()
            .fill(PANEL)
            .inner_margin(egui::Margin::symmetric(12.0, 10.0));
        // The two panels need different ids: egui remembers a panel's size by
        // id, and sharing one made the bottom log inherit the side panel's
        // width as its height, swallowing the options column.
        //
        // The projects tab gets the whole window: it is lists and file names,
        // and the compile log has nothing to say while you are managing them.
        // Its own side panel takes that space instead, so the folder explorer
        // is a tall column that shows everything at once.
        let show_log = self.tab != Tab::Projects && self.tab != Tab::Analysis;
        let editing_enabled = self.job.is_none() && !self.installing;
        if self.tab == Tab::Projects {
            let w = (ctx.screen_rect().width() * 0.34).clamp(340.0, 560.0);
            egui::SidePanel::right("project_files_panel")
                .frame(
                    egui::Frame::none()
                        .fill(PANEL)
                        .inner_margin(egui::Margin::symmetric(12.0, 10.0)),
                )
                .resizable(true)
                .default_width(w)
                .min_width(300.0)
                .max_width((ctx.screen_rect().width() * 0.5).max(360.0))
                .show(ctx, |ui| {
                    ui.add_enabled_ui(editing_enabled, |ui| self.ui_project_files(ui));
                });
        }
        if show_log && wide {
            let w = (ctx.screen_rect().width() * 0.36).clamp(300.0, 560.0);
            egui::SidePanel::right("log_side")
                .frame(log_frame)
                .resizable(true)
                .default_width(w)
                .min_width(280.0)
                .max_width(ctx.screen_rect().width() * 0.6)
                .show(ctx, |ui| self.ui_log(ui));
        } else if show_log {
            let h = (ctx.screen_rect().height() * 0.34).clamp(160.0, 460.0);
            egui::TopBottomPanel::bottom("log_bottom")
                .frame(log_frame)
                .resizable(true)
                .default_height(h)
                .min_height(120.0)
                .show(ctx, |ui| self.ui_log(ui));
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(12.0, 10.0)),
            )
            .show(ctx, |ui| {
                // One scroll state per tab: sharing it made a short tab open
                // half-scrolled after visiting a long one.
                egui::ScrollArea::vertical()
                    .id_source(self.tab.label())
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(4.0);
                        let max_w = if self.tab == Tab::Projects {
                            1500.0
                        } else {
                            CONTENT_MAX_W
                        };
                        centered_column_w(ui, max_w, |ui| {
                            ui.add_enabled_ui(editing_enabled, |ui| {
                                let m = Metrics::for_width(ui.available_width() - 28.0);
                                match self.tab {
                                    Tab::Projects => self.ui_projects(ui, &m),
                                    Tab::Compile => self.ui_compile(ui, &m),
                                    Tab::Csg => self.ui_csg(ui, &m),
                                    Tab::Bsp => self.ui_bsp(ui, &m),
                                    Tab::Vis => self.ui_vis(ui, &m),
                                    Tab::Rad => self.ui_rad(ui, &m),
                                    Tab::Analysis => self.ui_analysis(ui, &m),
                                    Tab::Advice => self.ui_advice(ui),
                                }
                            });
                        });
                    });
            });
    }
}

/// The window icon, as raw RGBA. Kept pre-decoded next to the .ico so the GUI
/// needs no image decoder at all; both are regenerated by
/// `assets/make_icon.ps1`.
fn window_icon() -> egui::IconData {
    const SIDE: u32 = 64;
    let rgba = include_bytes!("../assets/icon_64.rgba").to_vec();
    debug_assert_eq!(rgba.len(), (SIDE * SIDE * 4) as usize);
    egui::IconData {
        rgba,
        width: SIDE,
        height: SIDE,
    }
}

fn main() -> eframe::Result<()> {
    let instance_path = state_path("resdhlt-gui.lock")
        .unwrap_or_else(|| std::env::temp_dir().join("resdhlt-gui.lock"));
    let _instance_lock = match acquire_app_instance_lock(&instance_path) {
        Ok(lock) => lock,
        Err(error) => {
            rfd::MessageDialog::new()
                .set_title("ReSDHLT ya está abierto")
                .set_description(&error)
                .set_level(rfd::MessageLevel::Warning)
                .show();
            return Ok(());
        }
    };

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([760.0, 560.0])
            .with_title("ReSDHLT - compilador de mapas")
            .with_icon(window_icon())
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "ReSDHLT",
        native_options,
        // eframe 0.28's app creator returns a Result, so the boxed App has to be
        // wrapped in Ok. Older releases returned the Box directly.
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(App::default()))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_gui_instance_cannot_take_the_state_lock() {
        let root = std::env::temp_dir().join(format!("resdhlt-app-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let path = root.join("gui.lock");
        let first = acquire_app_instance_lock(&path).unwrap();
        assert!(acquire_app_instance_lock(&path).is_err());
        drop(first);
        assert!(acquire_app_instance_lock(&path).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn copying_the_bundled_tools_overwrites_the_old_ones() {
        let work = std::env::temp_dir().join("resdhlt-tools-copy-test");
        let _ = std::fs::remove_dir_all(&work);
        let from = work.join("bundled");
        let to = work.join("en_uso");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&to).unwrap();

        std::fs::write(from.join("sdHLCSG.exe"), "nuevo").unwrap();
        std::fs::write(from.join("sdhlt.fgd"), "nuevo").unwrap();
        std::fs::create_dir(from.join("subcarpeta")).unwrap();
        std::fs::write(to.join("sdHLCSG.exe"), "viejo").unwrap();
        // Something of the mapper's own that has no counterpart: it stays.
        std::fs::write(to.join("mis_wads.cfg"), "mío").unwrap();

        let copied = copy_bundled_tools(&from, &to).unwrap();

        assert_eq!(copied, 2, "solo los archivos, no la subcarpeta");
        assert_eq!(
            std::fs::read_to_string(to.join("sdHLCSG.exe")).unwrap(),
            "nuevo"
        );
        assert_eq!(
            std::fs::read_to_string(to.join("sdhlt.fgd")).unwrap(),
            "nuevo"
        );
        assert_eq!(
            std::fs::read_to_string(to.join("mis_wads.cfg")).unwrap(),
            "mío"
        );

        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn the_tools_in_use_are_not_stale_against_themselves() {
        // Whatever folder the shipped tools are in, it can never be reported as
        // out of date with respect to itself.
        if let Some(bundled) = bundled_tools_dir() {
            assert!(stale_against_bundled(&bundled).is_none());
        }
    }
}

//! Runs the four compile stages in a worker thread and streams their output.

use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use crate::options::Options;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Csg,
    Bsp,
    Vis,
    Rad,
}

impl Stage {
    pub fn exe(self) -> &'static str {
        match self {
            Stage::Csg => "sdHLCSG",
            Stage::Bsp => "sdHLBSP",
            Stage::Vis => "sdHLVIS",
            Stage::Rad => "sdHLRAD",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Stage::Csg => "CSG",
            Stage::Bsp => "BSP",
            Stage::Vis => "VIS",
            Stage::Rad => "RAD",
        }
    }

    /// One line on what this stage is for, shown next to the progress row.
    pub fn purpose(self) -> &'static str {
        match self {
            Stage::Csg => "Recorta los brushes entre sí y resuelve las texturas",
            Stage::Bsp => "Construye el árbol BSP, fusiona y subdivide las caras",
            Stage::Vis => "Calcula qué se ve desde dónde (el PVS). Define los FPS",
            Stage::Rad => "Calcula la iluminación. Es ~95% del tiempo total",
        }
    }
}

pub const STAGES: [Stage; 4] = [Stage::Csg, Stage::Bsp, Stage::Vis, Stage::Rad];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Normal,
    Command,
    Warning,
    Error,
    Success,
}

#[derive(Debug)]
pub enum Msg {
    Line(LineKind, String),
    StageStart(Stage),
    /// Stage finished: seconds taken, and whether it succeeded.
    StageDone(Stage, f64, bool),
    /// The whole run finished: total seconds, success.
    Finished(f64, bool),
}

pub struct Job {
    pub rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
}

impl Job {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

fn classify(line: &str) -> LineKind {
    let l = line.to_ascii_lowercase();
    if l.contains("error") || l.contains("fatal") || l.starts_with("leak") {
        LineKind::Error
    } else if l.contains("warning") || l.contains("leaked") {
        LineKind::Warning
    } else {
        LineKind::Normal
    }
}

/// Reads a stream and emits complete lines, treating both '\n' and '\r' as
/// terminators.
///
/// The compilers draw their progress meter by rewriting one line with carriage
/// returns. Reading with a normal line iterator would hold all of that in a
/// single enormous "line" until the stage ended, so the log would sit empty and
/// then dump everything at once.
fn pump<R: Read>(reader: R, tx: &Sender<Msg>) {
    let mut reader = BufReader::new(reader);
    let mut buf = [0u8; 4096];
    let mut line: Vec<u8> = Vec::with_capacity(256);

    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        for &b in &buf[..n] {
            if b == b'\n' || b == b'\r' {
                if !line.is_empty() {
                    let text = String::from_utf8_lossy(&line).trim_end().to_string();
                    if !text.is_empty() {
                        let _ = tx.send(Msg::Line(classify(&text), text));
                    }
                    line.clear();
                }
            } else {
                line.push(b);
            }
        }
    }

    if !line.is_empty() {
        let text = String::from_utf8_lossy(&line).trim_end().to_string();
        if !text.is_empty() {
            let _ = tx.send(Msg::Line(classify(&text), text));
        }
    }
}

fn tool_path(tools_dir: &str, exe: &str) -> Option<PathBuf> {
    let dir = Path::new(tools_dir);
    for candidate in [format!("{exe}.exe"), exe.to_string()] {
        let p = dir.join(candidate);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

#[cfg(windows)]
fn hide_console(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW: the tools are console programs, and without this each
    // stage flashes a console window over the GUI.
    cmd.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn hide_console(_cmd: &mut Command) {}

/// Every .wad in a folder, plus sdhlt.wad from the tools folder.
///
/// sdhlt.wad holds the tool textures (NULL, HINT, SKIP, BEVELHINT...) and a
/// compile fails without it, so it is always included even if the mapper keeps
/// it somewhere else.
fn collect_wads(wad_dir: &str, tools_dir: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();

    let tool_wad = Path::new(tools_dir).join("sdhlt.wad");
    if tool_wad.is_file() {
        out.push(tool_wad);
    }

    if let Ok(entries) = std::fs::read_dir(wad_dir) {
        let mut found: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case("wad"))
                    .unwrap_or(false)
            })
            .collect();
        found.sort();
        for f in found {
            // Skip a second copy of the tool wad.
            let dup = f
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.eq_ignore_ascii_case("sdhlt.wad"))
                .unwrap_or(false);
            if !dup {
                out.push(f);
            }
        }
    }

    out
}

/// Replaces the worldspawn "wad" key with the given list.
///
/// Maps carry absolute WAD paths from whoever built them, so a .map from
/// someone else's machine will not compile here. Rewriting the key is the only
/// way to fix that: CSG reads its WAD list from this key and has no equivalent
/// of RAD's -waddir.
fn rewrite_wad_key(map_text: &str, wads: &[PathBuf]) -> Option<String> {
    if wads.is_empty() {
        return None;
    }
    let joined = wads
        .iter()
        .map(|p| p.display().to_string().replace('\\', "/"))
        .collect::<Vec<_>>()
        .join(";");

    let needle = "\"wad\"";
    let start = map_text.find(needle)?;

    // The value is the quoted string that follows the key.
    let after_key = start + needle.len();
    let rest = &map_text[after_key..];
    let vstart = after_key + rest.find('"')?;
    let vend = vstart + 1 + map_text[vstart + 1..].find('"')?;

    let mut out = String::with_capacity(map_text.len() + joined.len());
    out.push_str(&map_text[..vstart + 1]);
    out.push_str(&joined);
    out.push_str(&map_text[vend..]);
    Some(out)
}

/// Decides where the compile happens and puts the .map there.
///
/// Returns the map path the tools should be pointed at. When an output folder
/// is configured the source map is copied into it and everything the tools
/// write (.bsp, .prt, logs, the .p0-.p3 intermediates) lands there, leaving the
/// source folder untouched.
fn prepare_workspace(opts: &Options, tx: &Sender<Msg>) -> Option<PathBuf> {
    let src = PathBuf::from(&opts.map_path);
    if !src.is_file() {
        let _ = tx.send(Msg::Line(
            LineKind::Error,
            format!("No encuentro el mapa: {}", src.display()),
        ));
        return None;
    }

    if !opts.uses_output_dir() {
        return Some(src);
    }

    let out_dir = PathBuf::from(opts.output_dir.trim());
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        let _ = tx.send(Msg::Line(
            LineKind::Error,
            format!("No pude crear {}: {e}", out_dir.display()),
        ));
        return None;
    }

    let name = src.file_name()?;
    let dst = out_dir.join(name);

    // Guard against copying a file onto itself.
    let same = std::fs::canonicalize(&src).ok() == std::fs::canonicalize(&dst).ok()
        && dst.exists();
    if same {
        return Some(dst);
    }

    let text = match std::fs::read_to_string(&src) {
        Ok(t) => t,
        Err(e) => {
            let _ = tx.send(Msg::Line(
                LineKind::Error,
                format!("No pude leer el mapa: {e}"),
            ));
            return None;
        }
    };

    let mut final_text = text;
    if opts.rewrite_wad_key && !opts.wad_dir.trim().is_empty() {
        let wads = collect_wads(&opts.wad_dir, &opts.tools_dir);
        if wads.is_empty() {
            let _ = tx.send(Msg::Line(
                LineKind::Warning,
                format!(
                    "No encontré ningún .wad en {}. Se deja la lista original del mapa.",
                    opts.wad_dir
                ),
            ));
        } else if let Some(patched) = rewrite_wad_key(&final_text, &wads) {
            let _ = tx.send(Msg::Line(
                LineKind::Command,
                format!(
                    "Lista de WADs reescrita en la copia ({} archivos desde {})",
                    wads.len(),
                    opts.wad_dir
                ),
            ));
            final_text = patched;
        } else {
            let _ = tx.send(Msg::Line(
                LineKind::Warning,
                "El mapa no tiene clave \"wad\" en worldspawn; no se reescribió nada."
                    .to_string(),
            ));
        }
    }

    if let Err(e) = std::fs::write(&dst, final_text) {
        let _ = tx.send(Msg::Line(
            LineKind::Error,
            format!("No pude escribir {}: {e}", dst.display()),
        ));
        return None;
    }

    let _ = tx.send(Msg::Line(
        LineKind::Command,
        format!("Compilando en {}", out_dir.display()),
    ));
    Some(dst)
}

/// Spawns the compile. Returns immediately; progress arrives on the channel.
pub fn start(opts: Options) -> Job {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_thread = cancel.clone();

    thread::spawn(move || {
        let overall = Instant::now();

        let map = match prepare_workspace(&opts, &tx) {
            Some(m) => m,
            None => {
                let _ = tx.send(Msg::Finished(overall.elapsed().as_secs_f64(), false));
                return;
            }
        };
        let base = map.with_extension("");
        let work_dir = map.parent().map(|p| p.to_path_buf());

        let enabled = [opts.run_csg, opts.run_bsp, opts.run_vis, opts.run_rad];
        let mut all_ok = true;

        for (i, stage) in STAGES.iter().enumerate() {
            if !enabled[i] {
                continue;
            }
            if cancel_thread.load(Ordering::SeqCst) {
                let _ = tx.send(Msg::Line(
                    LineKind::Warning,
                    "Cancelado por el usuario.".to_string(),
                ));
                all_ok = false;
                break;
            }

            let exe = match tool_path(&opts.tools_dir, stage.exe()) {
                Some(p) => p,
                None => {
                    let _ = tx.send(Msg::Line(
                        LineKind::Error,
                        format!(
                            "No encontré {} en {}. Revisa la carpeta de herramientas.",
                            stage.exe(),
                            opts.tools_dir
                        ),
                    ));
                    all_ok = false;
                    break;
                }
            };

            // CSG takes the .map; the later stages take the name without extension.
            let target = if *stage == Stage::Csg {
                map.clone()
            } else {
                base.clone()
            };

            let args = match stage {
                Stage::Csg => opts.csg_args(),
                Stage::Bsp => opts.bsp_args(),
                Stage::Vis => opts.vis_args(),
                Stage::Rad => opts.rad_args(),
            };

            let _ = tx.send(Msg::StageStart(*stage));
            let _ = tx.send(Msg::Line(
                LineKind::Command,
                format!(
                    "{} {} {}",
                    exe.display(),
                    args.join(" "),
                    target.display()
                ),
            ));

            let mut cmd = Command::new(&exe);
            cmd.args(&args)
                .arg(&target)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if let Some(dir) = &work_dir {
                cmd.current_dir(dir);
            }
            hide_console(&mut cmd);

            let started = Instant::now();
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(Msg::Line(
                        LineKind::Error,
                        format!("No pude ejecutar {}: {e}", exe.display()),
                    ));
                    all_ok = false;
                    break;
                }
            };

            // Drain both pipes concurrently, or a full stderr buffer can block
            // the child while we are still reading stdout.
            let out = child.stdout.take();
            let err = child.stderr.take();
            let tx_out = tx.clone();
            let tx_err = tx.clone();
            let h_out = out.map(move |s| thread::spawn(move || pump(s, &tx_out)));
            let h_err = err.map(move |s| thread::spawn(move || pump(s, &tx_err)));

            let status = loop {
                if cancel_thread.load(Ordering::SeqCst) {
                    let _ = child.kill();
                }
                match child.try_wait() {
                    Ok(Some(st)) => break Some(st),
                    Ok(None) => thread::sleep(std::time::Duration::from_millis(40)),
                    Err(_) => break None,
                }
            };

            if let Some(h) = h_out {
                let _ = h.join();
            }
            if let Some(h) = h_err {
                let _ = h.join();
            }

            let secs = started.elapsed().as_secs_f64();
            let ok = status.map(|s| s.success()).unwrap_or(false);
            let _ = tx.send(Msg::StageDone(*stage, secs, ok));

            if !ok {
                all_ok = false;
                let _ = tx.send(Msg::Line(
                    LineKind::Error,
                    format!("{} terminó con error. Se detiene aquí.", stage.name()),
                ));
                break;
            }
        }

        if all_ok {
            let bsp = base.with_extension("bsp");
            if bsp.is_file() {
                let size = std::fs::metadata(&bsp).map(|m| m.len()).unwrap_or(0);
                let _ = tx.send(Msg::Line(
                    LineKind::Success,
                    format!("Listo: {} ({:.1} MB)", bsp.display(), size as f64 / 1e6),
                ));
            } else {
                let _ = tx.send(Msg::Line(
                    LineKind::Success,
                    "Compilación terminada.".to_string(),
                ));
            }
        }
        let _ = tx.send(Msg::Finished(overall.elapsed().as_secs_f64(), all_ok));
    });

    Job { rx, cancel }
}

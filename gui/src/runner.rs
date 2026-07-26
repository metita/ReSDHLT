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

/// Spawns the compile. Returns immediately; progress arrives on the channel.
pub fn start(opts: Options) -> Job {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_thread = cancel.clone();

    thread::spawn(move || {
        let overall = Instant::now();
        let map = PathBuf::from(&opts.map_path);
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
                            "No encontré {} en {}. Revisá la carpeta de herramientas.",
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
                    format!("{} terminó con error. Se detiene acá.", stage.name()),
                ));
                break;
            }
        }

        if all_ok {
            let _ = tx.send(Msg::Line(
                LineKind::Success,
                "Compilación terminada.".to_string(),
            ));
        }
        let _ = tx.send(Msg::Finished(overall.elapsed().as_secs_f64(), all_ok));
    });

    Job { rx, cancel }
}

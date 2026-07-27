//! Runs the four compile stages in a worker thread and streams their output.

use std::collections::{HashMap, HashSet};
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

/// Name of the generated WAD list. It is written next to the map being
/// compiled and passed to CSG as `-wadcfgfile`, which makes CSG ignore the
/// map's own `wad` key.
pub const WAD_CFG_NAME: &str = "resdhlt-wads.cfg";

/// CSG refuses to load more than MAX_WADPATHS (`src/sdhlt/sdHLCSG/wadpath.h`,
/// raised in this fork from 128 to 512) and errors out with "too many wad
/// files" instead of ignoring the excess. A few slots are left spare, and the
/// number stays under the old 128 only if you pair this GUI with old tools —
/// which is why the package ships both together.
const MAX_WADS: usize = 500;

/// The value of the worldspawn `wad` key, if the map has one.
fn wad_key_value(map_text: &str) -> Option<&str> {
    let needle = "\"wad\"";
    let start = map_text.find(needle)?;
    let after_key = start + needle.len();
    let vstart = after_key + map_text[after_key..].find('"')?;
    let vend = vstart + 1 + map_text[vstart + 1..].find('"')?;
    Some(&map_text[vstart + 1..vend])
}

/// Every texture the map actually uses.
///
/// In a .map, each brush face ends with the texture name after its three
/// plane points: `( x y z ) ( x y z ) ( x y z ) TEXNAME [ .. ] [ .. ] 0 1 1`.
/// That is enough to know which WADs are worth loading and which are dead
/// weight.
fn map_textures(map_text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for line in map_text.lines() {
        let line = line.trim();
        if !line.starts_with('(') {
            continue;
        }
        let mut closes = 0;
        for (i, tok) in line.split_whitespace().enumerate() {
            if tok == ")" {
                closes += 1;
                if closes == 3 {
                    if let Some(name) = line.split_whitespace().nth(i + 1) {
                        out.insert(name.to_ascii_uppercase());
                    }
                    break;
                }
            }
        }
    }
    out
}

/// The texture names inside a WAD3 file.
///
/// Only the header and the lump directory are read, so scanning a folder of
/// hundreds of WADs costs a couple of seeks each instead of their full size.
fn wad_textures(path: &Path) -> Option<HashSet<String>> {
    use std::io::{Seek, SeekFrom};

    let mut f = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 12];
    f.read_exact(&mut header).ok()?;
    if &header[0..4] != b"WAD3" && &header[0..4] != b"WAD2" {
        return None;
    }
    let count = u32::from_le_bytes(header[4..8].try_into().ok()?) as usize;
    let table = u32::from_le_bytes(header[8..12].try_into().ok()?) as u64;
    if count == 0 || count > 100_000 {
        return None;
    }

    f.seek(SeekFrom::Start(table)).ok()?;
    let mut dir = vec![0u8; count * 32];
    f.read_exact(&mut dir).ok()?;

    let mut out = HashSet::with_capacity(count);
    for entry in dir.chunks_exact(32) {
        let raw = &entry[16..32];
        let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
        if let Ok(name) = std::str::from_utf8(&raw[..end]) {
            out.insert(name.to_ascii_uppercase());
        }
    }
    Some(out)
}

fn is_wad(p: &Path) -> bool {
    p.extension()
        .and_then(|x| x.to_str())
        .map(|x| x.eq_ignore_ascii_case("wad"))
        .unwrap_or(false)
}

/// Identity of a file for dedup purposes: the same WAD reached through two
/// different folders must not be listed twice.
fn canon_key(p: &Path) -> String {
    std::fs::canonicalize(p)
        .map(|c| c.display().to_string())
        .unwrap_or_else(|_| p.display().to_string())
        .to_ascii_lowercase()
}

fn file_key(p: &Path) -> String {
    p.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// Every .wad under a folder, a few levels deep. Mappers keep their WADs in
/// per-map subfolders more often than not.
fn collect_wads_deep(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if out.len() > 512 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut found: Vec<PathBuf> = Vec::new();
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            subdirs.push(p);
        } else if is_wad(&p) {
            found.push(p);
        }
    }
    found.sort();
    subdirs.sort();
    out.append(&mut found);
    if depth > 0 {
        for d in subdirs {
            collect_wads_deep(&d, depth - 1, out);
        }
    }
}

/// Builds the WAD list CSG will actually use.
///
/// A `.map` stores absolute WAD paths from the machine it was built on. They
/// are frequently wrong here: another PC ("/Users/Admin/..."), another drive,
/// or a path with no drive letter at all, which Windows resolves against
/// whatever the current drive happens to be. CSG then dies with "Could not
/// open wad file".
///
/// So every entry of the key is checked, and the broken ones are looked up by
/// file name in the WAD folder, next to the map, and in the tools folder. What
/// cannot be found is reported and skipped instead of killing the compile.
///
/// `map` is the file the tools will read, which with an output folder is a copy
/// somewhere else entirely; the WADs live next to the *source*, so both folders
/// are searched.
fn resolve_wads(opts: &Options, map: &Path, tx: &Sender<Msg>) -> Vec<PathBuf> {
    let tools = Path::new(opts.tools_dir.trim());

    // Folders that may hold a WAD, most specific first.
    let mut search: Vec<(PathBuf, usize)> = Vec::new();
    if !opts.wad_dir.trim().is_empty() {
        search.push((PathBuf::from(opts.wad_dir.trim()), 3));
    }
    if let Some(parent) = Path::new(opts.map_path.trim()).parent() {
        search.push((parent.to_path_buf(), 1));
    }
    if let Some(parent) = map.parent() {
        search.push((parent.to_path_buf(), 1));
    }

    // Everything we could substitute a broken entry with, indexed by file name.
    let mut pool: Vec<PathBuf> = Vec::new();
    for (dir, depth) in &search {
        collect_wads_deep(dir, *depth, &mut pool);
    }
    collect_wads_deep(tools, 0, &mut pool);

    let mut index: HashMap<String, PathBuf> = HashMap::new();
    for p in &pool {
        index.entry(file_key(p)).or_insert_with(|| p.clone());
    }

    let mut out: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let push = |out: &mut Vec<PathBuf>, seen: &mut HashSet<String>, p: PathBuf| {
        if seen.insert(canon_key(&p)) {
            out.push(p);
        }
    };

    let map_text = std::fs::read_to_string(map).unwrap_or_default();
    let entries: Vec<String> = wad_key_value(&map_text)
        .unwrap_or("")
        .split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let mut kept = 0usize;
    let mut fixed = 0usize;
    let mut missing: Vec<String> = Vec::new();
    for entry in &entries {
        let path = PathBuf::from(entry.replace('/', "\\"));
        if path.is_file() {
            kept += 1;
            push(&mut out, &mut seen, path);
            continue;
        }
        match index.get(&file_key(&path)) {
            Some(found) => {
                fixed += 1;
                push(&mut out, &mut seen, found.clone());
            }
            None => missing.push(
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(entry)
                    .to_string(),
            ),
        }
    }

    // sdhlt.wad holds the tool textures (NULL, HINT, SKIP, BEVELHINT...) and a
    // compile fails without it, so it goes in before anything optional.
    let tool_wad = tools.join("sdhlt.wad");
    if tool_wad.is_file() {
        push(&mut out, &mut seen, tool_wad);
    } else {
        let _ = tx.send(Msg::Line(
            LineKind::Warning,
            format!(
                "No encontré sdhlt.wad en {}. Sin él fallan las texturas de \
                 herramienta (NULL, HINT, SKIP).",
                tools.display()
            ),
        ));
    }

    // What the map needs is often in a WAD it never listed: typically the one
    // sitting next to it with the map's own name. Rather than dumping the whole
    // folder in (CSG stops at MAX_WADPATHS, and a folder of hundreds would be
    // cut off alphabetically), the missing textures decide: each candidate is
    // opened, and only the ones that actually supply something still missing
    // make the list.
    let extra_before = out.len();
    let wanted = map_textures(&map_text);
    let mut uncovered: HashSet<String> = wanted.clone();
    for w in &out {
        if let Some(have) = wad_textures(w) {
            uncovered.retain(|t| !have.contains(t));
        }
    }

    if !uncovered.is_empty() && !wanted.is_empty() {
        let _ = tx.send(Msg::Line(
            LineKind::Command,
            format!(
                "Faltan {} texturas. Reviso {} .wad de tus carpetas para ver cuáles las \
                 tienen.",
                uncovered.len(),
                pool.len()
            ),
        ));

        // Score every candidate once, then take the most useful first. A WAD
        // that covers nothing is never loaded, however close to the map it sits.
        let mut scored: Vec<(usize, PathBuf, HashSet<String>)> = Vec::new();
        for p in &pool {
            if seen.contains(&canon_key(p)) {
                continue;
            }
            if let Some(have) = wad_textures(p) {
                let hits = uncovered.iter().filter(|t| have.contains(*t)).count();
                if hits > 0 {
                    scored.push((hits, p.clone(), have));
                }
            }
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0));

        for (_, path, have) in scored {
            if uncovered.is_empty() || out.len() >= MAX_WADS {
                break;
            }
            let hits = uncovered.iter().filter(|t| have.contains(*t)).count();
            if hits == 0 {
                continue; // an earlier pick already covered it
            }
            uncovered.retain(|t| !have.contains(t));
            let _ = tx.send(Msg::Line(
                LineKind::Command,
                format!(
                    "  + {} ({hits} texturas)",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                ),
            ));
            push(&mut out, &mut seen, path);
        }
    }
    let extra = out.len() - extra_before;

    // The referenced WADs alone can already exceed the limit on a map that
    // lists dozens; trimming here beats CSG dying with "too many wad files".
    let dropped = out.len().saturating_sub(MAX_WADS);
    if dropped > 0 {
        out.truncate(MAX_WADS);
        let _ = tx.send(Msg::Line(
            LineKind::Warning,
            format!(
                "La lista pasaba el límite de {MAX_WADS} WADs de CSG: dejé fuera los \
                 últimos {dropped}. Si falta alguna textura, apunta la carpeta de WADs a \
                 algo más chico."
            ),
        ));
    }

    let _ = tx.send(Msg::Line(
        LineKind::Command,
        format!(
            "WADs: {kept} del mapa correctos, {fixed} reubicados, {extra} extra de las \
             carpetas buscadas, {} sin encontrar",
            missing.len()
        ),
    ));
    // A listed WAD that is nowhere to be found is not a problem in itself: what
    // matters is whether its textures turn up in another one. That verdict
    // comes below, once coverage is known, so these stay informational.
    for m in &missing {
        let note = if m.eq_ignore_ascii_case("zhlt.wad") || m.eq_ignore_ascii_case("sdhlt.wad") {
            " (normal: sus texturas de herramienta están en sdhlt.wad)"
        } else {
            ""
        };
        let _ = tx.send(Msg::Line(
            LineKind::Command,
            format!("{m} no está en tus carpetas; busco sus texturas en otros .wad{note}"),
        ));
    }
    if !missing.is_empty() {
        let dirs: Vec<String> = search.iter().map(|(d, _)| d.display().to_string()).collect();
        let _ = tx.send(Msg::Line(
            LineKind::Command,
            format!("Busqué .wad en: {}", dirs.join(" | ")),
        ));
        if pool.is_empty() {
            let _ = tx.send(Msg::Line(
                LineKind::Warning,
                "No hay ningún .wad en esas carpetas. Indica la carpeta de WADs donde \
                 tengas los del mapa."
                    .to_string(),
            ));
        }
    }

    // The verdict, last so it is the line left on screen: whether some listed
    // WAD went missing is noise next to whether the textures are covered.
    if wanted.is_empty() {
        let _ = tx.send(Msg::Line(
            LineKind::Warning,
            "No pude leer las texturas del .map, así que no puedo comprobar la lista de \
             WADs por adelantado."
                .to_string(),
        ));
    } else if uncovered.is_empty() {
        let _ = tx.send(Msg::Line(
            LineKind::Success,
            format!(
                "Las {} texturas del mapa están cubiertas por {} .wad. No falta ninguna.",
                wanted.len(),
                out.len()
            ),
        ));
    } else {
        let mut names: Vec<&String> = uncovered.iter().collect();
        names.sort();
        let shown: Vec<&str> = names.iter().take(6).map(|s| s.as_str()).collect();
        let _ = tx.send(Msg::Line(
            LineKind::Error,
            format!(
                "Faltan {} texturas y ningún .wad tuyo las tiene: {}{}. CSG va a fallar; \
                 hace falta conseguir el .wad que las contenga.",
                names.len(),
                shown.join(", "),
                if names.len() > shown.len() { ", ..." } else { "" }
            ),
        ));
    }

    out
}

/// Writes the list where CSG expects it and returns the file name to pass as
/// `-wadcfgfile`.
///
/// The bare name is enough: CSG looks for it next to the map first, and that is
/// exactly where it is written.
fn write_wad_cfg(dir: &Path, wads: &[PathBuf]) -> std::io::Result<String> {
    let mut text = String::from("// Generado por ReSDHLT GUI. Se regenera en cada compilación.\n");
    for w in wads {
        text.push('"');
        text.push_str(&w.display().to_string());
        text.push_str("\"\n");
    }
    std::fs::write(dir.join(WAD_CFG_NAME), text)?;
    Ok(WAD_CFG_NAME.to_string())
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

    // With organize_output this is <salida>/<proyecto>/intermedios, so the
    // tools scatter their output there and the .bsp gets moved up to
    // <salida>/<proyecto> once the run succeeds.
    let out_dir = opts.work_dir()?;
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

    // The copy is byte for byte the original: the WAD list is handled with a
    // generated wad.cfg instead of by patching the map.
    if let Err(e) = std::fs::copy(&src, &dst) {
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

        // WAD list for CSG. Written next to the map and passed as -wadcfgfile,
        // which makes CSG use it instead of the map's own (often broken) key.
        let mut wad_cfg: Option<String> = None;
        if opts.auto_wads && opts.run_csg {
            let wads = resolve_wads(&opts, &map, &tx);
            if wads.is_empty() {
                let _ = tx.send(Msg::Line(
                    LineKind::Warning,
                    "No pude resolver ningún .wad; se usa la lista del mapa tal cual."
                        .to_string(),
                ));
            } else if let Some(dir) = &work_dir {
                match write_wad_cfg(dir, &wads) {
                    Ok(name) => wad_cfg = Some(name),
                    Err(e) => {
                        let _ = tx.send(Msg::Line(
                            LineKind::Warning,
                            format!("No pude escribir {WAD_CFG_NAME}: {e}"),
                        ));
                    }
                }
            }
        }

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

            let mut args = match stage {
                Stage::Csg => opts.csg_args(),
                Stage::Bsp => opts.bsp_args(),
                Stage::Vis => opts.vis_args(),
                Stage::Rad => opts.rad_args(),
            };
            if *stage == Stage::Csg {
                if let Some(cfg) = &wad_cfg {
                    args.push("-wadcfgfile".to_string());
                    args.push(cfg.clone());
                }
            }

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
            let mut bsp = base.with_extension("bsp");

            // The .bsp is the only thing worth keeping in plain sight, so it
            // moves up out of the scratch folder and everything else stays
            // behind.
            if opts.organize_output {
                if let Some(dest_dir) = opts.output_base() {
                    if bsp.is_file() && Some(dest_dir.as_path()) != bsp.parent() {
                        let dest = dest_dir.join(bsp.file_name().unwrap_or_default());
                        let moved = std::fs::rename(&bsp, &dest).or_else(|_| {
                            // rename fails across volumes; copying is the fallback.
                            std::fs::copy(&bsp, &dest).and_then(|_| std::fs::remove_file(&bsp))
                        });
                        match moved {
                            Ok(()) => bsp = dest,
                            Err(e) => {
                                let _ = tx.send(Msg::Line(
                                    LineKind::Warning,
                                    format!(
                                        "No pude mover el .bsp a {}: {e}. Queda en {}.",
                                        dest_dir.display(),
                                        bsp.display()
                                    ),
                                ));
                            }
                        }
                    }
                }
            }

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A WAD3 file holding the given texture names. Only the header and lump
    /// directory are written, which is all the selection code reads.
    fn fake_wad(path: &Path, textures: &[&str]) {
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(b"WAD3");
        bytes.extend_from_slice(&(textures.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&12u32.to_le_bytes()); // directory starts right here
        for name in textures {
            bytes.extend_from_slice(&0u32.to_le_bytes()); // filepos
            bytes.extend_from_slice(&0u32.to_le_bytes()); // disksize
            bytes.extend_from_slice(&0u32.to_le_bytes()); // size
            bytes.push(0x43); // type: miptex
            bytes.push(0); // compression
            bytes.extend_from_slice(&[0, 0]); // padding
            let mut field = [0u8; 16];
            let raw = name.as_bytes();
            field[..raw.len().min(15)].copy_from_slice(&raw[..raw.len().min(15)]);
            bytes.extend_from_slice(&field);
        }
        std::fs::write(path, bytes).unwrap();
    }

    /// A worldspawn with the given wad key and one brush whose faces use the
    /// given textures.
    fn fake_map(wad_key: &str, textures: &[&str]) -> String {
        let mut s = format!("{{\n\"classname\" \"worldspawn\"\n\"wad\" \"{wad_key}\"\n{{\n");
        for t in textures {
            s.push_str(&format!(
                "( -64 -64 -16 ) ( -64 -63 -16 ) ( -64 -64 -15 ) {t} [ 0 1 0 0 ] \
                 [ 0 0 -1 0 ] 0 1 1\n"
            ));
        }
        s.push_str("}\n}\n");
        s
    }

    /// A map from another machine: one WAD reachable under a different path, one
    /// gone for good. The first must be relocated by file name, the second
    /// reported and dropped, and sdhlt.wad added.
    #[test]
    fn relocates_wads_from_another_machine() {
        let root = std::env::temp_dir().join(format!("resdhlt_wadtest_{}", std::process::id()));
        let wads = root.join("wads").join("ar_azteca");
        let tools = root.join("tools");
        std::fs::create_dir_all(&wads).unwrap();
        std::fs::create_dir_all(&tools).unwrap();
        std::fs::write(wads.join("ar_azteca.wad"), b"x").unwrap();
        std::fs::write(tools.join("sdhlt.wad"), b"x").unwrap();

        let map = root.join("ar_azteca.map");
        std::fs::write(
            &map,
            "{\n\"classname\" \"worldspawn\"\n\
             \"wad\" \"/Users/Admin/Documents/Mapping/Mapas/ar_azteca/ar_azteca.wad;\
             /Users/Admin/Documents/Mapping/WADS/zgaminglogos.wad\"\n}\n",
        )
        .unwrap();

        let mut opts = Options::default();
        opts.map_path = map.display().to_string();
        opts.tools_dir = tools.display().to_string();
        opts.wad_dir = root.join("wads").display().to_string();

        let (tx, rx) = channel();
        let found = resolve_wads(&opts, &map, &tx);
        drop(tx);

        let names: Vec<String> = found.iter().map(|p| file_key(p)).collect();
        assert!(names.contains(&"ar_azteca.wad".to_string()), "{names:?}");
        assert!(names.contains(&"sdhlt.wad".to_string()), "{names:?}");
        assert!(!names.contains(&"zgaminglogos.wad".to_string()), "{names:?}");

        // The one that cannot be found is reported, not swallowed. It is not a
        // warning on its own: only uncovered textures earn that.
        let reported = rx
            .try_iter()
            .any(|m| matches!(m, Msg::Line(_, t) if t.contains("zgaminglogos.wad")));
        assert!(reported);

        // And the generated cfg quotes every path, one per line.
        let name = write_wad_cfg(&root, &found).unwrap();
        let text = std::fs::read_to_string(root.join(&name)).unwrap();
        assert!(text.contains("ar_azteca.wad\""));

        std::fs::remove_dir_all(&root).ok();
    }

    /// With an output folder the tools read a copy elsewhere, but the WADs sit
    /// next to the source map. That folder must still be searched, including
    /// the common case of a WAD the map never listed at all.
    #[test]
    fn finds_wads_next_to_the_source_map() {
        let root = std::env::temp_dir().join(format!("resdhlt_wadsrc_{}", std::process::id()));
        let src_dir = root.join("Mapas").join("koth_sandy");
        let out_dir = root.join("Desktop").join("Maps");
        let tools = root.join("tools");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&out_dir).unwrap();
        std::fs::create_dir_all(&tools).unwrap();
        fake_wad(&tools.join("sdhlt.wad"), &["NULL", "HINT"]);
        fake_wad(&src_dir.join("koth_sandy.wad"), &["DUSAND", "PARE59"]);

        // The map lists three WADs nobody has, and not its own.
        let text = fake_map(
            "/valve/zhlt.wad;/WADS/texture_map_pack.wad;/WADS/ba_sand.wad",
            &["DUSAND", "PARE59", "NULL"],
        );
        let src_map = src_dir.join("koth_sandy.map");
        let work_map = out_dir.join("koth_sandy.map");
        std::fs::write(&src_map, &text).unwrap();
        std::fs::write(&work_map, &text).unwrap();

        let mut opts = Options::default();
        opts.map_path = src_map.display().to_string();
        opts.output_dir = out_dir.display().to_string();
        opts.tools_dir = tools.display().to_string();

        let (tx, _rx) = channel();
        let found = resolve_wads(&opts, &work_map, &tx);
        let names: Vec<String> = found.iter().map(|p| file_key(p)).collect();
        assert!(names.contains(&"koth_sandy.wad".to_string()), "{names:?}");
        assert!(names.contains(&"sdhlt.wad".to_string()), "{names:?}");

        std::fs::remove_dir_all(&root).ok();
    }

    /// The point of the whole thing: a folder with a thousand WADs, of which
    /// exactly two hold what the map uses. Those two must come out, and nothing
    /// else, however alphabetically lucky the decoys are.
    #[test]
    fn picks_only_the_wads_that_hold_the_textures() {
        let root = std::env::temp_dir().join(format!("resdhlt_wadpick_{}", std::process::id()));
        let wads = root.join("wads");
        let tools = root.join("tools");
        std::fs::create_dir_all(&wads).unwrap();
        std::fs::create_dir_all(&tools).unwrap();
        fake_wad(&tools.join("sdhlt.wad"), &["NULL", "HINT", "SKIP"]);

        for i in 0..1000 {
            fake_wad(
                &wads.join(format!("decoy{i:04}.wad")),
                &[&format!("JUNK{i}A"), &format!("JUNK{i}B")],
            );
        }
        fake_wad(&wads.join("zz_walls.wad"), &["DUST0_WALL_03", "PARE59"]);
        fake_wad(&wads.join("zz_water.wad"), &["!LEANWATER_W5"]);

        let map = root.join("m.map");
        std::fs::write(
            &map,
            fake_map(
                "/Users/Otro/WADS/nada.wad",
                &["DUST0_WALL_03", "PARE59", "!LEANWATER_W5", "NULL"],
            ),
        )
        .unwrap();

        let mut opts = Options::default();
        opts.map_path = map.display().to_string();
        opts.tools_dir = tools.display().to_string();
        opts.wad_dir = wads.display().to_string();

        let (tx, _rx) = channel();
        let found = resolve_wads(&opts, &map, &tx);
        let mut names: Vec<String> = found.iter().map(|p| file_key(p)).collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "sdhlt.wad".to_string(),
                "zz_walls.wad".to_string(),
                "zz_water.wad".to_string(),
            ],
            "{names:?}"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// A map whose own WADs all resolve should not drag the folder in at all,
    /// and no selection may ever exceed what CSG accepts.
    #[test]
    fn stays_under_the_csg_limit() {
        let root = std::env::temp_dir().join(format!("resdhlt_wadcap_{}", std::process::id()));
        let wads = root.join("wads");
        let tools = root.join("tools");
        std::fs::create_dir_all(&wads).unwrap();
        std::fs::create_dir_all(&tools).unwrap();
        fake_wad(&tools.join("sdhlt.wad"), &["NULL"]);

        // 300 WADs, each with its own texture, and a map that uses all of them.
        let mut used: Vec<String> = Vec::new();
        for i in 0..300 {
            let tex = format!("TEX{i:03}");
            fake_wad(&wads.join(format!("pack{i:03}.wad")), &[&tex]);
            used.push(tex);
        }

        let map = root.join("m.map");
        let good = wads.join("pack000.wad").display().to_string();
        std::fs::write(&map, fake_map(&good, &["TEX000", "NULL"])).unwrap();

        let mut opts = Options::default();
        opts.map_path = map.display().to_string();
        opts.tools_dir = tools.display().to_string();
        opts.wad_dir = wads.display().to_string();

        // Everything the map uses is already covered: no extras at all.
        let (tx, _rx) = channel();
        let found = resolve_wads(&opts, &map, &tx);
        assert_eq!(found.len(), 2, "{found:?}");

        // Now it needs one texture per WAD, far more than CSG can hold.
        let refs: Vec<&str> = used.iter().map(|s| s.as_str()).collect();
        std::fs::write(&map, fake_map("D:/nope/gone.wad", &refs)).unwrap();
        let (tx, _rx) = channel();
        let found = resolve_wads(&opts, &map, &tx);
        assert!(found.len() <= MAX_WADS, "{}", found.len());
        assert!(found.len() > 100, "{}", found.len());
        // 300 WADs, one texture each, all used: everything fits since the fork
        // raised MAX_WADPATHS.
        assert_eq!(found.len(), 301, "{}", found.len());

        std::fs::remove_dir_all(&root).ok();
    }
}

//! Saved projects: a name, the map, the folders and every compile option.
//!
//! Kept in `resdhlt-projects.json` next to the executable, separate from
//! `resdhlt-gui.json`: the working options file keeps its old shape, so an
//! install from before projects existed still loads exactly as it did.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::options::Options;

pub const MAX_NAME: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub name: String,
    pub opts: Options,
    /// Unix seconds, for the "last opened" column.
    #[serde(default)]
    pub last_used: u64,
}

impl Project {
    pub fn new(name: &str, opts: Options) -> Self {
        Self {
            name: name.to_string(),
            opts,
            last_used: now_secs(),
        }
    }

    /// The folder a project's files live in: where the .bsp ends up when there
    /// is an output folder, otherwise wherever the .map sits.
    pub fn folder(&self) -> Option<PathBuf> {
        if let Some(base) = self.opts.output_base() {
            if base.is_dir() {
                return Some(base);
            }
        }
        Path::new(self.opts.map_path.trim())
            .parent()
            .filter(|p| p.is_dir())
            .map(|p| p.to_path_buf())
    }

    pub fn map_name(&self) -> String {
        Path::new(self.opts.map_path.trim())
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("(sin mapa)")
            .to_string()
    }
}

/// The application's own saved state: the projects, and the few global
/// preferences that are not compile options.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    #[serde(default)]
    pub projects: Vec<Project>,
    /// Name of the project that was open when the window closed, so the next
    /// launch comes back to it.
    #[serde(default)]
    pub active: Option<String>,
    /// Ask GitHub about new releases. Global, not per project.
    #[serde(default = "yes")]
    pub check_updates: bool,
    /// Unix seconds of the last check, so opening the GUI ten times in an hour
    /// is one request, not ten.
    #[serde(default)]
    pub last_update_check: u64,
}

fn yes() -> bool {
    true
}

impl Default for Library {
    fn default() -> Self {
        Self {
            projects: Vec::new(),
            active: None,
            check_updates: true,
            last_update_check: 0,
        }
    }
}

impl Library {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.projects
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// Whether this name is free, ignoring the project at `skip` (so renaming
    /// something to its own name is allowed).
    pub fn name_taken(&self, name: &str, skip: Option<usize>) -> bool {
        self.projects.iter().enumerate().any(|(i, p)| {
            Some(i) != skip && p.name.eq_ignore_ascii_case(name.trim())
        })
    }

    /// "zm_hola", "zm_hola (2)", "zm_hola (3)"...
    pub fn unique_name(&self, base: &str) -> String {
        let base = sanitize_name(base);
        if !self.name_taken(&base, None) {
            return base;
        }
        for n in 2..1000 {
            let candidate = format!("{base} ({n})");
            if !self.name_taken(&candidate, None) {
                return candidate;
            }
        }
        base
    }

    pub fn sort_by_name(&mut self) {
        self.projects
            .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    }
}

/// Names go in a file, so they must survive being a JSON string and being read
/// by a human. Anything exotic is dropped rather than rejected with an error.
pub fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return "sin nombre".to_string();
    }
    cleaned.chars().take(MAX_NAME).collect()
}

/// A sensible project name from a map path: "C:/maps/zm_hola.map" -> "zm_hola".
pub fn name_from_map(map_path: &str) -> String {
    Path::new(map_path.trim())
        .file_stem()
        .and_then(|n| n.to_str())
        .map(sanitize_name)
        .unwrap_or_else(|| "nuevo proyecto".to_string())
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------- folder view

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// The source map.
    Map,
    /// The compiled map.
    Bsp,
    /// Portal file VIS reads.
    Portal,
    /// Logs and error files.
    Log,
    /// Scratch output that can be deleted without losing anything: .p0-.p3,
    /// .lin, .pts, .wa_, .ext.
    Intermediate,
    Wad,
    Other,
}

impl FileKind {
    pub fn label(self) -> &'static str {
        match self {
            FileKind::Map => "mapa",
            FileKind::Bsp => "compilado",
            FileKind::Portal => "portales",
            FileKind::Log => "log",
            FileKind::Intermediate => "intermedio",
            FileKind::Wad => "wad",
            FileKind::Other => "",
        }
    }

    fn of(name: &str) -> Self {
        let lower = name.to_ascii_lowercase();
        let ext = lower.rsplit('.').next().unwrap_or("");
        match ext {
            "map" => FileKind::Map,
            "bsp" => FileKind::Bsp,
            "prt" => FileKind::Portal,
            "log" | "err" => FileKind::Log,
            "p0" | "p1" | "p2" | "p3" | "lin" | "pts" | "wa_" | "ext" | "max" => {
                FileKind::Intermediate
            }
            "wad" => FileKind::Wad,
            _ => FileKind::Other,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub kind: FileKind,
}

/// Files in a project's folder, newest first, including one level of
/// subfolders so the scratch folder's contents are visible too - they are
/// listed as "intermedios/zm_hola.p0". Deeper than that is somebody else's
/// business; this is a view of what a compile produced, not a file manager.
pub fn scan_folder(dir: &Path) -> Vec<FileEntry> {
    let mut out: Vec<FileEntry> = Vec::new();
    collect_into(dir, "", &mut out);
    for sub in subdirs(dir) {
        let label = sub
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        collect_into(&sub, &format!("{label}/"), &mut out);
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.name.cmp(&b.name)));
    out
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.truncate(8);
    dirs
}

fn collect_into(dir: &Path, prefix: &str, out: &mut Vec<FileEntry>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        if out.len() >= 500 {
            return;
        }
        let path = e.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let meta = e.metadata().ok();
        out.push(FileEntry {
            name: format!("{prefix}{name}"),
            kind: FileKind::of(name),
            size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            modified: meta.and_then(|m| m.modified().ok()),
            path,
        });
    }
}

pub fn fmt_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// "hace 3 min", "hace 2 h", "hace 5 d".
pub fn fmt_age(t: SystemTime) -> String {
    let Ok(d) = SystemTime::now().duration_since(t) else {
        return "recién".to_string();
    };
    let s = d.as_secs();
    if s < 60 {
        "recién".to_string()
    } else if s < 3600 {
        format!("hace {} min", s / 60)
    } else if s < 86400 {
        format!("hace {} h", s / 3600)
    } else {
        format!("hace {} d", s / 86400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_unique_and_printable() {
        let mut lib = Library::default();
        lib.projects.push(Project::new("zm_hola", Options::default()));
        assert_eq!(lib.unique_name("zm_hola"), "zm_hola (2)");
        lib.projects
            .push(Project::new("zm_hola (2)", Options::default()));
        assert_eq!(lib.unique_name("zm_hola"), "zm_hola (3)");

        // Case-insensitive: two projects differing only in case would be a trap.
        assert!(lib.name_taken("ZM_HOLA", None));
        assert!(!lib.name_taken("zm_hola", Some(0)));

        assert_eq!(sanitize_name("  zm_\"raro\"\n  "), "zm_raro");
        assert_eq!(sanitize_name("   "), "sin nombre");
        assert_eq!(sanitize_name(&"x".repeat(200)).chars().count(), MAX_NAME);
    }

    #[test]
    fn derives_the_name_and_kind_from_the_map() {
        assert_eq!(name_from_map(r"C:\maps\zm_hola.map"), "zm_hola");
        assert_eq!(FileKind::of("zm_hola.bsp"), FileKind::Bsp);
        assert_eq!(FileKind::of("zm_hola.p2"), FileKind::Intermediate);
        assert_eq!(FileKind::of("ZM_HOLA.LOG"), FileKind::Log);
    }
}
